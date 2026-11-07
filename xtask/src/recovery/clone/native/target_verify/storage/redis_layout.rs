use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{StorageInputs, StorageObserver, common, json_io};
use crate::Result;
#[cfg(windows)]
use {
    super::{
        super::super::cache::{model::cygwin_path, windows_tool},
        super::input::request,
    },
    crate::recovery::{
        fixture_control::services::lifecycle::readonly::process,
        monitoring::execution::ProtectedInputs,
    },
};

pub(crate) fn verify<O: StorageObserver>(
    context: &StorageInputs<'_>,
    review: &Value,
    redis: &Value,
    observer: &O,
) -> Result<()> {
    if redis["platform"] == "windows" {
        #[cfg(windows)]
        return verify_windows(context, review, redis, observer);
        #[cfg(not(windows))]
        return Err("目标 Windows Redis 物理代次只能在本机 Windows 核验".into());
    }
    validate_identity(context, review, redis)?;
    let (directory, config) = configuration_paths(context, review, redis)?;
    let linux_file = wsl_path(&config)?;
    let stat = observer.wsl(
        context,
        redis,
        &[
            "/usr/bin/cat".into(),
            format!("/proc/{}/stat", redis["pid"]),
        ],
    )?;
    let executable = observer.wsl(
        context,
        redis,
        &[
            "/usr/bin/readlink".into(),
            "-f".into(),
            format!("/proc/{}/exe", redis["pid"]),
        ],
    )?;
    let sha = observer.wsl(
        context,
        redis,
        &[
            "/usr/bin/sha256sum".into(),
            json_io::text(&redis["executable"], "Redis executable")?.into(),
        ],
    )?;
    let raw_info = observer.redis(context, redis, &["INFO", "server"])?;
    let info = info(json_io::text(&raw_info, "Redis INFO")?)?;
    validate_kernel(redis, &stat, &executable, &sha, &info)?;
    let actual_path = observer.wsl(
        context,
        redis,
        &["/usr/bin/readlink".into(), "-f".into(), linux_file.clone()],
    )?;
    let actual_sha = observer.wsl(
        context,
        redis,
        &["/usr/bin/sha256sum".into(), linux_file.clone()],
    )?;
    if info.get("config_file") != Some(&linux_file)
        || actual_path != linux_file
        || actual_sha.split_whitespace().next() != redis["configuration"]["sha256"].as_str()
    {
        return Err("Redis 实际配置路径或字节变化".into());
    }
    verify_configuration(redis, &directory, &mut |parts| {
        observer.redis(context, redis, parts)
    })?;
    // 观察结束仍核对配置文件摘要，不能让 CONFIG GET 窗口中的替换通过。
    common::bound_file(context.backend, &redis["configuration"], "Redis 配置")?;
    Ok(())
}

fn configuration_paths(
    context: &StorageInputs<'_>,
    review: &Value,
    redis: &Value,
) -> Result<(PathBuf, PathBuf)> {
    let directory = common::local_directory(
        context.backend,
        Path::new(json_io::text(
            &review["services"]["redis"]["directory"],
            "Redis 审阅目录",
        )?),
        "Redis 配置目录",
    )?;
    let config = common::bound_file(context.backend, &redis["configuration"], "Redis 配置")?;
    if config != directory.join("redis.conf") {
        return Err("Redis 配置不是审阅目录中的 redis.conf".into());
    }
    Ok((directory, config))
}

#[cfg(windows)]
fn verify_windows<O: StorageObserver>(
    context: &StorageInputs<'_>,
    review: &Value,
    redis: &Value,
    observer: &O,
) -> Result<()> {
    let (pid, port) = windows_projection(context, review, redis)?;
    let (directory, config) = configuration_paths(context, review, redis)?;
    let package = windows_tool::package_bindings(&redis["tool"])?;
    let mut paths = package
        .iter()
        .map(request::external_file)
        .collect::<Result<Vec<_>>>()?;
    paths.push(config.clone());
    let protected = ProtectedInputs::open(paths)?;
    let result = (|| {
        // 在保护句柄下重新确认登记摘要，覆盖摘要检查到取得句柄之间的窗口。
        for descriptor in &package {
            request::external_file(descriptor)?;
        }
        common::bound_file(
            context.backend,
            &redis["configuration"],
            "Windows Redis 配置",
        )?;
        let identity = windows_process(redis, pid, port)?;
        let observed =
            observe_windows_configuration(context, redis, observer, &directory, &config, port);
        let after = (|| {
            if windows_process(redis, pid, port)? != identity
                || configuration_paths(context, review, redis)? != (directory, config)
            {
                return Err("Windows Redis 协议观察期间原进程或配置绑定变化".into());
            }
            Ok(())
        })();
        match (observed, after) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(after)) => {
                Err(format!("{error}；Windows Redis 物理代次后验失败：{after}").into())
            }
        }
    })();
    protected.finish(result)
}

#[cfg(windows)]
fn windows_projection(
    context: &StorageInputs<'_>,
    review: &Value,
    redis: &Value,
) -> Result<(u32, u16)> {
    json_io::exact(
        redis,
        &[
            "platform",
            "port",
            "pid",
            "started",
            "executable",
            "sha256",
            "configuration",
            "run_id",
            "tool",
        ],
        "Windows Redis 物理代次",
    )?;
    let pid = redis["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 1)
        .ok_or("Windows Redis PID 无效")?;
    let port = redis["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or("Windows Redis 端口无效")?;
    let side = json_io::text(&context.request["side"], "目标侧")?;
    let tool = &redis["tool"];
    let started = json_io::text(&redis["started"], "Windows Redis 创建代次")?;
    let run = json_io::text(&redis["run_id"], "Windows Redis run_id")?;
    let executable = json_io::text(&redis["executable"], "Windows Redis 登记映像")?;
    if redis["platform"] != "windows"
        || *tool != review["tools"]["redis_server"]
        || redis["sha256"] != tool["sha256"]
        || !same_windows_image(
            executable,
            json_io::text(&tool["path"], "Windows Redis 工具路径")?,
        )
        || !started.bytes().all(|byte| byte.is_ascii_digit())
        || started.parse::<u64>().is_err()
        || run.len() != 40
        || !run
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        || review["scopes"][side]["redis"]["url"] != json!(format!("redis://127.0.0.1:{port}/0"))
    {
        return Err("Windows Redis 物理身份、审阅工具或回环端点不同".into());
    }
    Ok((pid, port))
}

#[cfg(windows)]
fn windows_process(redis: &Value, pid: u32, port: u16) -> Result<Value> {
    let identity = common::process_identity(pid)?.ok_or("Windows Redis 登记进程不存在")?;
    let executable = json_io::text(&redis["executable"], "Windows Redis 登记映像")?;
    let image = json_io::text(&identity["executable"], "Windows Redis 实际映像")?;
    if identity["pid"] != pid
        || identity["started"] != redis["started"]
        || !same_windows_image(image, executable)
    {
        return Err("Windows Redis 内核创建身份与原登记不同".into());
    }
    let arguments = process::arguments(&identity)?;
    let [program, config] = arguments.as_slice() else {
        return Err("Windows Redis 实际参数不是登记映像和 redis.conf".into());
    };
    if !same_windows_image(program, executable)
        || config != "redis.conf"
        || !process::loopback_listener(&identity, port)?
    {
        return Err("Windows Redis 原命令行或回环监听归属不同".into());
    }
    Ok(identity)
}

#[cfg(windows)]
fn same_windows_image(left: &str, right: &str) -> bool {
    left.replace('\\', "/")
        .eq_ignore_ascii_case(&right.replace('\\', "/"))
}

#[cfg(windows)]
fn observe_windows_configuration<O: StorageObserver>(
    context: &StorageInputs<'_>,
    redis: &Value,
    observer: &O,
    directory: &Path,
    config: &Path,
    port: u16,
) -> Result<()> {
    let directory = cygwin_path(directory.to_str().ok_or("Windows Redis 目录不是 UTF-8")?)?;
    let config = cygwin_path(config.to_str().ok_or("Windows Redis 配置路径不是 UTF-8")?)?;
    observer.redis_window(context, redis, |send| {
        let raw = send(&["INFO", "server"])?;
        let info = info(json_io::text(&raw, "Windows Redis INFO")?)?;
        if info.get("run_id").map(String::as_str) != redis["run_id"].as_str()
            || info.get("config_file") != Some(&config)
            || !info.get("process_id").is_some_and(|pid| {
                pid.bytes().all(|byte| byte.is_ascii_digit())
                    && pid.parse::<u32>().is_ok_and(|pid| pid > 0)
            })
        {
            return Err("Windows Redis 自报进程号、认证运行代次或实际配置路径不同".into());
        }
        verify_configuration_values(&port.to_string(), &directory, &mut |parts| send(parts))
    })
}

fn validate_identity(context: &StorageInputs<'_>, review: &Value, redis: &Value) -> Result<()> {
    json_io::exact(
        redis,
        &[
            "port",
            "wsl",
            "distribution",
            "pid",
            "started",
            "executable",
            "sha256",
            "run_id",
            "configuration",
        ],
        "Redis 物理代次",
    )?;
    let wsl = json_io::exact(&redis["wsl"], &["path", "sha256"], "Redis WSL 工具")?;
    let path = Path::new(json_io::text(&wsl["path"], "Redis WSL 路径")?);
    let expected = &review["tools"]["redis_server"];
    json_io::text(&redis["distribution"], "Redis distribution")?;
    json_io::text(&redis["executable"], "Redis executable")?;
    let side = json_io::text(&context.request["side"], "目标侧")?;
    let port = redis["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or("Redis 端口无效")?;
    let started = json_io::text(&redis["started"], "Redis 创建代次")?;
    let run = json_io::text(&redis["run_id"], "Redis run_id")?;
    if super::super::super::digest::linked_path(path)?
        .into_iter()
        .any(|linked| linked)
        || super::super::super::digest::file(path)?.sha256
            != json_io::text(&wsl["sha256"], "WSL 摘要")?
        || redis.get("distribution") != expected.get("distribution")
        || redis.get("executable") != expected.get("resolved_path")
        || redis.get("sha256") != expected.get("sha256")
        || redis["pid"].as_u64().is_none_or(|pid| pid <= 1)
        || started.is_empty()
        || !started.bytes().all(|byte| byte.is_ascii_digit())
        || run.len() != 40
        || !run
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || review["scopes"][side]["redis"]["url"] != json!(format!("redis://127.0.0.1:{port}/0"))
    {
        return Err("Redis 物理身份或当前选中 WSL 工具不同".into());
    }
    Ok(())
}

pub(crate) fn info(raw: &str) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for line in raw.lines().filter(|line| !line.starts_with('#')) {
        if let Some((key, value)) = line.split_once(':')
            && result.insert(key.into(), value.into()).is_some()
        {
            return Err("Redis INFO 身份字段重复".into());
        }
    }
    Ok(result)
}

pub(crate) fn validate_kernel(
    redis: &Value,
    stat: &str,
    executable: &str,
    sha: &str,
    info: &BTreeMap<String, String>,
) -> Result<()> {
    let fields = stat
        .rsplit_once(')')
        .ok_or("Redis stat 缺少进程名称边界")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let pid = redis["pid"].as_u64().ok_or("Redis PID 无效")?.to_string();
    if fields.get(19).copied() != redis["started"].as_str()
        || Some(executable) != redis["executable"].as_str()
        || sha.split_whitespace().next() != redis["sha256"].as_str()
        || info.get("run_id").map(String::as_str) != redis["run_id"].as_str()
        || info.get("process_id") != Some(&pid)
    {
        return Err("Redis 端点或内核进程已重启或变化".into());
    }
    Ok(())
}

pub(crate) fn verify_configuration(
    redis: &Value,
    directory: &Path,
    command: &mut impl FnMut(&[&str]) -> Result<Value>,
) -> Result<()> {
    let port = redis["port"].as_u64().ok_or("Redis 端口无效")?.to_string();
    let directory = wsl_path(directory)?;
    verify_configuration_values(&port, &directory, command)
}

fn verify_configuration_values(
    port: &str,
    directory: &str,
    command: &mut impl FnMut(&[&str]) -> Result<Value>,
) -> Result<()> {
    for (key, value) in [
        ("bind", "127.0.0.1"),
        ("port", port),
        ("dir", directory),
        ("databases", "1"),
        ("protected-mode", "yes"),
        ("save", ""),
        ("appendonly", "no"),
        ("daemonize", "no"),
    ] {
        if command(&["CONFIG", "GET", key])? != json!([key, value]) {
            return Err("Redis 有效配置不同于固定目录、端口或持久化声明".into());
        }
    }
    Ok(())
}

pub(crate) fn wsl_path(path: &Path) -> Result<String> {
    let text = path.to_str().ok_or("Redis 配置路径必须是 UTF-8")?;
    let bytes = text.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'/' | b'\\')
    {
        return Err("Redis 配置须是本机 Windows 盘符目录".into());
    }
    Ok(format!(
        "/mnt/{}{}",
        char::from(bytes[0].to_ascii_lowercase()),
        text[2..].replace('\\', "/")
    ))
}
