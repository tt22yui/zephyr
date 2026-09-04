//! OCI image config blob 解析 —— 对应 docker-tar 的 `ImageConfigBlobFetcher` + 上游的 config 解析。
//!
//! 从 config blob 提取构造 v1 镜像所需的字段：
//! - `rootfs.diff_ids`：各层解压后的 sha256（顺序即父链顺序：第 0 个是基础层）
//! - `created` / `os` / `architecture` / `author` / `docker_version`：末层 V1Image 元信息
//! - `config`：容器运行配置（Env / Cmd / WorkingDir 等）

use crate::info::ContainerConfig;
use crate::info::V1Image;
use serde::Deserialize;

/// OCI image config（`application/vnd.docker.container.image.v1+json`）。
#[derive(Debug, Deserialize, Default)]
pub struct OciConfig {
    #[serde(default)]
    pub architecture: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(rename = "docker_version", default)]
    pub docker_version: String,
    #[serde(default)]
    pub config: Option<ContainerConfig>,
    #[serde(default)]
    pub rootfs: RootFs,
}

#[derive(Debug, Deserialize, Default)]
pub struct RootFs {
    /// 各层解压后内容 sha256，形如 `sha256:...`。
    #[serde(rename = "diff_ids", default)]
    pub diff_ids: Vec<String>,
    #[serde(rename = "type", default)]
    pub r#type: String,
}

impl OciConfig {
    /// 解析 config blob 的 JSON 文本。
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("解析 config blob 失败: {e}"))
    }

    /// DiffIDs 数组。
    pub fn diff_ids(&self) -> &[String] {
        &self.rootfs.diff_ids
    }

    /// 构造「末层」V1Image：用 config blob 里的真实元信息（os / arch / author / created 等）。
    /// 不含 id / parent（由 collector 计算后填入）。
    pub fn to_last_v1_image(&self) -> V1Image {
        V1Image {
            created: self.created.clone(),
            os: self.os.clone(),
            architecture: self.architecture.clone(),
            author: self.author.clone(),
            docker_version: self.docker_version.clone(),
            container_config: self.config.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG_JSON: &str = r#"{
        "architecture": "amd64",
        "config": {
            "Env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"],
            "Cmd": ["nginx", "-g", "daemon off;"],
            "WorkingDir": "/",
            "Image": "nginx:1.25"
        },
        "created": "2023-05-01T09:00:00Z",
        "docker_version": "24.0.5",
        "history": [],
        "os": "linux",
        "rootfs": {
            "type": "layers",
            "diff_ids": ["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
        }
    }"#;

    #[test]
    fn parse_diff_ids_in_order() {
        let cfg = OciConfig::from_json(CONFIG_JSON).unwrap();
        let ids = cfg.diff_ids();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(ids[1], "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    }

    #[test]
    fn parse_metadata_fields() {
        let cfg = OciConfig::from_json(CONFIG_JSON).unwrap();
        assert_eq!(cfg.os, "linux");
        assert_eq!(cfg.architecture, "amd64");
        assert_eq!(cfg.created.as_deref(), Some("2023-05-01T09:00:00Z"));
        assert_eq!(cfg.docker_version, "24.0.5");
    }

    #[test]
    fn parse_container_config() {
        let cfg = OciConfig::from_json(CONFIG_JSON).unwrap();
        let cc = cfg.config.as_ref().unwrap();
        assert_eq!(cc.cmd, vec!["nginx".to_string(), "-g".into(), "daemon off;".into()]);
        assert_eq!(cc.working_dir, "/");
        assert_eq!(cc.image, "nginx:1.25");
        assert_eq!(cc.env.len(), 1);
    }

    #[test]
    fn to_last_v1_image_maps_fields() {
        let cfg = OciConfig::from_json(CONFIG_JSON).unwrap();
        let img = cfg.to_last_v1_image();
        assert_eq!(img.os, "linux");
        assert_eq!(img.architecture, "amd64");
        assert_eq!(img.created.as_deref(), Some("2023-05-01T09:00:00Z"));
        assert_eq!(img.docker_version, "24.0.5");
        assert_eq!(img.container_config.cmd, vec!["nginx".to_string(), "-g".into(), "daemon off;".into()]);
        // id/parent 由 collector 填，这里仍为空。
        assert!(img.id.is_empty());
        assert!(img.parent.is_empty());
    }

    #[test]
    fn empty_config_defaults() {
        let cfg = OciConfig::from_json(r#"{"rootfs":{"type":"layers","diff_ids":[]}}"#).unwrap();
        assert!(cfg.diff_ids().is_empty());
        assert!(cfg.os.is_empty());
        let img = cfg.to_last_v1_image();
        assert_eq!(img.created, None);
    }

    #[test]
    fn created_null_is_none() {
        // `created: null` 应解析为 None，而不是 "null" 字符串或报错。
        let cfg = OciConfig::from_json(r#"{"created":null,"os":"linux"}"#).unwrap();
        assert_eq!(cfg.created, None);
        assert_eq!(cfg.os, "linux");
        assert!(cfg.diff_ids().is_empty());
    }

    #[test]
    fn unknown_extra_keys_ignored() {
        // 上游可能引入未知字段，解析不应失败，也不应污染已知字段。
        let json = r#"{
            "architecture":"arm64","os":"linux",
            "vendor":"oci","schema_version":2,
            "rootfs":{"type":"layers","diff_ids":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]}
        }"#;
        let cfg = OciConfig::from_json(json).unwrap();
        assert_eq!(cfg.architecture, "arm64");
        assert_eq!(cfg.diff_ids().len(), 1);
    }

    #[test]
    fn config_present_but_empty_is_some_empty() {
        // 空对象 config → Some(空 ContainerConfig)，与「缺失」区分开。
        let cfg = OciConfig::from_json(r#"{"config":{},"rootfs":{"diff_ids":[]}}"#).unwrap();
        let cc = cfg.config.as_ref().unwrap_or_else(|| panic!("config 键存在时应为 Some"));
        assert!(cc.env.is_empty());
        assert!(cc.cmd.is_empty());
        assert!(cfg.to_last_v1_image().container_config.env.is_empty());
    }

    #[test]
    fn missing_config_is_none_and_defaults() {
        let cfg = OciConfig::from_json(r#"{"os":"linux","rootfs":{}}"#).unwrap();
        assert!(cfg.config.is_none());
        assert!(cfg.to_last_v1_image().container_config.env.is_empty());
    }

    #[test]
    fn env_order_is_preserved() {
        // Env 是有序数组，顺序需原样保留。
        let cfg = OciConfig::from_json(
            r#"{"config":{"Env":["PATH=/a","A=1","B=2"]}}"#,
        )
        .unwrap();
        let cc = cfg.config.unwrap();
        assert_eq!(cc.env, vec!["PATH=/a", "A=1", "B=2"]);
    }
}
