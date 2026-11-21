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
    let (directory, files) = fresh_files(root, environment, request)?;
    let app_environment = BTreeMap::from([("APP_ENV", "test")]);
    let configuration_hash = configuration_identity(&directory, &files)?;
    if request["configuration_sha256"].as_str() != Some(configuration_hash.as_str()) {
        return Err("目标配置文件或所选 APP_ENV 与登记摘要不同；请重新核对配置并生成新请求".into());
    }
    let table =
        crate::rate_limit::load_app_table(root, Some(&directory), "test").map_err(|error| {
            format!(
                "目标运行配置无法读取：{}；{}",
                error.code(),
                error.message()
            )
        })?;
    let table = serde_json::to_value(table).map_err(|_| "目标运行配置无法安全解析")?;
    let physical = request_physical::binding(
        root,
        &request["target"],
        &table,
        environment,
        &binding.selected,
    )?;
    reset_configuration(request, &binding.selected, &table)?;
    addresses(&binding.selected, &table)?;
    let configuration = json!({"directory": manifest::path_text(&directory, "配置目录")?, "files": files,
        "environment_sha256": digest::plan_hash(&serde_json::to_value(app_environment)?)?});
    let password = request_physical::strict_text(&table["bootstrap"]["admin_password"])
        .map_err(|_| "所选配置文件缺少 bootstrap.admin_password；请显式配置初始化管理员密码")?;
    let secrets = json!({"bootstrap.admin_password": password});
    if fresh_files(root, environment, request)? != (directory, files) {
        return Err("目标配置在静态绑定核验期间变化".into());
    }
    // 生产者从未启动与端口关闭事实由主线显式观察器核验。
    Ok(
        json!({"execution": binding.execution, "physical": physical, "configuration": configuration,
        "configuration_sha256": configuration_hash, "seed_credentials_sha256": digest::plan_hash(&secrets)?,
        "request_sha256": digest::plan_hash(request)?, "review_sha256": request["review"]["sha256"], "selected": binding.selected}),
    )
}

pub(crate) fn fresh_files(
    root: &Path,
    environment: &BTreeMap<String, String>,
    request: &Value,
) -> Result<(PathBuf, BTreeMap<String, Value>)> {
    let directory = super::request::config_directory(root, request)?;
    configuration_files(root, environment, Some(&directory["path"]))
}

pub(crate) fn configuration_files(
    root: &Path,
    environment: &BTreeMap<String, String>,
    declared: Option<&Value>,
) -> Result<(PathBuf, BTreeMap<String, Value>)> {
    if environment.get("APP_ENV").map(String::as_str) != Some("test") {
        return Err("目标配置必须显式选择 APP_ENV=test；其他环境变量不参与配置".into());
    }
    let path = match declared {
        Some(value) => PathBuf::from(request_physical::strict_text(value)?),
        None => root.join("config"),
    };
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| "目标配置目录必须存在且不能经过链接；请检查 config_dir")?;
    let directory = canonical_path(&path)?;
    if !directory.starts_with(canonical_path(root)?) {
        return Err("目标配置必须位于明确执行后端内".into());
    }
    let files = configuration_file_digests(&directory, |path| {
        path.extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                if cfg!(windows) {
                    value.eq_ignore_ascii_case("toml")
                } else {
                    value == "toml"
                }
            })
    })?;
    if !files.contains_key("app.test.toml") {
        return Err("目标配置目录缺少 app.test.toml；请创建明确选择的测试环境文件，app.toml 为可选公共默认值".into());
    }
    Ok((directory, files))
}

pub(crate) fn configuration_file_digests(
    directory: &Path,
    is_toml: impl Fn(&Path) -> bool,
) -> Result<BTreeMap<String, Value>> {
    let mut files = BTreeMap::new();
    crate::recovery::runtime::dataset_authority::reads::directory(directory)?;
    for entry in fs::read_dir(directory).map_err(|_| "目标配置目录无法读取")? {
        let entry = entry.map_err(|_| "目标配置目录项无法读取")?;
        let path = entry.path();
        if !is_toml(&path) {
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
    Ok(files)
}

/// 应用身份只摘要实际加载的公共文件和已选 test 环境文件。
/// 全部 TOML 快照继续由 configuration_files 返回，供读取前后像守卫使用。
pub(crate) fn configuration_identity(
    directory: &Path,
    files: &BTreeMap<String, Value>,
) -> Result<String> {
    if !files.contains_key("app.test.toml") {
        return Err("应用配置身份缺少必需 app.test.toml".into());
    }
    let selected = files
        .iter()
        .filter(|(key, _)| matches!(key.as_str(), "app.toml" | "app.test.toml"))
        .map(|(key, value)| (key.clone(), value["sha256"].clone()))
        .collect::<BTreeMap<_, _>>();
    python_configuration_hash(&json!({
        "environment": {"APP_ENV": "test"},
        "config_directory": manifest::path_text(directory, "配置目录")?,
        "files": selected,
    }))
}

pub(crate) fn canonical_path(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path).map_err(|_| "配置路径无法解析")?;
    let text = path.to_str().ok_or("配置路径必须为 UTF-8")?;
    #[cfg(windows)]
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    Ok(PathBuf::from(text))
}

fn reset_configuration(request: &Value, selected: &Value, table: &Value) -> Result<()> {
    let declared = &request["reset"];
    for key in EXCLUSIVE {
        let field = format!("legacy_{key}");
        if table["reset"][field.as_str()].as_bool() != Some(true) {
            return Err(format!("所选配置文件的 reset.{field} 必须为 true，与请求保持一致").into());
        }
    }
    for (field, expected) in [
        ("credential_version", &declared["credential_version"]),
        ("redis_outside_sentinel_key", &declared["sentinel_key"]),
    ] {
        if table["reset"][field].as_str() != expected.as_str() {
            return Err("配置文件中的 reset 版本或 sentinel 与请求不同".into());
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
    let port = request["storage"]["redis"]["port"]
        .as_u64()
        .and_then(|number| u16::try_from(number).ok())
        .filter(|port| *port > 0)
        .ok_or("Redis 端口无效")?;
    if table["redis"]["host"].as_str() != Some("127.0.0.1")
        || table["redis"]["port"].as_u64() != Some(u64::from(port))
        || table["redis"]["database"].as_u64() != Some(0)
        || table["redis"]["tls"].as_bool() != Some(false)
        || selected["redis"]["url"].as_str() != Some(format!("redis://127.0.0.1:{port}/0").as_str())
    {
        return Err("配置文件中的 Redis 地址、数据库或 TLS 与本侧计划不同".into());
    }
    Ok(())
}

fn addresses(selected: &Value, table: &Value) -> Result<()> {
    let api = text(&selected["api_url"], "审阅 API 地址")?;
    let root = request_physical::endpoint(&json!(api))?;
    let (host, port) = root
        .split_once("://")
        .ok_or("API 地址无效")?
        .1
        .rsplit_once(':')
        .ok_or("API 端口缺失")?;
    let configured_host = table["app"]["host"]
        .as_str()
        .ok_or("配置文件缺少 app.host")?;
    let configured_port = table["app"]["port"]
        .as_u64()
        .and_then(|number| u16::try_from(number).ok())
        .filter(|port| *port > 0)
        .ok_or("配置文件缺少有效 app.port")?;
    let allowed = if host == "127.0.0.1" {
        ["127.0.0.1", "0.0.0.0"]
    } else {
        ["::1", "::"]
    };
    if !allowed.contains(&configured_host) || port.parse::<u16>().ok() != Some(configured_port) {
        return Err("API 地址未绑定实际文件配置".into());
    }
    let worker_host = table["jobs"]["health_host"]
        .as_str()
        .ok_or("配置文件缺少 jobs.health_host")?;
    let worker_port = table["jobs"]["health_port"]
        .as_u64()
        .and_then(|number| u16::try_from(number).ok())
        .filter(|port| *port > 0)
        .ok_or("配置文件缺少有效 jobs.health_port")?;
    if worker_host != "127.0.0.1"
        || table["jobs"]["mode"].as_str() != Some("external")
        || selected["worker_ready_url"].as_str()
            != Some(format!("http://{worker_host}:{worker_port}/readyz").as_str())
    {
        return Err("Worker 必须通过文件配置显式匹配本侧地址与 external 模式".into());
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
