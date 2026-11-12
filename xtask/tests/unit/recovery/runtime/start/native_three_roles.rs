use super::{
    native_stop::{runtime_command, verify_completion},
    *,
};
use crate::recovery::clone::native::target_verify::authority::require_closed;
use std::{collections::BTreeSet, io};

const CANCELLED: &str = "隔离测试注入就绪等待取消";
const WRITEBACK: &str = "隔离测试注入原树发布后的状态写回故障";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Complete,
    LaunchPublished,
    Cancelled(&'static str),
    TreePublished,
}

impl Case {
    fn failure(self) -> Option<&'static str> {
        match self {
            Self::Cancelled(_) => Some(CANCELLED),
            Self::TreePublished => Some(WRITEBACK),
            Self::Complete | Self::LaunchPublished => None,
        }
    }

    fn started_roles(self) -> &'static [&'static str] {
        match self {
            Self::Cancelled("api") | Self::TreePublished => &["api"],
            _ => &ROLES,
        }
    }
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
    run_case(Case::Cancelled("frontend"))
}

#[test]
fn first_readiness_cancellation_reclaims_api_and_leaves_other_roles_unstarted() -> Result<()> {
    run_case(Case::Cancelled("api"))
}

#[test]
fn start_cleanup_reclaims_published_api_tree_before_state_writeback() -> Result<()> {
    run_case(Case::TreePublished)
}

fn run_case(case: Case) -> Result<()> {
    let mut fixture = Fixture::with_prefix("运行 配置-");
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
                if case.failure() != Some(error.to_string().as_str()) {
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
        check_failed(fixture, &program, &state, case)?;
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
        if case == Case::TreePublished {
            wait_ready(fixture, owner, role, &tree, Duration::from_secs(15))?;
            let persisted = strict::read(&fixture.root.join(STATE))?;
            let pending = &persisted["generations"][0]["roles"][role];
            check(
                role == "api"
                    && pending["state"] == "pending"
                    && pending["tree"].is_null()
                    && strict::read(&state.directory().join("api-tree.json"))? == tree,
                "写回故障前必须已发布原 API 树，持久状态仍为待启动",
            )?;
            return Err(io::Error::other(WRITEBACK).into());
        }
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
            if case == Case::Cancelled(role) {
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
    check(case == Case::Complete, "未触发隔离启动故障")?;
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
    check_public_configuration(fixture, &before, &published, &receipt)?;
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
    if case == Case::Complete {
        reject_unknown_generation_file(fixture, program, state)?;
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
    verify_closed(fixture, state, generation, case)
}

fn check_public_configuration(
    fixture: &Fixture,
    state: &[u8],
    launch: &[u8],
    receipt: &Value,
) -> Result<()> {
    for document in [state, launch] {
        let text = std::str::from_utf8(document)?;
        for credential in [
            "synthetic-cleanup-token",
            "synthetic-mysql-marker",
            "synthetic-private-marker",
        ] {
            check(!text.contains(credential), "普通恢复收据包含隔离凭据")?;
        }
    }
    for role in ["api", "worker"] {
        let configuration = &fixture.request["environment"]["configurations"][role]["directory"];
        let command = &receipt["processes"][role]["command"];
        check(
            *command
                == json!([
                    fixture.request["artifacts"][role]["executable"],
                    "--config-dir",
                    configuration
                ])
                && document::text(configuration, "恢复角色配置目录")?.contains("运行 配置-"),
            "公开收据缺少独立角色的完整私有配置目录参数",
        )?;
    }
    Ok(())
}

fn reject_unknown_generation_file(fixture: &Fixture, program: &Path, state: &State) -> Result<()> {
    let state_path = fixture.root.join(STATE);
    let launch_path = state.directory().join(LAUNCH);
    let before = fs::read(&state_path)?;
    let launch = fs::read(&launch_path)?;
    let unknown = state.directory().join("foreign.json");
    strict::write_new(&unknown, &json!({"test":"unknown-generation-file"}))?;
    let injected = fs::read(&unknown)?;
    let result = (|| {
        let refusal = runtime_command(fixture, program, "stop")
            .err()
            .ok_or("公开停止接受了未登记代次文件")?;
        check(
            refusal.to_string().contains("未登记文件"),
            &refusal.to_string(),
        )?;
        check(
            fs::read(&state_path)? == before
                && fs::read(&launch_path)? == launch
                && !fixture.control.join(REGISTRATION_LOCK).try_exists()?,
            "拒绝停止后原代次、启动收据或控制锁发生变化",
        )?;
        for role in ROLES {
            let item = &state.generation()["roles"][role];
            let operation = document::text(&item["operation_id"], "原角色操作")?;
            check(
                !actor::control_path(state.directory(), role, operation).try_exists()?,
                "发现未登记文件后仍发出了停止请求",
            )?;
            for member in ["monitor", "supervisor", "process"] {
                actor::require_identity(&item["tree"][member])?;
            }
        }
        Ok(())
    })();
    check(
        fs::read(&unknown)? == injected,
        "未登记文件发生变化，保留现场",
    )?;
    fs::remove_file(&unknown)?;
    result
}

fn check_failed(fixture: &Fixture, program: &Path, state: &State, case: Case) -> Result<()> {
    let generation = state.generation();
    let observation = runtime_command(fixture, program, "status")?;
    check(
        generation["status"] == "start_failed"
            && generation["error_type"] == "RuntimeStartError"
            && generation["launch"].is_null()
            && !state.directory().join(LAUNCH).try_exists()?
            && observation["persisted_status"] == "start_failed",
        "启动故障后未保留失败代次或错误签发了启动收据",
    )?;
    verify_closed(fixture, state, generation, case)
}

fn verify_closed(fixture: &Fixture, state: &State, generation: &Value, case: Case) -> Result<()> {
    let mut requested = Vec::new();
    for role in ROLES.into_iter().rev() {
        let item = &generation["roles"][role];
        let endpoint = &fixture.request["authority"][format!("{role}_endpoint")];
        require_closed(document::text(endpoint, "三角色监听")?)?;
        check(
            item["state"] == "stopped" && item["tree"] == state.generation()["roles"][role]["tree"],
            "关闭后未保存原角色树",
        )?;
        if !case.started_roles().contains(&role) {
            check(
                item["tree"].is_null()
                    && item["completion"].is_null()
                    && !state.directory().join(format!("{role}.log")).try_exists()?
                    && !state
                        .directory()
                        .join(format!("{role}.json"))
                        .try_exists()?
                    && !state
                        .directory()
                        .join(format!("{role}-tree.json"))
                        .try_exists()?,
                "首角色失败后仍启动了后续角色",
            )?;
            continue;
        }
        check(
            strict::read(&state.directory().join(format!("{role}-tree.json")))? == item["tree"],
            "关闭后保存的角色树与原发布身份不同",
        )?;
        let descendant = role == "api"
            && (case != Case::Cancelled("api")
                || fixture
                    .control
                    .join("product/descendant.pid")
                    .try_exists()?);
        verify_completion(fixture, generation, role, descendant)?;
        let operation = document::text(&item["operation_id"], "三角色关闭操作")?;
        let request = strict::read(&actor::control_path(state.directory(), role, operation))?;
        requested.push(document::integer(
            &request["requested_at_ns"],
            "原关闭时间",
        )?);
    }
    check(
        requested.windows(2).all(|pair| pair[0] < pair[1]),
        "三角色未按逆序请求停止",
    )
}
