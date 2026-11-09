use super::*;

fn identity_plan() -> Value {
    let tenants = (1..=10)
        .map(|n| json!({"tenant_id":format!("tenant_{n}")}))
        .collect::<Vec<_>>();
    let mut groups = vec![
        json!({"slot":"system","tenant_id":"system","kind":"system","count":100,"users":vec![json!({});100]}),
    ];
    for (index, tenant) in tenants.iter().enumerate() {
        groups.push(json!({"slot":format!("tenant-{:02}",index+1),"tenant_id":tenant["tenant_id"],"kind":"tenant","count":10,"users":vec![json!({});10]}));
    }
    json!({"groups":groups,"environment":{"tenants":tenants,"quota":{"system_max_users":150,"tenant_max_users":30,"import_headroom_per_tenant":5}}})
}
fn observed(max_users: u64, max_roles: u64, users: u64, roles: u64) -> Value {
    let usage = json!({"tenant_id":"tenant_1","users":{"used":users,"limit":max_users},"roles":{"used":roles,"limit":if max_roles==0 {Value::Null}else{json!(max_roles)}}});
    json!({"tenant":{"code":200,"data":{"tenant_id":"tenant_1","name":"租户一","domain":null,"expire_at":null,"status":"enabled",
        "max_users":max_users,"max_roles":max_roles,"max_storage_mb":0,"max_requests_per_min":0,"usage":usage,
        "expiration_status":"never","capacity_status":{"users":"available"}}},"usage":{"code":200,"data":usage}})
}

#[test]
fn fixed_identity_plan_keeps_eleven_targets_and_rejects_invalid_plan() {
    let plan = identity_plan();
    let result = targets(&plan).unwrap();
    assert_eq!(result.len(), 11);
    assert_eq!(
        result[0],
        json!({"slot":"system","tenant_id":"system","new_users":100,"import_headroom":0,"max_users":150})
    );
    assert_eq!(
        result[10],
        json!({"slot":"tenant-10","tenant_id":"tenant_10","new_users":10,"import_headroom":5,"max_users":30})
    );
    for case in [
        "missing-quota",
        "missing-tenant",
        "duplicate-tenant",
        "unicode-tenant",
        "wrong-group",
        "missing-user",
        "boolean-quota",
        "oversized-quota",
    ] {
        let mut invalid = plan.clone();
        match case {
            "missing-quota" => {
                invalid["environment"]
                    .as_object_mut()
                    .unwrap()
                    .remove("quota");
            }
            "missing-tenant" => {
                invalid["environment"]["tenants"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "duplicate-tenant" => {
                invalid["environment"]["tenants"][1]["tenant_id"] = json!("tenant_1")
            }
            "unicode-tenant" => invalid["environment"]["tenants"][0]["tenant_id"] = json!("租户"),
            "wrong-group" => invalid["groups"][1]["slot"] = json!("tenant-02"),
            "missing-user" => {
                invalid["groups"][1]["users"].as_array_mut().unwrap().pop();
            }
            "boolean-quota" => invalid["environment"]["quota"]["tenant_max_users"] = json!(true),
            _ => invalid["environment"]["quota"]["tenant_max_users"] = json!(2147483648_u64),
        }
        assert!(targets(&invalid).is_err(), "{case}");
    }
}

#[test]
fn complete_action_images_classify_before_after_and_unrelated_changes() {
    let target = targets(&identity_plan()).unwrap().remove(1);
    let before = observed(20, 5, 8, 2);
    let item = action(&target, &before).unwrap();
    let expected_before = json!({"tenant":{"tenant_id":"tenant_1","name":"租户一","domain":null,"expire_at":null,"status":"enabled",
        "max_users":20,"max_roles":5,"max_storage_mb":0,"max_requests_per_min":0},"users":8,"roles":2});
    let mut expected_after = expected_before.clone();
    expected_after["tenant"]["max_users"] = json!(30);
    assert_eq!(
        item,
        json!({"target":target,"before":expected_before,"after":expected_after,"observed":before})
    );
    assert_eq!(classification(&item, &before).unwrap(), "before");
    assert_eq!(
        classification(&item, &observed(30, 5, 8, 2)).unwrap(),
        "after"
    );
    let mut changed = observed(30, 5, 8, 2);
    changed["tenant"]["data"]["name"] = json!("另一个名称");
    assert_eq!(classification(&item, &changed).unwrap(), "mismatch");
    let unchanged = action(&target, &observed(30, 0, 8, 2)).unwrap();
    assert_eq!(
        classification(&unchanged, &observed(30, 0, 8, 2)).unwrap(),
        "after"
    );
}

#[test]
fn capacity_rejects_shrinking_headroom_role_exhaustion_and_overflow() {
    let target = targets(&identity_plan()).unwrap().remove(1);
    for (label, snapshot) in [
        ("shrinking", observed(31, 5, 8, 2)),
        ("headroom", observed(20, 5, 16, 2)),
        ("role-full", observed(20, 2, 8, 2)),
        ("users-overflow", observed(20, 0, u64::MAX, 2)),
        ("roles-overflow", observed(20, u64::MAX, 8, u64::MAX)),
    ] {
        assert!(action(&target, &snapshot).is_err(), "{label}");
    }
    assert!(action(&target, &observed(20, 0, 15, 100)).is_ok());
}

#[test]
fn raw_envelopes_and_matching_embedded_usage_are_required() {
    let target = targets(&identity_plan()).unwrap().remove(1);
    let valid = observed(20, 5, 8, 2);
    assert_eq!(data(&valid["usage"]).unwrap(), &valid["usage"]["data"]);
    for case in [
        "code",
        "usage",
        "limit",
        "boolean-used",
        "null-name",
        "numeric-domain",
        "extra-detail",
        "expiration",
    ] {
        let mut invalid = valid.clone();
        match case {
            "code" => invalid["tenant"]["code"] = json!(403),
            "usage" => invalid["usage"]["data"]["users"]["used"] = json!(9),
            "limit" => invalid["usage"]["data"]["roles"]["limit"] = Value::Null,
            "boolean-used" => {
                invalid["usage"]["data"]["roles"]["used"] = json!(true);
                invalid["tenant"]["data"]["usage"]["roles"]["used"] = json!(true);
            }
            "null-name" => invalid["tenant"]["data"]["name"] = Value::Null,
            "numeric-domain" => invalid["tenant"]["data"]["domain"] = json!(123),
            "extra-detail" => invalid["tenant"]["data"]["unexpected"] = json!(true),
            _ => invalid["tenant"]["data"]["expiration_status"] = json!("expired"),
        }
        assert!(action(&target, &invalid).is_err(), "{case}");
    }
}

#[test]
fn authorization_keeps_registered_system_admin_and_required_permissions() {
    let post =
        json!({"source_admin":{"subject_id":"42","tenant_id":"system","username":"private-admin"}});
    let auth = json!({"subject_id":"42","tenant_id":"system","username":"private-admin","is_super_admin":false,"permissions":["tenant:list","tenant:edit","tenant:usage:list"]});
    assert!(authorization(&post, &auth).is_ok());
    for case in [
        "subject",
        "tenant",
        "username",
        "permission",
        "string-super-admin",
    ] {
        let mut invalid = auth.clone();
        match case {
            "subject" => invalid["subject_id"] = json!("43"),
            "tenant" => invalid["tenant_id"] = json!("tenant_1"),
            "username" => invalid["username"] = json!("other-private-admin"),
            _ => {
                invalid["permissions"] = json!(["tenant:list"]);
                if case == "string-super-admin" {
                    invalid["is_super_admin"] = json!("true");
                }
            }
        }
        let error = authorization(&post, &invalid).unwrap_err().to_string();
        assert!(!error.contains("private-admin"));
    }
    let mut wildcard = auth;
    wildcard["permissions"] = json!(["*"]);
    assert!(authorization(&post, &wildcard).is_ok());
    wildcard["is_super_admin"] = json!(true);
    wildcard["permissions"] = json!([]);
    assert!(authorization(&post, &wildcard).is_ok());
    wildcard["permissions"] = Value::Null;
    assert!(authorization(&post, &wildcard).is_err());
}
