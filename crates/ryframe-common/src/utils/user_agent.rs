//! User-Agent 解析工具
//!
//! 从 HTTP User-Agent 字符串中提取浏览器和操作系统信息。

/// 从 User-Agent 解析浏览器名称
pub fn parse_browser(ua: &str) -> Option<String> {
    if ua.is_empty() {
        return None;
    }
    let ua_lower = ua.to_lowercase();
    let browser = if ua_lower.contains("edg/") {
        "Edge"
    } else if ua_lower.contains("chrome/") && !ua_lower.contains("edg/") {
        "Chrome"
    } else if ua_lower.contains("firefox/") {
        "Firefox"
    } else if ua_lower.contains("safari/") && !ua_lower.contains("chrome/") {
        "Safari"
    } else if ua_lower.contains("opera") || ua_lower.contains("opr/") {
        "Opera"
    } else {
        "Other"
    };
    Some(browser.to_string())
}

/// 从 User-Agent 解析操作系统名称
pub fn parse_os(ua: &str) -> Option<String> {
    if ua.is_empty() {
        return None;
    }
    let os = if ua.contains("Windows NT 10") {
        "Windows 10"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Mac OS X") {
        "macOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else {
        "Other"
    };
    Some(os.to_string())
}
