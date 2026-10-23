use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, FixedOffset};
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::document::{array, digest, exact, integer, linked, text};
use crate::{
    Result,
    local_test_path::{
        LocalTestPathKind, validate_external_existing_file, validate_local_test_path,
    },
};

pub(crate) const BUCKETS: [&str; 5] =
    ["uploads", "avatar", "exports", "imports", "config-packages"];
pub(crate) const EXCLUDED_TABLES: [&str; 4] = [
    "ryframe_resource_ownership",
    "sys_backup_set",
    "sys_backup_resource",
    "sys_restore_run",
];
pub(crate) const TARGET_DATABASES: [(&str, &str, &str); 4] = [
    ("shared-control", "combined", "shared"),
    ("shared", "tenant", "shared"),
    ("dedicated-a", "tenant", "dedicated"),
    ("dedicated-b", "tenant", "dedicated"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub(crate) scheme: &'static str,
    pub(crate) host: &'static str,
    pub(crate) port: u16,
    pub(crate) path: String,
}

pub(crate) fn identifier(value: &str) -> Result<()> {
    if !Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$")?.is_match(value) {
        return Err("计划资源名称无效".into());
    }
    Ok(())
}

pub(crate) fn scope_identifier(value: &str) -> Result<()> {
    if !Regex::new(r"^[a-z0-9][a-z0-9_-]{0,46}[a-z0-9]$")?.is_match(value) {
        return Err("演练 scope 无效".into());
    }
    Ok(())
}

pub(crate) fn timestamp(value: &Value, label: &str) -> Result<DateTime<FixedOffset>> {
    let value = text(value, label)?;
    DateTime::parse_from_rfc3339(value).map_err(|_| format!("{label}不是有效时间或缺少时区").into())
}

pub(crate) fn hash_40(value: &Value, label: &str) -> Result<String> {
    let value = text(value, label)?;
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{label}格式无效").into());
    }
    Ok(value.to_owned())
}

pub(crate) fn endpoint(
    value: &Value,
    label: &str,
    expected_path: Option<&str>,
) -> Result<Endpoint> {
    let url = text(value, label)?;
    if url.contains(['\n', '\r', '\0', '?', '#', '@']) {
        return Err(format!("{label}只接受明确的本机 loopback 端点").into());
    }
    let (scheme, remainder) = url
        .split_once("://")
        .ok_or_else(|| format!("{label}只接受明确的本机 loopback 端点"))?;
    let scheme = if scheme.eq_ignore_ascii_case("http") {
        "http"
    } else if scheme.eq_ignore_ascii_case("https") {
        "https"
    } else {
        return Err(format!("{label}只接受明确的本机 loopback 端点").into());
    };
    let split = remainder.find('/').unwrap_or(remainder.len());
    let authority = &remainder[..split];
    let path = &remainder[split..];
    let (host, port_text) = if let Some(rest) = authority.strip_prefix("[::1]") {
        if rest.is_empty() {
            ("::1", None)
        } else if let Some(port) = rest.strip_prefix(':') {
            ("::1", Some(port))
        } else {
            return Err(format!("{label}端口无效").into());
        }
    } else if authority == "127.0.0.1" {
        ("127.0.0.1", None)
    } else if let Some(port) = authority.strip_prefix("127.0.0.1:") {
        ("127.0.0.1", Some(port))
    } else {
        return Err(format!("{label}只接受明确的本机 loopback 端点").into());
    };
    let explicit = port_text.is_some();
    let port = match port_text {
        Some(value) if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) => {
            return Err(format!("{label}端口无效").into());
        }
        Some(value) => value
            .parse::<u16>()
            .map_err(|_| format!("{label}端口无效"))?,
        None if scheme == "https" => 443,
        None => 80,
    };
    if port == 0 || expected_path.is_some() && (!explicit || path != expected_path.unwrap()) {
        return Err(format!("{label}必须使用显式端口和精确路径").into());
    }
    Ok(Endpoint {
        scheme,
        host,
        port,
        path: path.to_owned(),
    })
}

pub(crate) fn canonical_endpoint(value: &Value, path: &str, label: &str) -> Result<String> {
    let parsed = endpoint(value, label, Some(path))?;
    let authority = if parsed.host.contains(':') {
        format!("[{}]", parsed.host)
    } else {
        parsed.host.to_owned()
    };
    let canonical = format!(
        "{}://{}:{}{}",
        parsed.scheme, authority, parsed.port, parsed.path
    );
    if text(value, label)? != canonical {
        return Err(format!("{label}必须使用规范化 loopback 地址").into());
    }
    Ok(canonical)
}

pub(crate) fn validate_plan(plan: &Value, backend: &Path) -> Result<()> {
    let mut fields = vec![
        "format_version",
        "id",
        "target_side",
        "work_dir",
        "tools",
        "source",
        "target",
    ];
    if plan.get("dataset").is_some() {
        fields.push("dataset");
    }
    let root = exact(plan, &fields, "参考计划")?;
    if root.get("format_version") != Some(&Value::from(1)) {
        return Err("参考演练计划版本无效".into());
    }
    identifier(text(root.get("id").unwrap(), "参考计划 ID")?)?;
    if !matches!(
        root.get("target_side").and_then(Value::as_str),
        Some("base" | "candidate")
    ) {
        return Err("参考恢复目标必须显式选择 base 或 candidate".into());
    }
    if let Some(dataset) = root.get("dataset") {
        let dataset = exact(
            dataset,
            &[
                "timeout_seconds",
                "request_interval_ms",
                "api_validation_posts",
                "post_batch_rows",
                "tenant_targets",
                "records",
                "object_count",
                "object_bytes",
                "admin",
                "owner_password_env",
            ],
            "数据准备计划",
        )?;
        exact(
            dataset.get("admin").unwrap(),
            &["tenant_id", "username", "password_env"],
            "数据准备管理员",
        )?;
        let timeout = integer(dataset.get("timeout_seconds").unwrap(), "数据准备时限")?;
        if !(1..=604_800).contains(&timeout) {
            return Err("数据准备必须显式设置 1 至 604800 秒的阶段时限".into());
        }
    }
    let local = backend.join(".local-tests");
    validate_local_optional(
        Path::new(text(root.get("work_dir").unwrap(), "参考计划工作目录")?),
        &local,
        "演练目录",
    )?;
    validate_tools(root.get("tools").unwrap())?;
    let source = side(root.get("source").unwrap(), "source", backend, &local)?;
    let target = side(root.get("target").unwrap(), "target", backend, &local)?;
    if source.scope == target.scope
        || source.keys != target.keys
        || !source.physical.is_disjoint(&target.physical)
    {
        return Err("源与恢复目标必须同逻辑键、不同 scope 且物理数据库完全分离".into());
    }
    for database in array(
        root.get("source")
            .and_then(|value| value.get("databases"))
            .ok_or("参考计划缺少源数据库")?,
        "源数据库",
    )? {
        let key = text(database.get("key").unwrap(), "源数据库键")?;
        if target.modes.get(key)
            != Some(&(
                text(database.get("kind").unwrap(), "源数据库类型")?,
                text(database.get("mode").unwrap(), "源数据库模式")?,
            ))
        {
            return Err("源与恢复目标的逻辑类型和共享模式必须相同".into());
        }
    }
    Ok(())
}

struct SideIdentity<'a> {
    scope: &'a str,
    keys: HashSet<&'a str>,
    physical: HashSet<(&'a str, String)>,
    modes: HashMap<&'a str, (&'a str, &'a str)>,
}

fn side<'a>(
    value: &'a Value,
    name: &str,
    backend: &Path,
    local: &Path,
) -> Result<SideIdentity<'a>> {
    let fields = if name == "target" {
        vec![
            "scope_id",
            "runtime_dir",
            "api_url",
            "s3",
            "databases",
            "worker_ready_url",
            "frontend_url",
        ]
    } else {
        vec!["scope_id", "runtime_dir", "api_url", "s3", "databases"]
    };
    let side = exact(value, &fields, &format!("参考计划 {name} 侧"))?;
    let scope = text(side.get("scope_id").unwrap(), "演练 scope")?;
    scope_identifier(scope)?;
    let s3 = exact(
        side.get("s3").unwrap(),
        &["endpoint", "region", "configuration"],
        "对象存储定义",
    )?;
    endpoint(s3.get("endpoint").unwrap(), "对象存储端点", None)?;
    crate::recovery::clone::native::s3::Configuration::read_app_binding(
        backend,
        side.get("s3").unwrap(),
    )?;
    endpoint(side.get("api_url").unwrap(), "API 端点", None)?;
    if name == "target" {
        endpoint(side.get("worker_ready_url").unwrap(), "Worker 端点", None)?;
        endpoint(side.get("frontend_url").unwrap(), "前端端点", None)?;
        let api = format!(
            "{}/readyz",
            text(side.get("api_url").unwrap(), "API 端点")?.trim_end_matches('/')
        );
        if Some(api.as_str()) == side.get("worker_ready_url").and_then(Value::as_str) {
            return Err("恢复目标 API 和 Worker 探针不得重叠".into());
        }
    }
    validate_local_optional(
        Path::new(text(side.get("runtime_dir").unwrap(), "运行目录")?),
        local,
        "运行进程收据",
    )?;
    if !Regex::new(r"^[a-z0-9-]+$")?.is_match(text(s3.get("region").unwrap(), "S3 region")?) {
        return Err("S3 region 无效".into());
    }
    let databases = databases(side.get("databases").unwrap(), backend)?;
    Ok(SideIdentity {
        scope,
        keys: databases.keys,
        physical: databases.physical,
        modes: databases.modes,
    })
}

struct Databases<'a> {
    keys: HashSet<&'a str>,
    physical: HashSet<(&'a str, String)>,
    modes: HashMap<&'a str, (&'a str, &'a str)>,
}

fn databases<'a>(value: &'a Value, backend: &Path) -> Result<Databases<'a>> {
    let mut keys = HashSet::new();
    let mut physical = HashSet::new();
    let mut modes = HashMap::new();
    for database in array(value, "参考数据库")? {
        let database = exact(
            database,
            &[
                "key",
                "kind",
                "mode",
                "database",
                "server_uuid",
                "defaults_file",
                "defaults_sha256",
            ],
            "参考数据库",
        )?;
        let key = text(database.get("key").unwrap(), "数据库键")?;
        identifier(key)?;
        let database_name = text(database.get("database").unwrap(), "数据库名")?;
        identifier(database_name)?;
        let server_uuid = text(database.get("server_uuid").unwrap(), "数据库服务器")?;
        identifier(server_uuid)?;
        let kind = text(database.get("kind").unwrap(), "数据库类型")?;
        let mode = text(database.get("mode").unwrap(), "数据库模式")?;
        let identity = (server_uuid, database_name.to_ascii_lowercase());
        if !keys.insert(key)
            || !physical.insert(identity)
            || !matches!(kind, "combined" | "tenant")
            || !matches!(mode, "shared" | "dedicated")
            || kind == "combined" && mode != "shared"
        {
            return Err("逻辑目标、物理数据库重复或类型无效".into());
        }
        let defaults = Path::new(text(
            database.get("defaults_file").unwrap(),
            "MySQL 凭据文件",
        )?);
        validate_local_test_path(defaults, backend, LocalTestPathKind::ExistingFile)
            .map_err(|_| "MySQL 凭据文件必须显式保存在当前 .local-tests")?;
        let expected = digest(database.get("defaults_sha256").unwrap(), "MySQL 凭据摘要")?;
        if file_sha256(defaults)? != expected {
            return Err("MySQL 连接文件与计划摘要不一致".into());
        }
        modes.insert(key, (kind, mode));
    }
    if keys.is_empty()
        || modes
            .values()
            .filter(|(kind, _)| *kind == "combined")
            .count()
            != 1
    {
        return Err("参考环境必须恰有一个 combined 控制目标".into());
    }
    Ok(Databases {
        keys,
        physical,
        modes,
    })
}

fn validate_tools(value: &Value) -> Result<()> {
    let tools = exact(value, &["mysql", "mysqldump"], "参考工具")?;
    for tool in tools.values() {
        let tool = exact(tool, &["path", "sha256"], "参考工具")?;
        let path = PathBuf::from(text(tool.get("path").unwrap(), "工具路径")?);
        validate_external_existing_file(&path).map_err(|error| format!("外部工具无效：{error}"))?;
        if file_sha256(&path)? != digest(tool.get("sha256").unwrap(), "工具摘要")? {
            return Err("外部工具文件不存在或与计划摘要不一致".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_local_optional(value: &Path, boundary: &Path, label: &str) -> Result<()> {
    if !value.is_absolute()
        || value
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("{label}必须是规范绝对路径").into());
    }
    let relative = value
        .strip_prefix(boundary)
        .map_err(|_| format!("{label}必须位于当前后端 .local-tests 的子目录"))?;
    if relative.as_os_str().is_empty() {
        return Err(format!("{label}不能是 .local-tests 根目录").into());
    }
    let metadata = fs::symlink_metadata(boundary).map_err(|_| format!("{label}边界无法核验"))?;
    if !metadata.is_dir() || linked(&metadata) {
        return Err(format!("{label}边界必须是真实目录").into());
    }
    let mut current = boundary.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if linked(&metadata) => {
                return Err(format!("{label}不能经过链接或重解析点").into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(format!("无法核验{label}：{error}").into()),
        }
    }
    Ok(())
}

pub(crate) fn file_sha256(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || linked(&metadata) {
        return Err(format!("恢复核验输入必须是普通文件：{}", path.display()).into());
    }
    Ok(hex::encode(Sha256::digest(fs::read(path)?)))
}

pub(crate) fn file_size(path: &Path) -> Result<u64> {
    Ok(fs::symlink_metadata(path)?.len())
}

pub(crate) fn product_fields() -> [&'static str; 10] {
    [
        "id",
        "backup_id",
        "scope_id",
        "fault_at",
        "databases",
        "object_endpoint",
        "object_prefix",
        "api_ready_url",
        "worker_ready_url",
        "frontend_sha",
    ]
}

pub(crate) fn mode_for_key(key: &str) -> Result<(&'static str, &'static str)> {
    TARGET_DATABASES
        .iter()
        .find_map(|(name, kind, mode)| (*name == key).then_some((*kind, *mode)))
        .ok_or_else(|| "新目标必须精确包含四个逻辑数据库".into())
}
