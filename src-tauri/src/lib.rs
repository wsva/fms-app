// ---------------------------------------------------------------------------
// Grouped subsystems
//
// Each group's `mod.rs` declares its own children and carries the per-child
// feature gates; only a wholly-gated group is gated here.
// ---------------------------------------------------------------------------

/// Datasets: shared root/`meta.json` discovery plus one submodule per dataset
/// type (`dictation`, `cards`, `book`, `read_aloud`), mirroring the on-disk
/// `<datasets_dir>/{dictation,card,book,read_aloud}/` layout.
mod datasets;
/// Cross-device sync: the PC server stack (`server`/`rest`/`pairing`, desktop
/// only) and the client half (`client`/`discover`/`change_log`) that the Android
/// thin client runs. Owns `PROTOCOL_VERSION`.
mod sync;
/// Ollama LLM client, cross-device chat, and the Goose agent integrations.
mod ai;
/// STT model download/lifecycle/transcription, download catalogs, and the
/// unified model index.
#[cfg(feature = "stt")]
mod models;
/// Built-in MCP server: one submodule per tool domain, merged into a single
/// `ServerHandler` by `mcp::tool_router()`. Mounted at `/mcp` on the sync web
/// service, so it is desktop-only exactly like `sync::server`.
#[cfg(feature = "desktop")]
mod mcp;

// App infrastructure (cross-cutting: paths, DB helpers, settings, logging,
// workspaces, auth, audio decoding).
mod app_paths;
mod audio;
mod auth;
mod db;
mod logger;
mod settings;
mod workspace;

// Content and learning state that is *not* stored as a dataset: simple_words
// reads across card datasets, XP lives in the app-level SQLite.
mod simple_words;
mod xp;

// Voice synthesis (all platforms).
mod edge_tts;

// Desktop-only utilities.
#[cfg(feature = "desktop")]
mod capture;
#[cfg(feature = "desktop")]
mod ocr;

use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

/// Install the process-level rustls `CryptoProvider`.
///
/// Several crates in the tree (reqwest, rmcp, goose-sdk, tokio-tungstenite)
/// enable *both* the `ring` and `aws-lc-rs` features of rustls through cargo
/// feature unification. When that happens rustls cannot auto-select a provider
/// and panics on the first TLS handshake with "Could not automatically
/// determine the process-level CryptoProvider". We pin `ring` (already a direct
/// dependency and Android-portable) explicitly at startup, before any TLS use.
fn install_crypto_provider() {
    use rustls::crypto::CryptoProvider;
    if CryptoProvider::get_default().is_none() {
        // Ignore the error: a concurrent thread may have installed it first.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

// ---------------------------------------------------------------------------
// Deep link handler for OAuth login callback
// ---------------------------------------------------------------------------

/// Parse `fms-app://login?access_token=...&user_id=...&username=...&refresh_token=...`
/// and store the tokens via auth_process_token.
async fn handle_deep_link_login(app: tauri::AppHandle, url: &str) -> Result<(), String> {
    // Parse query parameters from the URL.
    let query = url.split('?').nth(1).unwrap_or("");
    let params: std::collections::HashMap<String, String> = query
        .split('&')
        .filter_map(|pair| {
            let mut kv = pair.splitn(2, '=');
            let key = kv.next()?.to_string();
            let raw = kv.next().unwrap_or("");
            let val = match urlencoding::decode(raw) {
                Ok(cow) => cow.into_owned(),
                Err(_) => raw.to_string(),
            };
            Some((key, val))
        })
        .collect();

    let access_token = params.get("access_token").cloned().unwrap_or_default();
    let refresh_token = params.get("refresh_token").cloned().unwrap_or_default();
    let user_id = params.get("user_id").cloned().unwrap_or_default();
    let username = params.get("username").cloned().unwrap_or_default();

    if access_token.is_empty() {
        return Err("No access_token in deep link URL".to_string());
    }

    log::info!("[DeepLink] Processing login for user={}, id={}", username, user_id);
    auth::auth_process_token(app, access_token, refresh_token, user_id, username).await?;
    Ok(())
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_crypto_provider();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // Second instance launched (e.g., via fms-app:// deep link from browser).
            // Instead of starting a new app, forward the URL to the existing instance.
            log::info!("[SingleInstance] Second instance launched with args: {:?}", argv);
            // Find the deep link URL in args (Windows passes it as a command line arg)
            for arg in &argv {
                if arg.starts_with("fms-app://") {
                    let app_handle = app.clone();
                    let url = arg.clone();
                    tauri::async_runtime::spawn(async move {
                        if url.starts_with("fms-app://login") {
                            if let Err(e) = handle_deep_link_login(app_handle, &url).await {
                                log::error!("[SingleInstance] Deep link handling failed: {}", e);
                            }
                        }
                    });
                    break;
                }
            }
        }))
        .invoke_handler(tauri::generate_handler![
            greet,
            models::model_get_status,
            models::model_select_version,
            models::model_set_default,
            models::model_download,
            models::model_start,
            models::model_stop,
            models::model_delete,
            models::model_cancel_download,
            models::model_transcribe,
            settings::settings_get,
            settings::settings_set,
            settings::settings_get_global,
            settings::settings_set_global,
            settings::settings_get_workspace,
            settings::settings_set_workspace,
            settings::settings_set_role,
            settings::settings_adopt_cluster,
            settings::settings_forget_hub,
            settings::settings_pick_folder,
            datasets::dataset_list,
            datasets::dataset_list_dirs,
            datasets::dataset_add_dir,
            datasets::dataset_remove_dir,
            datasets::dataset_import,
            datasets::dataset_get,
            datasets::dataset_update,
            datasets::dataset_delete,
            datasets::dataset_create,
            datasets::dataset_import_media,
            datasets::dataset_generate_subtitles,
            datasets::dataset_generate_subtitle_single,
            datasets::dataset_delete_subtitles,
            datasets::dataset_generate_waveform,
            datasets::dataset_generate_waveform_single,
            datasets::dataset_delete_waveforms,
            datasets::dataset_advance_to_stage2,
            datasets::dataset_generate_database,
            datasets::dataset_delete_database,
            datasets::dataset_write_subtitles_to_db,
            datasets::tools::dataset_write_transcripts,
            datasets::tools::dataset_parse_book,
            datasets::dictation::align::dataset_align_cues,
            datasets::dictation::align::dataset_align_cues_transcript,
            datasets::dictation::adjust::dataset_adjust_cue_time,
            datasets::dictation::adjust::dataset_adjust_cue_time_single,
            datasets::dictation::adjust::dataset_check_subtitle_adjusted,
            datasets::dictation::adjust::dataset_sync_cue_times,
            datasets::dictation::adjust::dataset_sync_cue_times_word_level,
            datasets::dictation::dictation_list_media,
            datasets::dictation::dictation_get_data,
            datasets::dictation::listen_list_media,
            datasets::dictation::listen_get_media,
            datasets::dictation::listen_get_subtitles,
            datasets::dictation::listen_get_cues,
            datasets::dictation::listen_get_dictation,
            datasets::dictation::listen_get_dataset_dictation_status,
            datasets::dictation::listen_save_media,
            datasets::dictation::listen_rename_media,
            datasets::dictation::listen_save_cue,
            datasets::dictation::listen_delete_cue,
            datasets::dictation::listen_delete_media,
            datasets::dictation::listen_save_dictation,
            datasets::dictation::listen_get_waveform,
            datasets::dictation::dictation_add_cue_to_favorites,
            datasets::dictation::dictation_list_favorite_cues,
            // Version management
            datasets::dictation::subtitle_create_version,
            datasets::dictation::subtitle_finalize_version,
            datasets::dictation::subtitle_get_versions,
            datasets::dictation::subtitle_get_cues_at_version,
            datasets::dictation::subtitle_rollback_to_version,
            auth::auth_open_login,
            auth::auth_login_password,
            auth::auth_get_user,
            auth::auth_logout,
            ai::llm::llm_check_connection,
            ai::llm::llm_list_models,
            ai::llm::llm_pull_model,
            ai::llm::llm_delete_model,
            ai::llm::llm_chat,
            ai::llm::llm_chat_stream,
            edge_tts::edge_tts_list_voices,
            edge_tts::edge_tts_synthesize,
            edge_tts::edge_tts_preview,
            datasets::book::book_list,
            datasets::book::book_create,
            datasets::book::book_rename,
            datasets::book::book_delete,
            datasets::book::book_list_chapters,
            datasets::book::book_save_chapter,
            datasets::book::book_delete_chapter,
            datasets::book::book_list_sentences,
            datasets::book::book_save_sentence,
            datasets::book::book_save_sentences,
            datasets::book::book_delete_sentence,
            datasets::book::book_list_words,
            datasets::book::book_save_word,
            datasets::book::book_delete_word,
            datasets::book::book_write_audio,
            datasets::book::book_import_audio,
            datasets::book::book_delete_audio,
            datasets::read_aloud::read_aloud_list,
            datasets::read_aloud::read_aloud_create,
            datasets::read_aloud::read_aloud_update,
            datasets::read_aloud::read_aloud_delete,
            datasets::read_aloud::read_aloud_list_texts,
            datasets::read_aloud::read_aloud_save_text,
            datasets::read_aloud::read_aloud_delete_text,
            datasets::read_aloud::read_aloud_list_attempts,
            datasets::read_aloud::read_aloud_delete_attempt,
            datasets::read_aloud::read_aloud_score,
            datasets::read_aloud::read_aloud_submit,
            sync::server::web_service_get_status,
            sync::server::web_service_start,
            sync::server::web_service_stop,
            ocr::ocr_recognize,
            ocr::ocr_list_languages,
            xp::xp_get_user,
            xp::xp_get_history,
            xp::xp_award_dictation_cue,
            xp::xp_award_dictation_subtitle,
            xp::xp_award_dictation_media,
            xp::xp_award_reading_sentence,
            xp::xp_award_reading_chapter,
            models::index::model_index_get,
            models::index::model_index_refresh,
            capture::capture_screenshot,
            logger::log_get_history,
            logger::log_clear,
            logger::log_frontend_message,
            logger::log_get_file_path,
            logger::log_read_file_history,
            // Wiki datasets (syncable markdown trees) — cross-platform.
            datasets::wiki::wiki_dataset_list,
            datasets::wiki::wiki_dataset_create,
            datasets::wiki::wiki_dataset_import_dir,
            datasets::wiki::wiki_dataset_delete,
            datasets::wiki::wiki_dataset_list_dir,
            datasets::wiki::wiki_dataset_read_file,
            datasets::wiki::wiki_dataset_write_file,
            datasets::wiki::wiki_dataset_create_file,
            datasets::wiki::wiki_dataset_create_dir,
            datasets::wiki::wiki_dataset_delete_file,
            datasets::wiki::wiki_dataset_index,
            datasets::wiki::wiki_dataset_search,
            // Workspace management
            workspace::workspace_list,
            workspace::workspace_get_current,
            workspace::workspace_create,
            workspace::workspace_select,
            workspace::workspace_delete,
            workspace::workspace_rename,
            workspace::workspace_claim,
            workspace::workspace_set_auto_login,
            // Card dataset management
            datasets::cards::card_dataset_list,
            datasets::cards::card_dataset_create,
            datasets::cards::card_dataset_update,
            datasets::cards::card_dataset_add_subscriber,
            datasets::cards::card_dataset_remove_subscriber,
            datasets::cards::card_dataset_delete,
            datasets::cards::card_dataset_move,
            // Card CRUD
            datasets::cards::card_list,
            datasets::cards::card_get,
            datasets::cards::card_save,
            datasets::cards::card_delete,
            datasets::cards::card_fork,
            // Card tags
            datasets::cards::card_tag_list,
            datasets::cards::card_tag_save,
            datasets::cards::card_tag_delete,
            datasets::cards::card_set_tags,
            datasets::cards::card_get_tags,
            // Card review (SM-2)
            datasets::cards::card_test_get,
            datasets::cards::card_test_stats,
            datasets::cards::card_test_submit,
            // Card sync
            datasets::cards::card_sync_status,
            datasets::cards::card_sync_get_changes,
            // Card FTS search
            datasets::cards::card_search,
            datasets::cards::card_fts_rebuild,
            // Card dataset sync (bidirectional)
            datasets::cards::sync::card_sync_full,
            datasets::cards::sync::card_sync_all,
            // Simple words
            simple_words::simple_words_get_config,
            simple_words::simple_words_save_config,
            simple_words::simple_words_load_language,
            simple_words::simple_words_add_word,
            simple_words::simple_words_list,
            simple_words::simple_words_contains,
            simple_words::simple_words_filter,
            simple_words::simple_words_reload,
            // PC sync + discovery (available on desktop too, for testing).
            sync::client::dataset_sync_snapshot,
            sync::client::sync_run_round,
            sync::client::sync_backfill_history,
            sync::client::writeback_flush,
            sync::client::writeback_pending_count,
            sync::client::dataset_sync_state,
            sync::client::pc_check_status,
            sync::client::pc_list_datasets,
            sync::client::sync_status,
            sync::client::sync_forget_dataset,
            sync::client::pc_pair_start,
            sync::client::pc_pair_reset_identity,
            sync::discover::pc_discover,
            // Read-only hub wiki browse (works from any follower role/platform).
            sync::client::wiki_hub_list,
            sync::client::wiki_hub_list_dir,
            sync::client::wiki_hub_read_file,
            sync::client::wiki_hub_search,
            // Device pairing (PC owner side). Approval is deliberately only
            // ever granted by answering the confirm dialog.
            sync::pairing::pairing_list,
            sync::pairing::pairing_respond,
            sync::pairing::pairing_revoke,
            sync::pairing::pairing_remove_denied,
            // Cross-device chat (desktop serves the store directly).
            ai::chat::chat_list_messages,
            ai::chat::chat_send_message,
            ai::chat::chat_resolve_attachment,
            ai::chat::chat_save_attachment,
            // Goose ACP agent client (desktop only)
            ai::agent_acp::agent_connect,
            ai::agent_acp::agent_send_prompt,
            ai::agent_acp::agent_cancel,
            ai::agent_acp::agent_respond_permission,
            ai::agent_acp::agent_disconnect,
            ai::agent_acp::agent_status,
            ai::agent_acp::agent_report_ui_state,
        ])
        .manage(workspace::WorkspaceState::new())
        .manage(models::ModelState::new())
        .manage(settings::SettingsState::new())
        .manage(datasets::DatasetState::new())
        .manage(sync::server::WebServiceState::new())
        .manage(ocr::OcrState::new())
        .manage(simple_words::SimpleWordsState::new())
        .manage(ai::agent_acp::AcpClientState::new())
        .setup(|app| {
            // Anchor all persistent storage on the platform-correct base dir
            // (app-private on Android/iOS) before anything reads a path or
            // writes a log. Idempotent and a no-op relocation on desktop.
            app_paths::init(app.handle());
            // Re-seed state that was constructed pre-init with placeholder paths.
            app.state::<workspace::WorkspaceState>().reload_registry();
            app.state::<settings::SettingsState>().reload();
            app.state::<models::ModelState>().reload();
            // Restore the user's preferred default STT model (settings is global,
            // so it survives workspace switching).
            let persisted_model = app.state::<settings::SettingsState>().selected_model();
            app.state::<models::ModelState>().apply_persisted_default(&persisted_model);
            let log_buffer = logger::init_logger(app.handle().clone());
            app.handle().manage(log_buffer);
            log::info!("Application starting up");

            // ── Initialize workspaces ──
            {
                let ws_state = app.handle().state::<workspace::WorkspaceState>();
                match workspace::init_workspaces(&*ws_state) {
                    Ok(count) => {
                        log::info!("[Startup] {} workspace(s) initialized", count);
                        // Try to auto-select a workspace
                        if let Some(ws) = workspace::auto_select_workspace(&*ws_state) {
                            log::info!("[Startup] Auto-selected workspace: '{}' (uuid={})", ws.name, ws.uuid);
                            // Point settings state at the workspace (reloads workspace settings.json)
                            let settings_state = app.handle().state::<settings::SettingsState>();
                            settings_state.set_workspace_dir(Some(workspace::workspace_dir(&ws.uuid)));
                        } else {
                            // Multiple workspaces, need chooser
                            log::info!("[Startup] Showing workspace chooser");
                            let _ = app.emit("workspace-show-chooser", ());
                        }
                    }
                    Err(e) => {
                        log::error!("[Startup] Failed to initialize workspaces: {}", e);
                    }
                }
            }

            // Initialize unified model index (scan filesystem on first run)
            let model_root = models::index::model_root();
            let index_state = models::index::ModelIndexState::new(&model_root);
            {
                let idx = index_state.index.lock().unwrap();
                if idx.models.is_empty() {
                    drop(idx);
                    log::info!("[ModelIndex] Empty index, running initial scan...");
                    let scanned = models::index::scan_models(&model_root);
                    let mut idx = index_state.index.lock().unwrap();
                    *idx = scanned;
                    let _ = models::index::ModelIndexState::save(&*idx, &index_state.index_path);
                }
            }
            app.handle().manage(index_state);

            // Load simple words into memory (from persisted dataset selection)
            {
                let sw_state = app.handle().state::<simple_words::SimpleWordsState>();
                let settings_state = app.handle().state::<settings::SettingsState>();
                match simple_words::init_simple_words(&settings_state, &sw_state) {
                    Ok(count) => log::info!("[Startup] Loaded {} simple words", count),
                    Err(e) => log::error!("[Startup] Failed to load simple words: {}", e),
                }
            }

            // Auto-start web service (MCP + HTTP API) on port 35711
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                sync::server::auto_start(handle).await;
            });

            // Register deep link handler for OAuth login callback.
            // Register the fms-app:// scheme with the OS so the browser can redirect to it.
            match app.deep_link().register_all() {
                Ok(_) => log::info!("[DeepLink] Successfully registered all schemes"),
                Err(e) => log::error!("[DeepLink] Failed to register schemes: {}", e),
            }
            // Also explicitly register fms-app scheme
            if let Err(e) = app.deep_link().register("fms-app") {
                log::error!("[DeepLink] Failed to register fms-app scheme: {}", e);
            } else {
                log::info!("[DeepLink] Registered fms-app:// protocol handler");
            }

            let dl_handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    let url_str = url.to_string();
                    log::info!("[DeepLink] Received URL: {}", url_str);
                    if url_str.starts_with("fms-app://login") {
                        let handle = dl_handle.clone();
                        let url_str = url_str.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = handle_deep_link_login(handle, &url_str).await {
                                log::error!("[DeepLink] Login failed: {}", e);
                            }
                        });
                    }
                }
            });

            // Also check if the app was launched via a deep link.
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for url in urls {
                    let url_str = url.to_string();
                    if url_str.starts_with("fms-app://login") {
                        let handle = app.handle().clone();
                        let url_str = url_str.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = handle_deep_link_login(handle, &url_str).await {
                                log::error!("[DeepLink] Login failed: {}", e);
                            }
                        });
                    }
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------------------------------------------------------------------------
// Mobile (thin-client) entry point.
//
// Built with `--no-default-features` (i.e. the `desktop` feature is off). It
// registers only the command set that compiles and is useful on Android: the
// local SQLite-backed learning features (dictation, cards, wiki, book, XP,
// simple words, workspace/settings) plus the PC snapshot-sync + discovery
// client. The heavy desktop subsystems (dataset generation pipeline, OCR,
// capture, tools, and the web_service server) are excluded entirely.
//
// Local STT (the `model_*` commands, ONNX-only via the `stt` feature) is
// opt-in on mobile through `build.features` in tauri.android.conf.json — it is
// currently there to verify the ONNX Runtime stack cross-compiles and runs on
// Android. transcribe-cpp (Whisper GGUF) stays desktop-only.
// ---------------------------------------------------------------------------

// Shared mobile command list. `generate_handler!` cannot contain cfg'd
// entries, so the STT commands are appended via `$($extra)` only in the
// `stt`-enabled build.
#[cfg(not(feature = "desktop"))]
macro_rules! mobile_invoke_handler {
    ($($extra:path),* $(,)?) => {
        tauri::generate_handler![
            greet,
            settings::settings_get,
            settings::settings_set,
            settings::settings_get_global,
            settings::settings_set_global,
            settings::settings_get_workspace,
            settings::settings_set_workspace,
            settings::settings_set_role,
            settings::settings_adopt_cluster,
            settings::settings_forget_hub,
            settings::settings_pick_folder,
            datasets::dataset_list,
            datasets::dataset_list_dirs,
            datasets::dataset_add_dir,
            datasets::dataset_remove_dir,
            datasets::dataset_get,
            datasets::dataset_update,
            datasets::dataset_delete,
            datasets::dataset_create,
            datasets::dictation::dictation_list_media,
            datasets::dictation::dictation_get_data,
            datasets::dictation::listen_list_media,
            datasets::dictation::listen_get_media,
            datasets::dictation::listen_get_subtitles,
            datasets::dictation::listen_get_cues,
            datasets::dictation::listen_get_dictation,
            datasets::dictation::listen_get_dataset_dictation_status,
            datasets::dictation::listen_save_media,
            datasets::dictation::listen_rename_media,
            datasets::dictation::listen_save_cue,
            datasets::dictation::listen_delete_cue,
            datasets::dictation::listen_delete_media,
            datasets::dictation::listen_save_dictation,
            datasets::dictation::listen_get_waveform,
            datasets::dictation::dictation_add_cue_to_favorites,
            datasets::dictation::dictation_list_favorite_cues,
            datasets::dictation::subtitle_create_version,
            datasets::dictation::subtitle_finalize_version,
            datasets::dictation::subtitle_get_versions,
            datasets::dictation::subtitle_get_cues_at_version,
            datasets::dictation::subtitle_rollback_to_version,
            auth::auth_open_login,
            auth::auth_login_password,
            auth::auth_get_user,
            auth::auth_logout,
            ai::llm::llm_check_connection,
            ai::llm::llm_list_models,
            ai::llm::llm_pull_model,
            ai::llm::llm_delete_model,
            ai::llm::llm_chat,
            ai::llm::llm_chat_stream,
            edge_tts::edge_tts_list_voices,
            edge_tts::edge_tts_synthesize,
            edge_tts::edge_tts_preview,
            datasets::book::book_list,
            datasets::book::book_create,
            datasets::book::book_rename,
            datasets::book::book_delete,
            datasets::book::book_list_chapters,
            datasets::book::book_save_chapter,
            datasets::book::book_delete_chapter,
            datasets::book::book_list_sentences,
            datasets::book::book_save_sentence,
            datasets::book::book_save_sentences,
            datasets::book::book_delete_sentence,
            datasets::book::book_list_words,
            datasets::book::book_save_word,
            datasets::book::book_delete_word,
            datasets::book::book_write_audio,
            datasets::book::book_import_audio,
            datasets::book::book_delete_audio,
            datasets::read_aloud::read_aloud_list,
            datasets::read_aloud::read_aloud_create,
            datasets::read_aloud::read_aloud_update,
            datasets::read_aloud::read_aloud_delete,
            datasets::read_aloud::read_aloud_list_texts,
            datasets::read_aloud::read_aloud_save_text,
            datasets::read_aloud::read_aloud_delete_text,
            datasets::read_aloud::read_aloud_list_attempts,
            datasets::read_aloud::read_aloud_delete_attempt,
            datasets::read_aloud::read_aloud_score,
            datasets::read_aloud::read_aloud_submit,
            xp::xp_get_user,
            xp::xp_get_history,
            xp::xp_award_dictation_cue,
            xp::xp_award_dictation_subtitle,
            xp::xp_award_dictation_media,
            xp::xp_award_reading_sentence,
            xp::xp_award_reading_chapter,
            logger::log_get_history,
            logger::log_clear,
            logger::log_frontend_message,
            logger::log_get_file_path,
            logger::log_read_file_history,
            // Wiki datasets (syncable markdown trees) — cross-platform.
            datasets::wiki::wiki_dataset_list,
            datasets::wiki::wiki_dataset_create,
            datasets::wiki::wiki_dataset_import_dir,
            datasets::wiki::wiki_dataset_delete,
            datasets::wiki::wiki_dataset_list_dir,
            datasets::wiki::wiki_dataset_read_file,
            datasets::wiki::wiki_dataset_write_file,
            datasets::wiki::wiki_dataset_create_file,
            datasets::wiki::wiki_dataset_create_dir,
            datasets::wiki::wiki_dataset_delete_file,
            datasets::wiki::wiki_dataset_index,
            datasets::wiki::wiki_dataset_search,
            workspace::workspace_list,
            workspace::workspace_get_current,
            workspace::workspace_create,
            workspace::workspace_select,
            workspace::workspace_delete,
            workspace::workspace_rename,
            workspace::workspace_claim,
            workspace::workspace_set_auto_login,
            datasets::cards::card_dataset_list,
            datasets::cards::card_dataset_create,
            datasets::cards::card_dataset_update,
            datasets::cards::card_dataset_add_subscriber,
            datasets::cards::card_dataset_remove_subscriber,
            datasets::cards::card_dataset_delete,
            datasets::cards::card_dataset_move,
            datasets::cards::card_list,
            datasets::cards::card_get,
            datasets::cards::card_save,
            datasets::cards::card_delete,
            datasets::cards::card_fork,
            datasets::cards::card_tag_list,
            datasets::cards::card_tag_save,
            datasets::cards::card_tag_delete,
            datasets::cards::card_set_tags,
            datasets::cards::card_get_tags,
            datasets::cards::card_test_get,
            datasets::cards::card_test_stats,
            datasets::cards::card_test_submit,
            datasets::cards::card_sync_status,
            datasets::cards::card_sync_get_changes,
            datasets::cards::card_search,
            datasets::cards::card_fts_rebuild,
            datasets::cards::sync::card_sync_full,
            datasets::cards::sync::card_sync_all,
            simple_words::simple_words_get_config,
            simple_words::simple_words_save_config,
            simple_words::simple_words_load_language,
            simple_words::simple_words_add_word,
            simple_words::simple_words_list,
            simple_words::simple_words_contains,
            simple_words::simple_words_filter,
            simple_words::simple_words_reload,
            // PC sync + discovery client.
            sync::client::dataset_sync_snapshot,
            sync::client::sync_run_round,
            sync::client::sync_backfill_history,
            sync::client::writeback_flush,
            sync::client::writeback_pending_count,
            sync::client::dataset_sync_state,
            sync::client::pc_check_status,
            sync::client::pc_list_datasets,
            sync::client::sync_status,
            sync::client::sync_forget_dataset,
            sync::client::pc_pair_start,
            sync::client::pc_pair_reset_identity,
            sync::discover::pc_discover,
            // Read-only hub wiki browse (works from any follower role/platform).
            sync::client::wiki_hub_list,
            sync::client::wiki_hub_list_dir,
            sync::client::wiki_hub_read_file,
            sync::client::wiki_hub_search,
            // Cross-device chat: same command names as desktop, relayed to the PC.
            ai::chat::chat_list_messages,
            ai::chat::chat_send_message,
            ai::chat::chat_resolve_attachment,
            ai::chat::chat_save_attachment,
            $($extra),*
        ]
    };
}
#[cfg(not(feature = "desktop"))]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_crypto_provider();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_deep_link::init())
        .manage(workspace::WorkspaceState::new())
        .manage(settings::SettingsState::new())
        .manage(datasets::DatasetState::new())
        .manage(simple_words::SimpleWordsState::new());

    // STT builds additionally manage ModelState and register the `model_*`
    // commands (see the mobile section header comment above).
    #[cfg(feature = "stt")]
    let builder = builder
        .manage(models::ModelState::new())
        .invoke_handler(mobile_invoke_handler!(
            models::model_get_status,
            models::model_select_version,
            models::model_set_default,
            models::model_download,
            models::model_start,
            models::model_stop,
            models::model_delete,
            models::model_cancel_download,
            models::model_transcribe,
            models::index::model_index_get,
            models::index::model_index_refresh
        ));
    #[cfg(not(feature = "stt"))]
    let builder = builder.invoke_handler(mobile_invoke_handler!());

    builder
        .setup(|app| {
            // Anchor all persistent storage on the platform-correct base dir
            // (app-private on Android/iOS) before anything reads a path or
            // writes a log. Idempotent and a no-op relocation on desktop.
            app_paths::init(app.handle());
            // Re-seed state that was constructed pre-init with placeholder paths.
            app.state::<workspace::WorkspaceState>().reload_registry();
            app.state::<settings::SettingsState>().reload();
            // ModelState is only managed on STT builds (see builder above).
            #[cfg(feature = "stt")]
            app.state::<models::ModelState>().reload();
            #[cfg(feature = "stt")]
            {
                let persisted_model = app.state::<settings::SettingsState>().selected_model();
                app.state::<models::ModelState>().apply_persisted_default(&persisted_model);
            }
            let log_buffer = logger::init_logger(app.handle().clone());
            app.handle().manage(log_buffer);
            log::info!("Application starting up (mobile)");

            // ── Initialize workspaces ──
            {
                let ws_state = app.handle().state::<workspace::WorkspaceState>();
                match workspace::init_workspaces(&*ws_state) {
                    Ok(count) => {
                        log::info!("[Startup] {} workspace(s) initialized", count);
                        if let Some(ws) = workspace::auto_select_workspace(&*ws_state) {
                            log::info!("[Startup] Auto-selected workspace: '{}' (uuid={})", ws.name, ws.uuid);
                            let settings_state = app.handle().state::<settings::SettingsState>();
                            settings_state.set_workspace_dir(Some(workspace::workspace_dir(&ws.uuid)));
                        } else {
                            log::info!("[Startup] Showing workspace chooser");
                            let _ = app.emit("workspace-show-chooser", ());
                        }
                    }
                    Err(e) => {
                        log::error!("[Startup] Failed to initialize workspaces: {}", e);
                    }
                }
            }

            // ── Initialize unified model index (STT builds only; scan filesystem
            // on first run) ──
            #[cfg(feature = "stt")]
            {
                let model_root = models::index::model_root();
                let index_state = models::index::ModelIndexState::new(&model_root);
                {
                    let idx = index_state.index.lock().unwrap();
                    if idx.models.is_empty() {
                        drop(idx);
                        log::info!("[ModelIndex] Empty index, running initial scan...");
                        let scanned = models::index::scan_models(&model_root);
                        let mut idx = index_state.index.lock().unwrap();
                        *idx = scanned;
                        let _ = models::index::ModelIndexState::save(&*idx, &index_state.index_path);
                    }
                }
                app.handle().manage(index_state);
            }

            // Load simple words into memory.
            {
                let sw_state = app.handle().state::<simple_words::SimpleWordsState>();
                let settings_state = app.handle().state::<settings::SettingsState>();
                match simple_words::init_simple_words(&settings_state, &sw_state) {
                    Ok(count) => log::info!("[Startup] Loaded {} simple words", count),
                    Err(e) => log::error!("[Startup] Failed to load simple words: {}", e),
                }
            }

            // On Android the fms-app:// scheme is registered by the OS at install
            // time via the AndroidManifest intent-filter, so there is no runtime
            // register_all()/register() call here (register_all() is desktop-only and
            // register() is a no-op on mobile). We only attach the on_open_url handler
            // below to receive incoming deep links (login callback + wiki nav).

            let dl_handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    let url_str = url.to_string();
                    log::info!("[DeepLink] Received URL: {}", url_str);
                    if url_str.starts_with("fms-app://login") {
                        let handle = dl_handle.clone();
                        let url_str = url_str.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = handle_deep_link_login(handle, &url_str).await {
                                log::error!("[DeepLink] Login failed: {}", e);
                            }
                        });
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
