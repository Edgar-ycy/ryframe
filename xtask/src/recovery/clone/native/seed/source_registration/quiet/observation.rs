//! 在受管原生子进程内观察 CIM；异常不等于旧进程已经停止。
use std::{path::Path, process::Output, time::Duration};

use serde_json::{Value, json};

use super::timing::{self, Creation};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::clone::native::json as document,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CimObservation {
    Observed(Creation),
    QueryFailed { code: Option<i32> },
}

pub(crate) fn cim(backend: &Path, run: &Path, pid: u32, tool: &Path) -> Result<CimObservation> {
    validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
    validate_absolute_path(tool, LocalTestPathKind::ExistingFile)?;
    if pid <= 1 {
        return Err("CIM 观察必须指定大于 1 的 PID".into());
    }
    let request = serde_json::to_vec(&json!({
        "format_version":1,"kind":"ryframe-seed-evidence","run_dir":run,
        "operation":"process-time","arguments":{"pid":pid}
    }))?;
    let output = crate::process::command_output_with_input_timeout(
        backend,
        tool.to_str().ok_or("原生 CIM 工具路径必须是 UTF-8")?,
        &["__seed-evidence"],
        &[],
        Some(&request),
        Duration::from_secs(15),
    )?;
    response(pid, &output)
}

pub(crate) fn response(pid: u32, output: &Output) -> Result<CimObservation> {
    if !output.status.success() {
        return Err(format!(
            "原生 CIM 子进程未完成有效查询（{}）；保留失败现场，不得推定旧进程已停止",
            output.status
        )
        .into());
    }
    if output.stdout.is_empty() || output.stdout.len() > 65536 {
        return Err("原生 CIM 响应为空或超过 64 KiB；不能证明进程已停止".into());
    }
    let response = document::parse_fragment(
        std::str::from_utf8(&output.stdout).map_err(|_| "原生 CIM 响应不是 UTF-8")?,
        "原生 CIM 响应",
    )?;
    document::exact(&response, &["ok", "result"], "原生 CIM 协议")?;
    if response["ok"] != true {
        return Err("原生 CIM 协议未确认查询结果".into());
    }
    result(pid, &response["result"])
}

fn result(pid: u32, value: &Value) -> Result<CimObservation> {
    match value["status"].as_str() {
        Some("observed") => {
            document::exact(value, &["status", "rows"], "CIM 观察")?;
            let rows = document::array(&value["rows"], "CIM 进程行")?;
            Ok(CimObservation::Observed(timing::cim(pid, rows)?))
        }
        Some("query_failed") => {
            document::exact(value, &["status", "category", "code"], "CIM 查询失败")?;
            if value["category"] != "CimQueryFailed" || pid <= 1 {
                return Err("CIM 查询失败分类或请求 PID 无效".into());
            }
            let code = if value["code"].is_null() {
                None
            } else {
                Some(
                    value["code"]
                        .as_i64()
                        .and_then(|code| i32::try_from(code).ok())
                        .ok_or("CIM 查询错误码必须是 i32 或 null")?,
                )
            };
            Ok(CimObservation::QueryFailed { code })
        }
        _ => Err("CIM 响应状态无效或当前平台不支持；不能证明进程已停止".into()),
    }
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/seed/source_registration/quiet/observation.rs"]
mod tests;
