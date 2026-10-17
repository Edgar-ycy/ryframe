use std::path::Path;

use serde_json::{Value, json};

use super::{
    super::{digest, json as json_io},
    identity, lock, manifest, source, state,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) fn run(backend: &Path, run_dir: &Path) -> Result<Value> {
    validate_local_test_path(run_dir, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| format!("复制运行目录无效：{error}"))?;
    let manifest = manifest::load(backend, &run_dir.join("manifest.json"))?;
    let state_value = state::load(run_dir, true)?;
    let current = source::current(backend)?;
    let controller = observe_controller(run_dir, &state_value)?;
    let attempts = state::attempts(&state_value)?
        .iter()
        .map(|attempt| {
            json!({
                "number": attempt["number"],
                "stage": attempt["stage"],
                "mode": attempt["mode"],
                "status": attempt["status"],
                "error_type": attempt["error_type"],
                "result": attempt["result"],
                "execution_source_matches": attempt["sources"] == current,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "status": "run_status",
        "id": manifest.value["id"],
        "controller": controller,
        "attempts": attempts,
        "remote_writes": 0,
        "restore_qualified": false,
    }))
}

fn observe_controller(run_dir: &Path, state_value: &Value) -> Result<Value> {
    let state_before = json_io::read(&run_dir.join("state.json"))?;
    let owner = lock::owner_observation(run_dir)?;
    let observed = if let Some((descriptor, value)) = owner {
        let lock_before = lock::owner_observation(run_dir)?;
        validate_owner(run_dir, &value, &state_before)?;
        let process = identity::process(
            value["identity"]["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or("控制器 PID 超出范围")?,
        )?;
        let (process_matches, process_missing) =
            identity::same(&value["identity"], process.as_ref())?;
        if json_io::read(&run_dir.join("state.json"))? != state_before
            || lock::owner_observation(run_dir)? != lock_before
        {
            return Err("控制状态在只读观察期间变化".into());
        }
        json!({
            "owner": descriptor,
            "process_matches": process_matches,
            "process_missing": process_missing,
        })
    } else if let Some(attempt) = state::running_attempt(state_value)? {
        let number = attempt
            .get("number")
            .and_then(Value::as_u64)
            .ok_or("运行中阶段序号无效")?;
        let controller = state::controller_path(run_dir, number);
        if !controller.exists() {
            return Err("未收尾阶段缺少控制器收据，须核对原始现场".into());
        }
        let (descriptor, value) = state::controller_record(run_dir, number, attempt)?;
        validate_owner(run_dir, &value, &state_before)?;
        let process = identity::process(
            value["identity"]["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or("控制器 PID 超出范围")?,
        )?;
        let (process_matches, process_missing) =
            identity::same(&value["identity"], process.as_ref())?;
        if json_io::read(&run_dir.join("state.json"))? != state_before
            || !descriptor_matches(&descriptor)?
            || lock::owner_observation(run_dir)?.is_some()
        {
            return Err("控制状态在只读观察期间变化".into());
        }
        json!({
            "owner": descriptor,
            "process_matches": process_matches,
            "process_missing": process_missing,
            "phase": "publication_pending",
        })
    } else {
        if json_io::read(&run_dir.join("state.json"))? != state_before
            || lock::owner_observation(run_dir)?.is_some()
        {
            return Err("控制状态在只读观察期间变化".into());
        }
        return Ok(Value::Null);
    };
    Ok(observed)
}

fn validate_owner(run_dir: &Path, owner: &Value, state_value: &Value) -> Result<()> {
    identity::validate(owner, run_dir)?;
    if owner.get("manifest_sha256").and_then(Value::as_str)
        != state_value["manifest"]
            .get("sha256")
            .and_then(Value::as_str)
    {
        return Err("控制器收据不属于固定运行目录或缺少完整进程身份".into());
    }
    Ok(())
}

fn descriptor_matches(descriptor: &Value) -> Result<bool> {
    let path = Path::new(
        descriptor
            .get("path")
            .and_then(Value::as_str)
            .ok_or("控制器描述缺少路径")?,
    );
    let expected = descriptor
        .get("bytes")
        .and_then(Value::as_u64)
        .zip(descriptor.get("sha256").and_then(Value::as_str));
    let actual = digest::file(path)?;
    Ok(expected == Some((actual.bytes, actual.sha256.as_str())))
}
