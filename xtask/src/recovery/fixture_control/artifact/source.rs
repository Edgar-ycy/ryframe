//! 复核构建、运行、源码组合和当前工作树，不接受陈旧来源证据。
use super::super::{exact_object, file_binding, git_root, read_json, text_field};
use crate::{
    Result,
    recovery::{clone::native::source, fixture_control::source_pair::execution::HeldTargetSource},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const BUSINESS_MODEL: &str = "tools/python/fixtures/order-business/src/resources/mod.rs";

pub(super) fn verify(root: &Path, directory: &Path) -> Result<()> {
    verify_observed(root, directory, || {})
}

pub(super) fn verify_observed(root: &Path, directory: &Path, observe: impl FnOnce()) -> Result<()> {
    verify_in(root, directory, None, observe)
}

pub(super) fn verify_held(root: &Path, directory: &Path, held: &HeldTargetSource) -> Result<()> {
    verify_in(root, directory, Some(held), || {})
}

fn verify_in(
    root: &Path,
    directory: &Path,
    held: Option<&HeldTargetSource>,
    observe: impl FnOnce(),
) -> Result<()> {
    let pair_path = directory.join("source-pair.json");
    let runtime_path = directory.join("runtime-evidence.json");
    if !pair_path.exists() {
        if held.is_some() || runtime_path.exists() {
            return Err("非全栈入口存在意外运行来源证据".into());
        }
        return Ok(());
    }
    let bindings = evidence_bindings(directory, &["runtime.json", "runtime-evidence.json"])?;
    verify_build_in(root, directory, held, |source, build_binding| {
        let actual = read_json(&runtime_path, "全栈运行来源证据", 4 * 1024 * 1024)?;
        if actual != runtime_evidence(directory, source, build_binding)? {
            return Err("全栈运行来源证据与当前来源不匹配".into());
        }
        observe();
        Ok(())
    })?;
    verify_bindings(bindings)
}

pub(super) fn verify_build_observed<T>(
    root: &Path,
    directory: &Path,
    observe: impl FnOnce(&Value, &Value) -> Result<T>,
) -> Result<T> {
    verify_build_in(root, directory, None, observe)
}

fn verify_build_in<T>(
    root: &Path,
    directory: &Path,
    held: Option<&HeldTargetSource>,
    observe: impl FnOnce(&Value, &Value) -> Result<T>,
) -> Result<T> {
    let mut bindings = evidence_bindings(directory, &["source-pair.json", "binaries.json"])?;
    let build_path = directory.join("build-evidence.json");
    let build_binding = evidence_binding(&build_path)?;
    let build = read_json(&build_path, "全栈构建证据", 4 * 1024 * 1024)?;
    bindings.push((build_path, build_binding.clone()));
    exact_object(
        &build,
        &[
            "format_version",
            "kind",
            "backend_root",
            "source",
            "artifacts",
            "build_jobs",
        ],
        "全栈构建证据",
    )?;
    if build["format_version"] != 1
        || build["kind"] != "full-stack-build"
        || build["backend_root"] != root.to_string_lossy().as_ref()
    {
        return Err("全栈构建证据格式或后端路径无效".into());
    }
    if build["build_jobs"].as_u64()
        != Some(u64::from(
            crate::recovery::clone::native::bridge::build_jobs(root)?,
        ))
    {
        return Err("全栈构建证据的 build_jobs 与当前 verification.build_jobs 不同；核对工具配置并保留原证据，使用新目录显式构建，不能改写或自动重放".into());
    }
    let source = &build["source"];
    let (path, pair) = source_pair(directory)?;
    let inspect = || {
        verify_artifacts(directory, &build)?;
        observe(source, &build_binding)
    };
    let observed = if held.is_some()
        || pair["kind"] == crate::recovery::fixture_control::source_pair::execution::PAIR_KIND
    {
        let verify_published = |current: &Value| {
            if current != source {
                return Err("已发布业务来源与完整当前来源不一致".into());
            }
            inspect()
        };
        match held {
            Some(held) => crate::recovery::full_stack::provenance::published_held(
                root,
                directory,
                &pair,
                held,
                verify_published,
            )?,
            None => crate::recovery::full_stack::provenance::published(
                root,
                directory,
                &pair,
                verify_published,
            )?,
        }
    } else {
        if source["fixture"] == "business" {
            let fixture = root
                .parent()
                .ok_or("业务后端缺少父目录")?
                .join("fixture.json");
            for path in [fixture, root.join(BUSINESS_MODEL)] {
                let binding = evidence_binding(&path)?;
                bindings.push((path, binding));
            }
        }
        verify_source(root, &path, &pair, source)?;
        let observed = inspect()?;
        let (path, pair) = source_pair(directory)?;
        verify_source(root, &path, &pair, source)?;
        observed
    };
    if build["build_jobs"].as_u64()
        != Some(u64::from(
            crate::recovery::clone::native::bridge::build_jobs(root)?,
        ))
    {
        return Err("全栈来源核验期间 verification.build_jobs 变化；保留现场，不自动重放".into());
    }
    verify_bindings(bindings)?;
    Ok(observed)
}

#[cfg(windows)]
pub(super) fn hold_published(root: &Path, directory: &Path) -> Result<Option<HeldTargetSource>> {
    use crate::recovery::fixture_control::source_pair::execution;

    if !directory.join("source-pair.json").try_exists()? {
        return Ok(None);
    }
    let (_, pair) = source_pair(directory)?;
    if pair["kind"] != execution::PAIR_KIND {
        return Ok(None);
    }
    let declared = json!({"path":root,"execution_source":pair["execution_source"]});
    let (_, binding) = execution::target_binding(root, &declared)?;
    Ok(Some(HeldTargetSource::open(root, &declared, &binding)?))
}

pub(super) fn runtime_evidence(
    directory: &Path,
    source: &Value,
    build_binding: &Value,
) -> Result<Value> {
    Ok(
        json!({"format_version":1,"kind":"full-stack-runtime","source":source,
        "build_evidence":build_binding,"runtime":evidence_binding(&directory.join("runtime.json"))?}),
    )
}

fn verify_bindings(bindings: Vec<(PathBuf, Value)>) -> Result<()> {
    for (path, original) in bindings {
        if evidence_binding(&path)? != original {
            return Err("全栈来源证据在产物核验期间发生变化".into());
        }
    }
    Ok(())
}

fn evidence_bindings(directory: &Path, names: &[&str]) -> Result<Vec<(PathBuf, Value)>> {
    names
        .iter()
        .map(|name| {
            let path = directory.join(name);
            let binding = evidence_binding(&path)?;
            Ok((path, binding))
        })
        .collect()
}

fn evidence_binding(path: &Path) -> Result<Value> {
    crate::local_test_path::validate_absolute_path(
        path,
        crate::local_test_path::LocalTestPathKind::ExistingFile,
    )?;
    file_binding(path, "全栈来源证据绑定")
}

fn verify_artifacts(directory: &Path, build: &Value) -> Result<()> {
    let roles = [
        "ryframe",
        "ryframe-worker",
        "ryframe-reset",
        "ryframe-migrate",
    ];
    let binaries = read_json(
        &directory.join("binaries.json"),
        "全栈二进制清单",
        64 * 1024,
    )?;
    exact_object(&binaries, &roles, "全栈二进制清单")?;
    let artifacts = exact_object(&build["artifacts"], &roles, "构建产物")?;
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
    Ok(())
}

fn source_pair(directory: &Path) -> Result<(PathBuf, Value)> {
    let original = directory.join("source-pair.json");
    crate::local_test_path::validate_absolute_path(
        &original,
        crate::local_test_path::LocalTestPathKind::ExistingFile,
    )?;
    // 与来源生产者保持同一规范路径文本；先核验原路径，避免解析时隐藏链接。
    let path =
        super::super::portable_path(&std::fs::canonicalize(directory)?).join("source-pair.json");
    let pair = read_json(&path, "全栈源码组合", 16 * 1024)?;
    Ok((path, pair))
}

fn verify_source(root: &Path, path: &Path, pair: &Value, evidence: &Value) -> Result<()> {
    let reference = pair["kind"] == "reference-fixture-source-pair";
    if reference {
        exact_object(
            pair,
            &["format_version", "kind", "fixture_receipt", "sources"],
            "参考源码组合",
        )?;
        if evidence["fixture"] != "business" {
            return Err("参考源码组合缺少业务 fixture".into());
        }
    } else {
        exact_object(
            pair,
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
    bound["receipt"] = evidence_binding(path)?;
    if evidence["format_version"] != 1 || evidence["source_pair"] != bound {
        return Err(format!(
            "构建来源与源码组合不匹配，不一致字段：{}；核对原 source-pair 与构建收据，保留现场，禁止覆盖或重建来源",
            source_mismatch_fields(evidence, &bound)
        )
        .into());
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
                pair,
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
        Some("business") => verify_business(root, evidence, pair)?,
        _ => return Err("未知全栈来源类型".into()),
    }
    verify_source_identity(pair)
}

fn verify_source_identity(pair: &Value) -> Result<()> {
    if pair["format_version"] != 1 {
        return Err("全栈源码组合版本无效".into());
    }
    let commit_pattern = regex::Regex::new(r"^[a-f0-9]{40}$")?;
    for side in ["backend", "frontend"] {
        let item = &pair["sources"][side];
        exact_object(item, &["head", "patch_sha256", "files"], "原始来源快照")?;
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

fn source_mismatch_fields(evidence: &Value, bound: &Value) -> String {
    let mut fields = match (evidence["source_pair"].as_object(), bound.as_object()) {
        (Some(actual), Some(expected)) => actual
            .keys()
            .chain(expected.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|key| actual.get(*key) != expected.get(*key))
            .map(|key| {
                if expected.contains_key(key) {
                    key.as_str()
                } else {
                    "未声明字段"
                }
            })
            .collect::<Vec<_>>(),
        _ => vec!["source_pair"],
    };
    if evidence["format_version"] != 1 {
        fields.push("format_version");
    }
    fields.join("、")
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
    if evidence["fixture_receipt"] != evidence_binding(&fixture_path)? {
        return Err("业务 fixture 来源或状态不匹配".into());
    }
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
    let model = root.join(BUSINESS_MODEL);
    let binding = evidence_binding(&model)?;
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
