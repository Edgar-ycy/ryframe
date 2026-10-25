use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use super::{StorageInputs, StorageObserver, common, json_io};
use crate::Result;

pub(crate) fn verify<O: StorageObserver>(
    context: &StorageInputs<'_>,
    review: &Value,
    redis: &Value,
    observer: &O,
) -> Result<()> {
    validate_identity(context, review, redis)?;
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
    for (key, value) in [
        ("bind", "127.0.0.1"),
        ("port", port.as_str()),
        ("dir", directory.as_str()),
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
