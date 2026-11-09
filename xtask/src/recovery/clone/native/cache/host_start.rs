//! 缓存启动只接受登记的 Windows Redis v3 代次。
use crate::Result;
use serde_json::Value;
use std::path::Path;

pub(crate) fn start(
    backend: &Path,
    request: &Value,
    private: &Value,
    output: &Path,
    guard: impl FnMut() -> Result<()>,
) -> Result<Value> {
    super::require_windows(request)?;
    #[cfg(windows)]
    return super::windows::start(backend, request, private, output, guard);
    #[cfg(not(windows))]
    {
        let _ = (backend, private, output, guard);
        Err("Windows Redis v3 启动仅支持本机 Windows".into())
    }
}
