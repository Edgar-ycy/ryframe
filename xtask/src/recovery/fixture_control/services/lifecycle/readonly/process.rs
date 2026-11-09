//! 直接观察登记进程的参数与监听所有者，不执行脚本或推断其他进程的身份。
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr};

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
    let (port, _) = common::endpoint(url)?;
    listener_at(identity, Ipv4Addr::LOCALHOST.into(), port)
}

pub(crate) fn listener_at(identity: &Value, address: IpAddr, port: u16) -> Result<()> {
    if !observe_listener(identity, address, port, ListenerMode::AnyOwned)?.owned {
        return Err(LISTENER_OWNER_ERROR.into());
    }
    require(identity)
}

/// 已登记身份的监听可以尚未出现；其它所有者或无法确认身份时直接拒绝。
pub(in crate::recovery) fn start_listener_at(
    identity: &Value,
    address: IpAddr,
    port: u16,
) -> Result<bool> {
    let observation = observe_listener(identity, address, port, ListenerMode::Exclusive)?;
    require(identity)?;
    if observation.foreign {
        return Err(LISTENER_OWNER_ERROR.into());
    }
    Ok(observation.owned)
}

const LISTENER_OWNER_ERROR: &str = "监听所有者不同于登记内核进程；核对绑定端点与服务进程";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListenerMode {
    AnyOwned,
    Exclusive,
}

#[derive(Default)]
struct ListenerObservation {
    owned: bool,
    foreign: bool,
}

impl ListenerObservation {
    fn record(&mut self, owned: bool) {
        self.owned |= owned;
        self.foreign |= !owned;
    }
}

fn observe_listener(
    identity: &Value,
    address: IpAddr,
    port: u16,
    mode: ListenerMode,
) -> Result<ListenerObservation> {
    if address != IpAddr::V4(Ipv4Addr::LOCALHOST)
        && address != IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
    {
        return Err("监听核验只接受登记的 IPv4 或 IPv6 回环地址".into());
    }
    let pid = pid(identity)?;
    require(identity)?;
    platform_listener(pid, address, port, mode)
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
pub(crate) fn loopback_listener(identity: &Value, port: u16) -> Result<bool> {
    let pid = pid(identity)?;
    if port == 0 {
        return Err("登记回环监听端口无效".into());
    }
    require(identity)?;
    let rows = windows_listeners(port);
    require(identity)?;
    match rows?.as_slice() {
        [] => Ok(false),
        [row] if row.owner == pid && row.address == "127.0.0.1" => Ok(true),
        _ => Err("监听记录必须是唯一登记进程的 127.0.0.1 端点".into()),
    }
}

#[cfg(windows)]
pub(crate) fn require_loopback_peer(identity: &Value, stream: &std::net::TcpStream) -> Result<()> {
    use std::net::{Ipv4Addr, SocketAddr};

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Row {
        #[serde(rename = "LocalAddress")]
        local_address: String,
        #[serde(rename = "LocalPort")]
        local_port: u16,
        #[serde(rename = "RemoteAddress")]
        remote_address: String,
        #[serde(rename = "RemotePort")]
        remote_port: u16,
        #[serde(rename = "OwningProcess")]
        owner: u32,
        #[serde(rename = "State")]
        state: u8,
    }

    let pid = pid(identity)?;
    require(identity)?;
    let (SocketAddr::V4(client), SocketAddr::V4(server)) =
        (stream.local_addr()?, stream.peer_addr()?)
    else {
        return Err("已连接套接字必须使用 IPv4 回环".into());
    };
    if *client.ip() != Ipv4Addr::LOCALHOST || *server.ip() != Ipv4Addr::LOCALHOST {
        return Err("已连接套接字必须使用精确的 127.0.0.1 回环".into());
    }
    let server_port = server.port();
    let client_port = client.port();
    // 查询当前客户端套接字的反向四元组，取得服务端的已建立连接。
    let connection = wmi::WMIConnection::with_namespace_path("ROOT\\StandardCimv2")
        .map_err(|_| "连接归属 CIM 连接失败；检查本机读取权限，保留现场")?;
    let rows: Result<Vec<Row>> = connection
        .raw_query(format!(
            "SELECT LocalAddress,LocalPort,RemoteAddress,RemotePort,OwningProcess,State \
             FROM MSFT_NetTCPConnection WHERE LocalAddress='127.0.0.1' AND LocalPort={server_port} \
             AND RemoteAddress='127.0.0.1' AND RemotePort={client_port} AND State=5"
        ))
        .map_err(|_| "连接归属 CIM 查询失败；检查本机读取权限，保留现场".into());
    require(identity)?;
    if !matches!(rows?.as_slice(), [row]
        if row.local_address == "127.0.0.1"
            && row.local_port == server_port
            && row.remote_address == "127.0.0.1"
            && row.remote_port == client_port
            && row.owner == pid
            && row.state == 5)
    {
        return Err("已建立连接的反向四元组未唯一归属于登记进程".into());
    }
    Ok(())
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowsListener {
    #[serde(rename = "LocalAddress")]
    address: String,
    #[serde(rename = "LocalPort")]
    port: u16,
    #[serde(rename = "OwningProcess")]
    owner: u32,
    #[serde(rename = "State")]
    state: u8,
}

#[cfg(windows)]
fn windows_listeners(port: u16) -> Result<Vec<WindowsListener>> {
    let connection = wmi::WMIConnection::with_namespace_path("ROOT\\StandardCimv2")
        .map_err(|_| "监听 CIM 连接失败；检查本机读取权限，保留现场")?;
    let rows: Vec<WindowsListener> = connection.raw_query(format!(
        "SELECT LocalAddress,LocalPort,OwningProcess,State FROM MSFT_NetTCPConnection WHERE LocalPort={port} AND State=2"))
        .map_err(|_| "监听 CIM 查询失败；检查本机读取权限，保留现场")?;
    for row in &rows {
        if row.port != port || row.state != 2 || row.address.parse::<std::net::IpAddr>().is_err() {
            return Err("监听 CIM 返回不符合固定查询的记录".into());
        }
    }
    Ok(rows)
}

#[cfg(windows)]
fn platform_listener(
    pid: u32,
    address: IpAddr,
    port: u16,
    _mode: ListenerMode,
) -> Result<ListenerObservation> {
    let mut observation = ListenerObservation::default();
    for row in windows_listeners(port)? {
        if row.address.parse::<IpAddr>().is_ok_and(|actual| {
            actual == address || (actual.is_unspecified() && actual.is_ipv4() == address.is_ipv4())
        }) {
            observation.record(row.owner == pid);
        }
    }
    Ok(observation)
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
fn platform_listener(
    pid: u32,
    address: IpAddr,
    port: u16,
    mode: ListenerMode,
) -> Result<ListenerObservation> {
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
    let (protocol, loopback, wildcard) = match address {
        IpAddr::V4(_) => ("tcp", "0100007F", "00000000"),
        IpAddr::V6(_) => (
            "tcp6",
            "00000000000000000000000001000000",
            "00000000000000000000000000000000",
        ),
    };
    let table = std::fs::read_to_string(format!("/proc/{pid}/net/{protocol}"))?;
    let mut observation = ListenerObservation::default();
    for line in table.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 10 {
            return Err("内核监听表记录不完整".into());
        }
        let (address, actual) = fields[1].split_once(':').ok_or("内核监听地址无效")?;
        let actual = u16::from_str_radix(actual, 16)?;
        if actual == port && fields[3] == "0A" && (address == loopback || address == wildcard) {
            observation.record(sockets.contains(fields[9]));
            if observation.owned && mode == ListenerMode::AnyOwned {
                break;
            }
        }
    }
    Ok(observation)
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
