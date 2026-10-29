use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::audio;
use crate::datasets::asr_metric;
use crate::settings::SettingsState;

use super::super::standard_audio::{self, MAX_SOURCE_SECS};
use super::super::{find_dataset_dir, find_dataset_dir_write};
use super::{
    round_ms, AUDIO_EXTENSIONS, PEAK_SECONDS, PREVIEW_MARGIN_MIN_SECS, PREVIEW_MARGIN_RATIO,
    PREVIEW_PAIRS,
};

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
pub(super) struct SourceIndex {
    /// The directory the files were picked from, shown so it is obvious the
    /// cache is tied to a folder and not to the dataset.
    #[serde(default)]
    pub(super) directory: String,
    #[serde(default)]
    pub(super) sources: Vec<SourceEntry>,
}

/// One spoken word as the model timed it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SpokenWord {
    pub(super) start: f64,
    pub(super) end: f64,
    /// The word's tokens after scoring's own normalization, so the alignment
    /// compares exactly the shapes it would compare from the reference text.
    pub(super) tokens: Vec<String>,
}

/// One recording's cache: the word timings a span can be read off, and the peak
/// envelope an edge can be snapped to and a preview drawn from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SourceTranscript {
    pub(super) uuid: String,
    pub(super) file_name: String,
    pub(super) duration: f64,
    /// Seconds one peak pair covers — stored rather than assumed, so changing
    /// [`PEAK_SECONDS`] does not silently misread old caches.
    pub(super) peak_seconds: f64,
    /// base64 of the interleaved min/max `i8` pairs (`WaveformPeaks` layout).
    pub(super) peaks: String,
    pub(super) words: Vec<SpokenWord>,
}


// ============================================================
// Paths and JSON
// ============================================================

pub(super) fn align_dir(dir: &Path) -> PathBuf {
    dir.join(crate::datasets::READ_ALOUD_ALIGN_DIR)
}

fn sources_path(dir: &Path) -> PathBuf {
    align_dir(dir).join("sources.json")
}

pub(super) fn transcript_path(dir: &Path, source_uuid: &str) -> PathBuf {
    align_dir(dir).join(format!("{}.json", source_uuid))
}


/// A missing or unreadable cache reads as "nothing cached yet". Derived data
/// must never turn into a failure: the next run simply rebuilds it.
pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(super) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

pub(super) fn load_index(dir: &Path) -> SourceIndex {
    read_json(&sources_path(dir)).unwrap_or_default()
}

fn save_index(dir: &Path, index: &SourceIndex) -> Result<(), String> {
    write_json(&sources_path(dir), index)
}

// ============================================================
// Looking a recording up from a stored span
// ============================================================
//
// A span row names the recording it came from and nothing else: the align cache
// is derived and never travels, so "where is `8d2c…` now?" is answered from the
// index on this device, or not at all. These two reads are how the clip cache
// (`standard_audio::materialize`) finds the audio to cut.

/// The recording a stored span names, if it is still where it was registered.
/// A moved or deleted file answers `None` — a clip is not cut from a guess.
pub(crate) fn source_file(dir: &Path, source_uuid: &str) -> Option<PathBuf> {
    let index = load_index(dir);
    let entry = index.sources.iter().find(|s| s.uuid == source_uuid)?;
    let path = PathBuf::from(&entry.path);
    path.is_file().then_some(path)
}

/// Every registered recording that is still on this device, for a listing that
/// wants to know which span clips it could cut without asking one by one.
pub(crate) fn present_sources(dir: &Path) -> std::collections::HashSet<String> {
    load_index(dir)
        .sources
        .into_iter()
        .filter(|s| Path::new(&s.path).is_file())
        .map(|s| s.uuid)
        .collect()
}

/// The registered recording a path names, if the dataset registered it: what a
/// caller that was handed a file needs in order to talk about the folder's own
/// entry. `None` means the file is not this dataset's business — an upload, a
/// crop, a chapter from somewhere else.
pub(crate) fn source_identity(dir: &Path, path: Option<&str>) -> Option<(String, String)> {
    let path = path.map(str::trim).filter(|s| !s.is_empty())?;
    load_index(dir)
        .sources
        .into_iter()
        .find(|s| same_path(&s.path, path))
        .map(|s| (s.uuid, s.file_name))
}

/// Two paths name one file when they are equal, or when both resolve to it. A
/// picked file is a literal string from a dialog; the index holds what the scan
/// saw, which may be the same file spelled differently.
fn same_path(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

// ============================================================
// Peaks
// ============================================================

/// Encode interleaved `i8` min/max pairs as base64. `i8 as u8` is a lossless
/// reinterpretation, which is why the round trip needs no scaling.
pub(super) fn encode_peaks(peaks: &[i8]) -> String {
    let bytes: Vec<u8> = peaks.iter().map(|v| *v as u8).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(super) fn decode_peaks(encoded: &str) -> Vec<i8> {
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
/// proposal's row. The window is the span plus thirty per cent of its own
/// length either side (never less than a second), so the passage's neighbours
/// are in view; wide spans are decimated by min/max so the preview keeps the
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

pub(super) fn preview(transcript: &SourceTranscript, start: f64, end: f64) -> Preview {
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
    // Context scales with the passage, not with a stopwatch: a twelve-second
    // span shows about three and a half either side, a two-second one still
    // shows a second. Both ends are clamped to the recording, so a span near the
    // head simply gets a shorter lead-in than it asked for.
    let margin = ((end - start).max(0.0) * PREVIEW_MARGIN_RATIO).max(PREVIEW_MARGIN_MIN_SECS);
    let from = (((start - margin) / peak).floor().max(0.0) as usize).min(pairs - 1);
    let to = (((end + margin) / peak).ceil() as usize).clamp(from + 1, pairs);
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
pub(super) fn natural_cmp(a: &str, b: &str) -> Ordering {
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
        .map_err(standard_audio::guess_error)?;

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

