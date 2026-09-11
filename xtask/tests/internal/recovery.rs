use std::path::Path;

use super::cli::RecoveryCommand;
use super::recovery::recovery_command;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn runtime_stage_uses_private_verifier_and_only_build_forwards_the_tool_frontend() {
    let frontend = Path::new("D:/前端 worktree");
    let (script, forwarded) = recovery_command(
        &RecoveryCommand::Runtime(strings(&["verify", "--receipt", "runtime.json"])),
        frontend,
    )
    .unwrap();
    assert_eq!(script, "scripts/restore_runtime.py");
    assert_eq!(
        forwarded,
        strings(&["verify", "--receipt", "runtime.json", "--backend-dir",])
            .into_iter()
            .chain([super::workspace::root_dir().display().to_string()])
            .collect::<Vec<_>>()
    );

    let (_, build) = recovery_command(
        &RecoveryCommand::Runtime(strings(&["build", "--write"])),
        frontend,
    )
    .unwrap();
    assert_eq!(
        build,
        strings(&["build", "--write", "--backend-dir"])
            .into_iter()
            .chain([super::workspace::root_dir().display().to_string()])
            .chain(strings(&["--frontend-dir", "D:/前端 worktree"]))
            .collect::<Vec<_>>()
    );

    let (_, registration) = recovery_command(
        &RecoveryCommand::Runtime(strings(&[
            "register",
            "--target-plan",
            "D:/恢复 target.json",
            "--write",
        ])),
        frontend,
    )
    .unwrap();
    assert_eq!(
        registration,
        strings(&[
            "register",
            "--target-plan",
            "D:/恢复 target.json",
            "--write",
            "--backend-dir",
        ])
        .into_iter()
        .chain([super::workspace::root_dir().display().to_string()])
        .collect::<Vec<_>>()
    );
}

#[test]
fn reference_stages_keep_their_existing_arguments() {
    let arguments = strings(&["plan", "--id", "r1"]);
    let (script, forwarded) =
        recovery_command(&RecoveryCommand::Reference(arguments), Path::new("unused")).unwrap();
    assert_eq!(script, "scripts/restore_reference.py");
    assert_eq!(
        forwarded,
        strings(&["plan", "--id", "r1", "--backend-dir"])
            .into_iter()
            .chain([super::workspace::root_dir().display().to_string()])
            .collect::<Vec<_>>()
    );
}

#[test]
fn restore_input_plans_use_the_private_generator() {
    let arguments = strings(&["reference", "--side", "base", "--work-dir", "D:/恢复 work"]);
    let (script, forwarded) = recovery_command(
        &RecoveryCommand::Inputs(arguments.clone()),
        Path::new("unused"),
    )
    .unwrap();
    assert_eq!(script, "scripts/restore_input_plan.py");
    assert_eq!(
        forwarded,
        arguments
            .into_iter()
            .chain(strings(&["--backend-dir"]))
            .chain([super::workspace::root_dir().display().to_string()])
            .collect::<Vec<_>>()
    );
}

#[test]
fn fresh_target_uses_the_private_state_machine_with_the_current_backend() {
    let (script, forwarded) = recovery_command(
        &RecoveryCommand::FreshTarget(strings(&[
            "--workspace",
            "D:/隔离 target",
            "--operation",
            "status",
        ])),
        Path::new("unused"),
    )
    .unwrap();
    assert_eq!(script, "scripts/devex_clone.py");
    assert_eq!(
        forwarded,
        strings(&[
            "fresh-target",
            "--workspace",
            "D:/隔离 target",
            "--operation",
            "status",
            "--backend-dir",
        ])
        .into_iter()
        .chain([super::workspace::root_dir().display().to_string()])
        .collect::<Vec<_>>(),
    );
    assert!(
        recovery_command(
            &RecoveryCommand::FreshTarget(strings(&["--backend-dir", "other"])),
            Path::new("unused"),
        )
        .is_err()
    );
}

#[test]
fn source_fixture_and_dataset_stages_fix_the_current_worktree_paths() {
    let frontend = Path::new("D:/前端 worktree");
    let backend = super::workspace::root_dir().display().to_string();
    let source_input = strings(&[
        "verify",
        "--source-generation",
        "D:/验收/start.json",
        "--output",
        "D:/验收/verification/source-runtime.json",
        "--write",
    ]);
    let (source, source_arguments) =
        recovery_command(&RecoveryCommand::Source(source_input.clone()), frontend).unwrap();
    assert_eq!(source, "scripts/restore_source.py");
    let mut expected_source = source_input;
    expected_source.extend(strings(&["--backend-dir", &backend]));
    assert_eq!(source_arguments, expected_source);
    let (source, comparison_arguments) = recovery_command(
        &RecoveryCommand::Source(strings(&[
            "comparison-verify",
            "--receipt",
            "D:/验收 comparison.json",
        ])),
        frontend,
    )
    .unwrap();
    assert_eq!(source, "scripts/restore_source.py");
    assert_eq!(
        comparison_arguments,
        strings(&[
            "comparison-verify",
            "--receipt",
            "D:/验收 comparison.json",
            "--backend-dir",
            &backend,
        ])
    );

    let (fixture, fixture_arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&["--output-dir", "fixture"])),
        frontend,
    )
    .unwrap();
    assert_eq!(fixture, "scripts/prepare_full_stack_fixture.py");
    assert_eq!(
        fixture_arguments,
        strings(&[
            "--output-dir",
            "fixture",
            "--backend-dir",
            &backend,
            "--frontend-dir",
            "D:/前端 worktree",
        ])
    );

    let (dataset, dataset_arguments) = recovery_command(
        &RecoveryCommand::DatasetPrepare(strings(&["--plan", "reference.json", "--write"])),
        frontend,
    )
    .unwrap();
    assert_eq!(dataset, "scripts/restore_reference_dataset.mjs");
    assert_eq!(
        dataset_arguments,
        strings(&[
            "--plan",
            "reference.json",
            "--write",
            "--backend-dir",
            &backend,
        ])
    );
    assert!(
        recovery_command(
            &RecoveryCommand::Fixture(strings(&["--backend-dir", "other"])),
            frontend,
        )
        .is_err()
    );
    assert!(
        recovery_command(
            &RecoveryCommand::Fixture(strings(&["--frontend-dir", "other"])),
            frontend,
        )
        .is_err()
    );

    let (clone, clone_arguments) = recovery_command(
        &RecoveryCommand::Clone(strings(&["status", "--run-dir", "run"])),
        frontend,
    )
    .unwrap();
    assert_eq!(clone, "scripts/devex_clone.py");
    assert_eq!(
        clone_arguments,
        strings(&["status", "--run-dir", "run", "--backend-dir", &backend])
    );
}

#[test]
fn fixture_environment_plan_uses_only_the_current_backend() {
    let frontend = Path::new("D:/前端 worktree");
    let backend = super::workspace::root_dir().display().to_string();
    let (script, arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&["environment", "plan", "--review", "review.json"])),
        frontend,
    )
    .unwrap();
    assert_eq!(script, "scripts/reference_fixture_environment.py");
    assert_eq!(
        arguments,
        strings(&["plan", "--review", "review.json", "--backend-dir", &backend])
    );
}

#[test]
fn fixture_review_uses_only_the_current_backend() {
    let frontend = Path::new("D:/前端 worktree");
    let backend = super::workspace::root_dir().display().to_string();
    let (script, arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&["review", "--template", "review.json"])),
        frontend,
    )
    .unwrap();
    assert_eq!(script, "scripts/reference_fixture_review.py");
    assert_eq!(
        arguments,
        strings(&["--template", "review.json", "--backend-dir", &backend])
    );
}

#[test]
fn fixture_request_uses_only_the_current_backend() {
    let frontend = Path::new("D:/前端 worktree");
    let backend = super::workspace::root_dir().display().to_string();
    let (script, arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&["request", "--side", "base", "--id", "fresh-r12"])),
        frontend,
    )
    .unwrap();
    assert_eq!(script, "scripts/reference_fixture_request.py");
    assert_eq!(
        arguments,
        strings(&[
            "--side",
            "base",
            "--id",
            "fresh-r12",
            "--backend-dir",
            &backend,
        ])
    );
}

#[test]
fn fixture_successor_uses_only_the_current_backend() {
    let frontend = Path::new("D:/前端 worktree");
    let backend = super::workspace::root_dir().display().to_string();
    let (script, arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&[
            "successor",
            "relationship",
            "--id",
            "successor-r1",
        ])),
        frontend,
    )
    .unwrap();
    assert_eq!(script, "scripts/reference_fixture_successor.py");
    assert_eq!(
        arguments,
        strings(&[
            "relationship",
            "--id",
            "successor-r1",
            "--backend-dir",
            &backend,
        ])
    );
}

#[test]
fn fixture_successor_arm_request_preserves_spaced_paths() {
    let frontend = Path::new("D:/前端 worktree");
    let backend = super::workspace::root_dir().display().to_string();
    let (script, arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&[
            "successor",
            "arm-request",
            "--workspace",
            "D:/fresh target/base",
            "--side",
            "base",
        ])),
        frontend,
    )
    .unwrap();
    assert_eq!(script, "scripts/reference_fixture_successor.py");
    assert_eq!(
        arguments,
        strings(&[
            "arm-request",
            "--workspace",
            "D:/fresh target/base",
            "--side",
            "base",
            "--backend-dir",
            &backend,
        ])
    );
}

#[test]
fn fixture_artifact_and_retention_use_registered_runtime_tools() {
    let backend = super::workspace::root_dir().display().to_string();
    for (operation, script, arguments) in [
        (
            "artifact",
            "scripts/full_stack_artifacts.py",
            strings(&["snapshot", "--runtime-dir", "D:/验收/runtime"]),
        ),
        (
            "retention",
            "scripts/full_stack_migration_history.py",
            strings(&["plan-history", "--runtime-dir", "D:/验收/runtime"]),
        ),
    ] {
        let mut request = vec![operation.to_owned()];
        request.extend(arguments.clone());
        let (actual_script, forwarded) =
            recovery_command(&RecoveryCommand::Fixture(request), Path::new("unused")).unwrap();
        assert_eq!(actual_script, script);
        assert_eq!(
            forwarded,
            arguments
                .into_iter()
                .chain(strings(&["--backend-dir", &backend]))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn fixture_services_use_the_private_service_controller() {
    for service in ["rustfs", "redis", "buckets", "status", "close", "recover"] {
        let mut input = strings(&["services", service]);
        if service != "status" {
            input.push("--write".to_owned());
        }
        let (script, arguments) =
            recovery_command(&RecoveryCommand::Fixture(input), Path::new("unused")).unwrap();
        assert_eq!(script, "scripts/reference_fixture_services.py");
        assert_eq!(
            arguments.last().map(String::as_str),
            Some(super::workspace::root_dir().to_str().unwrap())
        );
    }
}

#[test]
fn fixture_source_pair_uses_the_private_fixture_source_receipt() {
    let (script, arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&[
            "source-pair",
            "--output",
            "pair.json",
            "--write",
        ])),
        Path::new("unused"),
    )
    .unwrap();
    assert_eq!(script, "scripts/reference_fixture_source_pair.py");
    assert!(
        arguments
            .windows(2)
            .any(|item| item == ["--output", "pair.json"])
    );
}

#[test]
fn fixture_runtime_uses_the_private_device_runtime_controller() {
    let (script, arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&["runtime", "verify", "--output", "runtime-r1"])),
        Path::new("unused"),
    )
    .unwrap();
    assert_eq!(script, "scripts/reference_fixture_runtime.py");
    assert!(
        arguments
            .windows(2)
            .any(|item| item == ["--output", "runtime-r1"])
    );
    assert_eq!(
        arguments.last().map(String::as_str),
        Some(super::workspace::root_dir().to_str().unwrap())
    );
}

#[test]
fn fixture_dataset_uses_the_private_device_dataset_adapter() {
    let (script, arguments) = recovery_command(
        &RecoveryCommand::Fixture(strings(&["dataset", "plan", "--runtime", "runtime-r1"])),
        Path::new("unused"),
    )
    .unwrap();
    assert_eq!(script, "scripts/reference_fixture_dataset.py");
    assert!(
        arguments
            .windows(2)
            .any(|item| item == ["--runtime", "runtime-r1"])
    );
    assert_eq!(
        arguments.last().map(String::as_str),
        Some(super::workspace::root_dir().to_str().unwrap())
    );
}

#[test]
fn forwarded_recovery_scripts_exist_in_checkout() {
    let root = super::workspace::root_dir();
    for command in [
        RecoveryCommand::Reference(strings(&["plan"])),
        RecoveryCommand::Inputs(strings(&["reference"])),
        RecoveryCommand::Runtime(strings(&["verify"])),
        RecoveryCommand::Source(strings(&["verify"])),
        RecoveryCommand::Clone(strings(&["status"])),
        RecoveryCommand::Fixture(strings(&["--output-dir", "fixture"])),
        RecoveryCommand::Fixture(strings(&["artifact", "snapshot"])),
        RecoveryCommand::Fixture(strings(&["retention", "inspect"])),
        RecoveryCommand::Fixture(strings(&["dataset", "plan"])),
        RecoveryCommand::DatasetPrepare(strings(&["--plan", "reference.json"])),
        RecoveryCommand::FreshTarget(strings(&[
            "--operation",
            "status",
            "--workspace",
            "D:/target",
        ])),
    ] {
        let (script, _) = recovery_command(&command, Path::new("unused")).unwrap();
        assert!(
            root.join(script).is_file(),
            "恢复入口转发的脚本不在当前检出中：{script}"
        );
    }
}
