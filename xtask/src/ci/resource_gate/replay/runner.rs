use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::Instant,
};

use super::{
    decision::load_decision_after_command,
    environment::{ProcessEnvironment, ToolExecutables},
    frontend::install_frontend_dependencies,
    manifest::{MINIMUM_SUCCESSFUL_CASES, ReplayCase, ReplayManifest},
    path::{canonical_path, normalized_path, validate_inside},
    report::{CommandResult, ReplayPrime},
    sha256_file,
    workspace::{
        RepositoryIdentity, add_worktree, changed_file_count, changed_files, switch_worktree,
        verify_case_ranges,
    },
};

#[path = "command_protocol.rs"]
mod command_protocol;

use command_protocol::ReplayCommandRequest;

#[derive(Debug, Clone)]
pub(crate) struct PrimeSelection {
    pub(crate) case: ReplayCase,
    pub(crate) rust_crates: Vec<String>,
    pub(crate) change_count: usize,
}

pub(crate) struct CaseExecution<'a> {
    pub(crate) worktree: &'a Path,
    pub(crate) session: &'a Path,
    pub(crate) shared_target: &'a Path,
    pub(crate) case: &'a ReplayCase,
    pub(crate) command: &'a [String],
    pub(crate) base_environment: &'a ProcessEnvironment,
    pub(crate) tools: &'a ToolExecutables,
    pub(crate) runner_manifest: &'a Path,
    pub(crate) targeted: bool,
    pub(crate) order: i64,
    pub(crate) verify_inputs: &'a dyn Fn() -> Result<(), String>,
}

pub(crate) fn prime_cases(
    manifest: &ReplayManifest,
    backend: &RepositoryIdentity,
    frontend: &RepositoryIdentity,
) -> Result<Vec<PrimeSelection>, String> {
    select_prime_cases_with(
        &manifest.cases,
        |case| replay_case_rust_crates(backend, case),
        |case| {
            Ok(changed_file_count(&backend.root, &case.base, &case.head)?
                + changed_file_count(&frontend.root, &case.frontend_base, &case.frontend_head)?)
        },
    )
}

pub(crate) fn select_prime_cases_with(
    cases: &[ReplayCase],
    mut rust_crates: impl FnMut(&ReplayCase) -> Result<Vec<String>, String>,
    mut change_count: impl FnMut(&ReplayCase) -> Result<usize, String>,
) -> Result<Vec<PrimeSelection>, String> {
    let candidates = cases
        .iter()
        .filter(|case| case.expected == "pass" && case.targeted_mode == "targeted")
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err("replay 缺少可用于预热的成功定向案例".to_owned());
    }
    let mut selected = BTreeMap::<Vec<String>, (usize, ReplayCase, usize)>::new();
    for (index, case) in candidates.into_iter().enumerate() {
        let mut surface = rust_crates(case)?;
        surface.sort();
        surface.dedup();
        let count = change_count(case)?;
        let replace = selected
            .get(&surface)
            .is_none_or(|(_, _, current)| count > *current);
        if replace {
            selected.insert(surface, (index, case.clone(), count));
        }
    }
    let mut ordered = selected.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        right
            .0
            .len()
            .cmp(&left.0.len())
            .then_with(|| left.1.2.cmp(&right.1.2))
            .then_with(|| left.1.0.cmp(&right.1.0))
    });
    Ok(ordered
        .into_iter()
        .map(|(surface, (_, case, count))| PrimeSelection {
            case,
            rust_crates: surface,
            change_count: count,
        })
        .collect())
}

fn replay_case_rust_crates(
    backend: &RepositoryIdentity,
    case: &ReplayCase,
) -> Result<Vec<String>, String> {
    let mut crates = BTreeSet::new();
    for path in changed_files(&backend.root, &case.base, &case.head)? {
        let mut parts = path.split('/');
        let first = parts.next().unwrap_or_default();
        let second = parts.next();
        if first == "crates" {
            if let Some(name) = second.filter(|name| !name.is_empty()) {
                crates.insert(name.to_owned());
            }
        } else if first == "xtask" {
            crates.insert("xtask".to_owned());
        }
    }
    Ok(crates.into_iter().collect())
}

pub(crate) fn command_for_frontend(
    command: &[String],
    frontend: &Path,
) -> Result<Vec<String>, String> {
    let mut normalized = command.to_vec();
    match normalized
        .iter()
        .position(|value| value == "--frontend-dir")
    {
        Some(index) => {
            let value = normalized
                .get_mut(index + 1)
                .ok_or_else(|| "--frontend-dir 缺少路径参数".to_owned())?;
            *value = path_text(frontend)?;
        }
        None => {
            normalized.push("--frontend-dir".to_owned());
            normalized.push(path_text(frontend)?);
        }
    }
    Ok(normalized)
}

pub(crate) fn command_for_replay(
    command: &[String],
    frontend: &Path,
    runner_target: &Path,
    runner_manifest: &Path,
    options: &crate::cli::ResourceGateOptions,
) -> Result<Vec<String>, String> {
    if !command
        .iter()
        .take(2)
        .map(String::as_str)
        .eq(["cargo", "xtask"])
    {
        return Ok(command.to_vec());
    }
    let mut normalized = command_for_frontend(command, frontend)?;
    if command.iter().take(5).map(String::as_str).eq([
        "cargo",
        "xtask",
        "check",
        "ci",
        "resource-gate",
    ]) {
        append_gate_arguments(&mut normalized, options)?;
    }
    let mut expanded = vec![
        "cargo".to_owned(),
        "run".to_owned(),
        "--locked".to_owned(),
        "--manifest-path".to_owned(),
        path_text(runner_manifest)?,
        "--target-dir".to_owned(),
        path_text(runner_target)?,
        "-p".to_owned(),
        "xtask".to_owned(),
        "--".to_owned(),
    ];
    expanded.extend_from_slice(&normalized[2..]);
    Ok(expanded)
}

fn append_gate_arguments(
    command: &mut Vec<String>,
    options: &crate::cli::ResourceGateOptions,
) -> Result<(), String> {
    for (name, value) in [
        ("--base-sha", options.base_sha.as_deref()),
        ("--head-sha", options.head_sha.as_deref()),
        ("--frontend-ref", options.frontend_ref.as_deref()),
    ] {
        if let Some(value) = value {
            command.extend([name.to_owned(), value.to_owned()]);
        }
    }
    if options.targeted {
        command.push("--targeted".to_owned());
    }
    if let Some(path) = &options.decision_file {
        command.extend(["--decision-file".to_owned(), path_text(path)?]);
    }
    for (name, value) in [("--jobs", options.jobs), ("--test-jobs", options.test_jobs)] {
        if let Some(value) = value {
            command.extend([name.to_owned(), value.to_string()]);
        }
    }
    Ok(())
}

pub(crate) fn arm_order(index: usize) -> (bool, bool) {
    assert!(index >= 1, "replay case index 必须从 1 开始");
    if index % 2 == 1 {
        (true, false)
    } else {
        (false, true)
    }
}

pub(crate) fn execute_in_worktree(execution: CaseExecution<'_>) -> Result<CommandResult, String> {
    (execution.verify_inputs)()?;
    let backend_worktree = execution.worktree.join("b");
    let frontend_worktree = execution.worktree.join("f");
    let decision_path = execution.worktree.join("decision.json");
    let command_log = execution.worktree.join("command.log");
    let result = run_case_command(
        &execution,
        &backend_worktree,
        &frontend_worktree,
        &decision_path,
        &command_log,
    );
    // 漂移时保留命令日志、decision和配置现场，不能先清理再由外层发现。
    (execution.verify_inputs)()?;
    match result {
        Ok(result) => {
            cleanup_case_artifacts(&decision_path, &command_log)?;
            Ok(result)
        }
        Err(primary) => {
            let mut message = primary;
            if let Err(error) = cleanup_case_artifacts(&decision_path, &command_log) {
                message.push_str("；案例清理失败：");
                message.push_str(&error);
            }
            Err(message)
        }
    }
}

fn run_case_command(
    execution: &CaseExecution<'_>,
    backend_worktree: &Path,
    frontend_worktree: &Path,
    decision_path: &Path,
    command_log: &Path,
) -> Result<CommandResult, String> {
    switch_worktree(backend_worktree, &execution.case.head, "后端")?;
    switch_worktree(frontend_worktree, &execution.case.frontend_head, "前端")?;
    install_frontend_dependencies(
        execution.worktree,
        frontend_worktree,
        &execution.tools.corepack,
        execution.base_environment,
    )?;
    let environment = case_environment(execution)?;
    let target_root = canonical_path(execution.shared_target)
        .map_err(|error| format!("无法解析 replay Cargo target：{error}"))?;
    let command = command_for_replay(
        execution.command,
        frontend_worktree,
        &target_root.join("runner"),
        execution.runner_manifest,
        &crate::cli::ResourceGateOptions {
            base_sha: Some(execution.case.base.clone()),
            head_sha: Some(execution.case.head.clone()),
            frontend_ref: Some(execution.case.frontend_head.clone()),
            targeted: execution.targeted,
            decision_file: Some(decision_path.to_path_buf()),
            jobs: Some(12),
            test_jobs: Some(8),
        },
    )?;
    let request = ReplayCommandRequest::new(target_root.clone());
    let (status, duration_ms) = run_logged_command(
        &command,
        backend_worktree,
        &environment,
        command_log,
        &request,
    )?;
    let return_code = status_code(status);
    let decision = load_decision_after_command(decision_path, command_log, return_code)?;
    let passed = status.success();
    Ok(CommandResult {
        passed,
        return_code,
        duration_ms,
        order: execution.order,
        decision,
        target_fingerprint: super::sha256_text(&normalized_path(&target_root)),
        output_fingerprint: sha256_file(command_log)?,
        failure_tail: (!passed)
            .then(|| super::decision::bounded_log_tail(command_log))
            .transpose()?,
    })
}

fn case_environment(execution: &CaseExecution<'_>) -> Result<ProcessEnvironment, String> {
    let target_root = canonical_path(execution.shared_target)
        .map_err(|error| format!("无法解析 replay Cargo target：{error}"))?;
    validate_inside(&target_root, execution.session, "replay Cargo target")?;
    let mut environment = execution.base_environment.clone();
    environment.insert(
        "CARGO_TARGET_DIR".to_owned(),
        target_root.join("runner").as_os_str().to_owned(),
    );
    Ok(environment)
}

fn run_logged_command(
    command: &[String],
    working_directory: &Path,
    environment: &ProcessEnvironment,
    command_log: &Path,
    request: &ReplayCommandRequest,
) -> Result<(ExitStatus, u64), String> {
    let executable = command
        .first()
        .ok_or_else(|| "replay 命令为空".to_owned())?;
    let log = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(command_log)
        .map_err(|error| format!("无法创建 replay 命令日志：{error}"))?;
    let log_copy = log
        .try_clone()
        .map_err(|error| format!("无法复制 replay 命令日志句柄：{error}"))?;
    let input = request.input_file(
        command_log
            .parent()
            .ok_or_else(|| "replay 命令日志缺少受管父目录".to_owned())?,
    )?;
    let started = Instant::now();
    let status = Command::new(executable)
        .args(&command[1..])
        .current_dir(working_directory)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(log_copy))
        .stderr(Stdio::from(log))
        .status()
        .map_err(|error| format!("replay 命令无法启动 {executable}：{error}"))?;
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    Ok((status, duration_ms))
}

fn status_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map_or(1, |signal| -signal)
    }
    #[cfg(not(unix))]
    {
        1
    }
}

fn cleanup_case_artifacts(decision: &Path, log: &Path) -> Result<(), String> {
    let mut errors = Vec::new();
    for (path, label) in [(decision, "decision artifact"), (log, "命令日志")] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!("无法删除{label}：{error}")),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

pub(crate) fn create_shared_worktrees(
    backend: &RepositoryIdentity,
    frontend: &RepositoryIdentity,
    shared_worktree: &Path,
    prime: &ReplayCase,
) -> Result<(), String> {
    verify_case_ranges(backend, frontend, prime)?;
    fs::create_dir(shared_worktree)
        .map_err(|error| format!("无法创建 replay 共享 worktree：{error}"))?;
    add_worktree(
        &backend.root,
        &shared_worktree.join("b"),
        &prime.head,
        "后端",
    )?;
    add_worktree(
        &frontend.root,
        &shared_worktree.join("f"),
        &prime.frontend_head,
        "前端",
    )
}

pub(crate) fn prime_result(prime: &PrimeSelection, result: CommandResult) -> ReplayPrime {
    ReplayPrime {
        case: prime.case.name.clone(),
        rust_crates: prime.rust_crates.clone(),
        change_count: prime.change_count,
        result,
    }
}

pub(crate) fn minimum_successful_cases() -> usize {
    MINIMUM_SUCCESSFUL_CASES
}

fn path_text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("replay 路径不是有效 UTF-8：{}", path.display()))
}
