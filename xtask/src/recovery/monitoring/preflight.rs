use std::{path::Path, time::Duration};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::command_output_with_input_timeout,
    recovery::clone::native::json as json_io,
};

use super::{binding::validate_authority, contract::read_document, execution::ProtectedInputs};

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
    if protocol.len() > 16 * 1024 {
        return Err("监控预检协议超过 16 KiB".into());
    }
    let output = command_output_with_input_timeout(
        backend,
        "python",
        &[
            "-E",
            "-s",
            "-B",
            "tools/python/restore_monitoring_authority.py",
        ],
        &[],
        Some(&protocol),
        Duration::from_secs(120),
    )?;
    if !output.status.success() {
        return Err(format!(
            "正式恢复监控预检失败，退出状态 {}；静态来源或存活代次未获确认，未启动监控。\n{}\n请先运行 cargo xtask check recovery runtime verify --help，按原登记信息核对；不得重放数据写入。",
            output.status, String::from_utf8_lossy(&output.stderr).trim(),
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
