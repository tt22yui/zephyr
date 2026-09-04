//! Registry v2 API 交互 —— 复刻 docker-tar 的 Authenticator + ImageIndexFetcher + ImageConfigFetcher。
//!
//! 提供：
//! - Bearer token 认证（challenge → fetch token）
//! - 抓取镜像索引（manifest list）并按目标架构选中一个 manifest
//! - 抓取单架构 manifest（得到 config 描述符与各层 blob 描述符）
//!
//! 纯解析/选片逻辑拆成可离线单测的函数；真正走网络的统一用 `&Client` + bearer。

use crate::auth::{self, Credentials};
use crate::endpoint::ImageRef;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

/// OCI manifest 索引（media type: docker manifest list v2 或 oci image index）。
#[derive(Debug, Deserialize, Default)]
pub struct OciIndex {
    #[serde(default)]
    pub schema_version: i64,
    #[serde(rename = "mediaType", default)]
    pub media_type: String,
    #[serde(default)]
    pub manifests: Vec<IndexManifest>,
    #[serde(default)]
    pub annotations: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct IndexManifest {
    #[serde(rename = "mediaType", default)]
    pub media_type: String,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub platform: Option<Platform>,
    #[serde(default)]
    pub annotations: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Platform {
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub architecture: String,
    #[serde(default)]
    pub variant: String,
}

/// 单架构 manifest。
#[derive(Debug, Deserialize, Default)]
pub struct OciManifest {
    #[serde(rename = "schemaVersion", default)]
    pub schema_version: i64,
    #[serde(rename = "mediaType", default)]
    pub media_type: String,
    #[serde(default)]
    pub config: Descriptor,
    #[serde(default)]
    pub layers: Vec<Descriptor>,
    #[serde(default)]
    pub annotations: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Descriptor {
    #[serde(rename = "mediaType", default)]
    pub media_type: String,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub size: u64,
}

/// `GET /v2/<repo>/tags/list` 的响应（只取标签名）。
#[derive(Debug, Deserialize, Default)]
pub struct TagsList {
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 离线可测：从 tags/list 响应文本提取标签名。
fn parse_tags_response(body: &str) -> Result<Vec<String>, String> {
    let v: TagsList =
        serde_json::from_str(body).map_err(|e| format!("解析 tags 列表失败: {e}"))?;
    Ok(v.tags)
}

/// 选出的 manifest 引用：要么是具体 digest，要么回退用 tag。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestRef {
    Digest(String),
    Tag,
}

const BASHBREW_ARCH_ANNOTATION: &str = "com.docker.official-images.bashbrew.arch";

/// 从索引中按目标架构挑出要拉取的 manifest。
/// 行为参考 docker-tar，但做了增强：兼容 arm64 + variant 的匹配。
pub fn select_manifest_ref(index: &OciIndex, arch: &str) -> Result<ManifestRef, String> {
    let linux: Vec<&IndexManifest> = index
        .manifests
        .iter()
        .filter(|m| m.platform.as_ref().map(|p| p.os == "linux").unwrap_or(false))
        .collect();

    if linux.is_empty() {
        // 可能是单架构镜像（无 manifests 列表）：看官方镜像注解里是否就是该架构。
        if index.media_type.is_empty() || index.schema_version == 0 {
            if index.annotations.get(BASHBREW_ARCH_ANNOTATION).map(|a| a == arch).unwrap_or(false) {
                return Ok(ManifestRef::Tag);
            }
        }
        let avail: Vec<String> = index
            .manifests
            .iter()
            .filter_map(|m| m.platform.as_ref())
            .map(|p| p.architecture.clone())
            .collect();
        return Err(format!(
            "未找到目标架构 {arch}，可用架构: {}",
            if avail.is_empty() { "无".to_string() } else { avail.join(", ") }
        ));
    }

    // 先按 architecture 匹配；有 variant 时优先精确匹配。
    let exact = linux
        .iter()
        .find(|m| {
            let p = m.platform.as_ref().unwrap();
            p.architecture == arch && p.variant.is_empty()
        })
        .or_else(|| {
            linux.iter().find(|m| {
                let p = m.platform.as_ref().unwrap();
                let key = format!("{}{}", p.architecture, p.variant);
                key == format!("{arch}") || key == arch
            })
        })
        .or_else(|| linux.iter().find(|m| m.platform.as_ref().unwrap().architecture == arch));

    match exact {
        Some(m) => Ok(ManifestRef::Digest(m.digest.clone())),
        None => {
            let avail: Vec<String> = linux
                .iter()
                .filter_map(|m| m.platform.as_ref())
                .map(|p| p.architecture.clone())
                .collect();
            Err(format!(
                "目标架构 {arch} 未在索引中，可用: {}",
                avail.join(", ")
            ))
        }
    }
}

/// Registry v2 客户端。
pub struct RegistryClient {
    http: Client,
    image: ImageRef,
    arch: String,
    token: String,
    use_http: bool,
}

#[derive(Debug)]
pub struct ImageMeta {
    /// config 描述符（去拉 config blob 需用）。
    pub config: Descriptor,
    /// 目标架构单架构 manifest 的 digest（拉 config blob 参考用，可选的）。
    pub manifest_digest: Option<String>,
    /// 各层 blob（顺序即父链顺序）。
    pub layers: Vec<Descriptor>,
}

impl RegistryClient {
    pub fn new(image: ImageRef, http: Client, arch: impl Into<String>) -> Self {
        Self {
            http,
            image,
            arch: arch.into(),
            token: String::new(),
            use_http: false,
        }
    }

    pub fn with_http(mut self, use_http: bool) -> Self {
        self.use_http = use_http;
        self
    }

    fn scheme(&self) -> &'static str {
        if self.use_http { "http" } else { "https" }
    }

    fn endpoint(&self) -> String {
        format!("{0}://{1}", self.scheme(), self.image.registry)
    }

    /// 认证：触发 401 → 解析挑战 → 换取 token。已认证则直接返回现有 token。
    pub async fn authorize(&mut self, creds: &Credentials) -> Result<String, String> {
        if !self.token.is_empty() {
            return Ok(self.token.clone());
        }
        let url = format!("{}/v2/", self.endpoint());
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("连接 registry 失败: {e}"))?;
        let status = resp.status();
        // 有些 registry 对 /v2/ 直接给 200，跳过挑战也能用。
        if status.is_success() {
            return Ok(String::new());
        }
        if status != reqwest::StatusCode::UNAUTHORIZED {
            return Err(format!("认证发起失败: {status}"));
        }
        let header = resp
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| "缺少 WWW-Authenticate 头".to_string())?;
        let challenge = auth::parse_bearer_challenge(header)
            .ok_or_else(|| "WWW-Authenticate 不是预期的 Bearer 挑战".to_string())?;
        let scope = if challenge.scope.is_empty() {
            auth::default_scope(&self.image)
        } else {
            challenge.scope.clone()
        };
        let token = auth::fetch_token(&self.http, &challenge, &scope, creds).await?;
        self.token = token.clone();
        Ok(token)
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        accept: &str,
    ) -> Result<(T, Option<String>), String> {
        let mut req = self.http.get(url).header(reqwest::header::ACCEPT, accept);
        if !self.token.is_empty() {
            req = req.bearer_auth(&self.token);
        }
        let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("请求 {url} 失败: {status}: {body}"));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = resp.text().await.map_err(|e| format!("读取响应体失败: {e}"))?;
        let value = serde_json::from_str(&body).map_err(|e| format!("解析 JSON 失败: {e}: {body}"))?;
        Ok((value, content_type))
    }

    fn manifest_url(&self, reference: &str) -> String {
        format!(
            "{}/v2/{}/manifests/{}",
            self.endpoint(),
            self.image.repo_path,
            reference
        )
    }

    /// 抓取索引，选择目标架构。
    pub async fn fetch_selected_manifest(&self) -> Result<ImageMeta, String> {
        let (index, _) = self
            .get_json::<OciIndex>(
                &self.manifest_url(self.image.manifest_ref()),
                "application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.index.v1+json",
            )
            .await?;

        let sel = select_manifest_ref(&index, &self.arch)?;
        if let ManifestRef::Digest(d) = &sel {
            // 用该 digest 拉取单架构 manifest。
            return self.fetch_single_manifest(d.clone()).await;
        }
        // 单架构镜像：直接用 tag。
        self.fetch_single_manifest(self.image.tag.clone()).await
    }

    /// 抓取单个（单架构）manifest，汇总出 config 与各层 blob。
    pub async fn fetch_single_manifest(&self, reference: String) -> Result<ImageMeta, String> {
        let (manifest, _) = self
            .get_json::<OciManifest>(
                &self.manifest_url(&reference),
                "application/vnd.docker.distribution.manifest.v2+json, application/vnd.oci.image.manifest.v1+json",
            )
            .await?;
        Ok(ImageMeta {
            config: manifest.config,
            manifest_digest: Some(reference),
            layers: manifest.layers,
        })
    }

    /// 抓取原始索引（不选架构），供镜像检查（inspect）展示所有可用平台。
    /// 单架构或按 digest 引用时，返回的索引 `manifests` 为空数组。
    pub async fn fetch_index(&self) -> Result<OciIndex, String> {
        let (index, _) = self
            .get_json::<OciIndex>(
                &self.manifest_url(self.image.manifest_ref()),
                "application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.index.v1+json",
            )
            .await?;
        Ok(index)
    }

    /// 列出该仓库的标签。部分 registry 禁用 `tags/list`，此时返回空列表，不视为失败。
    pub async fn list_tags(&self) -> Result<Vec<String>, String> {
        let url = format!(
            "{}/v2/{}/tags/list?n=1000",
            self.endpoint(),
            self.image.repo_path
        );
        let mut req = self.http.get(&url);
        if !self.token.is_empty() {
            req = req.bearer_auth(&self.token);
        }
        let resp = req.send().await.map_err(|e| format!("请求 tags 列表失败: {e}"))?;
        if !resp.status().is_success() {
            // tags/list 非 200（常被 registry 禁用或需额外权限）：不作为硬失败。
            return Ok(Vec::new());
        }
        let body = resp.text().await.map_err(|e| format!("读取 tags 列表失败: {e}"))?;
        parse_tags_response(&body)
    }

    /// 拉取 config blob 原始字节（与 digest 对应）。
    pub async fn fetch_blob(&self, digest: &str) -> Result<Vec<u8>, String> {
        let url = format!(
            "{}/v2/{}/blobs/{}",
            self.endpoint(),
            self.image.repo_path,
            digest
        );
        let mut req = self.http.get(&url);
        if !self.token.is_empty() {
            req = req.bearer_auth(&self.token);
        }
        let resp = req.send().await.map_err(|e| format!("请求 blob 失败: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("抓取 blob {digest} 失败: {status}: {body}"));
        }
        resp.bytes().await.map(|b| b.to_vec()).map_err(|e| format!("读取 blob 失败: {e}"))
    }

    pub fn arch(&self) -> &str {
        &self.arch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_manifests(entries: &[(&str, &str)]) -> OciIndex {
        // entries: (arch, digest)
        OciIndex {
            manifests: entries
                .iter()
                .map(|(arch, digest)| IndexManifest {
                    digest: digest.to_string(),
                    platform: Some(Platform {
                        os: "linux".to_string(),
                        architecture: arch.to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn parse_real_index_json() {
        let json = r#"{
            "schemaVersion": 2,
            "mediaType": "application/vnd.docker.distribution.manifest.list.v2+json",
            "manifests": [
                {"mediaType":"application/vnd.docker.distribution.manifest.v2+json","size":525,"digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","platform":{"architecture":"amd64","os":"linux"}},
                {"mediaType":"application/vnd.docker.distribution.manifest.v2+json","size":528,"digest":"sha256:2222222222222222222222222222222222222222222222222222222222222222","platform":{"architecture":"arm64","os":"linux","variant":"v8"}}
            ]
        }"#;
        let index: OciIndex = serde_json::from_str(json).unwrap();
        assert_eq!(index.manifests.len(), 2);
        assert_eq!(index.media_type, "application/vnd.docker.distribution.manifest.list.v2+json");
    }

    #[test]
    fn select_amd64() {
        let index = index_manifests(&[
            ("amd64", "sha256:aaaa"),
            ("arm64", "sha256:bbbb"),
        ]);
        assert_eq!(
            select_manifest_ref(&index, "amd64").unwrap(),
            ManifestRef::Digest("sha256:aaaa".into())
        );
    }

    #[test]
    fn select_arm64_with_variant() {
        let index = OciIndex {
            manifests: vec![IndexManifest {
                digest: "sha256:arm".into(),
                platform: Some(Platform { os: "linux".into(), architecture: "arm64".into(), variant: "v8".into() }),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            select_manifest_ref(&index, "arm64").unwrap(),
            ManifestRef::Digest("sha256:arm".into())
        );
    }

    #[test]
    fn select_not_found_err() {
        let index = index_manifests(&[("amd64", "sha256:aaaa")]);
        assert!(select_manifest_ref(&index, "s390x").is_err());
    }

    #[test]
    fn select_fallback_to_tag_for_single_arch() {
        let index = OciIndex {
            schema_version: 0,
            annotations: HashMap::from([
                ("org.opencontainers.image.ref.name".to_string(), "x".into()),
                ("com.docker.official-images.bashbrew.arch".to_string(), "amd64".into()),
            ]),
            ..Default::default()
        };
        assert_eq!(select_manifest_ref(&index, "amd64").unwrap(), ManifestRef::Tag);
    }

    #[test]
    fn parse_real_manifest_json() {
        let json = r#"{
            "schemaVersion": 2,
            "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
            "config": {"mediaType":"application/vnd.docker.container.image.v1+json","size":7023,"digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},
            "layers": [
                {"mediaType":"application/vnd.docker.image.rootfs.diff.tar.gzip","size":2802163,"digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}
            ]
        }"#;
        let m: OciManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.config.digest, "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
        assert_eq!(m.layers.len(), 1);
        assert_eq!(m.layers[0].media_type, "application/vnd.docker.image.rootfs.diff.tar.gzip");
    }

    #[test]
    fn parse_tags_response_extracts_names() {
        let out = parse_tags_response(r#"{"name":"library/nginx","tags":["latest","1.25","stable"]}"#).unwrap();
        assert_eq!(out, vec!["latest", "1.25", "stable"]);
    }

    #[test]
    fn parse_tags_response_missing_tags_is_empty() {
        let out = parse_tags_response(r#"{"name":"library/nginx"}"#).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_tags_response_rejects_bad_json() {
        assert!(parse_tags_response("not json").is_err());
    }
}
