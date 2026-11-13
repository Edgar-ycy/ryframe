//! 参考恢复协调、文件证据与阶段收据；只读检查仅观察绑定资源。
use std::path::{Component, Path};

use serde_json::Value;

use crate::{
    Result,
    recovery::{clone::native, inputs},
};

mod backup;
mod dataset;
mod files;
mod local;
mod model;
mod plan;
mod proof;
mod protocol;
pub(crate) mod readonly;
mod receipts;
mod restore;
#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/source/evidence/reference_workflow/tests.rs"]
mod tests;

pub(crate) fn run_local(raw: &str) -> Result<Value> {
    local::run(raw)
}

pub(crate) fn handles(operation: &str) -> bool {
    matches!(
        operation,
        "reference-request"
            | "reference-readonly"
            | "reference-work-directory"
            | "reference-work-verify"
            | "reference-copy"
            | "reference-damage"
            | "reference-receipt-start"
            | "reference-receipt-finish"
            | "reference-summary"
            | "reference-backup-source"
            | "reference-backup-result"
    )
}

pub(crate) fn request(operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "reference-request" => protocol::request(args),
        "reference-readonly" => readonly::execute(args),
        "reference-work-directory" => files::work(args, true),
        "reference-work-verify" => files::work(args, false),
        "reference-copy" => files::copy(args),
        "reference-damage" => files::damage(args),
        "reference-receipt-start" => receipts::start(args),
        "reference-receipt-finish" => receipts::finish(args),
        "reference-summary" => model::summary(args),
        "reference-backup-source" => proof::backup_source(args),
        "reference-backup-result" => proof::backup_result(args),
        _ => Err("参考协调操作无效".into()),
    }
}

fn exact<'a>(value: &'a Value, fields: &[&str]) -> Result<&'a serde_json::Map<String, Value>> {
    native::json::exact(value, fields, "参考协调参数")
}

fn text(value: &Value) -> Result<&str> {
    native::json::text(value, "参考协调字段")
}

fn path(value: &Value) -> Result<&Path> {
    let source = text(value)?;
    let path = Path::new(source);
    if source.is_empty()
        || source.chars().any(char::is_control)
        || !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("参考协调路径必须是无父目录跳转和控制字符的绝对路径".into());
    }
    Ok(path)
}

fn unlinked(path: &Path) -> Result<()> {
    if native::digest::linked_path(path)?
        .into_iter()
        .any(|linked| linked)
    {
        return Err("参考协调路径不能经过链接或junction".into());
    }
    Ok(())
}

fn artifact_shape(manifest: &Value) -> Result<()> {
    for database in native::json::array(&manifest["databases"], "备份数据库")? {
        text(&database["key"])?;
    }
    native::json::array(&manifest["artifacts"], "备份产物")?;
    Ok(())
}

fn inventory_shape(plan: &Value, inventory: &Value) -> Result<()> {
    for field in ["id", "scope_id", "quiesced_at", "captured_at"] {
        text(&inventory[field])?;
    }
    text(&plan["id"])?;
    text(&plan["source"]["scope_id"])?;
    for database in native::json::array(&plan["source"]["databases"], "来源数据库")? {
        for field in ["key", "database", "server_uuid", "kind", "mode"] {
            text(&database[field])?;
        }
    }
    for database in native::json::array(&inventory["databases"], "库存数据库")? {
        text(&database["key"])?;
    }
    for objects in native::json::array(&inventory["objects"], "库存对象")? {
        text(&objects["bucket"])?;
    }
    Ok(())
}
