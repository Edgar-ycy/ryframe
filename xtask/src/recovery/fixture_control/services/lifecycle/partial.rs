//! 首代只允许核验后关闭实际启动者；保留失败证据，不能成为续建或重启前驱。
use super::*;
use crate::recovery::clone::native::resources::common;

#[path = "partial/inputs.rs"]
mod inputs;
#[path = "partial/proof.rs"]
pub(in crate::recovery::fixture_control::services) mod proof;

pub(super) fn close(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
) -> Result<Value> {
    context.no_unknown()?;
    check(
        controller(runtime, context)?.is_null(),
        "部分关闭前仍有当前或遗留控制器",
    )?;
    check(
        history["unsettled"].as_array().is_some_and(Vec::is_empty),
        "部分关闭有未确认结果，禁止重放",
    )?;
    let services = inputs::load(runtime, context)?;
    if history["closed"] == true {
        stopped(runtime, &services)?;
        return result(
            context,
            "close",
            "services_already_partially_closed",
            json!({"initial_complete":false}),
        );
    }
    running(runtime, &services)?;
    let _guard = ProcessGuard::acquire(&context.run)?;
    owned(context, |owner| {
        stable(runtime, context, &services, None)?;
        stage(
            context,
            "close",
            &context.sources(),
            owner,
            |_, controller, current, output| {
                stop(runtime, context, &services, controller, current, output)
            },
        )
    })
}

fn stop(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    controller: &Value,
    current: &Value,
    output: &Path,
) -> Result<Value> {
    new_output(context, output)?;
    stable(runtime, context, services, Some(current))?;
    if !services["runtime"].is_null() {
        let cache = runtime.call("cache-stop", json!({"request":services["requests"]["redis"],
                    "private":context.value["private"],"runtime":services["runtime"],"output":output}))?;
        check(
            cache["status"] == "redis_process_stopped",
            "部分关闭未确认 Redis 退出",
        )?;
    }
    stable(runtime, context, services, Some(current))?;
    if !services["tree"].is_null() {
        runtime.call("tree-stop", json!({"tree":services["tree"],"crash":false}))?;
    }
    stopped(runtime, services)?;
    let completion = if services["tree"].is_null() {
        Value::Null
    } else {
        runtime.call("completion", json!({"tree":services["tree"]}))?
    };
    stable(runtime, context, services, Some(current))?;
    let file = output.join("partial-closed.json");
    let receipt = proof::create(context, services, output, completion)?;
    json_io::write_new(&file, &receipt)?;
    let result = result(
        context,
        "close",
        "services_partially_closed",
        json!({"initial_complete":false,
                "controller":controller,"services":receipt["services"],"evidence":binding(&file)?}),
    )?;
    let ledger = state::load(&context.run, true)?;
    let attempt = state::attempts(&ledger)?
        .last()
        .ok_or("部分关闭缺少当前阶段")?;
    let number = usize::try_from(attempt["number"].as_u64().ok_or("部分关闭阶段编号无效")?)?;
    proof::validate(&context.run, number, attempt, &result)?;
    Ok(result)
}

fn stable(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    current: Option<&Value>,
) -> Result<()> {
    context.guard(current)?;
    context.no_unknown()?;
    check(
        inputs::load(runtime, context)? == *services,
        "部分关闭期间初代证据、请求或未启动事实变化",
    )
}

fn running(runtime: &mut dyn Runtime, services: &Value) -> Result<()> {
    if !services["runtime"].is_null() {
        let observed = runtime.call(
            "cache-status",
            json!({"request":services["requests"]["redis"],
            "runtime":services["runtime"],"effective":null}),
        )?;
        check(
            observed["state"] == "running",
            "部分关闭前 Redis 不再是登记运行身份",
        )?;
    }
    if !services["tree"].is_null() {
        let tree = &services["tree"];
        let directory = Path::new(text(tree, "runtime_directory")?);
        let operation = text(tree, "operation_id")?;
        for filename in [
            format!("rustfs-tree-{operation}-control.json"),
            format!("rustfs-tree-{operation}-result.json"),
            format!("rustfs-members-{operation}-stopped.json"),
        ] {
            check(
                !directory.join(filename).try_exists()?,
                "部分关闭前已有停止或未知控制结果，禁止重放",
            )?;
        }
        check(
            observation::identities(runtime, tree)?
                == json!({"supervisor":"running","monitor":"running","process":"running"}),
            "部分关闭前 RustFS 完整身份无法确认；禁止终止未知进程",
        )?;
        check(
            runtime.call(
                "storage-running",
                json!({"identity":tree["process"],"request":services["requests"]["rustfs"]}),
            )? == true,
            "部分关闭前 RustFS 产品身份已变化",
        )?;
    }
    Ok(())
}

fn stopped(runtime: &mut dyn Runtime, services: &Value) -> Result<()> {
    if !services["runtime"].is_null() {
        let observed = runtime.call(
            "cache-status",
            json!({"request":services["requests"]["redis"],
            "runtime":services["runtime"],"effective":null}),
        )?;
        check(
            observed["state"] == "stopped",
            "部分关闭后 Redis 尚未精确退出",
        )?;
    }
    if !services["tree"].is_null() {
        check(
            observation::identities(runtime, &services["tree"])?
                == json!({"supervisor":"missing","monitor":"missing","process":"missing"}),
            "部分关闭后 RustFS 仍有存活或未知成员",
        )?;
        runtime.call("members-wait", json!({"tree":services["tree"]}))?;
        observation::closed(&services["requests"]["rustfs"])?;
    }
    Ok(())
}

pub(super) fn verify_closed(runtime: &mut dyn Runtime, context: &Context) -> Result<Value> {
    let services = inputs::load(runtime, context)?;
    stopped(runtime, &services)?;
    let mut states = json!({});
    for role in ["rustfs", "redis"] {
        states[role] = json!(if services["not_started"].get(role).is_some() {
            "not_started"
        } else {
            "stopped"
        });
    }
    Ok(states)
}

pub(super) fn can_close(runtime: &mut dyn Runtime, context: &Context) -> Result<()> {
    let services = inputs::load(runtime, context)?;
    running(runtime, &services)
}
