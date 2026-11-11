use super::*;

pub(super) fn run(runtime: &mut dyn Runtime, context: &Context) -> Result<Value> {
    let initial = history::validate(&context.run, &context.value["state"], false, true)?;
    if initial["initial_complete"] != true {
        return partial::close(runtime, context, &initial);
    }
    let history = context.history(true)?;
    let recipient_guard = super::handoff::lock_recipient(context, &history)?;
    context.no_unknown()?;
    check(
        controller(runtime, context)?.is_null(),
        "存在当前或遗留控制器，不能开始服务关闭",
    )?;
    let services = registered(runtime, context, true)?;
    check(
        observe(runtime, &services)?["termination"].is_null(),
        "服务遭外部终止，必须先 recover 核对现场",
    )?;
    if history["closed"] == true {
        return result(
            context,
            "close",
            "services_already_closed",
            json!({"services":closed(runtime,&services)?}),
        );
    }
    let _guard = ProcessGuard::acquire(&context.run)?;
    owned(context, |owner| {
        context.guard(None)?;
        if let Some(guard) = &recipient_guard {
            guard.check()?;
            observation::closed(&services["requests"]["rustfs"])?;
        }
        stage(
            context,
            "close",
            &context.sources(),
            owner,
            |_, controller, running, output| {
                new_output(context, output)?;
                stable(runtime, context, &services, running)?;
                let cache = runtime.call(
                    "cache-stop",
                    json!({"request":services["requests"]["redis"],
                "private":context.value["private"],"runtime":services["runtime"],"output":output,
                "context":context.value,"running":running}),
                )?;
                stable(runtime, context, &services, running)?;
                let terminated =
                    runtime.call("tree-stop", json!({"tree":services["tree"],"crash":false}))?;
                let observations = closed(runtime, &services)?;
                let completion = runtime.call("completion", json!({"tree":services["tree"]}))?;
                stable(runtime, context, &services, running)?;
                let file = output.join("rustfs-stopped.json");
                json_io::write_new(
                    &file,
                    &json!({"tree":services["tree"],"terminated":terminated,"state":"stopped","completion":completion}),
                )?;
                check(
                    cache["status"] == "redis_process_stopped",
                    "Redis 关闭未返回明确结果",
                )?;
                result(
                    context,
                    "close",
                    "services_closed",
                    json!({"services":service_projection(&observations),"controller":controller,
                "evidence":{"redis":binding(&output.join("stopped.json"))?,"rustfs":binding(&file)?}}),
                )
            },
        )
    })
}

fn stable(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    running: &Value,
) -> Result<()> {
    context.guard(Some(running))?;
    context.no_unknown()?;
    check(
        registered(runtime, context, true)? == *services,
        "关闭期间服务请求、进程树或运行收据发生变化",
    )
}

pub(super) fn closed(runtime: &mut dyn Runtime, services: &Value) -> Result<Value> {
    let start = runtime.now();
    loop {
        let mut pending = false;
        for role in ["supervisor", "process"] {
            let expected = &services["tree"][role];
            let observation = runtime.call("identity", json!({"pid":expected["pid"]}));
            match observation {
                Err(error) if model::category(error.as_ref()) == "PermissionError" => {
                    pending = true
                }
                Err(error) => return Err(error),
                Ok(current) => pending |= !current.is_null() && current == *expected,
            }
        }
        if !pending {
            let observations = observe(runtime, services)?;
            check(
                observations == expected_stopped(),
                "夹具服务完整进程树尚未退出",
            )?;
            return Ok(observations);
        }
        check(
            runtime.now().saturating_sub(start) < Duration::from_secs(5),
            "关闭夹具服务后原进程身份仍存活或无法确认",
        )?;
        runtime.pause(Duration::from_millis(50));
    }
}
