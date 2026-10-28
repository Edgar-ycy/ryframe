//! 重启段只消费现有收尾和登记证据；不签发收尾、不启动或停止任何资源。
use super::{completed, stops};
use crate::recovery::source::evidence::ready;
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        cache, json as document,
        resources::{common, storage::registered},
    },
};
use serde_json::{Value, json};
use std::path::Path;
#[path = "reboot/host.rs"]
mod host;
#[path = "reboot/observation.rs"]
mod observation;
pub(in crate::recovery::source::evidence) use observation::request;

const CLOSE: &str = "source-generation-reboot-close";

pub(super) fn tail(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    tail: &[Value],
    archive: &Value,
    descriptor: &Value,
) -> Result<Option<Value>> {
    let closures = attempts
        .iter()
        .filter(|row| row["stage"] == "seed-runtime" && row["mode"] == CLOSE)
        .collect::<Vec<_>>();
    if closures.is_empty() {
        return Ok(None);
    }
    let published = closures
        .iter()
        .copied()
        .filter(|row| row["status"] == "passed" && !row["result"].is_null())
        .collect::<Vec<_>>();
    if published.len() != 1
        || closures
            .iter()
            .any(|row| !published.contains(row) && row["status"] != "failed")
    {
        return Err("主机重启收尾不能重复、失败或未收尾".into());
    }
    let authority = completed::read(backend, run, attempts)?
        .ok_or("存在重启收尾收据但缺少已收尾的 C70 源代次")?;
    let start = &authority["completed_successor"];
    let start_number = start["number"].as_u64().ok_or("C70 源代次序号无效")?;
    let rows = tail
        .iter()
        .filter(|row| row["number"].as_u64().is_some_and(|n| n > start_number))
        .cloned()
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Ok(None);
    }
    let shapes = [
        ("seed-runtime", CLOSE),
        ("storage-target", "restart"),
        ("cache-target", "restart"),
    ];
    if rows.len() != shapes.len() {
        return Err("主机重启收尾后的目标存储与缓存重启不完整".into());
    }
    for (offset, (row, (stage, mode))) in rows.iter().zip(shapes).enumerate() {
        if row["number"].as_u64() != start_number.checked_add(u64::try_from(offset + 1)?)
            || row["stage"] != stage
            || row["mode"] != mode
            || row["status"] != "passed"
        {
            return Err("主机重启收尾后的目标存储与缓存重启不完整或不相邻".into());
        }
    }
    let observed = observation::read(backend, run, attempts, start)?;
    observation::require_closed(&observed)?;
    let issued = closure(backend, run, published[0], start)?;
    if published[0]["number"] != rows[0]["number"] {
        return Err("重启段没有采用当前唯一收尾收据".into());
    }
    Ok(Some(restarted(
        backend, run, &rows, archive, descriptor, &issued,
    )?))
}

fn restarted(
    backend: &Path,
    run: &Path,
    rows: &[Value],
    archive: &Value,
    descriptor: &Value,
    issued: &Value,
) -> Result<Value> {
    let before = rows[2]["number"]
        .as_u64()
        .and_then(|n| n.checked_add(1))
        .ok_or("重启段缓存序号溢出")?;
    let storage = registered::registered(backend, run, CloneSide::Target, Some(before))?
        .unwrap_or(Value::Null);
    if storage["attempt"] != rows[1]["number"] || storage["restart_result"] != rows[1]["result"] {
        return Err("收尾后的对象存储重启没有绑定同一登记代次".into());
    }
    let cache = cache::binding::query(
        backend,
        run,
        None,
        &json!(before),
        &json!([descriptor, archive["receipt"]["review_successor"]]),
    )?;
    if cache["attempt"] != rows[2]["number"] || cache["restart_result"] != rows[2]["result"] {
        return Err("收尾后的缓存重启没有绑定同一登记代次".into());
    }
    common::bound_file(backend, &issued["binding"], "重启收尾")?;
    Ok(
        json!({"records":rows,"storage":storage,"cache":cache,"closure":issued["binding"],
        "baseline":issued["receipt"]["baseline"],"deltas":issued["receipt"]["declared_deltas"]}),
    )
}

fn closure(backend: &Path, run: &Path, record: &Value, start: &Value) -> Result<Value> {
    if record["stage"] != "seed-runtime"
        || record["mode"] != CLOSE
        || record["status"] != "passed"
        || !record["error_type"].is_null()
    {
        return Err("重启收尾必须为原账本唯一成功阶段".into());
    }
    let result = stops::result(backend, run, record)?;
    document::exact(
        &result,
        &[
            "status",
            "start",
            "request",
            "closure",
            "baseline",
            "reboot",
            "failed_prefix",
            "declared_deltas",
            "producer",
            "verification_qualified",
            "replay_allowed",
            "remote_writes",
            "restore_qualified",
        ],
        "重启收尾结果",
    )?;
    if result["status"] != "seed_source_generation_reboot_closed" {
        return Err("主机重启收尾阶段结果状态无效".into());
    }
    if common::bound_file(backend, &result["closure"], "重启收尾")?
        != run.join("g0070/reboot-close/closure.json")
    {
        return Err("主机重启收尾收据不属于固定代次目录".into());
    }
    let value = common::read_bound(backend, &result["closure"], "重启收尾")?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "status",
            "start",
            "request",
            "failed_prefix",
            "reboot",
            "roles",
            "storage",
            "cache",
            "baseline",
            "running",
            "declared_deltas",
            "verification_qualified",
            "replay_allowed",
            "remote_writes",
            "restore_qualified",
        ],
        "重启收尾",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "seed-source-generation-reboot-closure"
        || value["status"] != "seed_source_generation_reboot_closed"
        || value["verification_qualified"] != false
        || value["replay_allowed"] != false
        || value["remote_writes"].as_u64() != Some(0)
        || value["restore_qualified"] != false
        || value["declared_deltas"]
            != json!({"sys_login_info":11,"sys_oper_log":0,"sys_outbox_event":0})
    {
        return Err("主机重启收尾收据的状态、计数或声明增量无效".into());
    }
    closure_contract(backend, run, start, &result, &value)?;
    common::bound_file(backend, &record["result"], "重启收尾阶段后像")?;
    common::bound_file(backend, &result["closure"], "重启收尾内容后像")?;
    Ok(json!({"binding":result["closure"],"result":record["result"],"receipt":value}))
}

/// 历史收尾证明不重新应用当前采集的新鲜度；当前主机事实由 observation 另行核验。
fn closure_contract(
    backend: &Path,
    run: &Path,
    start: &Value,
    result: &Value,
    value: &Value,
) -> Result<()> {
    let generations = document::array(&value["cache"]["generations"], "重启收尾缓存代次")?;
    if generations.is_empty() || generations.iter().any(|row| row["state"] != "stopped") {
        return Err("主机重启收尾收据没有记录已完成退出的缓存代次".into());
    }
    let original = stops::result(backend, run, start)?;
    if start["status"] != "passed"
        || value["start"] != start["result"]
        || value["request"] != original["request"]
        || value["baseline"] != original["before"]
        || value["running"] != original["running"]
        || result["producer"] != value["failed_prefix"]["producer"]
    {
        return Err("重启收尾没有绑定同一原 START、请求及原始像".into());
    }
    for key in [
        "status",
        "start",
        "request",
        "baseline",
        "reboot",
        "failed_prefix",
        "declared_deltas",
        "verification_qualified",
        "replay_allowed",
        "remote_writes",
        "restore_qualified",
    ] {
        if result[key] != value[key] {
            return Err("重启收尾内外重复字段不一致".into());
        }
    }
    common::bound_file(backend, &value["request"], "重启收尾原请求")?;
    Ok(())
}
