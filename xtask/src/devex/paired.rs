use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::Utc;

use crate::{Result, dev::SaveMeasurementContract};

use super::{
    execution::{
        RunSession, SampleAudit, capture_sccache_stats, execute_sample,
        execute_warmup_with_contract, lock_cache, prepare_run_at, sample_record,
        stop_sccache_server,
    },
    model::{
        BaselineContract, BaselineProvenance, DevexPairedOptions, DevexRunOptions, PairedArm,
        PairingMetadata, SaveConfigBinding, SuiteDefinition,
    },
    preflight,
    report::{SampleKind, append_sample, ensure_compile_surface, summarize},
    support::{canonical_worktree, cleanup_successful_sample_target, sample_target},
};

#[path = "paired/provenance.rs"]
mod provenance;
#[path = "paired/sources.rs"]
mod sources;

use sources::{source_fingerprints, verify_save_inputs};

pub(super) fn execute(
    coordinator_backend: &Path,
    coordinator_frontend: &Path,
    options: &DevexPairedOptions,
) -> Result<(PathBuf, PathBuf)> {
    let definition = options
        .run
        .suite
        .definition(&options.run.variant)
        .map_err(|error| format!("DevEx 变体无效：{error}"))?;
    let roots = paired_roots(
        coordinator_backend,
        coordinator_frontend,
        options,
        definition,
    )?;
    let (baseline_config, candidate_config) = roots.bind_save_configs(options)?;
    let baseline_definition = arm_definition(&roots, options, PairedArm::Baseline)?;
    let candidate_definition = arm_definition(&roots, options, PairedArm::Candidate)?;
    let baseline_provenance = provenance::collect(&roots, options)?;

    let devex_root = coordinator_backend.join(".local-tests/devex");
    let directories = create_run_directories(&devex_root, &options.run)?;
    let baseline = prepare_arm(
        &roots,
        &devex_root,
        options,
        baseline_definition,
        &directories,
        PairedArm::Baseline,
        baseline_provenance.clone(),
        baseline_config,
    )?;
    let candidate = prepare_arm(
        &roots,
        &devex_root,
        options,
        candidate_definition,
        &directories,
        PairedArm::Candidate,
        baseline_provenance,
        candidate_config,
    )?;
    verify_ready(&roots, options, &baseline, &candidate)?;
    if options.check_comparability {
        println!(
            "DevEx paired 预检通过：base={}，candidate={}",
            baseline.run_dir.display(),
            candidate.run_dir.display()
        );
        return Ok((baseline.run_dir, candidate.run_dir));
    }
    run_pair(&roots, options, &baseline, &candidate)?;
    Ok((baseline.run_dir, candidate.run_dir))
}

fn arm_definition(
    roots: &PairedRoots,
    options: &DevexPairedOptions,
    arm: PairedArm,
) -> Result<SuiteDefinition> {
    let (backend, frontend) = roots.arm(arm);
    let definition = options
        .run
        .suite
        .paired_definition(&options.run.variant, options.baseline_contract, arm)
        .map_err(|error| format!("DevEx {} 变体无效：{error}", arm.as_str()))?;
    preflight::check_paired(
        backend,
        frontend,
        &roots.runner_frontend,
        &options.run,
        definition,
        options.baseline_contract,
        arm,
    )?;
    Ok(definition)
}

fn verify_ready(
    roots: &PairedRoots,
    options: &DevexPairedOptions,
    baseline: &RunSession,
    candidate: &RunSession,
) -> Result<()> {
    let prepared = provenance::collect(roots, options)?;
    if prepared.as_ref()
        != baseline
            .pairing
            .as_ref()
            .and_then(|pairing| pairing.baseline_provenance.as_ref())
    {
        return Err("baseline 来源在创建运行记录期间发生变化，拒绝开始测量".into());
    }
    ensure_compile_surface(
        &baseline.compile_surface_fingerprint,
        &candidate.compile_surface_fingerprint,
    )?;
    if baseline.common_toolchain != candidate.common_toolchain {
        return Err("共同 DevEx 工具链文件、实际版本或缓存绑定不同".into());
    }
    for (arm, session) in [
        (PairedArm::Baseline, baseline),
        (PairedArm::Candidate, candidate),
    ] {
        let (backend, frontend) = roots.arm(arm);
        source_fingerprints(
            backend,
            frontend,
            &roots.runner_frontend,
            &options.run,
            session,
        )?;
    }
    Ok(())
}

fn run_pair(
    roots: &PairedRoots,
    options: &DevexPairedOptions,
    baseline: &RunSession,
    candidate: &RunSession,
) -> Result<()> {
    let _caches = options
        .targeted_source
        .as_ref()
        .map(|_| -> Result<_> {
            Ok((
                lock_cache(&roots.baseline_backend)?,
                lock_cache(&roots.candidate_backend)?,
            ))
        })
        .transpose()?;
    let execution = execute_samples(
        roots,
        &options.run,
        options.baseline_contract,
        baseline,
        candidate,
    );
    let baseline_cleanup = stop_sccache_server(
        &roots.baseline_backend,
        options.run.suite,
        &baseline.environment,
    );
    let candidate_cleanup = stop_sccache_server(
        &roots.candidate_backend,
        options.run.suite,
        &candidate.environment,
    );
    if execution.is_err() {
        let _ = summarize(&baseline.run_dir);
        let _ = summarize(&candidate.run_dir);
    }
    execution?;
    baseline_cleanup?;
    candidate_cleanup?;
    if let Some(toolchain) = &baseline.common_toolchain {
        toolchain.verify_digests()?;
    }
    summarize(&baseline.run_dir)?;
    summarize(&candidate.run_dir)?;
    println!(
        "DevEx paired 完成：base={}，candidate={}",
        baseline
            .normalizer
            .normalize(&baseline.run_dir.to_string_lossy()),
        candidate
            .normalizer
            .normalize(&candidate.run_dir.to_string_lossy())
    );
    Ok(())
}

struct PairedRoots {
    baseline_backend: PathBuf,
    candidate_backend: PathBuf,
    baseline_frontend: PathBuf,
    candidate_frontend: PathBuf,
    runner_frontend: PathBuf,
}

impl PairedRoots {
    fn bind_save_configs(
        &self,
        options: &DevexPairedOptions,
    ) -> Result<(Option<SaveConfigBinding>, Option<SaveConfigBinding>)> {
        let required = options.baseline_contract.is_some_and(|contract| {
            contract.requires_save_configuration(options.run.suite, &options.run.variant)
        });
        if required != options.save_config_dirs.is_some() {
            return Err("配对保存配置与场景不匹配，拒绝启动任何测量".into());
        }
        options
            .save_config_dirs
            .as_deref()
            .map_or(Ok((None, None)), |dirs| {
                let (baseline, candidate) = dirs.bind([
                    &self.baseline_backend,
                    &self.baseline_frontend,
                    &self.candidate_backend,
                    &self.candidate_frontend,
                ])?;
                Ok((Some(baseline), Some(candidate)))
            })
    }

    fn arm(&self, arm: PairedArm) -> (&Path, &Path) {
        match arm {
            PairedArm::Baseline => (&self.baseline_backend, &self.baseline_frontend),
            PairedArm::Candidate => (&self.candidate_backend, &self.candidate_frontend),
        }
    }
}

fn paired_roots(
    coordinator_backend: &Path,
    coordinator_frontend: &Path,
    options: &DevexPairedOptions,
    definition: SuiteDefinition,
) -> Result<PairedRoots> {
    let coordinator_backend = canonical_worktree(coordinator_backend, "当前后端")?;
    let baseline_backend = canonical_worktree(&options.baseline_backend, "基线后端")?;
    let candidate_backend = canonical_worktree(&options.candidate_backend, "候选后端")?;
    ensure_distinct_roots(
        "后端",
        &coordinator_backend,
        &baseline_backend,
        &candidate_backend,
    )?;
    let binds_stable_product = options
        .baseline_contract
        .is_some_and(BaselineContract::is_stable_readiness_b0);
    let (baseline_frontend, candidate_frontend, runner_frontend) =
        if definition.requires_frontend || binds_stable_product {
            let coordinator = canonical_worktree(coordinator_frontend, "当前前端")?;
            let baseline = canonical_worktree(
                options
                    .baseline_frontend
                    .as_deref()
                    .ok_or("paired 前端 suite 缺少 --base-frontend")?,
                "基线前端",
            )?;
            let candidate = canonical_worktree(
                options
                    .candidate_frontend
                    .as_deref()
                    .ok_or("paired 前端 suite 缺少 --candidate-frontend")?,
                "候选前端",
            )?;
            ensure_distinct_roots("前端", &coordinator, &baseline, &candidate)?;
            (baseline, candidate, coordinator)
        } else {
            (
                coordinator_frontend.to_path_buf(),
                coordinator_frontend.to_path_buf(),
                coordinator_frontend.to_path_buf(),
            )
        };
    Ok(PairedRoots {
        baseline_backend,
        candidate_backend,
        baseline_frontend,
        candidate_frontend,
        runner_frontend,
    })
}

fn ensure_distinct_roots(
    label: &str,
    coordinator: &Path,
    baseline: &Path,
    candidate: &Path,
) -> Result<()> {
    if baseline == candidate || baseline == coordinator || candidate == coordinator {
        return Err(
            format!("paired {label} 要求当前、基线、候选为三个互不相同的独立 worktree").into(),
        );
    }
    Ok(())
}

struct PairedRunDirectories {
    comparison_id: String,
    baseline_id: String,
    baseline_dir: PathBuf,
    candidate_id: String,
    candidate_dir: PathBuf,
}

#[allow(clippy::too_many_arguments)]
fn prepare_arm(
    roots: &PairedRoots,
    devex_root: &Path,
    options: &DevexPairedOptions,
    definition: SuiteDefinition,
    directories: &PairedRunDirectories,
    arm: PairedArm,
    baseline_provenance: Option<BaselineProvenance>,
    save_config: Option<SaveConfigBinding>,
) -> Result<RunSession> {
    let (backend_root, frontend_root) = roots.arm(arm);
    let (run_id, run_dir) = match arm {
        PairedArm::Baseline => (&directories.baseline_id, &directories.baseline_dir),
        PairedArm::Candidate => (&directories.candidate_id, &directories.candidate_dir),
    };
    prepare_run_at(
        backend_root,
        frontend_root,
        &roots.runner_frontend,
        devex_root,
        &options.run,
        definition,
        run_id.clone(),
        run_dir.clone(),
        Some(PairingMetadata {
            comparison_id: directories.comparison_id.clone(),
            arm,
            baseline_contract: options.baseline_contract,
            baseline_provenance,
            workload_contract: options.baseline_contract.and_then(|contract| {
                contract.workload_contract(options.run.suite, &options.run.variant)
            }),
            save_config,
        }),
    )
}

fn execute_samples(
    roots: &PairedRoots,
    options: &DevexRunOptions,
    baseline_contract: Option<BaselineContract>,
    baseline: &RunSession,
    candidate: &RunSession,
) -> Result<()> {
    for (arm, session) in [
        (PairedArm::Baseline, baseline),
        (PairedArm::Candidate, candidate),
    ] {
        let (backend, frontend) = roots.arm(arm);
        source_fingerprints(backend, frontend, &roots.runner_frontend, options, session)?;
        let execution = execute_warmup_with_contract(
            backend,
            frontend,
            &roots.runner_frontend,
            options,
            session,
            save_contract(baseline_contract, arm),
        );
        if execution.is_err() {
            verify_save_inputs(backend, frontend, options, session)?;
        }
        execution?;
        source_fingerprints(backend, frontend, &roots.runner_frontend, options, session)?;
    }
    capture_sccache_stats(
        &roots.baseline_backend,
        options.suite,
        &baseline.environment,
        &baseline.run_dir,
        "sccache-before.json",
    )?;
    capture_sccache_stats(
        &roots.candidate_backend,
        options.suite,
        &candidate.environment,
        &candidate.run_dir,
        "sccache-before.json",
    )?;
    for pair in 1..=options.runs {
        for (position, arm) in abba_pair_order(pair).into_iter().enumerate() {
            let order = (pair - 1) * 2 + position + 1;
            execute_measurement(
                roots,
                options,
                baseline_contract,
                baseline,
                candidate,
                arm,
                pair,
                order,
            )?;
        }
    }
    capture_sccache_stats(
        &roots.baseline_backend,
        options.suite,
        &baseline.environment,
        &baseline.run_dir,
        "sccache-after.json",
    )?;
    capture_sccache_stats(
        &roots.candidate_backend,
        options.suite,
        &candidate.environment,
        &candidate.run_dir,
        "sccache-after.json",
    )
}

pub(crate) fn abba_pair_order(pair: usize) -> [PairedArm; 2] {
    if pair % 2 == 1 {
        [PairedArm::Baseline, PairedArm::Candidate]
    } else {
        [PairedArm::Candidate, PairedArm::Baseline]
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_measurement(
    roots: &PairedRoots,
    options: &DevexRunOptions,
    baseline_contract: Option<BaselineContract>,
    baseline: &RunSession,
    candidate: &RunSession,
    arm: PairedArm,
    pair: usize,
    order: usize,
) -> Result<()> {
    let (backend, frontend) = roots.arm(arm);
    let session = match arm {
        PairedArm::Baseline => baseline,
        PairedArm::Candidate => candidate,
    };
    let expected_fingerprints =
        source_fingerprints(backend, frontend, &roots.runner_frontend, options, session)?;
    let target = sample_target(&session.run_dir, options.suite, options.cache_state, pair);
    let label = format!("{}-pair-{pair:03}-order-{order:03}", arm.as_str());
    println!(
        "DevEx paired {} pair {pair}/{} order {order}/{}（{}）",
        arm.as_str(),
        options.runs,
        options.runs * 2,
        options.cache_state.as_str()
    );
    let outcome = execute_sample(
        backend,
        frontend,
        &roots.runner_frontend,
        &target,
        session,
        options,
        &label,
        save_contract(baseline_contract, arm),
    );
    if outcome.is_err() {
        verify_save_inputs(backend, frontend, options, session)?;
    }
    let outcome = outcome?;
    let restored =
        source_fingerprints(backend, frontend, &roots.runner_frontend, options, session)?;
    if restored != expected_fingerprints {
        return Err(format!(
            "paired {} 第 {pair} 对样本结束后源码指纹发生变化",
            arm.as_str()
        )
        .into());
    }
    append_sample(
        &session.run_dir,
        &sample_record(
            &session.run_id,
            pair,
            SampleKind::Measurement,
            options.cache_state,
            &target,
            &outcome,
            &session.normalizer,
            Some(SampleAudit {
                arm,
                pair: Some(pair),
                order: Some(order),
                source_fingerprints: expected_fingerprints,
            }),
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
        return Err(format!(
            "paired suite `{}` 的 {} 第 {pair} 对样本失败",
            options.suite.as_str(),
            arm.as_str()
        )
        .into());
    }
    Ok(())
}

fn save_contract(
    baseline_contract: Option<BaselineContract>,
    arm: PairedArm,
) -> SaveMeasurementContract {
    match (baseline_contract, arm) {
        (Some(BaselineContract::LegacyCargoDevV1), PairedArm::Baseline) => {
            SaveMeasurementContract::LegacyConfigOnlyBaseline
        }
        (Some(BaselineContract::LegacyCargoDevV2), PairedArm::Baseline) => {
            SaveMeasurementContract::LegacyApiWorkerBaseline
        }
        _ => SaveMeasurementContract::Current,
    }
}

fn create_run_directories(
    devex_root: &Path,
    options: &DevexRunOptions,
) -> Result<PairedRunDirectories> {
    let now = Utc::now();
    let date = now.format("%Y-%m-%d").to_string();
    let comparison_id = format!(
        "{}-{}-{}-{}-paired-{}",
        now.format("%Y%m%dT%H%M%S%9fZ"),
        options.suite.as_str(),
        options.variant,
        options.cache_state.as_str(),
        std::process::id()
    );
    let parent = devex_root.join(date);
    fs::create_dir_all(&parent)?;
    let baseline_id = format!("{comparison_id}-baseline");
    let candidate_id = format!("{comparison_id}-candidate");
    let baseline_dir = parent.join(&baseline_id);
    let candidate_dir = parent.join(&candidate_id);
    fs::create_dir(&baseline_dir)?;
    if let Err(error) = fs::create_dir(&candidate_dir) {
        let _ = fs::remove_dir(&baseline_dir);
        return Err(error.into());
    }
    Ok(PairedRunDirectories {
        comparison_id,
        baseline_id,
        baseline_dir,
        candidate_id,
        candidate_dir,
    })
}
