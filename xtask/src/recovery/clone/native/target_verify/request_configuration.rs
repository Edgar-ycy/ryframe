use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::super::{digest, json::text, lifecycle::manifest};
use super::{
    request::{Binding, EXCLUSIVE},
    request_physical,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};

pub(crate) fn validate(
    _backend: &Path,
    request: &Value,
    binding: &Binding,
    environment: &BTreeMap<String, String>,
) -> Result<Value> {
    let root = &binding.execution_backend;
    let (directory, files) = configuration_files(root, environment)?;
    let app_environment = environment
        .iter()
        .filter(|(key, _)| key.starts_with("APP_"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    let configuration_hash = python_configuration_hash(
        &json!({"environment": app_environment, "config_directory": manifest::path_text(&directory, "配置目录")?, "files": files.iter().map(|(key, value)| (key.clone(), value["sha256"].clone())).collect::<BTreeMap<_, _>>()}),
    )?;
    if request["configuration_sha256"].as_str() != Some(configuration_hash.as_str()) {
        return Err("目标 APP 配置或秘密发生变化".into());
    }
    let table = crate::rate_limit::load_app_table(root, environment)
        .map_err(|_| "目标运行配置无法安全读取")?;
    let table = serde_json::to_value(table).map_err(|_| "目标运行配置无法安全解析")?;
    let physical = request_physical::binding(
        root,
        &request["target"],
        &table,
        environment,
        &binding.selected,
    )?;
    reset_configuration(request, &binding.selected, &table, environment)?;
    addresses(&binding.selected, &table, environment)?;
    let mut names = app_environment;
    for field in ["access_key_env", "secret_key_env"] {
        let name = text(&request["target"]["s3"][field], "对象凭据变量")?;
        names.insert(
            name.into(),
            environment
                .get(name)
                .filter(|value| !value.is_empty())
                .ok_or("库存环境缺少已登记对象凭据")?
                .clone(),
        );
    }
    let configuration = json!({"directory": manifest::path_text(&directory, "配置目录")?, "files": files, "environment_sha256": digest::plan_hash(&serde_json::to_value(names)?)?});
    let key = "RYFRAME_RESET_ADMIN_PASSWORD";
    let secrets = BTreeMap::from([(
        key,
        environment
            .get(key)
            .filter(|value| !value.is_empty())
            .ok_or("必须显式提供初始化管理员密码")?,
    )]);
    if configuration_files(root, environment)? != (directory, files) {
        return Err("目标配置在静态绑定核验期间变化".into());
    }
    // 生产者从未启动与端口关闭事实由主线显式观察器核验。
    Ok(
        json!({"execution": binding.execution, "physical": physical, "configuration": configuration, "configuration_sha256": configuration_hash, "seed_credentials_sha256": digest::plan_hash(&serde_json::to_value(secrets)?)?, "request_sha256": digest::plan_hash(request)?, "review_sha256": request["review"]["sha256"], "selected": binding.selected}),
    )
}

pub(crate) fn configuration_files(
    root: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<(PathBuf, BTreeMap<String, Value>)> {
    let path = environment
        .get("APP_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("config"));
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| "库存配置目录必须无链接")?;
    let directory = canonical_path(&path)?;
    if !directory.starts_with(canonical_path(root)?) {
        return Err("库存配置必须位于明确执行后端内".into());
    }
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(&directory).map_err(|_| "库存配置目录无法读取")? {
        let entry = entry.map_err(|_| "库存配置目录项无法读取")?;
        let path = entry.path();
        let extension = path.extension().and_then(|value| value.to_str());
        if extension.is_none_or(|value| {
            if cfg!(windows) {
                !value.eq_ignore_ascii_case("toml")
            } else {
                value != "toml"
            }
        }) {
            continue;
        }
        let actual = digest::file(&path)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "配置文件名必须为 UTF-8")?;
        files.insert(
            name,
            json!({"bytes": actual.bytes, "sha256": actual.sha256}),
        );
    }
    if files.is_empty() {
        return Err("配置目录缺少 TOML 文件".into());
    }
    Ok((directory, files))
}

fn canonical_path(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path).map_err(|_| "配置路径无法解析")?;
    let text = path.to_str().ok_or("配置路径必须为 UTF-8")?;
    #[cfg(windows)]
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    Ok(PathBuf::from(text))
}

fn reset_configuration(
    request: &Value,
    selected: &Value,
    table: &Value,
    environment: &BTreeMap<String, String>,
) -> Result<()> {
    let declared = &request["reset"];
    for key in EXCLUSIVE {
        let field = format!("legacy_{key}");
        let variable = format!("APP_RESET_{}", field.to_uppercase());
        reject_file(environment, &variable)?;
        match environment.get(&variable) {
            Some(value) if value == "true" => {}
            None if table["reset"][&field].as_bool() == Some(true) => {}
            _ => return Err("实际 reset exclusive 配置与请求不同".into()),
        }
    }
    for (field, expected) in [
        ("credential_version", &declared["credential_version"]),
        ("redis_outside_sentinel_key", &declared["sentinel_key"]),
    ] {
        let variable = format!("APP_RESET_{}", field.to_uppercase());
        reject_file(environment, &variable)?;
        if environment
            .get(&variable)
            .map(String::as_str)
            .or_else(|| table["reset"][field].as_str())
            != expected.as_str()
        {
            return Err("实际 reset 版本或 sentinel 与请求不同".into());
        }
    }
    let scope = text(&request["target"]["scope_id"], "目标 scope")?;
    let namespace = format!("ryframe:{{{scope}}}:");
    if selected["redis"]["namespace"].as_str() != Some(namespace.as_str())
        || selected["redis"]["ownership_key"].as_str()
            != Some(format!("{namespace}.ryframe-owner").as_str())
    {
        return Err("Redis 计划 namespace 无效".into());
    }
    let redis_port = request["storage"]["redis"]["port"]
        .as_u64()
        .and_then(|number| u16::try_from(number).ok())
        .filter(|port| *port > 0)
        .ok_or("Redis 端口无效")?;
    for (key, value) in [
        ("APP_REDIS_HOST", "127.0.0.1".into()),
        ("APP_REDIS_PORT", redis_port.to_string()),
        ("APP_REDIS_DATABASE", "0".into()),
        ("APP_REDIS_TLS", "false".into()),
    ] {
        reject_file(environment, key)?;
        if environment.get(key) != Some(&value) {
            return Err("Redis 地址或 TLS 未显式匹配本侧配置".into());
        }
    }
    Ok(())
}

fn addresses(
    selected: &Value,
    table: &Value,
    environment: &BTreeMap<String, String>,
) -> Result<()> {
    for key in ["APP_APP_HOST", "APP_APP_PORT"] {
        reject_file(environment, key)?;
    }
    let api = text(&selected["api_url"], "审阅 API 地址")?;
    let root = request_physical::endpoint(&json!(api))?;
    let (host, port) = root
        .split_once("://")
        .ok_or("API 地址无效")?
        .1
        .rsplit_once(':')
        .ok_or("API 端口缺失")?;
    let configured_host = environment
        .get("APP_APP_HOST")
        .map(String::as_str)
        .or_else(|| table["app"]["host"].as_str())
        .ok_or("APP host 缺失")?;
    let configured_port = match environment.get("APP_APP_PORT") {
        Some(value) => value.parse::<u16>().map_err(|_| "APP port 无效")?,
        None => table["app"]["port"]
            .as_u64()
            .and_then(|number| u16::try_from(number).ok())
            .ok_or("APP port 缺失")?,
    };
    let allowed = if host == "127.0.0.1" {
        ["127.0.0.1", "0.0.0.0"]
    } else {
        ["::1", "::"]
    };
    if !allowed.contains(&configured_host) || port.parse::<u16>().ok() != Some(configured_port) {
        return Err("API 地址未绑定实际 APP 配置".into());
    }
    let worker_host = environment
        .get("APP_JOBS_HEALTH_HOST")
        .map(String::as_str)
        .ok_or("Worker health host 缺失")?;
    let worker_port = environment
        .get("APP_JOBS_HEALTH_PORT")
        .ok_or("Worker health port 缺失")?;
    if worker_host != "127.0.0.1"
        || worker_port.is_empty()
        || !worker_port.bytes().all(|value| value.is_ascii_digit())
        || worker_port
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .is_none()
        || environment.get("APP_JOBS_MODE").map(String::as_str) != Some("external")
        || selected["worker_ready_url"].as_str()
            != Some(format!("http://{worker_host}:{worker_port}/readyz").as_str())
    {
        return Err("Worker 必须显式匹配本侧地址与 external 模式".into());
    }
    Ok(())
}

fn reject_file(environment: &BTreeMap<String, String>, variable: &str) -> Result<()> {
    if environment.contains_key(&format!("{variable}_FILE")) {
        return Err("文件间接覆盖未绑定".into());
    }
    Ok(())
}

/// Python configuration_digest 使用带空格和 ASCII 转义的默认 JSON 编码。
pub(crate) fn python_configuration_hash(value: &Value) -> Result<String> {
    Ok(hex::encode(Sha256::digest(python_json(value)?.as_bytes())))
}

fn python_json(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => {
            let encoded = serde_json::to_string(value)?;
            let mut result = String::new();
            for character in encoded.chars() {
                if character.is_ascii() && character != '\u{7f}' {
                    result.push(character);
                } else {
                    for unit in character.encode_utf16(&mut [0; 2]) {
                        result.push_str(&format!("\\u{unit:04x}"));
                    }
                }
            }
            Ok(result)
        }
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let entries = keys
                .into_iter()
                .map(|key| {
                    Ok(format!(
                        "{}: {}",
                        python_json(&json!(key))?,
                        python_json(&values[key])?
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(format!("{{{}}}", entries.join(", ")))
        }
        Value::Array(values) => Ok(format!(
            "[{}]",
            values
                .iter()
                .map(python_json)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        )),
        _ => Ok(serde_json::to_string(value)?),
    }
}
