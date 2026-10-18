use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    document::{Document, digest, exact, integer, text},
    domain::file_sha256,
};
use crate::Result;

const B0_BACKEND_COMMIT: &str = "815c5eafb09d4b493319d255fb7ab88ddd02c8b6";
const B0_FRONTEND_COMMIT: &str = "0087ea2ecf62530d042b9e52f5c950fb34c66d78";
const B0_ADAPTER_COMMIT: &str = "c05114bcdf5c369cd74087db6317ce3c8f89bee8";
const B0_ADAPTER_TREE: &str = "2ff15e7f34da1749c7eb27a1a025b9d3811b87a7";
const B0_ADAPTER_PATCH: &str = "xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch";
const B0_ADAPTER_PATCH_SHA256: &str =
    "28ad29f21ca9fd58356d059700bcc871172466e0a62286c5ccdbf68b1ed3fba3";

const MANIFEST_FIELDS: [&str; 4] = ["format_version", "kind", "source_export", "arms"];
const ARM_FIELDS: [&str; 7] = [
    "roots",
    "sources",
    "execution_sources",
    "builds",
    "adapter",
    "source_export_result_sha256",
    "source_export_identity_sha256",
];
const EXPORT_FIELDS: [&str; 12] = [
    "result",
    "origin_attempt",
    "source_registration",
    "source_rebind",
    "source_generation",
    "review_successor",
    "source_request",
    "export",
    "export_sha256",
    "generation_sha256",
    "logical_inventory_sha256",
    "identity_sha256",
];

pub(crate) fn validate(backend: &Path, document: &Document) -> Result<Value> {
    let manifest = exact(&document.value, &MANIFEST_FIELDS, "双版本来源清单")?;
    if manifest.get("format_version") != Some(&Value::from(1))
        || manifest.get("kind").and_then(Value::as_str) != Some("restore-comparison-sources")
    {
        return Err("双版本来源清单版本或类型无效".into());
    }
    let export = validate_export(backend, manifest.get("source_export").unwrap())?;
    let arms = exact(
        manifest.get("arms").unwrap(),
        &["b0", "b1"],
        "双版本来源 arms",
    )?;
    for name in ["b0", "b1"] {
        validate_arm(backend, name, arms.get(name).unwrap(), &export)?;
    }
    let mut roots = HashSet::new();
    for arm in arms.values() {
        for root in arm["roots"].as_object().unwrap().values() {
            roots.insert(PathBuf::from(text(root, "来源工作树")?));
        }
    }
    if roots.len() != 5 {
        return Err("B0/B1 产品、适配和前端必须使用五个独立工作树".into());
    }
    let b1 = arms.get("b1").unwrap();
    if head(b1, "sources", "backend") == Some(B0_BACKEND_COMMIT)
        || head(b1, "sources", "frontend") == Some(B0_FRONTEND_COMMIT)
    {
        return Err("B1 必须分别绑定区别于 B0 的最终双端源码".into());
    }
    document.assert_unchanged()?;
    Ok(document.value.clone())
}

pub(crate) fn arm<'a>(comparison: &'a Value, side: &str) -> Result<&'a Value> {
    let name = match side {
        "base" => "b0",
        "candidate" => "b1",
        _ => return Err("恢复目标侧别无效".into()),
    };
    comparison
        .get("arms")
        .and_then(|value| value.get(name))
        .ok_or_else(|| "双版本来源缺少对应 arm".into())
}

fn validate_export(backend: &Path, value: &Value) -> Result<Value> {
    let export = exact(value, &EXPORT_FIELDS, "source-export 身份")?;
    for field in [
        "result",
        "source_registration",
        "source_rebind",
        "source_generation",
        "review_successor",
        "source_request",
        "export",
    ] {
        super::document::bound_file(backend, export.get(field).unwrap(), "source-export 绑定")?;
    }
    if integer(
        export.get("origin_attempt").unwrap(),
        "source-export attempt",
    )? == 0
    {
        return Err("source-export 内部身份无效".into());
    }
    for field in [
        "export_sha256",
        "generation_sha256",
        "logical_inventory_sha256",
        "identity_sha256",
    ] {
        digest(export.get(field).unwrap(), "source-export 摘要")?;
    }
    let mut identity = export.clone();
    let expected = digest(export.get("identity_sha256").unwrap(), "source-export 身份")?;
    identity.remove("identity_sha256");
    if super::document::hash(&Value::Object(identity))? != expected {
        return Err("source-export 内部身份摘要无效".into());
    }
    Ok(Value::Object(export.clone()))
}

fn validate_arm(backend: &Path, name: &str, value: &Value, export: &Value) -> Result<()> {
    let arm = exact(value, &ARM_FIELDS, "双版本来来源 arm")?;
    let roots = exact(
        arm.get("roots").unwrap(),
        &["source_backend", "execution_backend", "frontend"],
        "来源工作树",
    )?;
    for root in roots.values() {
        let path = PathBuf::from(text(root, "来源工作树")?);
        if !path.is_absolute() {
            return Err(format!("{name} 源码根目录无效").into());
        }
    }
    exact(
        arm.get("sources").unwrap(),
        &["backend", "frontend"],
        "产品来源",
    )?;
    exact(
        arm.get("execution_sources").unwrap(),
        &["backend", "frontend"],
        "执行来源",
    )?;
    for inventory in arm["sources"]
        .as_object()
        .unwrap()
        .values()
        .chain(arm["execution_sources"].as_object().unwrap().values())
    {
        crate::recovery::clone::native::source::validate_inventory(inventory)?;
    }
    let builds = exact(
        arm.get("builds").unwrap(),
        &["backend", "frontend"],
        "arm 构建",
    )?;
    for (role, binding) in builds {
        super::document::bound_file(backend, binding, &format!("{name} {role} 构建"))?;
        super::document::binding_shape(binding, "arm 构建")?;
    }
    if arm.get("source_export_result_sha256")
        != export.get("result").and_then(|value| value.get("sha256"))
        || arm.get("source_export_identity_sha256") != export.get("identity_sha256")
    {
        return Err("B0/B1 没有共同绑定同一 source-export".into());
    }
    if name == "b0" {
        validate_b0(backend, arm.get("adapter").unwrap())?;
    } else if !arm.get("adapter").unwrap().is_null()
        || arm.get("sources") != arm.get("execution_sources")
    {
        return Err("B1 不得携带 B0 工具适配或不同执行来源".into());
    }
    Ok(())
}

fn validate_b0(backend: &Path, value: &Value) -> Result<()> {
    let adapter = exact(
        value,
        &[
            "contract",
            "base_backend_sha",
            "base_frontend_sha",
            "reference_adapter_sha",
            "adapter_tree",
            "reconstructed_tree",
            "adapter_paths",
            "patch",
        ],
        "B0 工具适配",
    )?;
    let patch = exact(
        adapter.get("patch").unwrap(),
        &["path", "bytes", "sha256"],
        "B0 适配补丁",
    )?;
    let patch_path = Path::new(text(patch.get("path").unwrap(), "B0 适配补丁")?);
    let patch_file = backend.join(patch_path);
    if adapter.get("contract").and_then(Value::as_str) != Some("legacy-stable-readiness-b0-v1")
        || adapter.get("base_backend_sha").and_then(Value::as_str) != Some(B0_BACKEND_COMMIT)
        || adapter.get("base_frontend_sha").and_then(Value::as_str) != Some(B0_FRONTEND_COMMIT)
        || adapter.get("reference_adapter_sha").and_then(Value::as_str) != Some(B0_ADAPTER_COMMIT)
        || adapter.get("adapter_tree").and_then(Value::as_str) != Some(B0_ADAPTER_TREE)
        || adapter.get("reconstructed_tree").and_then(Value::as_str) != Some(B0_ADAPTER_TREE)
        || adapter.get("adapter_paths") != Some(&serde_json::json!(["xtask/src/cli.rs"]))
        || patch.get("path").and_then(Value::as_str) != Some(B0_ADAPTER_PATCH)
        || file_sha256(&patch_file)? != B0_ADAPTER_PATCH_SHA256
        || patch.get("sha256").and_then(Value::as_str) != Some(B0_ADAPTER_PATCH_SHA256)
        || fs::metadata(&patch_file)?.len()
            != super::document::integer(patch.get("bytes").unwrap(), "B0 适配补丁大小")?
    {
        return Err("B0 工具适配身份无效".into());
    }
    Ok(())
}

fn head<'a>(arm: &'a Value, group: &str, role: &str) -> Option<&'a str> {
    arm.get(group)?
        .get(role)?
        .get("source")?
        .get("snapshot")?
        .get("head")?
        .as_str()
}
