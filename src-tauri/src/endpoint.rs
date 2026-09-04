//! 镜像引用解析 —— 对应 docker-tar 的 request info / image info。
//!
//! 把用户输入的镜像串（如 `nginx`、`home-assistant/home-assistant:stable`、
//! `ghcr.io/org/app:1.0`、`localhost:5000/img@sha256:...`）解析成
//! registry / 仓库路径 / tag / digest，供后续 Registry v2 API 构造 URL。
//!
//! 语义对齐 Docker Registry HTTP API v2 的 reference 规则：
//! - 首段如果含 `.` 或 `:`，或等于 `localhost`，则视为 registry 主机；
//!   否则缺省为 Docker Hub。
//! - Docker Hub 会规范化 API 主机为 `registry-1.docker.io`；官方镜像缺省命名空间 `library`。
//! - tag 缺省为 `latest`；`@sha256:` 解析进 digest。

/// 规范化后的镜像引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    /// API 主机（不含 scheme），Docker Hub 规范化为 `registry-1.docker.io`。
    pub registry: String,
    /// 仓库路径，如 `library/nginx`、`org/app`。
    pub repo_path: String,
    /// 末段组件名（`nginx`、`app`），仅用于展示。
    pub name: String,
    pub tag: String,
    /// 若通过 digest 引用则非空。
    pub digest: Option<String>,
}

impl ImageRef {
    /// Manifest 引用：digest 优先，否则用 tag。
    pub fn manifest_ref(&self) -> &str {
        match &self.digest {
            Some(d) => d,
            None => &self.tag,
        }
    }

    /// Docker Hub 官方镜像不带 `library/` 前缀的仓库名（如 `nginx`、`home-assistant/home-assistant`）。
    pub fn repo_name(&self) -> String {
        if self.registry == "registry-1.docker.io" {
            self.repo_path.trim_start_matches("library/").to_string()
        } else {
            self.repo_path.clone()
        }
    }

    /// 形如 `nginx:latest` 的展示名（Docker Hub 官方镜像不带 `library/` 前缀）。
    pub fn display_name(&self) -> String {
        let raw_path = self.repo_name();
        match &self.digest {
            Some(d) => format!("{raw_path}@{d}"),
            None => format!("{raw_path}:{}", self.tag),
        }
    }
}

/// 判断某段是否像是 registry 主机（含 `.`/`:` 或就是 `localhost`）。
fn is_host_like(s: &str) -> bool {
    s.contains('.') || s.contains(':') || s.eq_ignore_ascii_case("localhost")
}

/// 把 Docker Hub 的各种主机别名统一为 API 主机 `registry-1.docker.io`。
///
/// 用户常写 `docker.io/xxx`、`index.docker.io/xxx`，但 `docker.io` 是营销域名，
/// 其 `https://docker.io/v2/` 会 302 到 `www.docker.com`，无法走 registry v2 API。
fn normalize_registry_host(host: &str) -> String {
    let h = host.to_ascii_lowercase();
    match h.as_str() {
        "docker.io"
        | "index.docker.io"
        | "registry.hub.docker.com"
        | "registry.docker.io"
        | "reg-1.docker.io" => default_registry_host(),
        _ => host.to_string(),
    }
}

/// 解析镜像引用。失败返回可读错误信息。
pub fn parse(input: &str) -> Result<ImageRef, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("镜像名不能为空".to_string());
    }
    if s.contains(char::is_whitespace) {
        return Err(format!("镜像名含空白字符: {s}"));
    }

    // 1) 先切 digest（最后一个 `@`）。
    let (name_part, digest) = match s.rfind('@') {
        Some(idx) => {
            let d = s[idx + 1..].to_string();
            if d.is_empty() {
                return Err(format!("digest 为空: {s}"));
            }
            (&s[..idx], Some(d))
        }
        None => (s, None),
    };
    if name_part.is_empty() {
        return Err(format!("缺少镜像名: {s}"));
    }

    // 2) 切 registry（依据首段是否 host-like；host 可能含 `:` 端口，须在切 tag 之前处理）。
    let (registry, path) = match name_part.find('/') {
        Some(idx) => {
            let first = &name_part[..idx];
            if is_host_like(first) {
                (normalize_registry_host(first), &name_part[idx + 1..])
            } else {
                (default_registry_host(), name_part)
            }
        }
        None => (default_registry_host(), name_part),
    };
    if path.is_empty() {
        return Err(format!("缺少仓库名: {s}"));
    }

    // 3) 切 tag（host 已剥离，剩余 `:` 即为 tag 分隔符）。
    let (path_no_tag, tag) = match path.rfind(':') {
        Some(idx) => {
            let t = &path[idx + 1..];
            if t.is_empty() {
                return Err(format!("tag 为空: {s}"));
            }
            (&path[..idx], t.to_string())
        }
        None => (path, "latest".to_string()),
    };
    if path_no_tag.is_empty() {
        return Err(format!("缺少仓库名: {s}"));
    }

    // 4) 组装仓库路径与末段名。
    let comps: Vec<&str> = path_no_tag.split('/').filter(|c| !c.is_empty()).collect();
    if comps.is_empty() {
        return Err(format!("仓库名为空: {s}"));
    }
    let is_docker_hub = registry == "registry-1.docker.io";
    let repo_path = if is_docker_hub && comps.len() == 1 {
        // Docker Hub 官方镜像：补 `library` 命名空间。
        format!("library/{}", comps[0])
    } else {
        comps.join("/")
    };
    let name = comps[comps.len() - 1].to_string();

    Ok(ImageRef {
        registry,
        repo_path,
        name,
        tag,
        digest,
    })
}

/// Docker Hub 的 API 主机。
fn default_registry_host() -> String {
    "registry-1.docker.io".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUB: &str = "registry-1.docker.io";

    fn assert_ref(r: &ImageRef, registry: &str, repo: &str, name: &str, tag: &str) {
        assert_eq!(r.registry, registry);
        assert_eq!(r.repo_path, repo);
        assert_eq!(r.name, name);
        assert_eq!(r.tag, tag);
    }

    #[test]
    fn parse_official_image_short() {
        let r = parse("nginx").unwrap();
        assert_ref(&r, HUB, "library/nginx", "nginx", "latest");
        assert_eq!(r.manifest_ref(), "latest");
        assert_eq!(r.display_name(), "nginx:latest");
    }

    #[test]
    fn parse_official_image_with_tag() {
        let r = parse("nginx:1.25").unwrap();
        assert_ref(&r, HUB, "library/nginx", "nginx", "1.25");
    }

    #[test]
    fn parse_official_image_full_namespace() {
        let r = parse("library/nginx:1.25").unwrap();
        assert_ref(&r, HUB, "library/nginx", "nginx", "1.25");
    }

    #[test]
    fn parse_user_image() {
        let r = parse("home-assistant/home-assistant:stable").unwrap();
        assert_ref(&r, HUB, "home-assistant/home-assistant", "home-assistant", "stable");
        assert_eq!(r.display_name(), "home-assistant/home-assistant:stable");
    }

    #[test]
    fn parse_custom_registry() {
        let r = parse("ghcr.io/home-assistant/home-assistant:stable").unwrap();
        assert_ref(&r, "ghcr.io", "home-assistant/home-assistant", "home-assistant", "stable");
    }

    #[test]
    fn parse_custom_registry_single_component() {
        let r = parse("ghcr.io/foo").unwrap();
        assert_ref(&r, "ghcr.io", "foo", "foo", "latest");
    }

    #[test]
    fn parse_docker_io_prefix_normalizes_host() {
        let r = parse("docker.io/motrixapp/motrix-server:latest").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repo_path, "motrixapp/motrix-server");
        assert_eq!(r.tag, "latest");

        let ri = parse("index.docker.io/library/nginx").unwrap();
        assert_eq!(ri.registry, "registry-1.docker.io");
    }

    #[test]
    fn parse_custom_registry_kept_as_is() {
        let r = parse("ghcr.io/org/app").unwrap();
        assert_eq!(r.registry, "ghcr.io");
    }

    #[test]
    fn parse_localhost_with_port() {
        let r = parse("localhost:5000/img:latest").unwrap();
        assert_ref(&r, "localhost:5000", "img", "img", "latest");
    }

    #[test]
    fn parse_localhost_port_no_tag() {
        let r = parse("localhost:5000/myapp").unwrap();
        assert_ref(&r, "localhost:5000", "myapp", "myapp", "latest");
    }

    #[test]
    fn parse_digest_reference() {
        let r = parse("nginx@sha256:abc").unwrap();
        assert_ref(&r, HUB, "library/nginx", "nginx", "latest");
        assert_eq!(r.digest.as_deref(), Some("sha256:abc"));
        assert_eq!(r.manifest_ref(), "sha256:abc");
    }

    #[test]
    fn parse_errors() {
        assert!(parse("").is_err());
        assert!(parse("  ").is_err());
        assert!(parse("nginx:").is_err());
        assert!(parse("nginx@").is_err());
        assert!(parse("nginx:x y").is_err());
    }

    #[test]
    fn parse_host_normalization_is_case_insensitive() {
        // docker.io 的大小写别名都应归一化到 API 主机。
        let r = parse("DOCKER.IO/motrixapp/motrix-server:latest").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repo_path, "motrixapp/motrix-server");
        let upper = parse("INDEX.DOCKER.IO/library/nginx").unwrap();
        assert_eq!(upper.registry, "registry-1.docker.io");
    }

    #[test]
    fn parse_localhost_without_port_is_host() {
        let r = parse("localhost/foo/bar:1").unwrap();
        assert_eq!(r.registry, "localhost");
        assert_eq!(r.repo_path, "foo/bar");
        assert_eq!(r.tag, "1");
    }

    #[test]
    fn parse_registry_port_plus_digest() {
        let r = parse("localhost:5000/img@sha256:abc").unwrap();
        assert_ref(&r, "localhost:5000", "img", "img", "latest");
        assert_eq!(r.digest.as_deref(), Some("sha256:abc"));
        assert_eq!(r.manifest_ref(), "sha256:abc");
    }

    #[test]
    fn parse_custom_registry_digest_reference() {
        let r = parse("ghcr.io/org/app@sha256:deadbeef").unwrap();
        assert_ref(&r, "ghcr.io", "org/app", "app", "latest");
        assert_eq!(r.digest.as_deref(), Some("sha256:deadbeef"));
        // digest 引用时 display_name 不带 tag。
        assert_eq!(r.display_name(), "org/app@sha256:deadbeef");
    }

    #[test]
    fn parse_both_tag_and_digest_prefers_digest() {
        // tag + digest 同时存在（罕见但合法）：manifest 以 digest 为准。
        let r = parse("nginx:1.25@sha256:abc").unwrap();
        assert_ref(&r, HUB, "library/nginx", "nginx", "1.25");
        assert_eq!(r.manifest_ref(), "sha256:abc");
    }

    #[test]
    fn parse_host_only_slash_rejected() {
        // registry 后跟空仓库路径。
        assert!(parse("ghcr.io/").is_err());
    }

    #[test]
    fn parse_filters_empty_segments_in_path() {
        // 仓库路径中的连续斜杠会产生空段，应被过滤而不报错/成空。
        let r = parse("nginx//latest").unwrap();
        assert_eq!(r.repo_path, "nginx/latest");
        assert_eq!(r.name, "latest");
        assert_eq!(r.tag, "latest");
        // 首尾斜杠同理被折叠。
        let r2 = parse("/nginx/").unwrap();
        assert_eq!(r2.repo_path, "library/nginx");
        assert_eq!(r2.name, "nginx");
    }
}