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
use crate::edge_tts::{self, TtsWordBoundary};
use crate::settings::SettingsState;

use super::{effective_reference, find_dataset_dir, find_dataset_dir_write, open_db};

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

    /// Word timings, or empty for an upload.
    fn boundary_list(&self) -> Vec<TtsWordBoundary> {
        self.boundaries
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default()
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
    /// Seconds, decoded from the wav. `0.0` when the wav is missing, and for the
    /// dataset-wide listing, which does not decode.
    pub duration: f64,
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
                .map(|s| s.len() as f64 / audio::DECODED_SAMPLE_RATE as f64)
                .unwrap_or(0.0)
        } else {
            0.0
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
// Writes
// ============================================================

/// Say the text with a voice and store the result. The note defaults to the
/// voice, so a text's clip list reads like a list of recordings, not of uuids.
pub(crate) async fn generate(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
    voice: &str,
    note: Option<&str>,
) -> Result<ReadStandardAudio, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let reference = {
        let conn = open_db(&dir)?;
        conn.query_row(
            "SELECT content, normalized_content FROM read_text WHERE uuid = ?1",
            [text_uuid],
            |row| {
                let content: String = row.get(0)?;
                Ok(effective_reference(&content, row.get::<_, Option<String>>(1)?.as_deref()))
            },
        )
        .map_err(|_| format!("Text {} not found in dataset", text_uuid))?
    };
    if reference.trim().is_empty() {
        return Err("That text is empty: there is nothing to say".to_string());
    }

    let voice = if voice.trim().is_empty() { super::DEFAULT_TTS_VOICE } else { voice.trim() };
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
        duration: samples.len() as f64 / audio::DECODED_SAMPLE_RATE as f64,
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
    let reference = {
        let conn = open_db(&dir)?;
        conn.query_row(
            "SELECT content, normalized_content FROM read_text WHERE uuid = ?1",
            [text_uuid],
            |row| {
                let content: String = row.get(0)?;
                Ok(effective_reference(&content, row.get::<_, Option<String>>(1)?.as_deref()))
            },
        )
        .map_err(|_| format!("Text {} not found in dataset", text_uuid))?
    };

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

    let audio_uuid = Uuid::new_v4().to_string();
    let path = clip_path(&dir, text_uuid, &audio_uuid);
    audio::write_wav(&path, clip, audio::DECODED_SAMPLE_RATE, 1)?;

    let conn = open_db(&dir)?;
    ensure_schema(&conn)?;
    let row = StandardAudioRow {
        uuid: audio_uuid.clone(),
        text_uuid: text_uuid.to_string(),
        note: note
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                file_name
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(|n| format!("Upload · {}", n))
            })
            .unwrap_or_else(|| "Upload".to_string()),
        // An upload has no word timings, which is how `kind` stays derivable.
        boundaries: None,
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
        kind: "upload".to_string(),
        duration: clip.len() as f64 / rate,
        audio_url: Some(path.to_string_lossy().into_owned()),
        is_default: row.is_default,
        stale: false,
        usable: true,
        created_at: row.created_at,
    })
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

/// Synthesise this text with Edge TTS and store the result as a standard clip.
/// `voice` defaults to the one configured on the TTS page.
#[tauri::command]
pub async fn read_aloud_standard_audio_generate(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
    voice: Option<String>,
    note: Option<String>,
) -> Result<ReadStandardAudio, String> {
    let voice = voice
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| super::tts_voice(&settings));
    generate(&settings, &dataset_uuid, &text_uuid, &voice, note.as_deref()).await
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
        assert!((clips[0].duration - 0.5).abs() < 1e-6);
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
        assert_eq!(clips[0].duration, 0.0, "the whole-dataset call does not decode");
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
}
