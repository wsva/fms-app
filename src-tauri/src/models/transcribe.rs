//! Running a model: text, segments and word-level timestamps, chunking for
//! audio longer than a model can take, and the guard that lends the model out
//! of [`ModelState`] for the length of one call.
//!
//! Backend differences live in two places only — [`ActiveModel::transcribe_segments`]
//! for every backend and [`ActiveModel::transcribe_words`] for Parakeet — so
//! adding an engine means touching those two matches, not four call sites.

use std::path::Path;

use serde::Serialize;
#[cfg(feature = "desktop")]
use transcribe_cpp::RunOptions as CppRunOptions;
use transcribe_rs::onnx::parakeet::{ParakeetParams, TimestampGranularity};
use transcribe_rs::{SpeechModel, TranscribeOptions, TranscriptionResult, TranscriptionSegment};

use crate::models::engine::ensure_model_loaded;
use crate::models::state::{ActiveModel, ModelState};

// ---------------------------------------------------------------------------
// Constants for chunked transcription of long audio files
// ---------------------------------------------------------------------------

/// Maximum audio duration (seconds) per STT chunk. Parakeet v3 struggles with
/// audio longer than ~4-5 minutes, so we split long files into overlapping chunks.
const MAX_CHUNK_DURATION_SECS: f64 = 240.0; // 4 minutes

/// Overlap between consecutive chunks (seconds). Words near chunk boundaries
/// may be missed, so overlap ensures they're captured in at least one chunk.
const CHUNK_OVERLAP_SECS: f64 = 15.0;

/// Audio sample rate used by decode_to_pcm (16 kHz).
const STT_SAMPLE_RATE: u32 = 16000;

// ---------------------------------------------------------------------------
// Detailed result types
// ---------------------------------------------------------------------------

/// One recognised word with the timing the model gave it.
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptWord {
    pub text: String,
    pub start: f32,
    pub end: f32,
}

/// What the active backend could actually deliver for this call. Reported so a
/// caller can tell "no pauses" from "pauses were not measurable".
#[derive(Debug, Clone, Copy, Serialize)]
pub struct TranscriptCapabilities {
    pub word_timings: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetailedTranscript {
    pub text: String,
    /// Empty when the backend has no word-level timestamps.
    pub words: Vec<TranscriptWord>,
    pub capabilities: TranscriptCapabilities,
}

impl DetailedTranscript {
    fn text_only(text: String) -> Self {
        Self {
            text,
            words: Vec::new(),
            capabilities: TranscriptCapabilities { word_timings: false },
        }
    }
}

// ---------------------------------------------------------------------------
// Backend seam
// ---------------------------------------------------------------------------

impl ActiveModel {
    /// Plain text from any backend.
    fn transcribe_text(&mut self, samples: &[f32]) -> Result<String, String> {
        Ok(self.transcribe_segments(samples, 0.0)?.text)
    }

    /// Segments + text from any backend, with every timestamp shifted by
    /// `time_offset_secs` (chunked processing).
    fn transcribe_segments(
        &mut self,
        samples: &[f32],
        time_offset_secs: f64,
    ) -> Result<TranscriptionResult, String> {
        let mut result = match self {
            #[cfg(feature = "desktop")]
            ActiveModel::TranscribeCpp(session) => {
                let transcript = session
                    .run(samples, &CppRunOptions::default())
                    .map_err(|e| format!("Whisper transcription failed: {}", e))?;
                let segments: Vec<TranscriptionSegment> = transcript
                    .segments
                    .iter()
                    .map(|s| TranscriptionSegment {
                        start: s.t0_ms as f32 / 1000.0,
                        end: s.t1_ms as f32 / 1000.0,
                        text: s.text.clone(),
                    })
                    .collect();
                TranscriptionResult {
                    text: transcript.text,
                    segments: (!segments.is_empty()).then_some(segments),
                }
            }
            ActiveModel::Parakeet(m) => m
                .transcribe(samples, &TranscribeOptions::default())
                .map_err(|e| format!("Parakeet transcription failed: {}", e))?,
            ActiveModel::Moonshine(m) => m
                .transcribe(samples, &TranscribeOptions::default())
                .map_err(|e| format!("Moonshine transcription failed: {}", e))?,
            ActiveModel::MoonshineStreaming(m) => m
                .transcribe(samples, &TranscribeOptions::default())
                .map_err(|e| format!("Moonshine Streaming transcription failed: {}", e))?,
            ActiveModel::SenseVoice(m) => m
                .transcribe(samples, &TranscribeOptions::default())
                .map_err(|e| format!("SenseVoice transcription failed: {}", e))?,
            ActiveModel::GigaAM(m) => m
                .transcribe(samples, &TranscribeOptions::default())
                .map_err(|e| format!("GigaAM transcription failed: {}", e))?,
            ActiveModel::Canary(m) => m
                .transcribe(samples, &TranscribeOptions::default())
                .map_err(|e| format!("Canary transcription failed: {}", e))?,
            ActiveModel::Cohere(m) => m
                .transcribe(samples, &TranscribeOptions::default())
                .map_err(|e| format!("Cohere transcription failed: {}", e))?,
        };

        // Only the chunked paths pass a non-zero offset, and only Parakeet is
        // ever chunked (`transcribe_file`), so this is a no-op elsewhere.
        if time_offset_secs > 0.0 {
            if let Some(segs) = result.segments.as_mut() {
                for seg in segs.iter_mut() {
                    seg.start += time_offset_secs as f32;
                    seg.end += time_offset_secs as f32;
                }
            }
        }

        Ok(result)
    }

    /// Word-level timestamps (Parakeet only — see the support table on
    /// [`transcribe_file_word_level`]).
    fn transcribe_words(
        &mut self,
        samples: &[f32],
        time_offset_secs: f64,
    ) -> Result<TranscriptionResult, String> {
        let ActiveModel::Parakeet(m) = self else {
            return Err("Word-level timestamps are only supported by Parakeet models".to_string());
        };
        let params = ParakeetParams {
            language: None,
            timestamp_granularity: Some(TimestampGranularity::Word),
        };
        let mut result = m
            .transcribe_with(samples, &params)
            .map_err(|e| format!("Parakeet word-level transcription failed: {}", e))?;
        if time_offset_secs > 0.0 {
            if let Some(segs) = result.segments.as_mut() {
                for seg in segs.iter_mut() {
                    seg.start += time_offset_secs as f32;
                    seg.end += time_offset_secs as f32;
                }
            }
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Lending the model out
// ---------------------------------------------------------------------------

/// Holds the active model out of [`ModelState`] for the duration of one
/// transcription and puts it back when dropped — including on the `?` returns
/// where a bare local would simply drop it, leaving the slot empty while
/// `active_version` still named it. Taking it out of the mutex (rather than
/// holding the lock) is what lets `unload_model_core` serialize on
/// `transcribe_lock` instead of blocking for the whole transcription.
struct ModelGuard<'a> {
    state: &'a ModelState,
    model: Option<ActiveModel>,
}

impl<'a> ModelGuard<'a> {
    fn take(state: &'a ModelState) -> Result<Self, String> {
        // Bind first so the model mutex is released before anything else locks.
        let taken = state.model.lock().unwrap().take();
        match taken {
            Some(model) => Ok(Self {
                state,
                model: Some(model),
            }),
            None => {
                let active = state.active_version.lock().unwrap();
                if active.is_some() {
                    log::warn!(
                        "Model slot is empty but active_version is {:?} — a transcription is in progress",
                        active
                    );
                    Err("Model is busy (another transcription in progress) or not loaded".to_string())
                } else {
                    log::warn!("No model loaded (active_version is None)");
                    Err("No model is loaded".to_string())
                }
            }
        }
    }

    fn model(&mut self) -> &mut ActiveModel {
        self.model
            .as_mut()
            .expect("ModelGuard is only constructed with a model")
    }
}

impl Drop for ModelGuard<'_> {
    fn drop(&mut self) {
        if let Some(model) = self.model.take() {
            *self.state.model.lock().unwrap() = Some(model);
        }
    }
}

// ---------------------------------------------------------------------------
// Sample-level entry points
// ---------------------------------------------------------------------------

/// Transcribe decoded 16 kHz mono samples with the model that is already loaded.
///
/// No auto-load here on purpose: this is the plain-text seam behind
/// `model_transcribe`, where an idle unload must surface as an error rather than
/// silently reloading a model the caller did not ask for.
pub fn transcribe_samples_text(state: &ModelState, samples: &[f32]) -> Result<String, String> {
    let _transcribe_guard = state.transcribe_lock.lock().unwrap();
    let mut guard = ModelGuard::take(state)?;
    guard.model().transcribe_text(samples)
}

/// Transcribe decoded samples and report word timings when the active backend
/// can produce them (Parakeet only — see [`ActiveModel::transcribe_words`]).
///
/// This is the read-aloud evidence call. `model_transcribe` stays the plain
/// text call every other caller uses, so nothing existing changes shape.
pub fn transcribe_samples_detailed(
    state: &ModelState,
    samples: &[f32],
) -> Result<DetailedTranscript, String> {
    // An unloaded model (the idle timeout unloads it) is loaded again from the
    // user's *default* preference instead of failing with "No model is loaded".
    // Called before the transcribe lock, since loading takes it internally.
    ensure_model_loaded(state)?;

    let _transcribe_guard = state.transcribe_lock.lock().unwrap();
    let mut guard = ModelGuard::take(state)?;
    transcribe_detailed_pass(guard.model(), samples)
}

fn transcribe_detailed_pass(
    model: &mut ActiveModel,
    samples: &[f32],
) -> Result<DetailedTranscript, String> {
    if matches!(model, ActiveModel::Parakeet(_)) {
        let result = model.transcribe_words(samples, 0.0)?;
        let words = result
            .segments
            .unwrap_or_default()
            .into_iter()
            .map(|s| TranscriptWord { text: s.text, start: s.start, end: s.end })
            .filter(|w| !w.text.trim().is_empty())
            .collect();
        return Ok(DetailedTranscript {
            text: result.text,
            words,
            capabilities: TranscriptCapabilities { word_timings: true },
        });
    }

    Ok(DetailedTranscript::text_only(model.transcribe_text(samples)?))
}

/// Transcribe raw 16kHz mono PCM WAV bytes, loading the model first if needed.
pub fn transcribe_wav_bytes(state: &ModelState, wav: &[u8]) -> Result<String, String> {
    ensure_model_loaded(state)?;
    let samples = crate::audio::decode_wav_pcm(wav)?;
    let mut guard = ModelGuard::take(state)?;
    guard.model().transcribe_text(&samples)
}

// ---------------------------------------------------------------------------
// File transcription (used by dataset module)
// ---------------------------------------------------------------------------

/// Transcribe an audio file for subtitle generation.
///
/// ## Long Audio Handling (Parakeet only)
/// Parakeet models have a ~4-5 minute limit. For Parakeet, long audio is
/// automatically split into overlapping chunks and results are combined.
/// Other models (Whisper, Moonshine, etc.) can handle longer audio natively.
pub fn transcribe_file(
    state: &ModelState,
    file_path: &Path,
) -> Result<TranscriptionResult, String> {
    log::info!("transcribe_file: {}", file_path.display());
    let samples = crate::audio::decode_to_pcm(file_path)?;

    let total_duration_secs = samples.len() as f64 / STT_SAMPLE_RATE as f64;
    log::info!("Audio duration: {:.1}s ({:.1} min)", total_duration_secs, total_duration_secs / 60.0);

    // One lock for both questions: is anything loaded, and is it Parakeet?
    let is_parakeet = {
        let m = state.model.lock().unwrap();
        match m.as_ref() {
            None => return Err("No model is loaded".to_string()),
            Some(model) => matches!(model, ActiveModel::Parakeet(_)),
        }
    };

    // For Parakeet with long audio, use chunked transcription
    if is_parakeet && total_duration_secs > MAX_CHUNK_DURATION_SECS {
        log::info!(
            "Parakeet: audio too long (>{:.0}s), splitting into chunks",
            MAX_CHUNK_DURATION_SECS
        );
        return transcribe_file_chunked(state, &samples, total_duration_secs);
    }

    // Single-pass transcription for short audio or non-Parakeet models
    transcribe_file_pass(state, &samples, 0.0)
}

/// Run a single pass of text transcription on a sample slice.
/// `time_offset_secs` is added to all segment timestamps (for chunked processing).
fn transcribe_file_pass(
    state: &ModelState,
    samples: &[f32],
    time_offset_secs: f64,
) -> Result<TranscriptionResult, String> {
    let mut guard = ModelGuard::take(state)?;
    guard.model().transcribe_segments(samples, time_offset_secs)
}

/// Chunked transcription for long audio files (Parakeet only).
/// Splits audio into overlapping chunks, transcribes each, and combines results.
fn transcribe_file_chunked(
    state: &ModelState,
    samples: &[f32],
    total_duration_secs: f64,
) -> Result<TranscriptionResult, String> {
    log::info!(
        "Splitting into chunks of {:.0}s with {:.0}s overlap",
        MAX_CHUNK_DURATION_SECS, CHUNK_OVERLAP_SECS
    );

    let chunk_samples = (MAX_CHUNK_DURATION_SECS * STT_SAMPLE_RATE as f64) as usize;
    let step_samples = ((MAX_CHUNK_DURATION_SECS - CHUNK_OVERLAP_SECS) * STT_SAMPLE_RATE as f64) as usize;

    let mut all_segments: Vec<TranscriptionSegment> = Vec::new();
    let mut combined_text = String::new();
    let mut covered_until_secs: f64 = 0.0;
    let mut chunk_index = 0;

    loop {
        let start_sample = chunk_index * step_samples;
        if start_sample >= samples.len() {
            break;
        }
        let end_sample = (start_sample + chunk_samples).min(samples.len());
        let chunk = &samples[start_sample..end_sample];
        let chunk_duration = chunk.len() as f64 / STT_SAMPLE_RATE as f64;
        let offset_secs = start_sample as f64 / STT_SAMPLE_RATE as f64;

        log::info!(
            "Chunk {}: {:.1}s - {:.1}s ({:.1}s of audio)",
            chunk_index, offset_secs, offset_secs + chunk_duration, chunk_duration
        );

        // Transcribe this chunk
        let chunk_result = transcribe_file_pass(state, chunk, offset_secs)?;

        // Add text with space separator
        if !combined_text.is_empty() && !chunk_result.text.is_empty() {
            combined_text.push(' ');
        }
        combined_text.push_str(&chunk_result.text);

        // Add segments, deduplicating in overlap regions
        if let Some(segments) = chunk_result.segments {
            for seg in segments {
                // Skip segments that fall in the already-covered region
                let tolerance = 1.0_f32; // 1 second tolerance for segment boundaries
                if seg.start < (covered_until_secs as f32) - tolerance {
                    continue;
                }
                all_segments.push(seg);
            }
        }

        // Update covered region
        covered_until_secs = (offset_secs + chunk_duration - CHUNK_OVERLAP_SECS).max(covered_until_secs);

        log::info!(
            "Chunk {} done: {} total segments, covered_until={:.1}s",
            chunk_index, all_segments.len(), covered_until_secs
        );

        chunk_index += 1;

        // If this chunk reached the end of the audio, stop
        if end_sample >= samples.len() {
            break;
        }
    }

    let result = TranscriptionResult {
        text: combined_text,
        segments: if all_segments.is_empty() { None } else { Some(all_segments) },
    };

    log::info!(
        "transcribe_file complete (chunked): {} chunks, {} segments, {:.1}s audio",
        chunk_index,
        result.segments.as_ref().map(|s| s.len()).unwrap_or(0),
        total_duration_secs
    );
    Ok(result)
}

/// Transcribe a file with word-level timestamps (Parakeet only).
///
/// Each segment in the result represents a single word with precise start/end times.
/// Returns an error if the active model is not Parakeet.
///
/// ## Backend word-level timestamp support (transcribe-rs v0.3.11)
///
/// | Backend | Word-level | Notes |
/// |---------|-----------|-------|
/// | Parakeet (ONNX) | Yes | `TimestampGranularity::Word` via `transcribe_with()` |
/// | Whisper (whisper.cpp/GGML) | Not exposed | whisper.cpp supports DTW word timestamps, but transcribe-rs does not expose the API |
/// | Canary, SenseVoice, Moonshine (ONNX) | No | Only segment-level timestamps |
/// | OpenAI Remote | Yes | `TimestampGranularity::Word` via API (cloud-only) |
///
/// To add Whisper word-level support, transcribe-rs needs to expose the DTW
/// parameters from whisper.cpp, or we need to call whisper-rs directly.
///
/// ## Long Audio Handling
/// Parakeet v3 cannot process very long audio files (>4-5 min). This function
/// automatically splits long audio into overlapping chunks, transcribes each
/// chunk separately, and combines the results with deduplication.
pub fn transcribe_file_word_level(
    state: &ModelState,
    file_path: &Path,
) -> Result<TranscriptionResult, String> {
    log::info!("transcribe_file_word_level: {}", file_path.display());
    let samples = crate::audio::decode_to_pcm(file_path)?;
    transcribe_word_level_samples(state, &samples)
}

/// Transcribe decoded 16 kHz mono samples with word-level timestamps.
///
/// The sample-taking twin of [`transcribe_file_word_level`]: a caller that
/// already holds the audio (a picked file arrives as a path *or* as bytes)
/// should not have to write it out to read it back in. Same chunking and
/// deduplication, same backend support table.
///
/// Two things differ, both because this is called from a gesture the user can
/// repeat: the default model is loaded when nothing is loaded (instead of
/// failing with "No model is loaded" after an idle unload), and the
/// transcription lock is taken so a second press cannot find the model taken
/// out from under the first.
pub fn transcribe_samples_word_level(
    state: &ModelState,
    samples: &[f32],
) -> Result<TranscriptionResult, String> {
    ensure_model_loaded(state)?;
    let _transcribe_guard = state.transcribe_lock.lock().unwrap();
    transcribe_word_level_samples(state, samples)
}

fn transcribe_word_level_samples(
    state: &ModelState,
    samples: &[f32],
) -> Result<TranscriptionResult, String> {
    let total_duration_secs = samples.len() as f64 / STT_SAMPLE_RATE as f64;
    log::info!("Audio duration: {:.1}s ({:.1} min)", total_duration_secs, total_duration_secs / 60.0);

    // Check if model is Parakeet before proceeding
    {
        let m = state.model.lock().unwrap();
        if m.is_none() {
            return Err("No model is loaded".to_string());
        }
    }

    // If audio is short enough, run single-pass (original behavior)
    if total_duration_secs <= MAX_CHUNK_DURATION_SECS {
        log::info!("Audio fits in single chunk, running single-pass transcription");
        return transcribe_word_level_pass(state, samples, 0.0);
    }

    // Long audio: split into overlapping chunks
    log::info!(
        "Audio too long (>{:.0}s), splitting into chunks of {:.0}s with {:.0}s overlap",
        MAX_CHUNK_DURATION_SECS, MAX_CHUNK_DURATION_SECS, CHUNK_OVERLAP_SECS
    );

    let chunk_samples = (MAX_CHUNK_DURATION_SECS * STT_SAMPLE_RATE as f64) as usize;
    let step_samples = ((MAX_CHUNK_DURATION_SECS - CHUNK_OVERLAP_SECS) * STT_SAMPLE_RATE as f64) as usize;

    let mut all_words: Vec<TranscriptionSegment> = Vec::new();
    let mut covered_until_secs: f64 = 0.0;
    let mut chunk_index = 0;
    let mut offset_secs: f64 = 0.0;

    loop {
        let start_sample = chunk_index * step_samples;
        if start_sample >= samples.len() {
            break;
        }
        let end_sample = (start_sample + chunk_samples).min(samples.len());
        let chunk = &samples[start_sample..end_sample];
        let chunk_duration = chunk.len() as f64 / STT_SAMPLE_RATE as f64;

        log::info!(
            "Chunk {}: {:.1}s - {:.1}s ({:.1}s of audio, {} samples)",
            chunk_index, offset_secs, offset_secs + chunk_duration, chunk_duration, chunk.len()
        );

        // Transcribe this chunk
        let chunk_result = transcribe_word_level_pass(state, chunk, offset_secs)?;

        if let Some(words) = chunk_result.segments {
            for word in words {
                // Skip words that fall in the already-covered region (deduplication).
                // Allow a small tolerance to avoid cutting words at the boundary.
                let tolerance = 0.5_f32; // seconds
                if word.start < (covered_until_secs as f32) - tolerance {
                    continue;
                }
                all_words.push(word);
            }
        }

        // Update covered region: everything up to (chunk_end - overlap) is covered
        covered_until_secs = (offset_secs + chunk_duration - CHUNK_OVERLAP_SECS).max(covered_until_secs);

        log::info!("Chunk {} done: {} total words, covered_until={:.1}s", chunk_index, all_words.len(), covered_until_secs);

        // Move to next chunk
        chunk_index += 1;
        offset_secs = (chunk_index * step_samples) as f64 / STT_SAMPLE_RATE as f64;

        // If this chunk reached the end of the audio, stop
        if end_sample >= samples.len() {
            break;
        }
    }

    // Build combined result
    let combined_text: String = all_words.iter().map(|w| w.text.as_str()).collect::<Vec<&str>>().join(" ");
    let result = TranscriptionResult {
        text: combined_text,
        segments: if all_words.is_empty() { None } else { Some(all_words) },
    };

    log::info!(
        "transcribe_file_word_level complete: {} chunks, {} words, {:.1}s audio",
        chunk_index + 1,
        result.segments.as_ref().map(|s| s.len()).unwrap_or(0),
        total_duration_secs
    );
    Ok(result)
}

/// Run a single pass of word-level transcription on a sample slice.
/// `time_offset_secs` is added to all timestamps (for chunked processing).
fn transcribe_word_level_pass(
    state: &ModelState,
    samples: &[f32],
    time_offset_secs: f64,
) -> Result<TranscriptionResult, String> {
    let mut guard = ModelGuard::take(state)?;
    guard.model().transcribe_words(samples, time_offset_secs)
}
