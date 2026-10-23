use super::super::{digest, json as document, resources::common};
use crate::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

pub(super) const FIELDS: &[&str] = &[
    "scope_id",
    "wsl",
    "distribution",
    "launcher",
    "executable",
    "sha256",
    "configuration",
    "directory",
    "previous_identity",
    "previous_boot_id",
    "previous_run_id",
    "port",
    "password",
    "timeout_seconds",
    "python",
];

pub(super) fn matches(value: &Value, pattern: &str) -> Result<bool> {
    Ok(value
        .as_str()
        .is_some_and(|text| Regex::new(pattern).expect("固定缓存规则").is_match(text)))
}

pub(super) fn password(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty() && !text.contains('\0'))
        .ok_or_else(|| "Redis password 必须是配置文件中的非空 UTF-8 文本，不含 NUL".into())
}

pub(super) fn validate(backend: &Path, request: &Value, files: bool) -> Result<()> {
    document::exact(request, FIELDS, "Redis 进程请求")?;
    password(&request["password"])?;
    if !matches(&request["scope_id"], r"^[a-z0-9][a-z0-9_-]{2,47}$")?
        || !matches(&request["distribution"], r"^[A-Za-z0-9._-]+$")?
        || request["launcher"] != "/usr/bin/redis-server"
        || !matches(&request["executable"], r"^/usr/(?:local/)?bin/[a-z0-9-]+$")?
        || !request["port"]
            .as_u64()
            .is_some_and(|v| (1024..=65535).contains(&v))
        || !request["timeout_seconds"]
            .as_u64()
            .is_some_and(|v| (1..=180).contains(&v))
        || !matches(&request["sha256"], r"^[a-f0-9]{64}$")?
    {
        return Err("Redis 原进程、端点或工具声明无效".into());
    }
    predecessor(request)?;
    document::exact(
        &request["directory"],
        &["path", "device", "inode"],
        "Redis 目录",
    )?;
    document::exact(&request["wsl"], &["path", "sha256"], "WSL 工具")?;
    document::exact(
        &request["python"],
        &["path", "executable", "sha256"],
        "Linux 解释器",
    )?;
    if request["python"]["path"] != "/usr/bin/python3"
        || !matches(
            &request["python"]["executable"],
            r"^/usr/bin/python3\.[0-9]+$",
        )?
        || !matches(&request["python"]["sha256"], r"^[a-f0-9]{64}$")?
    {
        return Err("WSL Python 必须绑定明确入口、实际文件与摘要".into());
    }
    let directory = Path::new(document::text(&request["directory"]["path"], "Redis 目录")?);
    if Path::new(document::text(
        &request["configuration"]["path"],
        "Redis 配置",
    )?) != directory.join("redis.conf")
    {
        return Err("Redis 仅使用原服务目录中的配置文件".into());
    }
    if files {
        let _ = backend;
        // Python 登记使用完整 st_dev；旧版 Windows FILE_INFORMATION 只有低32位，
        // 原控制器继续在调用前核对完整 device/inode，不以截断值替换该核验。
        if !plain(directory)?.is_dir() {
            return Err("Redis 原数据目录不是实际目录".into());
        }
        file_bound(&request["configuration"])?;
        let wsl = plain(Path::new(document::text(
            &request["wsl"]["path"],
            "WSL 工具",
        )?))?;
        if digest::file(&wsl)?.sha256 != document::text(&request["wsl"]["sha256"], "WSL 摘要")? {
            return Err("WSL 实际工具变化".into());
        }
    }
    Ok(())
}

fn predecessor(request: &Value) -> Result<()> {
    let previous = &request["previous_identity"];
    if previous.is_null() {
        if !request["previous_boot_id"].is_null() || !request["previous_run_id"].is_null() {
            return Err("首代 Redis 不得带前驱启动或运行代次".into());
        }
    } else {
        identity(previous, false)?;
        if previous["executable"] != request["executable"]
            || !matches(&request["previous_run_id"], r"^[a-f0-9]{40}$")?
            || !boot(&request["previous_boot_id"])?
        {
            return Err("Redis 原始 Linux 创建身份不完整".into());
        }
    }
    Ok(())
}

pub(super) fn boot(value: &Value) -> Result<bool> {
    matches(value, r"^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$")
}

pub(super) fn identity(value: &Value, linux: bool) -> Result<()> {
    let fields: &[&str] = if linux {
        &["pid", "started", "executable", "boot_id"]
    } else {
        &["pid", "started", "executable"]
    };
    document::exact(value, fields, "缓存创建身份")?;
    if !value["pid"]
        .as_u64()
        .is_some_and(|v| v > 1 && u32::try_from(v).is_ok())
        || !matches(&value["started"], r"^[0-9]+$")?
        || value["executable"].as_str().is_none_or(|v| v.is_empty())
        || linux && !boot(&value["boot_id"])?
    {
        return Err("缓存创建身份字段不完整".into());
    }
    Ok(())
}

pub(super) fn plain(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute()
        || !path.exists()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        || digest::linked_path(path)?.into_iter().any(|linked| linked)
    {
        return Err("缓存控制只接受无链接的已有绝对路径".into());
    }
    Ok(path.to_path_buf())
}

pub(super) fn file_bound(value: &Value) -> Result<PathBuf> {
    document::exact(value, &["path", "bytes", "sha256"], "缓存文件绑定")?;
    let path = plain(Path::new(document::text(&value["path"], "缓存文件")?))?;
    if common::binding(&path)? != *value {
        return Err("缓存控制文件摘要变化".into());
    }
    Ok(path)
}

pub(super) fn linux_path(value: &str) -> Result<String> {
    let normalized = value.replace('\\', "/");
    let bytes = normalized.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || &bytes[1..3] != b":/"
        || normalized[3..]
            .split('/')
            .any(|part| part == ".." || part.contains(':'))
    {
        return Err("WSL 仅映射已登记 Windows 盘符绝对路径".into());
    }
    Ok(format!(
        "/mnt/{}/{}",
        char::from(bytes[0]).to_ascii_lowercase(),
        &normalized[3..]
    ))
}

pub(super) fn configuration(request: &Value) -> Result<Value> {
    Ok(
        json!({"bind":"127.0.0.1", "port":request["port"].as_u64().ok_or("Redis 端口无效")?.to_string(),
        "dir":linux_path(document::text(&request["directory"]["path"], "Redis 目录")?)?,
        "databases":"1", "protected-mode":"yes", "save":"", "appendonly":"no", "daemonize":"no"}),
    )
}
