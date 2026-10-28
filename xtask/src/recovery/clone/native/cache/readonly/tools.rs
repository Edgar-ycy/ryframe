//! 后继审阅的实际工具复核；版本命令与摘要读取不会执行项目辅助脚本。
use super::{
    super::{
        super::{json as document, target_verify},
        model,
    },
    wsl,
};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn observe(
    backend: &Path,
    review: &Value,
    process: &Value,
    current: bool,
) -> Result<Value> {
    let (code, response) = crate::recovery::fixture_control::environment::protocol::execute(
        &serde_json::to_vec(
            &json!({"format_version":1,"operation":"preflight-binding","arguments":review,"write":false}),
        )?,
    );
    if code != 0 || response["ok"] != true {
        return Err("夹具审阅缺少正式工具预检收据".into());
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
    if !current {
        return Ok(Value::Object(tools));
    }
    for field in ["mysql", "mysqldump", "rustfs"] {
        target_verify::input::request::external_file(&tools[field])?;
    }
    let mut effective = process.clone();
    effective["wsl"] = tools["wsl"].clone();
    if process["format_version"] == 2 {
        effective = super::super::native_tool::successor(process, &Value::Object(tools.clone()))?;
    } else {
        effective["python"] = json!({"path":tools["redis_python"]["path"],"executable":tools["redis_python"]["resolved_path"],"sha256":tools["redis_python"]["sha256"]});
    }
    let observer = wsl::Observer::new(backend, &effective)?;
    let declared = target_verify::input::request_configuration::canonical_path(Path::new(
        document::text(&tools["wsl"]["path"], "预检 WSL")?,
    ))?;
    let actual = super::super::native_tool::wsl_program()?;
    if target_verify::input::request_configuration::canonical_path(&actual)? != declared {
        return Err("本机 WSL 入口不同于后继审阅冻结工具".into());
    }
    let executable = observer.read("/usr/bin/readlink", &["-f", "/usr/bin/redis-server"])?;
    let hash = observer.read("/usr/bin/sha256sum", &["/usr/bin/redis-server"])?;
    let version = observer.read("/usr/bin/redis-server", &["--version"])?;
    let redis = &tools["redis_server"];
    if executable.trim() != document::text(&redis["resolved_path"], "Redis 实际映像")?
        || hash.split_whitespace().next() != redis["sha256"].as_str()
        || version.trim() != document::text(&redis["version"], "Redis 版本")?
        || redis["distribution"] != effective["distribution"]
        || (process["format_version"] != 2
            && tools["redis_python"]["distribution"] != effective["distribution"])
    {
        return Err("夹具审阅中的当前 WSL 工具代次已经变化".into());
    }
    model::validate(backend, &effective, true)?;
    observer.finish()?;
    Ok(Value::Object(tools))
}
