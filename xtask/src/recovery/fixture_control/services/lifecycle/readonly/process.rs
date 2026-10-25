//! 直接观察登记进程的参数与监听所有者，不执行脚本或推断其他进程的身份。
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::Value;

pub(crate) fn arguments(identity: &Value) -> Result<Vec<String>> {
    let pid = pid(identity)?;
    require(identity)?;
    let result = platform_arguments(pid)?;
    require(identity)?;
    Ok(result)
}

pub(crate) fn require(identity: &Value) -> Result<()> {
    if common::process_identity(pid(identity)?)?.as_ref() != Some(identity) {
        return Err("登记进程已退出或内核创建身份变化".into());
    }
    Ok(())
}

pub(crate) fn listener(identity: &Value, url: &str) -> Result<()> {
    let pid = pid(identity)?;
    let (port, _) = common::endpoint(url)?;
    require(identity)?;
    if !platform_listener(pid, port)? {
        return Err("监听所有者不同于登记内核进程；核对绑定端点与服务进程".into());
    }
    require(identity)
}

fn pid(identity: &Value) -> Result<u32> {
    common::validate_process_identity(identity)?;
    u32::try_from(identity["pid"].as_u64().ok_or("进程 PID 无效")?)
        .map_err(|_| "进程 PID 超出范围".into())
}

#[cfg(windows)]
fn platform_arguments(pid: u32) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Row {
        #[serde(rename = "ProcessId")]
        pid: u32,
        #[serde(rename = "CommandLine")]
        command: Option<String>,
    }
    let connection = wmi::WMIConnection::new()
        .map_err(|_| "进程参数 CIM 连接失败；检查本机读取权限，不能推断进程已退出")?;
    let rows: Vec<Row> = connection
        .raw_query(format!(
            "SELECT ProcessId,CommandLine FROM Win32_Process WHERE ProcessId={pid}"
        ))
        .map_err(|_| "进程参数 CIM 查询失败；检查本机读取权限，保留现场")?;
    if rows.len() != 1 || rows[0].pid != pid {
        return Err("进程参数观察未返回唯一登记 PID".into());
    }
    let line = rows[0]
        .command
        .as_deref()
        .ok_or("进程参数不可读，不能用空参数放行")?;
    crate::recovery::clone::native::target_verify::storage::kernel::windows_argv(line)
}

#[cfg(windows)]
fn platform_listener(pid: u32, port: u16) -> Result<bool> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Row {
        #[serde(rename = "LocalAddress")]
        address: String,
        #[serde(rename = "LocalPort")]
        port: u16,
        #[serde(rename = "OwningProcess")]
        owner: u32,
        #[serde(rename = "State")]
        state: u8,
    }
    let connection = wmi::WMIConnection::with_namespace_path("ROOT\\StandardCimv2")
        .map_err(|_| "监听 CIM 连接失败；检查本机读取权限，保留现场")?;
    let rows: Vec<Row> = connection.raw_query(format!(
        "SELECT LocalAddress,LocalPort,OwningProcess,State FROM MSFT_NetTCPConnection WHERE LocalPort={port} AND State=2"))
        .map_err(|_| "监听 CIM 查询失败；检查本机读取权限，保留现场")?;
    for row in &rows {
        if row.port != port || row.state != 2 || row.address.parse::<std::net::IpAddr>().is_err() {
            return Err("监听 CIM 返回不符合固定查询的记录".into());
        }
    }
    Ok(rows
        .iter()
        .any(|row| row.owner == pid && matches!(row.address.as_str(), "127.0.0.1" | "0.0.0.0")))
}

#[cfg(not(windows))]
fn platform_arguments(pid: u32) -> Result<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline"))?;
    let mut result = bytes
        .split(|value| *value == 0)
        .map(|part| {
            std::str::from_utf8(part)
                .map(str::to_owned)
                .map_err(|_| "进程参数不是 UTF-8".into())
        })
        .collect::<Result<Vec<_>>>()?;
    if result.last().is_some_and(String::is_empty) {
        result.pop();
    }
    Ok(result)
}

#[cfg(not(windows))]
fn platform_listener(pid: u32, port: u16) -> Result<bool> {
    let mut sockets = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(format!("/proc/{pid}/fd"))? {
        let entry = entry?;
        let target = match std::fs::read_link(entry.path()) {
            Ok(target) => target,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if let Some(inode) = target
            .to_str()
            .and_then(|s| s.strip_prefix("socket:["))
            .and_then(|s| s.strip_suffix(']'))
        {
            sockets.insert(inode.to_owned());
        }
    }
    let table = std::fs::read_to_string(format!("/proc/{pid}/net/tcp"))?;
    for line in table.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 10 {
            return Err("内核监听表记录不完整".into());
        }
        let (address, actual) = fields[1].split_once(':').ok_or("内核监听地址无效")?;
        let actual = u16::from_str_radix(actual, 16)?;
        if actual == port
            && fields[3] == "0A"
            && matches!(address, "0100007F" | "00000000")
            && sockets.contains(fields[9])
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(all(test, windows))]
#[test]
#[ignore = "需要本机 CIM 查询权限；仅观察当前测试进程与自身临时监听端口"]
fn current_process_arguments_and_owned_listener_are_observed_by_native_cim() {
    let identity = common::process_identity(std::process::id())
        .unwrap()
        .unwrap();
    assert!(arguments(&identity).unwrap() == std::env::args().collect::<Vec<_>>());
    let socket = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://127.0.0.1:{}", socket.local_addr().unwrap().port());
    listener(&identity, &url).unwrap();
    let mut foreign = identity.clone();
    foreign["started"] = serde_json::json!("0");
    assert!(listener(&foreign, &url).is_err());
    drop(socket);
    assert!(listener(&identity, &url).is_err());
}
