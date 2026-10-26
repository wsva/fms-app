//! Standard audio: the reference clip a text's pronunciation is measured against.
//!
//! ## Why the dataset owns it
//!
//! Pronunciation needs a canonical rendering of the text to decode into a phone
//! timeline (see [`super::tts_ref`]). It used to be synthesised *during scoring*
//! and cached in the workspace, which meant a score depended on an HTTP request
//! and could not be inspected or corrected. Now each text carries a **list** of
//! clips, exactly one marked default, and scoring reads only that one — so a
//! mispronounced or clipped TTS rendering is replaceable by a human recording.
//!
//! ## The three terms
//!
//! | term | means | lives in |
//! |---|---|---|
//! | attempt | one submission: recording + transcript + assessment | `read_attempt_event` (app DB) |
//! | recording | the WAV of one attempt | `<workspace>/read_aloud_audio/<attempt_uuid>.wav` |
//! | standard audio | a reference clip the dataset owns | `read_standard_audio` + `media/<text_uuid>/<uuid>.wav` |
//!
//! ## What is stored, and what is not
//!
//! The row keeps only what cannot be derived: the note (free-form provenance —
//! origin, voice, how it was cropped; display-only, never parsed), the TTS word
//! boundaries (which an upload cannot have), the hash of the text it was made
//! from, and the default flag. The path is the row's identity, the duration is
//! decoded from the file, and `kind` is `boundaries IS NULL`. A column that can
//! be recomputed from the row's identity or its file would only be a second copy
//! that can drift from the truth.
//!
//! The clip itself is **16 kHz mono WAV** — the phoneme model's input rate, which
//! is exactly what [`crate::audio::decode_to_pcm`] returns, and small. No source
//! file is kept: a TTS clip is regenerable in one click, and an upload's original
//! is still where the user picked it.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::State;
use uuid::Uuid;

use crate::audio;
use crate::datasets::asr_metric;
use crate::edge_tts::{self, TtsWordBoundary};
use crate::settings::{AppSettings, SettingsState};

use super::{edges, effective_reference, find_dataset_dir, find_dataset_dir_write, open_db};

/// Columns of the waveform a probe returns. Roughly one per two pixels at the
/// dialog's width, so the trim track can draw without resampling.
const PROBE_COLUMNS: usize = 600;

/// Shortest clip the import path accepts. Anything less is a mis-drag, not a
/// reading.
const MIN_CLIP_SECS: f64 = 0.3;

// ============================================================
// Types
// ============================================================

/// The stored row — what the DB holds and what the change journal carries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StandardAudioRow {
    pub uuid: String,
    pub text_uuid: String,
    #[serde(default)]
    pub note: String,
    /// JSON `[{text,start,duration}]` from Edge TTS; `None` for an upload,
    /// which is what makes `kind` derivable.
    #[serde(default)]
    pub boundaries: Option<String>,
    /// sha256 of the effective reference the clip was made from.
    #[serde(default)]
    pub text_sha: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub created_at: String,
}

impl StandardAudioRow {
    fn from_payload(payload: &Value) -> Result<Self, String> {
        let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let uuid = get("uuid");
        if uuid.is_empty() {
            return Err("read_standard_audio change carries no uuid".to_string());
        }
        let text_uuid = get("text_uuid");
        if text_uuid.is_empty() {
            return Err("read_standard_audio change carries no text_uuid".to_string());
        }
        Ok(Self {
            uuid,
            text_uuid,
            note: get("note"),
            boundaries: payload.get("boundaries").and_then(|v| {
                match v {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    Value::Null | Value::String(_) => None,
                    other => Some(other.to_string()),
                }
            }),
            text_sha: get("text_sha"),
            is_default: payload.get("is_default").and_then(|v| v.as_bool()).unwrap_or(false),
            created_at: super::canonical_or_now(payload.get("created_at").and_then(|v| v.as_str())),
        })
    }

    fn to_payload(&self) -> Value {
        json!({
            "uuid": self.uuid,
            "text_uuid": self.text_uuid,
            "note": self.note,
            "boundaries": self.boundaries,
            "text_sha": self.text_sha,
            "is_default": self.is_default,
            "created_at": self.created_at,
        })
    }
}

/// What the UI and agents see. `kind`, `duration`, `stale` and `usable` are all
/// derived — the table does not carry them.
#[derive(Clone, Debug, Serialize)]
pub struct ReadStandardAudio {
    pub uuid: String,
    pub text_uuid: String,
    /// Free-form provenance shown verbatim: `Edge TTS · <voice>`, `Upload · <file>`.
    pub note: String,
    /// `"tts"` when the clip carries word boundaries, `"upload"` otherwise.
    pub kind: String,
    /// Seconds, decoded from the wav. `None` — JSON `null` — when this call did
    /// not decode it: the dataset-wide listing deliberately does not, and a
    /// missing wav has nothing to measure. A fabricated `0.0` would read as a
    /// real measurement, so absence is said out loud.
    pub duration: Option<f64>,
    /// Absolute path for playback, when the wav is on this device.
    pub audio_url: Option<String>,
    pub is_default: bool,
    /// The clip was made from an earlier version of the text.
    pub stale: bool,
    /// The wav exists here and is not stale: pronunciation will use it.
    pub usable: bool,
    pub created_at: String,
}

/// A decoded default clip, ready for scoring. `seconds()` is the denominator the
/// word grouping needs when the clip has no word timings.
pub(crate) struct StoredClip {
    pub uuid: String,
    /// 16 kHz mono PCM.
    pub samples: Vec<f32>,
    /// Empty for an upload — an upload has no word timings.
    pub boundaries: Vec<TtsWordBoundary>,
}

impl StoredClip {
    pub(crate) fn seconds(&self) -> f64 {
        self.samples.len() as f64 / audio::DECODED_SAMPLE_RATE as f64
    }
}

/// What a text's default clip amounts to. The two empty cases carry different
/// reasons at the scoring seam, which is why this is not an `Option`.
pub(crate) enum DefaultClip {
    /// The text has no clip on this device: pronunciation has nothing to compare.
    Missing,
    /// A clip exists but was made from an earlier version of the reference text.
    Stale,
    Ready(StoredClip),
}

/// A decoded upload, before it is cropped and stored.
#[derive(Serialize)]
pub struct AudioProbe {
    /// Seconds of the whole source.
    pub duration: f64,
    /// Interleaved min/max signed 8-bit pairs — the same layout `WaveformPeaks`
    /// produces, so the trim track draws it with no conversion.
    pub peaks: Vec<i8>,
    /// Seconds each pair covers, so the canvas maps x → time without guessing.
    pub seconds_per_pair: f64,
    /// What the source was decoded from, for the dialog's label.
    pub file_name: String,
}

// ============================================================
// Paths and helpers
// ============================================================

/// The one place the `media/<text_uuid>/<audio_uuid>.wav` layout is spelled, so
/// no caller can invent a second convention. The flat legacy
/// `media/<attempt_uuid>.wav` files cannot collide with it.
pub(crate) fn clip_path(dir: &Path, text_uuid: &str, audio_uuid: &str) -> PathBuf {
    dir.join("media").join(text_uuid).join(format!("{}.wav", audio_uuid))
}

/// sha256 of the effective reference — the exact string the phones are built
/// from, which is what makes staleness a comparison instead of a migration.
fn text_digest(reference: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(reference.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// The text's effective reference — what a clip is made from and what
/// [`text_digest`] is taken of. Spelled once so a caller in another module
/// cannot drift from what "the text" means.
pub(crate) fn text_reference(dir: &Path, text_uuid: &str) -> Result<String, String> {
    let conn = open_db(dir)?;
    conn.query_row(
        "SELECT content, normalized_content FROM read_text WHERE uuid = ?1",
        [text_uuid],
        |row| {
            let content: String = row.get(0)?;
            Ok(effective_reference(
                &content,
                row.get::<_, Option<String>>(1)?.as_deref(),
            ))
        },
    )
    .map_err(|_| format!("Text {} not found in dataset", text_uuid))
}

pub(crate) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS read_standard_audio (
            uuid       TEXT PRIMARY KEY,
            text_uuid  TEXT NOT NULL,
            note       TEXT NOT NULL DEFAULT '',
            boundaries TEXT,
            text_sha   TEXT NOT NULL DEFAULT '',
            is_default INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_read_standard_audio_text
            ON read_standard_audio(text_uuid, created_at);",
    )
    .map_err(|e| e.to_string())
}

fn row_count(conn: &Connection, text_uuid: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM read_standard_audio WHERE text_uuid = ?1",
        [text_uuid],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// Decode a source that arrived either as a filesystem path or as base64 bytes
/// (Android's picker returns a `content://` URI, which is not a path).
fn decode_source(source_path: Option<&str>, source_base64: Option<&str>) -> Result<Vec<f32>, String> {
    if let Some(raw) = source_base64.map(str::trim).filter(|s| !s.is_empty()) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .map_err(|e| format!("Invalid base64 audio: {}", e))?;
        return audio::decode_bytes_to_pcm(&bytes);
    }
    let path = source_path
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "No audio source given: pass `source_path` or `source_base64`".to_string())?;
    audio::decode_to_pcm(Path::new(path))
}

/// Write one row, honouring the one-default rule. A row that arrives flagged
/// default clears its siblings, so a replay converges instead of leaving two
/// defaults behind (the read path tolerates that anyway, but why store it).
fn write_row(conn: &Connection, row: &StandardAudioRow, replace: bool) -> Result<(), String> {
    if row.is_default {
        conn.execute(
            "UPDATE read_standard_audio SET is_default = 0 WHERE text_uuid = ?1 AND uuid <> ?2",
            rusqlite::params![row.text_uuid, row.uuid],
        )
        .map_err(|e| e.to_string())?;
    }
    let verb = if replace { "INSERT OR REPLACE" } else { "INSERT" };
    conn.execute(
        &format!(
            "{verb} INTO read_standard_audio \
             (uuid, text_uuid, note, boundaries, text_sha, is_default, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        ),
        rusqlite::params![
            row.uuid,
            row.text_uuid,
            row.note,
            row.boundaries,
            row.text_sha,
            row.is_default,
            row.created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn journal(settings: &SettingsState, kind: &str, dataset_uuid: &str, row: &StandardAudioRow) {
    let _ = crate::sync::change_log::commit_change(settings, kind, dataset_uuid, &row.to_payload());
}

/// Make the earliest remaining clip default. Returns its uuid so the promotion
/// can be journaled like any other save.
fn promote_earliest(conn: &Connection, text_uuid: &str) -> Result<Option<String>, String> {
    let earliest: Option<String> = conn
        .query_row(
            "SELECT uuid FROM read_standard_audio WHERE text_uuid = ?1 \
             ORDER BY created_at, rowid LIMIT 1",
            [text_uuid],
            |r| r.get(0),
        )
        .ok();
    if let Some(uuid) = earliest.clone() {
        conn.execute(
            "UPDATE read_standard_audio SET is_default = 1 WHERE uuid = ?1",
            [&uuid],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(earliest)
}

fn load_row(conn: &Connection, audio_uuid: &str) -> Result<Option<StandardAudioRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT uuid, text_uuid, note, boundaries, text_sha, is_default, created_at \
             FROM read_standard_audio WHERE uuid = ?1",
        )
        .map_err(|e| e.to_string())?;
    let row = stmt
        .query_row([audio_uuid], |r| {
            Ok(StandardAudioRow {
                uuid: r.get(0)?,
                text_uuid: r.get(1)?,
                note: r.get(2)?,
                boundaries: r.get(3)?,
                text_sha: r.get(4)?,
                is_default: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .ok();
    Ok(row)
}

// ============================================================
// Voice resolution
// ============================================================
//
// One place decides which voice a generation speaks with, and it says why. The
// log's agent had to guess a voice at step one — and, because `tts_list_voices`
// is alphabetical, guessed `de-AT-IngridNeural` (Austrian) for a German book.
// Encoding the choice as an ordered fallback plus a reported `source` is what
// makes "which German voice am I getting?" answerable instead of archaeological.

/// Which voice a generation will use, and where that choice came from.
///
/// `source` is one of `param` (the caller named it), `preference` (the voice
/// preferred for the dataset's language), `setting` (`read_aloud_tts_voice`,
/// the language-less fallback agents already set) or `default` (the built-in
/// multilingual voice).
#[derive(Clone, Debug, Serialize)]
pub struct VoiceResolution {
    pub voice: String,
    /// Parsed from the short name: `de-DE` for `de-DE-ConradNeural`.
    pub locale: String,
    pub source: String,
    /// The dataset's `info.json` language; empty when it has none.
    pub dataset_language: String,
    /// Keys of the configured preference map, so a caller can see what it may
    /// match without a second `settings_get`.
    pub languages_with_preference: Vec<String>,
    /// The chosen voice does not speak the dataset's language. A German book
    /// with an Austrian narrator is a legitimate choice; an accidental `en-US`
    /// one is the bug this names.
    pub warning: Option<String>,
    /// The dataset has no language at all, so no preference could apply.
    pub hint: Option<String>,
}

/// The locale of an Edge voice short name: `de-DE-ConradNeural` → `de-DE`.
/// Edge's catalogue is fetched, not compiled in, so this is parsed rather than
/// looked up. `""` for anything that does not have the `xx-YY-` shape.
pub(crate) fn voice_locale(voice: &str) -> String {
    let mut parts = voice.trim().split('-');
    let (language, region) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let shaped = |s: &str| s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic());
    if shaped(language) && shaped(region) {
        format!("{}-{}", language.to_ascii_lowercase(), region.to_ascii_uppercase())
    } else {
        String::new()
    }
}

/// The primary subtag of a language tag or locale: `de-DE` → `de`.
fn language_of(tag: &str) -> String {
    tag.trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Reject a preference that pairs a language with a voice that does not speak
/// it.
///
/// The map is only worth having if every pair is trustworthy: a raw
/// `settings_set {"tts_voice_by_language": {"de": "en-US-AriaNeural"}}` would be
/// the log's Austrian trap one layer up. There is no static catalogue to list
/// from — Edge's voices are fetched — so the message points at the tool that
/// does list them.
pub(crate) fn validate_preference(language: &str, voice: &str) -> Result<(), String> {
    let language = language_of(language);
    if language.is_empty() {
        return Err(
            "A preferred voice needs a language key. Prefer the language over a region: \
             `de` covers `de-DE` and `de-AT` datasets, `de-DE` covers only `de-DE` ones."
                .to_string(),
        );
    }
    let voice = voice.trim();
    let locale = voice_locale(voice);
    if locale.is_empty() {
        return Err(format!(
            "`{}` is not an Edge TTS voice short name (expected e.g. `de-DE-ConradNeural`)",
            voice
        ));
    }
    if language_of(&locale) != language {
        return Err(format!(
            "`{}` does not speak `{}` (it is `{}`). Call `tts_list_voices {{locale: \"{}\"}}` for the voices that do.",
            voice, language, locale, language
        ));
    }
    Ok(())
}

/// Validate every pair of a preference map — what `settings_set` runs before it
/// writes one.
pub(crate) fn validate_preferences(map: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
    for (language, voice) in map {
        validate_preference(language, voice)?;
    }
    Ok(())
}

/// Decide the voice for one generation: explicit `param` → the language's
/// `preference` → the language-less `setting` → the built-in `default`.
///
/// The `preference` lookup tries the dataset's exact tag first (`de-DE`), then
/// its primary subtag (`de`), so a user who set one voice for all German is not
/// overruled by a narrower key they never set.
pub(crate) fn resolve_voice(
    settings: &AppSettings,
    dataset_language: &str,
    explicit: Option<&str>,
) -> VoiceResolution {
    let dataset_language = dataset_language.trim().to_string();
    let language = language_of(&dataset_language);
    let mut languages_with_preference: Vec<String> =
        settings.tts_voice_by_language.keys().cloned().collect();
    languages_with_preference.sort();

    let choice = explicit
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|voice| (voice.to_string(), "param"))
        .or_else(|| {
            if language.is_empty() {
                return None;
            }
            // Exact tag, then primary subtag, both matched case-insensitively
            // so `de-de` in a hand-edited file still resolves.
            let mut candidates = vec![dataset_language.to_ascii_lowercase()];
            if language != dataset_language.to_ascii_lowercase() {
                candidates.push(language.clone());
            }
            candidates.into_iter().find_map(|key| {
                settings
                    .tts_voice_by_language
                    .iter()
                    .find(|(k, _)| k.trim().to_ascii_lowercase() == key)
                    .map(|(_, v)| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .map(|voice| (voice, "preference"))
            })
        })
        .or_else(|| {
            let configured = settings.read_aloud_tts_voice.trim();
            (!configured.is_empty()).then(|| (configured.to_string(), "setting"))
        })
        .unwrap_or_else(|| (super::DEFAULT_TTS_VOICE.to_string(), "default"));

    let locale = voice_locale(&choice.0);
    let warning = {
        let speaks = language_of(&locale);
        (!language.is_empty() && !speaks.is_empty() && speaks != language).then(|| {
            format!(
                "{} is a {} voice, but this dataset is {}. Fine on purpose — check that it is.",
                choice.0, locale, dataset_language
            )
        })
    };
    let hint = dataset_language.is_empty().then(|| {
        "This dataset has no language, so no preferred voice can apply. Set one with \
         `read_aloud_update {language: \"de\"}`, then choose a voice per language on the Edge TTS page."
            .to_string()
    });
    // The default is multilingual, so the reported locale is the voice's own
    // rather than a claim about what it speaks.
    VoiceResolution {
        voice: choice.0,
        locale,
        source: choice.1.to_string(),
        dataset_language,
        languages_with_preference,
        warning,
        hint,
    }
}

/// [`resolve_voice`] for a dataset on disk: reads its language first.
pub(crate) fn resolve_for_dataset(
    settings: &SettingsState,
    dataset_uuid: &str,
    explicit: Option<&str>,
) -> Result<VoiceResolution, String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let language = crate::datasets::info::read_info_opt(&dir)
        .map(|info| info.language)
        .unwrap_or_default();
    let snapshot = snapshot(settings);
    Ok(resolve_voice(&snapshot, &language, explicit))
}

/// The current settings, cloned out from under the lock so a caller can reason
/// about one consistent snapshot while it does work.
pub(crate) fn snapshot(settings: &SettingsState) -> AppSettings {
    settings
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// `(texts with a usable clip, texts)` for a dataset — the pair that turns
/// "how many are left?" into one subtraction instead of another listing.
pub(crate) fn audio_totals(settings: &SettingsState, dataset_uuid: &str) -> Result<(i64, i64), String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let conn = open_db(&dir)?;
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM read_text", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let with_audio: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT text_uuid) FROM read_standard_audio",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok((with_audio, total))
}

// ============================================================
// Reads
// ============================================================

/// One text's clips (`text_uuid: Some`) or every clip in the dataset (`None`),
/// default first. The dataset-wide listing is what badges the texts list, so it
/// does not decode the wavs — it only checks that they are on this device.
pub(crate) fn list(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: Option<&str>,
) -> Result<Vec<ReadStandardAudio>, String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let conn = open_db(&dir)?;
    ensure_schema(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT a.uuid, a.text_uuid, a.note, a.boundaries, a.text_sha, a.is_default, a.created_at, \
                    t.content, t.normalized_content \
             FROM read_standard_audio a LEFT JOIN read_text t ON t.uuid = a.text_uuid \
             WHERE (?1 IS NULL OR a.text_uuid = ?1) \
             ORDER BY a.text_uuid, a.is_default DESC, a.created_at, a.rowid",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![text_uuid], |r| {
            Ok((
                StandardAudioRow {
                    uuid: r.get(0)?,
                    text_uuid: r.get(1)?,
                    note: r.get(2)?,
                    boundaries: r.get(3)?,
                    text_sha: r.get(4)?,
                    is_default: r.get(5)?,
                    created_at: r.get(6)?,
                },
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let decode = text_uuid.is_some();
    let mut out = Vec::new();
    for (row, content, normalized) in rows.flatten() {
        let path = clip_path(&dir, &row.text_uuid, &row.uuid);
        let exists = path.exists();
        let expected = content
            .as_deref()
            .map(|c| text_digest(&effective_reference(c, normalized.as_deref())));
        let stale = match &expected {
            Some(digest) => !row.text_sha.is_empty() && &row.text_sha != digest,
            None => false,
        };
        let duration = if decode && exists {
            audio::decode_to_pcm(&path)
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| s.len() as f64 / audio::DECODED_SAMPLE_RATE as f64)
        } else {
            None
        };
        out.push(ReadStandardAudio {
            uuid: row.uuid,
            text_uuid: row.text_uuid,
            note: row.note,
            kind: if row.boundaries.is_some() { "tts" } else { "upload" }.to_string(),
            duration,
            audio_url: exists.then(|| path.to_string_lossy().into_owned()),
            is_default: row.is_default,
            stale,
            usable: exists && !stale,
            created_at: row.created_at,
        });
    }
    Ok(out)
}

/// The scoring seam: which clip this text should be measured against, decoded.
///
/// The flag is never trusted alone — a sync replay may leave two defaults, so
/// the read walks the priority order and returns the first clip that is present
/// and fresh. A clip whose text hash no longer matches is *not* silently
/// re-synthesised: it is reported stale so the user can regenerate it.
pub(crate) fn default_for(
    dir: &Path,
    text_uuid: &str,
    reference: &str,
) -> Result<DefaultClip, String> {
    let conn = open_db(dir)?;
    ensure_schema(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT uuid, boundaries, text_sha FROM read_standard_audio WHERE text_uuid = ?1 \
             ORDER BY is_default DESC, created_at, rowid",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, Option<String>, String)> = stmt
        .query_map([text_uuid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    if rows.is_empty() {
        return Ok(DefaultClip::Missing);
    }

    let expected = text_digest(reference);
    let mut saw_stale = false;
    for (audio_uuid, boundaries, text_sha) in rows {
        let path = clip_path(dir, text_uuid, &audio_uuid);
        if !path.exists() {
            continue;
        }
        if !text_sha.is_empty() && text_sha != expected {
            saw_stale = true;
            continue;
        }
        match audio::decode_to_pcm(&path) {
            Ok(samples) if !samples.is_empty() => {
                let boundaries = boundaries
                    .as_deref()
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or_default();
                return Ok(DefaultClip::Ready(StoredClip { uuid: audio_uuid, samples, boundaries }));
            }
            Ok(_) => log::warn!("read_aloud standard audio {} decoded to nothing", audio_uuid),
            Err(e) => log::warn!("read_aloud standard audio {} could not be decoded: {}", audio_uuid, e),
        }
    }
    if saw_stale {
        Ok(DefaultClip::Stale)
    } else {
        Ok(DefaultClip::Missing)
    }
}

// ============================================================
// Probe (read-only: nothing is written)
// ============================================================

pub(crate) fn probe(
    source_path: Option<&str>,
    source_base64: Option<&str>,
    file_name: Option<&str>,
) -> Result<AudioProbe, String> {
    let samples = decode_source(source_path, source_base64)?;
    if samples.is_empty() {
        return Err("That file decoded to no audio".to_string());
    }
    let (peaks, per_pair) = audio::peaks_from_samples(&samples, PROBE_COLUMNS);
    let name = file_name
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            source_path
                .and_then(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .unwrap_or_default();
    Ok(AudioProbe {
        duration: samples.len() as f64 / audio::DECODED_SAMPLE_RATE as f64,
        peaks,
        seconds_per_pair: per_pair as f64 / audio::DECODED_SAMPLE_RATE as f64,
        file_name: name,
    })
}

// ============================================================
// Span guess (read-only: nothing is written)
// ============================================================

/// A little air either side of the guessed span. A model's word boundary is a
/// coarse cut — it can land inside the first consonant — and a reference clip
/// that clips its first or last sound is a worse reference than one with a
/// fifth of a second of room around it.
pub(crate) const GUESS_PAD_BEFORE: f64 = 0.2;
pub(crate) const GUESS_PAD_AFTER: f64 = 0.3;

/// A guess is only offered when the model heard this much of the text, and at
/// least [`GUESS_MIN_WORDS`] of it. Below that the "span" would be a fragment
/// of the reading presented as the whole of it — worse than no guess, because
/// it looks like an answer.
pub(crate) const GUESS_MIN_MATCH_RATIO: f64 = 0.5;
pub(crate) const GUESS_MIN_WORDS: usize = 3;

/// How many words the model may skip — in the text or in the recording — before
/// a match counts as a different stretch instead of part of the same reading.
/// Roughly what one badly read sentence costs.
pub(crate) const GUESS_MAX_GAP: usize = 6;

/// Where a text was read inside a longer recording.
#[derive(Debug, Clone, Serialize)]
pub struct SpanGuess {
    /// Guessed start of the clip, in seconds: padded, clamped, rounded to the
    /// tenth of a second the crop track can show.
    pub start: f64,
    pub end: f64,
    /// Words of the text the model heard in order, out of `reference_words`.
    pub matched_words: usize,
    pub reference_words: usize,
    /// `matched_words / reference_words`. Well under 1 means the model misheard
    /// a good deal, so the two edges deserve a look before saving.
    pub confidence: f64,
    /// Everything the model heard in the file, so a wrong span can be diagnosed
    /// rather than only wondered at.
    pub transcript: String,
}

/// The longest run of aligned pairs that stays contiguous in both streams.
///
/// Alignment alone is not a span: a common word ("the", "a", "und") also
/// appears before and after the reading, and those stray matches would stretch
/// the span to cover most of the file. A run breaks where the model skipped
/// more than [`GUESS_MAX_GAP`] words on either side, so the reading survives as
/// one run while stray matches fall outside it. Ties go to the earlier run: if
/// the text really is read twice, the first reading is the one to keep.
pub(crate) fn densest_run(pairs: &[(usize, usize)]) -> &[(usize, usize)] {
    let mut best = &pairs[0..0];
    let mut from = 0;
    for i in 1..=pairs.len() {
        let breaks = i == pairs.len()
            || pairs[i].0.saturating_sub(pairs[i - 1].0) > GUESS_MAX_GAP
            || pairs[i].1.saturating_sub(pairs[i - 1].1) > GUESS_MAX_GAP;
        if breaks {
            if i - from > best.len() {
                best = &pairs[from..i];
            }
            from = i;
        }
    }
    best
}

pub(crate) fn round_tenth(seconds: f64) -> f64 {
    (seconds * 10.0).round() / 10.0
}

/// Turn a model failure into something the user can act on. The two failures
/// this call actually produces are "no model" and "this model reports no word
/// timings", and neither is the user's fault as phrased by the model layer.
#[cfg(feature = "stt")]
pub(crate) fn guess_error(message: String) -> String {
    if message.contains("No model is loaded") || message.contains("No model") {
        "No speech model is loaded, so the file cannot be listened to. Load one on the Models page, then try again — or set the edges by hand.".to_string()
    } else if message.contains("Word-level timestamps") {
        format!(
            "{}, so this guess needs a model that reports word timings (a Parakeet model). Choose one on the Models page, or set the edges by hand.",
            message.trim_end_matches('.')
        )
    } else {
        message
    }
}

/// Find where this text is spoken inside a longer recording.
///
/// A text is a *reading* of some part of the file, so the file cannot be cut by
/// listening for silence: the reading may begin in the middle of a sentence and
/// the file may hold far more than one text. Instead the model transcribes the
/// whole file with word timings, that transcript is aligned to the text through
/// the same preprocessing scoring uses, and the matched words give the span.
///
/// Deliberately a suggestion: nothing is written, the edges only move, and
/// dragging, nudging or typing over them still works afterwards.
#[cfg(feature = "stt")]
pub(crate) fn guess_span(
    model_state: &crate::models::ModelState,
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
    source_path: Option<&str>,
    source_base64: Option<&str>,
) -> Result<SpanGuess, String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let reference = text_reference(&dir, text_uuid)?;
    let reference_tokens = asr_metric::normalize_tokens(&reference);
    if reference_tokens.is_empty() {
        return Err("That text has no words to look for".to_string());
    }

    let samples = decode_source(source_path, source_base64)?;
    if samples.is_empty() {
        return Err("That file decoded to no audio".to_string());
    }
    let duration = samples.len() as f64 / audio::DECODED_SAMPLE_RATE as f64;

    log::info!(
        "[ReadAloud] guessing the span of text {} ({:.0}s) in {:.0}s of audio",
        text_uuid,
        reference_tokens.len(),
        duration
    );
    let heard = crate::models::transcribe_samples_word_level(model_state, &samples)
        .map_err(guess_error)?;

    // The hypothesis is the transcript re-tokenized through the same
    // preprocessing as the reference, plus a token → spoken-word map, because
    // only the spoken words carry times.
    let mut hypothesis: Vec<String> = Vec::new();
    let mut word_of_token: Vec<usize> = Vec::new();
    let mut times: Vec<(f32, f32)> = Vec::new();
    for word in heard.segments.unwrap_or_default() {
        let tokens = asr_metric::normalize_tokens(&word.text);
        if tokens.is_empty() {
            continue;
        }
        let index = times.len();
        times.push((word.start, word.end));
        for token in tokens {
            hypothesis.push(token);
            word_of_token.push(index);
        }
    }
    if hypothesis.is_empty() {
        return Err("The model heard no words in that file, so there is nothing to match the text against. Check the recording, or set the edges by hand.".to_string());
    }

    let pairs = asr_metric::aligned_pairs(&reference_tokens, &hypothesis);
    let run = densest_run(&pairs);
    let matched = run.len();
    let needed = (((reference_tokens.len() as f64) * GUESS_MIN_MATCH_RATIO).ceil() as usize)
        .max(GUESS_MIN_WORDS.min(reference_tokens.len()));
    if matched < needed {
        return Err(format!(
            "The model heard only {} of this text's {} words in that file, which is too little to place the reading. Try a stronger model, or set the edges by hand.",
            matched,
            reference_tokens.len()
        ));
    }

    // A run that starts or ends well inside the text matches a stretch of the
    // reading, not the reading: the sentences outside it would fall outside the
    // clip, so the guess is refused rather than offered as a near miss. A few
    // words at either end may be misheard — that is what the gap allows.
    let first_ref = run[0].0;
    let tail_ref = reference_tokens.len() - 1 - run[matched - 1].0;
    if first_ref > GUESS_MAX_GAP || tail_ref > GUESS_MAX_GAP {
        return Err(format!(
            "The model heard {} of this text's {} words, but not where it starts or ends, so no single clip contains the whole reading. Set the edges by hand.",
            matched,
            reference_tokens.len()
        ));
    }

    let (heard_start, _) = times[word_of_token[run[0].1]];
    let (_, heard_end) = times[word_of_token[run[matched - 1].1]];
    // The word boundaries are where the reading is; where the *clip* starts is
    // the middle of the pause that belongs to that word, so a neighbouring clip
    // and this one share the silence instead of one of them owning it. That pause
    // can be most of a second away, because the model's word times are loose.
    let loudness = edges::loudness_from_samples(&samples, edges::EDGE_FRAME_SECS);
    let start = round_tenth(edges::place_edge(
        &loudness,
        edges::EDGE_FRAME_SECS,
        heard_start as f64,
        GUESS_PAD_BEFORE,
        edges::Side::Head,
        0.0,
        duration,
    ));
    let mut end = round_tenth(edges::place_edge(
        &loudness,
        edges::EDGE_FRAME_SECS,
        heard_end as f64,
        GUESS_PAD_AFTER,
        edges::Side::Tail,
        start + MIN_CLIP_SECS,
        duration,
    ));
    if end - start < MIN_CLIP_SECS {
        end = round_tenth((start + MIN_CLIP_SECS).min(duration));
    }

    Ok(SpanGuess {
        start,
        end,
        matched_words: matched,
        reference_words: reference_tokens.len(),
        confidence: (matched as f64 / reference_tokens.len() as f64 * 100.0).round() / 100.0,
        transcript: heard.text,
    })
}

// ============================================================
// Writes
// ============================================================

/// Say the text with a resolved voice and store the result, reporting the clip
/// and the choice together so a caller can see *which* voice it got. The note
/// defaults to the voice, so a text's clip list reads like a list of
/// recordings, not of uuids.
pub(crate) async fn generate(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
    voice: Option<&str>,
    note: Option<&str>,
) -> Result<(ReadStandardAudio, VoiceResolution), String> {
    let resolved = resolve_for_dataset(settings, dataset_uuid, voice)?;
    let clip = generate_with(settings, dataset_uuid, text_uuid, &resolved, note).await?;
    Ok((clip, resolved))
}

/// [`generate`] with the voice already decided — the one body both the single
/// tool and the batch runner go through, so a fix to one is a fix to both.
pub(crate) async fn generate_with(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
    resolved: &VoiceResolution,
    note: Option<&str>,
) -> Result<ReadStandardAudio, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let reference = text_reference(&dir, text_uuid)?;
    if reference.trim().is_empty() {
        return Err("That text is empty: there is nothing to say".to_string());
    }

    let voice = resolved.voice.as_str();
    log::info!(
        "[ReadAloud] synthesising standard audio for text {} ({} chars, {})",
        text_uuid,
        reference.len(),
        voice
    );
    let spoken = edge_tts::synthesize_with_boundaries(&reference, voice, None, None, None)
        .await
        .map_err(|e| format!("Edge TTS could not synthesise this text: {}", e))?;
    if spoken.boundaries.is_empty() {
        return Err("Edge TTS returned no word boundaries for this text".to_string());
    }
    let samples = audio::decode_bytes_to_pcm(&spoken.audio)
        .map_err(|e| format!("The synthesised audio could not be decoded: {}", e))?;
    if samples.is_empty() {
        return Err("The synthesised audio decoded to nothing".to_string());
    }

    let audio_uuid = Uuid::new_v4().to_string();
    let path = clip_path(&dir, text_uuid, &audio_uuid);
    audio::write_wav(&path, &samples, audio::DECODED_SAMPLE_RATE, 1)?;

    let conn = open_db(&dir)?;
    ensure_schema(&conn)?;
    let row = StandardAudioRow {
        uuid: audio_uuid.clone(),
        text_uuid: text_uuid.to_string(),
        note: note
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Edge TTS · {}", voice)),
        boundaries: serde_json::to_string(&spoken.boundaries).ok(),
        text_sha: text_digest(&reference),
        is_default: row_count(&conn, text_uuid)? == 0,
        created_at: super::now_row_stamp(),
    };
    write_row(&conn, &row, false)?;
    let _ = super::touch_info(&dir);
    journal(settings, "read_standard_audio_save", dataset_uuid, &row);

    Ok(ReadStandardAudio {
        uuid: row.uuid,
        text_uuid: row.text_uuid,
        note: row.note,
        kind: "tts".to_string(),
        duration: Some(samples.len() as f64 / audio::DECODED_SAMPLE_RATE as f64),
        audio_url: Some(path.to_string_lossy().into_owned()),
        is_default: row.is_default,
        stale: false,
        usable: true,
        created_at: row.created_at,
    })
}

/// Roughly how fast a text is spoken: 150 words a minute at about five
/// characters a word. Only used to size a `dry_run`, never to fake a duration.
const SPEAKING_CHARS_PER_SEC: f64 = 12.5;

/// Bulk generation over `ORDER BY order_num`, through the same body as
/// [`generate`] — the batch tool adds ordering, `only_missing` and `limit`, not
/// a second synthesis implementation.
///
/// This is what makes the probe path cheap: `limit: 3` speaks exactly the first
/// three passages in one call so a human can listen before committing to the
/// rest, and `only_missing` makes a repeat safe. `dry_run` reports what would
/// happen and writes nothing. Progress is visible by `remaining` decreasing —
/// no job machinery, no lease.
pub(crate) async fn generate_all(
    settings: &SettingsState,
    dataset_uuid: &str,
    voice: Option<&str>,
    only_missing: bool,
    limit: Option<i64>,
    dry_run: bool,
) -> Result<Value, String> {
    let resolved = resolve_for_dataset(settings, dataset_uuid, voice)?;
    // Bounded per call: a clip takes seconds to synthesise, so an unbounded
    // batch would outlive any caller's patience. A caller repeats until
    // `remaining` is 0.
    let limit = limit.unwrap_or(10).clamp(1, 500);
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let targets: Vec<(String, i64)> = {
        let conn = open_db(&dir)?;
        let mut stmt = conn
            .prepare(
                "SELECT t.uuid, LENGTH(t.content) FROM read_text t \
                 WHERE (?1 = 0 OR NOT EXISTS ( \
                     SELECT 1 FROM read_standard_audio a WHERE a.text_uuid = t.uuid)) \
                 ORDER BY t.order_num, t.rowid LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![only_missing, limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.flatten().collect()
    };
    let (with_audio, total) = audio_totals(settings, dataset_uuid)?;
    let context = |extra: Value| {
        let mut base = json!({
            "status": "ok",
            "voice": resolved.voice,
            "voice_locale": resolved.locale,
            "voice_source": resolved.source,
            "texts_with_audio": with_audio,
            "texts_total": total,
        });
        if let (Value::Object(map), Value::Object(extra)) = (&mut base, extra) {
            map.extend(extra);
        }
        base
    };

    if dry_run {
        let chars: i64 = targets.iter().map(|(_, c)| *c).sum();
        let mut out = context(json!({
            "dry_run": true,
            "would_generate": targets.len(),
            "estimate_seconds": (chars as f64 / SPEAKING_CHARS_PER_SEC).round() as i64,
            "remaining": total - with_audio,
        }));
        if let (Value::Object(map), Some(warning)) = (&mut out, resolved.warning.clone()) {
            map.insert("voice_warning".into(), json!(warning));
        }
        if let (Value::Object(map), Some(hint)) = (&mut out, resolved.hint.clone()) {
            map.insert("hint".into(), json!(hint));
        }
        return Ok(out);
    }

    let mut generated = 0usize;
    let mut failed = Vec::new();
    for (text_uuid, _) in &targets {
        match generate_with(settings, dataset_uuid, text_uuid, &resolved, None).await {
            Ok(_) => generated += 1,
            Err(error) => failed.push(json!({ "text_uuid": text_uuid, "error": error })),
        }
    }
    let (with_audio, total) = audio_totals(settings, dataset_uuid)?;
    let failed_count = failed.len();
    let mut out = context(json!({
        "dry_run": false,
        "generated": generated,
        "failed": failed,
        "failed_count": failed_count,
        "texts_with_audio": with_audio,
        "texts_total": total,
        "remaining": total - with_audio,
    }));
    if let (Value::Object(map), Some(warning)) = (&mut out, resolved.warning.clone()) {
        map.insert("voice_warning".into(), json!(warning));
    }
    Ok(out)
}

/// Write a decoded clip and its row — the one body an upload, a synthesis and
/// a source-alignment crop all go through, so the wav layout, the one-default
/// rule and the journal entry are spelled once.
///
/// The row carries no word timings, which is how `kind` stays derivable: only a
/// TTS clip knows where its words are.
pub(crate) fn store_samples(
    settings: &SettingsState,
    dataset_uuid: &str,
    dir: &Path,
    text_uuid: &str,
    reference: &str,
    clip: &[f32],
    note: &str,
) -> Result<ReadStandardAudio, String> {
    let audio_uuid = Uuid::new_v4().to_string();
    let path = clip_path(dir, text_uuid, &audio_uuid);
    audio::write_wav(&path, clip, audio::DECODED_SAMPLE_RATE, 1)?;

    let conn = open_db(dir)?;
    ensure_schema(&conn)?;
    let row = StandardAudioRow {
        uuid: audio_uuid.clone(),
        text_uuid: text_uuid.to_string(),
        note: {
            let trimmed = note.trim();
            if trimmed.is_empty() {
                "Upload".to_string()
            } else {
                trimmed.to_string()
            }
        },
        boundaries: None,
        text_sha: text_digest(reference),
        is_default: row_count(&conn, text_uuid)? == 0,
        created_at: super::now_row_stamp(),
    };
    write_row(&conn, &row, false)?;
    let _ = super::touch_info(dir);
    journal(settings, "read_standard_audio_save", dataset_uuid, &row);

    Ok(ReadStandardAudio {
        uuid: row.uuid,
        text_uuid: row.text_uuid,
        note: row.note,
        kind: "upload".to_string(),
        duration: Some(clip.len() as f64 / audio::DECODED_SAMPLE_RATE as f64),
        audio_url: Some(path.to_string_lossy().into_owned()),
        is_default: row.is_default,
        stale: false,
        usable: true,
        created_at: row.created_at,
    })
}

/// Store a picked (and cut) recording as this text's standard audio. `start` and
/// `end` are seconds into the source; `None` means the whole thing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn import(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
    source_path: Option<&str>,
    source_base64: Option<&str>,
    file_name: Option<&str>,
    note: Option<&str>,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<ReadStandardAudio, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let reference = text_reference(&dir, text_uuid)?;

    let samples = decode_source(source_path, source_base64)?;
    if samples.is_empty() {
        return Err("That file decoded to no audio".to_string());
    }
    let rate = audio::DECODED_SAMPLE_RATE as f64;
    let total = samples.len() as f64 / rate;
    let from = start.unwrap_or(0.0).clamp(0.0, total);
    let to = end.unwrap_or(total).clamp(0.0, total);
    if to - from < MIN_CLIP_SECS {
        return Err(format!(
            "The selected range is {:.2} s; a standard audio clip needs at least {:.1} s",
            (to - from).max(0.0),
            MIN_CLIP_SECS
        ));
    }
    let first = (from * rate).round() as usize;
    let last = ((to * rate).round() as usize).min(samples.len());
    let clip = &samples[first.min(last)..last];

    let note = note
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            file_name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(|n| format!("Upload · {}", n))
        })
        .unwrap_or_else(|| "Upload".to_string());

    store_samples(
        settings,
        dataset_uuid,
        &dir,
        text_uuid,
        &reference,
        clip,
        &note,
    )
}

/// Move the default marker. One transaction, so the read path can never observe
/// a text with no default at all.
pub(crate) fn set_default(
    settings: &SettingsState,
    dataset_uuid: &str,
    audio_uuid: &str,
) -> Result<(), String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let conn = open_db(&dir)?;
    ensure_schema(&conn)?;
    let Some(row) = load_row(&conn, audio_uuid)? else {
        return Err(format!("Standard audio {} not found", audio_uuid));
    };
    conn.execute(
        "UPDATE read_standard_audio SET is_default = CASE WHEN uuid = ?1 THEN 1 ELSE 0 END \
         WHERE text_uuid = ?2",
        rusqlite::params![audio_uuid, row.text_uuid],
    )
    .map_err(|e| e.to_string())?;
    let _ = super::touch_info(&dir);
    let mut saved = row;
    saved.is_default = true;
    journal(settings, "read_standard_audio_save", dataset_uuid, &saved);
    Ok(())
}

/// Delete one clip: its row, its wav, and — if it was the default — promote the
/// earliest remaining so the text still has one.
pub(crate) fn delete(
    settings: &SettingsState,
    dataset_uuid: &str,
    audio_uuid: &str,
) -> Result<(), String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let conn = open_db(&dir)?;
    ensure_schema(&conn)?;
    let Some(row) = load_row(&conn, audio_uuid)? else {
        return Ok(());
    };
    conn.execute("DELETE FROM read_standard_audio WHERE uuid = ?1", [audio_uuid])
        .map_err(|e| e.to_string())?;
    let promoted = if row.is_default {
        promote_earliest(&conn, &row.text_uuid)?
    } else {
        None
    };
    let _ = fs::remove_file(clip_path(&dir, &row.text_uuid, audio_uuid));
    // Remove the per-text directory only when it is now empty, so a text with no
    // clips leaves nothing behind for the file manifest to carry.
    let _ = fs::remove_dir(dir.join("media").join(&row.text_uuid));
    let _ = super::touch_info(&dir);

    let _ = crate::sync::change_log::commit_change(
        settings,
        "read_standard_audio_delete",
        dataset_uuid,
        &json!({ "uuid": audio_uuid, "text_uuid": row.text_uuid }),
    );
    if let Some(uuid) = promoted {
        if let Some(saved) = load_row(&conn, &uuid)? {
            journal(settings, "read_standard_audio_save", dataset_uuid, &saved);
        }
    }
    Ok(())
}

/// Drop every clip belonging to a text — rows and wavs. Called when the text is
/// deleted (locally or by a replayed remote delete), so a dataset never keeps
/// audio for a text that no longer exists.
pub(crate) fn cascade_delete_for_text(
    settings: &SettingsState,
    dataset_uuid: &str,
    dir: &Path,
    text_uuid: &str,
) -> Result<(), String> {
    let conn = open_db(dir)?;
    ensure_schema(&conn)?;
    let uuids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT uuid FROM read_standard_audio WHERE text_uuid = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([text_uuid], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    if uuids.is_empty() {
        return Ok(());
    }
    conn.execute("DELETE FROM read_standard_audio WHERE text_uuid = ?1", [text_uuid])
        .map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(dir.join("media").join(text_uuid));
    for uuid in uuids {
        let _ = crate::sync::change_log::commit_change(
            settings,
            "read_standard_audio_delete",
            dataset_uuid,
            &json!({ "uuid": uuid, "text_uuid": text_uuid }),
        );
    }
    Ok(())
}

// ============================================================
// Replay (a journaled change arriving from another device)
// ============================================================

/// Replay one journaled `read_standard_audio_save`. The wav itself rides the
/// file manifest, so a row can legitimately arrive before its file — the read
/// path reports the clip as unusable until it does.
pub(crate) fn apply_remote_save(
    settings: &SettingsState,
    dataset_uuid: &str,
    payload: &Value,
) -> Result<bool, String> {
    let row = StandardAudioRow::from_payload(payload)?;
    let dataset_uuid = if dataset_uuid.is_empty() {
        payload
            .get("dataset_uuid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        dataset_uuid.to_string()
    };
    if dataset_uuid.is_empty() {
        return Err("read_standard_audio_save change carries no dataset_uuid".to_string());
    }
    let dir = find_dataset_dir_write(settings, &dataset_uuid)?;
    let conn = open_db(&dir)?;
    ensure_schema(&conn)?;
    write_row(&conn, &row, true)?;
    let _ = super::touch_info(&dir);
    // Re-journal so other followers converge. `commit_change` self-suppresses
    // while a remote change is being applied, so this is a no-op locally.
    journal(settings, "read_standard_audio_save", &dataset_uuid, &row);
    Ok(true)
}

/// Replay one journaled `read_standard_audio_delete`: drop the row and the local
/// copy of the wav.
pub(crate) fn apply_remote_delete(
    settings: &SettingsState,
    dataset_uuid: &str,
    payload: &Value,
) -> Result<bool, String> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let audio_uuid = get("uuid");
    if audio_uuid.is_empty() {
        return Err("read_standard_audio_delete change carries no uuid".to_string());
    }
    let dataset_uuid = if dataset_uuid.is_empty() { get("dataset_uuid") } else { dataset_uuid.to_string() };
    let dir = find_dataset_dir_write(settings, &dataset_uuid)?;
    let conn = open_db(&dir)?;
    ensure_schema(&conn)?;
    let row = load_row(&conn, &audio_uuid)?;
    let deleted = conn
        .execute("DELETE FROM read_standard_audio WHERE uuid = ?1", [&audio_uuid])
        .map_err(|e| e.to_string())?;
    if let Some(row) = &row {
        let _ = fs::remove_file(clip_path(&dir, &row.text_uuid, &audio_uuid));
        let _ = fs::remove_dir(dir.join("media").join(&row.text_uuid));
    }
    let _ = super::touch_info(&dir);
    let _ = crate::sync::change_log::commit_change(
        settings,
        "read_standard_audio_delete",
        &dataset_uuid,
        &json!({ "uuid": audio_uuid, "text_uuid": row.map(|r| r.text_uuid).unwrap_or_default() }),
    );
    Ok(deleted > 0)
}

// ============================================================
// Commands
// ============================================================

/// List a text's standard audio, or every clip in the dataset when `text_uuid`
/// is omitted (what the texts list uses for its badges). Wavs that are missing on
/// this device are still listed, with `usable = false`.
#[tauri::command]
pub async fn read_aloud_standard_audio_list(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: Option<String>,
) -> Result<Vec<ReadStandardAudio>, String> {
    list(&settings, &dataset_uuid, text_uuid.as_deref())
}

/// Read a picked file (or an uploaded blob) and report its duration and peaks so
/// the crop track can draw it. Writes nothing.
#[tauri::command]
pub async fn read_aloud_standard_audio_probe(
    source_path: Option<String>,
    source_base64: Option<String>,
    file_name: Option<String>,
) -> Result<AudioProbe, String> {
    probe(
        source_path.as_deref(),
        source_base64.as_deref(),
        file_name.as_deref(),
    )
}

/// Guess where this text is spoken inside a longer recording, by listening to
/// the file with the loaded STT model and aligning what it heard to the text.
///
/// Writes nothing: the caller gets the two edges to apply to the crop, and can
/// still drag, nudge or type over them. The whole file is transcribed, so this
/// costs more the longer the file is.
#[cfg(feature = "stt")]
#[tauri::command]
pub async fn read_aloud_standard_audio_guess(
    settings: State<'_, SettingsState>,
    model_state: State<'_, crate::models::ModelState>,
    dataset_uuid: String,
    text_uuid: String,
    source_path: Option<String>,
    source_base64: Option<String>,
) -> Result<SpanGuess, String> {
    guess_span(
        model_state.inner(),
        settings.inner(),
        &dataset_uuid,
        &text_uuid,
        source_path.as_deref(),
        source_base64.as_deref(),
    )
}

/// Without the `stt` feature there is no model to listen with; say so instead of
/// leaving the caller with an unknown command.
#[cfg(not(feature = "stt"))]
#[tauri::command]
pub async fn read_aloud_standard_audio_guess(
    _settings: State<'_, SettingsState>,
    _dataset_uuid: String,
    _text_uuid: String,
    _source_path: Option<String>,
    _source_base64: Option<String>,
) -> Result<SpanGuess, String> {
    Err(
        "This build has no speech recognition, so the span cannot be guessed. Set the edges by hand."
            .to_string(),
    )
}

/// Store a picked recording as this text's standard audio, optionally cut to
/// `[start, end)`. The first clip for a text becomes its default.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn read_aloud_standard_audio_import(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
    source_path: Option<String>,
    source_base64: Option<String>,
    file_name: Option<String>,
    note: Option<String>,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<ReadStandardAudio, String> {
    import(
        &settings,
        &dataset_uuid,
        &text_uuid,
        source_path.as_deref(),
        source_base64.as_deref(),
        file_name.as_deref(),
        note.as_deref(),
        start,
        end,
    )
}

/// Which voice a generation for this dataset would use, and why. Read-only:
/// what the read-aloud page renders as its provenance line.
#[tauri::command]
pub async fn read_aloud_voice_resolve(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    voice: Option<String>,
) -> Result<VoiceResolution, String> {
    resolve_for_dataset(&settings, &dataset_uuid, voice.as_deref())
}

/// Synthesise this text with Edge TTS and store the result as a standard clip.
/// `voice` defaults to the voice preferred for the dataset's language (see
/// [`resolve_voice`]).
#[tauri::command]
pub async fn read_aloud_standard_audio_generate(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
    voice: Option<String>,
    note: Option<String>,
) -> Result<ReadStandardAudio, String> {
    // The UI already knows the voice it asked for; the resolution is what the
    // MCP tool reports, so the command keeps returning just the clip and the
    // frontend's call site needs no change.
    let (clip, _) = generate(&settings, &dataset_uuid, &text_uuid, voice.as_deref(), note.as_deref()).await?;
    Ok(clip)
}

/// Make one clip the default the scorer reads. Exactly one stays default.
#[tauri::command]
pub async fn read_aloud_standard_audio_set_default(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    uuid: String,
) -> Result<(), String> {
    set_default(&settings, &dataset_uuid, &uuid)
}

/// Delete one clip: its row and its wav. Deleting the default promotes the
/// earliest remaining clip, so a text with clips always has a default.
#[tauri::command]
pub async fn read_aloud_standard_audio_delete(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    uuid: String,
) -> Result<(), String> {
    delete(&settings, &dataset_uuid, &uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::read_aloud::tests_support::{dataset_with_text, temp_settings};

    /// Write a 1 s silence so the file half of a row is present.
    fn write_clip(settings: &SettingsState, dataset_uuid: &str, text_uuid: &str, audio_uuid: &str) {
        let dir = find_dataset_dir(settings, dataset_uuid).unwrap();
        let path = clip_path(&dir, text_uuid, audio_uuid);
        let samples = vec![0.0f32; audio::DECODED_SAMPLE_RATE as usize / 2];
        audio::write_wav(&path, &samples, audio::DECODED_SAMPLE_RATE, 1).unwrap();
    }

    /// Insert a row directly, the way a raw replay would — deliberately without
    /// the one-default rule, so a test can build the two-defaults state a replay
    /// can leave behind.
    fn insert_row(
        settings: &SettingsState,
        dataset_uuid: &str,
        text_uuid: &str,
        audio_uuid: &str,
        is_default: bool,
        text_sha: &str,
        created_at: &str,
    ) {
        let dir = find_dataset_dir(settings, dataset_uuid).unwrap();
        let conn = open_db(&dir).unwrap();
        ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO read_standard_audio \
             (uuid, text_uuid, note, boundaries, text_sha, is_default, created_at) \
             VALUES (?1, ?2, 'test', NULL, ?3, ?4, ?5)",
            rusqlite::params![audio_uuid, text_uuid, text_sha, is_default, created_at],
        )
        .unwrap();
    }

    fn digest(text: &str) -> String {
        text_digest(text)
    }

    /// The voice rule: the dataset's language decides, and the resolution says
    /// which layer decided. One test walks the whole precedence chain, because
    /// the bugs live in the boundaries between the layers rather than inside
    /// any one of them.
    #[test]
    fn the_dataset_language_decides_the_voice_and_the_resolution_says_why() {
        let mut settings = AppSettings::default();
        settings.read_aloud_tts_voice = "en-US-AriaNeural".to_string();
        settings
            .tts_voice_by_language
            .insert("de".to_string(), "de-DE-ConradNeural".to_string());

        // A preference for the dataset's language beats the language-less
        // setting, and reports itself as the reason.
        let res = resolve_voice(&settings, "de", None);
        assert_eq!(res.voice, "de-DE-ConradNeural");
        assert_eq!(res.source, "preference");
        assert_eq!(res.locale, "de-DE");
        assert!(res.warning.is_none() && res.hint.is_none());

        // An exact tag beats the bare language it shares a prefix with — a
        // preference filed under `de-AT` must not be overruled by `de`…
        settings
            .tts_voice_by_language
            .insert("de-AT".to_string(), "de-AT-IngridNeural".to_string());
        assert_eq!(
            resolve_voice(&settings, "de-AT", None).voice,
            "de-AT-IngridNeural"
        );
        // …while a dataset that only says `de` still gets the `de` preference,
        // and a narrower dataset tag falls back to it when the tag is unset.
        assert_eq!(resolve_voice(&settings, "de", None).voice, "de-DE-ConradNeural");
        assert_eq!(resolve_voice(&settings, "de-DE", None).voice, "de-DE-ConradNeural");

        // No preference for the language: the language-less setting.
        let res = resolve_voice(&settings, "fr", None);
        assert_eq!(res.voice, "en-US-AriaNeural");
        assert_eq!(res.source, "setting");

        // An explicit voice wins over everything, and a mismatch is still
        // reported: a German book with an American narrator is a legitimate
        // choice only when it is a deliberate one.
        let res = resolve_voice(&settings, "de", Some("en-US-AriaNeural"));
        assert_eq!(res.source, "param");
        assert!(res.warning.is_some(), "a language mismatch has to be flagged");

        // No language at all: nothing can match, so the setting applies and the
        // hint says what is missing.
        let res = resolve_voice(&settings, "", None);
        assert_eq!(res.source, "setting");
        assert!(res.hint.is_some());

        // Nothing configured anywhere: the built-in default.
        let res = resolve_voice(&AppSettings::default(), "", None);
        assert_eq!(res.voice, crate::datasets::read_aloud::DEFAULT_TTS_VOICE);
        assert_eq!(res.source, "default");
    }

    /// A preference map is only worth having if every pair is trustworthy, so
    /// the write seam refuses a mismatched one — the log's Austrian trap one
    /// layer up.
    #[test]
    fn a_preference_must_pair_a_language_with_a_voice_that_speaks_it() {
        let mut map = std::collections::BTreeMap::new();
        map.insert("de".to_string(), "de-AT-IngridNeural".to_string());
        assert!(
            validate_preferences(&map).is_ok(),
            "Austrian German is German"
        );

        map.insert("de".to_string(), "en-US-AriaNeural".to_string());
        let err = validate_preferences(&map).unwrap_err();
        assert!(err.contains("does not speak"), "{err}");
        assert!(
            err.contains("tts_list_voices"),
            "the refusal must say where the valid voices are: {err}"
        );

        // A key that is not a language, and a value that is not a voice.
        assert!(validate_preference("", "de-DE-ConradNeural").is_err());
        assert!(validate_preference("de", "Conrad").is_err());
    }

    #[test]
    fn the_first_clip_is_default_and_a_later_one_is_not() {
        let (settings, _dir) = temp_settings("sa-first");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        write_clip(&settings, &ds, &text, "a");
        insert_row(&settings, &ds, &text, "a", true, &digest("hello world"), "2026-01-01T00:00:00Z");
        write_clip(&settings, &ds, &text, "b");
        insert_row(&settings, &ds, &text, "b", false, &digest("hello world"), "2026-01-02T00:00:00Z");

        let clips = list(&settings, &ds, Some(&text)).unwrap();
        assert_eq!(clips.len(), 2);
        assert!(clips[0].is_default, "the default sorts first");
        assert_eq!(clips[0].uuid, "a");
        assert!(clips.iter().all(|c| c.usable && !c.stale));
        // The per-text listing decodes, so a real duration is reported.
        assert!((clips[0].duration.unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn set_default_moves_the_marker_and_leaves_exactly_one() {
        let (settings, _dir) = temp_settings("sa-default");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        for (uuid, day) in [("a", "01"), ("b", "02")] {
            write_clip(&settings, &ds, &text, uuid);
            insert_row(&settings, &ds, &text, uuid, uuid == "a", &digest("hello world"), &format!("2026-01-{day}T00:00:00Z"));
        }
        set_default(&settings, &ds, "b").unwrap();

        let clips = list(&settings, &ds, Some(&text)).unwrap();
        let defaults: Vec<&str> = clips.iter().filter(|c| c.is_default).map(|c| c.uuid.as_str()).collect();
        assert_eq!(defaults, vec!["b"], "exactly one default, and it moved");
    }

    #[test]
    fn deleting_the_default_promotes_the_earliest_remaining() {
        let (settings, _dir) = temp_settings("sa-promote");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        for (uuid, day, is_default) in [("a", "01", true), ("b", "02", false), ("c", "03", false)] {
            write_clip(&settings, &ds, &text, uuid);
            insert_row(&settings, &ds, &text, uuid, is_default, &digest("hello world"), &format!("2026-01-{day}T00:00:00Z"));
        }
        delete(&settings, &ds, "a").unwrap();

        let clips = list(&settings, &ds, Some(&text)).unwrap();
        assert_eq!(clips.len(), 2);
        let defaults: Vec<&str> = clips.iter().filter(|c| c.is_default).map(|c| c.uuid.as_str()).collect();
        assert_eq!(defaults, vec!["b"], "the earliest remaining becomes default");
        // The wav is gone with the row.
        let dir = find_dataset_dir(&settings, &ds).unwrap();
        assert!(!clip_path(&dir, &text, "a").exists());
    }

    #[test]
    fn two_defaults_resolve_to_the_earliest_from_the_db() {
        let (settings, _dir) = temp_settings("sa-two-defaults");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        // A sync replay can leave two defaults behind; the write path would not
        // create this, so insert both directly.
        for (uuid, day) in [("late", "02"), ("early", "01")] {
            write_clip(&settings, &ds, &text, uuid);
            insert_row(&settings, &ds, &text, uuid, true, &digest("hello world"), &format!("2026-01-{day}T00:00:00Z"));
        }
        let dir = find_dataset_dir(&settings, &ds).unwrap();
        match default_for(&dir, &text, "hello world").unwrap() {
            DefaultClip::Ready(clip) => assert_eq!(clip.uuid, "early"),
            _ => panic!("a default clip should resolve"),
        }
    }

    #[test]
    fn no_rows_means_missing_and_a_stale_clip_is_reported_stale() {
        let (settings, _dir) = temp_settings("sa-stale");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        let dir = find_dataset_dir(&settings, &ds).unwrap();

        assert!(matches!(
            default_for(&dir, &text, "hello world").unwrap(),
            DefaultClip::Missing
        ));

        write_clip(&settings, &ds, &text, "a");
        insert_row(&settings, &ds, &text, "a", true, &digest("an older text"), "2026-01-01T00:00:00Z");
        assert!(matches!(
            default_for(&dir, &text, "hello world").unwrap(),
            DefaultClip::Stale
        ));

        // Editing the text back into sync makes the same clip usable again.
        match default_for(&dir, &text, "an older text").unwrap() {
            DefaultClip::Ready(clip) => assert_eq!(clip.uuid, "a"),
            _ => panic!("a fresh clip should resolve"),
        }
    }

    #[test]
    fn a_row_whose_wav_is_gone_is_skipped_not_used() {
        let (settings, _dir) = temp_settings("sa-missing-wav");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        // Row only: the wav never landed (a row that beat its file over the wire).
        insert_row(&settings, &ds, &text, "a", true, &digest("hello world"), "2026-01-01T00:00:00Z");
        let dir = find_dataset_dir(&settings, &ds).unwrap();
        assert!(matches!(
            default_for(&dir, &text, "hello world").unwrap(),
            DefaultClip::Missing
        ));
        let clips = list(&settings, &ds, Some(&text)).unwrap();
        assert_eq!(clips.len(), 1);
        assert!(!clips[0].usable && clips[0].audio_url.is_none());
    }

    #[test]
    fn the_dataset_wide_listing_does_not_decode_and_badges_only_usable_clips() {
        let (settings, _dir) = temp_settings("sa-dataset-list");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        write_clip(&settings, &ds, &text, "a");
        insert_row(&settings, &ds, &text, "a", true, &digest("hello world"), "2026-01-01T00:00:00Z");
        let clips = list(&settings, &ds, None).unwrap();
        assert_eq!(clips.len(), 1);
        assert!(clips[0].usable);
        assert_eq!(clips[0].duration, None, "the whole-dataset call does not decode");
    }

    #[test]
    fn deleting_a_text_cascades_to_its_clips_and_files() {
        let (settings, _dir) = temp_settings("sa-cascade");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        write_clip(&settings, &ds, &text, "a");
        insert_row(&settings, &ds, &text, "a", true, &digest("hello world"), "2026-01-01T00:00:00Z");
        let dir = find_dataset_dir(&settings, &ds).unwrap();

        cascade_delete_for_text(&settings, &ds, &dir, &text).unwrap();
        assert!(list(&settings, &ds, Some(&text)).unwrap().is_empty());
        assert!(!clip_path(&dir, &text, "a").exists());
    }

    #[test]
    fn a_replayed_save_replaces_the_row_and_clears_a_competing_default() {
        let (settings, _dir) = temp_settings("sa-replay");
        let (ds, text) = dataset_with_text(&settings, "hello world");
        write_clip(&settings, &ds, &text, "a");
        insert_row(&settings, &ds, &text, "a", true, &digest("hello world"), "2026-01-01T00:00:00Z");
        write_clip(&settings, &ds, &text, "b");

        let payload = json!({
            "uuid": "b", "text_uuid": text, "note": "Edge TTS · x", "boundaries": null,
            "text_sha": digest("hello world"), "is_default": true,
            "created_at": "2026-01-02T00:00:00Z",
        });
        assert!(apply_remote_save(&settings, &ds, &payload).unwrap());
        let clips = list(&settings, &ds, Some(&text)).unwrap();
        let defaults: Vec<&str> = clips.iter().filter(|c| c.is_default).map(|c| c.uuid.as_str()).collect();
        assert_eq!(defaults, vec!["b"], "a replayed default clears the other");

        // Replaying a delete removes the row and the local wav.
        assert!(apply_remote_delete(&settings, &ds, &json!({ "uuid": "b" })).unwrap());
        assert!(list(&settings, &ds, Some(&text)).unwrap().iter().all(|c| c.uuid != "b"));
        let dir = find_dataset_dir(&settings, &ds).unwrap();
        assert!(!clip_path(&dir, &text, "b").exists());
    }

    // ---- the span guess's only pure part ----

    /// Alignment alone is not a span: words matched far outside the reading
    /// must not stretch it over the rest of the file.
    #[test]
    fn the_span_ignores_matches_far_from_the_reading() {
        let pairs = vec![(0, 2), (1, 3), (2, 400), (3, 401), (4, 402), (5, 403)];
        let run = densest_run(&pairs);
        assert_eq!(run.first(), Some(&(2, 400)));
        assert_eq!(run.last(), Some(&(5, 403)));
    }

    /// Read twice? The earlier run wins, so the same file always guesses the
    /// same span.
    #[test]
    fn the_span_keeps_the_first_reading_on_a_tie() {
        let pairs = vec![(0, 10), (1, 11), (2, 100), (3, 101)];
        assert_eq!(densest_run(&pairs).first(), Some(&(0, 10)));
    }

    /// A misheard word or two is normal; it must not split the reading in two
    /// and hand back the shorter half.
    #[test]
    fn a_few_missed_words_do_not_split_the_reading() {
        let pairs = vec![(0, 0), (1, 1), (2, 2), (7, 7), (8, 8), (9, 9)];
        assert_eq!(densest_run(&pairs).len(), 6);
    }

    #[test]
    fn no_matches_means_no_run() {
        assert!(densest_run(&[]).is_empty());
    }
}
