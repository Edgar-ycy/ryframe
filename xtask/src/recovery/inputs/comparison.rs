use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::document::{Document, digest, exact, integer, text};
use crate::Result;

use crate::recovery::b0;

const MANIFEST_FIELDS: [&str; 4] = ["format_version", "kind", "source_export", "arms"];
const ARM_FIELDS: [&str; 8] = [
    "roots",
    "sources",
    "execution_sources",
    "builds",
    "backend_environment",
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
    if manifest.get("format_version") != Some(&Value::from(2))
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
        validate_arm(name, arms.get(name).unwrap(), &export)?;
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
    if head(b1, "sources", "backend") == Some(b0::BACKEND)
        || head(b1, "sources", "frontend") == Some(b0::FRONTEND)
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

fn validate_arm(name: &str, value: &Value, export: &Value) -> Result<()> {
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
    let execution_root = Path::new(text(&roots["execution_backend"], "执行工作树")?);
    super::document::bound_file(execution_root, &arm["backend_environment"], "后端构建环境")?;
    validate_builds(value)?;
    if arm.get("source_export_result_sha256")
        != export.get("result").and_then(|value| value.get("sha256"))
        || arm.get("source_export_identity_sha256") != export.get("identity_sha256")
    {
        return Err("B0/B1 没有共同绑定同一 source-export".into());
    }
    if name == "b0" {
        validate_b0(arm.get("adapter").unwrap())?;
    } else if !arm.get("adapter").unwrap().is_null()
        || arm.get("sources") != arm.get("execution_sources")
    {
        return Err("B1 不得携带 B0 工具适配或不同执行来源".into());
    }
    Ok(())
}

pub(super) fn validate_builds(value: &Value) -> Result<()> {
    use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};

    let roots = &value["roots"];
    let execution = Path::new(text(&roots["execution_backend"], "后端执行根")?);
    validate_absolute_path(execution, LocalTestPathKind::ExistingDirectory)?;
    let frontend = Path::new(text(&roots["frontend"], "前端执行根")?);
    validate_absolute_path(frontend, LocalTestPathKind::ExistingDirectory)?;
    let builds = exact(&value["builds"], &["backend", "frontend"], "产品执行构建")?;
    super::document::bound_file(execution, &builds["backend"], "后端构建收据")?;

    let expected = frontend.join("dist/.vite/restore-build.json");
    let recorded = super::document::binding_shape(&builds["frontend"], "前端构建收据")?;
    if recorded.path != expected {
        return Err("前端构建收据必须位于本侧执行根的固定 dist 路径".into());
    }
    validate_absolute_path(&recorded.path, LocalTestPathKind::ExistingFile)?;
    let observed = super::document::read_native(&recorded.path, "前端构建收据")?;
    if super::document::binding(&observed) != builds["frontend"] {
        return Err("前端构建收据与本侧登记描述不一致".into());
    }
    Ok(())
}

fn validate_b0(value: &Value) -> Result<()> {
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
    exact(
        &adapter["patch"],
        &["path", "bytes", "sha256"],
        "B0 适配补丁",
    )?;
    if *value != b0::evidence()? {
        return Err("B0 执行适配身份无效".into());
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
