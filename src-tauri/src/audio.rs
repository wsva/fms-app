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

/// Output sample rate produced by [`decode_to_pcm`] (it always resamples to this).
const DECODED_SAMPLE_RATE: u32 = 16_000;

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
/// (`WaveformCanvas`, `adjust::load_waveform`) divide by 128 and derive
/// `ms_per_pixel = samples_per_pixel / sample_rate * 1000`.
pub fn generate_waveform(path: &Path, pixels_per_second: u32) -> Result<WaveformPeaks, String> {
    log::debug!("Generating waveform for: {} ({} pps)", path.display(), pixels_per_second);
    let pps = pixels_per_second.max(1);
    let samples = decode_to_pcm(path)?;

    let samples_per_pixel = (DECODED_SAMPLE_RATE / pps).max(1) as usize;

    let mut data: Vec<i8> = Vec::with_capacity((samples.len() / samples_per_pixel + 1) * 2);
    for chunk in samples.chunks(samples_per_pixel) {
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

/// Scale a normalised sample (-1.0..=1.0) into audiowaveform's signed 8-bit range.
fn scale_to_i8(sample: f32) -> i8 {
    let v = (sample.clamp(-1.0, 1.0) * 128.0).round();
    v.clamp(-128.0, 127.0) as i8
}
