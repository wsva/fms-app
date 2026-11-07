//! Running a model: text, segments and word-level timestamps, chunking for
//! audio longer than a model can take, and the guard that lends the model out
//! of [`ModelState`] for the length of one call.
//!
//! Backend differences live in two places only — [`ActiveModel::transcribe_segments`]
//! for every backend and [`ActiveModel::transcribe_words`] for the backends that
//! have word rows, plus [`ActiveModel::engine_capabilities`] for what a loaded
//! engine can answer about itself. Whether word timings are *asked for* is never
//! decided by matching on the variant: the engine says, once, at load time.

use std::path::Path;

use serde::Serialize;
#[cfg(feature = "desktop")]
use transcribe_cpp::RunOptions as CppRunOptions;
#[cfg(feature = "desktop")]
use transcribe_cpp::TimestampKind as CppTimestampKind;
use transcribe_rs::onnx::parakeet::{ParakeetParams, TimestampGranularity};
use transcribe_rs::{
    SpeechModel, TranscribeOptions, TranscriptionResult, TranscriptionSegment,
};

use crate::models::catalog_stt::ModelCapabilities;
use crate::models::engine::ensure_model_loaded;
use crate::models::state::{ActiveModel, EngineCapabilities, ModelState};

// ---------------------------------------------------------------------------
// Constants for chunked transcription of long audio files
// ---------------------------------------------------------------------------

/// Overlap between consecutive chunks (seconds). Words near chunk boundaries
/// may be missed, so overlap ensures they're captured in at least one chunk.
///
/// The chunk *length* is not a constant: it is the input window the loaded engine
/// states (`EngineCapabilities::max_chunk_secs`), which is 240 s for the Parakeet
/// ONNX rows and nothing at all for an engine that windows internally.
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
    /// What the loaded engine itself can do.
    ///
    /// `advertised` is the catalog's answer for the row being loaded, and it is
    /// only used where the engine cannot say: transcribe-rs reports
    /// `supports_timestamps` as a bool with no granularity, so an ONNX model's
    /// word timings are the catalog's granularity *confirmed* by the runtime
    /// bool, never invented from it. A transcribe-cpp model answers every field
    /// from its own metadata and architecture code, and that answer *replaces*
    /// whatever was advertised — it is the same source the decoder will use.
    pub(crate) fn engine_capabilities(&self, advertised: &ModelCapabilities) -> EngineCapabilities {
        match self {
            #[cfg(feature = "desktop")]
            ActiveModel::TranscribeCpp(session) => match session.model().capabilities() {
                Ok(caps) => EngineCapabilities {
                    word_timings: matches!(
                        caps.max_timestamp_kind,
                        CppTimestampKind::Word | CppTimestampKind::Token
                    ),
                    // 0 is how the C ABI spells "this family has no window limit"
                    // — which is exactly the question `long_audio_windows` exists
                    // to work around, so it is worth reporting rather than
                    // folding into `None`-means-unknown.
                    max_chunk_secs: (caps.max_audio_ms > 0)
                        .then(|| caps.max_audio_ms as f64 / 1000.0),
                    streaming: caps.supports_streaming,
                },
                Err(e) => {
                    // A metadata read that fails on a loaded model is a bug or a
                    // corrupt file, not an answer: fall back to the catalog and
                    // say so, rather than reporting a confident "no".
                    log::warn!("Could not read engine capabilities: {e}; using the catalog's");
                    EngineCapabilities {
                        word_timings: advertised.word_timings(),
                        max_chunk_secs: advertised.max_chunk_secs,
                        streaming: advertised.streaming,
                    }
                }
            },
            other => {
                let caps = match other {
                    ActiveModel::Parakeet(m) => m.capabilities(),
                    ActiveModel::Moonshine(m) => m.capabilities(),
                    ActiveModel::MoonshineStreaming(m) => m.capabilities(),
                    ActiveModel::SenseVoice(m) => m.capabilities(),
                    ActiveModel::GigaAM(m) => m.capabilities(),
                    ActiveModel::Canary(m) => m.capabilities(),
                    ActiveModel::Cohere(m) => m.capabilities(),
                    #[allow(unreachable_patterns)]
                    _ => unreachable!("every ONNX variant is listed above"),
                };
                EngineCapabilities {
                    word_timings: advertised.word_timings() && caps.supports_timestamps,
                    max_chunk_secs: advertised.max_chunk_secs,
                    streaming: caps.supports_streaming,
                }
            }
        }
    }

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

        // Only the chunked paths pass a non-zero offset, and only an engine that
        // states an input window is ever chunked (`transcribe_file`), so this is a
        // no-op elsewhere.
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

    /// Word rows plus text, from the backends that have word rows.
    ///
    /// Whether to ask at all is decided by the measured
    /// [`EngineCapabilities::word_timings`], never by matching on the variant; this
    /// only answers for the engines that can. `time_offset_secs` is added to every
    /// timestamp, because a chunk's timings are relative to the chunk.
    fn transcribe_word_rows(
        &mut self,
        samples: &[f32],
        time_offset_secs: f64,
    ) -> Result<(String, Vec<TranscriptWord>), String> {
        match self {
            #[cfg(feature = "desktop")]
            ActiveModel::TranscribeCpp(session) => {
                // Asking for word rows below a family's own ceiling is allowed (the
                // ladder is `segment < word < token`), and a token-level family
                // still fills the word rows, so this one request covers both.
                // Callers only reach here once the engine said it can.
                let options =
                    CppRunOptions { timestamps: CppTimestampKind::Word, ..Default::default() };
                let transcript = session
                    .run(samples, &options)
                    .map_err(|e| format!("Word-level transcription failed: {}", e))?;
                Ok((transcript.text, words_from_cpp(&transcript.words, time_offset_secs)))
            }
            ActiveModel::Parakeet(m) => {
                let params = ParakeetParams {
                    language: None,
                    timestamp_granularity: Some(TimestampGranularity::Word),
                };
                let result = m
                    .transcribe_with(samples, &params)
                    .map_err(|e| format!("Parakeet word-level transcription failed: {}", e))?;
                let words = result
                    .segments
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| TranscriptWord { text: s.text, start: s.start, end: s.end })
                    .map(|w| shifted(w, time_offset_secs))
                    .collect();
                Ok((result.text, words))
            }
            _ => Err("The active model does not produce word-level timestamps".to_string()),
        }
    }

    /// The same word rows in the segment-shaped form the word-level file API has
    /// always returned, so its callers keep reading `result.segments`.
    fn transcribe_words(
        &mut self,
        samples: &[f32],
        time_offset_secs: f64,
    ) -> Result<TranscriptionResult, String> {
        let (text, words) = self.transcribe_word_rows(samples, time_offset_secs)?;
        let segments: Vec<TranscriptionSegment> = words
            .into_iter()
            .map(|w| TranscriptionSegment { start: w.start, end: w.end, text: w.text })
            .collect();
        Ok(TranscriptionResult { text, segments: (!segments.is_empty()).then_some(segments) })
    }
}

/// `transcribe_word_rows` for the engines that report times relative to the
/// buffer they were given.
fn shifted(word: TranscriptWord, time_offset_secs: f64) -> TranscriptWord {
    if time_offset_secs == 0.0 {
        return word;
    }
    TranscriptWord {
        text: word.text,
        start: word.start + time_offset_secs as f32,
        end: word.end + time_offset_secs as f32,
    }
}

/// Turns transcribe-cpp's word rows into ours.
///
/// The crate speaks integer milliseconds, this app speaks float seconds, and the
/// conversion belongs in one place a test can reach without a model. Whisper
/// tokens carry their joining space (`" hello"`), which is not part of the word —
/// every consumer here matches on trimmed text. A row that is *only* that space
/// (a punctuation or silence token) is not a word and is dropped.
#[cfg(feature = "desktop")]
fn words_from_cpp(words: &[transcribe_cpp::Word], time_offset_secs: f64) -> Vec<TranscriptWord> {
    words
        .iter()
        .filter_map(|w| {
            let text = w.text.trim();
            if text.is_empty() {
                return None;
            }
            let word =
                TranscriptWord { text: text.to_string(), start: w.t0_ms as f32 / 1000.0, end: w.t1_ms as f32 / 1000.0 };
            Some(shifted(word, time_offset_secs))
        })
        .collect()
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

/// Transcribe decoded samples and report word timings when the active engine
/// can produce them, as measured at load time (see
/// [`ActiveModel::engine_capabilities`]) rather than as assumed from its type.
///
/// This is the read-aloud evidence call. `model_transcribe` stays the plain
/// text call every other caller uses, so nothing existing changes shape.
///
/// Long audio is split first, but only for an engine that both yields word
/// timings and states a window: an engine with no limit takes the whole take in
/// one call, and an engine without word rows is not chunked here at all.
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
    let caps = measured_capabilities(state);

    // Past the window the encoder sees audio it was never trained on and the
    // decode collapses — observed as a whole German take coming back in English
    // while a sentence-long take of the same text was correct. Only an engine
    // that states a window is split: one with no limit takes the whole take in
    // one call, and an engine with no word rows is not on this path at all.
    let duration = samples.len() as f64 / STT_SAMPLE_RATE as f64;
    let window = if caps.word_timings { caps.max_chunk_secs } else { None };
    match window {
        Some(w) if duration > w => transcribe_detailed_chunked(guard.model(), samples, duration, w),
        _ => transcribe_detailed_pass(guard.model(), samples, caps),
    }
}

fn transcribe_detailed_pass(
    model: &mut ActiveModel,
    samples: &[f32],
    caps: EngineCapabilities,
) -> Result<DetailedTranscript, String> {
    if caps.word_timings {
        let (text, words) = model.transcribe_word_rows(samples, 0.0)?;
        return Ok(DetailedTranscript {
            text,
            words: words.into_iter().filter(|w| !w.text.trim().is_empty()).collect(),
            capabilities: TranscriptCapabilities { word_timings: true },
        });
    }

    Ok(DetailedTranscript::text_only(model.transcribe_text(samples)?))
}

/// The word-level detailed pass, split for audio longer than the engine's own
/// window (`window_secs`, from [`EngineCapabilities::max_chunk_secs`]).
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
    window_secs: f64,
) -> Result<DetailedTranscript, String> {
    let windows = audio_windows(samples.len(), window_secs);
    log::info!(
        "Audio longer than the engine's {:.0}s window ({:.1}s), splitting into {} chunks with {:.1}s overlap",
        window_secs,
        total_duration_secs,
        windows.len(),
        chunk_overlap_secs(window_secs),
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

        // `transcribe_word_rows` adds `offset_secs` to every timestamp it reports.
        let (_, chunk_words) = model.transcribe_word_rows(chunk, offset_secs)?;

        // Where the previous chunk's transcript stopped: the end of the last word
        // it kept. It is deliberately *not* `offset + duration - overlap`, which
        // is this chunk's own first sample — a timestamp can never precede the
        // chunk it was measured in, so that comparison dropped nothing and every
        // overlap was transcribed twice. A four-minute window with fifteen
        // seconds of it said again is what pushed the text guess a minute late
        // on a whole chapter: the repeat split the alignment in two.
        let resume_from = covered_until_secs;
        for word in chunk_words {
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

/// The sample ranges long audio is split into for an engine with a `window_secs`
/// input limit.
///
/// Every window but the last is `window_secs` long and the step is the window
/// minus [`chunk_overlap_secs`], so consecutive windows share that much audio and
/// no sample is skipped. The last window is clipped to the end.
///
/// Shared by every chunking caller so the same audio is cut, and therefore
/// transcribed, identically no matter which page asked.
fn audio_windows(total_samples: usize, window_secs: f64) -> Vec<(usize, usize)> {
    let chunk_samples = (window_secs * STT_SAMPLE_RATE as f64) as usize;
    let step_samples = ((window_secs - chunk_overlap_secs(window_secs)) * STT_SAMPLE_RATE as f64) as usize;

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

/// Audio each consecutive window says again.
///
/// [`CHUNK_OVERLAP_SECS`] is tuned for a four-minute window. An engine with a
/// shorter one still needs the boundary words repeated, but a fixed fifteen
/// seconds would make the step smaller than the window and the loop would never
/// advance — so the overlap is capped at half the window.
fn chunk_overlap_secs(window_secs: f64) -> f64 {
    CHUNK_OVERLAP_SECS.min(window_secs / 2.0)
}

/// What the loaded engine reported about itself at load time.
///
/// Callers reach this only after a [`ModelGuard`] has proved a model is resident,
/// and `load_model_core` measures before it puts the model in the slot — so the
/// `None` this would mean ("never measured") cannot be produced by a working
/// load path. It is still the difference between "the engine said no" and "nobody
/// asked", so an unmeasured engine claims nothing: no word rows, no window.
fn measured_capabilities(state: &ModelState) -> EngineCapabilities {
    let measured = *state.active_capabilities.lock().unwrap();
    measured.unwrap_or_else(|| {
        log::warn!("No measured capabilities for the active model; assuming the least it can do");
        EngineCapabilities::default()
    })
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
/// ## Long Audio Handling
/// An engine that states an input window (`EngineCapabilities::max_chunk_secs`,
/// measured from the model itself at load time) is not handed more than it: long
/// audio is split into overlapping chunks and the results combined. An engine
/// that states no window — Whisper windows internally, GigaAM says so outright —
/// gets the whole file in one call, exactly as before.
pub fn transcribe_file(
    state: &ModelState,
    file_path: &Path,
) -> Result<TranscriptionResult, String> {
    log::info!("transcribe_file: {}", file_path.display());
    let samples = crate::audio::decode_to_pcm(file_path)?;

    let total_duration_secs = samples.len() as f64 / STT_SAMPLE_RATE as f64;
    log::info!("Audio duration: {:.1}s ({:.1} min)", total_duration_secs, total_duration_secs / 60.0);

    // The model slot is the only thing that says whether a model is resident —
    // a transcription in progress holds it out, and this path must not start a
    // second one on top of it.
    let loaded = state.model.lock().unwrap().is_some();
    if !loaded {
        return Err("No model is loaded".to_string());
    }
    let caps = measured_capabilities(state);

    match caps.max_chunk_secs {
        Some(window) if total_duration_secs > window => {
            log::info!(
                "Audio exceeds the engine's {:.0}s window, splitting into chunks",
                window
            );
            transcribe_file_chunked(state, &samples, total_duration_secs, window)
        }
        // Single-pass transcription for short audio or engines with no window.
        _ => transcribe_file_pass(state, &samples, 0.0),
    }
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

/// Chunked transcription for audio longer than an engine's input window.
/// Splits audio into overlapping chunks, transcribes each, and combines results.
fn transcribe_file_chunked(
    state: &ModelState,
    samples: &[f32],
    total_duration_secs: f64,
    window_secs: f64,
) -> Result<TranscriptionResult, String> {
    let windows = audio_windows(samples.len(), window_secs);
    log::info!(
        "Splitting into {} chunks of {:.0}s with {:.1}s overlap",
        windows.len(), window_secs, chunk_overlap_secs(window_secs)
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

/// Transcribe a file with word-level timestamps.
///
/// Each segment in the result represents a single word with precise start/end
/// times. Returns an error when the loaded engine reported it cannot produce
/// word rows at all.
///
/// ## Which engines produce word-level timestamps
///
/// The answer is measured per model at load time
/// ([`ActiveModel::engine_capabilities`]), not tabulated by engine type. What the
/// engines can say:
///
/// | Engine | Word rows | How it is known |
/// |---------|-----------|-----------------|
/// | Parakeet (ONNX) | Yes | card granularity + `SpeechModel::capabilities().supports_timestamps` |
/// | Parakeet, Granite, Canary-family (GGUF) | Yes (token rows imply word rows) | `Capabilities::max_timestamp_kind` |
/// | Whisper (GGML `.bin` and GGUF) | No — segment only | `Capabilities::max_timestamp_kind` |
/// | Moonshine, SenseVoice, GigaAM, Cohere (ONNX) | No | card granularity |
///
/// A token-level family always fills the word rows too: the ladder in
/// transcribe-cpp is `segment < word < token`, and a word row indexes the token
/// rows it was made of. Granularity is never readable from a GGUF header, which
/// is why the engine has to be loaded to answer it.
///
/// ## Long Audio Handling
/// Audio longer than the engine's stated input window is split into overlapping
/// chunks, transcribed separately, and combined with deduplication. An engine
/// that states no window takes the file in one call.
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
/// deduplication, same measured answer on whether word rows exist.
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

    // The loaded engine is the authority on whether it can be asked for word
    // rows at all; a model still in the slot is the authority on whether one is
    // loaded (a transcription in progress holds it out).
    let loaded = state.model.lock().unwrap().is_some();
    if !loaded {
        return Err("No model is loaded".to_string());
    }
    let caps = measured_capabilities(state);
    if !caps.word_timings {
        return Err("The active model does not produce word-level timestamps".to_string());
    }

    // An engine that states no input window is not handed a split: it windows
    // internally, and cutting its audio up would only repeat work.
    let window = match caps.max_chunk_secs {
        Some(window) if total_duration_secs > window => window,
        _ => {
            log::info!("Audio fits the engine's window, running single-pass transcription");
            return transcribe_word_level_pass(state, samples, 0.0);
        }
    };

    // Long audio: split into overlapping chunks
    let windows = audio_windows(samples.len(), window);
    log::info!(
        "Audio exceeds the engine's {:.0}s window ({:.1}s), splitting into {} chunks with {:.1}s overlap",
        window, total_duration_secs, windows.len(), chunk_overlap_secs(window)
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
        covered_until_secs =
            (offset_secs + chunk_duration - chunk_overlap_secs(window)).max(covered_until_secs);

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

    /// The window the shipped catalog states for the Parakeet ONNX rows — the
    /// four-minute limit that is the reason this chunking exists. Read from the
    /// generated data rather than restated here, so a curated change shows up in
    /// these windows instead of quietly disagreeing with them.
    fn parakeet_window_secs() -> f64 {
        crate::models::catalog_stt::find_model("parakeet-v3")
            .and_then(|def| def.capabilities.max_chunk_secs)
            .expect("the Parakeet ONNX rows must state their input window")
    }

    fn samples_for(seconds: f64) -> usize {
        (seconds * RATE) as usize
    }

    #[test]
    fn audio_that_fits_the_window_is_one_chunk() {
        let total = samples_for(parakeet_window_secs());
        assert_eq!(audio_windows(total, parakeet_window_secs()), vec![(0, total)]);
    }

    #[test]
    fn long_audio_steps_by_the_duration_minus_the_overlap() {
        let window = parakeet_window_secs();
        let total = samples_for(600.0);
        let windows = audio_windows(total, window);
        let step = samples_for(window - CHUNK_OVERLAP_SECS);

        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0], (0, samples_for(window)));
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
        let windows = audio_windows(total, parakeet_window_secs());

        assert_eq!(windows.first().unwrap().0, 0);
        assert_eq!(windows.last().unwrap().1, total);
        for pair in windows.windows(2) {
            assert!(pair[1].0 < pair[0].1, "a gap between chunks would drop audio");
        }
    }

    #[test]
    fn no_audio_means_no_chunk() {
        assert!(audio_windows(0, parakeet_window_secs()).is_empty());
    }

    /// A window far shorter than the tuned overlap still has to advance, or the
    /// chunk loop would never reach the end of the file.
    #[test]
    fn a_short_window_still_advances() {
        let total = samples_for(60.0);
        let windows = audio_windows(total, 20.0);

        assert_eq!(windows.first().unwrap(), &(0, samples_for(20.0)));
        assert_eq!(windows.last().unwrap().1, total);
        for pair in windows.windows(2) {
            assert!(pair[1].0 > pair[0].0, "a step of zero would loop forever");
        }
        // Half the window, not the fixed fifteen seconds.
        assert_eq!(windows[1].0, samples_for(10.0));
    }

    #[cfg(feature = "desktop")]
    fn cpp_word(t0_ms: i64, t1_ms: i64, text: &str) -> transcribe_cpp::Word {
        transcribe_cpp::Word {
            t0_ms,
            t1_ms,
            seg_index: 0,
            first_token: 0,
            n_tokens: 1,
            text: text.to_string(),
        }
    }

    /// transcribe-cpp answers in integer milliseconds; everything downstream in
    /// this app reads float seconds.
    #[cfg(feature = "desktop")]
    #[test]
    fn cpp_milliseconds_become_our_seconds() {
        let words = words_from_cpp(&[cpp_word(1500, 2750, " hello")], 0.0);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "hello");
        assert_eq!(words[0].start, 1.5);
        assert_eq!(words[0].end, 2.75);
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn a_chunk_offset_moves_the_cpp_word_times() {
        let words = words_from_cpp(&[cpp_word(0, 500, "first")], 240.0);
        assert_eq!(words[0].start, 240.0);
        assert_eq!(words[0].end, 240.5);
    }

    /// Whitespace-only rows are the joining space of a punctuation or silence
    /// token, not a word the read-aloud aligner could ever match.
    #[cfg(feature = "desktop")]
    #[test]
    fn space_only_cpp_rows_are_dropped_and_order_survives() {
        let words =
            words_from_cpp(&[cpp_word(0, 100, "one"), cpp_word(100, 150, " "), cpp_word(150, 300, "two")], 0.0);
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, vec!["one", "two"]);
        assert!(words.windows(2).all(|p| p[0].end <= p[1].start), "word rows stay in time order");
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn no_cpp_words_is_no_words() {
        assert!(words_from_cpp(&[], 12.0).is_empty());
    }
}
