use std::path::Path;

use serde_json::{Value, json};

use crate::Result;

#[cfg(any(windows, test))]
pub(crate) fn validate_windows_acl(value: &Value) -> Result<Value> {
    use super::contract::exact;
    const FILE_READ_DATA: u64 = 0x0000_0001;
    const REQUIRED_READ_RIGHTS: u64 = FILE_READ_DATA | crate::windows_acl::SYNCHRONIZE as u64;
    let acl = exact(
        value,
        &["current_sid", "owner_sid", "protected", "entries"],
        "监控凭据 ACL",
    )?;
    let current = acl["current_sid"]
        .as_str()
        .filter(|sid| sid.starts_with("S-1-"))
        .ok_or("监控凭据当前用户 SID 无效")?;
    let allowed = [current, "S-1-5-18", "S-1-5-32-544"];
    if acl["owner_sid"]
        .as_str()
        .is_none_or(|owner| !allowed.contains(&owner))
        || acl["protected"] != true
    {
        return Err("监控凭据必须由当前用户、System 或 Administrators 独占管理".into());
    }
    let entries = acl["entries"]
        .as_array()
        .filter(|entries| !entries.is_empty())
        .ok_or("监控凭据缺少明确 DACL")?;
    let mut current_rights = 0;
    let mut normalized = Vec::new();
    for entry in entries {
        let fields = exact(
            entry,
            &["sid", "type", "rights", "inherited"],
            "监控凭据 ACL 项",
        )?;
        let sid = fields["sid"].as_str().ok_or("监控凭据 ACL SID 无效")?;
        let rights = fields["rights"]
            .as_u64()
            .filter(|rights| *rights <= u32::MAX as u64)
            .ok_or("监控凭据 ACL 权限位无效")?;
        if !allowed.contains(&sid)
            || fields["type"].as_u64() != Some(0)
            || fields["inherited"] != false
        {
            return Err("监控凭据 ACL 包含未授权、继承或无法核验的主体".into());
        }
        if sid == current {
            current_rights |= rights;
        }
        normalized.push((sid.to_owned(), rights, entry.clone()));
    }
    if current_rights & REQUIRED_READ_RIGHTS != REQUIRED_READ_RIGHTS {
        return Err(
            "当前用户的显式允许项必须包含 FILE_READ_DATA 和 SYNCHRONIZE。标准 Read 包含两项，仅 RC,RD 不足"
                .into(),
        );
    }
    normalized.sort_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
    Ok(json!({
        "platform": "windows",
        "current_sid": current,
        "owner_sid": acl["owner_sid"],
        "entries": normalized.into_iter().map(|(_, _, value)| value).collect::<Vec<_>>(),
    }))
}

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
