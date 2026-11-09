//! Worker 使用工具编译根的独立来源；产品运行根始终由原 runtime 决定。
use super::{model::*, plan::SourceGuard};
use crate::{
    Result,
    check::consumer_context::existing_origin,
    recovery::{clone::native::source, fixture_control::portable_path},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const REQUIRED: &str = "浏览器 Worker 工具来源未确认：请在当前程序的真实编译根使用同一工具执行 __frontend-origin prepare --write，再核对固定 .local-tests/frontend-origin-tool.json；此操作只读，不自动发布或修复绑定";

fn ordinary_directory(root: &Path) -> Result<PathBuf> {
    directory(root)?;
    Ok(portable_path(&root.canonicalize()?))
}

pub(super) fn capture() -> Result<Value> {
    let compiled =
        crate::frontend_origin::compiled_root().map_err(|_| Rejected(REQUIRED.to_owned()))?;
    let root = ordinary_directory(&compiled)?;
    let origin = existing_origin(&root).map_err(|_| Rejected(REQUIRED.to_owned()))?;
    let binding_file = root.join(".local-tests/frontend-origin-tool.json");
    let binding = bound(&binding_file)?;
    let executable = std::env::current_exe()?;
    safe(&executable)?;
    let executable = bound(&portable_path(&executable.canonicalize()?))?;
    let inventory = source::capture(&root)?;
    let mut source = source::binding_value(&inventory.source)["snapshot"].clone();
    source
        .as_object_mut()
        .ok_or("Worker 工具源码快照无效")?
        .remove("clean");
    origin.verify().map_err(|_| Rejected(REQUIRED.to_owned()))?;
    descriptor(&binding, &binding_file)?;
    Ok(json!({"root":root,"binding":binding,"source":source,"executable":executable}))
}

/// Dependencies 可注入事实；真实依赖先通过 existing_origin，再守卫实际文件与完整源码。
pub(super) fn source_guard(value: &Value) -> Result<SourceGuard> {
    crate::recovery::clone::native::json::exact(
        value,
        &["root", "binding", "source", "executable"],
        "Worker 工具来源",
    )?;
    let root = ordinary_directory(&path(&value["root"])?)?;
    let binding = path(&value["binding"]["path"])?;
    descriptor(&value["binding"], &binding)?;
    check(
        binding.canonicalize()?
            == root
                .join(".local-tests/frontend-origin-tool.json")
                .canonicalize()?,
        "Worker 工具必须使用其根内固定 origin 绑定文件",
    )?;
    let executable = path(&value["executable"]["path"])?;
    descriptor(&value["executable"], &executable)?;
    SourceGuard::capture(&root, &value["source"])
}

fn same_file_binding(expected: &Value, actual: &Value) -> Result<bool> {
    let selected = path(&expected["path"])?;
    let recorded = path(&actual["path"])?;
    descriptor(expected, &selected)?;
    descriptor(actual, &recorded)?;
    Ok(selected.canonicalize()? == recorded.canonicalize()?
        && expected["bytes"] == actual["bytes"]
        && expected["sha256"] == actual["sha256"])
}

/// 有限前端监督器独立验证当前程序/编译根，不信任调用者只提交的来源摘要。
pub(super) fn verify(value: &Value) -> Result<SourceGuard> {
    let current = capture()?;
    check(
        ordinary_directory(&path(&value["root"])?)? == path(&current["root"])?,
        "Worker 工具根与当前程序的编译根不一致",
    )?;
    check(
        same_file_binding(&value["binding"], &current["binding"])?
            && same_file_binding(&value["executable"], &current["executable"])?
            && value["source"] == current["source"],
        "Worker 工具程序、固定绑定或完整来源与当前程序不一致",
    )?;
    let source = source_guard(value)?;
    source.unchanged()?;
    Ok(source)
}
