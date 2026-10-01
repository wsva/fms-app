mod audio;
mod adjust;
mod align;
mod auth;
mod book;
mod dataset;
mod db;
mod dictation;
mod llm;
mod logger;
mod model;
mod model_download;
mod model_list;
mod model_list_stt;
mod mcp;
mod settings;
mod tools;
mod edge_tts;
mod web_service;
mod capture;
mod ocr;
mod xp;
mod wiki;
mod workspace;

// Unified model index
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
            model::model_download,
            model::model_start,
            model::model_stop,
            model::model_delete,
            model::model_cancel_download,
            model::model_transcribe,
            settings::settings_get,
            settings::settings_set,
            settings::settings_pick_folder,
            dataset::dataset_list,
            dataset::dataset_list_locations,
            dataset::dataset_add_location,
            dataset::dataset_remove_location,
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
            // Version management
            dictation::subtitle_create_version,
            dictation::subtitle_finalize_version,
            dictation::subtitle_get_versions,
            dictation::subtitle_get_cues_at_version,
            dictation::subtitle_rollback_to_version,
            auth::auth_open_login,
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
        ])
        .manage(workspace::WorkspaceState::new())
        .manage(model::ModelState::new())
        .manage(settings::SettingsState::new())
        .manage(dataset::DatasetState::new())
        .manage(web_service::WebServiceState::new())
        .manage(ocr::OcrState::new())
        .setup(|app| {
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

            // Auto-start web service (MCP + HTTP API) on port 8787
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
