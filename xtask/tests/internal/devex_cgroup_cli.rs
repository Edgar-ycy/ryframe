use std::path::Path;

use super::{
    check::TaskExecutor,
    cli::{CheckCommand, Command, parse},
    devex::{DevexCgroupOperation, DevexCommand, cgroup_plan_at},
    workspace::root_dir,
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn cgroup_cli_accepts_only_typed_run_and_cleanup_operations() {
    for (operation, expected) in [
        ("run", DevexCgroupOperation::Run),
        ("cleanup", DevexCgroupOperation::Cleanup),
    ] {
        let cli = parse(strings(&[
            "check",
            "perf",
            "cgroup",
            operation,
            "--output",
            ".local-tests/含 空格/cgroup",
            "--plan",
        ]))
        .unwrap();
        let Command::Check(CheckCommand::Perf(DevexCommand::Cgroup(options))) = cli.command else {
            panic!("应解析为 DevEx cgroup")
        };
        assert_eq!(options.operation, expected);
        assert_eq!(options.output, Path::new(".local-tests/含 空格/cgroup"));
        assert!(options.plan);
    }
}

#[test]
fn cgroup_cli_rejects_missing_duplicate_unknown_and_escaping_arguments() {
    for arguments in [
        ["check", "perf", "cgroup"].as_slice(),
        ["check", "perf", "cgroup", "other"].as_slice(),
        ["check", "perf", "cgroup", "run"].as_slice(),
        [
            "check",
            "perf",
            "cgroup",
            "run",
            "--output",
            ".local-tests/a",
            "--output",
            ".local-tests/b",
        ]
        .as_slice(),
        [
            "check",
            "perf",
            "cgroup",
            "run",
            "--output",
            ".local-tests/a",
            "--plan",
            "--plan",
        ]
        .as_slice(),
        [
            "check",
            "perf",
            "cgroup",
            "cleanup",
            "--output",
            "../outside",
        ]
        .as_slice(),
        [
            "check",
            "perf",
            "cgroup",
            "cleanup",
            "--output",
            ".local-tests/a",
            "--unknown",
        ]
        .as_slice(),
    ] {
        assert!(parse(strings(arguments)).is_err(), "{arguments:?}");
    }
    assert!(
        parse(strings(&[
            "check",
            "perf",
            "cgroup",
            "run",
            "--output",
            ".local-tests",
        ]))
        .is_err()
    );
    let root = root_dir();
    let outside = root.parent().unwrap().join("ryframe-cgroup-outside");
    assert!(!outside.starts_with(root.join(".local-tests")));
    assert!(
        parse(vec![
            "check".to_owned(),
            "perf".to_owned(),
            "cgroup".to_owned(),
            "run".to_owned(),
            "--output".to_owned(),
            outside.to_string_lossy().into_owned(),
        ])
        .is_err()
    );
}

#[test]
fn cgroup_execution_consumes_the_same_registered_task_plan_without_python() {
    let root = root_dir();
    for (operation, executor) in [
        ("run", TaskExecutor::PerfCgroupRun),
        ("cleanup", TaskExecutor::PerfCgroupCleanup),
    ] {
        let cli = parse(strings(&[
            "check",
            "perf",
            "cgroup",
            operation,
            "--output",
            ".local-tests/cgroup-plan",
        ]))
        .unwrap();
        let Command::Check(CheckCommand::Perf(DevexCommand::Cgroup(options))) = cli.command else {
            panic!("应解析为 DevEx cgroup")
        };
        let plan = cgroup_plan_at(&options, &root).unwrap();
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].executor, executor);
    }
    let source = std::fs::read_to_string(root.join("xtask/src/devex/cgroup.rs")).unwrap();
    assert!(!source.contains("python"));
}
