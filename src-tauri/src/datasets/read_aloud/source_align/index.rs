use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
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

use super::cache;
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
    /// sha256 of the bytes. This — not the path, not the mtime, and not the
    /// name — is what says a cache describes this file: a copy to another
    /// machine changes the first two and may change the third, and none of
    /// that changes a word the model heard. Empty on a dataset written before
    /// content hashes existed.
    #[serde(default)]
    pub content_hash: String,
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
    /// The bytes this cache describes, so the file can be adopted by the align
    /// index of another machine without consulting `sources.json`.
    #[serde(default)]
    pub(super) content_hash: String,
    /// Byte length of that recording — a cheap "same file?" cross-check when a
    /// cache arrives without a manifest or a binding.
    #[serde(default)]
    pub(super) size: u64,
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

/// One cached transcript, by cache key: a recording's content hash, or — for a
/// cache written before hashes existed — its file-name hash.
pub(super) fn transcript_path(dir: &Path, cache_key: &str) -> PathBuf {
    align_dir(dir).join(format!("{}.json", cache_key))
}

/// A recording's cached transcript, by content hash and then — for a dataset
/// that has not been re-picked since the upgrade — by its file-name hash.
pub(super) fn read_transcript(dir: &Path, source: &SourceEntry) -> Option<SourceTranscript> {
    let hash = source.content_hash.trim();
    if !hash.is_empty() {
        if let Some(transcript) = read_json(&transcript_path(dir, hash)) {
            return Some(transcript);
        }
    }
    read_json(&transcript_path(dir, &source.uuid))
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

/// Where a registered recording is now: the path it was scanned at, or — when
/// that path is gone — the same file name under the folder the index is bound
/// to. A folder copied to another machine keeps its recordings and only moves
/// them, so the second answer is the common one, not the exception.
fn resolve_source(index: &SourceIndex, entry: &SourceEntry) -> Option<PathBuf> {
    let stored = PathBuf::from(&entry.path);
    if stored.is_file() {
        return Some(stored);
    }
    if index.directory.is_empty() {
        return None;
    }
    let relinked = Path::new(&index.directory).join(&entry.file_name);
    relinked.is_file().then_some(relinked)
}

/// The recording a stored span names, if it is still where it was registered.
/// A moved or deleted file answers `None` — a clip is not cut from a guess.
pub(crate) fn source_file(dir: &Path, source_uuid: &str) -> Option<PathBuf> {
    let index = load_index(dir);
    let entry = index.sources.iter().find(|s| s.uuid == source_uuid)?;
    resolve_source(&index, entry)
}

/// Every registered recording that is still on this device, for a listing that
/// wants to know which span clips it could cut without asking one by one.
pub(crate) fn present_sources(dir: &Path) -> HashSet<String> {
    let index = load_index(dir);
    index
        .sources
        .iter()
        .filter(|source| resolve_source(&index, source).is_some())
        .map(|source| source.uuid.clone())
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
/// length either side (never less than a second), so the text's neighbours
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
    // Context scales with the text, not with a stopwatch: a twelve-second
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

/// The file's slot in the dataset: a hash of its name.
///
/// Not its identity — [`SourceEntry::content_hash`] is — but the name a stored
/// span and a clip row use, so keeping it means a copied folder keeps working
/// without a migration of `proposals.json` or `read_standard_audio`.
fn source_uuid(file_name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(file_name.as_bytes());
    cache::hex_digest(hasher.finalize())
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

/// Bind this dataset to `directory` and reuse every cache the bytes allow.
///
/// Registration is a *connect*, not a rebuild. Each file is identified by the
/// sha256 of its bytes: a recording whose bytes this dataset already heard
/// keeps its transcript however its mtime moved, whatever it is called now, and
/// wherever the folder lives — which is what makes a recordings folder plus its
/// `align/` portable to another machine with no re-recognition. A cache written
/// before content hashes existed is adopted by file name (and size, when a
/// previous binding is there to compare against) and renamed to its hash.
///
/// Nothing is deleted: a cache for a recording that is no longer in the folder
/// stays on disk and is reported as `orphaned`, so switching folders is free
/// and reversible. Omitted `directory` lists what is already registered, so the
/// panel can render without re-picking — and without hashing anything.
pub(crate) fn register(
    settings: &SettingsState,
    dataset_uuid: &str,
    directory: Option<&str>,
) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let mut index = load_index(&dir);
    let previous = index.sources.clone();
    let mut reconcile: Option<Value> = None;

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

        // What is already cached, read before anything is hashed: the answer to
        // "do I have this file's transcript?" is a file name, not a decode.
        let cache = cache::scan(&dir);
        let mut manifest = cache::load_manifest(&dir);
        let previous_by_uuid: HashMap<&str, &SourceEntry> = previous
            .iter()
            .map(|source| (source.uuid.as_str(), source))
            .collect();

        let mut sources = Vec::with_capacity(files.len());
        let mut reused = 0usize;
        let mut adopted = 0usize;
        let mut adopted_unverified = 0usize;
        let mut fresh = 0usize;

        for path in files {
            let file_name = file_name_of(&path);
            let uuid = source_uuid(&file_name);
            let path_string = path.to_string_lossy().into_owned();
            let (size, modified_ms) = fingerprint(&path);
            let known = previous_by_uuid.get(uuid.as_str()).copied();

            // One sequential read for a file this device has not hashed at this
            // path, size and mtime before — and none at all for one it has.
            let content_hash = match known.filter(|source| {
                source.path == path_string
                    && source.size == size
                    && source.modified_ms == modified_ms
                    && !source.content_hash.is_empty()
            }) {
                Some(hashed) => hashed.content_hash.clone(),
                None => cache::hash_file(&path)
                    .map_err(|e| format!("Cannot hash {}: {}", file_name, e))?,
            };

            // A cache named after the file can only be believed when nothing
            // says the bytes changed: a previous hash that disagrees, or a
            // previous size that disagrees, is the end of the adoption.
            let adoptable = match known {
                Some(previous) if !previous.content_hash.is_empty() => {
                    previous.content_hash == content_hash
                }
                Some(previous) => previous.size == size,
                None => true,
            };
            let reused_here = cache.has(&content_hash);
            let adopted_here = !reused_here && adoptable && cache.has(&uuid);

            let (duration, words) = if reused_here || adopted_here {
                cache::facts(&dir, &content_hash, &uuid, known, &manifest)
                    .map(|(duration, words)| (Some(duration), Some(words)))
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };

            if reused_here {
                reused += 1;
            } else if adopted_here {
                adopted += 1;
                // An align dir copied in on its own carries no size to check,
                // so the name is all the evidence there is. Counted, and said
                // out loud, rather than presented as proof.
                if !known.is_some_and(|previous| previous.size == size) {
                    adopted_unverified += 1;
                }
                cache::promote(&dir, &uuid, &content_hash)?;
            } else {
                fresh += 1;
            }

            sources.push(SourceEntry {
                duration,
                words,
                uuid,
                file_name,
                path: path_string,
                size,
                modified_ms,
                content_hash,
            });
        }

        // One manifest write for the whole folder, and one binding write below.
        let mut keep: HashSet<String> = HashSet::with_capacity(sources.len() * 2);
        for source in &sources {
            keep.insert(source.content_hash.clone());
            keep.insert(source.uuid.clone());
            if let (Some(duration), Some(words)) = (source.duration, source.words) {
                manifest.upsert(cache::ManifestEntry {
                    content_hash: source.content_hash.clone(),
                    file_name: source.file_name.clone(),
                    size: source.size,
                    duration,
                    words,
                });
            }
        }
        manifest.dataset_uuid = dataset_uuid.to_string();
        manifest.updated_at = crate::datasets::now_stamp();
        cache::save_manifest(&dir, &manifest)?;

        let new_uuids: HashSet<&str> = sources.iter().map(|s| s.uuid.as_str()).collect();
        let missing = previous
            .iter()
            .filter(|source| !new_uuids.contains(source.uuid.as_str()))
            .count();
        let orphaned = cache.orphaned(&keep);

        // Order is what a guess reads — the recordings are one stream — so a
        // folder that sorts differently than the last one may move every span
        // after the change. Rank is the position in the whole sorted list, not
        // among the files both folders hold: a chapter inserted or removed
        // shifts everything behind it, and that is exactly what invalidates a
        // span. Only files that survived are counted; the one that moved shows
        // up below as `missing` and `new` instead.
        let old_rank: HashMap<&str, usize> = previous
            .iter()
            .enumerate()
            .map(|(rank, source)| (source.uuid.as_str(), rank))
            .collect();
        let reordered = sources
            .iter()
            .enumerate()
            .filter(|(position, source)| {
                old_rank
                    .get(source.uuid.as_str())
                    .is_some_and(|rank| rank != position)
            })
            .count();

        index.directory = root.to_string_lossy().into_owned();
        index.sources = sources;

        reconcile = Some(json!({
            "directory": index.directory.clone(),
            "total": index.sources.len(),
            "reused": reused,
            "adopted": adopted,
            "adopted_unverified": adopted_unverified,
            "new": fresh,
            "missing": missing,
            "orphaned": orphaned,
            "reordered": reordered,
        }));
    }

    save_index(&dir, &index)?;
    Ok(index_view(&index, reconcile))
}

fn index_view(index: &SourceIndex, reconcile: Option<Value>) -> Value {
    let transcribed = index.sources.iter().filter(|s| s.words.is_some()).count();
    let mut out = json!({
        "status": "ok",
        "directory": index.directory,
        "sources": index.sources,
        "source_count": index.sources.len(),
        "transcribed": transcribed,
        "pending_transcription": index.sources.len() - transcribed,
    });
    if let Value::Object(map) = &mut out {
        map.insert("reconcile".into(), reconcile.unwrap_or(Value::Null));
    }
    out
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
                let mut out = index_view(&index, None);
                if let Value::Object(map) = &mut out {
                    map.insert("done".into(), json!(true));
                    map.insert("remaining".into(), json!(0));
                }
                return Ok(out);
            }
        },
    };

    let mut entry = index.sources[position].clone();
    let Some(path) = resolve_source(&index, &entry) else {
        return Err(format!(
            "{} is no longer at {}. Re-pick the folder to register it again.",
            entry.file_name, entry.path
        ));
    };

    // A dataset registered before content hashes existed: hash the one file this
    // call was about to read anyway, so the cache it writes is named for the
    // bytes and can travel to another machine.
    if entry.content_hash.is_empty() {
        entry.content_hash = cache::hash_file(&path)
            .map_err(|e| format!("Cannot hash {}: {}", entry.file_name, e))?;
        index.sources[position].content_hash = entry.content_hash.clone();
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
        content_hash: entry.content_hash.clone(),
        size: entry.size,
        duration,
        peak_seconds,
        peaks: encode_peaks(&peaks),
        words,
    };
    write_json(&transcript_path(&dir, &entry.content_hash), &transcript)?;
    index.sources[position].duration = Some(duration);
    index.sources[position].words = Some(transcript.words.len());

    // The manifest is the cache's own table of contents, written before the
    // binding: whatever copies this `align/` next can describe itself without
    // reading a single transcript.
    let mut manifest = cache::load_manifest(&dir);
    manifest.dataset_uuid = dataset_uuid.to_string();
    manifest.updated_at = crate::datasets::now_stamp();
    manifest.upsert(cache::ManifestEntry {
        content_hash: entry.content_hash.clone(),
        file_name: entry.file_name.clone(),
        size: entry.size,
        duration,
        words: transcript.words.len(),
    });
    cache::save_manifest(&dir, &manifest)?;
    save_index(&dir, &index)?;

    let pending = index.sources.iter().filter(|s| s.words.is_none()).count();
    let mut out = index_view(&index, None);
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
/// This is the page's result pane, and the answer to "why was this text
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
    let cache: SourceTranscript = read_transcript(&dir, source).ok_or_else(|| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::read_aloud::tests_support::{dataset_with_text, temp_settings};

    // ---- fixtures -----------------------------------------------------------

    /// A folder of recordings, beside the dataset as the real ones are.
    fn recordings(workspace: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        let root = workspace.join("recordings");
        fs::create_dir_all(&root).unwrap();
        for (name, bytes) in files {
            fs::write(root.join(name), bytes).unwrap();
        }
        root
    }

    /// Connect a dataset to a folder, and hand back the view plus where the
    /// dataset lives.
    fn connect(settings: &SettingsState, dataset: &str, folder: &Path) -> (Value, PathBuf) {
        let view = register(settings, dataset, Some(&folder.to_string_lossy())).unwrap();
        let dir = find_dataset_dir_write(settings, dataset).unwrap();
        (view, dir)
    }

    fn count(view: &Value, key: &str) -> u64 {
        view["reconcile"][key]
            .as_u64()
            .unwrap_or_else(|| panic!("reconcile.{key} is missing from {view}"))
    }

    /// One cached recording, as `transcribe` would have written it. `hash` is
    /// empty for a cache from a build that had no content hashes.
    fn cache_for(dir: &Path, key: &str, source: &SourceEntry, hash: &str) {
        let word = |start: f64, end: f64, text: &str| SpokenWord {
            start,
            end,
            tokens: vec![text.to_string()],
        };
        let transcript = SourceTranscript {
            uuid: source.uuid.clone(),
            file_name: source.file_name.clone(),
            content_hash: hash.to_string(),
            size: source.size,
            duration: 12.5,
            peak_seconds: PEAK_SECONDS,
            peaks: String::new(),
            words: vec![
                word(0.0, 0.5, "one"),
                word(0.5, 1.0, "two"),
                word(1.0, 1.5, "three"),
            ],
        };
        write_json(&transcript_path(dir, key), &transcript).unwrap();
    }

    fn only_source(dir: &Path) -> SourceEntry {
        load_index(dir)
            .sources
            .into_iter()
            .next()
            .expect("one registered recording")
    }

    // ---- the cache follows the bytes ----------------------------------------

    /// A cache describes the bytes, not the moment they landed. Forgetting the
    /// timestamp — what a copy to another machine does — must cost nothing.
    #[test]
    fn a_cache_survives_a_changed_mtime() {
        let (settings, workspace) = temp_settings("align-mtime");
        let (dataset, _text) = dataset_with_text(&settings, "one two three");
        let folder = recordings(&workspace, &[("ch1.mp3", b"abcdef"), ("ch2.mp3", b"ghijkl")]);

        let (view, dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "new"), 2);
        for source in load_index(&dir).sources {
            let hash = source.content_hash.clone();
            cache_for(&dir, &hash, &source, &hash);
        }

        let mut moved = load_index(&dir);
        for source in &mut moved.sources {
            source.modified_ms = 0;
        }
        save_index(&dir, &moved).unwrap();

        let (view, dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "reused"), 2);
        assert_eq!(count(&view, "new"), 0);
        // ...and the recording's size comes back with it. There is no manifest
        // entry and no previous duration to read, so this one answer is read off
        // the cache — once, and written to the manifest below.
        let index = load_index(&dir);
        assert_eq!(index.sources[0].duration, Some(12.5));
        assert_eq!(index.sources[0].words, Some(3));
    }

    /// A cache from before content hashes is adopted and renamed, not rebuilt:
    /// the words in it describe the same bytes either way.
    #[test]
    fn a_legacy_cache_is_adopted_and_promoted() {
        let (settings, workspace) = temp_settings("align-adopt");
        let (dataset, _text) = dataset_with_text(&settings, "one two three");
        let folder = recordings(&workspace, &[("ch1.mp3", b"abcdef")]);

        let (view, dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "new"), 1);
        let source = only_source(&dir);
        let legacy = format!("{}.json", source_uuid(&source.file_name));
        assert_eq!(legacy, format!("{}.json", source.uuid));
        cache_for(&dir, &source.uuid, &source, "");

        let (view, dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "adopted"), 1);
        assert_eq!(count(&view, "reused"), 0);
        // The previous binding had the same size, so the name is not the only
        // evidence there is.
        assert_eq!(count(&view, "adopted_unverified"), 0);
        assert_eq!(count(&view, "new"), 0);
        // One payload, under the content hash now.
        let align = align_dir(&dir);
        assert!(align.join(format!("{}.json", source.content_hash)).is_file());
        assert!(!align.join(legacy).is_file());
        assert_eq!(only_source(&dir).words, Some(3));

        // Which means the next connect is an ordinary reuse.
        let (view, _dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "reused"), 1);
        assert_eq!(count(&view, "adopted"), 0);
    }

    /// Same name, different bytes: the cache describes audio that is gone. It is
    /// neither reused nor adopted — and it is not deleted either.
    #[test]
    fn a_re_ripped_file_is_new_and_its_old_cache_is_reported() {
        let (settings, workspace) = temp_settings("align-rerip");
        let (dataset, _text) = dataset_with_text(&settings, "one two three");
        let folder = recordings(&workspace, &[("ch1.mp3", b"aaaaaaaa"), ("ch2.mp3", b"bbbbbbbb")]);

        let (view, dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "new"), 2);
        let before = load_index(&dir);
        for source in &before.sources {
            let hash = source.content_hash.clone();
            cache_for(&dir, &hash, source, &hash);
        }
        let stale = before
            .sources
            .iter()
            .find(|source| source.file_name == "ch1.mp3")
            .unwrap()
            .content_hash
            .clone();

        fs::write(folder.join("ch1.mp3"), b"ZZZZZZZZZZZZ").unwrap();
        let (view, dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "new"), 1);
        assert_eq!(count(&view, "reused"), 1);
        assert_eq!(count(&view, "adopted"), 0);
        assert_eq!(count(&view, "orphaned"), 1);
        assert!(align_dir(&dir).join(format!("{}.json", stale)).is_file());
        let ch1 = load_index(&dir)
            .sources
            .into_iter()
            .find(|source| source.file_name == "ch1.mp3")
            .unwrap();
        assert_eq!(ch1.words, None);
    }

    /// A folder that moved is followed: the stored absolute path is a hint, and
    /// the binding's folder plus the file name is the answer.
    #[test]
    fn a_moved_folder_is_followed() {
        let (settings, workspace) = temp_settings("align-moved");
        let (dataset, _text) = dataset_with_text(&settings, "one two three");
        let original = recordings(&workspace, &[("ch1.mp3", b"abcdef")]);
        let (_view, dir) = connect(&settings, &dataset, &original);
        let uuid = only_source(&dir).uuid;
        assert!(source_file(&dir, &uuid).is_some());

        let moved = workspace.join("recordings-elsewhere");
        fs::rename(&original, &moved).unwrap();
        // Every recorded path is a dead string now, so nothing resolves...
        assert!(source_file(&dir, &uuid).is_none());

        // ...until the binding names the folder it is pointed at.
        let mut index = load_index(&dir);
        index.directory = moved.to_string_lossy().into_owned();
        save_index(&dir, &index).unwrap();
        assert_eq!(source_file(&dir, &uuid).unwrap(), moved.join("ch1.mp3"));
    }

    /// A rename that changes where the file sorts moves every span behind it,
    /// and only the recordings that survived are counted as moved.
    #[test]
    fn a_rename_that_changes_the_order_is_reported() {
        let (settings, workspace) = temp_settings("align-reorder");
        let (dataset, _text) = dataset_with_text(&settings, "one two three");
        let folder = recordings(&workspace, &[("ch1.mp3", b"aaa"), ("ch2.mp3", b"bbb")]);

        let (view, dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "reordered"), 0);
        for source in load_index(&dir).sources {
            let hash = source.content_hash.clone();
            cache_for(&dir, &hash, &source, &hash);
        }

        // `ch2` becomes `ch0`, so `ch1` is no longer first.
        fs::rename(folder.join("ch2.mp3"), folder.join("ch0.mp3")).unwrap();
        let (view, _dir) = connect(&settings, &dataset, &folder);
        assert_eq!(count(&view, "reordered"), 1);
        // The rename itself costs nothing: both caches are found by their bytes,
        // so neither file is listened to again. Only the old name is gone.
        assert_eq!(count(&view, "reused"), 2);
        assert_eq!(count(&view, "new"), 0);
        assert_eq!(count(&view, "missing"), 1);
    }

    // ---- shapes that outlive a build ----------------------------------------

    /// A `sources.json` written before content hashes existed — the fixture the
    /// standard-audio tests build by hand — still parses.
    #[test]
    fn a_legacy_sources_file_still_parses() {
        let (settings, _workspace) = temp_settings("align-legacy");
        let (dataset, _text) = dataset_with_text(&settings, "one two three");
        let dir = find_dataset_dir_write(&settings, &dataset).unwrap();
        fs::create_dir_all(align_dir(&dir)).unwrap();
        fs::write(
            sources_path(&dir),
            r#"{"directory":"D:/books","sources":[{"uuid":"aa","file_name":"ch1.wav","path":"D:/books/ch1.wav","size":10,"modified_ms":0}]}"#,
        )
        .unwrap();

        let index = load_index(&dir);
        assert_eq!(index.directory, "D:/books");
        assert_eq!(index.sources.len(), 1);
        assert_eq!(index.sources[0].content_hash, "");
        assert_eq!(index.sources[0].duration, None);
        assert_eq!(index.sources[0].words, None);
    }

    /// The manifest is the part that travels on its own, so an upsert replaces
    /// by content hash and a reload reads back what was written.
    #[test]
    fn the_manifest_round_trips() {
        let (settings, _workspace) = temp_settings("align-manifest");
        let (dataset, _text) = dataset_with_text(&settings, "one two three");
        let dir = find_dataset_dir_write(&settings, &dataset).unwrap();
        let entry = |hash: &str, name: &str, words: usize| cache::ManifestEntry {
            content_hash: hash.to_string(),
            file_name: name.to_string(),
            size: 10,
            duration: 12.5,
            words,
        };

        let mut manifest = cache::load_manifest(&dir);
        assert_eq!(manifest.version, 2);
        assert!(manifest.entries.is_empty());
        manifest.upsert(entry("h1", "ch1.mp3", 3));
        manifest.upsert(entry("h2", "ch2.mp3", 4));
        cache::save_manifest(&dir, &manifest).unwrap();

        let mut reloaded = cache::load_manifest(&dir);
        assert_eq!(reloaded.entries.len(), 2);
        assert_eq!(reloaded.entry("h2").unwrap().words, 4);

        // A rename updates the row instead of adding a second claim about the
        // same bytes.
        reloaded.upsert(entry("h1", "renamed.mp3", 9));
        assert_eq!(reloaded.entries.len(), 2);
        assert_eq!(reloaded.entry("h1").unwrap().file_name, "renamed.mp3");
        assert_eq!(reloaded.entry("h1").unwrap().words, 9);
    }
}

