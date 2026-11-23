use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};

use super::super::super::{
    digest,
    input::{digest_shape, name},
    json::{self as json_io, exact, text},
    lifecycle::manifest,
    resources::common::endpoint,
};
use super::request::KEYS;
use crate::{Result, recovery::clone::native::cache::windows_tool};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Comparison,
    B1RuntimeMeasurement,
}

impl Kind {
    const MEASUREMENT: &str = "review-only-b1-runtime-measurement-with-readonly-preflight";

    pub(crate) fn read(review: &Value) -> Result<Self> {
        match review["kind"].as_str() {
            Some("review-only-perf-resource-plan-with-readonly-preflight") => Ok(Self::Comparison),
            Some(Self::MEASUREMENT) => Ok(Self::B1RuntimeMeasurement),
            _ => Err("审阅计划类型无效".into()),
        }
    }

    pub(crate) fn is_measurement(review: &Value) -> bool {
        review["kind"] == Self::MEASUREMENT
    }

    fn sides(self) -> &'static [&'static str] {
        match self {
            Self::Comparison => &["seed", "base", "candidate"],
            Self::B1RuntimeMeasurement => &["candidate"],
        }
    }

    pub(crate) fn service_side(self) -> &'static str {
        match self {
            Self::Comparison => "seed",
            Self::B1RuntimeMeasurement => "candidate",
        }
    }
}

pub(crate) fn read(backend: &Path, descriptor: &Value) -> Result<Value> {
    let binding = exact(
        descriptor,
        &["path", "bytes", "sha256", "canonical_sha256"],
        "审阅计划绑定",
    )?;
    let descriptor =
        json!({"path": binding["path"], "bytes": binding["bytes"], "sha256": binding["sha256"]});
    let path = manifest::bound_file(backend, &descriptor, "审阅计划", true)?;
    let review = json_io::read(&path)?;
    validate(&review)?;
    if digest::plan_hash(&review)? != digest_shape(&binding["canonical_sha256"])? {
        return Err("审阅计划规范摘要不匹配".into());
    }
    manifest::bound_file(backend, &descriptor, "审阅计划", true)?;
    Ok(review)
}

pub(crate) fn validate(review: &Value) -> Result<()> {
    let root = json_io::object(review, "审阅计划")?;
    let reference = json_io::object(
        root.get("reference").ok_or("审阅计划缺少保护资源")?,
        "审阅保护资源",
    )?;
    let kind = Kind::read(review).map_err(|_| "审阅计划类型错误或尚未就绪")?;
    if review["ready_for_execution"].as_bool() != Some(true) {
        return Err("审阅计划类型错误或尚未就绪".into());
    }
    let scopes = exact(
        root.get("scopes").ok_or("审阅计划缺少资源")?,
        kind.sides(),
        match kind {
            Kind::Comparison => "审阅三侧资源",
            Kind::B1RuntimeMeasurement => "B1 测量审阅 candidate 资源",
        },
    )?;
    let tools = json_io::object(root.get("tools").ok_or("审阅计划缺少工具")?, "审阅工具")?;
    for role in ["mysql", "mysqldump", "rustfs"] {
        let tool = tools.get(role).ok_or("审阅计划工具定义缺失")?;
        text(&tool["path"], "审阅工具路径")?;
        digest_shape(&tool["sha256"])?;
    }
    let redis = &review["tools"]["redis_server"];
    if kind == Kind::B1RuntimeMeasurement && redis["platform"] != "windows" {
        return Err("B1 测量审阅必须绑定 Windows Redis".into());
    }
    if redis["platform"] == "windows" {
        if ["wsl", "native_tool", "redis_python"]
            .iter()
            .any(|key| tools.contains_key(*key))
        {
            return Err("Windows Redis 审阅不能混用 WSL 或 Linux Redis 工具绑定".into());
        }
        windows_tool::validate(redis)?;
    } else {
        for key in ["distribution", "resolved_path", "sha256"] {
            text(&redis[key], "审阅 Redis 工具")?;
        }
    }
    let services = json_io::object(
        root.get("services").ok_or("审阅计划服务定义缺失")?,
        "审阅服务",
    )?;
    for key in ["api", "console", "data_dir"] {
        text(
            &services.get("rustfs").ok_or("审阅 RustFS 缺失")?[key],
            "审阅 RustFS 服务",
        )?;
    }
    text(
        &services.get("redis").ok_or("审阅 Redis 服务缺失")?["directory"],
        "审阅 Redis 服务目录",
    )?;
    for role in ["source", "protected_target"] {
        json_io::array(
            &reference.get(role).ok_or("审阅保护资源缺失")?["databases"],
            "审阅保护数据库",
        )?;
    }
    for side in scopes.values() {
        validate_side(side)?;
    }
    if kind == Kind::B1RuntimeMeasurement {
        validate_measurement_resources(review)?;
    }
    isolation(review, kind)
}

fn validate_measurement_resources(review: &Value) -> Result<()> {
    if review["sources"].get("reconciled_seed").is_some() {
        return Err("B1 测量审阅不得附带原 seed 接续".into());
    }
    let candidate = &review["scopes"]["candidate"];
    let services = &review["services"];
    let root = Path::new(text(&review["future_root"], "B1 测量运行根")?);
    for (value, name) in [
        (&candidate["runtime_dir"], "runtime-candidate"),
        (&candidate["identity_ledger"], "identities-candidate"),
        (&services["rustfs"]["data_dir"], "rustfs"),
        (&services["redis"]["directory"], "redis"),
    ] {
        if !root.is_absolute() || Path::new(text(value, "B1 测量资源目录")?) != root.join(name)
        {
            return Err("B1 测量资源目录必须归属于同一独立运行根".into());
        }
    }
    let redis = text(&services["redis"]["endpoint"], "B1 测量 Redis 端点")?;
    let worker = text(&candidate["worker_ready_url"], "B1 测量 Worker 就绪地址")?
        .strip_suffix("/readyz")
        .ok_or("B1 测量 Worker 必须登记 /readyz")?;
    let redis_address = format!("http://{redis}");
    let mut ports = BTreeSet::new();
    for address in [
        text(&candidate["api_url"], "B1 测量 API 地址")?,
        worker,
        text(&candidate["frontend_url"], "B1 测量前端地址")?,
        text(&services["rustfs"]["api"], "B1 测量 RustFS API 地址")?,
        text(&services["rustfs"]["console"], "B1 测量 RustFS 控制台地址")?,
        &redis_address,
    ] {
        let port = endpoint(address)?.0;
        if port < 1024 || !ports.insert(port) {
            return Err("B1 测量服务必须使用互不重叠的本机端口".into());
        }
    }
    let scope = text(&candidate["scope_id"], "B1 测量 scope")?;
    let namespace = format!("ryframe:{{{scope}}}:");
    if candidate["objects"]["endpoint"] != services["rustfs"]["api"]
        || candidate["redis"]["url"] != format!("redis://{redis}/0")
        || candidate["redis"]["namespace"] != namespace
        || candidate["redis"]["ownership_key"] != format!("{namespace}.ryframe-owner")
        || candidate["redis"]["ownership_value"] != format!("ryframe-owner:v1:{scope}:redis")
    {
        return Err("B1 测量对象或 Redis 必须绑定相同的隔离服务和归属".into());
    }
    let databases = json_io::array(&candidate["databases"], "B1 测量数据库")?;
    let first = &databases[0];
    let mysql_port = first["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok());
    if !mysql_port.is_some_and(|port| port >= 1024 && ports.insert(port))
        || databases.iter().any(|database| {
            database["host"] != "127.0.0.1"
                || database["port"] != first["port"]
                || database["expected_server_uuid"] != first["expected_server_uuid"]
        })
        || ["source", "protected_target"]
            .iter()
            .any(|role| review["reference"][*role]["scope_id"] == scope)
    {
        return Err("B1 测量必须使用独立 scope 和本机独立 MySQL 实例的四个库".into());
    }
    Ok(())
}

fn validate_side(side: &Value) -> Result<()> {
    json_io::object(side, "审阅单侧资源")?;
    name(text(&side["scope_id"], "审阅 scope")?)?;
    for key in [
        "runtime_dir",
        "identity_ledger",
        "api_url",
        "worker_ready_url",
        "frontend_url",
    ] {
        text(&side[key], "审阅运行路径")?;
    }
    for key in ["endpoint", "region"] {
        text(&side["objects"][key], "审阅对象存储")?;
    }
    for key in ["url", "namespace", "ownership_key", "ownership_value"] {
        text(&side["redis"][key], "审阅 Redis")?;
    }
    let databases = json_io::array(&side["databases"], "审阅数据库")?;
    let mut keys = BTreeSet::new();
    for database in databases {
        json_io::object(database, "审阅数据库")?;
        if !keys.insert(text(&database["key"], "审阅数据库 key")?) {
            return Err("审阅数据库 key 重复".into());
        }
        text(&database["database"], "审阅数据库名称")?;
        text(&database["expected_server_uuid"], "审阅数据库身份")?;
    }
    if databases.len() != 4 || keys != KEYS.iter().map(|(key, _, _)| *key).collect() {
        return Err("审阅计划必须精确包含四个逻辑数据库".into());
    }
    Ok(())
}

fn isolation(review: &Value, kind: Kind) -> Result<()> {
    let mut names = BTreeSet::new();
    let mut count = 0;
    for side in json_io::object(&review["scopes"], "审阅资源")?.values() {
        for database in json_io::array(&side["databases"], "审阅数据库")? {
            count += 1;
            names.insert((
                text(&database["expected_server_uuid"], "数据库 UUID")?,
                text(&database["database"], "数据库名称")?.to_ascii_lowercase(),
            ));
        }
    }
    let expected = kind.sides().len() * KEYS.len();
    let message = match kind {
        Kind::Comparison => "三侧数据库相互重叠或碰到参考资源",
        Kind::B1RuntimeMeasurement => "B1 测量数据库相互重叠或碰到参考资源",
    };
    if count != expected || names.len() != expected {
        return Err(message.into());
    }
    for role in ["source", "protected_target"] {
        for database in json_io::array(&review["reference"][role]["databases"], "保护数据库")?
        {
            let identity = (
                text(&database["server_uuid"], "保护数据库 UUID")?,
                text(&database["database"], "保护数据库名称")?.to_ascii_lowercase(),
            );
            if names.contains(&identity) {
                return Err(message.into());
            }
        }
    }
    Ok(())
}
