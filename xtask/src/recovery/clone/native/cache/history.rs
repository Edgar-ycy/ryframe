//! 缓存创建谱系、只读观察与外层成功发布核验。
use super::super::{digest, json as doc, lifecycle::state, resources::common};
use super::markers;
use super::rejected;
use super::stage::{Context, STAGE, number, path, successor};
use crate::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) type Generation = (Value, PathBuf, Value);
pub(super) fn generation(
    context: &mut Context<'_>,
    request: &Value,
    descriptor: &Value,
    output: &Path,
    frozen: &Value,
) -> Result<(Value, Value)> {
    if frozen.is_null() {
        successor::generation(context, request, output, &Value::Null, true)
    } else {
        successor::frozen_generation(context, request, descriptor, output, frozen)
    }
}
pub(super) fn starts(
    context: &mut Context<'_>,
    request: &Value,
    descriptor: &Value,
    before: Option<u64>,
    frozen: &Value,
) -> Result<Vec<Generation>> {
    let mut result: Vec<Generation> = Vec::new();
    for attempt in context.records(true)? {
        if attempt["mode"] != "restart"
            || before.is_some_and(|limit| number(&attempt).is_ok_and(|n| n >= limit))
        {
            continue;
        }
        let output = context.output(number(&attempt)?);
        common::local_directory(context.backend, &output, "缓存历史目录")?;
        let file = output.join("start.json");
        if !file.exists() {
            if output.join("process").exists() {
                return Err(rejected("缓存进程目录缺少持久控制阶段绑定"));
            }
            continue;
        }
        let (process, tools) = generation(context, request, descriptor, &output, frozen)?;
        let mut expected = json!({"request":descriptor,"controller":context.controller(&attempt)?,"attempt":attempt["number"],"process_request_sha256":digest::plan_hash(&process)?,"output":output.join("process")});
        if !tools.is_null() {
            expected["tools"] = tools;
            let mut predecessors = Vec::new();
            for (earlier, earlier_output, earlier_runtime) in &result {
                let file = output.join(format!("predecessor-{:04}.json", number(earlier)?));
                let proof = doc::read(&file)?;
                let (old, _) = generation(context, request, descriptor, earlier_output, frozen)?;
                doc::exact(
                    &proof,
                    &["process_sha256", "runtime_sha256", "observation"],
                    "缓存前驱证明",
                )?;
                if proof["process_sha256"] != digest::plan_hash(&old)?
                    || proof["runtime_sha256"] != digest::plan_hash(earlier_runtime)?
                    || proof["observation"]["state"] != "stopped"
                    || old != process
                        && proof["observation"]["tools_sha256"]
                            != digest::plan_hash(
                                &json!({"wsl":process["wsl"],"python":process["python"]}),
                            )?
                {
                    return Err(rejected("缓存工具后继缺少每个旧代次的死亡证明"));
                }
                predecessors.push(common::binding(&file)?);
            }
            expected["predecessors"] = json!(predecessors);
        }
        if doc::read(&file)? != expected {
            return Err(rejected("缓存启动记录与原控制器、请求或进程目录不符"));
        }
        let runtime = context.call(
            "inspect",
            json!({"request":process,"output":output.join("process")}),
        )?;
        if !runtime.is_null() {
            result.push((attempt, output, runtime));
        }
    }
    Ok(result)
}
pub(super) fn owned(context: &Context<'_>, output: &Path, base: &Value) -> Result<(Value, Value)> {
    let file = output.join("runtime.json");
    let runtime = doc::read(&file)?;
    if !successor::contains(&runtime, base) {
        return Err(rejected("缓存完整运行记录与实际创建收据不符"));
    }
    common::bound_file(context.backend, &runtime["process_receipt"], "缓存进程收据")?;
    Ok((runtime, common::binding(&file)?))
}
fn expected_redis(request: &Value, process: &Value, redis: &Value) -> Result<Value> {
    let mut expected = request["previous"].clone();
    doc::object(&expected, "原 Redis 代次")?;
    expected["wsl"] = process["wsl"].clone();
    for key in ["pid", "started", "run_id"] {
        expected[key] = redis.get(key).ok_or("Redis 运行身份缺失")?.clone();
    }
    Ok(expected)
}
pub(super) fn publish(context: &mut Context<'_>, p: &Value) -> Result<Value> {
    let output = path(&p["output"])?;
    let runtime = &p["runtime"];
    let proof = markers::confirmation(
        context,
        &path(&p["owner"])?,
        &p["request"],
        &p["descriptor"],
        &p["runtime_binding"],
    )?;
    if runtime["redis"] != expected_redis(&p["request"], &p["process"], &runtime["redis"])? {
        return Err(rejected("缓存重启改变原配置、工具或监听端点"));
    }
    let mut ready = json!({"request":p["descriptor"],"runtime":p["runtime_binding"],"redis":runtime["redis"],"owner":proof,"controller":p["controller"],"attempt":p["number"]});
    if !p["tools"].is_null() {
        ready["tools"] = p["tools"].clone();
        ready["start"] = common::binding(
            &path(&p["runtime_binding"]["path"])?
                .parent()
                .ok_or("缓存运行目录缺失")?
                .join("start.json"),
        )?;
    }
    doc::write_new(&output.join("ready.json"), &ready)?;
    let mut result = ready;
    let object = result.as_object_mut().unwrap();
    object.extend(json!({"status":"cache_ready","ready":common::binding(&output.join("ready.json"))?,"side":"target","remote_writes":0,"restored_markers":2,"sessions_restored":false,"locks_restored":false,"resources_deleted":false,"restore_qualified":false}).as_object().unwrap().clone());
    Ok(result)
}
pub(super) fn registered(context: &mut Context<'_>, p: &Value) -> Result<Value> {
    doc::exact(
        p,
        &["owned_lock_identity", "before", "frozen"],
        "缓存发布查询",
    )?;
    let selected = context
        .records(false)?
        .into_iter()
        .filter(|r| {
            p["before"]
                .as_u64()
                .is_none_or(|limit| r["number"].as_u64().is_some_and(|n| n < limit))
        })
        .collect::<Vec<_>>();
    if !context.directory.join(STAGE).exists() && selected.is_empty() {
        return Ok(Value::Null);
    }
    let (request, descriptor) = context.registration()?;
    let attempt = selected
        .last()
        .ok_or("缓存最新阶段不是成功发布的可用代次，不能回退旧成功")?;
    if attempt["status"] != "passed"
        || !matches!(
            attempt["mode"].as_str(),
            Some("restart" | "resume" | "reconcile")
        )
    {
        return Err(rejected(
            "缓存最新阶段不是成功发布的可用代次，不能回退旧成功",
        ));
    }
    let result = common::read_bound(context.backend, &attempt["result"], "缓存发布结果")?;
    if result["status"] != "cache_ready" {
        return Err(rejected("缓存最新成功阶段尚未确认可用后像，必须显式继续"));
    }
    let output = context.output(number(attempt)?);
    let (process, tools) = if p["frozen"].is_null() {
        successor::generation(context, &request, &output, &p["owned_lock_identity"], true)?
    } else {
        successor::frozen_generation(context, &request, &descriptor, &output, &p["frozen"])?
    };
    if tools.is_null() {
        context.validate(&request, &p["owned_lock_identity"])?;
    }
    let ready = doc::read(&output.join("ready.json"))?;
    let created = starts(
        context,
        &request,
        &descriptor,
        Some(number(attempt)? + 1),
        &p["frozen"],
    )?;
    let (_, process_output, base) = created.last().ok_or("可用缓存缺少实际创建记录")?;
    let (actual, _) = generation(context, &request, &descriptor, process_output, &p["frozen"])?;
    if actual != process {
        return Err(rejected("缓存发布的工具与实际启动代次不同"));
    }
    if base.get("state").is_some() {
        return Err(rejected("未完成启动或已清理的 Redis 不能作为可用缓存"));
    }
    let (runtime, runtime_binding) = owned(context, process_output, base)?;
    let owner = common::bound_file(context.backend, &ready["owner"], "缓存标记确认")?;
    if owner.file_name().and_then(|v| v.to_str()) != Some("confirmed.json")
        || owner.parent().and_then(Path::parent).and_then(Path::parent)
            != Some(context.directory.join(STAGE).as_path())
    {
        return Err(rejected("缓存标记确认不属于本运行"));
    }
    markers::confirmation(
        context,
        owner.parent().unwrap(),
        &request,
        &descriptor,
        &runtime_binding,
    )?;
    let mut expected = json!({"request":descriptor,"runtime":runtime_binding,"redis":runtime["redis"],"owner":ready["owner"],"controller":context.controller(attempt)?,"attempt":attempt["number"]});
    if !tools.is_null() {
        expected["tools"] = tools;
        expected["start"] = common::binding(&process_output.join("start.json"))?;
    }
    if ready != expected
        || result["ready"] != common::binding(&output.join("ready.json"))?
        || !successor::contains(&result, &expected)
    {
        return Err(rejected("缓存代次尚未取得同一外层阶段的成功发布"));
    }
    if runtime["redis"] != expected_redis(&request, &process, &runtime["redis"])? {
        return Err(rejected("已发布 Redis 改变原配置或物理目标"));
    }
    expected["ready"] = common::binding(&output.join("ready.json"))?;
    expected["restart_result"] = attempt["result"].clone();
    Ok(expected)
}
fn published_start(context: &Context<'_>, output: &Path) -> Result<bool> {
    let mut found = false;
    for attempt in context.records(false)? {
        if attempt["status"] != "passed" || attempt["result"].is_null() {
            continue;
        }
        let result = common::read_bound(context.backend, &attempt["result"], "缓存成功结果")?;
        if result["status"] != "cache_ready"
            || path(&result["runtime"]["path"])? != output.join("runtime.json")
        {
            continue;
        }
        let ready = common::read_bound(context.backend, &result["ready"], "缓存就绪证据")?;
        if result["start"] != common::binding(&output.join("start.json"))?
            || ready["start"] != result["start"]
        {
            return Err(rejected("缓存历史死亡证明未被已发布 start 摘要锚定"));
        }
        found = true;
    }
    Ok(found)
}
pub(super) fn observe_or_stop(
    context: &mut Context<'_>,
    request: &Value,
    descriptor: &Value,
    output: &Path,
    mode: &str,
    limit: u64,
) -> Result<Value> {
    let mut previous = starts(context, request, descriptor, Some(limit), &Value::Null)?;
    let candidates = successor::tool_paths(context)?;
    let selected = candidates
        .last()
        .and_then(|p| p.parent())
        .or_else(|| previous.last().map(|r| r.1.as_path()))
        .unwrap_or(output);
    let effective = successor::generation(context, request, selected, &Value::Null, true)?.0;
    if mode != "status" {
        previous.reverse();
    }
    let mut results = Vec::new();
    for (attempt, process_output, runtime) in &previous {
        let original =
            successor::generation(context, request, process_output, &Value::Null, true)?.0;
        // 顺序反转不改变原谱系的最新创建代次。
        let latest = previous
            .iter()
            .max_by_key(|r| r.0["number"].as_u64())
            .map(|r| &r.1);
        let proof = latest.map(|p| p.join(format!("predecessor-{:04}.json", number_of(attempt))));
        let observed = if let Some(proof) = proof.filter(|p| p.exists())
            && published_start(context, latest.unwrap())?
        {
            let mut observed = doc::read(&proof)?["observation"].clone();
            doc::object(&observed, "缓存死亡观察")?;
            observed["evidence"] = common::binding(&proof)?;
            observed
        } else if original != effective {
            context.call(
                "predecessor",
                json!({"request":original,"runtime":runtime,"effective":effective}),
            )?
        } else if mode == "status" {
            context.call("status", json!({"request":effective,"runtime":runtime}))?
        } else {
            let item = output.join(format!("p{:04}", number(attempt)?));
            common::ensure_new_directory(context.backend, &item, "缓存停止证据")?;
            context.call(
                "stop",
                json!({"request":effective,"private":{},"runtime":runtime,"output":item}),
            )?
        };
        results.push(json!({"attempt":attempt["number"],"process":observed}));
    }
    results.sort_by_key(|r| r["attempt"].as_u64());
    Ok(
        json!({"status":if mode=="status"{"cache_status"}else{"cache_stopped"},"side":"target","request":descriptor,"processes":results,"copy_usable":false,"remote_writes":0,"resources_deleted":false}),
    )
}
fn number_of(attempt: &Value) -> u64 {
    attempt["number"].as_u64().unwrap_or(0)
}
pub(super) fn status(context: &mut Context<'_>, p: &Value) -> Result<Value> {
    doc::exact(p, &["allow_close_attempt"], "缓存观察参数")?;
    let initial = common::binding(&context.directory.join("state.json"))?;
    let state = state::load(&context.directory, false)?;
    let running = state::attempts(&state)?
        .iter()
        .filter(|r| r["status"] == "running")
        .collect::<Vec<_>>();
    if !running.is_empty()
        && !(doc::boolean(&p["allow_close_attempt"], "允许收尾观察")?
            && running.len() == 1
            && running[0]["stage"] == "seed-runtime"
            && running[0]["mode"] == "source-generation-reboot-close")
    {
        return Err(rejected("统一运行中还有未完成阶段，不能形成稳定观察"));
    }
    if context.records(true)?.is_empty() && !context.directory.join(STAGE).exists() {
        return Ok(json!({"status":"cache_unregistered","copy_usable":false,"processes":[]}));
    }
    let (request, descriptor) = context.registration()?;
    let directory = context.directory.clone();
    let result = observe_or_stop(
        context,
        &request,
        &descriptor,
        &directory,
        "status",
        state::attempts(&state)?.len() as u64 + 1,
    )?;
    if common::binding(&context.directory.join("state.json"))? != initial
        || context.registration()? != (request, descriptor)
    {
        return Err(rejected("缓存只读观察期间阶段或登记变化"));
    }
    Ok(result)
}
