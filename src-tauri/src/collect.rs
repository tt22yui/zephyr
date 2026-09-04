//! V1 内容收集器 —— 复刻 docker-tar 的 `ImageContentCollector`。
//!
//! 把 registry 抓到的元数据（diff_ids、blob digests、config）编排成 docker v1 保存格式：
//! - 用 `info::chain_ids` 算 ChainID 链
//! - 逐层 `info::create_id` 算 v1 ID，构造各层 `<v1ID>/json`
//! - 生成 `manifest.json` 与 `repositories`
//! - 记录共享 blob（空层）→ 供后续写 layer.tar 时做软链
//!
//! 字节级复刻点（来自上游源码）：
//! - 非末层算 digest 时 `os` 为空字符串；存 json 时才填入 config 的 os。
//! - 末层 image 直接「反序列化自 config blob」，故 created/architecture/author/os 都来自 config。
//! - `manifest.json` 的 `Config` 字段 = `<configDigest>.json`。

use crate::config::OciConfig;
use crate::endpoint::ImageRef;
use crate::info::{self, V1Image};
use serde::Serialize;
use std::collections::BTreeMap;

const ZERO_CREATED: &str = "0001-01-01T00:00:00Z";

/// 一层的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerInfo {
    pub v1_id: String,
    /// `<v1ID>/json` 的字节内容。
    pub json: Vec<u8>,
    /// 父层 v1 ID（根层为空）。
    pub parent: String,
}

/// 收集结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectOutput {
    pub layers: Vec<LayerInfo>,
    /// 顶层（最后一层）v1 ID。
    pub top_id: String,
    pub manifest_json: Vec<u8>,
    pub repositories_json: Vec<u8>,
    /// 共享 blob（多次出现的层，空层）的 digest 列表，需为其除首个外的 v1ID 建软链。
    pub empty_layer_blob_sums: Vec<String>,
    /// blob digest → 引用它的 v1ID 列表。
    pub blob_sum_v1: BTreeMap<String, Vec<String>>,
}

#[derive(Serialize)]
struct Summary {
    Config: String,
    RepoTags: Vec<String>,
    Layers: Vec<String>,
}

/// 执行收集。
///
/// - `diff_ids`：来自 config blob 的 `rootfs.diff_ids`（与层一一对应）。
/// - `blob_digests`：manifest 里的各层 blob digest（与 `diff_ids` 等长）。
/// - `config_digest`：config blob 的 digest（用于 `manifest.json` 的 Config 文件名）。
pub fn collect(
    image: &ImageRef,
    diff_ids: &[String],
    blob_digests: &[String],
    config_digest: &str,
    config: &OciConfig,
) -> Result<CollectOutput, String> {
    if blob_digests.len() < diff_ids.len() {
        return Err(format!(
            "blob digest 数({}) 少于 diff_id 数({})",
            blob_digests.len(),
            diff_ids.len()
        ));
    }
    let chain = info::chain_ids(diff_ids);
    let last_v1_image = config.to_last_v1_image();

    let mut layers = Vec::with_capacity(chain.len());
    let mut v1_ids = Vec::with_capacity(chain.len());
    let mut blob_sum_v1: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut empty_layer_blob_sums: Vec<String> = Vec::new();
    let mut parent = String::new();

    for (index, chain_id) in chain.iter().enumerate() {
        // 非末层只给 created=zero；末层直接取 config 反序列化的 image。
        let mut v1_image: V1Image = if index == chain.len() - 1 {
            last_v1_image.clone()
        } else {
            V1Image {
                created: Some(ZERO_CREATED.to_string()),
                ..Default::default()
            }
        };

        let v1_id = info::create_id(&v1_image, chain_id, &parent);

        // 记 json 前才设置 os / id / parent（与上游一致）。
        v1_image.os = config.os.clone();
        v1_image.id = v1_id.clone();
        if !parent.is_empty() {
            v1_image.parent = parent.clone();
        }
        let json = serde_json::to_vec(&v1_image)
            .map_err(|e| format!("序列化 V1Image 失败: {e}"))?;

        // 共享 blob 跟踪：同一 blob 被多个层引用时需要软链。
        let blob = &blob_digests[index];
        let list = blob_sum_v1.entry(blob.clone()).or_default();
        if !list.is_empty() {
            empty_layer_blob_sums.push(blob.clone());
        }
        list.push(v1_id.clone());

        layers.push(LayerInfo {
            v1_id: v1_id.clone(),
            parent: parent.clone(),
            json,
        });
        parent = v1_id.clone();
        v1_ids.push(v1_id);
    }

    let top_id = v1_ids.last().cloned().unwrap_or_default();

    // manifest.json
    let mut summary_layers = Vec::with_capacity(v1_ids.len());
    for vid in &v1_ids {
        summary_layers.push(format!("{vid}/layer.tar"));
    }
    let summary = Summary {
        Config: format!("{config_digest}.json"),
        RepoTags: vec![image.display_name()],
        Layers: summary_layers,
    };
    let mjson = serde_json::to_string(&vec![summary]).map_err(|e| format!("序列化 manifest 失败: {e}"))?;
    let mut manifest_json = mjson.into_bytes();
    manifest_json.push(b'\n');

    // repositories：{ repo: { tag: topID } }，serde_json Map 按键排序与 Go map 一致。
    let mut repositories: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut version_map = serde_json::Map::new();
    version_map.insert(image.tag.clone(), serde_json::Value::String(top_id.clone()));
    repositories.insert(image.repo_name(), serde_json::Value::Object(version_map));
    let rjson = serde_json::to_string(&repositories)
        .map_err(|e| format!("序列化 repositories 失败: {e}"))?;
    let mut repositories_json = rjson.into_bytes();
    repositories_json.push(b'\n');

    Ok(CollectOutput {
        layers,
        top_id,
        manifest_json,
        repositories_json,
        empty_layer_blob_sums,
        blob_sum_v1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OciConfig;
    use crate::endpoint;
    
    const CFG: &str = r#"{
        "architecture":"amd64","os":"linux",
        "config":{"Env":["PATH=/x"],"Cmd":["a"],"WorkingDir":"/"},
        "created":"2023-05-01T09:00:00Z","docker_version":"24.0.5",
        "rootfs":{"type":"layers","diff_ids":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]}
    }"#;

    fn setup() -> (ImageRef, Vec<String>, Vec<String>, String, OciConfig) {
        let image = endpoint::parse("nginx:1.25").unwrap();
        let diff = vec![
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        ];
        let blobs = diff.clone();
        let cfg = OciConfig::from_json(CFG).unwrap();
        (image, diff, blobs, "sha256:configdigestconfigdigestconfigdigestconfigdigest".into(), cfg)
    }

    #[test]
    fn one_layer_per_diff_id() {
        let (image, diff, blobs, cdigest, cfg) = setup();
        let out = collect(&image, &diff, &blobs, &cdigest, &cfg).unwrap();
        assert_eq!(out.layers.len(), 2);
        assert_eq!(out.layers.len(), diff.len());
    }

    #[test]
    fn parent_chain_linking() {
        let (image, diff, blobs, cdigest, cfg) = setup();
        let out = collect(&image, &diff, &blobs, &cdigest, &cfg).unwrap();
        // 根层无 parent
        assert!(out.layers[0].parent.is_empty());
        // 第二层 parent 指向第一层 id
        assert_eq!(out.layers[1].parent, out.layers[0].v1_id);
        // top_id == 最后一层 id
        assert_eq!(out.top_id, out.layers[1].v1_id);
    }

    #[test]
    fn stored_json_has_id_and_os() {
        let (image, diff, blobs, cdigest, cfg) = setup();
        let out = collect(&image, &diff, &blobs, &cdigest, &cfg).unwrap();
        let root: serde_json::Value = serde_json::from_slice(&out.layers[0].json).unwrap();
        assert_eq!(root["id"], out.layers[0].v1_id);
        assert_eq!(root["os"], "linux");
        // 根层 stored json 不含 parent
        assert!(root.get("parent").is_none());
        let child: serde_json::Value = serde_json::from_slice(&out.layers[1].json).unwrap();
        assert_eq!(child["parent"], out.layers[0].v1_id);
    }

    #[test]
    fn manifest_and_repositories_shape() {
        let (image, diff, blobs, cdigest, cfg) = setup();
        let out = collect(&image, &diff, &blobs, &cdigest, &cfg).unwrap();

        let m: Vec<serde_json::Value> = serde_json::from_slice(&out.manifest_json).unwrap();
        assert_eq!(m.len(), 1);
        let expected_cfg = format!("{cdigest}.json");
        assert_eq!(m[0]["Config"].as_str(), Some(expected_cfg.as_str()));
        assert_eq!(m[0]["RepoTags"][0].as_str(), Some("nginx:1.25"));
        let expected_l0 = format!("{}/layer.tar", out.layers[0].v1_id);
        let expected_l1 = format!("{}/layer.tar", out.layers[1].v1_id);
        assert_eq!(m[0]["Layers"][0].as_str(), Some(expected_l0.as_str()));
        assert_eq!(m[0]["Layers"][1].as_str(), Some(expected_l1.as_str()));

        let r: serde_json::Value = serde_json::from_slice(&out.repositories_json).unwrap();
        assert_eq!(r["nginx"]["1.25"].as_str(), Some(out.top_id.as_str()));
    }

    #[test]
    fn shared_blob_marks_empty_layer() {
        let (image, diff, _, cdigest, cfg) = setup();
        // 两层引用同一 blob digest（模拟空层重复）
        let shared = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let blobs = vec![shared.to_string(), shared.to_string()];
        let out = collect(&image, &diff, &blobs, &cdigest, &cfg).unwrap();
        assert_eq!(out.empty_layer_blob_sums, vec![shared.to_string()]);
        let ids = out.blob_sum_v1.get(shared).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], out.layers[0].v1_id);
        assert_eq!(ids[1], out.layers[1].v1_id);
    }

    #[test]
    fn deterministic_and_len_mismatch_err() {
        let (image, diff, blobs, cdigest, cfg) = setup();
        let a = collect(&image, &diff, &blobs, &cdigest, &cfg).unwrap();
        let b = collect(&image, &diff, &blobs, &cdigest, &cfg).unwrap();
        assert_eq!(a, b);
        // 缺少 blob digest 应报错
        assert!(collect(&image, &diff, &blobs[..1], &cdigest, &cfg).is_err());
    }
}
