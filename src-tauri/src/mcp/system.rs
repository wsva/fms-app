//! App-level tools that act on the running app rather than on content: settings,
//! auth, logs, OCR, screenshot capture and UI control.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::{Emitter, Manager};

use crate::auth;
use crate::capture;
use crate::logger::{LogBuffer, MAX_BUFFER_SIZE};
use crate::ocr::{self, OcrState};
use crate::ocr::hyphen::{self, HyphenState};
#[cfg(feature = "textsplit")]
use crate::textsplit::{self, TextSplitState};
use crate::settings::SettingsState;
use crate::workspace::WorkspaceState;

use super::{DatasetMcpServer, JsonValue};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct SettingsSetParam {
    settings: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct OcrRecognizeParam {
    image_base64: String,
    #[serde(default)]
    lang: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct OcrRecognizeTsvParam {
    image_base64: String,
    #[serde(default)]
    lang: String,
    /// How to treat line-break hyphens: "auto" (default, dictionary-driven),
    /// "join" (fold every one into a single word) or "keep" (leave all as printed).
    #[serde(default)]
    hyphen_policy: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct OcrDictionaryParam {
    /// Hunspell dictionary code, e.g. "de", "en", "fr" (see
    /// ocr_hyphen_dictionary_status for the list).
    code: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct LogHistoryParam {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppEventsParam {
    /// Cursor: only events with a larger `seq` are returned. Omit it for the
    /// newest `limit` events; echo `cursor.next_cursor` from the previous answer
    /// to poll for what happened since.
    #[serde(default)]
    after: Option<u64>,
    /// Newest N (default 100, max 1000 — the buffer size).
    #[serde(default)]
    limit: Option<usize>,
    /// Minimum level: trace, debug, info, warn, error (default info).
    #[serde(default)]
    min_level: Option<String>,
    /// Only events whose module path contains this substring, e.g. "sync",
    /// "workflow" or "mcp::".
    #[serde(default)]
    module: Option<String>,
}

/// Filter order for [`AppEventsParam::min_level`]. The buffer stores the level
/// as the string `log` printed; ranking it here beats matching the one string.
fn level_rank(level: &str) -> u8 {
    match level {
        "TRACE" => 1,
        "DEBUG" => 2,
        "INFO" => 3,
        "WARN" => 4,
        "ERROR" => 5,
        _ => 3,
    }
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppAuditGetParam {
    /// How many of the newest entries to return (default 50, max 5000).
    #[serde(default)]
    limit: Option<usize>,
    /// Only entries of this tier: read, write, destructive, ui-control,
    /// human-handoff.
    #[serde(default)]
    tier: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppRestoreParam {
    /// Trash entry name, as returned by app_trash_list.
    name: String,
    /// Absolute path to restore the entry to (parent + folder name). Required
    /// when the entry carries no info.json, i.e. is not a dataset.
    #[serde(default)]
    dest: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppNavigateParam {
    /// Target tab id, e.g. "cards", "dictation", "wiki", "llm-chat".
    tab: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppOpenReviewParam {
    /// Optional card dataset to preselect in the review/quiz panel.
    #[serde(default)]
    dataset_uuid: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppStartDictationParam {
    #[serde(default)]
    dataset_uuid: Option<String>,
    #[serde(default)]
    media_uuid: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct AppNotifyParam {
    message: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CaptureScreenshotParam {
    /// Hide the app window first so the desktop under it is captured (default
    /// false). On Windows this opens the interactive snipping overlay, which
    /// needs a human to drag out a region — leave it false to capture unattended.
    #[serde(default)]
    minimize: Option<bool>,
}

#[cfg(feature = "textsplit")]
#[derive(Deserialize, schemars::JsonSchema, Default)]
struct TextSplitParam {
    /// Text to segment into sentences.
    text: String,
    /// Optional model id (defaults to the built-in SaT 3L Small).
    #[serde(default)]
    model: Option<String>,
    /// Optional boundary probability threshold (default 0.01).
    #[serde(default)]
    threshold: Option<f32>,
    /// Strip surrounding whitespace from each sentence (default true).
    #[serde(default)]
    strip_whitespace: Option<bool>,
    /// Also split on the input's own newlines (default true).
    #[serde(default)]
    split_on_input_newlines: Option<bool>,
}

#[tool_router(router = system_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "settings_get", description = "Get the current app settings: the directory fields (model_dir, recordings_dir, datasets_dir, books_dir), model preferences (selected_model, model_unload_timeout, onboarding_completed), LLM configuration (ollama_url, llm_provider, llm_model), this device's sync identity (pc_url, role, cluster_id), the MCP policy mode (mcp_policy: \"permissive\" journals every tool call and denies nothing, \"enforce\" also refuses destructive ones — see app_audit_get) and whether the MCP surface is exposed at all (mcp_enabled) and whether the agent dock has a floating Bot button (agent_fab_visible). Credentials (llm_api_key, pc_token, device_seed) are never returned: a set one reads as \"***\" with a matching has_<field>=true, an unset one reads as \"\". Pass this object straight back to settings_set — the placeholders are recognised and skipped, not written.")]
    async fn settings_get(&self) -> String {
        log::info!("[MCP] settings_get");
        let state = self.app.state::<SettingsState>();
        let settings = state.settings.lock().unwrap().clone();
        serde_json::to_string_pretty(&settings.redacted_json()).unwrap_or_default()
    }

    #[tool(name = "settings_set", description = "Update app settings. Pass a JSON object holding ONLY the fields to change; everything else keeps its current value, so a partial object is correct and a full settings_get payload echoed back is harmless. Writable: model_dir, recordings_dir, datasets_dir, books_dir, selected_model, model_unload_timeout, onboarding_completed, ollama_url, llm_provider, llm_model, llm_api_key, pc_url, role, read_aloud_tts_voice, mcp_policy, mcp_enabled, agent_fab_visible. Credentials are write-only — set one by passing its real value; \"***\" means 'leave it alone'. device_seed and device_id are derived by the app and always refused. mcp_policy and mcp_enabled are refused too while the policy is \"enforce\", because an agent that can loosen — or switch off — its own surface has none; ask the user to change them on the Settings page. agent_fab_visible is cosmetic: it shows or hides the floating button that opens the dock (the panel itself stays reachable with Ctrl+Space). Flipping mcp_enabled restarts the local web service, so /mcp and /agent.md start or stop answering. The response names what was updated and what was skipped (with the reason), so a no-op write is never silent.")]
    async fn settings_set(&self, Parameters(param): Parameters<SettingsSetParam>) -> Result<String, String> {
        log::info!("[MCP] settings_set");
        let state = self.app.state::<SettingsState>();
        let mut new_settings = state.settings.lock().unwrap().clone();
        let (updated, skipped) = new_settings.merge_json(param.settings.into())?;
        if updated.is_empty() {
            return Ok(serde_json::json!({
                "status": "ok",
                "updated": [],
                "skipped": skipped,
                "message": "nothing to change — every field passed was either unknown, derived, or a \"***\" placeholder. Call settings_get to see the writable fields and their current values.",
            })
            .to_string());
        }
        let ws_dir = state.workspace_dir.lock().unwrap().clone();
        crate::settings::SettingsState::save(&new_settings, ws_dir.as_ref())?;
        let mcp_flipped = {
            let mut s = state.settings.lock().unwrap();
            let flipped = s.mcp_enabled != new_settings.mcp_enabled;
            *s = new_settings;
            flipped
        };
        let _ = self.app.emit("settings-changed", ());
        if mcp_flipped {
            crate::sync::server::restart_after_mcp_toggle(&self.app).await?;
        }
        Ok(serde_json::json!({
            "status": "ok",
            "updated": updated,
            "skipped": skipped,
            "message": "Settings updated",
        })
        .to_string())
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

    #[tool(name = "ocr_recognize_tsv", description = "Recognize text in an image using Tesseract's TSV layout output, then rebuild it in Rust: words are grouped into lines and paragraphs (gap, Tesseract ids, indentation) and every line-break hyphen is decided against the downloaded Hunspell dictionaries of the page's language set (Sprach- + verarbeitung joins when German knows 'Sprachverarbeitung'; E- + Mail stays as printed; a word known to any selected language's dictionary joins). No LLM needed. Pass image as base64 (with or without data URL prefix), optional language code (default: 'eng', combine with '+' e.g. 'deu+eng') and optional hyphen_policy 'auto' | 'join' | 'keep'. Returns {text, baseline, tsv, hyphens, dictionaries, uncovered}: the decided text, the same page with all hyphens kept, the raw dump, one entry per hyphen ({left, hyphen, right, joined, decision, reason}), one entry per dictionary of the language set ({code, label, language, installed}) and the selected languages no dictionary exists for. 'dictionaries' tells you which installs would improve the decisions (use ocr_hyphen_dictionary_download); 'hyphens' lets a caller review or override them by re-running with a hyphen_policy. Prefer this over ocr_recognize when paragraph structure and layout fidelity matter.")]
    async fn ocr_recognize_tsv(&self, Parameters(param): Parameters<OcrRecognizeTsvParam>) -> Result<String, String> {
        log::info!("[MCP] ocr_recognize_tsv: lang={:?} policy={:?}", param.lang, param.hyphen_policy);
        let state = self.app.state::<OcrState>();
        let hyphen_state = self.app.state::<HyphenState>();
        let lang = if param.lang.is_empty() { None } else { Some(param.lang) };
        let result = ocr::ocr_recognize_tsv(state, hyphen_state, param.image_base64, lang, param.hyphen_policy).await?;
        Ok(serde_json::json!({
            "status": "ok",
            "text": result.text,
            "baseline": result.baseline,
            "tsv": result.tsv,
            "hyphens": result.hyphens,
            "dictionaries": result.dictionaries,
            "uncovered": result.uncovered,
        })
        .to_string())
    }

    #[tool(name = "ocr_hyphen_dictionary_status", description = "List the Hunspell dictionaries available for line-break hyphen repair in TSV mode (code, label, upstream version, licence, Tesseract languages it serves, whether it is installed, its size on disk and its download size). 15 languages: de, en, fr, es, it, pt, nl, sv, da, pl, cs, ru, uk, el, tr. Call this before ocr_recognize_tsv to know whether hyphens will be decided by dictionaries or by the fallback shape heuristic, and to see which installs are worth making.")]
    async fn ocr_hyphen_dictionary_status(&self) -> String {
        log::info!("[MCP] ocr_hyphen_dictionary_status");
        let dictionaries = ocr::hyphen::status_all();
        let installed: Vec<&str> = dictionaries
            .iter()
            .filter(|entry| entry.installed)
            .map(|entry| entry.code)
            .collect();
        serde_json::json!({
            "status": "ok",
            "installed": installed,
            "dictionaries": dictionaries,
        })
        .to_string()
    }

    #[tool(name = "ocr_hyphen_dictionary_download", description = "Download and install the Hunspell dictionary for one language (0.5-19 MB, fetched from an npm CDN with mirrors as fallback). The download size is in ocr_hyphen_dictionary_status as approx_bytes. Install the codes of the languages you recognise pages in: every installed dictionary of the recognised language set is consulted, so a German page with English terms benefits from both 'de' and 'en'. Returns the updated status.")]
    async fn ocr_hyphen_dictionary_download(&self, Parameters(param): Parameters<OcrDictionaryParam>) -> Result<String, String> {
        log::info!("[MCP] ocr_hyphen_dictionary_download: {}", param.code);
        let state = self.app.state::<HyphenState>();
        let status = hyphen::download(&param.code).await?;
        state.forget(&param.code);
        Ok(serde_json::json!({"status": "ok", "dictionary": status}).to_string())
    }

    #[tool(name = "ocr_hyphen_dictionary_delete", description = "Delete an installed Hunspell dictionary to free disk space. It can be downloaded again at any time with ocr_hyphen_dictionary_download, so this is safe.")]
    async fn ocr_hyphen_dictionary_delete(&self, Parameters(param): Parameters<OcrDictionaryParam>) -> Result<String, String> {
        log::info!("[MCP] ocr_hyphen_dictionary_delete: {}", param.code);
        let state = self.app.state::<HyphenState>();
        hyphen::delete(&param.code)?;
        state.forget(&param.code);
        Ok(serde_json::json!({"status": "ok", "message": format!("Dictionary '{}' removed", param.code)}).to_string())
    }

    #[cfg(feature = "textsplit")]
    #[tool(name = "text_split", description = "Segment text into sentences using the SaT subword model (85+ languages). Pass the text and optional threshold/model. Auto-loads the model (download SaT 3L Small on the Models page first). Returns the list of sentences.")]
    async fn text_split(&self, Parameters(param): Parameters<TextSplitParam>) -> Result<String, String> {
        log::info!("[MCP] text_split: {} chars, model={:?}", param.text.chars().count(), param.model);
        let state = self.app.state::<TextSplitState>();
        let sentences = textsplit::text_split(
            state,
            param.text,
            param.model.clone(),
            param.threshold,
            None,
            None,
            None,
            param.strip_whitespace,
            param.split_on_input_newlines,
        )
        .await?;
        let count = sentences.len();
        Ok(serde_json::json!({
            "status": "ok",
            "sentences": sentences,
            "count": count,
            "model": param.model,
            "threshold": param.threshold,
        })
        .to_string())
    }

    #[cfg(feature = "textsplit")]
    #[tool(name = "text_split_unload", description = "Unload the cached SaT text-split model to free memory. The model stays downloaded and auto-loads again on the next text_split call, so this is safe to call any time. A no-op when nothing is loaded.")]
    async fn text_split_unload(&self) -> Result<String, String> {
        log::info!("[MCP] text_split_unload");
        let state = self.app.state::<TextSplitState>();
        textsplit::text_split_unload(state).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Text-split model unloaded"}).to_string())
    }

    #[tool(name = "capture_screenshot", description = "Capture the screen and return a data:image/png;base64 URL to pass straight to ocr_recognize — together they are a see-then-act loop over the rendered UI, which no read tool exposes. By default the app window is left alone and the primary monitor is captured as it looks (minimize=false); pass minimize=true to hide the app first and capture the desktop underneath, which on Windows opens the native snipping overlay and therefore needs a human to drag out the region — an agent calling it with minimize=true will park until that overlay is dismissed or the 60 s deadline passes. The image is large, so call this when the text alone is not enough.")]
    async fn capture_screenshot(&self, Parameters(param): Parameters<CaptureScreenshotParam>) -> Result<String, String> {
        let minimize = param.minimize.unwrap_or(false);
        log::info!("[MCP] capture_screenshot: minimize={}", minimize);
        let image_url = capture::capture_screenshot(self.app.clone(), Some(minimize)).await?;
        Ok(serde_json::json!({
            "status": "ok",
            "minimize": minimize,
            "image_url": image_url,
        })
        .to_string())
    }

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

    #[tool(name = "log_clear", description = "Clear the application log buffer. The on-disk log file keeps the history and stays readable via log_read_file_history.")]
    async fn log_clear(&self) -> String {
        let buffer = self.app.state::<LogBuffer>();
        buffer.clear();
        serde_json::json!({"status": "ok", "message": "Logs cleared"}).to_string()
    }

    #[tool(name = "log_get_file_path", description = "Get the path of the persistent on-disk log file (with size-based rotation). The in-memory buffer behind log_get_history only keeps the latest 1000 entries and is lost on restart; this file survives both.")]
    async fn log_get_file_path(&self) -> String {
        log::info!("[MCP] log_get_file_path");
        let path = crate::logger::LogBuffer::file_path();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        serde_json::json!({"status": "ok", "path": path.to_string_lossy(), "size_bytes": size}).to_string()
    }

    #[tool(name = "app_bootstrap", description = "The whole self-description of this server in one call: build identity, the live MCP endpoint and the port it actually bound, your trust zone, the capabilities of this build, the policy mode, the tool index (name + tier + one-line summary per domain, no schemas — use tools/list for those), the recommended first five calls, the house rules, the error convention, the built-in workflow templates and the copy-out payloads for telling another agent about this server. It is the same body GET /agent.json serves and the same body /agent.md renders as prose, generated from the live tool registry, so it cannot name a tool that does not exist. Call this once at the start of a session instead of probing tool by tool. The response is large (~30 KB): if you only need the endpoint, web_service_get_status is cheaper.")]
    async fn app_bootstrap(&self) -> String {
        log::info!("[MCP] app_bootstrap");
        // Anyone who can call a tool got through zone_guard on /mcp, which admits
        // only the trusted zones or a peer with a valid device signature — so
        // "trusted" is true of every caller by construction. The per-reader zone
        // (loopback vs tailnet vs unpaired) is only knowable from the HTTP routes,
        // which is why /agent.json echoes it and this cannot.
        let body = super::manual::render_json(&self.app, "trusted");
        serde_json::to_string_pretty(&body).unwrap_or_default()
    }

    #[tool(name = "app_version", description = "Get the running app's build identity: git_describe (the release tag baked at build time; empty on a shallow/tag-less CI build) and package_version (the Cargo version, which CI stamps to the release tag). Prefer git_describe, fall back to package_version to report which build is installed.")]
    async fn app_version(&self) -> String {
        log::info!("[MCP] app_version");
        serde_json::json!({
            "status": "ok",
            "git_describe": option_env!("FMS_GIT_DESCRIBE").unwrap_or_default(),
            "package_version": env!("CARGO_PKG_VERSION"),
        })
        .to_string()
    }

    #[tool(name = "log_read_file_history", description = "Read archived log entries from the persistent log file (oldest first, includes the rotated backup). Survives app restarts and log_clear, unlike log_get_history. Optional 'limit' returns only the newest N entries.")]
    async fn log_read_file_history(&self, Parameters(param): Parameters<LogHistoryParam>) -> String {
        log::info!("[MCP] log_read_file_history: limit={:?}", param.limit);
        let buffer = self.app.state::<LogBuffer>();
        let entries = buffer.read_file_history(param.limit);
        serde_json::to_string_pretty(&entries).unwrap_or_default()
    }

    #[tool(name = "app_events", description = "One polled timeline of what the app has done, read off the same in-memory log buffer the log_* tools see — so an agent can watch a running job (a subtitle pass, a sync push, a workflow step) instead of asking each domain in turn. Each event is {seq, timestamp, level, module, message}. Poll by echoing cursor.next_cursor into `after`: the first call without `after` returns the newest `limit` events (default 100), and every later call returns only what happened since. Filters: `min_level` (trace|debug|info|warn|error, default info) and `module` (substring of the Rust module path, e.g. \"sync\" or \"workflow\"). cursor.oldest_available is the oldest seq still buffered and evicted_since_cursor counts events the 1000-entry ring dropped unread — when that is non-zero the tail is gone, so use log_read_file_history for the archive.")]
    async fn app_events(&self, Parameters(param): Parameters<AppEventsParam>) -> Result<String, String> {
        let min_level = param.min_level.as_deref().unwrap_or("info").trim().to_ascii_lowercase();
        let floor = match min_level.as_str() {
            "" | "info" => 3,
            "trace" => 1,
            "debug" => 2,
            "warn" => 4,
            "error" => 5,
            other => {
                return Err(format!(
                    "unknown min_level '{}'. Valid levels: trace, debug, info, warn, error",
                    other
                ))
            }
        };
        let needle = param
            .module
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let limit = param.limit.unwrap_or(100).clamp(1, MAX_BUFFER_SIZE);

        let buffer = self.app.state::<LogBuffer>();
        let (events, oldest) = buffer.query(param.after, limit, |e| {
            level_rank(&e.level) >= floor
                && needle.as_ref().is_none_or(|n| e.module.contains(n.as_str()))
        });
        // An `after` that is older than the oldest buffered seq means the ring
        // overwrote events nobody read. Counted in seq space, so it is an upper
        // bound on what was actually lost: the filters above hide some of it.
        let evicted = match (param.after, oldest) {
            (Some(after), Some(oldest)) if after + 1 < oldest => oldest - after - 1,
            _ => 0,
        };
        let next_cursor = events.last().map(|e| e.seq).or(param.after);
        let count = events.len();
        let note = if evicted > 0 {
            format!(
                "{} events were appended and evicted from the {}-entry ring before this read, so the timeline has a hole. Use log_read_file_history for the archive.",
                evicted, MAX_BUFFER_SIZE
            )
        } else if count == 0 {
            "Nothing new since the cursor. Poll again with the same `after` after doing something else — do not spin on this call.".to_string()
        } else {
            "Poll again with the `cursor.next_cursor` value in `after` to get only what happens next.".to_string()
        };
        Ok(serde_json::json!({
            "status": "ok",
            "count": count,
            "events": events,
            "cursor": {
                "after": param.after,
                "next_cursor": next_cursor,
                "oldest_available": oldest,
            },
            "evicted_since_cursor": evicted,
            "filters": { "min_level": min_level, "module": needle },
            "buffer_size": MAX_BUFFER_SIZE,
            "note": note,
        })
        .to_string())
    }

    #[tool(name = "app_trash_list", description = "List what the app trash holds: one entry per delete that can still be undone, newest first, each {name, path, kind, trashed_at, original_name, bytes, dataset}. `dataset` is the info.json summary (uuid/type/name/format/language) when the entry is a dataset, book or read-aloud folder — that is how you tell which uuid a delete actually took. Pass an entry's `name` to app_restore to put it back. Cross-volume leftovers named *.partial are not listed: they never finished publishing, so they are not a backup.")]
    async fn app_trash_list(&self) -> Result<String, String> {
        let entries = crate::trash::list()?;
        let count = entries.len();
        let note = if count == 0 {
            "Nothing is in the trash. Deletes that are meant to be undoable land there and report `trashed_to`."
        } else {
            "Restore one with app_restore({name}), which moves it back out — nothing in the trash is ever deleted by a tool."
        };
        Ok(serde_json::json!({
            "status": "ok",
            "trash_dir": crate::trash::trash_dir().to_string_lossy(),
            "count": count,
            "entries": entries,
            "note": note,
        })
        .to_string())
    }

    #[tool(name = "app_restore", description = "Undo a delete: move one app-trash entry back onto disk. `name` is the bare entry name from app_trash_list. Without `dest` the entry goes back to its dataset type's default root (<datasets_dir>/<type>/<original name>), which only works when it still carries its info.json — books, wiki folders and plain files need an explicit absolute `dest`. Refuses to write over anything that already exists, and refuses a `dest` inside the trash. The restored folder keeps its uuid, so dataset_list / book_list / read_aloud_list show it again on the next read; nothing else has to be re-imported.")]
    async fn app_restore(&self, Parameters(param): Parameters<AppRestoreParam>) -> Result<String, String> {
        log::info!("[MCP] app_restore: name={} dest={:?}", param.name, param.dest);
        let settings = self.app.state::<SettingsState>();
        let restored = crate::trash::restore(&settings, &param.name, param.dest.as_deref())?;
        Ok(restored.to_string())
    }

    #[tool(name = "app_navigate", description = "Switch the app to another page/tab. Valid tabs: dictation, books, read-aloud, cards, tasks, agent, workflow, datasets-sync, devices-hub, simple-words, models, edge-tts, llm-chat, chat, ocr, split-text, pdf-reader, wiki, workspaces, logs, settings. Use this to bring the user to the right screen before/while acting. `agent` is the agent *configuration* page — the copyable MCP snippets and the policy mode; it holds no conversation, because you (the agent) run in your own window. ui_list_actions reports which tab each registered action belongs to, so the two vocabularies agree.")]
    async fn app_navigate(&self, Parameters(param): Parameters<AppNavigateParam>) -> Result<String, String> {
        // Must match `TabId` in src/components/layout/Sidebar.tsx exactly: the
        // shell does `setActiveTab(tab)` with this string, and a name that is
        // not a TabId matches no page, so the content area goes empty with no
        // error anywhere. (It once listed a "studio" tab that never existed and
        // omitted split-text and pdf-reader.)
        const VALID: [&str; 21] = [
            "dictation", "books", "read-aloud", "cards", "tasks", "agent", "workflow",
            "datasets-sync", "devices-hub", "simple-words", "models", "edge-tts", "llm-chat",
            "chat", "ocr", "split-text", "pdf-reader", "wiki", "workspaces", "logs", "settings",
        ];
        // `read-book` was this tab's id before it was renamed to `books`. Fold it
        // onto the current name so an agent carrying an older recipe still lands
        // on the page instead of hitting an "unknown tab" dead end.
        let tab = match param.tab.trim() {
            "read-book" => "books",
            other => other,
        };
        if !VALID.contains(&tab) {
            return Err(format!(
                "unknown tab '{}'. Valid tabs: {}",
                tab,
                VALID.join(", ")
            ));
        }
        log::info!("[MCP] app_navigate: tab={}", tab);
        self.app
            .emit("agent-action", serde_json::json!({ "type": "navigate", "tab": tab }))
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "ok": true, "action": "navigate", "tab": tab }).to_string())
    }

    // Named `app_*` rather than `workspace_*`: the tools in this file act on the
    // running app, `workspace_` is not a domain prefix in mcp/manual.rs (a
    // `workspace_*` tool would fall into the `misc` bucket and fail
    // `every_tool_has_a_domain`), and the command it calls keeps the
    // `workspace_get_current_dir` name because that one only returns a path.
    #[tool(name = "app_open_workspace_dir", description = "Reveal the current workspace's directory in the OS file explorer (Explorer / Finder / xdg) and return its absolute path. Nothing on disk changes, but it does put an explorer window on the user's desktop — which is why the policy tier is UI_CONTROL and not a read. Errors with a next-step hint when no workspace is selected; ask the user to switch or create one on the Workspaces page rather than guessing a path.")]
    async fn app_open_workspace_dir(&self) -> Result<String, String> {
        let state = self.app.state::<WorkspaceState>();
        let dir = crate::workspace::workspace_get_current_dir(state).await?;
        log::info!("[MCP] app_open_workspace_dir: {}", dir);
        tauri_plugin_opener::reveal_item_in_dir(&dir).map_err(|e| {
            format!("the workspace directory exists but the OS explorer refused to open it: {e}. Path: {dir}")
        })?;
        Ok(serde_json::json!({ "ok": true, "path": dir }).to_string())
    }

    #[tool(name = "app_open_review", description = "Open the interactive Cards review/quiz panel. Optionally preselect a card dataset by uuid. Use this when the user wants the visual quiz UI (as opposed to answering inline in the chat).")]
    async fn app_open_review(&self, Parameters(param): Parameters<AppOpenReviewParam>) -> Result<String, String> {
        log::info!("[MCP] app_open_review: dataset={:?}", param.dataset_uuid);
        let payload = serde_json::json!({
            "type": "open-review",
            "dataset_uuid": param.dataset_uuid,
        });
        self.app.emit("agent-action", payload).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "ok": true,
            "action": "open-review",
            "dataset_uuid": param.dataset_uuid,
        }).to_string())
    }

    #[tool(name = "app_start_dictation", description = "Open the Dictation page and optionally select a dataset/media item to start practicing.")]
    async fn app_start_dictation(&self, Parameters(param): Parameters<AppStartDictationParam>) -> Result<String, String> {
        log::info!("[MCP] app_start_dictation: dataset={:?} media={:?}", param.dataset_uuid, param.media_uuid);
        let payload = serde_json::json!({
            "type": "start-dictation",
            "dataset_uuid": param.dataset_uuid,
            "media_uuid": param.media_uuid,
        });
        self.app.emit("agent-action", payload).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "ok": true,
            "action": "start-dictation",
            "dataset_uuid": param.dataset_uuid,
            "media_uuid": param.media_uuid,
        }).to_string())
    }

    #[tool(name = "app_notify", description = "Show a toast notification to the user in the app UI.")]
    async fn app_notify(&self, Parameters(param): Parameters<AppNotifyParam>) -> Result<String, String> {
        log::info!("[MCP] app_notify: {}", param.message);
        self.app
            .emit("agent-action", serde_json::json!({ "type": "notify", "message": param.message }))
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "ok": true, "action": "notify", "message": param.message }).to_string())
    }

    #[tool(name = "app_get_ui_state", description = "Read what the user is currently looking at: the active tab, the selected dataset, the open media item and cue, any text selection. Call this before navigating or acting so you know where the user is. ui_mounted=false means the webview has not reported yet (app still starting, or window closed) — every other field is then null and you should not treat that as 'nothing selected'.")]
    async fn app_get_ui_state(&self) -> String {
        let ui = self.app.state::<crate::ui_state::UiStateHandle>();
        let mut out = crate::ui_state::context_json(&ui);
        out.insert("ui_mounted".to_string(), serde_json::json!(ui.ui_mounted()));
        serde_json::json!({ "ok": true, "state": out }).to_string()
    }

    #[tool(name = "app_audit_get", description = "Read the MCP audit journal: newest-first entries of {id, ts, tool, tier, decision, agent_id, args_digest}, one row per tool call this server has served (including this one). tier is what the call was classified as (read, write, destructive, ui-control, human-handoff) and decision is allow|deny. Optional limit (default 50) and tier filter. args_digest is a hash of the arguments, not the arguments — the journal never stores content. Use it to answer 'what has already been done in this session' and, under mcp_policy=\"enforce\", to see what was refused and why.")]
    async fn app_audit_get(&self, Parameters(param): Parameters<AppAuditGetParam>) -> Result<String, String> {
        let tier = match param.tier.as_deref() {
            None | Some("") => None,
            Some(raw) => Some(super::policy::Tier::parse(raw).ok_or_else(|| {
                format!(
                    "unknown tier '{}'. Valid tiers: read, write, destructive, ui-control, human-handoff",
                    raw
                )
            })?),
        };
        let state = self.app.state::<SettingsState>();
        let entries = super::policy::audit_query(&state, param.limit.unwrap_or(50), tier)?;
        let policy = state
            .settings
            .lock()
            .map(|s| super::policy::normalize_mode(&s.mcp_policy).to_string())
            .unwrap_or_else(|_| "permissive".to_string());
        let count = entries.len();
        Ok(serde_json::json!({
            "status": "ok",
            "mcp_policy": policy,
            "count": count,
            "entries": entries,
        })
        .to_string())
    }
}
