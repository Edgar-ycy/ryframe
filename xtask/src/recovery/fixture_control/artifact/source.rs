//! 复核构建、运行、源码组合和当前工作树，不接受陈旧来源证据。
use super::super::{exact_object, file_binding, git_root, read_json, text_field};
use crate::{Result, recovery::clone::native::source};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn verify(root: &Path, directory: &Path) -> Result<()> {
    let pair_path = directory.join("source-pair.json");
    let runtime_path = directory.join("runtime-evidence.json");
    if !pair_path.exists() {
        if runtime_path.exists() {
            return Err("非全栈入口存在意外运行来源证据".into());
        }
        return Ok(());
    }
    let build_path = directory.join("build-evidence.json");
    let build = read_json(&build_path, "全栈构建证据", 4 * 1024 * 1024)?;
    exact_object(
        &build,
        &[
            "format_version",
            "kind",
            "backend_root",
            "source",
            "artifacts",
        ],
        "全栈构建证据",
    )?;
    if build["format_version"] != 1
        || build["kind"] != "full-stack-build"
        || build["backend_root"] != root.to_string_lossy().as_ref()
    {
        return Err("全栈构建证据格式或后端路径无效".into());
    }
    let source = &build["source"];
    verify_source(root, directory, source)?;
    let binaries = read_json(
        &directory.join("binaries.json"),
        "全栈二进制清单",
        64 * 1024,
    )?;
    let artifacts = exact_object(
        &build["artifacts"],
        &[
            "ryframe",
            "ryframe-worker",
            "ryframe-reset",
            "ryframe-migrate",
        ],
        "构建产物",
    )?;
    for (name, binding) in artifacts {
        let path = PathBuf::from(text_field(binding, "path", "构建产物")?);
        crate::local_test_path::validate_absolute_path(
            &path,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
        if binaries[name] != binding["path"] || *binding != file_binding(&path, "构建产物")? {
            return Err("全栈构建证据与二进制清单或产物不匹配".into());
        }
    }
    let actual = read_json(&runtime_path, "全栈运行来源证据", 4 * 1024 * 1024)?;
    let expected = json!({"format_version":1,"kind":"full-stack-runtime","source":source,"build_evidence":file_binding(&build_path,"构建证据")?,"runtime":file_binding(&directory.join("runtime.json"),"运行收据")?});
    if actual != expected {
        return Err("全栈运行来源证据与当前来源不匹配".into());
    }
    Ok(())
}

fn verify_source(root: &Path, directory: &Path, evidence: &Value) -> Result<()> {
    let path = directory.join("source-pair.json");
    let pair = read_json(&path, "全栈源码组合", 16 * 1024)?;
    let reference = pair["kind"] == "reference-fixture-source-pair";
    if reference {
        exact_object(
            &pair,
            &["format_version", "kind", "fixture_receipt", "sources"],
            "参考源码组合",
        )?;
        if evidence["fixture"] != "business" {
            return Err("参考源码组合缺少业务 fixture".into());
        }
    } else {
        exact_object(
            &pair,
            &[
                "format_version",
                "backend_sha",
                "frontend_sha",
                "run_id",
                "attempt",
                "sources",
            ],
            "全栈源码组合",
        )?;
    }
    exact_object(&pair["sources"], &["backend", "frontend"], "源码组合来源")?;
    if root
        .parent()
        .is_some_and(|parent| parent.join("fixture.json").is_file())
        && evidence["fixture"] != "business"
    {
        return Err("业务 fixture 不得使用 core 来源证据".into());
    }
    let mut bound = pair.clone();
    bound["receipt"] = file_binding(&path, "全栈源码组合")?;
    if evidence["format_version"] != 1 || evidence["source_pair"] != bound {
        return Err("构建来源与源码组合不匹配".into());
    }
    match evidence["fixture"].as_str() {
        Some("core") => {
            exact_object(
                evidence,
                &[
                    "format_version",
                    "fixture",
                    "backend_root",
                    "source_pair",
                    "original",
                ],
                "core 构建来源",
            )?;
            exact_object(
                &pair,
                &[
                    "format_version",
                    "backend_sha",
                    "frontend_sha",
                    "run_id",
                    "attempt",
                    "sources",
                ],
                "全栈源码组合",
            )?;
            if evidence["backend_root"] != root.to_string_lossy().as_ref() {
                return Err("core 来源根目录不匹配".into());
            }
            let current = snapshot(root)?;
            if current != pair["sources"]["backend"]
                || evidence["original"] != json!({"backend":current})
            {
                return Err("core 来源与当前源码不匹配".into());
            }
        }
        Some("business") => verify_business(root, evidence, &pair)?,
        _ => return Err("未知全栈来源类型".into()),
    }
    verify_source_identity(&pair)
}

fn verify_source_identity(pair: &Value) -> Result<()> {
    if pair["format_version"] != 1 {
        return Err("全栈源码组合版本无效".into());
    }
    let commit_pattern = regex::Regex::new(r"^[a-f0-9]{40}$")?;
    for side in ["backend", "frontend"] {
        let item = &pair["sources"][side];
        let head = text_field(item, "head", "原始来源")?;
        if !commit_pattern.is_match(head)
            || item["patch_sha256"] != crate::devex::runtime_native::artifacts::sha(&[])
            || item["files"] != json!([])
        {
            return Err("源码组合必须登记干净的原始来源".into());
        }
        if pair.get("backend_sha").is_some()
            && (pair[format!("{side}_sha")] != head
                || pair["run_id"].as_u64().is_none_or(|v| v == 0)
                || pair["attempt"].as_u64().is_none_or(|v| v == 0))
        {
            return Err("源码组合提交或运行身份无效".into());
        }
    }
    Ok(())
}

fn verify_business(root: &Path, evidence: &Value, pair: &Value) -> Result<()> {
    exact_object(
        evidence,
        &[
            "format_version",
            "fixture",
            "fixture_receipt",
            "fixture_definition",
            "roots",
            "source_pair",
            "original",
            "generated",
        ],
        "业务构建来源",
    )?;
    let fixture_path = root
        .parent()
        .ok_or("业务后端缺少父目录")?
        .join("fixture.json");
    let fixture = read_json(&fixture_path, "业务 fixture", 4 * 1024 * 1024)?;
    exact_object(
        &fixture,
        &[
            "format_version",
            "fixture",
            "status",
            "fixture_sha256",
            "sources",
            "paths",
            "generated",
        ],
        "业务 fixture",
    )?;
    for field in ["sources", "paths", "generated"] {
        exact_object(
            &fixture[field],
            &["backend", "frontend"],
            "业务 fixture 双仓字段",
        )?;
    }
    if fixture["format_version"] != 1
        || fixture["fixture"] != "business"
        || fixture["status"] != "ready"
        || evidence["fixture_receipt"] != file_binding(&fixture_path, "业务 fixture")?
        || evidence["original"] != fixture["sources"]
        || pair["sources"] != fixture["sources"]
    {
        return Err("业务 fixture 来源或状态不匹配".into());
    }
    if pair["kind"] == "reference-fixture-source-pair"
        && pair["fixture_receipt"] != evidence["fixture_receipt"]
    {
        return Err("参考来源组合未绑定当前 fixture".into());
    }
    for side in ["backend", "frontend"] {
        let path = PathBuf::from(text_field(&fixture["paths"], side, "fixture 工作树")?);
        if path.parent() != root.parent()
            || evidence["roots"][side] != path.to_string_lossy().as_ref()
            || (side == "backend" && path != root)
        {
            return Err("业务 fixture 工作树路径不匹配".into());
        }
        let actual = snapshot(&path)?;
        if actual != fixture["generated"][side]
            || actual != evidence["generated"][side]
            || actual["head"] != fixture["sources"][side]["head"]
        {
            return Err("业务 fixture 工作树发生漂移".into());
        }
    }
    let model = root.join("tools/python/fixtures/order-business/src/resources/mod.rs");
    let binding = file_binding(&model, "业务模型")?;
    if evidence["fixture_definition"] != json!({"model":binding})
        || binding["sha256"] != fixture["fixture_sha256"]
    {
        return Err("业务模型与登记摘要不匹配".into());
    }
    Ok(())
}

fn snapshot(root: &Path) -> Result<Value> {
    if git_root(root, "来源工作树")? != root {
        return Err("来源必须为 Git 根目录".into());
    }
    let value = source::capture(root)?.source.snapshot;
    Ok(
        json!({"head":value.head,"patch_sha256":value.patch_sha256,"files":value.files.into_iter().map(|file|json!({"path":file.path,"sha256":file.sha256})).collect::<Vec<_>>()}),
    )
}
