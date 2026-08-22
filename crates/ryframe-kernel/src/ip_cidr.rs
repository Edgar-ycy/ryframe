use std::net::IpAddr;

/// 已规范化的 IPv4 或 IPv6 CIDR 值对象。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpCidr {
    V4 { network: u32, prefix: u8 },
    V6 { network: u128, prefix: u8 },
}

impl IpCidr {
    pub fn parse(value: &str) -> Result<Self, String> {
        let (address, prefix) = value
            .trim()
            .split_once('/')
            .map_or((value.trim(), None), |(address, prefix)| {
                (address.trim(), Some(prefix.trim()))
            });
        let address = address
            .parse::<IpAddr>()
            .map_err(|error| format!("无效 IP 地址 {address:?}: {error}"))?;

        match address {
            IpAddr::V4(address) => {
                let prefix = parse_prefix(prefix, 32, value)?;
                Ok(Self::V4 {
                    network: u32::from(address) & prefix_mask_v4(prefix),
                    prefix,
                })
            }
            IpAddr::V6(address) => {
                let prefix = parse_prefix(prefix, 128, value)?;
                Ok(Self::V6 {
                    network: u128::from(address) & prefix_mask_v6(prefix),
                    prefix,
                })
            }
        }
    }

    pub fn contains(self, address: IpAddr) -> bool {
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
            .map_err(|error| format!("CIDR 前缀无效 {original:?}: {error}"))
    })?;
    if prefix > max {
        return Err(format!("CIDR 前缀 {original:?} 必须小于或等于 {max}"));
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
