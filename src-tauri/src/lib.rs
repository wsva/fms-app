mod app_paths;
mod audio;
#[cfg(feature = "desktop")]
mod adjust;
#[cfg(feature = "desktop")]
mod align;
mod auth;
mod book;
mod cards;
mod cards_sync;
mod dataset;
mod db;
mod dictation;
mod llm;
mod logger;
#[cfg(feature = "stt")]
mod model;
#[cfg(feature = "stt")]
mod model_download;
#[cfg(feature = "stt")]
mod model_list;
#[cfg(feature = "stt")]
mod model_list_stt;
#[cfg(feature = "desktop")]
mod mcp;
mod settings;
#[cfg(feature = "desktop")]
mod tools;
mod edge_tts;
#[cfg(feature = "desktop")]
mod web_service;
#[cfg(feature = "desktop")]
mod rest;
#[cfg(feature = "desktop")]
mod pairing;
#[cfg(feature = "desktop")]
mod capture;
#[cfg(feature = "desktop")]
mod ocr;
mod xp;
mod simple_words;
mod wiki;
mod workspace;
mod sync;
mod discover;

// Unified model index
#[cfg(feature = "stt")]
mod model_index;

use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

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

// ---------------------------------------------------------------------------
// Deep link handler for wiki navigation
// ---------------------------------------------------------------------------

/// Parse `fms-app://wiki/path/to/file.md` and emit event to navigate to that wiki page.
fn handle_deep_link_wiki(app: tauri::AppHandle, url: &str) -> Result<(), String> {
    use tauri::Emitter;

    // Extract path after "fms-app://wiki/"
    let wiki_prefix = "fms-app://wiki/";
    if !url.starts_with(wiki_prefix) {
        return Err("Invalid wiki deep link URL".to_string());
    }

    let relative_path = &url[wiki_prefix.len()..];
    if relative_path.is_empty() {
        return Err("No file path in wiki deep link URL".to_string());
    }

    // URL-decode the path
    let decoded_path = urlencoding::decode(relative_path)
        .map(|cow| cow.into_owned())
        .unwrap_or_else(|_| relative_path.to_string());

    log::info!("[DeepLink] Wiki navigation to: {}", decoded_path);

    // Emit event for frontend to handle
    app.emit("wiki-navigate", &decoded_path)
        .map_err(|e| format!("Failed to emit wiki-navigate event: {}", e))?;

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
                        } else if url.starts_with("fms-app://wiki") {
                            if let Err(e) = handle_deep_link_wiki(app_handle, &url) {
                                log::error!("[SingleInstance] Wiki deep link handling failed: {}", e);
                            }
                        }
                    });
                    break;
                }
            }
        }))
        .invoke_handler(tauri::generate_handler![
            greet,
            model::model_get_status,
            model::model_select_version,
            model::model_set_default,
            model::model_download,
            model::model_start,
            model::model_stop,
            model::model_delete,
            model::model_cancel_download,
            model::model_transcribe,
            settings::settings_get,
            settings::settings_set,
            settings::settings_get_global,
            settings::settings_set_global,
            settings::settings_get_workspace,
            settings::settings_set_workspace,
            settings::settings_pick_folder,
            dataset::dataset_list,
            dataset::dataset_list_dirs,
            dataset::dataset_add_dir,
            dataset::dataset_remove_dir,
            dataset::dataset_import,
            dataset::dataset_get,
            dataset::dataset_update,
            dataset::dataset_delete,
            dataset::dataset_create,
            dataset::dataset_import_media,
            dataset::dataset_generate_subtitles,
            dataset::dataset_generate_subtitle_single,
            dataset::dataset_delete_subtitles,
            dataset::dataset_generate_waveform,
            dataset::dataset_generate_waveform_single,
            dataset::dataset_delete_waveforms,
            dataset::dataset_advance_to_stage2,
            dataset::dataset_generate_database,
            dataset::dataset_delete_database,
            dataset::dataset_write_subtitles_to_db,
            tools::dataset_write_transcripts,
            tools::dataset_parse_book,
            align::dataset_align_cues,
            align::dataset_align_cues_transcript,
            adjust::dataset_adjust_cue_time,
            adjust::dataset_adjust_cue_time_single,
            adjust::dataset_check_subtitle_adjusted,
            adjust::dataset_sync_cue_times,
            adjust::dataset_sync_cue_times_word_level,
            dictation::dictation_list_media,
            dictation::dictation_get_data,
            dictation::listen_list_media,
            dictation::listen_get_media,
            dictation::listen_get_subtitles,
            dictation::listen_get_cues,
            dictation::listen_get_dictation,
            dictation::listen_get_dataset_dictation_status,
            dictation::listen_save_media,
            dictation::listen_rename_media,
            dictation::listen_save_cue,
            dictation::listen_delete_cue,
            dictation::listen_delete_media,
            dictation::listen_save_dictation,
            dictation::listen_get_waveform,
            dictation::dictation_add_cue_to_favorites,
            dictation::dictation_list_favorite_cues,
            // Version management
            dictation::subtitle_create_version,
            dictation::subtitle_finalize_version,
            dictation::subtitle_get_versions,
            dictation::subtitle_get_cues_at_version,
            dictation::subtitle_rollback_to_version,
            auth::auth_open_login,
            auth::auth_login_password,
            auth::auth_get_user,
            auth::auth_logout,
            llm::llm_check_connection,
            llm::llm_list_models,
            llm::llm_pull_model,
            llm::llm_delete_model,
            llm::llm_chat,
            edge_tts::edge_tts_list_voices,
            edge_tts::edge_tts_synthesize,
            edge_tts::edge_tts_preview,
            book::book_list,
            book::book_create,
            book::book_rename,
            book::book_delete,
            book::book_list_chapters,
            book::book_save_chapter,
            book::book_delete_chapter,
            book::book_list_sentences,
            book::book_save_sentence,
            book::book_save_sentences,
            book::book_delete_sentence,
            book::book_list_words,
            book::book_save_word,
            book::book_delete_word,
            book::book_write_audio,
            book::book_import_audio,
            book::book_delete_audio,
            web_service::web_service_get_status,
            web_service::web_service_start,
            web_service::web_service_stop,
            ocr::ocr_recognize,
            ocr::ocr_list_languages,
            xp::xp_get_user,
            xp::xp_get_history,
            xp::xp_award_dictation_cue,
            xp::xp_award_dictation_subtitle,
            xp::xp_award_dictation_media,
            xp::xp_award_reading_sentence,
            xp::xp_award_reading_chapter,
            model_index::model_index_get,
            model_index::model_index_refresh,
            capture::capture_screenshot,
            logger::log_get_history,
            logger::log_clear,
            logger::log_frontend_message,
            wiki::wiki_list_dirs,
            wiki::wiki_list_dir,
            wiki::wiki_read_file,
            wiki::wiki_write_file,
            wiki::wiki_delete_file,
            wiki::wiki_search,
            wiki::wiki_index,
            wiki::wiki_add_dir,
            wiki::wiki_remove_dir,
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
            cards::card_dataset_list,
            cards::card_dataset_create,
            cards::card_dataset_update,
            cards::card_dataset_add_subscriber,
            cards::card_dataset_remove_subscriber,
            cards::card_dataset_delete,
            cards::card_dataset_move,
            // Card CRUD
            cards::card_list,
            cards::card_get,
            cards::card_save,
            cards::card_delete,
            cards::card_fork,
            // Card tags
            cards::card_tag_list,
            cards::card_tag_save,
            cards::card_tag_delete,
            cards::card_set_tags,
            cards::card_get_tags,
            // Card review (SM-2)
            cards::card_test_get,
            cards::card_test_stats,
            cards::card_test_submit,
            // Card sync
            cards::card_sync_status,
            cards::card_sync_get_changes,
            // Card FTS search
            cards::card_search,
            cards::card_fts_rebuild,
            // Card dataset sync (bidirectional)
            cards_sync::card_sync_full,
            cards_sync::card_sync_all,
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
            sync::dataset_sync_snapshot,
            sync::writeback_flush,
            sync::writeback_pending_count,
            sync::dataset_sync_state,
            sync::pc_check_status,
            sync::pc_list_datasets,
            sync::pc_pair_start,
            sync::pc_pair_reset_identity,
            discover::pc_discover,
            // Device pairing (PC owner side). Approval is deliberately only
            // ever granted by answering the confirm dialog.
            pairing::pairing_list,
            pairing::pairing_respond,
            pairing::pairing_revoke,
            pairing::pairing_remove_denied,
        ])
        .manage(workspace::WorkspaceState::new())
        .manage(model::ModelState::new())
        .manage(settings::SettingsState::new())
        .manage(dataset::DatasetState::new())
        .manage(web_service::WebServiceState::new())
        .manage(ocr::OcrState::new())
        .manage(simple_words::SimpleWordsState::new())
        .setup(|app| {
            // Anchor all persistent storage on the platform-correct base dir
            // (app-private on Android/iOS) before anything reads a path or
            // writes a log. Idempotent and a no-op relocation on desktop.
            app_paths::init(app.handle());
            // Re-seed state that was constructed pre-init with placeholder paths.
            app.state::<workspace::WorkspaceState>().reload_registry();
            app.state::<settings::SettingsState>().reload();
            app.state::<model::ModelState>().reload();
            // Restore the user's preferred default STT model (settings is global,
            // so it survives workspace switching).
            let persisted_model = app.state::<settings::SettingsState>().selected_model();
            app.state::<model::ModelState>().apply_persisted_default(&persisted_model);
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
            let model_root = model_index::model_root();
            let index_state = model_index::ModelIndexState::new(&model_root);
            {
                let idx = index_state.index.lock().unwrap();
                if idx.models.is_empty() {
                    drop(idx);
                    log::info!("[ModelIndex] Empty index, running initial scan...");
                    let scanned = model_index::scan_models(&model_root);
                    let mut idx = index_state.index.lock().unwrap();
                    *idx = scanned;
                    let _ = model_index::ModelIndexState::save(&*idx, &index_state.index_path);
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
                web_service::auto_start(handle).await;
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
                    } else if url_str.starts_with("fms-app://wiki") {
                        let handle = dl_handle.clone();
                        let url_str = url_str.clone();
                        if let Err(e) = handle_deep_link_wiki(handle, &url_str) {
                            log::error!("[DeepLink] Wiki navigation failed: {}", e);
                        }
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
                    } else if url_str.starts_with("fms-app://wiki") {
                        let handle = app.handle().clone();
                        let url_str = url_str.clone();
                        if let Err(e) = handle_deep_link_wiki(handle, &url_str) {
                            log::error!("[DeepLink] Wiki navigation failed: {}", e);
                        }
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
            settings::settings_pick_folder,
            dataset::dataset_list,
            dataset::dataset_list_dirs,
            dataset::dataset_add_dir,
            dataset::dataset_remove_dir,
            dataset::dataset_get,
            dataset::dataset_update,
            dataset::dataset_delete,
            dataset::dataset_create,
            dictation::dictation_list_media,
            dictation::dictation_get_data,
            dictation::listen_list_media,
            dictation::listen_get_media,
            dictation::listen_get_subtitles,
            dictation::listen_get_cues,
            dictation::listen_get_dictation,
            dictation::listen_get_dataset_dictation_status,
            dictation::listen_save_media,
            dictation::listen_rename_media,
            dictation::listen_save_cue,
            dictation::listen_delete_cue,
            dictation::listen_delete_media,
            dictation::listen_save_dictation,
            dictation::listen_get_waveform,
            dictation::dictation_add_cue_to_favorites,
            dictation::dictation_list_favorite_cues,
            dictation::subtitle_create_version,
            dictation::subtitle_finalize_version,
            dictation::subtitle_get_versions,
            dictation::subtitle_get_cues_at_version,
            dictation::subtitle_rollback_to_version,
            auth::auth_open_login,
            auth::auth_login_password,
            auth::auth_get_user,
            auth::auth_logout,
            llm::llm_check_connection,
            llm::llm_list_models,
            llm::llm_pull_model,
            llm::llm_delete_model,
            llm::llm_chat,
            edge_tts::edge_tts_list_voices,
            edge_tts::edge_tts_synthesize,
            edge_tts::edge_tts_preview,
            book::book_list,
            book::book_create,
            book::book_rename,
            book::book_delete,
            book::book_list_chapters,
            book::book_save_chapter,
            book::book_delete_chapter,
            book::book_list_sentences,
            book::book_save_sentence,
            book::book_save_sentences,
            book::book_delete_sentence,
            book::book_list_words,
            book::book_save_word,
            book::book_delete_word,
            book::book_write_audio,
            book::book_import_audio,
            book::book_delete_audio,
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
            wiki::wiki_list_dirs,
            wiki::wiki_list_dir,
            wiki::wiki_read_file,
            wiki::wiki_write_file,
            wiki::wiki_delete_file,
            wiki::wiki_search,
            wiki::wiki_index,
            wiki::wiki_add_dir,
            wiki::wiki_remove_dir,
            workspace::workspace_list,
            workspace::workspace_get_current,
            workspace::workspace_create,
            workspace::workspace_select,
            workspace::workspace_delete,
            workspace::workspace_rename,
            workspace::workspace_claim,
            workspace::workspace_set_auto_login,
            cards::card_dataset_list,
            cards::card_dataset_create,
            cards::card_dataset_update,
            cards::card_dataset_add_subscriber,
            cards::card_dataset_remove_subscriber,
            cards::card_dataset_delete,
            cards::card_dataset_move,
            cards::card_list,
            cards::card_get,
            cards::card_save,
            cards::card_delete,
            cards::card_fork,
            cards::card_tag_list,
            cards::card_tag_save,
            cards::card_tag_delete,
            cards::card_set_tags,
            cards::card_get_tags,
            cards::card_test_get,
            cards::card_test_stats,
            cards::card_test_submit,
            cards::card_sync_status,
            cards::card_sync_get_changes,
            cards::card_search,
            cards::card_fts_rebuild,
            cards_sync::card_sync_full,
            cards_sync::card_sync_all,
            simple_words::simple_words_get_config,
            simple_words::simple_words_save_config,
            simple_words::simple_words_load_language,
            simple_words::simple_words_add_word,
            simple_words::simple_words_list,
            simple_words::simple_words_contains,
            simple_words::simple_words_filter,
            simple_words::simple_words_reload,
            // PC sync + discovery client.
            sync::dataset_sync_snapshot,
            sync::writeback_flush,
            sync::writeback_pending_count,
            sync::dataset_sync_state,
            sync::pc_check_status,
            sync::pc_list_datasets,
            sync::pc_pair_start,
            sync::pc_pair_reset_identity,
            discover::pc_discover,
            $($extra),*
        ]
    };
}
#[cfg(not(feature = "desktop"))]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_deep_link::init())
        .manage(workspace::WorkspaceState::new())
        .manage(settings::SettingsState::new())
        .manage(dataset::DatasetState::new())
        .manage(simple_words::SimpleWordsState::new());

    // STT builds additionally manage ModelState and register the `model_*`
    // commands (see the mobile section header comment above).
    #[cfg(feature = "stt")]
    let builder = builder
        .manage(model::ModelState::new())
        .invoke_handler(mobile_invoke_handler!(
            model::model_get_status,
            model::model_select_version,
            model::model_set_default,
            model::model_download,
            model::model_start,
            model::model_stop,
            model::model_delete,
            model::model_cancel_download,
            model::model_transcribe,
            model_index::model_index_get,
            model_index::model_index_refresh
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
            app.state::<model::ModelState>().reload();
            #[cfg(feature = "stt")]
            {
                let persisted_model = app.state::<settings::SettingsState>().selected_model();
                app.state::<model::ModelState>().apply_persisted_default(&persisted_model);
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
                let model_root = model_index::model_root();
                let index_state = model_index::ModelIndexState::new(&model_root);
                {
                    let idx = index_state.index.lock().unwrap();
                    if idx.models.is_empty() {
                        drop(idx);
                        log::info!("[ModelIndex] Empty index, running initial scan...");
                        let scanned = model_index::scan_models(&model_root);
                        let mut idx = index_state.index.lock().unwrap();
                        *idx = scanned;
                        let _ = model_index::ModelIndexState::save(&*idx, &index_state.index_path);
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
                    } else if url_str.starts_with("fms-app://wiki") {
                        let handle = dl_handle.clone();
                        let url_str = url_str.clone();
                        if let Err(e) = handle_deep_link_wiki(handle, &url_str) {
                            log::error!("[DeepLink] Wiki navigation failed: {}", e);
                        }
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
