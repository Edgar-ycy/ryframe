use super::*;

fn call(action: &str, input: Value) -> Value {
    dispatch(&json!({"action":action,"input":input})).unwrap()
}
fn failed() -> Value {
    json!({"number":68,"stage":"seed-runtime","mode":"source-generation-start",
        "status":"failed","result":null,"error_type":"ValueError"})
}
fn request() -> Value {
    json!({"format_version":1,"kind":"devex-clone-seed-source-generation","id":"fixed-request",
        "source_registration":{"path":"registration"},"source_rebind":{"path":"rebind"},
        "review_successor":{"path":"successor"},"current_storage":{"attempt":65},
        "execution_backend":"execution","expected_backend_sha":"a".repeat(40),
        "adapter_contract":null,"product_backend":null,"backend_build":{"path":"old-runtime"},
        "maintenance_build":{"path":"old-maintenance"},"source_environment":{"path":"environment"}})
}
fn source(request: &Value) -> Value {
    json!({"directory":"run","review_successor":{"source_result":request["source_registration"]},
        "review_successor_binding":request["review_successor"],"source_rebind":request["source_rebind"],
        "storage":{"storage":request["current_storage"]},"source_generation":null})
}
fn product() -> Value {
    json!({"sha256":"a".repeat(64),"files":1})
}
fn clean_source() -> Value {
    json!({"snapshot":{"clean":true,"files":[],"patch_sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},"fingerprints":{"product":product()}})
}
fn assert_rejected(action: &str, input: Value, message: &str) {
    let result = call(action, input);
    assert!(
        result["rejected"]
            .as_str()
            .is_some_and(|text| text.contains(message)),
        "{result}"
    );
    assert!(!result.to_string().contains("secret-credential"));
}

#[test]
fn pending_and_failed_tail_require_exact_c68_without_authorizing_replay() {
    assert_eq!(
        call("pending", json!({"attempts":[failed()],"previous":null})),
        json!({"pending":true})
    );
    assert_eq!(
        call("pending", json!({"attempts":[failed()],"previous":67})),
        json!({"pending":true})
    );
    for (field, value) in [
        ("number", json!(69)),
        ("number", json!(68.0)),
        ("number", json!(true)),
        ("stage", json!("other")),
        ("mode", json!("source-generation-stop")),
        ("status", json!("running")),
        ("result", json!({"published":true})),
        ("error_type", json!("RuntimeError")),
    ] {
        let mut row = failed();
        row[field] = value;
        for previous in [json!(null), json!(67)] {
            assert_eq!(
                call("pending", json!({"attempts":[row],"previous":previous}))["pending"],
                false
            );
        }
    }
    for previous in [json!(66), json!(67.0), json!(true)] {
        assert_eq!(
            call(
                "pending",
                json!({"attempts":[failed()],"previous":previous})
            )["pending"],
            false
        );
    }
    assert_eq!(
        call("pending", json!({"attempts":[],"previous":null}))["pending"],
        false
    );
}

fn history_fixture() -> Value {
    let archive = json!({"number":60,"sources":clean_source()});
    let shapes = [
        ("cache-target", "stop", "passed", Value::Null),
        ("storage-target", "stop", "failed", json!("ValueError")),
        ("storage-target", "stop", "passed", Value::Null),
        (
            "storage-target",
            "restart",
            "failed",
            json!("CalledProcessError"),
        ),
        ("storage-target", "restart", "passed", Value::Null),
        ("cache-target", "restart", "passed", Value::Null),
        (
            "seed-runtime",
            "source-generation-start",
            "failed",
            json!("ValueError"),
        ),
        (
            "seed-runtime",
            "source-generation-start",
            "failed",
            json!("ValueError"),
        ),
    ];
    let mut rows = vec![archive.clone()];
    for (index, (stage, mode, status, error)) in shapes.into_iter().enumerate() {
        rows.push(
            json!({"number":61+index,"stage":stage,"mode":mode,"status":status,"error_type":error,
            "result":null,"sources":clean_source()}),
        );
    }
    json!({"attempts":rows,"archive":{"recovery":archive}})
}

#[test]
fn fixed_history_rejects_missing_duplicate_failed_output_and_product_drift() {
    let args = history_fixture();
    assert_eq!(
        call("history", args.clone())["tail"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let mut changed = args.clone();
    changed["archive"] = Value::Null;
    assert_rejected("history", changed, "C60");
    for index in 1..9 {
        for (field, value) in [
            ("number", json!(100)),
            ("status", json!("running")),
            ("mode", json!("other")),
        ] {
            let mut changed = args.clone();
            changed["attempts"][index][field] = value;
            assert_rejected("history", changed, "固定相邻");
        }
        for (field, value) in [
            ("clean", json!(false)),
            ("clean", json!(1)),
            ("files", json!(["x"])),
            ("patch_sha256", json!("changed")),
        ] {
            let mut changed = args.clone();
            changed["attempts"][index]["sources"]["snapshot"][field] = value;
            assert_rejected("history", changed, "产品来源");
        }
    }
    let mut changed = args.clone();
    changed["attempts"][8]["result"] = json!({"unknown_write":true});
    assert_rejected("history", changed, "意外发布");
    let mut changed = args.clone();
    changed["attempts"][8]["sources"]["fingerprints"]["product"] =
        json!({"secret-credential":true});
    assert_rejected("history", changed, "产品来源");
    let mut changed = args;
    changed["attempts"].as_array_mut().unwrap().pop();
    assert_rejected("history", changed, "固定相邻");
}

#[test]
fn paired_external_request_changes_only_runtime_and_maintenance_receipts() {
    let previous = request();
    let mut current = previous.clone();
    current["backend_build"] = json!({"path":"current-runtime"});
    current["maintenance_build"] = json!({"path":"current-maintenance"});
    assert_eq!(
        call(
            "request",
            json!({"previous":previous,"request":current,"external":true})
        ),
        json!({"reregister":true})
    );
    assert_eq!(
        call(
            "request",
            json!({"previous":previous,"request":previous,"external":false})
        ),
        json!({"reregister":false})
    );
    for key in previous.as_object().unwrap().keys() {
        let mut changed = previous.clone();
        changed[key] = json!("secret-credential");
        assert_rejected(
            "request",
            json!({"previous":previous,"request":changed,"external":true}),
            if ["format_version", "kind"].contains(&key.as_str()) {
                "失败代次"
            } else {
                "成对"
            },
        );
    }
    assert_rejected(
        "request",
        json!({"previous":previous,"request":previous,"external":true}),
        "成对",
    );
    assert_rejected(
        "request",
        json!({"previous":previous,"request":current,"external":false}),
        "读取期间变化",
    );
    let mut changed = current;
    changed["id"] = json!("other");
    assert_rejected(
        "request",
        json!({"previous":previous,"request":changed,"external":true}),
        "成对",
    );
}

#[test]
fn source_request_binds_first_generation_to_same_registration_rebind_and_storage() {
    let request = request();
    let source = source(&request);
    assert_eq!(
        call("request-source", json!({"request":request,"source":source}))["verified"],
        true
    );
    for field in [
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
    ] {
        let mut changed = request.clone();
        changed[field] = json!({"other":true});
        assert_rejected(
            "request-source",
            json!({"request":changed,"source":source}),
            "首个正式",
        );
    }
    let mut changed = source;
    changed["source_generation"] = json!({"already_published":true});
    assert_rejected(
        "request-source",
        json!({"request":request,"source":changed}),
        "首个正式",
    );
}

#[test]
fn source_selection_requires_unique_successful_registration_and_rebind() {
    let request = request();
    let registered = json!({"number":52,"stage":"seed-runtime","mode":"source-register","status":"passed","result":request["source_registration"]});
    let rebound = json!({"number":56,"stage":"seed-runtime","mode":"source-rebind","status":"passed","result":request["source_rebind"]});
    let args = json!({"attempts":[registered,rebound],"request":request});
    let selected = call("select-source", args);
    assert_eq!(selected["rebind"], rebound);
    assert_eq!(selected["prefix"], json!([registered]));
    for rows in [
        json!([]),
        json!([registered]),
        json!([registered, registered, rebound]),
        json!([registered, rebound, rebound]),
    ] {
        assert_rejected(
            "select-source",
            json!({"attempts":rows,"request":request}),
            "唯一 C52",
        );
    }
}

#[test]
fn source_model_rejects_generation_changes_and_frozen_successor_drift() {
    let request = request();
    let source = source(&request);
    let args = json!({"source":source,"directory":"run","request":request,
        "rebound":{"review_successor":request["review_successor"],"original_storage":request["current_storage"]},
        "storage":request["current_storage"],"cache":{"attempt":66,"restart_result":{"fixed":true}},
        "cache_record":{"number":66,"result":{"fixed":true}}});
    assert_eq!(call("source", args.clone())["verified"], true);
    for field in ["storage", "cache"] {
        let mut changed = args.clone();
        changed[field] = Value::Null;
        assert_rejected("source", changed, "C65 RustFS");
        let mut changed = args.clone();
        changed[field]["attempt"] = json!(100);
        assert_rejected("source", changed, "C65 RustFS");
    }
    let mut changed = args.clone();
    changed["cache"]["restart_result"] = json!({"different":true});
    assert_rejected("source", changed, "C66 Redis");
    let mut changed = args.clone();
    changed["source"]["directory"] = json!("another");
    assert_rejected("source", changed, "原 C52");
    let mut changed = args;
    changed["rebound"]["review_successor"] = json!({"different":true});
    assert_rejected("source", changed, "完整历史");
}

fn failure_fixture() -> Value {
    let frames: Vec<_>=[("devex_clone_run","execute",717),("devex_clone_seed_runtime","execute_seed",341),
        ("devex_clone_seed_generation","execute_generation",369),("restore_source_lineage","derive_dataset_lineage",405),
        ("restore_source_lineage","_dataset_facts",107),("restore_reference_plan","validate_plan",68)].iter()
        .map(|(file,function,line)|json!({"file":format!("scripts/{file}.py"),"function":function,"line":line})).collect();
    json!({"controller":{"fixed":true},"failure":{"format_version":1,"kind":"devex-stage-failure","attempt":68,
        "stage":"seed-runtime","mode":"source-generation-start","error_type":"ValueError","frames":frames,"controller":{"fixed":true}}})
}

#[test]
fn fixed_failure_stack_rejects_start_positions_types_and_controller_changes() {
    let args = failure_fixture();
    assert_eq!(call("failure", args.clone())["verified"], true);
    for (field, value) in [
        ("format_version", json!(true)),
        ("attempt", json!(68.0)),
        ("controller", json!({"other":true})),
        ("error_type", json!("secret-credential")),
        ("extra", json!(true)),
    ] {
        let mut changed = args.clone();
        changed["failure"][field] = value;
        assert_rejected("failure", changed, "失败栈");
    }
    for index in 0..6 {
        let mut changed = args.clone();
        changed["failure"]["frames"][index]["line"] = json!(1);
        assert_rejected("failure", changed, "失败栈");
    }
}

#[test]
fn intent_rejects_duplicate_roles_unknown_fields_and_loose_scalars() {
    let args = json!({"intent":{"format_version":1,"kind":"seed-source-runtime-intent","request":{},
        "runtime_directory":"runtime","operations":{"api":"0".repeat(32),"worker":"1".repeat(32)}},"request":{},"runtime":"runtime"});
    assert!(call("intent", args.clone())["operations"].is_object());
    for (field, value) in [
        ("format_version", json!(true)),
        ("request", json!({"changed":true})),
        ("runtime_directory", json!("elsewhere")),
    ] {
        let mut changed = args.clone();
        changed["intent"][field] = value;
        assert_rejected("intent", changed, "启动意图");
    }
    let mut changed = args.clone();
    changed["intent"]["operations"]["worker"] = json!("0".repeat(32));
    assert_rejected("intent", changed, "启动意图");
    let mut changed = args;
    changed["intent"]["operations"]["extra"] = json!("secret-credential");
    assert_rejected("intent", changed, "operation");
}

#[test]
fn replay_plan_enforces_unique_c69_and_immediately_adjacent_c70() {
    let record = json!({"number":69,"stage":"seed-runtime","mode":"source-generation-recover","status":"passed","error_type":null,"result":{}});
    let rows = json!([failed(), record]);
    assert_eq!(
        call("replay-plan", json!({"attempts":rows}))["record"],
        record
    );
    assert_eq!(
        call("replay-plan", json!({"attempts":[failed()]}))["record"],
        Value::Null
    );
    assert_rejected(
        "replay-plan",
        json!({"attempts":[failed(),record,record]}),
        "不能重复",
    );
    for (field, value) in [
        ("number", json!(70.0)),
        ("number", json!(71)),
        ("stage", json!("other")),
        ("mode", json!("source-generation-stop")),
    ] {
        let mut next = json!({"number":70,"stage":"seed-runtime","mode":"source-generation-start"});
        next[field] = value;
        assert_rejected(
            "replay-plan",
            json!({"attempts":[failed(),record,next]}),
            "C69 后首个阶段",
        );
    }
    for status in ["failed", "running"] {
        let mut changed = record.clone();
        changed["status"] = json!(status);
        let plan = call("replay-plan", json!({"attempts":[failed(),changed]}));
        assert_eq!(plan["fallback"], true);
        assert_eq!(plan["record"].is_null(), status == "running");
    }
}

fn receipt_fixture() -> Value {
    let mut failed = failed();
    failed["sources"] = clean_source();
    json!({"facts":{"request":request(),"request_descriptor":{"path":"request"},"proof":{"same":true},
        "before":{"path":"before"},"failed":failed},"prefix":[failed],"coordinator":clean_source(),
        "runtime":{"path":"runtime"},"after":{"path":"after"},"manifest":{"files":2,"bytes":100,"sha256":"b".repeat(64)}})
}

#[test]
fn receipt_recalculation_binds_history_proof_product_and_zero_write_flags() {
    let args = receipt_fixture();
    let mut fields = args.clone();
    fields["facts"].as_object_mut().unwrap().remove("failed");
    let receipt = call("receipt", fields)["receipt"].clone();
    let record = json!({"sources":args["coordinator"]});
    let verify =
        json!({"receipt":receipt,"facts":args["facts"],"prefix":args["prefix"],"record":record});
    assert_eq!(call("verify-receipt", verify.clone())["verified"], true);
    for (field, value) in [
        ("failed_start", json!(68.0)),
        ("history_length", json!(1.0)),
        ("remote_writes", json!(false)),
        ("source_generation_published", json!(0)),
        ("replay_allowed", json!(1)),
        ("restore_qualified", json!(0)),
        ("replay_allowed", json!(false)),
        ("failure_proof", json!({"changed":true})),
        ("history_sha256", json!("wrong")),
    ] {
        let mut changed = verify.clone();
        changed["receipt"][field] = value;
        assert!(call("verify-receipt", changed)["rejected"].is_string());
    }
    let mut changed = verify.clone();
    changed["prefix"].as_array_mut().unwrap().push(failed());
    assert_rejected("verify-receipt", changed, "完整历史");
    let mut changed = verify;
    changed["record"]["sources"]["fingerprints"]["product"] = json!({"changed":true});
    assert_rejected("verify-receipt", changed, "产品来源");
}

#[test]
fn authorization_stage_never_accepts_nonadjacent_or_changed_product() {
    let args = json!({"number":69,"prefix":[failed()]});
    assert_eq!(call("authorize-stage", args.clone())["verified"], true);
    for number in [json!(68), json!(70), json!(69.0), json!(true)] {
        let mut changed = args.clone();
        changed["number"] = number;
        assert_rejected("authorize-stage", changed, "唯一相邻");
    }
    let args = json!({"coordinator":clean_source(),"failed":{"sources":clean_source()}});
    assert_eq!(call("authorize-product", args.clone())["verified"], true);
    let mut changed = args;
    changed["coordinator"]["fingerprints"]["product"] = json!({"changed":true});
    assert_rejected("authorize-product", changed, "产品来源");
}

#[test]
fn reregister_request_preserves_frozen_inputs_and_requires_both_new_builds() {
    let previous = request();
    let unchanged: serde_json::Map<String, Value> = [
        "id",
        "review_successor",
        "execution_backend",
        "expected_backend_sha",
        "adapter_contract",
        "product_backend",
        "source_environment",
    ]
    .iter()
    .map(|key| ((*key).to_owned(), previous[key].clone()))
    .collect();
    let args = json!({"previous":previous,"unchanged":unchanged,"backend_build":{"path":"new-runtime"},"maintenance_build":{"path":"new-maintenance"}});
    let rebuilt = call("reregister-request", args.clone())["request"].clone();
    assert_eq!(rebuilt["backend_build"], args["backend_build"]);
    assert_eq!(
        call(
            "request",
            json!({"previous":previous,"request":rebuilt,"external":true})
        )["reregister"],
        true
    );
    for field in unchanged.keys() {
        let mut changed = args.clone();
        changed["unchanged"][field] = json!("secret-credential");
        assert_rejected("reregister-request", changed, "构建收据以外");
    }
    for field in ["backend_build", "maintenance_build"] {
        let mut changed = args.clone();
        changed[field] = previous[field].clone();
        assert_rejected("reregister-request", changed, "成对");
    }
}
