//! 固定相邻失败与重试选择；不推断未知写入可以重放。
use super::*;

fn is_failed(row: &Value) -> bool {
    row["number"].as_u64() == Some(FAILED)
        && row["stage"] == "seed-runtime"
        && row["mode"] == "source-generation-start"
        && row["status"] == "failed"
        && row["result"].is_null()
        && row["error_type"] == "ValueError"
}

pub(super) fn pending(args: &Value) -> Result<Value> {
    document::exact(args, &["attempts", "previous"], "谱系失败选择")?;
    let rows = document::array(&args["attempts"], "谱系历史")?;
    let previous = &args["previous"];
    Ok(json!({"pending":rows.last().is_some_and(is_failed)
        && (previous.is_null() || previous.as_u64() == Some(FAILED-1))}))
}

pub(super) fn history(args: &Value) -> Result<Value> {
    document::exact(args, &["attempts", "archive"], "谱系固定历史")?;
    let rows = document::array(&args["attempts"], "谱系历史")?;
    let recovery = &args["archive"]["recovery"];
    let index = rows
        .iter()
        .position(|row| row == recovery)
        .filter(|_| recovery["number"].as_u64() == Some(60))
        .ok_or("C68 谱系失败缺少固定 C60 封存边界")?;
    let tail = rows
        .get(index + 1..index + 9)
        .ok_or("C68 谱系失败不属于 C60 后的固定相邻资源续作")?;
    let expected = [
        ("cache-target", "stop", "passed", Value::Null),
        ("storage-target", "stop", "failed", json!("ValueError")),
        ("storage-target", "stop", "passed", Value::Null),
        (
            "storage-target",
            "restart",
            "failed",
            json!("CalledProcessError"),
        ),
        ("storage-target", "restart", "passed", Value::Null),
        ("cache-target", "restart", "passed", Value::Null),
        (
            "seed-runtime",
            "source-generation-start",
            "failed",
            json!("ValueError"),
        ),
        (
            "seed-runtime",
            "source-generation-start",
            "failed",
            json!("ValueError"),
        ),
    ];
    for (offset, (row, (stage, mode, status, error))) in tail.iter().zip(expected).enumerate() {
        if row["number"].as_u64() != Some(61 + offset as u64)
            || row["stage"] != stage
            || row["mode"] != mode
            || row["status"] != status
            || row["error_type"] != error
        {
            return Err("C68 谱系失败不属于 C60 后的固定相邻资源续作".into());
        }
        if status == "failed" && !row["result"].is_null() {
            return Err("C68 谱系失败历史包含意外发布结果".into());
        }
        let source = &row["sources"];
        if source["snapshot"]["clean"].as_bool() != Some(true)
            || source["snapshot"]["files"]
                .as_array()
                .is_none_or(|files| !files.is_empty())
            || source["snapshot"]["patch_sha256"]
                != "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            || source["fingerprints"]["product"] != recovery["sources"]["fingerprints"]["product"]
        {
            return Err("C68 续作改变了产品来源或不是干净提交".into());
        }
    }
    Ok(json!({"tail":tail}))
}

pub(super) fn select_source(args: &Value) -> Result<Value> {
    document::exact(args, &["attempts", "request"], "谱系来源选择")?;
    let rows = document::array(&args["attempts"], "谱系历史")?;
    let select = |mode| {
        rows.iter()
            .filter(|row| {
                row["stage"] == "seed-runtime" && row["mode"] == mode && row["status"] == "passed"
            })
            .collect::<Vec<_>>()
    };
    let registrations = select("source-register");
    let rebinds = select("source-rebind");
    if registrations.len() != 1
        || rebinds.len() != 1
        || registrations[0]["result"] != args["request"]["source_registration"]
        || rebinds[0]["result"] != args["request"]["source_rebind"]
    {
        return Err("C68 谱系失败没有继承唯一 C52 与重绑定".into());
    }
    let number = rebinds[0]["number"].as_u64().ok_or("谱系重绑定序号无效")?;
    let prefix: Vec<_> = rows
        .iter()
        .filter(|row| row["number"].as_u64().is_some_and(|n| n < number))
        .collect();
    Ok(json!({"rebind":rebinds[0],"prefix":prefix}))
}

pub(super) fn authorize_stage(args: &Value) -> Result<Value> {
    document::exact(args, &["number", "prefix"], "谱系恢复授权阶段")?;
    let prefix = document::array(&args["prefix"], "谱系前缀")?;
    if args["number"].as_u64() != Some(RECOVERY) || !prefix.last().is_some_and(is_failed) {
        return Err("谱系恢复只接受 C68 后唯一相邻 C69".into());
    }
    Ok(json!({"verified":true}))
}

pub(super) fn authorize_product(args: &Value) -> Result<Value> {
    document::exact(args, &["coordinator", "failed"], "谱系恢复产品来源")?;
    document::object(
        &args["coordinator"]["fingerprints"]["product"],
        "谱系当前产品来源",
    )?;
    document::object(
        &args["failed"]["sources"]["fingerprints"]["product"],
        "谱系失败产品来源",
    )?;
    if args["coordinator"]["fingerprints"]["product"]
        != args["failed"]["sources"]["fingerprints"]["product"]
    {
        return Err("C69 恢复改变了 seed 产品来源".into());
    }
    Ok(json!({"verified":true}))
}

pub(super) fn replay_plan(args: &Value) -> Result<Value> {
    document::exact(args, &["attempts"], "谱系重放授权选择")?;
    let rows = document::array(&args["attempts"], "谱系历史")?;
    let matches: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row["number"].as_u64() == Some(RECOVERY)
                && row["stage"] == "seed-runtime"
                && row["mode"] == "source-generation-recover"
        })
        .collect();
    if matches.is_empty() {
        return Ok(json!({"record":null,"prefix":[],"fallback":false}));
    }
    if matches.len() != 1 {
        return Err("C69 谱系恢复不能重复".into());
    }
    let (position, record) = matches[0];
    if let Some(next) = rows.get(position + 1)
        && (next["number"].as_u64() != Some(RETRY)
            || next["stage"] != "seed-runtime"
            || next["mode"] != "source-generation-start")
    {
        return Err("C69 后首个阶段必须是唯一 C70 seed-runtime/source-generation-start".into());
    }
    let fallback = record["status"] != "passed"
        || !record["error_type"].is_null()
        || record["result"].is_null();
    let prefix: Vec<_> = rows
        .iter()
        .filter(|row| row["number"].as_u64().is_some_and(|n| n < RECOVERY))
        .collect();
    Ok(
        json!({"record":if record["status"] == "running" { &Value::Null } else {record},"prefix":prefix,"fallback":fallback}),
    )
}
