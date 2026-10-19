use std::path::Path;

use chrono::DateTime;
use serde_json::{Value, json};

use super::super::super::{
    digest,
    json::{self as json_io, exact, text},
    lifecycle::manifest,
};
use crate::Result;

const GENERATION_FIELDS: &[&str] = &[
    "execution",
    "maintenance",
    "physical",
    "configuration",
    "configuration_sha256",
    "seed_credentials_sha256",
    "request_sha256",
    "review_sha256",
    "selected",
    "operator_declared_controlled_generation_only",
    "external_writers_discovered",
    "storage",
];

pub(crate) fn validate_prepare(
    backend: &Path,
    target: &Path,
    prepared: &Value,
    initialized: &Value,
    request: &Value,
) -> Result<()> {
    exact(
        prepared,
        &[
            "format_version",
            "status",
            "prepared_at",
            "request",
            "request_sha256",
            "generation",
            "absence",
            "fresh_target_initialized",
            "controlled_generation_only",
            "external_writers_discovered",
            "create_intents",
        ],
        "目标 prepare",
    )?;
    let generation = exact(
        &prepared["generation"],
        GENERATION_FIELDS,
        "prepare generation",
    )?;
    exact(
        &initialized["generation"],
        GENERATION_FIELDS,
        "initialized generation",
    )?;
    let hash = digest::plan_hash(request)?;
    if prepared["format_version"].as_u64() != Some(1)
        || prepared["status"].as_str() != Some("fresh_creation_prepared")
        || prepared["request_sha256"].as_str() != Some(hash.as_str())
        || prepared["fresh_target_initialized"].as_bool() != Some(false)
        || prepared["controlled_generation_only"].as_bool() != Some(true)
        || prepared["external_writers_discovered"].as_bool() != Some(false)
        || prepared["generation"]["storage"] != request["storage"]
    {
        return Err("prepare 声明、请求摘要或初始存储绑定无效".into());
    }
    let prepared_at = timestamp(&prepared["prepared_at"])?;
    let completed_at = timestamp(&initialized["completed_at"])?;
    let started = json_io::read(&target.join("initialize.started.json"))?;
    exact(&started, &["at", "generation_sha256"], "初始化开始记录")?;
    let started_at = timestamp(&started["at"])?;
    if prepared_at > started_at
        || started_at > completed_at
        || started["generation_sha256"].as_str()
            != Some(digest::plan_hash(&prepared["generation"])?.as_str())
    {
        return Err("prepare、初始化开始或完成的静态因果绑定无效".into());
    }
    // 存储重启的谱系由显式存储观察器校验；其余 generation 字段必须保持不变。
    for (field, expected) in generation {
        if field != "storage" && initialized["generation"].get(field) != Some(expected) {
            return Err(format!("prepare 与 initialized generation 不同：{field}").into());
        }
    }
    if generation["operator_declared_controlled_generation_only"].as_bool() != Some(true)
        || generation["external_writers_discovered"].as_bool() != Some(false)
    {
        return Err("generation 受控资格声明无效".into());
    }
    let maintenance_path =
        manifest::bound_file(backend, &request["maintenance_build"], "目标维护构建", true)?;
    if generation["maintenance"] != json_io::read(&maintenance_path)? {
        return Err("prepare generation 维护构建与请求绑定不同".into());
    }
    validate_absence(prepared, request)
}

fn validate_absence(prepared: &Value, request: &Value) -> Result<()> {
    let databases = json_io::array(&request["target"]["databases"], "请求数据库")?;
    let intents = databases.iter().map(|database| json!({"key": database["key"], "server_uuid": database["server_uuid"], "database": database["database"]})).collect::<Vec<_>>();
    let databases = databases.iter().map(|database| json!({"server_uuid": database["server_uuid"], "database": database["database"], "exists": false, "empty_checked": false})).collect::<Vec<_>>();
    let objects = ["uploads", "avatar", "exports", "imports", "config-packages"]
        .into_iter()
        .map(|bucket| (bucket.to_owned(), json!({"keys": []})))
        .collect::<serde_json::Map<_, _>>();
    if prepared["create_intents"] != json!(intents)
        || prepared["absence"]
            != json!({"databases": databases, "objects": objects, "redis": {"keys": [], "owner": null, "sentinel": null}})
    {
        return Err("prepare 创建声明或精确空资源前像无效".into());
    }
    Ok(())
}

fn timestamp(value: &Value) -> Result<DateTime<chrono::FixedOffset>> {
    DateTime::parse_from_rfc3339(text(value, "目标证据时间")?)
        .map_err(|_| "目标证据时间无效".into())
}
