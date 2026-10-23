//! 创建阶段的精确证据闭集与初始化必需路径。
use super::{super::json as json_io, evidence_model::*};
use crate::Result;
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, path::Path};

pub(crate) fn creation(backend: &Path, target: &Path, request: &Value) -> Result<Value> {
    let mut result = Map::new();
    for db in json_io::array(&request["target"]["databases"], "fresh 数据库")? {
        let stage = format!("create-{}", json_io::text(&db["key"], "数据库 key")?);
        let resource =
            json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]});
        let observed = json!({"server_uuid":db["server_uuid"],"database":db["database"],"exists":true,"empty_checked":true});
        result.insert(
            stage.clone(),
            json!({
            "intent":stage_record(backend,target,&stage,"intent",&resource)?,
            "confirmed":stage_record(backend,target,&stage,"confirmed",&observed)?}),
        );
    }
    result.insert("sentinel".into(), json!({
        "intent":stage_record(backend,target,"sentinel","intent",&json!({"key":request["reset"]["sentinel_key"]}))?,
        "confirmed":stage_record(backend,target,"sentinel","confirmed",&json!({"keys":[],"owner":null,"sentinel":request["reset"]["sentinel_value"]}))?}));
    let expected: BTreeSet<_> = result
        .keys()
        .flat_map(|stage| {
            [
                format!("{stage}.intent.json"),
                format!("{stage}.confirmed.json"),
            ]
        })
        .collect();
    check(
        names(target, r"^(create-|sentinel).*\.(intent|confirmed)\.json$")? == expected,
        "fresh 创建阶段包含缺失、重复命名或未知证据",
    )?;
    Ok(Value::Object(result))
}

pub(crate) fn initial_stages(request: &Value) -> Result<BTreeSet<String>> {
    let mut stages = vec!["sentinel".to_owned(), "reset".to_owned()];
    for db in json_io::array(&request["target"]["databases"], "fresh 数据库")? {
        stages.push(format!(
            "create-{}",
            json_io::text(&db["key"], "数据库 key")?
        ));
    }
    Ok(stages
        .into_iter()
        .flat_map(|stage| {
            [
                format!("{stage}.intent.json"),
                format!("{stage}.confirmed.json"),
            ]
        })
        .collect())
}

pub(crate) fn stage_names(target: &Path, expected: &BTreeSet<String>) -> Result<()> {
    check(
        names(target, r"^.*\.(intent|confirmed)\.json$")? == *expected,
        "fresh 初始化包含未登记的 intent 或 confirmed 证据",
    )
}

pub(crate) fn paths(
    target: &Path,
    creation: &Value,
    reset: &Value,
    prefix: &Value,
) -> Result<BTreeSet<String>> {
    let mut result: BTreeSet<_> = [
        "copy-target.guard",
        "failure.json",
        "initialize.started.json",
        "reset-state",
    ]
    .map(str::to_owned)
    .into();
    let mut descriptors = Vec::new();
    for stage in json_io::object(creation, "创建聚合")?.values() {
        descriptors.extend(json_io::object(stage, "创建阶段")?.values());
    }
    descriptors.extend([
        &reset["plan"],
        &reset["plan_confirm"],
        &reset["intent"],
        &reset["confirmed"],
    ]);
    descriptors.extend(json_io::object(&reset["commands"], "reset 命令聚合")?.values());
    descriptors.extend(
        json_io::array(prefix, "迁移前缀")?
            .iter()
            .map(|item| &item["receipt"]),
    );
    for item in descriptors {
        let path = Path::new(json_io::text(&item["path"], "初始化阶段路径")?);
        let relative = path
            .strip_prefix(target)
            .map_err(|_| "初始化阶段路径不属于当前目标")?;
        result.insert(
            relative
                .to_str()
                .ok_or("初始化阶段路径无效")?
                .replace('\\', "/"),
        );
    }
    for field in ["report", "ledger"] {
        let name = json_io::text(&reset["completed"][field]["file"], "reset 完成路径")?;
        let path = Path::new(name);
        check(
            !path.is_absolute()
                && !name.contains(':')
                && !name
                    .replace('\\', "/")
                    .split('/')
                    .any(|part| matches!(part, "" | "." | "..")),
            "reset 完成路径越界",
        )?;
        result.insert(name.replace('\\', "/"));
    }
    let ledger = Path::new(json_io::text(
        &reset["completed"]["ledger"]["file"],
        "reset 账本路径",
    )?);
    let parent = ledger.parent().ok_or("reset 账本缺少父目录")?;
    let file = ledger
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("reset 账本文件名无效")?;
    result.insert(
        parent
            .join(format!(".{file}.previous"))
            .to_str()
            .ok_or("reset 前像路径无效")?
            .replace('\\', "/"),
    );
    Ok(result)
}
