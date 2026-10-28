//! 独立显式 Linux 写入口；观察入口不接受这些操作。
use super::{
    super::{digest, json as doc},
    native_tool::NativeTool,
};
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(target_os = "linux")]
use serde_json::json;
#[cfg(target_os = "linux")]
use std::io::{BufRead, Read};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
mod inputs;
#[cfg(target_os = "linux")]
mod owned;
#[cfg(target_os = "linux")]
mod supervisor;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Launch {
    pub format_version: u8,
    pub request: Value,
    pub request_sha256: String,
    pub windows_output: String,
    pub linux_output: PathBuf,
    pub native_tool: NativeTool,
}

pub(super) fn publish(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().ok_or("缓存收据没有父目录")?;
    let pending = parent.join(format!(".{}.pending", uuid::Uuid::new_v4().simple()));
    doc::write_new(&pending, value)?;
    let result = std::fs::hard_link(&pending, path);
    let cleanup = std::fs::remove_file(&pending);
    result?;
    cleanup?;
    Ok(())
}

fn load(path: &Path, sha: &str) -> Result<Launch> {
    super::model::plain(path)?;
    let before = digest::file(path)?;
    if before.sha256 != sha {
        return Err("Linux 缓存请求文件摘要不同".into());
    }
    let launch: Launch = serde_json::from_value(doc::read(path)?)?;
    super::model::shape(&launch.request)?;
    if launch.format_version != 2
        || launch.request["format_version"] != 2
        || launch.request_sha256 != digest::plan_hash(&launch.request)?
        || serde_json::to_value(&launch.native_tool)? != launch.request["native_tool"]
        || launch.request["distribution"] != launch.native_tool.distribution
        || super::model::linux_path(&launch.windows_output)?
            != launch.linux_output.to_str().ok_or("Linux 输出不是UTF-8")?
        || digest::file(path)? != before
    {
        return Err("Linux 缓存仅执行绑定的 v2 原生请求".into());
    }
    launch.native_tool.validate()?;
    Ok(launch)
}

pub(super) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let exec = args.first().is_some_and(|s| s == "__linux-cache-exec");
    if !exec && args.first().is_none_or(|s| s != "__linux-cache") {
        return None;
    }
    let result = run(&args, exec);
    if let Ok(Some(value)) = &result
        && writeln!(std::io::stdout().lock(), "{value}").is_err()
    {
        return Some(2);
    }
    // 只发布已校验的固定诊断，原始错误和 stderr 不进入公开协议。
    if let Err(error) = &result {
        let value = serde_json::json!({"ok":false,"error":"NativeCacheRejected",
            "diagnostic":super::diagnostic::from_error(error.as_ref())});
        let _ = writeln!(std::io::stdout().lock(), "{value}");
    }
    Some(if result.is_ok() { 0 } else { 2 })
}

fn run(args: &[String], exec: bool) -> Result<Option<Value>> {
    if !exec && args == ["__linux-cache", "describe"] {
        return describe();
    }
    let offset = if exec { 1 } else { 2 };
    if args.len() != offset + 4 || args[offset] != "--request" || args[offset + 2] != "--sha256" {
        return Err("Linux 缓存参数无效".into());
    }
    let path = Path::new(&args[offset + 1]);
    let launch = load(path, &args[offset + 3]).map_err(|_| {
        super::diagnostic::failure(
            super::diagnostic::Category::RequestRejected,
            super::diagnostic::Outcome::NotStarted,
        )
    })?;
    #[cfg(target_os = "linux")]
    {
        launch.native_tool.verify_linux()?;
        if exec {
            return inputs::exec(&launch, path, &args[offset + 3]).map(|()| None);
        }
        match args[1].as_str() {
            "serve" => {
                let mut bytes = Vec::new();
                std::io::Read::take(&mut std::io::stdin().lock(), 16385)
                    .read_until(b'\n', &mut bytes)?;
                let auth = doc::parse_fragment(std::str::from_utf8(&bytes)?, "Linux 缓存授权")?;
                if bytes.len() > 16384
                    || auth
                        != json!({"request_sha256":launch.request_sha256,"output":launch.windows_output})
                {
                    return Err("Linux 缓存未获得先行登记授权".into());
                }
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(supervisor::serve(&launch, path, &args[offset + 3]))?;
                Ok(None)
            }
            action @ ("stop" | "check" | "observe") => {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .lock()
                    .take(16385)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > 16384 {
                    return Err("缓存身份输入超限".into());
                }
                let expected = doc::parse_fragment(std::str::from_utf8(&bytes)?, "Linux 缓存身份")?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let _entered = runtime.enter();
                inputs::check(&launch, &expected, action == "stop").map(Some)
            }
            _ => Err("未知 Linux 缓存写动作".into()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = launch;
        Err("缓存原生写入口只允许 Linux".into())
    }
}

fn describe() -> Result<Option<Value>> {
    #[cfg(target_os = "linux")]
    {
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.is_empty() || bytes.len() > 1024 * 1024 {
            return Err("缓存工具描述输入超限".into());
        }
        let value = doc::parse_fragment(std::str::from_utf8(&bytes)?, "缓存原生工具描述")?;
        doc::exact(
            &value,
            &["format_version", "native_tool"],
            "缓存原生工具描述",
        )?;
        if value["format_version"] != 2 {
            return Err("缓存工具描述版本无效".into());
        }
        let tool: NativeTool = serde_json::from_value(value["native_tool"].clone())?;
        tool.verify_linux()?;
        Ok(Some(
            json!({"format_version":2,"uid":tool.uid,"program":tool.linux_program,
            "sha256":tool.sha256,"source_commit":tool.source_commit,"source_sha256":tool.source_sha256}),
        ))
    }
    #[cfg(not(target_os = "linux"))]
    Err("缓存工具描述仅允许 Linux".into())
}
