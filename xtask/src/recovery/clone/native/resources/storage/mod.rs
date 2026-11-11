pub(crate) mod model;
pub(crate) mod process;
pub(crate) mod registered;

use std::path::Path;

use serde_json::{Value, json};

use super::{
    super::{
        digest,
        lifecycle::{lock::ProcessGuard, manifest::fixture_storage, state},
    },
    common, producers,
};
use crate::{
    Result,
    cli::{CloneSide, StorageOperation, StorageOptions},
};

pub(crate) fn preflight(backend: &Path, run_dir: &Path, options: &StorageOptions) -> Result<()> {
    common::local_directory(backend, run_dir, "复制运行目录")?;
    let manifest = manifest(run_dir)?;
    let _ = state::load(run_dir, true)?;
    match options.operation {
        StorageOperation::Restart => {
            if run_dir.join(registered::stage(options.side)).exists() {
                let (request, descriptor) =
                    registered::registration(backend, run_dir, options.side)?;
                if let Some(filename) = &options.request {
                    let supplied = serde_json::from_slice::<Value>(&std::fs::read(
                        common::common_file(backend, filename, "存储重启请求")?,
                    )?)?;
                    if supplied != request {
                        return Err("不能替换同一存储运行的固定请求".into());
                    }
                }
                model::validate(backend, run_dir, &manifest, options.side, &request)?;
                transferred(backend, run_dir, &manifest, options.side, &request)?;
                registered::require_previous_stopped(backend, run_dir, &request, &descriptor, None)
            } else {
                let filename = options
                    .request
                    .as_deref()
                    .ok_or("首次存储重启必须显式提供请求")?;
                let request = common::read_bound(
                    backend,
                    &json!({
                        "path": common::path_text(filename, "存储重启请求")?,
                        "bytes": digest::file(&common::common_file(
                            backend,
                            filename,
                            "存储重启请求",
                        )?)?.bytes,
                        "sha256": digest::file(&common::common_file(
                            backend,
                            filename,
                            "存储重启请求",
                        )?)?.sha256,
                    }),
                    "存储重启请求",
                )?;
                model::validate(backend, run_dir, &manifest, options.side, &request)?;
                transferred(backend, run_dir, &manifest, options.side, &request)?;
                for key in ["api_url", "console_url"] {
                    common::require_closed_port(
                        request
                            .get(key)
                            .and_then(Value::as_str)
                            .ok_or("存储请求缺少监听地址")?,
                    )?;
                }
                Ok(())
            }
        }
        StorageOperation::Stop | StorageOperation::Recover => {
            if options.request.is_some() {
                return Err("清理和观察不能重新登记或更换存储请求".into());
            }
            let _ = registered::registration(backend, run_dir, options.side)?;
            Ok(())
        }
        StorageOperation::Status => Err("storage status 不应进入阶段执行".into()),
    }
}

pub(crate) fn execute(
    backend: &Path,
    run_dir: &Path,
    number: u64,
    options: &StorageOptions,
) -> Result<Value> {
    let mode = options.operation.as_str();
    let controller = registered::active(backend, run_dir, mode, number, options.side)?;
    let _guard = ProcessGuard::acquire_named(
        run_dir,
        &format!("{}.guard", registered::stage(options.side)),
    )?;
    let manifest = manifest(run_dir)?;
    let (request, descriptor, private) = if options.operation == StorageOperation::Restart {
        registered::register(
            backend,
            run_dir,
            &manifest,
            options.side,
            options.request.as_deref(),
        )?
    } else {
        if options.request.is_some() {
            return Err("清理和观察不能重新登记或更换存储请求".into());
        }
        let (request, descriptor) = registered::registration(backend, run_dir, options.side)?;
        (
            request,
            descriptor,
            model::Validated {
                private: Default::default(),
            },
        )
    };
    let output = run_dir
        .join(registered::stage(options.side))
        .join(format!("a{number:04}"));
    common::ensure_new_directory(backend, &output, "存储阶段目录")?;
    if options.operation != StorageOperation::Restart {
        return registered::observe_or_stop(
            backend,
            run_dir,
            &request,
            &descriptor,
            &output,
            mode,
            number,
        );
    }
    let private_environment = private.private;
    let handoff = transferred(backend, run_dir, &manifest, options.side, &request)?;
    let mut guard = || {
        if registered::active(backend, run_dir, mode, number, options.side)? != controller
            || registered::registration(backend, run_dir, options.side)?
                != (request.clone(), descriptor.clone())
        {
            return Err("存储控制器或固定登记变化".into());
        }
        let checked = model::validate(backend, run_dir, &manifest, options.side, &request)?;
        if checked.private != private_environment {
            return Err("存储启动期间私有环境变化".into());
        }
        producers::stopped(backend, run_dir, &manifest)?;
        common::directory_identity(backend, &request["data_directory"])?;
        Ok(())
    };
    guard()?;
    registered::require_previous_stopped(backend, run_dir, &request, &descriptor, Some(number))?;
    let storage = process::start(
        &request,
        &private_environment,
        &output,
        &descriptor,
        &controller,
        number,
        &mut guard,
    )?;
    let mut result = json!({
        "status": "storage_restarted",
        "side": options.side.as_str(),
        "request": descriptor,
        "attempt": number,
        "storage": storage,
        "ready": common::binding(&output.join("ready.json"))?,
        "data_directory": request["data_directory"],
        "api_url": request["api_url"],
        "console_url": request["console_url"],
        "object_api_calls": 0,
        "database_calls": 0,
        "storage_startup_may_write_files": true,
        "reconciliation_required": true,
        "resources_deleted": false,
        "restore_qualified": false,
    });
    if let Some(handoff) = handoff {
        common::bound_file(backend, &handoff, "存储重启的 RustFS 移交结果")?;
        result["fixture_handoff"] = handoff;
    }
    Ok(result)
}

fn transferred(
    backend: &Path,
    run: &Path,
    manifest: &Value,
    side: CloneSide,
    request: &Value,
) -> Result<Option<Value>> {
    if side == CloneSide::Target && manifest["copy_stage"] == "source_to_seed" {
        fixture_storage::rustfs_handoff(backend, run, request)
    } else {
        Ok(None)
    }
}

pub(crate) fn status(backend: &Path, run_dir: &Path, side: CloneSide) -> Result<Value> {
    common::local_directory(backend, run_dir, "复制运行目录")?;
    let initial = common::binding(&run_dir.join("state.json"))?;
    let selected = registered::records(run_dir, side, false)?;
    let state = state::load(run_dir, false)?;
    if state::running_attempt(&state)?.is_some() {
        return Err("统一运行仍有控制器动作，暂不能形成稳定存储状态".into());
    }
    if !run_dir.join(registered::stage(side)).exists() && selected.is_empty() {
        return Ok(registered::unregistered(side));
    }
    let (request, descriptor) = registered::registration(backend, run_dir, side)?;
    let result = registered::observe_or_stop(
        backend,
        run_dir,
        &request,
        &descriptor,
        run_dir,
        "status",
        state::attempts(&state)?.len() as u64 + 1,
    )?;
    if common::binding(&run_dir.join("state.json"))? != initial
        || registered::registration(backend, run_dir, side)? != (request, descriptor)
    {
        return Err("存储只读观察期间登记或阶段记录变化".into());
    }
    let mut result = result;
    result["copy_usable"] = Value::Bool(false);
    Ok(result)
}

fn manifest(run_dir: &Path) -> Result<Value> {
    super::super::json::read(&run_dir.join("manifest.json"))
}
