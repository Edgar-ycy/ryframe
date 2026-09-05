use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    dev::{
        ReadyKind, SaveCase, SaveMeasurement, SaveMeasurementContract, read_measurement,
        read_measurement_with_contract,
    },
    devex::{
        BaselineContract, CacheState, DevexRunOptions, DevexSuite, PairedArm, PathNormalizer,
        abba_pair_order, cleanup_successful_sample_target, compare, distribution,
        filter_environment, read_resource_gate_decision, require_frontend_dependencies,
        sample_target, summarize, with_source_edit,
    },
    source_edit::SourceEdit,
};

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

    let sccache = sample_target(&run, DevexSuite::RustSccache, CacheState::Warm, 1);
    fs::create_dir_all(&sccache).unwrap();
    fs::write(sccache.join("artifact"), b"ok").unwrap();
    cleanup_successful_sample_target(&run, &sccache, DevexSuite::RustSccache, CacheState::Warm)
        .unwrap();
    assert!(!sccache.exists());

    let isolated = run.join("cache/sccache-measure-001");
    fs::create_dir_all(&isolated).unwrap();
    cleanup_successful_sample_target(&run, &isolated, DevexSuite::RustGate, CacheState::Warm)
        .unwrap();
    assert!(!isolated.exists());
    fs::remove_dir_all(run).unwrap();
}

#[test]
fn rust_gate_preflight_requires_installed_frontend_dependencies() {
    let frontend = temporary_directory("rust-gate-frontend-preflight");
    fs::write(frontend.join("package.json"), b"{}\n").unwrap();
    let options = DevexRunOptions {
        suite: DevexSuite::RustGate,
        variant: "default".to_owned(),
        cache_state: CacheState::Warm,
        runs: 20,
    };

    let error = require_frontend_dependencies(&frontend, &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("corepack pnpm install --frozen-lockfile"));
    fs::create_dir(frontend.join("node_modules")).unwrap();
    require_frontend_dependencies(&frontend, &options).unwrap();
    fs::remove_dir_all(frontend).unwrap();
}

#[test]
fn resource_gate_decision_artifact_must_prove_targeted_mode() {
    let target = temporary_directory("resource-gate-decision");
    let valid = target.join("resource-gate-decision-sample-001.json");
    fs::write(
        &valid,
        br#"{"formatVersion":1,"recognized":true,"mode":"targeted","fallback":null,"steps":["resource-drift"]}"#,
    )
    .unwrap();
    let evidence = read_resource_gate_decision(&target, "sample-001").unwrap();
    assert!(evidence.recognized && evidence.mode == "targeted");
    assert!(evidence.artifact_sha256.starts_with("sha256:"));

    let invalid = target.join("resource-gate-decision-sample-002.json");
    fs::write(
        invalid,
        br#"{"formatVersion":1,"recognized":true,"mode":"targeted","fallback":null,"steps":[]}"#,
    )
    .unwrap();
    let error = read_resource_gate_decision(&target, "sample-002")
        .unwrap_err()
        .to_string();
    assert!(error.contains("未证明") && error.contains("targeted"));
    fs::remove_dir_all(target).unwrap();
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
fn suite_definitions_select_the_measured_workload() {
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
    assert_eq!(frontend.steps[0].args, &["pnpm", "check"]);

    let incremental = DevexSuite::RustIncremental
        .definition("application")
        .unwrap();
    assert_eq!(
        incremental.steps[0].args,
        &["check", "--locked", "-p", "ryframe-application"]
    );
    assert_eq!(incremental.features, &["application"]);
    assert_eq!(
        DevexSuite::RustIncremental
            .incremental_source("application")
            .unwrap(),
        Some("crates/ryframe-application/src/lib.rs")
    );
    assert!(DevexSuite::RustColdBuild.definition("application").is_err());

    let generator = DevexSuite::ResourceGenerator.definition("post").unwrap();
    assert!(generator.steps[0].args.contains(&"{target}"));
    assert!(
        api.remove_environment
            .contains(&"CMAKE_C_COMPILER_LAUNCHER")
    );
    assert!(
        api.remove_environment
            .contains(&"CMAKE_CXX_COMPILER_LAUNCHER")
    );
}

#[test]
fn suite_sample_policy_rejects_semantic_cache_mismatches() {
    assert!(DevexSuite::RustColdBuild.definition("baseline").is_err());
    assert!(DevexSuite::CargoDevSave.definition("baseline").is_err());
    assert_eq!(DevexSuite::RustColdBuild.minimum_runs("api"), 20);
    assert_eq!(DevexSuite::RustIncremental.minimum_runs("application"), 5);
    assert_eq!(DevexSuite::RustIncremental.minimum_runs("workspace"), 20);
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
    assert!(
        DevexSuite::RustColdBuild
            .validate_cache_state(CacheState::Cold)
            .is_ok()
    );
    assert!(
        DevexSuite::RustIncremental
            .validate_cache_state(CacheState::Warm)
            .is_ok()
    );
    for suite in [
        DevexSuite::CargoDevSave,
        DevexSuite::ResourceGenerator,
        DevexSuite::FrontendFast,
        DevexSuite::FrontendBuild,
    ] {
        assert!(suite.validate_cache_state(CacheState::Cold).is_ok());
        assert!(suite.validate_cache_state(CacheState::Warm).is_ok());
    }
}

#[test]
fn gate_suite_definitions_preserve_audited_execution_contracts() {
    let gate = DevexSuite::ResourceGate.definition("auto").unwrap();
    assert!(gate.steps[0].args.contains(&"{target}/driver"));
    assert_eq!(
        gate.environment,
        &[
            ("RYFRAME_DEVEX_TARGET_ROOT", "{target}"),
            ("RYFRAME_RESOURCE_GATE_TARGETED", "replay-verified-v1"),
            (
                "RYFRAME_RESOURCE_GATE_DECISION_FILE",
                "{target}/resource-gate-decision-{label}.json"
            )
        ]
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
    assert_eq!(
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
        ready_kind: ReadyKind::VerifiedNoRestart,
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
fn environment_snapshot_only_keeps_explicit_safe_names() {
    let environment = filter_environment([
        (OsString::from("CARGO_INCREMENTAL"), OsString::from("0")),
        (OsString::from("CARGO_NET_OFFLINE"), OsString::from("true")),
        (OsString::from("RYFRAME_VERIFY_JOBS"), OsString::from("8")),
        (OsString::from("RUSTFLAGS"), OsString::from("-Cdebuginfo=0")),
        (
            OsString::from("RUSTC_WORKSPACE_WRAPPER"),
            OsString::from("sccache"),
        ),
        (OsString::from("SCCACHE_CLIENT_SIDE"), OsString::from("1")),
        (
            OsString::from("SCCACHE_GHA_VERSION"),
            OsString::from("aws-lc-canary"),
        ),
        (
            OsString::from("APP_AUTH_JWT_SECRET"),
            OsString::from("never-record-this"),
        ),
        (
            OsString::from("ACTIONS_RUNTIME_TOKEN"),
            OsString::from("never-record-this-either"),
        ),
        (
            OsString::from("SCCACHE_REDIS"),
            OsString::from("redis://secret@localhost"),
        ),
    ]);
    assert_eq!(environment.get("CARGO_INCREMENTAL").unwrap(), "0");
    assert_eq!(environment.get("CARGO_NET_OFFLINE").unwrap(), "true");
    assert_eq!(environment.get("RYFRAME_VERIFY_JOBS").unwrap(), "8");
    assert!(environment.contains_key("RUSTFLAGS"));
    assert_eq!(
        environment.get("RUSTC_WORKSPACE_WRAPPER").unwrap(),
        "sccache"
    );
    assert_eq!(environment.get("SCCACHE_CLIENT_SIDE").unwrap(), "1");
    assert_eq!(
        environment.get("SCCACHE_GHA_VERSION").unwrap(),
        "aws-lc-canary"
    );
    assert!(!environment.contains_key("APP_AUTH_JWT_SECRET"));
    assert!(!environment.contains_key("ACTIONS_RUNTIME_TOKEN"));
    assert!(!environment.contains_key("SCCACHE_REDIS"));
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
    assert!(candidate.join("comparison.json").is_file());
    assert!(candidate.join("comparison.md").is_file());
    let regressed = fake_paired_run(
        "regressed",
        "sha256:same",
        &[95.0, 195.0],
        "comparison-a",
        PairedArm::Candidate,
    );
    assert!(compare(&baseline, &regressed).is_err());
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(regressed.join("comparison.json")).unwrap()).unwrap();
    assert_eq!(report["passed"], false);
    assert!(regressed.join("comparison.md").is_file());

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
    for path in [baseline, candidate, regressed, incompatible, incomplete] {
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
                "memory": super::devex_memory_tests::evidence(),
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
                "memory": super::devex_memory_tests::evidence(),
                "cache_state": "warm",
                "started_at": format!("2026-08-27T00:00:{order:02}Z"),
                "duration_ms": if arm == PairedArm::Baseline {
                    100.0 + pair as f64
                } else {
                    20.0 + pair as f64
                },
                "cargo_invocations": cargo_invocations,
                "ready_kind": if arm == PairedArm::Baseline { "promoted" } else { "verified-no-restart" },
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

pub(super) fn fake_run(name: &str, fingerprint: &str, durations: &[f64]) -> PathBuf {
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
                "memory": super::devex_memory_tests::evidence(),
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
