use std::{fs, path::Path};

pub(super) fn secure_directory(path: &Path) -> Result<(), &'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "mysql-option-permissions")?;
    }
    #[cfg(windows)]
    windows_acl(path, true)?;
    verify(path, true)
}

pub(super) fn verify(path: &Path, directory: bool) -> Result<(), &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "mysql-option-permissions")?;
    if super::super::invocation::linked(&metadata)
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err("mysql-option-permissions");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let expected = if directory { 0o700 } else { 0o600 };
        if metadata.permissions().mode() & 0o777 != expected
            || metadata.uid() != nix::unistd::geteuid().as_raw()
        {
            return Err("mysql-option-permissions");
        }
    }
    #[cfg(windows)]
    windows_acl(path, false)?;
    #[cfg(not(any(unix, windows)))]
    return Err("mysql-option-permissions");
    Ok(())
}

/// 使用系统自带的 ACL API，仅传路径；不传递或读取任何凭据。
#[cfg(windows)]
fn windows_acl(path: &Path, secure: bool) -> Result<(), &'static str> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let root = std::env::var_os("SystemRoot").ok_or("mysql-option-permissions")?;
    let executable = Path::new(&root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    if !executable.is_absolute() {
        return Err("mysql-option-permissions");
    }
    let path = path
        .to_str()
        .ok_or("mysql-option-permissions")?
        .replace('\'', "''");
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
try {{
  $path = '{path}'
  $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
  if ({secure}) {{
    $acl = [System.Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner($sid)
    $acl.SetAccessRuleProtection($true, $false)
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new($sid, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
    $acl.AddAccessRule($rule)
    Set-Acl -LiteralPath $path -AclObject $acl
  }}
  $acl = Get-Acl -LiteralPath $path
  if ($acl.GetOwner([System.Security.Principal.SecurityIdentifier]) -ne $sid) {{ exit 1 }}
  $rules = @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
  if ($rules.Count -eq 0) {{ exit 1 }}
  foreach ($rule in $rules) {{
    if ($rule.IdentityReference -ne $sid -or $rule.AccessControlType -ne 'Allow' -or $rule.FileSystemRights -ne 'FullControl') {{ exit 1 }}
  }}
  exit 0
}} catch {{ exit 1 }}
"#,
        secure = if secure { "$true" } else { "$false" }
    );
    let encoded = STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let mut command = std::process::Command::new(executable);
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-EncodedCommand",
        &encoded,
    ]);
    command.env_clear().env("SystemRoot", root);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    super::super::invocation::hide_window(&mut command);
    let mut child = command.spawn().map_err(|_| "mysql-option-permissions")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("mysql-option-permissions");
            }
        }
    }
}
