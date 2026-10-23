use super::super::{exact_object, file_binding, read_json, text_field};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct Binding {
    pub receipt: Value,
    pub table: toml::Table,
    pub scope: String,
}

pub(crate) fn verify(root: &Path, directory: &Path) -> Result<Binding> {
    let receipt = read_json(&directory.join("runtime.json"), "全栈运行收据", 64 * 1024)?;
    exact_object(
        &receipt,
        &[
            "format_version",
            "backend_root",
            "scope_id",
            "api_config_dir",
            "worker_config_dir",
            "api_configuration_sha256",
            "worker_configuration_sha256",
            "configuration_sha256",
            "worker_ready_url",
            "artifacts",
        ],
        "全栈运行收据",
    )?;
    if receipt["format_version"] != 1 || receipt["backend_root"] != root.to_string_lossy().as_ref()
    {
        return Err("全栈运行收据版本或后端目录不匹配".into());
    }
    let api = PathBuf::from(text_field(&receipt, "api_config_dir", "运行收据")?);
    let worker = PathBuf::from(text_field(&receipt, "worker_config_dir", "运行收据")?);
    if api.file_name().and_then(|v| v.to_str()) != Some("config")
        || worker.file_name().and_then(|v| v.to_str()) != Some("worker-config")
        || api.parent() != worker.parent()
    {
        return Err("运行收据必须绑定同一受管配置根内的 config 与 worker-config".into());
    }
    let mut tables = Vec::new();
    let mut digests = Vec::new();
    for path in [&api, &worker] {
        validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
        let table = crate::rate_limit::load_app_table(root, Some(path), "test")
            .map_err(|error| error.message().to_owned())?;
        let digest = configuration_digest(path)?;
        tables.push(table);
        digests.push(digest);
    }
    if receipt["api_configuration_sha256"] != digests[0]
        || receipt["worker_configuration_sha256"] != digests[1]
        || receipt["configuration_sha256"] != hash(&json!(digests))?
    {
        return Err("全栈运行收据与绑定文件配置摘要不匹配".into());
    }
    let (api_table, scope) = validate_tables(&receipt, tables)?;
    verify_binaries(directory, &receipt)?;
    super::source::verify(root, directory)?;
    Ok(Binding {
        receipt,
        table: api_table,
        scope,
    })
}

fn validate_tables(receipt: &Value, mut tables: Vec<toml::Table>) -> Result<(toml::Table, String)> {
    let mut api_table = tables.remove(0);
    let mut worker_table = tables.remove(0);
    let api_id = api_table
        .remove("snowflake_worker_id")
        .and_then(|v| v.as_integer());
    let worker_id = worker_table
        .remove("snowflake_worker_id")
        .and_then(|v| v.as_integer());
    if api_id.is_none_or(|v| !(0..=1023).contains(&v))
        || worker_id.is_none_or(|v| !(0..=1023).contains(&v))
        || api_id == worker_id
        || api_table != worker_table
    {
        return Err("API 与 Worker 的配置不匹配或实例编号重复".into());
    }
    let scope = api_table
        .get("scope_id")
        .and_then(toml::Value::as_str)
        .ok_or("配置缺少 scope_id")?
        .to_owned();
    if !regex::Regex::new(r"^[a-z0-9][a-z0-9-]{2,63}$")?.is_match(&scope)
        || receipt["scope_id"] != scope
    {
        return Err("全栈运行 scope 与配置不匹配".into());
    }
    let jobs = api_table
        .get("jobs")
        .and_then(toml::Value::as_table)
        .ok_or("配置缺少 jobs")?;
    let port = jobs
        .get("health_port")
        .and_then(toml::Value::as_integer)
        .filter(|v| (1..=65535).contains(v))
        .ok_or("jobs.health_port 无效")?;
    if jobs.get("mode").and_then(toml::Value::as_str) != Some("external")
        || jobs.get("health_host").and_then(toml::Value::as_str) != Some("127.0.0.1")
        || receipt["worker_ready_url"] != format!("http://127.0.0.1:{port}/readyz")
    {
        return Err("全栈 Worker 就绪端点与绑定配置不匹配".into());
    }
    Ok((api_table, scope))
}

fn verify_binaries(directory: &Path, receipt: &Value) -> Result<()> {
    let binaries = read_json(
        &directory.join("binaries.json"),
        "全栈二进制清单",
        64 * 1024,
    )?;
    exact_object(
        &binaries,
        &[
            "ryframe",
            "ryframe-worker",
            "ryframe-reset",
            "ryframe-migrate",
        ],
        "全栈二进制清单",
    )?;
    exact_object(&receipt["artifacts"], &["api", "worker"], "运行产物")?;
    for name in [
        "ryframe",
        "ryframe-worker",
        "ryframe-reset",
        "ryframe-migrate",
    ] {
        let path = PathBuf::from(text_field(&binaries, name, "二进制清单")?);
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    }
    for (role, name) in [("api", "ryframe"), ("worker", "ryframe-worker")] {
        let path = PathBuf::from(text_field(&binaries, name, "二进制清单")?);
        validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
        let binding = file_binding(&path, "运行产物")?;
        if receipt["artifacts"][role] != json!({"path":binding["path"], "sha256":binding["sha256"]})
        {
            return Err("全栈运行二进制与登记摘要不匹配".into());
        }
    }
    Ok(())
}

pub(super) fn configuration_digest(path: &Path) -> Result<String> {
    let mut files = serde_json::Map::new();
    for name in ["app.toml", "app.test.toml"] {
        let file = path.join(name);
        if name == "app.toml" && !file.exists() {
            continue;
        }
        validate_absolute_path(&file, LocalTestPathKind::ExistingFile)?;
        files.insert(
            name.into(),
            file_binding(&file, "绑定配置")?["sha256"].clone(),
        );
    }
    hash(&json!({"environment":{"APP_ENV":"test"}, "config_directory":path, "files":files}))
}

// 现有运行收据使用 Python 默认 JSON 空格和 ASCII 转义；保留该字节契约。
pub(super) fn hash(value: &Value) -> Result<String> {
    Ok(crate::devex::runtime_native::artifacts::sha(
        encode(value)?.as_bytes(),
    ))
}
fn encode(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Object(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| Ok(format!("{}: {}", encode(&json!(key))?, encode(value)?)))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(encode)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Value::String(_) => {
            let encoded = serde_json::to_string(value)?;
            let mut output = String::new();
            for character in encoded.chars() {
                if character.is_ascii() {
                    output.push(character);
                } else {
                    for unit in character.encode_utf16(&mut [0; 2]) {
                        use std::fmt::Write;
                        write!(output, "\\u{unit:04x}")?;
                    }
                }
            }
            output
        }
        value => serde_json::to_string(value)?,
    })
}
