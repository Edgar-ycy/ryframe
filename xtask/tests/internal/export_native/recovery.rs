use super::*;

fn origin(fixture: &Fixture) -> (Context, Value) {
    let sources = json!({"fingerprints":{"product":{"sha256":"a".repeat(64),"files":1}},
        "tool":"origin"});
    let (number, attempt) = state::begin(&fixture.run, "export", "run", &sources).unwrap();
    let owner = json!({"format_version":1,"identity":{"pid":100,"started":"fixture",
        "executable":fixture.local.join("controller.exe")},"directory":fixture.run,
        "manifest_sha256":binding(&fixture.run.join("manifest.json"))["sha256"]});
    fs::create_dir(fixture.run.join("run.lock")).unwrap();
    write_json(&fixture.run.join("run.lock/owner.json"), &owner);
    state::bind_controller(&fixture.run, number, &owner, &attempt).unwrap();
    let context = Context::load(
        &fixture.backend,
        &StageRequest {
            backend: &fixture.backend,
            stage: "export",
            mode: "run",
            run_dir: &fixture.run,
            attempt: Some(number),
        },
    )
    .unwrap();
    recovery::prepare_intent(&context).unwrap();
    fs::create_dir(context.output()).unwrap();
    write_json(
        &context.output().join("export.json"),
        &json!({"candidate":true}),
    );
    let verified = json!({"export":{"logical_inventory_sha256":"b".repeat(64),
        "databases":[{}],"objects":[{"entries":[{},{}]}]},
        "binding":binding(&context.output().join("export.json")),
        "generation":{"fixed":"generation"},"proof_files":{"fixed":{"bytes":1,"sha256":"c".repeat(64)}}});
    (context, verified)
}

#[test]
fn export_seal_preserves_complete_summary_and_rejects_original_binding_drift() {
    let fixture = Fixture::new();
    let (context, verified) = origin(&fixture);
    recovery::record_verified(&context, &verified).unwrap();
    let seal: Value =
        serde_json::from_slice(&fs::read(fixture.run.join("export-0001.verified.json")).unwrap())
            .unwrap();
    assert_eq!(seal["export"], verified["binding"]);
    assert_eq!(
        seal["summary"],
        json!({
        "export_sha256":native::digest::plan_hash(&verified["export"]).unwrap(),
        "generation_sha256":native::digest::plan_hash(&verified["generation"]).unwrap(),
        "proof_files_sha256":native::digest::plan_hash(&verified["proof_files"]).unwrap(),
        "logical_inventory_sha256":"b".repeat(64),"databases":1,"objects":2})
    );
    let path = fixture.run.join("export-0001.intent.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for field in [
        "output",
        "manifest",
        "controller",
        "source_request",
        "execution_source",
    ] {
        let mut changed = original.clone();
        changed[field] = json!("drift");
        write_json(&path, &changed);
        let before = snapshot(&fixture.run);
        let error = recovery::record_verified(&context, &verified)
            .unwrap_err()
            .to_string();
        assert!(error.contains("intent"), "{field}: {error}");
        assert_eq!(snapshot(&fixture.run), before);
    }
}

#[test]
fn recovery_selects_run_candidate_and_refuses_storage_product_and_adoption_drift() {
    let fixture = Fixture::new();
    let (mut context, _) = origin(&fixture);
    state::finish(&fixture.run, 1, None, Some("StageError")).unwrap();
    let sources =
        json!({"fingerprints":{"product":{"sha256":"a".repeat(64),"files":1}},"tool":"reconcile"});
    let (number, _) = state::begin(&fixture.run, "export", "reconcile", &sources).unwrap();
    context.attempt = number;
    context.mode = "reconcile".into();
    let path = fixture.run.join("export-0001.intent.json");
    let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    saved["source_storage"] = json!({"drift":true});
    write_json(&path, &saved);
    let before = snapshot(&fixture.run);
    let error = recovery::reconcile(&context).unwrap_err().to_string();
    assert!(error.contains("storage provenance"), "{error}");
    assert_eq!(snapshot(&fixture.run), before);
    saved["source_storage"] = Value::Null;
    saved["execution_source"]["fingerprints"]["product"]["sha256"] = json!("d".repeat(64));
    write_json(&path, &saved);
    // 修改原意图不能替换原 attempt 的来源，即使摘要形状合法。
    assert!(
        recovery::reconcile(&context)
            .unwrap_err()
            .to_string()
            .contains("intent")
    );
    let receipt = json!({"status":"export_reconciled","origin_attempt":1,
        "origin_controller":{},"origin_intent":{},"export":{},"source_request":{},
        "source_storage":null,"summary":{},"origin_execution_source":{},
        "reconciled_execution_source":{"tool":"forged"},"remote_writes":0,"restore_qualified":false});
    state::finish(&fixture.run, number, Some(&receipt), None).unwrap();
    let (number, _) = state::begin(&fixture.run, "export", "resume", &sources).unwrap();
    context.attempt = number;
    context.mode = "resume".into();
    let before = snapshot(&fixture.run);
    assert!(
        recovery::resume(&context)
            .unwrap_err()
            .to_string()
            .contains("采用结果")
    );
    assert_eq!(snapshot(&fixture.run), before);
}

#[test]
fn recovery_refuses_product_drift_and_ambiguous_complete_candidates_without_writes() {
    let fixture = Fixture::new();
    let (mut context, _) = origin(&fixture);
    state::finish(&fixture.run, 1, None, Some("StageError")).unwrap();
    let changed = json!({"fingerprints":{"product":{"sha256":"d".repeat(64),"files":1}}});
    let (number, _) = state::begin(&fixture.run, "export", "reconcile", &changed).unwrap();
    context.attempt = number;
    context.mode = "reconcile".into();
    let before = snapshot(&fixture.run);
    let error = recovery::reconcile(&context).unwrap_err().to_string();
    assert!(error.contains("产品源码指纹不同"), "{error}");
    assert_eq!(snapshot(&fixture.run), before);
    state::finish(&fixture.run, number, None, Some("StageError")).unwrap();
    let (second, _) = state::begin(&fixture.run, "export", "run", &changed).unwrap();
    let output = fixture.run.join(format!("e{second:04}"));
    fs::create_dir(&output).unwrap();
    write_json(&output.join("export.json"), &json!({"candidate":2}));
    state::finish(&fixture.run, second, Some(&json!({"published":true})), None).unwrap();
    let (number, _) = state::begin(&fixture.run, "export", "reconcile", &changed).unwrap();
    context.attempt = number;
    let before = snapshot(&fixture.run);
    let error = recovery::reconcile(&context).unwrap_err().to_string();
    assert!(error.contains("恰有一个"), "{error}");
    assert_eq!(snapshot(&fixture.run), before);
}
