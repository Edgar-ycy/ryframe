use super::*;
use crate::session_pacing::artifact::Artifacts;

#[test]
fn authority_and_summary_receipts_keep_phases_and_exclude_secrets() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    let authority = fixture.receipt("pacing-authority.json");
    assert_eq!(authority["success"], json!(true));
    assert_eq!(authority["authority"], super::authority());
    assert_eq!(authority["actual_sha256"], pacing()["authority_sha256"]);
    assert_eq!(authority["expected_sha256"], pacing()["authority_sha256"]);
    assert!(authority["duration_ms"].is_number());
    assert!(authority.get("failure").is_none());
    run(hook.before(&metadata("get_auth_csrf"))).unwrap();
    run(hook.complete()).unwrap();
    run(hook.close()).unwrap();
    let summary = fixture.receipt("pacing-summary.json");
    assert_eq!(summary["measurement"], json!({}));
    assert_eq!(summary["pacing"], pacing());
    assert_eq!(
        summary["preparation"]["get_auth_csrf"]["requests"],
        json!(1)
    );
    assert_eq!(summary["login_budget"]["waits"], json!(0));
    let text = format!("{authority}{summary}");
    for secret in [
        "fixture-secret-not-for-receipts",
        "login-budget.json",
        "reader",
        "198.18.10.1",
    ] {
        assert!(!text.contains(secret));
    }
}

#[test]
fn digest_mismatch_records_failure_before_any_request_or_budget_mutation() {
    let mut fixture = Fixture::new();
    fixture.config["contract"]["pacing"]["authority_sha256"] = json!("0".repeat(64));
    assert_eq!(category(fixture.hook()), "pacing-authority-digest");
    let receipt = fixture.receipt("pacing-authority.json");
    assert_eq!(receipt["success"], json!(false));
    assert_eq!(receipt["failure"], json!("pacing-authority-digest"));
    assert_eq!(receipt["actual_sha256"], pacing()["authority_sha256"]);
    assert_eq!(receipt["expected_sha256"], json!("0".repeat(64)));
    assert!(!fixture.budget().exists());
    assert!(!fixture.artifacts.join("pacing-summary.json").exists());
}

#[test]
fn binding_failure_records_only_fixed_category_without_mutating_budget() {
    let mut fixture = Fixture::new();
    fixture.config["bindings"]["pacing"]["extra"] = json!("fixture-secret-not-for-receipts");
    fixture.config["contract"]["pacing"]["authority_sha256"] =
        json!("fixture-secret-not-for-receipts");
    assert_eq!(category(fixture.hook()), "pacing-binding");
    let receipt = fixture.receipt("pacing-authority.json");
    assert_eq!(receipt["failure"], json!("pacing-binding"));
    assert!(receipt.get("actual_sha256").is_none());
    assert!(receipt.get("expected_sha256").is_none());
    assert!(
        !receipt
            .to_string()
            .contains("fixture-secret-not-for-receipts")
    );
    assert!(!fixture.budget().exists());
}

#[test]
fn authority_read_and_catalog_failures_are_sanitized() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join("config/app.toml"),
        "fixture-secret-not-for-receipts",
    )
    .unwrap();
    assert_eq!(category(fixture.hook()), "pacing-authority");
    assert_eq!(
        fixture.receipt("pacing-authority.json")["failure"],
        json!("pacing-authority")
    );
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join("openapi/openapi.json"),
        "fixture-secret-not-for-receipts",
    )
    .unwrap();
    assert_eq!(category(fixture.hook()), "pacing-catalog");
    let receipt = fixture.receipt("pacing-authority.json");
    assert_eq!(receipt["failure"], json!("pacing-catalog"));
    assert!(
        !receipt
            .to_string()
            .contains("fixture-secret-not-for-receipts")
    );
}

#[test]
fn login_reservation_is_completed_once_even_after_http_failure() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    run(hook.before(&metadata("post_auth_login"))).unwrap();
    assert!(fixture.budget().is_file());
    assert_eq!(
        category(run(hook.before(&metadata("get_auth_csrf")))),
        "pacing-pending"
    );
    // HTTP 失败与成功均走同一 complete，hook 不根据状态码退款或重发。
    let http_failure = SessionError {
        category: "http-status",
        operation: Some("post_auth_login".into()),
        status: Some(429),
    };
    assert_eq!(http_failure.status, Some(429));
    run(hook.complete()).unwrap();
    let completed = fs::read(fixture.budget()).unwrap();
    run(hook.complete()).unwrap();
    assert_eq!(fs::read(fixture.budget()).unwrap(), completed);
    run(hook.close()).unwrap();
    assert_eq!(
        fixture.receipt("pacing-summary.json")["preparation"]["post_auth_login"]["requests"],
        json!(1)
    );
}

#[test]
fn completion_failure_is_consumed_without_retrying_unknown_ledger_write() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    run(hook.before(&metadata("post_auth_login"))).unwrap();
    fs::write(fixture.budget(), "fixture-corrupt-ledger").unwrap();
    assert_eq!(category(run(hook.complete())), "pacing-budget-complete");
    run(hook.complete()).unwrap();
    assert_eq!(
        fs::read(fixture.budget()).unwrap(),
        b"fixture-corrupt-ledger"
    );
    run(hook.close()).unwrap();
}

#[test]
fn nonlogin_complete_is_noop_and_invalid_metadata_does_not_poison_queue() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    let budget_before = fs::read(fixture.budget()).ok();
    let mut invalid = metadata("get_auth_csrf");
    invalid.route = "/wrong-route-with-secret".into();
    assert_eq!(category(run(hook.before(&invalid))), "pacing-operation");
    run(hook.before(&metadata("get_auth_csrf"))).unwrap();
    run(hook.complete()).unwrap();
    run(hook.complete()).unwrap();
    assert_eq!(fs::read(fixture.budget()).ok(), budget_before);
    run(hook.close()).unwrap();
    let summary = fixture.receipt("pacing-summary.json");
    assert_eq!(
        summary["preparation"]["get_auth_csrf"]["requests"],
        json!(1)
    );
    assert!(!summary.to_string().contains("wrong-route-with-secret"));
}

#[test]
fn close_is_idempotent_and_closes_request_entry() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    run(hook.close()).unwrap();
    let original = fs::read(fixture.artifacts.join("pacing-summary.json")).unwrap();
    run(hook.close()).unwrap();
    assert_eq!(
        fs::read(fixture.artifacts.join("pacing-summary.json")).unwrap(),
        original
    );
    assert_eq!(
        category(run(hook.before(&metadata("get_auth_csrf")))),
        "pacing-closed"
    );
}

#[test]
fn summary_create_new_failure_is_latched_and_never_retried() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    let path = fixture.artifacts.join("pacing-summary.json");
    fs::write(&path, b"existing-evidence").unwrap();
    assert_eq!(category(run(hook.close())), "pacing-artifact");
    assert_eq!(fs::read(&path).unwrap(), b"existing-evidence");
    fs::remove_file(&path).unwrap();
    assert_eq!(category(run(hook.close())), "pacing-artifact");
    assert!(!path.exists());
}

#[test]
fn authority_create_new_failure_preserves_existing_evidence() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.artifacts).unwrap();
    let path = fixture.artifacts.join("pacing-authority.json");
    fs::write(&path, b"existing-authority").unwrap();
    assert_eq!(category(fixture.hook()), "pacing-artifact");
    assert_eq!(fs::read(path).unwrap(), b"existing-authority");
    assert!(!fixture.budget().exists());
}

#[test]
fn receipts_use_regular_directories_and_reject_replaced_directory_identity() {
    let fixture = Fixture::new();
    fs::write(&fixture.artifacts, b"ordinary-file").unwrap();
    assert_eq!(
        category(Artifacts::new(&fixture.artifacts)),
        "pacing-artifact"
    );
    fs::remove_file(&fixture.artifacts).unwrap();
    let artifacts = Artifacts::new(&fixture.artifacts).unwrap();
    // 保留旧目录，确保重建目录不能复用旧文件身份。
    let moved = fixture.root().join("retained-artifacts");
    fs::rename(&fixture.artifacts, &moved).unwrap();
    fs::create_dir(&fixture.artifacts).unwrap();
    assert_eq!(
        category(artifacts.write("pacing-summary.json", &json!({}))),
        "pacing-artifact"
    );
    assert!(!fixture.artifacts.join("pacing-summary.json").exists());
    assert!(!moved.join("pacing-summary.json").exists());
}

#[test]
fn canonical_authority_digest_is_order_independent() {
    let first: Value = serde_json::from_str(r#"{"b":{"z":1,"a":2},"a":[{"y":3,"x":4}]}"#).unwrap();
    let second: Value = serde_json::from_str(r#"{"a":[{"x":4,"y":3}],"b":{"a":2,"z":1}}"#).unwrap();
    assert_eq!(
        artifact::digest(&first).unwrap(),
        artifact::digest(&second).unwrap()
    );
}

#[test]
fn corrupt_shared_budget_is_not_replaced_and_before_failure_leaves_no_start() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    fs::write(fixture.budget(), "fixture-corrupt-ledger").unwrap();
    assert_eq!(
        category(run(hook.before(&metadata("post_auth_login")))),
        "pacing-budget"
    );
    assert_eq!(
        fs::read(fixture.budget()).unwrap(),
        b"fixture-corrupt-ledger"
    );
    run(hook.complete()).unwrap();
    run(hook.before(&metadata("get_auth_csrf"))).unwrap();
    run(hook.complete()).unwrap();
    run(hook.close()).unwrap();
    let summary = fixture.receipt("pacing-summary.json");
    assert!(summary["preparation"].get("post_auth_login").is_none());
    assert_eq!(
        summary["preparation"]["get_auth_csrf"]["requests"],
        json!(1)
    );
}

#[test]
fn hook_is_send_without_global_state() {
    fn send<T: Send>() {}
    send::<PreparationHook>();
}

#[test]
fn close_completes_cancelled_pending_login_once_without_http_replay_or_refund() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    run(hook.before(&metadata("post_auth_login"))).unwrap();
    // 模拟 before 已成功而请求 future 被控制器取消，没有发起替代请求。
    run(hook.close()).unwrap();
    let completed = fs::read(fixture.budget()).unwrap();
    run(hook.close()).unwrap();
    run(hook.complete()).unwrap();
    assert_eq!(fs::read(fixture.budget()).unwrap(), completed);
    assert_eq!(
        fixture.receipt("pacing-summary.json")["preparation"]["post_auth_login"]["requests"],
        json!(1)
    );
    assert_eq!(
        category(run(hook.before(&metadata("post_auth_login")))),
        "pacing-closed"
    );
}

#[test]
fn close_still_writes_summary_when_pending_completion_fails_and_latches_failure() {
    let fixture = Fixture::new();
    let mut hook = fixture.hook().unwrap();
    run(hook.before(&metadata("post_auth_login"))).unwrap();
    fs::write(fixture.budget(), "fixture-corrupt-ledger").unwrap();
    assert_eq!(category(run(hook.close())), "pacing-budget-complete");
    assert_eq!(
        fixture.receipt("pacing-summary.json")["preparation"]["post_auth_login"]["requests"],
        json!(1)
    );
    assert_eq!(category(run(hook.close())), "pacing-budget-complete");
    assert_eq!(
        fs::read(fixture.budget()).unwrap(),
        b"fixture-corrupt-ledger"
    );
}
