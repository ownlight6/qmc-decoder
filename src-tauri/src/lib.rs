//! QMC Decoder desktop application (Tauri shell).
//!
//! Registers the [`commands`] as invoke handlers. Native file drag-and-drop is
//! delivered by Tauri to the web frontend as a `tauri://drag-drop` event,
//! which the frontend consumes via `getCurrentWebview().onDragDropEvent`.

mod commands;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::decrypt_paths,
            commands::get_file_info,
            commands::fetch_ekey_musicex,
            commands::check_credentials,
            commands::pick_files,
            commands::pick_folder,
            commands::inspect_paths,
            commands::get_default_download_dir,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run qmc-decoder app");
}