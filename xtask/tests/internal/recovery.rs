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
    let mut options = FreshTargetOptions {
        operation: FreshTargetOperation::ReconcilePreflight,
        workspace: root.join(".local-tests/隔离 target"),
        request: None,
        environment: None,
        storage_run: None,
        observation_dir: None,
        write: true,
    };
    let document: serde_json::Value =
        serde_json::from_str(&fresh_target_protocol(&options).unwrap()).unwrap();
    assert_eq!(document["format_version"], 1);
    assert_eq!(document["kind"], "ryframe-xtask-recovery-fresh-target");
    assert_eq!(document.as_object().unwrap().len(), 4);
    assert_eq!(
        document["native_tool"],
        super::check::python_suite::native_tool_binding().unwrap()
    );
    assert_eq!(document["request"]["operation"], "reconcile-preflight");
    assert_eq!(document["request"]["write"], true);
    assert_eq!(
        document["request"]["workspace"],
        options.workspace.to_str().unwrap()
    );
    assert!(document["request"]["request"].is_null());
    options.write = false;
    assert!(fresh_target_protocol(&options).is_err());
    options.write = true;
    options.request = Some(root.join(".local-tests/unused-request.json"));
    assert!(fresh_target_protocol(&options).is_err());
    options.request = None;
    for operation in [
        FreshTargetOperation::Prepare,
        FreshTargetOperation::ResumePrepare,
        FreshTargetOperation::Status,
        FreshTargetOperation::Initialize,
        FreshTargetOperation::ResumeInitialize,
        FreshTargetOperation::Verify,
    ] {
        options.operation = operation;
        assert!(fresh_target_protocol(&options).is_err());
    }
}
