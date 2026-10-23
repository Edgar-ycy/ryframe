use std::path::Path;

use serde_json::{Value, json};

use crate::Result;

#[cfg(any(windows, test))]
pub(crate) fn validate_windows_acl(value: &Value) -> Result<Value> {
    use super::contract::exact;
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
    let mut readable = false;
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
        readable |= sid == current && rights & 1 != 0;
        normalized.push((sid.to_owned(), rights, entry.clone()));
    }
    if !readable {
        return Err("当前用户没有读取监控凭据的明确权限".into());
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
    use std::{
        io::Write,
        path::PathBuf,
        process::{Command, Stdio},
    };
    // 由系统提供的安全 API 读取原始 ACE；校验和收据规范化仍在 Rust 完成。
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$credentialPath = [Console]::In.ReadToEnd()
if ([string]::IsNullOrEmpty($credentialPath)) { throw 'Missing credential path' }
$acl = Get-Acl -LiteralPath $credentialPath
$raw = [System.Security.AccessControl.RawSecurityDescriptor]::new($acl.GetSecurityDescriptorBinaryForm(), 0)
$entries = @()
foreach ($ace in $raw.DiscretionaryAcl) {
    if ([int]$ace.AceType -notin @(0, 1)) { throw 'Unsupported credential ACE type' }
    $entries += @{
        sid = $ace.SecurityIdentifier.Value
        type = [int]$ace.AceType
        rights = ([long]$ace.AccessMask -band 4294967295)
        inherited = (([int]$ace.AceFlags -band 16) -ne 0)
    }
}
@{
    current_sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    owner_sid = $raw.Owner.Value
    protected = (([int]$raw.ControlFlags -band 4096) -ne 0)
    entries = $entries
} | ConvertTo-Json -Compress -Depth 8
"#;
    let system = std::env::var_os("SystemRoot").ok_or("Windows 缺少 SystemRoot")?;
    let credential_path = path
        .to_str()
        .ok_or("监控凭据路径必须是有效的 Unicode 路径")?;
    let mut child = Command::new(
        PathBuf::from(system)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe"),
    )
    .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
    .env_clear()
    .envs(super::config::safe_environment(
        path.parent().ok_or("监控凭据缺少父目录")?,
    )?)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| format!("无法启动监控凭据 Windows DACL 检查：{error}"))?;
    child
        .stdin
        .take()
        .ok_or("监控凭据 Windows DACL 检查缺少标准输入")?
        .write_all(credential_path.as_bytes())
        .map_err(|error| format!("无法传递监控凭据路径：{error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("无法读取监控凭据 Windows DACL：{error}"))?;
    if !output.status.success() {
        return Err(format!("读取监控凭据 Windows DACL 失败：退出状态 {}", output.status).into());
    }
    let raw: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "监控凭据 Windows DACL 结果不是有效 JSON")?;
    validate_windows_acl(&raw)
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
