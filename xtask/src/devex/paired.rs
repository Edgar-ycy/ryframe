use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chrono::Utc;

use crate::Result;

use super::{
    execution::{
        RunSession, SampleAudit, capture_sccache_stats, execute_sample, execute_warmup,
        prepare_run_at, sample_record, stop_sccache_server,
    },
    metadata::collect_source_fingerprints,
    model::{DevexPairedOptions, DevexRunOptions, PairedArm, PairingMetadata, SuiteDefinition},
    preflight,
    report::{SampleKind, append_sample, summarize},
    support::{cleanup_successful_sample_target, sample_target},
};

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
    preflight::check(
        &roots.baseline_backend,
        &roots.baseline_frontend,
        &options.run,
        definition,
    )?;
    preflight::check(
        &roots.candidate_backend,
        &roots.candidate_frontend,
        &options.run,
        definition,
    )?;

    let devex_root = coordinator_backend.join(".local-tests/devex");
    let directories = create_run_directories(&devex_root, &options.run)?;
    let baseline = prepare_arm(
        &roots.baseline_backend,
        &roots.baseline_frontend,
        &devex_root,
        &options.run,
        definition,
        &directories,
        PairedArm::Baseline,
    )?;
    let candidate = prepare_arm(
        &roots.candidate_backend,
        &roots.candidate_frontend,
        &devex_root,
        &options.run,
        definition,
        &directories,
        PairedArm::Candidate,
    )?;

    let execution = execute_samples(&roots, &options.run, &baseline, &candidate);
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
    Ok((baseline.run_dir, candidate.run_dir))
}

struct PairedRoots {
    baseline_backend: PathBuf,
    candidate_backend: PathBuf,
    baseline_frontend: PathBuf,
    candidate_frontend: PathBuf,
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
    let (baseline_frontend, candidate_frontend) = if definition.requires_frontend {
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
        (baseline, candidate)
    } else {
        (
            coordinator_frontend.to_path_buf(),
            coordinator_frontend.to_path_buf(),
        )
    };
    Ok(PairedRoots {
        baseline_backend,
        candidate_backend,
        baseline_frontend,
        candidate_frontend,
    })
}

fn canonical_worktree(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "{label} worktree 不存在或不可读 {}：{error}",
            path.display()
        )
    })?;
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&canonical)
        .output()?;
    if !output.status.success() {
        return Err(format!("{label} 不是 Git worktree：{}", canonical.display()).into());
    }
    let top = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        .canonicalize()
        .map_err(|error| format!("无法规范化 {label} Git 根目录：{error}"))?;
    if top != canonical {
        return Err(format!("{label} 必须显式指向 worktree 根目录：{}", top.display()).into());
    }
    Ok(canonical)
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
    backend_root: &Path,
    frontend_root: &Path,
    devex_root: &Path,
    options: &DevexRunOptions,
    definition: SuiteDefinition,
    directories: &PairedRunDirectories,
    arm: PairedArm,
) -> Result<RunSession> {
    let (run_id, run_dir) = match arm {
        PairedArm::Baseline => (&directories.baseline_id, &directories.baseline_dir),
        PairedArm::Candidate => (&directories.candidate_id, &directories.candidate_dir),
    };
    prepare_run_at(
        backend_root,
        frontend_root,
        devex_root,
        options,
        definition,
        run_id.clone(),
        run_dir.clone(),
        Some(PairingMetadata {
            comparison_id: directories.comparison_id.clone(),
            arm,
        }),
    )
}

fn execute_samples(
    roots: &PairedRoots,
    options: &DevexRunOptions,
    baseline: &RunSession,
    candidate: &RunSession,
) -> Result<()> {
    execute_warmup(
        &roots.baseline_backend,
        &roots.baseline_frontend,
        options,
        baseline,
    )?;
    execute_warmup(
        &roots.candidate_backend,
        &roots.candidate_frontend,
        options,
        candidate,
    )?;
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
            execute_measurement(roots, options, baseline, candidate, arm, pair, order)?;
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
    baseline: &RunSession,
    candidate: &RunSession,
    arm: PairedArm,
    pair: usize,
    order: usize,
) -> Result<()> {
    let (backend_root, frontend_root, session) = match arm {
        PairedArm::Baseline => (
            roots.baseline_backend.as_path(),
            roots.baseline_frontend.as_path(),
            baseline,
        ),
        PairedArm::Candidate => (
            roots.candidate_backend.as_path(),
            roots.candidate_frontend.as_path(),
            candidate,
        ),
    };
    let expected_fingerprints = source_fingerprints(backend_root, frontend_root, session)?;
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
        backend_root,
        frontend_root,
        &target,
        session.definition,
        &session.environment,
        options.suite,
        &options.variant,
        &label,
    )?;
    let restored = source_fingerprints(backend_root, frontend_root, session)?;
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
            outcome,
            &session.normalizer,
            Some(SampleAudit {
                arm,
                pair: Some(pair),
                order: Some(order),
                source_fingerprints: expected_fingerprints,
            }),
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
        return Err(format!(
            "paired suite `{}` 的 {} 第 {pair} 对样本失败",
            options.suite.as_str(),
            arm.as_str()
        )
        .into());
    }
    Ok(())
}

fn source_fingerprints(
    backend_root: &Path,
    frontend_root: &Path,
    session: &RunSession,
) -> Result<super::metadata::SourceFingerprints> {
    let observed = collect_source_fingerprints(
        backend_root,
        session
            .definition
            .requires_frontend
            .then_some(frontend_root),
    )?;
    if observed != session.source_fingerprints {
        return Err(format!(
            "paired {} worktree 在测量期间发生变化，拒绝混入样本",
            session
                .pairing
                .as_ref()
                .map_or("unknown", |pairing| pairing.arm.as_str())
        )
        .into());
    }
    Ok(observed)
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
