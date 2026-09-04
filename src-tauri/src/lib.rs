// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

use tauri::Manager;

pub mod auth;
pub mod cancel;
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
        .setup(|app| {
            // 启动时定位主窗口：默认居中；若窗口尺寸超过所在工作区则自动最大化。
            // 这里统一比较物理像素，因此对 Windows 高 DPI 缩放（scale_factor > 1）
            // 也能正确判定，无需在逻辑像素上额外换算。
            if let Some(win) = app.get_webview_window("main") {
                let too_big = match (win.current_monitor().ok().flatten(), win.outer_size()) {
                    (Some(mon), Ok(size)) => {
                        let wa = mon.work_area().size;
                        size.width > wa.width || size.height > wa.height
                    }
                    _ => false,
                };
                if too_big {
                    let _ = win.maximize();
                } else {
                    let _ = win.center();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pull::pull_image,
            pull::stop_image,
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
