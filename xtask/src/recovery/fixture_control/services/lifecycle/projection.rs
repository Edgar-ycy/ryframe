//! 服务代次只读投影；只沿已经验证的账本 lineage 更新调用方身份。
use super::*;

pub(super) fn transition(
    runtime: &mut dyn Runtime,
    backend: &Path,
    run: &Path,
    expected: &Value,
) -> Result<Value> {
    crate::local_test_path::validate_local_test_path(
        run,
        backend,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    let manifest = read(&run.join("manifest.json"))?;
    let state = state::load(run, true)?;
    let history = history::validate(run, &state, false, false)?;
    let (review, bootstrap) = active_inputs(&manifest, &state, &history)?;
    let value = runtime.call(
        "context",
        json!({"review":review["path"],"environment":bootstrap["path"]}),
    )?;
    let context = Context::load(backend, value)?;
    let current_history = history::validate(&context.run, &context.value["state"], false, false)?;
    check(
        context.run == run && current_history["active_generation"] == history["active_generation"],
        "夹具服务代次在解析工具续签期间变化",
    )?;
    if history["active_generation"]["kind"] == "initial" {
        return Ok(json!([null, null]));
    }
    let services = registered(runtime, &context, true)?;
    let result = project(&context, &history, &services, expected)?;
    context.guard(None)?;
    Ok(result)
}

fn active_inputs(manifest: &Value, state: &Value, history: &Value) -> Result<(Value, Value)> {
    if history["active_generation"]["kind"] == "initial" {
        return Ok((manifest["review"].clone(), manifest["bootstrap"].clone()));
    }
    let matches = state::attempts(state)?
        .iter()
        .filter(|item| {
            item["stage"] == "fixture-services"
                && item["mode"] == "restart"
                && item["status"] == "passed"
                && item["result"] == history["active_generation"]
        })
        .collect::<Vec<_>>();
    check(
        matches.len() == 1,
        "夹具活跃服务代次缺少唯一已通过 restart 来源",
    )?;
    let sources = &matches[0]["sources"];
    check(
        sources["review"].is_object() && sources["bootstrap"].is_object(),
        "夹具 restart 未绑定当前审阅或环境",
    )?;
    Ok((sources["review"].clone(), sources["bootstrap"].clone()))
}

pub(super) fn project(
    context: &Context,
    history: &Value,
    services: &Value,
    expected: &Value,
) -> Result<Value> {
    let storage = storage(&services["storage"])?;
    let current = json!({"storage":storage,"redis":services["runtime"]["redis"]});
    check(
        *expected == services["origin"]
            || *expected == current
            || recorded(context, history)?.contains(expected),
        "夹具服务重启 lineage 未绑定 fresh-target 的原始或已登记服务代次",
    )?;
    let request = &services["requests"]["rustfs"];
    for key in ["data_directory", "api_url", "console_url"] {
        check(!request[key].is_null(), "夹具服务投影缺少冻结请求字段")?;
    }
    Ok(
        json!([{"storage":storage,"data_directory":request["data_directory"],"api_url":request["api_url"],
        "console_url":request["console_url"],"generation":history["active_generation"]},
        {"redis":services["runtime"]["redis"],"generation":history["active_generation"]}]),
    )
}

fn storage(value: &Value) -> Result<Value> {
    let mut projection = serde_json::Map::new();
    for key in ["identity", "sha256", "process_receipt", "launch_receipt"] {
        projection.insert(
            key.to_owned(),
            value
                .get(key)
                .ok_or("已登记服务代次缺少存储身份字段")?
                .clone(),
        );
    }
    Ok(Value::Object(projection))
}

fn recorded(context: &Context, history: &Value) -> Result<Vec<Value>> {
    let mut recorded = Vec::new();
    for item in state::attempts(&context.value["state"])? {
        if item["stage"] != "fixture-services"
            || item["mode"] != "restart"
            || item["status"] != "passed"
            || item["result"] == history["active_generation"]
        {
            continue;
        }
        let outer = history::evidence::bound(&context.run, &item["result"])?;
        let generation = history::evidence::bound(&context.run, &outer["generation"])?;
        recorded.push(json!({"storage":storage(&generation["rustfs"]["storage"])?,"redis":generation["redis"]["runtime"]["redis"]}));
    }
    Ok(recorded)
}

/// 复用完整登记校验后投影所有已发布代次；不观察旧 PID 活体，不把 latest 当作历史。
pub(super) fn historical(runtime: &mut dyn Runtime, backend: &Path, run: &Path) -> Result<Value> {
    let manifest = read(&run.join("manifest.json"))?;
    let state = state::load(run, true)?;
    let history = history::validate(run, &state, true, true)?;
    let (review, bootstrap) = active_inputs(&manifest, &state, &history)?;
    let value = runtime.call(
        "context",
        json!({"review":review["path"],"environment":bootstrap["path"]}),
    )?;
    let context = Context::load(backend, value)?;
    check(
        context.run == run && context.value["state"] == state,
        "服务历史读取期间完整账本变化",
    )?;
    let services = registered(runtime, &context, true)?;
    let mut result = vec![
        json!({"value":{"rustfs":services["origin"]["storage"],"redis":services["origin"]["redis"]},"runtime":null}),
    ];
    for attempt in state::attempts(&state)? {
        if attempt["stage"] != "fixture-services"
            || attempt["mode"] != "restart"
            || attempt["status"] != "passed"
        {
            continue;
        }
        let outer = history::evidence::bound(run, &attempt["result"])?;
        let generation = history::evidence::bound(run, &outer["generation"])?;
        let request = history::evidence::bound(run, &generation["rustfs"]["request"])?;
        let storage = storage(&generation["rustfs"]["storage"])?;
        let runtime = json!({"storage":storage,"data_directory":request["data_directory"],
            "api_url":request["api_url"],"console_url":request["console_url"],"generation":attempt["result"]});
        result.push(json!({"value":{"rustfs":storage,"redis":generation["redis"]["runtime"]["redis"]},"runtime":runtime}));
    }
    context.guard(None)?;
    Ok(json!(result))
}
