//! 拉取编排 —— 把 9 步流水线串成一个可被前端调用的 `[tauri::command]`。
//!
//! 流程：endpoint 解析 → 认证 → 索引/manifest 选择 → config blob → 内容收集（V1 ID 链）
//! → 并发下载各层并把所有组件打进 docker load 兼容的 tar。
//!
//! 进度：通过 `pull://progress` 事件把各阶段与逐层下载进度实时推给前端。

use std::path::PathBuf;

use serde::Serialize;
use tauri::Emitter;

use crate::auth::Credentials;
use crate::{collect, config, endpoint, output, registry};

/// 命令返回结果，供前端展示。
#[derive(Debug, Serialize)]
pub struct PullResult {
    pub top_id: String,
    pub layer_count: usize,
    pub tar_path: String,
    pub image: String,
}

/// 前端进度事件名。
pub const PROGRESS_EVENT: &str = "pull://progress";

/// 推送给前端的进度负载。
#[derive(Clone, Serialize)]
pub struct ProgressPayload {
    /// 阶段/事件名：auth / manifest / config / blob / write。
    pub name: String,
    /// 已完成量。
    pub done: u64,
    /// 总量。
    pub total: u64,
    /// 人类可读的进度文案。
    pub message: String,
}

/// 根据镜像名生成缺省输出文件名（`<仓库名>_<tag>.tar`）。
fn default_out_file(image: &endpoint::ImageRef) -> String {
    let base = image.repo_name().replace('/', "_");
    format!("{base}_{}.tar", image.tag)
}

/// 把阶段名 + 进度翻译成中文文案。
fn progress_message(name: &str, done: u64, total: u64) -> String {
    match name {
        "auth" => {
            if done == 0 { "正在认证…".to_string() } else { "认证完成".to_string() }
        }
        "manifest" => {
            if done == 0 { "正在获取镜像清单…".to_string() } else { "清单获取完成".to_string() }
        }
        "config" => {
            if done == 0 { "正在解析层配置…".to_string() } else { "配置解析完成".to_string() }
        }
        "download" => format!("准备下载 {} 层", total),
        "blob" => format!("下载层 {}/{total}", done.min(total)),
        "write" => {
            if done == 0 { "正在打包 tar…".to_string() } else { "打包完成".to_string() }
        }
        _ => format!("{name} {done}/{total}"),
    }
}

/// 执行拉取。`progress(name, done, total)` 作为进度回调整体上报。
pub async fn pull(
    image: &str,
    out_file: Option<String>,
    arch: String,
    use_http: bool,
    username: Option<String>,
    password: Option<String>,
    progress: &(dyn Fn(&str, u64, u64) + Send + Sync),
) -> Result<PullResult, String> {
    // 1) 解析镜像引用
    let image_ref = endpoint::parse(image)?;
    let display = image_ref.display_name();

    let http = reqwest::Client::new();
    let creds = Credentials { username, password };

    let mut client = registry::RegistryClient::new(image_ref.clone(), http, arch.clone())
        .with_http(use_http);

    // 2) 认证
    progress("auth", 0, 1);
    client.authorize(&creds).await?;
    progress("auth", 1, 1);

    // 3) 索引/manifest
    progress("manifest", 0, 1);
    let meta = client.fetch_selected_manifest().await?;
    progress("manifest", 1, 1);

    // 4) config blob → DiffIDs
    progress("config", 0, 1);
    let config_digest = meta.config.digest.clone();
    let config_bytes = client.fetch_blob(&config_digest).await?;
    let config_text = String::from_utf8_lossy(&config_bytes);
    let cfg = config::OciConfig::from_json(&config_text)?;
    let diff_ids = cfg.diff_ids().to_vec();
    progress("config", 1, 1);

    // 5) 层 blob digest（跳过无 digest 的 foreign/空层）
    let blob_digests: Vec<String> = meta
        .layers
        .iter()
        .filter(|l| !l.digest.is_empty())
        .map(|l| l.digest.clone())
        .collect();
    if blob_digests.len() < diff_ids.len() {
        return Err(format!(
            "有效层({}) 少于 diff_id({})，镜像含 foreign/空层暂不支持",
            blob_digests.len(),
            diff_ids.len()
        ));
    }

    // 6) 内容收集
    let collected = collect::collect(&image_ref, &diff_ids, &blob_digests, &config_digest, &cfg)?;

    // 7) 并发下载 + 打包
    let tar_path = match out_file {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(default_out_file(&image_ref)),
    };
    output::write_tar(
        &client,
        &collected,
        &config_digest,
        &config_bytes,
        &blob_digests,
        &tar_path,
        progress,
    )
    .await?;

    Ok(PullResult {
        top_id: collected.top_id,
        layer_count: collected.layers.len(),
        tar_path: tar_path.to_string_lossy().into_owned(),
        image: display,
    })
}

/// 暴露给前端的命令。`app` 由 Tauri 注入，用于推送进度事件。
#[tauri::command]
pub async fn pull_image(
    app: tauri::AppHandle,
    image: String,
    out_file: Option<String>,
    arch: Option<String>,
    use_http: Option<bool>,
    username: Option<String>,
    password: Option<String>,
) -> Result<PullResult, String> {
    let arch = arch.unwrap_or_else(|| "amd64".to_string());
    let use_http = use_http.unwrap_or(false);

    let app2 = app.clone();
    let progress = move |name: &str, done: u64, total: u64| {
        let payload = ProgressPayload {
            name: name.to_string(),
            done,
            total,
            message: progress_message(name, done, total),
        };
        let _ = app2.emit(PROGRESS_EVENT, payload);
    };

    pull(
        &image,
        out_file,
        arch,
        use_http,
        username,
        password,
        &progress,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_out_file_shape() {
        let r = endpoint::parse("nginx:1.25").unwrap();
        assert_eq!(default_out_file(&r), "nginx_1.25.tar");
        let r2 = endpoint::parse("ghcr.io/org/app").unwrap();
        assert_eq!(default_out_file(&r2), "org_app_latest.tar");
    }

    #[test]
    fn progress_messages_localized() {
        assert_eq!(progress_message("auth", 0, 1), "正在认证…");
        assert_eq!(progress_message("blob", 1, 3), "下载层 1/3");
        assert_eq!(progress_message("write", 1, 1), "打包完成");
    }
}