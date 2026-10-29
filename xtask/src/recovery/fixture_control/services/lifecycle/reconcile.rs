use super::*;

pub(super) fn proof(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    failed: &Value,
    services: &Value,
    close: bool,
) -> Result<(Value, Value)> {
    if close {
        let (evidence, controller) = history::evidence::close_evidence(
            &context.run,
            failed,
            &history["active_generation"],
            &context.sources(),
            services,
        )?;
        let completion = runtime.call("completion", json!({"tree":services["tree"]}))?;
        check(
            completion == evidence["rustfs_members"],
            "RustFS 完整成员关闭证明在进程核对期间变化",
        )?;
        let observations = observe(runtime, services)?;
        check(
            observations == expected_stopped(),
            "失败 close 和解时服务或端口尚未完整停止",
        )?;
        Ok((
            observations,
            json!({"evidence":evidence,"failed_controller":controller}),
        ))
    } else {
        let recovery = read(Path::new(text(&history["external_recovery"], "path")?))?;
        let recorded = read(Path::new(text(&recovery["evidence"], "path")?))?;
        let sources = history::evidence::restart_evidence(
            &context.run,
            failed,
            &context.sources(),
            &history["external_recovery"],
        )?;
        let observations = observe(runtime, services)?;
        check(
            observations
                == json!({"redis":"stopped","rustfs":"external-termination-unreconciled","termination":recorded["observation"]}),
            "失败 restart 和解时服务未回到已核对的外部终止现场",
        )?;
        Ok((observations, sources))
    }
}

pub(super) fn run(
    runtime: &mut dyn Runtime,
    context: &Context,
    owner_file: &Path,
) -> Result<Value> {
    context.require_current_owner(owner_file)?;
    let history = context.history(false)?;
    let close = history::reconcilable(&context.value["state"], &history, "close");
    let restart = history::reconcilable(&context.value["state"], &history, "restart");
    check(
        close.is_some() != restart.is_some(),
        "只允许和解唯一失败 close 或外部终止核对后的失败 restart",
    )?;
    context.no_unknown()?;
    check(
        controller(runtime, context)?.is_null(),
        "不能在控制器存在时和解",
    )?;
    let failed = close.or(restart).ok_or("缺少可和解失败阶段")?;
    let is_close = close.is_some();
    let services = registered(runtime, context, false)?;
    if is_close
        && !context
            .run
            .join(format!(
                "lifecycle-{:04}/stopped.json",
                failed["number"].as_u64().ok_or("原关闭编号无效")?
            ))
            .try_exists()?
    {
        return super::reconcile_close::run(runtime, context, &history, failed, &services);
    }
    let before = proof(runtime, context, &history, failed, &services, is_close)?;
    let _guard = ProcessGuard::acquire(&context.run)?;
    owned(context, |owner| {
        context.guard(None)?;
        context.no_unknown()?;
        stage(
            context,
            "reconcile",
            &context.sources(),
            owner,
            |number, controller, running, output| {
                new_output(context, output)?;
                check(
                    proof(runtime, context, &history, failed, &services, is_close)? == before,
                    "发布和解前证据或服务状态发生变化",
                )?;
                let value = document(context, &history, failed, &before, number, is_close)?;
                let file = output.join("reconciliation.json");
                json_io::write_new(&file, &value)?;
                context.guard(Some(running))?;
                check(
                    proof(runtime, context, &history, failed, &services, is_close)? == before,
                    "和解证据写入后现场发生变化",
                )?;
                let mut details = json!({"controller":controller,"owner":context.value["sources"]["state_before"],
                "failed_attempt":failed["number"],"services":value["services"],"reconciliation":binding(&file)?});
                if !is_close {
                    details["external_termination"] = value["external_termination"].clone();
                }
                result(
                    context,
                    "reconcile",
                    if is_close {
                        "failed_close_reconciled"
                    } else {
                        "failed_restart_reconciled"
                    },
                    details,
                )
            },
        )
    })
}

pub(super) fn document(
    context: &Context,
    history: &Value,
    failed: &Value,
    proof: &(Value, Value),
    number: u64,
    close: bool,
) -> Result<Value> {
    let mut value = json!({"format_version":1,"kind":if close {"reference-fixture-failed-close-reconciliation"} else {"reference-fixture-failed-restart-reconciliation"},
        "run":context.run,"attempt":number,"owner":context.value["sources"]["state_before"],"failed_attempt":failed["number"],
        "failed_error_type":failed["error_type"],"failed_sources":failed["sources"],"failed_controller":proof.1["failed_controller"],
        "services":service_projection(&proof.0),"remote_writes":0,"resources_deleted":false});
    if close {
        value["evidence"] = proof.1["evidence"].clone();
    } else {
        let recovery = read(Path::new(text(&history["external_recovery"], "path")?))?;
        value["external_termination"] = recovery["evidence"].clone();
    }
    Ok(value)
}
