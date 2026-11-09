//! 已保存源导出的对象采集只读核验，不观察或写入源目标资源。
use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    recovery::clone::native::{
        digest,
        export::{SavedObjectInputs, bound_relative, inventory},
        input::BUCKETS,
        json as document,
    },
};

pub(super) struct ProofDigest(Sha256);

impl ProofDigest {
    fn new() -> Self {
        let mut digest = Sha256::new();
        digest.update(b"ryframe-copy-saved-object-proofs-v1\n");
        Self(digest)
    }

    pub(super) fn record(&mut self, name: &str, file: &digest::FileDigest) {
        // 固定 ASCII 基名、十进制长度和 64 位小写十六进制 SHA；对象顺序来自原导出。
        self.0
            .update(format!("{name}\t{}\t{}\n", file.bytes, file.sha256));
    }

    fn response(self) -> Value {
        json!({"proofs_sha256":hex::encode(self.0.finalize())})
    }
}

pub(in crate::recovery) fn request(args: &Value) -> Result<Value> {
    document::exact(args, &["backend", "export"], "保存对象采集核验请求")?;
    let backend = Path::new(document::text(&args["backend"], "协调后端目录")?);
    let inputs = SavedObjectInputs::load(backend, &args["export"])?;
    let result = verify_captures(backend, &inputs);
    let checked = inputs.check();
    match (result, checked) {
        (Ok(proofs), Ok(())) => Ok(proofs.response()),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(check)) => {
            Err(format!("对象采集核验失败：{error}；原绑定后验失败：{check}").into())
        }
    }
}

fn verify_captures(backend: &Path, inputs: &SavedObjectInputs) -> Result<ProofDigest> {
    let source = &inputs.request["source"];
    let scope = document::text(&source["scope_id"], "来源 scope")?;
    let prefix = format!("{scope}/");
    let max = inputs.request["max_object_bytes"]
        .as_u64()
        .ok_or("来源对象上限无效")?;
    let mut expected = inventory::object_index(&inputs.inventory)?;
    let mut buckets = BTreeSet::new();
    let mut receipts = BTreeSet::new();
    let mut proofs = ProofDigest::new();
    for bucket in document::array(&inputs.export["objects"], "源导出五桶")? {
        document::exact(bucket, &["bucket", "entries"], "源导出对象桶")?;
        let name = document::text(&bucket["bucket"], "源导出桶名")?;
        if !BUCKETS.contains(&name) || !buckets.insert(name) {
            return Err("源对象导出必须完整覆盖五桶且每桶唯一".into());
        }
        for entry in document::array(&bucket["entries"], "源导出对象条目")? {
            document::exact(
                entry,
                &[
                    "key",
                    "artifact",
                    "metadata",
                    "capture",
                    "get_header_consistent",
                ],
                "源导出对象",
            )?;
            if !receipts.insert(document::text(&entry["capture"]["file"], "对象采集路径")?) {
                return Err("对象原始证据路径重复".into());
            }
            let key = document::text(&entry["key"], "源导出对象键")?;
            let logical_key = key
                .strip_prefix(&prefix)
                .ok_or("源导出对象不属于明确来源 scope")?;
            let binding = expected
                .remove(&(name.to_owned(), logical_key.to_owned()))
                .ok_or("源对象导出重复或超出完整 inventory")?;
            verify_entry(backend, inputs, entry, name, &binding, max, &mut proofs)?;
        }
    }
    if buckets.len() != BUCKETS.len() || !expected.is_empty() {
        return Err("源对象导出缺失完整五桶或原 inventory 中的对象".into());
    }
    Ok(proofs)
}

fn verify_entry(
    backend: &Path,
    inputs: &SavedObjectInputs,
    entry: &Value,
    bucket: &str,
    expected: &Value,
    max: u64,
    proofs: &mut ProofDigest,
) -> Result<()> {
    let root = &inputs.root;
    let receipt_name = document::text(&entry["capture"]["file"], "对象采集路径")?;
    let receipt = bound_relative(root, &entry["capture"], receipt_name)?;
    let directory = receipt.parent().ok_or("对象采集缺少目录")?;
    let artifact = &entry["artifact"];
    document::exact(artifact, &["file", "bytes", "sha256"], "源导出对象载荷")?;
    let body = digest::safe_file(root, document::text(&artifact["file"], "对象载荷路径")?)?;
    if receipt.file_name().and_then(|name| name.to_str()) != Some("capture.json")
        || directory == root
        || body != directory.join("object.bin")
        || artifact["bytes"].as_u64().is_none_or(|bytes| bytes == 0)
    {
        return Err("对象字节与采集收据不属于同一明确目录".into());
    }
    let value = super::verify_bound(
        backend,
        &inputs.request["source"],
        directory,
        expected,
        max,
        Some(proofs),
    )?;
    if value["bucket"] != bucket
        || value["key"] != entry["key"]
        || value["artifact"]["bytes"] != artifact["bytes"]
        || value["artifact"]["sha256"] != artifact["sha256"]
        || value["metadata"] != entry["metadata"]
        || value["get_header_consistent"] != entry["get_header_consistent"]
    {
        return Err("对象载荷、完整存储元数据或 GET 差异标记与源证据不同".into());
    }
    bound_relative(root, &entry["capture"], receipt_name)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_object_proof_response_stays_fixed_size_for_large_vectors() {
        let empty = ProofDigest::new().response();
        let file = digest::FileDigest {
            bytes: 1024,
            sha256: "a".repeat(64),
        };
        let mut proofs = ProofDigest::new();
        for _ in 0..200_000 {
            proofs.record("owner-before-uploads.diagnostic.json", &file);
        }
        let full = proofs.response();
        assert_eq!(
            serde_json::to_vec(&full).unwrap().len(),
            serde_json::to_vec(&empty).unwrap().len()
        );
        assert_eq!(
            full["proofs_sha256"],
            "ef3314ee3bf7d554c6bf20854ba6c8f0bab04d2d71026f4a17bc7327a928b30c"
        );
    }
}
