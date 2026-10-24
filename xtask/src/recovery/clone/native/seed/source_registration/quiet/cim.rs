//! 本机 CIMV2 的安全只读查询；超时与停止证明由受管父进程负责。
use crate::Result;
use serde_json::{Value, json};

#[cfg(any(windows, test))]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    #[serde(rename = "ProcessId")]
    process_id: u32,
    #[serde(rename = "CreationDate")]
    creation_date: String,
}

pub(crate) fn query(pid: u32) -> Result<Value> {
    let statement = statement(pid)?;
    #[cfg(windows)]
    {
        // 连接只在当前线程当前调用内使用，不接受命名空间、远端或凭据。
        let connection = match wmi::WMIConnection::new() {
            Ok(connection) => connection,
            Err(error) => return Ok(failed(error_code(&error))),
        };
        Ok(match connection.raw_query::<Row>(&statement) {
            Ok(rows) => observed(rows),
            Err(error) => failed(error_code(&error)),
        })
    }
    #[cfg(not(windows))]
    {
        let _ = statement;
        Ok(json!({"status":"unsupported","category":"CimUnsupported"}))
    }
}

fn statement(pid: u32) -> Result<String> {
    if pid <= 1 {
        return Err("CIM 查询必须指定大于 1 的整数 PID".into());
    }
    Ok(format!(
        "SELECT ProcessId,CreationDate FROM Win32_Process WHERE ProcessId={pid}"
    ))
}

#[cfg(any(windows, test))]
fn observed(rows: Vec<Row>) -> Value {
    let rows = rows
        .into_iter()
        .map(|row| json!({"ProcessId":row.process_id,"CreationDate":row.creation_date}))
        .collect::<Vec<_>>();
    json!({"status":"observed","rows":rows})
}

#[cfg(any(windows, test))]
fn failed(code: Option<i32>) -> Value {
    json!({"status":"query_failed","category":"CimQueryFailed","code":code})
}

#[cfg(windows)]
fn error_code(error: &wmi::WMIError) -> Option<i32> {
    match error {
        wmi::WMIError::HResultError { hres } => Some(*hres),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/seed/source_registration/quiet/cim.rs"]
mod tests;
