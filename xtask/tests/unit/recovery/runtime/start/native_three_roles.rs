use super::{
    native_stop::{runtime_command, verify_completion},
    *,
};
use crate::recovery::clone::native::target_verify::authority::require_closed;
use std::{collections::BTreeSet, io};

const CANCELLED: &str = "隔离测试注入就绪等待取消";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Complete,
    LaunchPublished,
    Cancelled,
}

#[test]
fn ordinary_three_role_generation_publishes_and_stops_original_trees() -> Result<()> {
    run_case(Case::Complete)
}

#[test]
fn stop_retains_published_launch_before_running_state_writeback() -> Result<()> {
    run_case(Case::LaunchPublished)
}

#[test]
fn readiness_cancellation_reclaims_three_role_trees_without_launch() -> Result<()> {
    run_case(Case::Cancelled)
}

fn run_case(case: Case) -> Result<()> {
    let mut fixture = Fixture::new();
    let frontend = Frontend::new()?;
    let result = run_fixture(&mut fixture, &frontend, case);
    if let Err(error) = result {
        let backend = fixture._work.keep();
        let frontend = frontend.keep();
        return Err(format!(
            "{error}；原三角色证据：{}；前端收据：{}",
            backend.display(),
            frontend.display()
        )
        .into());
    }
    Ok(())
}

fn run_fixture(fixture: &mut Fixture, frontend: &Frontend, case: Case) -> Result<()> {
    fixture.root = fixture.control.join("case").join("runtime");
    let program = native_coordinator()?;
    let artifact = compile_product(&fixture.control)?;
    let ports = configure_with_artifact(fixture, &program, &artifact, Some(frontend))?;
    fs::write(fixture.control.join("product/worker-mode"), b"ready")?;
    let operation = start_operation(&fixture.binding)?;
    drop(ports);
    let (mut state, mut attempts, launch) = Owner::with(&fixture.control, &operation, |owner| {
        let mut state = fixture.start(owner)?;
        let mut attempts = Vec::new();
        let result = start_and_publish(
            fixture,
            frontend,
            &program,
            owner,
            &mut state,
            &mut attempts,
            case,
        );
        let launch = match result {
            Ok(launch) => Some(launch),
            Err(error) => {
                let error = cleanup(&fixture.backend, owner, &mut state, &mut attempts, error);
                if case != Case::Cancelled || error.to_string() != CANCELLED {
                    return Err(error);
                }
                None
            }
        };
        for (_, attempt) in &mut attempts {
            attempt.release();
        }
        Ok((state, attempts, launch))
    })?;
    if let Some(launch) = launch {
        if let Err(error) = observe_and_stop(fixture, &program, &state, &launch, case) {
            let cleanup_result = Owner::with(&fixture.control, &operation, |owner| {
                Ok(cleanup(
                    &fixture.backend,
                    owner,
                    &mut state,
                    &mut attempts,
                    "隔离三角色测试失败后关闭原树".into(),
                ))
            });
            return Err(match cleanup_result {
                Ok(cleaned) => format!("{error}；{cleaned}").into(),
                Err(cleanup_error) => format!("{error}；隔离测试收口失败：{cleanup_error}").into(),
            });
        }
    } else {
        check_cancelled(fixture, &program, &state)?;
    }
    check(
        !fixture.control.join(REGISTRATION_LOCK).exists(),
        "三角色回归未释放原控制锁",
    )
}

fn start_and_publish(
    fixture: &Fixture,
    frontend: &Frontend,
    program: &Path,
    owner: &Owner,
    state: &mut State,
    attempts: &mut Vec<(&'static str, Attempt)>,
    case: Case,
) -> Result<Value> {
    let mut pids = BTreeSet::new();
    for role in ROLES {
        let tree = start_role(fixture, state, owner, program, attempts, role)?;
        record_started(state, owner, role, &tree)?;
        for member in ["monitor", "supervisor", "process"] {
            check(
                pids.insert(document::integer(&tree[member]["pid"], "三角色进程")?),
                "三角色实际进程身份重叠",
            )?;
        }
        let mut probe = json!({"role":role,
            "url":fixture.request["authority"][format!("{role}_endpoint")]});
        if role == "frontend" {
            probe["files"] = frontend.files.clone();
        }
        readiness::wait(&tree, &probe, Duration::from_secs(15), || {
            owner.check()?;
            if case == Case::Cancelled && role == "frontend" {
                return Err(io::Error::new(io::ErrorKind::Interrupted, CANCELLED).into());
            }
            Ok(())
        })?;
    }
    if case == Case::LaunchPublished {
        // 调用正式写入器后停在状态写回之前，保留起始状态原字节。
        return state.runtime.model(
            "write-launch",
            state.directory(),
            state.generation(),
            &Value::Null,
            &fixture.commands,
            state.number(),
        );
    }
    check(case == Case::Complete, "未触发就绪取消")?;
    state.publish_launch(owner)
}

fn observe_and_stop(
    fixture: &Fixture,
    program: &Path,
    state: &State,
    launch: &Value,
    case: Case,
) -> Result<()> {
    let state_path = fixture.root.join(STATE);
    let before = fs::read(&state_path)?;
    let launch_path = state.directory().join(LAUNCH);
    let published = fs::read(&launch_path)?;
    let receipt = strict::read(&launch_path)?;
    check(
        file_binding(fixture, &launch_path)? == *launch,
        "发布的原启动收据身份不同",
    )?;
    let observation = runtime_command(fixture, program, "status")?;
    let persisted = if case == Case::LaunchPublished {
        "starting"
    } else {
        "running"
    };
    check(
        observation["status"] == "running"
            && observation["persisted_status"] == persisted
            && observation["generation"] == 1
            && fs::read(&state_path)? == before,
        "公开观察未识别原代次或写入了持久状态",
    )?;
    let expected_launch = if case == Case::LaunchPublished {
        Value::Null
    } else {
        launch.clone()
    };
    check(
        observation["launch"] == expected_launch,
        "公开观察改写了启动发布边界",
    )?;
    for role in ROLES {
        let tree = &state.generation()["roles"][role]["tree"];
        let observed = &observation["processes"][role];
        check(
            observed["state"] == "running"
                && observed["tree"] == *tree
                && receipt["processes"][role]["tree"] == *tree
                && receipt["processes"][role]["process_receipt"]["identity"] == tree["process"],
            "原启动收据与实际角色身份不同",
        )?;
    }
    let stopped = runtime_command(fixture, program, "stop")?;
    check(stopped["status"] == "stopped", "公开停止未完成三角色代次")?;
    let saved = strict::read(&state_path)?;
    let generation = &saved["generations"][0];
    check(
        generation["status"] == "stopped"
            && generation["launch"] == *launch
            && fs::read(&launch_path)? == published,
        "公开停止未保留原启动收据或重签发布",
    )?;
    verify_closed(fixture, state, generation)
}

fn check_cancelled(fixture: &Fixture, program: &Path, state: &State) -> Result<()> {
    let generation = state.generation();
    let observation = runtime_command(fixture, program, "status")?;
    check(
        generation["status"] == "start_failed"
            && generation["error_type"] == "RuntimeStartError"
            && generation["launch"].is_null()
            && !state.directory().join(LAUNCH).try_exists()?
            && observation["persisted_status"] == "start_failed",
        "取消后未保留失败代次或错误签发了启动收据",
    )?;
    verify_closed(fixture, state, generation)
}

fn verify_closed(fixture: &Fixture, state: &State, generation: &Value) -> Result<()> {
    let mut requested = Vec::new();
    for role in ROLES.into_iter().rev() {
        let item = &generation["roles"][role];
        check(
            item["state"] == "stopped" && item["tree"] == state.generation()["roles"][role]["tree"],
            "关闭后未保存原角色树",
        )?;
        verify_completion(fixture, generation, role, role == "api")?;
        let operation = document::text(&item["operation_id"], "三角色关闭操作")?;
        let request = strict::read(&actor::control_path(state.directory(), role, operation))?;
        requested.push(document::integer(
            &request["requested_at_ns"],
            "原关闭时间",
        )?);
        let endpoint = &fixture.request["authority"][format!("{role}_endpoint")];
        require_closed(document::text(endpoint, "三角色监听")?)?;
    }
    check(
        requested.windows(2).all(|pair| pair[0] < pair[1]),
        "三角色未按逆序请求停止",
    )
}
