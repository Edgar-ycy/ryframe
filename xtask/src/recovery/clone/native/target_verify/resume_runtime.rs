//! 从已发布重启结果与完整夹具账本重新派生运行代次。
use super::{
    super::{digest, json as json_io},
    evidence_model::*,
};
use crate::Result;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

pub(super) fn fixture(storage: &Value, cache: &Value) -> Result<Value> {
    check(
        [storage, cache]
            .iter()
            .all(|item| item.is_null() || item.is_object()),
        "迁移续作的存储恢复代次无效",
    )?;
    let flags = [storage, cache].map(|item| item.get("generation").is_some());
    if flags == [false, false] {
        return Ok(Value::Null);
    }
    check(
        flags == [true, true] && storage["generation"] == cache["generation"],
        "夹具存储与缓存恢复不是同一服务代次",
    )?;
    json_io::exact(
        storage,
        &[
            "storage",
            "data_directory",
            "api_url",
            "console_url",
            "generation",
        ],
        "夹具存储代次",
    )?;
    json_io::exact(cache, &["redis", "generation"], "夹具缓存代次")?;
    json_io::exact(
        &storage["generation"],
        &["path", "bytes", "sha256"],
        "夹具重启绑定",
    )?;
    Ok(storage["generation"].clone())
}

pub(super) fn started(backend: &Path, expected: &Value, started: &Value) -> Result<()> {
    let storage = &started["storage_runtime"];
    let cache = &started["cache_runtime"];
    let generation = fixture(storage, cache)?;
    if generation.is_null() {
        registered(backend, &expected["storage"], storage, cache)?;
    } else {
        let (actual_storage, actual_cache) =
            fixture_evidence(backend, &expected["storage"], &generation)?;
        check(
            *storage == actual_storage && *cache == actual_cache,
            "迁移续作 started 的存储代次不同于可信发布证据",
        )?;
    }
    super::storage::generation::validate_transition(
        &expected["storage"],
        &started["storage"],
        (!storage.is_null()).then_some(storage),
        (!cache.is_null()).then_some(cache),
    )?;
    let mut actual = expected.clone();
    actual["storage"] = started["storage"].clone();
    check(
        !(actual == *expected && (!storage.is_null() || !cache.is_null())),
        "存储恢复证据没有产生新的实际进程代次",
    )?;
    check(
        started["current_generation_sha256"] == digest::plan_hash(&actual)?,
        "迁移续作 started 未绑定允许的当前存储代次",
    )
}

fn registered(backend: &Path, expected: &Value, storage: &Value, cache: &Value) -> Result<()> {
    if !storage.is_null() {
        json_io::exact(
            storage,
            &[
                "request",
                "storage",
                "attempt",
                "data_directory",
                "api_url",
                "console_url",
                "restart_result",
            ],
            "存储重启",
        )?;
        let request = read(backend, &bound(backend, &storage["request"])?)?;
        let result = read(backend, &bound(backend, &storage["restart_result"])?)?;
        check(
            ["identity", "process_receipt", "launch_receipt"]
                .iter()
                .all(|key| request["previous"][*key] == expected["rustfs"][*key])
                && request["executable"]["sha256"] == expected["rustfs"]["sha256"],
            "RustFS 重启请求没有绑定原 fresh 代次",
        )?;
        check(
            result["status"] == "storage_restarted"
                && result["side"] == "target"
                && [
                    "request",
                    "storage",
                    "attempt",
                    "data_directory",
                    "api_url",
                    "console_url",
                ]
                .iter()
                .all(|key| result[*key] == storage[*key]),
            "RustFS started 证据不同于已发布重启结果",
        )?;
        bound(backend, &result["ready"])?;
    }
    if !cache.is_null() {
        json_io::exact(
            cache,
            &[
                "request",
                "runtime",
                "redis",
                "owner",
                "controller",
                "attempt",
                "ready",
                "restart_result",
            ],
            "缓存重启",
        )?;
        let request = read(backend, &bound(backend, &cache["request"])?)?;
        let result = read(backend, &bound(backend, &cache["restart_result"])?)?;
        check(
            request["previous"] == expected["redis"],
            "Redis 重启请求没有绑定原 fresh 代次",
        )?;
        check(
            result["status"] == "cache_ready"
                && [
                    "request",
                    "runtime",
                    "redis",
                    "owner",
                    "controller",
                    "attempt",
                    "ready",
                ]
                .iter()
                .all(|key| result[*key] == cache[*key]),
            "Redis started 证据不同于已发布重启结果",
        )?;
        for key in ["runtime", "owner", "controller", "ready"] {
            bound(backend, &cache[key])?;
        }
    }
    Ok(())
}

fn storage_projection(value: &Value) -> Value {
    json!({"identity":value["identity"],"sha256":value["sha256"],"process_receipt":value["process_receipt"],"launch_receipt":value["launch_receipt"]})
}

fn fixture_evidence(
    backend: &Path,
    expected: &Value,
    generation: &Value,
) -> Result<(Value, Value)> {
    use crate::recovery::{clone::native::lifecycle::state, fixture_control::services::history};
    let result_path = bound(backend, generation)?;
    check(
        result_path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "results"),
        "夹具服务代次结果不属于统一账本",
    )?;
    let run = result_path
        .parent()
        .and_then(Path::parent)
        .ok_or("夹具账本目录无效")?;
    let manifest = read(backend, &run.join("manifest.json"))?;
    history::validate_manifest(&manifest)?;
    let state = state::load(run, true)?;
    let history = history::validate(run, &state, true, false)?;
    let attempts = state::attempts(&state)?;
    let matches = attempts
        .iter()
        .filter(|item| {
            item["stage"] == "fixture-services"
                && item["mode"] == "restart"
                && item["status"] == "passed"
                && item["result"] == *generation
        })
        .count();
    check(
        matches == 1 && history["active_generation"] == *generation,
        "夹具服务代次不是唯一成功且当前活跃 restart",
    )?;
    let outer = read(backend, &result_path)?;
    let active = read(backend, &bound(backend, &outer["generation"])?)?;
    let request = read(backend, &bound(backend, &active["rustfs"]["request"])?)?;
    let storage = json!({"storage":storage_projection(&active["rustfs"]["storage"]),"data_directory":request["data_directory"],"api_url":request["api_url"],"console_url":request["console_url"],"generation":generation});
    let cache = json!({"redis":active["redis"]["runtime"]["redis"],"generation":generation});
    let mut known = Vec::new();
    let initial_storage = read(
        backend,
        &bound(backend, &attempts.first().ok_or("夹具首代缺失")?["result"])?,
    )?;
    let initial_cache = read(
        backend,
        &bound(
            backend,
            &attempts.get(1).ok_or("夹具缓存首代缺失")?["result"],
        )?,
    )?;
    known.push(json!({"storage":storage_projection(&initial_storage),"redis":initial_cache["runtime"]["redis"]}));
    for item in attempts.iter().filter(|item| {
        item["stage"] == "fixture-services"
            && item["mode"] == "restart"
            && item["status"] == "passed"
    }) {
        let result = read(backend, &bound(backend, &item["result"])?)?;
        let proof = read(backend, &bound(backend, &result["generation"])?)?;
        known.push(json!({"storage":storage_projection(&proof["rustfs"]["storage"]),"redis":proof["redis"]["runtime"]["redis"]}));
    }
    let expected = json!({"storage":expected["rustfs"],"redis":expected["redis"]});
    check(
        known.contains(&expected),
        "夹具服务重启 lineage 未绑定 fresh-target 的原始或已登记服务代次",
    )?;
    Ok((storage, cache))
}

pub(super) struct Markers {
    pub(super) confirmation: Value,
    pub(super) files: BTreeSet<String>,
    pub(super) at: Value,
}

pub(super) fn markers(
    backend: &Path,
    target: &Path,
    request: &Value,
    attempt: &str,
    resume: &Path,
    started_path: &Path,
    started: &Value,
) -> Result<Markers> {
    let generation = fixture(&started["storage_runtime"], &started["cache_runtime"])?;
    let paths = names(target, r"^resume-redis-markers-.*\.json$")?;
    if generation.is_null() {
        check(paths.is_empty(), "非夹具重启代次出现 Redis 标记重绑证据")?;
        return Ok(Markers {
            confirmation: Value::Null,
            files: BTreeSet::new(),
            at: started["at"].clone(),
        });
    }
    let intent_name = format!("resume-redis-markers-{attempt}.intent.json");
    let confirmed_name = format!("resume-redis-markers-{attempt}.confirmed.json");
    let files = BTreeSet::from([intent_name.clone(), confirmed_name.clone()]);
    check(paths == files, "夹具重启缺少唯一 Redis 标记重绑确认")?;
    let intent = read(backend, &target.join(&intent_name))?;
    let confirmed = read(backend, &target.join(&confirmed_name))?;
    json_io::exact(
        &intent,
        &[
            "format_version",
            "kind",
            "attempt",
            "at",
            "resume",
            "started",
            "restart_generation",
            "markers",
            "before",
        ],
        "标记 intent",
    )?;
    json_io::exact(
        &confirmed,
        &[
            "format_version",
            "kind",
            "attempt",
            "at",
            "intent",
            "restart_generation",
            "after",
            "atomic_result",
            "remote_write_operations",
            "unknown_result",
        ],
        "标记 confirmed",
    )?;
    let selected = super::resume_model::selected(backend, request)?;
    let redis = &selected["redis"];
    let markers = json!({"namespace":redis["namespace"],"ownership_key":redis["ownership_key"],"ownership_value":redis["ownership_value"],"sentinel_key":request["reset"]["sentinel_key"],"sentinel_value":request["reset"]["sentinel_value"]});
    let before = json!({"keys":[],"owner":null,"sentinel":null});
    let after = json!({"keys":[redis["ownership_key"]],"owner":redis["ownership_value"],"sentinel":request["reset"]["sentinel_value"]});
    check(
        intent["format_version"] == 1
            && intent["kind"] == "devex-clone-redis-marker-rebind-intent"
            && intent["attempt"] == attempt
            && intent["resume"] == binding(backend, resume)?
            && intent["started"] == binding(backend, started_path)?
            && intent["restart_generation"] == generation
            && intent["markers"] == markers
            && intent["before"] == before
            && started["resources_before"]["redis"] == before
            && confirmed["format_version"] == 1
            && confirmed["kind"] == "devex-clone-redis-marker-rebind-confirmed"
            && confirmed["attempt"] == attempt
            && confirmed["intent"] == binding(backend, &target.join(&intent_name))?
            && confirmed["restart_generation"] == generation
            && confirmed["after"] == after
            && confirmed["atomic_result"].as_u64() == Some(2)
            && confirmed["remote_write_operations"].as_u64() == Some(1)
            && confirmed["unknown_result"] == false
            && ordered(&[&started["at"], &intent["at"], &confirmed["at"]]),
        "Redis 标记重绑证据内容或因果顺序无效",
    )?;
    Ok(Markers {
        confirmation: binding(backend, &target.join(&confirmed_name))?,
        files,
        at: confirmed["at"].clone(),
    })
}
