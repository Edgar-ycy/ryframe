use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::devex::{DevexSuite, PairedArm, compare, summarize};

#[test]
fn real_sccache_017_schema_is_accepted() {
    let run = fake_run("sccache-real-schema", &[100.0]);
    let snapshot = br#"{
      "stats": {
        "compile_requests": 0,
        "requests_unsupported_compiler": 0,
        "requests_not_compile": 0,
        "requests_not_cacheable": 0,
        "not_cached": {},
        "requests_executed": 0,
        "cache_errors": {"counts": {}, "adv_counts": {}},
        "cache_hits": {"counts": {}, "adv_counts": {}},
        "cache_misses": {"counts": {}, "adv_counts": {}},
        "cache_timeouts": 0,
        "cache_read_errors": 0,
        "non_cacheable_compilations": 0,
        "forced_recaches": 0,
        "cache_write_errors": 0,
        "cache_writes": 0,
        "dist_errors": 0,
        "multi_level": null
      },
      "cache_location": "Local disk",
      "version": "0.17.0",
      "basedirs": []
    }"#;
    fs::write(run.join("sccache-before.json"), snapshot).unwrap();
    fs::write(run.join("sccache-after.json"), snapshot).unwrap();

    let summary = summarize(&run).unwrap();
    assert_eq!(summary.sccache.unwrap().compile_requests, 0);
    fs::remove_dir_all(run).unwrap();
}

#[test]
fn sccache_summary_reports_delta_hit_rate_and_errors() {
    let run = fake_run("sccache", &[100.0]);
    write_stats(&run.join("sccache-before.json"), 10, 4, 2, 1, 0);
    write_stats(&run.join("sccache-after.json"), 20, 12, 4, 2, 1);

    let summary = summarize(&run).unwrap();
    assert_eq!(summary.sccache_version.as_deref(), Some("sccache 0.17.0"));
    let stats = summary.sccache.unwrap();
    assert_eq!(stats.compile_requests, 10);
    assert_eq!(stats.cache_hits, 8);
    assert_eq!(stats.cache_misses, 2);
    assert_eq!(stats.not_cacheable, 1);
    assert_eq!(stats.not_cacheable_control_probes, 0);
    assert_eq!(stats.not_cacheable_compilations, 1);
    assert_eq!(stats.cache_errors, 1);
    assert_eq!(stats.hit_rate, Some(0.8));
    fs::remove_dir_all(run).unwrap();
}

#[test]
fn sccache_summary_accepts_new_categories_and_rejects_disappearing_categories() {
    let added = fake_run("sccache-added-category", &[100.0]);
    write_stats(&added.join("sccache-before.json"), 10, 4, 2, 1, 0);
    let added_after = added.join("sccache-after.json");
    write_stats(&added_after, 20, 12, 4, 2, 0);
    mutate_counts(&added_after, "cache_hits", |counts| {
        counts.insert("C/C++".to_owned(), serde_json::json!(3));
    });
    assert_eq!(summarize(&added).unwrap().sccache.unwrap().cache_hits, 11);

    let missing = fake_run("sccache-missing-category", &[100.0]);
    write_stats(&missing.join("sccache-before.json"), 10, 4, 2, 1, 0);
    let missing_after = missing.join("sccache-after.json");
    write_stats(&missing_after, 20, 12, 4, 2, 0);
    mutate_counts(&missing_after, "cache_hits", |counts| {
        counts.remove("Rust");
    });
    let error = summarize(&missing).unwrap_err().to_string();
    assert!(
        error.contains("cache_hits.counts.Rust") && error.contains("回退"),
        "{error}"
    );

    fs::remove_dir_all(added).unwrap();
    fs::remove_dir_all(missing).unwrap();
}

#[test]
fn sccache_summary_rejects_missing_or_mistyped_required_counters() {
    for field in [
        "compile_requests",
        "requests_not_cacheable",
        "not_cached",
        "cache_hits",
        "cache_misses",
        "cache_errors",
        "cache_timeouts",
        "cache_read_errors",
        "cache_write_errors",
        "dist_errors",
    ] {
        let run = fake_run(&format!("missing-{field}"), &[100.0]);
        write_stats(&run.join("sccache-before.json"), 10, 4, 2, 1, 0);
        let after_path = run.join("sccache-after.json");
        write_stats(&after_path, 20, 12, 4, 2, 0);
        let mut after: serde_json::Value =
            serde_json::from_slice(&fs::read(&after_path).unwrap()).unwrap();
        after["stats"].as_object_mut().unwrap().remove(field);
        fs::write(&after_path, serde_json::to_vec(&after).unwrap()).unwrap();
        let error = summarize(&run).unwrap_err().to_string();
        assert!(error.contains(field), "{error}");
        fs::remove_dir_all(run).unwrap();
    }

    let mistyped = fake_run("mistyped-counter", &[100.0]);
    write_stats(&mistyped.join("sccache-before.json"), 10, 4, 2, 1, 0);
    let after_path = mistyped.join("sccache-after.json");
    write_stats(&after_path, 20, 12, 4, 2, 0);
    let mut after: serde_json::Value =
        serde_json::from_slice(&fs::read(&after_path).unwrap()).unwrap();
    after["stats"]["cache_timeouts"] = serde_json::json!("0");
    fs::write(&after_path, serde_json::to_vec(&after).unwrap()).unwrap();
    let error = summarize(&mistyped).unwrap_err().to_string();
    assert!(error.contains("invalid type"), "{error}");
    fs::remove_dir_all(mistyped).unwrap();

    let no_snapshots = fake_run("missing-snapshots", &[100.0]);
    let error = summarize(&no_snapshots).unwrap_err().to_string();
    assert!(error.contains("缺少统计快照"), "{error}");
    fs::remove_dir_all(no_snapshots).unwrap();
}

#[test]
fn sccache_summary_excludes_cargo_control_probes_from_compilations() {
    let run = fake_run("sccache-control-probes", &[100.0]);
    write_stats(&run.join("sccache-before.json"), 0, 0, 0, 0, 0);
    let after = run.join("sccache-after.json");
    write_stats(&after, 20, 8, 2, 10, 0);
    set_not_cached_reasons(&after, [("crate-type", 4), ("-", 3), ("missing input", 3)]);

    let stats = summarize(&run).unwrap().sccache.unwrap();
    assert_eq!(stats.not_cacheable, 10);
    assert_eq!(stats.not_cacheable_control_probes, 6);
    assert_eq!(stats.not_cacheable_compilations, 4);
    fs::remove_dir_all(run).unwrap();
}

#[test]
fn sccache_summary_rejects_scalar_and_category_counter_rollbacks() {
    let scalar = fake_run("sccache-scalar-rollback", &[100.0]);
    write_stats(&scalar.join("sccache-before.json"), 10, 4, 2, 1, 0);
    write_stats(&scalar.join("sccache-after.json"), 9, 4, 2, 1, 0);
    let error = summarize(&scalar).unwrap_err().to_string();
    assert!(
        error.contains("compile_requests") && error.contains("回退"),
        "{error}"
    );

    let category = fake_run("sccache-category-rollback", &[100.0]);
    write_stats(&category.join("sccache-before.json"), 10, 4, 2, 1, 0);
    write_stats(&category.join("sccache-after.json"), 20, 3, 4, 2, 0);
    let error = summarize(&category).unwrap_err().to_string();
    assert!(
        error.contains("cache_hits.counts.Rust") && error.contains("回退"),
        "{error}"
    );

    fs::remove_dir_all(scalar).unwrap();
    fs::remove_dir_all(category).unwrap();
}

#[test]
fn warm_sccache_comparison_accepts_exact_thresholds_and_zero_baseline() {
    let exact = compare_pair(
        "sccache-exact",
        DevexSuite::RustSccache,
        &[100.0],
        &[90.0],
        FakeStats::new(100, 50, 50, 20, 0),
        FakeStats::new(100, 80, 20, 10, 0),
    )
    .unwrap();
    assert!(exact.contains("验收判定") && exact.contains("总判定：通过"));
    assert!(exact.contains("80.0%") && exact.contains("改善 50.0%"));

    let zero = compare_pair(
        "sccache-zero",
        DevexSuite::RustSccache,
        &[100.0],
        &[90.0],
        FakeStats::new(100, 50, 50, 0, 0),
        FakeStats::new(100, 80, 20, 0, 0),
    )
    .unwrap();
    assert!(zero.contains("基线 0 / 候选 0"));
}

#[test]
fn warm_sccache_comparison_rejects_invalid_evidence_and_regressions() {
    let valid_base = FakeStats::new(100, 50, 50, 20, 0);
    let valid_candidate = FakeStats::new(100, 80, 20, 10, 0);
    let cases = [
        (
            "base-requests",
            FakeStats::new(0, 50, 50, 20, 0),
            valid_candidate,
            "基线编译请求",
        ),
        (
            "candidate-requests",
            valid_base,
            FakeStats::new(0, 80, 20, 10, 0),
            "候选编译请求",
        ),
        (
            "base-cacheable",
            FakeStats::new(100, 0, 0, 20, 0),
            valid_candidate,
            "基线可缓存请求",
        ),
        (
            "candidate-cacheable",
            valid_base,
            FakeStats::new(100, 0, 0, 10, 0),
            "候选可缓存请求",
        ),
        (
            "base-errors",
            FakeStats::new(100, 50, 50, 20, 1),
            valid_candidate,
            "基线缓存错误",
        ),
        (
            "candidate-errors",
            valid_base,
            FakeStats::new(100, 80, 20, 10, 1),
            "候选缓存错误",
        ),
        (
            "hit-rate",
            valid_base,
            FakeStats::new(100, 79, 21, 10, 0),
            "候选暖缓存命中率",
        ),
        (
            "not-cacheable",
            valid_base,
            FakeStats::new(100, 80, 20, 11, 0),
            "实际不可缓存编译改善",
        ),
        (
            "zero-base-regression",
            FakeStats::new(100, 50, 50, 0, 0),
            FakeStats::new(100, 80, 20, 1, 0),
            "实际不可缓存编译改善",
        ),
    ];
    for (name, baseline, candidate, expected) in cases {
        let error = compare_pair(
            name,
            DevexSuite::RustSccache,
            &[100.0],
            &[90.0],
            baseline,
            candidate,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("总判定：失败") && error.contains(expected),
            "{error}"
        );
    }
}

#[test]
fn warm_rust_gate_comparison_enforces_duration_thresholds() {
    let baseline_stats = FakeStats::new(100, 50, 50, 20, 0);
    let candidate_stats = FakeStats::new(100, 80, 20, 10, 0);
    let baseline = [100.0, 100.0, 200.0, 200.0, 200.0];
    let exact_candidate = [180.0, 180.0, 180.0, 210.0, 210.0];
    let exact = compare_pair(
        "rust-gate-exact",
        DevexSuite::RustGate,
        &baseline,
        &exact_candidate,
        baseline_stats,
        candidate_stats,
    )
    .unwrap();
    assert!(exact.contains("rust-gate P50 改善") && exact.contains("rust-gate P95 回退"));

    for (name, durations, expected) in [
        (
            "rust-gate-p50",
            [180.1, 180.1, 180.1, 210.0, 210.0],
            "rust-gate P50 改善",
        ),
        (
            "rust-gate-p95",
            [180.0, 180.0, 180.0, 210.1, 210.1],
            "rust-gate P95 回退",
        ),
    ] {
        let error = compare_pair(
            name,
            DevexSuite::RustGate,
            &baseline,
            &durations,
            baseline_stats,
            candidate_stats,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn compare_rejects_different_sccache_executables() {
    let baseline = fake_paired_run(
        "sccache-version-base",
        DevexSuite::RustSccache,
        &[100.0],
        "comparison-version",
        PairedArm::Baseline,
    );
    let candidate = fake_paired_run(
        "sccache-version-candidate",
        DevexSuite::RustSccache,
        &[90.0],
        "comparison-version",
        PairedArm::Candidate,
    );
    write_stats(&baseline.join("sccache-before.json"), 0, 0, 0, 0, 0);
    write_stats(&baseline.join("sccache-after.json"), 100, 50, 50, 20, 0);
    write_stats(&candidate.join("sccache-before.json"), 0, 0, 0, 0, 0);
    write_stats(&candidate.join("sccache-after.json"), 100, 80, 20, 10, 0);
    set_sccache_version(&baseline, "sccache 0.15.0");
    set_sccache_version(&candidate, "sccache 0.17.0");

    let error = compare(&baseline, &candidate).unwrap_err().to_string();
    assert!(error.contains("sccache 可执行版本"), "{error}");
    fs::remove_dir_all(baseline).unwrap();
    fs::remove_dir_all(candidate).unwrap();
}

#[derive(Clone, Copy)]
struct FakeStats {
    requests: u64,
    hits: u64,
    misses: u64,
    not_cacheable: u64,
    errors: u64,
}

impl FakeStats {
    const fn new(requests: u64, hits: u64, misses: u64, not_cacheable: u64, errors: u64) -> Self {
        Self {
            requests,
            hits,
            misses,
            not_cacheable,
            errors,
        }
    }
}

fn compare_pair(
    name: &str,
    suite: DevexSuite,
    baseline_durations: &[f64],
    candidate_durations: &[f64],
    baseline_stats: FakeStats,
    candidate_stats: FakeStats,
) -> crate::Result<String> {
    let comparison_id = format!("comparison-{name}");
    let baseline = fake_stats_run(
        &format!("{name}-base"),
        suite,
        baseline_durations,
        &comparison_id,
        PairedArm::Baseline,
        baseline_stats,
    );
    let candidate = fake_stats_run(
        &format!("{name}-candidate"),
        suite,
        candidate_durations,
        &comparison_id,
        PairedArm::Candidate,
        candidate_stats,
    );
    let result = compare(&baseline, &candidate);
    fs::remove_dir_all(baseline).unwrap();
    fs::remove_dir_all(candidate).unwrap();
    result
}

fn fake_stats_run(
    name: &str,
    suite: DevexSuite,
    durations: &[f64],
    comparison_id: &str,
    arm: PairedArm,
    stats: FakeStats,
) -> PathBuf {
    let directory = fake_paired_run(name, suite, durations, comparison_id, arm);
    write_stats(&directory.join("sccache-before.json"), 0, 0, 0, 0, 0);
    write_stats(
        &directory.join("sccache-after.json"),
        stats.requests,
        stats.hits,
        stats.misses,
        stats.not_cacheable,
        stats.errors,
    );
    directory
}

fn write_stats(
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
            "not_cached": { "crate-type": not_cacheable },
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

fn set_not_cached_reasons<const N: usize>(path: &Path, reasons: [(&str, u64); N]) {
    let mut document: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let reasons = reasons
        .into_iter()
        .map(|(reason, count)| (reason.to_owned(), serde_json::Value::from(count)));
    document["stats"]["not_cached"] = serde_json::Value::Object(reasons.collect());
    fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
}

fn mutate_counts(
    path: &Path,
    group: &str,
    mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) {
    let mut document: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    mutate(document["stats"][group]["counts"].as_object_mut().unwrap());
    fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
}

fn set_sccache_version(directory: &Path, version: &str) {
    let metadata_path = directory.join("metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["toolchain"]["sccache"] = serde_json::json!(version);
    fs::write(metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
}

fn fake_run(name: &str, durations: &[f64]) -> PathBuf {
    let directory = temporary_directory(name);
    let metadata = serde_json::json!({
        "schema_version": 1,
        "run_id": name,
        "started_at": "2026-08-27T00:00:00Z",
        "suite": "rust-sccache",
        "variant": "api",
        "cache_state": "warm",
        "requested_runs": durations.len(),
        "backend": { "commit": null, "dirty": false, "worktree_fingerprint": "sha256:source" },
        "frontend": null,
        "toolchain": {
            "cargo": "cargo",
            "rustc": "rustc",
            "sccache": "sccache 0.17.0",
            "node": null,
            "pnpm": null
        },
        "target": "x86_64-pc-windows-msvc",
        "features": ["bin-api"],
        "jobs": 4,
        "environment": {},
        "environment_hash": "sha256:environment",
        "commands": [],
        "compile_surface_fingerprint": "sha256:sccache",
        "input_fingerprint": format!("sha256:{name}"),
    });
    fs::write(
        directory.join("metadata.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    write_samples(&directory, name, durations, None, "sha256:source", None);
    directory
}

fn fake_paired_run(
    name: &str,
    suite: DevexSuite,
    durations: &[f64],
    comparison_id: &str,
    arm: PairedArm,
) -> PathBuf {
    let directory = temporary_directory(name);
    let backend_fingerprint = format!("sha256:source-{}", arm.as_str());
    let frontend_fingerprint =
        (suite == DevexSuite::RustGate).then(|| format!("sha256:frontend-{}", arm.as_str()));
    let frontend = frontend_fingerprint.as_ref().map(|fingerprint| {
        serde_json::json!({
            "commit": null,
            "dirty": false,
            "worktree_fingerprint": fingerprint,
        })
    });
    let metadata = serde_json::json!({
        "schema_version": 1,
        "run_id": name,
        "started_at": "2026-08-27T00:00:00Z",
        "suite": suite.as_str(),
        "variant": if suite == DevexSuite::RustGate { "default" } else { "api" },
        "cache_state": "warm",
        "requested_runs": durations.len(),
        "backend": {
            "commit": null,
            "dirty": false,
            "worktree_fingerprint": backend_fingerprint,
        },
        "frontend": frontend,
        "toolchain": {
            "cargo": "cargo",
            "rustc": "rustc",
            "sccache": "sccache 0.17.0",
            "node": null,
            "pnpm": null
        },
        "target": "x86_64-pc-windows-msvc",
        "features": ["bin-api"],
        "jobs": 4,
        "environment": {},
        "environment_hash": "sha256:environment",
        "commands": [],
        "compile_surface_fingerprint": "sha256:sccache",
        "input_fingerprint": format!("sha256:{name}"),
        "pairing": { "comparison_id": comparison_id, "arm": arm.as_str() },
    });
    fs::write(
        directory.join("metadata.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    write_samples(
        &directory,
        name,
        durations,
        Some(arm),
        &backend_fingerprint,
        frontend_fingerprint.as_deref(),
    );
    directory
}

fn write_samples(
    directory: &Path,
    name: &str,
    durations: &[f64],
    arm: Option<PairedArm>,
    backend_fingerprint: &str,
    frontend_fingerprint: Option<&str>,
) {
    let samples = durations
        .iter()
        .enumerate()
        .map(|(index, duration)| {
            let pair = index + 1;
            let order = arm.map(|arm| match (pair % 2, arm) {
                (1, PairedArm::Baseline) | (0, PairedArm::Candidate) => pair * 2 - 1,
                _ => pair * 2,
            });
            serde_json::json!({
                "schema_version": 1,
                "run_id": name,
                "sequence": pair,
                "kind": "measurement",
                "cache_state": "warm",
                "started_at": format!("2026-08-27T00:00:{:02}Z", order.unwrap_or(pair)),
                "duration_ms": duration,
                "status": "passed",
                "exit_code": 0,
                "target_directory": "$DEVEX/cache/sccache-measure",
                "arm": arm.map(PairedArm::as_str),
                "pair": arm.map(|_| pair),
                "order": order,
                "source_fingerprints": arm.map(|_| serde_json::json!({
                    "backend": backend_fingerprint,
                    "frontend": frontend_fingerprint,
                })),
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(directory.join("samples.jsonl"), format!("{samples}\n")).unwrap();
}

fn temporary_directory(name: &str) -> PathBuf {
    static NEXT_RUN: AtomicUsize = AtomicUsize::new(0);
    let id = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "ryframe-sccache-{name}-{}-{id}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    directory
}
