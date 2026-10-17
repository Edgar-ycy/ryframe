use std::{
    collections::BTreeMap,
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use nix::{
    fcntl::AtFlags,
    unistd::{Gid, Uid, fchownat, geteuid, getgid, getuid},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::check::{TaskExecutor, TaskPlan};

use super::{
    CGROUP_ROOT, CgroupPlan, RUNNER_MARKER, TEST_CASE,
    error::{GateError, GateResult},
    evidence::{
        CleanupPreflight, CleanupResult, CleanupStatus, CreatedReceipt, PrepareResult, TestResult,
        inspect_cleanup, path_absent, read_cleanup_result, read_json, write_json_new,
        write_runner_failure,
    },
    plan::{ENVIRONMENT_ALLOWLIST, validate_plan_at, validate_test_output},
    processes::{append_registry, read_registry, register_children, verify_processes},
    stage,
};

#[path = "linux/fs.rs"]
mod cgroup_fs;
#[path = "linux/runner.rs"]
mod runner;

pub(super) use runner::run_cli as run_runner_cli;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn run_public(
    operation: super::super::DevexCgroupOperation,
    output: &Path,
    backend: &Path,
    task_plan: &TaskPlan,
) -> GateResult<()> {
    let expected = match operation {
        super::super::DevexCgroupOperation::Run => TaskExecutor::PerfCgroupRun,
        super::super::DevexCgroupOperation::Cleanup => TaskExecutor::PerfCgroupCleanup,
    };
    if task_plan.tasks.len() != 1 || task_plan.tasks[0].executor != expected {
        return Err(GateError::new(
            "plan",
            "cgroup 执行计划与登记的 DevEx 任务图不一致",
        ));
    }
    require_public_runner()?;
    match operation {
        super::super::DevexCgroupOperation::Run => execute_run(output, backend),
        super::super::DevexCgroupOperation::Cleanup => cleanup_public(output, backend),
    }
}

fn require_public_runner() -> GateResult<()> {
    if env::var("GITHUB_ACTIONS").as_deref() != Ok("true") || geteuid().as_raw() == 0 {
        return Err(GateError::new(
            "platform",
            "DevEx cgroup 门禁只允许 Linux GitHub Actions 的非 root runner 显式运行；不要在普通本机或以 root 直接执行",
        ));
    }
    Ok(())
}

fn execute_run(output: &Path, backend: &Path) -> GateResult<()> {
    create_output(output)?;
    let result = create_plan_and_execute(output, backend);
    match result {
        Ok(()) => Ok(()),
        Err(error) => match write_runner_failure(output, &error) {
            Ok(_) => Err(error),
            Err(receipt_error) => Err(GateError::new(
                error.kind(),
                format!("{error}；另外无法写入 runner 失败收据：{receipt_error}"),
            )),
        },
    }
}

fn create_output(output: &Path) -> GateResult<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| GateError::io("evidence", "创建 cgroup 证据父目录", parent, &error))?;
    }
    fs::create_dir(output).map_err(|error| {
        GateError::io(
            "evidence",
            "创建本次 cgroup 证据目录（不得覆盖历史运行）",
            output,
            &error,
        )
    })
}

fn create_plan_and_execute(output: &Path, backend: &Path) -> GateResult<()> {
    let plan = create_plan(backend)?;
    write_json_new(&output.join("plan.json"), &plan)?;

    let primary = stage::sudo_phase(&plan, output, super::evidence::CgroupPhase::Prepare)
        .and_then(|()| stage::sudo_phase(&plan, output, super::evidence::CgroupPhase::Test));
    let cleanup = stage::sudo_phase(&plan, output, super::evidence::CgroupPhase::Cleanup);
    combine_phase_results(primary, cleanup)
}

fn combine_phase_results(primary: GateResult<()>, cleanup: GateResult<()>) -> GateResult<()> {
    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Err(primary), Err(cleanup)) => Err(GateError::new(
            primary.kind(),
            format!("{primary}；随后 cleanup 阶段也失败：{cleanup}"),
        )),
    }
}

fn create_plan(backend: &Path) -> GateResult<CgroupPlan> {
    let run_id = required_environment("GITHUB_RUN_ID")?;
    let attempt = required_environment("GITHUB_RUN_ATTEMPT")?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let source_sha = git_head(backend)?;
    let github_sha = required_environment("GITHUB_SHA")?;
    if source_sha != github_sha {
        return Err(GateError::new(
            "source",
            format!(
                "实际 checkout {source_sha} 与本次 GitHub SHA {github_sha} 不匹配；拒绝用不同源码生成 cgroup 证据"
            ),
        ));
    }
    let cargo = find_cargo()?;
    let environment = ENVIRONMENT_ALLOWLIST
        .iter()
        .filter_map(|name| env::var(name).ok().map(|value| ((*name).to_owned(), value)))
        .collect::<BTreeMap<_, _>>();
    let plan = CgroupPlan {
        format_version: 1,
        root: CgroupPlan::root_for(Path::new(CGROUP_ROOT), &run_id, &attempt, &nonce),
        run_id,
        attempt,
        nonce,
        source_sha,
        uid: getuid().as_raw(),
        gid: getgid().as_raw(),
        backend: backend.to_path_buf(),
        cargo,
        environment,
    };
    validate_plan_at(&plan, backend, Path::new(CGROUP_ROOT))?;
    Ok(plan)
}

fn required_environment(name: &str) -> GateResult<String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            GateError::new(
                "environment",
                format!(
                    "缺少环境变量 {name}；cgroup 门禁必须在 Extended CI 的 GitHub runner 中执行"
                ),
            )
        })
}

fn git_head(backend: &Path) -> GateResult<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(backend)
        .output()
        .map_err(|error| {
            GateError::new(
                "source",
                format!(
                    "无法在 {} 执行 git rev-parse HEAD：{error}",
                    backend.display()
                ),
            )
        })?;
    if !output.status.success() {
        return Err(GateError::new(
            "source",
            format!(
                "git rev-parse HEAD 失败（{}）：{}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|error| GateError::new("source", format!("Git SHA 输出不是 UTF-8：{error}")))
}

fn find_cargo() -> GateResult<PathBuf> {
    let path = env::var_os("PATH").ok_or_else(|| {
        GateError::new(
            "environment",
            "PATH 为空，无法定位固定 Rust 工具链的 Cargo；请安装 rustup 后重新执行",
        )
    })?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join("cargo");
        if candidate
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            let candidate = if candidate.is_absolute() {
                candidate
            } else {
                env::current_dir()
                    .map_err(|error| {
                        GateError::new(
                            "environment",
                            format!("无法解析相对 Cargo 路径的当前目录：{error}"),
                        )
                    })?
                    .join(candidate)
            };
            return Ok(candidate);
        }
    }
    Err(GateError::new(
        "environment",
        "PATH 中找不到可执行 Cargo；请安装项目固定 Rust 工具链并确认 `cargo --version` 可用",
    ))
}

fn cleanup_public(output: &Path, backend: &Path) -> GateResult<()> {
    let plan_path = output.join("plan.json");
    if path_absent(&plan_path)? {
        println!(
            "DevEx cgroup cleanup：{} 没有本次计划，无需清理",
            output.display()
        );
        return Ok(());
    }
    let plan: CgroupPlan = read_json(&plan_path)?;
    validate_plan_at(&plan, backend, Path::new(CGROUP_ROOT))?;
    stage::sudo_phase(&plan, output, super::evidence::CgroupPhase::Cleanup)
}

pub(super) fn prepare(plan: &CgroupPlan, output: &Path) -> GateResult<PrepareResult> {
    let parent = cgroup_fs::parent_state(Path::new(CGROUP_ROOT))?;
    if !path_absent(&output.join("created.json"))? {
        return Err(GateError::new(
            "evidence",
            "created.json 已存在；拒绝覆盖或接管本次 cgroup",
        ));
    }
    fs::create_dir(&plan.root).map_err(|error| {
        GateError::io(
            "cgroup",
            "创建本次专属 cgroup（已有同名目录时必须失败）",
            &plan.root,
            &error,
        )
    })?;
    let identity = cgroup_fs::kernel_identity(&plan.root)?;
    let created = CreatedReceipt {
        root: plan.root.clone(),
        run_id: plan.run_id.clone(),
        attempt: plan.attempt.clone(),
        identity: identity.clone(),
        parent: parent.clone(),
    };
    if let Err(error) = write_json_new(&output.join("created.json"), &created) {
        let cleanup = fs::remove_dir(&plan.root);
        return Err(GateError::new(
            error.kind(),
            format!(
                "{error}；已尝试撤销刚创建的精确 cgroup 目录：{}",
                cleanup.map_or_else(|error| format!("失败（{error}）"), |()| "成功".to_owned())
            ),
        ));
    }
    configure_new_root(plan)?;
    Ok(PrepareResult {
        status: "prepared",
        root: plan.root.clone(),
        identity,
        parent,
        global_controller_modified: false,
        limits_modified: false,
    })
}

fn configure_new_root(plan: &CgroupPlan) -> GateResult<()> {
    cgroup_fs::write_control(&plan.root.join("cgroup.subtree_control"), "+memory")?;
    fs::create_dir(plan.root.join("driver")).map_err(|error| {
        GateError::io(
            "cgroup",
            "创建 driver cgroup",
            &plan.root.join("driver"),
            &error,
        )
    })?;
    fs::create_dir(plan.root.join("measure")).map_err(|error| {
        GateError::io(
            "cgroup",
            "创建 measure cgroup",
            &plan.root.join("measure"),
            &error,
        )
    })?;
    cgroup_fs::write_control(&plan.root.join("measure/cgroup.subtree_control"), "+memory")?;
    chown_delegated_paths(plan)
}

fn chown_delegated_paths(plan: &CgroupPlan) -> GateResult<()> {
    let uid = Uid::from_raw(plan.uid);
    let gid = Gid::from_raw(plan.gid);
    for directory in [&plan.root, &plan.root.join("measure")] {
        for path in [
            directory.to_path_buf(),
            directory.join("cgroup.procs"),
            directory.join("cgroup.threads"),
            directory.join("cgroup.subtree_control"),
        ] {
            fchownat(
                None,
                &path,
                Some(uid),
                Some(gid),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .map_err(|error| {
                GateError::new(
                    "cgroup",
                    format!(
                        "无法把 cgroup 委托路径 {} 交给 uid={} gid={}：{error}",
                        path.display(),
                        plan.uid,
                        plan.gid
                    ),
                )
            })?;
        }
    }
    Ok(())
}

pub(super) fn test(plan: &CgroupPlan, output: &Path) -> GateResult<TestResult> {
    owned_root(plan, output)?;
    let executable = env::current_exe().map_err(|error| {
        GateError::new(
            "test",
            format!("无法定位 Cargo 降权跳板可执行文件：{error}"),
        )
    })?;
    let mut command = Command::new(executable);
    command
        .arg(RUNNER_MARKER)
        .arg("--output")
        .arg(output)
        .current_dir(&plan.backend)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = command
        .spawn()
        .map_err(|error| GateError::new("test", format!("无法启动 Cargo 降权跳板：{error}")))?;
    let status = child
        .wait()
        .map_err(|error| GateError::new("test", format!("等待 Cargo 降权跳板失败：{error}")))?;
    if !status.success() {
        return Err(GateError::new(
            "test",
            format!(
                "Cargo 降权跳板失败（{status}）；查看 test-controller 日志及 cargo.log，随后由 cleanup 阶段精确回收本次 cgroup"
            ),
        ));
    }
    let log_path = output.join("cargo.log");
    let log = fs::read(&log_path)
        .map_err(|error| GateError::io("test", "读取 Cargo 验收日志", &log_path, &error))?;
    validate_test_output(0, &String::from_utf8_lossy(&log))?;
    let measure = plan.root.join("measure");
    let leftovers = fs::read_dir(&measure)
        .map_err(|error| GateError::io("test", "扫描测量 cgroup", &measure, &error))?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count();
    if leftovers != 0 {
        return Err(GateError::new(
            "test",
            format!(
                "Rust 测试后 {} 下仍有 {leftovers} 个测量子 cgroup 未清理",
                measure.display()
            ),
        ));
    }
    Ok(TestResult {
        status: "passed",
        case: TEST_CASE,
        exit_code: 0,
        uid: plan.uid,
        gid: plan.gid,
        cargo_log_sha256: sha256_hex(&log),
    })
}

pub(super) fn cleanup(plan: &CgroupPlan, output: &Path) -> GateResult<CleanupResult> {
    match inspect_cleanup(plan, output)? {
        CleanupPreflight::Reuse => read_cleanup_result(output),
        CleanupPreflight::NotCreated => Ok(CleanupResult::not_created(plan.root.clone())),
        CleanupPreflight::Proceed => cleanup_created(plan, output),
    }
}

fn cleanup_created(plan: &CgroupPlan, output: &Path) -> GateResult<CleanupResult> {
    let created = owned_root(plan, output)?;
    let frozen_events = cgroup_fs::freeze(&plan.root)?;
    let current = cgroup_fs::processes(&plan.root)?;
    let mut registry = read_registry(&super::processes::registry_path(output))?;
    let additions = register_children(&current, &mut registry, plan.uid)?;
    append_registry(&super::processes::registry_path(output), &additions)?;
    write_cleanup_observation(
        output,
        plan,
        &created.identity,
        &frozen_events,
        &current,
        &registry,
    )?;
    verify_processes(&current, &registry, plan.uid)?;
    if !current.is_empty() {
        cgroup_fs::write_control(&plan.root.join("cgroup.kill"), "1")?;
    }
    cgroup_fs::wait_until_empty(&plan.root, CLEANUP_TIMEOUT)?;
    let parent = cgroup_fs::parent_state(Path::new(CGROUP_ROOT))?;
    if parent != created.parent {
        return Err(GateError::new(
            "cgroup",
            "父 cgroup 的内核身份或 controller 状态在验收期间发生变化；拒绝把本次结果视为完整通过",
        ));
    }
    cgroup_fs::remove_directories(&plan.root)?;
    let root_absent = path_absent(&plan.root)?;
    if !root_absent {
        return Err(GateError::new(
            "cgroup",
            format!("精确清理后 cgroup 仍存在：{}", plan.root.display()),
        ));
    }
    Ok(CleanupResult {
        status: CleanupStatus::Cleaned,
        root: plan.root.clone(),
        identity: Some(created.identity),
        terminated_registered_processes: current.values().cloned().collect(),
        root_absent,
    })
}

#[derive(Serialize)]
struct CleanupObservation<'a> {
    root: &'a Path,
    identity: &'a super::evidence::KernelIdentity,
    frozen_events: &'a BTreeMap<String, String>,
    current: &'a super::processes::ProcessSnapshot,
    registered: &'a super::processes::ProcessRegistry,
}

fn write_cleanup_observation(
    output: &Path,
    plan: &CgroupPlan,
    identity: &super::evidence::KernelIdentity,
    frozen_events: &BTreeMap<String, String>,
    current: &super::processes::ProcessSnapshot,
    registry: &super::processes::ProcessRegistry,
) -> GateResult<()> {
    let path = output.join(format!(
        "cleanup-observation-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    write_json_new(
        &path,
        &CleanupObservation {
            root: &plan.root,
            identity,
            frozen_events,
            current,
            registered: registry,
        },
    )
}

pub(super) fn owned_root(plan: &CgroupPlan, output: &Path) -> GateResult<CreatedReceipt> {
    let created = super::evidence::read_created(output)?;
    if created.root != plan.root || created.run_id != plan.run_id || created.attempt != plan.attempt
    {
        return Err(GateError::new(
            "cgroup",
            "created.json 与当前 run、attempt 或精确 cgroup 路径不一致",
        ));
    }
    let identity = cgroup_fs::kernel_identity(&plan.root).map_err(|error| {
        GateError::new(
            error.kind(),
            format!("{error}；拒绝按路径猜测接管无法核验的 cgroup"),
        )
    })?;
    if identity != created.identity {
        return Err(GateError::new(
            "cgroup",
            format!(
                "cgroup 内核 device/inode 身份已改变（计划 {:?}，实际 {:?}），拒绝接管",
                created.identity, identity
            ),
        ));
    }
    Ok(created)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
