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
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_messages(
            uuid          TEXT PRIMARY KEY,
            sender_device TEXT NOT NULL,
            sender_name   TEXT NOT NULL,
            text          TEXT NOT NULL DEFAULT '',
            created_at    TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chat_attachments(
            uuid         TEXT PRIMARY KEY,
            message_uuid TEXT NOT NULL,
            filename     TEXT NOT NULL,
            mime         TEXT NOT NULL DEFAULT '',
            size         INTEGER NOT NULL DEFAULT 0,
            stored_name  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_chat_messages_created ON chat_messages(created_at);
        CREATE INDEX IF NOT EXISTS idx_chat_attachments_msg ON chat_attachments(message_uuid);",
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
        uuid: r.get(0)?,
        sender_device: r.get(1)?,
        sender_name: r.get(2)?,
        text: r.get(3)?,
        created_at: r.get(4)?,
        attachments: Vec::new(),
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
            "SELECT uuid, sender_device, sender_name, text, created_at \
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
            "SELECT uuid, sender_device, sender_name, text, created_at FROM chat_messages \
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

    let db_result = (|| -> Result<(), String> {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO chat_messages(uuid, sender_device, sender_name, text, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![uuid, sender_device, sender_name, text, created_at],
        )
        .map_err(|e| e.to_string())?;
        for (att, stored_name) in stored.iter().zip(copied_names.iter()) {
            tx.execute(
                "INSERT INTO chat_attachments(uuid, message_uuid, filename, mime, size, stored_name) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![att.uuid, uuid, att.filename, att.mime, att.size, stored_name],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    })();
    if let Err(e) = db_result {
        rollback(&copied_names);
        return Err(e);
    }

    Ok(ChatMessage {
        uuid: uuid.to_string(),
        sender_device: sender_device.to_string(),
        sender_name: sender_name.to_string(),
        text: text.to_string(),
        created_at,
        attachments: stored,
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
        .map(|p| {
            let name = Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_string());
            (PathBuf::from(p), name)
        })
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
    let msg = insert_message(settings, &uuid, "pc", &pc_sender_name(), &text, &files)?;
    use tauri::Emitter;
    let _ = app.emit("chat-message", &msg);
    Ok(msg)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn chat_list_messages(
    settings: tauri::State<'_, SettingsState>,
    after: Option<String>,
    limit: Option<i64>,
) -> Result<ChatListResponse, String> {
    let messages = list_messages(&settings, after.as_deref().unwrap_or(""), limit.unwrap_or(200))?;
    Ok(ChatListResponse { messages, self_device: "pc".to_string() })
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn chat_send_message(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    text: String,
    file_paths: Vec<String>,
    uuid: Option<String>,
) -> Result<ChatMessage, String> {
    send_pc_message(&app, &settings, text, file_paths, uuid)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn chat_resolve_attachment(
    settings: tauri::State<'_, SettingsState>,
    uuid: String,
    // Accepted for symmetry with the mobile command (which needs the name to
    // label its download cache); the stored file path is resolved by uuid only.
    #[allow(unused_variables)] filename: Option<String>,
) -> Result<String, String> {
    let (path, _att) = attachment_file(&settings, &uuid)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Copy a stored attachment to a location the user picked, reporting progress.
/// The desktop file is already local, so this is a plain chunked copy.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn chat_save_attachment(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    uuid: String,
    // Accepted for symmetry with the mobile command (which needs the name to
    // build its cache filename); the desktop copy reads the stored file by uuid.
    #[allow(unused_variables)] filename: Option<String>,
    dest: String,
) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (src, _att) = attachment_file(&settings, &uuid)?;
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
                emit_progress(&app, &uuid, received, total, false);
            }
        }
        output.flush().await.map_err(|e| format!("write failed: {e}"))?;
        Ok(received)
    }
    .await;
    match copied {
        Ok(received) => {
            emit_progress(&app, &uuid, received, total, true);
            Ok(dest)
        }
        // Never leave a truncated file behind at the destination.
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Mobile (thin client) commands — relay to the PC over REST
// ---------------------------------------------------------------------------

#[cfg(not(feature = "desktop"))]
#[tauri::command]
pub async fn chat_list_messages(
    settings: tauri::State<'_, SettingsState>,
    after: Option<String>,
    limit: Option<i64>,
) -> Result<serde_json::Value, String> {
    let mut query: Vec<(&str, String)> =
        vec![("limit", limit.unwrap_or(200).to_string())];
    if let Some(a) = after.as_deref().filter(|s| !s.is_empty()) {
        query.push(("after", a.to_string()));
    }
    crate::sync::pc_get_json(&settings, "/api/v1/chat/messages", &query).await
}

#[cfg(not(feature = "desktop"))]
#[tauri::command]
pub async fn chat_send_message(
    settings: tauri::State<'_, SettingsState>,
    text: String,
    file_paths: Vec<String>,
    uuid: Option<String>,
) -> Result<ChatMessage, String> {
    use std::time::Duration;

    if text.trim().is_empty() && file_paths.is_empty() {
        return Err("Nothing to send — pass text or at least one file.".into());
    }
    let mut form = reqwest::multipart::Form::new()
        .text(
            "uuid",
            uuid.unwrap_or_else(|| Uuid::new_v4().to_string()),
        )
        .text("text", text)
        .text("device_name", crate::sync::device_name());
    for p in &file_paths {
        let name = Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        let bytes = tokio::fs::read(p)
            .await
            .map_err(|e| format!("Cannot read {p}: {e}"))?;
        form = form.part("file", reqwest::multipart::Part::bytes(bytes).file_name(name));
    }

    let base = crate::sync::pc_base(&settings);
    if base.is_empty() {
        return Err("No PC configured — connect on the Datasets page first.".into());
    }
    // No overall timeout: large attachments over WLAN can legitimately take
    // minutes; only the connect phase is bounded.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let req = crate::sync::with_device_auth(
        client.post(format!("{base}/api/v1/chat/message")),
        &settings,
        "POST",
        "/api/v1/chat/message",
    )?;
    let resp = req
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("PC unreachable: {e}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        let msg = body["error"].as_str().unwrap_or("device not paired");
        return Err(format!("{msg} (http://{base})"));
    }
    let v: serde_json::Value = resp
        .error_for_status()
        .map_err(|e| format!("PC rejected the message: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Unexpected response from PC: {e}"))?;
    serde_json::from_value(v).map_err(|e| format!("Unexpected chat/message response: {e}"))
}

#[cfg(not(feature = "desktop"))]
#[tauri::command]
pub async fn chat_resolve_attachment(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    uuid: String,
    filename: Option<String>,
) -> Result<String, String> {
    if !valid_message_uuid(&uuid) {
        return Err("invalid attachment id".into());
    }
    // Same layout as the PC store, one level down: `<workspace>/chat/cache`.
    let dest = cache_path(&chat_root(&settings)?.join("cache"), &uuid, filename.as_deref());
    download_to(&app, &settings, &uuid, &dest).await?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Pull an attachment from the PC into the phone's chat cache. Android has no
/// native "save as" dialog the WebView can drive, so saving here means keeping
/// a local copy that survives going offline; an explicit `dest` is honoured for
/// callers that already have a writable path.
#[cfg(not(feature = "desktop"))]
#[tauri::command]
pub async fn chat_save_attachment(
    app: tauri::AppHandle,
    settings: tauri::State<'_, SettingsState>,
    uuid: String,
    filename: Option<String>,
    dest: Option<String>,
) -> Result<String, String> {
    if !valid_message_uuid(&uuid) {
        return Err("invalid attachment id".into());
    }
    let path = match dest.filter(|d| !d.trim().is_empty()) {
        Some(d) => PathBuf::from(d),
        None => cache_path(&chat_root(&settings)?.join("cache"), &uuid, filename.as_deref()),
    };
    download_to(&app, &settings, &uuid, &path).await?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(not(feature = "desktop"))]
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
#[cfg(not(feature = "desktop"))]
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

    let base = crate::sync::pc_base(settings);
    if base.is_empty() {
        return Err("No PC configured — connect on the Datasets page first.".into());
    }
    let api_path = format!("/api/v1/chat/attachment/{uuid}");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let req = crate::sync::with_device_auth(
        client.get(format!("{base}{api_path}")),
        settings,
        "GET",
        &api_path,
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
