use std::collections::BTreeMap;

use serde_json::{Value, json};

pub(super) fn config(count: usize, tenants: usize, system: bool, selections: &[usize]) -> Value {
    let slots = (0..count)
        .map(|index| {
            json!({
                "tenant_slot": if system { "system".to_owned() } else {
                    format!("tenant-{}", index % tenants + 1)
                },
                "user_slot": format!("user-{}", index + 1),
                "client_address": format!("198.18.{}.{}", index / 250 + 10, index % 250 + 1),
            })
        })
        .collect::<Vec<_>>();
    let bindings = slots
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            json!({
                "tenant_id": if system { "system".to_owned() } else {
                    format!("actual-{}", slot["tenant_slot"].as_str().expect("逻辑租户"))
                },
                "username": format!("user-{}", index + 1),
                "password": "RYFRAME_SELECTION_TEST_PASSWORD",
            })
        })
        .collect::<Vec<_>>();
    let selections = selections
        .iter()
        .map(|count| {
            let mut distribution = BTreeMap::<String, u64>::new();
            for slot in slots.iter().take(*count) {
                *distribution
                    .entry(slot["tenant_slot"].as_str().expect("逻辑租户").into())
                    .or_default() += 1;
            }
            (count.to_string(), json!(distribution))
        })
        .collect::<BTreeMap<_, _>>();
    json!({
        "contract": {
            "identity_pools": {"readers": {
                "roles_sha256": "a".repeat(64), "permissions_sha256": "b".repeat(64),
                "client_address_model": "fixed-benchmark-per-user", "slots": slots,
                "selections": selections,
            }},
            "workloads": {"api": [{"name": "list", "identity_pool": "readers", "steps": []}]},
        },
        "bindings": {"identity_pools": {"readers": bindings}},
    })
}

pub(super) fn scenario(mut config: Value, suite: &str, name: &str) -> Value {
    config["contract"]["workloads"] = json!({suite: [{"name": name, "identity_pool": "readers"}]});
    config
}

pub(super) fn changed(config: &Value, pointer: &str, value: Value) -> Value {
    let mut config = config.clone();
    *config.pointer_mut(pointer).expect("夹具字段必须存在") = value;
    config
}

pub(super) fn jobs_config(count: usize) -> Value {
    let imports = config(count, 10, false, &[count]);
    let system = config(count, 1, true, &[count]);
    json!({
        "contract": {
            "cycles": 1,
            "identity_pools": {
                "imports": imports["contract"]["identity_pools"]["readers"],
                "system": system["contract"]["identity_pools"]["readers"],
            },
            "workloads": {"jobs": [
                {"name": "import", "identity_pool": "imports", "job": {"kind": "import"}},
                {"name": "schedule", "identity_pool": "system", "job": {"kind": "schedule"}},
            ]},
        },
        "bindings": {"identity_pools": {
            "imports": imports["bindings"]["identity_pools"]["readers"],
            "system": system["bindings"]["identity_pools"]["readers"],
        }},
    })
}

pub(super) fn home_config(count: usize) -> Value {
    let mut config = config(count, 1, false, &[count]);
    config["contract"]["cycles"] = json!(count);
    config["contract"]["homepage"] =
        json!({"identity_pool": "readers", "visitor_interval_ms": 250});
    config
}
