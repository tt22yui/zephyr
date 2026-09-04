// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
pub mod auth;
pub mod collect;
pub mod config;
pub mod endpoint;
pub mod info;
pub mod output;
pub mod pull;
pub mod registry;
pub mod search;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![pull::pull_image, search::search_image])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
