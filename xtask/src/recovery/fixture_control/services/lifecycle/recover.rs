use super::*;

pub(super) fn run(
    runtime: &mut dyn Runtime,
    context: &Context,
    owner_file: &Path,
) -> Result<Value> {
    let owner_path = context.owner(owner_file)?;
    let observed = controller(runtime, context)?;
    if observed.is_null() {
        return external(runtime, context, &owner_path);
    }
    let owner_binding = read(&owner_path)?;
    check(
        observed["owner"] == owner_binding && observed["process_missing"] == true,
        "只允许恢复明确绑定且确已死亡的原控制器；存活或 PID 复用均拒绝",
    )?;
    context.no_unknown()?;
    let _guard = ProcessGuard::acquire(&context.run)?;
    context.guard(None)?;
    let before = receipts(&context.run)?;
    let recovered = runtime.call(
        "recover-controller",
        json!({"run":context.run,"owner":owner_binding}),
    )?;
    let after = receipts(&context.run)?;
    let published = after
        .into_iter()
        .filter(|path| !before.contains(path))
        .collect::<Vec<_>>();
    check(
        published.len() == 1,
        "控制恢复没有发布唯一收尾收据，保留现场",
    )?;
    let receipt = binding(&published[0])?;
    context.guard(Some(&binding(&context.run.join("state.json"))?))?;
    owned(context, |owner| {
        let mut sources = context.sources();
        sources["state_before"] = binding(&context.run.join("state.json"))?;
        stage(
            context,
            "recover",
            &sources,
            owner,
            |_, controller, _, _| {
                let state = state::load(&context.run, true)?;
                let attempts = state::attempts(&state)?;
                let unsettled = attempts[..attempts.len() - 1]
                    .iter()
                    .any(|item| item["status"] != "passed");
                check(
                    recovered["status"] == "controller_recovered",
                    "控制恢复未返回已收尾状态",
                )?;
                result(
                    context,
                    "recover",
                    "controller_recovered",
                    json!({"controller":controller,"recovery":receipt,
                "next_operation":if unsettled {"reconcile-evidence"} else {"status"}}),
                )
            },
        )
    })
}

fn external(runtime: &mut dyn Runtime, context: &Context, owner_file: &Path) -> Result<Value> {
    let history = context.history(true)?;
    context.require_current_owner(owner_file)?;
    check(
        history["closed"] == false
            && history["external_recovery"].is_null()
            && history["unsettled"].as_array().is_some_and(Vec::is_empty)
            && history["initial_complete"] == true,
        "当前服务账本不允许重复或跨阶段核对外部终止",
    )?;
    context.no_unknown()?;
    let services = registered(runtime, context, true)?;
    let observed = incident::observe_registered(runtime, context, &services, None)?;
    check(
        !observed["termination"].is_null(),
        "只有全部登记服务明确外部终止时才能执行该恢复",
    )?;
    let _guard = ProcessGuard::acquire(&context.run)?;
    context.guard(None)?;
    check(
        controller(runtime, context)?.is_null(),
        "核对外部终止前出现其他控制器",
    )?;
    incident::check_registered(
        runtime,
        context,
        &services,
        &observed,
        "核对外部终止前服务状态变化",
    )?;
    owned(context, |owner| {
        let mut sources = context.sources();
        sources["external_owner"] = context.value["sources"]["state_before"].clone();
        stage(
            context,
            "recover",
            &sources,
            owner,
            |_, controller, running, output| {
                new_output(context, output)?;
                context.guard(Some(running))?;
                incident::check_registered(
                    runtime,
                    context,
                    &services,
                    &observed,
                    "发布外部终止证据前服务状态变化",
                )?;
                let file = output.join("external-termination.json");
                let evidence = &observed["termination"]["evidence"];
                let version = if evidence.get("host_reboot").is_some() {
                    incident::VERSION
                } else if evidence.get("redis_external").is_some() {
                    2
                } else {
                    1
                };
                json_io::write_new(
                    &file,
                    &json!({"format_version":version,"kind":"reference-fixture-external-termination", "run":context.run,
                "owner":context.value["sources"]["state_before"],"observation":observed["termination"],"normal_shutdown_proof":null,
                "remote_writes":0,"resources_deleted":false}),
                )?;
                context.guard(Some(running))?;
                incident::check_registered(
                    runtime,
                    context,
                    &services,
                    &observed,
                    "保存外部终止证据后服务状态变化",
                )?;
                result(
                    context,
                    "recover",
                    "external_termination_reconciled",
                    json!({"controller":controller,
                "owner":context.value["sources"]["state_before"],"evidence":binding(&file)?,"next_operation":"restart"}),
                )
            },
        )
    })
}

fn receipts(run: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for item in std::fs::read_dir(run)? {
        let item = item?;
        if item
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("recovered-") && name.ends_with(".json"))
        {
            paths.push(item.path());
        }
    }
    Ok(paths)
}
