use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};

use super::super::super::{
    digest,
    input::{digest_shape, name},
    json::{self as json_io, exact, text},
    lifecycle::manifest,
};
use super::request::KEYS;
use crate::Result;

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
    if review["kind"].as_str() != Some("review-only-perf-resource-plan-with-readonly-preflight")
        || review["ready_for_execution"].as_bool() != Some(true)
    {
        return Err("审阅计划类型错误或尚未就绪".into());
    }
    let scopes = exact(
        root.get("scopes").ok_or("审阅计划缺少资源")?,
        &["seed", "base", "candidate"],
        "审阅三侧资源",
    )?;
    let tools = json_io::object(root.get("tools").ok_or("审阅计划缺少工具")?, "审阅工具")?;
    for role in ["mysql", "mysqldump", "rustfs"] {
        let tool = tools.get(role).ok_or("审阅计划工具定义缺失")?;
        text(&tool["path"], "审阅工具路径")?;
        digest_shape(&tool["sha256"])?;
    }
    for key in ["distribution", "resolved_path", "sha256"] {
        text(&review["tools"]["redis_server"][key], "审阅 Redis 工具")?;
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
    isolation(review)
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

fn isolation(review: &Value) -> Result<()> {
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
    if count != 12 || names.len() != 12 {
        return Err("三侧数据库相互重叠或碰到参考资源".into());
    }
    for role in ["source", "protected_target"] {
        for database in json_io::array(&review["reference"][role]["databases"], "保护数据库")?
        {
            let identity = (
                text(&database["server_uuid"], "保护数据库 UUID")?,
                text(&database["database"], "保护数据库名称")?.to_ascii_lowercase(),
            );
            if names.contains(&identity) {
                return Err("三侧数据库相互重叠或碰到参考资源".into());
            }
        }
    }
    Ok(())
}
