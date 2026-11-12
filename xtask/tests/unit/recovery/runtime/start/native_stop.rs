use super::*;
use crate::recovery::fixture_control::services::lifecycle::readonly::restore_completion;

#[derive(Clone, Copy)]
enum StopCase {
    NaturalExit,
    UnrecordedTree,
}

#[test]
fn stop_keeps_the_original_completion_after_natural_api_exit() -> Result<()> {
    run_case(StopCase::NaturalExit)
}

#[test]
fn stop_reclaims_the_original_live_tree_before_state_writeback() -> Result<()> {
    run_case(StopCase::UnrecordedTree)
}

fn run_case(case: StopCase) -> Result<()> {
    let mut fixture = Fixture::new();
    // 公开 stop/status 入口在登记阶段会规范化 Windows 分隔符。
    fixture.root = fixture.control.join("case").join("runtime");
    let program = native_coordinator()?;
    let ports = configure(&mut fixture, &program)?;
    fs::write(fixture.control.join("product/worker-mode"), b"ready")?;
    let operation = start_operation(&fixture.binding)?;
    drop(ports);
    let result = run_with_original_trees(&fixture, &program, &operation, case);
    if let Err(error) = result {
        let path = fixture._work.keep();
        return Err(format!("{error}；原生停止测试证据：{}", path.display()).into());
    }
    check(
        !fixture.control.join(REGISTRATION_LOCK).exists(),
        "公开停止未释放原登记锁",
    )
}

fn run_with_original_trees(
    fixture: &Fixture,
    program: &Path,
    operation: &str,
    case: StopCase,
) -> Result<()> {
    let (mut state, mut attempts, api, natural) =
        Owner::with(&fixture.control, operation, |owner| {
            let mut state = fixture.start(owner)?;
            let mut attempts = Vec::new();
            match prepare_case(fixture, program, &mut state, owner, &mut attempts, case) {
                Ok((api, natural)) => Ok((state, attempts, api, natural)),
                Err(error) => Err(cleanup(
                    &fixture.backend,
                    owner,
                    &mut state,
                    &mut attempts,
                    error,
                )),
            }
        })?;
    let result = verify_and_stop(fixture, program, &state, case, &api, natural.as_ref());
    if let Err(error) = result {
        // 公开入口终态失败后，仅对本夹具实际启动且仍可确认身份的原树收口。
        let after = Owner::with(&fixture.control, operation, |owner| {
            Ok(cleanup(
                &fixture.backend,
                owner,
                &mut state,
                &mut attempts,
                "隔离停止回归收口".into(),
            ))
        });
        return match after {
            Ok(after) => Err(format!("{error}；{after}").into()),
            Err(after) => Err(format!("{error}；隔离停止回归取锁失败：{after}").into()),
        };
    }
    Ok(())
}

struct Completion {
    descriptor: Value,
    bytes: Vec<u8>,
}

fn prepare_case(
    fixture: &Fixture,
    program: &Path,
    state: &mut State,
    owner: &Owner,
    attempts: &mut Vec<(&'static str, Attempt)>,
    case: StopCase,
) -> Result<(Value, Option<Completion>)> {
    let api = start_role(fixture, state, owner, program, attempts, "api")?;
    wait_ready(fixture, owner, "api", &api, Duration::from_secs(8))?;
    if matches!(case, StopCase::UnrecordedTree) {
        check(
            state.generation()["roles"]["api"]["state"] == "pending"
                && state.generation()["roles"]["api"]["tree"].is_null()
                && strict::read(&state.directory().join("api-tree.json"))? == api,
            "启动中断夹具未停在原树已发布、状态未写回的边界",
        )?;
        return Ok((api, None));
    }
    record_started(state, owner, "api", &api)?;
    let worker = start_role(fixture, state, owner, program, attempts, "worker")?;
    record_started(state, owner, "worker", &worker)?;
    wait_ready(fixture, owner, "worker", &worker, Duration::from_secs(8))?;
    fs::write(fixture.control.join("product/api-natural-exit"), b"exit")?;
    let descriptor = restore_completion(&fixture.backend, &api)?;
    let path = document::binding_shape(&descriptor, "自然退出原收据")?.path;
    let bytes = fs::read(path)?;
    stop::require_exited(&api["process"])?;
    Ok((api, Some(Completion { descriptor, bytes })))
}

fn verify_and_stop(
    fixture: &Fixture,
    program: &Path,
    state: &State,
    case: StopCase,
    api: &Value,
    natural: Option<&Completion>,
) -> Result<()> {
    let state_path = fixture.root.join(STATE);
    let before = fs::read(&state_path)?;
    let observation = runtime_command(fixture, program, "status")?;
    let expected = match case {
        StopCase::NaturalExit => ("stopped", "running"),
        StopCase::UnrecordedTree => ("running", "pending"),
    };
    check(
        observation["persisted_status"] == "starting"
            && observation["processes"]["api"]["state"] == expected.0
            && observation["processes"]["worker"]["state"] == expected.1
            && observation["processes"]["api"]["tree"] == *api
            && fs::read(&state_path)? == before,
        "公开观察未保持原生命周期或未认领原进程树",
    )?;
    if let Some(natural) = natural {
        check(
            observation["processes"]["api"]["completion"] == natural.descriptor,
            "公开观察未保留自然退出的原关闭收据",
        )?;
    }
    let stopped = runtime_command(fixture, program, "stop")?;
    check(stopped["status"] == "stopped", "公开停止未确认当代已关闭")?;
    let saved = strict::read(&state_path)?;
    let generation = &saved["generations"][0];
    check(
        generation["status"] == "stopped"
            && generation["launch"].is_null()
            && !state.directory().join(LAUNCH).try_exists()?
            && ROLES
                .iter()
                .all(|role| generation["roles"][*role]["state"] == "stopped"),
        "公开停止未保存正确代次或错误签发 launch",
    )?;
    check(
        generation["roles"]["api"]["tree"] == *api,
        "停止未保持原 API 树",
    )?;
    verify_completion(fixture, generation, "api", true)?;
    if let Some(natural) = natural {
        let completion = &generation["roles"]["api"]["completion"];
        let path = document::binding_shape(completion, "停止后的原自然退出收据")?.path;
        check(
            *completion == natural.descriptor
                && fs::read(path)? == natural.bytes
                && !actor::control_path(
                    state.directory(),
                    "api",
                    api["operation_id"].as_str().unwrap(),
                )
                .try_exists()?,
            "自然退出后不应重签关闭收据或发送停止请求",
        )?;
        verify_completion(fixture, generation, "worker", false)?;
    }
    Ok(())
}

fn runtime_command(fixture: &Fixture, program: &Path, operation: &str) -> Result<Value> {
    let mut command = crate::process::child_command(program);
    command
        .current_dir(&fixture.backend)
        .args([
            "check",
            "recovery",
            "runtime",
            operation,
            "--runtime-registration",
        ])
        .arg(fixture.control.join("registration.json"))
        .arg("--target-plan")
        .arg(fixture.control.join("target.json"));
    if operation == "stop" {
        command.args(["--generation", "1", "--write"]);
    }
    let result = command.output()?;
    check(
        result.status.success(),
        &String::from_utf8_lossy(&result.stderr),
    )?;
    strict::parse_fragment(std::str::from_utf8(&result.stdout)?, "原生恢复控制结果")
}

fn verify_completion(
    fixture: &Fixture,
    generation: &Value,
    role: &str,
    descendant: bool,
) -> Result<()> {
    let item = &generation["roles"][role];
    let tree = &item["tree"];
    let (_, proof) = document::bound_file(&fixture.backend, &item["completion"], "停止原成员证明")?;
    check_restore_completion(tree, &proof.value)?;
    stop::require_exited(&tree["monitor"])?;
    let members = strict::array(&proof.value["members"], "原已退出成员")?;
    check(members.contains(&tree["process"]), "停止证明缺少原产品")?;
    for member in members {
        stop::require_exited(member)?;
    }
    if descendant {
        let marker = fixture.control.join("product/descendant.pid");
        let pid: u64 = fs::read_to_string(marker)?.parse()?;
        check(
            members.iter().any(|member| member["pid"] == pid),
            "停止证明缺少 API 后代",
        )?;
    }
    Ok(())
}
