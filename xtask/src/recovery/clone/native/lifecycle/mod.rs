use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{digest, json as json_io};
use crate::{
    Result,
    cli::CloneCommand,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

mod identity;
mod lock;
mod manifest;
mod source;
pub(crate) mod stage;
mod state;
mod status;

pub(crate) fn run(command: &CloneCommand, backend: &Path) -> Result<Value> {
    if is_seed_to_arm(command) {
        return Err("seed_to_arm init/status/stage 必须整体使用 Python 状态机".into());
    }
    match command {
        CloneCommand::Init { manifest, run_dir } => init(backend, manifest, run_dir),
        CloneCommand::Status { run_dir } => status::run(backend, run_dir),
        CloneCommand::Stage { run_dir, stage } => {
            stage::run(backend, run_dir, *stage, &stage::PythonRunner::new(backend))
        }
        _ => Err("clone 生命周期模块不处理该命令".into()),
    }
}

fn is_seed_to_arm(command: &CloneCommand) -> bool {
    let path = match command {
        CloneCommand::Init { manifest, .. } => manifest.clone(),
        CloneCommand::Status { run_dir } | CloneCommand::Stage { run_dir, .. } => {
            run_dir.join("manifest.json")
        }
        _ => return false,
    };
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|value| value.get("copy_stage").and_then(Value::as_str) == Some("seed_to_arm"))
}

fn init(backend: &Path, manifest_path: &Path, run_dir: &Path) -> Result<Value> {
    validate_local_test_path(manifest_path, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("复制运行清单无效：{error}"))?;
    validate_local_test_path(run_dir, backend, LocalTestPathKind::NewDirectory)
        .map_err(|error| format!("复制运行目录无效：{error}"))?;
    let manifest = manifest::load(backend, manifest_path)?;
    if let Some(storage) = manifest::target_storage_path(backend, &manifest.value)?
        && manifest::overlaps(run_dir, &storage)
    {
        return Err("seed_to_arm 运行目录必须与外部目标 storage run 拓扑分离".into());
    }
    if digest::file(manifest_path)? != manifest.digest {
        return Err("验收清单读取期间变化或父目录不存在".into());
    }
    fs::create_dir(run_dir)
        .map_err(|error| format!("无法创建复制运行目录 {}：{error}", run_dir.display()))?;
    json_io::write_new(&run_dir.join("manifest.json"), &manifest.value)?;
    state::initialize(run_dir)?;
    Ok(json!({
        "status": "run_registered",
        "id": manifest.value["id"],
        "directory": manifest::path_text(run_dir, "复制运行目录")?,
        "remote_writes": 0,
        "stages_passed": [],
        "restore_qualified": false,
    }))
}
