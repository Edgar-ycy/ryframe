use ryframe_kernel::{AppError, AppResult};

use crate::Environment;

pub(super) const MIN_PRODUCTION_JWT_SECRET_BYTES: usize = 32;

const PRODUCTION_FILE_SECRET_KEYS: &[&str] = &[
    "password",
    "jwt_secret",
    "access_key",
    "secret_key",
    "metrics_bearer_token",
];

pub(super) fn reject_production_file_secrets(table: &toml::Table) -> AppResult<()> {
    inspect_production_file_secrets(&toml::Value::Table(table.clone()), "")
}

fn inspect_production_file_secrets(value: &toml::Value, path: &str) -> AppResult<()> {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if PRODUCTION_FILE_SECRET_KEYS.contains(&key.as_str())
                    && let toml::Value::String(secret) = child
                    && !secret.is_empty()
                    && !(key == "jwt_secret" && secret == "change-me-in-production")
                {
                    return Err(AppError::Config(format!(
                        "生产配置文件不得包含敏感值 {child_path}；请使用对应 APP_* 环境变量或外部 secret manager 注入"
                    )));
                }
                inspect_production_file_secrets(child, &child_path)?;
            }
        }
        toml::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                inspect_production_file_secrets(child, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }

    Ok(())
}

pub(super) fn reject_removed_secret_encoding(table: &toml::Table) -> AppResult<()> {
    inspect_removed_secret_encoding(&toml::Value::Table(table.clone()), "")
}

fn inspect_removed_secret_encoding(value: &toml::Value, path: &str) -> AppResult<()> {
    match value {
        toml::Value::String(value) if value.starts_with("ENC[") => Err(AppError::Config(format!(
            "配置 {path} 使用了已删除的 ENC[...] 格式；请通过 APP_* 环境变量或外部 secret manager 注入原始值"
        ))),
        toml::Value::Table(table) => {
            for (key, child) in table {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                inspect_removed_secret_encoding(child, &child_path)?;
            }
            Ok(())
        }
        toml::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                inspect_removed_secret_encoding(child, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(super) fn resolve_snowflake_worker_id(environment: Environment) -> AppResult<i64> {
    match std::env::var("SNOWFLAKE_WORKER_ID") {
        Ok(value) => {
            let worker_id = value.trim().parse::<i64>().map_err(|_| {
                AppError::Config(format!(
                    "SNOWFLAKE_WORKER_ID 必须是 0~{} 的整数，当前值: {value}",
                    ryframe_kernel::MAX_SNOWFLAKE_WORKER_ID
                ))
            })?;
            if ryframe_kernel::SnowflakeWorkerId::new(worker_id).is_none() {
                return Err(AppError::Config(format!(
                    "SNOWFLAKE_WORKER_ID 必须在 0~{} 之间，当前值: {worker_id}",
                    ryframe_kernel::MAX_SNOWFLAKE_WORKER_ID
                )));
            }
            Ok(worker_id)
        }
        Err(std::env::VarError::NotPresent) if environment.is_production() => {
            Err(AppError::Config(
                "生产环境必须显式设置 SNOWFLAKE_WORKER_ID，且每个应用实例必须使用不同值".into(),
            ))
        }
        Err(std::env::VarError::NotPresent) => Ok(1),
        Err(std::env::VarError::NotUnicode(_)) => Err(AppError::Config(
            "SNOWFLAKE_WORKER_ID 必须是有效的 UTF-8 整数".into(),
        )),
    }
}
