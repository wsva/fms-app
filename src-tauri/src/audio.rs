use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decode an audio file to 16 kHz mono f32 samples.
///
/// Backed by symphonia (built with the `all` feature), so it handles the common
/// audio containers/codecs (MP3, WAV, FLAC, OGG, AAC/MP4, Opus) and also extracts
/// the audio track from video containers (MP4/MOV/MKV/WebM).
pub fn decode_to_pcm(path: &Path) -> Result<Vec<f32>, String> {
    log::debug!("Decoding audio file: {}", path.display());
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    decode_stream(mss, hint, None)
}

/// Decode a file like [`decode_to_pcm`], refusing once it runs past
/// `max_seconds` of audio.
///
/// The bound is checked while packets are still arriving, so an over-long book
/// chapter is rejected after a moment instead of after several gigabytes of
/// f32 have been collected. Callers use it for bulk work where a single
/// enormous file would otherwise take the whole process down.
pub fn decode_to_pcm_capped(path: &Path, max_seconds: f64) -> Result<Vec<f32>, String> {
    log::debug!(
        "Decoding audio file (max {} s): {}",
        max_seconds.round(),
        path.display()
    );
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    decode_stream(mss, hint, Some(max_seconds))
}

/// Decode an in-memory container to 16 kHz mono f32 — the same conversion
/// [`decode_to_pcm`] performs, for audio that arrives as bytes rather than as a
/// file. No extension hint is available; symphonia probes the header instead.
///
/// Used by the read-aloud standard-audio import path, where an Android document
/// picker returns a `content://` URI that is not a filesystem path.
pub fn decode_bytes_to_pcm(data: &[u8]) -> Result<Vec<f32>, String> {
    log::debug!("Decoding {} in-memory bytes", data.len());
    let source = std::io::Cursor::new(data.to_vec());
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    decode_stream(mss, Hint::new(), None)
}

/// The shared decoder behind [`decode_to_pcm`] and [`decode_bytes_to_pcm`]:
/// probe, decode every packet of the first audio track, mix down to mono, then
/// resample to 16 kHz.
///
/// `max_seconds` bounds how much audio may be collected; it is enforced per
/// packet so the refusal costs one packet's worth of memory, not one file's.
fn decode_stream(
    mss: MediaSourceStream,
    hint: Hint,
    max_seconds: Option<f64>,
) -> Result<Vec<f32>, String> {
    let fmt_opts = FormatOptions::default();
    let meta_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .map_err(|e| format!("Unsupported format: {}", e))?;

    let mut format = probed.format;

    // Pick the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| "No audio track found".to_string())?;

    let track_id = track.id;
    let dec_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &dec_opts)
        .map_err(|e| format!("Failed to create decoder: {}", e))?;

    let mut all_samples: Vec<f32> = Vec::new();
    let mut sample_rate: u32 = 0;
    let mut channels: u16 = 0;

    // Decode all packets
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(format!("Decode error: {}", e)),
        };

        // Skip packets from other tracks
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(format!("Decode error: {}", e)),
        };

        let spec = *decoded.spec();
        sample_rate = spec.rate;
        channels = spec.channels.count() as u16;

        let duration = decoded.capacity() as u64;
        let mut sb = SampleBuffer::<f32>::new(duration, spec);
        sb.copy_interleaved_ref(decoded);
        all_samples.extend_from_slice(sb.samples());

        if let Some(max) = max_seconds {
            if sample_rate > 0 && channels > 0 {
                let seconds = all_samples.len() as f64 / channels as f64 / sample_rate as f64;
                if seconds > max {
                    return Err(format!(
                        "Recording is longer than the {} minute limit — split it into chapter files.",
                        (max / 60.0).round().max(1.0)
                    ));
                }
            }
        }
    }

    if sample_rate == 0 {
        return Err("No audio data decoded".into());
    }
    log::debug!("Audio decoded: {} samples at {}Hz, {} channels", all_samples.len(), sample_rate, channels);

    // Mix down to mono if needed
    let mono = if channels > 1 {
        let ch = channels as usize;
        all_samples
            .chunks_exact(ch)
            .map(|frame| frame.iter().sum::<f32>() / ch as f32)
            .collect()
    } else {
        all_samples
    };

    // Resample to 16 kHz if needed
    if sample_rate == 16000 {
        Ok(mono)
    } else {
        log::debug!("Resampling from {}Hz to 16000Hz", sample_rate);
        Ok(resample_linear(&mono, sample_rate, 16000))
    }
}

/// Simple linear interpolation resampler.
fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = (samples.len() as f64 * ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;

        let s0 = samples[idx.min(samples.len() - 1)];
        let s1 = samples[(idx + 1).min(samples.len() - 1)];
        out.push(s0 + frac * (s1 - s0));
    }

    out
}

/// Interleaved f32 PCM at the source's native sample rate and channel count.
pub struct PcmBuffer {
    /// Interleaved samples: `[L0, R0, L1, R1, ...]` for stereo.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Decode an audio file to interleaved f32 samples at its NATIVE sample rate and
/// channel count (no mono down-mix, no resampling). Contrast with
/// [`decode_to_pcm`], which always yields 16 kHz mono for STT/waveform use.
pub fn decode_pcm_native(path: &Path) -> Result<PcmBuffer, String> {
    log::debug!("Decoding audio (native): {}", path.display());
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let fmt_opts = FormatOptions::default();
    let meta_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .map_err(|e| format!("Unsupported format: {}", e))?;

    let mut format = probed.format;

    // Pick the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| "No audio track found".to_string())?;

    let track_id = track.id;
    let dec_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &dec_opts)
        .map_err(|e| format!("Failed to create decoder: {}", e))?;

    let mut all_samples: Vec<f32> = Vec::new();
    let mut sample_rate: u32 = 0;
    let mut channels: u16 = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(format!("Decode error: {}", e)),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(format!("Decode error: {}", e)),
        };

        let spec = *decoded.spec();
        sample_rate = spec.rate;
        channels = spec.channels.count() as u16;

        let duration = decoded.capacity() as u64;
        let mut sb = SampleBuffer::<f32>::new(duration, spec);
        sb.copy_interleaved_ref(decoded);
        all_samples.extend_from_slice(sb.samples());
    }

    if sample_rate == 0 {
        return Err("No audio data decoded".into());
    }
    log::debug!(
        "Audio decoded (native): {} frames x {}ch at {}Hz",
        all_samples.len() / channels.max(1) as usize,
        channels,
        sample_rate
    );

    Ok(PcmBuffer {
        samples: all_samples,
        sample_rate,
        channels,
    })
}

/// Decode `path` and return only the interleaved samples spanning
/// `[start_ms, end_ms]`, at the source's native rate/channels.
///
/// NOTE: this decodes the whole file then slices. For very long sources a
/// seek-based trim (`format.seek(SeekMode::Accurate, ..)`) would be cheaper;
/// that optimization is a documented follow-up.
pub fn decode_range_native(path: &Path, start_ms: i64, end_ms: i64) -> Result<PcmBuffer, String> {
    let buf = decode_pcm_native(path)?;
    let ch = buf.channels.max(1) as usize;
    let rate = buf.sample_rate as i64;

    let start_ms = start_ms.max(0);
    let end_ms = end_ms.max(start_ms);

    let total_frames = buf.samples.len() / ch;
    let start_frame = (((start_ms * rate) / 1000) as usize).min(total_frames);
    let end_frame = (((end_ms * rate) / 1000) as usize)
        .min(total_frames)
        .max(start_frame);

    let slice = buf.samples[start_frame * ch..end_frame * ch].to_vec();
    log::debug!(
        "Trimmed range {}..{}ms -> frames {}..{} ({} samples)",
        start_ms,
        end_ms,
        start_frame,
        end_frame,
        slice.len()
    );

    Ok(PcmBuffer {
        samples: slice,
        sample_rate: buf.sample_rate,
        channels: buf.channels,
    })
}

/// Write interleaved f32 samples (-1.0..=1.0) as a canonical 16-bit PCM WAV file.
pub fn write_wav(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), String> {
    use std::io::Write;

    let ch = channels.max(1);
    let bits: u16 = 16;
    let byte_rate = sample_rate * ch as u32 * (bits as u32 / 8);
    let block_align = ch * (bits / 8);

    // Convert f32 -> i16 PCM (little-endian).
    let mut data: Vec<u8> = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        data.extend_from_slice(&v.to_le_bytes());
    }
    let data_len = data.len() as u32;

    let mut out: Vec<u8> = Vec::with_capacity(44 + data.len());
    // RIFF header
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    // fmt sub-chunk (PCM)
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    out.extend_from_slice(&ch.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    // data sub-chunk
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&data);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {}", e))?;
    }
    let mut file = std::fs::File::create(path)
        .map_err(|e| format!("Failed to create {}: {}", path.display(), e))?;
    file.write_all(&out)
        .map_err(|e| format!("Failed to write WAV: {}", e))?;
    log::debug!(
        "Wrote WAV {} ({} bytes, {}Hz {}ch)",
        path.display(),
        out.len(),
        sample_rate,
        ch
    );
    Ok(())
}

/// Output sample rate produced by [`decode_to_pcm`] (it always resamples to this).
/// Public so callers that reason about seconds (VAD, phoneme GOP) agree with it.
pub(crate) const DECODED_SAMPLE_RATE: u32 = 16_000;

/// Peak-envelope waveform in the audiowaveform v2 JSON layout: interleaved
/// `[min, max]` signed 8-bit pairs, one pair per pixel bucket. Produced entirely
/// in Rust via symphonia, replacing the external `audiowaveform` binary.
pub struct WaveformPeaks {
    pub version: u8,
    pub channels: u8,
    pub sample_rate: u32,
    pub samples_per_pixel: u32,
    pub bits: u8,
    /// Interleaved min/max pairs; `data.len() == 2 * pixel_count`.
    pub data: Vec<i8>,
}

/// Decode `path` and reduce it to a peak envelope at `pixels_per_second`.
///
/// The audio is decoded to 16 kHz mono f32 (see [`decode_to_pcm`]); every bucket
/// of `sample_rate / pixels_per_second` consecutive samples collapses to its
/// min/max, scaled into the signed 8-bit range used by audiowaveform. Consumers
/// (`WaveformCanvas`, `datasets::dictation::adjust::load_waveform`) divide by 128 and derive
/// `ms_per_pixel = samples_per_pixel / sample_rate * 1000`.
pub fn generate_waveform(path: &Path, pixels_per_second: u32) -> Result<WaveformPeaks, String> {
    log::debug!("Generating waveform for: {} ({} pps)", path.display(), pixels_per_second);
    let pps = pixels_per_second.max(1);
    let samples = decode_to_pcm(path)?;

    let samples_per_pixel = (DECODED_SAMPLE_RATE / pps).max(1) as usize;

    let data = envelope(&samples, samples_per_pixel);

    Ok(WaveformPeaks {
        version: 2,
        channels: 1,
        sample_rate: DECODED_SAMPLE_RATE,
        samples_per_pixel: samples_per_pixel as u32,
        bits: 8,
        data,
    })
}

// Note: scale_to_i8 helper below

/// Collapse `samples` into interleaved `[min, max]` signed 8-bit pairs, one pair
/// per `per_pair` samples — the `WaveformPeaks::data` layout, from samples a
/// caller already holds. Shared by [`generate_waveform`] and
/// [`peaks_from_samples`], so both scale identically.
fn envelope(samples: &[f32], per_pair: usize) -> Vec<i8> {
    let per_pair = per_pair.max(1);
    let mut data: Vec<i8> = Vec::with_capacity((samples.len() / per_pair + 1) * 2);
    for chunk in samples.chunks(per_pair) {
        let mut lo: f32 = 1.0;
        let mut hi: f32 = -1.0;
        for &s in chunk {
            if s < lo {
                lo = s;
            }
            if s > hi {
                hi = s;
            }
        }
        data.push(scale_to_i8(lo));
        data.push(scale_to_i8(hi));
    }
    data
}

/// Peak envelope of already-decoded 16 kHz mono samples, `columns` wide: returns
/// the interleaved `[min, max]` signed 8-bit pairs and how many samples each pair
/// covers. Consumers divide by 128; the caller derives
/// `seconds_per_pair = pairs_per_sample / DECODED_SAMPLE_RATE`.
pub fn peaks_from_samples(samples: &[f32], columns: usize) -> (Vec<i8>, usize) {
    let columns = columns.max(1);
    let per_pair = samples.len().div_ceil(columns).max(1);
    (envelope(samples, per_pair), per_pair)
}

/// Scale a normalised sample (-1.0..=1.0) into audiowaveform's signed 8-bit range.
fn scale_to_i8(sample: f32) -> i8 {
    let v = (sample.clamp(-1.0, 1.0) * 128.0).round();
    v.clamp(-128.0, 127.0) as i8
}

/// Decode an in-memory WAV buffer that is already 16 kHz mono PCM.
///
/// Contrast with [`decode_to_pcm`], which probes a file and resamples whatever
/// it finds. This one is for audio that arrives as bytes and whose format is
/// known by construction: the browser recorder writes 16 kHz mono WAV itself
/// before handing it over for scoring. Anything else is rejected rather than
/// resampled, so a scoring path can never silently run on audio the models were
/// not trained on.
pub fn decode_wav_pcm(data: &[u8]) -> Result<Vec<f32>, String> {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err("Invalid WAV file".into());
    }

    let mut pos = 12;
    let mut channels: u16 = 0;
    let mut sample_rate: u32 = 0;
    let mut bits_per_sample: u16 = 0;
    let mut pcm_data: Option<&[u8]> = None;

    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        pos += 8;

        if chunk_id == b"fmt " {
            if chunk_size < 16 || pos + 16 > data.len() {
                return Err("Invalid fmt chunk".into());
            }
            let format_tag = u16::from_le_bytes([data[pos], data[pos + 1]]);
            if format_tag != 1 {
                return Err("Only PCM WAV files are supported".into());
            }
            channels = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
            sample_rate = u32::from_le_bytes([
                data[pos + 4],
                data[pos + 5],
                data[pos + 6],
                data[pos + 7],
            ]);
            bits_per_sample = u16::from_le_bytes([data[pos + 14], data[pos + 15]]);
        } else if chunk_id == b"data" {
            let end = (pos + chunk_size).min(data.len());
            pcm_data = Some(&data[pos..end]);
        }

        pos += chunk_size;
    }

    if sample_rate != DECODED_SAMPLE_RATE {
        return Err(format!("Expected 16kHz audio, got {}Hz", sample_rate));
    }
    if channels != 1 {
        return Err(format!("Expected mono audio, got {} channels", channels));
    }

    let raw = pcm_data.ok_or("No data chunk in WAV file")?;

    match bits_per_sample {
        16 => Ok(raw
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect()),
        32 => Ok(raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()),
        other => Err(format!("Unsupported bit depth: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical 16 kHz mono WAV around the given samples.
    fn wav(samples: &[f32]) -> Vec<u8> {
        let dir = std::env::temp_dir().join(format!("fms-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        write_wav(&path, samples, 16_000, 1).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        bytes
    }

    #[test]
    fn a_written_wav_round_trips() {
        let original = vec![0.0f32, 0.5, -0.5, 0.25];
        let decoded = decode_wav_pcm(&wav(&original)).unwrap();
        assert_eq!(decoded.len(), original.len());
        for (a, b) in original.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn non_wav_input_is_rejected_rather_than_guessed() {
        assert!(decode_wav_pcm(b"not a wav file at all, just bytes").is_err());
        assert!(decode_wav_pcm(&[]).is_err());
    }
}
