//! 从真实登记与明文文件配置装配角色合同，不借用 CI Context。
use super::role::Role;
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn load(role: Role, backend: &Path, directory: &Path) -> Result<Value> {
    if role == Role::Frontend {
        return crate::recovery::fixture_runtime::frontend_runtime_contract(backend, directory);
    }
    let mut runtime =
        crate::recovery::fixture_control::verify_full_stack_runtime(backend, directory)?;
    if role == Role::Api {
        let original = runtime.clone();
        let config = Path::new(
            runtime["api_config_dir"]
                .as_str()
                .ok_or("缺少 API 配置目录")?,
        );
        let table = crate::rate_limit::load_app_table(backend, Some(config), "test")
            .map_err(|_| "API 文件配置读取失败；核对已绑定配置，不回显正文")?;
        let app = table
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
        runtime["api_ready_url"] = json!(format!("http://{host}:{port}/readyz"));
        if crate::recovery::fixture_control::verify_full_stack_runtime(backend, directory)?
            != original
        {
            return Err("API 配置或运行登记在装配期间发生变化".into());
        }
    }
    Ok(runtime)
}
