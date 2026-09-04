use crate::endpoint::ImageRef;
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerChallenge {
    pub realm: String,
    pub service: Option<String>,
    pub scope: String,
}

#[derive(Debug, Clone, Default)]
pub struct Credentials {
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Credentials {
    pub fn is_empty(&self) -> bool {
        self.username.as_deref().unwrap_or("").is_empty()
    }
}

pub fn default_scope(image: &ImageRef) -> String {
    format!("repository:{}:pull", image.repo_path)
}

fn parse_auth_params(after_scheme: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = after_scheme.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t' || b[i] == b',') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'-') {
            i += 1;
        }
        let key = &after_scheme[start..i];
        let mut j = i;
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        if j >= b.len() || b[j] != b'=' {
            if key.is_empty() {
                i += 1;
            } else {
                out.push((key.to_string(), String::new()));
                i = j;
            }
            continue;
        }
        i = j + 1;
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        if i < b.len() && b[i] == b'"' {
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            let val = &after_scheme[vs..i];
            if i < b.len() {
                i += 1;
            }
            out.push((key.to_string(), val.to_string()));
        } else {
            let vs = i;
            while i < b.len() && b[i] != b',' && b[i] != b' ' {
                i += 1;
            }
            out.push((key.to_string(), after_scheme[vs..i].to_string()));
        }
    }
    out
}

pub fn parse_bearer_challenge(header: &str) -> Option<BearerChallenge> {
    let h = header.trim();
    let after: &str = match h.find(' ') {
        Some(i) => {
            let scheme = &h[..i];
            if !scheme.eq_ignore_ascii_case("bearer") {
                return None;
            }
            &h[i..]
        }
        None => {
            if h.eq_ignore_ascii_case("bearer") {
                ""
            } else {
                return None;
            }
        }
    };
    let params = parse_auth_params(after);
    let realm = params.iter().find(|(k, _)| k == "realm")?.1.clone();
    if realm.is_empty() {
        return None;
    }
    let service = params
        .iter()
        .find(|(k, _)| k == "service")
        .map(|(_, v)| v.clone())
        .filter(|s| !s.is_empty());
    let scope = params
        .iter()
        .find(|(k, _)| k == "scope")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    Some(BearerChallenge { realm, service, scope })
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token: Option<String>,
    #[serde(default, rename = "access_token")]
    access_token: Option<String>,
}

#[allow(clippy::borrowed_box)]
pub async fn fetch_token(
    client: &Client,
    challenge: &BearerChallenge,
    scope: &str,
    creds: &Credentials,
) -> Result<String, String> {
    let mut url = reqwest::Url::parse(&challenge.realm).map_err(|e| format!("realm 解析失败: {e}"))?;
    {
        let mut qp = url.query_pairs_mut();
        if let Some(service) = &challenge.service {
            qp.append_pair("service", service);
        }
        qp.append_pair("scope", scope);
    }
    let mut builder = client.get(url);
    if !creds.is_empty() {
        builder = builder.basic_auth(creds.username.clone().unwrap_or_default(), creds.password.clone());
    }
    let resp = builder.send().await.map_err(|e| format!("请求 token 失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("获取 token 失败: {status}: {body}"));
    }
    let t: TokenResponse = resp.json().await.map_err(|e| format!("解析 token 响应失败: {e}"))?;
    t.token
        .or(t.access_token)
        .ok_or_else(|| "token 响应中未找到 token".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_docker_hub_challenge() {
        let c = parse_bearer_challenge(
            r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/nginx:pull""#,
        )
        .unwrap();
        assert_eq!(c.realm, "https://auth.docker.io/token");
        assert_eq!(c.service.as_deref(), Some("registry.docker.io"));
        assert_eq!(c.scope, "repository:library/nginx:pull");
    }

    #[test]
    fn parse_reject_basic_or_absent() {
        assert!(parse_bearer_challenge("Basic realm=\"x\"").is_none());
        assert!(parse_bearer_challenge("realm=\"x\"").is_none());
        assert!(parse_bearer_challenge("Bearer").is_none());
        assert!(parse_bearer_challenge("").is_none());
    }

    #[test]
    fn parse_missing_realm_returns_none() {
        assert!(parse_bearer_challenge(r#"Bearer service="a.docker.io""#).is_none());
    }

    #[test]
    fn parse_unquoted_values() {
        let c = parse_bearer_challenge("Bearer realm=https://auth.example/token,service=auth.example").unwrap();
        assert_eq!(c.realm, "https://auth.example/token");
        assert_eq!(c.service.as_deref(), Some("auth.example"));
        assert_eq!(c.scope, "");
    }

    #[test]
    fn parse_no_service() {
        let c = parse_bearer_challenge(r#"Bearer realm="https://auth.example/token""#).unwrap();
        assert_eq!(c.service, None);
    }

    #[test]
    fn scope_matches_repo_path() {
        let img = crate::endpoint::parse("nginx").unwrap();
        assert_eq!(default_scope(&img), "repository:library/nginx:pull");
    }
}
