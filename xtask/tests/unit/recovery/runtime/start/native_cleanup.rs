use super::*;
use crate::recovery::{
    fixture_control::portable_path,
    runtime::{
        REGISTRATION_LOCK,
        dataset_authority::{check, commands, environment},
        start::{actor, cleanup, launch::Attempt, readiness},
    },
};
use std::{net::TcpListener, process::Command, time::Duration};

#[path = "native_stop.rs"]
mod native_stop;

#[test]
fn worker_readiness_failure_reclaims_started_role_trees_without_launch_receipt() -> Result<()> {
    let mut fixture = Fixture::new();
    let program = native_coordinator()?;
    let ports = configure(&mut fixture, &program)?;
    let operation = start_operation(&fixture.binding)?;
    drop(ports);
    let result = Owner::with(&fixture.control, &operation, |owner| {
        let mut state = fixture.start(owner)?;
        let mut attempts = Vec::new();
        let result = start_two_roles(&fixture, &mut state, owner, &program, &mut attempts);
        let error = result
            .err()
            .unwrap_or_else(|| "未触发 Worker 就绪失败".into());
        let error = cleanup(&fixture.backend, owner, &mut state, &mut attempts, error);
        for (_, attempt) in &mut attempts {
            attempt.release();
        }
        check(
            error.to_string() == "恢复运行角色就绪超时",
            &error.to_string(),
        )?;
        owner.check()?;
        assert_failure_evidence(&fixture, &state)
    });
    if let Err(error) = result {
        let path = fixture._work.keep();
        return Err(format!("{error}；原生恢复测试证据：{}", path.display()).into());
    }
    assert!(!fixture.control.join(REGISTRATION_LOCK).exists());
    Ok(())
}

fn native_coordinator() -> Result<PathBuf> {
    let executable = option_env!("CARGO_BIN_EXE_xtask")
        .ok_or("恢复进程回归须由 Cargo internal 测试提供原生 xtask 路径")?;
    Ok(portable_path(&Path::new(executable).canonicalize()?))
}

fn start_operation(binding: &Value) -> Result<String> {
    let registration = document::digest(&binding["registration"]["sha256"], "运行登记摘要")?;
    let target = document::digest(&binding["target_plan"]["sha256"], "目标计划摘要")?;
    Ok(format!("runtime-start:{registration}:{target}"))
}

fn configure(fixture: &mut Fixture, coordinator: &Path) -> Result<[TcpListener; 3]> {
    let git = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&fixture.backend)
        .output()?;
    check(git.status.success(), "无法创建隔离 Git 根")?;
    fs::write(fixture.backend.join("Cargo.toml"), b"[workspace]\n")?;
    fs::create_dir(fixture.backend.join("xtask"))?;
    let listeners = [
        TcpListener::bind("127.0.0.1:0")?,
        TcpListener::bind("127.0.0.1:0")?,
        TcpListener::bind("127.0.0.1:0")?,
    ];
    let [api, worker, frontend] = listeners
        .each_ref()
        .map(|listener| listener.local_addr().unwrap().port());
    let native = compile_product(&fixture.control)?;
    let execution = fixture.control.join("product");
    let api_dir = execution.join("config");
    let worker_dir = execution.join("worker-config");
    let frontend_root = fixture.control.join("frontend");
    for path in [&api_dir, &worker_dir, &frontend_root] {
        fs::create_dir_all(path)?;
    }
    let scope = "recovery-native-cleanup";
    write_configuration(&api_dir, &worker_dir, &execution, scope, api, worker)?;
    let registration = fixture.control.join("registration.json");
    let sidecar = crate::recovery::runtime::dataset_authority::configuration_registration::sidecar(
        &registration,
    )?;
    fs::create_dir(&sidecar)?;
    let private = sidecar.join("environment.json");
    strict::write_new(
        &private,
        &json!({
            "metrics_bearer_token":"synthetic-cleanup-token","scope_id":scope,
            "api_config_dir":api_dir,"worker_config_dir":worker_dir,
        }),
    )?;
    let authority = authority(scope, api, worker, frontend);
    fixture.binding = register_fixture(fixture, &registration, &private, &authority)?;
    let paths = json!({"bindings":fixture.control.join("bindings.json"),
        "backend_build":fixture.control.join("backend-build.json"),
        "frontend_build":frontend_root.join("frontend-build.json")});
    let roots =
        json!({"backend_product":execution,"backend_execution":execution,"frontend":frontend_root});
    let artifact = digest::file(&native)?;
    let artifact =
        json!({"executable":native,"command":[],"bytes":artifact.bytes,"sha256":artifact.sha256});
    let tool = digest::file(coordinator)?;
    let tools = json!({"coordinator_root":fixture.backend,"coordinator_head":"a".repeat(40),
        "inventory_sha256":"b".repeat(64),"frontend_server":{"path":coordinator,"bytes":tool.bytes,"sha256":tool.sha256}});
    fixture.request = json!({
        "format_version":2,"kind":"restore-runtime-launch-request","runtime_directory":fixture.root,
        "authority":authority,"registration":fixture.binding,
        "roots":roots,"paths":paths,"digests":{"bindings":"a".repeat(64),"backend_build":"b".repeat(64),"frontend_build":"c".repeat(64)},
        "artifacts":{"api":artifact,"worker":artifact},"tools":tools,
    });
    fixture.request["environment"] =
        environment::capture(&mut Inputs::new(&fixture.backend), &fixture.request)?;
    fixture.commands = commands::expected(&fixture.request, &tools)?;
    Ok(listeners)
}

fn register_fixture(
    fixture: &Fixture,
    registration: &Path,
    private: &Path,
    authority: &Value,
) -> Result<Value> {
    let reference = fixture.control.join("reference.json");
    strict::write_new(
        &reference,
        &json!({
            "target_side":"candidate","target":{"scope_id":authority["scope_id"],
                "runtime_dir":fixture.root,"frontend_url":authority["frontend_endpoint"]},
        }),
    )?;
    let target = fixture.control.join("target.json");
    strict::write_new(
        &target,
        &json!({
            "target_side":"candidate","fresh_target":{},"maintenance_execution":{},"product_execution":{},
            "product_plan":{"scope_id":authority["scope_id"],"frontend_sha":authority["frontend_sha"],
                "api_ready_url":authority["api_endpoint"],"worker_ready_url":authority["worker_endpoint"]},
        }),
    )?;
    let target = file_binding(fixture, &target)?;
    let (_, observed) = runtime_observation(&fixture.backend, &fixture.root)?;
    strict::write_new(
        registration,
        &json!({
            "format_version":2,"kind":"restore-runtime-registration","remote_writes":0,
            "reference_plan":file_binding(fixture,&reference)?,"target_plan":target,
            "observation":{"runtime":observed,"ports_idle":["api","worker","frontend"]},
            "runtime_environment":file_binding(fixture,private)?,
        }),
    )?;
    Ok(json!({"registration":file_binding(fixture,registration)?,"target_plan":target}))
}

fn authority(scope: &str, api: u16, worker: u16, frontend: u16) -> Value {
    json!({"format_version":2,"kind":"restore-runtime-authority","scope_id":scope,
        "restore_id":"restore-1","backup_id":"backup-1","plan_hash":"1".repeat(64),
        "data_verified_at":"2026-09-06T00:00:00Z","backup_source_sha":"2".repeat(40),
        "backend_product_sha":"3".repeat(40),"backend_execution_sha":"3".repeat(40),
        "frontend_sha":"4".repeat(40),"backend_adapter_contract":null,
        "api_endpoint":format!("http://127.0.0.1:{api}/readyz"),
        "worker_endpoint":format!("http://127.0.0.1:{worker}/readyz"),
        "frontend_endpoint":format!("http://127.0.0.1:{frontend}"),
    })
}

fn write_configuration(
    api: &Path,
    worker: &Path,
    execution: &Path,
    scope: &str,
    api_port: u16,
    worker_port: u16,
) -> Result<()> {
    let base = format!(
        "scope_id = \"{scope}\"\n[monitor]\nmetrics_bearer_token = \"synthetic-cleanup-token\"\n"
    );
    fs::write(
        api.join("app.test.toml"),
        format!(
            "snowflake_worker_id = 2\n{base}[app]\nhost=\"127.0.0.1\"\nport={api_port}\n[jobs]\nmode=\"external\"\n"
        ),
    )?;
    fs::write(
        worker.join("app.test.toml"),
        format!(
            "snowflake_worker_id = 3\n{base}[jobs]\nmode=\"external\"\nhealth_host=\"127.0.0.1\"\nhealth_port={worker_port}\n"
        ),
    )?;
    fs::write(execution.join("api-mode"), b"tree-ready-json")?;
    fs::write(execution.join("worker-mode"), b"idle")?;
    Ok(())
}

fn compile_product(control: &Path) -> Result<PathBuf> {
    let product = control.join("recovery-role.exe");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/full_stack_worker.rs");
    let compile = Command::new("rustc")
        .args([
            "--edition=2024",
            "-C",
            "debuginfo=0",
            "-C",
            "linker=rust-lld.exe",
        ])
        .arg(source)
        .arg("-o")
        .arg(&product)
        .output()?;
    check(
        compile.status.success(),
        &String::from_utf8_lossy(&compile.stderr),
    )?;
    Ok(portable_path(&product.canonicalize()?))
}

fn file_binding(fixture: &Fixture, path: &Path) -> Result<Value> {
    let value = document::read(path, &fixture.backend, "原生恢复测试文件")?;
    Ok(document::binding(&value))
}

fn start_two_roles(
    fixture: &Fixture,
    state: &mut State,
    owner: &Owner,
    program: &Path,
    attempts: &mut Vec<(&'static str, Attempt)>,
) -> Result<()> {
    for (role, budget) in [
        ("api", Duration::from_secs(8)),
        ("worker", Duration::from_secs(2)),
    ] {
        let tree = start_role(fixture, state, owner, program, attempts, role)?;
        record_started(state, owner, role, &tree)?;
        wait_ready(fixture, owner, role, &tree, budget)?;
    }
    Ok(())
}

fn start_role(
    fixture: &Fixture,
    state: &State,
    owner: &Owner,
    program: &Path,
    attempts: &mut Vec<(&'static str, Attempt)>,
    role: &'static str,
) -> Result<Value> {
    attempts.push((role, Attempt::new(&fixture.backend, state, role)?));
    attempts.last_mut().unwrap().1.start(state, owner, program)
}

fn record_started(state: &mut State, owner: &Owner, role: &str, tree: &Value) -> Result<()> {
    state.update(owner, |generation| {
        generation["roles"][role]["tree"] = tree.clone();
        generation["roles"][role]["state"] = json!("running");
        Ok(())
    })
}

fn wait_ready(
    fixture: &Fixture,
    owner: &Owner,
    role: &str,
    tree: &Value,
    budget: Duration,
) -> Result<()> {
    let probe = json!({"role":role,"url":fixture.request["authority"][format!("{role}_endpoint")]});
    readiness::wait(tree, &probe, budget, || owner.check())
}

fn assert_failure_evidence(fixture: &Fixture, state: &State) -> Result<()> {
    let generation = state.generation();
    let directory = state.directory();
    check(
        generation["status"] == "start_failed" && generation["error_type"] == "RuntimeStartError",
        "恢复失败代次未完整登记",
    )?;
    check(
        generation["launch"].is_null() && !directory.join(LAUNCH).try_exists()?,
        "失败启动不能签发 launch",
    )?;
    check(
        generation["roles"]["frontend"]["state"] == "stopped"
            && generation["roles"]["frontend"]["tree"].is_null()
            && !directory.join("frontend.log").try_exists()?,
        "前端应保持未启动",
    )?;
    let mut stop_requests = Vec::new();
    for role in ["worker", "api"] {
        let item = &generation["roles"][role];
        check(
            item["state"] == "stopped" && item["tree"].is_object(),
            "缺少原角色关闭状态或树",
        )?;
        let (_, proof) =
            document::bound_file(&fixture.backend, &item["completion"], "原角色关闭收据")?;
        check_restore_completion(&item["tree"], &proof.value)?;
        stop::require_exited(&item["tree"]["monitor"])?;
        let members = strict::array(&proof.value["members"], "原已退出成员")?;
        check(
            members.contains(&item["tree"]["process"]),
            "原产品未进入完整退出收据",
        )?;
        for member in members {
            stop::require_exited(member)?;
        }
        let control = actor::control_path(directory, role, item["operation_id"].as_str().unwrap());
        stop_requests.push(strict::read(&control)?["requested_at_ns"].as_u64().unwrap());
        if role == "api" {
            let marker = fixture.control.join("product/descendant.pid");
            let descendant: u64 = fs::read_to_string(marker)?.parse()?;
            check(
                members.iter().any(|member| member["pid"] == descendant),
                "原 API 后代未进入完整退出收据",
            )?;
        }
    }
    check(
        stop_requests[0] < stop_requests[1],
        "已启动角色未按逆序关闭",
    )
}
