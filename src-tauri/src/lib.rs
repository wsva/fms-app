mod audio;
mod adjust;
mod align;
mod auth;
mod book;
mod dataset;
mod dictation;
mod llm;
mod llm_model_list;
mod logger;
mod model;
mod model_download;
mod model_list;
mod mcp;
mod settings;
mod tools;
mod edge_tts;
mod web_service;
mod capture;
mod ocr;

use tauri::Manager;

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
            adjust::dataset_sync_cue_times,
            adjust::dataset_sync_cue_times_word_level,
            dictation::dictation_list_media,
            dictation::dictation_get_data,
            dictation::listen_list_media,
            dictation::listen_get_media,
            dictation::listen_get_subtitles,
            dictation::listen_get_cues,
            dictation::listen_get_dictation,
            dictation::listen_save_media,
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
            auth::auth_login,
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
            capture::capture_screenshot,
            logger::log_get_history,
            logger::log_clear,
        ])
        .manage(model::ModelState::new())
        .manage(settings::SettingsState::new())
        .manage(dataset::DatasetState::new())
        .manage(web_service::WebServiceState::new())
        .manage(ocr::OcrState::new())
        .setup(|app| {
            let log_buffer = logger::init_logger(app.handle().clone());
            app.handle().manage(log_buffer);
            log::info!("Application starting up");

            // Auto-start web service (MCP + HTTP API) on port 8787
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                web_service::auto_start(handle).await;
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
