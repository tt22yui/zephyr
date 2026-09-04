// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

use tauri::Manager;

pub mod auth;
pub mod collect;
pub mod config;
pub mod endpoint;
pub mod info;
pub mod inspect;
pub mod output;
pub mod pull;
pub mod registry;
pub mod search;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            pull::pull_image,
            search::search_image,
            inspect::inspect_image,
            get_download_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 返回系统「下载」目录；解析失败时返回空串，由前端回退到当前目录。
#[tauri::command]
fn get_download_dir(app: tauri::AppHandle) -> String {
    app.path()
        .download_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}
