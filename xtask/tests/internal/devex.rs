use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    cli::{Command, parse},
    dev::{
        ReadyKind, SaveCase, SaveMeasurement, SaveMeasurementContract, read_measurement,
        read_measurement_with_contract,
    },
    devex::{
        BaselineContract, CacheState, DevexCommand, DevexSuite, PairedArm, PathNormalizer,
        abba_pair_order, cleanup_successful_sample_target, compare, distribution,
        filter_environment, sample_target, summarize, with_source_edit,
    },
    source_edit::SourceEdit,
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn successful_targets_follow_suite_storage_policy() {
    let run = temporary_directory("cold-target-cleanup");
    let cold = run.join("cache/cold-001");
    fs::create_dir_all(&cold).unwrap();
    fs::write(cold.join("artifact"), b"ok").unwrap();
    cleanup_successful_sample_target(&run, &cold, DevexSuite::RustColdBuild, CacheState::Cold)
        .unwrap();
    assert!(!cold.exists());

    let warm = run.join("cache/warm");
    fs::create_dir_all(&warm).unwrap();
    cleanup_successful_sample_target(&run, &warm, DevexSuite::RustIncremental, CacheState::Warm)
        .unwrap();
    assert!(warm.is_dir());

    let isolated = run.join("cache/sccache-measure-001");
    fs::create_dir_all(&isolated).unwrap();
    cleanup_successful_sample_target(&run, &isolated, DevexSuite::RustGate, CacheState::Warm)
        .unwrap();
    assert!(!isolated.exists());
    fs::remove_dir_all(run).unwrap();
}

#[test]
fn fixed_suite_whitelist_is_complete_and_closed() {
    let names = DevexSuite::ALL.map(DevexSuite::as_str);
    assert_eq!(
        names,
        [
            "rust-cold-build",
            "rust-incremental",
            "cargo-dev-save",
            "resource-generator",
            "resource-gate",
            "rust-gate",
            "rust-sccache",
            "frontend-fast",
            "frontend-build",
        ]
    );
    assert!(
        names
            .into_iter()
            .all(|name| DevexSuite::parse(name).is_some())
    );
    assert!(DevexSuite::parse("backend-check").is_none());
}

#[test]
fn cli_requires_named_run_and_compare_arguments() {
    let cli = parse(strings(&[
        "devex",
        "run",
        "--suite",
        "rust-cold-build",
        "--variant",
        "workspace",
        "--runs",
        "20",
        "--cache",
        "cold",
    ]))
    .unwrap();
    let Command::Devex(DevexCommand::Run(options)) = cli.command else {
        panic!("应解析为 DevEx run");
    };
    assert_eq!(options.suite, DevexSuite::RustColdBuild);
    assert_eq!(options.variant, "workspace");
    assert_eq!(options.runs, 20);
    assert_eq!(options.cache_state, CacheState::Cold);

    assert!(parse(strings(&["devex", "run", "rust-cold-build"])).is_err());
    assert!(
        parse(strings(&[
            "devex",
            "run",
            "--suite",
            "backend-check",
            "--variant",
            "x",
            "--runs",
            "1",
            "--cache",
            "warm",
        ]))
        .is_err()
    );
    assert!(parse(strings(&["devex", "compare", "base/run", "candidate/run",])).is_err());
    assert!(matches!(
        parse(strings(&[
            "devex",
            "compare",
            "--base",
            "2026-08-01/base",
            "--candidate",
            "2026-08-02/candidate",
        ]))
        .unwrap()
        .command,
        Command::Devex(DevexCommand::Compare { .. })
    ));
}

#[test]
fn paired_cli_requires_two_explicit_backend_worktrees() {
    let cli = parse(strings(&[
        "devex",
        "paired",
        "--base-backend",
        "D:/worktrees/base",
        "--candidate-backend",
        "D:/worktrees/candidate",
        "--suite",
        "rust-cold-build",
        "--variant",
        "api",
        "--runs",
        "20",
        "--cache",
        "cold",
    ]))
    .unwrap();
    let Command::Devex(DevexCommand::Paired(options)) = cli.command else {
        panic!("应解析为 DevEx paired");
    };
    assert_eq!(options.baseline_backend, Path::new("D:/worktrees/base"));
    assert_eq!(
        options.candidate_backend,
        Path::new("D:/worktrees/candidate")
    );
    assert!(options.baseline_frontend.is_none());
    assert!(options.candidate_frontend.is_none());

    let error = parse(strings(&[
        "devex",
        "paired",
        "--base-backend",
        "D:/worktrees/base",
        "--suite",
        "rust-cold-build",
        "--variant",
        "api",
        "--runs",
        "20",
        "--cache",
        "cold",
    ]))
    .unwrap_err()
    .to_string();
    assert!(error.contains("--candidate-backend"), "{error}");

    let error = parse(strings(&[
        "devex",
        "paired",
        "--base-backend",
        "D:/worktrees/base",
        "--candidate-backend",
        "D:/worktrees/candidate",
        "--suite",
        "rust-gate",
        "--variant",
        "default",
        "--runs",
        "20",
        "--cache",
        "warm",
    ]))
    .unwrap_err()
    .to_string();
    assert!(error.contains("两个前端 worktree"), "{error}");
}

#[test]
fn legacy_baseline_contract_is_closed_to_config_only() {
    let cli = parse(strings(&[
        "devex",
        "paired",
        "--base-backend",
        "D:/worktrees/base",
        "--candidate-backend",
        "D:/worktrees/candidate",
        "--baseline-contract",
        "legacy-cargo-dev-v1",
        "--suite",
        "cargo-dev-save",
        "--variant",
        "config-only",
        "--runs",
        "5",
        "--cache",
        "warm",
    ]))
    .unwrap();
    let Command::Devex(DevexCommand::Paired(options)) = cli.command else {
        panic!("应解析为 DevEx paired");
    };
    assert_eq!(
        options.baseline_contract,
        Some(BaselineContract::LegacyCargoDevV1)
    );

    for (suite, variant, contract) in [
        ("rust-cold-build", "api", "legacy-cargo-dev-v1"),
        ("cargo-dev-save", "api-only", "legacy-cargo-dev-v1"),
        ("cargo-dev-save", "config-only", "legacy-cargo-dev-v2"),
    ] {
        assert!(
            parse(strings(&[
                "devex",
                "paired",
                "--base-backend",
                "D:/worktrees/base",
                "--candidate-backend",
                "D:/worktrees/candidate",
                "--baseline-contract",
                contract,
                "--suite",
                suite,
                "--variant",
                variant,
                "--runs",
                "20",
                "--cache",
                "warm",
            ]))
            .is_err()
        );
    }
}

#[test]
fn suite_variant_selects_the_measured_workload_and_sample_policy() {
    let api = DevexSuite::RustColdBuild.definition("api").unwrap();
    assert_eq!(api.features, &["bin-api"]);
    assert_eq!(api.steps.len(), 1);
    assert!(
        api.steps[0]
            .args
            .windows(2)
            .any(|pair| pair == ["--bin", "ryframe"])
    );

    let save = DevexSuite::CargoDevSave.definition("config-only").unwrap();
    assert_eq!(
        save.environment,
        &[
            ("RYFRAME_DEVEX_SAVE_CASE", "config-only"),
            (
                "RYFRAME_DEVEX_SAVE_RESULT_PATH",
                "{target}/cargo-dev-save-result.json"
            )
        ]
    );
    let frontend = DevexSuite::FrontendFast.definition("default").unwrap();
    assert_eq!(frontend.steps[0].args, &["pnpm", "check:fast"]);

    assert!(DevexSuite::RustColdBuild.definition("baseline").is_err());
    assert!(DevexSuite::CargoDevSave.definition("baseline").is_err());
    assert_eq!(DevexSuite::RustColdBuild.minimum_runs("api"), 20);
    assert_eq!(DevexSuite::CargoDevSave.minimum_runs("api-only"), 20);
    assert_eq!(DevexSuite::CargoDevSave.minimum_runs("cancellation"), 20);
    assert_eq!(DevexSuite::CargoDevSave.minimum_runs("config-only"), 5);
    let cancellation = DevexSuite::CargoDevSave.definition("cancellation").unwrap();
    assert!(
        cancellation
            .environment
            .contains(&("RYFRAME_DEVEX_SAVE_CASE", "cancellation"))
    );
    assert_eq!(DevexSuite::FrontendFast.minimum_runs("default"), 5);

    let generator = DevexSuite::ResourceGenerator.definition("post").unwrap();
    assert!(generator.steps[0].args.contains(&"{target}"));
    let gate = DevexSuite::ResourceGate.definition("auto").unwrap();
    assert!(gate.steps[0].args.contains(&"{target}/driver"));
    assert_eq!(
        gate.environment,
        &[("RYFRAME_DEVEX_TARGET_ROOT", "{target}")]
    );
    let rust_gate = DevexSuite::RustGate.definition("default").unwrap();
    assert!(rust_gate.steps[0].args.contains(&"rust-gate"));
    assert!(rust_gate.steps[0].args.contains(&"{target}/driver"));
    assert!(
        rust_gate
            .environment
            .contains(&("RUSTC_WRAPPER", "sccache"))
    );
    assert!(
        rust_gate
            .environment
            .contains(&("RYFRAME_CI_RUST_GATE_PROFILE", "standard"))
    );
    assert!(
        rust_gate
            .remove_environment
            .contains(&"RYFRAME_CI_FRONTEND_REF")
    );
    assert_eq!(DevexSuite::RustGate.minimum_runs("default"), 20);
}

#[test]
fn abba_order_and_sccache_targets_are_auditable() {
    assert_eq!(
        [1, 2, 3, 4].map(abba_pair_order),
        [
            [PairedArm::Baseline, PairedArm::Candidate],
            [PairedArm::Candidate, PairedArm::Baseline],
            [PairedArm::Baseline, PairedArm::Candidate],
            [PairedArm::Candidate, PairedArm::Baseline],
        ]
    );
    let run = Path::new("D:/devex/run");
    assert_ne!(
        sample_target(run, DevexSuite::RustSccache, CacheState::Warm, 1),
        sample_target(run, DevexSuite::RustSccache, CacheState::Warm, 2)
    );
    assert_ne!(
        sample_target(run, DevexSuite::RustGate, CacheState::Warm, 1),
        sample_target(run, DevexSuite::RustGate, CacheState::Warm, 2)
    );
    assert_eq!(
        sample_target(run, DevexSuite::RustIncremental, CacheState::Warm, 1),
        sample_target(run, DevexSuite::RustIncremental, CacheState::Warm, 2)
    );
}

#[test]
fn incremental_edit_is_restored_after_success_and_failure() {
    let directory = temporary_directory("incremental-restore");
    let source = directory.join("source.rs");
    let original = b"pub fn value() -> u8 { 1 }\n";
    fs::write(&source, original).unwrap();

    let modified = with_source_edit(&source, "success", || Ok(fs::read(&source)?)).unwrap();
    assert_ne!(modified, original);
    assert_eq!(fs::read(&source).unwrap(), original);

    let failure = with_source_edit(&source, "failure", || -> crate::Result<()> {
        assert_ne!(fs::read(&source)?, original);
        Err("模拟编译失败".into())
    });
    assert!(failure.unwrap_err().to_string().contains("模拟编译失败"));
    assert_eq!(fs::read(&source).unwrap(), original);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn source_restore_refuses_to_overwrite_a_concurrent_edit() {
    let directory = temporary_directory("incremental-conflict");
    let source = directory.join("source.rs");
    fs::write(&source, b"pub fn value() -> u8 { 1 }\n").unwrap();

    let (mut edit, _) = SourceEdit::apply(&source, "conflict", "//").unwrap();
    let concurrent = b"pub fn value() -> u8 { 2 }\n";
    fs::write(&source, concurrent).unwrap();
    let error = edit.restore().unwrap_err().to_string();

    assert!(error.contains("拒绝覆盖"));
    assert_eq!(fs::read(&source).unwrap(), concurrent);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cargo_dev_save_result_is_strict_and_structured() {
    let directory = temporary_directory("cargo-dev-save-result");
    let path = directory.join("result.json");
    let expected = SaveMeasurement {
        schema_version: 1,
        case: SaveCase::ConfigOnly,
        started_at: "2026-08-27T00:00:00Z".to_owned(),
        save_to_ready_ms: 42.5,
        cargo_invocations: 0,
        ready_kind: ReadyKind::Promoted,
    };
    fs::write(&path, serde_json::to_vec(&expected).unwrap()).unwrap();
    assert_eq!(read_measurement(&path).unwrap(), expected);

    fs::write(
        &path,
        br#"{"schemaVersion":2,"case":"config-only","startedAt":"bad","saveToReadyMs":1.0,"cargoInvocations":0,"readyKind":"promoted"}"#,
    )
    .unwrap();
    assert!(read_measurement(&path).is_err());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn legacy_config_result_requires_one_cargo_without_relaxing_current_results() {
    let directory = temporary_directory("legacy-cargo-dev-save-result");
    let path = directory.join("result.json");
    let legacy = SaveMeasurement {
        schema_version: 1,
        case: SaveCase::ConfigOnly,
        started_at: "2026-08-27T00:00:00Z".to_owned(),
        save_to_ready_ms: 42.5,
        cargo_invocations: 1,
        ready_kind: ReadyKind::Promoted,
    };
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(read_measurement(&path).is_err());
    assert_eq!(
        read_measurement_with_contract(&path, SaveMeasurementContract::LegacyConfigOnlyBaseline)
            .unwrap(),
        legacy
    );

    let mut wrong_case = legacy;
    wrong_case.case = SaveCase::ApiOnly;
    fs::write(&path, serde_json::to_vec(&wrong_case).unwrap()).unwrap();
    assert!(
        read_measurement_with_contract(&path, SaveMeasurementContract::LegacyConfigOnlyBaseline)
            .is_err()
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cli_rejects_under_sampled_and_unknown_variants() {
    let under_sampled = parse(strings(&[
        "devex",
        "run",
        "--suite",
        "frontend-fast",
        "--variant",
        "default",
        "--runs",
        "4",
        "--cache",
        "warm",
    ]));
    assert!(
        under_sampled
            .unwrap_err()
            .to_string()
            .contains("至少需要 5 次")
    );

    let unknown = parse(strings(&[
        "devex",
        "run",
        "--suite",
        "cargo-dev-save",
        "--variant",
        "baseline",
        "--runs",
        "5",
        "--cache",
        "warm",
    ]));
    assert!(unknown.unwrap_err().to_string().contains("变体"));
}

#[test]
fn environment_snapshot_only_keeps_explicit_safe_names() {
    let environment = filter_environment([
        (OsString::from("CARGO_INCREMENTAL"), OsString::from("0")),
        (OsString::from("RYFRAME_VERIFY_JOBS"), OsString::from("8")),
        (OsString::from("RUSTFLAGS"), OsString::from("-Cdebuginfo=0")),
        (
            OsString::from("APP_AUTH_JWT_SECRET"),
            OsString::from("never-record-this"),
        ),
        (
            OsString::from("ACTIONS_RUNTIME_TOKEN"),
            OsString::from("never-record-this-either"),
        ),
    ]);
    assert_eq!(environment.get("CARGO_INCREMENTAL").unwrap(), "0");
    assert_eq!(environment.get("RYFRAME_VERIFY_JOBS").unwrap(), "8");
    assert!(environment.contains_key("RUSTFLAGS"));
    assert!(!environment.contains_key("APP_AUTH_JWT_SECRET"));
    assert!(!environment.contains_key("ACTIONS_RUNTIME_TOKEN"));
}

#[test]
fn metadata_paths_use_stable_workspace_tokens() {
    let normalizer = PathNormalizer::new(
        Path::new("D:/workspace/backend"),
        Path::new("D:/workspace/frontend"),
        Path::new("D:/workspace/backend/.local-tests/devex"),
    );
    assert_eq!(
        normalizer.normalize("D:\\workspace\\backend\\.local-tests\\devex\\2026-08-27\\run"),
        "$DEVEX/2026-08-27/run"
    );
    assert_eq!(
        normalizer.normalize("D:\\workspace\\frontend\\src\\main.ts"),
        "$FRONTEND/src/main.ts"
    );
}

#[test]
fn summary_uses_nearest_rank_p50_and_p95() {
    let distribution = distribution(&[50.0, 10.0, 20.0, 40.0, 30.0]).unwrap();
    assert_eq!(distribution.min, 10.0);
    assert_eq!(distribution.p50, 30.0);
    assert_eq!(distribution.p95, 50.0);
    assert_eq!(distribution.max, 50.0);
    assert_eq!(distribution.mean, 30.0);
}

#[test]
fn summarize_and_compare_validate_the_recorded_compile_surface() {
    let baseline = fake_paired_run(
        "baseline",
        "sha256:same",
        &[100.0, 200.0],
        "comparison-a",
        PairedArm::Baseline,
    );
    let candidate = fake_paired_run(
        "candidate",
        "sha256:same",
        &[80.0, 180.0],
        "comparison-a",
        PairedArm::Candidate,
    );
    let summary = summarize(&baseline).unwrap();
    assert_eq!(summary.samples, 2);
    assert_eq!(summary.duration_ms.unwrap().p95, 200.0);
    assert!(baseline.join("summary.md").is_file());
    assert!(
        compare(&baseline, &candidate)
            .unwrap()
            .contains("DevEx 对比")
    );

    let incompatible = fake_paired_run(
        "incompatible",
        "sha256:different",
        &[90.0, 95.0],
        "comparison-a",
        PairedArm::Candidate,
    );
    let error = compare(&baseline, &incompatible).unwrap_err().to_string();
    assert!(error.contains("compile_surface_fingerprint"), "{error}");
    let incomplete = fake_paired_run(
        "incomplete",
        "sha256:same",
        &[90.0, 95.0],
        "comparison-a",
        PairedArm::Candidate,
    );
    let metadata_path = incomplete.join("metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["requested_runs"] = serde_json::json!(3);
    fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let error = compare(&baseline, &incomplete).unwrap_err().to_string();
    assert!(error.contains("样本不完整"), "{error}");
    for path in [baseline, candidate, incompatible, incomplete] {
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn compare_rejects_unpaired_runs() {
    let baseline = fake_run("standalone-base", "sha256:same", &[100.0]);
    let candidate = fake_run("standalone-candidate", "sha256:same", &[90.0]);
    let error = compare(&baseline, &candidate).unwrap_err().to_string();
    assert!(error.contains("paired runner"), "{error}");
    fs::remove_dir_all(baseline).unwrap();
    fs::remove_dir_all(candidate).unwrap();
}

#[test]
fn compare_enforces_legacy_baseline_cargo_counts() {
    let baseline = fake_legacy_paired_run("legacy-base", PairedArm::Baseline, 1);
    let candidate = fake_legacy_paired_run("legacy-candidate", PairedArm::Candidate, 0);
    assert!(compare(&baseline, &candidate).is_ok());

    let invalid = fake_legacy_paired_run("legacy-invalid", PairedArm::Baseline, 0);
    let error = compare(&invalid, &candidate).unwrap_err().to_string();
    assert!(error.contains("Cargo=1"), "{error}");
    for path in [baseline, candidate, invalid] {
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn sccache_summary_reports_delta_hit_rate_and_errors() {
    let run = fake_run("sccache", "sha256:same", &[100.0]);
    let metadata_path = run.join("metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["toolchain"]["sccache"] = serde_json::json!("sccache 0.17.0");
    fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    write_sccache_stats(&run.join("sccache-before.json"), 10, 4, 2, 1, 0);
    write_sccache_stats(&run.join("sccache-after.json"), 20, 12, 4, 2, 1);

    let summary = summarize(&run).unwrap();
    assert_eq!(summary.sccache_version.as_deref(), Some("sccache 0.17.0"));
    let stats = summary.sccache.unwrap();

    assert_eq!(stats.compile_requests, 10);
    assert_eq!(stats.cache_hits, 8);
    assert_eq!(stats.cache_misses, 2);
    assert_eq!(stats.not_cacheable, 1);
    assert_eq!(stats.cache_errors, 1);
    assert_eq!(stats.hit_rate, Some(0.8));
    fs::remove_dir_all(run).unwrap();
}

#[test]
fn compare_rejects_different_sccache_executables() {
    let baseline = fake_paired_run(
        "sccache-version-base",
        "sha256:same",
        &[100.0],
        "comparison-version",
        PairedArm::Baseline,
    );
    let candidate = fake_paired_run(
        "sccache-version-candidate",
        "sha256:same",
        &[90.0],
        "comparison-version",
        PairedArm::Candidate,
    );
    for (path, version) in [
        (&baseline, "sccache 0.15.0"),
        (&candidate, "sccache 0.17.0"),
    ] {
        let metadata_path = path.join("metadata.json");
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
        metadata["toolchain"]["sccache"] = serde_json::json!(version);
        fs::write(metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    }
    let error = compare(&baseline, &candidate).unwrap_err().to_string();
    assert!(error.contains("sccache 可执行版本"), "{error}");
    fs::remove_dir_all(baseline).unwrap();
    fs::remove_dir_all(candidate).unwrap();
}

fn write_sccache_stats(
    path: &Path,
    requests: u64,
    hits: u64,
    misses: u64,
    not_cacheable: u64,
    errors: u64,
) {
    let document = serde_json::json!({
        "stats": {
            "compile_requests": requests,
            "requests_not_cacheable": not_cacheable,
            "cache_hits": { "counts": { "Rust": hits } },
            "cache_misses": { "counts": { "Rust": misses } },
            "cache_errors": { "counts": { "Rust": errors } },
            "cache_timeouts": 0,
            "cache_read_errors": 0,
            "cache_write_errors": 0,
            "dist_errors": 0,
        }
    });
    fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
}

fn fake_paired_run(
    name: &str,
    fingerprint: &str,
    durations: &[f64],
    comparison_id: &str,
    arm: PairedArm,
) -> PathBuf {
    let directory = temporary_directory(name);
    let source_fingerprint = format!("sha256:source-{}", arm.as_str());
    let metadata = serde_json::json!({
        "schema_version": 1,
        "run_id": name,
        "started_at": "2026-08-27T00:00:00Z",
        "suite": "rust-cold-build",
        "variant": "api",
        "cache_state": "cold",
        "requested_runs": durations.len(),
        "backend": {
            "commit": null,
            "dirty": false,
            "worktree_fingerprint": source_fingerprint,
        },
        "frontend": null,
        "toolchain": { "cargo": "cargo", "rustc": "rustc", "node": null, "pnpm": null },
        "target": "x86_64-pc-windows-msvc",
        "features": ["bin-api"],
        "jobs": 4,
        "environment": {},
        "environment_hash": "sha256:environment",
        "commands": [],
        "compile_surface_fingerprint": fingerprint,
        "input_fingerprint": format!("sha256:{name}"),
        "pairing": { "comparison_id": comparison_id, "arm": arm.as_str() },
    });
    fs::write(
        directory.join("metadata.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let samples = durations
        .iter()
        .enumerate()
        .map(|(index, duration)| {
            let pair = index + 1;
            let order = match (pair % 2, arm) {
                (1, PairedArm::Baseline) | (0, PairedArm::Candidate) => pair * 2 - 1,
                _ => pair * 2,
            };
            serde_json::json!({
                "schema_version": 1,
                "run_id": name,
                "sequence": pair,
                "kind": "measurement",
                "cache_state": "cold",
                "started_at": format!("2026-08-27T00:00:{order:02}Z"),
                "duration_ms": duration,
                "status": "passed",
                "exit_code": 0,
                "target_directory": format!("$DEVEX/cache/cold-{pair:03}"),
                "arm": arm.as_str(),
                "pair": pair,
                "order": order,
                "source_fingerprints": {
                    "backend": source_fingerprint,
                    "frontend": null,
                },
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(directory.join("samples.jsonl"), format!("{samples}\n")).unwrap();
    directory
}

fn fake_legacy_paired_run(name: &str, arm: PairedArm, cargo_invocations: usize) -> PathBuf {
    let directory = temporary_directory(name);
    let source_fingerprint = format!("sha256:source-{}", arm.as_str());
    let metadata = serde_json::json!({
        "schema_version": 1,
        "run_id": name,
        "started_at": "2026-08-27T00:00:00Z",
        "suite": "cargo-dev-save",
        "variant": "config-only",
        "cache_state": "warm",
        "requested_runs": 2,
        "backend": {
            "commit": null,
            "dirty": false,
            "worktree_fingerprint": source_fingerprint,
        },
        "frontend": null,
        "toolchain": { "cargo": "cargo", "rustc": "rustc", "node": null, "pnpm": null },
        "target": "x86_64-pc-windows-msvc",
        "features": [],
        "jobs": 4,
        "environment": {},
        "environment_hash": "sha256:environment",
        "commands": [],
        "compile_surface_fingerprint": "sha256:legacy-same",
        "input_fingerprint": format!("sha256:{name}"),
        "pairing": {
            "comparison_id": "legacy-comparison",
            "arm": arm.as_str(),
            "baseline_contract": "legacy-cargo-dev-v1",
            "baseline_provenance": {
                "base_commit": BaselineContract::LEGACY_CARGO_DEV_BASE_COMMIT,
                "adapter_commit": BaselineContract::LEGACY_CARGO_DEV_ADAPTER_COMMIT,
                "patch_sha256": format!("sha256:{}", "a".repeat(64)),
            }
        },
    });
    fs::write(
        directory.join("metadata.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let samples = [1, 2]
        .into_iter()
        .map(|pair| {
            let order = match (pair % 2, arm) {
                (1, PairedArm::Baseline) | (0, PairedArm::Candidate) => pair * 2 - 1,
                _ => pair * 2,
            };
            serde_json::json!({
                "schema_version": 1,
                "run_id": name,
                "sequence": pair,
                "kind": "measurement",
                "cache_state": "warm",
                "started_at": format!("2026-08-27T00:00:{order:02}Z"),
                "duration_ms": 100.0 + pair as f64,
                "cargo_invocations": cargo_invocations,
                "ready_kind": "promoted",
                "status": "passed",
                "exit_code": 0,
                "target_directory": "$DEVEX/cache/warm",
                "arm": arm.as_str(),
                "pair": pair,
                "order": order,
                "source_fingerprints": {
                    "backend": source_fingerprint,
                    "frontend": null,
                },
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(directory.join("samples.jsonl"), format!("{samples}\n")).unwrap();
    directory
}

fn temporary_directory(name: &str) -> PathBuf {
    static NEXT_RUN: AtomicUsize = AtomicUsize::new(0);
    let id = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let directory =
        std::env::temp_dir().join(format!("ryframe-devex-{name}-{}-{id}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    directory
}

fn fake_run(name: &str, fingerprint: &str, durations: &[f64]) -> PathBuf {
    let directory = temporary_directory(name);
    let metadata = serde_json::json!({
        "schema_version": 1,
        "run_id": name,
        "started_at": "2026-08-27T00:00:00Z",
        "suite": "rust-cold-build",
        "variant": "api",
        "cache_state": "cold",
        "requested_runs": durations.len(),
        "backend": { "commit": null, "dirty": false },
        "frontend": null,
        "toolchain": { "cargo": "cargo", "rustc": "rustc", "node": null, "pnpm": null },
        "target": "x86_64-pc-windows-msvc",
        "features": ["workspace-default"],
        "jobs": 4,
        "environment": {},
        "environment_hash": "sha256:environment",
        "commands": [],
        "compile_surface_fingerprint": fingerprint,
        "input_fingerprint": format!("sha256:{name}"),
    });
    fs::write(
        directory.join("metadata.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let samples = durations
        .iter()
        .enumerate()
        .map(|(index, duration)| {
            serde_json::json!({
                "schema_version": 1,
                "run_id": name,
                "sequence": index + 1,
                "kind": "measurement",
                "cache_state": "cold",
                "started_at": "2026-08-27T00:00:00Z",
                "duration_ms": duration,
                "status": "passed",
                "exit_code": 0,
                "target_directory": "$DEVEX/cache/cold",
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(directory.join("samples.jsonl"), format!("{samples}\n")).unwrap();
    directory
}
