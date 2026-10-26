//! Energy voice-activity detection over 16 kHz mono PCM.
//!
//! Deliberately not Silero: a read-aloud recording is one speaker, one microphone
//! and a handful of seconds, so a frame-RMS threshold with an adaptive noise
//! floor is enough — and it costs no model download, no extra `ort` session and
//! no per-platform file handling. It is also *pure*, so the whole detector is
//! unit-testable with synthetic signals.
//!
//! Why this exists at all: ASR word timings are the only timing evidence the
//! stack provides, and they are relative to the *transcript*, not to silence.
//! A recording with a two-second gap between two words produces two word segments
//! whose `end`/`start` differ — but so does a recording with an unnaturally slow
//! reader. Only an independent look at the waveform can say "the room was
//! quiet here", which is what a pause is.

/// Sample rate every recording is decoded to before anything here sees it.
pub(crate) const SAMPLE_RATE: u32 = 16_000;

/// One continuous stretch of speech, in seconds from the start of the recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segment {
    pub start: f64,
    pub end: f64,
}

/// Every threshold the detector uses, in one place so none hides in a body.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VadConfig {
    /// Frame length in samples. 20 ms is the usual speech-analysis floor and
    /// keeps a 10-minute recording at 30 000 frames.
    pub frame_samples: usize,
    /// Fraction of frame RMS values treated as the noise floor.
    pub floor_percentile: f64,
    /// Speech threshold as a multiple of the noise floor.
    pub floor_multiplier: f32,
    /// Ceiling on the threshold, as a fraction of the loud percentile. Without
    /// it a recording that is *only* speech has no quiet frames, so the "noise
    /// floor" is speech level, `floor_multiplier` lifts the threshold above
    /// every frame and the whole recording reads as silence.
    pub max_threshold_ratio: f32,
    /// Hard floor, so a digitally silent recording (floor = 0) detects nothing
    /// instead of treating dither as speech.
    pub absolute_floor: f32,
    /// Frames of disagreement required to flip the speech/silence verdict.
    pub hysteresis_frames: usize,
    /// Shorter bursts are not speech (a door click, a mouse button).
    pub min_speech_secs: f64,
    /// Gaps shorter than this are inside a word, not between them.
    pub min_silence_secs: f64,
    /// Padding kept on each side of a segment, so a soft onset is not cut.
    pub pad_secs: f64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            frame_samples: 320, // 20 ms at 16 kHz
            floor_percentile: 0.10,
            floor_multiplier: 3.0,
            max_threshold_ratio: 0.5,
            absolute_floor: 0.004,
            hysteresis_frames: 2, // 40 ms
            min_speech_secs: 0.15,
            min_silence_secs: 0.12,
            pad_secs: 0.04,
        }
    }
}

pub(crate) fn frame_rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f32 = frame.iter().map(|s| s * s).sum();
    (sum / frame.len() as f32).sqrt()
}

/// The `p`-quantile of `values` (linear interpolation between order statistics).
fn percentile(values: &[f32], p: f64) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f32> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pos = (sorted.len() - 1) as f64 * p.clamp(0.0, 1.0);
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = (pos - lo as f64) as f32;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

/// A run of equal flags: `(value, first_frame, frame_count)`.
fn runs(flags: &[bool]) -> Vec<(bool, usize, usize)> {
    let mut out: Vec<(bool, usize, usize)> = Vec::new();
    for (i, &f) in flags.iter().enumerate() {
        match out.last_mut() {
            Some((v, _, len)) if *v == f => *len += 1,
            _ => out.push((f, i, 1)),
        }
    }
    out
}

/// Frames-per-second the config implies.
fn frame_rate(cfg: &VadConfig) -> f64 {
    SAMPLE_RATE as f64 / cfg.frame_samples.max(1) as f64
}

/// The speech/silence verdict per frame of an already-computed loudness track.
///
/// `loudness` is one level per frame in any consistent unit — frame RMS for a
/// decoded recording, the peak magnitude for a cached envelope — and
/// `frame_secs` says how much time one value covers, so the short-burst and
/// bridging rules keep their meaning whatever the frame is. Only the frame size
/// may differ between callers; the threshold, the ceiling and the hysteresis are
/// the same ones [`detect`] applies, so a caller that has a loudness track and
/// not the samples gets the same answer as one that has the samples.
pub(crate) fn speech_frames(loudness: &[f32], frame_secs: f64, cfg: &VadConfig) -> Vec<bool> {
    if loudness.is_empty() || !(frame_secs > 0.0) {
        return Vec::new();
    }

    let floor = percentile(loudness, cfg.floor_percentile);
    let loud = percentile(loudness, 0.90);
    // Two-sided: above the noise, but never above half the loud level — a
    // speech-only recording has no silence to estimate a floor from.
    let threshold = (floor * cfg.floor_multiplier)
        .min(loud * cfg.max_threshold_ratio)
        .max(cfg.absolute_floor);
    let mut flags: Vec<bool> = loudness.iter().map(|level| *level > threshold).collect();

    let rate = 1.0 / frame_secs;
    let min_speech_frames = ((cfg.min_speech_secs * rate).round() as usize).max(1);

    // Drop bursts too short to be a word, then bridge gaps too short to be a
    // breath: both are the same hysteresis in two directions.
    for (value, start, len) in runs(&flags) {
        if value && len < min_speech_frames {
            flags[start..start + len].fill(false);
        }
    }
    let hysteresis = cfg.hysteresis_frames.max(1);
    for (value, start, len) in runs(&flags) {
        if !value && len < hysteresis && start > 0 && start + len < flags.len() {
            flags[start..start + len].fill(true);
        }
    }

    flags
}

/// Detect speech segments in a 16 kHz mono recording.
///
/// The pipeline is: frame RMS → [`speech_frames`] → runs to segments → merge
/// close segments → pad. Every threshold is expressed in the config, so a caller
/// tuning one of them cannot accidentally change another.
pub(crate) fn detect(samples: &[f32], cfg: &VadConfig) -> Vec<Segment> {
    let frame = cfg.frame_samples.max(1);
    if samples.is_empty() || samples.len() < frame {
        return Vec::new();
    }

    let rate = frame_rate(cfg);
    let rms: Vec<f32> = samples.chunks(frame).map(frame_rms).collect();
    let flags = speech_frames(&rms, 1.0 / rate, cfg);

    // Frames → seconds.
    let mut spans: Vec<Segment> = runs(&flags)
        .into_iter()
        .filter(|(value, _, _)| *value)
        .map(|(_, start, len)| Segment {
            start: start as f64 / rate,
            end: (start + len) as f64 / rate,
        })
        .collect();
    if spans.is_empty() {
        return spans;
    }

    // Merge segments separated by less than min_silence: a stop consonant gap
    // is not a pause, and neither is the frame quantisation at a segment edge.
    let mut merged: Vec<Segment> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        match merged.last_mut() {
            Some(last) if span.start - last.end < cfg.min_silence_secs => last.end = span.end,
            _ => merged.push(span),
        }
    }

    // Pad, clamp into the recording, and re-merge in case padding closed a gap.
    let total = samples.len() as f64 / SAMPLE_RATE as f64;
    let mut padded: Vec<Segment> = Vec::with_capacity(merged.len());
    for span in merged {
        let p = Segment {
            start: (span.start - cfg.pad_secs).max(0.0),
            end: (span.end + cfg.pad_secs).min(total),
        };
        match padded.last_mut() {
            Some(last) if p.start <= last.end => last.end = last.end.max(p.end),
            _ => padded.push(p),
        }
    }
    padded
}


#[cfg(test)]
mod tests {
    use super::*;

    /// `secs` of a 440 Hz tone at the given peak amplitude.
    fn tone(secs: f64, amp: f32) -> Vec<f32> {
        let n = (secs * SAMPLE_RATE as f64) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                (2.0 * std::f32::consts::PI * 440.0 * t).sin() * amp
            })
            .collect()
    }

    fn silence(secs: f64) -> Vec<f32> {
        vec![0.0; (secs * SAMPLE_RATE as f64) as usize]
    }

    #[test]
    fn digital_silence_yields_no_segments() {
        assert!(detect(&silence(2.0), &VadConfig::default()).is_empty());
        // Empty and sub-frame input are the same answer, not a panic.
        assert!(detect(&[], &VadConfig::default()).is_empty());
        assert!(detect(&vec![0.0; 100], &VadConfig::default()).is_empty());
    }

    #[test]
    fn one_tone_burst_is_one_segment() {
        let segments = detect(&tone(0.6, 0.3), &VadConfig::default());
        assert_eq!(segments.len(), 1);
        assert!(segments[0].start <= 0.05, "start: {}", segments[0].start);
        assert!(segments[0].end >= 0.55, "end: {}", segments[0].end);
    }

    #[test]
    fn tone_gap_tone_is_two_segments() {
        let mut samples = tone(0.5, 0.3);
        samples.extend(silence(0.5));
        samples.extend(tone(0.5, 0.3));
        let segments = detect(&samples, &VadConfig::default());
        assert_eq!(segments.len(), 2, "segments: {:?}", segments);
        assert!(segments[0].end <= 0.55);
        assert!(segments[1].start >= 0.95);
    }

    #[test]
    fn a_click_shorter_than_min_speech_is_not_a_segment() {
        let mut samples = silence(0.2);
        samples.extend(tone(0.06, 0.5)); // 60 ms, below the 150 ms floor
        samples.extend(silence(0.2));
        assert!(detect(&samples, &VadConfig::default()).is_empty());
    }

    #[test]
    fn a_stop_consonant_gap_stays_inside_one_segment() {
        let mut samples = tone(0.4, 0.3);
        samples.extend(silence(0.06)); // 60 ms < min_silence (120 ms)
        samples.extend(tone(0.4, 0.3));
        let segments = detect(&samples, &VadConfig::default());
        assert_eq!(segments.len(), 1, "a 60 ms gap is inside a word: {:?}", segments);
    }
}
