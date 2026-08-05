//! 日志脱敏工具
//!
//! 提供常见敏感数据的掩码函数，用于日志输出前脱敏：
//! - 手机号 / 邮箱 / 身份证 / 银行卡
//! - 密码 / 令牌 / IP 地址
//! - JSON 敏感字段自动识别
//!
//! # 示例
//!
//! ```text
//! use ryframe_utils::log_mask::{mask_phone, mask_email, mask_token};
//!
//! ```

use std::{collections::HashSet, sync::LazyLock};

// ============ 敏感字段名 ============

/// 已知敏感 JSON 字段名集合。
static SENSITIVE_KEYS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut set = HashSet::new();
    // 密码相关
    set.insert("password");
    set.insert("passwd");
    set.insert("pwd");
    set.insert("oldPassword");
    set.insert("newPassword");
    set.insert("confirmPassword");
    set.insert("secret");
    set.insert("secretKey");
    // 令牌相关
    set.insert("token");
    set.insert("accessToken");
    set.insert("refreshToken");
    set.insert("apiKey");
    set.insert("apiSecret");
    set.insert("jwt");
    set.insert("authorization");
    // 身份相关
    set.insert("idCard");
    set.insert("idCardNo");
    set.insert("socialCreditCode");
    set.insert("passport");
    // 支付相关
    set.insert("bankCard");
    set.insert("bankCardNo");
    set.insert("cvv");
    set.insert("cvv2");
    // 其他
    set.insert("privateKey");
    set.insert("certificate");
    set
});

/// 检查字段名是否为敏感字段
pub fn is_sensitive_key(key: &str) -> bool {
    SENSITIVE_KEYS.contains(key)
}

// ============ 掩码函数 ============

/// 掩码手机号：保留前 3 后 4 位
///
/// ```text
/// # use ryframe_utils::log_mask::mask_phone;
/// ```
pub fn mask_phone(phone: &str) -> String {
    if phone.len() < 7 {
        return phone.to_string();
    }
    let prefix = &phone[..3];
    let suffix = &phone[phone.len() - 4..];
    format!("{}****{}", prefix, suffix)
}

/// 掩码邮箱：保留首字符和域名
///
/// ```text
/// # use ryframe_utils::log_mask::mask_email;
/// ```
pub fn mask_email(email: &str) -> String {
    if let Some(at_pos) = email.find('@') {
        let name = &email[..at_pos];
        let domain = &email[at_pos..];
        if name.len() <= 1 {
            format!("{}***{}", name, domain)
        } else {
            format!("{}***{}", &name[..1], domain)
        }
    } else {
        email.to_string()
    }
}

/// 掩码身份证号：保留前 3 后 4 位
///
/// ```text
/// # use ryframe_utils::log_mask::mask_id_card;
/// ```
pub fn mask_id_card(id_card: &str) -> String {
    if id_card.len() < 8 {
        return id_card.to_string();
    }
    let prefix = &id_card[..3];
    let suffix = &id_card[id_card.len() - 4..];
    let masked_len = id_card.len() - 7;
    format!("{}{}{}", prefix, "*".repeat(masked_len), suffix)
}

/// 掩码银行卡号：保留前 4 后 4 位
///
/// ```text
/// # use ryframe_utils::log_mask::mask_bank_card;
/// ```
pub fn mask_bank_card(card: &str) -> String {
    if card.len() < 8 {
        return card.to_string();
    }
    let prefix = &card[..4];
    let suffix = &card[card.len() - 4..];
    let masked_len = card.len() - 8;
    format!("{}{}{}", prefix, "*".repeat(masked_len), suffix)
}

/// 掩码密码：固定返回 "******"
pub fn mask_password(_password: &str) -> String {
    "******".to_string()
}

/// 掩码令牌：保留前 4 个字符。
///
/// ```text
/// # use ryframe_utils::log_mask::mask_token;
/// ```
pub fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        let show = token.len().min(2);
        format!("{}...", &token[..show])
    } else {
        format!("{}...", &token[..4])
    }
}

/// 掩码 IP 地址：保留前两段
///
/// ```text
/// # use ryframe_utils::log_mask::mask_ip;
/// ```
pub fn mask_ip(ip: &str) -> String {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() == 4 {
        format!("{}.{}.*.*", parts[0], parts[1])
    } else if ip.contains(':') {
        // IPv6：保留前两组
        let parts: Vec<&str> = ip.split(':').collect();
        if parts.len() >= 2 {
            format!("{}:{}:*", parts[0], parts[1])
        } else {
            ip.to_string()
        }
    } else {
        ip.to_string()
    }
}

/// 按字段名自动选择掩码方式
///
/// 根据字段名和值智能选择掩码策略。
pub fn mask_by_key(key: &str, value: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }

    let key_lower = key.to_lowercase();

    if key_lower.contains("password") || key_lower == "pwd" {
        return mask_password(value);
    }
    if key_lower.contains("token") || key_lower == "jwt" || key_lower.contains("secret") {
        return mask_token(value);
    }
    if key_lower.contains("phone") || key_lower.contains("mobile") {
        return mask_phone(value);
    }
    if key_lower.contains("email") || key_lower.contains("mail") {
        return mask_email(value);
    }
    if key_lower.contains("idcard") || key_lower.contains("id_card") || key_lower.contains("idno") {
        return mask_id_card(value);
    }
    if key_lower.contains("bank") || key_lower.contains("cardno") {
        return mask_bank_card(value);
    }
    if key_lower.contains("ip") || key_lower == "addr" {
        return mask_ip(value);
    }

    value.to_string()
}

// ============ 批量掩码 ============

/// 掩码查询字符串中的敏感参数值
///
/// 识别 `password=xxx&token=yyy` 中的敏感字段并掩码。
pub fn mask_query_string(query: &str) -> String {
    if query.is_empty() {
        return query.to_string();
    }

    query
        .split('&')
        .map(|pair| {
            if let Some((key, value)) = pair.split_once('=') {
                if is_sensitive_key(key) {
                    format!("{}={}", key, mask_by_key(key, value))
                } else {
                    pair.to_string()
                }
            } else {
                pair.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}
