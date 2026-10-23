//! 重绑定的纯阶段判定；实际文件、进程和封存证明仍由受锁采集器提供。
use crate::{
    Result,
    recovery::clone::native::{digest, json as document},
};
use serde_json::{Value, json};

mod history;
pub(super) mod live;
#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/guards/rebind/tests.rs"]
mod tests;

pub(super) fn dispatch(input: &Value) -> Result<Value> {
    document::exact(input, &["action", "input"], "重绑定编排模型")?;
    let args = &input["input"];
    match input["action"].as_str() {
        Some("history") => history::plan(args),
        Some("history-proofs") => history::proofs(args),
        Some("active-read") => active_read(args),
        Some("register-stage") => register_stage(args),
        Some("register-plan") => register_plan(args),
        Some("register-receipt") => receipt(args),
        Some("restart-stage") => restart_stage(args),
        Some("restart-boundary") => restart_boundary(args),
        Some("resolve-plan") => resolve_plan(args),
        Some("resolve-storage") => resolve_storage(args),
        _ => Err("未知重绑定编排操作".into()),
    }
}

fn reject(message: &str) -> Value {
    // 错误仅使用固定中文分类，不回显配置或底层异常。
    json!({"rejected":message})
}

fn attempts(value: &Value) -> Result<&[Value]> {
    let rows = document::array(value, "重绑定历史")?;
    let mut previous = 0;
    for row in rows {
        previous = row["number"]
            .as_u64()
            .filter(|n| *n > previous)
            .ok_or("重绑定历史序号必须严格递增")?;
        document::text(&row["stage"], "重绑定阶段")?;
        document::text(&row["mode"], "重绑定模式")?;
        if !matches!(
            row["status"].as_str(),
            Some("passed" | "failed" | "running")
        ) {
            return Err("重绑定阶段状态无效".into());
        }
    }
    Ok(rows)
}

fn current(value: &Value) -> Result<Option<u64>> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(
        value
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("重绑定当前序号无效")?,
    ))
}

fn operation(row: &Value) -> (&str, &str) {
    (
        row["stage"].as_str().unwrap_or(""),
        row["mode"].as_str().unwrap_or(""),
    )
}

fn active_read(args: &Value) -> Result<Value> {
    document::exact(args, &["attempts"], "重绑定活动读取")?;
    let active: Vec<_> = attempts(&args["attempts"])?
        .iter()
        .filter(|row| row["status"] == "running")
        .collect();
    if active.is_empty() {
        return Ok(json!({"current":null}));
    }
    if active.len() != 1 || !history::read_only(operation(active[0])) {
        return Ok(reject("seed 来源仍有未收尾写入或控制动作"));
    }
    Ok(json!({"current":active[0]["number"]}))
}

fn register_stage(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &["attempts", "number", "source_directory", "run_directory"],
        "重绑定登记阶段",
    )?;
    if args["source_directory"] != args["run_directory"] {
        return Ok(reject("source-rebind 请求的 C52 不属于当前 run"));
    }
    let rows = attempts(&args["attempts"])?;
    let number = current(&args["number"])?.ok_or("重绑定登记缺少序号")?;
    if rows.last().is_none_or(|row| {
        row["number"] != number
            || operation(row) != ("seed-runtime", "source-rebind")
            || row["status"] != "running"
    }) {
        return Ok(reject("source-rebind 必须由同一登记阶段执行"));
    }
    Ok(json!({"prefix":&rows[..rows.len()-1]}))
}

fn register_plan(args: &Value) -> Result<Value> {
    document::exact(args, &["later", "number"], "重绑定登记前缀")?;
    let rows = attempts(&args["later"])?;
    let number = current(&args["number"])?.ok_or("重绑定登记缺少序号")?;
    if rows.iter().any(|row| {
        matches!(row["mode"].as_str(), Some("source-rebind" | "arm-input"))
            && row["number"] != number
    }) {
        return Ok(reject("已重绑定或发布 arm 的来源不能再次重绑定"));
    }
    Ok(json!({"cache":rows.iter().rfind(|row| row["stage"] == "cache-target")}))
}

fn receipt(args: &Value) -> Result<Value> {
    document::exact(args, &["before", "after", "request"], "重绑定双次核验")?;
    if args["before"] != args["after"] {
        return Ok(reject("重绑定期间来源、存储代次或历史变化"));
    }
    let before = &args["before"];
    document::exact(before, &["source", "current", "prefix"], "重绑定观察")?;
    let source = &before["source"];
    let prefix = attempts(&before["prefix"])?;
    super::transition(&source["storage"]["storage"], &before["current"])?;
    Ok(
        json!({"status":"seed_source_rebound", "source_registration":source["review_successor"]["source_result"],
        "review_successor":args["request"], "original_storage":source["storage"]["storage"],
        "current_storage":before["current"], "history_length":prefix.len(),
        "history_sha256":digest::plan_hash(&before["prefix"])?, "remote_writes":0,"restore_qualified":false}),
    )
}

fn restart_stage(args: &Value) -> Result<Value> {
    document::exact(args, &["attempts", "number"], "发布源重启阶段")?;
    let rows = attempts(&args["attempts"])?;
    if let Some(number) = current(&args["number"])?
        && rows.last().is_none_or(|row| {
            row["number"] != number
                || operation(row) != ("storage-target", "restart")
                || row["status"] != "running"
        })
    {
        return Ok(reject("发布源存储重启不是当前登记阶段"));
    }
    if rows
        .iter()
        .any(|row| operation(row) == ("seed-runtime", "source-generation-reboot-close"))
    {
        return Ok(json!({"reboot":true}));
    }
    let record = rows
        .iter()
        .rfind(|row| operation(row) == ("seed-runtime", "source-register"));
    if record.is_none_or(|row| row["status"] != "passed") {
        return Ok(reject("seed 存储重启需要完整发布源"));
    }
    Ok(json!({"reboot":false, "descriptor":record.ok_or("缺少发布源")?["result"]}))
}

fn restart_boundary(args: &Value) -> Result<Value> {
    document::exact(args, &["later", "segment", "number"], "发布源重启边界")?;
    let rows = document::array(&args["later"], "发布源后续阶段")?;
    let phase = args["segment"]["phase"].as_str();
    let phase_ok = if current(&args["number"])?.is_some() {
        phase == Some("storage-running")
    } else {
        matches!(phase, Some("stopped" | "storage-retryable"))
    };
    if rows.iter().any(|row| row["mode"] == "arm-input")
        || rows.iter().any(|row| row["mode"] == "source-rebind") && !phase_ok
    {
        return Ok(reject("已交接来源只能从封存停机边界执行唯一存储重启"));
    }
    Ok(json!({"ready":true}))
}

fn resolve_plan(args: &Value) -> Result<Value> {
    document::exact(args, &["attempts", "later"], "重绑定结果选择")?;
    let rows = attempts(&args["attempts"])?;
    let records: Vec<_> = attempts(&args["later"])?
        .iter()
        .filter(|row| operation(row) == ("seed-runtime", "source-rebind"))
        .collect();
    if records.len() > 1 || records.first().is_some_and(|row| row["status"] != "passed") {
        return Ok(reject("seed 源存在重复、冲突或未收尾重绑定"));
    }
    let Some(row) = records.first() else {
        return Ok(json!({"record":null,"prefix":[]}));
    };
    let prefix: Vec<_> = rows
        .iter()
        .filter(|item| item["number"].as_u64() < row["number"].as_u64())
        .collect();
    Ok(json!({"record":row,"prefix":prefix}))
}

fn resolve_storage(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "original",
            "rebound",
            "registered",
            "segment",
            "live",
            "current",
        ],
        "重绑定有效代次",
    )?;
    let mut effective = args["original"].clone();
    if !args["rebound"].is_null() {
        effective = args["rebound"].clone();
        if args["segment"].is_null() {
            if args["registered"] != effective {
                return Ok(reject("重绑定后存储代次变化"));
            }
        } else {
            if args["segment"]["storage"].is_null() {
                return Ok(reject("封存来源已停机，必须先完成同请求资源重启"));
            }
            effective = args["segment"]["storage"].clone();
        }
    }
    if document::boolean(&args["live"], "实时存储核验")? {
        if !args["segment"].is_null() && args["segment"]["phase"] != "ready" {
            return Ok(reject("封存来源的 Redis 尚未完成同请求重启"));
        }
        if args["current"] != effective {
            return Ok(reject("seed 源 RustFS 已停止、重启或缺少显式重绑定"));
        }
    }
    Ok(json!({"effective":effective}))
}
