//! 镜像搜索 —— 走 Docker Hub 公开搜索 API（与 `docker search` 同源）。
//!
//! 接口：`GET https://hub.docker.com/v2/search/repositories/?query={q}&page_size={n}`
//! 公开接口、无需认证；返回的 `repo_name`（如 `nginx`、`user/repo`）可直接
//! 作为镜像名喂给 [`crate::endpoint::parse`] 发起拉取（官方镜像会自动补 `library/`）。
//!
//! 说明：Registry v2 规范的 `/_catalog` 只覆盖私有 registry 且不做关键词搜索，
//! 因此首期搜索仅支持 Docker Hub，第三方 registry 搜索留作后续扩展。

use reqwest::Url;
use serde::{Deserialize, Serialize};

/// 返回给前端的单条搜索结果。
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    /// 仓库名，如 `nginx`、`user/repo`，可直接用于拉取。
    pub name: String,
    /// 仓库简介（可能为空）。
    pub description: String,
    /// star 数。
    pub stars: u64,
    /// 是否为 Docker 官方镜像。
    pub is_official: bool,
    /// 最后更新时间（API 返回的时间字符串）。
    pub updated_at: String,
}

/// Hub 搜索 API 的响应体（只取需要的字段）。
#[derive(Debug, Deserialize, Default)]
struct HubSearchResponse {
    #[serde(default)]
    results: Vec<HubSearchItem>,
}

#[derive(Debug, Deserialize, Default)]
struct HubSearchItem {
    #[serde(rename = "repo_name", default)]
    repo_name: String,
    #[serde(rename = "short_description", default)]
    short_description: String,
    #[serde(rename = "star_count", default)]
    star_count: u64,
    #[serde(rename = "is_official", default)]
    is_official: bool,
    #[serde(rename = "last_updated", default)]
    last_updated: String,
}

const HUB_SEARCH_URL: &str = "https://hub.docker.com/v2/search/repositories";

/// 默认每页条数。
pub const DEFAULT_PAGE_SIZE: u32 = 25;
/// page_size 上限，避免一次拉取过多结果。
pub const MAX_PAGE_SIZE: u32 = 100;

/// 解析 Hub 搜索 API 的 JSON 响应体（可离线单测）。
fn parse_search_response(body: &str) -> Result<Vec<SearchResult>, String> {
    let resp: HubSearchResponse =
        serde_json::from_str(body).map_err(|e| format!("无法解析搜索结果: {e}"))?;
    Ok(resp
        .results
        .into_iter()
        .filter(|r| !r.repo_name.is_empty())
        .map(|r| SearchResult {
            name: r.repo_name,
            description: r.short_description,
            stars: r.star_count,
            is_official: r.is_official,
            updated_at: r.last_updated,
        })
        .collect())
}

/// 归一化 page_size：至少 1、至多 [`MAX_PAGE_SIZE`]。
fn clamp_page_size(page_size: u32) -> u32 {
    page_size.clamp(1, MAX_PAGE_SIZE)
}

/// 在 Docker Hub 搜索镜像仓库。query 为空时直接返回空列表。
///
/// `base` 可指定自定义搜索端点（如第三方 registry 提供的搜索地址）；
/// 为 `None` 时使用 Docker Hub 公开搜索 API。
pub async fn search(
    query: &str,
    page_size: u32,
    base: Option<&str>,
) -> Result<Vec<SearchResult>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let size = clamp_page_size(page_size);
    let base_url = base.unwrap_or(HUB_SEARCH_URL);

    let url = Url::parse_with_params(
        base_url,
        &[("query", q), ("page_size", &size.to_string())],
    )
    .map_err(|e| format!("构造搜索地址失败: {e}"))?;

    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("搜索请求失败（请检查网络/代理）: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("镜像库搜索接口返回 {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取搜索结果失败: {e}"))?;
    parse_search_response(&body)
}

/// 暴露给前端的命令。
#[tauri::command]
pub async fn search_image(
    query: String,
    page_size: Option<u32>,
    base: Option<String>,
) -> Result<Vec<SearchResult>, String> {
    search(&query, page_size.unwrap_or(DEFAULT_PAGE_SIZE), base.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_response_full_fields() {
        let body = r#"{
            "count": 2,
            "results": [
                {"repo_name": "nginx", "short_description": "Official build of Nginx.", "star_count": 18000, "is_official": true, "last_updated": "2024-06-01T00:00:00.000000Z"},
                {"repo_name": "user/my-app", "short_description": "", "star_count": 3, "is_official": false, "last_updated": "2023-01-01T00:00:00.000000Z"}
            ]
        }"#;
        let out = parse_search_response(body).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "nginx");
        assert_eq!(out[0].description, "Official build of Nginx.");
        assert_eq!(out[0].stars, 18000);
        assert!(out[0].is_official);
        assert_eq!(out[1].name, "user/my-app");
        assert_eq!(out[1].description, "");
        assert!(!out[1].is_official);
    }

    #[test]
    fn parse_response_defaults_for_missing_fields() {
        let body = r#"{"results": [{"repo_name": "bare"}]}"#;
        let out = parse_search_response(body).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "bare");
        assert_eq!(out[0].stars, 0);
        assert!(!out[0].is_official);
        assert_eq!(out[0].updated_at, "");
    }

    #[test]
    fn parse_response_empty_results() {
        let out = parse_search_response(r#"{"count": 0, "results": []}"#).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_response_filters_empty_names() {
        let body = r#"{"results": [{"repo_name": ""}, {"repo_name": "ok/repo"}]}"#;
        let out = parse_search_response(body).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "ok/repo");
    }

    #[test]
    fn parse_response_rejects_bad_json() {
        assert!(parse_search_response("not json").is_err());
    }

    #[test]
    fn page_size_is_clamped() {
        assert_eq!(clamp_page_size(0), 1);
        assert_eq!(clamp_page_size(1), 1);
        assert_eq!(clamp_page_size(50), 50);
        assert_eq!(clamp_page_size(500), MAX_PAGE_SIZE);
    }

    #[test]
    fn url_build_encodes_query() {
        let url = Url::parse_with_params(
            HUB_SEARCH_URL,
            &[("query", "hello world"), ("page_size", "25")],
        )
        .unwrap();
        assert!(url.as_str().contains("hello+world"));
        assert!(url.as_str().contains("page_size=25"));
    }
}
