use std::path::Path;

use super::{
    cli::{CheckCommand, Command, parse},
    devex::{BaselineContract, CacheState, DevexCommand, DevexSuite},
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn cli_requires_named_run_and_compare_arguments() {
    let cli = parse(strings(&[
        "check",
        "perf",
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
    let Command::Check(CheckCommand::Perf(DevexCommand::Run(options))) = cli.command else {
        panic!("应解析为 DevEx run");
    };
    assert_eq!(options.suite, DevexSuite::RustColdBuild);
    assert_eq!(options.variant, "workspace");
    assert_eq!(options.runs, 20);
    assert_eq!(options.cache_state, CacheState::Cold);

    assert!(parse(strings(&["check", "perf", "run", "rust-cold-build"])).is_err());
    assert!(
        parse(strings(&[
            "check",
            "perf",
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
    assert!(
        parse(strings(&[
            "check",
            "perf",
            "compare",
            "base/run",
            "candidate/run",
        ]))
        .is_err()
    );
    assert!(matches!(
        parse(strings(&[
            "check",
            "perf",
            "compare",
            "--base",
            "2026-08-01/base",
            "--candidate",
            "2026-08-02/candidate",
        ]))
        .unwrap()
        .command,
        Command::Check(CheckCommand::Perf(DevexCommand::Compare { .. }))
    ));
}

#[test]
fn cli_rejects_cache_states_that_change_suite_semantics() {
    for (suite, variant, runs, cache) in [
        ("rust-cold-build", "api", "20", "warm"),
        ("rust-incremental", "application", "6", "cold"),
        ("resource-gate", "auto", "6", "cold"),
        ("rust-gate", "default", "20", "cold"),
        ("rust-sccache", "workspace", "20", "cold"),
    ] {
        let error = parse(strings(&[
            "check",
            "perf",
            "run",
            "--suite",
            suite,
            "--variant",
            variant,
            "--runs",
            runs,
            "--cache",
            cache,
        ]))
        .unwrap_err()
        .to_string();
        assert!(error.contains("只允许 --cache"), "{suite}: {error}");
    }
}

#[test]
fn paired_cli_requires_two_explicit_backend_worktrees() {
    let cli = parse(strings(&[
        "check",
        "perf",
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
    let Command::Check(CheckCommand::Perf(DevexCommand::Paired(options))) = cli.command else {
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
        "check",
        "perf",
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
        "check",
        "perf",
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
        "check",
        "perf",
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
        "6",
        "--cache",
        "warm",
    ]))
    .unwrap();
    let Command::Check(CheckCommand::Perf(DevexCommand::Paired(options))) = cli.command else {
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
                "check",
                "perf",
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
fn stable_readiness_b0_contract_accepts_all_suites_and_binds_both_product_roots() {
    for (suite, variant, runs, cache) in [
        ("rust-cold-build", "api", "20", "cold"),
        ("resource-generator", "post", "6", "warm"),
        ("resource-gate", "auto", "6", "warm"),
        ("rust-gate", "default", "20", "warm"),
        ("frontend-fast", "default", "6", "warm"),
        ("runtime-api", "10", "6", "warm"),
    ] {
        let cli = parse(strings(&[
            "check",
            "perf",
            "paired",
            "--base-backend",
            "D:/worktrees/base",
            "--candidate-backend",
            "D:/worktrees/candidate",
            "--base-frontend",
            "D:/worktrees/base-frontend",
            "--candidate-frontend",
            "D:/worktrees/candidate-frontend",
            "--baseline-contract",
            "legacy-stable-readiness-b0-v1",
            "--suite",
            suite,
            "--variant",
            variant,
            "--runs",
            runs,
            "--cache",
            cache,
        ]))
        .unwrap();
        let Command::Check(CheckCommand::Perf(DevexCommand::Paired(options))) = cli.command else {
            panic!("应解析为 DevEx paired");
        };
        assert_eq!(
            options.baseline_contract,
            Some(BaselineContract::LegacyStableReadinessB0V1)
        );
    }

    let error = parse(strings(&[
        "check",
        "perf",
        "paired",
        "--base-backend",
        "D:/worktrees/base",
        "--candidate-backend",
        "D:/worktrees/candidate",
        "--baseline-contract",
        "legacy-stable-readiness-b0-v1",
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
    assert!(error.contains("两个前端 worktree"), "{error}");

    let cli = parse(strings(&[
        "check",
        "perf",
        "paired",
        "--base-backend",
        "D:/性能 基线/后端",
        "--candidate-backend",
        "D:/性能 候选/后端",
        "--base-frontend",
        "D:/性能 基线/前端",
        "--candidate-frontend",
        "D:/性能 候选/前端",
        "--baseline-contract",
        "legacy-stable-readiness-b0-v1",
        "--suite",
        "resource-generator",
        "--variant",
        "post",
        "--runs",
        "6",
        "--cache",
        "warm",
    ]))
    .unwrap();
    let Command::Check(CheckCommand::Perf(DevexCommand::Paired(options))) = cli.command else {
        panic!("应解析为 DevEx paired");
    };
    assert_eq!(options.baseline_backend, Path::new("D:/性能 基线/后端"));
    assert_eq!(
        options.candidate_frontend.as_deref(),
        Some(Path::new("D:/性能 候选/前端"))
    );
}

#[test]
fn cli_rejects_under_sampled_and_unknown_variants() {
    let under_sampled = parse(strings(&[
        "check",
        "perf",
        "run",
        "--suite",
        "frontend-fast",
        "--variant",
        "default",
        "--runs",
        "5",
        "--cache",
        "warm",
    ]));
    assert!(
        under_sampled
            .unwrap_err()
            .to_string()
            .contains("至少需要 6 次")
    );

    let unknown = parse(strings(&[
        "check",
        "perf",
        "run",
        "--suite",
        "cargo-dev-save",
        "--variant",
        "baseline",
        "--runs",
        "6",
        "--cache",
        "warm",
    ]));
    assert!(unknown.unwrap_err().to_string().contains("变体"));
}
