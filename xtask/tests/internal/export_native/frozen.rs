use super::*;

#[test]
fn saved_schema_requires_the_original_ledgers_and_all_declared_columns() {
    let known = BTreeMap::from([(
        "sys_tenant".into(),
        BTreeMap::from([("tenant_id".into(), "VARCHAR".into())]),
    )]);
    let valid = json!({
        "sys_tenant":{"tenant_id":"VARCHAR"},
        "seaql_migrations":{"version":"VARCHAR"},
        "seaql_tenant_data_migrations":{"version":"VARCHAR"}
    });
    schema_columns(&valid, &known, true).unwrap();
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("seaql_migrations");
    assert!(schema_columns(&missing, &known, true).is_err());
    schema_columns(&missing, &known, false).unwrap();
    for (table, altered) in [
        ("sys_tenant", json!({"tenant_id":"BIGINT"})),
        ("seaql_migrations", json!({})),
        ("unexpected", json!({"tenant_id":"VARCHAR"})),
    ] {
        let mut changed = valid.clone();
        changed[table] = altered;
        assert!(schema_columns(&changed, &known, true).is_err(), "{table}");
    }
}

#[test]
fn saved_dump_rejects_missing_slot_even_if_the_original_inventory_declares_zero() {
    let catalog = BTreeMap::from([
        ("biz_tenant_target_slot".into(), BTreeMap::new()),
        ("biz_post".into(), BTreeMap::new()),
    ]);
    let counts = json!({"biz_tenant_target_slot":1,"biz_post":0});
    let inventory = json!([
        {"table":"biz_tenant_target_slot","rows":1},
        {"table":"biz_post","rows":0}
    ]);
    dump_tables(counts.as_object().unwrap(), &inventory, &catalog).unwrap();
    for counts in [
        json!({"biz_tenant_target_slot":1}),
        json!({"biz_tenant_target_slot":1,"biz_post":1}),
    ] {
        assert!(dump_tables(counts.as_object().unwrap(), &inventory, &catalog).is_err());
    }
    let slot_missing = json!({"biz_tenant_target_slot":0,"biz_post":0});
    let mut matching_inventory = inventory;
    matching_inventory[0]["rows"] = json!(0);
    assert!(
        dump_tables(
            slot_missing.as_object().unwrap(),
            &matching_inventory,
            &catalog
        )
        .is_err()
    );
}

#[test]
fn saved_generation_rejects_a_changed_original_process_even_when_other_receipts_agree() {
    let head = json!({"head":"a".repeat(40)});
    let build = json!({"kind":"devex-clone-tool-build","source":{"snapshot":head}});
    let maintenance = json!({"artifacts":{"migrate":"saved"}});
    let runtime = json!({"scope_id":"sample"});
    let api = json!({"identity":{"pid":51,"started":"1100","executable":"saved-api"}});
    let worker = json!({"identity":{"pid":52,"started":"1101","executable":"saved-worker"}});
    let generation = json!({
        "source":head,"runtime":runtime,"maintenance":maintenance,
        "processes":{"api":api["identity"],"worker":worker["identity"]}
    });
    receipts(&generation, &build, &maintenance, &runtime, &api, &worker).unwrap();
    let mut replaced = worker;
    replaced["identity"]["started"] = json!("1122");
    assert!(receipts(&generation, &build, &maintenance, &runtime, &api, &replaced).is_err());
}

#[test]
fn saved_logical_tenants_cannot_mask_the_original_scope_or_database_names() {
    let source = json!({
        "scope_id":"source_scope",
        "databases":[{"database":"control"},{"database":"private_db"}]
    });
    let identities = |name: &str| BTreeSet::from(["system".to_owned(), name.to_owned()]);
    tenant_identifiers(&identities("business-tenant"), &source).unwrap();
    for collision in ["source_scope", "control", "private_db"] {
        assert!(tenant_identifiers(&identities(collision), &source).is_err());
    }
}
