use std::{
    collections::BTreeMap,
    fs,
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::Instant,
};

use chrono::Utc;

use crate::{
    Result,
    dev::{ReadyKind, SaveMeasurementContract},
};

use super::{
    incremental::with_source_edit,
    metadata::{
        MetadataContext, PathNormalizer, SourceFingerprints, collect as collect_metadata,
        corepack_executable, inherited_environment,
    },
    model::{
        CacheState, DevexRunOptions, DevexSuite, PairedArm, PairingMetadata, StepDefinition,
        SuiteDefinition, WorkingDirectory,
    },
    preflight,
    report::{SampleKind, SampleRecord, SampleStatus, append_sample, summarize, write_metadata},
    support::{
        cleanup_successful_sample_target, display_step, metric, sample_target, success_status,
    },
};

#[path = "run/save.rs"]
mod save;

pub(super) fn execute(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
) -> Result<PathBuf> {
    let definition = options
        .suite
        .definition(&options.variant)
        .map_err(|error| format!("DevEx 变体无效：{error}"))?;
    preflight::check(backend_root, frontend_root, options, definition)?;
    let session = prepare_run(backend_root, frontend_root, options, definition)?;
    let execution = (|| {
        execute_warmup(backend_root, frontend_root, options, &session)?;
        capture_sccache_stats(
            backend_root,
            options.suite,
            &session.environment,
            &session.run_dir,
            "sccache-before.json",
        )?;
        execute_measurements(backend_root, frontend_root, options, &session)?;
        capture_sccache_stats(
            backend_root,
            options.suite,
            &session.environment,
            &session.run_dir,
            "sccache-after.json",
        )
    })();
    let cleanup = stop_sccache_server(backend_root, options.suite, &session.environment);
    if execution.is_err() {
        // 失败样本可能只写入了 sccache before 快照；摘要不完整不得覆盖原始门禁错误。
        let _ = summarize(&session.run_dir);
    }
    execution?;
    cleanup?;
    let summary = summarize(&session.run_dir)?;
    println!(
        "DevEx 完成：{}（P50 {}，P95 {}）",
        session
            .normalizer
            .normalize(&session.run_dir.to_string_lossy()),
        metric(summary.duration_ms.as_ref().map(|value| value.p50)),
        metric(summary.duration_ms.as_ref().map(|value| value.p95)),
    );
    Ok(session.run_dir)
}

pub(super) fn capture_sccache_stats(
    backend_root: &Path,
    suite: DevexSuite,
    environment: &BTreeMap<String, String>,
    run_dir: &Path,
    filename: &str,
) -> Result<()> {
    if !suite.uses_sccache() {
        return Ok(());
    }
    let output = Command::new("sccache")
        .args(["--show-stats", "--stats-format", "json"])
        .envs(environment)
        .current_dir(backend_root)
        .output()?;
    if !output.status.success() {
        return Err(format!("sccache 统计采集失败：{}", output.status).into());
    }
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .map_err(|error| format!("sccache 统计不是合法 JSON：{error}"))?;
    fs::write(run_dir.join(filename), output.stdout)?;
    Ok(())
}

pub(super) fn stop_sccache_server(
    backend_root: &Path,
    suite: DevexSuite,
    environment: &BTreeMap<String, String>,
) -> Result<()> {
    if !suite.uses_sccache() {
        return Ok(());
    }
    let status = Command::new("sccache")
        .arg("--stop-server")
        .envs(environment)
        .current_dir(backend_root)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("DevEx 专用 sccache server 停止失败：{status}").into())
    }
}

pub(super) struct RunSession {
    pub(super) run_id: String,
    pub(super) run_dir: PathBuf,
    pub(super) definition: SuiteDefinition,
    pub(super) environment: BTreeMap<String, String>,
    pub(super) source_fingerprints: SourceFingerprints,
    pub(super) pairing: Option<PairingMetadata>,
    pub(super) normalizer: PathNormalizer,
}

fn prepare_run(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    definition: SuiteDefinition,
) -> Result<RunSession> {
    let devex_root = backend_root.join(".local-tests/devex");
    let (run_id, run_dir) = create_run_directory(&devex_root, options)?;
    prepare_run_at(
        backend_root,
        frontend_root,
        &devex_root,
        options,
        definition,
        run_id,
        run_dir,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_run_at(
    backend_root: &Path,
    frontend_root: &Path,
    devex_root: &Path,
    options: &DevexRunOptions,
    definition: SuiteDefinition,
    run_id: String,
    run_dir: PathBuf,
    pairing: Option<PairingMetadata>,
) -> Result<RunSession> {
    let mut environment = effective_environment(definition);
    if definition.steps.iter().any(|step| step.program == "cargo") {
        environment.insert("CARGO_TARGET_DIR".to_owned(), "{target}".to_owned());
    }
    if options.suite.uses_sccache() {
        let cache_dir = run_dir.join("cache/sccache");
        fs::create_dir_all(&cache_dir)?;
        environment.insert(
            "SCCACHE_DIR".to_owned(),
            cache_dir.to_string_lossy().into_owned(),
        );
        environment.insert(
            "SCCACHE_SERVER_PORT".to_owned(),
            available_sccache_port()?.to_string(),
        );
        if options.cache_state == CacheState::Cold {
            environment.insert("SCCACHE_RECACHE".to_owned(), "1".to_owned());
        } else {
            environment.remove("SCCACHE_RECACHE");
        }
    }
    let metadata = collect_metadata(MetadataContext {
        backend_root,
        frontend_root,
        devex_root,
        run_id: &run_id,
        options,
        definition,
        effective_environment: &environment,
        pairing: pairing.clone(),
    })?;
    let source_fingerprints = metadata.source_fingerprints();
    write_metadata(&run_dir, &metadata)?;
    fs::write(run_dir.join("samples.jsonl"), [])?;
    let normalizer = PathNormalizer::new(backend_root, frontend_root, devex_root);
    Ok(RunSession {
        run_id,
        run_dir,
        definition,
        environment,
        source_fingerprints,
        pairing,
        normalizer,
    })
}

fn available_sccache_port() -> Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

pub(super) fn execute_warmup(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    session: &RunSession,
) -> Result<()> {
    execute_warmup_with_contract(
        backend_root,
        frontend_root,
        options,
        session,
        SaveMeasurementContract::Current,
    )
}

pub(super) fn execute_warmup_with_contract(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    session: &RunSession,
    save_contract: SaveMeasurementContract,
) -> Result<()> {
    if options.cache_state == CacheState::Warm {
        let target = session.run_dir.join("cache/warm");
        let outcome = execute_sample_with_contract(
            backend_root,
            frontend_root,
            &target,
            session.definition,
            &session.environment,
            options.suite,
            &options.variant,
            "warmup",
            save_contract,
        )?;
        append_sample(
            &session.run_dir,
            &sample_record(
                &session.run_id,
                0,
                SampleKind::Warmup,
                options.cache_state,
                &target,
                outcome,
                &session.normalizer,
                session.pairing.as_ref().map(|pairing| SampleAudit {
                    arm: pairing.arm,
                    pair: None,
                    order: None,
                    source_fingerprints: session.source_fingerprints.clone(),
                }),
            ),
        )?;
        if !outcome.status.success() {
            summarize(&session.run_dir)?;
            return Err(format!("suite `{}` 预热失败", options.suite.as_str()).into());
        }
    }
    Ok(())
}

fn execute_measurements(
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    session: &RunSession,
) -> Result<()> {
    for sequence in 1..=options.runs {
        let target = sample_target(
            &session.run_dir,
            options.suite,
            options.cache_state,
            sequence,
        );
        let label = format!("sample-{sequence:03}");
        println!(
            "DevEx {} {}/{}（{}）",
            options.suite.as_str(),
            sequence,
            options.runs,
            options.cache_state.as_str()
        );
        let outcome = execute_sample(
            backend_root,
            frontend_root,
            &target,
            session.definition,
            &session.environment,
            options.suite,
            &options.variant,
            &label,
        )?;
        append_sample(
            &session.run_dir,
            &sample_record(
                &session.run_id,
                sequence,
                SampleKind::Measurement,
                options.cache_state,
                &target,
                outcome,
                &session.normalizer,
                None,
            ),
        )?;
        if outcome.status.success() {
            cleanup_successful_sample_target(
                &session.run_dir,
                &target,
                options.suite,
                options.cache_state,
            )?;
        } else {
            summarize(&session.run_dir)?;
            return Err(format!(
                "suite `{}` 第 {sequence} 个样本失败，记录保留在 {}",
                options.suite.as_str(),
                session
                    .normalizer
                    .normalize(&session.run_dir.to_string_lossy())
            )
            .into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SampleOutcome {
    started_at: chrono::DateTime<Utc>,
    duration_ms: f64,
    cargo_invocations: Option<usize>,
    ready_kind: Option<ReadyKind>,
    pub(super) status: ExitStatus,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_sample(
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    suite: DevexSuite,
    variant: &str,
    label: &str,
) -> Result<SampleOutcome> {
    execute_sample_with_contract(
        backend_root,
        frontend_root,
        target,
        definition,
        environment,
        suite,
        variant,
        label,
        SaveMeasurementContract::Current,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_sample_with_contract(
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    suite: DevexSuite,
    variant: &str,
    label: &str,
    save_contract: SaveMeasurementContract,
) -> Result<SampleOutcome> {
    fs::create_dir_all(target)?;
    let started_at = Utc::now();
    let started = Instant::now();
    let source = suite
        .incremental_source(variant)
        .map_err(|error| format!("DevEx 增量源码选择失败：{error}"))?
        .map(|relative| backend_root.join(relative));
    let execute = || {
        execute_steps(
            backend_root,
            frontend_root,
            target,
            definition,
            environment,
            label,
        )
    };
    let status = if let Some(source) = source.as_deref() {
        with_source_edit(source, label, execute)?
    } else {
        execute()?
    };
    let outcome = SampleOutcome {
        started_at,
        duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
        cargo_invocations: None,
        ready_kind: None,
        status,
    };
    if suite == DevexSuite::CargoDevSave && outcome.status.success() {
        save::measurement_outcome(target, variant, outcome.status, save_contract)
    } else {
        Ok(outcome)
    }
}

fn execute_steps(
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
    label: &str,
) -> Result<ExitStatus> {
    let mut last_status = success_status()?;
    for (index, step) in definition.steps.iter().enumerate() {
        println!("  → {label} step {:02}: {}", index + 1, display_step(step));
        last_status = step_command(
            step,
            backend_root,
            frontend_root,
            target,
            definition,
            environment,
        )
        .status()?;
        if !last_status.success() {
            break;
        }
    }
    Ok(last_status)
}

fn step_command(
    step: &StepDefinition,
    backend_root: &Path,
    frontend_root: &Path,
    target: &Path,
    definition: SuiteDefinition,
    environment: &BTreeMap<String, String>,
) -> Command {
    let program = if step.program == "corepack" {
        corepack_executable()
    } else {
        step.program
    };
    let mut command = Command::new(program);
    let target = target.to_string_lossy();
    let frontend = frontend_root.to_string_lossy();
    let args = step
        .args
        .iter()
        .map(|arg| {
            arg.replace("{target}", &target)
                .replace("{frontend}", &frontend)
        })
        .collect::<Vec<_>>();
    command
        .args(args)
        .current_dir(match step.working_directory {
            WorkingDirectory::Backend => backend_root,
            WorkingDirectory::Frontend => frontend_root,
        })
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for key in definition.remove_environment {
        command.env_remove(key);
    }
    for (key, value) in environment {
        command.env(
            key,
            value
                .replace("{target}", &target)
                .replace("{frontend}", &frontend),
        );
    }
    if step.program == "cargo" {
        command.env("CARGO_TARGET_DIR", target.as_ref());
    }
    command
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sample_record(
    run_id: &str,
    sequence: usize,
    kind: SampleKind,
    cache_state: CacheState,
    target: &Path,
    outcome: SampleOutcome,
    normalizer: &PathNormalizer,
    audit: Option<SampleAudit>,
) -> SampleRecord {
    let (arm, pair, order, source_fingerprints) = audit.map_or((None, None, None, None), |audit| {
        (
            Some(audit.arm),
            audit.pair,
            audit.order,
            Some(audit.source_fingerprints),
        )
    });
    SampleRecord {
        schema_version: 1,
        run_id: run_id.to_owned(),
        sequence,
        kind,
        cache_state,
        started_at: outcome.started_at.to_rfc3339(),
        duration_ms: outcome.duration_ms,
        cargo_invocations: outcome.cargo_invocations,
        ready_kind: outcome.ready_kind,
        status: if outcome.status.success() {
            SampleStatus::Passed
        } else {
            SampleStatus::Failed
        },
        exit_code: outcome.status.code(),
        target_directory: normalizer.normalize(&target.to_string_lossy()),
        arm,
        pair,
        order,
        source_fingerprints,
    }
}

pub(super) struct SampleAudit {
    pub(super) arm: PairedArm,
    pub(super) pair: Option<usize>,
    pub(super) order: Option<usize>,
    pub(super) source_fingerprints: SourceFingerprints,
}

fn effective_environment(definition: SuiteDefinition) -> BTreeMap<String, String> {
    let mut environment = inherited_environment();
    for key in definition.remove_environment {
        environment.remove(*key);
    }
    environment.extend(
        definition
            .environment
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    environment
}

fn create_run_directory(devex_root: &Path, options: &DevexRunOptions) -> Result<(String, PathBuf)> {
    let now = Utc::now();
    let date = now.format("%Y-%m-%d").to_string();
    let run_id = format!(
        "{}-{}-{}-{}-{}",
        now.format("%Y%m%dT%H%M%S%9fZ"),
        options.suite.as_str(),
        options.variant,
        options.cache_state.as_str(),
        std::process::id()
    );
    let directory = devex_root.join(date).join(&run_id);
    fs::create_dir_all(directory.parent().expect("run 目录必须具有日期父目录"))?;
    fs::create_dir(&directory)?;
    Ok((run_id, directory))
}
