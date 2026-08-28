use super::*;

#[test]
fn decision_artifact_records_actual_mode_and_is_atomic() {
    let targeted = analyze(&input_with_post(
        Some(&source("post", "title", "post.read", &[])),
        &source("post", "body", "post.read", &[]),
    ));
    let decision = decision_for(&targeted, true);
    assert!(decision.recognized);
    assert_eq!(decision.mode, ResourceGateMode::Targeted);
    assert_eq!(decision.fallback, None);
    assert!(
        decision
            .steps
            .iter()
            .any(|step| step == "resource-workspace")
    );

    let root = std::env::current_dir()
        .unwrap()
        .join(".local-tests")
        .join(format!(
            "resource-gate-decision-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("decision.json");
    write_decision_artifact(&path, &decision).unwrap();
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(document["formatVersion"], 1);
    assert_eq!(document["recognized"], true);
    assert_eq!(document["mode"], "targeted");
    assert!(document["fallback"].is_null());
    assert!(write_decision_artifact(&path, &decision).is_err());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    std::fs::remove_dir_all(root).unwrap();

    let fallback = decision_for(&enforce_targeted_activation(targeted, false), false);
    assert!(!fallback.recognized);
    assert_eq!(fallback.mode, ResourceGateMode::Full);
    assert!(fallback.fallback.is_some());
    assert!(fallback.steps.iter().any(|step| step == "full-rust-gate"));
}

#[test]
fn targeted_execution_parallelizes_only_the_independent_resource_workspace() {
    let packages = set(&["ryframe-api", "ryframe-application"]);
    let contract_steps = targeted_contract_steps(packages.clone());
    assert_eq!(
        contract_steps,
        vec![
            GateStep::AffectedClippy(packages.clone()),
            GateStep::AffectedTest(packages.clone()),
            GateStep::PermissionContract,
            GateStep::MigrationContract,
            GateStep::OpenApiAndFrontendConsumer,
        ]
    );
    let mut executed = vec![GateStep::ResourceDrift, GateStep::ResourceWorkspace];
    executed.extend(contract_steps);
    assert_eq!(executed, targeted_steps(&packages));
}

#[test]
fn targeted_test_jobs_never_exceed_the_backend_branch_budget() {
    assert_eq!(targeted_test_jobs_from(None, false, 3).unwrap(), 3);
    assert_eq!(targeted_test_jobs_from(None, true, 2).unwrap(), 2);
    assert_eq!(targeted_test_jobs_from(Some("6"), false, 4).unwrap(), 4);
    assert!(targeted_test_jobs_from(Some("0"), false, 4).is_err());
}

#[test]
fn resource_rename_falls_back_to_full_gates() {
    let base = source("post", "title", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &base);
    input.changes = vec![ChangedFile {
        status: ChangeStatus::Renamed,
        path: "catalog/resources/article.toml".to_owned(),
        old_path: Some(POST_PATH.to_owned()),
    }];

    let result = analyze(&input);

    assert!(
        result
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("重命名"))
    );
}

#[test]
fn resource_delete_falls_back_to_full_gates() {
    let base = source("post", "title", "post.read", &[]);
    let mut input = input_with_post(Some(&base), &base);
    input.changes = vec![changed(ChangeStatus::Deleted, POST_PATH)];

    let result = analyze(&input);

    assert!(
        result
            .ambiguous_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("删除"))
    );
}
