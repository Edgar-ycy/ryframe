//! 复制及恢复前后像只读取原目标已登记的 Redis 代次。
use std::{collections::BTreeMap, path::Path};

use serde_json::Value;

use super::{StorageInputs, SystemObserver, common, json_io, protocol, redis_layout};
use crate::{Result, recovery::clone::native::lifecycle::state};

pub(super) fn initialized(backend: &Path, target: &Path, value: &Value) -> Result<Value> {
    let descriptor = &value["request"];
    let request = common::read_bound(backend, descriptor, "目标 Redis 原请求")?;
    super::super::published::request_binding(backend, &request)?;
    let run = value["storage_run"]
        .as_str()
        .map(|path| common::local_directory(backend, Path::new(path), "目标 Redis 运行账本"))
        .transpose()?;
    if run.is_none() && !value["storage_run"].is_null() {
        return Err("目标 Redis 运行账本必须是本机目录或 null".into());
    }
    let original = &request["storage"];
    redis_layout::require_windows(&original["redis"])?;
    let cache = &value["cache_runtime"];
    let before = registration(backend, run.as_deref())?;
    let selected = redis_generation(backend, run.as_deref(), original, cache, before.as_ref())?;
    if selected != value["redis"] {
        return Err("目标 Redis 读取不属于调用前已经核定的代次".into());
    }
    let environment = BTreeMap::from([("APP_ENV".to_owned(), "test".to_owned())]);
    let inputs = StorageInputs {
        backend,
        request: &request,
        environment: &environment,
        observation_dir: target,
    };
    let observer = SystemObserver;
    let observed =
        super::super::redis::state_with_observer(&inputs, &selected, true, true, &observer);
    let after = (|| {
        if registration(backend, run.as_deref())? != before
            || common::read_bound(backend, descriptor, "目标 Redis 原请求")? != request
            || redis_generation(backend, run.as_deref(), original, cache, before.as_ref())?
                != selected
        {
            return Err("目标 Redis 读取期间请求或已登记代次发生变化".into());
        }
        Ok(())
    })();
    super::super::execution::combine(observed, after)
}

fn registration(backend: &Path, run: Option<&Path>) -> Result<Option<[Value; 2]>> {
    run.map(|run| {
        let manifest = common::common_file(backend, &run.join("manifest.json"), "目标运行清单")?;
        let ledger = common::common_file(backend, &run.join("state.json"), "目标运行账本")?;
        Ok([common::binding(&manifest)?, common::binding(&ledger)?])
    })
    .transpose()
}

fn redis_generation(
    backend: &Path,
    run: Option<&Path>,
    original: &Value,
    cache: &Value,
    registration: Option<&[Value; 2]>,
) -> Result<Value> {
    let Some((run, [manifest, ledger])) = run.zip(registration) else {
        if !cache.is_null() {
            return Err("目标 Redis 后继缺少绑定运行账本".into());
        }
        return Ok(original["redis"].clone());
    };
    let manifest_value = common::read_bound(backend, manifest, "目标 Redis 运行清单")?;
    if manifest_value["kind"] == "reference-fixture-service-run" {
        let projection = protocol::fixture_projection(backend, run, original)?;
        if projection["cache_runtime"] != *cache {
            return Err("目标 Redis 读取未绑定实际夹具后继".into());
        }
        let redis = super::super::fresh::selection::redis(
            backend,
            &original["redis"],
            (!cache.is_null()).then_some(cache),
            true,
        )?;
        if projection["storage"]["redis"] != redis {
            return Err("目标 Redis 夹具后继改变了原端点或工具".into());
        }
        return Ok(redis);
    }
    let state = state::load(run, true)?;
    common::bound_file(backend, ledger, "目标 Redis 运行账本")?;
    if state["manifest"] != *manifest {
        return Err("目标 Redis 运行账本与原清单不同".into());
    }
    require_cache_result(backend, run, manifest, &state, cache)?;
    super::super::fresh::selection::redis(
        backend,
        &original["redis"],
        (!cache.is_null()).then_some(cache),
        false,
    )
}

fn require_cache_result(
    backend: &Path,
    run: &Path,
    manifest: &Value,
    state: &Value,
    cache: &Value,
) -> Result<()> {
    let latest = state::attempts(state)?
        .iter()
        .rfind(|attempt| attempt["stage"] == "cache-target");
    let Some(latest) = latest else {
        if !cache.is_null() || run.join("cache-target").try_exists()? {
            return Err("目标 Redis 后继缺少成功发布阶段".into());
        }
        return Ok(());
    };
    if latest["status"] != "passed"
        || !matches!(
            latest["mode"].as_str(),
            Some("restart" | "reconcile" | "resume")
        )
        || latest["result"] != cache["restart_result"]
    {
        return Err("目标 Redis 读取只接受账本当前成功后继".into());
    }
    let result = common::read_bound(backend, &latest["result"], "目标缓存发布结果")?;
    let ready = common::read_bound(backend, &cache["ready"], "目标缓存就绪收据")?;
    let runtime = common::read_bound(backend, &cache["runtime"], "目标缓存运行收据")?;
    let request = common::read_bound(backend, &cache["request"], "目标缓存后继请求")?;
    let projected = json_io::object(cache, "目标缓存完整后继")?;
    let ready_fields = json_io::object(&ready, "目标缓存就绪字段")?;
    if result["status"] != "cache_ready"
        || result["side"] != "target"
        || request["manifest"] != *manifest
        || common::bound_file(backend, &cache["request"], "目标缓存后继请求")?
            != run.join("cache-target/request.json")
        || runtime["redis"] != cache["redis"]
        || ready_fields.len() + 2 != projected.len()
        || projected.iter().any(|(key, value)| match key.as_str() {
            "restart_result" => false,
            "ready" => result.get(key) != Some(value),
            _ => result.get(key) != Some(value) || ready.get(key) != Some(value),
        })
    {
        return Err("目标 Redis 后继与原请求、运行及已发布就绪收据不同".into());
    }
    Ok(())
}
