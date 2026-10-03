//! Built-in MCP (Model Context Protocol) server for Goose integration.
//!
//! Uses the `rmcp` crate (v3.4) for protocol handling with Streamable HTTP transport.
//! Runs on the same axum HTTP server as the web service, nested at `/mcp`.
//!
//! Connect Goose Desktop at: http://localhost:35711/mcp
use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::adjust;
use crate::align;
use crate::auth;
use crate::book;
use crate::cards;
use crate::cards_sync;
use crate::capture;
use crate::dataset;
use crate::dictation;
use crate::edge_tts;
use crate::llm;
use crate::logger::LogBuffer;
use crate::model::ModelState;
use crate::ocr::{self, OcrState};
use crate::settings::SettingsState;
use crate::web_service::{self, WebServiceState, WebServiceConfig};

// ---------------------------------------------------------------------------
// MCP server state — shared across all connections
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct DatasetMcpServer {
    pub app: AppHandle,
}

// ---------------------------------------------------------------------------
// Tool parameter types
// ---------------------------------------------------------------------------

/// Wrapper around `serde_json::Value` that generates a proper JSON Schema
/// (`{"type": "object"}`) instead of the boolean `true` that `serde_json::Value`
/// produces. Ollama's tool parser cannot handle boolean schemas in `properties`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(transparent)]
struct JsonValue(serde_json::Value);

impl schemars::JsonSchema for JsonValue {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "JsonValue".into()
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object"
        })
    }

    fn inline_schema() -> bool {
        true
    }
}

impl From<JsonValue> for serde_json::Value {
    fn from(v: JsonValue) -> Self {
        v.0
    }
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct UuidParam {
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadFileParam {
    uuid: String,
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WaveformSingleParam {
    uuid: String,
    media_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WordLevelSyncParam {
    uuid: String,
    /// Optional: limit to a single media file. If omitted, processes all media.
    media_uuid: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ListSubtitlesParam {
    uuid: String,
    /// Optional: filter by media UUID.
    #[serde(default)]
    media_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ListCuesParam {
    uuid: String,
    subtitle_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AdjustCueTimeParam {
    uuid: String,
    #[serde(default = "default_adjust_mode")]
    mode: String,
    /// Force re-adjustment of already-adjusted subtitles (default: false, skips them).
    #[serde(default)]
    force: bool,
    /// Silence detection strategy: "noise_floor" (default), "dual_bound", "peak_relative", "otsu".
    #[serde(default)]
    strategy: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct AdjustCueTimeSingleParam {
    /// Dataset UUID.
    uuid: String,
    /// Media UUID to adjust cues for.
    media_uuid: String,
    /// Mode: 'new' (create adjusted copy) or 'in_place' (overwrite original cues).
    #[serde(default = "default_adjust_mode")]
    mode: String,
    /// Force re-adjustment of already-adjusted subtitles (default: false).
    #[serde(default)]
    force: bool,
    /// Silence detection strategy: "noise_floor" (default), "dual_bound", "peak_relative", "otsu".
    #[serde(default)]
    strategy: Option<String>,
}

fn default_adjust_mode() -> String {
    "new".to_string()
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SyncCueTimesParam {
    uuid: String,
    /// Minimum similarity score (0-100) to match a fresh cue to a DB cue. Default: 70.
    threshold: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SubtitleVersionParam {
    subtitle_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SubtitleVersionAtParam {
    subtitle_uuid: String,
    version: i64,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SubtitleRollbackParam {
    subtitle_uuid: String,
    target_version: i64,
}

// -- Model management --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ModelVersionParam {
    version: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ModelLoadParam {
    #[serde(default)]
    version: String,
}

// -- Settings --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SettingsSetParam {
    settings: JsonValue,
}

// -- Dataset management --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct PathParam {
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DatasetListDirsParam {
    dataset_type: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DatasetAddDirParam {
    name: String,
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DatasetImportParam {
    source_dir: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DatasetCreateParam {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    location: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DatasetUpdateParam {
    uuid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DatasetImportMediaParam {
    uuid: String,
    source_dir: String,
    #[serde(default)]
    link: bool,
}

// -- Dictation --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DictationSaveProgressParam {
    media_uuid: String,
    subtitle_uuid: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    completed: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DictationSaveCueParam {
    dataset_uuid: String,
    cue: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AddCueToFavoritesParam {
    dataset_uuid: String,
    media_uuid: String,
    cue_uuid: String,
    /// Optional padding (ms) added before/after the cue bounds. Default 150.
    #[serde(default)]
    padding_ms: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DictationDeleteCueParam {
    dataset_uuid: String,
    cue_uuid: String,
}

// -- Subtitle versions --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SubtitleCreateVersionParam {
    dataset_uuid: String,
    subtitle_uuid: String,
    #[serde(default = "default_change_type")]
    change_type: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    created_by: String,
}

fn default_change_type() -> String {
    "mcp_edit".to_string()
}

// -- LLM --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct LlmModelParam {
    model: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct LlmChatParam {
    model: String,
    messages: Vec<JsonValue>,
    #[serde(default)]
    temperature: Option<f32>,
}

// -- TTS --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct TtsSynthesizeParam {
    text: String,
    voice: String,
    output_path: String,
    #[serde(default)]
    rate: String,
    #[serde(default)]
    volume: String,
    #[serde(default)]
    pitch: String,
}

// -- Books --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookTitleParam {
    title: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookUuidParam {
    book_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookRenameParam {
    uuid: String,
    title: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookChapterParam {
    book_uuid: String,
    chapter: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookDeleteChapterParam {
    book_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookSentenceParam {
    book_uuid: String,
    sentence: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookDeleteSentenceParam {
    book_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookWordParam {
    book_uuid: String,
    word: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookDeleteWordParam {
    book_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookListSentencesParam {
    book_uuid: String,
    chapter_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookListWordsParam {
    book_uuid: String,
    sentence_uuid: String,
}

// -- System --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WebServiceStartParam {
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default = "default_true")]
    stt: bool,
    #[serde(default = "default_true")]
    dataset: bool,
    #[serde(default = "default_true")]
    tts: bool,
}

fn default_port() -> u16 { 35711 }
fn default_true() -> bool { true }

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct OcrRecognizeParam {
    image_base64: String,
    #[serde(default)]
    lang: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct LogHistoryParam {
    #[serde(default)]
    limit: Option<usize>,
}

// -- Wiki --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiPathParam {
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiWriteFileParam {
    path: String,
    content: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiSearchParam {
    keyword: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiAddDirParam {
    name: String,
    path: String,
}

// -- Card datasets --

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardDatasetCreateParam {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    location: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardDatasetUpdateParam {
    uuid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    sync_url: String,
    #[serde(default)]
    visibility: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardDatasetSubscriberParam {
    uuid: String,
    email: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardListParam {
    dataset_uuid: String,
    #[serde(default)]
    keyword: String,
    #[serde(default)]
    filter: String,
    #[serde(default)]
    tag_uuid: String,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardGetParam {
    dataset_uuid: String,
    card_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardSaveParam {
    dataset_uuid: String,
    card: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardForkParam {
    source_dataset_uuid: String,
    card_uuid: String,
    target_dataset_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardTestSubmitParam {
    dataset_uuid: String,
    card_uuid: String,
    quality: i32,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardTagSaveParam {
    dataset_uuid: String,
    tag: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardSetTagsParam {
    dataset_uuid: String,
    card_uuid: String,
    tag_uuids: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardSearchParam {
    query: String,
    #[serde(default)]
    mode: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardFtsRebuildParam {
    #[serde(default)]
    location: String,
}

// ---------------------------------------------------------------------------
// Tool definitions — #[tool_router(server_handler)] generates ServerHandler impl
// ---------------------------------------------------------------------------

#[tool_router(server_handler)]
impl DatasetMcpServer {
    #[tool(name = "dataset_list", description = "List all datasets with their UUID, name, description, status, and path")]
    async fn dataset_list(&self) -> String {
        log::info!("[MCP] dataset_list called");
        let settings = self.app.state::<SettingsState>();
        let datasets = dataset::list_datasets(&settings);
        let items: Vec<serde_json::Value> = datasets
            .iter()
            .map(|d| {
                serde_json::json!({
                    "uuid": d.info.uuid,
                    "name": d.info.name,
                    "description": d.info.description,
                    "status": d.status,
                    "media_count": d.media_count,
                    "path": d.path,
                })
            })
            .collect();
        serde_json::to_string_pretty(&items).unwrap_or_default()
    }

    #[tool(name = "dataset_get", description = "Get detailed info about a dataset: info.json contents and which artifacts exist")]
    async fn dataset_get(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_get: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let info_text =
            std::fs::read_to_string(dir.join("info.json")).map_err(|e| e.to_string())?;
        let has = |p: &str| dir.join(p).exists();
        let info: serde_json::Value = serde_json::from_str(&info_text).unwrap_or(serde_json::json!({}));
        Ok(serde_json::json!({
            "info": info,
            "artifacts": {
                "media": has("media"),
                "subtitle": has("subtitle"),
                "waveform": has("waveform"),
                "database": has("data.sqlite3"),
                "book_txt": has("book.txt"),
                "transcript": has("transcript")
            }
        }).to_string())
    }

    #[tool(name = "dataset_read_file", description = "Read a file from a dataset directory (e.g. info.json, a VTT subtitle, book.txt)")]
    async fn dataset_read_file(
        &self,
        Parameters(param): Parameters<ReadFileParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let target = dir.join(&param.path);
        let base = dir.canonicalize().map_err(|e| e.to_string())?;
        let target = target.canonicalize().map_err(|_| "File not found".to_string())?;
        if !target.starts_with(&base) {
            return Err("Access denied: path traversal detected".to_string());
        }
        let content = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
        if content.len() > 50_000 {
            Ok(format!(
                "{}... (truncated, {} bytes total)",
                &content[..50_000],
                content.len()
            ))
        } else {
            Ok(content)
        }
    }

    #[tool(name = "dataset_list_files", description = "List all files in a dataset directory recursively with relative paths and sizes")]
    async fn dataset_list_files(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let mut files: Vec<serde_json::Value> = Vec::new();
        collect_files_recursive(&dir, &dir, &mut files);
        Ok(serde_json::to_string_pretty(&files).unwrap_or_default())
    }

    #[tool(name = "dataset_list_subtitles", description = "List all subtitles in a dataset with their UUID, media UUID, name, cue count, and timestamps. Optionally filter by media_uuid.")]
    async fn dataset_list_subtitles(
        &self,
        Parameters(param): Parameters<ListSubtitlesParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] dataset_list_subtitles: uuid={}, media_uuid={}", param.uuid, param.media_uuid);
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let db_path = dir.join("data.sqlite3");
        if !db_path.exists() {
            return Err("Database not found. Build the database first.".to_string());
        }
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
        let (sql, has_media_filter) = if param.media_uuid.is_empty() {
            ("SELECT s.uuid, s.media_uuid, s.name, s.created_at, s.updated_at, \
                    (SELECT COUNT(*) FROM listen_subtitle_cue c WHERE c.subtitle_uuid = s.uuid) as cue_count \
             FROM listen_subtitle s ORDER BY s.media_uuid, s.created_at".to_string(), false)
        } else {
            ("SELECT s.uuid, s.media_uuid, s.name, s.created_at, s.updated_at, \
                    (SELECT COUNT(*) FROM listen_subtitle_cue c WHERE c.subtitle_uuid = s.uuid) as cue_count \
             FROM listen_subtitle s WHERE s.media_uuid = ?1 ORDER BY s.created_at".to_string(), true)
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = if has_media_filter {
            stmt.query_map(rusqlite::params![param.media_uuid], |row| {
                Ok(serde_json::json!({
                    "uuid": row.get::<_, String>(0)?,
                    "media_uuid": row.get::<_, String>(1)?,
                    "name": row.get::<_, String>(2)?,
                    "created_at": row.get::<_, String>(3)?,
                    "updated_at": row.get::<_, String>(4)?,
                    "cue_count": row.get::<_, i64>(5)?,
                }))
            }).map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>()
        } else {
            stmt.query_map([], |row| {
                Ok(serde_json::json!({
                    "uuid": row.get::<_, String>(0)?,
                    "media_uuid": row.get::<_, String>(1)?,
                    "name": row.get::<_, String>(2)?,
                    "created_at": row.get::<_, String>(3)?,
                    "updated_at": row.get::<_, String>(4)?,
                    "cue_count": row.get::<_, i64>(5)?,
                }))
            }).map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>()
        };
        Ok(serde_json::json!({"subtitles": rows, "count": rows.len()}).to_string())
    }

    #[tool(name = "dataset_list_cues", description = "List all cues for a subtitle, ordered by order_num. Returns UUID, timestamps (start_ms/end_ms), content text, and order number for each cue.")]
    async fn dataset_list_cues(
        &self,
        Parameters(param): Parameters<ListCuesParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] dataset_list_cues: uuid={}, subtitle={}", param.uuid, param.subtitle_uuid);
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let db_path = dir.join("data.sqlite3");
        if !db_path.exists() {
            return Err("Database not found. Build the database first.".to_string());
        }
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT uuid, order_num, start_ms, end_ms, content \
                 FROM listen_subtitle_cue WHERE subtitle_uuid = ?1 ORDER BY order_num",
            )
            .map_err(|e| e.to_string())?;
        let cues: Vec<serde_json::Value> = stmt
            .query_map(rusqlite::params![param.subtitle_uuid], |row| {
                Ok(serde_json::json!({
                    "uuid": row.get::<_, String>(0)?,
                    "order_num": row.get::<_, i64>(1)?,
                    "start_ms": row.get::<_, i64>(2)?,
                    "end_ms": row.get::<_, i64>(3)?,
                    "content": row.get::<_, String>(4)?,
                }))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(serde_json::json!({"cues": cues, "count": cues.len()}).to_string())
    }

    #[tool(name = "dataset_get_summary", description = "Get a summary of a dataset: media count, subtitle count, total cue count, and per-media breakdown.")]
    async fn dataset_get_summary(
        &self,
        Parameters(param): Parameters<UuidParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] dataset_get_summary: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let db_path = dir.join("data.sqlite3");
        if !db_path.exists() {
            return Err("Database not found. Build the database first.".to_string());
        }
        let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
        let media_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM listen_media", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let subtitle_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM listen_subtitle", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let cue_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM listen_subtitle_cue", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let per_media: Vec<serde_json::Value> = {
            let mut stmt = conn
                .prepare(
                    "SELECT m.uuid, m.source, m.duration_ms, \
                            (SELECT COUNT(*) FROM listen_subtitle s WHERE s.media_uuid = m.uuid) as sub_count, \
                            (SELECT COUNT(*) FROM listen_subtitle_cue c \
                             JOIN listen_subtitle s ON c.subtitle_uuid = s.uuid \
                             WHERE s.media_uuid = m.uuid) as cue_count \
                     FROM listen_media m ORDER BY m.source",
                )
                .map_err(|e| e.to_string())?;
            let mut items = Vec::new();
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            }).map_err(|e| e.to_string())?;
            for row in rows.filter_map(|r| r.ok()) {
                items.push(serde_json::json!({
                    "media_uuid": row.0,
                    "source": row.1,
                    "duration_ms": row.2,
                    "subtitle_count": row.3,
                    "cue_count": row.4,
                }));
            }
            items
        };
        Ok(serde_json::json!({
            "media_count": media_count,
            "subtitle_count": subtitle_count,
            "cue_count": cue_count,
            "per_media": per_media,
        }).to_string())
    }

    #[tool(name = "dataset_delete_subtitles", description = "Delete all subtitles (VTT files + database entries) for a dataset")]
    async fn dataset_delete_subtitles(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::warn!("[MCP] dataset_delete_subtitles: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let subtitle_dir = dir.join("subtitle");
        if subtitle_dir.exists() {
            std::fs::remove_dir_all(&subtitle_dir).map_err(|e| e.to_string())?;
        }
        let db_path = dir.join("data.sqlite3");
        if db_path.exists() {
            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                let _ = conn.execute("DELETE FROM listen_subtitle_cue", []);
                let _ = conn.execute("DELETE FROM listen_subtitle", []);
            }
        }
        Ok(serde_json::json!({"status": "ok", "message": "Deleted all subtitles (VTT files + database entries)"}).to_string())
    }

    #[tool(name = "dataset_delete_waveforms", description = "Delete all waveform JSON files for a dataset")]
    async fn dataset_delete_waveforms(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let wf_dir = dir.join("waveform");
        if wf_dir.exists() {
            std::fs::remove_dir_all(&wf_dir).map_err(|e| e.to_string())?;
        }
        Ok(serde_json::json!({"status": "ok", "message": "Deleted all waveform files"}).to_string())
    }

    #[tool(name = "dataset_delete_database", description = "Delete the SQLite database for a dataset")]
    async fn dataset_delete_database(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::warn!("[MCP] dataset_delete_database: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let dir = dataset::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let db_path = dir.join("data.sqlite3");
        if db_path.exists() {
            std::fs::remove_file(&db_path).map_err(|e| e.to_string())?;
        }
        Ok(serde_json::json!({"status": "ok", "message": "Deleted the database"}).to_string())
    }

    #[tool(name = "dataset_write_subtitles_to_db", description = "Re-import VTT subtitles into the existing database by walking the subtitle/ directory")]
    async fn dataset_write_subtitles_to_db(
        &self,
        Parameters(param): Parameters<UuidParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result = dataset::dataset_write_subtitles_to_db(self.app.clone(), settings, param.uuid)
            .await?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    #[tool(name = "dataset_generate_subtitles", description = "Generate subtitles for all media files using STT model. Requires a model to be downloaded first.")]
    async fn dataset_generate_subtitles(
        &self,
        Parameters(param): Parameters<UuidParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let model_state = self.app.state::<ModelState>();
        dataset::dataset_generate_subtitles(self.app.clone(), settings, model_state, param.uuid)
            .await?;
        Ok(serde_json::json!({"status": "ok", "message": "Subtitles generated for all media files"}).to_string())
    }

    #[tool(name = "dataset_generate_subtitle_single", description = "Generate subtitle for a single media file using STT model. Use dataset_list_media first to get media UUIDs. Returns the source path on success.")]
    async fn dataset_generate_subtitle_single(
        &self,
        Parameters(param): Parameters<WaveformSingleParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let model_state = self.app.state::<ModelState>();
        let source = dataset::dataset_generate_subtitle_single(self.app.clone(), settings, model_state, param.uuid, param.media_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "source": source}).to_string())
    }

    #[tool(name = "dataset_generate_waveforms", description = "Generate waveform JSON files for all media files that don't have one yet. Uses Symphonia for peak detection.")]
    async fn dataset_generate_waveforms(
        &self,
        Parameters(param): Parameters<UuidParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let count = dataset::dataset_generate_waveform(self.app.clone(), settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "files_generated": count}).to_string())
    }

    #[tool(name = "dataset_generate_waveform_single", description = "Generate waveform for a single media file. Use dataset_list_media first to get media UUIDs, then call this for each media. Returns the source path on success.")]
    async fn dataset_generate_waveform_single(
        &self,
        Parameters(param): Parameters<WaveformSingleParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let source = dataset::dataset_generate_waveform_single(settings, param.uuid, param.media_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "source": source}).to_string())
    }

    #[tool(name = "dataset_list_media", description = "List all media files in a dataset. Returns UUID, source path, and duration for each media.")]
    async fn dataset_list_media(
        &self,
        Parameters(param): Parameters<UuidParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let media = dictation::listen_list_media(settings, param.uuid).await?;
        let items: Vec<serde_json::Value> = media.iter().map(|m| {
            serde_json::json!({
                "uuid": m.uuid,
                "source": m.source,
                "duration_ms": m.duration_ms
            })
        }).collect();
        Ok(serde_json::json!({"media": items, "count": items.len()}).to_string())
    }

    #[tool(name = "dataset_adjust_cue_time", description = "Adjust cue timestamps based on silence detection in the audio. Use mode='new' for first adjustment or 'in_place' to overwrite original cues. Already-adjusted subtitles are skipped by default; set force=true to re-process them. Strategy options: 'noise_floor' (default, good for consistent noise), 'dual_bound' (prevents over-detection on clean audio), 'peak_relative' (simple peak-based), 'otsu' (automatic optimal threshold).")]
    async fn dataset_adjust_cue_time(
        &self,
        Parameters(param): Parameters<AdjustCueTimeParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result: String = adjust::dataset_adjust_cue_time(settings, param.uuid, param.mode, Some(param.force), param.strategy)?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    #[tool(name = "dataset_adjust_cue_time_single", description = "Adjust cue timestamps for a SINGLE media file based on silence detection. Use mode='new' for first adjustment or 'in_place' to overwrite original cues. Already-adjusted subtitles are skipped by default; set force=true to re-process them. Strategy options: 'noise_floor' (default), 'dual_bound', 'peak_relative', 'otsu' (automatic optimal threshold).")]
    async fn dataset_adjust_cue_time_single(
        &self,
        Parameters(param): Parameters<AdjustCueTimeSingleParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result: String = adjust::dataset_adjust_cue_time_single(settings, param.uuid, param.media_uuid, param.mode, Some(param.force), param.strategy)?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    #[tool(name = "dataset_sync_cue_times", description = "Sync cue timestamps from fresh VTT subtitles to existing DB cues. For each media, matches fresh cues to DB cues by text similarity (Ratcliff-Obershelp) and updates timestamps. Cue text is NOT changed. Use after generating fresh subtitles. Default similarity threshold is 70.")]
    async fn dataset_sync_cue_times(
        &self,
        Parameters(param): Parameters<SyncCueTimesParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result = adjust::dataset_sync_cue_times(settings, param.uuid, param.threshold)
            .await?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    #[tool(name = "dataset_sync_cue_times_word_level", description = "Sync cue timestamps using word-level STT transcription (Parakeet only). Transcribes media with word-level timestamps, then DP-aligns cue words against STT words to find precise boundaries. More accurate than dataset_sync_cue_times. Requires Parakeet model loaded. Pass media_uuid to process a single file, or omit to process all media.")]
    async fn dataset_sync_cue_times_word_level(
        &self,
        Parameters(param): Parameters<WordLevelSyncParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let model_state = self.app.state::<ModelState>();
        let result = adjust::dataset_sync_cue_times_word_level(self.app.clone(), settings, model_state, param.uuid, param.media_uuid)
            .await?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    // -------------------------------------------------------------------------
    // Version management tools
    // -------------------------------------------------------------------------

    #[tool(name = "subtitle_get_versions", description = "Get the version history for a subtitle, showing all changes made over time.")]
    async fn subtitle_get_versions(
        &self,
        Parameters(param): Parameters<SubtitleVersionParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let versions = dictation::subtitle_get_versions(settings, String::new(), param.subtitle_uuid)
            .await
            .map_err(|e| e)?;
        Ok(serde_json::to_string_pretty(&versions).unwrap_or_default())
    }

    #[tool(name = "subtitle_get_cues_at_version", description = "Get cues as they existed at a specific version. Useful for comparing changes or rolling back.")]
    async fn subtitle_get_cues_at_version(
        &self,
        Parameters(param): Parameters<SubtitleVersionAtParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let cues = dictation::subtitle_get_cues_at_version(settings, String::new(), param.subtitle_uuid, param.version)
            .await
            .map_err(|e| e)?;
        Ok(serde_json::to_string_pretty(&cues).unwrap_or_default())
    }

    #[tool(name = "subtitle_rollback_to_version", description = "Rollback a subtitle to a previous version. Creates a new version with restored cues without deleting history.")]
    async fn subtitle_rollback_to_version(
        &self,
        Parameters(param): Parameters<SubtitleRollbackParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let new_version = dictation::subtitle_rollback_to_version(settings, String::new(), param.subtitle_uuid, param.target_version)
            .await
            .map_err(|e| e)?;
        Ok(serde_json::json!({
            "status": "ok",
            "rolled_back_to": param.target_version,
            "new_version": new_version
        }).to_string())
    }

    // -------------------------------------------------------------------------
    // Model management tools
    // -------------------------------------------------------------------------

    #[tool(name = "model_status", description = "Get STT model status: downloaded models, loaded model, and an actionable 'hint' field telling you exactly what to do next. If hint says 'Call model_load', do NOT call model_download — the model is already downloaded. Always check model_status before model_download.")]
    async fn model_status(&self) -> String {
        log::info!("[MCP] model_status");
        let state = self.app.state::<ModelState>();
        match crate::model::model_get_status(state).await {
            Ok(resp) => serde_json::to_string_pretty(&resp).unwrap_or_default(),
            Err(e) => serde_json::json!({"error": e}).to_string(),
        }
    }

    #[tool(name = "model_download", description = "Download an STT model by version ID. Use model_status first to see available versions. Download may take a while.")]
    async fn model_download(&self, Parameters(param): Parameters<ModelVersionParam>) -> Result<String, String> {
        log::info!("[MCP] model_download: version={}", param.version);
        let app = self.app.clone();
        let state = self.app.state::<ModelState>();
        let settings = self.app.state::<SettingsState>();
        let index = self.app.state::<crate::model_index::ModelIndexState>();
        crate::model::model_download_inner(app.clone(), &state, &settings, &index, param.version).await?;
        let _ = app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "message": "Model downloaded successfully"}).to_string())
    }

    #[tool(name = "model_load", description = "Load a downloaded STT model for transcription. If version is empty, auto-selects the best downloaded model (prefers parakeet-v3). Use model_status to see available versions.")]
    async fn model_load(&self, Parameters(param): Parameters<ModelLoadParam>) -> Result<String, String> {
        log::info!("[MCP] model_load: version={}", param.version);
        let state = self.app.state::<ModelState>();
        let version = if param.version.is_empty() {
            // Auto-select: prefer parakeet-v3 if downloaded, otherwise pick first downloaded model
            let statuses = state.download_status.lock().unwrap();
            let downloaded: Vec<&String> = statuses
                .iter()
                .filter(|(_, s)| **s == crate::model::ModelStatus::Downloaded)
                .map(|(v, _)| v)
                .collect();
            if downloaded.is_empty() {
                return Err("No models are downloaded. Use model_download first.".into());
            }
            if downloaded.contains(&&"parakeet-v3".to_string()) {
                "parakeet-v3".to_string()
            } else {
                downloaded[0].clone()
            }
        } else {
            param.version
        };
        crate::model::load_model_core(&state, &version)?;
        let _ = self.app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "loaded_version": version}).to_string())
    }

    #[tool(name = "model_unload", description = "Unload the currently active STT model, freeing memory.")]
    async fn model_unload(&self) -> Result<String, String> {
        log::info!("[MCP] model_unload");
        let state = self.app.state::<ModelState>();
        crate::model::unload_model_core(&state)?;
        let _ = self.app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "message": "Model unloaded"}).to_string())
    }

    #[tool(name = "model_delete", description = "Delete a downloaded STT model to free disk space. Cannot delete a model that is currently loaded — unload it first.")]
    async fn model_delete(&self, Parameters(param): Parameters<ModelVersionParam>) -> Result<String, String> {
        log::info!("[MCP] model_delete: version={}", param.version);
        let state = self.app.state::<ModelState>();
        let index = self.app.state::<crate::model_index::ModelIndexState>();
        crate::model::delete_model_core(&state, &index, &param.version)?;
        let _ = self.app.emit("model-status-changed", ());
        Ok(serde_json::json!({"status": "ok", "deleted": param.version}).to_string())
    }

    #[tool(name = "model_transcribe", description = "Transcribe base64-encoded 16kHz mono WAV audio using the loaded STT model. Returns the transcript text. Requires a model to be loaded first (use model_load).")]
    async fn model_transcribe(&self, Parameters(param): Parameters<ReadFileParam>) -> Result<String, String> {
        // Reuse ReadFileParam but we only need the path field as wav_base64
        // Actually let's use a dedicated approach: treat 'path' as wav_base64
        log::info!("[MCP] model_transcribe: base64_len={}", param.path.len());
        let state = self.app.state::<ModelState>();
        let result = crate::model::model_transcribe(state, param.path).await?;
        Ok(serde_json::json!({"status": "ok", "transcript": result}).to_string())
    }

    // -------------------------------------------------------------------------
    // Settings & Auth tools
    // -------------------------------------------------------------------------

    #[tool(name = "settings_get", description = "Get current app settings: model directory, datasets directory, books directory, recordings directory, and other configuration.")]
    async fn settings_get(&self) -> String {
        log::info!("[MCP] settings_get");
        let state = self.app.state::<SettingsState>();
        let settings = state.settings.lock().unwrap().clone();
        serde_json::to_string_pretty(&settings).unwrap_or_default()
    }

    #[tool(name = "settings_set", description = "Update app settings. Pass a JSON object with the settings fields to update. Use settings_get first to see current values. Fields: model_dir, recordings_dir, datasets_dir, books_dir, selected_model, model_unload_timeout, onboarding_completed.")]
    async fn settings_set(&self, Parameters(param): Parameters<SettingsSetParam>) -> Result<String, String> {
        log::info!("[MCP] settings_set");
        let state = self.app.state::<SettingsState>();
        let new_settings: crate::settings::AppSettings = serde_json::from_value(param.settings.into())
            .map_err(|e| format!("Invalid settings JSON: {}", e))?;
        let ws_dir = state.workspace_dir.lock().unwrap().clone();
        crate::settings::SettingsState::save(&new_settings, ws_dir.as_ref())?;
        {
            let mut s = state.settings.lock().unwrap();
            *s = new_settings;
        }
        let _ = self.app.emit("settings-changed", ());
        Ok(serde_json::json!({"status": "ok", "message": "Settings updated"}).to_string())
    }

    #[tool(name = "auth_status", description = "Get the currently logged-in user. Returns null if not logged in. Use auth_login to log in.")]
    async fn auth_status(&self) -> String {
        log::info!("[MCP] auth_status");
        let state = self.app.state::<SettingsState>();
        match auth::auth_get_user(state).await {
            Ok(Some(user)) => serde_json::json!({"logged_in": true, "user": user}).to_string(),
            Ok(None) => serde_json::json!({"logged_in": false}).to_string(),
            Err(e) => serde_json::json!({"logged_in": false, "error": e}).to_string(),
        }
    }

    // -------------------------------------------------------------------------
    // Dataset management tools
    // -------------------------------------------------------------------------

    #[tool(name = "dataset_list_dirs", description = "List all dataset directories (default datasets directory + linked directories) for a specific dataset type. Returns entries with name, path, and is_linked flag.")]
    async fn dataset_list_dirs(&self, Parameters(param): Parameters<DatasetListDirsParam>) -> String {
        log::info!("[MCP] dataset_list_dirs: dataset_type={}", param.dataset_type);
        let settings = self.app.state::<SettingsState>();
        match dataset::dataset_list_dirs(settings, param.dataset_type).await {
            Ok(entries) => serde_json::json!({"directories": entries}).to_string(),
            Err(e) => serde_json::json!({"error": e}).to_string(),
        }
    }

    #[tool(name = "dataset_add_dir", description = "Add a linked directory for datasets. The path must exist and be a directory. Requires a name for display.")]
    async fn dataset_add_dir(&self, Parameters(param): Parameters<DatasetAddDirParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_add_dir: name={}, path={}", param.name, param.path);
        let settings = self.app.state::<SettingsState>();
        dataset::dataset_add_dir(settings, param.name.clone(), param.path.clone()).await?;
        Ok(serde_json::json!({"status": "ok", "message": format!("Linked directory '{}' -> {}", param.name, param.path)}).to_string())
    }

    #[tool(name = "dataset_remove_dir", description = "Remove a linked directory from datasets. Datasets in that directory are no longer visible.")]
    async fn dataset_remove_dir(&self, Parameters(param): Parameters<PathParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_remove_dir: path={}", param.path);
        let settings = self.app.state::<SettingsState>();
        dataset::dataset_remove_dir(settings, param.path.clone()).await?;
        Ok(serde_json::json!({"status": "ok", "message": format!("Unlinked directory: {}", param.path)}).to_string())
    }

    #[tool(name = "dataset_import", description = "Import an existing dataset directory. The directory must contain an info.json file. Returns the dataset summary with UUID.")]
    async fn dataset_import(&self, Parameters(param): Parameters<DatasetImportParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_import: source={}", param.source_dir);
        let settings = self.app.state::<SettingsState>();
        let summary = dataset::dataset_import(settings, param.source_dir).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "dataset_create", description = "Create a new empty dataset with a name and optional description. Returns the dataset summary with UUID. Use dataset_import_media afterwards to add audio files.")]
    async fn dataset_create(&self, Parameters(param): Parameters<DatasetCreateParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_create: name={}", param.name);
        let settings = self.app.state::<SettingsState>();
        let desc = if param.description.is_empty() { None } else { Some(param.description) };
        let loc = if param.location.is_empty() { None } else { Some(param.location) };
        let summary = dataset::dataset_create(settings, param.name, desc, loc).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "dataset_update", description = "Update a dataset's name and/or description.")]
    async fn dataset_update(&self, Parameters(param): Parameters<DatasetUpdateParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_update: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let name = if param.name.is_empty() { None } else { Some(param.name) };
        let desc = if param.description.is_empty() { None } else { Some(param.description) };
        dataset::dataset_update(settings, param.uuid, name, desc).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset updated"}).to_string())
    }

    #[tool(name = "dataset_delete", description = "Delete a dataset and all its files. This is irreversible — the dataset directory is removed from disk.")]
    async fn dataset_delete(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::warn!("[MCP] dataset_delete: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        dataset::dataset_delete(settings, param.uuid).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::json!({"status": "ok", "message": "Dataset deleted"}).to_string())
    }

    #[tool(name = "dataset_import_media", description = "Import media files from a directory into a dataset. Set link=true to create symlinks instead of copying files. Returns the number of files imported.")]
    async fn dataset_import_media(&self, Parameters(param): Parameters<DatasetImportMediaParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_import_media: uuid={}, source={}, link={}", param.uuid, param.source_dir, param.link);
        let settings = self.app.state::<SettingsState>();
        let count = dataset::dataset_import_media(settings, param.uuid, param.source_dir, param.link).await?;
        Ok(serde_json::json!({"status": "ok", "files_imported": count}).to_string())
    }

    // -------------------------------------------------------------------------
    // Align & Tools
    // -------------------------------------------------------------------------

    #[tool(name = "dataset_align_cues", description = "Align cue text to a book.txt reference using multi-pass similarity matching. Requires book.txt in the dataset directory. Use after dataset_generate_database.")]
    async fn dataset_align_cues(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_align_cues: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        align::dataset_align_cues(self.app.clone(), settings, param.uuid).await
    }

    #[tool(name = "dataset_align_cues_transcript", description = "Align cue text using per-subtitle transcript files. Requires transcript/ directory. Use after dataset_generate_database.")]
    async fn dataset_align_cues_transcript(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_align_cues_transcript: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        align::dataset_align_cues_transcript(self.app.clone(), settings, param.uuid).await
    }

    // -------------------------------------------------------------------------
    // Dictation tools
    // -------------------------------------------------------------------------

    #[tool(name = "dictation_save_progress", description = "Save or update dictation practice progress for a media/subtitle pair. Status values: 'in_progress', 'completed'.")]
    async fn dictation_save_progress(&self, Parameters(param): Parameters<DictationSaveProgressParam>) -> Result<String, String> {
        log::info!("[MCP] dictation_save_progress: media={}", param.media_uuid);
        let settings = self.app.state::<SettingsState>();
        let dictation = dictation::ListenDictation {
            media_uuid: param.media_uuid,
            subtitle_uuid: param.subtitle_uuid,
            status: param.status,
            completed: param.completed,
        };
        dictation::listen_save_dictation(settings, String::new(), dictation).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Progress saved"}).to_string())
    }

    #[tool(name = "dictation_save_cue", description = "Create or update a cue in a dataset. Pass the cue as a JSON object with fields: uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, confidence.")]
    async fn dictation_save_cue(&self, Parameters(param): Parameters<DictationSaveCueParam>) -> Result<String, String> {
        log::info!("[MCP] dictation_save_cue: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let cue: dictation::ListenCue = serde_json::from_value(param.cue.into())
            .map_err(|e| format!("Invalid cue JSON: {}", e))?;
        dictation::listen_save_cue(settings, param.dataset_uuid, cue).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Cue saved"}).to_string())
    }

    #[tool(name = "dataset_add_cue_to_favorites", description = "Cut a cue's audio into a WAV clip and add it to the Favorites dataset (created on demand). Optionally pass padding_ms (default 150) to extend the clip beyond the cue bounds. Returns JSON with the new favorites dataset/media/subtitle/cue UUIDs and clip duration.")]
    async fn dataset_add_cue_to_favorites(&self, Parameters(param): Parameters<AddCueToFavoritesParam>) -> Result<String, String> {
        log::info!(
            "[MCP] dataset_add_cue_to_favorites: dataset={}, media={}, cue={}",
            param.dataset_uuid,
            param.media_uuid,
            param.cue_uuid
        );
        let settings = self.app.state::<SettingsState>();
        let result = dictation::dictation_add_cue_to_favorites(
            self.app.clone(),
            settings,
            param.dataset_uuid,
            param.media_uuid,
            param.cue_uuid,
            param.padding_ms,
        )
        .await?;
        Ok(result.to_string())
    }

    #[tool(name = "dataset_list_favorite_cues", description = "List the cue UUIDs currently in the Favorites dataset. Favorite clips reuse their source cue's UUID as the cue key, so this set identifies which cues (in any dataset) are already favorited. Returns JSON {status, count, cue_uuids}.")]
    async fn dataset_list_favorite_cues(&self) -> Result<String, String> {
        log::info!("[MCP] dataset_list_favorite_cues");
        let settings = self.app.state::<SettingsState>();
        let uuids = dictation::dictation_list_favorite_cues(settings).await?;
        Ok(serde_json::json!({ "status": "ok", "count": uuids.len(), "cue_uuids": uuids }).to_string())
    }

    #[tool(name = "dictation_delete_cue", description = "Delete a cue from a dataset by its UUID.")]
    async fn dictation_delete_cue(&self, Parameters(param): Parameters<DictationDeleteCueParam>) -> Result<String, String> {
        log::info!("[MCP] dictation_delete_cue: dataset={}, cue={}", param.dataset_uuid, param.cue_uuid);
        let settings = self.app.state::<SettingsState>();
        dictation::listen_delete_cue(settings, param.dataset_uuid, param.cue_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Cue deleted"}).to_string())
    }

    // -------------------------------------------------------------------------
    // Subtitle version tools
    // -------------------------------------------------------------------------

    #[tool(name = "subtitle_create_version", description = "Create a new version snapshot of current cues for a subtitle. Call this before making edits, so you can rollback later. Returns the version number.")]
    async fn subtitle_create_version(
        &self,
        Parameters(param): Parameters<SubtitleCreateVersionParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] subtitle_create_version: subtitle={}", param.subtitle_uuid);
        let settings = self.app.state::<SettingsState>();
        let version = dictation::subtitle_create_version(
            settings, param.dataset_uuid, param.subtitle_uuid,
            param.change_type, param.description, param.created_by,
        ).await?;
        Ok(serde_json::json!({"status": "ok", "version": version}).to_string())
    }

    // -------------------------------------------------------------------------
    // LLM tools
    // -------------------------------------------------------------------------

    #[tool(name = "llm_check_connection", description = "Check if Ollama is running and reachable. Returns true/false.")]
    async fn llm_check_connection(&self) -> String {
        log::info!("[MCP] llm_check_connection");
        let settings = self.app.state::<SettingsState>();
        match llm::llm_check_connection(settings).await {
            Ok(connected) => serde_json::json!({"connected": connected}).to_string(),
            Err(e) => serde_json::json!({"connected": false, "error": e}).to_string(),
        }
    }

    #[tool(name = "llm_list_models", description = "List LLM models installed in Ollama plus our recommended catalog. Use llm_pull_model to download a recommended model.")]
    async fn llm_list_models(&self) -> Result<String, String> {
        log::info!("[MCP] llm_list_models");
        let settings = self.app.state::<SettingsState>();
        let result = llm::llm_list_models(settings).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "llm_pull_model", description = "Pull/download a model into Ollama. Use llm_list_models to see recommended model names. May take a while.")]
    async fn llm_pull_model(&self, Parameters(param): Parameters<LlmModelParam>) -> Result<String, String> {
        log::info!("[MCP] llm_pull_model: model={}", param.model);
        let settings = self.app.state::<SettingsState>();
        llm::llm_pull_model(self.app.clone(), settings, param.model).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Model pulled successfully"}).to_string())
    }

    #[tool(name = "llm_delete_model", description = "Delete a model from Ollama to free disk space.")]
    async fn llm_delete_model(&self, Parameters(param): Parameters<LlmModelParam>) -> Result<String, String> {
        log::info!("[MCP] llm_delete_model: model={}", param.model);
        let settings = self.app.state::<SettingsState>();
        llm::llm_delete_model(settings, param.model.clone()).await?;
        Ok(serde_json::json!({"status": "ok", "deleted": param.model}).to_string())
    }

    #[tool(name = "llm_chat", description = "Send a chat completion request to Ollama. Pass model name, messages array [{role:'user',content:'...'}], and optional temperature. Returns the assistant response.")]
    async fn llm_chat(&self, Parameters(param): Parameters<LlmChatParam>) -> Result<String, String> {
        log::info!("[MCP] llm_chat: model={}, msgs={}", param.model, param.messages.len());
        let settings = self.app.state::<SettingsState>();
        let messages: Vec<llm::ChatMessage> = param.messages.into_iter().map(|m| {
            let m: serde_json::Value = m.into();
            llm::ChatMessage {
                role: m["role"].as_str().unwrap_or("user").to_string(),
                content: m["content"].as_str().unwrap_or("").to_string(),
            }
        }).collect();
        let result = llm::llm_chat(settings, param.model, messages, param.temperature).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    // -------------------------------------------------------------------------
    // TTS tools
    // -------------------------------------------------------------------------

    #[tool(name = "tts_list_voices", description = "List all available Edge TTS voices. Returns name, short_name, locale, and gender for each voice.")]
    async fn tts_list_voices(&self) -> Result<String, String> {
        log::info!("[MCP] tts_list_voices");
        let voices = edge_tts::edge_tts_list_voices().await?;
        Ok(serde_json::to_string_pretty(&voices).unwrap_or_default())
    }

    #[tool(name = "tts_synthesize", description = "Synthesize text to audio using Edge TTS and save to a file. Pass text, voice (e.g. 'en-US-AriaNeural'), output_path, and optional rate/volume/pitch adjustments.")]
    async fn tts_synthesize(&self, Parameters(param): Parameters<TtsSynthesizeParam>) -> Result<String, String> {
        log::info!("[MCP] tts_synthesize: voice={}, text_len={}", param.voice, param.text.len());
        let args = edge_tts::TtsSynthesizeArgs {
            text: param.text,
            voice: param.voice,
            rate: if param.rate.is_empty() { None } else { Some(param.rate) },
            volume: if param.volume.is_empty() { None } else { Some(param.volume) },
            pitch: if param.pitch.is_empty() { None } else { Some(param.pitch) },
            output_path: param.output_path,
        };
        let result = edge_tts::edge_tts_synthesize(args).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    // -------------------------------------------------------------------------
    // Book tools
    // -------------------------------------------------------------------------

    #[tool(name = "book_list", description = "List all books in the reading library. Returns UUID, title, path, and timestamps.")]
    async fn book_list(&self) -> Result<String, String> {
        log::info!("[MCP] book_list");
        let settings = self.app.state::<SettingsState>();
        let books = book::book_list(settings).await?;
        Ok(serde_json::to_string_pretty(&books).unwrap_or_default())
    }

    #[tool(name = "book_create", description = "Create a new book in the reading library. Returns the book metadata with UUID.")]
    async fn book_create(&self, Parameters(param): Parameters<BookTitleParam>) -> Result<String, String> {
        log::info!("[MCP] book_create: title={}", param.title);
        let settings = self.app.state::<SettingsState>();
        let result = book::book_create(settings, param.title).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "book_rename", description = "Rename a book by UUID.")]
    async fn book_rename(&self, Parameters(param): Parameters<BookRenameParam>) -> Result<String, String> {
        log::info!("[MCP] book_rename: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        book::book_rename(settings, param.uuid, param.title).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Book renamed"}).to_string())
    }

    #[tool(name = "book_delete", description = "Delete a book and all its contents (chapters, sentences, words, audio).")]
    async fn book_delete(&self, Parameters(param): Parameters<BookUuidParam>) -> Result<String, String> {
        log::warn!("[MCP] book_delete: uuid={}", param.book_uuid);
        let settings = self.app.state::<SettingsState>();
        book::book_delete(settings, param.book_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Book deleted"}).to_string())
    }

    #[tool(name = "book_list_chapters", description = "List all chapters of a book, ordered.")]
    async fn book_list_chapters(&self, Parameters(param): Parameters<BookUuidParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let chapters = book::book_list_chapters(settings, param.book_uuid).await?;
        Ok(serde_json::to_string_pretty(&chapters).unwrap_or_default())
    }

    #[tool(name = "book_save_chapter", description = "Create or update a chapter. Pass chapter as JSON with fields: uuid, book_uuid, parent_uuid, order_num, title, status.")]
    async fn book_save_chapter(&self, Parameters(param): Parameters<BookChapterParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let chapter: book::BookChapter = serde_json::from_value(param.chapter.into())
            .map_err(|e| format!("Invalid chapter JSON: {}", e))?;
        book::book_save_chapter(settings, param.book_uuid, chapter).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Chapter saved"}).to_string())
    }

    #[tool(name = "book_delete_chapter", description = "Delete a chapter and all its sentences and words.")]
    async fn book_delete_chapter(&self, Parameters(param): Parameters<BookDeleteChapterParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        book::book_delete_chapter(settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Chapter deleted"}).to_string())
    }

    #[tool(name = "book_list_sentences", description = "List all sentences for a chapter, ordered, with resolved audio URLs.")]
    async fn book_list_sentences(&self, Parameters(param): Parameters<BookListSentencesParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let sentences = book::book_list_sentences(settings, param.book_uuid, param.chapter_uuid).await?;
        Ok(serde_json::to_string_pretty(&sentences).unwrap_or_default())
    }

    #[tool(name = "book_save_sentence", description = "Create or update a sentence. Pass sentence as JSON with fields: uuid, chapter_uuid, order_num, content, sentence_type, audio_path, etc.")]
    async fn book_save_sentence(&self, Parameters(param): Parameters<BookSentenceParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let sentence: book::BookSentence = serde_json::from_value(param.sentence.into())
            .map_err(|e| format!("Invalid sentence JSON: {}", e))?;
        book::book_save_sentence(settings, param.book_uuid, sentence).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Sentence saved"}).to_string())
    }

    #[tool(name = "book_delete_sentence", description = "Delete a sentence, its words, and its audio file.")]
    async fn book_delete_sentence(&self, Parameters(param): Parameters<BookDeleteSentenceParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        book::book_delete_sentence(settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Sentence deleted"}).to_string())
    }

    #[tool(name = "book_list_words", description = "List vocabulary words saved for a sentence.")]
    async fn book_list_words(&self, Parameters(param): Parameters<BookListWordsParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let words = book::book_list_words(settings, param.book_uuid, param.sentence_uuid).await?;
        Ok(serde_json::to_string_pretty(&words).unwrap_or_default())
    }

    #[tool(name = "book_save_word", description = "Create or update a vocabulary word. Pass word as JSON with fields: uuid, sentence_uuid, word, word_type, note.")]
    async fn book_save_word(&self, Parameters(param): Parameters<BookWordParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let word: book::BookSentenceWord = serde_json::from_value(param.word.into())
            .map_err(|e| format!("Invalid word JSON: {}", e))?;
        book::book_save_word(settings, param.book_uuid, word).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Word saved"}).to_string())
    }

    #[tool(name = "book_delete_word", description = "Delete a vocabulary word.")]
    async fn book_delete_word(&self, Parameters(param): Parameters<BookDeleteWordParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        book::book_delete_word(settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Word deleted"}).to_string())
    }

    // -------------------------------------------------------------------------
    // Web service tools
    // -------------------------------------------------------------------------

    #[tool(name = "web_service_get_status", description = "Get the web service status: whether it's running, port, enabled features, and URLs.")]
    async fn web_service_get_status(&self) -> String {
        log::info!("[MCP] web_service_get_status");
        let state = self.app.state::<WebServiceState>();
        let status = state.build_status();
        serde_json::to_string_pretty(&status).unwrap_or_default()
    }

    #[tool(name = "web_service_start", description = "Start the web service (HTTP server) with the given config. Default port is 35711. Enables STT, dataset, and TTS endpoints by default.")]
    async fn web_service_start(&self, Parameters(param): Parameters<WebServiceStartParam>) -> Result<String, String> {
        log::info!("[MCP] web_service_start: port={}", param.port);
        let state = self.app.state::<WebServiceState>();
        let config = WebServiceConfig {
            port: param.port,
            stt: param.stt,
            dataset: param.dataset,
            tts: param.tts,
            api_token: String::new(),
        };
        let status = web_service::web_service_start(self.app.clone(), state, config).await?;
        Ok(serde_json::to_string_pretty(&status).unwrap_or_default())
    }

    #[tool(name = "web_service_stop", description = "Stop the web service.")]
    async fn web_service_stop(&self) -> Result<String, String> {
        log::info!("[MCP] web_service_stop");
        let state = self.app.state::<WebServiceState>();
        let status = web_service::web_service_stop(state).await?;
        Ok(serde_json::to_string_pretty(&status).unwrap_or_default())
    }

    // -------------------------------------------------------------------------
    // OCR & Capture tools
    // -------------------------------------------------------------------------

    #[tool(name = "ocr_recognize", description = "Recognize text in an image using Tesseract OCR. Pass image as base64 (with or without data URL prefix) and optional language code (default: 'eng'). Use ocr_list_languages to see available languages.")]
    async fn ocr_recognize(&self, Parameters(param): Parameters<OcrRecognizeParam>) -> Result<String, String> {
        log::info!("[MCP] ocr_recognize: lang={:?}", param.lang);
        let state = self.app.state::<OcrState>();
        let lang = if param.lang.is_empty() { None } else { Some(param.lang) };
        let text = ocr::ocr_recognize(state, param.image_base64, lang).await?;
        Ok(serde_json::json!({"status": "ok", "text": text}).to_string())
    }

    #[tool(name = "ocr_list_languages", description = "List available Tesseract OCR languages installed on the system. Returns language codes that can be used with ocr_recognize (e.g., 'eng', 'deu', 'eng+deu' for multi-language).")]
    async fn ocr_list_languages(&self) -> Result<String, String> {
        log::info!("[MCP] ocr_list_languages");
        let state = self.app.state::<OcrState>();
        let langs = ocr::ocr_list_languages(state).await?;
        Ok(serde_json::json!({"status": "ok", "languages": langs}).to_string())
    }

    #[tool(name = "capture_screenshot", description = "Minimize the app window and capture a screenshot using the native snipping tool (Windows) or full-screen capture (other platforms). Returns a data:image/png;base64 URL.")]
    async fn capture_screenshot(&self) -> Result<String, String> {
        log::info!("[MCP] capture_screenshot");
        let result = capture::capture_screenshot(self.app.clone()).await?;
        Ok(serde_json::json!({"status": "ok", "image_url": result}).to_string())
    }

    // -------------------------------------------------------------------------
    // Wiki tools
    // -------------------------------------------------------------------------

    #[tool(name = "wiki_list_dirs", description = "List top-level wiki directories (default wiki directory + linked directories). Returns entries with name, path, is_dir, is_linked, and modified timestamp.")]
    async fn wiki_list_dirs(&self) -> Result<String, String> {
        log::info!("[MCP] wiki_list_dirs");
        let state = self.app.state::<SettingsState>();
        let entries = crate::wiki::wiki_list_dirs(state.into()).await?;
        Ok(serde_json::to_string_pretty(&entries).unwrap_or_default())
    }

    #[tool(name = "wiki_list_dir", description = "List contents of a specific wiki directory. Returns entries with name, path, is_dir, is_linked, and modified timestamp.")]
    async fn wiki_list_dir(&self, Parameters(param): Parameters<WikiPathParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_list_dir: path={}", param.path);
        let entries = crate::wiki::wiki_list_dir(param.path).await?;
        Ok(serde_json::to_string_pretty(&entries).unwrap_or_default())
    }

    #[tool(name = "wiki_read_file", description = "Read the content of a wiki markdown file. Returns the full file content as a string.")]
    async fn wiki_read_file(&self, Parameters(param): Parameters<WikiPathParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_read_file: path={}", param.path);
        let content = crate::wiki::wiki_read_file(param.path).await?;
        Ok(serde_json::json!({"status": "ok", "content": content}).to_string())
    }

    #[tool(name = "wiki_write_file", description = "Write or create a wiki markdown file at the specified path. Creates parent directories if they don't exist. Use this to create new wiki pages or overwrite existing ones.")]
    async fn wiki_write_file(&self, Parameters(param): Parameters<WikiWriteFileParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_write_file: path={}", param.path);
        let state = self.app.state::<SettingsState>();
        crate::wiki::wiki_write_file(state.into(), param.path, param.content).await?;
        Ok(serde_json::json!({"status": "ok", "message": "File written successfully"}).to_string())
    }

    #[tool(name = "wiki_delete_file", description = "Delete a wiki markdown file. The file is moved to trash (not permanently deleted) for safety. Returns the trash location.")]
    async fn wiki_delete_file(&self, Parameters(param): Parameters<WikiPathParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_delete_file: path={}", param.path);
        let state = self.app.state::<SettingsState>();
        crate::wiki::wiki_delete_file(state.into(), param.path).await?;
        Ok(serde_json::json!({"status": "ok", "message": "File moved to trash"}).to_string())
    }

    #[tool(name = "wiki_search", description = "Search wiki markdown files using full-text search. Returns matching files with snippets, ranked by relevance. Use wiki_index first if search returns no results.")]
    async fn wiki_search(&self, Parameters(param): Parameters<WikiSearchParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_search: keyword={}", param.keyword);
        let state = self.app.state::<SettingsState>();
        let results = crate::wiki::wiki_search(state.into(), param.keyword).await?;
        Ok(serde_json::to_string_pretty(&results).unwrap_or_default())
    }

    #[tool(name = "wiki_index", description = "Index all markdown files in the wiki directory and linked directories for search. Returns the number of files indexed. Run this after adding new files to make them searchable.")]
    async fn wiki_index(&self) -> Result<String, String> {
        log::info!("[MCP] wiki_index");
        let state = self.app.state::<SettingsState>();
        let count = crate::wiki::wiki_index(state.into()).await?;
        Ok(serde_json::json!({"status": "ok", "indexed_count": count}).to_string())
    }

    #[tool(name = "wiki_add_dir", description = "Link an external directory to the wiki. The directory will appear in the wiki sidebar with the specified name. Files in linked directories are included in search and indexing.")]
    async fn wiki_add_dir(&self, Parameters(param): Parameters<WikiAddDirParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_add_dir: name={}, path={}", param.name, param.path);
        let state = self.app.state::<SettingsState>();
        crate::wiki::wiki_add_dir(state.into(), param.name, param.path).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Directory linked successfully"}).to_string())
    }

    #[tool(name = "wiki_remove_dir", description = "Unlink an external directory from the wiki. The directory itself is not deleted, only the link is removed.")]
    async fn wiki_remove_dir(&self, Parameters(param): Parameters<WikiPathParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_remove_dir: path={}", param.path);
        let state = self.app.state::<SettingsState>();
        crate::wiki::wiki_remove_dir(state.into(), param.path).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Directory unlinked successfully"}).to_string())
    }

    // -------------------------------------------------------------------------
    // Card dataset tools
    // -------------------------------------------------------------------------

    #[tool(name = "card_dataset_list", description = "List all card datasets from all locations (default + linked directories). Returns UUID, name, description, visibility, owner, card count, and location for each dataset.")]
    async fn card_dataset_list(&self) -> Result<String, String> {
        log::info!("[MCP] card_dataset_list");
        let settings = self.app.state::<SettingsState>();
        let datasets = cards::card_dataset_list(settings).await?;
        let items: Vec<serde_json::Value> = datasets.iter().map(|d| {
            serde_json::json!({
                "uuid": d.info.uuid,
                "name": d.info.name,
                "description": d.info.description,
                "visibility": d.info.visibility,
                "owner_id": d.info.owner_id,
                "card_count": d.card_count,
                "path": d.path,
                "location": d.location,
            })
        }).collect();
        Ok(serde_json::json!({"datasets": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_dataset_create", description = "Create a new card dataset with a name and optional description. Returns the dataset summary with UUID. Use card_save to add cards afterwards.")]
    async fn card_dataset_create(&self, Parameters(param): Parameters<CardDatasetCreateParam>) -> Result<String, String> {
        log::info!("[MCP] card_dataset_create: name={}", param.name);
        let settings = self.app.state::<SettingsState>();
        let desc = if param.description.is_empty() { None } else { Some(param.description) };
        let loc = if param.location.is_empty() { None } else { Some(param.location) };
        let summary = cards::card_dataset_create(settings, param.name, desc, loc).await?;
        Ok(serde_json::json!({"status": "ok", "dataset": {
            "uuid": summary.info.uuid,
            "name": summary.info.name,
            "card_count": summary.card_count,
            "path": summary.path,
        }}).to_string())
    }

    #[tool(name = "card_dataset_update", description = "Update a card dataset's metadata: name, description, sync_url, and/or visibility ('private', 'shared', 'public').")]
    async fn card_dataset_update(&self, Parameters(param): Parameters<CardDatasetUpdateParam>) -> Result<String, String> {
        log::info!("[MCP] card_dataset_update: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let name = if param.name.is_empty() { None } else { Some(param.name) };
        let desc = if param.description.is_empty() { None } else { Some(param.description) };
        let sync_url = if param.sync_url.is_empty() { None } else { Some(param.sync_url) };
        let visibility = if param.visibility.is_empty() { None } else { Some(param.visibility) };
        cards::card_dataset_update(settings, param.uuid, name, desc, sync_url, visibility).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset updated"}).to_string())
    }

    #[tool(name = "card_dataset_delete", description = "Delete a card dataset and all its files (database, info.json). This is irreversible.")]
    async fn card_dataset_delete(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::warn!("[MCP] card_dataset_delete: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        cards::card_dataset_delete(settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset deleted"}).to_string())
    }

    #[tool(name = "card_dataset_add_subscriber", description = "Add a subscriber (by email) to a card dataset. Only the dataset owner can do this.")]
    async fn card_dataset_add_subscriber(&self, Parameters(param): Parameters<CardDatasetSubscriberParam>) -> Result<String, String> {
        log::info!("[MCP] card_dataset_add_subscriber: uuid={}, email={}", param.uuid, param.email);
        let settings = self.app.state::<SettingsState>();
        cards::card_dataset_add_subscriber(settings, param.uuid, param.email).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Subscriber added"}).to_string())
    }

    #[tool(name = "card_dataset_remove_subscriber", description = "Remove a subscriber (by email) from a card dataset.")]
    async fn card_dataset_remove_subscriber(&self, Parameters(param): Parameters<CardDatasetSubscriberParam>) -> Result<String, String> {
        log::info!("[MCP] card_dataset_remove_subscriber: uuid={}, email={}", param.uuid, param.email);
        let settings = self.app.state::<SettingsState>();
        cards::card_dataset_remove_subscriber(settings, param.uuid, param.email).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Subscriber removed"}).to_string())
    }

    // -------------------------------------------------------------------------
    // Card CRUD tools
    // -------------------------------------------------------------------------

    #[tool(name = "card_list", description = "List cards in a dataset with optional keyword search, filter ('all', 'normal', 'easy', 'incomplete'), tag filter, and pagination. Returns cards and total count.")]
    async fn card_list(&self, Parameters(param): Parameters<CardListParam>) -> Result<String, String> {
        log::info!("[MCP] card_list: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let filter = if param.filter.is_empty() { None } else { Some(serde_json::from_value(serde_json::json!(param.filter)).unwrap_or(cards::CardFilter::All)) };
        let keyword = if param.keyword.is_empty() { None } else { Some(param.keyword) };
        let tag_uuid = if param.tag_uuid.is_empty() { None } else { Some(param.tag_uuid) };
        let (card_list, total) = cards::card_list(settings, param.dataset_uuid, filter, tag_uuid, keyword, param.limit, param.offset).await?;
        let items: Vec<serde_json::Value> = card_list.iter().map(|c| {
            serde_json::json!({
                "uuid": c.uuid,
                "question": c.question,
                "answer": c.answer,
                "note": c.note,
                "familiarity": c.familiarity,
                "created_at": c.created_at,
                "updated_at": c.updated_at,
            })
        }).collect();
        Ok(serde_json::json!({"cards": items, "total": total}).to_string())
    }

    #[tool(name = "card_get", description = "Get full details of a single card by dataset UUID and card UUID.")]
    async fn card_get(&self, Parameters(param): Parameters<CardGetParam>) -> Result<String, String> {
        log::info!("[MCP] card_get: dataset={}, card={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        let card = cards::card_get(settings, param.dataset_uuid, param.card_uuid).await?;
        Ok(serde_json::to_string_pretty(&card).unwrap_or_default())
    }

    #[tool(name = "card_save", description = "Create or update a card in a dataset. Pass card as JSON with fields: uuid (empty for new), question, suggestion, answer, note. Returns the saved card.")]
    async fn card_save(&self, Parameters(param): Parameters<CardSaveParam>) -> Result<String, String> {
        log::info!("[MCP] card_save: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let card: cards::Card = serde_json::from_value(param.card.into())
            .map_err(|e| format!("Invalid card JSON: {}", e))?;
        let result = cards::card_save(settings, param.dataset_uuid, card).await?;
        Ok(serde_json::json!({"status": "ok", "card": {
            "uuid": result.uuid,
            "question": result.question,
            "answer": result.answer,
            "familiarity": result.familiarity,
        }}).to_string())
    }

    #[tool(name = "card_delete", description = "Soft-delete a card from a dataset. The card is marked as deleted but not permanently removed (for sync purposes).")]
    async fn card_delete(&self, Parameters(param): Parameters<CardGetParam>) -> Result<String, String> {
        log::info!("[MCP] card_delete: dataset={}, card={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        cards::card_delete(settings, param.dataset_uuid, param.card_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Card deleted"}).to_string())
    }

    #[tool(name = "card_fork", description = "Fork a card from one dataset to another. Creates a copy with lineage tracking (source_card_uuid, source_dataset_uuid). Use for customizing shared cards.")]
    async fn card_fork(&self, Parameters(param): Parameters<CardForkParam>) -> Result<String, String> {
        log::info!("[MCP] card_fork: from={}, card={}, to={}", param.source_dataset_uuid, param.card_uuid, param.target_dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let result = cards::card_fork(settings, param.source_dataset_uuid, param.card_uuid, param.target_dataset_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "forked_card": {
            "uuid": result.uuid,
            "question": result.question,
            "answer": result.answer,
            "source_card_uuid": result.source_card_uuid,
            "source_dataset_uuid": result.source_dataset_uuid,
        }}).to_string())
    }

    // -------------------------------------------------------------------------
    // Card FTS search tools
    // -------------------------------------------------------------------------

    #[tool(name = "card_search", description = "Search cards across all datasets using FTS5 full-text search. Returns matching cards with dataset info. Supports FTS5 query syntax (e.g., 'word1 word2' for AND, 'word1 OR word2' for OR). Mode: 'question' searches only questions, 'fulltext' (default) searches all fields.")]
    async fn card_search(&self, Parameters(param): Parameters<CardSearchParam>) -> Result<String, String> {
        log::info!("[MCP] card_search: query='{}', mode='{}'", param.query, param.mode);
        let settings = self.app.state::<SettingsState>();
        let mode = if param.mode.is_empty() { None } else { Some(param.mode) };
        let results = cards::card_search(settings, param.query, mode).await?;
        let items: Vec<serde_json::Value> = results.iter().map(|r| {
            serde_json::json!({
                "dataset_uuid": r.dataset_uuid,
                "dataset_name": r.dataset_name,
                "card_uuid": r.card_uuid,
                "question": r.question,
                "answer": r.answer,
                "note": r.note,
                "location": r.location,
            })
        }).collect();
        Ok(serde_json::json!({"results": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_fts_rebuild", description = "Rebuild the FTS5 full-text search index for card datasets. Optionally specify a location path to rebuild only that location's index. Returns the number of cards indexed.")]
    async fn card_fts_rebuild(&self, Parameters(param): Parameters<CardFtsRebuildParam>) -> Result<String, String> {
        log::info!("[MCP] card_fts_rebuild: location='{}'", param.location);
        let settings = self.app.state::<SettingsState>();
        let location = if param.location.is_empty() { None } else { Some(param.location) };
        let count = cards::card_fts_rebuild(settings, location).await?;
        Ok(serde_json::json!({"status": "ok", "cards_indexed": count}).to_string())
    }

    // -------------------------------------------------------------------------
    // Card review (SM-2 spaced repetition) tools
    // -------------------------------------------------------------------------

    #[tool(name = "card_test_next", description = "Get the next card due for review using SM-2 spaced repetition. Returns the card and its review state, or null if no cards are due.")]
    async fn card_test_next(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_test_next: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        match cards::card_test_get(settings, param.uuid).await? {
            Some((card, review)) => Ok(serde_json::json!({
                "card": {
                    "uuid": card.uuid,
                    "question": card.question,
                    "answer": card.answer,
                    "note": card.note,
                    "familiarity": card.familiarity,
                },
                "review": review.map(|r| serde_json::json!({
                    "familiarity": r.familiarity,
                    "interval_days": r.interval_days,
                    "ease_factor": r.ease_factor,
                    "repetitions": r.repetitions,
                    "next_review_at": r.next_review_at,
                }))
            }).to_string()),
            None => Ok(serde_json::json!({"card": null, "message": "No cards due for review"}).to_string()),
        }
    }

    #[tool(name = "card_test_submit", description = "Submit a review result for a card. Quality rating: 0-5 (0=blackout, 2=hard, 3=okay, 4=good, 5=perfect). Returns updated review state with next review date.")]
    async fn card_test_submit(&self, Parameters(param): Parameters<CardTestSubmitParam>) -> Result<String, String> {
        log::info!("[MCP] card_test_submit: dataset={}, card={}, quality={}", param.dataset_uuid, param.card_uuid, param.quality);
        let settings = self.app.state::<SettingsState>();
        let review = cards::card_test_submit(settings, param.dataset_uuid, param.card_uuid, param.quality).await?;
        Ok(serde_json::json!({"status": "ok", "review": {
            "familiarity": review.familiarity,
            "interval_days": review.interval_days,
            "ease_factor": review.ease_factor,
            "repetitions": review.repetitions,
            "next_review_at": review.next_review_at,
        }}).to_string())
    }

    // -------------------------------------------------------------------------
    // Card tag tools
    // -------------------------------------------------------------------------

    #[tool(name = "card_tag_list", description = "List all tags in a card dataset. Returns UUID, name, and color for each tag.")]
    async fn card_tag_list(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_tag_list: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let tags = cards::card_tag_list(settings, param.uuid).await?;
        let items: Vec<serde_json::Value> = tags.iter().map(|t| {
            serde_json::json!({
                "uuid": t.uuid,
                "name": t.name,
                "color": t.color,
            })
        }).collect();
        Ok(serde_json::json!({"tags": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_tag_save", description = "Create or update a tag in a card dataset. Pass tag as JSON with fields: uuid (empty for new), name, color (optional hex string).")]
    async fn card_tag_save(&self, Parameters(param): Parameters<CardTagSaveParam>) -> Result<String, String> {
        log::info!("[MCP] card_tag_save: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let tag: cards::Tag = serde_json::from_value(param.tag.into())
            .map_err(|e| format!("Invalid tag JSON: {}", e))?;
        let result = cards::card_tag_save(settings, param.dataset_uuid, tag).await?;
        Ok(serde_json::json!({"status": "ok", "tag": {
            "uuid": result.uuid,
            "name": result.name,
            "color": result.color,
        }}).to_string())
    }

    #[tool(name = "card_tag_delete", description = "Soft-delete a tag from a card dataset.")]
    async fn card_tag_delete(&self, Parameters(param): Parameters<CardGetParam>) -> Result<String, String> {
        log::info!("[MCP] card_tag_delete: dataset={}, tag={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        cards::card_tag_delete(settings, param.dataset_uuid, param.card_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Tag deleted"}).to_string())
    }

    #[tool(name = "card_set_tags", description = "Set the tags for a card. Pass an array of tag UUIDs to assign to the card. Existing tag assignments are replaced.")]
    async fn card_set_tags(&self, Parameters(param): Parameters<CardSetTagsParam>) -> Result<String, String> {
        log::info!("[MCP] card_set_tags: dataset={}, card={}, tags={}", param.dataset_uuid, param.card_uuid, param.tag_uuids.len());
        let settings = self.app.state::<SettingsState>();
        cards::card_set_tags(settings, param.dataset_uuid, param.card_uuid, param.tag_uuids).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Tags set"}).to_string())
    }

    #[tool(name = "card_get_tags", description = "Get all tags assigned to a card. Returns tag UUIDs, names, and colors.")]
    async fn card_get_tags(&self, Parameters(param): Parameters<CardGetParam>) -> Result<String, String> {
        log::info!("[MCP] card_get_tags: dataset={}, card={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        let tags = cards::card_get_tags(settings, param.dataset_uuid, param.card_uuid).await?;
        let items: Vec<serde_json::Value> = tags.iter().map(|t| {
            serde_json::json!({
                "uuid": t.uuid,
                "name": t.name,
                "color": t.color,
            })
        }).collect();
        Ok(serde_json::json!({"tags": items, "count": items.len()}).to_string())
    }

    // -------------------------------------------------------------------------
    // Card sync tools
    // -------------------------------------------------------------------------

    #[tool(name = "card_sync", description = "Perform a full bidirectional sync for a card dataset with the online server. Pulls remote changes and pushes local changes. Only works for datasets owned by the current user.")]
    async fn card_sync(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_sync: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let result = cards_sync::card_sync_full(settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "result": {
            "pulled": result.pulled,
            "pushed": result.pushed,
            "conflicts": result.conflicts,
            "server_time": result.server_time,
        }}).to_string())
    }

    #[tool(name = "card_sync_all", description = "Sync all owned card datasets with the online server. Returns results for each dataset synced.")]
    async fn card_sync_all(&self) -> Result<String, String> {
        log::info!("[MCP] card_sync_all");
        let settings = self.app.state::<SettingsState>();
        let results = cards_sync::card_sync_all(settings).await?;
        let items: Vec<serde_json::Value> = results.iter().map(|(uuid, result)| {
            match result {
                Ok(r) => serde_json::json!({
                    "dataset_uuid": uuid,
                    "status": "ok",
                    "pulled": r.pulled,
                    "pushed": r.pushed,
                    "conflicts": r.conflicts,
                }),
                Err(e) => serde_json::json!({
                    "dataset_uuid": uuid,
                    "status": "error",
                    "error": e,
                }),
            }
        }).collect();
        Ok(serde_json::json!({"results": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_sync_status", description = "Get the sync status for a card dataset: last synced timestamp, clock offset, and pending changes count.")]
    async fn card_sync_status(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_sync_status: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let dataset_uuid = param.uuid.clone();
        match cards::card_sync_status(settings, param.uuid).await? {
            Some(status) => Ok(serde_json::json!({
                "dataset_uuid": status.dataset_uuid,
                "last_synced_at": status.last_synced_at,
                "clock_offset_ms": status.clock_offset_ms,
                "pending_push": status.pending_push,
            }).to_string()),
            None => Ok(serde_json::json!({"dataset_uuid": dataset_uuid, "last_synced_at": null, "message": "Never synced"}).to_string()),
        }
    }

    // -------------------------------------------------------------------------
    // Log tools
    // -------------------------------------------------------------------------

    #[tool(name = "log_get_history", description = "Get recent application log entries. Useful for debugging or checking what happened during operations. Returns up to 1000 entries.")]
    async fn log_get_history(&self, Parameters(param): Parameters<LogHistoryParam>) -> String {
        let buffer = self.app.state::<LogBuffer>();
        let mut entries = buffer.get_history();
        if let Some(limit) = param.limit {
            if limit < entries.len() {
                entries = entries[entries.len() - limit..].to_vec();
            }
        }
        serde_json::to_string_pretty(&entries).unwrap_or_default()
    }

    #[tool(name = "log_clear", description = "Clear the application log buffer.")]
    async fn log_clear(&self) -> String {
        let buffer = self.app.state::<LogBuffer>();
        buffer.clear();
        serde_json::json!({"status": "ok", "message": "Logs cleared"}).to_string()
    }
}

// ---------------------------------------------------------------------------
// Create the MCP service for nesting in axum router (called from web_service.rs)
// ---------------------------------------------------------------------------

/// Create a StreamableHttpService for the MCP server.
/// Nest this in the axum router at `/mcp`.
pub fn create_mcp_service(app: AppHandle) -> StreamableHttpService<DatasetMcpServer, LocalSessionManager> {
    log::info!("[MCP] Creating MCP service");
    let server = DatasetMcpServer { app };
    let ct = CancellationToken::new();
    
    StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(ct),
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn collect_files_recursive(
    base: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<serde_json::Value>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(base, &path, out);
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(serde_json::json!({ "path": rel, "size": size }));
        }
    }
}
