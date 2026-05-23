use axum::http::HeaderMap;

/// 从请求头提取客户端真实 IP
///
/// 优先级：X-Forwarded-For（第一个） → X-Real-IP → 直连 IP（由 SocketAddr 获取）
pub fn get_client_ip(headers: &HeaderMap, remote_addr: &str) -> String {
    // 1. 尝试 X-Forwarded-For
    if let Some(forwarded) = headers.get("x-forwarded-for")
        && let Ok(value) = forwarded.to_str() {
            // 取第一个 IP（最左侧的是原始客户端）
            if let Some(ip) = value.split(',').next() {
                let ip = ip.trim();
                if !ip.is_empty() {
                    return ip.to_string();
                }
            }
        }

    // 2. 尝试 X-Real-IP
    if let Some(real_ip) = headers.get("x-real-ip")
        && let Ok(value) = real_ip.to_str() {
            let ip = value.trim();
            if !ip.is_empty() {
                return ip.to_string();
            }
        }

    // 3. 回退到直连 IP
    // remote_addr 格式通常是 "127.0.0.1:8080"，去掉端口
    remote_addr
        .split(':')
        .next()
        .unwrap_or("unknown")
        .to_string()
}

/// 判断是否为内网 IP
pub fn is_internal_ip(ip: &str) -> bool {
    // 简单判断：以 10. / 172.16-31. / 192.168. / 127. 开头
    ip.starts_with("10.")
        || ip.starts_with("127.")
        || ip.starts_with("192.168.")
        || ip.starts_with("172.")
        && ip[4..]
        .split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .is_some_and(|n| (16..=31).contains(&n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn test_get_client_ip_and_internal() {
        // X-Forwarded-For 优先
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        h.insert("x-real-ip", "10.0.0.1".parse().unwrap());
        assert_eq!(get_client_ip(&h, "127.0.0.1:8080"), "1.2.3.4");

        // 回退到 X-Real-IP
        let mut h2 = HeaderMap::new();
        h2.insert("x-real-ip", "10.0.0.1".parse().unwrap());
        assert_eq!(get_client_ip(&h2, "127.0.0.1:8080"), "10.0.0.1");

        // 回退到直连
        assert_eq!(get_client_ip(&HeaderMap::new(), "192.168.1.1:8080"), "192.168.1.1");

        // 内网 IP 判断
        assert!(is_internal_ip("10.0.0.1"));
        assert!(is_internal_ip("192.168.1.100"));
        assert!(is_internal_ip("127.0.0.1"));
        assert!(!is_internal_ip("8.8.8.8"));
    }
}