use std::{net::IpAddr, sync::Arc};

use axum::http::HeaderMap;
use ryframe_kernel::IpCidr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

/// 已预解析的反向代理白名单，其中的转发请求头可以被信任。
#[derive(Debug, Clone, Default)]
pub struct TrustedProxySet {
    networks: Arc<Vec<IpCidr>>,
}

impl TrustedProxySet {
    pub fn new(cidrs: &[String]) -> Result<Self, String> {
        let networks = cidrs
            .iter()
            .map(|cidr| IpCidr::parse(cidr))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            networks: Arc::new(networks),
        })
    }

    pub fn is_trusted(&self, address: IpAddr) -> bool {
        self.networks
            .iter()
            .any(|network| network.contains(address))
    }

    /// 从右向左剥离受信任代理，解析真实客户端地址。
    /// 来自不受信任直连对端的转发请求头始终会被忽略。
    pub fn client_ip(&self, headers: &HeaderMap, peer: IpAddr) -> IpAddr {
        if !self.is_trusted(peer) {
            return peer;
        }

        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .into_iter()
            .flat_map(|value| value.split(','))
            .filter_map(|value| value.trim().parse::<IpAddr>().ok())
            .collect::<Vec<_>>();

        if forwarded.is_empty() {
            return headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<IpAddr>().ok())
                .unwrap_or(peer);
        }

        let mut current = peer;
        for address in forwarded.into_iter().rev() {
            if !self.is_trusted(current) {
                break;
            }
            current = address;
        }
        current
    }
}

/// 判断是否为内网 IP。
pub fn is_internal_ip(ip: &str) -> bool {
    ip.parse::<IpAddr>().is_ok_and(|address| match address {
        IpAddr::V4(address) => address.is_private() || address.is_loopback(),
        IpAddr::V6(address) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
    })
}

/// 解析用于登录和在线用户展示的粗粒度位置标签。
pub fn get_ip_location(ip: &str) -> Option<String> {
    let address = ip.parse::<IpAddr>().ok()?;
    if address.is_loopback() {
        return Some("本地".to_string());
    }
    if is_internal_ip(ip) {
        return Some("内网IP".to_string());
    }
    None
}
