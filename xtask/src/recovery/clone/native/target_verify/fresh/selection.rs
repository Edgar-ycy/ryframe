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

pub(crate) fn transition(
    backend: &Path,
    request: &Value,
    run: Option<&Path>,
    owner: Option<&super::super::guard::Generation>,
) -> Result<(Selected, Option<Value>)> {
    let expected = &request["storage"];
    let Some(run) = run else {
        return Ok((
            Selected {
                value: expected.clone(),
                runtime: None,
            },
            None,
        ));
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
            non_null(&cache::binding::load(backend, run, owner)?),
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
    Ok((Selected { value, runtime }, cache))
}

fn non_null(value: &Value) -> Option<Value> {
    (!value.is_null()).then(|| value.clone())
}

pub(in crate::recovery::clone::native::target_verify) fn redis(
    backend: &Path,
    expected: &Value,
    cache: Option<&Value>,
    fixture: bool,
) -> Result<Value> {
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
        let runtime = &generation["redis"]["runtime"];
        if expected["platform"] == "windows" {
            validate_windows_transition(expected, &request, runtime, &cache["redis"])?;
        } else {
            if request["format_version"] == 3
                || runtime["format_version"] == 3
                || cache["redis"]["platform"] == "windows"
            {
                return Err("夹具缓存后继不能切换 Redis 平台".into());
            }
            allowed["wsl"] = request["wsl"].clone();
        }
    } else {
        let request = common::read_bound(backend, &cache["request"], "缓存后继请求")?;
        if request["previous"] != *expected {
            return Err("缓存后继未绑定原目标完整 Redis".into());
        }
        if !cache["tools"].is_null() {
            let tools = common::read_bound(backend, &cache["tools"], "缓存工具后继")?;
            if expected["platform"] == "windows" {
                if tools["process"]["format_version"] != 3
                    || tools["process"]["tool"] != expected["tool"]
                {
                    return Err("Windows 缓存后继未绑定原登记工具".into());
                }
            } else {
                allowed["wsl"] = tools["process"]["wsl"].clone();
            }
        }
    }
    if cache["redis"] != allowed {
        return Err("缓存后继改变了未授权物理配置或工具".into());
    }
    Ok(allowed)
}

fn validate_windows_transition(
    expected: &Value,
    request: &Value,
    runtime: &Value,
    observed: &Value,
) -> Result<()> {
    let tool = &expected["tool"];
    cache::windows_tool::validate(tool)?;
    if request["format_version"] != 3
        || runtime["format_version"] != 3
        || request["tool"] != *tool
        || request["port"] != expected["port"]
        || !expected["port"].is_u64()
        || request["configuration"] != expected["configuration"]
        || !expected["configuration"].is_object()
        || expected["executable"] != tool["path"]
        || expected["sha256"] != tool["sha256"]
        || [
            "wsl",
            "distribution",
            "python",
            "native_tool",
            "previous_boot_id",
        ]
        .iter()
        .any(|key| request.get(*key).is_some() || expected.get(*key).is_some())
        || runtime["redis"] != *observed
    {
        return Err("Windows Redis 后继缺少原工具、配置或完整运行代次证明".into());
    }
    Ok(())
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

#[cfg(test)]
#[test]
fn windows_fixture_restart_updates_only_bound_process_generation() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let config = temporary.path().join("redis.conf");
    std::fs::write(&config, "dir .\n").unwrap();
    let configuration = common::binding(&config).unwrap();
    let tool = json!({
        "platform":"windows","path":"D:/tools/redis-server.exe","sha256":"a".repeat(64),
        "version":"Redis server v=7.4.11 sha=00000000:0 malloc=libc bits=64 build=5bd2587910691cb8",
        "dlls":{"msys-2.0.dll":"b".repeat(64)},
    });
    let original = json!({
        "platform":"windows","port":29770,"pid":10,"started":"123","run_id":"a".repeat(40),
        "executable":tool["path"],"sha256":tool["sha256"],"configuration":configuration,"tool":tool,
    });
    let mut current = original.clone();
    current["pid"] = json!(11);
    current["started"] = json!("124");
    current["run_id"] = json!("b".repeat(40));
    let request =
        json!({"format_version":3,"tool":tool,"port":29770,"configuration":configuration});
    let runtime = json!({"format_version":3,"redis":current});
    let publish = |name: &str, request: &Value, runtime: &Value| {
        let dir = temporary.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        let request_file = dir.join("request.json");
        document::write_new(&request_file, request).unwrap();
        let generation = dir.join("generation.json");
        document::write_new(
            &generation,
            &json!({"redis":{"request":common::binding(&request_file).unwrap(),"runtime":runtime}}),
        )
        .unwrap();
        let outer = dir.join("result.json");
        document::write_new(
            &outer,
            &json!({"generation":common::binding(&generation).unwrap()}),
        )
        .unwrap();
        json!({"generation":common::binding(&outer).unwrap(),"redis":current})
    };
    assert_eq!(redis(&backend, &original, None, true).unwrap(), original);
    let cache = publish("accepted", &request, &runtime);
    assert_eq!(
        redis(&backend, &original, Some(&cache), true).unwrap(),
        current
    );
    let mut changed = request.clone();
    changed["tool"]["sha256"] = json!("c".repeat(64));
    let changed_tool = publish("different-tool", &changed, &runtime);
    assert!(redis(&backend, &original, Some(&changed_tool), true).is_err());
    changed = request.clone();
    changed["port"] = json!(29771);
    let changed_port = publish("different-port", &changed, &runtime);
    assert!(redis(&backend, &original, Some(&changed_port), true).is_err());
    let mut changed_runtime = runtime.clone();
    changed_runtime["redis"]["pid"] = json!(12);
    let changed_process = publish("different-runtime", &request, &changed_runtime);
    assert!(redis(&backend, &original, Some(&changed_process), true).is_err());
    let missing = publish("missing-runtime", &request, &Value::Null);
    assert!(redis(&backend, &original, Some(&missing), true).is_err());
    let linux =
        json!({"wsl":{"path":"D:/wsl.exe"},"pid":10,"started":"123","run_id":"a".repeat(40)});
    let error = redis(&backend, &linux, Some(&cache), true).unwrap_err();
    assert!(error.to_string().contains("不能切换 Redis 平台"));
}
