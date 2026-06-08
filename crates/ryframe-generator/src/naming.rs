/// Rust 保留关键字，用作字段名时追加 _ 后缀
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn",
    "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "override", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true",
    "try", "type", "union", "unsafe", "use", "where", "while", "yield",
];

/// 将字段名转为安全的 Rust 标识符（对关键字追加 _ 后缀）
pub fn safe_field_name(name: &str) -> String {
    let snake = to_snake_case(name);
    if RUST_KEYWORDS.contains(&snake.as_str()) {
        format!("{}_", snake)
    } else {
        snake
    }
}

/// 从表名中剥离前缀后转为 PascalCase 结构体名
/// 例如 "t_user" + prefixes ["t_"] → "User"
pub fn table_to_struct_name(table_name: &str, prefixes: &[String]) -> String {
    let stripped = strip_prefixes(table_name, prefixes);
    to_pascal_case(&stripped)
}

/// 剥离表名的公共前缀
/// 例如 "t_gongxv" + ["t_"] → "gongxv"
pub fn strip_prefixes(name: &str, prefixes: &[String]) -> String {
    for prefix in prefixes {
        if name.starts_with(prefix.as_str()) {
            return name[prefix.len()..].to_string();
        }
    }
    name.to_string()
}

/// 字符串转 snake_case
pub fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap_or(c));
        } else {
            result.push(c);
        }
    }
    result
}

/// 字符串转 PascalCase
pub fn to_pascal_case(s: &str) -> String {
    let snake = to_snake_case(s);
    snake
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().chain(chars).collect(),
            }
        })
        .collect()
}

/// 字符串转 camelCase
pub fn to_camel_case(s: &str) -> String {
    let pascal = to_pascal_case(s);
    let mut chars = pascal.chars();
    match chars.next() {
        None => String::new(),
        Some(f) => f.to_lowercase().chain(chars).collect(),
    }
}
