use super::*;
use crate::{
    cli::{RuntimeSources, RuntimeVerifyOptions},
    recovery::{
        clone::native::lifecycle::lock::ProcessGuard,
        runtime::{
            REGISTRATION_GUARD, dataset_authority::path, live_verify::verify_live_generation_with,
            verify,
        },
    },
};

#[test]
fn live_verify_holds_owner_and_rejects_generation_directory_drift() -> Result<()> {
    let (fixture, ports) = prepared_fixture()?;
    drop(ports);
    if let Err(error) = verify_cases(&fixture) {
        let path = fixture._work.keep();
        return Err(format!("{error}；在线代次回归证据：{}", path.display()).into());
    }
    Ok(())
}

#[test]
fn online_rejects_worker_configuration_drift_before_any_probe() -> Result<()> {
    let (fixture, listeners) = prepared_fixture()?;
    if let Err(error) = verify_configuration_preprobe(&fixture, &listeners) {
        let path = fixture._work.keep();
        return Err(format!("{error}；探针前配置回归证据：{}", path.display()).into());
    }
    Ok(())
}

fn prepared_fixture() -> Result<(Fixture, [TcpListener; 3])> {
    let mut fixture = Fixture::new();
    fixture.root = fixture.control.join("case").join("runtime");
    let program = portable_path(&std::env::current_exe()?.canonicalize()?);
    let ports = configure_with_artifact(&mut fixture, &program, &program)?;
    for key in ["bindings", "backend_build", "frontend_build"] {
        let file = path(&fixture.request["paths"][key])?;
        strict::write_new(&file, &json!({}))?;
        fixture.request["digests"][key] = file_binding(&fixture, &file)?["sha256"].clone();
    }
    fixture.commands = commands::expected(&fixture.request, &fixture.request["tools"])?;
    Ok((fixture, ports))
}

fn verify_configuration_preprobe(fixture: &Fixture, listeners: &[TcpListener; 3]) -> Result<()> {
    let launch = publish_running(fixture)?;
    let options = runtime_receipt(fixture, &launch)?;
    let receipt = document::read(&options.receipt, &fixture.backend, "探针前运行收据")?;
    let request = &launch.value["request"];
    let authority = &request["authority"];

    let baseline = online_error(fixture, &options, &receipt, authority)?;
    check(
        baseline == "恢复静态权威绑定必须为对象",
        &format!("原配置应通过校验并到达缺少比较绑定的夹具边界：{baseline}"),
    )?;
    check_no_probes(listeners)?;

    let directory = path(&request["environment"]["configurations"]["worker"]["directory"])?;
    let drift = directory.join("app.dev.toml");
    let bytes = b"unselected = true\n";
    check(!drift.try_exists()?, "夹具已存在未登记的 Worker 配置")?;
    fs::write(&drift, bytes)?;
    let error = online_error(fixture, &options, &receipt, authority)?;
    check(
        error == "恢复启动角色配置快照发生漂移",
        &format!("应在探针前拒绝真实 Worker 配置漂移：{error}"),
    )?;
    check_no_probes(listeners)?;
    for original in [&launch, &receipt] {
        original.assert_unchanged()?;
    }
    check(fs::read(drift)? == bytes, "应保留原漂移文件")
}

fn online_error(
    fixture: &Fixture,
    options: &RuntimeVerifyOptions,
    receipt: &Document,
    authority: &Value,
) -> Result<String> {
    let window = Window::direct()?;
    let mut inputs = Inputs::new(&fixture.backend);
    let result = verify::online(
        &fixture.backend,
        &options.sources,
        &options.bindings,
        &receipt.value,
        authority,
        &mut inputs,
    );
    let error = window
        .finish(inputs.finish(result))
        .err()
        .ok_or("夹具应在发出探针前拒绝在线核验")?;
    Ok(error.to_string())
}

fn check_no_probes(listeners: &[TcpListener; 3]) -> Result<()> {
    for (role, listener) in ROLES.into_iter().zip(listeners) {
        listener.set_nonblocking(true)?;
        check(
            listener
                .accept()
                .err()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock),
            &format!("前置配置核验期间 {role} 不应收到探针连接"),
        )?;
    }
    Ok(())
}

fn verify_cases(fixture: &Fixture) -> Result<()> {
    let launch = publish_running(fixture)?;
    let directory = path(&launch.value["runtime_directory"])?;
    let lifecycle = document::read(
        &fixture.root.join(STATE),
        &fixture.backend,
        "在线代次生命周期",
    )?;
    let options = runtime_receipt(fixture, &launch)?;
    let receipt = document::read(&options.receipt, &fixture.backend, "在线代次运行收据")?;

    verify_stable(fixture, &options, &receipt, &lifecycle)?;
    let foreign = directory.join("foreign.json");
    verify_drift(fixture, &options, &receipt, &foreign)?;

    check(fs::read(foreign)? == b"foreign", "应保留原外来文件")?;
    for original in [&launch, &lifecycle, &receipt] {
        original.assert_unchanged()?;
    }
    Ok(())
}

fn publish_running(fixture: &Fixture) -> Result<Document> {
    let operation = start_operation(&fixture.binding)?;
    Owner::with(&fixture.control, &operation, |owner| {
        let mut state = fixture.start(owner)?;
        // 只用结构夹具发布代次，锁回归不启动这些角色。
        for (ordinal, role) in ROLES.into_iter().enumerate() {
            let tree = fixture.seed_role(&state, role, ordinal)?;
            record_started(&mut state, owner, role, &tree)?;
        }
        let descriptor = state.publish_launch(owner)?;
        let launch = state.directory().join(LAUNCH);
        let original = document::read(&launch, &fixture.backend, "在线代次启动收据")?;
        check(
            state.generation()["status"] == "running" && document::binding(&original) == descriptor,
            "夹具未发布原三角色运行代次",
        )?;
        Ok(original)
    })
}

fn runtime_receipt(fixture: &Fixture, launch: &Document) -> Result<RuntimeVerifyOptions> {
    let request = &launch.value["request"];
    let authority = &request["authority"];
    let roots = &request["roots"];
    let paths = &request["paths"];
    let mut digests = request["digests"].clone();
    digests["launch"] = json!(launch.sha256());
    let mut processes = json!({});
    for role in ROLES {
        let record = &launch.value["processes"][role]["process_receipt"];
        processes[role] = json!({
            "receipt_path":record["path"],"receipt_sha256":record["sha256"],
            "identity":record["identity"],
        });
    }
    let receipt = fixture.control.join("runtime.json");
    strict::write_new(
        &receipt,
        &json!({
            "format_version":3,"kind":"restore-runtime",
            "restore":{"id":authority["restore_id"],"backup_id":authority["backup_id"],
                "scope_id":authority["scope_id"],"plan_hash":authority["plan_hash"],
                "data_verified_at":authority["data_verified_at"]},
            "paths":{"backend_product_root":roots["backend_product"],
                "backend_execution_root":roots["backend_execution"],"frontend_root":roots["frontend"],
                "runtime_dir":launch.value["runtime_directory"],"bindings":paths["bindings"],
                "backend_build":paths["backend_build"],"frontend_build":paths["frontend_build"],
                "launch":launch.path},
            "digests":digests,
            "source":{"backup_source_sha":authority["backup_source_sha"],
                "backend_product_sha":authority["backend_product_sha"],
                "backend_execution_sha":authority["backend_execution_sha"],
                "backend_adapter_contract":authority["backend_adapter_contract"],
                "frontend_sha":authority["frontend_sha"]},
            "endpoints":{"api":authority["api_endpoint"],"worker":authority["worker_endpoint"],
                "frontend":authority["frontend_endpoint"]},
            "backend":strict::read(&path(&paths["backend_build"])?)?,
            "frontend":strict::read(&path(&paths["frontend_build"])?)?,
            "processes":processes,
        }),
    )?;
    Ok(RuntimeVerifyOptions {
        sources: RuntimeSources {
            source_backend: path(&roots["backend_execution"])?,
            source_frontend: path(&roots["frontend"])?,
            product_backend: None,
            adapter_contract: None,
        },
        bindings: path(&paths["bindings"])?,
        receipt,
    })
}

fn verify_stable(
    fixture: &Fixture,
    options: &RuntimeVerifyOptions,
    receipt: &Document,
    lifecycle: &Document,
) -> Result<()> {
    let mut calls = 0;
    let window = Window::begin()?;
    let result = verify_live_generation_with(
        &fixture.backend,
        options,
        &receipt.sha256(),
        &window,
        |observed, _| {
            calls += 1;
            check(observed.raw == receipt.raw, "未使用原运行收据")?;
            check_owner(fixture)?;
            let rewrite = fs::write(&lifecycle.path, &lifecycle.raw);
            check(
                rewrite
                    .as_ref()
                    .err()
                    .and_then(std::io::Error::raw_os_error)
                    == Some(32),
                "锁内读取窗口应以 Windows 共享冲突阻止原生命周期写回",
            )?;
            Ok(json!({"receipt_sha256":observed.sha256()}))
        },
    )?;
    check(
        calls == 1 && result["receipt_sha256"] == receipt.sha256(),
        "锁内在线正文必须使用同一收据并执行一次",
    )?;
    check_owner_released(fixture)?;
    window.finish(Ok(()))
}

fn verify_drift(
    fixture: &Fixture,
    options: &RuntimeVerifyOptions,
    receipt: &Document,
    foreign: &Path,
) -> Result<()> {
    let mut calls = 0;
    let window = Window::begin()?;
    let result = verify_live_generation_with(
        &fixture.backend,
        options,
        &receipt.sha256(),
        &window,
        |_, _| {
            calls += 1;
            check_owner(fixture)?;
            fs::write(foreign, b"foreign")?;
            Ok(json!({"status":"verified"}))
        },
    );
    let error = result.err().ok_or("锁内代次目录漂移未被拒绝")?;
    let message = error.to_string();
    check(
        calls == 1 && message.contains("未登记文件") && message.contains("输入目录成员漂移"),
        &format!("应保留代次与窗口的实际后验错误：{message}"),
    )?;
    check_owner_released(fixture)
}

fn check_owner(fixture: &Fixture) -> Result<()> {
    let path = fixture.control.join(REGISTRATION_LOCK).join("owner.json");
    // owner 自身由锁内控制复核，不能交给外层窗口持有读句柄。
    let owner = strict::parse_fragment(&fs::read_to_string(path)?, "在线代次 owner")?;
    let registration = &fixture.binding["registration"]["sha256"];
    let target = &fixture.binding["target_plan"]["sha256"];
    let operation = format!(
        "runtime-verify:{}:{}:1",
        document::digest(registration, "运行登记摘要")?,
        document::digest(target, "目标计划摘要")?,
    );
    check(
        owner["operation"] == operation && owner["identity"]["pid"] == std::process::id(),
        "锁内 owner 未绑定原登记、目标和代次",
    )?;
    let contender = ProcessGuard::acquire_named(&fixture.control, REGISTRATION_GUARD);
    check(
        contender
            .err()
            .is_some_and(|error| error.to_string().contains("并发")),
        "锁内竞争者必须被同一内核 guard 拒绝",
    )
}

fn check_owner_released(fixture: &Fixture) -> Result<()> {
    check(
        !fixture.control.join(REGISTRATION_LOCK).try_exists()?,
        "在线代次核验未释放原 owner",
    )?;
    ProcessGuard::acquire_named(&fixture.control, REGISTRATION_GUARD)?;
    Ok(())
}
