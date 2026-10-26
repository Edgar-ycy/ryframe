use super::*;

fn fixture() -> (Value, Value) {
    let subjects = json!(
        (0..11)
            .map(|index| json!({"tenant_id":format!("tenant-{index}"),
        "user_id":format!("{}",9007199254740993_u64+index),"user_authorization_version":1}))
            .collect::<Vec<_>>()
    );
    let plan =
        model::cache_plan(&json!({"namespace":"ryframe:{source-unit}:","subjects":subjects}))
            .unwrap();
    let before = json!(plan.as_array().unwrap().iter().enumerate().map(|(index,row)| {
        let snapshot = if index % 2 == 0 { json!({"key":row["keys"]["snapshot"],"type":"none"}) }
            else { json!({"key":row["keys"]["snapshot"],"type":"hash","field":"2:1","bytes":12,"sha256":"a".repeat(64)}) };
        json!({"tenant_id":row["tenant_id"],"user_id":row["user_id"],"user_authorization_version":row["user_authorization_version"],
            "tenant_epoch":{"key":row["keys"]["tenant_epoch"],"value":"2"},
            "user_version":{"key":row["keys"]["user_version"],"value":"1"},"snapshot":snapshot})
    }).collect::<Vec<_>>());
    let observation = json!({"namespace":"ryframe:{source-unit}:","subjects":subjects,"plan":plan,"before":before});
    let expired = expired(&before).unwrap();
    let receipt = json!({"format_version":2,"kind":"restore-source-authorization-cache-cleanup","status":"cleaned",
        "namespace":observation["namespace"],"before":before,"deleted":33-expired.as_array().unwrap().len(),
        "expired_snapshot_keys":expired,"after":keys(&plan).unwrap().iter().map(|key| json!({"key":key,"type":"none"})).collect::<Vec<_>>(),
        "attempt":{"path":std::env::current_dir().unwrap().join("attempt.json"),"bytes":1,"sha256":"b".repeat(64)},
        "execution_record":null,"attempt_phase":"normal","execution":"confirmed_watch_exec","cleaned_at":"2026-10-23T00:00:00+00:00"});
    (observation, receipt)
}

#[test]
fn cache_receipts_bind_all_subjects_expired_snapshots_and_exact_33_key_order() {
    let (observation, value) = fixture();
    let args = json!({"value":value,"subjects":observation["subjects"]});
    assert!(receipt(&args).is_ok());
    for (pointer, replacement) in [
        ("/value/deleted", json!(true)),
        ("/value/deleted", json!(0)),
        ("/value/before/1/snapshot/sha256", json!("invalid")),
        ("/value/before/1/snapshot/field", json!("3:1")),
        ("/value/before/0/tenant_epoch/value", json!("0")),
        ("/value/after/0/type", json!("string")),
        (
            "/value/expired_snapshot_keys/0",
            value["before"][1]["snapshot"]["key"].clone(),
        ),
        ("/value/cleaned_at", json!("private-secret-marker")),
    ] {
        let mut changed = args.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert!(receipt(&changed).is_err(), "{pointer}");
    }
    let mut reordered = args;
    reordered["value"]["after"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    assert!(receipt(&reordered).is_err());
}

#[test]
fn unknown_result_preserves_null_count_and_never_allows_mixed_state_replay() {
    let (observation, mut value) = fixture();
    value["execution"] = json!("reconciled_unknown_exec_result");
    value["deleted"] = Value::Null;
    assert!(receipt(&json!({"value":value,"subjects":observation["subjects"]})).is_ok());
    value["deleted"] = json!(27);
    assert!(receipt(&json!({"value":value,"subjects":observation["subjects"]})).is_err());
    let mut args = json!({"frozen":observation,"types":vec!["none";33],"observed":null,
        "attempt":{"status":"unknown_exec","observation":observation}});
    assert_eq!(resume(&args).unwrap()["action"], "publish");
    for status in ["pre_exec_failed", "watch_conflict", "not_attempted"] {
        args["attempt"]["status"] = json!(status);
        assert!(
            resume(&args).is_err(),
            "{status}: absence cannot invent unknown cleanup"
        );
    }
    args["attempt"]["status"] = json!("confirmed");
    assert_eq!(resume(&args).unwrap()["action"], "publish");
    args["types"][0] = json!("string");
    args["observed"] = observation.clone();
    for status in [
        "confirmed",
        "unknown_exec",
        "pre_exec_failed",
        "watch_conflict",
    ] {
        args["attempt"]["status"] = json!(status);
        assert!(
            resume(&args).is_err(),
            "{status}: equal values never authorize replay"
        );
    }
    args["attempt"]["status"] = json!("not_attempted");
    assert_eq!(resume(&args).unwrap()["action"], "execute");
    args["observed"]["before"][0]["tenant_epoch"]["value"] = json!("3");
    assert!(resume(&args).is_err());
    args["frozen"]["plan"][0]["keys"]["tenant_epoch"] = json!("unknown-key");
    assert!(resume(&args).is_err());
}

#[test]
fn cache_observation_rejects_unknown_fields_and_mismatched_subjects() {
    let (value, _) = fixture();
    let mut args = json!({"value":value,"subjects":value["subjects"]});
    assert!(observation(&args).is_ok());
    args["value"]["unexpected"] = json!(true);
    assert!(observation(&args).is_err());
    args["value"].as_object_mut().unwrap().remove("unexpected");
    args["subjects"][0]["user_id"] = json!("9");
    assert!(observation(&args).is_err());
}
