use std::path::Path;

use serde_json::Value;
#[cfg(unix)]
use serde_json::json;

use crate::Result;

#[cfg(windows)]
#[path = "permissions/windows_policy.rs"]
mod windows_policy;

#[cfg(windows)]
pub(crate) use windows_policy::validate_windows_acl;

#[cfg(windows)]
pub(crate) fn security_receipt(path: &Path) -> Result<Value> {
    let acl = crate::windows_acl::read(path).map_err(|error| {
        format!(
            "无法核验监控凭据 Windows DACL（{error}）。请核对当前用户的标准 Read 权限，仅 RC,RD 缺少 SYNCHRONIZE"
        )
    })?;
    validate_windows_acl(&acl.monitoring_value())
}

#[cfg(unix)]
pub(crate) fn security_receipt(path: &Path) -> Result<Value> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = std::fs::metadata(path)?;
    let mode = metadata.permissions().mode() & 0o7777;
    if metadata.uid() != nix::unistd::getuid().as_raw() || mode & 0o077 != 0 {
        return Err("监控凭据必须由当前用户拥有且只允许当前用户访问".into());
    }
    Ok(json!({"platform": "posix", "owner": metadata.uid(), "mode": mode}))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn security_receipt(_path: &Path) -> Result<Value> {
    Err("当前平台无法核验监控凭据权限".into())
}
