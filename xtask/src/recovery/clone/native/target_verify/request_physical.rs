use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use regex::Regex;
use serde_json::{Map, Value, json};

use super::super::super::{
    digest,
    json::{self as json_io, text},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const FIELDS: [&str; 9] = [
    "host",
    "port",
    "database",
    "username",
    "password",
    "tls_mode",
    "tls_ca",
    "tls_client_cert",
    "tls_client_key",
];

pub(crate) fn binding(
    backend: &Path,
    target: &Value,
    table: &Value,
    env: &BTreeMap<String, String>,
    selected: &Value,
) -> Result<Value> {
    if env.get("APP_ENV").map(String::as_str) != Some("test")
        || strict_text(&table["scope_id"])? != strict_text(&target["scope_id"])?
    {
        return Err("目标环境与计划 scope 必须明确一致".into());
    }
    let databases = databases(backend, target, table, selected)?;
    let s3 = storage(target, table)?;
    let mut result = json!({"format_version": 1, "scope_id": target["scope_id"], "databases": databases, "s3": s3});
    let hash = digest::plan_hash(&result)?;
    result
        .as_object_mut()
        .ok_or("物理绑定无效")?
        .insert("sha256".into(), json!(hash));
    Ok(result)
}

fn databases(
    backend: &Path,
    target: &Value,
    table: &Value,
    selected: &Value,
) -> Result<Vec<Value>> {
    for field in ["replicas", "sources"] {
        if let Some(values) = table["database"].get(field)
            && !json_io::array(values, "目标连接数组")?.is_empty()
        {
            return Err("目标不允许未登记的副本或命名数据源".into());
        }
    }
    let primary = json_io::object(&table["database"]["primary"], "运行连接配置")?;
    let targets = runtime_targets(table)?;
    let declared = json_io::array(&target["databases"], "目标数据库")?;
    let keys = declared
        .iter()
        .map(|db| text(&db["key"], "数据库 key"))
        .collect::<Result<BTreeSet<_>>>()?;
    if keys != targets.keys().map(String::as_str).collect() || declared.len() != targets.len() {
        return Err("配置与计划必须精确覆盖相同数据库目标".into());
    }
    let mut identities = BTreeSet::new();
    let mut server_identities = BTreeSet::new();
    let mut result = Vec::new();
    let mut declared = declared.iter().collect::<Vec<_>>();
    declared.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    for db in declared {
        let key = text(&db["key"], "数据库 key")?;
        let actual = database_connection(backend, db, &targets[key], primary)?;
        let planned = selected["databases"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["key"].as_str() == Some(key)))
            .ok_or("审阅数据库缺失")?;
        if planned["host"] != actual["host"] || planned["port"] != actual["port"] {
            return Err("MySQL host/port 不是审阅地址".into());
        }
        if !identities.insert((
            strict_text(&actual["host"])?.to_owned(),
            port(&actual["port"])?,
            strict_text(&actual["database"])?.to_ascii_lowercase(),
        )) || !server_identities.insert((
            strict_text(&db["server_uuid"])?.to_owned(),
            strict_text(&actual["database"])?.to_ascii_lowercase(),
        )) {
            return Err("不同逻辑目标引用同一物理数据库".into());
        }
        result.push(json!({"key": key, "kind": db["kind"], "mode": db["mode"], "server_uuid": db["server_uuid"], "host": actual["host"], "port": actual["port"], "database": actual["database"], "tls_mode": actual["tls_mode"]}));
    }
    Ok(result)
}

pub(crate) fn defaults_connection(backend: &Path, db: &Value) -> Result<Value> {
    let path = Path::new(text(&db["defaults_file"], "MySQL 凭据路径")?);
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
        .map_err(|_| "MySQL 凭据路径无效")?;
    if digest::file(path)?.sha256 != text(&db["defaults_sha256"], "MySQL 凭据摘要")? {
        return Err("MySQL 凭据摘要变化".into());
    }
    let raw = fs::read_to_string(path).map_err(|_| "MySQL 凭据无法按 UTF-8 读取")?;
    let fields = ["host", "port", "user", "password", "ssl-mode"];
    let mut section = false;
    let mut entries = BTreeMap::new();
    let quoted_value = Regex::new(r"^[A-Za-z0-9._-]+$")?;
    for line in raw.lines() {
        if line == "[client]" && !section {
            section = true;
            continue;
        }
        let (key, value) = line.split_once('=').ok_or("MySQL 凭据选项格式不明确")?;
        if !section
            || !fields.contains(&key)
            || entries.contains_key(key)
            || value.is_empty()
            || value.trim() != value
        {
            return Err("MySQL 凭据选项格式不明确".into());
        }
        let value = if value.starts_with('"') || value.ends_with('"') {
            let inner = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .ok_or("MySQL 凭据引号无效")?;
            if !quoted_value.is_match(inner) {
                return Err("MySQL 凭据包含未支持的转义或注释".into());
            }
            inner
        } else {
            if value.contains(['"', '\'', '\\', '#', ';', '\r', '\n']) {
                return Err("MySQL 凭据包含未支持的转义或注释".into());
            }
            value
        };
        entries.insert(key, value);
    }
    if entries.len() != fields.len() || fields.iter().any(|key| !entries.contains_key(key)) {
        return Err("MySQL 凭据必须完整且仅含五个选项".into());
    }
    if !["127.0.0.1", "::1"].contains(&entries["host"])
        || !["REQUIRED", "DISABLED"].contains(&entries["ssl-mode"])
        || !Regex::new(r"^[0-9]+$")?.is_match(entries["port"])
    {
        return Err("MySQL 凭据主机、端口或 TLS 无效".into());
    }
    let number: u16 = entries["port"].parse().map_err(|_| "MySQL 凭据端口无效")?;
    if number == 0 {
        return Err("MySQL 凭据端口无效".into());
    }
    Ok(
        json!({"host": entries["host"], "port": number, "database": strict_text(&db["database"])?, "username": entries["user"], "password": entries["password"], "tls_mode": entries["ssl-mode"].to_lowercase()}),
    )
}

fn storage(target: &Value, table: &Value) -> Result<Value> {
    let actual = json_io::object(&table["object_storage"], "对象存储配置")?;
    if !matches!(
        actual.get("backend").and_then(Value::as_str),
        Some("rustfs" | "minio" | "s3")
    ) {
        return Err("对象存储后端无效".into());
    }
    let endpoint = endpoint(actual.get("endpoint").ok_or("对象端点缺失")?)?;
    if endpoint != self::endpoint(&target["s3"]["endpoint"])?
        || actual.get("region") != target["s3"].get("region")
        || actual.get("use_ssl").and_then(Value::as_bool) != Some(endpoint.starts_with("https://"))
    {
        return Err("对象端点、区域或协议与计划不同".into());
    }
    for field in ["access_key", "secret_key"] {
        let legacy = format!("{field}_env");
        if actual.contains_key(&legacy) || target["s3"].get(&legacy).is_some() {
            return Err("对象认证只接受明确的 access_key 与 secret_key 字段".into());
        }
        if strict_text(actual.get(field).ok_or("对象认证缺失")?)?
            != strict_text(&target["s3"][field])?
        {
            return Err("对象认证与计划不同".into());
        }
    }
    Ok(
        json!({"backend": actual["backend"], "endpoint": endpoint, "region": strict_text(&actual["region"])?}),
    )
}

pub(crate) fn strict_text(value: &Value) -> Result<&str> {
    let value = text(value, "明确配置文本")?;
    if value.is_empty() || value.trim() != value {
        return Err("配置缺少明确文本值".into());
    }
    Ok(value)
}

fn port(value: &Value) -> Result<u16> {
    value
        .as_u64()
        .and_then(|number| u16::try_from(number).ok())
        .filter(|number| *number > 0)
        .ok_or_else(|| "端口必须明确且有效".into())
}

pub(crate) fn endpoint(value: &Value) -> Result<String> {
    let value = strict_text(value)?;
    let pattern = Regex::new(r"^(http|https)://(127\.0\.0\.1|\[::1\]):([0-9]+)/?$")?;
    let capture = pattern
        .captures(value)
        .ok_or("对象端点必须为显式端口的本机根地址")?;
    if capture[3]
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .is_none()
    {
        return Err("对象端点端口无效".into());
    }
    Ok(value.trim_end_matches('/').to_owned())
}

fn runtime_targets(table: &Value) -> Result<BTreeMap<String, Value>> {
    let mut targets = BTreeMap::new();
    let values = table["tenant_data"]
        .get("targets")
        .cloned()
        .unwrap_or_else(|| json!([]));
    for value in json_io::array(&values, "目标连接数组")? {
        let key = strict_text(&value["key"])?;
        if targets.insert(key.to_owned(), value.clone()).is_some() {
            return Err("目标租户连接 key 重复".into());
        }
    }
    targets
        .entry("shared-control".into())
        .or_insert_with(|| json!({"key": "shared-control", "kind": "control", "mode": "shared"}));
    let control = &targets["shared-control"];
    if control["kind"].as_str() != Some("control")
        || control["mode"].as_str() != Some("shared")
        || control.get("password_env").is_some()
        || FIELDS
            .iter()
            .copied()
            .chain(["max_connections"])
            .any(|key| !control[key].is_null())
    {
        return Err("控制目标必须复用明确的 primary 连接".into());
    }
    Ok(targets)
}

fn database_connection(
    backend: &Path,
    db: &Value,
    configuration: &Value,
    primary: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let key = text(&db["key"], "数据库 key")?;
    let control = key == "shared-control";
    if db["kind"].as_str() != Some(if control { "combined" } else { "tenant" })
        || !matches!(db["mode"].as_str(), Some("shared" | "dedicated"))
        || configuration["kind"].as_str() != Some(if control { "control" } else { "mysql" })
        || configuration["mode"] != db["mode"]
    {
        return Err("数据库逻辑类型或共享模式不同".into());
    }
    let connection = if control {
        primary.clone()
    } else {
        json_io::object(configuration, "租户连接")?.clone()
    };
    if connection.contains_key("password_env") {
        return Err("数据库认证只接受明确的 password 明文字段".into());
    }
    let expected = defaults_connection(backend, db)?;
    for field in ["tls_ca", "tls_client_cert", "tls_client_key"] {
        if connection
            .get(field)
            .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        {
            return Err("目标 TLS 外部文件未绑定".into());
        }
    }
    let mut actual = Map::new();
    for field in expected.as_object().ok_or("MySQL 连接无效")?.keys() {
        let value = connection.get(field).ok_or("目标数据库连接字段缺失")?;
        actual.insert(
            field.clone(),
            if field == "port" {
                json!(port(value)?)
            } else {
                json!(strict_text(value)?)
            },
        );
    }
    if Value::Object(actual.clone()) != expected {
        return Err("目标数据库地址、认证或 TLS 与凭据不同".into());
    }
    Ok(actual)
}
