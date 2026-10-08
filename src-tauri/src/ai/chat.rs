//! Cross-device chat: one shared message thread persisted on the PC.
//!
//! The PC is the hub (matching the thin-client sync model). All chat state
//! lives in one directory, `<workspace>/chat/`:
//!
//! * `messages.sqlite3` — the thread (`chat_messages` / `chat_attachments`). It
//!   is its own SQLite file rather than tables in the shared app DB, so a
//!   thread and its attachments can be backed up or removed as one unit.
//! * `attachments/` — attachment bytes, named `<attachment uuid>--<name>`.
//! * `cache/` — on the phone only: attachments downloaded for viewing.
//!
//! How the two platforms reach that store:
//!
//! * Desktop commands read/write that store directly and emit a
//!   `chat-message` Tauri event so open desktop UIs refresh without polling.
//! * The Android thin client registers the *same command names*, but its
//!   implementations relay to the PC's `/api/v1/chat/*` REST endpoints
//!   (`rest.rs`) with the device signature — never WebView `fetch()`
//!   (CORS-blocked; see the `sync.rs` module docs).
//! * Online-only: a send while the PC is unreachable fails and the UI offers
//!   a retry. Retries reuse the client-generated message uuid and
//!   [`insert_message`] is idempotent on it, so a message is stored exactly
//!   once even when the first attempt's response was lost.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::settings::SettingsState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct ChatAttachment {
    pub uuid: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Hub-side monotonic rowid cursor (§4.3). Zero on a client's pending
    /// outbox echo until the hub assigns it; `serde(default)` keeps older
    /// serialized messages readable.
    #[serde(default)]
    pub id: i64,
    pub uuid: String,
    /// `"pc"` or the paired device's id. Clients use it only to decide which
    /// side of the thread a bubble is drawn on (`self_device` in the list
    /// response tells each device which id is "mine").
    pub sender_device: String,
    pub sender_name: String,
    pub text: String,
    /// ISO-8601 UTC with millisecond precision; doubles as the poll cursor.
    pub created_at: String,
    #[serde(default)]
    pub attachments: Vec<ChatAttachment>,
    /// Phone only (§4): a message sitting in the local outbox that the hub has
    /// not acknowledged yet (`id` is still 0). Rendered at the thread end with a
    /// pending marker; cleared once the flush succeeds and the real hub row is
    /// mirrored back. `serde(default)` keeps every hub/PC message non-pending.
    #[serde(default)]
    pub pending: bool,
}

/// Shape shared by the desktop and mobile `chat_list_messages` commands:
/// `{ messages, self_device }` where `self_device` is `"pc"` on the desktop
/// and the caller's paired device id on the phone.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
#[derive(Serialize)]
pub struct ChatListResponse {
    pub messages: Vec<ChatMessage>,
    pub self_device: String,
}

/// Payload of the `chat-save-progress` event, emitted while an attachment is
/// copied (desktop) or downloaded (phone) so the UI can show a percentage.
/// `total` is 0 when the size is unknown, in which case only `done` matters.
#[derive(Clone, Serialize)]
struct SaveProgress {
    uuid: String,
    received: u64,
    total: u64,
    done: bool,
}

/// Report save/copy progress to the UI. Callers throttle this to one event per
/// whole percent — enough resolution for a percentage readout without flooding
/// the event channel.
fn emit_progress(app: &tauri::AppHandle, uuid: &str, received: u64, total: u64, done: bool) {
    use tauri::Emitter;
    let _ = app.emit(
        "chat-save-progress",
        SaveProgress { uuid: uuid.to_string(), received, total, done },
    );
}

// ---------------------------------------------------------------------------
// Schema + storage helpers
// ---------------------------------------------------------------------------
// The store itself only ever lives on the PC, so on the thin-client build these
// are unreferenced (the phone talks REST through `chat_*` commands below);
// `dead_code` is silenced there rather than duplicated behind cfg gates.

#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn ensure_chat_tables(conn: &Connection) -> Result<(), String> {
    // One-time rebuild promoting `chat_messages` to an explicit rowid-cursor
    // table (§4.3): `id INTEGER PRIMARY KEY AUTOINCREMENT` gives a monotonic,
    // gap-safe poll cursor that a `created_at` timestamp (which the hub stamps,
    // so two sends in the same millisecond tie) cannot. Guarded by
    // `PRAGMA user_version` so it runs exactly once and never re-shuffles ids.
    let ver: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if ver < 1 {
        // Ensure the legacy table exists first (a brand-new DB has none, so the
        // copy below is then a no-op and yields the fresh schema).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chat_messages(
                uuid          TEXT PRIMARY KEY,
                sender_device TEXT NOT NULL,
                sender_name   TEXT NOT NULL,
                text          TEXT NOT NULL DEFAULT '',
                created_at    TEXT NOT NULL
            );",
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE chat_messages_new(
                 id            INTEGER PRIMARY KEY AUTOINCREMENT,
                 uuid          TEXT UNIQUE NOT NULL,
                 sender_device TEXT NOT NULL,
                 sender_name   TEXT NOT NULL,
                 text          TEXT NOT NULL DEFAULT '',
                 created_at    TEXT NOT NULL
             );
             INSERT INTO chat_messages_new(uuid, sender_device, sender_name, text, created_at)
                 SELECT uuid, sender_device, sender_name, text, created_at
                   FROM chat_messages ORDER BY created_at;
             DROP TABLE chat_messages;
             ALTER TABLE chat_messages_new RENAME TO chat_messages;
             COMMIT;",
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_attachments(
            uuid         TEXT PRIMARY KEY,
            message_uuid TEXT NOT NULL,
            filename     TEXT NOT NULL,
            mime         TEXT NOT NULL DEFAULT '',
            size         INTEGER NOT NULL DEFAULT 0,
            stored_name  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_chat_messages_created ON chat_messages(created_at);
        CREATE INDEX IF NOT EXISTS idx_chat_attachments_msg ON chat_attachments(message_uuid);
        PRAGMA user_version = 1;",
    )
    .map_err(|e| e.to_string())
}

/// Chat root: `<workspace>/chat`, holding the message DB, the attachment store
/// and the phone's download cache. Workspace-scoped, like the app DB.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn chat_root(settings: &SettingsState) -> Result<PathBuf, String> {
    let dir = settings.workspace_subdir("chat", "");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Where attachment bytes live: `<workspace>/chat/attachments`.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn attachments_dir(settings: &SettingsState) -> Result<PathBuf, String> {
    let dir = chat_root(settings)?.join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Open (and if necessary create) the chat database.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn open(settings: &SettingsState) -> Result<Connection, String> {
    let path = chat_root(settings)?.join("messages.sqlite3");
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    ensure_chat_tables(&conn)?;
    Ok(conn)
}

/// Client-supplied ids are stored verbatim as primary keys, so constrain them
/// to a safe shape (length + charset) before they reach the DB or a filename.
pub(crate) fn valid_message_uuid(s: &str) -> bool {
    (8..=64).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Strip path components and shell-hostile characters from a client filename.
pub(crate) fn sanitize_filename(raw: &str) -> String {
    let base = raw
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(raw)
        .trim_matches('.');
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .take(100)
        .collect();
    if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned
    }
}

/// Tiny extension table — avoids pulling `mime_guess` into the mobile build.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
fn guess_mime(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" | "md" | "vtt" | "srt" => "text/plain",
        "json" => "application/json",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
fn row_to_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: r.get(0)?,
        uuid: r.get(1)?,
        sender_device: r.get(2)?,
        sender_name: r.get(3)?,
        text: r.get(4)?,
        created_at: r.get(5)?,
        attachments: Vec::new(),
        pending: false,
    })
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
fn attachments_for(conn: &Connection, message_uuid: &str) -> Result<Vec<ChatAttachment>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT uuid, filename, mime, size FROM chat_attachments \
             WHERE message_uuid = ?1 ORDER BY rowid",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![message_uuid], |r| {
            Ok(ChatAttachment {
                uuid: r.get(0)?,
                filename: r.get(1)?,
                mime: r.get(2)?,
                size: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn load_message(conn: &Connection, uuid: &str) -> Result<Option<ChatMessage>, String> {
    let mut msg: Option<ChatMessage> = conn
        .query_row(
            "SELECT id, uuid, sender_device, sender_name, text, created_at \
             FROM chat_messages WHERE uuid = ?1",
            params![uuid],
            row_to_message,
        )
        .ok();
    if let Some(m) = &mut msg {
        m.attachments = attachments_for(conn, &m.uuid)?;
    }
    Ok(msg)
}

/// List messages ascending. `after` is a `created_at` cursor (empty = latest
/// page). Used by the desktop command, the mobile poll and `rest.rs`.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn list_messages(
    settings: &SettingsState,
    after: &str,
    limit: i64,
) -> Result<Vec<ChatMessage>, String> {
    let conn = open(settings)?;
    let limit = limit.clamp(1, 500);
    let mut stmt = conn
        .prepare(
            "SELECT id, uuid, sender_device, sender_name, text, created_at FROM chat_messages \
             WHERE ?1 = '' OR created_at > ?1 ORDER BY created_at DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![after, limit], row_to_message)
        .map_err(|e| e.to_string())?;
    let mut msgs: Vec<ChatMessage> = rows.filter_map(|r| r.ok()).collect();
    drop(stmt);
    msgs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    for m in &mut msgs {
        m.attachments = attachments_for(&conn, &m.uuid)?;
    }
    Ok(msgs)
}

/// List messages newer than an integer rowid cursor (§4.3). Preferred over the
/// legacy `created_at` poll for incremental sync: the AUTOINCREMENT `id` never
/// ties, so two sends in the same millisecond are both delivered. `after_id < 0`
/// returns the latest page (same convention as the timestamp cursor).
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn list_messages_after_id(
    settings: &SettingsState,
    after_id: i64,
    limit: i64,
) -> Result<Vec<ChatMessage>, String> {
    let conn = open(settings)?;
    let limit = limit.clamp(1, 500);
    let mut stmt = conn
        .prepare(
            "SELECT id, uuid, sender_device, sender_name, text, created_at FROM chat_messages \
             WHERE id > ?1 ORDER BY id DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![after_id, limit], row_to_message)
        .map_err(|e| e.to_string())?;
    let mut msgs: Vec<ChatMessage> = rows.filter_map(|r| r.ok()).collect();
    drop(stmt);
    msgs.sort_by_key(|m| m.id);
    for m in &mut msgs {
        m.attachments = attachments_for(&conn, &m.uuid)?;
    }
    Ok(msgs)
}

/// Store one message with its attachment files. `files` are
/// `(source path, original name)`; sources are copied into
/// `<workspace>/chat/attachments` under `<attachment uuid>--<sanitized name>`.
/// Idempotent on `uuid`: an existing message is returned unchanged (this is what
/// makes UI retries safe).
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn insert_message(
    settings: &SettingsState,
    uuid: &str,
    sender_device: &str,
    sender_name: &str,
    text: &str,
    files: &[(PathBuf, String)],
) -> Result<ChatMessage, String> {
    let conn = open(settings)?;
    if let Some(existing) = load_message(&conn, uuid)? {
        return Ok(existing);
    }
    let dir = attachments_dir(settings)?;
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    // Copy attachments first; roll them back if the DB write fails.
    let mut stored: Vec<ChatAttachment> = Vec::new();
    let mut copied_names: Vec<String> = Vec::new();
    let rollback = |names: &[String]| {
        for n in names {
            let _ = std::fs::remove_file(dir.join(n));
        }
    };
    for (src, orig) in files {
        let att_uuid = Uuid::new_v4().to_string();
        let filename = sanitize_filename(orig);
        let mime = guess_mime(&filename).to_string();
        let stored_name = format!("{att_uuid}--{filename}");
        match std::fs::copy(src, dir.join(&stored_name)) {
            Ok(_) => {
                let size = std::fs::metadata(dir.join(&stored_name))
                    .map(|m| m.len() as i64)
                    .unwrap_or(0);
                copied_names.push(stored_name);
                stored.push(ChatAttachment {
                    uuid: att_uuid,
                    filename,
                    mime,
                    size,
                });
            }
            Err(e) => {
                rollback(&copied_names);
                return Err(format!("cannot attach {filename}: {e}"));
            }
        }
    }

    let db_result = (|| -> Result<i64, String> {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO chat_messages(uuid, sender_device, sender_name, text, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![uuid, sender_device, sender_name, text, created_at],
        )
        .map_err(|e| e.to_string())?;
        // Capture the hub-side rowid before the attachment inserts advance it;
        // this is the monotonic cursor followers poll with (§4.3).
        let row_id = tx.last_insert_rowid();
        for (att, stored_name) in stored.iter().zip(copied_names.iter()) {
            tx.execute(
                "INSERT INTO chat_attachments(uuid, message_uuid, filename, mime, size, stored_name) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![att.uuid, uuid, att.filename, att.mime, att.size, stored_name],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(row_id)
    })();
    let row_id = match db_result {
        Ok(id) => id,
        Err(e) => {
            rollback(&copied_names);
            return Err(e);
        }
    };

    Ok(ChatMessage {
        id: row_id,
        uuid: uuid.to_string(),
        sender_device: sender_device.to_string(),
        sender_name: sender_name.to_string(),
        text: text.to_string(),
        created_at,
        attachments: stored,
        pending: false,
    })
}

/// Resolve an attachment uuid to its stored file (DB-row-only lookup, so a
/// client can never steer this at an arbitrary PC path).
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn attachment_file(
    settings: &SettingsState,
    uuid: &str,
) -> Result<(PathBuf, ChatAttachment), String> {
    let conn = open(settings)?;
    let row: (String, String, String, i64, String) = conn
        .query_row(
            "SELECT uuid, filename, mime, size, stored_name FROM chat_attachments WHERE uuid = ?1",
            params![uuid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|_| format!("attachment not found: {uuid}"))?;
    let path = attachments_dir(settings)?.join(&row.4);
    if !path.exists() {
        return Err(format!("attachment file missing: {}", row.4));
    }
    Ok((
        path,
        ChatAttachment { uuid: row.0, filename: row.1, mime: row.2, size: row.3 },
    ))
}

// ---------------------------------------------------------------------------
// Desktop commands — direct store access
// ---------------------------------------------------------------------------

#[cfg(feature = "desktop")]
pub(crate) fn pc_sender_name() -> String {
    if let Ok(v) = std::env::var("COMPUTERNAME") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    #[cfg(unix)]
    {
        if let Ok(out) = std::process::Command::new("hostname").output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "PC".to_string()
}

/// Shared by the desktop command and the MCP tool: store + event broadcast.
#[cfg(feature = "desktop")]
pub(crate) fn send_pc_message(
    app: &tauri::AppHandle,
    settings: &SettingsState,
    text: String,
    file_paths: Vec<String>,
    uuid: Option<String>,
) -> Result<ChatMessage, String> {
    let files: Vec<(PathBuf, String)> = file_paths
        .iter()
        .map(|p| (PathBuf::from(p), derive_attachment_name(p)))
        .collect();
    if text.trim().is_empty() && files.is_empty() {
        return Err("Nothing to send — pass text or at least one file.".into());
    }
    for (p, _) in &files {
        if !p.is_file() {
            return Err(format!("attachment not readable: {}", p.display()));
        }
    }
    let uuid = uuid.unwrap_or_else(|| Uuid::new_v4().to_string());
    // A demoted PC (`role = "follower"`) never writes a local chat copy: stage
    // the send in the durable outbox mirror instead, delivered by the next sync
    // round or an open chat view (§3.4 writeback + §4 mirror discipline).
    if settings.role() != "hub" {
        let conn = open_mirror(settings)?;
        enqueue_outbox(settings, &conn, &uuid, &text, &files)?;
        let me = crate::sync::client::local_device_id(settings).unwrap_or_default();
        return Ok(ChatMessage {
            id: 0,
            uuid,
            sender_device: me,
            sender_name: crate::sync::client::device_name(),
            text,
            created_at: chrono::Utc::now().to_rfc3339(),
            attachments: Vec::new(),
            pending: true,
        });
    }
    let msg = insert_message(settings, &uuid, "pc", &pc_sender_name(), &text, &files)?;
    use tauri::Emitter;
    let _ = app.emit("chat-message", &msg);
    Ok(msg)
}

/// List the thread. On the hub this PC owns `messages.sqlite3` and is read
/// directly; on a follower (the phone, or a demoted PC running `role =
/// "follower"`) the durable mirror + outbox is served instead — after a
/// best-effort flush of anything queued and a pull of new hub rows (§4).
#[tauri::command]
#[allow(unused_variables)]
pub async fn chat_list_messages(
    settings: tauri::State<'_, SettingsState>,
    after: Option<String>,
    after_id: Option<i64>,
    limit: Option<i64>,
) -> Result<ChatListResponse, String> {
    #[cfg(feature = "desktop")]
    if settings.role() == "hub" {
        let messages = match after_id {
            Some(id) => list_messages_after_id(&settings, id, limit.unwrap_or(200))?,
            None => list_messages(&settings, after.as_deref().unwrap_or(""), limit.unwrap_or(200))?,
        };
        return Ok(ChatListResponse { messages, self_device: "pc".to_string() });
    }

    // Follower: deliver anything queued while offline, then pull new hub rows,
    // so the returned thread reflects the flush + the latest sync (§4).
    flush_pending(&settings).await;
    let _ = pull_into_mirror(&settings).await;
    let me = crate::sync::client::local_device_id(&settings).unwrap_or_default();
    let (mut messages, pending) = {
        let conn = open_mirror(&settings)?;
        (list_mirror(&conn, limit.unwrap_or(200))?, pending_outbox(&conn, &me)?)
    };
    messages.extend(pending);
    Ok(ChatListResponse { messages, self_device: me })
}

/// Send one message. On the hub it is stored on this PC and broadcast to open
/// desktop views; on a follower it is staged in the durable outbox (attachment
/// bytes copied so they survive) and flushed to the hub, staying queued if the
/// hub is unreachable (§4).
#[tauri::command]
#[allow(unused_variables)]
pub async fn chat_send_message(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    text: String,
    file_paths: Vec<String>,
    uuid: Option<String>,
) -> Result<ChatMessage, String> {
    log::info!(
        "[chat] chat_send_message: text_len={} files={} paths={:?}",
        text.trim().chars().count(),
        file_paths.len(),
        file_paths,
    );
    #[cfg(feature = "desktop")]
    if settings.role() == "hub" {
        return send_pc_message(&app, &settings, text, file_paths, uuid);
    }

    if text.trim().is_empty() && file_paths.is_empty() {
        return Err("Nothing to send — pass text or at least one file.".into());
    }
    if crate::sync::client::pc_base(&settings).is_empty() {
        return Err("No PC configured — connect on the Datasets page first.".into());
    }
    let uuid = uuid.unwrap_or_else(|| Uuid::new_v4().to_string());
    let files: Vec<(PathBuf, String)> = file_paths
        .iter()
        .map(|p| (PathBuf::from(p), derive_attachment_name(p)))
        .collect();

    // Stage in the durable outbox first, then release the connection before the
    // delivery await (no `!Send` `Connection` across `.await`).
    {
        let conn = open_mirror(&settings)?;
        if let Err(e) = enqueue_outbox(&settings, &conn, &uuid, &text, &files) {
            log::error!("[chat] enqueue_outbox failed for {uuid}: {e}");
            return Err(e);
        }
    }

    match flush_one(&settings, &uuid).await {
        // Delivered: `flush_one` mirrored the acknowledged row; return it so the
        // UI replaces the pending bubble.
        Ok(msg) => Ok(msg),
        // Offline / transient: the entry stays queued; echo its pending view so
        // it shows at the thread end, and a later poll flushes it.
        Err(e) => {
            log::warn!("[chat] send queued offline ({uuid}): {e}");
            let me = crate::sync::client::local_device_id(&settings).unwrap_or_default();
            let conn = open_mirror(&settings)?;
            if let Some(m) = pending_outbox(&conn, &me)?.into_iter().find(|m| m.uuid == uuid) {
                return Ok(m);
            }
            Ok(ChatMessage {
                id: 0,
                uuid,
                sender_device: me,
                sender_name: crate::sync::client::device_name(),
                text,
                created_at: chrono::Utc::now().to_rfc3339(),
                attachments: Vec::new(),
                pending: true,
            })
        }
    }
}

/// Resolve an attachment to a local path the WebView can open. On the hub it is
/// the stored file (resolved by uuid, never a client-steered path); on a follower
/// a pending offline send's staged bytes serve directly, else the bytes are
/// streamed from the hub into the download cache (§4).
#[tauri::command]
#[allow(unused_variables)]
pub async fn chat_resolve_attachment(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    uuid: String,
    filename: Option<String>,
) -> Result<String, String> {
    #[cfg(feature = "desktop")]
    if settings.role() == "hub" {
        let (path, _att) = attachment_file(&settings, &uuid)?;
        return Ok(path.to_string_lossy().into_owned());
    }

    if !valid_message_uuid(&uuid) {
        return Err("invalid attachment id".into());
    }
    if let Some(local) = outbox_attachment(&settings, &uuid) {
        return Ok(local.to_string_lossy().into_owned());
    }
    let cache = chat_root(&settings)?.join("cache");
    let dest = cache_path(&cache, &uuid, filename.as_deref());
    download_to(&app, &settings, &uuid, &dest).await?;
    prune_cache(&cache, CACHE_KEEP_BYTES);
    Ok(dest.to_string_lossy().into_owned())
}

/// Copy a hub-stored attachment to a user-picked location, reporting progress.
/// The desktop file is already local, so this is a plain chunked copy.
#[cfg(feature = "desktop")]
async fn copy_local_attachment(
    app: &tauri::AppHandle,
    settings: &SettingsState,
    uuid: &str,
    dest: String,
) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (src, _att) = attachment_file(settings, uuid)?;
    let mut input = tokio::fs::File::open(&src)
        .await
        .map_err(|e| format!("cannot open attachment: {e}"))?;
    let total = input.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut output = tokio::fs::File::create(&dest)
        .await
        .map_err(|e| format!("cannot create {dest}: {e}"))?;

    let copied: Result<u64, String> = async {
        let mut buf = vec![0u8; 64 * 1024];
        let mut received: u64 = 0;
        let mut last_pct: i64 = -1;
        loop {
            let n = input.read(&mut buf).await.map_err(|e| format!("read failed: {e}"))?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n]).await.map_err(|e| format!("write failed: {e}"))?;
            received += n as u64;
            let pct = if total > 0 { (received * 100 / total) as i64 } else { -1 };
            if pct != last_pct {
                last_pct = pct;
                emit_progress(app, uuid, received, total, false);
            }
        }
        output.flush().await.map_err(|e| format!("write failed: {e}"))?;
        Ok(received)
    }
    .await;
    match copied {
        Ok(received) => {
            emit_progress(app, uuid, received, total, true);
            Ok(dest)
        }
        // Never leave a truncated file behind at the destination.
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            Err(e)
        }
    }
}

/// Save an attachment. On the hub this is a copy of the local store to the
/// user's chosen `dest`; on a follower saving means keeping a hub-downloaded
/// copy in the cache (honouring an explicit `dest` when given) so it survives
/// going offline (§4).
#[tauri::command]
#[allow(unused_variables)]
pub async fn chat_save_attachment(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    uuid: String,
    filename: Option<String>,
    dest: Option<String>,
) -> Result<String, String> {
    #[cfg(feature = "desktop")]
    if settings.role() == "hub" {
        let dest = dest.ok_or_else(|| "no destination chosen".to_string())?;
        return copy_local_attachment(&app, &settings, &uuid, dest).await;
    }

    if !valid_message_uuid(&uuid) {
        return Err("invalid attachment id".into());
    }
    if let Some(local) = outbox_attachment(&settings, &uuid) {
        let path = match dest.filter(|d| !d.trim().is_empty()) {
            Some(d) => {
                let _ = std::fs::copy(&local, &d);
                PathBuf::from(d)
            }
            None => local,
        };
        return Ok(path.to_string_lossy().into_owned());
    }
    let cache = chat_root(&settings)?.join("cache");
    let path = match dest.filter(|d| !d.trim().is_empty()) {
        Some(d) => PathBuf::from(d),
        None => cache_path(&cache, &uuid, filename.as_deref()),
    };
    download_to(&app, &settings, &uuid, &path).await?;
    prune_cache(&cache, CACHE_KEEP_BYTES);
    Ok(path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Mobile (thin client) commands — offline mirror + outbox over REST (§4)
// ---------------------------------------------------------------------------

/// The phone-local chat DB, `<workspace>/chat/mirror.sqlite3`. It mirrors the
/// hub thread (rows carry the hub's monotonic `id` and are pulled incrementally
/// with `after_id`) and holds a durable `chat_outbox` of messages composed while
/// offline. Shares the message schema via [`ensure_chat_tables`] so ids rebuild
/// identically, then layers the outbox + a cursor row on top.
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
fn open_mirror(settings: &SettingsState) -> Result<Connection, String> {
    let path = chat_root(settings)?.join("mirror.sqlite3");
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    ensure_chat_tables(&conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_sync_state(
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS chat_outbox(
             uuid        TEXT PRIMARY KEY,
             text        TEXT NOT NULL DEFAULT '',
             edit_time   TEXT NOT NULL,
             sender_name TEXT NOT NULL DEFAULT ''
         );
         CREATE TABLE IF NOT EXISTS chat_outbox_file(
             uuid         TEXT PRIMARY KEY,
             outbox_uuid  TEXT NOT NULL,
             stored_name  TEXT NOT NULL,
             filename     TEXT NOT NULL,
             mime         TEXT NOT NULL DEFAULT '',
             size         INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_outbox_file ON chat_outbox_file(outbox_uuid);",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// App-private directory for offline-sent attachment bytes. Android content
/// URIs do not survive a reboot / provider teardown, so a pending send copies
/// its files here at enqueue time and flushes from here (§4).
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
fn outbox_dir(settings: &SettingsState) -> Result<PathBuf, String> {
    let dir = chat_root(settings)?.join("outbox");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
fn mirror_cursor(conn: &Connection) -> i64 {
    conn.query_row("SELECT value FROM chat_sync_state WHERE key = 'after_id'", [], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(0)
}

// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
fn set_mirror_cursor(conn: &Connection, id: i64) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO chat_sync_state(key, value) VALUES('after_id', ?1)",
        params![id.to_string()],
    );
}

/// Idempotent upsert of a hub message (with its `id`) into the mirror; the
/// attachment rows let the thread render file chips offline.
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
fn upsert_mirror(conn: &Connection, m: &ChatMessage) -> Result<(), String> {
    conn.execute(
        "INSERT INTO chat_messages(id, uuid, sender_device, sender_name, text, created_at) \
         VALUES(?1,?2,?3,?4,?5,?6) \
         ON CONFLICT(uuid) DO UPDATE SET id=excluded.id, sender_device=excluded.sender_device, \
             sender_name=excluded.sender_name, text=excluded.text, created_at=excluded.created_at",
        params![m.id, m.uuid, m.sender_device, m.sender_name, m.text, m.created_at],
    )
    .map_err(|e| e.to_string())?;
    for a in &m.attachments {
        conn.execute(
            "INSERT OR REPLACE INTO chat_attachments(uuid, message_uuid, filename, mime, size, stored_name) \
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![a.uuid, m.uuid, a.filename, a.mime, a.size, format!("{}--{}", a.uuid, sanitize_filename(&a.filename))],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Newest `limit` mirrored messages, hub `id` ascending (§4.3 mirror read).
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
fn list_mirror(conn: &Connection, limit: i64) -> Result<Vec<ChatMessage>, String> {
    let limit = limit.clamp(1, 500);
    let mut stmt = conn
        .prepare(
            "SELECT id, uuid, sender_device, sender_name, text, created_at FROM chat_messages \
             ORDER BY id DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit], row_to_message)
        .map_err(|e| e.to_string())?;
    let mut msgs: Vec<ChatMessage> = rows.filter_map(|r| r.ok()).collect();
    drop(stmt);
    msgs.sort_by_key(|m| m.id);
    for m in &mut msgs {
        m.attachments = attachments_for(conn, &m.uuid)?;
    }
    Ok(msgs)
}

/// Still-pending offline sends, rendered at the thread end with `pending = true`
/// and `id = 0` (no hub rowid yet). Their attachments point at the local outbox
/// files so previews work with no network.
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
/// Number of offline messages still sitting in the local outbox, awaiting a
/// successful flush to the hub. Read-only status accessor (§3.5) — returns `0`
/// when no mirror exists yet or the read fails, so a status surface never
/// errors out just because chat has not been opened.
pub fn pending_outbox_count(settings: &SettingsState) -> i64 {
    open_mirror(settings)
        .and_then(|conn| {
            conn.query_row("SELECT COUNT(*) FROM chat_outbox", [], |r| r.get::<_, i64>(0))
                .map_err(|e| e.to_string())
        })
        .unwrap_or(0)
}

fn pending_outbox(conn: &Connection, me_device: &str) -> Result<Vec<ChatMessage>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT uuid, text, edit_time, sender_name FROM chat_outbox ORDER BY edit_time",
        )
        .map_err(|e| e.to_string())?;
    let base: Vec<ChatMessage> = stmt
        .query_map([], |r| {
            Ok(ChatMessage {
                id: 0,
                uuid: r.get(0)?,
                sender_device: me_device.to_string(),
                sender_name: r.get(3)?,
                text: r.get(1)?,
                created_at: r.get(2)?,
                attachments: Vec::new(),
                pending: true,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    let mut out = Vec::with_capacity(base.len());
    for mut m in base {
        let mut fs = conn
            .prepare(
                "SELECT uuid, filename, mime, size FROM chat_outbox_file WHERE outbox_uuid = ?1",
            )
            .map_err(|e| e.to_string())?;
        let atts = fs
            .query_map(params![m.uuid], |r| {
                Ok(ChatAttachment {
                    uuid: r.get(0)?,
                    filename: r.get(1)?,
                    mime: r.get(2)?,
                    size: r.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        m.attachments = atts;
        out.push(m);
    }
    Ok(out)
}

/// Copy a send's attachments into the outbox dir and record the row + file
/// entries. Idempotent on `uuid` (a retry overwrites the same outbox row).
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
fn enqueue_outbox(
    settings: &SettingsState,
    conn: &Connection,
    uuid: &str,
    text: &str,
    files: &[(PathBuf, String)],
) -> Result<(), String> {
    let dir = outbox_dir(settings)?;
    let edit_time = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    conn
        .execute(
            "INSERT OR REPLACE INTO chat_outbox(uuid, text, edit_time, sender_name) \
             VALUES(?1, ?2, ?3, ?4)",
            params![uuid, text, edit_time, crate::sync::client::device_name()],
        )
        .map_err(|e| e.to_string())?;
    for (src, orig) in files {
        let att_uuid = Uuid::new_v4().to_string();
        let filename = sanitize_filename(orig);
        let mime = guess_mime(&filename).to_string();
        let stored_name = format!("{att_uuid}--{filename}");
        let dest = dir.join(&stored_name);
        if let Err(e) = stage_source(src, &dest) {
            log::error!("[chat] cannot stage {orig}: {e}");
            let _ = std::fs::remove_file(&dest);
            // The outbox row was inserted above; drop it (and any files staged
            // so far) so a failed send never lingers as a permanent "pending"
            // ghost that a later poll keeps trying to flush.
            drop_outbox(settings, conn, uuid);
            return Err(format!("cannot stage {filename}: {e}"));
        }
        let size = std::fs::metadata(&dest).map(|m| m.len() as i64).unwrap_or(0);
        conn.execute(
            "INSERT OR REPLACE INTO chat_outbox_file(uuid, outbox_uuid, stored_name, filename, mime, size) \
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![att_uuid, uuid, stored_name, filename, mime, size],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Stage one attachment source into `dest`. A plain filesystem path (desktop,
/// or any already-materialized file) is copied directly; an Android SAF
/// `content://` URI carries no readable path, so it is streamed through the OS
/// ContentResolver into a real file (see [`copy_content_uri`]).
fn stage_source(src: &Path, dest: &Path) -> Result<(), String> {
    if src.is_file() {
        return std::fs::copy(src, dest)
            .map(|_| ())
            .map_err(|e| format!("copy {}: {e}", src.display()));
    }
    let raw = src.to_string_lossy().into_owned();
    #[cfg(target_os = "android")]
    if raw.starts_with("content://") || raw.starts_with("android.resource://") {
        return copy_content_uri(&raw, dest);
    }
    Err(format!("attachment not readable: {raw}"))
}

/// Derive a display filename from a picked attachment path. A SAF `content://`
/// URI percent-encodes the real document tail (`…/primary%3ADCIM%2FCamera%2FIMG.jpg`),
/// so it is percent-decoded and the last path segment taken; anything else falls
/// back to the plain filesystem basename.
fn derive_attachment_name(raw: &str) -> String {
    if raw.starts_with("content://")
        || raw.starts_with("android.resource://")
        || raw.starts_with("file://")
    {
        let decoded = urlencoding::decode(raw)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| raw.to_string());
        if let Some(last) = decoded.rsplit(['/', '\\']).next().map(str::trim) {
            if !last.is_empty() {
                return last.to_string();
            }
        }
    }
    Path::new(raw)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string())
}

/// Read a SAF `content://` URI into `dest` via the Android ContentResolver over
/// JNI. `openFileDescriptor` yields a native fd copied with `std::io::copy`
/// (streaming, so large files don't load into RAM); ownership is returned to
/// Java with `mem::forget` so Rust never closes an fd the `ParcelFileDescriptor`
/// still owns, and the descriptor is closed on the Java side afterwards.
#[cfg(target_os = "android")]
fn copy_content_uri(uri: &str, dest: &Path) -> Result<(), String> {
    use jni::objects::{JObject, JValue};

    let ctx = ndk_context::android_context();
    let vm =
        unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| format!("jni vm: {e}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("jni attach: {e}"))?;
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };
    let err = |e: jni::errors::Error| e.to_string();

    // android.net.Uri uri = android.net.Uri.parse(path)
    let uri_str = env.new_string(uri).map_err(err)?;
    let uri_obj: &JObject = &uri_str;
    let uri_cls = env.find_class("android/net/Uri").map_err(err)?;
    let juri = env
        .call_static_method(
            uri_cls,
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(uri_obj)],
        )
        .map_err(err)?
        .l()
        .map_err(err)?;

    // ContentResolver r = context.getContentResolver()
    let resolver = env
        .call_method(
            &context,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )
        .map_err(err)?
        .l()
        .map_err(err)?;

    // ParcelFileDescriptor pfd = r.openFileDescriptor(uri, "r")
    let mode = env.new_string("r").map_err(err)?;
    let mode_obj: &JObject = &mode;
    let pfd = env
        .call_method(
            &resolver,
            "openFileDescriptor",
            "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
            &[JValue::Object(&juri), JValue::Object(mode_obj)],
        )
        .map_err(err)?
        .l()
        .map_err(err)?;
    if pfd.is_null() {
        return Err(format!("openFileDescriptor returned null for {uri}"));
    }

    // int fd = pfd.getFd() -> copy the fd straight into dest, then hand the fd
    // back to Java (mem::forget) before closing the descriptor on the JVM side.
    let fd = env
        .call_method(&pfd, "getFd", "()I", &[])
        .map_err(err)?
        .i()
        .map_err(err)?;
    let copied = (|| -> Result<(), String> {
        use std::os::unix::io::FromRawFd;
        let mut src = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut out = std::fs::File::create(dest).map_err(|e| format!("create {dest:?}: {e}"))?;
        std::io::copy(&mut src, &mut out).map_err(|e| format!("read {uri}: {e}"))?;
        std::mem::forget(src);
        Ok(())
    })();

    let _ = env.call_method(&pfd, "close", "()V", &[]);
    copied?;
    log::info!("[chat] staged content uri {uri} -> {dest:?}");
    Ok(())
}

/// Remove a flushed outbox entry and its staged attachment files.
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
fn drop_outbox(settings: &SettingsState, conn: &Connection, uuid: &str) {
    if let Ok(dir) = outbox_dir(settings) {
        let names: Vec<String> = conn
            .prepare("SELECT stored_name FROM chat_outbox_file WHERE outbox_uuid = ?1")
            .and_then(|mut s| {
                s.query_map(params![uuid], |r| r.get::<_, String>(0))
                    .map(|rows| rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();
        for n in names {
            let _ = std::fs::remove_file(dir.join(n));
        }
    }
    let _ = conn.execute("DELETE FROM chat_outbox_file WHERE outbox_uuid = ?1", params![uuid]);
    let _ = conn.execute("DELETE FROM chat_outbox WHERE uuid = ?1", params![uuid]);
}

/// POST one pending outbox message to the hub. On success mirrors the returned
/// hub row, advances the cursor, and clears the outbox entry, returning the
/// acknowledged message. Transport/status errors are returned as `Err` so the
/// entry stays queued for a later flush (§4: offline send flushes when reachable).
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
async fn flush_one(settings: &SettingsState, uuid: &str) -> Result<ChatMessage, String> {
    let base = crate::sync::client::pc_base(settings);
    if base.is_empty() {
        return Err("no PC configured".to_string());
    }
    // 1. Snapshot the queued entry into owned data, then drop the connection so
    //    no `!Send` `Connection` stays alive across the network await below.
    let (text, files) = {
        let conn = open_mirror(settings)?;
        let text: String = conn
            .query_row(
                "SELECT text FROM chat_outbox WHERE uuid = ?1",
                params![uuid],
                |r| r.get(0),
            )
            .map_err(|_| "outbox entry gone".to_string())?;
        let mut stmt = conn
            .prepare("SELECT stored_name, filename FROM chat_outbox_file WHERE outbox_uuid = ?1")
            .map_err(|e| e.to_string())?;
        let files: Vec<(String, String)> = stmt
            .query_map(params![uuid], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        (text, files)
    };

    // 2. POST the multipart upload, reading the staged bytes off disk. No
    //    connection is held here, so the future stays `Send`.
    let dir = outbox_dir(settings)?;
    let mut form = reqwest::multipart::Form::new()
        .text("uuid", uuid.to_string())
        .text("text", text)
        .text("device_name", crate::sync::client::device_name());
    for (stored_name, filename) in &files {
        let bytes = tokio::fs::read(dir.join(stored_name))
            .await
            .map_err(|e| format!("cannot read staged attachment {filename}: {e}"))?;
        form = form.part("file", reqwest::multipart::Part::bytes(bytes).file_name(filename.clone()));
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let req = crate::sync::client::with_device_auth(
        client.post(format!("{base}/api/v1/chat/message")),
        settings,
        "POST",
        "/api/v1/chat/message",
        "",
        b"",
    )?;
    let resp = req
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("PC unreachable: {e}"))?;
    let v: serde_json::Value = resp
        .error_for_status()
        .map_err(|e| format!("PC rejected the message: {e}"))?
        .json()
        .await
        .map_err(|e| format!("unexpected response from PC: {e}"))?;
    let msg: ChatMessage =
        serde_json::from_value(v).map_err(|e| format!("unexpected chat/message response: {e}"))?;

    // 3. Mirror the acknowledged row + clear the outbox on a fresh connection.
    {
        let conn = open_mirror(settings)?;
        upsert_mirror(&conn, &msg)?;
        if msg.id > mirror_cursor(&conn) {
            set_mirror_cursor(&conn, msg.id);
        }
        drop_outbox(settings, &conn, uuid);
    }
    Ok(msg)
}

/// Flush every pending outbox entry, stopping at the first transport failure so
/// an offline call is cheap and a later poll resumes where it left off.
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
async fn flush_pending(settings: &SettingsState) {
    let ids: Vec<String> = {
        let conn = match open_mirror(settings) {
            Ok(c) => c,
            Err(_) => return,
        };
        // Drop ghost entries left by earlier failed sends: empty text and no
        // staged attachment bytes. They can never flush, so clearing them stops
        // the thread showing a permanent "pending" bubble.
        let ghosts: Vec<String> = conn
            .prepare(
                "SELECT uuid FROM chat_outbox \
                 WHERE trim(text) = '' AND uuid NOT IN (SELECT outbox_uuid FROM chat_outbox_file)",
            )
            .and_then(|mut s| {
                s.query_map([], |r| r.get::<_, String>(0))
                    .map(|rows| rows.filter_map(|r| r.ok()).collect())
            })
            .unwrap_or_default();
        for g in &ghosts {
            log::info!("[chat] dropping stale empty outbox entry {g}");
            drop_outbox(settings, &conn, g);
        }
        let mut stmt = match conn.prepare("SELECT uuid FROM chat_outbox ORDER BY edit_time") {
            Ok(s) => s,
            Err(_) => return,
        };
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        drop(stmt);
        ids
    };
    for id in ids {
        if flush_one(settings, &id).await.is_err() {
            break; // offline: leave the rest queued for a later poll
        }
    }
}

/// Pull the hub thread incrementally (`after_id` = mirror cursor) into the
/// mirror and advance the cursor to the highest id seen. The network read happens
/// with no connection held (the cursor read and the upsert write are separate
/// short-lived opens) so the future is `Send`.
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
async fn pull_into_mirror(settings: &SettingsState) -> Result<(), String> {
    let after = {
        let conn = open_mirror(settings)?;
        mirror_cursor(&conn)
    };
    let query = vec![("after_id", after.to_string()), ("limit", "500".to_string())];
    let resp = crate::sync::client::pc_get_json(settings, "/api/v1/chat/messages", &query).await?;
    let arr = resp.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let msgs: Vec<ChatMessage> =
        arr.into_iter().filter_map(|i| serde_json::from_value(i).ok()).collect();
    let conn = open_mirror(settings)?;
    let mut highest = after;
    for m in &msgs {
        upsert_mirror(&conn, m)?;
        if m.id > highest {
            highest = m.id;
        }
    }
    set_mirror_cursor(&conn, highest);
    Ok(())
}

/// If `att_uuid` belongs to a still-pending offline send, its bytes are already
/// staged in the app-private outbox dir — return that path so a preview of a
/// just-composed attachment needs no network (§4).
fn outbox_attachment(settings: &SettingsState, att_uuid: &str) -> Option<PathBuf> {
    let conn = open_mirror(settings).ok()?;
    let stored: String = conn
        .query_row(
            "SELECT stored_name FROM chat_outbox_file WHERE uuid = ?1",
            params![att_uuid],
            |r| r.get(0),
        )
        .ok()?;
    let path = outbox_dir(settings).ok()?.join(stored);
    if path.exists() { Some(path) } else { None }
}

/// Cap on the follower's attachment download cache (§4). Once over it, the
/// oldest files (by mtime) are removed until the newest `KEEP`-worth remain.
const CACHE_KEEP_BYTES: u64 = 256 * 1024 * 1024;

/// Prune a directory of loose files down to the newest `keep_bytes` total,
/// deleting older ones. Best-effort: any I/O error just leaves a file in place.
fn prune_cache(dir: &Path, keep_bytes: u64) {
    use std::time::SystemTime;
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file() {
            if let Ok(m) = std::fs::metadata(&p) {
                let t = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((t, m.len(), p));
            }
        }
    }
    // Oldest first, so we drop from the front while over budget.
    files.sort_by_key(|(t, _, _)| *t);
    let mut total: u64 = files.iter().map(|(_, s, _)| *s).sum();
    for (_, size, path) in files {
        if total <= keep_bytes {
            break;
        }
        let _ = std::fs::remove_file(&path);
        total = total.saturating_sub(size);
    }
}

/// Cache filename for a downloaded attachment: `<uuid>--<sanitized name>` when
/// the name is known (mirrors the PC store layout one level down), else `<uuid>`.
#[cfg_attr(feature = "desktop", allow(dead_code))]
fn cache_path(dir: &Path, uuid: &str, filename: Option<&str>) -> PathBuf {
    match filename {
        Some(f) if !f.is_empty() => dir.join(format!("{uuid}--{}", sanitize_filename(f))),
        _ => dir.join(uuid),
    }
}

/// Stream one attachment over REST into `dest`, reporting byte progress. A file
/// that is already there short-circuits at 100 %, so re-opening a preview or
/// pressing save twice costs nothing. Bytes land in a `.part` sibling that is
/// renamed only on success, so an interrupted download never looks complete.
// These mirror helpers run on the phone always and on the desktop whenever this
// workspace runs `role = "follower"` (a demoted PC relays chat over REST), so
// they compile on both targets and are reached from the unified commands below.
async fn download_to(
    app: &tauri::AppHandle,
    settings: &SettingsState,
    uuid: &str,
    dest: &Path,
) -> Result<(), String> {
    use futures_util::StreamExt;
    use std::io::Write as _;

    if dest.exists() {
        let total = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        emit_progress(app, uuid, total, total, true);
        return Ok(());
    }
    let dir = dest
        .parent()
        .ok_or_else(|| "attachment path has no directory".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let base = crate::sync::client::pc_base(settings);
    if base.is_empty() {
        return Err("No PC configured — connect on the Datasets page first.".into());
    }
    let api_path = format!("/api/v1/chat/attachment/{uuid}");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let req = crate::sync::client::with_device_auth(
        client.get(format!("{base}{api_path}")),
        settings,
        "GET",
        &api_path,
        "",
        b"",
    )?;
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Cannot reach PC at {base}: {e}"))?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(0);
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| uuid.to_string());

    let tmp = dir.join(format!("{name}.part"));
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut stream = resp.bytes_stream();
    let mut received: u64 = 0;
    let mut last_pct: i64 = -1;
    let mut failure: Option<String> = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                if let Err(e) = file.write_all(&bytes) {
                    failure = Some(format!("cannot write {name}: {e}"));
                    break;
                }
                received += bytes.len() as u64;
                let pct = if total > 0 { (received * 100 / total) as i64 } else { -1 };
                if pct != last_pct {
                    last_pct = pct;
                    emit_progress(app, uuid, received, total, false);
                }
            }
            Err(e) => {
                failure = Some(format!("download failed: {e}"));
                break;
            }
        }
    }
    if failure.is_none() {
        if let Err(e) = file.flush() {
            failure = Some(format!("cannot write {name}: {e}"));
        }
    }
    drop(file);
    if let Some(e) = failure {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot store {name}: {e}")
    })?;
    emit_progress(app, uuid, received, total, true);
    Ok(())
}
