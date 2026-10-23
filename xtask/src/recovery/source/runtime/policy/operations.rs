//! 来源运行适配计划；约束数据库选择、查询边界、代次和完整像，不执行外部写入。
use super::{documents, model};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn control_database(args: &Value) -> Result<Value> {
    documents::exact(args, &["databases"], "来源控制数据库参数")?;
    let databases = documents::array(&args["databases"], "来源数据库")?;
    let matches: Vec<_> = databases
        .iter()
        .filter(|value| value["key"] == "shared-control")
        .collect();
    if matches.len() != 1 || matches[0]["kind"] != "combined" {
        return Err("来源验收缺少唯一控制数据库".into());
    }
    Ok(matches[0].clone())
}

pub(super) fn paths(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "start", "output"], "来源运行路径参数")?;
    let backend = Path::new(documents::text(&args["backend"], "来源后端")?);
    let start = Path::new(documents::text(&args["start"], "来源代次文件")?);
    let output = Path::new(documents::text(&args["output"], "来源运行输出")?);
    validate_local_test_path(start, backend, LocalTestPathKind::ExistingFile)?;
    // 静态预检不拒绝已经存在的收据，恢复和普通执行各自检查阶段状态。
    validate_local_test_path(output, backend, LocalTestPathKind::ExistingFile)
        .or_else(|_| validate_local_test_path(output, backend, LocalTestPathKind::OutputFile))?;
    if output
        .file_name()
        .is_none_or(|name| name != "source-runtime.json")
        || output
            .parent()
            .and_then(Path::file_name)
            .is_none_or(|name| name != "verification")
    {
        return Err("来源运行收据必须使用同代 verification/source-runtime.json".into());
    }
    Ok(json!({"start":start,"output":output}))
}

pub(super) fn query(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["database", "selection", "upper_id"],
        "来源登录查询参数",
    )?;
    let database = documents::text(&args["database"], "来源控制库名")?;
    crate::recovery::inputs::domain::identifier(database)?;
    let condition = match args["selection"].as_str() {
        Some("all") if args["upper_id"].is_null() => String::new(),
        Some(selection @ ("old" | "new")) => {
            let upper = args["upper_id"].as_u64().ok_or("来源登录主键上界无效")?;
            format!(
                "WHERE `id` {} {upper}",
                if selection == "old" { "<=" } else { ">" }
            )
        }
        _ => return Err("来源登录查询范围无效".into()),
    };
    Ok(json!(format!(
        "SELECT CAST(`id` AS CHAR),HEX(`tenant_id`),HEX(`user_name`),HEX(`ipaddr`),IFNULL(HEX(`login_location`),'NULL'),IFNULL(HEX(`browser`),'NULL'),IFNULL(HEX(`os`),'NULL'),HEX(`status`),IFNULL(HEX(`msg`),'NULL'),DATE_FORMAT(`login_time`,'%Y-%m-%dT%H:%i:%s.%f') FROM `{database}`.`sys_login_info` {condition} ORDER BY `id`;"
    )))
}

pub(super) fn rows(args: &Value) -> Result<Value> {
    documents::exact(args, &["raw"], "来源登录行证据参数")?;
    let raw = documents::text(&args["raw"], "来源登录原始行")?;
    let rows = model::login_rows(args)?;
    let upper = documents::array(&rows, "来源登录行")?
        .last()
        .map(|row| -> Result<u64> {
            documents::text(&row["id"], "来源登录主键")?
                .parse::<u64>()
                .map_err(Into::into)
        })
        .transpose()?
        .unwrap_or(0);
    Ok(
        json!({"hex":hex::encode(format!("{raw}{}",if raw.is_empty() {""} else {"\n"})),"upper_id":upper}),
    )
}

pub(super) fn image(args: &Value) -> Result<Value> {
    documents::exact(args, &["before", "expected"], "来源运行前像参数")?;
    if args["before"] != args["expected"] {
        return Err("来源验收开始或恢复前完整像与同一运行 start 不同".into());
    }
    Ok(Value::Null)
}

pub(super) fn facts_binding(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["directory", "generation", "receipt", "lineage"],
        "来源运行代次绑定",
    )?;
    let directory = Path::new(documents::text(&args["directory"], "来源运行目录")?);
    let generation = Path::new(documents::text(&args["generation"], "来源代次目录")?);
    if directory != generation.join("verification")
        || args["receipt"]["dataset_lineage"] != args["lineage"]
    {
        return Err("来源运行收据没有绑定同一 generation 与数据血缘".into());
    }
    Ok(Value::Null)
}

pub(super) fn effects(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["cleanup", "effects", "recovered"],
        "来源缓存副作用参数",
    )?;
    let recovered = args["recovered"].as_bool().ok_or("来源恢复标志无效")?;
    let cleanup = &args["cleanup"];
    if recovered {
        if documents::array(&cleanup["after"], "来源缓存清理后像")?.len() != 33 {
            return Err("来源验收恢复缓存清理没有完整解释 33 个计划键".into());
        }
    } else {
        let deleted = cleanup["deleted"].as_u64().ok_or("来源缓存删除计数无效")?;
        let expired =
            documents::array(&cleanup["expired_snapshot_keys"], "来源过期快照")?.len() as u64;
        if deleted.checked_add(expired).is_none_or(|n| {
            Some(n) != args["effects"]["authorization_cache"]["created_and_removed_keys"].as_u64()
        }) {
            return Err("来源验收授权缓存清理与完整前后像合同不同".into());
        }
    }
    Ok(Value::Null)
}
