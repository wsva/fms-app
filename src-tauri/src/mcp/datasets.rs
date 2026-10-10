//! Dataset discovery, CRUD and the generation pipeline: list/import/create/update
//! datasets, browse their files, and drive subtitle + waveform generation.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::{Emitter, Manager};

use crate::datasets;
use crate::models::ModelState;
use crate::settings::SettingsState;

use super::{DatasetMcpServer, ReadFileParam, UuidParam};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WaveformSingleParam {
    uuid: String,
    media_uuid: String,
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
struct DatasetInfoSaveParam {
    uuid: String,
    /// The whole `info.json` document as JSON text. Identity fields (uuid, type,
    /// format, created_at) are ignored on write — the server keeps the existing
    /// ones — and `updated_at` is refreshed.
    content: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DatasetInfoUpdateParam {
    uuid: String,
    /// Empty means "leave this field alone" — the same convention every other
    /// update tool here uses, so an agent can set one field without resending
    /// the rest.
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    language: String,
    /// `"private"` | `"shared"` | `"public"`. Sets `sharing.visibility`.
    #[serde(default)]
    visibility: String,
    /// Sets `sharing.owner_id` (the account that may push this dataset).
    #[serde(default)]
    owner_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct DatasetImportMediaParam {
    uuid: String,
    source_dir: String,
    #[serde(default)]
    link: bool,
}

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

/// `dataset_list` entry: the whole unified descriptor, plus the three things only
/// a local scan can say — where the dataset is, which root it came from, and
/// whether its primary store exists. Every type gets the same keys, so a caller
/// needs no per-type schema.
fn list_entry(
    info: &datasets::DatasetInfo,
    path: &std::path::Path,
    location: &str,
    is_wiki: bool,
) -> serde_json::Value {
    let mut v = serde_json::to_value(info).unwrap_or_else(|_| serde_json::json!({}));
    // Wiki datasets hold their content in `.md` files and have no database by
    // design, so absence of `data.sqlite3` is not "not ready" for them.
    let status = if is_wiki || path.join("data.sqlite3").exists() {
        "ready"
    } else {
        "not_ready"
    };
    if let Some(obj) = v.as_object_mut() {
        obj.insert("path".into(), serde_json::json!(path.to_string_lossy()));
        obj.insert("location".into(), serde_json::json!(location));
        obj.insert("status".into(), serde_json::json!(status));
    }
    v
}

#[tool_router(router = datasets_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "dataset_list", description = "List every dataset of every type (dictation, card, book, read_aloud, wiki). Each entry carries the unified info.json fields (uuid, type, format, name, description, language, created_at, updated_at, sharing) plus path, location and status on this machine.")]
    async fn dataset_list(&self) -> String {
        log::info!("[MCP] dataset_list called");
        let settings = self.app.state::<SettingsState>();
        let items: Vec<serde_json::Value> = datasets::DatasetType::ALL
            .iter()
            .flat_map(|ty| {
                let is_wiki = *ty == datasets::DatasetType::Wiki;
                datasets::scan_datasets(&settings, *ty)
                    .into_iter()
                    .map(move |d| list_entry(&d.info, &d.path, &d.location, is_wiki))
            })
            .collect();
        serde_json::to_string_pretty(&items).unwrap_or_default()
    }

    #[tool(name = "dataset_get", description = "Get detailed info about a dictation dataset: info.json contents and which artifacts exist")]
    async fn dataset_get(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_get: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
        let info = datasets::read_info(&dir).map_err(|e| e)?;
        let has = |p: &str| dir.join(p).exists();
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

    #[tool(name = "dataset_info_get", description = "Read a dataset's unified info.json descriptor, whatever its type: spec, uuid, type, format, name, description, language, created_at, updated_at, sharing (visibility, owner_id, subscribers), plus its path and location on this machine.")]
    async fn dataset_info_get(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_info_get: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let (dir, _) = datasets::find_dataset_dir_typed(&settings, &param.uuid)?;
        let info = datasets::read_info(&dir)?;
        let location = dir
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(serde_json::to_string_pretty(&list_entry(
            &info,
            &dir,
            &location,
            info.dataset_type == datasets::DatasetType::Wiki,
        ))
        .map_err(|e| e.to_string())?)
    }

    #[tool(name = "dataset_read_file", description = "Read a file from a dataset directory (e.g. info.json, a VTT subtitle, book.txt)")]
    async fn dataset_read_file(
        &self,
        Parameters(param): Parameters<ReadFileParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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
        let result = datasets::dataset_write_subtitles_to_db(self.app.clone(), settings, param.uuid)
            .await?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    #[tool(name = "dataset_generate_subtitles", description = "Generate subtitles for all media files using the STT model (requires a model downloaded first). Writes VTT files under subtitle/ AND imports each subtitle into data.sqlite3 as an active listen_subtitle + cues (version 1), keyed by media source and idempotently (existing active subtitles are left untouched) - so no separate build-database step is needed. Returns a summary of subtitles generated and DB rows written.")]
    async fn dataset_generate_subtitles(
        &self,
        Parameters(param): Parameters<UuidParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let model_state = self.app.state::<ModelState>();
        datasets::dataset_generate_subtitles(self.app.clone(), settings, model_state, param.uuid)
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
        let source = datasets::dataset_generate_subtitle_single(self.app.clone(), settings, model_state, param.uuid, param.media_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "source": source}).to_string())
    }

    #[tool(name = "dataset_generate_waveforms", description = "Generate waveform JSON files for all media files that don't have one yet. Uses Symphonia for peak detection.")]
    async fn dataset_generate_waveforms(
        &self,
        Parameters(param): Parameters<UuidParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let count = datasets::dataset_generate_waveform(self.app.clone(), settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "files_generated": count}).to_string())
    }

    #[tool(name = "dataset_generate_waveform_single", description = "Generate waveform for a single media file. Use dataset_list_media first to get media UUIDs, then call this for each media. Returns the source path on success.")]
    async fn dataset_generate_waveform_single(
        &self,
        Parameters(param): Parameters<WaveformSingleParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let source = datasets::dataset_generate_waveform_single(settings, param.uuid, param.media_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "source": source}).to_string())
    }

    #[tool(name = "dataset_list_media", description = "List all media files in a dataset. Returns UUID, source path, and duration for each media.")]
    async fn dataset_list_media(
        &self,
        Parameters(param): Parameters<UuidParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let media = datasets::dictation::listen_list_media(settings, param.uuid).await?;
        let items: Vec<serde_json::Value> = media.iter().map(|m| {
            serde_json::json!({
                "uuid": m.uuid,
                "source": m.source,
                "duration_ms": m.duration_ms
            })
        }).collect();
        Ok(serde_json::json!({"media": items, "count": items.len()}).to_string())
    }

    #[tool(name = "dataset_list_dirs", description = "List all dataset directories (default datasets directory + linked directories) for a specific dataset type. Returns entries with name, path, and is_linked flag.")]
    async fn dataset_list_dirs(&self, Parameters(param): Parameters<DatasetListDirsParam>) -> String {
        log::info!("[MCP] dataset_list_dirs: dataset_type={}", param.dataset_type);
        let settings = self.app.state::<SettingsState>();
        match datasets::dataset_list_dirs(settings, param.dataset_type).await {
            Ok(entries) => serde_json::json!({"directories": entries}).to_string(),
            Err(e) => serde_json::json!({"error": e}).to_string(),
        }
    }

    #[tool(name = "dataset_add_dir", description = "Add a linked directory for datasets. The path must exist and be a directory. Requires a name for display.")]
    async fn dataset_add_dir(&self, Parameters(param): Parameters<DatasetAddDirParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_add_dir: name={}, path={}", param.name, param.path);
        let settings = self.app.state::<SettingsState>();
        datasets::dataset_add_dir(settings, param.name.clone(), param.path.clone()).await?;
        Ok(serde_json::json!({"status": "ok", "message": format!("Linked directory '{}' -> {}", param.name, param.path)}).to_string())
    }

    #[tool(name = "dataset_remove_dir", description = "Remove a linked directory from datasets. Datasets in that directory are no longer visible.")]
    async fn dataset_remove_dir(&self, Parameters(param): Parameters<PathParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_remove_dir: path={}", param.path);
        let settings = self.app.state::<SettingsState>();
        datasets::dataset_remove_dir(settings, param.path.clone()).await?;
        Ok(serde_json::json!({"status": "ok", "message": format!("Unlinked directory: {}", param.path)}).to_string())
    }

    #[tool(name = "dataset_import", description = "Import an existing dataset directory. The directory must contain an info.json file. Returns the dataset summary with UUID.")]
    async fn dataset_import(&self, Parameters(param): Parameters<DatasetImportParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_import: source={}", param.source_dir);
        let settings = self.app.state::<SettingsState>();
        let summary = datasets::dataset_import(settings, param.source_dir).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "dataset_create", description = "Create a new empty dataset with a name and optional description. Returns the dataset summary with UUID. Use dataset_import_media afterwards to add audio files.")]
    async fn dataset_create(&self, Parameters(param): Parameters<DatasetCreateParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_create: name={}", param.name);
        let settings = self.app.state::<SettingsState>();
        let desc = if param.description.is_empty() { None } else { Some(param.description) };
        let loc = if param.location.is_empty() { None } else { Some(param.location) };
        let summary = datasets::dataset_create(settings, param.name, desc, loc).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "dataset_init_dir", description = "Initialize a raw media folder IN PLACE as a dictation dataset: mint a server-side uuid and write a default info.json into the folder where it already lives (no copy — unlike dataset_import). The folder must sit directly inside a configured dataset location and contain media/. Idempotent: if a readable info.json already exists its summary is returned unchanged. Follow with dataset_info_save to edit the metadata.")]
    async fn dataset_init_dir(&self, Parameters(param): Parameters<PathParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_init_dir: path={}", param.path);
        let settings = self.app.state::<SettingsState>();
        let summary = datasets::dataset_init_dir(settings, param.path).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "dataset_sync_media", description = "Reconcile a dictation dataset's listen_media table with the files under media/ IN PLACE: register a row for every media file not yet in the DB and delete rows whose file is gone (dropping their subtitles/cues/versions/transcripts too). Idempotent and, unlike dataset_generate_database, it never rebuilds the DB or re-keys surviving media, so existing cues and practice history stay intact. Requires an existing data.sqlite3 (created empty at dataset_create/dataset_init_dir). Returns added_count and removed_count.")]
    async fn dataset_sync_media(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_sync_media: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let result = datasets::dataset_sync_media(settings, param.uuid).await?;
        Ok(serde_json::json!({
            "status": "ok",
            "added_count": result.added_count,
            "removed_count": result.removed_count
        })
        .to_string())
    }

    #[tool(name = "dataset_audit", description = "Read-only health check of a dictation dataset: walk media/, subtitle/, waveform/, transcript/ and data.sqlite3 against each other and report every disagreement as a named check. Covers media registered vs present (new / deleted files), VTT files that exist but were never imported, cue-count and mtime drift between a VTT file and its database rows, missing or stale waveform JSON, whether cues still carry raw STT timings (no 'adjusted using waveform' note), cue-level sanity (empty, zero-length, overlapping, non-increasing or past-the-end cues), ambiguous subtitle versions and reference material (book.txt / transcript/). Each check returns an exact count, up to 100 offending files with a reason, and the fix_step name of the template step that repairs it. Never writes anything - run it before deciding which pipeline step is next. Also returns the six when-guard facts the graph evaluates.")]
    async fn dataset_audit(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_audit: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let report = datasets::dictation::audit::audit(&settings, &param.uuid)?;
        Ok(serde_json::to_string_pretty(&report).unwrap_or_default())
    }

    async fn dataset_info_save(&self, Parameters(param): Parameters<DatasetInfoSaveParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_info_save: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let info = datasets::dataset_info_save(settings, param.uuid, param.content).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::to_string_pretty(&info).unwrap_or_default())
    }

    #[tool(name = "dataset_info_update", description = "Update a dataset's info.json metadata — any type. Settable: name, description, language, visibility ('private' | 'shared' | 'public') and owner_id. Empty fields are left unchanged. 'subscribers' is deliberately not settable: the website owns it and a local edit would dirty the dataset for every follower.")]
    async fn dataset_info_update(
        &self,
        Parameters(param): Parameters<DatasetInfoUpdateParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] dataset_info_update: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let (dir, ty) = datasets::find_dataset_dir_typed(&settings, &param.uuid)?;
        let mut info = datasets::read_info(&dir)?;
        if !param.name.is_empty() {
            info.name = param.name.clone();
        }
        if !param.description.is_empty() {
            info.description = param.description.clone();
        }
        if !param.language.is_empty() {
            info.language = param.language.clone();
        }
        if !param.visibility.is_empty() {
            if !matches!(param.visibility.as_str(), "private" | "shared" | "public") {
                return Err(format!(
                    "Invalid visibility '{}' (expected private, shared or public)",
                    param.visibility
                ));
            }
            info.sharing.visibility = param.visibility.clone();
        }
        if !param.owner_id.is_empty() {
            info.sharing.owner_id = param.owner_id.clone();
        }
        info.updated_at = datasets::now_stamp();
        datasets::write_info(&dir, &info)?;
        let _ = self.app.emit("dataset-list-changed", ());
        log::info!(
            "[MCP] dataset_info_update: {} ({}) updated",
            param.uuid,
            ty.as_str()
        );
        Ok(serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?)
    }

    #[tool(name = "dataset_delete", description = "Delete a dataset by moving its whole directory into the app trash folder — not permanently removed, so this is recoverable. Returns `trashed_to`, the path the directory landed at. Use `dataset_list` first to get the UUID.")]
    async fn dataset_delete(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::warn!("[MCP] dataset_delete: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let trashed_to = datasets::dataset_delete(settings, param.uuid).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::json!({
            "status": "ok",
            "message": "Dataset moved to trash",
            "trashed_to": trashed_to
        })
        .to_string())
    }

    #[tool(name = "dataset_import_media", description = "Import media files from a directory into a dataset. Set link=true to create symlinks instead of copying files. Returns the number of files imported.")]
    async fn dataset_import_media(&self, Parameters(param): Parameters<DatasetImportMediaParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_import_media: uuid={}, source={}, link={}", param.uuid, param.source_dir, param.link);
        let settings = self.app.state::<SettingsState>();
        let count = datasets::dataset_import_media(settings, param.uuid, param.source_dir, param.link).await?;
        Ok(serde_json::json!({"status": "ok", "files_imported": count}).to_string())
    }
}
