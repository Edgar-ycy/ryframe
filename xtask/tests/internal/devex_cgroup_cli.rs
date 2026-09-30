use std::path::Path;

use super::{
    check::TaskExecutor,
    cli::{CheckCommand, Command, parse},
    devex::{DevexCgroupOperation, DevexCommand, cgroup_command_at, cgroup_plan_at},
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
    let outside = std::env::temp_dir().join("ryframe-cgroup-outside");
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
fn cgroup_command_consumes_the_same_registered_task_plan() {
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
        let (program, arguments) = cgroup_command_at(&options, &plan, &root).unwrap();
        assert_eq!(program, "python");
        assert_eq!(
            arguments,
            [
                "-B".to_owned(),
                "tools/python/ci_devex_cgroup.py".to_owned(),
                operation.to_owned(),
                "--output".to_owned(),
                root.join(".local-tests/cgroup-plan")
                    .to_string_lossy()
                    .into_owned(),
            ]
        );
    }

    let run = parse(strings(&[
        "check",
        "perf",
        "cgroup",
        "run",
        "--output",
        ".local-tests/cgroup-plan",
    ]))
    .unwrap();
    let cleanup = parse(strings(&[
        "check",
        "perf",
        "cgroup",
        "cleanup",
        "--output",
        ".local-tests/cgroup-plan",
    ]))
    .unwrap();
    let Command::Check(CheckCommand::Perf(DevexCommand::Cgroup(run))) = run.command else {
        unreachable!()
    };
    let Command::Check(CheckCommand::Perf(DevexCommand::Cgroup(cleanup))) = cleanup.command else {
        unreachable!()
    };
    let cleanup_plan = cgroup_plan_at(&cleanup, &root).unwrap();
    assert!(cgroup_command_at(&run, &cleanup_plan, &root).is_err());
}
