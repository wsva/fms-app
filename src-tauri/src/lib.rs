mod audio;
mod auth;
mod dataset;
mod dictation;
mod model_download;
mod model_list;
mod settings;
mod model;
mod tools;

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
            dataset::dataset_import,
            dataset::dataset_get,
            dataset::dataset_update,
            dataset::dataset_delete,
            dataset::dataset_generate_subtitles,
            dataset::dataset_delete_subtitles,
            dataset::dataset_generate_waveform,
            dataset::dataset_delete_waveforms,
            dataset::dataset_advance_to_stage2,
            dataset::dataset_generate_database,
            dataset::dataset_delete_database,
            tools::dataset_write_transcripts,
            tools::dataset_parse_book,
            tools::dataset_align_cues,
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
            dictation::listen_save_dictation,
            dictation::listen_get_waveform,
            auth::auth_login,
            auth::auth_get_user,
            auth::auth_logout,
        ])
        .manage(model::ModelState::new())
        .manage(settings::SettingsState::new())
        .manage(dataset::DatasetState::new())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
