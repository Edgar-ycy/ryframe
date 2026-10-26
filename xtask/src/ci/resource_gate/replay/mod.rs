//! 在隔离 Git worktree 中比较 resource targeted 与 full gate 的通过/失败结果。

use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::{
    Result, cli::ResourceGateReplayOptions, process::PreservedFailure, workspace::root_dir,
};

mod activation;
mod decision;
mod environment;
mod frontend;
mod manifest;
mod path;
mod report;
mod runner;
mod session;
use session::{cleanup_replay, prepare_workspace};
mod workspace;

#[allow(unused_imports)]
pub(crate) use decision::{load_decision, load_decision_after_command, require_arm};
#[allow(unused_imports)]
pub(crate) use environment::{
    ProcessEnvironment, ReplayTools, ToolExecutables, base_environment, collect_tools,
    controlled_environment, ensure_sccache_server, recorded_environment, reset_sccache_stats,
    resolve_tool_executables, sccache_error_count, sccache_stats, stop_sccache,
};
#[allow(unused_imports)]
pub(crate) use frontend::{frontend_dependency_fingerprint, install_frontend_dependencies};
#[allow(unused_imports)]
pub(crate) use manifest::{
    MINIMUM_CASES, MINIMUM_SUCCESSFUL_CASES, ReplayCase, ReplayManifest, load_manifest,
    report_allowed_root, validate_case_coverage,
};
use path::{canonical_path, ensure_real_directory, resolve_requested_path, validate_inside};
#[allow(unused_imports)]
pub(crate) use report::{
    CommandResult, ReplayEvidence, ReplayPrime, ReplayResult, ReplayRun, ResourceGateDecision,
    TARGETED_P95_LIMIT_MS, activation_cache_error, environment_fingerprint,
    percentile_nearest_rank, replay_activation_eligible, replay_mismatches, repository_record,
    successful_targeted_runs, tools_fingerprint, validated_report_path, write_report,
};
#[allow(unused_imports)]
pub(crate) use runner::{
    CaseExecution, arm_order, command_for_frontend, command_for_replay, create_shared_worktrees,
    execute_in_worktree, minimum_successful_cases, prime_cases, prime_result,
    select_prime_cases_with,
};
#[allow(unused_imports)]
pub(crate) use workspace::{
    RepositoryIdentity, cleanup_created_worktrees, cleanup_worktree_with,
    cleanup_worktree_with_attempts, fail_on_cleanup, git_output, prune_worktrees,
    remove_isolated_tree, remove_partial_worktree_directory, repository_identity,
    switch_worktree_with, validate_repository_pair, verify_case_ranges,
};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
type ReplayStep<T> = std::result::Result<T, String>;

#[derive(Debug)]
struct ReplayRequest {
    backend: PathBuf,
    frontend: PathBuf,
    manifest: PathBuf,
    work_dir: PathBuf,
    report: PathBuf,
    activation_gate: bool,
}

#[derive(Debug, Clone, Copy)]
struct ReplayRuntime<'a> {
    base_environment: &'a ProcessEnvironment,
    tools: Option<&'a ToolExecutables>,
}

struct ReplayWorkspace {
    session: PathBuf,
    cache: PathBuf,
    shared_target: PathBuf,
    shared_worktree: PathBuf,
}

struct ReplayContext<'a> {
    backend: &'a RepositoryIdentity,
    frontend: &'a RepositoryIdentity,
    manifest: &'a ReplayManifest,
    workspace: &'a ReplayWorkspace,
    environment: &'a ProcessEnvironment,
    tools: &'a ReplayTools,
    runner_manifest: &'a Path,
    activation: &'a std::sync::Arc<activation::Activation>,
    published: std::cell::RefCell<Option<activation::Published>>,
    preserve_scene: std::cell::Cell<bool>,
}

impl ReplayContext<'_> {
    fn verify_inputs(&self) -> ReplayStep<()> {
        self.activation.verify()?;
        if let Some(published) = self.published.borrow().as_ref() {
            published.verify()?;
        }
        Ok(())
    }
}

pub(crate) fn run(options: &ResourceGateReplayOptions, frontend_dir: &Path) -> Result<()> {
    let request = ReplayRequest {
        backend: root_dir(),
        frontend: frontend_dir.to_path_buf(),
        manifest: options.manifest.clone(),
        work_dir: options.work_dir.clone(),
        report: options.report.clone(),
        activation_gate: options.activation_gate,
    };
    let environment = base_environment();
    let runtime = ReplayRuntime {
        base_environment: &environment,
        tools: None,
    };
    let run = execute(&request, runtime).map_err(|error| {
        PreservedFailure::new(format!("resource gate replay 配置错误：{error}"), Some(2))
    })?;
    let report_path =
        validated_report_path(&request.backend, &request.report).map_err(|error| {
            PreservedFailure::new(format!("resource gate replay 配置错误：{error}"), Some(2))
        })?;
    write_report(&report_path, &run, request.activation_gate).map_err(|error| {
        PreservedFailure::new(format!("resource gate replay 配置错误：{error}"), Some(2))
    })?;
    finish(&run, request.activation_gate)
}

pub(crate) fn finish(run: &ReplayRun, activation_gate: bool) -> Result<()> {
    let mismatches = replay_mismatches(&run.results);
    if !mismatches.is_empty() {
        return Err(PreservedFailure::new(
            format!("resource gate replay 存在分歧：{}", mismatches.join(", ")),
            Some(1),
        )
        .into());
    }
    if activation_gate
        && let Some(error) = activation_cache_error(&run.evidence)
            .map_err(|error| PreservedFailure::new(error, Some(2)))?
    {
        return Err(PreservedFailure::new(error, Some(1)).into());
    }
    let successful = successful_targeted_runs(&run.results);
    let p95 = percentile_nearest_rank(&successful, 95);
    println!(
        "resource gate replay 零分歧：{} 个案例，targeted P95={}ms",
        run.results.len(),
        p95.unwrap_or(0)
    );
    if activation_gate
        && (successful.len() < minimum_successful_cases()
            || p95.is_none_or(|value| value > TARGETED_P95_LIMIT_MS))
    {
        return Err(PreservedFailure::new(
            format!(
                "resource gate replay targeted P95 超出激活预算，拒绝激活：{}ms > {TARGETED_P95_LIMIT_MS}ms",
                p95.unwrap_or(0)
            ),
            Some(1),
        )
        .into());
    }
    Ok(())
}

fn execute(request: &ReplayRequest, runtime: ReplayRuntime<'_>) -> ReplayStep<ReplayRun> {
    let manifest = load_manifest(&request.manifest)?;
    if request.activation_gate {
        validate_activation_commands(&manifest)?;
    }
    let activation = std::sync::Arc::new(activation::Activation::load(
        &request.backend,
        request.activation_gate,
    )?);
    let manifest_fingerprint = sha256_file(&request.manifest)?;
    let backend = repository_identity(&request.backend, "后端", "Cargo.toml")?;
    let frontend = repository_identity(&request.frontend, "前端", "package.json")?;
    validate_repository_pair(&backend, &frontend)?;
    if !frontend.root.join("pnpm-lock.yaml").is_file() {
        return Err("前端 replay 仓库缺少 pnpm-lock.yaml".to_owned());
    }
    ensure_real_directory(&backend.root.join(".local-tests"))?;
    let allowed = report_allowed_root(&backend.root);
    ensure_real_directory(&allowed)?;
    let allowed =
        canonical_path(&allowed).map_err(|error| format!("无法解析 replay 证据根目录：{error}"))?;
    let work_root = resolve_requested_path(&backend.root, &request.work_dir)?;
    validate_inside(&work_root, &allowed, "replay work-dir")?;
    ensure_real_directory(&work_root)?;
    let report = validated_report_path(&backend.root, &request.report)?;
    validate_inside(&report, &allowed, "replay report")?;
    let session_name = unique_session_name();
    let cache = work_root.join("cache").join(&session_name);
    validate_inside(&cache, &allowed, "replay sccache 目录")?;
    let resolved_tools = match runtime.tools {
        Some(tools) => tools.clone(),
        None => resolve_tool_executables()?,
    };
    let environment = controlled_environment(
        runtime.base_environment.clone(),
        &resolved_tools.sccache,
        &cache,
    )?;
    activation.verify()?;
    let tools = collect_tools(&backend.root, &frontend.root, resolved_tools, &environment)?;
    activation.verify()?;
    run_replay(
        &backend,
        &frontend,
        &manifest,
        &work_root,
        &allowed,
        manifest_fingerprint,
        request.activation_gate,
        environment,
        tools,
        session_name,
        activation,
    )
}

#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_configured(
    backend: PathBuf,
    frontend: PathBuf,
    manifest: PathBuf,
    work_dir: PathBuf,
    report: PathBuf,
    activation_gate: bool,
    base_environment: &ProcessEnvironment,
    tools: &ToolExecutables,
) -> ReplayStep<ReplayRun> {
    execute(
        &ReplayRequest {
            backend,
            frontend,
            manifest,
            work_dir,
            report,
            activation_gate,
        },
        ReplayRuntime {
            base_environment,
            tools: Some(tools),
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn run_replay(
    backend: &RepositoryIdentity,
    frontend: &RepositoryIdentity,
    manifest: &ReplayManifest,
    work_root: &Path,
    allowed: &Path,
    manifest_fingerprint: String,
    activation_gate: bool,
    environment: ProcessEnvironment,
    tools: ReplayTools,
    session_name: String,
    activation: std::sync::Arc<activation::Activation>,
) -> ReplayStep<ReplayRun> {
    activation.verify()?;
    let workspace = prepare_workspace(work_root, &session_name)?;
    let runner_manifest = backend.root.join("Cargo.toml");
    let context = ReplayContext {
        backend,
        frontend,
        manifest,
        workspace: &workspace,
        environment: &environment,
        tools: &tools,
        runner_manifest: &runner_manifest,
        activation: &activation,
        published: std::cell::RefCell::new(None),
        preserve_scene: std::cell::Cell::new(false),
    };
    let mut sccache_started = false;
    let result = (|| {
        context.verify_inputs()?;
        ensure_sccache_server(&tools.executables.sccache, &environment)?;
        sccache_started = true;
        run_replay_work(&context, manifest_fingerprint, activation_gate)
    })();
    let verification = context.verify_inputs();
    if context.preserve_scene.get() || verification.is_err() {
        let stop_error = sccache_started
            .then(|| stop_sccache(&tools.executables.sccache, &environment))
            .flatten();
        return Err(format!(
            "{}；{}；replay现场保留{}",
            result
                .as_ref()
                .err()
                .map(String::as_str)
                .unwrap_or("来源漂移"),
            verification
                .err()
                .unwrap_or_else(|| "配置发布未完成".into()),
            stop_error
                .map(|error| format!("；sccache停止失败：{error}"))
                .unwrap_or_default()
        ));
    }
    let cleanup_errors = cleanup_replay(&context, allowed, sccache_started);
    fail_on_cleanup(
        result.as_ref().err().map(String::as_str),
        "replay 清理",
        &cleanup_errors,
    )?;
    result
}

fn run_replay_work(
    context: &ReplayContext<'_>,
    manifest_fingerprint: String,
    activation_gate: bool,
) -> ReplayStep<ReplayRun> {
    let primes = prime_cases(context.manifest, context.backend, context.frontend)?;
    let first = primes
        .first()
        .ok_or_else(|| "replay 缺少可用于预热的成功定向案例".to_owned())?;
    create_shared_worktrees(
        context.backend,
        context.frontend,
        &context.workspace.shared_worktree,
        &first.case,
    )?;
    context.preserve_scene.set(true);
    *context.published.borrow_mut() = Some(
        context
            .activation
            .publish(&context.workspace.shared_worktree.join("b"))?,
    );
    context.preserve_scene.set(false);
    let prime_results = run_prime_cases(context, &primes)?;
    reset_sccache_stats(&context.tools.executables.sccache, context.environment)?;
    let before = sccache_stats(&context.tools.executables.sccache, context.environment)?;
    let results = run_replay_cases(context)?;
    let after = sccache_stats(&context.tools.executables.sccache, context.environment)?;
    let evidence = build_evidence(
        context,
        manifest_fingerprint,
        activation_gate,
        prime_results,
        before,
        after,
    )?;
    Ok(ReplayRun {
        results,
        evidence,
        activation: Some(context.activation.clone()),
    })
}

fn run_prime_cases(
    context: &ReplayContext<'_>,
    primes: &[runner::PrimeSelection],
) -> ReplayStep<Vec<ReplayPrime>> {
    let mut results = Vec::new();
    let verify = || context.verify_inputs();
    for (index, prime) in primes.iter().enumerate() {
        context.verify_inputs()?;
        verify_case_ranges(context.backend, context.frontend, &prime.case)?;
        let result = execute_in_worktree(CaseExecution {
            worktree: &context.workspace.shared_worktree,
            session: &context.workspace.session,
            shared_target: &context.workspace.shared_target,
            case: &prime.case,
            command: &context.manifest.targeted_command,
            base_environment: context.environment,
            tools: &context.tools.executables,
            runner_manifest: context.runner_manifest,
            targeted: true,
            order: index as i64 - primes.len() as i64 + 1,
            verify_inputs: &verify,
        });
        context.verify_inputs()?;
        let result = result?;
        require_arm(&prime.case, &result, true, true)?;
        results.push(prime_result(prime, result));
    }
    Ok(results)
}

fn run_replay_cases(context: &ReplayContext<'_>) -> ReplayStep<Vec<ReplayResult>> {
    let mut results = Vec::new();
    let verify = || context.verify_inputs();
    let mut order = 0_i64;
    for (index, case) in context.manifest.cases.iter().enumerate() {
        verify_case_ranges(context.backend, context.frontend, case)?;
        let mut arms = std::collections::BTreeMap::new();
        for targeted in [arm_order(index + 1).0, arm_order(index + 1).1] {
            context.verify_inputs()?;
            order += 1;
            let command = if targeted {
                &context.manifest.targeted_command
            } else {
                &context.manifest.full_command
            };
            let result = execute_in_worktree(CaseExecution {
                worktree: &context.workspace.shared_worktree,
                session: &context.workspace.session,
                shared_target: &context.workspace.shared_target,
                case,
                command,
                base_environment: context.environment,
                tools: &context.tools.executables,
                runner_manifest: context.runner_manifest,
                targeted,
                order,
                verify_inputs: &verify,
            });
            context.verify_inputs()?;
            let result = result?;
            require_arm(case, &result, targeted, false)?;
            arms.insert(targeted, result);
        }
        let targeted = arms
            .remove(&true)
            .ok_or_else(|| "replay 缺少 targeted arm".to_owned())?;
        let full = arms
            .remove(&false)
            .ok_or_else(|| "replay 缺少 full arm".to_owned())?;
        results.push(ReplayResult::from_arms(case, targeted, full));
    }
    Ok(results)
}

fn build_evidence(
    context: &ReplayContext<'_>,
    manifest_fingerprint: String,
    activation_gate: bool,
    primes: Vec<ReplayPrime>,
    before: serde_json::Value,
    after: serde_json::Value,
) -> ReplayStep<ReplayEvidence> {
    context.verify_inputs()?;
    let mut environment = recorded_environment(context.environment, activation_gate)?;
    let integration_files = context.activation.record(&mut environment)?;
    let repositories = std::collections::BTreeMap::from([
        ("backend".to_owned(), repository_record(context.backend)),
        ("frontend".to_owned(), repository_record(context.frontend)),
    ]);
    Ok(ReplayEvidence {
        integration_files,
        manifest_fingerprint,
        runner_fingerprint: sha256_file(
            &std::env::current_exe()
                .map_err(|error| format!("无法定位 replay runner 可执行文件：{error}"))?,
        )?,
        runner_commit: git_output(&context.backend.root, &["rev-parse", "HEAD"])?,
        environment_fingerprint: environment_fingerprint(&environment)?,
        environment,
        tools_fingerprint: tools_fingerprint(context.tools)?,
        tools: context.tools.versions.clone(),
        repositories,
        primes,
        sccache_before: before,
        sccache_after: after,
    })
}

fn unique_session_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    let digest = Sha256::digest(format!("{}-{nanos}-{sequence}", process::id()));
    let mut output = String::from("r-");
    for byte in &digest[..6] {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

pub(crate) fn validate_activation_commands(manifest: &ReplayManifest) -> ReplayStep<()> {
    if manifest.targeted_command != manifest.full_command {
        return Err("activation gate 的 targeted/full 必须运行同一资源门禁入口".to_owned());
    }
    let command = &manifest.targeted_command;
    if command.len() < 5
        || !command[..5].iter().map(String::as_str).eq([
            "cargo",
            "xtask",
            "check",
            "ci",
            "resource-gate",
        ])
    {
        return Err(
            "activation gate 只接受 cargo xtask check ci resource-gate 作为比较入口".to_owned(),
        );
    }
    let remaining = &command[5..];
    if !remaining.is_empty()
        && (remaining.len() != 2 || remaining.first().map(String::as_str) != Some("--frontend-dir"))
    {
        return Err("activation gate 资源门禁入口只允许可选的 --frontend-dir PATH".to_owned());
    }
    Ok(())
}

pub(super) fn sha256_text(value: &str) -> String {
    format!("sha256:{}", hex(&Sha256::digest(value.as_bytes())))
}

pub(super) fn sha256_file(path: &Path) -> ReplayStep<String> {
    let body = fs::read(path)
        .map_err(|error| format!("无法读取 replay 证据文件 {}：{error}", path.display()))?;
    Ok(format!("sha256:{}", hex(&Sha256::digest(body))))
}

pub(super) fn sha256_json(value: &impl serde::Serialize) -> ReplayStep<String> {
    let body =
        serde_json::to_vec(value).map_err(|error| format!("无法序列化 replay 摘要：{error}"))?;
    Ok(format!("sha256:{}", hex(&Sha256::digest(body))))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}
