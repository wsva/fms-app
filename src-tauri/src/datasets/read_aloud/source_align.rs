//! Source alignment: guess every passage's clip from a directory of recordings.
//!
//! ## The problem this exists for
//!
//! An audiobook's recordings are **outside** the dataset and each holds many
//! passages. [`super::standard_audio::guess_span`] answers "where is *this* text
//! in *that* file" by decoding and transcribing the whole file, so guessing a
//! whole dataset that way costs O(texts × files) minutes and throws away the one
//! fact the bulk case has: the texts are in the same order as the recordings.
//!
//! This module pays for the transcription **once per file**, then walks the
//! passages in `order_num` along one global token stream with a cursor that only
//! moves forward. Each passage yields a span, the spans are written down as
//! *proposals*, a human accepts or rejects them, and only then are the clips cut.
//!
//! ## Storage
//!
//! Files, not schema, under `<dataset>/align/`:
//!
//! | file | holds |
//! |---|---|
//! | `sources.json` | the registered recordings, in playing order, with size+mtime |
//! | `<source_uuid>.json` | one recording's word timings and 20 ms peak envelope |
//! | `proposals.json` | one entry per passage: the span, its confidence, its verdict |
//!
//! All three are derived and regenerable, so none of them is journalled, and
//! [`crate::datasets::is_derived_index`] keeps the whole directory out of the
//! sync manifest — a cached transcript is a device's own minutes of work, not
//! content, and the recordings themselves stay where the user keeps them.
//!
//! ## What a caller does
//!
//! `register` → `transcribe` (once per file) → `run` → `decide` → `apply`.
//! Each step is idempotent and resumable, and every one of them is a tool, so an
//! agent can drive the same pipeline a human does in the Advanced panel.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::State;

use crate::audio;
use crate::datasets::asr_metric;
use crate::settings::SettingsState;

use super::standard_audio::{
    densest_run, text_reference, GUESS_MIN_MATCH_RATIO, GUESS_MIN_WORDS, GUESS_PAD_AFTER,
    GUESS_PAD_BEFORE,
};
use super::{find_dataset_dir, find_dataset_dir_write, open_db};

// ============================================================
// Constants
// ============================================================

/// Extensions the registration scan accepts. A list rather than "anything the
/// decoder opens", because a chapter directory usually also holds cover art and
/// a cue sheet, and a bogus entry costs a failed transcription to discover.
const AUDIO_EXTENSIONS: [&str; 12] = [
    "mp3", "m4a", "m4b", "aac", "wav", "flac", "ogg", "oga", "opus", "wma", "mp4", "mkv",
];

/// The longest single recording one call will decode. Two hours of 16 kHz f32 is
/// ~460 MB, which the transcription call holds on top of the samples, so this is
/// where a single-file unabridged audiobook is refused with a message instead of
/// being swapped to disk until the process dies.
const MAX_SOURCE_SECS: f64 = 7200.0;

/// Width of one peak-envelope bucket. Short enough that a syllable boundary is
/// visible at the crop edges, long enough that ten hours is 1.8M pairs.
const PEAK_SECONDS: f64 = 0.02;

/// How far either edge may travel to land on a quiet bucket. A model's word
/// boundary can land inside a consonant; the quietest 20 ms nearby is a better
/// cut than the boundary itself.
const SNAP_WINDOW_SECS: f64 = 0.15;

/// The first window a passage is searched in, how much it grows, and the factor
/// past which a final attempt may as well be the rest of the stream.
const SEARCH_INITIAL_TOKENS: usize = 400;
const SEARCH_GROWTH: usize = 4;
const SEARCH_MAX_FACTOR: usize = 64;

/// Shortest span a proposal may describe. Below this the two edges are a
/// mis-drag, not a reading.
const MIN_SPAN_SECS: f64 = 0.3;

/// How many peak pairs one proposal's preview carries. Enough for a readable
/// thumbnail, small enough that forty of them are still one response.
const PREVIEW_PAIRS: usize = 400;

/// How much of the surrounding audio a preview keeps beyond the span itself.
const PREVIEW_MARGIN_SECS: f64 = 0.25;

// ============================================================
// Stored shapes
// ============================================================

/// One registered recording. Identity is the file name's hash, so re-picking a
/// folder keeps the cache of every file that kept its name.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceEntry {
    pub uuid: String,
    pub file_name: String,
    /// Absolute path as it was when the folder was picked. Re-checked on every
    /// transcription, because a renamed or moved folder must say so rather than
    /// silently transcribe the wrong thing.
    pub path: String,
    pub size: u64,
    pub modified_ms: u64,
    /// Seconds of decoded audio; `None` until this file is transcribed.
    #[serde(default)]
    pub duration: Option<f64>,
    /// Spoken words in the cached transcript; `None` when there is no cache.
    #[serde(default)]
    pub words: Option<usize>,
}

/// The ordered recording list.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SourceIndex {
    /// The directory the files were picked from, shown so it is obvious the
    /// cache is tied to a folder and not to the dataset.
    #[serde(default)]
    directory: String,
    #[serde(default)]
    sources: Vec<SourceEntry>,
}

/// One spoken word as the model timed it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SpokenWord {
    start: f64,
    end: f64,
    /// The word's tokens after scoring's own normalization, so the alignment
    /// compares exactly the shapes it would compare from the reference text.
    tokens: Vec<String>,
}

/// One recording's cache: the word timings a span can be read off, and the peak
/// envelope an edge can be snapped to and a preview drawn from.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SourceTranscript {
    uuid: String,
    file_name: String,
    duration: f64,
    /// Seconds one peak pair covers — stored rather than assumed, so changing
    /// [`PEAK_SECONDS`] does not silently misread old caches.
    peak_seconds: f64,
    /// base64 of the interleaved min/max `i8` pairs (`WaveformPeaks` layout).
    peaks: String,
    words: Vec<SpokenWord>,
}

/// What one passage amounts to.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Proposal {
    text_uuid: String,
    #[serde(default)]
    source_uuid: Option<String>,
    #[serde(default)]
    source_name: String,
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    #[serde(default)]
    confidence: f64,
    #[serde(default)]
    matched_words: usize,
    #[serde(default)]
    reference_words: usize,
    /// `"proposed"` when there is a span, `"refused"` when there is not.
    status: String,
    /// Why it was refused, or what to look at when it is unsure.
    #[serde(default)]
    note: String,
    /// `"pending"`, `"accepted"` or `"rejected"`.
    state: String,
    /// Set by `apply`: the standard-audio uuid now holding this span.
    #[serde(default)]
    applied_uuid: Option<String>,
}

impl Proposal {
    fn refusal(text_uuid: &str, note: String) -> Self {
        Self {
            text_uuid: text_uuid.to_string(),
            source_uuid: None,
            source_name: String::new(),
            start: 0.0,
            end: 0.0,
            confidence: 0.0,
            matched_words: 0,
            reference_words: 0,
            status: "refused".to_string(),
            note,
            state: "pending".to_string(),
            applied_uuid: None,
        }
    }

    fn is_open(&self) -> bool {
        self.status == "proposed" && self.state == "accepted" && self.applied_uuid.is_none()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ProposalSet {
    #[serde(default)]
    generated_at: String,
    #[serde(default)]
    proposals: Vec<Proposal>,
}

// ============================================================
// Paths and JSON
// ============================================================

fn align_dir(dir: &Path) -> PathBuf {
    dir.join(crate::datasets::READ_ALOUD_ALIGN_DIR)
}

fn sources_path(dir: &Path) -> PathBuf {
    align_dir(dir).join("sources.json")
}

fn transcript_path(dir: &Path, source_uuid: &str) -> PathBuf {
    align_dir(dir).join(format!("{}.json", source_uuid))
}

fn proposals_path(dir: &Path) -> PathBuf {
    align_dir(dir).join("proposals.json")
}

/// A missing or unreadable cache reads as "nothing cached yet". Derived data
/// must never turn into a failure: the next run simply rebuilds it.
fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

fn load_index(dir: &Path) -> SourceIndex {
    read_json(&sources_path(dir)).unwrap_or_default()
}

fn save_index(dir: &Path, index: &SourceIndex) -> Result<(), String> {
    write_json(&sources_path(dir), index)
}

fn load_proposals(dir: &Path) -> ProposalSet {
    read_json(&proposals_path(dir)).unwrap_or_default()
}

// ============================================================
// Peaks
// ============================================================

/// Encode interleaved `i8` min/max pairs as base64. `i8 as u8` is a lossless
/// reinterpretation, which is why the round trip needs no scaling.
fn encode_peaks(peaks: &[i8]) -> String {
    let bytes: Vec<u8> = peaks.iter().map(|v| *v as u8).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_peaks(encoded: &str) -> Vec<i8> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map(|bytes| bytes.iter().map(|b| *b as i8).collect())
        .unwrap_or_default()
}

/// Reduce decoded 16 kHz mono samples to a [`PEAK_SECONDS`] envelope, and report
/// the bucket width actually achieved (rounding the bucket to whole samples can
/// shift it a hair).
fn envelope(samples: &[f32]) -> (Vec<i8>, f64) {
    let wanted = ((PEAK_SECONDS * audio::DECODED_SAMPLE_RATE as f64).round() as usize).max(1);
    let columns = samples.len().div_ceil(wanted).max(1);
    let (peaks, per_pair) = audio::peaks_from_samples(samples, columns);
    (peaks, per_pair as f64 / audio::DECODED_SAMPLE_RATE as f64)
}

/// The quietest bucket within [`SNAP_WINDOW_SECS`] of `seconds`, clamped to
/// `[low, high]`. Ties go to the bucket nearest `seconds`, so a flat stretch of
/// silence does not push the edge to a window corner.
fn snap_quiet(peaks: &[i8], peak_seconds: f64, seconds: f64, low: f64, high: f64) -> f64 {
    if peaks.len() < 2 || !(peak_seconds > 0.0) {
        return seconds;
    }
    let pairs = peaks.len() / 2;
    let centre = (seconds / peak_seconds).round() as i64;
    let reach = (SNAP_WINDOW_SECS / peak_seconds).round() as i64;
    let mut best: Option<(i64, i64, i64)> = None; // (loudness, distance, bucket)
    for offset in -reach..=reach {
        let bucket = centre + offset;
        if bucket < 0 || bucket as usize >= pairs {
            continue;
        }
        let index = bucket as usize * 2;
        let loudness = (peaks[index] as i32).abs().max((peaks[index + 1] as i32).abs());
        let candidate = (loudness as i64, offset.abs(), bucket);
        if best
            .map(|b| (candidate.0, candidate.1) < (b.0, b.1))
            .unwrap_or(true)
        {
            best = Some(candidate);
        }
    }
    let Some((_, _, bucket)) = best else {
        return seconds;
    };
    (bucket as f64 * peak_seconds).clamp(low, high.max(low))
}

/// A window of the peak envelope around a span: the thumbnail beside one
/// proposal's row. Wide spans are decimated by min/max so the preview keeps the
/// loudness shape instead of aliasing it away.
#[derive(Serialize)]
pub struct Preview {
    /// Absolute seconds of the first pair.
    pub start: f64,
    /// Seconds each returned pair covers.
    pub peak_seconds: f64,
    /// Interleaved min/max `i8` pairs.
    pub peaks: Vec<i8>,
}

fn preview(transcript: &SourceTranscript, start: f64, end: f64) -> Preview {
    let all = decode_peaks(&transcript.peaks);
    let pairs = all.len() / 2;
    let peak = transcript.peak_seconds.max(f64::EPSILON);
    if pairs == 0 {
        return Preview {
            start: 0.0,
            peak_seconds: peak,
            peaks: Vec::new(),
        };
    }
    let from = (((start - PREVIEW_MARGIN_SECS) / peak).floor().max(0.0) as usize).min(pairs - 1);
    let to = (((end + PREVIEW_MARGIN_SECS) / peak).ceil() as usize).clamp(from + 1, pairs);
    let stride = (to - from).div_ceil(PREVIEW_PAIRS).max(1);

    let mut peaks = Vec::with_capacity(2 * ((to - from) / stride + 1));
    let mut index = from;
    while index < to {
        let stop = (index + stride).min(to);
        let mut lo: i8 = 0;
        let mut hi: i8 = 0;
        for pair in index..stop {
            lo = lo.min(all[pair * 2]);
            hi = hi.max(all[pair * 2 + 1]);
        }
        peaks.push(lo);
        peaks.push(hi);
        index = stop;
    }
    Preview {
        start: from as f64 * peak,
        peak_seconds: peak * stride as f64,
        peaks,
    }
}

// ============================================================
// Files
// ============================================================

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

/// The recording's identity: the hash of its file name. Two files cannot share
/// a name in one directory, so this is unique within a dataset and survives a
/// re-pick that keeps the names — which is exactly when the cache should survive
/// too.
fn source_uuid(file_name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(file_name.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn fingerprint(path: &Path) -> (u64, u64) {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return (0, 0),
    };
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    (meta.len(), modified_ms)
}

/// Order file names the way a person reads them: `ch2` before `ch10`.
///
/// Digit runs compare as numbers (leading zeros ignored), everything else
/// case-insensitively, so `Chapter 10.mp3` sorts after `chapter 9.mp3` whether or
/// not the ripper zero-padded the numbers.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let start_a = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let start_b = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let digits_a = trim_leading_zeros(&a[start_a..i]);
            let digits_b = trim_leading_zeros(&b[start_b..j]);
            let order = digits_a.len().cmp(&digits_b.len()).then_with(|| digits_a.cmp(digits_b));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let left = a[i].to_ascii_lowercase();
            let right = b[j].to_ascii_lowercase();
            if left != right {
                return left.cmp(&right);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim_leading_zeros(digits: &[u8]) -> &[u8] {
    let mut from = 0;
    while from + 1 < digits.len() && digits[from] == b'0' {
        from += 1;
    }
    &digits[from..]
}

// ============================================================
// Register
// ============================================================

/// Scan `directory` for recordings and record them in playing order.
///
/// A file whose size or mtime changed drops its cached transcript: the bytes it
/// described are gone, and keeping a stale transcript would put every span in
/// that file at the wrong second. Omitted `directory` lists what is already
/// registered, so the panel can render without re-picking.
pub(crate) fn register(
    settings: &SettingsState,
    dataset_uuid: &str,
    directory: Option<&str>,
) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let mut index = load_index(&dir);

    if let Some(given) = directory.map(str::trim).filter(|s| !s.is_empty()) {
        let root = Path::new(given);
        let entries = fs::read_dir(root).map_err(|e| format!("Cannot read {}: {}", root.display(), e))?;
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_audio_file(path))
            .collect();
        if files.is_empty() {
            return Err(format!(
                "{} holds no audio files (looked for {}).",
                root.display(),
                AUDIO_EXTENSIONS.join(", ")
            ));
        }
        files.sort_by(|a, b| natural_cmp(&file_name_of(a), &file_name_of(b)));

        let mut sources = Vec::with_capacity(files.len());
        for path in files {
            let file_name = file_name_of(&path);
            let uuid = source_uuid(&file_name);
            let (size, modified_ms) = fingerprint(&path);
            // Only a file that is byte-for-byte the one transcribed keeps its
            // cache; anything else had its cache dropped just above.
            let previous = index
                .sources
                .iter()
                .find(|s| s.uuid == uuid && s.size == size && s.modified_ms == modified_ms && s.words.is_some());
            if previous.is_none() {
                let _ = fs::remove_file(transcript_path(&dir, &uuid));
            }
            sources.push(SourceEntry {
                duration: previous.and_then(|s| s.duration),
                words: previous.and_then(|s| s.words),
                uuid,
                file_name,
                path: path.to_string_lossy().into_owned(),
                size,
                modified_ms,
            });
        }
        index.directory = root.to_string_lossy().into_owned();
        index.sources = sources;
    }

    save_index(&dir, &index)?;
    Ok(index_view(&index))
}

fn index_view(index: &SourceIndex) -> Value {
    let transcribed = index.sources.iter().filter(|s| s.words.is_some()).count();
    json!({
        "status": "ok",
        "directory": index.directory,
        "sources": index.sources,
        "source_count": index.sources.len(),
        "transcribed": transcribed,
        "pending_transcription": index.sources.len() - transcribed,
    })
}

// ============================================================
// Transcribe (one file per call)
// ============================================================

/// Cache one recording's word timings and peak envelope.
///
/// One file per call on purpose: a chapter is minutes of model time, and no
/// caller — the panel or an agent — waits that long for a batch. The index is
/// saved after the file, so a caller that keeps calling until `remaining` is 0
/// resumes instead of starting over.
#[cfg(feature = "stt")]
pub(crate) fn transcribe(
    model_state: &crate::models::ModelState,
    settings: &SettingsState,
    dataset_uuid: &str,
    source_uuid: Option<&str>,
) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let mut index = load_index(&dir);
    if index.sources.is_empty() {
        return Err("No recordings are registered: pick the folder that holds them first.".to_string());
    }

    let position = match source_uuid.map(str::trim).filter(|s| !s.is_empty()) {
        Some(wanted) => index
            .sources
            .iter()
            .position(|s| s.uuid == wanted)
            .ok_or_else(|| format!("Recording {} is not registered in this dataset", wanted))?,
        None => match index.sources.iter().position(|s| s.words.is_none()) {
            Some(position) => position,
            None => {
                // Nothing left to do: answer in the same shape as a real pass,
                // so the caller can read `sources` without a special case.
                let mut out = index_view(&index);
                if let Value::Object(map) = &mut out {
                    map.insert("done".into(), json!(true));
                    map.insert("remaining".into(), json!(0));
                }
                return Ok(out);
            }
        },
    };

    let entry = index.sources[position].clone();
    let path = PathBuf::from(&entry.path);
    if !path.is_file() {
        return Err(format!(
            "{} is no longer at {}. Re-pick the folder to register it again.",
            entry.file_name, entry.path
        ));
    }

    log::info!("[ReadAloud] transcribing source {} ({})", entry.file_name, entry.path);
    let samples = audio::decode_to_pcm_capped(&path, MAX_SOURCE_SECS)?;
    if samples.is_empty() {
        return Err(format!("{} decoded to no audio", entry.file_name));
    }
    let duration = samples.len() as f64 / audio::DECODED_SAMPLE_RATE as f64;
    let (peaks, peak_seconds) = envelope(&samples);
    let heard = crate::models::transcribe_samples_word_level(model_state, &samples)
        .map_err(super::standard_audio::guess_error)?;

    let mut words: Vec<SpokenWord> = Vec::new();
    for word in heard.segments.unwrap_or_default() {
        let tokens = asr_metric::normalize_tokens(&word.text);
        if tokens.is_empty() {
            continue;
        }
        words.push(SpokenWord {
            start: word.start as f64,
            end: word.end as f64,
            tokens,
        });
    }
    if words.is_empty() {
        return Err(format!(
            "The model heard no words in {}. Check that the file holds speech, then try again.",
            entry.file_name
        ));
    }

    let transcript = SourceTranscript {
        uuid: entry.uuid.clone(),
        file_name: entry.file_name.clone(),
        duration,
        peak_seconds,
        peaks: encode_peaks(&peaks),
        words,
    };
    write_json(&transcript_path(&dir, &entry.uuid), &transcript)?;
    index.sources[position].duration = Some(duration);
    index.sources[position].words = Some(transcript.words.len());
    save_index(&dir, &index)?;

    let pending = index.sources.iter().filter(|s| s.words.is_none()).count();
    let mut out = index_view(&index);
    if let Value::Object(map) = &mut out {
        map.insert("done".into(), json!(pending == 0));
        map.insert("remaining".into(), json!(pending));
        map.insert("source_uuid".into(), json!(entry.uuid));
        map.insert("file_name".into(), json!(entry.file_name));
        map.insert("duration".into(), json!(duration));
        map.insert("words".into(), json!(transcript.words.len()));
    }
    Ok(out)
}

/// Without the `stt` feature there is no model to listen with.
#[cfg(not(feature = "stt"))]
pub(crate) fn transcribe(
    _settings: &SettingsState,
    _dataset_uuid: &str,
    _source_uuid: Option<&str>,
) -> Result<Value, String> {
    Err("This build has no speech recognition, so the recordings cannot be transcribed.".to_string())
}

// ============================================================
// Align
// ============================================================

/// A stretch of the stream that matches a passage.
struct Found {
    /// Absolute index of the run's first and last token in the stream.
    from: usize,
    to: usize,
    matched: usize,
}

/// Search forward from `cursor` for the passage's reading.
///
/// The window starts small and grows, because most passages are found a file
/// away from where the cursor stands and aligning a 400-token window costs a
/// fraction of aligning the whole book. A run that reaches the window's last
/// token is *truncated*: the reading may well continue past it, so widening is
/// the right move and refusing would be wrong. When nothing satisfies `need`,
/// the best run seen is returned anyway, so a refusal can say how much was heard.
fn find_run(reference: &[String], stream: &[String], cursor: usize, need: usize) -> Option<Found> {
    if cursor >= stream.len() || reference.is_empty() {
        return None;
    }
    let mut window = reference.len().saturating_mul(SEARCH_GROWTH).max(SEARCH_INITIAL_TOKENS);
    let mut best: Option<Found> = None;
    loop {
        let last_attempt =
            cursor + window >= stream.len() || window >= reference.len().saturating_mul(SEARCH_MAX_FACTOR);
        let end = if last_attempt { stream.len() } else { cursor + window };
        let slice = &stream[cursor..end];
        let pairs = asr_metric::aligned_pairs(reference, slice);
        let run = densest_run(&pairs);
        if !run.is_empty() {
            let from = cursor + run[0].1;
            let to = cursor + run[run.len() - 1].1;
            let truncated = end < stream.len() && run[run.len() - 1].1 + 1 >= slice.len();
            let found = Found {
                from,
                to,
                matched: run.len(),
            };
            if !truncated && found.matched >= need {
                return Some(found);
            }
            best = Some(found);
        }
        if last_attempt {
            return best;
        }
        window = window.saturating_mul(SEARCH_GROWTH);
    }
}

/// The padded span of a passage, before the silence snap: `[start, end)` in
/// seconds, never overlapping the passage before it and never shorter than
/// [`MIN_SPAN_SECS`].
fn span_edges(first_start: f64, last_end: f64, duration: f64, previous_end: Option<f64>) -> (f64, f64) {
    let mut start = (first_start - GUESS_PAD_BEFORE).max(previous_end.unwrap_or(0.0));
    let mut end = (last_end + GUESS_PAD_AFTER).min(duration);
    if end - start < MIN_SPAN_SECS {
        end = (start + MIN_SPAN_SECS).min(duration);
        start = (end - MIN_SPAN_SECS).max(previous_end.unwrap_or(0.0));
    }
    (start.max(0.0), end.max(0.0))
}

fn round_ms(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

/// Walk every passage along one token stream and write the proposals.
///
/// The cursor only moves forward, which is the whole trick: it is *known* the
/// texts are the recordings read in order, so a passage cannot be inside an
/// earlier one, and the search never has to consider the part of the book it has
/// already assigned. Re-running is therefore idempotent — same stream, same
/// cursor path, same spans — except where a transcript changed under it.
#[cfg(feature = "stt")]
pub(crate) fn run(settings: &SettingsState, dataset_uuid: &str) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let index = load_index(&dir);
    if index.sources.is_empty() {
        return Err("No recordings are registered: pick the folder that holds them first.".to_string());
    }

    let transcripts: Vec<Option<SourceTranscript>> = index
        .sources
        .iter()
        .map(|source| read_json(&transcript_path(&dir, &source.uuid)))
        .collect();
    if transcripts.iter().all(Option::is_none) {
        return Err(
            "None of the registered recordings has been transcribed yet. Transcribe at least one, then guess."
                .to_string(),
        );
    }

    // One token stream across every recording, in playing order.
    let mut stream: Vec<String> = Vec::new();
    let mut at: Vec<(usize, f64, f64)> = Vec::new();
    for (position, transcript) in transcripts.iter().enumerate() {
        let Some(transcript) = transcript else { continue };
        for word in &transcript.words {
            for token in &word.tokens {
                stream.push(token.clone());
                at.push((position, word.start, word.end));
            }
        }
    }
    if stream.is_empty() {
        return Err("The cached transcripts hold no words to align against.".to_string());
    }

    let texts: Vec<(String, String)> = {
        let conn = open_db(&dir)?;
        let mut stmt = conn
            .prepare("SELECT uuid, content, normalized_content FROM read_text ORDER BY order_num, rowid")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.flatten()
            .map(|(uuid, content, normalized)| {
                (uuid, super::effective_reference(&content, normalized.as_deref()))
            })
            .collect()
    };
    if texts.is_empty() {
        return Err("This dataset has no texts to place.".to_string());
    }

    let previous = load_proposals(&dir);
    let mut proposals: Vec<Proposal> = Vec::with_capacity(texts.len());
    let mut cursor = 0usize;
    let mut previous_end: Option<(usize, f64)> = None;

    for (text_uuid, reference) in &texts {
        let tokens = asr_metric::normalize_tokens(reference);
        if tokens.is_empty() {
            proposals.push(Proposal::refusal(text_uuid, "That text has no words to look for.".to_string()));
            continue;
        }
        let need = (((tokens.len() as f64) * GUESS_MIN_MATCH_RATIO).ceil() as usize)
            .max(GUESS_MIN_WORDS.min(tokens.len()));

        let Some(found) = find_run(&tokens, &stream, cursor, need) else {
            proposals.push(Proposal::refusal(
                text_uuid,
                "There is no recording left after the previous passage, so this one was not looked for. Check the file order and length.".to_string(),
            ));
            continue;
        };

        if found.matched < need {
            let mut refusal = Proposal::refusal(
                text_uuid,
                format!(
                    "The model heard only {} of this text's {} words in the recordings, which is too little to place the reading. Try a stronger model, or set the edges by hand.",
                    found.matched,
                    tokens.len()
                ),
            );
            refusal.matched_words = found.matched;
            refusal.reference_words = tokens.len();
            proposals.push(refusal);
            cursor = found.to + 1;
            continue;
        }

        let (first_source, first_start, _) = at[found.from];
        let (last_source, _, last_end) = at[found.to];
        if first_source != last_source {
            let mut refusal = Proposal::refusal(
                text_uuid,
                format!(
                    "This passage is read across the boundary between {} and {}. Crop it by hand from the two files.",
                    index.sources[first_source].file_name, index.sources[last_source].file_name
                ),
            );
            refusal.matched_words = found.matched;
            refusal.reference_words = tokens.len();
            proposals.push(refusal);
            cursor = found.to + 1;
            previous_end = Some((last_source, last_end));
            continue;
        }

        let Some(transcript) = transcripts[first_source].as_ref() else {
            proposals.push(Proposal::refusal(
                text_uuid,
                "The recording this passage matched has no cached transcript.".to_string(),
            ));
            continue;
        };
        let clamped = previous_end.filter(|(source, _)| *source == first_source).map(|(_, end)| end);
        let (start, end) = span_edges(first_start, last_end, transcript.duration, clamped);
        let peaks = decode_peaks(&transcript.peaks);
        let start = round_ms(snap_quiet(&peaks, transcript.peak_seconds, start, 0.0, end - MIN_SPAN_SECS));
        let end = round_ms(snap_quiet(
            &peaks,
            transcript.peak_seconds,
            end,
            start + MIN_SPAN_SECS,
            transcript.duration,
        ));
        let confidence = (found.matched as f64 / tokens.len() as f64 * 100.0).round() / 100.0;

        // A span that did not move keeps the verdict it already had, so a
        // re-run after one corrected transcript does not throw away forty
        // acceptances; a span that moved is a new question and goes back to
        // pending.
        let carried = previous
            .proposals
            .iter()
            .find(|p| {
                p.text_uuid == *text_uuid
                    && p.status == "proposed"
                    && p.source_uuid.as_deref() == Some(index.sources[first_source].uuid.as_str())
                    && (p.start - start).abs() <= 0.05
                    && (p.end - end).abs() <= 0.05
            })
            .map(|p| (p.state.clone(), p.applied_uuid.clone()))
            .unwrap_or_else(|| ("pending".to_string(), None));

        proposals.push(Proposal {
            text_uuid: text_uuid.clone(),
            source_uuid: Some(index.sources[first_source].uuid.clone()),
            source_name: transcript.file_name.clone(),
            start,
            end,
            confidence,
            matched_words: found.matched,
            reference_words: tokens.len(),
            status: "proposed".to_string(),
            note: if confidence >= 0.95 {
                String::new()
            } else {
                format!(
                    "The model heard {} of {} words; check both edges.",
                    found.matched,
                    tokens.len()
                )
            },
            state: carried.0,
            applied_uuid: carried.1,
        });
        previous_end = Some((first_source, end));
        cursor = found.to + 1;
    }

    let set = ProposalSet {
        generated_at: crate::datasets::now_row_stamp(),
        proposals,
    };
    write_json(&proposals_path(&dir), &set)?;

    let proposed = set.proposals.iter().filter(|p| p.status == "proposed").count();
    let refused = set.proposals.len() - proposed;
    let mean_confidence = if proposed == 0 {
        0.0
    } else {
        let total: f64 = set
            .proposals
            .iter()
            .filter(|p| p.status == "proposed")
            .map(|p| p.confidence)
            .sum();
        (total / proposed as f64 * 100.0).round() / 100.0
    };
    Ok(json!({
        "status": "ok",
        "passages": set.proposals.len(),
        "proposed": proposed,
        "refused": refused,
        "mean_confidence": mean_confidence,
        "unused_words": stream.len().saturating_sub(cursor),
        "sources": index.sources.len(),
        "generated_at": set.generated_at,
    }))
}

#[cfg(not(feature = "stt"))]
pub(crate) fn run(_settings: &SettingsState, _dataset_uuid: &str) -> Result<Value, String> {
    Err("This build has no speech recognition, so the spans cannot be guessed.".to_string())
}

// ============================================================
// Review
// ============================================================

/// One proposal as the panel and an agent see it: the span, the passage it is
/// for, and a thumbnail of the audio around it.
#[derive(Serialize)]
pub struct ProposalView {
    pub text_uuid: String,
    pub title: String,
    pub content: String,
    pub status: String,
    pub state: String,
    pub note: String,
    pub source_uuid: Option<String>,
    pub source_name: String,
    /// Absolute path of the recording, for playback.
    pub source_path: String,
    pub start: f64,
    pub end: f64,
    pub confidence: f64,
    pub matched_words: usize,
    pub reference_words: usize,
    pub applied_uuid: Option<String>,
    /// `None` when the span cannot be previewed — a refusal, or a recording
    /// whose cache is gone on this device.
    pub preview: Option<Preview>,
}

/// The proposals, joined with the texts they are for.
pub(crate) fn list(settings: &SettingsState, dataset_uuid: &str) -> Result<Value, String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let set = load_proposals(&dir);
    let index = load_index(&dir);

    let (titles, passages) = {
        let conn = open_db(&dir)?;
        let mut stmt = conn
            .prepare("SELECT uuid, title, content FROM read_text")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let pairs: Vec<(String, String, String)> = rows.flatten().collect();
        let total = pairs.len();
        (
            pairs
                .into_iter()
                .map(|(uuid, title, content)| (uuid, (title, content)))
                .collect::<HashMap<_, _>>(),
            total,
        )
    };

    // One envelope decode per recording, not per passage: a ten-hour book is a
    // few megabytes of base64 and forty passages would decode it forty times.
    let mut cache: HashMap<String, SourceTranscript> = HashMap::new();
    let mut views: Vec<ProposalView> = Vec::with_capacity(set.proposals.len());
    for proposal in &set.proposals {
        let (title, content) = titles.get(&proposal.text_uuid).cloned().unwrap_or_default();
        let source = index
            .sources
            .iter()
            .find(|s| Some(s.uuid.as_str()) == proposal.source_uuid.as_deref());
        let preview = if proposal.status == "proposed" {
            source.and_then(|source| {
                let transcript = cache.entry(source.uuid.clone()).or_insert_with(|| {
                    read_json(&transcript_path(&dir, &source.uuid)).unwrap_or(SourceTranscript {
                        uuid: source.uuid.clone(),
                        file_name: source.file_name.clone(),
                        duration: source.duration.unwrap_or(0.0),
                        peak_seconds: PEAK_SECONDS,
                        peaks: String::new(),
                        words: Vec::new(),
                    })
                });
                (transcript.duration > 0.0).then(|| preview(transcript, proposal.start, proposal.end))
            })
        } else {
            None
        };
        views.push(ProposalView {
            text_uuid: proposal.text_uuid.clone(),
            title,
            content,
            status: proposal.status.clone(),
            state: proposal.state.clone(),
            note: proposal.note.clone(),
            source_uuid: proposal.source_uuid.clone(),
            source_name: proposal.source_name.clone(),
            source_path: source.map(|s| s.path.clone()).unwrap_or_default(),
            start: proposal.start,
            end: proposal.end,
            confidence: proposal.confidence,
            matched_words: proposal.matched_words,
            reference_words: proposal.reference_words,
            applied_uuid: proposal.applied_uuid.clone(),
            preview,
        });
    }

    Ok(json!({
        "status": "ok",
        "generated_at": set.generated_at,
        "proposals": views,
        "passages_total": passages,
        "counts": counts(&set),
    }))
}

fn counts(set: &ProposalSet) -> Value {
    let proposed = set.proposals.iter().filter(|p| p.status == "proposed").count();
    json!({
        "proposed": proposed,
        "refused": set.proposals.len() - proposed,
        "accepted": set.proposals.iter().filter(|p| p.state == "accepted").count(),
        "rejected": set.proposals.iter().filter(|p| p.state == "rejected").count(),
        "applied": set.proposals.iter().filter(|p| p.applied_uuid.is_some()).count(),
        "open": set.proposals.iter().filter(|p| p.is_open()).count(),
    })
}

/// Accept or reject one proposal, or every proposal at or above a confidence.
///
/// Accepting is a verdict, not a write: nothing reaches `media/` until `apply`,
/// so a batch can be accepted, looked over and narrowed before any crop is made.
///
/// `start`/`end` move one passage's span (the panel's 10 ms nudges); they need a
/// `text_uuid` and must be given together. A moved span is *not* re-cut on its
/// own — the verdict it carries is what `apply` reads — so moving one on a
/// passage that was already cut is refused rather than silently duplicated: the
/// clip that already exists would no longer be the span on the row.
pub(crate) fn decide(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: Option<&str>,
    state: &str,
    min_confidence: Option<f64>,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<Value, String> {
    let wanted = match state.trim() {
        "accepted" => "accepted",
        "rejected" => "rejected",
        "pending" => "pending",
        other => {
            return Err(format!(
                "Unknown state '{}': use accepted, rejected or pending.",
                other
            ))
        }
    };
    let one = text_uuid.map(str::trim).filter(|s| !s.is_empty());
    let span = match (start, end) {
        (None, None) => None,
        (Some(start), Some(end)) => {
            if one.is_none() {
                return Err("Moving a span needs a text_uuid.".to_string());
            }
            if !start.is_finite() || !end.is_finite() || start < 0.0 {
                return Err("The span must be two finite times after 0.".to_string());
            }
            let (start, end) = (round_ms(start), round_ms(end));
            if end - start < MIN_SPAN_SECS {
                return Err(format!(
                    "The span is {:.3} s long; it must be at least {:.1} s.",
                    end - start,
                    MIN_SPAN_SECS
                ));
            }
            Some((start, end))
        }
        _ => return Err("Pass both start and end, or neither.".to_string()),
    };

    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let mut set = load_proposals(&dir);
    if set.proposals.is_empty() {
        return Err("There are no proposals yet — run the guess first.".to_string());
    }

    let mut changed = 0usize;
    match one {
        Some(uuid) => {
            let Some(proposal) = set.proposals.iter_mut().find(|p| p.text_uuid == uuid) else {
                return Err(format!("No proposal for text {}", uuid));
            };
            if proposal.status != "proposed" {
                return Err(format!(
                    "That passage has no span to accept: {}",
                    if proposal.note.is_empty() {
                        "the guess refused it"
                    } else {
                        proposal.note.as_str()
                    }
                ));
            }
            if let Some((start, end)) = span {
                let moved = (proposal.start - start).abs() > 1e-9 || (proposal.end - end).abs() > 1e-9;
                if moved && proposal.applied_uuid.is_some() {
                    return Err(
                        "That span was already cut into the dataset. Re-run the guess and apply again if it is wrong."
                            .to_string(),
                    );
                }
                if moved {
                    proposal.start = start;
                    proposal.end = end;
                    changed = 1;
                }
            }
            if proposal.state != wanted {
                proposal.state = wanted.to_string();
                changed = 1;
            }
        }
        None => {
            let Some(threshold) = min_confidence else {
                return Err("Pass a text_uuid, or a min_confidence to decide the whole batch.".to_string());
            };
            for proposal in set.proposals.iter_mut().filter(|p| p.status == "proposed") {
                if proposal.confidence + f64::EPSILON >= threshold && proposal.state != wanted {
                    proposal.state = wanted.to_string();
                    changed += 1;
                }
            }
        }
    }

    write_json(&proposals_path(&dir), &set)?;
    Ok(json!({
        "status": "ok",
        "changed": changed,
        "state": wanted,
        "counts": counts(&set),
    }))
}

// ============================================================
// Apply
// ============================================================

/// Cut every accepted span out of its recording and store it as the text's
/// standard audio.
///
/// Each recording is decoded **once** however many spans come out of it, which
/// is the whole reason this is a batch call and not a loop of imports. `limit`
/// bounds the call (`generate_all`'s pattern): an agent repeats until `remaining`
/// is 0 rather than holding a request open for an hour.
pub(crate) fn apply(settings: &SettingsState, dataset_uuid: &str, limit: Option<i64>) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let index = load_index(&dir);
    let mut set = load_proposals(&dir);
    let limit = limit.unwrap_or(10).clamp(1, 200) as usize;

    let planned: Vec<usize> = set
        .proposals
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_open())
        .map(|(i, _)| i)
        .take(limit)
        .collect();
    if planned.is_empty() {
        return Ok(json!({
            "status": "ok",
            "applied": 0,
            "failed": [],
            "failed_count": 0,
            "remaining": 0,
        }));
    }

    let mut applied = 0usize;
    let mut failed: Vec<Value> = Vec::new();
    for source in &index.sources {
        let targets: Vec<usize> = planned
            .iter()
            .copied()
            .filter(|i| set.proposals[*i].source_uuid.as_deref() == Some(source.uuid.as_str()))
            .collect();
        if targets.is_empty() {
            continue;
        }
        let samples = match audio::decode_to_pcm_capped(Path::new(&source.path), MAX_SOURCE_SECS) {
            Ok(samples) => samples,
            Err(error) => {
                for i in targets {
                    failed.push(json!({
                        "text_uuid": set.proposals[i].text_uuid,
                        "error": error,
                    }));
                }
                continue;
            }
        };
        let rate = audio::DECODED_SAMPLE_RATE as f64;
        for i in targets {
            let proposal = set.proposals[i].clone();
            let reference = match text_reference(&dir, &proposal.text_uuid) {
                Ok(reference) => reference,
                Err(error) => {
                    failed.push(json!({ "text_uuid": proposal.text_uuid, "error": error }));
                    continue;
                }
            };
            let first = ((proposal.start * rate).round() as usize).min(samples.len());
            let last = ((proposal.end * rate).round() as usize).min(samples.len());
            if last <= first {
                failed.push(json!({
                    "text_uuid": proposal.text_uuid,
                    "error": "The span falls outside the recording; re-run the guess.",
                }));
                continue;
            }
            let note = format!(
                "Align · {} · {:.2}–{:.2} s",
                proposal.source_name, proposal.start, proposal.end
            );
            match super::standard_audio::store_samples(
                settings,
                dataset_uuid,
                &dir,
                &proposal.text_uuid,
                &reference,
                &samples[first..last],
                &note,
            ) {
                Ok(clip) => {
                    set.proposals[i].applied_uuid = Some(clip.uuid);
                    applied += 1;
                }
                Err(error) => failed.push(json!({
                    "text_uuid": proposal.text_uuid,
                    "error": error,
                })),
            }
        }
        // Save per recording, so a long apply that is interrupted keeps what it
        // already stored instead of doing it again.
        write_json(&proposals_path(&dir), &set)?;
    }

    let remaining = set.proposals.iter().filter(|p| p.is_open()).count();
    let failed_count = failed.len();
    Ok(json!({
        "status": "ok",
        "applied": applied,
        "failed": failed,
        "failed_count": failed_count,
        "remaining": remaining,
        "counts": counts(&set),
    }))
}

// ============================================================
// Commands
// ============================================================

/// Register (or list) the recordings that hold this dataset's passages.
#[tauri::command]
pub async fn read_aloud_align_sources(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    directory: Option<String>,
) -> Result<Value, String> {
    register(&settings, &dataset_uuid, directory.as_deref())
}

/// Cache the word timings of one recording. Repeat until `remaining` is 0.
#[cfg(feature = "stt")]
#[tauri::command]
pub async fn read_aloud_align_transcribe(
    settings: State<'_, SettingsState>,
    model_state: State<'_, crate::models::ModelState>,
    dataset_uuid: String,
    source_uuid: Option<String>,
) -> Result<Value, String> {
    transcribe(
        model_state.inner(),
        &settings,
        &dataset_uuid,
        source_uuid.as_deref(),
    )
}

#[cfg(not(feature = "stt"))]
#[tauri::command]
pub async fn read_aloud_align_transcribe(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    source_uuid: Option<String>,
) -> Result<Value, String> {
    transcribe(&settings, &dataset_uuid, source_uuid.as_deref())
}

/// Place every passage along the recordings, in order, and write the proposals.
#[tauri::command]
pub async fn read_aloud_align_guess(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Value, String> {
    run(&settings, &dataset_uuid)
}

/// The proposals with their texts and a thumbnail of the audio around each span.
#[tauri::command]
pub async fn read_aloud_align_proposals(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Value, String> {
    list(&settings, &dataset_uuid)
}

/// Accept or reject one proposal, or every proposal at or above a confidence.
/// `start`/`end` move one passage's span by hand.
#[tauri::command]
pub async fn read_aloud_align_decide(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: Option<String>,
    state: String,
    min_confidence: Option<f64>,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<Value, String> {
    decide(
        &settings,
        &dataset_uuid,
        text_uuid.as_deref(),
        &state,
        min_confidence,
        start,
        end,
    )
}

/// Cut the accepted spans into standard audio, one decode per recording.
#[tauri::command]
pub async fn read_aloud_align_apply(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    limit: Option<i64>,
) -> Result<Value, String> {
    apply(&settings, &dataset_uuid, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chapters ripped with and without zero padding have to agree: a person
    /// reads `ch2` before `ch10`, and so must the stream.
    #[test]
    fn file_names_sort_the_way_they_are_read() {
        let mut names = vec!["ch10.mp3", "ch2.mp3", "Chapter 1.mp3", "ch1.mp3", "ch02.mp3"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec!["ch1.mp3", "ch2.mp3", "ch02.mp3", "ch10.mp3", "Chapter 1.mp3"]
        );
    }

    /// The envelope is stored as base64 of raw `i8`; a round trip must not
    /// change a single bucket, -128 included.
    #[test]
    fn peaks_survive_the_round_trip() {
        let peaks = vec![-128i8, 127, 0, 0, -1, 1, 5, -5];
        assert_eq!(decode_peaks(&encode_peaks(&peaks)), peaks);
    }

    /// An edge moves to the quietest 20 ms bucket nearby, not to the model's
    /// word boundary, which can sit inside a consonant.
    #[test]
    fn edges_snap_to_the_quietest_bucket() {
        let mut peaks = vec![100i8; 40];
        peaks[10] = 0; // bucket 5, both halves quiet
        peaks[11] = 0;
        let snapped = snap_quiet(&peaks, 0.02, 0.10, 0.0, 10.0);
        assert!((snapped - 0.10).abs() < 1e-9, "snapped to {snapped}");
    }

    /// The snap still respects the neighbouring edge and the recording's bounds.
    #[test]
    fn snap_respects_the_clamp() {
        let mut peaks = vec![100i8; 40];
        peaks[10] = 0;
        peaks[11] = 0;
        let snapped = snap_quiet(&peaks, 0.02, 0.10, 0.12, 10.0);
        assert!((snapped - 0.12).abs() < 1e-9, "snapped to {snapped}");
    }

    /// Padding, the minimum length, and the monotonic clamp — the three ways a
    /// span can be the wrong size.
    #[test]
    fn span_edges_pad_clamp_and_never_overlap() {
        let (start, end) = span_edges(10.0, 12.0, 100.0, None);
        assert!((start - 9.8).abs() < 1e-9);
        assert!((end - 12.3).abs() < 1e-9);

        let (start, _) = span_edges(10.0, 12.0, 100.0, Some(10.5));
        assert!((start - 10.5).abs() < 1e-9, "start {start} overlaps the previous passage");

        let (start, end) = span_edges(1.0, 99.95, 100.0, None);
        assert!(end <= 100.0 && start < end);

        let (start, end) = span_edges(0.0, 0.0, 100.0, None);
        assert!(end - start >= MIN_SPAN_SECS - 1e-9, "{start}..{end} is too short");
    }

    /// Milli precision, so a typed 12.345 s is stored as 12.345 s.
    #[test]
    fn spans_round_to_milliseconds() {
        assert!((round_ms(12.3454) - 12.345).abs() < 1e-9);
        assert!((round_ms(12.3456) - 12.346).abs() < 1e-9);
    }

    /// A passage found inside the first window is returned in absolute stream
    /// positions; a passage past it makes the search widen instead.
    #[test]
    fn the_search_widens_until_the_passage_is_found() {
        let reference: Vec<String> = (0..101).map(|i| format!("w{}", i)).collect();
        let mut stream: Vec<String> = (0..600).map(|i| format!("x{}", i)).collect();
        for (offset, token) in reference.iter().enumerate() {
            stream[500 + offset] = token.clone();
        }
        let found = find_run(&reference, &stream, 0, reference.len()).expect("the passage is there");
        assert_eq!(found.from, 500);
        assert_eq!(found.to, 600);
        assert_eq!(found.matched, 101);
    }

    /// The cursor is what keeps the pass monotonic: a search that starts after
    /// a passage's words can never find it again.
    #[test]
    fn the_search_never_looks_backwards() {
        let reference: Vec<String> = (0..10).map(|i| format!("w{}", i)).collect();
        let stream: Vec<String> = reference.clone();
        assert!(find_run(&reference, &stream, 0, 10).is_some());
        assert!(find_run(&reference, &stream, 10, 10).is_none());
    }
}
