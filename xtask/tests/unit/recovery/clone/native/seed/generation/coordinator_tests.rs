use super::*;

fn record(number: u64, mode: &str, status: &str) -> Value {
    json!({"number":number,"stage":"seed-runtime","mode":mode,"status":status})
}

fn request_fixture() -> Value {
    json!({"format_version":1,"kind":"devex-clone-seed-source-generation","id":"same-seed",
        "source_registration":{"c52":true},"source_rebind":{"same":true},"review_successor":{"review":true},
        "current_storage":{"storage":true},"execution_backend":"execution","expected_backend_sha":"a".repeat(40),
        "adapter_contract":null,"product_backend":null,"backend_build":{},"maintenance_build":{},"source_environment":{}})
}
fn source_fixture(request: &Value) -> Value {
    json!({"review_successor":{"source_result":request["source_registration"]},
        "source_rebind":request["source_rebind"],"review_successor_binding":request["review_successor"],
        "storage":{"storage":request["current_storage"]},"source_generation":null})
}
fn call(action: &str, input: Value) -> Result<Value> {
    dispatch(&json!({"action":action,"input":input}))
}

#[test]
fn preflight_never_replays_running_failed_or_successful_attempts() {
    for status in ["running", "failed", "passed"] {
        for mode in [START, STOP, RECOVER, "source-export", "arm-input"] {
            assert_eq!(
                call(
                    "preflight-plan",
                    json!({"attempts":[record(54,mode,status)],"archive":null})
                )
                .unwrap(),
                reject("replay")
            );
        }
    }
    for invalid in [json!(true), json!(0), json!("54")] {
        let mut row = record(54, START, "failed");
        row["number"] = invalid;
        assert!(call("preflight-plan", json!({"attempts":[row],"archive":null})).is_err());
    }
}

#[test]
fn audited_resume_requires_complete_resources_exact_request_and_unique_rebind() {
    let rebound = record(53, "source-rebind", "passed");
    let failed = record(54, START, "failed");
    let plan = json!({"attempts":[rebound,failed],"archive":{"records":[]}});
    assert_eq!(
        call("preflight-plan", plan.clone()).unwrap(),
        json!({"needs_segment":true})
    );
    let valid = json!({"attempts":plan["attempts"],"archive":plan["archive"],
        "segment":{"phase":"ready","storage":{},"cache":{},"records":[failed],"replay":{"request":{"sha256":"same"}}},
        "request":{"sha256":"same"},"current_request":{"sha256":"same"}});
    assert_eq!(
        call("preflight-finish", valid.clone()).unwrap(),
        json!({"ready":true})
    );
    for (pointer, value, reason) in [
        ("/segment/cache", Value::Null, "resume"),
        ("/segment/storage", Value::Null, "resume"),
        ("/segment/phase", json!("storage-ready"), "resume"),
        ("/segment/records", json!([]), "resume"),
        (
            "/current_request",
            json!({"sha256":"changed"}),
            "replay_request",
        ),
        ("/request", Value::Null, "replay_request"),
        ("/attempts/0/status", json!("failed"), "rebind"),
    ] {
        let mut changed = valid.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            call("preflight-finish", changed).unwrap(),
            reject(reason),
            "{pointer}"
        );
    }
    let mut changed = valid.clone();
    changed["attempts"]
        .as_array_mut()
        .unwrap()
        .push(record(55, START, "failed"));
    assert_eq!(call("preflight-finish", changed).unwrap(), reject("replay"));
}

#[test]
fn inputs_bind_only_current_running_start_and_preserve_full_prefix() {
    let request = request_fixture();
    let valid = json!({"request":request,"source":source_fixture(&request),
        "attempts":[record(53,"source-rebind","passed"),record(54,START,"running")],"number":54});
    assert_eq!(
        call("inputs", valid.clone()).unwrap(),
        json!({"prefix":[record(53,"source-rebind","passed")]})
    );
    for field in [
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
    ] {
        let mut changed = valid.clone();
        changed["request"][field] = json!({"drift":true});
        assert_eq!(call("inputs", changed).unwrap(), reject("source"));
    }
    for value in [json!(false), json!(1.0), json!("1")] {
        let mut changed = valid.clone();
        changed["request"]["format_version"] = value;
        assert!(call("inputs", changed).is_err());
    }
    for status in ["passed", "failed"] {
        let mut changed = valid.clone();
        changed["attempts"][1]["status"] = json!(status);
        assert_eq!(call("inputs", changed).unwrap(), reject("current"));
    }
}

fn start_fixture() -> (Value, Value) {
    let request = request_fixture();
    let prefix = json!([record(53, "source-rebind", "passed")]);
    let args = json!({"request":request,"descriptor":{"file":"request"},"coordinator_source":{"clean":true},
        "prefix":prefix,"generation_id":"g0054","bindings":{"intent":{},"runtime_evidence":{},"before":{},"running":{},"dataset_lineage":{}}});
    let receipt = call("start-receipt", args).unwrap();
    let validation = json!({"receipt":receipt,"request":request,"request_copy":request,"source":source_fixture(&request),
        "prefix":prefix,"coordinator_source":{"clean":true},"generation_id":"g0054",
        "attempts":[record(53,"source-rebind","passed"),record(54,START,"passed")],"number":54,"live":false});
    (receipt, validation)
}

#[test]
fn running_receipt_freezes_history_tools_request_and_strict_zero_write_fields() {
    let (_, valid) = start_fixture();
    assert_eq!(
        call("running", valid.clone()).unwrap(),
        json!({"verified":true})
    );
    for pointer in [
        "/receipt/coordinator_source",
        "/receipt/history_sha256",
        "/receipt/generation_id",
        "/receipt/source_registration",
        "/request_copy",
        "/receipt/history_length",
        "/receipt/remote_writes",
        "/receipt/restore_qualified",
    ] {
        let mut changed = valid.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("drift");
        assert_eq!(
            call("running", changed).unwrap(),
            reject("running"),
            "{pointer}"
        );
    }
    let mut changed = valid.clone();
    changed["receipt"]["remote_writes"] = json!(false);
    assert_eq!(call("running", changed).unwrap(), reject("running"));
    let mut changed = valid.clone();
    changed["receipt"]["extra"] = json!(true);
    assert!(call("running", changed).is_err());
}

#[test]
fn online_verification_rejects_later_control_and_preserves_historical_validation() {
    let (_, valid) = start_fixture();
    for mode in [STOP, RECOVER] {
        for status in ["failed", "passed"] {
            let mut changed = valid.clone();
            changed["attempts"]
                .as_array_mut()
                .unwrap()
                .push(record(55, mode, status));
            call("running", changed.clone()).unwrap();
            changed["live"] = json!(true);
            assert_eq!(call("running", changed).unwrap(), reject("stopped"));
        }
    }
}

#[test]
fn source_request_keeps_original_physical_target_and_only_replaces_generation_bindings() {
    let request = request_fixture();
    let original =
        json!({"source":{"database":"same","runtime_dir":"old"},"other":{"retained":true}});
    let args = json!({"source_request":original,"request":request,"fingerprint":"fingerprint","runtime":{},
        "processes":{"api":{},"worker":{}},"producers_registry":{},"runtime_dir":"new"});
    let value = call("source-request", args).unwrap();
    assert_eq!(value["source"]["database"], "same");
    assert_eq!(value["source"]["runtime_dir"], "new");
    assert_eq!(value["other"], original["other"]);
    assert_eq!(original["source"]["runtime_dir"], "old");
}

#[test]
fn complete_images_compare_nested_data_without_boolean_integer_coercion() {
    let before =
        json!({"tables":{"rows":[false,0]},"objects":{"metadata":{"owner":"same"}},"redis":{}});
    assert_eq!(
        call("equal-images", json!({"before":before,"after":before})).unwrap(),
        json!({"equal":true})
    );
    for pointer in ["/tables/rows/0", "/objects/metadata/owner", "/redis"] {
        let mut after = before.clone();
        *after.pointer_mut(pointer).unwrap() = json!(1);
        assert_eq!(
            call("equal-images", json!({"before":before,"after":after})).unwrap(),
            json!({"equal":false})
        );
    }
}

#[test]
fn stop_receipt_cannot_replace_dataset_start_or_complete_history() {
    let (start, running) = start_fixture();
    let mut receipt = start.clone();
    for key in ["generation_id", "intent", "coordinator_source"] {
        receipt.as_object_mut().unwrap().remove(key);
    }
    for key in [
        "source_request",
        "generation_verified",
        "after",
        "start",
        "source_runtime",
        "stop_before",
    ] {
        receipt[key] = json!({"file":key});
    }
    receipt["status"] = json!("seed_source_generation_published");
    let args = json!({"receipt":receipt,"request":running["request"],"source":running["source"],
        "descriptor":running["request"]["source_registration"],"prefix":running["prefix"],
        "source_receipt":start,"verified_source":running["source"],
        "verified_receipt":{"source_generation":receipt["start"],"dataset_lineage":receipt["dataset_lineage"]},
        "before":start["before"],"running":start["running"]});
    assert_eq!(
        call("published", args.clone()).unwrap(),
        json!({"verified":true})
    );
    // source-runtime 已建立完整请求事实；消费边界只需要本阶段使用的四个绑定。
    let mut consumed = args.clone();
    consumed["request"] = [
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
    ]
    .into_iter()
    .map(|key| (key.to_owned(), args["request"][key].clone()))
    .collect::<serde_json::Map<_, _>>()
    .into();
    assert_eq!(
        call("published", consumed).unwrap(),
        json!({"verified":true})
    );
    for (pointer, reason) in [
        ("/source_receipt/request", "dataset"),
        ("/verified_source/source_rebind", "dataset"),
        ("/receipt/history_sha256", "published"),
        ("/receipt/source_registration", "published"),
        ("/verified_receipt/source_generation", "lineage"),
        ("/receipt/before", "lineage"),
    ] {
        let mut changed = args.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("drift");
        assert_eq!(
            call("published", changed).unwrap(),
            reject(reason),
            "{pointer}"
        );
    }
}

#[test]
fn generation_proof_preserves_exact_artifacts_physical_binding_and_sorted_producers() {
    let effective = json!({"worktree_fingerprint":"same","source":{"database":"same"}});
    let generation = json!({"request_sha256":digest::plan_hash(&effective).unwrap(),
        "runtime":{"same":true},"source":{"head":"same"},"worktree_fingerprint":"same",
        "maintenance":{"same":true},"processes":{"api":{},"worker":{}},
        "operator_declared_producers":[{"name":"a"},{"name":"z"}],"physical_binding":{"same":true},
        "external_writers_discovered":false,"clone_verified":false,"restore_qualified":false});
    let args = json!({"effective":effective,"runtime":generation["runtime"],
        "source_snapshot":generation["source"],"maintenance":generation["maintenance"],
        "processes":generation["processes"],"producers":[{"name":"z"},{"name":"a"}],
        "physical_binding":generation["physical_binding"],"generation":generation,
        "runtime_physical":generation["physical_binding"]});
    assert_eq!(
        call("generation-proof", args.clone()).unwrap(),
        json!({"verified":true})
    );
    for field in generation.as_object().unwrap().keys() {
        let mut changed = args.clone();
        changed["generation"][field] = json!("drift");
        assert_eq!(
            call("generation-proof", changed).unwrap(),
            reject("proof"),
            "{field}"
        );
    }
    let mut changed = args.clone();
    changed["runtime_physical"] = json!({"same":false});
    assert_eq!(call("generation-proof", changed).unwrap(), reject("proof"));
}

#[test]
fn selection_never_adopts_an_older_success_after_failed_replay() {
    for (action, mode, reason) in [
        ("select-rebind", "source-rebind", "rebind"),
        ("select-start", START, "start"),
        ("select-published", STOP, "duplicate"),
    ] {
        let one = record(54, mode, "passed");
        let value = call(action, json!({"attempts":[one]})).unwrap();
        assert_eq!(value["record"], one);
        for status in ["failed", "running", "passed"] {
            let value = call(action, json!({"attempts":[one,record(55,mode,status)]})).unwrap();
            assert_eq!(value, reject(reason));
        }
    }
    for status in ["failed", "running"] {
        let value = call(
            "select-published",
            json!({"attempts":[record(54,STOP,status)]}),
        )
        .unwrap();
        assert_eq!(value, json!({"record":null,"prefix":[]}));
    }
}
