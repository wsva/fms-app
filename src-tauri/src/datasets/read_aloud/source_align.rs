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
//! This module pays for the transcription **once per file**, then places the
//! passages along one global token stream in `order_num`: first the passages long
//! enough to be found on their own, then everything in the gaps those leave. Each
//! passage yields a span, the spans are written down as *proposals*, a human
//! accepts or rejects them, and only then are the clips cut.
//!
//! ## Why two passes and not one walk
//!
//! The first version walked the passages with a cursor that only moved forward
//! and aligned each one against a window growing from that cursor. On a real
//! audiobook (784 mails, 34 files, 45 000 heard words) it placed five of them.
//! Three things were wrong, and each one on its own is enough to lose a dataset:
//!
//! * the alignment was **global**, so a fifteen-word mail aligned against a
//!   four-minute window had to delete three quarters of the window, and paid for
//!   it with the words it should have matched — 7 matches where 11 were heard;
//! * a wider window made that *worse*, but the wider answer overwrote the
//!   narrower one, so refusing was the only way out of the loop;
//! * one passage placed in the wrong place moved the cursor past the passages
//!   that came after it, and everything behind the cursor refused. 500 of the
//!   784 refusals in that run were one bad span in the first third of the book.
//!
//! So the guess no longer trusts a cursor. It trusts **reading order**: a text's
//! position is predicted from the words counted before it ([`prior_position`]),
//! a semi-global alignment asks "where in this window is this text" without
//! paying for the rest of the window, and every passage's span is decided
//! against that prediction rather than against where its neighbour happened to
//! land. The passages too short to be evidence on their own are then aligned as
//! one block inside the gap their neighbours leave, which cannot put them out of
//! order even when the phrase they consist of occurs twenty times in the book.
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
    text_reference, GUESS_MAX_GAP, GUESS_MIN_MATCH_RATIO, GUESS_MIN_WORDS, GUESS_PAD_AFTER,
    GUESS_PAD_BEFORE,
};
use super::{edges, find_dataset_dir, find_dataset_dir_write, open_db};

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

/// How far from its predicted position a passage is looked for. The tight value
/// is the common case and also what keeps a five-word one-liner from being
/// matched to the same phrase twenty minutes later; the wide one is the retry
/// for a passage the prediction put more than a page away.
const PRIOR_SLACK_TOKENS: usize = 400;
const PRIOR_WIDE_SLACK_TOKENS: usize = 1500;

/// Longest window one semi-global pass aligns. The matrix costs a byte per cell,
/// so a text of several hundred words must not be allowed to ask for a window
/// the size of the whole book.
const MAX_WINDOW_TOKENS: usize = 12_000;

/// A text with fewer words than this is not evidence of anything by itself:
/// "40 Sekunden später" is read a dozen times in a 20-hour book, so aligning it
/// alone finds *an* occurrence, not *the* one. Those are placed as a block
/// inside the gap their longer neighbours leave.
const ANCHOR_MIN_WORDS: usize = 12;

/// What a text long enough to divide a gap must still have been heard as. Below
/// this it is a hint at a span, not a fact about where the book is.
const ANCHOR_MIN_MATCH_RATIO: f64 = 0.7;

/// How many tokens of distance from the predicted position one matched word is
/// worth when two runs compete for the same passage. At 0.005 a run 400 tokens
/// nearer the prediction beats a run that merely matched two more words, which
/// is the point: both are plausible alignments, and only one is in the right
/// part of the book.
const PRIOR_DISTANCE_PENALTY: f64 = 0.005;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Found {
    /// Absolute index of the run's first and last token in the stream.
    from: usize,
    to: usize,
    matched: usize,
}

/// What the guess decided about one passage: the span worth proposing, and the
/// best stretch of the stream pass one saw — kept even when nothing is worth
/// proposing, because "the model heard 3 of your 5 words" is the difference
/// between a text that is missing and a model that misheard.
#[derive(Clone, Copy, Debug)]
struct Guess {
    span: Option<Found>,
    heard: Option<Found>,
}

/// What one search turned up: the run worth a span, and the best run seen
/// regardless, so a refusal can still say how much of the text was heard.
#[derive(Debug, Default)]
struct Search {
    hit: Option<Found>,
    best: Option<Found>,
}

/// `(first, last, matched)` of every maximal run of aligned pairs, in order.
///
/// The break rule is [`densest_run`]'s, but all the runs are kept: a passage can
/// match two places in one window, and which of the two to keep is a question
/// about where the passage *belongs*, not about which run is longer.
fn runs_of(pairs: &[(usize, usize)]) -> Vec<(usize, usize, usize)> {
    let mut runs: Vec<(usize, usize, usize)> = Vec::new();
    let mut from = 0usize;
    for index in 1..=pairs.len() {
        let breaks = index == pairs.len()
            || pairs[index].0.saturating_sub(pairs[index - 1].0) > GUESS_MAX_GAP
            || pairs[index].1.saturating_sub(pairs[index - 1].1) > GUESS_MAX_GAP;
        if breaks {
            if index > from {
                runs.push((pairs[from].1, pairs[index - 1].1, index - from));
            }
            from = index;
        }
    }
    runs
}

/// Where reading order says a passage starts, in stream tokens.
///
/// A passage's text *is* what a stretch of the recording says, so the number of
/// reference words before it maps onto the stream arithmetically: a book whose
/// 784 mails hold 44 699 words against 45 828 heard tokens settles at 1:1, and
/// every mail's expected position follows from the words counted before it. This
/// is what keeps one bad span from dragging the rest with it — the prediction
/// does not depend on where the previous passage was put, only on the order the
/// texts were written in.
fn prior_position(before: usize, total_reference: usize, total_stream: usize) -> usize {
    if total_reference == 0 || total_stream == 0 {
        return 0;
    }
    let position = (before as u128 * total_stream as u128 / total_reference as u128) as usize;
    position.min(total_stream - 1)
}

/// The score two candidate runs are compared by: words matched, less a small
/// charge for every token of distance from the predicted position.
fn run_score(found: &Found, expected: usize) -> f64 {
    found.matched as f64 - PRIOR_DISTANCE_PENALTY * found.from.abs_diff(expected) as f64
}

/// Scan one window: semi-global alignment of the whole passage, then the runs of
/// the result, keeping the run that scores best against `expected`.
fn scan_window(
    reference: &[String],
    stream: &[String],
    lo: usize,
    hi: usize,
    expected: usize,
    need: usize,
) -> Search {
    let mut search = Search::default();
    if lo >= hi || reference.is_empty() {
        return search;
    }
    let pairs = asr_metric::aligned_pairs_glocal(reference, &stream[lo..hi]);
    for (first, last, matched) in runs_of(&pairs) {
        let found = Found { from: first + lo, to: last + lo, matched };
        if matched >= need && search.hit.map_or(true, |best| run_score(&found, expected) > run_score(&best, expected)) {
            search.hit = Some(found);
        }
        if search.best.map_or(true, |best| found.matched > best.matched) {
            search.best = Some(found);
        }
    }
    search
}

/// Where one passage is read, judged against the position reading order predicts
/// for it.
///
/// Tight window first, wide second. The tight one is where nine passages in ten
/// are, and it is also what stops an eight-word one-liner from being matched to
/// the same words twenty minutes later — a match that would otherwise score
/// just as well.
fn find_predicted(reference: &[String], stream: &[String], expected: usize, need: usize) -> Search {
    let mut best: Option<Found> = None;
    for slack in [PRIOR_SLACK_TOKENS, PRIOR_WIDE_SLACK_TOKENS] {
        let lo = expected.saturating_sub(slack);
        let width = slack
            .saturating_mul(2)
            .saturating_add(reference.len().saturating_mul(3))
            .saturating_add(300)
            .min(MAX_WINDOW_TOKENS);
        let hi = lo.saturating_add(width).min(stream.len());
        let found = scan_window(reference, stream, lo, hi, expected, need);
        if found.best.map_or(false, |f| best.map_or(true, |b| f.matched > b.matched)) {
            best = found.best;
        }
        if found.hit.is_some() {
            return Search { hit: found.hit, best: found.hit.or(best) };
        }
    }
    Search { hit: None, best }
}

/// Search forward from `cursor` for the passage's reading.
///
/// The last resort, for the case the prediction cannot serve: the texts are not
/// the recordings read in order — a sample chapter added in front, half the
/// book's files untranscribed — so there is nothing to count words against and
/// the windows [`find_predicted`] builds point at unrelated audio. Walking a
/// cursor is worse than predicting (that is what [`place_all`] is for), but it
/// does find what is there in the order it was written, which beats placing
/// nothing at all.
#[cfg(feature = "stt")]
fn walk_from_cursor(stream: &[String], texts: &[Vec<String>]) -> Vec<Guess> {
    let mut slots: Vec<Guess> = vec![Guess { span: None, heard: None }; texts.len()];
    let mut cursor = 0usize;
    for (index, reference) in texts.iter().enumerate() {
        if reference.is_empty() {
            continue;
        }
        let search = find_predicted(reference, stream, cursor, need_for(reference));
        if let Some(found) = search.hit {
            slots[index] = Guess { span: Some(found), heard: Some(found) };
            cursor = found.to + 1;
        }
    }
    slots
}

fn round_ms(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

/// The reading both guesses share: the recorded material on one side, the
/// reference texts on the other.
#[cfg(feature = "stt")]
struct Prepared {
    index: SourceIndex,
    transcripts: Vec<Option<SourceTranscript>>,
    /// Every heard token, in playing order, across every transcribed file.
    stream: Vec<String>,
    /// For each stream token: the recording it came from, and its word's
    /// `[start, end)` in that recording.
    at: Vec<(usize, f64, f64)>,
    /// `(text uuid, reference text)` in `order_num` — reading order.
    texts: Vec<(String, String)>,
}

#[cfg(feature = "stt")]
impl Prepared {
    fn source_position(&self, uuid: &str) -> Option<usize> {
        self.index.sources.iter().position(|source| source.uuid == uuid)
    }

    /// Where the stream resumes after `end` of `source`.
    ///
    /// The bulk guess knows this exactly — it holds the token index it stopped
    /// on — but a one-passage run starts from a stored span, so that span's end
    /// time is the only handle it has. Landing a word early costs nothing: an
    /// unmatched prefix is skipped by the alignment.
    fn cursor_after(&self, source: usize, end: f64) -> Option<usize> {
        self.at.iter().position(|(position, _, word_end)| {
            *position > source || (*position == source && *word_end >= end - 1e-6)
        })
    }
}

/// Read the recordings and the texts a guess walks.
///
/// Every refusal about *missing* input lives here, so a single passage cannot
/// be answered differently from a bulk run: same registrations, same caches,
/// same reading order.
#[cfg(feature = "stt")]
fn prepare(dir: &Path) -> Result<Prepared, String> {
    let index = load_index(dir);
    if index.sources.is_empty() {
        return Err("No recordings are registered: pick the folder that holds them first.".to_string());
    }

    let transcripts: Vec<Option<SourceTranscript>> = index
        .sources
        .iter()
        .map(|source| read_json(&transcript_path(dir, &source.uuid)))
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
        let conn = open_db(dir)?;
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

    Ok(Prepared { index, transcripts, stream, at, texts })
}

/// How many of a text's tokens must be heard, in order, before the placement is
/// worth a span. A short text cannot be held to a percentage of itself.
#[cfg(feature = "stt")]
fn need_for(tokens: &[String]) -> usize {
    (((tokens.len() as f64) * GUESS_MIN_MATCH_RATIO).ceil() as usize)
        .max(GUESS_MIN_WORDS.min(tokens.len()))
}

/// Place every passage along the stream: anchors first, then the gaps.
///
/// Pass one asks every passage long enough to be evidence in itself where it is,
/// against the position its order predicts. What comes out is an absolute span
/// with a confidence, and the spans are forced to be increasing, so a wrong one
/// costs itself and nothing else — it cannot drag the next passage with it the
/// way a shared cursor could.
///
/// Pass two takes the gaps pass one left and aligns everything unplaced in each
/// as one block against the audio between the two surrounding anchors. That is
/// where the one-liners go: "40 Sekunden später" is eight words and occurs a
/// dozen times, so aligning it alone finds *an* occurrence — the block alignment
/// cannot put it out of order, and it can tell that there is no room here for
/// the passages that claim to be.
///
/// A passage that survives both passes unplaced keeps the best run pass one saw,
/// so a refusal can still report it heard 5 of the text's 310 words — but that
/// run is *not* a placement: it is wherever the words happened to occur, which
/// is not where the passage belongs.
#[cfg(feature = "stt")]
fn place_all(stream: &[String], texts: &[Vec<String>]) -> Vec<Guess> {
    let total_reference: usize = texts.iter().map(Vec::len).sum();
    let mut slots: Vec<Option<Found>> = vec![None; texts.len()];
    let mut heard: Vec<Option<Found>> = vec![None; texts.len()];
    let mut anchors: Vec<(usize, usize, usize)> = Vec::new();
    let mut before = 0usize;
    let mut floor = 0usize;

    for (index, reference) in texts.iter().enumerate() {
        let start = before;
        before += reference.len();
        if reference.len() < ANCHOR_MIN_WORDS {
            continue;
        }
        let need = need_for(reference);
        let expected = prior_position(start, total_reference, stream.len());
        let search = find_predicted(reference, stream, expected, need);
        heard[index] = search.hit.or(search.best);
        let Some(found) = search.hit else { continue };
        if (found.matched as f64) < ANCHOR_MIN_MATCH_RATIO * reference.len() as f64 {
            continue;
        }
        if found.from <= floor {
            continue;
        }
        slots[index] = Some(found);
        anchors.push((index, found.from, found.to));
        floor = found.to;
    }

    let mut next_text = 0usize;
    let mut next_stream = 0usize;
    for &(index, from, to) in &anchors {
        fill_gap(stream, texts, &mut slots, next_text..index, next_stream..from);
        next_text = index + 1;
        next_stream = to + 1;
    }
    fill_gap(stream, texts, &mut slots, next_text..texts.len(), next_stream..stream.len());

    slots
        .into_iter()
        .zip(heard)
        .map(|(span, heard)| Guess { span, heard })
        .collect()
}

/// Align one gap's passages as a single block.
///
/// The concatenation of their texts is aligned once against the audio between
/// the two anchors that bound the gap, which makes the answer monotone by
/// construction: no passage in a block can be placed before another one, and
/// none can claim audio that belongs to its neighbour. Where the block's own
/// alignment heard too little of a passage — a gap that does not hold it at all —
/// that passage stays unplaced instead of being stretched across the gap.
#[cfg(feature = "stt")]
fn fill_gap(
    stream: &[String],
    texts: &[Vec<String>],
    slots: &mut [Option<Found>],
    range: std::ops::Range<usize>,
    audio: std::ops::Range<usize>,
) {
    let pending: Vec<usize> = range
        .filter(|&index| !texts[index].is_empty() && slots[index].is_none())
        .collect();
    if pending.is_empty() || audio.start >= audio.end {
        return;
    }
    let start = audio.start.min(stream.len());
    let end = audio.end.min(stream.len());
    if start >= end {
        return;
    }

    let mut block: Vec<String> = Vec::new();
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    for &index in &pending {
        spans.push((index, block.len(), block.len() + texts[index].len()));
        block.extend(texts[index].iter().cloned());
    }

    let pairs = asr_metric::aligned_pairs(&block, &stream[start..end]);
    let mut cursor = 0usize;
    let mut previous_end: Option<usize> = None;
    for (index, from, to) in spans {
        while cursor < pairs.len() && pairs[cursor].0 < from {
            cursor += 1;
        }
        let first = cursor;
        while cursor < pairs.len() && pairs[cursor].0 < to {
            cursor += 1;
        }
        let matched = cursor - first;
        if matched < need_for(&texts[index]) {
            continue;
        }
        let lower = previous_end.map_or(start, |end| end + 1);
        let span = Found {
            from: (pairs[first].1 + start).max(lower),
            to: (pairs[cursor - 1].1 + start).max(lower),
            matched,
        };
        slots[index] = Some(span);
        previous_end = Some(span.to);
    }
}

/// A placed (or refused) passage, and the floor the *next* passage in reading
/// order must not start before.
#[cfg(feature = "stt")]
struct Placed {
    proposal: Proposal,
    floor: Option<(usize, f64)>,
}

/// Turn a found run into a proposal: the padded span, both edges snapped to the
/// quietest frame nearby, clamped against the passage before it, and carrying
/// the verdict a previous run gave the same span — so a re-run does not throw
/// away forty acceptances because one transcript was corrected.
#[cfg(feature = "stt")]
fn span_proposal(
    prepared: &Prepared,
    text_uuid: &str,
    tokens: &[String],
    found: &Found,
    floor: Option<(usize, f64)>,
    previous: &ProposalSet,
) -> Placed {
    let (first_source, first_start, _) = prepared.at[found.from];
    let (last_source, _, last_end) = prepared.at[found.to];
    if first_source != last_source {
        let mut refusal = Proposal::refusal(
            text_uuid,
            format!(
                "This passage is read across the boundary between {} and {}. Crop it by hand from the two files.",
                prepared.index.sources[first_source].file_name,
                prepared.index.sources[last_source].file_name
            ),
        );
        refusal.matched_words = found.matched;
        refusal.reference_words = tokens.len();
        return Placed {
            proposal: refusal,
            floor: Some((last_source, last_end)),
        };
    }

    let Some(transcript) = prepared.transcripts[first_source].as_ref() else {
        return Placed {
            proposal: Proposal::refusal(
                text_uuid,
                "The recording this passage matched has no cached transcript.".to_string(),
            ),
            floor: None,
        };
    };
    let clamped = floor
        .filter(|(source, _)| *source == first_source)
        .map(|(_, end)| end);
    // The pads are the fallback; where a pause meets the word, the edge goes
    // into the middle of it instead, so two neighbouring clips divide the
    // silence rather than leaving half of it to neither. The bounds are what
    // keeps a clip off its neighbour and off the minimum length.
    let peaks = decode_peaks(&transcript.peaks);
    let loudness = edges::loudness_from_envelope(&peaks);
    let start = round_ms(edges::place_edge(
        &loudness,
        transcript.peak_seconds,
        first_start,
        GUESS_PAD_BEFORE,
        edges::Side::Head,
        clamped.unwrap_or(0.0),
        transcript.duration,
    ));
    let mut end = round_ms(edges::place_edge(
        &loudness,
        transcript.peak_seconds,
        last_end,
        GUESS_PAD_AFTER,
        edges::Side::Tail,
        start + MIN_SPAN_SECS,
        transcript.duration,
    ));
    if end - start < MIN_SPAN_SECS {
        end = round_ms((start + MIN_SPAN_SECS).min(transcript.duration));
    }
    let confidence = (found.matched as f64 / tokens.len() as f64 * 100.0).round() / 100.0;
    let source_uuid = prepared.index.sources[first_source].uuid.clone();

    // A span that did not move keeps the verdict it already had; a span that
    // moved is a new question and goes back to pending.
    let carried = previous
        .proposals
        .iter()
        .find(|p| {
            p.text_uuid == text_uuid
                && p.status == "proposed"
                && p.source_uuid.as_deref() == Some(source_uuid.as_str())
                && (p.start - start).abs() <= 0.05
                && (p.end - end).abs() <= 0.05
        })
        .map(|p| (p.state.clone(), p.applied_uuid.clone()))
        .unwrap_or_else(|| ("pending".to_string(), None));

    Placed {
        proposal: Proposal {
            text_uuid: text_uuid.to_string(),
            source_uuid: Some(source_uuid),
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
        },
        floor: Some((first_source, end)),
    }
}

/// Place every passage and write the proposals.
///
/// Every passage is decided from its own position in reading order, so re-running
/// is idempotent — same stream, same spans — except where a transcript changed
/// under it, and one surprising span costs only itself.
#[cfg(feature = "stt")]
pub(crate) fn run(settings: &SettingsState, dataset_uuid: &str) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let prepared = prepare(&dir)?;
    let previous = load_proposals(&dir);
    let texts: Vec<Vec<String>> = prepared
        .texts
        .iter()
        .map(|(_, reference)| asr_metric::normalize_tokens(reference))
        .collect();
    // Reading order is the model the guess is built on, and when it does not
    // hold — half the files were never transcribed, the texts were reordered
    // by hand — the predicted windows point at unrelated audio and everything
    // refuses. A quarter placed is the line where that is the explanation
    // rather than a hard book, and a cursor walk that finds what is there in
    // the order it was written beats placing nothing.
    let mut found = place_all(&prepared.stream, &texts);
    let placed = found.iter().filter(|guess| guess.span.is_some()).count();
    if placed * 4 < texts.len() {
        found = walk_from_cursor(&prepared.stream, &texts);
    }

    let mut proposals: Vec<Proposal> = Vec::with_capacity(prepared.texts.len());
    let mut floor: Option<(usize, f64)> = None;
    let mut last_end = 0usize;

    for (index, (text_uuid, _)) in prepared.texts.iter().enumerate() {
        let tokens = &texts[index];
        if tokens.is_empty() {
            proposals.push(Proposal::refusal(text_uuid, "That text has no words to look for.".to_string()));
            continue;
        }
        let guess = found[index];
        // Nothing worth proposing. A refusal that can say how much of the text
        // the model did hear points at a mishearing (a weak model, a strange
        // name); one that cannot points at the text itself — a subject line, a
        // greeting, a date that the narrator never reads.
        let Some(span) = guess.span else {
            let mut refusal = Proposal::refusal(
                text_uuid,
                match guess.heard {
                    Some(heard) => format!(
                        "The model heard only {} of this text's {} words in the recordings, which is too little to place the reading. Try a stronger model, normalize the text, or set the edges by hand.",
                        heard.matched, tokens.len()
                    ),
                    None => "No stretch of the recordings holds this text, so it was not placed. It may be missing from the audio, or it may be all header — a subject line, a greeting, a date — that is never read aloud. Normalize the text, then guess again.".to_string(),
                },
            );
            if let Some(heard) = guess.heard {
                refusal.matched_words = heard.matched;
                refusal.reference_words = tokens.len();
            }
            proposals.push(refusal);
            continue;
        };

        let placed = span_proposal(&prepared, text_uuid, tokens, &span, floor, &previous);
        if let Some(next) = placed.floor {
            floor = Some(next);
        }
        last_end = last_end.max(span.to + 1);
        proposals.push(placed.proposal);
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
        "unused_words": prepared.stream.len().saturating_sub(last_end),
        "sources": prepared.index.sources.len(),
        "generated_at": set.generated_at,
    }))
}

#[cfg(not(feature = "stt"))]
pub(crate) fn run(_settings: &SettingsState, _dataset_uuid: &str) -> Result<Value, String> {
    Err("This build has no speech recognition, so the spans cannot be guessed.".to_string())
}

/// Re-place one passage, leaving every other row exactly as it is.
///
/// The bulk guess is the right tool for a set that has never been placed and
/// the wrong one for a single row: it rewrites the whole file, so a span
/// somebody nudged by hand would be replaced by the model's opinion of it. This
/// places only the requested text — at the position its order predicts, along the
/// same stream the bulk guess reads, and with the end of the nearest earlier
/// passage as a fallback when that prediction finds nothing — and replaces that
/// one row. Everything else, verdicts included, is untouched.
#[cfg(feature = "stt")]
pub(crate) fn run_one(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let prepared = prepare(&dir)?;
    let mut set = load_proposals(&dir);

    let position = prepared
        .texts
        .iter()
        .position(|(uuid, _)| uuid == text_uuid)
        .ok_or_else(|| format!("No text with id {text_uuid} is in this dataset — reload the list."))?;
    if !set.proposals.iter().any(|p| p.text_uuid == text_uuid) {
        return Err(
            "This text has never been placed, so there is nothing to re-place: guess for all first."
                .to_string(),
        );
    }

    // Where to start looking: the end of the nearest earlier passage that has a
    // span. Reading order is the only order the guess knows, and it is the one
    // the recordings follow.
    let mut cursor = 0usize;
    let mut floor: Option<(usize, f64)> = None;
    for (uuid, _) in prepared.texts[..position].iter().rev() {
        let Some(row) = set
            .proposals
            .iter()
            .find(|p| p.text_uuid == *uuid && p.status == "proposed")
        else {
            continue;
        };
        let Some(source) = row
            .source_uuid
            .as_deref()
            .and_then(|uuid| prepared.source_position(uuid))
        else {
            continue;
        };
        cursor = prepared.cursor_after(source, row.end).unwrap_or(prepared.stream.len());
        floor = Some((source, row.end));
        break;
    }

    let tokens = asr_metric::normalize_tokens(&prepared.texts[position].1);
    if tokens.is_empty() {
        return Err("That text has no words to look for, so it cannot be placed.".to_string());
    }
    let need = need_for(&tokens);

    // The prediction first, the neighbour's end second. The prediction is the
    // better answer — it is where the bulk guess would put this row — and the
    // cursor is only a fallback for a set whose order no longer matches the
    // recordings, e.g. after passages were added or re-placed by hand.
    let every_text: Vec<Vec<String>> = prepared
        .texts
        .iter()
        .map(|(_, reference)| asr_metric::normalize_tokens(reference))
        .collect();
    let total_reference: usize = every_text.iter().map(Vec::len).sum();
    let before: usize = every_text[..position].iter().map(Vec::len).sum();
    let expected = prior_position(before, total_reference, prepared.stream.len());
    let mut search = find_predicted(&tokens, &prepared.stream, expected, need);
    if search.hit.is_none() && cursor.abs_diff(expected) > PRIOR_SLACK_TOKENS {
        let retry = find_predicted(&tokens, &prepared.stream, cursor, need);
        if retry.hit.is_some() {
            search = retry;
        }
    }
    let found = search.hit.or(search.best);

    // The two ways to come up empty are written as refusals, not errors: the row
    // exists, and the answer to "why is it still not placed?" belongs on it.
    let placed = match found {
        None => Placed {
            proposal: Proposal::refusal(
                text_uuid,
                "No stretch of the recordings holds this text, so it was not placed. It may be missing from the audio, or it may be all header — a subject line, a greeting, a date — that is never read aloud. Normalize the text, then guess again.".to_string(),
            ),
            floor: None,
        },
        Some(found) if found.matched < need => {
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
            Placed { proposal: refusal, floor: None }
        }
        Some(found) => span_proposal(&prepared, text_uuid, &tokens, &found, floor, &set),
    };

    // Replace the row in place: the set's order is the queue's order, and
    // nothing else about it changes.
    if let Some(slot) = set.proposals.iter_mut().find(|p| p.text_uuid == text_uuid) {
        *slot = placed.proposal.clone();
    }
    set.generated_at = crate::datasets::now_row_stamp();
    write_json(&proposals_path(&dir), &set)?;

    let proposal = &placed.proposal;
    Ok(json!({
        "status": "ok",
        "text_uuid": proposal.text_uuid,
        "placed": proposal.status == "proposed",
        "source_uuid": proposal.source_uuid,
        "source_name": proposal.source_name,
        "start": proposal.start,
        "end": proposal.end,
        "confidence": proposal.confidence,
        "matched_words": proposal.matched_words,
        "reference_words": proposal.reference_words,
        "note": proposal.note,
        "counts": counts(&set),
        "generated_at": set.generated_at,
    }))
}

#[cfg(not(feature = "stt"))]
pub(crate) fn run_one(
    _settings: &SettingsState,
    _dataset_uuid: &str,
    _text_uuid: &str,
) -> Result<Value, String> {
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

/// One recording's cached transcript: what the model heard, word by word.
///
/// This is the page's result pane, and the answer to "why was this passage
/// placed here?" — the alignment compared exactly these tokens, at these
/// times. Read-only, and free: the cache is already on disk.
///
/// Each word carries the tokens after scoring's normalization (lowercased,
/// numbers spelled out), because that is the shape the matcher sees; the
/// original spelling is not kept.
pub(crate) fn transcript(
    settings: &SettingsState,
    dataset_uuid: &str,
    source_uuid: &str,
) -> Result<Value, String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let index = load_index(&dir);
    let source = index
        .sources
        .iter()
        .find(|s| s.uuid == source_uuid)
        .ok_or_else(|| {
            format!("No recording with id {source_uuid} is registered — pick the folder again.")
        })?;
    let cache: SourceTranscript = read_json(&transcript_path(&dir, source_uuid)).ok_or_else(|| {
        format!(
            "{} has no transcript yet — listen to it first.",
            source.file_name
        )
    })?;
    let words: Vec<Value> = cache
        .words
        .iter()
        .map(|word| {
            json!({
                "start": round_ms(word.start),
                "end": round_ms(word.end),
                "text": word.tokens.join(" "),
            })
        })
        .collect();
    Ok(json!({
        "status": "ok",
        "source_uuid": source.uuid,
        "file_name": source.file_name,
        "path": source.path,
        "duration": round_ms(cache.duration),
        "peak_seconds": cache.peak_seconds,
        "word_count": words.len(),
        "words": words,
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

/// Re-place one passage, leaving every other row of the proposal set untouched.
#[tauri::command]
pub async fn read_aloud_align_guess_one(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
) -> Result<Value, String> {
    run_one(&settings, &dataset_uuid, &text_uuid)
}

/// The proposals with their texts and a thumbnail of the audio around each span.
#[tauri::command]
pub async fn read_aloud_align_proposals(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Value, String> {
    list(&settings, &dataset_uuid)
}

/// One recording's cached transcript, for the result pane.
#[tauri::command]
pub async fn read_aloud_align_transcript(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    source_uuid: String,
) -> Result<Value, String> {
    transcript(&settings, &dataset_uuid, &source_uuid)
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

    /// Padding, the minimum length, and the monotonic clamp — the three ways a
    /// span can be the wrong size. Where an edge lands inside the pause it
    /// meets is `edges`' question; the bounds this call site hands it are what
    /// keeps two clips apart.
    #[test]
    fn a_span_is_padded_and_never_overlaps_the_one_before() {
        let loud = vec![100i8; 20_000];
        let loudness = edges::loudness_from_envelope(&loud);
        let start = round_ms(edges::place_edge(
            &loudness,
            edges::EDGE_FRAME_SECS,
            10.0,
            GUESS_PAD_BEFORE,
            edges::Side::Head,
            0.0,
            100.0,
        ));
        assert!((start - 9.8).abs() < 1e-9, "start {start}");

        // The previous passage ends at 10.5, so this one may not start before it.
        let start = round_ms(edges::place_edge(
            &loudness,
            edges::EDGE_FRAME_SECS,
            10.0,
            GUESS_PAD_BEFORE,
            edges::Side::Head,
            10.5,
            100.0,
        ));
        assert!((start - 10.5).abs() < 1e-9, "start {start} overlaps the previous passage");
    }

    /// Milli precision, so a typed 12.345 s is stored as 12.345 s.
    #[test]
    fn spans_round_to_milliseconds() {
        assert!((round_ms(12.3454) - 12.345).abs() < 1e-9);
        assert!((round_ms(12.3456) - 12.346).abs() < 1e-9);
    }

    /// Reading order maps word counts onto stream positions, and the answer can
    /// never point past the last heard token.
    #[test]
    fn the_prior_follows_the_word_count() {
        assert_eq!(prior_position(0, 100, 1000), 0);
        assert_eq!(prior_position(50, 100, 1000), 500);
        assert_eq!(prior_position(100, 100, 1000), 999);
        assert_eq!(prior_position(5, 0, 1000), 0);
    }

    /// The predicted window decides *which* occurrence: a passage that occurs
    /// once near the start and once far later belongs to the one reading order
    /// points at, even though the earlier one scores just as well.
    #[cfg(feature = "stt")]
    #[test]
    fn the_prediction_picks_the_occurrence_in_reading_order() {
        let reference: Vec<String> = (0..15).map(|i| format!("w{}", i)).collect();
        let mut stream: Vec<String> = (0..6000).map(|i| format!("x{}", i)).collect();
        for offset in [200usize, 5000] {
            for (index, token) in reference.iter().enumerate() {
                stream[offset + index] = token.clone();
            }
        }
        let need = need_for(&reference);
        let found = find_predicted(&reference, &stream, 5000, need).hit.expect("the passage is there");
        assert_eq!(found.from, 5000, "placed at {} instead of the predicted 5000", found.from);
        assert_eq!(found.matched, 15);
    }

    /// The gap between two anchors is where the one-liners go: a phrase that
    /// occurs a dozen times in the book still lands between its neighbours,
    /// because a block alignment cannot put it anywhere else.
    #[cfg(feature = "stt")]
    #[test]
    fn the_gap_fill_keeps_a_one_liner_between_its_neighbours() {
        let words = |prefix: &str| -> Vec<String> {
            (0..14).map(|i| format!("{prefix}{i}")).collect()
        };
        let texts = vec![words("a"), ["ja", "ja", "ja"].map(String::from).to_vec(), words("c")];

        let mut stream: Vec<String> = (0..100).map(|i| format!("x{}", i)).collect();
        stream.extend(texts[0].iter().cloned());
        stream.extend((0..40).map(|i| format!("y{}", i)));
        stream.extend(texts[1].iter().cloned());
        stream.extend((0..40).map(|i| format!("z{}", i)));
        stream.extend(texts[2].iter().cloned());
        stream.extend((0..100).map(|i| format!("q{}", i)));

        let slots = place_all(&stream, &texts);
        let first = slots[0].span.expect("the long first text is placed");
        let middle = slots[1].span.expect("the one-liner is placed in the gap");
        let last = slots[2].span.expect("the long last text is placed");
        assert_eq!((first.from, first.to), (100, 113));
        assert_eq!((middle.from, middle.to), (154, 156));
        assert_eq!((last.from, last.to), (197, 210));
    }

    /// A gap that does not hold the passages claiming to be in it leaves them
    /// unplaced rather than stretching them across the audio it does hold.
    #[cfg(feature = "stt")]
    #[test]
    fn a_gap_that_holds_nothing_refuses_instead_of_stretching() {
        let words = |prefix: &str| -> Vec<String> {
            (0..14).map(|i| format!("{prefix}{i}")).collect()
        };
        let texts = vec![words("a"), words("m"), words("c")];

        let mut stream: Vec<String> = (0..100).map(|i| format!("x{}", i)).collect();
        stream.extend(texts[0].iter().cloned());
        stream.extend((0..40).map(|i| format!("y{}", i)));
        stream.extend(texts[2].iter().cloned());
        stream.extend((0..100).map(|i| format!("q{}", i)));

        let slots = place_all(&stream, &texts);
        assert!(slots[0].span.is_some() && slots[2].span.is_some());
        assert!(slots[1].span.is_none(), "the missing text was placed at {:?}", slots[1].span);
    }
}
