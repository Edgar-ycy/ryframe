//! 从真实登记与明文文件配置装配角色合同，不借用 CI Context。
use super::role::Role;
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn load(role: Role, backend: &Path, directory: &Path) -> Result<Value> {
    if role == Role::Frontend {
        return crate::recovery::fixture_runtime::frontend_runtime_contract(backend, directory);
    }
    use crate::recovery::fixture_control::artifact::runtime;

    let binding = runtime::verify(backend, directory)?;
    let mut runtime = binding.receipt;
    if role == Role::Api {
        let app = binding
            .table
            .get("app")
            .and_then(toml::Value::as_table)
            .ok_or("缺少 app 配置")?;
        let host = app
            .get("host")
            .and_then(toml::Value::as_str)
            .ok_or("缺少 app.host")?;
        let port = app
            .get("port")
            .and_then(toml::Value::as_integer)
            .filter(|port| (1..=65535).contains(port))
            .ok_or("app.port 无效")?;
        let host = match host {
            "127.0.0.1" | "0.0.0.0" => "127.0.0.1",
            "::1" | "::" => "[::1]",
            _ => return Err("受监督 API 必须绑定可核验的本机监听地址".into()),
        };
        let current = runtime::verify_configuration(backend, directory)?;
        if current.receipt != runtime || current.table != binding.table {
            return Err("API 配置或运行登记在装配期间发生变化".into());
        }
        runtime["api_ready_url"] = json!(format!("http://{host}:{port}/readyz"));
    }
    Ok(runtime)
}
