use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    cli::{Command, parse},
    devex::{
        CacheState, DevexCommand, DevexSuite, PathNormalizer, compare, distribution,
        filter_environment, summarize,
    },
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
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
        "baseline",
        "--runs",
        "7",
        "--cache",
        "cold",
    ]))
    .unwrap();
    let Command::Devex(DevexCommand::Run(options)) = cli.command else {
        panic!("应解析为 DevEx run");
    };
    assert_eq!(options.suite, DevexSuite::RustColdBuild);
    assert_eq!(options.variant, "baseline");
    assert_eq!(options.runs, 7);
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
fn environment_snapshot_only_keeps_explicit_safe_names() {
    let environment = filter_environment([
        (OsString::from("CARGO_INCREMENTAL"), OsString::from("0")),
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
    let baseline = fake_run("baseline", "sha256:same", &[100.0, 200.0]);
    let candidate = fake_run("candidate", "sha256:same", &[80.0, 180.0]);
    let summary = summarize(&baseline).unwrap();
    assert_eq!(summary.samples, 2);
    assert_eq!(summary.duration_ms.unwrap().p95, 200.0);
    assert!(baseline.join("summary.md").is_file());
    assert!(
        compare(&baseline, &candidate)
            .unwrap()
            .contains("DevEx 对比")
    );

    let incompatible = fake_run("incompatible", "sha256:different", &[90.0]);
    let error = compare(&baseline, &incompatible).unwrap_err().to_string();
    assert!(error.contains("compile_surface_fingerprint"), "{error}");
    for path in [baseline, candidate, incompatible] {
        fs::remove_dir_all(path).unwrap();
    }
}

fn fake_run(name: &str, fingerprint: &str, durations: &[f64]) -> PathBuf {
    static NEXT_RUN: AtomicUsize = AtomicUsize::new(0);
    let id = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let directory =
        std::env::temp_dir().join(format!("ryframe-devex-{name}-{}-{id}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let metadata = serde_json::json!({
        "schema_version": 1,
        "run_id": name,
        "started_at": "2026-08-27T00:00:00Z",
        "suite": "rust-cold-build",
        "variant": name,
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
