//! 后继审阅的实际工具复核；版本命令与摘要读取不会执行项目辅助脚本。
use super::super::super::json as document;
#[cfg(windows)]
use super::super::{super::target_verify, model};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn observe(
    backend: &Path,
    review: &Value,
    process: &Value,
    current: bool,
) -> Result<Value> {
    #[cfg(not(windows))]
    let _ = backend;
    if current {
        super::super::require_windows(process)?;
    }
    let (code, response) = crate::recovery::fixture_control::environment::protocol::execute(
        &serde_json::to_vec(
            &json!({"format_version":1,"operation":"preflight-binding","arguments":review,"write":false}),
        )?,
    );
    if code != 0 || response["ok"] != true {
        return Err("夹具审阅缺少正式工具预检收据".into());
    }
    if process["format_version"] == 3 {
        let tools = &review["tools"];
        document::exact(
            tools,
            &["mysql", "mysqldump", "rustfs", "redis_server"],
            "Windows 缓存审阅工具",
        )?;
        super::super::windows_tool::validate(&tools["redis_server"])?;
        if tools["redis_server"] != process["tool"] {
            return Err("Windows 缓存后继不能替换原 Redis 工具或 DLL".into());
        }
        if current {
            #[cfg(not(windows))]
            return Err("Windows 缓存工具只能在本机 Windows 核验".into());
            #[cfg(windows)]
            {
                for field in ["mysql", "mysqldump", "rustfs"] {
                    target_verify::input::request::external_file(&tools[field])?;
                }
                super::super::windows_tool::verify_registered(backend, &tools["redis_server"])?;
                model::validate(backend, process, true)?;
            }
        }
        return Ok(tools.clone());
    }
    let mut tools = serde_json::Map::new();
    for field in [
        "mysql",
        "mysqldump",
        "rustfs",
        "redis_server",
        "wsl",
        if process["format_version"] == 2 {
            "native_tool"
        } else {
            "redis_python"
        },
    ] {
        tools.insert(field.into(), review["tools"][field].clone());
    }
    Ok(Value::Object(tools))
}
