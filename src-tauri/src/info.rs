//! V1 ID 链算法 —— 复刻 docker-tar 所依赖的两条链路：
//! 1. OCI `identity.ChainIDs`（opencontainers/image-spec/identity/chainid.go）
//!    递归计算链式 ChainID。
//! 2. spec-go/moby 的 `CreateID`（记在 v1id.go 里）
//!    由 V1Image + chainID + parent 算出 docker v1 镜像 ID。
//!
//! 字节级语义（移植时必须保持）：
//! - ChainIDs: `chainIDs[0] = diffIDs[0]`；
//!   `chainIDs[i] = sha256(chainIDs[i-1] + " " + diffIDs[i])`。
//!   注意拼接用的是「带 alg 前缀的完整 digest 字符串」（如 `sha256:xxxx`）。
//! - CreateID: 先把 ID 字段清空、序列化 V1Image，再并入 `layer_id`（及非空时 `parent`），
//!   最终对**按键名字典序排序后的 JSON** 做 sha256 —— 复用 Go `map` 序列化的有序语义
//!   （serde_json 默认 `Map` 由 BTreeMap 支撑，序列化即按 key 排序，正好对齐）。
//! - 结果 digest 统一为 `sha256:<hex>`。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 空字符串哨兵。
fn is_empty_str(s: &str) -> bool {
    s.is_empty()
}

/// 计算 `data` 的 sha256，返回完整 digest 串 `sha256:<hex>`。
pub fn digest_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    let out = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in out {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    format!("sha256:{hex}")
}

/// 逐层计算 ChainID。`diff_ids` 为 config 里的 `rootfs.diff_ids`（形如 `sha256:...`）。
/// 结果与 `diff_ids` 等长：`result[0] == diff_ids[0]`。
///
/// 对应 OCI `identity.ChainIDs`：
/// ```text
/// chainIDs[0] = diffIDs[0]
/// chainIDs[i] = sha256(chainIDs[i-1] + " " + diffIDs[i])
/// ```
pub fn chain_ids(diff_ids: &[String]) -> Vec<String> {
    if diff_ids.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(diff_ids.len());
    result.push(diff_ids[0].clone());
    for i in 1..diff_ids.len() {
        let prev = &result[i - 1];
        let input = format!("{prev} {}", diff_ids[i]);
        result.push(digest_hex(input.as_bytes()));
    }
    result
}

/// 容器运行配置，对应 spec-go/moby 的 `Config`。字段名与 JSON 键保持一致。
/// 目前只覆盖 ID 计算相关的常用字段；空配置序列化为 `{}`。
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerConfig {
    #[serde(default, skip_serializing_if = "is_empty_str")]
    pub hostname: String,
    #[serde(default, skip_serializing_if = "is_empty_str")]
    pub user: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cmd: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entrypoint: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub onbuild: Vec<String>,
    #[serde(default, skip_serializing_if = "is_empty_str")]
    pub working_dir: String,
    #[serde(default, skip_serializing_if = "is_empty_str")]
    pub image: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell: Vec<String>,
}

/// docker v1 镜像描述，对应 spec-go/moby 的 `V1Image`。
///
/// Go 语义说明（决定 JSON 输出）：
/// - `id` 带 `omitempty`：空则省略（CreateID 前会被显式清空）。
/// - `created` 无 omitempty 且为指针：nil 时输出 `null`，恒在。
/// - `container_config` 是不定大小的 struct，Go 恒输出（即使为空也输出 `{}`），因此这里不做 skip。
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct V1Image {
    #[serde(skip_serializing_if = "is_empty_str")]
    pub id: String,
    #[serde(skip_serializing_if = "is_empty_str")]
    pub parent: String,
    #[serde(skip_serializing_if = "is_empty_str")]
    pub comment: String,
    /// RFC3339Nano 字符串（由上层从 config blob 解析后传入）。
    pub created: Option<String>,
    #[serde(rename = "container_config")]
    pub container_config: ContainerConfig,
    #[serde(rename = "docker_version", skip_serializing_if = "is_empty_str")]
    pub docker_version: String,
    #[serde(skip_serializing_if = "is_empty_str")]
    pub author: String,
    #[serde(skip_serializing_if = "is_empty_str")]
    pub architecture: String,
    #[serde(skip_serializing_if = "is_empty_str")]
    pub variant: String,
    #[serde(skip_serializing_if = "is_empty_str")]
    pub os: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub size: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// 复刻 spec-go/moby `CreateID`：由当前层 `chain_id` 与父层 `parent` v1ID 计算本层 ID。
///
/// 算法：
/// 1. 复制镜像并清空 `id`。
/// 2. 序列化该镜像成 JSON 对象。
/// 3. 并入 `layer_id`（chain_id），`parent` 非空时并入 `parent`。
/// 4. 对**按 key 字典序排序**后的 JSON 求 sha256。
pub fn create_id(image: &V1Image, chain_id: &str, parent: &str) -> String {
    let mut copy = image.clone();
    copy.id = String::new();

    let mut obj = match serde_json::to_value(&copy) {
        Ok(serde_json::Value::Object(map)) => map,
        Ok(_) | Err(_) => {
            // 正常情况下一定序列化为对象；此处仅作防御。
            return String::new();
        }
    };
    obj.insert("layer_id".to_string(), serde_json::Value::String(chain_id.to_string()));
    if !parent.is_empty() {
        obj.insert("parent".to_string(), serde_json::Value::String(parent.to_string()));
    }
    // serde_json::Map 默认由 BTreeMap 支撑，序列化时按键字典序输出。
    let config_json = serde_json::to_string(&obj).unwrap_or_default();
    digest_hex(config_json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const D0: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const D1: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    // 哈希由独立 node 脚本 calc_vectors.cjs 按 Go 语义算得。
    const CHAIN0: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CHAIN1: &str =
        "sha256:ccd722928bd92476ba1745586fed6e45a102504185ad88cd89e01ff116fd146c";
    const V1ROOT: &str = "sha256:ea8c836bd43b232dc92e32735542e142a544f51cada5b583caffbafe01d191a6";
    const V1CHILD: &str =
        "sha256:2410f7c2b757dafea95b4ed24c2a61f532fcaffa7ed1c4bbbad402f008f59ac4";
    const ZERO_CREATED: &str = "0001-01-01T00:00:00Z";

    fn base_image() -> V1Image {
        V1Image {
            created: Some(ZERO_CREATED.to_string()),
            os: "linux".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn chain_ids_empty() {
        let out = chain_ids(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn chain_ids_single_layer_is_diff_id() {
        // 单层：ChainID[0] == diffID[0]（恒等，不重新哈希）。
        let out = chain_ids(&[D0.to_string()]);
        assert_eq!(out, vec![CHAIN0.to_string()]);
    }

    #[test]
    fn chain_ids_two_layers_recursive() {
        let out = chain_ids(&[D0.to_string(), D1.to_string()]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], CHAIN0);
        // ChainID[1] = sha256(ChainID[0] + " " + diffID[1])
        assert_eq!(out[1], CHAIN1);
    }

    #[test]
    fn chain_ids_three_layers_real() {
        let c2_in = digest_hex(
            format!("{CHAIN1} sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc").as_bytes(),
        );
        let out = chain_ids(&[
            D0.to_string(),
            D1.to_string(),
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
        ]);
        assert_eq!(out[0], CHAIN0);
        assert_eq!(out[1], CHAIN1);
        assert_eq!(out[2], c2_in);
    }

    #[test]
    fn create_id_root_layer_no_parent() {
        // 仅一层（根）：parent 为空，JSON 不含 parent 键。
        let id = create_id(&base_image(), CHAIN1, "");
        assert_eq!(id, V1ROOT);
    }

    #[test]
    fn create_id_child_with_parent() {
        // 第二层：parent 注入，按键排序后 parent 仍参与哈希。
        let id = create_id(&base_image(), CHAIN1, V1ROOT);
        assert_eq!(id, V1CHILD);
    }

    #[test]
    fn create_id_deterministic_and_parent_sensitive() {
        let a = create_id(&base_image(), CHAIN1, V1ROOT);
        let b = create_id(&base_image(), CHAIN1, V1ROOT);
        assert_eq!(a, b);
        // 不同 parent 得出不同 ID。
        let other = create_id(&base_image(), CHAIN0, "");
        assert_ne!(a, other);
    }

    #[test]
    fn create_id_id_field_is_cleared() {
        // image 自带的 id 不应参与哈希（Go 里会先清空）。
        let mut img = base_image();
        img.id = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
        let with_id = create_id(&img, CHAIN1, "");
        assert_eq!(with_id, V1ROOT);
    }

    #[test]
    fn chain_ids_order_sensitive() {
        // 层顺序决定 ChainID 链，两条顺序相反应得到不同结果。
        let a = chain_ids(&[D0.to_string(), D1.to_string()]);
        let b = chain_ids(&[D1.to_string(), D0.to_string()]);
        assert_ne!(a[1], b[1]);
        assert_eq!(b.len(), 2);
        // 首元素恒等于 diffID（无论顺序）。
        assert_eq!(b[0], D1);
    }

    #[test]
    fn create_id_sensitive_to_container_config() {
        // container_config 参与哈希（按键排序后 byte 级进入 sha256）。
        let a = {
            let mut i = base_image();
            i.container_config.env = vec!["A=1".to_string()];
            i
        };
        let b = {
            let mut i = base_image();
            i.container_config.env = vec!["B=1".to_string()];
            i
        };
        assert_ne!(create_id(&a, CHAIN1, ""), create_id(&b, CHAIN1, ""));
    }

    #[test]
    fn create_id_sensitive_to_metadata_fields() {
        // os / author 等元信息也参与哈希；同 config 下仍需决定。
        let bare = create_id(&base_image(), CHAIN1, V1ROOT);
        let mut img = base_image();
        img.author = "zephyr".to_string();
        assert_ne!(create_id(&img, CHAIN1, V1ROOT), bare);
    }

    #[test]
    fn serialized_v1_image_keys_are_sorted() {
        // CreateID 依赖「按键字典序的 JSON」，锚定 serde_json::Map（BTreeMap）的有序序列化。
        let img = base_image();
        let v = serde_json::to_value(&img).unwrap();
        let obj = v.as_object().unwrap();
        let keys: Vec<&String> = obj.keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "V1Image JSON 键必须按字典序序列化以对齐 Go map 语义");
    }
}