//! 只消费已经过服务账本历史重核的 Redis 单独接续。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Evidence, Restart, check_redis_successor, common, document};
use crate::Result;

pub(super) fn select<'a>(
    attempts: &'a [Value],
    third: Option<&Value>,
) -> Result<Option<&'a Value>> {
    let mut accepted = attempts.iter().filter(|row| {
        row["stage"] == "fixture-services"
            && row["mode"] == "restart-redis"
            && row["status"] == "passed"
    });
    let Some(row) = accepted.next() else {
        return Ok(None);
    };
    let number = row["number"].as_u64();
    let preceding = third.and_then(|row| row["number"].as_u64());
    if accepted.next().is_some()
        || !matches!((preceding, number), (Some(previous), Some(current)) if previous < current)
    {
        return Err("Redis 单独接续必须唯一且晚于第三次完整重启".into());
    }
    Ok(Some(row))
}

pub(super) fn link(
    evidence: &mut Evidence<'_>,
    run: &Path,
    initial: &Value,
    third: &Restart<'_>,
    row: &Value,
    active: &Restart<'_>,
) -> Result<Value> {
    let phase = lifecycle(run, &row["number"])?;
    check_generation(run, &phase, third, row, active)?;
    let descriptor = &active.generation["incident"];
    require_path(descriptor, &phase.join("redis-incident.json"))?;
    let proof = evidence.read(descriptor, "已登记 Redis 失败接续证明")?;
    document::exact(
        &proof,
        &["kind", "runtime", "bindings", "windows_closed"],
        "Redis 失败接续证明",
    )?;
    let bindings = document::object(&proof["bindings"], "原 Redis 失败文件")?;
    let original = &third.generation["redis"]["runtime"];
    let original_directory = lifecycle(run, &third.generation["attempt"])?.join("redis");
    require_path(&proof["runtime"], &original_directory.join("runtime.json"))?;
    if proof["kind"] != "windows-redis-accepted-failure"
        || !proof["windows_closed"].is_object()
        || bindings.get("redis_runtime.json") != Some(&proof["runtime"])
        || evidence.read(&proof["runtime"], "第三次重启的原 Redis 运行收据")? != *original
    {
        return Err("Redis 失败接续证明不同于第三次重启原代次".into());
    }
    for (name, binding) in bindings {
        let file = name
            .strip_prefix("redis_")
            .ok_or("原 Redis 失败文件名无效")?;
        if Path::new(file).file_name().and_then(|name| name.to_str()) != Some(file) {
            return Err("原 Redis 失败文件名无效".into());
        }
        require_path(binding, &original_directory.join(file))?;
        evidence.read(binding, "原 Redis 失败证据文件")?;
    }
    let current = &active.generation["redis"]["runtime"];
    let runtime_path = phase.join("redis/runtime.json");
    let runtime = common::binding(&runtime_path)?;
    if evidence.read(&runtime, "Redis 单独接续的新运行收据")? != *current {
        return Err("Redis 单独接续的新运行收据与已登记代次不同".into());
    }
    check_redis_successor(initial, &current["redis"], original, &active.request)?;
    Ok(json!({
        "previous_restart":third.result,
        "previous_generation":third.receipt,
        "previous_runtime":proof["runtime"],
        "incident":descriptor,
        "current_request":active.generation["redis"]["request"],
        "current_runtime":runtime,
    }))
}

pub(super) fn check_generation(
    run: &Path,
    phase: &Path,
    third: &Restart<'_>,
    row: &Value,
    active: &Restart<'_>,
) -> Result<()> {
    let outer = &active.outer;
    let generation = &active.generation;
    let services = json!({"redis":"running","rustfs":"running"});
    let expected = json!({
        "format_version":1,"kind":"reference-fixture-service-lifecycle",
        "operation":"restart-redis","status":"redis_restarted","run":outer["run"],
        "remote_writes":0,"resources_deleted":false,
        "controller":generation["controller"],"services":services,
        "generation":active.receipt,"previous_generation":third.result,"next_operation":"close",
    });
    document::exact(
        generation,
        &[
            "format_version",
            "kind",
            "run",
            "attempt",
            "owner",
            "previous_generation",
            "controller",
            "services",
            "rustfs",
            "redis",
            "incident",
            "remote_writes",
            "resources_deleted",
        ],
        "Redis 单独接续代次",
    )?;
    let redis = &generation["redis"];
    document::exact(redis, &["request", "runtime"], "新 Redis 代次")?;
    require_path(&active.receipt, &phase.join("restart-redis.json"))?;
    require_path(&redis["request"], &phase.join("redis/request.json"))?;
    if *outer != expected
        || Path::new(document::text(&outer["run"], "Redis 单独接续结果的账本")?) != run
        || generation["format_version"] != 1
        || generation["kind"] != "reference-fixture-redis-generation"
        || Path::new(document::text(&generation["run"], "Redis 单独接续账本")?) != run
        || generation["attempt"] != row["number"]
        || generation["owner"] != row["sources"]["state_before"]
        || generation["previous_generation"] != *third.result
        || generation["services"] != services
        || generation["rustfs"] != third.generation["rustfs"]
        || generation["remote_writes"] != 0
        || generation["resources_deleted"] != false
        || Path::new(document::text(
            &redis["runtime"]["output"],
            "新 Redis 目录",
        )?) != phase.join("redis")
    {
        return Err("Redis 单独接续未直接绑定第三次重启及其仍保持的 RustFS".into());
    }
    Ok(())
}

fn lifecycle(run: &Path, number: &Value) -> Result<PathBuf> {
    let number = number
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or("缺少原夹具阶段序号")?;
    Ok(run.join(format!("lifecycle-{number:04}")))
}

fn require_path(binding: &Value, expected: &Path) -> Result<()> {
    if Path::new(document::text(&binding["path"], "Redis 接续证据路径")?) != expected {
        return Err("Redis 接续证据路径不同于原账本登记代次".into());
    }
    Ok(())
}
