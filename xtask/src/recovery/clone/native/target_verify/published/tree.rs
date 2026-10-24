//! initialized → prepared → registration 双树谱系与完整 prepare 观察。
use std::path::Path;

use serde_json::{Value, json};

use super::super::{super::json as json_io, evidence_model::*, resume_baseline};
use crate::Result;

pub(super) fn initialized(backend: &Path, target: &Path) -> Result<Value> {
    let workspace = target.parent().ok_or("初始化目标缺少登记目录")?;
    let initialized_path = workspace.join("initialized-files.json");
    let prepared_path = workspace.join("prepared-files.json");
    let registration_path = workspace.join("registration.json");
    let initialized = binding(backend, &initialized_path)?;
    let prepared = binding(backend, &prepared_path)?;
    let registration = binding(backend, &registration_path)?;
    let value = read(backend, &bound(backend, &initialized)?)?;
    json_io::exact(
        &value,
        &[
            "format_version",
            "kind",
            "registration",
            "predecessor",
            "files",
        ],
        "初始化文件快照",
    )?;
    check(
        value["format_version"] == 1
            && value["kind"] == "devex-clone-fresh-target-initialized-files"
            && value["registration"] == registration
            && value["predecessor"] == prepared,
        "初始化文件快照或直接前驱无效",
    )?;
    let prepared_value = read(backend, &bound(backend, &prepared)?)?;
    let registration_value = read(backend, &bound(backend, &registration)?)?;
    resume_baseline::prepared(
        backend,
        target,
        &json!({
            "descriptor":prepared,"registration":prepared_value["registration"],
            "predecessor":prepared_value["predecessor"]
        }),
        &registration_value["request"],
    )?;
    super::super::resume_observations::file_map(&value["files"])?;
    check(
        value["files"] == super::super::tree::snapshot(backend, target, false)?
            && binding(backend, &initialized_path)? == initialized
            && binding(backend, &prepared_path)? == prepared
            && binding(backend, &registration_path)? == registration,
        "初始化目标文件树或双树登记绑定变化",
    )?;
    Ok(initialized)
}
