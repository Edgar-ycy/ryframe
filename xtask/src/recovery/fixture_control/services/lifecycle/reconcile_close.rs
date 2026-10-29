//! 在独立 reconcile 阶段收尾已正常退出的原 Redis；绝不重发 cache-stop。
use super::*;
use crate::recovery::clone::native::cache::host_control::recovery_port_error;

pub(super) fn run(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
) -> Result<Value> {
    let started = runtime.now();
    let original = candidate(context, history, failed, services)?;
    let observed = cache_observation(runtime, services)?;
    require_tree(runtime, services, true)?;
    let _guard = ProcessGuard::acquire(&context.run)?;
    owned(context, |owner| {
        context.guard(None)?;
        context.no_unknown()?;
        check(
            candidate(context, history, failed, services)? == original,
            "关闭恢复加锁前证据变化",
        )?;
        stage(
            context,
            "reconcile",
            &context.sources(),
            owner,
            |number, controller, running, output| {
                new_output(context, output)?;
                finish(
                    runtime,
                    context,
                    history,
                    failed,
                    services,
                    &original,
                    (number, controller, running, output, started, observed),
                )
            },
        )
    })
}

fn finish(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    original: &Value,
    stage: (u64, &Value, &Value, &Path, Duration, Value),
) -> Result<Value> {
    let (number, controller, running, output, started, observed) = stage;
    let deadline = started + Duration::from_secs(180);
    wait_port(
        runtime,
        context,
        history,
        failed,
        services,
        original,
        (running, deadline, true),
    )?;
    stable(
        runtime,
        context,
        history,
        failed,
        services,
        original,
        (running, true),
    )?;
    check(
        runtime.now() < deadline,
        "关闭恢复绑定与观察完成时已过原期限",
    )?;
    runtime.call("tree-stop", json!({"tree":services["tree"],"crash":false}))?;
    let completion = runtime.call("completion", json!({"tree":services["tree"]}))?;
    check(
        observe(runtime, services)? == expected_stopped(),
        "关闭恢复未完整证明成员退出及端口释放",
    )?;
    wait_port(
        runtime,
        context,
        history,
        failed,
        services,
        original,
        (running, deadline, false),
    )?;
    context.guard(Some(running))?;
    context.no_unknown()?;
    check(
        runtime.now() < deadline,
        "关闭恢复整树收尾超过本次原期限；保留已写入现场",
    )?;
    publish(
        runtime,
        context,
        history,
        failed,
        services,
        (original, observed, completion),
        (number, controller, running, output, started),
    )
}

fn wait_port(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    original: &Value,
    stage: (&Value, Duration, bool),
) -> Result<()> {
    let (running, deadline, tree_running) = stage;
    let mut last = None;
    loop {
        if runtime.now() >= deadline {
            return Err(recovery_port_error(last.unwrap_or_else(|| {
                std::io::Error::from(std::io::ErrorKind::TimedOut).into()
            })));
        }
        stable(
            runtime,
            context,
            history,
            failed,
            services,
            original,
            (running, tree_running),
        )?;
        if runtime.now() >= deadline {
            return Err(recovery_port_error(last.unwrap_or_else(|| {
                std::io::Error::from(std::io::ErrorKind::TimedOut).into()
            })));
        }
        match runtime.call(
            "cache-closed-port",
            json!({"request":services["requests"]["redis"],"runtime":services["runtime"]}),
        ) {
            Ok(value) => {
                check(value == true, "关闭恢复缺少实际独占端口证明")?;
                break;
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse) =>
            {
                last = Some(error)
            }
            Err(error) => return Err(recovery_port_error(error)),
        }
        let remaining = deadline.saturating_sub(runtime.now());
        if remaining.is_zero() {
            return Err(recovery_port_error(last.unwrap_or_else(|| {
                std::io::Error::from(std::io::ErrorKind::TimedOut).into()
            })));
        }
        runtime.pause(remaining.min(Duration::from_millis(500)));
    }
    Ok(())
}

fn publish(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    evidence: (&Value, Value, Value),
    stage: (u64, &Value, &Value, &Path, Duration),
) -> Result<Value> {
    let (original, observed, completion) = evidence;
    let (number, controller, running, output, started) = stage;
    let deadline = started + Duration::from_secs(180);
    let recovery_file = output.join("close-recovery.json");
    let receipt = json!({"format_version":1,"kind":"reference-fixture-close-recovery","owner":context.value["sources"]["state_before"],
        "candidate":original,"redis_observation":observed,"rustfs_tree":services["tree"],"rustfs_members":completion,
        "windows_port":{"port":services["requests"]["redis"]["port"],"bound_and_closed":true},
        "elapsed_millis":runtime.now().saturating_sub(started).as_millis(),"budget_seconds":180,"remote_writes":0,"resources_deleted":false});
    json_io::write_new(&recovery_file, &receipt)?;
    let evidence = history::evidence::recovery_evidence(
        &context.run,
        failed,
        &history["active_generation"],
        &context.sources(),
        services,
        &binding(&recovery_file)?,
    )?;
    let proof = (
        expected_stopped(),
        json!({"evidence":evidence,"failed_controller":original["failed_controller"]}),
    );
    let document = reconcile::document(context, history, failed, &proof, number, true)?;
    let file = output.join("reconciliation.json");
    json_io::write_new(&file, &document)?;
    context.guard(Some(running))?;
    require_tree(runtime, services, false)?;
    runtime.call(
        "cache-closed-port",
        json!({"request":services["requests"]["redis"],"runtime":services["runtime"]}),
    )?;
    check(runtime.now() < deadline, "关闭恢复发布后原期限或状态变化")?;
    check(
        history::evidence::recovery_evidence(
            &context.run,
            failed,
            &history["active_generation"],
            &context.sources(),
            services,
            &binding(&recovery_file)?,
        )? == proof.1["evidence"],
        "关闭恢复发布后证据变化",
    )?;
    check(
        runtime.now() < deadline,
        "关闭恢复证据核验后已超过本次原期限",
    )?;
    result(
        context,
        "reconcile",
        "failed_close_reconciled",
        json!({"controller":controller,"owner":context.value["sources"]["state_before"],
        "failed_attempt":failed["number"],"services":document["services"],"reconciliation":binding(&file)?}),
    )
}

fn candidate(
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
) -> Result<Value> {
    history::evidence::close_candidate(
        &context.run,
        failed,
        &history["active_generation"],
        &context.sources(),
        services,
        false,
    )
}

fn stable(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    original: &Value,
    stage: (&Value, bool),
) -> Result<()> {
    let (running, tree_running) = stage;
    context.guard(Some(running))?;
    context.no_unknown()?;
    check(
        history::evidence::close_candidate(
            &context.run,
            failed,
            &history["active_generation"],
            &context.sources(),
            services,
            !tree_running,
        )? == *original,
        "关闭恢复原失败前后像、退出时间或请求变化",
    )?;
    require_tree(runtime, services, tree_running)
}

fn require_tree(runtime: &mut dyn Runtime, services: &Value, running: bool) -> Result<()> {
    for role in ["supervisor", "monitor", "process"] {
        let expected = &services["tree"][role];
        let actual = runtime.call("identity", json!({"pid":expected["pid"]}))?;
        check(
            if running {
                actual == *expected
            } else {
                actual.is_null()
            },
            "关闭恢复原RustFS成员实际身份不符、PID复用或未退出",
        )?;
    }
    if running {
        check(runtime.call("storage-running",json!({"identity":services["tree"]["process"],"request":services["requests"]["rustfs"]}))? == true,"关闭恢复原RustFS命令、来源或监听归属变化")?;
    }
    Ok(())
}
fn cache_observation(runtime: &mut dyn Runtime, services: &Value) -> Result<Value> {
    let value = runtime.call("cache-status",json!({"request":services["requests"]["redis"],"runtime":services["runtime"],"effective":services["cache_effective"]}))?;
    check(
        value["state"] == "stopped"
            && value["launcher_alive"] == false
            && value["linux"]["alive"] == false
            && value["linux"]["terminated"] == false
            && value["linux"]["identity"] == services["runtime"]["linux_identity"]
            && value["linux"]["boot_id"] == services["runtime"]["boot_id"],
        "关闭恢复未观察到原Linux身份及launcher退出",
    )?;
    Ok(value)
}
