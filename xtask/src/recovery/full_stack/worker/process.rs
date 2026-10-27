//! 创建身份与监听归属核验。只查询登记 PID，绝不按端口清理。
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::Value;
#[cfg(unix)]
use std::fs;
use std::path::Path;

pub(super) fn current() -> Result<Value> {
    common::process_identity(std::process::id())?.ok_or_else(|| "无法取得当前控制器创建身份".into())
}
pub(super) fn alive(identity: &Value) -> Result<bool> {
    super::documents::identity(identity)?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("进程PID无效")?)?;
    match common::process_identity(pid)? {
        None => Ok(false),
        Some(actual) if actual == *identity => Ok(true),
        Some(_) => Err("进程PID已复用，拒绝操作其他创建身份".into()),
    }
}
pub(super) fn listener(identity: &Value, url: &str) -> Result<()> {
    if !alive(identity)? {
        return Err("已登记角色退出".into());
    }
    let url = reqwest::Url::parse(url)?;
    let port = url.port_or_known_default().ok_or("缺少监听端口")?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
        || url.path() != "/readyz"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("角色必须使用明确的本机就绪端点".into());
    }
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("PID无效")?)?;
    if !owns_listener(pid, port, url.host_str().ok_or("缺少监听地址")?)? {
        return Err("就绪端口不属于已登记的角色进程".into());
    }
    if !alive(identity)? {
        return Err("角色在监听核验后退出".into());
    }
    Ok(())
}
#[cfg(windows)]
fn owns_listener(pid: u32, port: u16, host: &str) -> Result<bool> {
    // Windows 系统 netstat 只读导出内核 TCP 表；固定绝对入口和参数，无 shell 插值。
    let executable =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows缺少SystemRoot")?)
            .join("System32/netstat.exe");
    let mut command = std::process::Command::new(executable);
    super::launch::environment(&mut command);
    use std::os::windows::process::CommandExt;
    let output = command
        .creation_flags(0x0800_0000)
        .args(["-ano", "-p", "tcp"])
        .output()?;
    if !output.status.success() {
        return Err("无法读取Windows监听表".into());
    }
    let (local, wildcard) = if host == "127.0.0.1" {
        (format!("127.0.0.1:{port}"), format!("0.0.0.0:{port}"))
    } else {
        (format!("[::1]:{port}"), format!("[::]:{port}"))
    };
    let pid = pid.to_string();
    Ok(String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        fields.len() == 5
            && fields[0] == "TCP"
            && (fields[1] == local || fields[1] == wildcard)
            && fields[3] == "LISTENING"
            && fields[4] == pid
    }))
}
#[cfg(unix)]
fn owns_listener(pid: u32, port: u16, host: &str) -> Result<bool> {
    let root = std::path::PathBuf::from(format!("/proc/{pid}"));
    let mut sockets = std::collections::HashSet::new();
    for entry in fs::read_dir(root.join("fd"))? {
        match fs::read_link(entry?.path()) {
            Ok(target) => {
                let name = target.to_string_lossy();
                if let Some(inode) = name
                    .strip_prefix("socket:[")
                    .and_then(|v| v.strip_suffix(']'))
                {
                    sockets.insert(inode.to_owned());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let ipv6 = host != "127.0.0.1";
    for line in fs::read_to_string(root.join(if ipv6 { "net/tcp6" } else { "net/tcp" }))?
        .lines()
        .skip(1)
    {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 10 {
            return Err("Linux监听表格式无效".into());
        }
        let Some((address, raw_port)) = fields[1].split_once(':') else {
            return Err("Linux监听地址无效".into());
        };
        if fields[3] == "0A"
            && sockets.contains(fields[9])
            && u16::from_str_radix(raw_port, 16)? == port
            && loopback_or_unspecified(address, ipv6)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(unix)]
fn loopback_or_unspecified(address: &str, ipv6: bool) -> Result<bool> {
    let expected = if ipv6 { 32 } else { 8 };
    if address.len() != expected || !address.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Linux监听地址编码无效".into());
    }
    let mut bytes = vec![];
    for offset in (0..expected).step_by(8) {
        bytes.extend_from_slice(
            &u32::from_str_radix(&address[offset..offset + 8], 16)?.to_ne_bytes(),
        );
    }
    if ipv6 {
        let value = std::net::Ipv6Addr::from(<[u8; 16]>::try_from(bytes.as_slice())?);
        Ok(value.is_loopback() || value.is_unspecified())
    } else {
        let value = std::net::Ipv4Addr::from(<[u8; 4]>::try_from(bytes.as_slice())?);
        Ok(value == std::net::Ipv4Addr::LOCALHOST || value.is_unspecified())
    }
}
pub(super) fn registered(
    role: super::role::Role,
    directory: &Path,
    runtime: &Value,
) -> Result<Option<Value>> {
    let path = directory.join(format!("{}.json", role.name()));
    if !path.exists() {
        return Ok(None);
    }
    let receipt = super::operation::read(&path)?;
    crate::recovery::clone::native::json::exact(
        &receipt,
        &["format_version", "role", "scope_id", "identity"],
        "角色进程收据",
    )?;
    if receipt["format_version"].as_u64() != Some(1)
        || receipt["role"] != role.name()
        || receipt["scope_id"] != runtime["scope_id"]
        || receipt["identity"]["executable"] != runtime["artifacts"][role.name()]["path"]
    {
        return Err("角色进程收据与构建产物或scope不匹配".into());
    }
    let identity = receipt["identity"].clone();
    Ok(alive(&identity)?.then_some(identity))
}
