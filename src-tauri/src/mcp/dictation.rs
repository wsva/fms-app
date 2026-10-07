//! Dictation practice surface: cue-time adjustment and alignment, dictation
//! progress, favourite cues, and subtitle version history.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::Manager;

use crate::datasets;
use crate::models::ModelState;
use crate::settings::SettingsState;

use super::{DatasetMcpServer, JsonValue, UuidParam};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WordLevelSyncParam {
    uuid: String,
    /// Optional: limit to a single media file. If omitted, processes all media.
    media_uuid: Option<String>,
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

#[tool_router(router = dictation_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "dataset_adjust_cue_time", description = "Adjust cue timestamps based on silence detection in the audio. Use mode='new' for first adjustment or 'in_place' to overwrite original cues. Already-adjusted subtitles are skipped by default; set force=true to re-process them. Strategy options: 'noise_floor' (default, good for consistent noise), 'dual_bound' (prevents over-detection on clean audio), 'peak_relative' (simple peak-based), 'otsu' (automatic optimal threshold).")]
    async fn dataset_adjust_cue_time(
        &self,
        Parameters(param): Parameters<AdjustCueTimeParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result: String = datasets::dictation::adjust::dataset_adjust_cue_time(settings, param.uuid, param.mode, Some(param.force), param.strategy)?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    #[tool(name = "dataset_adjust_cue_time_single", description = "Adjust cue timestamps for a SINGLE media file based on silence detection. Use mode='new' for first adjustment or 'in_place' to overwrite original cues. Already-adjusted subtitles are skipped by default; set force=true to re-process them. Strategy options: 'noise_floor' (default), 'dual_bound', 'peak_relative', 'otsu' (automatic optimal threshold).")]
    async fn dataset_adjust_cue_time_single(
        &self,
        Parameters(param): Parameters<AdjustCueTimeSingleParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result: String = datasets::dictation::adjust::dataset_adjust_cue_time_single(settings, param.uuid, param.media_uuid, param.mode, Some(param.force), param.strategy)?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    #[tool(name = "dataset_sync_cue_times", description = "Sync cue timestamps from fresh VTT subtitles to existing DB cues. For each media, matches fresh cues to DB cues by text similarity (Ratcliff-Obershelp) and updates timestamps. Cue text is NOT changed. Use after generating fresh subtitles. Default similarity threshold is 70.")]
    async fn dataset_sync_cue_times(
        &self,
        Parameters(param): Parameters<SyncCueTimesParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result = datasets::dictation::adjust::dataset_sync_cue_times(settings, param.uuid, param.threshold)
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
        let result = datasets::dictation::adjust::dataset_sync_cue_times_word_level(self.app.clone(), settings, model_state, param.uuid, param.media_uuid)
            .await?;
        Ok(serde_json::json!({"status": "ok", "result": result}).to_string())
    }

    #[tool(name = "subtitle_get_versions", description = "Get the version history for a subtitle, showing all changes made over time.")]
    async fn subtitle_get_versions(
        &self,
        Parameters(param): Parameters<SubtitleVersionParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let versions = datasets::dictation::subtitle_get_versions(settings, String::new(), param.subtitle_uuid)
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
        let cues = datasets::dictation::subtitle_get_cues_at_version(settings, String::new(), param.subtitle_uuid, param.version)
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
        let new_version = datasets::dictation::subtitle_rollback_to_version(settings, String::new(), param.subtitle_uuid, param.target_version)
            .await
            .map_err(|e| e)?;
        Ok(serde_json::json!({
            "status": "ok",
            "rolled_back_to": param.target_version,
            "new_version": new_version
        }).to_string())
    }

    #[tool(name = "dataset_align_cues", description = "Align cue text to a book.txt reference using multi-pass similarity matching. Requires book.txt in the dataset directory. Use after dataset_generate_database.")]
    async fn dataset_align_cues(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_align_cues: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::dictation::align::dataset_align_cues(self.app.clone(), settings, param.uuid).await
    }

    #[tool(name = "dataset_align_cues_transcript", description = "Align cue text using per-subtitle transcript files. Requires transcript/ directory. Use after dataset_generate_database.")]
    async fn dataset_align_cues_transcript(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] dataset_align_cues_transcript: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::dictation::align::dataset_align_cues_transcript(self.app.clone(), settings, param.uuid).await
    }

    #[tool(name = "dictation_save_progress", description = "Save or update dictation practice progress for a media/subtitle pair. Status values: 'in_progress', 'completed'.")]
    async fn dictation_save_progress(&self, Parameters(param): Parameters<DictationSaveProgressParam>) -> Result<String, String> {
        log::info!("[MCP] dictation_save_progress: media={}", param.media_uuid);
        let settings = self.app.state::<SettingsState>();
        let dictation = datasets::dictation::ListenDictation {
            media_uuid: param.media_uuid,
            subtitle_uuid: param.subtitle_uuid,
            status: param.status,
            completed: param.completed,
        };
        datasets::dictation::listen_save_dictation(settings, String::new(), dictation).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Progress saved"}).to_string())
    }

    #[tool(name = "dictation_save_cue", description = "Create or update a cue in a dataset. Pass the cue as a JSON object with fields: uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, confidence.")]
    async fn dictation_save_cue(&self, Parameters(param): Parameters<DictationSaveCueParam>) -> Result<String, String> {
        log::info!("[MCP] dictation_save_cue: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let cue: datasets::dictation::ListenCue = serde_json::from_value(param.cue.into())
            .map_err(|e| format!("Invalid cue JSON: {}", e))?;
        datasets::dictation::listen_save_cue(settings, param.dataset_uuid, cue).await?;
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
        let result = datasets::dictation::dictation_add_cue_to_favorites(
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
        let uuids = datasets::dictation::dictation_list_favorite_cues(settings).await?;
        Ok(serde_json::json!({ "status": "ok", "count": uuids.len(), "cue_uuids": uuids }).to_string())
    }

    #[tool(name = "dictation_delete_cue", description = "Delete a cue from a dataset by its UUID.")]
    async fn dictation_delete_cue(&self, Parameters(param): Parameters<DictationDeleteCueParam>) -> Result<String, String> {
        log::info!("[MCP] dictation_delete_cue: dataset={}, cue={}", param.dataset_uuid, param.cue_uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::dictation::listen_delete_cue(settings, param.dataset_uuid, param.cue_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Cue deleted"}).to_string())
    }

    #[tool(name = "subtitle_create_version", description = "Create a new version snapshot of current cues for a subtitle. Call this before making edits, so you can rollback later. Returns the version number.")]
    async fn subtitle_create_version(
        &self,
        Parameters(param): Parameters<SubtitleCreateVersionParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] subtitle_create_version: subtitle={}", param.subtitle_uuid);
        let settings = self.app.state::<SettingsState>();
        let version = datasets::dictation::subtitle_create_version(
            settings, param.dataset_uuid, param.subtitle_uuid,
            param.change_type, param.description, param.created_by,
        ).await?;
        Ok(serde_json::json!({"status": "ok", "version": version}).to_string())
    }
}
