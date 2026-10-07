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

#[tool_router(router = datasets_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "dataset_list", description = "List all datasets with their UUID, name, description, status, and path")]
    async fn dataset_list(&self) -> String {
        log::info!("[MCP] dataset_list called");
        let settings = self.app.state::<SettingsState>();
        let datasets = datasets::list_datasets(&settings);
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
        let dir = datasets::find_dataset_dir(&settings, &param.uuid).map_err(|e| e)?;
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

    #[tool(name = "dataset_generate_subtitles", description = "Generate subtitles for all media files using STT model. Requires a model to be downloaded first.")]
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

    #[tool(name = "dataset_update", description = "Update a dataset's name and/or description.")]
    async fn dataset_update(&self, Parameters(param): Parameters<DatasetUpdateParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_update: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let name = if param.name.is_empty() { None } else { Some(param.name) };
        let desc = if param.description.is_empty() { None } else { Some(param.description) };
        datasets::dataset_update(settings, param.uuid, name, desc).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset updated"}).to_string())
    }

    #[tool(name = "dataset_delete", description = "Delete a dataset and all its files. This is irreversible — the dataset directory is removed from disk.")]
    async fn dataset_delete(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::warn!("[MCP] dataset_delete: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::dataset_delete(settings, param.uuid).await?;
        let _ = self.app.emit("dataset-list-changed", ());
        Ok(serde_json::json!({"status": "ok", "message": "Dataset deleted"}).to_string())
    }

    #[tool(name = "dataset_import_media", description = "Import media files from a directory into a dataset. Set link=true to create symlinks instead of copying files. Returns the number of files imported.")]
    async fn dataset_import_media(&self, Parameters(param): Parameters<DatasetImportMediaParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_import_media: uuid={}, source={}, link={}", param.uuid, param.source_dir, param.link);
        let settings = self.app.state::<SettingsState>();
        let count = datasets::dataset_import_media(settings, param.uuid, param.source_dir, param.link).await?;
        Ok(serde_json::json!({"status": "ok", "files_imported": count}).to_string())
    }
}
