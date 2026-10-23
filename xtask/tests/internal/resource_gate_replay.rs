use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::json;

use super::ci::resource_gate::replay::{
    CommandResult, MINIMUM_CASES, ReplayCase, ReplayEvidence, ReplayManifest, ReplayResult,
    ReplayRun, ResourceGateDecision, TARGETED_P95_LIMIT_MS, arm_order,
    cleanup_worktree_with_attempts, command_for_frontend, command_for_replay,
    controlled_environment, fail_on_cleanup, load_decision, load_decision_after_command,
    percentile_nearest_rank, recorded_environment, remove_partial_worktree_directory,
    replay_activation_eligible, replay_mismatches, repository_identity, require_arm,
    sccache_error_count, select_prime_cases_with, switch_worktree_with,
    validate_activation_commands, validate_case_coverage, validate_repository_pair,
    validated_report_path, verify_case_ranges,
};

const CATEGORIES: [&str; 7] = [
    "addition",
    "field",
    "permission",
    "relation",
    "sql",
    "rename",
    "delete",
];

#[test]
fn manifest_requires_twenty_unique_complete_cases() {
    let target = tempfile::tempdir().unwrap();
    let short = target.path().join("short.json");
    fs::write(
        &short,
        serde_json::to_vec(&json!({
            "formatVersion": 2,
            "targetedCommand": ["targeted"],
            "fullCommand": ["full"],
            "cases": []
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        super::ci::resource_gate::replay::load_manifest(&short)
            .unwrap_err()
            .contains("至少需要 20")
    );

    let cases = valid_cases();
    validate_case_coverage(&cases).unwrap();
    let mut duplicate = cases.clone();
    duplicate[19].base = duplicate[0].base.clone();
    duplicate[19].head = duplicate[0].head.clone();
    assert!(
        validate_case_coverage(&duplicate)
            .unwrap_err()
            .contains("范围必须唯一")
    );
    let all_pass = cases
        .iter()
        .cloned()
        .map(|mut case| {
            case.expected = "pass".to_owned();
            case
        })
        .collect::<Vec<_>>();
    assert!(
        validate_case_coverage(&all_pass)
            .unwrap_err()
            .contains("同时覆盖")
    );
}

#[test]
fn activation_gate_requires_one_stable_resource_gate_command() {
    let base = ReplayManifest {
        targeted_command: vec![
            "cargo".to_owned(),
            "xtask".to_owned(),
            "check".to_owned(),
            "ci".to_owned(),
            "resource-gate".to_owned(),
        ],
        full_command: vec![
            "cargo".to_owned(),
            "xtask".to_owned(),
            "check".to_owned(),
            "ci".to_owned(),
            "resource-gate".to_owned(),
        ],
        cases: Vec::new(),
    };
    validate_activation_commands(&base).unwrap();
    assert!(
        validate_activation_commands(&ReplayManifest {
            full_command: vec!["cargo".to_owned(), "test".to_owned()],
            ..base.clone()
        })
        .unwrap_err()
        .contains("同一资源门禁")
    );
    assert!(
        validate_activation_commands(&ReplayManifest {
            targeted_command: [base.targeted_command, vec!["--help".to_owned()]].concat(),
            full_command: [base.full_command, vec!["--help".to_owned()]].concat(),
            cases: Vec::new(),
        })
        .unwrap_err()
        .contains("只允许可选")
    );
}

#[test]
fn fail_closed_arms_and_prime_selection_preserve_coverage() {
    let cases = valid_cases();
    let targeted = command_result(true, 1, true, "targeted");
    let mut full_decision = command_result(true, 1, true, "full");
    full_decision.decision.fallback = None;
    assert!(
        require_arm(&cases[0], &full_decision, true, false)
            .unwrap_err()
            .contains("full")
            || require_arm(&cases[0], &full_decision, true, false)
                .unwrap_err()
                .contains("targeted mode")
    );
    let mut recognized_full = command_result(true, 1, false, "full");
    recognized_full.decision.recognized = true;
    assert!(
        require_arm(&cases[0], &recognized_full, false, false)
            .unwrap_err()
            .contains("full arm")
    );
    let mut failed_prime = command_result(false, 1, true, "targeted");
    failed_prime.failure_tail = Some("精确失败原因".to_owned());
    assert!(
        require_arm(&cases[0], &failed_prime, true, true)
            .unwrap_err()
            .contains("精确失败原因")
    );
    assert!(require_arm(&cases[0], &targeted, true, false).is_ok());

    let selected = select_prime_cases_with(
        &cases,
        |case| {
            Ok(if case == &cases[2] {
                vec!["ryframe-application".to_owned(), "ryframe-api".to_owned()]
            } else {
                vec!["ryframe-api".to_owned()]
            })
        },
        |case| Ok(if case == &cases[2] { 9 } else { 2 }),
    )
    .unwrap();
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].case, cases[2]);
    assert_eq!(selected[0].change_count, 9);
    assert_eq!(
        selected[0].rust_crates,
        ["ryframe-api", "ryframe-application"]
    );
    assert!(
        select_prime_cases_with(&cases, |_| Ok(Vec::new()), |_| Ok(2))
            .unwrap()
            .len()
            == 1
    );
}

#[test]
fn replay_report_metrics_are_computed_from_targeted_successes() {
    let within = (0..10)
        .map(|index| replay_result(index, 10_000 + index as u64))
        .collect::<Vec<_>>();
    let over = within
        .iter()
        .cloned()
        .chain(std::iter::once(replay_result(10, 60_001)))
        .collect::<Vec<_>>();
    assert_eq!(
        percentile_nearest_rank(
            &within
                .iter()
                .map(|result| result.targeted.duration_ms)
                .collect::<Vec<_>>(),
            95
        ),
        Some(10_009)
    );
    assert!(
        percentile_nearest_rank(
            &over
                .iter()
                .map(|result| result.targeted.duration_ms)
                .collect::<Vec<_>>(),
            95
        )
        .unwrap()
            > TARGETED_P95_LIMIT_MS
    );
    assert_eq!(percentile_nearest_rank(&[3, 1, 2], 95), Some(3));
    assert!(replay_activation_eligible(true, true, true, true, true));
    assert!(!replay_activation_eligible(true, false, true, true, true));
    assert!(!replay_activation_eligible(true, true, true, true, false));
    let mismatch = replay_result(0, 1);
    let mut mismatch = mismatch;
    mismatch.matches = false;
    assert_eq!(replay_mismatches(&[mismatch]), ["case-0"]);
}

#[test]
fn mismatch_decisions_preserve_exit_code_one() {
    let mut result = replay_result(0, 1);
    result.matches = false;
    let run = ReplayRun {
        results: vec![result],
        activation: None,
        evidence: ReplayEvidence {
            integration_files: json!({}),
            manifest_fingerprint: "sha256:manifest".to_owned(),
            runner_fingerprint: "sha256:runner".to_owned(),
            runner_commit: "a".repeat(40),
            environment: BTreeMap::new(),
            environment_fingerprint: "sha256:environment".to_owned(),
            tools: BTreeMap::new(),
            tools_fingerprint: "sha256:tools".to_owned(),
            repositories: BTreeMap::new(),
            primes: Vec::new(),
            sccache_before: json!({}),
            sccache_after: json!({}),
        },
    };
    let error = super::ci::resource_gate::replay::finish(&run, false).unwrap_err();
    assert_eq!(super::process::failure_exit_code(error.as_ref()), Some(1));
    assert!(error.to_string().contains("存在分歧"));
}

#[test]
fn report_is_create_only_inside_the_backend_evidence_root() {
    let target = tempfile::tempdir().unwrap();
    let repository = target.path().join("repository");
    fs::create_dir(&repository).unwrap();
    let report = validated_report_path(
        &repository,
        Path::new(".local-tests/resource-gate-replay/report.json"),
    )
    .unwrap();
    assert!(report.strip_prefix(&repository).is_ok());
    assert!(
        validated_report_path(&repository, &target.path().join("outside.json"))
            .unwrap_err()
            .contains("子路径")
    );
    fs::create_dir_all(report.parent().unwrap()).unwrap();
    fs::write(&report, "existing").unwrap();
    assert!(
        validated_report_path(&repository, &report)
            .unwrap_err()
            .contains("尚不存在")
    );
}

#[test]
fn decision_schema_and_log_tail_fail_closed() {
    let target = tempfile::tempdir().unwrap();
    let decision_path = target.path().join("decision.json");
    fs::write(
        &decision_path,
        serde_json::to_vec(&json!({
            "formatVersion": 1,
            "recognized": true,
            "mode": "targeted",
            "fallback": null,
            "steps": ["resource-drift"]
        }))
        .unwrap(),
    )
    .unwrap();
    let decision = load_decision(&decision_path).unwrap();
    assert!(decision.recognized);
    assert_eq!(decision.mode, "targeted");

    let log = target.path().join("command.log");
    fs::write(&log, "前置编译精确失败").unwrap();
    assert!(
        load_decision_after_command(&target.path().join("missing.json"), &log, 101)
            .unwrap_err()
            .contains("前置编译精确失败")
    );
    fs::write(
        &decision_path,
        serde_json::to_vec(&json!({
            "formatVersion": 1,
            "recognized": true,
            "mode": "full",
            "fallback": null,
            "steps": ["full-rust-gate"]
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        load_decision(&decision_path)
            .unwrap_err()
            .contains("fallback")
    );
}

#[test]
fn command_normalization_keeps_frontend_absolute_and_expands_xtask() {
    let frontend = Path::new("D:/前端 worktree");
    let command = vec![
        "cargo".to_owned(),
        "xtask".to_owned(),
        "check".to_owned(),
        "ci".to_owned(),
        "resource-gate".to_owned(),
        "--frontend-dir".to_owned(),
        "../frontend".to_owned(),
    ];
    let normalized = command_for_frontend(&command, frontend).unwrap();
    assert_eq!(normalized.last().unwrap(), &frontend.to_string_lossy());
    let runner = Path::new("D:/runner/Cargo.toml");
    let target = Path::new("D:/runner-target");
    let expanded = command_for_replay(
        &command,
        frontend,
        target,
        runner,
        &crate::cli::ResourceGateOptions {
            base_sha: Some("1111111111111111111111111111111111111111".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        expanded.iter().map(String::as_str).collect::<Vec<_>>(),
        vec![
            "cargo",
            "run",
            "--locked",
            "--manifest-path",
            "D:/runner/Cargo.toml",
            "--target-dir",
            "D:/runner-target",
            "-p",
            "xtask",
            "--",
            "check",
            "ci",
            "resource-gate",
            "--frontend-dir",
            "D:/前端 worktree",
            "--base-sha",
            "1111111111111111111111111111111111111111",
        ]
    );
    assert_eq!(expanded[8], "xtask");
    assert_eq!(arm_order(1), (true, false));
    assert_eq!(arm_order(2), (false, true));
}

#[test]
fn command_expansion_preserves_option_and_unicode_path_argument_boundaries() {
    let frontend = Path::new("D:/前端 目录/中文 worktree");
    let runner = Path::new("D:/runner 目录/Cargo.toml");
    let target = Path::new("D:/独立 runner target");
    let command = [
        "cargo",
        "xtask",
        "check",
        "--scope",
        "backend",
        "--frontend-dir",
        "../旧 前端",
        "--plan",
    ]
    .map(str::to_owned)
    .to_vec();
    let normalized = command_for_frontend(&command, frontend).unwrap();
    assert_eq!(normalized[6], frontend.to_string_lossy());
    let expanded = command_for_replay(
        &command,
        frontend,
        target,
        runner,
        &crate::cli::ResourceGateOptions {
            base_sha: Some("1111111111111111111111111111111111111111".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        expanded.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "cargo",
            "run",
            "--locked",
            "--manifest-path",
            "D:/runner 目录/Cargo.toml",
            "--target-dir",
            "D:/独立 runner target",
            "-p",
            "xtask",
            "--",
            "check",
            "--scope",
            "backend",
            "--frontend-dir",
            "D:/前端 目录/中文 worktree",
            "--plan",
        ]
    );
}

#[test]
fn controlled_environment_removes_stale_compiler_and_gate_variables() {
    let mut environment = BTreeMap::from([
        ("RYFRAME_CI_TEST_JOBS".to_owned(), "64".into()),
        ("RUSTFLAGS".to_owned(), "-Ctarget-cpu=native".into()),
        ("CARGO_INCREMENTAL".to_owned(), "1".into()),
        ("SCCACHE_GHA_ENABLED".to_owned(), "true".into()),
        (
            "CMAKE_CXX_COMPILER_LAUNCHER".to_owned(),
            "stale-launcher".into(),
        ),
    ]);
    for name in [
        "RYFRAME_CI_HEAD_SHA",
        "RYFRAME_CI_FRONTEND_REF",
        "RYFRAME_RESOURCE_GATE_TARGETED",
        "RYFRAME_RESOURCE_GATE_DECISION_FILE",
        "RYFRAME_RESOURCE_GATE_TEST_JOBS",
        "RYFRAME_VERIFY_JOBS",
        "RYFRAME_MYSQL_INTEGRATION",
        "RYFRAME_MYSQL_TLS_INTEGRATION",
        "RYFRAME_MYSQL_HOST",
        "RYFRAME_REDIS_INTEGRATION",
        "RYFRAME_REDIS_HOST",
        "RYFRAME_REDIS_DATABASE",
    ] {
        environment.insert(name.to_owned(), "stale-value".into());
    }
    let cache = Path::new("D:/cache");
    let controlled = controlled_environment(
        std::mem::take(&mut environment),
        Path::new("sccache"),
        cache,
    )
    .unwrap();
    assert!(!controlled.contains_key("RYFRAME_CI_RUST_GATE_PROFILE"));
    assert!(!controlled.contains_key("RYFRAME_CI_TEST_JOBS"));
    assert_eq!(controlled.get("CARGO_INCREMENTAL").unwrap(), "0");
    assert!(!controlled.contains_key("RYFRAME_VERIFY_JOBS"));
    assert!(!controlled.contains_key("RYFRAME_RESOURCE_GATE_TEST_JOBS"));
    for name in [
        "RYFRAME_CI_HEAD_SHA",
        "RYFRAME_CI_FRONTEND_REF",
        "RYFRAME_RESOURCE_GATE_TARGETED",
        "RYFRAME_RESOURCE_GATE_DECISION_FILE",
        "RYFRAME_RESOURCE_GATE_TEST_JOBS",
        "RYFRAME_VERIFY_JOBS",
        "RYFRAME_MYSQL_INTEGRATION",
        "RYFRAME_MYSQL_TLS_INTEGRATION",
        "RYFRAME_MYSQL_HOST",
        "RYFRAME_REDIS_INTEGRATION",
        "RYFRAME_REDIS_HOST",
        "RYFRAME_REDIS_DATABASE",
    ] {
        assert!(!controlled.contains_key(name));
        assert!(
            !recorded_environment(&controlled, false)
                .unwrap()
                .contains_key(name)
        );
    }
    assert!(!controlled.contains_key("RUSTFLAGS"));
    assert!(!controlled.contains_key("SCCACHE_GHA_ENABLED"));
    assert!(!controlled.contains_key("CMAKE_CXX_COMPILER_LAUNCHER"));
    let stats = json!({
        "stats": {
            "cache_errors": {"counts": {"Disk": 2}},
            "cache_timeouts": 1,
            "cache_read_errors": 0,
            "cache_write_errors": 3,
            "dist_errors": 0
        }
    });
    assert_eq!(sccache_error_count(&stats).unwrap(), 6);
}

#[test]
fn cleanup_retries_and_scope_checks_preserve_failure_evidence() {
    let target = tempfile::tempdir().unwrap();
    let allowed = target.path().join("allowed");
    let worktree = allowed.join("case").join("f");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(worktree.join("artifact.txt"), "locked then released").unwrap();
    let calls = AtomicUsize::new(0);
    cleanup_worktree_with_attempts(
        Path::new("repository"),
        &worktree,
        "Windows",
        &allowed,
        |arguments| {
            if arguments.first().copied() == Some("worktree")
                && arguments.get(1).copied() == Some("remove")
            {
                calls.fetch_add(1, Ordering::Relaxed);
                return Err("locked".to_owned());
            }
            assert_eq!(arguments, ["worktree", "prune"]);
            Ok(())
        },
        5,
    )
    .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 5);
    assert!(!worktree.exists());

    let outside = target.path().join("outside");
    fs::create_dir(&outside).unwrap();
    assert!(
        remove_partial_worktree_directory(&outside, &allowed)
            .unwrap_err()
            .contains("拒绝清理允许范围外目录")
    );
    assert!(outside.exists());

    let errors = vec!["前端清理失败".to_owned()];
    let error = fail_on_cleanup(Some("原始门禁失败"), "案例清理", &errors).unwrap_err();
    assert!(error.contains("原始门禁失败"));
    assert!(error.contains("前端清理失败"));
}

#[test]
fn switch_worktree_does_not_checkout_an_unchanged_commit() {
    let commit = "a".repeat(40);
    let calls = AtomicUsize::new(0);
    switch_worktree_with(Path::new("worktree"), &commit, "后端", |arguments| {
        calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(arguments, ["rev-parse", "HEAD"]);
        Ok(commit.to_uppercase())
    })
    .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    let mut checkout = Vec::new();
    switch_worktree_with(Path::new("worktree"), &commit, "后端", |arguments| {
        checkout.push(
            arguments
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
        );
        Ok(if arguments[0] == "rev-parse" {
            "b".repeat(40)
        } else {
            String::new()
        })
    })
    .unwrap();
    assert_eq!(
        checkout[1],
        vec![
            "checkout".to_owned(),
            "--quiet".to_owned(),
            "--detach".to_owned(),
            commit,
        ]
    );
    assert!(!checkout[1].contains(&"--force".to_owned()));
}

#[test]
fn repositories_sharing_git_common_dir_and_invalid_ranges_fail_closed() {
    let target = tempfile::tempdir().unwrap();
    let repository = target.path().join("repository");
    let linked = target.path().join("linked");
    fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "--quiet"]);
    git(&repository, &["config", "user.email", "ci@example.invalid"]);
    git(&repository, &["config", "user.name", "CI"]);
    fs::write(repository.join("Cargo.toml"), "[workspace]\n").unwrap();
    git(&repository, &["add", "Cargo.toml"]);
    git(&repository, &["commit", "--quiet", "-m", "base"]);
    git(
        &repository,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    let primary = repository_identity(&repository, "主", "Cargo.toml").unwrap();
    let secondary = repository_identity(&linked, "副", "Cargo.toml").unwrap();
    assert!(
        validate_repository_pair(&primary, &secondary)
            .unwrap_err()
            .contains("common-dir")
    );
    let head = git_output(&repository, &["rev-parse", "HEAD"]);
    let invalid_case = ReplayCase {
        name: "invalid-range".to_owned(),
        category: "addition".to_owned(),
        base: "f".repeat(40),
        head: head.clone(),
        frontend_base: head.clone(),
        frontend_head: head,
        targeted_mode: "targeted".to_owned(),
        expected: "pass".to_owned(),
    };
    assert!(
        verify_case_ranges(&primary, &secondary, &invalid_case)
            .unwrap_err()
            .contains("commit 不存在")
    );
    git(
        &repository,
        &["worktree", "remove", "--force", linked.to_str().unwrap()],
    );
}

#[test]
fn frontend_dependency_fingerprint_tracks_lock_and_patch_inputs() {
    let target = tempfile::tempdir().unwrap();
    fs::write(target.path().join("package.json"), "{}\n").unwrap();
    fs::write(target.path().join("pnpm-lock.yaml"), "lock\n").unwrap();
    let first =
        super::ci::resource_gate::replay::frontend_dependency_fingerprint(target.path()).unwrap();
    fs::write(target.path().join("pnpm-lock.yaml"), "lock-2\n").unwrap();
    let second =
        super::ci::resource_gate::replay::frontend_dependency_fingerprint(target.path()).unwrap();
    assert_ne!(first, second);
    fs::create_dir(target.path().join("patches")).unwrap();
    fs::write(target.path().join("patches/fix.patch"), "diff\n").unwrap();
    let third =
        super::ci::resource_gate::replay::frontend_dependency_fingerprint(target.path()).unwrap();
    assert_ne!(second, third);
}

fn valid_cases() -> Vec<ReplayCase> {
    (0..MINIMUM_CASES)
        .map(|index| ReplayCase {
            name: format!("case-{index}"),
            category: CATEGORIES[index % CATEGORIES.len()].to_owned(),
            base: format!("{:040x}", index + 1),
            head: format!("{:040x}", index + 101),
            frontend_base: format!("{:040x}", index + 201),
            frontend_head: format!("{:040x}", index + 301),
            targeted_mode: if index % 2 == 0 {
                "targeted".to_owned()
            } else {
                "full".to_owned()
            },
            expected: if index % 2 == 0 {
                "pass".to_owned()
            } else {
                "fail".to_owned()
            },
        })
        .collect()
}

fn command_result(passed: bool, duration_ms: u64, recognized: bool, mode: &str) -> CommandResult {
    CommandResult {
        passed,
        return_code: if passed { 0 } else { 1 },
        duration_ms,
        order: 0,
        decision: ResourceGateDecision {
            recognized,
            mode: mode.to_owned(),
            fallback: (mode == "full").then(|| "fixture fallback".to_owned()),
            steps: vec!["resource-drift".to_owned()],
        },
        target_fingerprint: format!("sha256:{duration_ms:064x}"),
        output_fingerprint: format!("sha256:{:064x}", duration_ms + 1),
        failure_tail: None,
    }
}

fn replay_result(index: usize, duration_ms: u64) -> ReplayResult {
    ReplayResult {
        name: format!("case-{index}"),
        category: CATEGORIES[index % CATEGORIES.len()].to_owned(),
        base: format!("{:040x}", index + 1),
        head: format!("{:040x}", index + 101),
        frontend_base: format!("{:040x}", index + 201),
        frontend_head: format!("{:040x}", index + 301),
        targeted_mode: "targeted".to_owned(),
        expected: "pass".to_owned(),
        targeted: command_result(true, duration_ms, true, "targeted"),
        full: command_result(true, duration_ms, false, "full"),
        matches: true,
    }
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} 失败：{}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn replay_case_protocol_uses_arguments_for_range_activation_and_budgets() {
    let path = std::env::temp_dir().join("case 中文 decision.json");
    let mut options = crate::cli::ResourceGateOptions {
        base_sha: Some("1".repeat(40)),
        head_sha: Some("2".repeat(40)),
        frontend_ref: Some("3".repeat(40)),
        targeted: true,
        decision_file: Some(path.clone()),
        jobs: Some(12),
        test_jobs: Some(8),
    };
    let command = ["cargo", "xtask", "check", "ci", "resource-gate"].map(str::to_owned);
    let expand = |options: &crate::cli::ResourceGateOptions| {
        command_for_replay(
            &command,
            Path::new("D:/前端 worktree"),
            Path::new("D:/target"),
            Path::new("D:/runner/Cargo.toml"),
            options,
        )
        .unwrap()
    };
    let expanded = expand(&options);
    for (name, value) in [
        ("--base-sha", "1".repeat(40)),
        ("--head-sha", "2".repeat(40)),
        ("--frontend-ref", "3".repeat(40)),
        ("--decision-file", path.to_str().unwrap().to_owned()),
        ("--jobs", "12".to_owned()),
        ("--test-jobs", "8".to_owned()),
    ] {
        assert_eq!(expanded.iter().filter(|arg| *arg == name).count(), 1);
        assert!(
            expanded
                .windows(2)
                .any(|pair| pair == [name, value.as_str()])
        );
    }
    assert_eq!(
        expanded.iter().filter(|arg| *arg == "--targeted").count(),
        1
    );
    options.targeted = false;
    assert!(!expand(&options).iter().any(|arg| arg == "--targeted"));
}

#[test]
fn replay_does_not_append_gate_protocol_to_external_or_other_xtask_commands() {
    let options = crate::cli::ResourceGateOptions {
        base_sha: Some("1".repeat(40)),
        head_sha: Some("2".repeat(40)),
        frontend_ref: Some("3".repeat(40)),
        targeted: true,
        decision_file: Some(std::env::temp_dir().join("decision.json")),
        jobs: Some(12),
        test_jobs: Some(8),
    };
    let expand = |command: &[String]| {
        command_for_replay(
            command,
            Path::new("D:/frontend"),
            Path::new("D:/target"),
            Path::new("D:/runner/Cargo.toml"),
            &options,
        )
        .unwrap()
    };
    for command in [
        vec!["external-driver", "--mode", "check"],
        vec!["cargo", "test", "--test", "resource-gate"],
        vec!["external-driver", "--frontend-dir", "unchanged"],
    ] {
        let command = command.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(expand(&command), command);
    }
    for operation in ["plan", "preflight"] {
        let command = ["cargo", "xtask", "check", "ci", operation].map(str::to_owned);
        let expanded = expand(&command);
        assert!(
            expanded
                .windows(3)
                .any(|args| args == ["check", "ci", operation])
        );
        for flag in [
            "--base-sha",
            "--head-sha",
            "--frontend-ref",
            "--targeted",
            "--decision-file",
            "--jobs",
            "--test-jobs",
        ] {
            assert!(
                !expanded.iter().any(|arg| arg == flag),
                "{operation}: {flag}"
            );
        }
    }
    let command = ["cargo", "xtask", "other", "ci", "resource-gate"].map(str::to_owned);
    assert!(!expand(&command).iter().any(|arg| arg == "--targeted"));
}
