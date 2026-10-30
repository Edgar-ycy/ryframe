use super::*;
use native::export::{database::Models, inventory};

const PROJECTIONS: [&str; 3] = [
    "biz_data_scope_owner",
    "biz_data_scope_dept",
    "biz_data_scope_projection",
];

fn inputs(rows: u64, in_runtime: bool) -> (Value, Value, Value, Models) {
    let database = json!({
        "key":"dedicated-a", "kind":"tenant", "server_uuid":UUID,
        "database":"fixture", "shared":false, "placements":[],
        "tables":[
            {"table":"biz_order","rows":0,"sha256":"a".repeat(64)},
            {"table":"biz_tenant_target_slot","rows":1,"sha256":"b".repeat(64)}
        ]
    });
    let mut kept = PROJECTIONS
        .into_iter()
        .map(|table| json!({"table":table,"rows":rows,"sha256":"c".repeat(64)}))
        .collect::<Vec<_>>();
    kept.push(json!({"table":"ryframe_business_migration","rows":1,"sha256":"d".repeat(64)}));
    let objects = ["avatar", "config-packages", "exports", "imports", "uploads"]
        .map(|bucket| json!({"bucket":bucket,"prefix":"fixture-source/","entries":[]}));
    let mut runtime = json!({
        "scope_id":"fixture-source", "source_sha":"a".repeat(40),
        "quiesced_at":"2026-01-01T00:00:00Z", "captured_at":"2026-01-01T00:00:01Z",
        "control_schema_fingerprint":"a".repeat(16),
        "tenant_schema_fingerprint":"b".repeat(64), "databases":[],
        "objects":objects
    });
    if in_runtime {
        runtime["databases"] = json!([database]);
    }
    let target = json!({
        "scope_id":runtime["scope_id"],
        "control_schema_fingerprint":runtime["control_schema_fingerprint"],
        "tenant_schema_fingerprint":runtime["tenant_schema_fingerprint"],
        "target":{"database":database,"preserved_tables":kept}
    });
    let selected = json!({"scope_id":"fixture-source","databases":[{
        "key":"dedicated-a","kind":"tenant","server_uuid":UUID,
        "database":"fixture","mode":"dedicated"
    }]});
    let tenant = PROJECTIONS
        .into_iter()
        .chain(["biz_order", "biz_tenant_target_slot"])
        .map(|table| (table.to_owned(), Default::default()))
        .collect();
    let models = Models {
        control: Default::default(),
        tenant,
        full_control: Default::default(),
        full_tenant: Default::default(),
    };
    (runtime, target, selected, models)
}

fn validate(value: &Value, selected: &Value, models: &Models) -> crate::Result<()> {
    inventory::validate_selected(
        value,
        selected,
        models,
        &"a".repeat(40),
        "2026-01-01T00:00:00Z",
        false,
    )
}

#[test]
fn complete_inventory_checks_raw_business_before_adding_required_authorization_projections() {
    let (runtime, target, selected, models) = inputs(2, true);
    let original_runtime = runtime.clone();
    let original_target = target.clone();
    let complete = inventory::complete_selected(&runtime, vec![target.clone()], &selected).unwrap();
    validate(&complete, &selected, &models).unwrap();
    assert_eq!(runtime, original_runtime);
    assert_eq!(target, original_target);
    let tables = complete["databases"][0]["tables"].as_array().unwrap();
    for projection in PROJECTIONS {
        let row = tables
            .iter()
            .find(|row| row["table"] == projection)
            .unwrap();
        assert_eq!(row["rows"], 2);
        assert_eq!(row["sha256"], "c".repeat(64));
    }
    assert!(
        !tables
            .iter()
            .any(|row| row["table"] == "ryframe_business_migration")
    );
    let mut altered_runtime = runtime.clone();
    altered_runtime["databases"][0]["tables"]
        .as_array_mut()
        .unwrap()
        .push(target["target"]["preserved_tables"][0].clone());
    assert!(
        inventory::complete_selected(&altered_runtime, vec![target.clone()], &selected)
            .unwrap_err()
            .to_string()
            .contains("常规库存与逐目标实际库存不一致")
    );
    for drift in ["missing", "duplicate", "unknown"] {
        let mut altered = target.clone();
        let kept = altered["target"]["preserved_tables"]
            .as_array_mut()
            .unwrap();
        match drift {
            "missing" => {
                kept.remove(0);
            }
            "duplicate" => kept.push(kept[0].clone()),
            _ => altered["target"]["database"]["tables"]
                .as_array_mut()
                .unwrap()
                .push(json!({"table":"unknown","rows":0,"sha256":"e".repeat(64)})),
        }
        let result = inventory::complete_selected(&runtime, vec![altered], &selected);
        assert!(
            result
                .and_then(|value| validate(&value, &selected, &models))
                .is_err(),
            "{drift}"
        );
    }
}

#[test]
fn omitted_target_is_empty_only_when_its_authorization_projections_are_empty() {
    for rows in [0, 1] {
        let (runtime, target, selected, models) = inputs(rows, false);
        let result = inventory::complete_selected(&runtime, vec![target], &selected);
        if rows == 0 {
            let value = result.unwrap();
            validate(&value, &selected, &models).unwrap();
            assert_eq!(value["databases"][0]["tables"].as_array().unwrap().len(), 5);
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("常规库存遗漏了非空目标")
            );
        }
    }
}
