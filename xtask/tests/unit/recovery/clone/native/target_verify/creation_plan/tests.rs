use super::*;

#[test]
fn reset_output_binds_exact_raw_bytes_with_crlf_normalization() {
    let raw = "{\n  \"value\": 0, \"text\": \"中文\"\n}";
    let sha = hex::encode(Sha256::digest(raw.as_bytes()));
    let expected = json!({"manifest":{"value":0,"text":"中文"},"plan_hash":sha});
    for ending in ["", "\n"] {
        let value = format!("{raw}\nplan_hash={sha}{ending}");
        assert_eq!(reset_output(&value).unwrap(), expected);
        assert_eq!(
            reset_output(&value.replace('\n', "\r\n")).unwrap(),
            expected
        );
    }
}

#[test]
fn reset_output_rejects_tampering_duplicates_missing_hash_and_trailing_output() {
    let raw = "{\"value\":0}";
    let sha = hex::encode(Sha256::digest(raw.as_bytes()));
    let valid = format!("{raw}\nplan_hash={sha}\n");
    for bad in [
        raw.to_owned(),
        valid.replace("0}", "1}"),
        format!("prefix{valid}"),
        format!("{valid}extra"),
        format!("{valid}\n"),
        valid.replace(&sha, &sha.to_uppercase()),
    ] {
        assert!(reset_output(&bad).is_err(), "{bad}");
    }
    let duplicate = "{\"value\":0,\"value\":1}";
    assert!(
        reset_output(&format!(
            "{duplicate}\nplan_hash={}\n",
            hex::encode(Sha256::digest(duplicate))
        ))
        .is_err()
    );
}

#[test]
fn reset_configuration_requires_exact_source_stopped_flag_and_local_state_directory() {
    let target = crate::workspace::root_dir().join(".local-tests/target-cli-config");
    let maintenance = json!({"source":{"snapshot":{"head":"A".repeat(40)}}});
    let valid = json!({"reset":{"source_sha":"a".repeat(40),"services_stopped":true,"state_directory":target.join("reset-state")}});
    assert_eq!(
        reset_configuration(&target, &valid, &maintenance).unwrap()["path"],
        json!(target.join("reset-state"))
    );
    for (field, value, message) in [
        ("source_sha", json!("b".repeat(40)), DIAGNOSTICS[2]),
        ("services_stopped", json!("true"), DIAGNOSTICS[3]),
        ("services_stopped", json!(false), DIAGNOSTICS[3]),
        ("state_directory", json!("relative-state"), DIAGNOSTICS[5]),
        (
            "state_directory",
            json!(target.join("other-state")),
            DIAGNOSTICS[5],
        ),
        ("state_directory", Value::Null, DIAGNOSTICS[4]),
    ] {
        let mut changed = valid.clone();
        changed["reset"][field] = value;
        assert_eq!(
            reset_configuration(&target, &changed, &maintenance)
                .unwrap_err()
                .to_string(),
            message
        );
    }
    assert_eq!(
        reset_configuration(&target, &json!({"reset":true}), &maintenance)
            .unwrap_err()
            .to_string(),
        DIAGNOSTICS[1]
    );
}

#[test]
fn fresh_inventory_rejects_business_rows_wrong_slots_and_missing_fences() {
    let databases =
        json!([{"database":"control","kind":"combined"},{"database":"tenant","kind":"tenant"}]);
    let valid = json!({"control":{"tables":{"sys_user":{"rows":2},"biz_order":{"rows":0}}},
        "tenant":{"tables":{"biz_tenant_fence":{"rows":0},"biz_tenant_target_slot":{"rows":1},"biz_order":{"rows":0}}}});
    inventory(&databases, &valid).unwrap();
    for (path, value, message) in [
        ("/control/tables/biz_order/rows", json!(1), DIAGNOSTICS[7]),
        (
            "/tenant/tables/biz_tenant_fence/rows",
            json!(1),
            DIAGNOSTICS[8],
        ),
        (
            "/tenant/tables/biz_tenant_target_slot/rows",
            json!(0),
            DIAGNOSTICS[8],
        ),
        (
            "/tenant/tables/biz_order/rows",
            json!(false),
            DIAGNOSTICS[9],
        ),
    ] {
        let mut changed = valid.clone();
        *changed.pointer_mut(path).unwrap() = value;
        assert_eq!(
            inventory(&databases, &changed).unwrap_err().to_string(),
            message
        );
    }
    let mut missing = valid;
    missing["tenant"]["tables"]
        .as_object_mut()
        .unwrap()
        .remove("biz_tenant_fence");
    assert_eq!(
        inventory(&databases, &missing).unwrap_err().to_string(),
        DIAGNOSTICS[8]
    );
}
