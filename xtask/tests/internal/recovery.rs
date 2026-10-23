use super::cli::{
    ExistingReferenceSide, FreshTargetOperation, FreshTargetOptions, RecoveryReferenceCommand,
};
use super::recovery::{fresh_target_protocol, reference};

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
    assert_eq!(document.as_object().unwrap().len(), 4);
    assert_eq!(
        document["native_tool"],
        super::recovery::clone::native_tool_binding().unwrap()
    );
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
        root.join("xtask/src/recovery/full_stack.rs").is_file(),
        "全栈恢复私有脚本不在当前检出中"
    );
    assert!(!root.join("tools/python/full_stack_artifacts.py").exists());
    assert!(root.join("tools/python/devex_clone.py").is_file());
    assert!(root.join("tools/python/restore_runtime.py").is_file());
    assert!(root.join("tools/python/restore_source.py").is_file());
    assert!(!root.join("tools/js/restore_reference_dataset.mjs").exists());
    assert!(root.join("xtask/src/recovery/dataset_prepare.rs").is_file());
    assert!(root.join("xtask/src/recovery/fixture_prepare.rs").is_file());
    assert!(
        root.join("tools/python/reference_fixture_source_pair.py")
            .is_file()
    );
    assert!(root.join("tools/python/restore_reference.py").is_file());
    assert!(!root.join("tools/python/restore_input_plan.py").exists());
}
