//! Redis 单独接续继承原 RustFS，并绑定已接受代次的完整清理收据。
use std::path::Path;

use serde_json::{Value, json};

use super::{
    evidence::{INCOMPLETE_REDIS_CLEANUP, bound, check, exact, path_is, read, restart_generation},
    generation,
};
use crate::{
    Result,
    recovery::clone::native::{
        cache::host_control, digest, resources::common::validate_process_identity,
    },
};

pub(super) fn validate(
    run: &Path,
    number: usize,
    item: &Value,
    result: &Value,
    history: &mut Value,
) -> Result<()> {
    let previous = &history["active_generation"];
    check(
        history["closed"] == false
            && history["unsettled"].as_array().is_some_and(Vec::is_empty)
            && ["external_recovery", "restart_predecessor", "rustfs_handoff"]
                .iter()
                .all(|key| history[*key].is_null())
            && item["sources"]["previous_generation"] == *previous
            && result["previous_generation"] == *previous
            && result["status"] == "redis_restarted"
            && result["services"] == json!({"redis":"running","rustfs":"running"})
            && result["next_operation"] == "close",
        "Redis 单独接续未绑定唯一活跃前代或夹具状态不可用",
    )?;
    let predecessor = restart_generation(run, previous)?;
    let root = run.join(format!("lifecycle-{number:04}"));
    check(
        path_is(
            &result["generation"]["path"],
            &root.join("restart-redis.json"),
        ),
        "Redis 单独接续代次收据不属于本阶段",
    )?;
    let proof = bound(run, &result["generation"])?;
    check_generation(run, &root, number, item, result, &proof, &predecessor)?;
    history["active_generation"] = item["result"].clone();
    Ok(())
}

fn check_generation(
    run: &Path,
    root: &Path,
    number: usize,
    item: &Value,
    outer: &Value,
    proof: &Value,
    previous: &Value,
) -> Result<()> {
    exact(
        proof,
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
    )?;
    let redis = &proof["redis"];
    exact(redis, &["request", "runtime"])?;
    check(
        proof["format_version"] == 1
            && proof["kind"] == "reference-fixture-redis-generation"
            && path_is(&proof["run"], run)
            && proof["attempt"] == number
            && proof["owner"] == item["sources"]["state_before"]
            && proof["previous_generation"] == outer["previous_generation"]
            && proof["controller"] == outer["controller"]
            && proof["services"] == outer["services"]
            && proof["rustfs"] == previous["rustfs"]
            && proof["remote_writes"] == 0
            && proof["resources_deleted"] == false
            && path_is(&redis["request"]["path"], &root.join("redis/request.json"))
            && path_is(
                &proof["incident"]["path"],
                &root.join("redis-incident.json"),
            ),
        "Redis 单独接续改变了原 RustFS 绑定，或新代次来源不一致",
    )?;
    let old = &previous["redis"];
    let old_request = bound(run, &old["request"])?;
    let old_runtime = &old["runtime"];
    let backend = crate::workspace::root_dir();
    let incident = host_control::accepted_failure(&backend, &old_request, old_runtime, false)?;
    check(
        (previous["kind"] != "reference-fixture-redis-generation"
            || incident["kind"] == INCOMPLETE_REDIS_CLEANUP)
            && bound(run, &proof["incident"])? == incident,
        "Redis 单独接续未绑定原已确认事故或当前前驱不支持该事故证据",
    )?;
    let original = read(&run.join("redis/request.json"))?;
    let request = bound(run, &redis["request"])?;
    generation::validate_windows_redis(
        run,
        &outer["previous_generation"],
        &original,
        &request,
        &item["sources"]["review"],
    )?;
    runtime(root, &request, &redis["runtime"], old_runtime)
}

fn runtime(root: &Path, request: &Value, current: &Value, previous: &Value) -> Result<()> {
    let redis = &current["redis"];
    let process = &current["process"];
    validate_process_identity(process)?;
    validate_process_identity(&current["supervisor"])?;
    let run_id = redis["run_id"].as_str().ok_or("Redis 新代缺少 run_id")?;
    check(
        current["format_version"] == 3
            && current["request_sha256"] == digest::plan_hash(request)?
            && path_is(&current["output"], &root.join("redis"))
            && read(&root.join("redis/runtime.json"))? == *current
            && *process != previous["process"]
            && current["supervisor"] != previous["supervisor"]
            && current["supervisor"]["pid"] != process["pid"]
            && redis["platform"] == "windows"
            && redis["tool"] == request["tool"]
            && redis["port"] == request["port"]
            && redis["pid"] == process["pid"]
            && redis["started"] == process["started"]
            && redis["run_id"] != previous["redis"]["run_id"]
            && current["facts"]["run_id"] == run_id
            && run_id.len() == 40
            && run_id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "Redis 单独接续缺少当前精确的原配置和新进程身份",
    )
}
