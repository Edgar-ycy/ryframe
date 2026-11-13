use super::{
    dependency::Dependencies,
    manifest,
    model::*,
    plan::{self, Context, Paths},
    process, publication, receipts, review,
};
use crate::{Result, process::failure_exit_code};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn execute(api: &mut impl Dependencies, request: &Value) -> Result<Value> {
    validate_request(request)?;
    let paths = Paths {
        backend: path(&request["backend_dir"])?,
        environment: path(&request["environment"])?,
        output: path(&request["output"])?,
        binding: path(&request["browser_binding"])?,
    };
    let operation = text(&request["operation"])?;
    check(
        paths.binding.parent() == Some(paths.output.as_path()),
        "浏览器绑定必须直接位于本次运行目录",
    )?;
    if operation == "bind" {
        return bind(
            api,
            &paths,
            text(&request["run_id"])?,
            text(&request["server"])?,
        );
    }
    let (binding, guard) = document(&paths.binding)?;
    let run_id = text(&binding["run_id"])?;
    let server = text(&binding["server"])?;
    binding_name(&paths, run_id)?;
    let context = plan::make(
        api,
        &paths,
        run_id,
        server,
        operation == "browser",
        operation == "browser-close",
        operation != "browser",
    )?;
    check(
        context.binding == binding,
        "浏览器绑定与当前源码、运行时或环境不一致",
    )?;
    unchanged(&guard)?;
    if operation == "browser" {
        run(
            api,
            &paths,
            context,
            guard,
            text(&request["credential_endpoint"])?,
        )
    } else {
        check(
            ["browser-verify", "browser-close"].contains(&operation),
            "浏览器操作无效",
        )?;
        context.unchanged()?;
        review::result(api, &context, &paths.binding)?;
        context.unchanged()?;
        unchanged(&guard)?;
        Ok(
            json!({"status":if operation=="browser-close"{"reference_fixture_browser_closed"}else{"reference_fixture_browser_verified"},"binding":bound(&paths.binding)?,"result":bound(context.output("result")?)?,"run_id":run_id,"server":server,"remote_writes":0}),
        )
    }
}
fn validate_request(value: &Value) -> Result<()> {
    let operation = text(&value["operation"])?;
    check(
        ["bind", "browser", "browser-verify", "browser-close"].contains(&operation),
        "浏览器操作无效",
    )?;
    let mut fields = vec![
        "format_version",
        "operation",
        "backend_dir",
        "environment",
        "output",
        "browser_binding",
        "write",
    ];
    if operation == "bind" {
        fields.extend(["run_id", "server"]);
    }
    if operation == "browser" {
        fields.push("credential_endpoint");
    }
    crate::recovery::clone::native::json::exact(value, &fields, "浏览器执行协议")?;
    check(
        value["format_version"].as_u64() == Some(1)
            && value["write"].as_bool() == Some(["bind", "browser"].contains(&operation)),
        "浏览器版本或写入授权无效",
    )
}
fn binding_name(paths: &Paths, run_id: &str) -> Result<()> {
    check(
        paths.binding.file_name().and_then(|s| s.to_str())
            == Some(format!("browser-binding-{run_id}.json").as_str()),
        "浏览器绑定文件名与run id不一致",
    )
}
fn bind(api: &mut impl Dependencies, paths: &Paths, run_id: &str, server: &str) -> Result<Value> {
    binding_name(paths, run_id)?;
    check(
        !paths.binding.try_exists()?,
        "浏览器绑定已存在，拒绝覆盖或重放",
    )?;
    let context = plan::make(api, paths, run_id, server, true, false, false)?;
    context.unchanged()?;
    write_new(&paths.binding, &context.binding)?;
    context.unchanged()?;
    let repeated = plan::make(api, paths, run_id, server, true, false, false)?;
    check(
        document(&paths.binding)?.0 == context.binding && repeated.binding == context.binding,
        "绑定发布后来源变化；保留文件禁止重放",
    )?;
    Ok(
        json!({"status":"reference_fixture_browser_bound","binding":bound(&paths.binding)?,"run_id":run_id,"server":server,"remote_writes":0}),
    )
}
fn environment(context: &Context) -> Result<Value> {
    let app = text(&context.private["APP_ENV"])?;
    check(!app.is_empty(), "浏览器缺少APP_ENV")?;
    let mut values = serde_json::Map::new();
    for (key, value) in std::env::vars() {
        if [
            "PATH",
            "PATHEXT",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "TEMP",
            "TMP",
            "APPDATA",
            "LOCALAPPDATA",
            "PROGRAMFILES",
            "PROGRAMFILES(X86)",
            "PROGRAMDATA",
            "SYSTEMDRIVE",
            "OS",
            "NUMBER_OF_PROCESSORS",
        ]
        .contains(&key.to_ascii_uppercase().as_str())
        {
            values.insert(key, json!(value));
        }
    }
    values.insert("APP_ENV".into(), json!(app));
    values.insert("COREPACK_ENABLE_NETWORK".into(), json!("0"));
    values.insert(
        "COREPACK_HOME".into(),
        context.binding["tools"]["corepack_home"].clone(),
    );
    for key in ["TEMP", "TMP"] {
        if context.private[key].is_string() {
            values.insert(key.into(), context.private[key].clone());
        }
    }
    Ok(Value::Object(values))
}
fn configuration(binding: &Value, endpoint: &str) -> Result<Value> {
    let url = url::Url::parse(endpoint)?;
    check(
        url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
            && url.port().is_some()
            && url.path() == "/v1/credentials"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "浏览器凭据端点必须是本机受管HTTP地址",
    )?;
    let frontend = plan::validate_endpoint(text(&binding["endpoints"]["frontend"])?)?;
    let mut value = json!({"scopeId":binding["scope_id"],"tenantId":binding["identity"]["tenant_id"],"username":binding["identity"]["username"],"credentialEndpoint":endpoint,"backendDir":binding["backend"]["path"],"runtimeDir":path(&binding["runtime"]["path"])?.parent().ok_or("runtime缺少父目录")?,"requestCapacity":binding["rate_limits"]["request"]["capacity"],"requestWindowSeconds":binding["rate_limits"]["request"]["window_secs"],"loginCapacity":binding["rate_limits"]["login"]["capacity"],"loginWindowSeconds":binding["rate_limits"]["login"]["window_secs"],"loginBudgetState":binding["login_budget"]["path"],"port":frontend.port(),"serverMode":binding["server"],"fixture":"business","runId":binding["run_id"],"channel":"chrome"});
    value["workerTool"] = json!({"root":binding["tools"]["worker_origin"]["root"],
        "binding":binding["tools"]["worker_origin"]["binding"]});
    if binding["server"] == "preview" {
        crate::recovery::clone::native::json::exact(
            &binding["response_audit"],
            &["path", "initial_state", "first_writer"],
            "preview响应审计绑定",
        )?;
        value["previewResponseAudit"] = binding["response_audit"]["path"].clone();
    } else {
        check(
            binding["response_audit"].is_null(),
            "dev不得绑定静态响应审计",
        )?;
    }
    Ok(value)
}
fn command(
    api: &mut impl Dependencies,
    context: &Context,
    name: &str,
    index: usize,
    timeout: u64,
    processes: &mut Value,
) -> Result<()> {
    processes[name] = api.frontend(super::dependency::FrontendCommand {
        binding: &context.binding,
        arguments: &context.binding["commands"][index],
        environment: &environment(context)?,
        log: context.output(&format!("{name}_log"))?,
        directory: context.output(&format!("{name}_process"))?,
        timeout,
        secrets: &context.secrets,
    })?;
    Ok(())
}
fn build(
    api: &mut impl Dependencies,
    context: &Context,
    processes: &mut Value,
) -> Result<(Value, Value)> {
    command(api, context, "build", 0, 1800, processes)?;
    command(api, context, "build_verify_before", 1, 300, processes)?;
    let receipt = api.call("frontend-build", json!({"frontend":context.frontend}))?;
    let receipt = path(&receipt["path"])?;
    check(
        receipt == context.output("build_receipt")?,
        "前端构建收据路径不一致",
    )?;
    let guard = manifest::capture(
        &context.frontend.join("dist"),
        &context.frontend,
        (MAX_FILES, MAX_BYTES),
    )?;
    Ok((
        json!({"receipt":bound(&receipt)?,"dist":guard["manifest"]}),
        guard,
    ))
}
fn repeat(api: &mut impl Dependencies, paths: &Paths, context: &Context) -> Result<()> {
    context.unchanged()?;
    let repeated = plan::make(
        api,
        paths,
        text(&context.binding["run_id"])?,
        text(&context.binding["server"])?,
        false,
        false,
        false,
    )?;
    check(
        repeated.binding == context.binding,
        "浏览器运行期间来源、环境或端点发生变化",
    )
}
pub(super) fn ensure_worker(api: &mut impl Dependencies, paths: &Paths) -> Result<()> {
    let mut args = plan::runtime_args(paths);
    args["action"] = json!("status");
    let status = api.call("control", args.clone())?;
    check(
        status["processes"]["api"] == "running",
        "浏览器证据阶段要求API保持运行",
    )?;
    if status["processes"]["worker"] == "running" {
        return Ok(());
    }
    check(
        status["processes"]["worker"] == "stopped",
        "浏览器证据阶段Worker状态无效",
    )?;
    let mut start = args.clone();
    start["action"] = json!("start");
    check(
        api.call("worker", start)? == "running",
        "浏览器证据阶段未恢复Worker",
    )?;
    check(
        api.call("control", args)?["processes"]["worker"] == "running",
        "浏览器证据阶段Worker恢复未确认",
    )
}
fn run(
    api: &mut impl Dependencies,
    paths: &Paths,
    context: Context,
    binding_guard: Value,
    endpoint: &str,
) -> Result<Value> {
    run_with(api, paths, context, binding_guard, endpoint, write_new)
}
pub(super) fn run_with(
    api: &mut impl Dependencies,
    paths: &Paths,
    mut context: Context,
    binding_guard: Value,
    endpoint: &str,
    publish_intent: impl FnOnce(&Path, &Value) -> Result<()>,
) -> Result<Value> {
    let mut stage = "intent";
    let mut business_started = false;
    let mut processes = json!({});
    let result: Result<Value> = (|| {
        context.unchanged()?;
        publish_intent(
            context.output("intent")?,
            &review::intent(&context.binding, &paths.binding, &context)?,
        )?;
        stage = "tool_configuration";
        let tool =
            publication::tool_publish(&context.tool, &configuration(&context.binding, endpoint)?)?;
        context.input_guards.push(tool["guard"].clone());
        let preview = context.binding["server"] == "preview";
        stage = if preview { "build" } else { "browser" };
        let (build, build_guard) = if preview {
            build(api, &context, &mut processes)?
        } else {
            (Value::Null, Value::Null)
        };
        repeat(api, paths, &context)?;
        unchanged(&binding_guard)?;
        stage = "browser";
        business_started = true;
        browser_stage(api, &context, &mut processes)?;
        if preview {
            stage = "build_verify_after";
            verify_after(api, &context, &mut processes, &build, &build_guard)?;
        }
        stage = "evidence";
        let result = finish(
            api,
            paths,
            &context,
            &binding_guard,
            &processes,
            &build,
            &build_guard,
        )?;
        stage = "publish";
        Ok(result)
    })();
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            publish_failure(
                api,
                &context,
                paths,
                stage,
                business_started,
                &processes,
                error.as_ref(),
            )?;
            return Err(error);
        }
    };
    if let Err(error) = publication::terminal(&context.runtime, context.output("result")?, &result)
    {
        publish_failure(
            api,
            &context,
            paths,
            "publish",
            business_started,
            &processes,
            error.as_ref(),
        )?;
        return Err(error);
    }
    Ok(result)
}
fn browser_stage(
    api: &mut impl Dependencies,
    context: &Context,
    processes: &mut Value,
) -> Result<()> {
    let preview = context.binding["server"] == "preview";
    check(
        !context.output("login_budget")?.try_exists()?,
        "登录预算在浏览器首次写入前已出现",
    )?;
    if preview {
        check(
            !context.output("response_audit")?.try_exists()?,
            "静态响应审计在浏览器首次写入前已出现",
        )?;
    }
    command(
        api,
        context,
        "browser",
        if preview { 2 } else { 0 },
        3600,
        processes,
    )
}
fn verify_after(
    api: &mut impl Dependencies,
    context: &Context,
    processes: &mut Value,
    build: &Value,
    guard: &Value,
) -> Result<()> {
    command(api, context, "build_verify_after", 3, 300, processes)?;
    manifest::assert_unchanged(guard)?;
    let receipt = api.call("frontend-build", json!({"frontend":context.frontend}))?;
    check(
        bound(&path(&receipt["path"])?)? == build["receipt"],
        "浏览器期间生产构建收据变化",
    )
}
fn finish(
    api: &mut impl Dependencies,
    paths: &Paths,
    context: &Context,
    binding_guard: &Value,
    processes: &Value,
    build: &Value,
    build_guard: &Value,
) -> Result<Value> {
    ensure_worker(api, paths)?;
    repeat(api, paths, context)?;
    unchanged(binding_guard)?;
    let (artifacts, guards) = review::artifacts(context, build)?;
    let budget = receipts::budget(context.output("login_budget")?, &context.binding)?;
    let mut logs = json!({});
    for name in review::keys(text(&context.binding["server"])?) {
        logs[name] = bound(context.output(&format!("{name}_log"))?)?;
    }
    let result = json!({
        "format_version":1,"kind":"reference-fixture-browser-result","status":"passed",
        "run_id":context.binding["run_id"],"server":context.binding["server"],
        "binding":bound(&paths.binding)?,"intent":bound(context.output("intent")?)?,
        "build":build,"logs":logs,"processes":processes,"artifacts":artifacts,
        "login_budget":budget,"remote_writes":{"business_data":true}
    });
    let result_guards = plan::input_guards(&result)?;
    review::evidence(api, context, &paths.binding, &result)?;
    for guard in guards {
        manifest::assert_unchanged(&guard)?;
    }
    if !build_guard.is_null() {
        manifest::assert_unchanged(build_guard)?;
    }
    context.unchanged()?;
    unchanged(binding_guard)?;
    for guard in result_guards {
        unchanged(&guard)?;
    }
    Ok(result)
}
fn intent_evidence(path: &Path) -> Value {
    match path.try_exists() {
        Ok(false) => json!({"path":path,"status":"absent"}),
        Ok(true) => match bound(path) {
            Ok(mut v) => {
                v["status"] = json!("published");
                v
            }
            Err(_) => json!({"path":path,"status":"unreadable","error_type":"EvidenceReadError"}),
        },
        Err(_) => json!({"path":path,"status":"unreadable","error_type":"EvidenceReadError"}),
    }
}
fn publish_failure(
    api: &mut impl Dependencies,
    context: &Context,
    paths: &Paths,
    stage: &str,
    business_started: bool,
    completed: &Value,
    error: &(dyn std::error::Error + 'static),
) -> Result<()> {
    let mut failure = json!({"format_version":1,"kind":"reference-fixture-browser-failure","status":"failed","stage":stage,"binding":bound(&paths.binding)?,"intent":intent_evidence(context.output("intent")?),"error_type":"FixtureBrowserError","returncode":failure_exit_code(error),"unknown_business_writes":business_started,"processes":completed,"logs":{}});
    for name in review::keys(text(&context.binding["server"])?) {
        let directory = context.output(&format!("{name}_process"))?;
        if failure["processes"].get(name).is_none() && directory.is_dir() {
            failure["processes"][name] =
                process::partial(api, directory, text(&context.binding["scope_id"])?)?;
        }
        let log = context.output(&format!("{name}_log"))?;
        if log.is_file() {
            failure["logs"][name] = bound(log)?;
        }
    }
    for (key, label) in [
        ("result", "published_result"),
        ("login_budget", "login_budget"),
        ("response_audit", "response_audit"),
    ] {
        if let Some(path) = context.outputs.get(key)
            && path.is_file()
        {
            failure[label] = bound(path).unwrap_or_else(
                |_| json!({"status":"unreadable","error_type":"EvidenceReadError"}),
            );
        }
    }
    write_new(context.output("failure")?, &failure)
        .map_err(|_| "浏览器操作失败且失败证据发布未确认；保留全部现场，禁止重放".into())
}
