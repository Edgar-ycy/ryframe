use super::cli::{CheckCommand, Command, parse};
use super::devex::{CacheState, DevexCommand, DevexSuite, comparison_checks, distribution};
use super::devex::{DevexRunOptions, require_runtime_frontend_layout};
use super::devex_acceptance_tests::summary;
use serde_json::json;
use std::{fs, path::Path, process::Command as ProcessCommand};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn public_perf_plan_resolves_to_the_actual_runtime_driver() {
    let cli = parse(strings(&[
        "check",
        "perf",
        "run",
        "--suite",
        "runtime-api",
        "--variant",
        "10",
        "--runs",
        "6",
        "--cache",
        "warm",
    ]))
    .unwrap();
    let Command::Check(CheckCommand::Perf(DevexCommand::Run(options))) = cli.command else {
        panic!("应解析为 DevEx runtime run");
    };
    let definition = options.suite.definition(&options.variant).unwrap();
    assert_eq!(definition.steps.len(), 1);
    let step = &definition.steps[0];
    assert_eq!(step.program, "node");
    assert_eq!(step.args[0], "{driver}/tools/js/runtime.mjs");
    assert!(
        step.args
            .windows(2)
            .any(|pair| { pair == ["--runner-frontend", "{runner-frontend}"] })
    );

    let backend = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let driver = backend.join("tools/js/runtime.mjs");
    assert!(driver.is_file(), "{}", driver.display());
    let node_test = backend.join("tools/js/devex-runtime.test.mjs");
    let integration = ProcessCommand::new("node")
        .args(["--test", "--test-isolation=none", "--test-concurrency=1"])
        .arg(&node_test)
        .current_dir(backend)
        .output()
        .unwrap();
    assert!(
        integration.status.success(),
        "{}{}",
        String::from_utf8_lossy(&integration.stdout),
        String::from_utf8_lossy(&integration.stderr)
    );

    let invalid = ProcessCommand::new("node")
        .arg(driver)
        .current_dir(backend)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("参数无效"));
}

#[test]
fn stable_b0_paired_runtime_plan_binds_both_products_and_the_shared_runner_argument() {
    let cli = parse(strings(&[
        "check",
        "perf",
        "paired",
        "--base-backend",
        "D:/worktrees/b0-adapter",
        "--candidate-backend",
        "D:/worktrees/b1-backend",
        "--base-frontend",
        "D:/worktrees/b0-frontend",
        "--candidate-frontend",
        "D:/worktrees/b1-frontend",
        "--baseline-contract",
        "legacy-stable-readiness-b0-v1",
        "--suite",
        "runtime-api",
        "--variant",
        "10",
        "--runs",
        "6",
        "--cache",
        "warm",
    ]))
    .unwrap();
    let Command::Check(CheckCommand::Perf(DevexCommand::Paired(options))) = cli.command else {
        panic!("应解析为 stable-readiness B0/B1 runtime paired");
    };
    assert_eq!(
        options.baseline_backend,
        Path::new("D:/worktrees/b0-adapter")
    );
    assert_eq!(
        options.baseline_frontend.as_deref(),
        Some(Path::new("D:/worktrees/b0-frontend"))
    );
    assert_eq!(
        options.candidate_frontend.as_deref(),
        Some(Path::new("D:/worktrees/b1-frontend"))
    );
    let definitions = [
        options
            .run
            .suite
            .paired_definition(
                &options.run.variant,
                options.baseline_contract,
                super::devex::PairedArm::Baseline,
            )
            .unwrap(),
        options
            .run
            .suite
            .paired_definition(
                &options.run.variant,
                options.baseline_contract,
                super::devex::PairedArm::Candidate,
            )
            .unwrap(),
    ];
    for definition in definitions {
        assert_eq!(definition.steps.len(), 1);
        assert_eq!(definition.steps[0].program, "node");
        assert!(
            definition.steps[0]
                .args
                .windows(2)
                .any(|pair| pair == ["--frontend", "{frontend}"])
        );
        assert!(
            definition.steps[0]
                .args
                .windows(2)
                .any(|pair| pair == ["--runner-frontend", "{runner-frontend}"])
        );
    }
}

#[test]
fn runtime_preflight_separates_product_from_versioned_runner_frontend() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "target/runtime-frontend-preflight-{}",
        std::process::id()
    ));
    let product = root.join("product");
    let runner = root.join("runner");
    fs::create_dir_all(&product).unwrap();
    fs::create_dir_all(runner.join("node_modules")).unwrap();
    fs::write(product.join("package.json"), "{}\n").unwrap();
    fs::write(runner.join("package.json"), "{}\n").unwrap();
    let options = DevexRunOptions {
        suite: DevexSuite::RuntimeApi,
        variant: "10".into(),
        cache_state: CacheState::Warm,
        runs: 6,
    };
    let error = require_runtime_frontend_layout(&product, &runner, &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("browser-login-budget.mjs"));
    for relative in [
        "scripts/browser-login-budget.mjs",
        "scripts/browser-login-budget-model.mjs",
        "scripts/browser-login-ledger.mjs",
    ] {
        let path = runner.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "// fixture\n").unwrap();
    }
    require_runtime_frontend_layout(&product, &runner, &options).unwrap();
    assert!(!product.join("node_modules").exists());
    assert!(!product.join("scripts/browser-login-budget.mjs").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn homepage_failure_is_counted_once_across_metrics_and_session_failures_block_success() {
    let run = super::devex_tests::fake_run("runtime-cycles", "sha256:same", &[100.0, 100.0]);
    let path = run.join("metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    metadata["suite"] = json!("runtime-homepage");
    metadata["variant"] = json!("default");
    metadata["environment"]["RYFRAME_DEVEX_RUNTIME_INPUT_SHA256"] = json!("a".repeat(64));
    fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let path = run.join("samples.jsonl");
    let mut samples = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let mut sample: serde_json::Value = serde_json::from_str(line).unwrap();
            let failed = index as u64;
            let metric = json!({"completed": 10 - failed, "failed": failed, "elapsed_ms": 100,
            "p50_ms": 10, "p95_ms": 20, "p99_ms": 30, "throughput": 100,
            "queue_p95_ms": null, "execution_p95_ms": null});
            sample["runtime"] = json!({"input_sha256": "a".repeat(64), "cache_state": "cold",
            "network_requests": 20, "completed_cycles": 10 - failed, "failed_cycles": failed,
            "session_failures": 0, "peak_resident_memory_bytes": 100, "cpu_seconds": 1,
            "peak_database_connections": 5, "collector_failures": 0,
            "scenarios": {"navigation": metric, "lcp": metric, "content": metric}});
            if failed != 0 {
                sample["status"] = json!("failed");
                sample["exit_code"] = json!(1);
            }
            sample
        })
        .collect::<Vec<_>>();
    let save = |samples: &[serde_json::Value]| {
        fs::write(
            &path,
            samples
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
    };
    save(&samples);
    super::devex::summarize(&run).unwrap();
    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(run.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["runtime"]["completed_cycles"], 19);
    assert_eq!(summary["runtime"]["failed_cycles"], 1);
    assert_eq!(summary["runtime"]["cycle_failure_rate"], 0.05);
    assert_eq!(summary["failed"], 1);
    let report = fs::read_to_string(run.join("summary.md")).unwrap();
    assert!(report.contains("业务周期成功 19、失败 1，周期失败率 5.00%"));
    assert!(!report.contains("失败请求"));
    for pointer in ["/runtime/failed_cycles", "/runtime/scenarios/lcp/failed"] {
        let mut inconsistent = samples.clone();
        *inconsistent[1].pointer_mut(pointer).unwrap() = json!(0);
        save(&inconsistent);
        assert!(
            super::devex::summarize(&run)
                .unwrap_err()
                .to_string()
                .contains("场景计数不一致")
        );
    }
    for (pointer, value, expected) in [
        ("/runtime/network_requests", json!(0), "真实网络请求"),
        ("/runtime/scenarios/lcp/throughput", json!(0), "端到端指标"),
    ] {
        let mut invalid = samples.clone();
        *invalid[0].pointer_mut(pointer).unwrap() = value;
        save(&invalid);
        let error = super::devex::summarize(&run).unwrap_err().to_string();
        assert!(error.contains(expected), "{pointer}: {error}");
    }
    samples[0]["runtime"]["session_failures"] = json!(1);
    save(&samples);
    assert!(
        super::devex::summarize(&run)
            .unwrap_err()
            .to_string()
            .contains("会话失败")
    );
    fs::remove_dir_all(run).unwrap();
}

#[test]
fn partial_queue_measurements_cannot_disappear_from_success_summary() {
    let run = super::devex_tests::fake_run("runtime-partial", "sha256:same", &[100.0, 100.0]);
    let path = run.join("metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    metadata["suite"] = json!("runtime-jobs");
    metadata["variant"] = json!("10");
    metadata["cache_state"] = json!("warm");
    metadata["environment"]["RYFRAME_DEVEX_RUNTIME_INPUT_SHA256"] = json!("a".repeat(64));
    fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let path = run.join("samples.jsonl");
    let mut samples = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut sample: serde_json::Value = serde_json::from_str(line).unwrap();
            sample["cache_state"] = json!("warm");
            let metric = json!({"completed": 10, "failed": 0, "elapsed_ms": 100,
                "p50_ms": 10, "p95_ms": 20, "p99_ms": 30, "throughput": 100,
                "queue_p95_ms": 5, "execution_p95_ms": 15});
            sample["runtime"] = json!({
                "input_sha256": "a".repeat(64), "cache_state": "warm",
                "network_requests": 20, "completed_cycles": 40, "failed_cycles": 0, "session_failures": 0,
                "peak_resident_memory_bytes": 100, "cpu_seconds": 1,
                "peak_database_connections": 5, "collector_failures": 0,
                "scenarios": {"export": metric, "import": metric, "message": metric,
                    "schedule": metric}
            });
            sample
        })
        .collect::<Vec<_>>();
    let save = |samples: &[serde_json::Value]| {
        fs::write(
            &path,
            samples
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
    };
    save(&samples);
    assert!(super::devex::summarize(&run).is_ok());
    let report = fs::read_to_string(run.join("summary.md")).unwrap();
    assert!(report.contains("周期 P99 的样本 P95 ms"));
    assert!(report.contains("不是合并全部周期后的分位数"));
    assert!(report.contains("服务 RSS 采样峰值"));
    let mut inconsistent = samples.clone();
    inconsistent[1]["status"] = json!("failed");
    inconsistent[1]["exit_code"] = json!(1);
    inconsistent[1]["runtime"]["scenarios"]
        .as_object_mut()
        .unwrap()
        .remove("import");
    save(&inconsistent);
    assert!(
        super::devex::summarize(&run)
            .unwrap_err()
            .to_string()
            .contains("场景计数不一致")
    );
    samples[1]["runtime"]["scenarios"]["export"]["queue_p95_ms"] = json!(null);
    save(&samples);
    let error = super::devex::summarize(&run).unwrap_err().to_string();
    assert!(error.contains("部分缺失"), "{error}");
    fs::remove_dir_all(run).unwrap();
}

#[test]
fn runtime_variants_reject_undefined_concurrency_and_unmeasured_cold_server_state() {
    assert!(DevexSuite::RuntimeApi.definition("20").is_err());
    for suite in [
        DevexSuite::RuntimeApi,
        DevexSuite::RuntimeJobs,
        DevexSuite::RuntimeTenants,
    ] {
        for variant in ["10", "50", "100"] {
            assert!(suite.definition(variant).is_ok());
        }
        assert!(suite.validate_cache_state(CacheState::Cold).is_err());
        assert_eq!(suite.minimum_runs("10"), 6);
    }
    assert!(
        DevexSuite::RuntimeHomepage
            .validate_cache_state(CacheState::Cold)
            .is_ok()
    );
    assert!(
        DevexSuite::RuntimeHomepage
            .validate_cache_state(CacheState::Warm)
            .is_ok()
    );
}

#[test]
fn runtime_comparison_rejects_hidden_regression_missing_metrics_and_failures() {
    let stats = distribution(&[100.0, 100.0, 100.0, 100.0, 100.0]).unwrap();
    let mut base = summary(DevexSuite::RuntimeJobs, "10");
    let mut candidate = base.clone();
    let document = json!({ "input_sha256": "a".repeat(64), "network_requests": 100,
        "completed_cycles": 100, "failed_cycles": 0, "cycle_failure_rate": 0, "session_failures": 0,
        "scenarios": {"export": {"p50_ms": stats, "p95_ms": stats, "p99_ms": stats,
        "throughput": stats, "queue_p95_ms": stats, "execution_p95_ms": stats}},
        "peak_resident_memory_bytes": 1000, "cpu_seconds": stats,
        "peak_database_connections": 10, "collector_failures": 0 });
    base.runtime = Some(serde_json::from_value(document.clone()).unwrap());
    candidate.runtime = base.runtime.clone();
    assert!(
        comparison_checks(&base, &candidate, &stats, &stats)
            .unwrap()
            .iter()
            .all(|v| v.passed)
    );
    for (pointer, value) in [
        ("/scenarios/export/p95_ms/p95", json!(111.0)),
        ("/scenarios/export/queue_p95_ms/p95", json!(111.0)),
        ("/failed_cycles", json!(1)),
        ("/session_failures", json!(1)),
        ("/collector_failures", json!(1)),
    ] {
        let mut changed = document.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        candidate.runtime = Some(serde_json::from_value(changed).unwrap());
        assert!(
            comparison_checks(&base, &candidate, &stats, &stats)
                .unwrap()
                .iter()
                .any(|v| !v.passed)
        );
    }
    let mut changed = document;
    *changed
        .pointer_mut("/scenarios/export/queue_p95_ms")
        .unwrap() = json!(null);
    candidate.runtime = Some(serde_json::from_value(changed).unwrap());
    assert!(comparison_checks(&base, &candidate, &stats, &stats).is_err());
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
            "runtime-homepage",
            "runtime-api",
            "runtime-jobs",
            "runtime-tenants",
        ]
    );
    assert!(
        names
            .into_iter()
            .all(|name| DevexSuite::parse(name).is_some())
    );
    assert!(DevexSuite::parse("backend-check").is_none());
}
