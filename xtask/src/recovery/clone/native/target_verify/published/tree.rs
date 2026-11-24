//! initialized → prepared → registration 双树谱系与完整 prepare 观察。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::super::{super::json as json_io, evidence_model::*, resume_baseline};
use crate::Result;

pub(in super::super) fn registered(
    backend: &Path,
    selected: &Path,
) -> Result<(PathBuf, super::guard::Inputs)> {
    let target = selected.parent().ok_or("初始化历史缺少目标目录")?;
    let workspace = target.parent().ok_or("初始化目标缺少登记目录")?;
    let registration_path = workspace.join("registration.json");
    let inputs = super::guard::Inputs::open([selected.to_path_buf(), registration_path.clone()])?;
    let registration = read(backend, &registration_path)?;
    let recorded = Path::new(json_io::text(
        &registration["target_directory"],
        "登记目标目录",
    )?)
    .join("initialized.json");
    check(
        crate::local_test_path::same_local_test_file(backend, selected, &recorded)?,
        "所选初始化历史不是登记目标的原文件；保留原收据并选择 registration 中的目标目录",
    )?;
    inputs.check()?;
    Ok((recorded, inputs))
}

#[derive(PartialEq, Eq)]
pub(super) struct InitializedSnapshot {
    binding: Value,
    pub(super) files: Value,
}

pub(in super::super) fn initialized(
    backend: &Path,
    target: &Path,
    owner: Option<&super::guard::Generation>,
) -> Result<Value> {
    Ok(initialized_snapshot(
        backend,
        target,
        owner,
        resume_baseline::HistorySource::Original(None),
    )?
    .binding)
}

pub(super) fn initialized_snapshot(
    backend: &Path,
    target: &Path,
    owner: Option<&super::guard::Generation>,
    source: resume_baseline::HistorySource<'_>,
) -> Result<InitializedSnapshot> {
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
    resume_baseline::prepared_with_history(
        backend,
        target,
        &json!({
            "descriptor":prepared,"registration":prepared_value["registration"],
            "predecessor":prepared_value["predecessor"]
        }),
        &registration_value["request"],
        source,
    )?;
    super::super::resume_observations::file_map(&value["files"])?;
    let files = super::super::tree::protected_snapshot(backend, target, owner)?;
    check(
        value["files"] == files
            && binding(backend, &initialized_path)? == initialized
            && binding(backend, &prepared_path)? == prepared
            && binding(backend, &registration_path)? == registration,
        "初始化目标文件树或双树登记绑定变化",
    )?;
    Ok(InitializedSnapshot {
        binding: initialized,
        files,
    })
}
