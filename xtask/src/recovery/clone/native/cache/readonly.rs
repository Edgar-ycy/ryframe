//! 缓存的活体状态只通过本机 Windows Redis v3 观察。
use super::super::json as document;
use crate::Result;
use serde_json::Value;
use std::path::Path;

pub(crate) fn status(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    effective: Option<&Value>,
) -> Result<Value> {
    super::require_windows(request)?;
    if let Some(effective) = effective {
        super::require_windows(effective)?;
    }
    #[cfg(windows)]
    return super::windows::status(backend, request, runtime, effective);
    #[cfg(not(windows))]
    {
        let _ = (backend, runtime);
        Err("Windows Redis 只读状态只支持本机 Windows".into())
    }
}

pub(in crate::recovery) fn handoff_markers(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    markers: &Value,
) -> Result<Value> {
    super::require_windows(request)?;
    #[cfg(windows)]
    return super::windows::handoff_markers(backend, request, runtime, markers);
    #[cfg(not(windows))]
    {
        let _ = (backend, runtime, markers);
        Err("RustFS 移交的 Redis 标记核对只支持本机 Windows".into())
    }
}

pub(in crate::recovery) fn external_termination(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    effective: Option<&Value>,
) -> Result<Value> {
    super::require_windows(request)?;
    if effective.is_some_and(|current| current != request) {
        return Err("Windows Redis 外部终止拒绝不同的有效请求".into());
    }
    #[cfg(windows)]
    return super::windows::external_termination(backend, request, runtime);
    #[cfg(not(windows))]
    {
        let _ = (backend, runtime);
        Err("Windows Redis 外部终止观察只支持本机 Windows".into())
    }
}

pub(super) fn port(backend: &Path, operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "status" => {
            document::exact(args, &["request", "runtime"], "缓存只读状态")?;
            status(backend, &args["request"], &args["runtime"], None)
        }
        "predecessor" => {
            document::exact(
                args,
                &["request", "runtime", "effective"],
                "缓存前代只读状态",
            )?;
            status(
                backend,
                &args["request"],
                &args["runtime"],
                Some(&args["effective"]),
            )
        }
        _ => Err("未知缓存只读端口".into()),
    }
}
