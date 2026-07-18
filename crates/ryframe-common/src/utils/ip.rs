use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::http::HeaderMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpNetwork {
    V4 { network: u32, prefix: u8 },
    V6 { network: u128, prefix: u8 },
}

impl IpNetwork {
    fn parse(value: &str) -> Result<Self, String> {
        let (address, prefix) = value
            .trim()
            .split_once('/')
            .map_or((value.trim(), None), |(address, prefix)| {
                (address.trim(), Some(prefix.trim()))
            });
        let address = address
            .parse::<IpAddr>()
            .map_err(|error| format!("invalid IP address {address:?}: {error}"))?;

        match address {
            IpAddr::V4(address) => {
                let prefix = parse_prefix(prefix, 32, value)?;
                let mask = prefix_mask_v4(prefix);
                Ok(Self::V4 {
                    network: u32::from(address) & mask,
                    prefix,
                })
            }
            IpAddr::V6(address) => {
                let prefix = parse_prefix(prefix, 128, value)?;
                let mask = prefix_mask_v6(prefix);
                Ok(Self::V6 {
                    network: u128::from(address) & mask,
                    prefix,
                })
            }
        }
    }

    fn contains(self, address: IpAddr) -> bool {
        match (self, address) {
            (Self::V4 { network, prefix }, IpAddr::V4(address)) => {
                u32::from(address) & prefix_mask_v4(prefix) == network
            }
            (Self::V6 { network, prefix }, IpAddr::V6(address)) => {
                u128::from(address) & prefix_mask_v6(prefix) == network
            }
            _ => false,
        }
    }
}

fn parse_prefix(prefix: Option<&str>, max: u8, original: &str) -> Result<u8, String> {
    let prefix = prefix.map_or(Ok(max), |prefix| {
        prefix
            .parse::<u8>()
            .map_err(|error| format!("invalid CIDR prefix in {original:?}: {error}"))
    })?;
    if prefix > max {
        return Err(format!("CIDR prefix in {original:?} must be <= {max}"));
    }
    Ok(prefix)
}

const fn prefix_mask_v4(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

const fn prefix_mask_v6(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

/// A pre-parsed allow-list of reverse proxies whose forwarding headers may be trusted.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxySet {
    networks: Arc<Vec<IpNetwork>>,
}

impl TrustedProxySet {
    pub fn new(cidrs: &[String]) -> Result<Self, String> {
        let networks = cidrs
            .iter()
            .map(|cidr| IpNetwork::parse(cidr))
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

    /// Resolve the client address by peeling trusted proxies from the right-hand side.
    /// Forwarding headers from an untrusted direct peer are always ignored.
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

/// Resolve a direct peer address without trusting request-controlled headers.
/// New HTTP call sites should use [`TrustedProxySet::client_ip`] instead.
pub fn get_client_ip(_headers: &HeaderMap, remote_addr: &str) -> String {
    parse_remote_ip(remote_addr)
        .map(|address| address.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn parse_remote_ip(value: &str) -> Option<IpAddr> {
    value
        .parse::<SocketAddr>()
        .map(|address| address.ip())
        .or_else(|_| value.parse::<IpAddr>())
        .ok()
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

/// Resolve the coarse location label used by login and online-user displays.
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
