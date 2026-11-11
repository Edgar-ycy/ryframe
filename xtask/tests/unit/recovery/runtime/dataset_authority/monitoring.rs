use super::{VerifiedTarget, authority_result, document};
use crate::recovery::{
    inputs::document::Document, runtime::dataset_authority::cross::RuntimeDocuments,
};
use serde_json::{Value, json};

fn fixture_document(name: &str, value: Value) -> Document {
    Document {
        path: name.into(),
        raw: format!("{value}\n").into_bytes(),
        value,
    }
}

#[test]
fn monitoring_authority_projects_exact_inputs_lineage_and_metrics() {
    let environment = json!({"path":"environment.json","bytes":5,"sha256":"a".repeat(64)});
    let launch = fixture_document(
        "runtime-launch.json",
        json!({"request":{"environment":{"document":environment}}}),
    );
    let runtime = fixture_document(
        "runtime.json",
        json!({
            "restore":{"id":"restore","backup_id":"backup"},
            "source":{"frontend_sha":"b".repeat(40)},
            "endpoints":{
                "api":"http://127.0.0.1:18080/readyz",
                "worker":"http://127.0.0.1:19091/readyz",
                "frontend":"http://127.0.0.1:14174"
            },
            "processes":{
                "api":{"identity":{"pid":10}},
                "worker":{"identity":{"pid":11}},
                "frontend":{"identity":{"pid":12}}
            }
        }),
    );
    let target = fixture_document(
        "target.json",
        json!({"maintenance_execution":{"root":"maintenance"},
            "product_execution":{"roots":{"source_backend":"product","execution_backend":"execution"}}}),
    );
    let verified = VerifiedTarget {
        runtime,
        documents: RuntimeDocuments {
            bindings: fixture_document("bindings.json", json!({})),
            backend: fixture_document("backend-build.json", json!({})),
            frontend: fixture_document("frontend-build.json", json!({})),
            launch,
        },
        value: target.value.clone(),
        target,
        manifest: json!({}),
        authority: json!({}),
        lineage: json!({
            "source_generation":{"path":"stopped.json","bytes":6,"sha256":"c".repeat(64)},
            "dataset_lineage":{"path":"lineage.json","bytes":7,"sha256":"d".repeat(64)}
        }),
    };

    let result = authority_result(&verified).unwrap();

    assert_eq!(
        result,
        json!({
            "format_version":1,
            "kind":"restore-monitoring-authority",
            "runtime":document::binding(&verified.runtime),
            "target_plan":document::binding(&verified.target),
            "launch":document::binding(&verified.documents.launch),
            "environment":environment,
            "restore":verified.runtime.value["restore"],
            "source":verified.runtime.value["source"],
            "endpoints":{
                "api_ready":"http://127.0.0.1:18080/readyz",
                "worker_ready":"http://127.0.0.1:19091/readyz",
                "frontend":"http://127.0.0.1:14174",
                "api_metrics":"http://127.0.0.1:18080/api/v1/monitor/metrics",
                "worker_metrics":"http://127.0.0.1:19091/metrics"
            },
            "maintenance_execution":verified.value["maintenance_execution"],
            "product_execution":verified.value["product_execution"],
            "processes":{"api":{"pid":10},"worker":{"pid":11},"frontend":{"pid":12}},
            "source_generation":verified.lineage["source_generation"],
            "dataset_lineage":verified.lineage["dataset_lineage"]
        })
    );
}
