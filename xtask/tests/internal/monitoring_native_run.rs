use std::fs;

use serde_json::{Value, json};

use super::{Fixture, descriptor, write_json};
use crate::recovery::monitoring::contract;

pub(super) fn prepare_complete_run(fixture: &Fixture) {
    let binding_receipt = descriptor(&fixture.binding);
    prepare_layout(fixture);
    let (intent_receipt, operation_ids) = write_intent(fixture, &binding_receipt);
    let mut processes = serde_json::Map::new();
    for (index, role) in contract::ROLES.iter().enumerate() {
        let identity = json!({
            "pid": 4_000_000_000_u64 - index as u64,
            "started": (1000 + index).to_string(),
            "executable": fixture.backend.join(format!("{role}.exe")),
        });
        let tree = json!({
            "format_version": 3,
            "kind": "restore-monitoring-process-tree",
            "runtime_directory": fixture.run.join("processes"),
            "role": role,
            "scope_id": fixture.value["scope_id"],
            "operation_id": operation_ids[index],
            "supervisor": identity,
            "process": identity,
            "group_id": identity["pid"],
        });
        let tree_path = fixture
            .run
            .join("processes")
            .join(format!("{role}-tree.json"));
        write_json(&tree_path, &tree);
        for filename in [
            format!("{role}.log"),
            format!("{role}.json"),
            format!("{role}-members-{}-ready.json", operation_ids[index]),
            format!("{role}-members-{}-stopped.json", operation_ids[index]),
            format!("{role}-tree-{}-control.json", operation_ids[index]),
            format!("{role}-tree-{}-result.json", operation_ids[index]),
        ] {
            fs::write(fixture.run.join("processes").join(filename), b"{}\n").unwrap();
        }
        processes.insert(
            (*role).to_owned(),
            json!({
                "operation_id": operation_ids[index],
                "tree": descriptor(&tree_path),
                "identity": identity,
            }),
        );
    }
    let configs = contract::CONFIG_NAMES
        .iter()
        .map(|(name, filename)| {
            (
                (*name).to_owned(),
                descriptor(&fixture.run.join("configs").join(filename)),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let validations = contract::VALIDATION_NAMES
        .iter()
        .map(|(name, filename)| {
            (
                (*name).to_owned(),
                descriptor(&fixture.run.join("configs").join(filename)),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let start = json!({
        "format_version": 1,
        "kind": "restore-monitoring-start",
        "status": "running",
        "binding": binding_receipt,
        "intent": intent_receipt,
        "run_id": fixture.value["run_id"],
        "scope_id": fixture.value["scope_id"],
        "configs": configs,
        "validations": validations,
        "processes": processes,
        "endpoints": {
            "webhook": "http://127.0.0.1:29092/readyz",
            "alertmanager": "http://127.0.0.1:29091/-/ready",
            "prometheus": "http://127.0.0.1:29090/-/ready",
        },
        "started_at": "2026-09-12T08:00:01Z",
    });
    write_json(&fixture.run.join("start.json"), &start);
    let start_receipt = descriptor(&fixture.run.join("start.json"));
    let evidence = fixture.run.join("evidence");
    let observation = json!({
        "format_version": 1,
        "kind": "restore-monitoring-observation",
        "status": "passed",
        "binding": binding_receipt,
        "start": start_receipt,
        "run_id": fixture.value["run_id"],
        "scope_id": fixture.value["scope_id"],
        "metrics": {
            "api": {
                "artifact": descriptor(&evidence.join("api-metrics.txt")),
                "missing_status": 401,
                "invalid_status": 401,
                "accepted_status": 200,
                "sample_count": 2,
                "ryframe_sample_count": 1,
            },
            "worker": {
                "artifact": descriptor(&evidence.join("worker-metrics.txt")),
                "missing_status": 401,
                "invalid_status": 401,
                "accepted_status": 200,
                "sample_count": 2,
                "ryframe_sample_count": 1,
            },
        },
        "prometheus": {
            "targets": ["ryframe-api", "ryframe-worker"],
            "targets_artifact": descriptor(&evidence.join("prometheus-targets.json")),
            "alerts": contract::MONITORING_ALERTS,
            "rules_artifact": descriptor(&evidence.join("prometheus-rules.json")),
        },
        "boundaries": {
            "passed": true,
            "cases": ["23h", "23h30s", "24h30s"],
            "artifact": descriptor(&evidence.join("promtool-boundaries.log")),
        },
        "delivery": {
            "probe_id": "a".repeat(32),
            "firing": true,
            "resolved": true,
            "receiver": "http://127.0.0.1:29092/alerts",
            "events": descriptor(&evidence.join("webhook-events.jsonl")),
        },
        "external_contacts": 0,
        "observed_at": "2026-09-12T08:00:02Z",
    });
    write_json(&fixture.run.join("observation.json"), &observation);
    let observation_receipt = descriptor(&fixture.run.join("observation.json"));
    write_close(fixture, &binding_receipt, &observation_receipt);
}

fn write_intent(fixture: &Fixture, binding_receipt: &Value) -> (Value, [String; 3]) {
    let operation_ids = ["1".repeat(32), "2".repeat(32), "3".repeat(32)];
    let intent = json!({
        "format_version": 1,
        "kind": "restore-monitoring-start-intent",
        "binding": binding_receipt,
        "run_id": fixture.value["run_id"],
        "scope_id": fixture.value["scope_id"],
        "operations": {
            "webhook": operation_ids[0],
            "alertmanager": operation_ids[1],
            "prometheus": operation_ids[2],
        },
        "requested_at": "2026-09-12T08:00:00Z",
    });
    write_json(&fixture.run.join("start-intent.json"), &intent);
    (
        descriptor(&fixture.run.join("start-intent.json")),
        operation_ids,
    )
}

fn prepare_layout(fixture: &Fixture) {
    for name in [
        "configs",
        "processes",
        "evidence",
        "prometheus-data",
        "alertmanager-data",
    ] {
        fs::create_dir_all(fixture.run.join(name)).unwrap();
    }
    for (_, filename) in contract::CONFIG_NAMES
        .iter()
        .chain(contract::VALIDATION_NAMES.iter())
    {
        fs::write(fixture.run.join("configs").join(filename), b"config\n").unwrap();
    }
    for filename in contract::OBSERVATION_FILES {
        fs::write(fixture.run.join("evidence").join(filename), b"evidence\n").unwrap();
    }
}

fn write_close(fixture: &Fixture, binding_receipt: &Value, observation_receipt: &Value) {
    let operation_ids = ["1".repeat(32), "2".repeat(32), "3".repeat(32)];
    let close_intent = json!({
        "format_version": 1,
        "kind": "restore-monitoring-close-intent",
        "binding": binding_receipt,
        "source": observation_receipt,
        "run_id": fixture.value["run_id"],
        "scope_id": fixture.value["scope_id"],
        "requested_at": "2026-09-12T08:00:03Z",
    });
    write_json(&fixture.run.join("close-intent.json"), &close_intent);
    let close_intent_receipt = descriptor(&fixture.run.join("close-intent.json"));
    let completions = contract::ROLES
        .iter()
        .enumerate()
        .map(|(index, role)| {
            let path = fixture.run.join("processes").join(format!(
                "{role}-members-{}-stopped.json",
                operation_ids[index]
            ));
            ((*role).to_owned(), descriptor(&path))
        })
        .collect::<serde_json::Map<_, _>>();
    let close = json!({
        "format_version": 1,
        "kind": "restore-monitoring-close",
        "status": "closed",
        "binding": binding_receipt,
        "source": observation_receipt,
        "close_intent": close_intent_receipt,
        "run_id": fixture.value["run_id"],
        "scope_id": fixture.value["scope_id"],
        "completions": completions,
        "ports_closed": ["alertmanager", "prometheus", "webhook"],
        "closed_at": "2026-09-12T08:00:04Z",
    });
    write_json(&fixture.run.join("close.json"), &close);
}
