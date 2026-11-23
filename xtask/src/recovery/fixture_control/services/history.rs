//! 参考夹具的追加式历史；只验证证据，不启动服务或重放写入。
use std::path::Path;

use serde_json::{Value, json};

use crate::{Result, recovery::clone::native::lifecycle::state};

#[path = "history/evidence.rs"]
pub(super) mod evidence;
#[path = "history/generation.rs"]
mod generation;
#[path = "history/handoff.rs"]
pub(in crate::recovery) mod handoff;
#[path = "history/prefix.rs"]
pub(super) mod prefix;
#[path = "history/protocol.rs"]
pub(crate) mod protocol;
#[path = "history/redis_successor.rs"]
mod redis_successor;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/services/history/tests.rs"]
mod tests;

pub(super) const INITIAL: [(&str, &str); 3] = [
    ("storage-target", "initial"),
    ("cache-target", "initial"),
    ("fixture-buckets", "prepare"),
];
pub(super) const STAGE: &str = "fixture-services";
use evidence::{bound, check, exact, text};

pub(crate) fn validate_manifest(value: &Value) -> Result<()> {
    exact(
        value,
        &[
            "format_version",
            "kind",
            "review",
            "bootstrap",
            "execution_backend",
            "scope_id",
            "data_directory_was_empty",
        ],
    )?;
    check(
        value["format_version"] == 1
            && value["kind"] == "reference-fixture-service-run"
            && value["execution_backend"].is_string()
            && value["scope_id"].is_string()
            && value["data_directory_was_empty"] == true,
        "夹具服务清单无效",
    )
}

pub(crate) fn validate(run: &Path, value: &Value, ready: bool, settled: bool) -> Result<Value> {
    let attempts = state::attempts(value)?;
    let boundary = attempts
        .iter()
        .position(|item| item["stage"] == STAGE)
        .unwrap_or(attempts.len());
    let mut history = initial_history(attempts, boundary, ready)?;
    let mut prefixes = None;
    for (index, item) in attempts.iter().enumerate().skip(boundary) {
        let mode = text(item, "mode")?;
        check(
            item["stage"] == STAGE
                && [
                    "close",
                    "recover",
                    "reconcile",
                    "restart",
                    "restart-redis",
                    "handoff-rustfs",
                ]
                .contains(&mode),
            "夹具服务生命周期阶段无效",
        )?;
        if item["status"] != "passed" {
            history["unsettled"]
                .as_array_mut()
                .ok_or("历史状态无效")?
                .push(item["number"].clone());
            continue;
        }
        let sources = &item["sources"];
        check(
            sources["manifest"] == value["manifest"],
            "夹具生命周期未绑定原始服务清单",
        )?;
        let descriptor = prefix::Descriptor::read(run, &sources["state_before"])?;
        let snapshot = match &mut prefixes {
            Some(snapshot) => snapshot,
            slot @ None => slot.insert(prefix::Snapshot::open(run)?),
        };
        check(
            snapshot.count(descriptor)? == index,
            "夹具生命周期未绑定完整先行账本",
        )?;
        let result = bound(run, &item["result"])?;
        check(
            result["kind"] == "reference-fixture-service-lifecycle"
                && result["operation"] == mode
                && evidence::path_is(&result["run"], run)
                && result["remote_writes"] == 0
                && result["resources_deleted"] == false,
            "夹具生命周期结果未证明原运行目录及零资源删除",
        )?;
        match mode {
            "close" => close(run, index + 1, item, &result, &mut history)?,
            "handoff-rustfs" => handoff::validate(run, index + 1, item, &result, &mut history)?,
            "recover" => recover(run, index + 1, item, &result, &mut history)?,
            "reconcile" => {
                evidence::reconcile(run, index + 1, attempts, item, &result, &mut history)?
            }
            "restart" => restart(run, index + 1, item, &result, &mut history)?,
            "restart-redis" => {
                redis_successor::validate(run, index + 1, item, &result, &mut history)?;
            }
            _ => unreachable!("已校验生命周期模式"),
        }
    }
    check(
        !settled || history["unsettled"].as_array().is_some_and(Vec::is_empty),
        "夹具服务存在未成功收尾的生命周期阶段",
    )?;
    if let Some(snapshot) = prefixes {
        snapshot.check_end()?;
    }
    Ok(history)
}

fn initial_history(attempts: &[Value], boundary: usize, ready: bool) -> Result<Value> {
    let initial = &attempts[..boundary];
    check(initial.len() <= INITIAL.len(), "夹具首代账本包含多余阶段")?;
    for (item, (stage, mode)) in initial.iter().zip(INITIAL) {
        check(
            item["stage"] == stage && item["mode"] == mode,
            "夹具首代账本包含未知阶段或顺序",
        )?;
    }
    let complete =
        initial.len() == INITIAL.len() && initial.iter().all(|item| item["status"] == "passed");
    check(!ready || complete, "夹具首代服务尚未完整就绪")?;
    Ok(
        json!({"closed":false,"external_recovery":null,"restart_predecessor":null,
        "active_generation":{"kind":"initial","rustfs":attempts.first().map(|item| &item["result"]),
            "redis":attempts.get(1).map(|item| &item["result"])},
        "initial_complete":complete,"unsettled":[]}),
    )
}

pub(in crate::recovery) fn is_published_generation(item: &Value) -> bool {
    item["stage"] == STAGE
        && matches!(item["mode"].as_str(), Some("restart" | "restart-redis"))
        && item["status"] == "passed"
}

fn close(
    run: &Path,
    number: usize,
    item: &Value,
    result: &Value,
    history: &mut Value,
) -> Result<()> {
    check(
        history["unsettled"].as_array().is_some_and(Vec::is_empty),
        "夹具服务未和解失败阶段后不能再次关闭",
    )?;
    check(
        history["external_recovery"].is_null(),
        "夹具外部终止核对后必须重启，不能补造正常关闭",
    )?;
    if history["initial_complete"] != true {
        check(history["closed"] == false, "部分关闭不能重复追加或续建首代")?;
        super::lifecycle::partial::proof::validate(run, number, item, result)?;
        history["closed"] = json!(true);
        return Ok(());
    }
    check(
        history["closed"] == false
            && result["status"] == "services_closed"
            && result["services"] == json!({"redis":"stopped","rustfs":"stopped"}),
        "夹具关闭结果重复或未证明两项服务退出",
    )?;
    exact(&result["evidence"], &["redis", "rustfs"])?;
    for (role, filename) in [("redis", "stopped.json"), ("rustfs", "rustfs-stopped.json")] {
        let path = run.join(format!("lifecycle-{number:04}")).join(filename);
        check(
            evidence::path_is(&result["evidence"][role]["path"], &path),
            "夹具关闭收据不属于本阶段",
        )?;
        let proof = bound(run, &result["evidence"][role]).map_err(|_| {
            Box::new(evidence::HistoryError(
                "夹具关闭收据发生变化、缺失或无法验证",
            )) as Box<dyn std::error::Error>
        })?;
        if role == "redis" {
            check(
                proof["status"] == "redis_process_stopped" && proof["resources_deleted"] == false,
                "Redis 关闭收据未确认精确退出",
            )?;
            let (_, directory) = generation::paths(run, &history["active_generation"])?;
            let runtime_path = directory.join("runtime.json");
            let registered = if runtime_path.try_exists()? {
                evidence::read(&runtime_path)?
            } else {
                Value::Null
            };
            if registered["format_version"] == 3 || proof["runtime"]["format_version"] == 3 {
                check(
                    proof["runtime"] == registered,
                    "Windows Redis 正常关闭收据与当前登记代次不同",
                )?;
                evidence::windows_redis_stop(run, &proof)?;
            } else {
                check(proof["alive"] == false, "Redis 关闭收据未确认精确退出")?;
            }
        } else {
            evidence::members(&proof, run)?;
        }
    }
    history["closed"] = json!(true);
    if history["rustfs_handoff"].is_null() {
        history["restart_predecessor"] = item["result"].clone();
    }
    Ok(())
}

fn recover(
    run: &Path,
    number: usize,
    item: &Value,
    result: &Value,
    history: &mut Value,
) -> Result<()> {
    if result["status"] == "external_termination_reconciled" {
        check(
            history["closed"] == false && history["external_recovery"].is_null(),
            "夹具外部终止不能在关闭后或未重启时重复核对",
        )?;
        let path = run.join(format!("lifecycle-{number:04}/external-termination.json"));
        check(
            result["owner"] == item["sources"]["state_before"]
                && evidence::path_is(&result["evidence"]["path"], &path),
            "夹具外部终止核对未绑定完整先行账本及独立证据",
        )?;
        generation::external(
            &bound(run, &result["evidence"])?,
            run,
            &item["sources"]["state_before"],
            &history["active_generation"],
        )?;
        history["external_recovery"] = item["result"].clone();
        history["restart_predecessor"] = item["result"].clone();
    } else {
        let path = Path::new(text(&result["recovery"], "path")?);
        let name = path
            .file_name()
            .and_then(|item| item.to_str())
            .unwrap_or("");
        check(
            result["status"] == "controller_recovered"
                && path.parent() == Some(run)
                && name.starts_with("recovered-")
                && name.ends_with(".json")
                && bound(run, &result["recovery"])?["status"] == "controller_recovered",
            "夹具控制器恢复结果不完整",
        )?;
    }
    Ok(())
}

fn restart(
    run: &Path,
    number: usize,
    item: &Value,
    result: &Value,
    history: &mut Value,
) -> Result<()> {
    check(
        history["unsettled"].as_array().is_some_and(Vec::is_empty),
        "夹具服务未和解失败阶段后不能重启",
    )?;
    check(
        !history["restart_predecessor"].is_null() && history["rustfs_handoff"].is_null(),
        "夹具服务重启必须紧接明确的正常关闭或外部终止核对",
    )?;
    check(
        item["sources"]["restart_predecessor"] == history["restart_predecessor"],
        "夹具服务重启未绑定当前关闭前驱",
    )?;
    check(
        result["status"] == "services_restarted"
            && result["services"] == json!({"redis":"running","rustfs":"running"}),
        "夹具服务重启未证明两项服务的新运行代次",
    )?;
    let path = run.join(format!("lifecycle-{number:04}/restart.json"));
    check(
        evidence::path_is(&result["generation"]["path"], &path),
        "夹具服务重启代次收据不属于本阶段",
    )?;
    generation::restart(
        &bound(run, &result["generation"])?,
        run,
        number,
        item,
        &history["restart_predecessor"],
        &history["active_generation"],
        &result["controller"],
    )?;
    history["active_generation"] = item["result"].clone();
    history["external_recovery"] = Value::Null;
    history["restart_predecessor"] = Value::Null;
    history["closed"] = json!(false);
    Ok(())
}

pub(crate) fn reconcilable<'a>(value: &'a Value, history: &Value, mode: &str) -> Option<&'a Value> {
    let attempts = value["attempts"].as_array()?;
    let unsettled = history["unsettled"].as_array()?;
    if history["closed"] != false
        || unsettled.len() != 1
        || (mode == "close") != history["external_recovery"].is_null()
    {
        return None;
    }
    let number = usize::try_from(unsettled[0].as_u64()?).ok()?;
    let failed = attempts.get(number.checked_sub(1)?)?;
    if failed["stage"] != STAGE || failed["mode"] != mode || failed["status"] != "failed" {
        return None;
    }
    if mode == "restart" && number != attempts.len() {
        return None;
    }
    if mode == "close"
        && attempts[number..].iter().any(|item| {
            item["stage"] != STAGE || item["mode"] != "recover" || item["status"] != "passed"
        })
    {
        return None;
    }
    Some(failed)
}
