//! strict v1 比较来源的唯一实现；正式命令与只读适配器共用此入口。
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, resources::common, source},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

mod adapter;
mod arms;
mod generation;
mod published;
mod shape;

/// 复制后消费者复用已发布源/代次/导出的同一权威，不重建另一套来源模型。
pub(crate) fn copy_source(backend: &Path, manifest: &Value) -> Result<Value> {
    use crate::recovery::clone::native::{cache, lifecycle::state};
    if manifest.get("review_successor").is_some() {
        let identity = published::identity(backend, &manifest["source_export_result"])?;
        for field in [
            "source_registration",
            "source_rebind",
            "source_generation",
            "review_successor",
            "source_request",
        ] {
            if identity[field] != manifest[field] {
                return Err("复制清单与已发布来源代次或导出绑定不同".into());
            }
        }
        if identity["export"] != manifest["source_export"] {
            return Err("复制清单采用了其他已发布导出".into());
        }
        let initial = cache::binding::source::load(backend, &manifest["review_successor"])?;
        let run = path(&initial["directory"])?;
        let saved = state::load(&run, true)?;
        let source = super::running::history::predecessor(backend, &run, &saved)?;
        let resolved = generation::resolve(backend, &source, &saved)?;
        if state::load(&run, true)? != saved {
            return Err("复制来源完整历史在核验期间变化".into());
        }
        return Ok(resolved);
    }
    let mut source = cache::binding::source::published(backend, &manifest["source_registration"])?;
    let run = path(&source["directory"])?;
    super::cache_history(
        backend,
        &run,
        "seed-history",
        &json!({
        "source":manifest["source_registration"],"current":null,"mode":"run"}),
    )?;
    if !manifest["source_rebind"].is_null() {
        let saved = state::load(&run, true)?;
        source = super::running::history::predecessor(backend, &run, &saved)?;
        if source["source_rebind"] != manifest["source_rebind"] || state::load(&run, true)? != saved
        {
            return Err("复制清单的原来源重绑定收据不同于已发布完整历史".into());
        }
    }
    source["source_request"] = source["registration"]["source_request"].clone();
    source["source_environment"] = source["registration"]["source_environment"].clone();
    Ok(source)
}
#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/source/comparison.rs"]
mod tests;

pub(crate) fn run_capture(
    backend: &Path,
    options: &crate::cli::SourceComparisonCaptureOptions,
) -> Result<Value> {
    validate_output(backend, &options.output)?;
    let export = common::binding(&options.source_export_result)?;
    let value = capture(&json!({"backend":backend,"b0_backend":options.b0_backend,
        "b0_adapter_backend":options.b0_adapter_backend,"b0_frontend":options.b0_frontend,
        "b0_backend_build":options.b0_backend_build,"b0_frontend_build":options.b0_frontend_build,
        "b1_backend":options.b1_backend,"b1_frontend":options.b1_frontend,
        "b1_backend_build":options.b1_backend_build,"b1_frontend_build":options.b1_frontend_build,
        "source_export_result":export,"reconstruct_adapter":true}))?;
    if common::binding(&options.source_export_result)? != export {
        return Err("导出结果在采集期间变化".into());
    }
    validate_output(backend, &options.output)?;
    document::write_new(&options.output, &value)?;
    Ok(
        json!({"output":options.output,"status":"comparison_sources_captured","restore_success":false}),
    )
}

fn validate_output(backend: &Path, output: &Path) -> Result<()> {
    crate::local_test_path::validate_local_test_path(
        output,
        backend,
        crate::local_test_path::LocalTestPathKind::OutputFile,
    )?;
    let parent = output.parent().ok_or("比较来源输出缺少父目录")?;
    crate::local_test_path::validate_absolute_path(
        parent,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    let relative = output
        .strip_prefix(backend)?
        .to_str()
        .ok_or("比较来源输出必须为 UTF-8")?;
    adapter::git(backend, &["check-ignore", relative], None)?;
    Ok(())
}

pub(crate) fn run_verify(backend: &Path, receipt: &Path) -> Result<Value> {
    let binding = common::binding(receipt)?;
    let value = common::read_bound(backend, &binding, "比较来源清单")?;
    let value = verify(backend, &value, true)?;
    common::bound_file(backend, &binding, "比较来源清单")?;
    Ok(
        json!({"receipt":receipt,"status":"comparison_sources_verified",
        "source_export_identity_sha256":value["source_export"]["identity_sha256"],"restore_success":false}),
    )
}

const INPUTS: &[&str] = &[
    "backend",
    "b0_backend",
    "b0_adapter_backend",
    "b0_frontend",
    "b0_backend_build",
    "b0_frontend_build",
    "b1_backend",
    "b1_frontend",
    "b1_backend_build",
    "b1_frontend_build",
    "source_export_result",
    "reconstruct_adapter",
];

pub(crate) fn writes(operation: &str, args: &Value) -> Result<bool> {
    match operation {
        "comparison-verify" => Ok(!boolean(&args["read_only"])?),
        "comparison-build-source" => boolean(&args["reconstruct"]),
        "comparison-source-export" => Ok(false),
        _ => Err("未知比较来源操作".into()),
    }
}

pub(crate) fn request(operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "comparison-verify" => {
            document::exact(args, &["backend", "value", "read_only"], "比较来源验证参数")?;
            verify(
                &path(&args["backend"])?,
                &args["value"],
                boolean(&args["read_only"])?,
            )
        }
        "comparison-build-source" => adapter::build_source(args),
        "comparison-source-export" => {
            document::exact(args, &["backend", "descriptor"], "比较导出身份参数")?;
            published::identity(&repository(&path(&args["backend"])?)?, &args["descriptor"])
        }
        _ => Err("未知比较来源操作".into()),
    }
}

pub(super) fn repository(root: &Path) -> Result<PathBuf> {
    if digest::linked_path(root)?.into_iter().any(|v| v)
        || crate::recovery::fixture_control::portable_path(&root.canonicalize()?) != root
    {
        return Err("比较来源必须是无链接的规范绝对 Git 根目录".into());
    }
    let actual = crate::recovery::fixture_control::git_root(root, "比较来源 Git 根")?;
    if actual != root {
        return Err("比较来源必须是实际 Git 工作树根目录".into());
    }
    Ok(actual)
}

pub(super) fn path(value: &Value) -> Result<PathBuf> {
    let path = PathBuf::from(document::text(value, "比较来源绝对路径")?);
    if !path.is_absolute() {
        return Err("比较来源必须显式传绝对路径".into());
    }
    Ok(path)
}

pub(super) fn boolean(value: &Value) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| "比较来源写入选择必须是 boolean".into())
}

pub(crate) fn capture(args: &Value) -> Result<Value> {
    document::exact(args, INPUTS, "比较来源采集参数")?;
    let backend = repository(&path(&args["backend"])?)?;
    let reconstruct = boolean(&args["reconstruct_adapter"])?;
    let export = published::identity(&backend, &args["source_export_result"])?;
    let b0 = arms::capture(&backend, args, &export, true, reconstruct)?;
    let b1 = arms::capture(&backend, args, &export, false, reconstruct)?;
    let roots = [&b0, &b1]
        .into_iter()
        .flat_map(|arm| {
            ["source_backend", "execution_backend", "frontend"]
                .map(|key| arm["roots"][key].as_str().unwrap_or(""))
        })
        .collect::<BTreeSet<_>>();
    if roots.len() != 5
        || b1["sources"]["backend"]["source"]["snapshot"]["head"] == adapter::BACKEND
        || b1["sources"]["frontend"]["source"]["snapshot"]["head"] == adapter::FRONTEND
    {
        return Err("比较来源必须绑定五个独立根，B1 双端提交均须不同于 B0".into());
    }
    if published::identity(&backend, &args["source_export_result"])? != export
        || arms::capture(&backend, args, &export, true, reconstruct)? != b0
        || arms::capture(&backend, args, &export, false, reconstruct)? != b1
    {
        return Err("比较来源采集期间源码、构建或共享导出变化".into());
    }
    Ok(
        json!({"format_version":1,"kind":"restore-comparison-sources","source_export":export,"arms":{"b0":b0,"b1":b1}}),
    )
}

pub(crate) fn verify(backend: &Path, value: &Value, read_only: bool) -> Result<Value> {
    shape::validate(value)?;
    let b0 = &value["arms"]["b0"];
    let b1 = &value["arms"]["b1"];
    let args = json!({"backend":backend,"b0_backend":b0["roots"]["source_backend"],
        "b0_adapter_backend":b0["roots"]["execution_backend"],"b0_frontend":b0["roots"]["frontend"],
        "b0_backend_build":b0["builds"]["backend"]["path"],"b0_frontend_build":b0["builds"]["frontend"]["path"],
        "b1_backend":b1["roots"]["source_backend"],"b1_frontend":b1["roots"]["frontend"],
        "b1_backend_build":b1["builds"]["backend"]["path"],"b1_frontend_build":b1["builds"]["frontend"]["path"],
        "source_export_result":value["source_export"]["result"],"reconstruct_adapter":!read_only});
    if capture(&args)? != *value {
        return Err("双版本源码、构建或共享导出与来源清单不一致".into());
    }
    Ok(value.clone())
}
