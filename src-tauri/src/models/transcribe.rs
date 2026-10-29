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

/// A word starting this close to the edge of the already-covered region is the
/// next chunk's echo of a word the previous chunk already produced.
const CHUNK_DEDUPE_TOLERANCE_SECS: f64 = 0.5;

/// The same, for segment-level results: a segment boundary is a chunk of speech,
/// so the echo to forgive is wider.
const SEGMENT_DEDUPE_TOLERANCE_SECS: f64 = 1.0;

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
///
/// Long audio is split first: Parakeet is both the only backend that yields
/// word timings and the only one with an audio-length limit, so this is the one
/// caller that could otherwise feed it minutes of audio at once. See
/// [`transcribe_detailed_chunked`].
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

    // Past the window the encoder sees audio it was never trained on and the
    // decode collapses — observed as a whole German take coming back in English
    // while a sentence-long take of the same text was correct. Split it the same
    // way the word-level file path already does.
    if matches!(guard.model(), ActiveModel::Parakeet(_)) {
        let duration = samples.len() as f64 / STT_SAMPLE_RATE as f64;
        if duration > MAX_CHUNK_DURATION_SECS {
            return transcribe_detailed_chunked(guard.model(), samples, duration);
        }
    }

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

/// The word-level detailed pass, split for audio longer than the model's
/// window.
///
/// Same windows, same overlap and same de-duplication as
/// [`transcribe_word_level_samples`]; the one difference is that the model is
/// held for the whole loop instead of being handed back between chunks, because
/// the caller arrived with the guard already taken.
///
/// The text is rebuilt from the surviving words rather than joined from each
/// chunk's own text: it has to stay token-for-token identical to the words the
/// assessment aligns timings against, and a chunk's text still contains the
/// overlap words that were just dropped here.
fn transcribe_detailed_chunked(
    model: &mut ActiveModel,
    samples: &[f32],
    total_duration_secs: f64,
) -> Result<DetailedTranscript, String> {
    let windows = long_audio_windows(samples.len());
    log::info!(
        "Audio too long (>{:.0}s), splitting into {} chunks of {:.0}s with {:.0}s overlap",
        MAX_CHUNK_DURATION_SECS,
        windows.len(),
        MAX_CHUNK_DURATION_SECS,
        CHUNK_OVERLAP_SECS
    );

    let mut words: Vec<TranscriptWord> = Vec::new();
    let mut covered_until_secs: f64 = 0.0;

    for (index, (start_sample, end_sample)) in windows.iter().copied().enumerate() {
        let chunk = &samples[start_sample..end_sample];
        let chunk_duration = chunk.len() as f64 / STT_SAMPLE_RATE as f64;
        let offset_secs = start_sample as f64 / STT_SAMPLE_RATE as f64;

        log::info!(
            "Chunk {}: {:.1}s - {:.1}s ({:.1}s of audio, {} samples)",
            index, offset_secs, offset_secs + chunk_duration, chunk_duration, chunk.len()
        );

        // `transcribe_words` adds `offset_secs` to every timestamp it reports.
        let chunk_result = model.transcribe_words(chunk, offset_secs)?;

        // Where the previous chunk's transcript stopped: the end of the last word
        // it kept. It is deliberately *not* `offset + duration - overlap`, which
        // is this chunk's own first sample — a timestamp can never precede the
        // chunk it was measured in, so that comparison dropped nothing and every
        // overlap was transcribed twice. A four-minute window with fifteen
        // seconds of it said again is what pushed the passage guess a minute late
        // on a whole chapter: the repeat split the alignment in two.
        let resume_from = covered_until_secs;
        for word in chunk_result.segments.unwrap_or_default() {
            if word.text.trim().is_empty() {
                continue;
            }
            // Already spoken by the previous chunk's tail.
            if (word.start as f64) < resume_from - CHUNK_DEDUPE_TOLERANCE_SECS {
                continue;
            }
            covered_until_secs = covered_until_secs.max(word.end as f64);
            words.push(TranscriptWord { text: word.text, start: word.start, end: word.end });
        }

        log::info!(
            "Chunk {} done: {} total words, covered_until={:.1}s",
            index, words.len(), covered_until_secs
        );
    }

    let text = words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<&str>>()
        .join(" ");

    log::info!(
        "chunked detailed pass complete: {} chunks, {} words, {:.1}s audio",
        windows.len(),
        words.len(),
        total_duration_secs
    );

    Ok(DetailedTranscript {
        text,
        words,
        capabilities: TranscriptCapabilities { word_timings: true },
    })
}

/// The sample ranges long audio is split into.
///
/// Every window but the last is `MAX_CHUNK_DURATION_SECS` long and the step is
/// the duration minus `CHUNK_OVERLAP_SECS`, so consecutive windows share that
/// much audio and no sample is skipped. The last window is clipped to the end.
///
/// Shared by every chunking caller so the same audio is cut, and therefore
/// transcribed, identically no matter which page asked.
fn long_audio_windows(total_samples: usize) -> Vec<(usize, usize)> {
    let chunk_samples = (MAX_CHUNK_DURATION_SECS * STT_SAMPLE_RATE as f64) as usize;
    let step_samples = ((MAX_CHUNK_DURATION_SECS - CHUNK_OVERLAP_SECS) * STT_SAMPLE_RATE as f64) as usize;

    let mut windows = Vec::new();
    let mut start = 0usize;
    while start < total_samples {
        let end = (start + chunk_samples).min(total_samples);
        windows.push((start, end));
        if end >= total_samples {
            break;
        }
        start += step_samples;
    }
    windows
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
    let windows = long_audio_windows(samples.len());
    log::info!(
        "Splitting into {} chunks of {:.0}s with {:.0}s overlap",
        windows.len(), MAX_CHUNK_DURATION_SECS, CHUNK_OVERLAP_SECS
    );

    let mut all_segments: Vec<TranscriptionSegment> = Vec::new();
    let mut combined_text = String::new();
    let mut covered_until_secs: f64 = 0.0;

    for (index, (start_sample, end_sample)) in windows.iter().copied().enumerate() {
        let chunk = &samples[start_sample..end_sample];
        let chunk_duration = chunk.len() as f64 / STT_SAMPLE_RATE as f64;
        let offset_secs = start_sample as f64 / STT_SAMPLE_RATE as f64;

        log::info!(
            "Chunk {}: {:.1}s - {:.1}s ({:.1}s of audio)",
            index, offset_secs, offset_secs + chunk_duration, chunk_duration
        );

        // Transcribe this chunk
        let chunk_result = transcribe_file_pass(state, chunk, offset_secs)?;

        // Add text with space separator
        if !combined_text.is_empty() && !chunk_result.text.is_empty() {
            combined_text.push(' ');
        }
        combined_text.push_str(&chunk_result.text);

        // Add segments, deduplicating in overlap regions. A segment boundary is
        // a whole chunk of speech rather than a word, hence the wider tolerance.
        //
        // `resume_from` is the end of the last segment kept so far, not
        // `offset + duration - overlap`: that expression equals this chunk's own
        // first sample, and no segment can start before the chunk it was measured
        // in, so it filtered nothing and repeated every overlap in the result.
        let resume_from = covered_until_secs as f32;
        if let Some(segments) = chunk_result.segments {
            for seg in segments {
                if seg.start < resume_from - SEGMENT_DEDUPE_TOLERANCE_SECS as f32 {
                    continue;
                }
                covered_until_secs = covered_until_secs.max(seg.end as f64);
                all_segments.push(seg);
            }
        }

        log::info!(
            "Chunk {} done: {} total segments, covered_until={:.1}s",
            index, all_segments.len(), covered_until_secs
        );
    }

    let result = TranscriptionResult {
        text: combined_text,
        segments: if all_segments.is_empty() { None } else { Some(all_segments) },
    };

    log::info!(
        "transcribe_file complete (chunked): {} chunks, {} segments, {:.1}s audio",
        windows.len(),
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
    let windows = long_audio_windows(samples.len());
    log::info!(
        "Audio too long (>{:.0}s), splitting into {} chunks of {:.0}s with {:.0}s overlap",
        MAX_CHUNK_DURATION_SECS, windows.len(), MAX_CHUNK_DURATION_SECS, CHUNK_OVERLAP_SECS
    );

    let mut all_words: Vec<TranscriptionSegment> = Vec::new();
    let mut covered_until_secs: f64 = 0.0;

    for (index, (start_sample, end_sample)) in windows.iter().copied().enumerate() {
        let chunk = &samples[start_sample..end_sample];
        let chunk_duration = chunk.len() as f64 / STT_SAMPLE_RATE as f64;
        let offset_secs = start_sample as f64 / STT_SAMPLE_RATE as f64;

        log::info!(
            "Chunk {}: {:.1}s - {:.1}s ({:.1}s of audio, {} samples)",
            index, offset_secs, offset_secs + chunk_duration, chunk_duration, chunk.len()
        );

        // Transcribe this chunk
        let chunk_result = transcribe_word_level_pass(state, chunk, offset_secs)?;

        if let Some(words) = chunk_result.segments {
            for word in words {
                // Skip words that fall in the already-covered region (deduplication).
                // Allow a small tolerance to avoid cutting words at the boundary.
                let tolerance = CHUNK_DEDUPE_TOLERANCE_SECS as f32;
                if word.start < (covered_until_secs as f32) - tolerance {
                    continue;
                }
                all_words.push(word);
            }
        }

        // Update covered region: everything up to (chunk_end - overlap) is covered
        covered_until_secs = (offset_secs + chunk_duration - CHUNK_OVERLAP_SECS).max(covered_until_secs);

        log::info!("Chunk {} done: {} total words, covered_until={:.1}s", index, all_words.len(), covered_until_secs);
    }

    // Build combined result
    let combined_text: String = all_words.iter().map(|w| w.text.as_str()).collect::<Vec<&str>>().join(" ");
    let result = TranscriptionResult {
        text: combined_text,
        segments: if all_words.is_empty() { None } else { Some(all_words) },
    };

    log::info!(
        "transcribe_file_word_level complete: {} chunks, {} words, {:.1}s audio",
        windows.len(),
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

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = STT_SAMPLE_RATE as f64;

    fn samples_for(seconds: f64) -> usize {
        (seconds * RATE) as usize
    }

    #[test]
    fn audio_that_fits_the_window_is_one_chunk() {
        let total = samples_for(MAX_CHUNK_DURATION_SECS);
        assert_eq!(long_audio_windows(total), vec![(0, total)]);
    }

    #[test]
    fn long_audio_steps_by_the_duration_minus_the_overlap() {
        let total = samples_for(600.0);
        let windows = long_audio_windows(total);
        let step = samples_for(MAX_CHUNK_DURATION_SECS - CHUNK_OVERLAP_SECS);

        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0], (0, samples_for(MAX_CHUNK_DURATION_SECS)));
        assert_eq!(windows[1].0, step);
        assert_eq!(windows[2].0, 2 * step);
        assert_eq!(windows.last().unwrap().1, total, "the last chunk ends with the audio");

        // Consecutive chunks share exactly the configured overlap.
        for pair in windows.windows(2) {
            assert_eq!(pair[0].1 - pair[1].0, samples_for(CHUNK_OVERLAP_SECS));
        }
    }

    #[test]
    fn every_sample_is_covered() {
        let total = samples_for(1100.0);
        let windows = long_audio_windows(total);

        assert_eq!(windows.first().unwrap().0, 0);
        assert_eq!(windows.last().unwrap().1, total);
        for pair in windows.windows(2) {
            assert!(pair[1].0 < pair[0].1, "a gap between chunks would drop audio");
        }
    }

    #[test]
    fn no_audio_means_no_chunk() {
        assert!(long_audio_windows(0).is_empty());
    }
}
