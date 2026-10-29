//! 清理错误只输出固定操作、类型与系统码；原错误文本不进入协议。
use super::diagnostic::{Diagnostic, primitive_detail};
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Operation {
    Cleanup,
    Inspect,
    CreateDirectory,
    Stop,
    WaitLauncher,
    ReadFailure,
    PublishPartial,
}

#[derive(Debug)]
struct Context {
    operation: Operation,
    error: Box<dyn Error>,
}
impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "缓存清理操作 {:?} 未完成", self.operation)
    }
}
impl Error for Context {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.error.as_ref())
    }
}

pub(super) fn context(operation: Operation, error: Box<dyn Error>) -> Box<dyn Error> {
    Box::new(Context { operation, error })
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Cause {
    PermissionDenied,
    TimedOut,
    NotFound,
    AlreadyExists,
    OtherIo,
    Diagnostic,
    RemoteExit,
    Rejected,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Detail {
    format_version: u8,
    kind: String,
    operation: Operation,
    cause: Cause,
    os_code: Option<i32>,
    diagnostic: Value,
    exit_code: Value,
}

pub(super) fn capture(error: &(dyn Error + 'static)) -> Value {
    let (operation, error) = error
        .downcast_ref::<Context>()
        .map_or((Operation::Cleanup, error), |value| {
            (value.operation, value.error.as_ref())
        });
    let (diagnostic, exit_code) = primitive_detail(error);
    let (cause, os_code) = if let Some(error) = error.downcast_ref::<std::io::Error>() {
        use std::io::ErrorKind::*;
        (
            match error.kind() {
                PermissionDenied => Cause::PermissionDenied,
                TimedOut => Cause::TimedOut,
                NotFound => Cause::NotFound,
                AlreadyExists => Cause::AlreadyExists,
                _ => Cause::OtherIo,
            },
            error.raw_os_error(),
        )
    } else if !diagnostic.is_null() {
        (Cause::Diagnostic, None)
    } else if !exit_code.is_null() {
        (Cause::RemoteExit, None)
    } else {
        (Cause::Rejected, None)
    };
    json!(Detail {
        format_version: 1,
        kind: "cache-cleanup-diagnostic".into(),
        operation,
        cause,
        os_code,
        diagnostic,
        exit_code
    })
}

pub(super) fn validate(value: &Value) -> Result<()> {
    super::super::json::exact(
        value,
        &[
            "format_version",
            "kind",
            "operation",
            "cause",
            "os_code",
            "diagnostic",
            "exit_code",
        ],
        "固定清理诊断",
    )?;
    let detail: Detail =
        serde_json::from_value(value.clone()).map_err(|_| "清理诊断字段或类别无效")?;
    let io = matches!(
        detail.cause,
        Cause::PermissionDenied
            | Cause::TimedOut
            | Cause::NotFound
            | Cause::AlreadyExists
            | Cause::OtherIo
    );
    if detail.format_version != 1
        || detail.kind != "cache-cleanup-diagnostic"
        || (!io && detail.os_code.is_some())
        || (!detail.exit_code.is_null()
            && detail
                .exit_code
                .as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .is_none_or(|v| v == 0))
        || (io && (!detail.diagnostic.is_null() || !detail.exit_code.is_null()))
        || (matches!(detail.cause, Cause::Diagnostic) != !detail.diagnostic.is_null())
        || (matches!(detail.cause, Cause::RemoteExit) && detail.exit_code.is_null())
        || (matches!(detail.cause, Cause::Rejected) && !detail.exit_code.is_null())
    {
        return Err("清理诊断类型、系统码或固定原因不一致".into());
    }
    if !detail.diagnostic.is_null() {
        if detail.diagnostic["kind"] == "cache-stop-diagnostic" {
            let _ = super::diagnostic::primitive_error(
                &detail.diagnostic,
                &detail.diagnostic["exit_code"],
            )?;
        } else {
            Diagnostic::decode(&detail.diagnostic)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct Failure {
    pub primary: Box<dyn Error>,
    pub cleanup: Value,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let primary = super::diagnostic::primitive_message(self.primary.as_ref())
            .unwrap_or_else(|| "启动失败；保留原始证据".into());
        write!(f, "{primary}；清理未确认：{}", self.cleanup)
    }
}
impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.primary.as_ref())
    }
}

pub(super) fn attach(primary: Box<dyn Error>, cleanup: Value) -> Result<Box<dyn Error>> {
    validate(&cleanup)?;
    Ok(Box::new(Failure { primary, cleanup }))
}

pub(crate) fn detail(error: &(dyn Error + 'static)) -> Value {
    error
        .downcast_ref::<Failure>()
        .map_or(Value::Null, |v| v.cleanup.clone())
}

pub(super) fn primary<'a>(error: &'a (dyn Error + 'static)) -> &'a (dyn Error + 'static) {
    error
        .downcast_ref::<Failure>()
        .map_or(error, |failure| primary(failure.primary.as_ref()))
}

pub(crate) fn decode(primary: Box<dyn Error>, cleanup: &Value) -> Result<Box<dyn Error>> {
    if cleanup.is_null() {
        Ok(primary)
    } else {
        attach(primary, cleanup.clone())
    }
}

pub(super) fn sidecar(request: &Value, output: &std::path::Path, original: &Value) -> Result<()> {
    use super::super::{digest, json as doc};
    let path = output.join("cleanup-diagnostic.json");
    if !path.try_exists()? {
        return Ok(());
    }
    let value = doc::read(&path)?;
    doc::exact(
        &value,
        &["request_sha256", "launcher", "cleanup"],
        "清理诊断绑定",
    )?;
    if value["request_sha256"] != digest::plan_hash(request)?
        || value["launcher"] != original["launcher"]
        || doc::read(&output.join("failure.json"))?["cleanup_completed"] != false
    {
        return Err("清理诊断未绑定原请求、launcher或父未确认事实".into());
    }
    validate(&value["cleanup"])
}
