//! 初始化的文档、精确 reset 原文及文件配置约束；此模块不执行资源写入。
use std::path::Path;

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    super::{digest, json as json_io},
    evidence_model::{bound, check, read, timestamp},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};

const DIAGNOSTICS: &[&str] = &[
    "实际 reset plan 原文或 hash 缺失、重复、不一致",
    "所选配置文件的 reset 必须是配置表",
    "所选配置文件的 reset.source_sha 与维护构建提交不一致",
    "所选配置文件的 reset.services_stopped 必须为 true",
    "所选配置文件缺少 reset.state_directory",
    "reset.state_directory 必须精确指向本次初始化的 reset-state 目录",
    "fresh 目标文件配置无法读取；请检查 APP_ENV 及执行树 config/app.<环境>.toml，不通过环境变量注入其他配置",
    "fresh 目标已出现业务数据",
    "外部新租户库 fence 或空槽行不符",
    "fresh 目标库存缺少明确数据库或行数；请核对完整只读库存，不自动初始化",
];

pub(super) fn diagnostic(reason: &str) -> Option<&'static str> {
    DIAGNOSTICS
        .iter()
        .copied()
        .find(|message| *message == reason)
}

pub(super) fn execute(
    backend: &Path,
    target: &Path,
    operation: &str,
    args: &Value,
) -> Result<Value> {
    match operation {
        "prepare-document" | "prepare-current" => {
            prepare_document(backend, target, args, operation == "prepare-current")
        }
        "reset-plan-output" => {
            json_io::exact(args, &["text", "request", "original"], "reset 原文参数")?;
            let value = reset_output(json_io::text(&args["text"], "reset 原文")?)?;
            super::evidence_reset::validate_manifest(
                backend,
                &value["manifest"],
                &args["request"],
                &args["original"],
            )?;
            Ok(value)
        }
        "reset-execution-configuration" => configuration(target, args),
        "initial-inventory-shape" => {
            json_io::exact(args, &["databases", "images"], "fresh 库存参数")?;
            inventory(&args["databases"], &args["images"])?;
            Ok(json!({"status":"fresh_inventory_verified","remote_writes":0}))
        }
        _ => Err("未知目标 CLI 证据操作；请使用当前受管工具".into()),
    }
}

fn prepare_document(backend: &Path, target: &Path, args: &Value, current: bool) -> Result<Value> {
    let mut fields = vec!["at", "request_descriptor", "generation", "absence"];
    if current {
        fields.push("observed");
    }
    json_io::exact(args, &fields, "prepare 文档参数")?;
    timestamp(&args["at"])?;
    let descriptor = &args["request_descriptor"];
    let request = read(backend, &bound(backend, descriptor)?)?;
    let intents = json_io::array(&request["target"]["databases"], "fresh 数据库")?
        .iter()
        .map(
            |db| json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]}),
        )
        .collect::<Vec<_>>();
    let expected = json!({"format_version":1,"status":"fresh_creation_prepared","prepared_at":args["at"],
        "request":descriptor,"request_sha256":digest::plan_hash(&request)?,"generation":args["generation"],
        "absence":args["absence"],"fresh_target_initialized":false,"controlled_generation_only":true,
        "external_writers_discovered":false,"create_intents":intents});
    if !current {
        return Ok(expected);
    }
    check(
        read(backend, &target.join("prepare.json"))? == args["observed"]
            && args["observed"] == expected,
        "fresh 目标已发布 prepare 与当前只读前像不同",
    )?;
    Ok(json!({"status":"prepared_verified","remote_writes":0}))
}

fn reset_output(text: &str) -> Result<Value> {
    let text = text.replace("\r\n", "\n");
    let pattern = Regex::new(r"\A(\{[\s\S]*\})\nplan_hash=([a-f0-9]{64})\n?\z")?;
    let captures = pattern.captures(&text).ok_or(DIAGNOSTICS[0])?;
    check(
        hex::encode(Sha256::digest(captures[1].as_bytes())) == captures[2],
        DIAGNOSTICS[0],
    )?;
    let manifest = json_io::parse_fragment(&captures[1], "reset 原始文档")?;
    Ok(json!({"manifest":manifest,"plan_hash":&captures[2]}))
}

fn configuration(target: &Path, args: &Value) -> Result<Value> {
    json_io::exact(
        args,
        &["execution_backend", "app_env", "maintenance"],
        "reset 文件配置参数",
    )?;
    let backend = Path::new(json_io::text(&args["execution_backend"], "reset 执行树")?);
    validate_absolute_path(backend, LocalTestPathKind::ExistingDirectory)?;
    let environment = json_io::text(&args["app_env"], "APP_ENV")?;
    let request = json_io::read(&target.join("request.json"))?;
    let snapshot = super::fresh::initialize::reset_configuration::Snapshot::open(
        backend,
        target,
        &request,
        &args["maintenance"],
    )?;
    let table = crate::rate_limit::load_app_table(backend, Some(&snapshot.directory), environment)
        .map_err(|_| DIAGNOSTICS[6])?;
    let table = serde_json::to_value(table)?;
    let result = reset_configuration(target, &table, &args["maintenance"]);
    snapshot.check()?;
    result
}

pub(crate) fn reset_configuration(
    target: &Path,
    table: &Value,
    maintenance: &Value,
) -> Result<Value> {
    let reset = table
        .get("reset")
        .and_then(Value::as_object)
        .ok_or(DIAGNOSTICS[1])?;
    let source =
        json_io::text(&maintenance["source"]["snapshot"]["head"], "维护提交")?.to_ascii_lowercase();
    check(
        reset.get("source_sha").and_then(Value::as_str) == Some(source.as_str()),
        DIAGNOSTICS[2],
    )?;
    check(
        reset.get("services_stopped").and_then(Value::as_bool) == Some(true),
        DIAGNOSTICS[3],
    )?;
    let state = reset
        .get("state_directory")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or(DIAGNOSTICS[4])?;
    let state = Path::new(state);
    validate_absolute_path(state, LocalTestPathKind::StateDirectory).map_err(|_| DIAGNOSTICS[5])?;
    check(state == target.join("reset-state"), DIAGNOSTICS[5])?;
    Ok(json!({"path":state}))
}

fn inventory(databases: &Value, images: &Value) -> Result<()> {
    for db in json_io::array(databases, "fresh 数据库")? {
        let database = json_io::text(&db["database"], "fresh 库存数据库")?;
        let tables = json_io::object(&images[database]["tables"], "fresh 库存表")?;
        for (table, value) in tables {
            let rows = value["rows"].as_u64().ok_or(DIAGNOSTICS[9])?;
            check(
                !table.starts_with("biz_")
                    || ["biz_tenant_target_slot", "biz_tenant_fence"].contains(&table.as_str())
                    || rows == 0,
                DIAGNOSTICS[7],
            )?;
        }
        if db["kind"] != "combined" {
            check(
                tables
                    .get("biz_tenant_fence")
                    .and_then(|value| value["rows"].as_u64())
                    == Some(0)
                    && tables
                        .get("biz_tenant_target_slot")
                        .and_then(|value| value["rows"].as_u64())
                        == Some(1),
                DIAGNOSTICS[8],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/target_verify/creation_plan/tests.rs"]
mod tests;
