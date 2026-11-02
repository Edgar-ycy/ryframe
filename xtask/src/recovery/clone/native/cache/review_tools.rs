//! 当前夹具工具预检由 Rust 读取配置和实际工具，不探测 Linux Python。
use super::{
    super::{digest, json as doc},
    native_tool::{self, NativeTool},
};
use crate::Result;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub(super) fn local_tools(review: &Value) -> Result<Value> {
    let mut result = json!({});
    for role in ["mysql", "mysqldump", "rustfs"] {
        let value = &review["tools"][role];
        let path = super::model::plain(Path::new(doc::text(&value["path"], "夹具工具")?))?;
        if digest::file(&path)?.sha256 != doc::text(&value["sha256"], "夹具工具摘要")? {
            return Err("夹具本机工具字节变化".into());
        }
        result[role] = json!({"path":path,"sha256":value["sha256"]});
    }
    Ok(result)
}

pub(in crate::recovery) fn observe(backend: &Path, review: &Value) -> Result<Value> {
    if review["ready_for_execution"].as_bool().is_none() {
        return Err("夹具预检状态无效".into());
    }
    let mut result = local_tools(review)?;
    let path = native_tool::wsl_program()?;
    let wsl = json!({"path":path,"sha256":digest::file(&path)?.sha256});
    let tool = NativeTool::capture(backend)?;
    if review["tools"]["redis_server"]["distribution"] != tool.distribution {
        return Err("Redis 工具与绑定 Linux 工具发行版不同".into());
    }
    native_tool::verify_remote(backend, &wsl, &tool)?;
    let read = |program: &str, args: &[&str]| read(backend, &wsl, &tool, program, args);
    let executable = read("/usr/bin/readlink", &["-f", "/usr/bin/redis-server"])?;
    let sha = read("/usr/bin/sha256sum", &["/usr/bin/redis-server"])?;
    let version = read("/usr/bin/redis-server", &["--version"])?;
    let executable = executable.trim();
    let sha = sha.split_whitespace().next().ok_or("Redis 工具缺少摘要")?;
    if !matches!(
        executable,
        "/usr/bin/redis-server" | "/usr/bin/redis-check-rdb"
    ) || !crate::frontend_origin::manifest::hex_digest(sha, 64)
        || !version.trim().starts_with("Redis server v=")
    {
        return Err("Redis 实际工具预检失败".into());
    }
    result["redis_server"] = json!({"distribution":tool.distribution,"path":"/usr/bin/redis-server",
        "resolved_path":executable,"sha256":sha,"version":version.trim()});
    result["wsl"] = wsl;
    result["native_tool"] = serde_json::to_value(&tool)?;
    tool.finish(backend)?;
    Ok(result)
}

fn read(
    backend: &Path,
    wsl: &Value,
    tool: &NativeTool,
    program: &str,
    args: &[&str],
) -> Result<String> {
    let mut arguments = vec![
        "--distribution",
        &tool.distribution,
        "--user",
        &tool.user,
        "--exec",
        program,
    ];
    arguments.extend_from_slice(args);
    let output = crate::process::command_output_with_input_timeout(
        backend,
        doc::text(&wsl["path"], "WSL 工具")?,
        &arguments,
        &[],
        None,
        Duration::from_secs(20),
    )?;
    if !output.status.success()
        || output.stdout.len() > 16384
        || digest::file(Path::new(doc::text(&wsl["path"], "WSL 工具")?))?.sha256 != wsl["sha256"]
    {
        return Err("Redis 预检读取失败或 WSL 字节变化".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "Redis 预检不是UTF-8".into())
}
