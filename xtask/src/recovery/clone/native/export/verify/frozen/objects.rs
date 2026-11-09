use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{digest, document, evidence, reject_failure};
use crate::{
    Result,
    recovery::{
        clone::native::{
            input::BUCKETS, lifecycle::manifest, resources::common, s3::protocol::validate_key,
            target_verify::guard::Inputs,
        },
        inputs::document as proof,
    },
};

pub(super) struct Verified {
    captures: BTreeMap<String, BTreeMap<String, Value>>,
    held: Vec<(PathBuf, Inputs)>,
}

impl Verified {
    pub(super) fn finish(self) -> Result<BTreeMap<String, BTreeMap<String, Value>>> {
        for (directory, held) in &self.held {
            held.check()?;
            reject_failure(directory)?;
        }
        Ok(self.captures)
    }
}

/// 将原始小型证明集合与原生对象权威实际验证的同一批文件相连。
pub(super) fn verify(
    backend: &Path,
    root: &Path,
    export: &Value,
    files: &Value,
    authority: &Value,
) -> Result<Verified> {
    document::exact(authority, &["proofs_sha256"], "原始对象权威")?;
    let scope = document::text(&export["scope_id"], "原始对象 scope")?;
    common::scope_id(scope)?;
    let mut observed = Sha256::new();
    observed.update(b"ryframe-copy-saved-object-proofs-v1\n");
    let names = evidence::capture_names();
    let mut captures = BTreeMap::new();
    let mut held = Vec::new();
    let mut seen = BTreeSet::new();
    for bucket in document::array(&export["objects"], "原始对象桶")? {
        let name = document::text(&bucket["bucket"], "原始对象桶名")?;
        if !BUCKETS.contains(&name) || captures.contains_key(name) {
            return Err("原始对象投影的桶越界或重复".into());
        }
        let mut entries = BTreeMap::new();
        for item in document::array(&bucket["entries"], "原始对象条目")? {
            let key = document::text(&item["key"], "原始对象 key")?;
            validate_key(scope, key, false)?;
            if entries.contains_key(key) {
                return Err("原始对象投影的 key 重复".into());
            }
            let descriptor = &item["capture"];
            document::exact(descriptor, &["file", "bytes", "sha256"], "原始对象采集绑定")?;
            let capture = digest::safe_file(
                root,
                document::text(&descriptor["file"], "原始对象采集路径")?,
            )?;
            let directory = capture.parent().ok_or("对象采集缺少目录")?;
            if directory == root
                || capture.file_name().and_then(|name| name.to_str()) != Some("capture.json")
                || !seen.insert(manifest::path_text(&capture, "原始对象采集路径")?)
            {
                return Err("原始对象采集路径不同或被多个对象引用".into());
            }
            reject_failure(directory)?;
            let guard = Inputs::open([capture.clone()])?;
            let value = read_capture(backend, &capture, files, descriptor)?;
            if value["side"] != "source"
                || value["scope_id"] != scope
                || value["bucket"] != name
                || value["key"] != key
                || value["metadata"] != item["metadata"]
                || value["get_header_consistent"] != item["get_header_consistent"]
                || value["artifact"]["bytes"] != item["artifact"]["bytes"]
                || value["artifact"]["sha256"] != item["artifact"]["sha256"]
            {
                return Err("原始对象采集与登记 scope、桶、key 或导出对象不同".into());
            }
            record_proofs(&mut observed, files, directory, &names)?;
            held.push((directory.to_path_buf(), guard));
            entries.insert(key.to_owned(), value);
        }
        captures.insert(name.to_owned(), entries);
    }
    if captures.len() != BUCKETS.len() {
        return Err("原始对象投影缺少完整五桶".into());
    }
    if authority["proofs_sha256"] != hex::encode(observed.finalize()) {
        return Err("源原始小型对象证据已变化".into());
    }
    Ok(Verified { captures, held })
}

fn read_capture(backend: &Path, path: &Path, files: &Value, descriptor: &Value) -> Result<Value> {
    let text = manifest::path_text(path, "原始对象证据路径")?;
    let original = &files[text.as_str()];
    let receipt = proof::read(path, backend, "原始对象采集收据")?;
    let bytes = receipt.raw.len() as u64;
    let sha256 = receipt.sha256();
    if descriptor["bytes"].as_u64() != Some(bytes)
        || descriptor["sha256"].as_str() != Some(sha256.as_str())
        || original["bytes"].as_u64() != Some(bytes)
        || original["sha256"].as_str() != Some(sha256.as_str())
    {
        return Err("原始对象采集收据字节与原导出或证据集合不同".into());
    }
    Ok(receipt.value)
}

fn record_proofs(
    observed: &mut Sha256,
    files: &Value,
    directory: &Path,
    names: &BTreeSet<String>,
) -> Result<()> {
    // 权威摘要按原导出数组遍历，每个对象的原始证明名称固定排序。
    for name in names {
        let path = manifest::path_text(&directory.join(name), "原始对象证据路径")?;
        let actual = &files[path.as_str()];
        let bytes = actual["bytes"].as_u64().ok_or("原始对象证据大小无效")?;
        let sha = document::text(&actual["sha256"], "原始对象证据摘要")?;
        observed.update(format!("{name}\t{bytes}\t{sha}\n"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/export/verify/frozen_objects.rs"]
mod tests;
