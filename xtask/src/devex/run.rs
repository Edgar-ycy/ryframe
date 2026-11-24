use std::{
    collections::BTreeMap,
    fs,
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::Command,
};

use time::OffsetDateTime;

use crate::{
    Result,
    dev::SaveMeasurementContract,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};

use super::{
    metadata::{MetadataContext, PathNormalizer, SourceFingerprints, collect as collect_metadata},
    model::{CacheState, DevexRunOptions, DevexSuite, PairingMetadata, SuiteDefinition},
    preflight,
    report::{SampleKind, append_sample, summarize, write_metadata},
    support::{canonical_worktree, cleanup_successful_sample_target, metric, sample_target},
};

#[path = "run/environment.rs"]
mod environment;
#[path = "run/process.rs"]
mod process;
use process::StepInvocation;
#[path = "run/resource_gate.rs"]
mod resource_gate;
#[path = "run/sample.rs"]
mod sample;
pub(super) use resource_gate::{lock_cache, verify_cache};
use sample::SampleOutcome;
pub(super) use sample::{SampleAudit, execute_sample, sample_record};
#[path = "run/save.rs"]
mod save;

use environment::effective_environment;
pub(crate) use resource_gate::read_resource_gate_decision;

pub(super) fn execute(
    coordinator_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    source_backend: Option<&Path>,
) -> Result<PathBuf> {
    let source_backend = source_backend.map(validate_source_backend).transpose()?;
    let backend_root = source_backend.as_deref().unwrap_or(coordinator_root);
    let definition = options
        .suite
        .definition(&options.variant)
        .map_err(|error| format!("DevEx 变体无效：{error}"))?;
    preflight::check(
        backend_root,
        frontend_root,
        frontend_root,
        options,
        definition,
    )?;
    let session = prepare_run(
        coordinator_root,
        backend_root,
        frontend_root,
        options,
        definition,
    )?;
    let execution = (|| {
        execute_warmup(
            backend_root,
            frontend_root,
            frontend_root,
            options,
            &session,
        )?;
        capture_sccache_stats(
            backend_root,
            options.suite,
            &session.environment,
            &session.run_dir,
            "sccache-before.json",
        )?;
        execute_measurements(
            backend_root,
            frontend_root,
            frontend_root,
            options,
            &session,
        )?;
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

fn validate_source_backend(path: &Path) -> Result<PathBuf> {
    let source = validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| format!("--source-backend 无效：{error}"))?;
    let root = canonical_worktree(&source, "--source-backend")?;
    for manifest in ["Cargo.toml", "xtask/Cargo.toml"] {
        validate_absolute_path(&root.join(manifest), LocalTestPathKind::ExistingFile)
            .map_err(|error| format!("--source-backend 必须指向后端工作区：{manifest}：{error}"))?;
    }
    if super::metadata::source_state(&root)?.dirty != Some(false) {
        return Err("--source-backend 必须指向干净的后端工作树".into());
    }
    Ok(root)
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
    pub(super) runtime_input: Option<super::runtime_native::Input>,
    pub(super) source_fingerprints: SourceFingerprints,
    pub(super) compile_surface_fingerprint: String,
    pub(super) common_toolchain: Option<super::common_toolchain::CommonToolchain>,
    pub(super) pairing: Option<PairingMetadata>,
    pub(super) normalizer: PathNormalizer,
}

fn prepare_run(
    coordinator_root: &Path,
    backend_root: &Path,
    frontend_root: &Path,
    options: &DevexRunOptions,
    definition: SuiteDefinition,
) -> Result<RunSession> {
    let devex_root = coordinator_root.join(".local-tests/devex");
    let (run_id, run_dir) = create_run_directory(&devex_root, options)?;
    prepare_run_at(
        backend_root,
        frontend_root,
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
    runner_frontend_root: &Path,
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
    let runtime_input = options
        .suite
        .is_runtime()
        .then(|| {
            super::runtime::prepare(
                backend_root,
                frontend_root,
                runner_frontend_root,
                options,
                &run_dir.join("runtime.json"),
            )
        })
        .transpose()?;
    let mut metadata = collect_metadata(MetadataContext {
        backend_root,
        frontend_root,
        runner_frontend_root,
        devex_root,
        run_id: &run_id,
        options,
        definition,
        effective_environment: &environment,
        pairing: pairing.clone(),
    })?;
    let source_fingerprints = metadata.source_fingerprints();
    if let Some(input) = &runtime_input {
        if input.source_fingerprints != source_fingerprints {
            return Err("运行时源码在准备收据期间发生变化".into());
        }
        metadata.runtime = Some(super::runtime::RuntimeContract {
            input_sha256: input.input_sha256.clone(),
            concurrency: input.concurrency,
        });
    }
    write_metadata(&run_dir, &metadata)?;
    fs::write(run_dir.join("samples.jsonl"), [])?;
    let normalizer = PathNormalizer::new(backend_root, frontend_root, devex_root);
    Ok(RunSession {
        run_id,
        run_dir,
        definition,
        environment,
        runtime_input,
        source_fingerprints,
        compile_surface_fingerprint: metadata.compile_surface_fingerprint,
        common_toolchain: metadata.toolchain.common,
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
    runner_frontend_root: &Path,
    options: &DevexRunOptions,
    session: &RunSession,
) -> Result<()> {
    execute_warmup_with_contract(
        backend_root,
        frontend_root,
        runner_frontend_root,
        options,
        session,
        SaveMeasurementContract::Current,
    )
}

pub(super) fn execute_warmup_with_contract(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
    options: &DevexRunOptions,
    session: &RunSession,
    save_contract: SaveMeasurementContract,
) -> Result<()> {
    if options.cache_state == CacheState::Warm {
        let target = super::support::compiler_target_root(&session.run_dir).join("warm");
        let outcome = execute_sample(
            backend_root,
            frontend_root,
            runner_frontend_root,
            &target,
            session,
            options,
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
                &outcome,
                &session.normalizer,
                session.pairing.as_ref().map(|pairing| SampleAudit {
                    arm: pairing.arm,
                    pair: None,
                    order: None,
                    source_fingerprints: session.source_fingerprints.clone(),
                }),
            ),
        )?;
        if !outcome.passed() {
            summarize(&session.run_dir)?;
            return Err(format!("suite `{}` 预热失败", options.suite.as_str()).into());
        }
    }
    Ok(())
}

fn execute_measurements(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: &Path,
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
            runner_frontend_root,
            &target,
            session,
            options,
            &label,
            SaveMeasurementContract::Current,
        )?;
        append_sample(
            &session.run_dir,
            &sample_record(
                &session.run_id,
                sequence,
                SampleKind::Measurement,
                options.cache_state,
                &target,
                &outcome,
                &session.normalizer,
                None,
            ),
        )?;
        if outcome.passed() {
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

fn create_run_directory(devex_root: &Path, options: &DevexRunOptions) -> Result<(String, PathBuf)> {
    let (date, stamp) = crate::utc_timestamp::run_directory_parts(OffsetDateTime::now_utc());
    let run_id = format!(
        "{stamp}-{}-{}-{}-{}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_runtime_reads_product_configuration_and_keeps_reports_with_coordinator() {
        let scratch = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&scratch).unwrap();
        let temp = tempfile::tempdir_in(scratch).unwrap();
        let coordinator = temp.path().join("coordinator");
        let product = temp.path().join("product");
        fs::create_dir(&product).unwrap();
        let options = DevexRunOptions {
            suite: DevexSuite::RuntimeApi,
            variant: "10".to_owned(),
            cache_state: CacheState::Warm,
            runs: 6,
        };
        let definition = options.suite.definition(&options.variant).unwrap();
        let Err(error) = prepare_run(&coordinator, &product, temp.path(), &options, definition)
        else {
            panic!("产品工作树缺少运行配置时必须拒绝测量");
        };
        let configuration = product.join(".local-tests/devex/runtime.json");
        assert!(
            error
                .to_string()
                .contains(&configuration.display().to_string())
        );
        let reports = coordinator.join(".local-tests/devex");
        let date = fs::read_dir(reports)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read_dir(date).unwrap().count(), 1);
        assert!(!product.join(".local-tests").exists());
    }
}
