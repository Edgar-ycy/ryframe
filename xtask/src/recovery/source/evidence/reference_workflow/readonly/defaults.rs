//! reference 的 client INI 接受规则；无插值，不解析为应用配置。
use crate::Result;
use std::{collections::BTreeMap, path::Path};

pub(super) fn validate(path: &Path) -> Result<()> {
    let raw = std::fs::read_to_string(path).map_err(|_| "MySQL 凭据文件不是可读 UTF-8")?;
    let values = parse(&raw)?;
    if !matches!(
        values.get("host").map(String::as_str),
        Some("127.0.0.1" | "::1")
    ) || values.get("user").is_none_or(String::is_empty)
        || !values.contains_key("password")
        || !values.contains_key("port")
    {
        return Err("MySQL 凭据文件只允许本机连接、认证和 TLS 字段".into());
    }
    Ok(())
}

fn parse(raw: &str) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::<String, String>::new();
    let mut section = false;
    let mut client = false;
    let mut previous: Option<(String, usize)> = None;
    for line in raw.lines() {
        let stripped = line.trim();
        if stripped.starts_with(['#', ';']) {
            continue;
        }
        if stripped.is_empty() {
            if let Some((key, _)) = &previous {
                values.get_mut(key).ok_or("INI 字段缺失")?.push('\n');
            }
            continue;
        }
        let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
        if let Some((key, previous_indent)) = &previous
            && indent > *previous_indent
        {
            let value = values.get_mut(key).ok_or("INI 字段缺失")?;
            value.push('\n');
            value.push_str(stripped);
            continue;
        }
        if stripped.starts_with('[') {
            let end = stripped.rfind(']').ok_or("MySQL 凭据段格式无效")?;
            match &stripped[1..end] {
                "client" if !client => {
                    client = true;
                    section = true;
                }
                "DEFAULT" => section = false,
                _ => return Err("MySQL 凭据只允许唯一 client 段及空 DEFAULT".into()),
            }
            previous = None;
            continue;
        }
        if !section {
            return Err("MySQL 凭据字段必须位于 client 段".into());
        }
        let delimiter = stripped
            .find(['=', ':'])
            .ok_or("MySQL 凭据字段缺少分隔符")?;
        let key = stripped[..delimiter].trim_end().to_lowercase();
        if !matches!(
            key.as_str(),
            "host" | "port" | "user" | "password" | "ssl-mode" | "ssl-ca" | "ssl-cert" | "ssl-key"
        ) || values.contains_key(&key)
        {
            return Err("MySQL 凭据字段重复或不在允许列表".into());
        }
        values.insert(key.clone(), stripped[delimiter + 1..].trim().to_owned());
        previous = Some((key, indent));
    }
    if !client {
        return Err("MySQL 凭据缺少 client 段".into());
    }
    for value in values.values_mut() {
        value.truncate(value.trim_end().len());
    }
    Ok(values)
}
