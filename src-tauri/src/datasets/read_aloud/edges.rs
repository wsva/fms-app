//! Where a clip starts and ends.
//!
//! A guess knows when the first and the last *word* were read and nothing about
//! the room around them: the model's word boundaries are coarse, and the audio
//! between two passages is ambiguous — it is silent, so a cut anywhere inside it
//! sounds equally right.
//!
//! This module answers the question with the waveform instead of the transcript.
//! It finds the pause that meets an edge ([`pause_beside`]) and puts the cut in
//! the **middle of that pause**, so two neighbouring clips share it rather than
//! one of them taking all of it because the model's timestamp happened to land
//! at one end. The share has a ceiling, because the middle of a long pause is a
//! long silence to listen to: a one-line mail behind a two-second pause would
//! otherwise open with a second of nothing, and the point of the middle is
//! fairness, not dead air.
//!
//! Everything here is pure over a loudness track — one level per 20 ms frame —
//! so the bulk guess, which only has a cached peak envelope, and the single-file
//! guess, which has the samples, place their edges through the same code and
//! agree with each other.

use super::vad;

/// Frame length clips are placed at: the same 20 ms the cached peak envelope
/// uses, so both guesses look at the audio with the same resolution.
pub(crate) const EDGE_FRAME_SECS: f64 = 0.02;

/// How far from an edge a pause is looked for. A model's word boundary can land
/// inside the first or last consonant, so the pause is not always exactly where
/// the timestamp says; half a syllable of tolerance is enough to find it.
const SNAP_WINDOW_SECS: f64 = 0.15;

/// How much of a pause one clip takes at one edge. Half is the whole point: the
/// pause belongs to both neighbours, so the cut goes in the middle of it.
const PAUSE_SHARE: f64 = 0.5;

/// The longest pause share a head may take. Measured on a narrated book, the
/// pause in front of a passage is 0.44 s at the median and 1.1 s at the ninth
/// decile, so an uncapped middle would open most clips with half a second of
/// room and the slow ones with well over one — dead air, not context.
const MAX_LEAD_SECS: f64 = 0.5;

/// The same for a tail, a little longer because a decay into silence reads as
/// the end of the phrase and a hard cut does not.
const MAX_TAIL_SECS: f64 = 0.6;

/// Which way an edge faces — into the passage it belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Side {
    /// The start of a clip: the passage is to the right.
    Head,
    /// The end of a clip: the passage is to the left.
    Tail,
}

/// Per-frame RMS of decoded 16 kHz mono audio.
pub(crate) fn loudness_from_samples(samples: &[f32], frame_secs: f64) -> Vec<f32> {
    let frame = ((frame_secs * vad::SAMPLE_RATE as f64).round() as usize).max(1);
    samples.chunks(frame).map(vad::frame_rms).collect()
}

/// Per-frame loudness of a cached peak envelope (interleaved min/max `i8`
/// pairs), scaled so that full scale is `1.0` like [`loudness_from_samples`].
pub(crate) fn loudness_from_envelope(peaks: &[i8]) -> Vec<f32> {
    peaks
        .chunks_exact(2)
        .map(|pair| {
            let low = (pair[0] as i32).unsigned_abs().min(127);
            let high = (pair[1] as i32).unsigned_abs().min(127);
            low.max(high) as f32 / 127.0
        })
        .collect()
}

/// The pause that meets one edge: `(start, end)` in seconds of the run of quiet
/// frames the edge sits in, or `None` when there is no pause within
/// [`SNAP_WINDOW_SECS`] or the "pause" is not one.
///
/// A run only counts when speech is on the far side of it, away from the edge.
/// Without that check a track that reads as silence everywhere — a music bed, a
/// broken envelope, a file of room tone — would offer its whole length as a
/// pause and the cut would land in the middle of the recording.
pub(crate) fn pause_beside(
    loudness: &[f32],
    frame_secs: f64,
    edge: f64,
    side: Side,
) -> Option<(f64, f64)> {
    if loudness.len() < 2 || !(frame_secs > 0.0) {
        return None;
    }
    let cfg = vad::VadConfig::default();
    let speech = vad::speech_frames(loudness, frame_secs, &cfg);
    if speech.len() < 2 {
        return None;
    }

    let centre = (edge / frame_secs).round() as isize;
    let reach = (SNAP_WINDOW_SECS / frame_secs).round() as isize;
    // The edge faces the passage, so its own side of the timestamp is searched
    // first: the pause is in front of it, not behind it.
    let mut quiet: Option<usize> = None;
    'search: for offset in 0..=reach {
        let candidates = match side {
            Side::Head => [centre - offset, centre + offset],
            Side::Tail => [centre + offset, centre - offset],
        };
        for candidate in candidates {
            if candidate >= 0 && (candidate as usize) < speech.len() && !speech[candidate as usize] {
                quiet = Some(candidate as usize);
                break 'search;
            }
        }
    }
    let frame = quiet?;

    let mut start = frame;
    while start > 0 && !speech[start - 1] {
        start -= 1;
    }
    let mut end = frame + 1;
    while end < speech.len() && !speech[end] {
        end += 1;
    }

    let anchored = match side {
        Side::Head => end < speech.len() && speech[end],
        Side::Tail => start > 0 && speech[start - 1],
    };
    if !anchored {
        return None;
    }
    if ((end - start) as f64 * frame_secs) < cfg.min_silence_secs {
        return None;
    }
    Some((start as f64 * frame_secs, end as f64 * frame_secs))
}

/// One edge, in seconds: the middle of the pause it meets, but never more than
/// [`MAX_LEAD_SECS`]/[`MAX_TAIL_SECS`] away from the word — and clamped to
/// `[low, high]`.
///
/// With no pause (speech runs into the word) `pad` is taken into the audio, as
/// before; the caller's bounds are what keeps that from eating its neighbour.
pub(crate) fn place_edge(
    loudness: &[f32],
    frame_secs: f64,
    edge: f64,
    pad: f64,
    side: Side,
    low: f64,
    high: f64,
) -> f64 {
    let fallback = match side {
        Side::Head => edge - pad,
        Side::Tail => edge + pad,
    };
    let Some((pause_start, pause_end)) = pause_beside(loudness, frame_secs, edge, side) else {
        return fallback.clamp(low, high.max(low));
    };
    let ceiling = match side {
        Side::Head => MAX_LEAD_SECS,
        Side::Tail => MAX_TAIL_SECS,
    };
    let share = ((pause_end - pause_start) * PAUSE_SHARE).min(ceiling);
    let seconds = match side {
        Side::Head => pause_end - share,
        Side::Tail => pause_start + share,
    };
    seconds.clamp(low, high.max(low))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEECH: f32 = 0.4;
    const QUIET: f32 = 0.001;

    /// A track of `(level, seconds)` blocks at [`EDGE_FRAME_SECS`].
    fn track(blocks: &[(f32, f64)]) -> Vec<f32> {
        let mut out = Vec::new();
        for (level, secs) in blocks {
            let frames = (secs / EDGE_FRAME_SECS).round() as usize;
            out.resize(out.len() + frames, *level);
        }
        out
    }

    #[test]
    fn a_pause_shorter_than_two_pads_is_split_in_the_middle() {
        // 0.3 s of quiet before the word: half of it is 0.15, which is under the
        // 0.2 s pad, so the cut lands in the middle of the pause rather than
        // 0.2 s before the word — that would reach into the previous passage.
        let loudness = track(&[(SPEECH, 1.0), (QUIET, 0.3), (SPEECH, 1.0)]);
        let cut = place_edge(&loudness, EDGE_FRAME_SECS, 1.3, 0.2, Side::Head, 0.0, 10.0);
        assert!((cut - 1.15).abs() < 1e-6, "cut: {cut}");
    }

    #[test]
    fn a_long_pause_is_shared_up_to_the_ceiling() {
        // Two seconds of quiet behind a short mail: half of it would be a second
        // of dead air, so the share stops at half a second.
        let loudness = track(&[(SPEECH, 1.0), (QUIET, 2.0), (SPEECH, 1.0)]);
        let cut = place_edge(&loudness, EDGE_FRAME_SECS, 3.0, 0.2, Side::Head, 0.0, 10.0);
        assert!((cut - 2.5).abs() < 1e-6, "cut: {cut}");
    }

    #[test]
    fn the_tail_takes_its_share_after_the_word() {
        let loudness = track(&[(SPEECH, 1.0), (QUIET, 0.4), (SPEECH, 1.0)]);
        let cut = place_edge(&loudness, EDGE_FRAME_SECS, 1.0, 0.3, Side::Tail, 0.0, 10.0);
        assert!((cut - 1.2).abs() < 1e-6, "cut: {cut}");
    }

    #[test]
    fn speech_running_into_the_word_falls_back_to_the_pad() {
        let loudness = track(&[(SPEECH, 1.0), (QUIET, 0.6), (SPEECH, 1.0)]);
        // The edge is halfway through the pause, further than the window allows.
        let cut = place_edge(&loudness, EDGE_FRAME_SECS, 1.9, 0.2, Side::Head, 0.0, 10.0);
        assert!((cut - 1.7).abs() < 1e-6, "cut: {cut}");

        // No pause anywhere near: the pad goes into the audio, as it always did.
        let flat = track(&[(SPEECH, 3.0)]);
        let cut = place_edge(&flat, EDGE_FRAME_SECS, 1.5, 0.2, Side::Head, 0.0, 10.0);
        assert!((cut - 1.3).abs() < 1e-6, "cut: {cut}");
    }

    #[test]
    fn a_track_of_nothing_but_quiet_is_not_a_pause() {
        // A music bed or a broken envelope reads as silence end to end. Half of
        // that "pause" would put the cut in the middle of the recording.
        let loudness = track(&[(QUIET, 20.0)]);
        let cut = place_edge(&loudness, EDGE_FRAME_SECS, 8.0, 0.2, Side::Head, 0.0, 20.0);
        assert!((cut - 7.8).abs() < 1e-6, "cut: {cut}");
    }

    #[test]
    fn the_bounds_win_over_the_pause() {
        let loudness = track(&[(SPEECH, 1.0), (QUIET, 1.0), (SPEECH, 1.0)]);
        // The previous clip ends 30 ms before the word: no lead-in is available.
        let cut = place_edge(&loudness, EDGE_FRAME_SECS, 2.0, 0.2, Side::Head, 1.97, 10.0);
        assert!((cut - 1.97).abs() < 1e-6, "cut: {cut}");
    }

    #[test]
    fn an_envelope_track_is_read_at_the_same_scale_as_samples() {
        // Full-scale pair, a quiet pair, and a half-scale pair.
        let peaks = [127i8, -127, 1, -1, 64, -64, 0, 0];
        let loudness = loudness_from_envelope(&peaks);
        assert_eq!(loudness.len(), 4);
        assert!((loudness[0] - 1.0).abs() < 1e-6);
        assert!(loudness[1] < 0.02);
        assert!((loudness[2] - 64.0 / 127.0).abs() < 1e-6);
        assert_eq!(loudness[3], 0.0);
    }

    #[test]
    fn an_envelope_finds_the_same_pause_as_the_samples() {
        let blocks = [(0.3f32, 0.5), (0.0, 0.4), (0.3, 0.5)];
        let mut samples = Vec::new();
        for (level, secs) in blocks {
            let n = (secs * vad::SAMPLE_RATE as f64) as usize;
            samples.resize(samples.len() + n, level);
        }
        let from_samples = loudness_from_samples(&samples, EDGE_FRAME_SECS);
        let cut = place_edge(&from_samples, EDGE_FRAME_SECS, 0.9, 0.3, Side::Head, 0.0, 10.0);
        assert!((cut - 0.7).abs() < 0.03, "cut: {cut}");
    }
}
