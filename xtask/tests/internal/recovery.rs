use std::path::Path;

use super::cli::{
    ExistingReferenceSide, FreshTargetOperation, FreshTargetOptions, FullStackCommand,
    RecoveryReferenceCommand,
};
use super::recovery::{fresh_target_protocol, full_stack_environment, reference};

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
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn full_stack_stages_use_typed_environment() {
    for (command, operation) in [
        (FullStackCommand::Prepare, "prepare"),
        (FullStackCommand::Start, "start"),
        (FullStackCommand::Collect, "collect"),
    ] {
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
}

#[test]
fn private_recovery_scripts_exist_in_checkout() {
    let root = super::workspace::root_dir();
    assert!(
        root.join("tools/python/ci_full_stack.py").is_file(),
        "全栈恢复私有脚本不在当前检出中"
    );
    assert!(root.join("tools/python/full_stack_artifacts.py").is_file());
    assert!(root.join("tools/python/devex_clone.py").is_file());
    assert!(root.join("tools/python/restore_runtime.py").is_file());
    assert!(root.join("tools/python/restore_source.py").is_file());
    assert!(
        root.join("tools/js/restore_reference_dataset.mjs")
            .is_file()
    );
    assert!(
        root.join("tools/python/restore_monitoring_delivery.py")
            .is_file()
    );
    assert!(
        root.join("tools/python/prepare_full_stack_fixture.py")
            .is_file()
    );
    assert!(
        root.join("tools/python/reference_fixture_source_pair.py")
            .is_file()
    );
    assert!(root.join("tools/python/restore_reference.py").is_file());
    assert!(!root.join("tools/python/restore_input_plan.py").exists());
}
