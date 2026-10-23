//! 当前 fresh 初始化调用方的文件配置检查，禁止从环境覆盖连接或凭据。

use std::path::Path;

use serde_json::{Value, json};

use super::super::json as json_io;
use super::{binding, evidence_model::check, input::request};
use crate::Result;

pub(super) fn validate(backend: &Path, args: &Value, reset: bool) -> Result<Value> {
    json_io::exact(
        args,
        &["request", "selected", "app_env"],
        "目标文件配置请求",
    )?;
    let request = &args["request"];
    let selected = &args["selected"];
    let environment = json_io::text(&args["app_env"], "APP_ENV")?;
    let directory = binding::config_directory(backend, request)?;
    let path = Path::new(json_io::text(&directory["path"], "config_dir")?);
    let table = crate::rate_limit::load_app_table(backend, Some(path), environment)
        .map_err(|_| "目标文件配置无法读取；请检查 APP_ENV 与 config_dir 下的 TOML 文件")?;
    let table = serde_json::to_value(table)?;
    if reset {
        request::reset(request, selected)?;
        for key in request::EXCLUSIVE {
            let field = format!("legacy_{key}");
            if table["reset"][&field].as_bool() != Some(true) {
                return Err(
                    format!("所选配置文件的 reset.{field} 必须为 true，与请求保持一致").into(),
                );
            }
        }
        for (field, expected) in [
            (
                "credential_version",
                &request["reset"]["credential_version"],
            ),
            (
                "redis_outside_sentinel_key",
                &request["reset"]["sentinel_key"],
            ),
        ] {
            if table["reset"][field] != *expected {
                return Err(format!("所选配置文件的 reset.{field} 与显式请求不同").into());
            }
        }
    }
    let scope = json_io::text(&request["target"]["scope_id"], "目标 scope")?;
    let namespace = format!("ryframe:{{{scope}}}:");
    check(
        selected["redis"]["namespace"] == namespace
            && selected["redis"]["ownership_key"] == format!("{namespace}.ryframe-owner"),
        "Redis 计划 namespace 无效",
    )?;
    for (field, expected) in [
        ("host", json!("127.0.0.1")),
        ("port", request["storage"]["redis"]["port"].clone()),
        ("database", json!(0)),
        ("tls", json!(false)),
    ] {
        if table["redis"][field] != expected {
            return Err(format!("所选配置文件的 redis.{field} 未显式匹配本侧配置").into());
        }
    }
    Ok(json!({"status":"file_configuration_verified"}))
}

pub(super) fn diagnostic(reason: &str) -> Option<&'static str> {
    const MESSAGES: &[&str] = &[
        "目标文件配置无法读取；请检查 APP_ENV 与 config_dir 下的 TOML 文件",
        "所选配置文件的 reset.legacy_mysql_exclusive 必须为 true，与请求保持一致",
        "所选配置文件的 reset.legacy_redis_exclusive 必须为 true，与请求保持一致",
        "所选配置文件的 reset.legacy_object_storage_exclusive 必须为 true，与请求保持一致",
        "所选配置文件的 reset.credential_version 与显式请求不同",
        "所选配置文件的 reset.redis_outside_sentinel_key 与显式请求不同",
        "所选配置文件的 redis.host 未显式匹配本侧配置",
        "所选配置文件的 redis.port 未显式匹配本侧配置",
        "所选配置文件的 redis.database 未显式匹配本侧配置",
        "所选配置文件的 redis.tls 未显式匹配本侧配置",
    ];
    MESSAGES.iter().copied().find(|message| *message == reason)
}
