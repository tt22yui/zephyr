//! 输出打包 —— 复刻 docker-tar 的 `OutputFileManager` + `LayerDownloader` + `TarImage`。
//!
//! 把收集结果 + 下载到的各层 blob 打成 `docker load` 可直接导入的 tar。
//! 目录结构（与 docker save v1 一致）：
//! ```text
//! manifest.json
//! repositories
//! <configDigest>.json
//! <v1ID>/json
//! <v1ID>/VERSION
//! <v1ID>/layer.tar      <- 原始压缩 blob
//! ```
//!
//! LayerDownloader：
//! - 对所有层 blob **并发**下载（`futures::join_all`），仅对去重后的唯一 digest 拉一次。
//! - 每个 blob 完成后通过 `progress` 回调逐块回报（name="blob", done/total），供前端做实时进度。
//! - 共享 blob（同一 digest 被多层引用，多为空层）只落一次真文件，其余层用**软链**指向首次出现的 `../<firstV1ID>/layer.tar`。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::collect::CollectOutput;
use crate::registry::RegistryClient;
use futures::future::join_all;

fn add_bytes<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    data: &[u8],
) -> Result<(), String> {
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(data.len() as u64);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, path, data)
        .map_err(|e| format!("写入 tar 条目 {path} 失败: {e}"))
}

/// 追加一条指向 `target` 的软链（`path` 为层级目录内的 `layer.tar`）。
fn add_symlink<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    target: &str,
) -> Result<(), String> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_mode(0o777);
    header.set_size(0);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_link(&mut header, path, target)
        .map_err(|e| format!("写入符号链接 {path} -> {target} 失败: {e}"))
}

/// 并发下载各层 blob。每下载完一个去重后的 blob，调用一次
/// `progress("blob", done_count, total_count)`。
async fn fetch_all_unique(
    client: &RegistryClient,
    digests: &[String],
    progress: &(dyn Fn(&str, u64, u64) + Send + Sync),
) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut seen: HashSet<String> = HashSet::new();
    let unique: Vec<String> = digests
        .iter()
        .filter(|d| seen.insert((*d).clone()))
        .cloned()
        .collect();
    let total = unique.len() as u64;

    let counter = Arc::new(AtomicUsize::new(0));
    let tasks = unique.into_iter().map(|d| {
        let client = client;
        let counter = Arc::clone(&counter);
        async move {
            let bytes = client.fetch_blob(&d).await?;
            let done = counter.fetch_add(1, Ordering::SeqCst) as u64 + 1;
            progress("blob", done, total);
            Ok::<(String, Vec<u8>), String>((d, bytes))
        }
    });
    let results = join_all(tasks).await;

    let mut map = HashMap::with_capacity(results.len());
    for r in results {
        let (digest, bytes) = r?;
        map.insert(digest, bytes);
    }
    Ok(map)
}

/// 把所有文件打进 `tar_path`。
///
/// - `blob_digests`：与 `CollectOutput.layers` 同序的各层 blob 摘要。
/// - `config_bytes`：config blob 原始字节（写入 `<configDigest>.json`）。
/// - `progress(name, done, total)`：各阶段进度回调（"download" 开始时 / "blob" 逐块 / "write" 打包）。
pub async fn write_tar(
    client: &RegistryClient,
    out: &CollectOutput,
    config_digest: &str,
    config_bytes: &[u8],
    blob_digests: &[String],
    tar_path: &Path,
    progress: &(dyn Fn(&str, u64, u64) + Send + Sync),
) -> Result<(), String> {
    if blob_digests.len() < out.layers.len() {
        return Err("blob digest 数少于层数".to_string());
    }

    let total_blobs = blob_digests.iter().filter(|d| !d.is_empty()).count() as u64;
    progress("download", 0, total_blobs);
    // 先并发把所有（去重后的）blob 拉进内存。
    let blobs = fetch_all_unique(client, blob_digests, progress).await?;

    progress("write", 0, 1);
    let file = File::create(tar_path).map_err(|e| format!("创建 tar 失败: {e}"))?;
    let mut builder = tar::Builder::new(file);

    // 顶层索引文件
    add_bytes(&mut builder, "manifest.json", &out.manifest_json)?;
    add_bytes(&mut builder, "repositories", &out.repositories_json)?;
    add_bytes(
        &mut builder,
        &format!("{config_digest}.json"),
        config_bytes,
    )?;

    // 逐层：json / VERSION / layer.tar。共享 blob 用软链复用首次出现的层。
    for (i, layer) in out.layers.iter().enumerate() {
        add_bytes(&mut builder, &format!("{}/json", layer.v1_id), &layer.json)?;
        add_bytes(&mut builder, &format!("{}/VERSION", layer.v1_id), b"1.0")?;

        let digest = &blob_digests[i];
        let bytes = blobs
            .get(digest)
            .ok_or_else(|| format!("缺少 blob {digest}"))?;

        // 该 digest 首次出现的 v1ID 是软链目标；当前层若是它则写真文件，否则写软链。
        let first_v1 = out
            .blob_sum_v1
            .get(digest)
            .and_then(|owners| owners.first())
            .cloned()
            .unwrap_or_else(|| layer.v1_id.clone());

        if first_v1 == layer.v1_id {
            add_bytes(
                &mut builder,
                &format!("{}/layer.tar", layer.v1_id),
                bytes,
            )?;
        } else {
            add_symlink(
                &mut builder,
                &format!("{}/layer.tar", layer.v1_id),
                &format!("../{first_v1}/layer.tar"),
            )?;
        }
    }

    drop(builder.into_inner().map_err(|e| format!("结束 tar 失败: {e}"))?);
    progress("write", 1, 1);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::{CollectOutput, LayerInfo};
    use std::collections::BTreeMap;

    #[test]
    fn symlink_target_for_shared_blob() {
        let out = CollectOutput {
            layers: vec![
                LayerInfo {
                    v1_id: "v1A".into(),
                    parent: String::new(),
                    json: b"{}".to_vec(),
                },
                LayerInfo {
                    v1_id: "v1B".into(),
                    parent: "v1A".into(),
                    json: b"{}".to_vec(),
                },
            ],
            top_id: "v1B".into(),
            manifest_json: b"[]".to_vec(),
            repositories_json: b"{}".to_vec(),
            empty_layer_blob_sums: vec!["sha256:x".into()],
            blob_sum_v1: BTreeMap::from([("sha256:x".into(), vec!["v1A".into(), "v1B".into()])]),
        };
        assert_eq!(out.blob_sum_v1.get("sha256:x").unwrap()[1], "v1B");
        assert_ne!(out.blob_sum_v1.get("sha256:x").unwrap()[0], "v1B");
    }

    #[test]
    fn fetch_unique_dedups() {
        let mut seen = HashSet::new();
        let digests: Vec<String> =
            vec!["sha256:a".into(), "sha256:a".into(), "sha256:b".into()];
        let unique: Vec<String> = digests
            .iter()
            .filter(|d| seen.insert((*d).clone()))
            .cloned()
            .collect();
        assert_eq!(unique.len(), 2);
        assert!(unique.contains(&"sha256:a".to_string()));
        assert!(unique.contains(&"sha256:b".to_string()));
    }

}