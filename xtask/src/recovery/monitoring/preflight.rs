use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::{clone::native::json as json_io, runtime::dataset_authority::monitoring},
};

use super::{binding::validate_authority, contract::read_document, execution::ProtectedInputs};

#[path = "preflight/process.rs"]
pub(crate) mod process;

pub(super) const MARKER: &str = "__monitoring-preflight";
const MAX_PROTOCOL_BYTES: u64 = 16 * 1024;

/// 配置仅通过只读路径协议进入 stdin，预检进程不继承项目或凭据环境。
pub(crate) fn command(backend: &Path) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(MARKER)
        .current_dir(backend)
        .env_clear()
        .envs(super::config::safe_environment(
            &backend.join(".local-tests"),
        )?);
    Ok(command)
}

pub(super) fn run_native() -> Result<()> {
    let mut raw = Vec::new();
    io::stdin()
        .take(MAX_PROTOCOL_BYTES + 1)
        .read_to_end(&mut raw)?;
    if raw.is_empty() || raw.len() as u64 > MAX_PROTOCOL_BYTES {
        return Err("监控预检协议为空或超过 16 KiB".into());
    }
    let request = json_io::parse_fragment(std::str::from_utf8(&raw)?, "监控预检协议")?;
    let result = run_protocol(&request)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn run_protocol(request: &Value) -> Result<Value> {
    json_io::exact(
        request,
        &["format_version", "kind", "backend", "runtime", "target"],
        "监控预检协议",
    )?;
    if request["format_version"].as_u64() != Some(1) || request["kind"] != "monitoring-preflight" {
        return Err("监控预检协议版本或类型无效".into());
    }
    let path = |name| -> Result<PathBuf> {
        Ok(PathBuf::from(json_io::text(
            &request[name],
            "监控预检路径",
        )?))
    };
    let backend = path("backend")?;
    validate_absolute_path(&backend, LocalTestPathKind::ExistingDirectory)?;
    if crate::recovery::fixture_control::git_root(&backend, "监控协调后端")? != backend {
        return Err("监控预检后端必须是实际 Git 工作树根".into());
    }
    let runtime = path("runtime")?;
    let target = path("target")?;
    for file in [&runtime, &target] {
        validate_local_test_path(file, &backend, LocalTestPathKind::ExistingFile)?;
    }
    // 原预检失败输出不包含上游的配置或过程错误。
    let authority = monitoring::verify(&backend, &runtime, &target)
        .map_err(|_| "正式恢复预检的来源或存活代次未获确认")?;
    validate_authority(&authority)
}

/// 正式运行的静态来源及存活代次仍复用现有核验器，不能用收据字段校验替代。
pub(crate) fn verify(backend: &Path, runtime_path: &Path, target_path: &Path) -> Result<Value> {
    for path in [runtime_path, target_path] {
        validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    }
    let inputs = ProtectedInputs::open([runtime_path.to_path_buf(), target_path.to_path_buf()])?;
    let runtime = read_document(runtime_path, "监控恢复运行收据")?;
    let target = read_document(target_path, "监控正式目标计划")?;
    let protocol = serde_json::to_vec(&json!({
        "format_version": 1,
        "kind": "monitoring-preflight",
        "backend": backend,
        "runtime": runtime_path,
        "target": target_path,
    }))?;
    if protocol.len() as u64 > MAX_PROTOCOL_BYTES {
        return Err("监控预检协议超过 16 KiB".into());
    }
    let output = process::capture(
        &backend.join(".local-tests"),
        command(backend)?,
        &protocol,
        Duration::from_secs(120),
    )?;
    if !output.status.success() {
        return Err(format!(
            "正式恢复监控预检失败，退出状态 {}；静态来源或存活代次未获确认，未启动监控。请先运行 cargo xtask check recovery runtime verify --help，按原登记信息核对；不得重放数据写入。",
            output.status,
        ).into());
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|_| "监控预检输出不是 UTF-8")?;
    let value = json_io::parse_fragment(text, "监控正式运行权威")?;
    validate_authority(&value)?;
    if value["runtime"] != runtime.descriptor() || value["target_plan"] != target.descriptor() {
        return Err("监控预检没有核验当前输入的运行收据和目标计划".into());
    }
    runtime.assert_unchanged()?;
    target.assert_unchanged()?;
    inputs.finish(Ok(value))
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/monitoring/preflight.rs"]
mod tests;
