use std::path::Path;

use super::cli::{
    BindingsInputOptions, DatasetPrepareCommand, ExistingReferenceSide, FreshTargetCommand,
    FreshTargetOperation, FreshTargetOptions, FullStackCommand, MonitoringCommand, RecoveryCommand,
    RecoveryInputsCommand, RecoveryReferenceCommand, RuntimeCommand, SourceCommand,
};
use super::recovery::{
    fresh_target_protocol, full_stack_environment, inputs, recovery_command, reference,
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn runtime_and_source_are_not_available_through_the_argv_forwarder() {
    assert!(
        recovery_command(
            &RecoveryCommand::Runtime(RuntimeCommand::Help(None)),
            Path::new("unused"),
        )
        .is_err()
    );
    assert!(
        recovery_command(
            &RecoveryCommand::Source(SourceCommand::Help(None)),
            Path::new("unused"),
        )
        .is_err()
    );
}

#[test]
fn reference_stages_use_a_versioned_private_protocol_without_forwarded_argv() {
    let root = super::workspace::root_dir();
    let directory = root.join(format!(
        ".local-tests/reference-protocol-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let plan = directory.join("计划.json");
    std::fs::write(&plan, "{}").unwrap();
    let command = RecoveryReferenceCommand::CheckExisting {
        plan: plan.clone(),
        side: ExistingReferenceSide::Source,
    };
    let document: serde_json::Value =
        serde_json::from_str(&reference::protocol_at(&command, &root).unwrap()).unwrap();
    assert_eq!(document["format_version"], 1);
    assert_eq!(document["kind"], "ryframe-xtask-recovery-reference");
    assert_eq!(document["request"]["operation"], "check-existing");
    assert_eq!(document["request"]["side"], "source");
    assert_eq!(document["request"]["write"], false);
    assert_eq!(document["request"]["plan"], plan.to_str().unwrap());
    assert!(recovery_command(&RecoveryCommand::Reference(command), Path::new("unused")).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn full_stack_stages_use_one_private_program_and_typed_environment() {
    for (command, operation) in [
        (FullStackCommand::Prepare, "prepare"),
        (FullStackCommand::Start, "start"),
        (FullStackCommand::Collect, "collect"),
    ] {
        let (script, arguments) = recovery_command(
            &RecoveryCommand::FullStack(command.clone()),
            Path::new("unused"),
        )
        .unwrap();
        assert_eq!(script, "scripts/ci_full_stack.py");
        assert!(arguments.is_empty());
        assert_eq!(
            full_stack_environment(&command, "D:/当前 后端").unwrap(),
            vec![
                ("RYFRAME_XTASK_FULL_STACK_OPERATION", operation.to_owned()),
                ("RYFRAME_XTASK_BACKEND_ROOT", "D:/当前 后端".to_owned()),
            ]
        );
    }
    let environment_file = Path::new("D:/临时/github-environment");
    assert_eq!(
        full_stack_environment(
            &FullStackCommand::RateLimit {
                environment_file: environment_file.to_path_buf(),
            },
            "D:/当前 后端",
        )
        .unwrap(),
        vec![
            (
                "RYFRAME_XTASK_FULL_STACK_OPERATION",
                "rate-limit".to_owned()
            ),
            ("RYFRAME_XTASK_BACKEND_ROOT", "D:/当前 后端".to_owned()),
            (
                "RYFRAME_XTASK_FULL_STACK_ENVIRONMENT_FILE",
                "D:/临时/github-environment".to_owned(),
            ),
        ]
    );
}

#[test]
fn restore_input_plans_use_a_versioned_private_protocol_without_forwarded_argv() {
    let root = super::workspace::root_dir();
    let directory = root.join(format!(
        ".local-tests/inputs-protocol-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let file = |name: &str| {
        let path = directory.join(name);
        std::fs::write(&path, "{}").unwrap();
        path
    };
    let command = RecoveryInputsCommand::Bindings(BindingsInputOptions {
        reference_plan: file("reference.json"),
        target_plan: file("target.json"),
        backup_receipt: file("backup.json"),
        record: file("record.json"),
        publication: None,
    });
    let document: serde_json::Value =
        serde_json::from_str(&inputs::protocol_at(&command, &root).unwrap()).unwrap();
    assert_eq!(document["format_version"], 1);
    assert_eq!(document["kind"], "ryframe-xtask-recovery-inputs");
    assert_eq!(document["request"]["operation"], "bindings");
    assert_eq!(document["request"]["write"], false);
    assert!(document["request"].get("output").is_none());
    assert!(recovery_command(&RecoveryCommand::Inputs(command), Path::new("unused")).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn fresh_target_uses_a_versioned_private_protocol_without_forwarded_argv() {
    let root = super::workspace::root_dir();
    let options = FreshTargetOptions {
        operation: FreshTargetOperation::Status,
        workspace: root.join(".local-tests/隔离 target"),
        request: None,
        environment: None,
        storage_run: None,
        observation_dir: None,
        write: false,
    };
    let document: serde_json::Value =
        serde_json::from_str(&fresh_target_protocol(&options).unwrap()).unwrap();
    assert_eq!(document["format_version"], 1);
    assert_eq!(document["kind"], "ryframe-xtask-recovery-fresh-target");
    assert_eq!(document["request"]["operation"], "status");
    assert_eq!(document["request"]["write"], false);
    assert_eq!(
        document["request"]["workspace"],
        options.workspace.to_str().unwrap()
    );
    assert!(document["request"]["request"].is_null());
    assert!(
        recovery_command(
            &RecoveryCommand::FreshTarget(FreshTargetCommand::Run(options)),
            Path::new("unused"),
        )
        .is_err()
    );
}

#[test]
fn dataset_stage_and_clone_keep_current_boundaries() {
    let frontend = Path::new("D:/前端 worktree");
    let backend = super::workspace::root_dir().display().to_string();
    assert!(
        recovery_command(
            &RecoveryCommand::DatasetPrepare(DatasetPrepareCommand::Help),
            frontend,
        )
        .is_err()
    );
    assert!(
        recovery_command(
            &RecoveryCommand::Fixture(strings(&["--output-dir", "fixture"])),
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
fn monitoring_stage_uses_the_private_lifecycle_and_fixed_backend() {
    assert!(
        recovery_command(
            &RecoveryCommand::Monitoring(MonitoringCommand::Help),
            Path::new("unused"),
        )
        .is_err()
    );
}

#[test]
fn fixture_control_domains_cannot_bypass_typed_private_protocol() {
    for domain in ["environment", "review", "request", "successor", "services"] {
        assert!(
            recovery_command(
                &RecoveryCommand::Fixture(strings(&[domain, "unparsed"])),
                Path::new("unused"),
            )
            .is_err()
        );
    }
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
fn fixture_source_pair_cannot_bypass_the_typed_private_protocol() {
    assert!(
        recovery_command(
            &RecoveryCommand::Fixture(strings(&[
                "source-pair",
                "--output",
                "pair.json",
                "--write",
            ])),
            Path::new("unused"),
        )
        .is_err()
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
        RecoveryCommand::Clone(strings(&["status"])),
        RecoveryCommand::Fixture(strings(&["artifact", "snapshot"])),
        RecoveryCommand::Fixture(strings(&["retention", "inspect"])),
        RecoveryCommand::Fixture(strings(&["dataset", "plan"])),
        RecoveryCommand::FullStack(FullStackCommand::Collect),
    ] {
        let (script, _) = recovery_command(&command, Path::new("unused")).unwrap();
        assert!(
            root.join(script).is_file(),
            "恢复入口转发的脚本不在当前检出中：{script}"
        );
    }
    assert!(root.join("scripts/restore_runtime.py").is_file());
    assert!(root.join("scripts/restore_source.py").is_file());
    assert!(root.join("scripts/restore_reference_dataset.mjs").is_file());
    assert!(
        root.join("scripts/restore_monitoring_delivery.py")
            .is_file()
    );
    assert!(root.join("scripts/prepare_full_stack_fixture.py").is_file());
    assert!(
        root.join("scripts/reference_fixture_source_pair.py")
            .is_file()
    );
    assert!(root.join("scripts/restore_reference.py").is_file());
    assert!(root.join("scripts/restore_input_plan.py").is_file());
}
