//! 从首代或已验证的完整后继账本选择实际存储；不采信内嵌替换值。
use super::super::super::{
    cache, json as document,
    resources::{common, storage},
};
use crate::{Result, cli::CloneSide, recovery::fixture_control::services::lifecycle::query};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Clone)]
pub(crate) struct Selected {
    pub(crate) value: Value,
    pub(crate) runtime: Option<Value>,
}

pub(super) fn load(backend: &Path, request: &Value, run: Option<&Path>) -> Result<Selected> {
    let expected = &request["storage"];
    let Some(run) = run else {
        return Ok(Selected {
            value: expected.clone(),
            runtime: None,
        });
    };
    let manifest = document::read(&run.join("manifest.json"))?;
    let fixture = manifest["kind"] == "reference-fixture-service-run";
    let (runtime, cache) = if fixture {
        let result = query::dispatch(
            backend,
            "service-transition",
            &json!({
                "run":run,"expected":{"storage":expected["rustfs"],"redis":expected["redis"]}
            }),
        )?;
        let pair = document::array(&result, "夹具存储后继")?;
        if pair.len() != 2 {
            return Err("夹具后继必须精确包含 RustFS 与 Redis".into());
        }
        (non_null(&pair[0]), non_null(&pair[1]))
    } else {
        (
            storage::registered::registered(backend, run, CloneSide::Target, None)?,
            non_null(&cache::binding::load(backend, run, None)?),
        )
    };
    if let Some(runtime) = &runtime
        && !fixture
    {
        let previous = common::read_bound(backend, &runtime["request"], "RustFS 后继请求")?;
        for key in ["identity", "process_receipt", "launch_receipt"] {
            if previous["previous"][key] != expected["rustfs"][key] {
                return Err("RustFS 后继未绑定原目标完整进程收据".into());
            }
        }
    }
    let redis = redis(backend, &expected["redis"], cache.as_ref(), fixture)?;
    let value = json!({"rustfs":runtime.as_ref().map(|v| &v["storage"]).unwrap_or(&expected["rustfs"]),"redis":redis});
    Ok(Selected { value, runtime })
}

fn non_null(value: &Value) -> Option<Value> {
    (!value.is_null()).then(|| value.clone())
}

fn redis(backend: &Path, expected: &Value, cache: Option<&Value>, fixture: bool) -> Result<Value> {
    let Some(cache) = cache else {
        return Ok(expected.clone());
    };
    let mut allowed = expected.clone();
    for key in ["pid", "started", "run_id"] {
        allowed[key] = cache["redis"][key].clone();
    }
    if fixture {
        let outer = common::read_bound(backend, &cache["generation"], "夹具缓存后继")?;
        let generation = common::read_bound(backend, &outer["generation"], "夹具完整后继")?;
        let request = common::read_bound(backend, &generation["redis"]["request"], "夹具缓存请求")?;
        allowed["wsl"] = request["wsl"].clone();
    } else {
        let request = common::read_bound(backend, &cache["request"], "缓存后继请求")?;
        if request["previous"] != *expected {
            return Err("缓存后继未绑定原目标完整 Redis".into());
        }
        if !cache["tools"].is_null() {
            let tools = common::read_bound(backend, &cache["tools"], "缓存工具后继")?;
            allowed["wsl"] = tools["process"]["wsl"].clone();
        }
    }
    if cache["redis"] != allowed {
        return Err("缓存后继改变了未授权物理配置或工具".into());
    }
    Ok(allowed)
}

/// 固定登记的完整历史，只返回账本实际出现过的存储组合，不生成跨阶段笛卡尔积。
pub(crate) fn history(backend: &Path, target: &Path, request: &Value) -> Result<Vec<Selected>> {
    let mut result = vec![Selected {
        value: request["storage"].clone(),
        runtime: None,
    }];
    let registration_path = target
        .parent()
        .ok_or("fresh 目标没有工作区")?
        .join("registration.json");
    if !registration_path.try_exists()? {
        return Ok(result);
    }
    let workspace = target.parent().ok_or("fresh 工作区缺失")?;
    let observed = super::super::cli::execute(backend, workspace, "cli-registration", &json!({}))?;
    let registration = &observed["value"];
    let registered_request =
        common::read_bound(backend, &registration["request"], "fresh 固定请求")?;
    if registered_request != *request
        || Path::new(document::text(
            &registration["target_directory"],
            "fresh 目标",
        )?) != target
    {
        return Err("prepare 历史不属于固定工作区及请求".into());
    }
    let run = super::storage::load(backend, &registration["storage_run"])?;
    let state_before = common::binding(&run.join("state.json"))?;
    let manifest = document::read(&run.join("manifest.json"))?;
    if manifest["kind"] == "reference-fixture-service-run" {
        let values = query::storage_history(backend, &run)?;
        let values = document::array(&values, "夹具完整存储历史")?;
        let matches = values
            .iter()
            .enumerate()
            .filter(|(_, item)| item["value"] == request["storage"])
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err("固定请求存储没有唯一夹具谱系锚点".into());
        }
        for item in values.iter().skip(matches[0]) {
            push(
                &mut result,
                Selected {
                    value: item["value"].clone(),
                    runtime: non_null(&item["runtime"]),
                },
            );
        }
    } else {
        ordinary_history(backend, &run, request, &mut result)?;
    }
    if common::binding(&run.join("state.json"))? != state_before
        || common::binding(&registration_path)? != observed["binding"]
    {
        return Err("prepare 完整存储历史在核验期间变化".into());
    }
    Ok(result)
}

fn ordinary_history(
    backend: &Path,
    run: &Path,
    request: &Value,
    result: &mut Vec<Selected>,
) -> Result<()> {
    let state = super::super::super::lifecycle::state::load(run, true)?;
    let mut current = result[0].clone();
    let mut available = [true, true];
    for attempt in super::super::super::lifecycle::state::attempts(&state)? {
        let stage = attempt["stage"].as_str();
        if !matches!(stage, Some("storage-target" | "cache-target")) {
            continue;
        }
        if attempt["status"] != "passed" {
            continue;
        }
        if matches!(attempt["mode"].as_str(), Some("stop" | "recover")) {
            available[usize::from(stage == Some("cache-target"))] = false;
            continue;
        }
        let number = attempt["number"]
            .as_u64()
            .and_then(|n| n.checked_add(1))
            .ok_or("历史阶段序号溢出")?;
        if stage == Some("storage-target") && attempt["mode"] == "restart" {
            let runtime =
                storage::registered::registered(backend, run, CloneSide::Target, Some(number))?
                    .ok_or("历史存储发布缺失")?;
            let previous = common::read_bound(backend, &runtime["request"], "历史存储请求")?;
            for key in ["identity", "process_receipt", "launch_receipt"] {
                if previous["previous"][key] != request["storage"]["rustfs"][key] {
                    return Err("历史存储未绑定固定原始身份".into());
                }
            }
            current.value["rustfs"] = runtime["storage"].clone();
            current.runtime = Some(runtime);
            available[0] = true;
        } else if stage == Some("cache-target")
            && matches!(
                attempt["mode"].as_str(),
                Some("restart" | "resume" | "reconcile")
            )
        {
            let published = common::read_bound(backend, &attempt["result"], "历史缓存发布")?;
            if published["status"] != "cache_ready" {
                continue;
            }
            let tools = if published["tools"].is_null() {
                Value::Null
            } else {
                common::read_bound(backend, &published["tools"], "历史缓存工具")?
            };
            let frozen = if tools.is_null() {
                Value::Null
            } else {
                json!([tools["source_result"], tools["successor"]])
            };
            let cache = cache::binding::query(backend, run, None, &json!(number), &frozen)?;
            current.value["redis"] =
                redis(backend, &request["storage"]["redis"], Some(&cache), false)?;
            available[1] = true;
        } else {
            continue;
        }
        if available.into_iter().all(|value| value) {
            push(result, current.clone());
        }
    }
    Ok(())
}

fn push(result: &mut Vec<Selected>, item: Selected) {
    if !result
        .iter()
        .any(|old| old.value == item.value && old.runtime == item.runtime)
    {
        result.push(item);
    }
}
