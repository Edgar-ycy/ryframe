//! 关闭端口观察保留IPv4、IPv6及双栈wildcard边界；未知观察不能当作关闭。
use crate::Result;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[cfg(any(windows, test))]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Listener {
    #[serde(rename = "LocalAddress")]
    address: String,
    #[serde(rename = "LocalPort")]
    port: u16,
    #[serde(rename = "State")]
    state: u8,
}

pub(crate) fn require_closed(value: &str) -> Result<()> {
    let address = endpoint(value)?;
    #[cfg(windows)]
    {
        // 固定本机provider；Listen=2及字段类型由微软MSFT_NetTCPConnection契约定义。
        let connection = wmi::WMIConnection::with_namespace_path("ROOT\\StandardCimv2")
            .map_err(|_| "目标监听表无法读取；检查本机CIM服务与读取权限，不得推断端口已关闭")?;
        let rows = connection
            .raw_query::<Listener>(
                "SELECT LocalAddress,LocalPort,State FROM MSFT_NetTCPConnection WHERE State=2",
            )
            .map_err(|_| "目标监听表查询失败；检查本机CIM服务与读取权限，不得推断端口已关闭")?;
        require_idle(address, &rows)
    }
    #[cfg(not(windows))]
    {
        use std::{io::ErrorKind, net::TcpStream, time::Duration};
        match TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
            Err(error) if error.kind() == ErrorKind::ConnectionRefused => Ok(()),
            _ => Err("目标精确端口仍在监听或无法明确证明连接被拒绝".into()),
        }
    }
}

fn endpoint(value: &str) -> Result<SocketAddr> {
    let url = url::Url::parse(value).map_err(|_| "目标关闭端口地址无效")?;
    let host = match url.host() {
        Some(url::Host::Ipv4(value)) if value == Ipv4Addr::LOCALHOST => IpAddr::V4(value),
        Some(url::Host::Ipv6(value)) if value == Ipv6Addr::LOCALHOST => IpAddr::V6(value),
        _ => return Err("目标关闭端口必须使用明确loopback地址".into()),
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("目标关闭端口地址含不支持的协议或额外字段".into());
    }
    let port = url
        .port_or_known_default()
        .filter(|value| *value > 0)
        .ok_or("目标端口无效")?;
    Ok(SocketAddr::new(host, port))
}

#[cfg(any(windows, test))]
fn require_idle(address: SocketAddr, rows: &[Listener]) -> Result<()> {
    for row in rows {
        let host: IpAddr = row
            .address
            .parse()
            .map_err(|_| "目标监听表地址无法解释；保留现场")?;
        if row.port == 0 || row.state != 2 {
            return Err("目标监听表返回非监听或无效端口；保留现场".into());
        }
        if row.port == address.port()
            && (host == address.ip()
                || host.is_unspecified() && (host.is_ipv6() || address.is_ipv4()))
        {
            return Err("明确测试端口已有监听，不能声明生产者停止".into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/target_verify/authority/ports.rs"]
mod tests;
