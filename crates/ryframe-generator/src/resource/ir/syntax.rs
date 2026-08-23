pub(super) fn is_snake_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !value.ends_with('_')
        && !value.contains("__")
}

pub(super) fn is_operation_symbol(value: &str) -> bool {
    matches!(value.bytes().next(), Some(b'a'..=b'z' | b'A'..=b'Z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(super) fn is_safe_symbol(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

pub(super) fn is_safe_route(value: &str) -> bool {
    value.starts_with('/')
        && value.len() > 1
        && !value.contains("..")
        && !value.contains('?')
        && !value.contains('#')
        && !value.chars().any(char::is_whitespace)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'{' | b'}' | b':')
        })
}
