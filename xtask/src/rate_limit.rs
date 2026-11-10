//! 浏览器与 DevEx 夹具共享的限流配置读取协议。

use std::{
    env,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Result, recovery::clone::native::digest};

#[path = "rate_limit/config.rs"]
mod config;
#[path = "rate_limit/protocol.rs"]
mod protocol;

#[allow(unused_imports)]
pub(crate) use config::{
    DEFAULT_API_WINDOW_SECS, DEFAULT_LOGIN_CAPACITY, LOGIN_RULE, RateLimitError, authority,
    load_app_table, settings,
};

const MARKER: &str = "__rate-limit";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// 文件配置请求只携带明确目录和环境选择，不接收字段环境覆盖。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileContext {
    pub(crate) backend: PathBuf,
    pub(crate) app_env: String,
    #[serde(default)]
    pub(crate) config_dir: Option<PathBuf>,
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut arguments = env::args_os().skip(1);
    if arguments.next()?.as_os_str() != MARKER {
        return None;
    }
    let arguments = arguments.collect::<Vec<_>>();
    let mut input = Vec::new();
    let read = std::io::stdin()
        .take((MAX_REQUEST_BYTES as u64) + 1)
        .read_to_end(&mut input);
    let (code, output) = match read {
        Ok(_) if input.len() <= MAX_REQUEST_BYTES => execute(&arguments, &input),
        Ok(_) => protocol::invalid_request(),
        Err(_) => protocol::invalid_request(),
    };
    println!("{output}");
    Some(code)
}

pub(crate) fn execute(arguments: &[OsString], input: &[u8]) -> (i32, String) {
    if input.len() > MAX_REQUEST_BYTES {
        return protocol::invalid_request();
    }
    protocol::execute(arguments, input)
}

pub(crate) fn tool_binding(executable: &Path) -> Result<Option<String>> {
    let path = executable.to_str().ok_or("当前检查工具路径不是 UTF-8")?;
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    // 既有 Windows 二进制保护仅支持本机盘符；正式入口由必需构造器拒绝其他布局。
    if cfg!(windows) && (path.starts_with("UNC\\") || path.starts_with(r"\\")) {
        return Ok(None);
    }
    if digest::linked_path(executable)?
        .into_iter()
        .any(|linked| linked)
    {
        return Ok(None);
    }
    let observed = digest::file(executable)?;
    Ok(Some(serde_json::to_string(&json!({
        "format_version": 1,
        "kind": "ryframe-native-rate-limit-tool",
        "path": path,
        "bytes": observed.bytes,
        "sha256": observed.sha256,
    }))?))
}

pub(crate) fn current_tool_binding() -> Result<Value> {
    let executable = std::env::current_exe()?;
    let binding = tool_binding(&executable)?
        .ok_or("Python 原生调用要求显式绑定无链接的本机当前 xtask 工具")?;
    Ok(serde_json::from_str(&binding)?)
}
