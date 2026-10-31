//! Card media: images, audio, and video attached to a card's question,
//! answer, or note field.
//!
//! ## Division of truth
//!
//! The **directory** `media/<card-uuid>/<original name>` under the dataset is
//! the truth about bytes; the **row** in `card_media` is the truth about
//! meaning — the anchor handle, the kind, the field, the order, the playback
//! behaviour. Neither is authoritative for both, and `card_media_register`
//! exists to reconcile them when they drift (see `docs/design/card-media.md`).
//!
//! ## Why files, not BLOBs
//!
//! The three sync planes already walk the dataset tree: `sync/blobpush.rs`
//! ships a content-addressed blob per changed file, `sync/rest.rs` `/snapshot`
//! tars the directory, and only the row-log journal moves SQLite content.
//! Storing media as BLOBs would defeat all three (a 40 MB video would rewrite
//! the entire DB on every card edit and get rejected by `is_db_file`). Files
//! also stay legible to the user in a plain file manager, which is the property
//! the per-card directory exists to serve.
//!
//! ## Anchors
//!
//! Position inside the text is a `{{handle}}` token, and the token is what the
//! editor and the reviewer both render against. Handles match
//! `^[a-z0-9][a-z0-9-]{0,31}$` and are unique among the card's live rows; see
//! [`super::strip_media_anchors`] for the hash/search-side rule.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;
use uuid::Uuid;

use crate::datasets::media_kind::{card_exts_for, card_kind_of, MediaKind};
use crate::datasets::now_row_stamp;
use crate::settings::SettingsState;

use super::{
    find_card_dataset_dir, find_card_dataset_dir_write, is_valid_handle, open_card_db, MEDIA_DIR,
};

/// The column list every `SELECT` of `card_media` spells, in the order
/// [`row_of`] reads it. One string instead of five copies keeps the two in
/// step: a column added here but not to `row_of` is a compile error, one added
/// to neither is a test failure.
const ROW_COLUMNS: &str = "uuid, card_uuid, field, handle, rel_path, kind, title, sort, \
                          autoplay, play_on, duration_ms, width, height, size, \
                          deleted_at, created_at, updated_at";

/// The stored row plus the two answers the caller always wants but the table
/// does not hold: `present` (does the file exist on this device?) and the
/// optional `path` (an absolute path for the asset protocol, filled in by
/// `list` and `add`). Both are computed per-command from the row + the
/// filesystem; neither is stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CardMedia {
    pub uuid: String,
    pub card_uuid: String,
    pub field: String,
    pub handle: String,
    pub rel_path: String,
    pub kind: String,
    pub title: String,
    pub sort: i64,
    pub autoplay: bool,
    pub play_on: String,
    pub duration_ms: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size: i64,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub present: bool,
    /// Absolute path on *this* device, set by `list`/`add` for the caller's
    /// convenience; deliberately absent from sync payloads (a path is not
    /// portable — the row's `rel_path` is).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Which field an anchor lives in, spelled as the column and the wire both
/// expect. Rejected values become an error, not a silent `"question"`.
fn normalize_field(field: &str) -> Result<&'static str, String> {
    match field.trim().to_ascii_lowercase().as_str() {
        "" | "question" => Ok("question"),
        "answer" => Ok("answer"),
        "note" => Ok("note"),
        "suggestion" => Ok("suggestion"),
        other => Err(format!(
            "Unknown card_media field '{other}'; expected one of question, answer, note, suggestion."
        )),
    }
}

fn normalize_play_on(play_on: &str) -> Result<&'static str, String> {
    match play_on.trim().to_ascii_lowercase().as_str() {
        "" | "tap" => Ok("tap"),
        "reveal" => Ok("reveal"),
        other => Err(format!(
            "Unknown card_media play_on '{other}'; expected tap or reveal."
        )),
    }
}

// ============================================================
// Path helpers
// ============================================================

/// The `<dataset>/media/<card-uuid>` directory a card's attachments live in.
/// The dataset dir is resolved by `find_*_dataset_dir{,_write}`; this helper
/// only appends the layout.
fn card_media_dir(dataset_dir: &Path, card_uuid: &str) -> PathBuf {
    dataset_dir.join(MEDIA_DIR).join(card_uuid)
}

/// Turn a picked file's absolute path into the `rel_path` we store: a
/// dataset-relative POSIX path (`media/<card>/<name>`). Windows separators are
/// normalised so a sync payload is portable.
fn to_rel_path(dataset_dir: &Path, abs: &Path) -> String {
    let rel = abs
        .strip_prefix(dataset_dir)
        .unwrap_or(abs)
        .to_string_lossy()
        .replace('\\', "/");
    rel
}

fn file_stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("media")
        .to_string()
}

/// Slugify a file stem to the handle grammar; when the stem has no ASCII
/// alphanumeric in it at all, fall back to `"m"`. Length-clamped to 32.
pub(crate) fn slugify(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(32));
    let mut last_dash = true; // suppress a leading `-`
    for ch in raw.chars() {
        if out.len() >= 32 {
            break;
        }
        let lower = ch.to_ascii_lowercase();
        let mapped = if lower.is_ascii_alphanumeric() {
            Some(lower.to_string())
        } else if matches!(ch, ' ' | '_' | '-' | '.' | '/' | '\\') {
            Some("-".to_string())
        } else {
            None
        };
        if let Some(s) = mapped {
            if s == "-" {
                if !last_dash {
                    out.push('-');
                    last_dash = true;
                }
            } else {
                out.push_str(&s);
                last_dash = false;
            }
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "m".to_string()
    } else if trimmed.len() > 32 {
        trimmed[..32].to_string()
    } else {
        trimmed
    }
}

/// A handle is unique among the card's live rows; on collision, `-2`, `-3`, …
/// suffixes are appended until one fits. Callers pass an already-slugified
/// stem; this fn does not re-slugify.
pub(crate) fn mint_handle(conn: &Connection, card_uuid: &str, stem: &str) -> Result<String, String> {
    let base = slugify(stem);
    let mut candidate = base.clone();
    let mut n = 2u32;
    loop {
        if !is_valid_handle(&candidate) {
            // slugify guarantees grammar; if it somehow does not (a stem that
            // slugified to only `-` was already fixed by the "m" fallback),
            // bail rather than loop forever.
            candidate = format!("m{}", &Uuid::new_v4().simple().to_string()[..6]);
        }
        let taken: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM card_media WHERE card_uuid = ?1 AND handle = ?2 AND deleted_at IS NULL",
                params![card_uuid, candidate],
                |_| Ok(1i64),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if taken.is_none() {
            return Ok(candidate);
        }
        let suffix = format!("-{}", n);
        // Reserve space so the base can be trimmed to fit within 32 total.
        let trimmed_base = if base.len() + suffix.len() > 32 {
            let cut = 32usize.saturating_sub(suffix.len());
            base[..cut].trim_end_matches('-').to_string()
        } else {
            base.clone()
        };
        candidate = format!("{}{}", trimmed_base, suffix);
        n += 1;
    }
}

/// The next free `-N` suffix on a copy of a source file into `dir` whose name
/// already exists. Kept in the same directory, so an existing "photo.png"
/// becomes "photo-2.png", then "photo-3.png", and so on. Preserves the stem
/// (a legibility promise, since browsing the folder is the point of the
/// per-card directory).
fn unique_target(dir: &Path, name: &str) -> PathBuf {
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{}", s))
        .unwrap_or_default();
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }
    let mut n = 2u32;
    loop {
        let cand = dir.join(format!("{}-{}{}", stem, n, ext));
        if !cand.exists() {
            return cand;
        }
        n += 1;
    }
}

// ============================================================
// Row read
// ============================================================

fn row_of(row: &rusqlite::Row<'_>) -> Result<CardMedia, rusqlite::Error> {
    Ok(CardMedia {
        uuid: row.get(0)?,
        card_uuid: row.get(1)?,
        field: row.get(2)?,
        handle: row.get(3)?,
        rel_path: row.get(4)?,
        kind: row.get(5)?,
        title: row.get(6)?,
        sort: row.get(7)?,
        autoplay: row.get::<_, i64>(8)? != 0,
        play_on: row.get(9)?,
        duration_ms: row.get(10)?,
        width: row.get(11)?,
        height: row.get(12)?,
        size: row.get(13)?,
        deleted_at: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
        // Filled per-caller after the query — a row read from the DB alone
        // cannot know whether the file is on this device.
        present: true,
        path: None,
    })
}

/// Attach `present`/`path` to each row against the dataset dir. Called by
/// every list-shaped command so the two fields never drift.
fn annotate_present(dataset_dir: &Path, rows: &mut [CardMedia]) {
    for r in rows.iter_mut() {
        annotate_one(dataset_dir, r);
    }
}

fn annotate_one(dataset_dir: &Path, r: &mut CardMedia) {
    let abs = dataset_dir.join(&r.rel_path);
    r.present = abs.is_file();
    r.path = Some(abs.to_string_lossy().into_owned());
}

// ============================================================
// Probing
// ============================================================

/// Best-effort probe of a file's size + (for audio) duration. Never fails:
/// a probe error degrades to a null duration, and the row still writes.
///
/// Width/height probing is desktop-only for now — the `image` crate is not
/// compiled on Android, and a browser-side `<img>` reports naturalWidth/naturalHeight
/// anyway, so a null pair is only a UI nicety lost.
fn probe(path: &Path, kind: MediaKind) -> (i64, Option<i64>, Option<i64>, Option<i64>) {
    let size = fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    let duration = if matches!(kind, MediaKind::Audio) {
        probe_audio_duration_ms(path)
    } else {
        None
    };
    let (w, h) = probe_image_dims(path, kind);
    (size, duration, w, h)
}

fn probe_audio_duration_ms(path: &Path) -> Option<i64> {
    // The 16 kHz mono f32 buffer symphonia returns decodes in step with the
    // samples-per-millisecond the frontend expects. A decode failure
    // (unsupported codec, a truncated file) yields `None` rather than an
    // error — the row still writes, and the reader can play it anyway.
    let samples = crate::audio::decode_to_pcm(path).ok()?;
    Some(samples.len() as i64 * 1000 / crate::audio::DECODED_SAMPLE_RATE as i64)
}

#[cfg(feature = "desktop")]
fn probe_image_dims(path: &Path, kind: MediaKind) -> (Option<i64>, Option<i64>) {
    if !matches!(kind, MediaKind::Image) {
        return (None, None);
    }
    // `image::image_dimensions` reads only the header — no full decode, no
    // memory spike on a large image.
    match image::image_dimensions(path) {
        Ok((w, h)) => (Some(w as i64), Some(h as i64)),
        Err(_) => (None, None),
    }
}

#[cfg(not(feature = "desktop"))]
fn probe_image_dims(_path: &Path, _kind: MediaKind) -> (Option<i64>, Option<i64>) {
    (None, None)
}

// ============================================================
// Journaling
// ============================================================

fn journal_save(settings: &SettingsState, dataset_uuid: &str, row: &CardMedia) {
    let payload = json!({
        "uuid": row.uuid,
        "card_uuid": row.card_uuid,
        "field": row.field,
        "handle": row.handle,
        "rel_path": row.rel_path,
        "kind": row.kind,
        "title": row.title,
        "sort": row.sort,
        "autoplay": row.autoplay,
        "play_on": row.play_on,
        "duration_ms": row.duration_ms,
        "width": row.width,
        "height": row.height,
        "size": row.size,
        "deleted_at": row.deleted_at,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    });
    let _ = crate::sync::change_log::commit_change(
        settings,
        "card_media_save",
        dataset_uuid,
        &payload,
    );
}

fn journal_delete(settings: &SettingsState, dataset_uuid: &str, media_uuid: &str, card_uuid: &str) {
    let _ = crate::sync::change_log::commit_change(
        settings,
        "card_media_delete",
        dataset_uuid,
        &json!({ "media_uuid": media_uuid, "card_uuid": card_uuid }),
    );
}

// ============================================================
// Commands
// ============================================================

/// Every live `card_media` row of one card, in `(field, sort, created_at)`
/// order, annotated with whether the file is present on this device. Never
/// lists a file the table does not know — that is `card_media_register`'s job.
#[tauri::command]
pub async fn card_media_list(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card_uuid: String,
) -> Result<Vec<CardMedia>, String> {
    let dir = find_card_dataset_dir(&settings, &dataset_uuid)?;
    let conn = open_card_db(&dir)?;
    list_inner(&conn, &dir, &card_uuid)
}

fn list_inner(
    conn: &Connection,
    dataset_dir: &Path,
    card_uuid: &str,
) -> Result<Vec<CardMedia>, String> {
    let sql = format!(
        "SELECT {} FROM card_media \
         WHERE card_uuid = ?1 AND deleted_at IS NULL \
         ORDER BY field ASC, sort ASC, created_at ASC",
        ROW_COLUMNS
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows: Vec<CardMedia> = stmt
        .query_map(params![card_uuid], row_of)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    annotate_present(dataset_dir, &mut rows);
    Ok(rows)
}

/// Copy `source_path` into `media/<card_uuid>/` and insert a row for it. The
/// copy happens here (not in the webview) because the `fs` capability only
/// grants `$APPDATA/**`, so a picked file outside the app dir is unreadable
/// from JS. Returns the row so the caller can insert `{{handle}}` at the caret.
#[tauri::command]
pub async fn card_media_add(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card_uuid: String,
    source_path: String,
    field: String,
) -> Result<CardMedia, String> {
    let field = normalize_field(&field)?.to_string();
    let dir = find_card_dataset_dir_write(&settings, &dataset_uuid)?;
    if card_uuid.trim().is_empty() {
        return Err(
            "card_media_add needs a card_uuid; open the editor with a minted uuid before attaching."
                .to_string(),
        );
    }
    let src = PathBuf::from(&source_path);
    if !src.is_file() {
        return Err(format!("Source file does not exist: {}", source_path));
    }
    let name_hint = src
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Source path has no file name".to_string())?
        .to_string();
    let kind = card_kind_of(&name_hint).ok_or_else(|| {
        format!(
            "Card media accepts only these extensions: image=[{}], audio=[{}], video=[{}]. Refused: {}",
            card_exts_for(MediaKind::Image).join(","),
            card_exts_for(MediaKind::Audio).join(","),
            card_exts_for(MediaKind::Video).join(","),
            name_hint,
        )
    })?;

    let dest_dir = card_media_dir(&dir, &card_uuid);
    fs::create_dir_all(&dest_dir).map_err(|e| format!("Cannot create {}: {}", dest_dir.display(), e))?;
    let dest = unique_target(&dest_dir, &name_hint);
    fs::copy(&src, &dest).map_err(|e| format!("Copy failed: {}", e))?;

    let (size, duration_ms, width, height) = probe(&dest, kind);
    let rel_path = to_rel_path(&dir, &dest);
    let stem = file_stem_of(&dest);
    let conn = open_card_db(&dir)?;
    let handle = mint_handle(&conn, &card_uuid, &stem)?;
    let sort: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort), -1) + 1 FROM card_media WHERE card_uuid = ?1 AND deleted_at IS NULL",
            params![card_uuid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let now = now_row_stamp();
    let uuid = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO card_media (uuid, card_uuid, field, handle, rel_path, kind, title, sort, \
         autoplay, play_on, duration_ms, width, height, size, deleted_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 'tap', ?9, ?10, ?11, ?12, NULL, ?13, ?13)",
        params![
            uuid,
            card_uuid,
            field,
            handle,
            rel_path,
            kind.as_str(),
            stem,
            sort,
            duration_ms,
            width,
            height,
            size,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;

    let saved = CardMedia {
        uuid,
        card_uuid,
        field,
        handle,
        rel_path,
        kind: kind.as_str().to_string(),
        title: stem,
        sort,
        autoplay: false,
        play_on: "tap".to_string(),
        duration_ms,
        width,
        height,
        size,
        deleted_at: None,
        created_at: now.clone(),
        updated_at: now,
        present: true,
        path: Some(dest.to_string_lossy().into_owned()),
    };
    journal_save(settings.inner(), &dataset_uuid, &saved);
    Ok(saved)
}

/// Update the mutable fields of one row. A handle change rewrites the owning
/// field's `{{old}}` → `{{new}}` in the same transaction so the anchor and the
/// token cannot disagree after a partial write.
#[tauri::command]
pub async fn card_media_update(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media: CardMedia,
) -> Result<CardMedia, String> {
    let dir = find_card_dataset_dir_write(&settings, &dataset_uuid)?;
    let conn = open_card_db(&dir)?;
    let old = conn
        .query_row(
            &format!("SELECT {} FROM card_media WHERE uuid = ?1", ROW_COLUMNS),
            params![media.uuid],
            row_of,
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("card_media row {} not found", media.uuid))?;

    let field = normalize_field(&media.field)?.to_string();
    let play_on = normalize_play_on(&media.play_on)?.to_string();
    if !is_valid_handle(&media.handle) {
        return Err(format!(
            "Handle '{}' does not match ^[a-z0-9][a-z0-9-]{{0,31}}$.",
            media.handle
        ));
    }
    if media.handle != old.handle {
        let clash: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM card_media WHERE card_uuid = ?1 AND handle = ?2 AND uuid != ?3 AND deleted_at IS NULL",
                params![old.card_uuid, media.handle, old.uuid],
                |_| Ok(1i64),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if clash.is_some() {
            return Err(format!(
                "Another attachment of this card already uses handle '{}'; pick a different one.",
                media.handle
            ));
        }
    }

    let now = now_row_stamp();
    conn.execute(
        "UPDATE card_media SET field = ?1, handle = ?2, title = ?3, sort = ?4, autoplay = ?5, \
         play_on = ?6, updated_at = ?7 WHERE uuid = ?8",
        params![
            field,
            media.handle,
            media.title,
            media.sort,
            media.autoplay as i64,
            play_on,
            now,
            media.uuid,
        ],
    )
    .map_err(|e| e.to_string())?;

    // Rewrite the token only when the handle moved; the owning column is the
    // one the row already had (fields do not move here — the caller would need
    // to physically re-anchor the token, which is a UI action, not this fn).
    if media.handle != old.handle {
        let old_field = old.field.clone();
        let column = match old_field.as_str() {
            "question" => "question",
            "answer" => "answer",
            "note" => "note",
            "suggestion" => "suggestion",
            // The row's field was already validated at write; a stored value
            // outside the four cannot be rewritten by this command and would
            // need a data migration — refuse rather than silently corrupt.
            other => {
                return Err(format!(
                    "card_media row has unknown field '{}' — cannot rewrite anchor.",
                    other
                ))
            }
        };
        let current: String = conn
            .query_row(
                &format!("SELECT {} FROM card WHERE uuid = ?1", column),
                params![old.card_uuid],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let rewritten = current.replace(&format!("{{{{{}}}}}", old.handle), &format!("{{{{{}}}}}", media.handle));
        conn.execute(
            &format!("UPDATE card SET {} = ?1, updated_at = ?2 WHERE uuid = ?3", column),
            params![rewritten, now, old.card_uuid],
        )
        .map_err(|e| e.to_string())?;
    }

    let mut saved = CardMedia {
        uuid: old.uuid,
        card_uuid: old.card_uuid,
        field,
        handle: media.handle,
        rel_path: old.rel_path,
        kind: old.kind,
        title: media.title,
        sort: media.sort,
        autoplay: media.autoplay,
        play_on,
        duration_ms: old.duration_ms,
        width: old.width,
        height: old.height,
        size: old.size,
        deleted_at: None,
        created_at: old.created_at,
        updated_at: now,
        present: true,
        path: None,
    };
    annotate_one(&dir, &mut saved);
    journal_save(settings.inner(), &dataset_uuid, &saved);
    Ok(saved)
}

/// Soft-delete the row; the bytes stay put. `trashed_to` is `null` in this
/// response — bytes are only moved to the app trash by an explicit sweep or
/// when the dataset is deleted. Undo of a card delete must still find its
/// media, which is exactly what keeping the bytes gives us.
#[tauri::command]
pub async fn card_media_delete(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    media_uuid: String,
) -> Result<serde_json::Value, String> {
    let dir = find_card_dataset_dir_write(&settings, &dataset_uuid)?;
    let conn = open_card_db(&dir)?;
    let row = conn
        .query_row(
            &format!("SELECT {} FROM card_media WHERE uuid = ?1", ROW_COLUMNS),
            params![media_uuid],
            row_of,
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(row) = row else {
        return Ok(json!({ "status": "already_gone", "trashed_to": null }));
    };
    let now = now_row_stamp();
    conn.execute(
        "UPDATE card_media SET deleted_at = ?1, updated_at = ?1 WHERE uuid = ?2",
        params![now, media_uuid],
    )
    .map_err(|e| e.to_string())?;
    journal_delete(settings.inner(), &dataset_uuid, &media_uuid, &row.card_uuid);
    Ok(json!({ "status": "ok", "trashed_to": null }))
}

/// Reconcile the directory with the table.
///
/// * A file under `media/<card_uuid>/` with no live row is imported as an
///   unanchored attachment (a new row, default field `question`, sort after
///   the current tail). The caller's Library drawer surfaces it so a
///   hand-placed or sync-arrived file is not invisible.
/// * A live row whose bytes are gone on this device is reported in `missing`
///   and never deleted — the sync planes can legitimately deliver a row before
///   its file, and deleting on a transient miss would lose user data.
///
/// Nothing is deleted. This mirrors
/// [`crate::datasets::read_aloud::source_align::register`]'s stance: switching
/// folders and syncing both have to be free and reversible.
#[tauri::command]
pub async fn card_media_register(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    card_uuid: String,
) -> Result<serde_json::Value, String> {
    let dir = find_card_dataset_dir_write(&settings, &dataset_uuid)?;
    let conn = open_card_db(&dir)?;
    register_inner(&settings, &dataset_uuid, &dir, &conn, &card_uuid)
}

fn register_inner(
    settings: &SettingsState,
    dataset_uuid: &str,
    dataset_dir: &Path,
    conn: &Connection,
    card_uuid: &str,
) -> Result<serde_json::Value, String> {
    let live = list_inner(conn, dataset_dir, card_uuid)?;
    let known: std::collections::HashSet<String> =
        live.iter().map(|r| r.rel_path.clone()).collect();

    let mut imported: Vec<CardMedia> = Vec::new();
    let mut orphaned: Vec<String> = Vec::new();
    let dir = card_media_dir(dataset_dir, card_uuid);
    if dir.is_dir() {
        let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let rel = to_rel_path(dataset_dir, &path);
            if known.contains(&rel) {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            // Only import a file a card is allowed to hold. Anything else stays
            // on disk and is reported as `orphaned` — the user's own scratch in
            // this dir, not ours to guess.
            let Some(kind) = card_kind_of(&name) else {
                orphaned.push(rel);
                continue;
            };
            let stem = file_stem_of(&path);
            let handle = mint_handle(conn, card_uuid, &stem)?;
            let sort: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(sort), -1) + 1 FROM card_media WHERE card_uuid = ?1 AND deleted_at IS NULL",
                    params![card_uuid],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let (size, duration_ms, width, height) = probe(&path, kind);
            let now = now_row_stamp();
            let uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO card_media (uuid, card_uuid, field, handle, rel_path, kind, title, sort, \
                 autoplay, play_on, duration_ms, width, height, size, deleted_at, created_at, updated_at) \
                 VALUES (?1, ?2, 'question', ?3, ?4, ?5, ?6, ?7, 0, 'tap', ?8, ?9, ?10, ?11, NULL, ?12, ?12)",
                params![
                    uuid,
                    card_uuid,
                    handle,
                    rel,
                    kind.as_str(),
                    stem,
                    sort,
                    duration_ms,
                    width,
                    height,
                    size,
                    now,
                ],
            )
            .map_err(|e| e.to_string())?;
            let mut saved = CardMedia {
                uuid,
                card_uuid: card_uuid.to_string(),
                field: "question".to_string(),
                handle,
                rel_path: rel,
                kind: kind.as_str().to_string(),
                title: stem,
                sort,
                autoplay: false,
                play_on: "tap".to_string(),
                duration_ms,
                width,
                height,
                size,
                deleted_at: None,
                created_at: now.clone(),
                updated_at: now,
                present: true,
                path: Some(path.to_string_lossy().into_owned()),
            };
            annotate_present(dataset_dir, std::slice::from_mut(&mut saved));
            journal_save(settings, dataset_uuid, &saved);
            imported.push(saved);
        }
    }

    let missing: Vec<String> = live.iter().filter(|r| !r.present).map(|r| r.rel_path.clone()).collect();
    Ok(json!({
        "imported": imported,
        "missing": missing,
        "orphaned": orphaned,
    }))
}

// Keep the module warning-free about the `card_exts_for` symbol when the
// `card_media_add` error path is not compiled in (feature combos): the import
// is used inside a `format!` there.
#[allow(dead_code)]
fn _use_exts() -> &'static [&'static str] {
    card_exts_for(MediaKind::Image)
}

// ---------------------------------------------------------------------------
// Fork support: copy one card's attachments to another
// ---------------------------------------------------------------------------

/// Copy every live attachment of `src_card_uuid` (in `src_dataset_uuid`) into
/// `dst_card_uuid` (in `dst_dataset_uuid`), preserving the `handle` so the
/// copied text's `{{handle}}` tokens resolve unchanged. Called from
/// [`super::card_fork`] after the destination card's row is written; a failure
/// here does not roll back the fork (the card is still useful without media),
/// but a partial copy is reported so the caller can retry. Best-effort per
/// file: an individual failure is collected in `skipped` and the walk
/// continues, matching the register-side rule that nothing is ever deleted
/// silently.
pub(crate) fn copy_attachments_for_fork(
    settings: &SettingsState,
    src_dataset_uuid: &str,
    src_card_uuid: &str,
    dst_dataset_uuid: &str,
    dst_card_uuid: &str,
) -> Result<(Vec<CardMedia>, Vec<String>), String> {
    let src_dir = find_card_dataset_dir(settings, src_dataset_uuid)?;
    let dst_dir = find_card_dataset_dir_write(settings, dst_dataset_uuid)?;
    let src_conn = open_card_db(&src_dir)?;
    let mut src_rows = list_inner(&src_conn, &src_dir, src_card_uuid)?;
    let dst_conn = open_card_db(&dst_dir)?;

    let mut copied = Vec::with_capacity(src_rows.len());
    let mut skipped = Vec::new();
    let dst_media_dir = card_media_dir(&dst_dir, dst_card_uuid);

    for row in src_rows.drain(..) {
        let abs_src = src_dir.join(&row.rel_path);
        if !abs_src.is_file() {
            // The source's bytes are already missing on this device; do not
            // propagate the drift into a fresh destination row.
            skipped.push(row.rel_path.clone());
            continue;
        }
        if !dst_media_dir.exists() {
            fs::create_dir_all(&dst_media_dir)
                .map_err(|e| format!("Cannot create {}: {}", dst_media_dir.display(), e))?;
        }
        // Preserve the original file name (the legibility promise). A collision
        // inside the destination dir means the target card already has a file
        // with that name — impossible for a fresh fork, defensive for a re-run.
        let file_name = Path::new(&row.rel_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("media")
            .to_string();
        let dst = unique_target(&dst_media_dir, &file_name);
        if let Err(e) = fs::copy(&abs_src, &dst) {
            skipped.push(format!("{} ({})", row.rel_path, e));
            continue;
        }
        let rel = to_rel_path(&dst_dir, &dst);
        let now = now_row_stamp();
        let new_uuid = Uuid::new_v4().to_string();
        let rc = dst_conn.execute(
            "INSERT INTO card_media (uuid, card_uuid, field, handle, rel_path, kind, title, sort, \
             autoplay, play_on, duration_ms, width, height, size, deleted_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL, ?15, ?15)",
            params![
                new_uuid,
                dst_card_uuid,
                row.field,
                row.handle,
                rel,
                row.kind,
                row.title,
                row.sort,
                row.autoplay as i64,
                row.play_on,
                row.duration_ms,
                row.width,
                row.height,
                row.size,
                now,
            ],
        );
        if let Err(e) = rc {
            skipped.push(format!("{} (insert: {})", row.rel_path, e));
            continue;
        }
        let mut saved = CardMedia {
            uuid: new_uuid,
            card_uuid: dst_card_uuid.to_string(),
            field: row.field,
            handle: row.handle,
            rel_path: rel,
            kind: row.kind,
            title: row.title,
            sort: row.sort,
            autoplay: row.autoplay,
            play_on: row.play_on,
            duration_ms: row.duration_ms,
            width: row.width,
            height: row.height,
            size: row.size,
            deleted_at: None,
            created_at: now.clone(),
            updated_at: now,
            present: true,
            path: None,
        };
        annotate_one(&dst_dir, &mut saved);
        journal_save(settings, dst_dataset_uuid, &saved);
        copied.push(saved);
    }

    Ok((copied, skipped))
}

// ============================================================
// Replay (a journaled change arriving from another device)
// ============================================================

/// Replay one journaled `card_media_save`. The bytes themselves ride the file
/// manifest, so a row may legitimately arrive before its file: `list_inner`
/// reads `present` from the filesystem, and a false renders the placeholder
/// the plan §5 promises; a subsequent `card_media_register` reclaims the file
/// once it lands. Upsert on the row's uuid so a re-run and the local save's
/// own journal entry both settle on the same row (INV-8 style).
pub(crate) fn apply_remote_save(
    settings: &SettingsState,
    dataset_uuid: &str,
    payload: &serde_json::Value,
) -> Result<bool, String> {
    let row = card_media_from_payload(payload)?;
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
        return Err("card_media_save change carries no dataset_uuid".to_string());
    }
    let dir = find_card_dataset_dir_write(settings, &dataset_uuid)?;
    let conn = open_card_db(&dir)?;
    conn.execute(
        "INSERT INTO card_media (uuid, card_uuid, field, handle, rel_path, kind, title, sort, \
         autoplay, play_on, duration_ms, width, height, size, deleted_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17) \
         ON CONFLICT(uuid) DO UPDATE SET \
            card_uuid = ?2, field = ?3, handle = ?4, rel_path = ?5, kind = ?6, title = ?7, \
            sort = ?8, autoplay = ?9, play_on = ?10, duration_ms = ?11, width = ?12, height = ?13, \
            size = ?14, deleted_at = ?15, updated_at = ?17",
        params![
            row.uuid,
            row.card_uuid,
            row.field,
            row.handle,
            row.rel_path,
            row.kind,
            row.title,
            row.sort,
            row.autoplay as i64,
            row.play_on,
            row.duration_ms,
            row.width,
            row.height,
            row.size,
            row.deleted_at,
            row.created_at,
            row.updated_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    let _ = crate::datasets::touch_info(&dir);
    // Re-journal so other followers converge; `commit_change` self-suppresses
    // while a remote change is being applied, so this is a no-op locally.
    journal_save(settings, &dataset_uuid, &row);
    Ok(true)
}

/// Replay one journaled `card_media_delete`: soft-delete the row and leave
/// bytes alone. Undo of a card delete must still find its media on every
/// device, so a hard unlink here would be a data-loss bug the moment a user
/// restores a trashed card.
pub(crate) fn apply_remote_delete(
    settings: &SettingsState,
    dataset_uuid: &str,
    payload: &serde_json::Value,
) -> Result<bool, String> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let media_uuid = get("media_uuid");
    if media_uuid.is_empty() {
        return Err("card_media_delete change carries no media_uuid".to_string());
    }
    let dataset_uuid = if dataset_uuid.is_empty() { get("dataset_uuid") } else { dataset_uuid.to_string() };
    if dataset_uuid.is_empty() {
        return Err("card_media_delete change carries no dataset_uuid".to_string());
    }
    let dir = find_card_dataset_dir_write(settings, &dataset_uuid)?;
    let conn = open_card_db(&dir)?;
    let now = now_row_stamp();
    let deleted = conn
        .execute(
            "UPDATE card_media SET deleted_at = ?1, updated_at = ?1 WHERE uuid = ?2 AND deleted_at IS NULL",
            params![now, media_uuid],
        )
        .map_err(|e| e.to_string())?;
    let _ = crate::datasets::touch_info(&dir);
    // Re-journal for followers; a no-op locally.
    let _ = crate::sync::change_log::commit_change(
        settings,
        "card_media_delete",
        &dataset_uuid,
        &json!({ "media_uuid": media_uuid, "card_uuid": get("card_uuid") }),
    );
    Ok(deleted > 0)
}

/// Deserialize a journaled payload back into a [`CardMedia`]. Kept strict
/// about the identity fields (uuid, card_uuid, rel_path, kind, handle) and
/// forgiving about the optional ones (`null` numbers pass through). A payload
/// missing any required column is an actionable error naming the field, per
/// the agent-friendly rules.
fn card_media_from_payload(payload: &serde_json::Value) -> Result<CardMedia, String> {
    let s = |k: &str| -> Result<String, String> {
        payload
            .get(k)
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .ok_or_else(|| format!("card_media_save payload missing string field '{k}'"))
    };
    let opt_i64 = |k: &str| payload.get(k).and_then(|v| v.as_i64());
    let req_i64 = |k: &str, default: i64| payload.get(k).and_then(|v| v.as_i64()).unwrap_or(default);
    let opt_str = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(|v| v.to_string());
    Ok(CardMedia {
        uuid: s("uuid")?,
        card_uuid: s("card_uuid")?,
        field: s("field")?,
        handle: s("handle")?,
        rel_path: s("rel_path")?,
        kind: s("kind")?,
        title: opt_str("title").unwrap_or_default(),
        sort: req_i64("sort", 0),
        autoplay: payload.get("autoplay").and_then(|v| v.as_bool()).unwrap_or(false),
        play_on: opt_str("play_on").unwrap_or_else(|| "tap".to_string()),
        duration_ms: opt_i64("duration_ms"),
        width: opt_i64("width"),
        height: opt_i64("height"),
        size: req_i64("size", 0),
        deleted_at: opt_str("deleted_at"),
        created_at: opt_str("created_at").unwrap_or_default(),
        updated_at: opt_str("updated_at").unwrap_or_default(),
        present: false,
        path: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fms-card-media-{}-{}", tag, Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn slugify_maps_to_handle_grammar() {
        assert_eq!(slugify("My Photo"), "my-photo");
        assert_eq!(slugify("  leading and trailing  "), "leading-and-trailing");
        assert_eq!(slugify("snake_case-name"), "snake-case-name");
        assert_eq!(slugify("a.b.c"), "a-b-c");
        // No ASCII alphanumeric at all -> the "m" fallback, never empty.
        assert_eq!(slugify("!!!"), "m");
        // Uppercase folds down.
        assert_eq!(slugify("PNG"), "png");
        // Every result satisfies the handle grammar the DB and the anchor rely on.
        for stem in ["My Photo", "!!!", "a.b.c", "---"] {
            let s = slugify(stem);
            assert!(is_valid_handle(&s), "slugify({stem:?}) = {s:?} is not a valid handle");
        }
    }

    #[test]
    fn slugify_clamps_to_thirty_two() {
        let long = "a".repeat(50);
        let s = slugify(&long);
        assert_eq!(s.len(), 32, "slugify must clamp to the 32-char handle limit");
        assert!(is_valid_handle(&s));
    }

    #[test]
    fn normalize_field_and_play_on() {
        assert_eq!(normalize_field(""), Ok("question"));
        assert_eq!(normalize_field(" Answer "), Ok("answer"));
        assert_eq!(normalize_field("suggestion"), Ok("suggestion"));
        assert!(normalize_field("bogus").is_err());
        assert_eq!(normalize_play_on(""), Ok("tap"));
        assert_eq!(normalize_play_on("REVEAL"), Ok("reveal"));
        assert!(normalize_play_on("hover").is_err());
    }

    #[test]
    fn rel_path_is_forward_slashed_and_dataset_relative() {
        let ds = temp_dir("rel");
        let abs = ds.join(MEDIA_DIR).join("card-1").join("photo.png");
        let rel = to_rel_path(&ds, &abs);
        assert!(rel.starts_with("media/"), "got {rel}");
        assert!(!rel.contains('\\'), "separators must be normalised: {rel}");
        assert!(rel.ends_with("photo.png"));
        fs::remove_dir_all(&ds).ok();
    }

    #[test]
    fn file_stem_of_falls_back() {
        assert_eq!(file_stem_of(Path::new("a/b/photo.png")), "photo");
        assert_eq!(file_stem_of(Path::new("noextension")), "noextension");
    }

    #[test]
    fn unique_target_adds_dash_suffix_on_collision() {
        let dir = temp_dir("unique");
        fs::write(dir.join("photo.png"), b"x").unwrap();
        let first = unique_target(&dir, "photo.png");
        assert_eq!(first.file_name().unwrap().to_str().unwrap(), "photo-2.png");
        fs::write(dir.join("photo-2.png"), b"x").unwrap();
        let second = unique_target(&dir, "photo.png");
        assert_eq!(second.file_name().unwrap().to_str().unwrap(), "photo-3.png");
        fs::remove_dir_all(&dir).ok();
    }

    fn conn_with_table() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE card_media (
                uuid TEXT PRIMARY KEY,
                card_uuid TEXT NOT NULL,
                handle TEXT NOT NULL,
                deleted_at TEXT
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn mint_handle_is_unique_among_live_rows() {
        let conn = conn_with_table();
        // First mint of a stem returns the plain slug.
        assert_eq!(mint_handle(&conn, "card-1", "photo").unwrap(), "photo");
        conn.execute(
            "INSERT INTO card_media (uuid, card_uuid, handle, deleted_at) VALUES ('u1', 'card-1', 'photo', NULL)",
            [],
        )
        .unwrap();
        // A live row squats on `photo`, so the next mint bumps to `photo-2`.
        assert_eq!(mint_handle(&conn, "card-1", "photo").unwrap(), "photo-2");
        // A different card is unaffected — uniqueness is per card.
        assert_eq!(mint_handle(&conn, "card-2", "photo").unwrap(), "photo");
    }

    #[test]
    fn mint_handle_ignores_tombstoned_rows() {
        let conn = conn_with_table();
        // A soft-deleted (tombstoned) row does not reserve its handle.
        conn.execute(
            "INSERT INTO card_media (uuid, card_uuid, handle, deleted_at) VALUES ('u1', 'card-1', 'photo', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        assert_eq!(mint_handle(&conn, "card-1", "photo").unwrap(), "photo");
    }
}