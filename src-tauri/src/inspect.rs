//! 镜像检查 —— 拉取前先「看一眼」目标镜像。
//!
//! 供前端「详情」页展示：仓库有哪些可用平台（os/arch/variant/digest/size）、
//! 有哪些标签、选中架构的 config 元信息（created / docker_version / env / cmd）与层规模。
//!
//! 逻辑分两层：
//! - 真正走网络的部分复用 [`crate::registry::RegistryClient`]（授权 / 拉索引 / 拉单架构 manifest / 拉 config blob / 列标签）。
//! - 索引 → 平台列表、config blob → 展示结构 的纯转换拆成可离线单测的函数。

use serde::Serialize;

use crate::auth::Credentials;
use crate::config::OciConfig;
use crate::endpoint;
use crate::registry::{
    ManifestRef, OciIndex, RegistryClient, select_manifest_ref,
};

/// 索引暴露的单个可用平台。
#[derive(Debug, Clone, Serialize)]
pub struct InspectPlatform {
    pub os: String,
    pub architecture: String,
    pub variant: String,
    pub digest: String,
    pub size: u64,
}

/// 选中架构的 config 展示信息（模板里只展示常用字段）。
#[derive(Debug, Clone, Serialize)]
pub struct InspectConfig {
    pub architecture: String,
    pub os: String,
    pub docker_version: String,
    pub created: Option<String>,
    pub env: Vec<String>,
    pub cmd: Vec<String>,
    pub working_dir: String,
}

/// 检查结果，返回给前端「详情」页。
#[derive(Debug, Serialize)]
pub struct InspectResult {
    /// 展示名，如 `nginx:latest`。
    pub image: String,
    /// 当前选用的 tag。
    pub tag: String,
    /// 若按 digest 引用则非空。
    pub digest: Option<String>,
    /// 仓库暴露的所有可用平台。
    pub platforms: Vec<InspectPlatform>,
    /// 仓库的标签列表（registry 禁用 tags/list 时为空）。
    pub tags: Vec<String>,
    /// 选中架构的 config 元信息（拉 config blob 失败或缺失时为 None）。
    pub config: Option<InspectConfig>,
    /// 选中架构的层数。
    pub layer_count: usize,
    /// 选中架构各层大小总和（字节）。
    pub total_size: u64,
}

/// 从索引提取平台列表（离线可测）。单架构索引 `manifests` 为空 → 返回空。
fn platforms_from_index(index: &OciIndex) -> Vec<InspectPlatform> {
    index
        .manifests
        .iter()
        .filter_map(|m| {
            let p = m.platform.as_ref()?;
            Some(InspectPlatform {
                os: p.os.clone(),
                architecture: p.architecture.clone(),
                variant: p.variant.clone(),
                digest: m.digest.clone(),
                size: m.size,
            })
        })
        .collect()
}

/// 把 OCI config 转成展示结构（离线可测）。
fn config_to_inspect(cfg: &OciConfig) -> InspectConfig {
    let cc = cfg.config.as_ref();
    InspectConfig {
        architecture: cfg.architecture.clone(),
        os: cfg.os.clone(),
        docker_version: cfg.docker_version.clone(),
        created: cfg.created.clone(),
        env: cc.map(|c| c.env.clone()).unwrap_or_default(),
        cmd: cc.map(|c| c.cmd.clone()).unwrap_or_default(),
        working_dir: cc.map(|c| c.working_dir.clone()).unwrap_or_default(),
    }
}

/// 从索引选「要展示 config 的那个架构」的 manifest 引用。
///
/// 优先请求的目标架构；若不存在，退回索引里第一个 linux 平台；都没有才报错。
/// 单架构（manifests 为空）时返回 None，由调用方直接用 tag 拉单架构 manifest。
fn select_reference_or_none(index: &OciIndex, arch: &str) -> Result<Option<ManifestRef>, String> {
    if index.manifests.is_empty() {
        return Ok(None);
    }
    match select_manifest_ref(index, arch) {
        Ok(r) => Ok(Some(r)),
        Err(_) => {
            let first = index
                .manifests
                .iter()
                .find(|m| m.platform.as_ref().map(|p| p.os == "linux").unwrap_or(false))
                .ok_or_else(|| "索引中没有可用的 linux 平台".to_string())?;
            Ok(Some(ManifestRef::Digest(first.digest.clone())))
        }
    }
}

/// 执行镜像检查。
pub async fn inspect(
    image: &str,
    arch: &str,
    use_http: bool,
    username: Option<String>,
    password: Option<String>,
) -> Result<InspectResult, String> {
    let image_ref = endpoint::parse(image)?;
    let display = image_ref.display_name();

    let http = reqwest::Client::new();
    let creds = Credentials { username, password };
    let mut client =
        RegistryClient::new(image_ref.clone(), http, arch.to_string()).with_http(use_http);
    client.authorize(&creds).await?;

    let index = client.fetch_index().await?;
    let tags = client.list_tags().await.unwrap_or_default();

    // 拿到展示 default 架构的 manifest（config / 层信息）。
    let reference: String = match select_reference_or_none(&index, arch)? {
        Some(ManifestRef::Digest(d)) => d,
        Some(ManifestRef::Tag) | None => image_ref.tag.clone(),
    };
    let meta = client.fetch_single_manifest(reference).await?;

    // config blob → InspectConfig（失败时降级为 None，不阻断详情展示）。
    let config = if meta.config.digest.is_empty() {
        None
    } else {
        match client.fetch_blob(&meta.config.digest).await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                OciConfig::from_json(&text).ok().map(|c| config_to_inspect(&c))
            }
            Err(_) => None,
        }
    };

    let total_size: u64 = meta.layers.iter().map(|l| l.size).sum();
    let layer_count = meta.layers.len();

    // 平台列表：索引有多平台就用索引；单架构则从 config 反推。
    let mut platforms = platforms_from_index(&index);
    if platforms.is_empty() {
        if let Some(c) = &config {
            platforms.push(InspectPlatform {
                os: c.os.clone(),
                architecture: c.architecture.clone(),
                variant: String::new(),
                digest: String::new(),
                size: total_size,
            });
        }
    }

    Ok(InspectResult {
        image: display,
        tag: image_ref.tag.clone(),
        digest: image_ref.digest.clone(),
        platforms,
        tags,
        config,
        layer_count,
        total_size,
    })
}

/// 暴露给前端的命令。
#[tauri::command]
pub async fn inspect_image(
    image: String,
    arch: Option<String>,
    use_http: Option<bool>,
    username: Option<String>,
    password: Option<String>,
) -> Result<InspectResult, String> {
    let arch = arch.unwrap_or_else(|| "amd64".to_string());
    let use_http = use_http.unwrap_or(false);
    inspect(&image, &arch, use_http, username, password).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{IndexManifest, Platform};

    fn index_manifests(entries: &[(&str, &str, u64)]) -> OciIndex {
        // entries: (os, arch, size)
        OciIndex {
            manifests: entries
                .iter()
                .map(|(os, arch, size)| IndexManifest {
                    digest: format!("sha256:{arch}"),
                    size: *size,
                    platform: Some(Platform {
                        os: os.to_string(),
                        architecture: arch.to_string(),
                        variant: String::new(),
                    }),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn platforms_from_index_lists_all() {
        let index = index_manifests(&[("linux", "amd64", 500), ("linux", "arm64", 600)]);
        let ps = platforms_from_index(&index);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].architecture, "amd64");
        assert_eq!(ps[0].os, "linux");
        assert_eq!(ps[0].size, 500);
        assert_eq!(ps[1].architecture, "arm64");
    }

    #[test]
    fn platforms_from_empty_index() {
        let ps = platforms_from_index(&OciIndex::default());
        assert!(ps.is_empty());
    }

    #[test]
    fn platforms_skip_missing_platform() {
        let mut index = index_manifests(&[("linux", "amd64", 1)]);
        index.manifests.push(IndexManifest {
            digest: "sha256:none".into(),
            ..Default::default()
        });
        assert_eq!(platforms_from_index(&index).len(), 1);
    }

    #[test]
    fn select_prefers_requested_arch_falls_back_to_first() {
        let index = index_manifests(&[("linux", "arm64", 1), ("linux", "amd64", 1)]);
        // 本就在索引里 → 返回对应 digest。
        assert_eq!(
            select_reference_or_none(&index, "amd64").unwrap(),
            Some(ManifestRef::Digest("sha256:amd64".into()))
        );
        // 不在索引里 → 退回第一个 linux 平台（arm64）。
        assert_eq!(
            select_reference_or_none(&index, "s390x").unwrap(),
            Some(ManifestRef::Digest("sha256:arm64".into()))
        );
    }

    #[test]
    fn select_for_single_arch_returns_none() {
        assert_eq!(select_reference_or_none(&OciIndex::default(), "amd64").unwrap(), None);
    }

    #[test]
    fn config_to_inspect_maps_fields() {
        let cfg = OciConfig::from_json(
            r#"{"architecture":"amd64","os":"linux","docker_version":"24.0.5","created":"2023-05-01T09:00:00Z","config":{"Env":["A=1"],"Cmd":["nginx","-g"],"WorkingDir":"/app"}}"#,
        )
        .unwrap();
        let ic = config_to_inspect(&cfg);
        assert_eq!(ic.architecture, "amd64");
        assert_eq!(ic.os, "linux");
        assert_eq!(ic.docker_version, "24.0.5");
        assert_eq!(ic.created.as_deref(), Some("2023-05-01T09:00:00Z"));
        assert_eq!(ic.env, vec!["A=1"]);
        assert_eq!(ic.cmd, vec!["nginx", "-g"]);
        assert_eq!(ic.working_dir, "/app");
    }

    #[test]
    fn config_to_inspect_empty_config_defaults() {
        let cfg = OciConfig::from_json(r#"{"rootfs":{"diff_ids":[]}}"#).unwrap();
        let ic = config_to_inspect(&cfg);
        assert!(ic.env.is_empty());
        assert!(ic.cmd.is_empty());
        assert!(ic.working_dir.is_empty());
        assert!(ic.created.is_none());
    }
}