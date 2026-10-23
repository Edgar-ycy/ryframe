use super::{Checked, files};
use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlRequest {
    pub backend: PathBuf,
}

// 不派生 Debug；快照只经私有 stdin 传递，不能写入公开收据。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    pub format_version: u32,
    pub backend: PathBuf,
    pub output: PathBuf,
    pub config_dir: PathBuf,
    pub github_actions: String,
    pub run_id: String,
    pub attempt: String,
    pub app_env: String,
    pub container: String,
    pub settings: Value,
    pub common_text: String,
    pub environment_text: String,
}

pub(super) struct Plan<'a> {
    pub input: &'a Input,
    pub binding: &'a Value,
    pub schemas: Vec<String>,
    pub scope: String,
    pub api_worker_id: u64,
}

impl Input {
    pub fn validate(&self) -> Checked<Plan<'_>> {
        if self.format_version != 1
            || self.github_actions != "true"
            || self.app_env != "test"
            || !digits(&self.run_id, 20)
            || !digits(&self.attempt, 6)
            || !container(&self.container)
        {
            return Err("ci_context");
        }
        for path in [&self.backend, &self.config_dir] {
            validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)
                .map_err(|_| "config_path")?;
        }
        validate_absolute_path(&self.output, LocalTestPathKind::StateDirectory)
            .map_err(|_| "config_path")?;
        for name in ["config", "worker-config", "database-targets.json"] {
            if self.output.join(name).symlink_metadata().is_ok() {
                return Err("replay_refused");
            }
        }
        // 语法错误不输出原始 TOML 行；准备使用规划时捕获的完整快照。
        let _: toml::Table = toml::from_str(&self.common_text).map_err(|_| "config_snapshot")?;
        let _: toml::Table =
            toml::from_str(&self.environment_text).map_err(|_| "config_snapshot")?;
        let settings = self.settings.as_object().ok_or("binding_invalid")?;
        if let Some(tenant) = settings.get("tenant_data")
            && tenant.as_object().is_none_or(|v| v.contains_key("targets"))
        {
            return Err("binding_invalid");
        }
        let binding = self
            .settings
            .pointer("/database/primary")
            .ok_or("binding_invalid")?;
        validate_binding(binding)?;
        let prefix = format!("ryframe_e2e_{}_{}", self.run_id, self.attempt);
        let scope = format!("ci-{}-{}", self.run_id, self.attempt);
        if binding["database"] != prefix || self.settings["scope_id"] != scope {
            return Err("binding_invalid");
        }
        let api_worker_id = self.settings["snowflake_worker_id"]
            .as_u64()
            .filter(|v| *v <= 1023 && *v != 2)
            .ok_or("instance_id")?;
        Ok(Plan {
            input: self,
            binding,
            scope,
            api_worker_id,
            schemas: ["", "_shared", "_dedicated_a", "_dedicated_b"]
                .iter()
                .map(|suffix| format!("{prefix}{suffix}"))
                .collect(),
        })
    }
}

impl Plan<'_> {
    pub fn public(&self) -> Value {
        json!({"scope_id":self.scope,"schemas":self.schemas,
            "config_dir":self.input.output.join("config")})
    }
}

fn digits(value: &str, max: usize) -> bool {
    !value.starts_with('0')
        && !value.is_empty()
        && value.len() <= max
        && value.bytes().all(|v| v.is_ascii_digit())
}

pub(super) fn container(value: &str) -> bool {
    (12..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn validate_binding(value: &Value) -> Checked<()> {
    let object = value.as_object().ok_or("binding_invalid")?;
    let name = value["database"].as_str().ok_or("binding_invalid")?;
    if name.is_empty()
        || name.len() > 64
        || !name.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'_')
        || !matches!(value["host"].as_str(), Some("localhost" | "127.0.0.1"))
        || value["port"]
            .as_u64()
            .is_none_or(|v| !(1..=65535).contains(&v))
        || object.keys().any(|v| v.ends_with("_env"))
    {
        return Err("binding_invalid");
    }
    let tls = value["tls_mode"].as_str().ok_or("binding_invalid")?;
    if !matches!(
        tls,
        "disabled" | "required" | "verify_ca" | "verify_identity"
    ) {
        return Err("binding_invalid");
    }
    for key in ["username", "password"] {
        let text = value[key].as_str().ok_or("binding_invalid")?;
        if text.is_empty()
            || text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{8}'))
            || text.contains('\0')
        {
            return Err("binding_invalid");
        }
    }
    for key in ["tls_ca", "tls_client_cert", "tls_client_key"] {
        if value
            .get(key)
            .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            return Err("binding_invalid");
        }
        if value[key]
            .as_str()
            .is_some_and(|v| v.contains(['\0', '\r', '\n']))
        {
            return Err("binding_invalid");
        }
    }
    let present = |key: &str| value[key].as_str().is_some_and(|v| !v.is_empty());
    if (matches!(tls, "verify_ca" | "verify_identity") && !present("tls_ca"))
        || present("tls_client_cert") != present("tls_client_key")
    {
        return Err("binding_invalid");
    }
    Ok(())
}

pub(super) fn control(backend: &Path) -> Checked<Value> {
    let path = backend.join(".local-tests/full-stack-control.json");
    let bytes = files::read(&path, 16 * 1024).map_err(|_| "control_invalid")?;
    let value = crate::recovery::clone::native::json::parse_fragment(
        std::str::from_utf8(&bytes).map_err(|_| "control_invalid")?,
        "全栈控制文件",
    )
    .map_err(|_| "control_invalid")?;
    let object = value.as_object().ok_or("control_invalid")?;
    let local = object.contains_key("mysql_client");
    let keys = if local {
        vec!["format_version", "runtime_dir", "mysql_client"]
    } else {
        vec![
            "format_version",
            "runtime_dir",
            "mysql_container_id",
            "redis_container_id",
            "s3_container_id",
        ]
    };
    if object.len() != keys.len()
        || keys.iter().any(|k| !object.contains_key(*k))
        || value["format_version"].as_u64() != Some(1)
    {
        return Err("control_invalid");
    }
    let runtime = Path::new(value["runtime_dir"].as_str().ok_or("control_invalid")?);
    validate_absolute_path(runtime, LocalTestPathKind::StateDirectory)
        .map_err(|_| "control_invalid")?;
    if local {
        let client = Path::new(value["mysql_client"].as_str().ok_or("control_invalid")?);
        validate_absolute_path(client, LocalTestPathKind::ExistingFile)
            .map_err(|_| "control_invalid")?;
    } else if [
        "mysql_container_id",
        "redis_container_id",
        "s3_container_id",
    ]
    .iter()
    .any(|k| !value[*k].as_str().is_some_and(container))
    {
        return Err("control_invalid");
    }
    Ok(value)
}
