#![allow(dead_code)]

use rand::RngExt;

/// 将驼峰命名转为下划线命名
///
/// 示例："UserName" → "user_name", "HTTPResponse" → "http_response"
pub fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_uppercase() {
            if !result.is_empty() {
                // 下一个字符是小写时，当前大写前加下划线
                if chars.peek().is_none_or(|next| next.is_lowercase()) {
                    result.push('_');
                }
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

/// 将下划线命名转为驼峰命名
///
/// 示例："user_name" → "userName"
pub fn to_camel_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = false;

    for c in s.chars() {
        if c == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(c.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }
    result
}

/// 将下划线命名转为帕斯卡命名（首字母大写驼峰）
///
/// 示例："user_name" → "UserName"
pub fn to_pascal_case(s: &str) -> String {
    let camel = to_camel_case(s);
    let mut chars = camel.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
    }
}

/// 生成指定长度的随机字符串（字母 + 数字）
pub fn random_string(length: usize) -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_snake_case() {
        assert_eq!(to_snake_case("UserName"), "user_name");
        assert_eq!(to_snake_case("HTTPResponse"), "http_response");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
        assert_eq!(to_snake_case(""), "");
        assert_eq!(to_snake_case("A"), "a");
    }

    #[test]
    fn test_to_camel_case() {
        assert_eq!(to_camel_case("user_name"), "userName");
        assert_eq!(to_camel_case("hello_world_test"), "helloWorldTest");
        assert_eq!(to_camel_case(""), "");
        assert_eq!(to_camel_case("already"), "already");
    }

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("user_name"), "UserName");
        assert_eq!(to_pascal_case("hello"), "Hello");
        assert_eq!(to_pascal_case(""), "");
    }

    #[test]
    fn test_random_string() {
        let s = random_string(10);
        assert_eq!(s.len(), 10);
        assert!(s.chars().all(|c| c.is_alphanumeric()));

        let s2 = random_string(0);
        assert!(s2.is_empty());

        // 两次生成的应该不同（极低概率重复）
        let a = random_string(20);
        let b = random_string(20);
        assert_ne!(a, b);
    }
}
