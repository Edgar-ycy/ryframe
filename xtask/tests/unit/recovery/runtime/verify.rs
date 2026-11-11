use super::*;
use crate::cli::RuntimeSources;

fn authority() -> Value {
    json!({
        "format_version":2, "kind":"restore-runtime-authority", "restore_id":"restore-1",
        "backup_id":"backup-1", "scope_id":"fixture-restore-1", "plan_hash":"a".repeat(64),
        "data_verified_at":"2026-09-06T00:00:00Z", "backup_source_sha":"b".repeat(40),
        "backend_product_sha":"c".repeat(40), "backend_execution_sha":"c".repeat(40),
        "backend_adapter_contract":null, "frontend_sha":"d".repeat(40),
        "api_endpoint":"http://127.0.0.1:18080/readyz", "worker_endpoint":"http://[::1]:19091/readyz",
        "frontend_endpoint":"http://127.0.0.1:14174",
    })
}

#[test]
fn direct_verify_stdin_is_bounded_strict_and_rejects_extra_authority() {
    let value = authority();
    let encoded = serde_json::to_vec(&value).unwrap();
    assert_eq!(read_authority(encoded.as_slice()).unwrap(), value);
    let duplicate = format!(
        "{{\"format_version\":2,{}",
        &str::from_utf8(&encoded).unwrap()[1..]
    );
    assert!(read_authority(duplicate.as_bytes()).is_err());
    assert!(read_authority([0xff].as_slice()).is_err());
    assert!(read_authority(io::repeat(b' ').take(strict::MAX_JSON_BYTES + 1)).is_err());
    let mut unbound = value;
    unbound["target_plan"] = json!("unknown");
    assert!(read_authority(serde_json::to_vec(&unbound).unwrap().as_slice()).is_err());
}

#[test]
fn direct_verify_binds_explicit_paths_and_adapter_before_network() {
    let root = std::env::current_dir().unwrap();
    let execution = root.join("execution");
    let product = root.join("product");
    let frontend = root.join("frontend");
    let bindings = root.join("bindings.json");
    let options = RuntimeVerifyOptions {
        sources: RuntimeSources {
            source_backend: execution.clone(),
            source_frontend: frontend.clone(),
            product_backend: Some(product.clone()),
            adapter_contract: Some("legacy-stable-readiness-b0-v1".to_owned()),
        },
        bindings: bindings.clone(),
        receipt: root.join("runtime.json"),
    };
    let runtime = json!({
        "paths":{"backend_product_root":product,"backend_execution_root":execution,
            "frontend_root":frontend,"bindings":bindings},
        "source":{"backend_adapter_contract":"legacy-stable-readiness-b0-v1"},
    });
    verify_paths(&runtime, &options.sources, &options.bindings).unwrap();
    let mut changed = runtime.clone();
    changed["paths"]["backend_product_root"] = json!(execution);
    assert!(verify_paths(&changed, &options.sources, &options.bindings).is_err());
    changed = runtime;
    changed["source"]["backend_adapter_contract"] = Value::Null;
    assert!(verify_paths(&changed, &options.sources, &options.bindings).is_err());
}

#[test]
fn only_the_exact_legacy_source_selects_the_b0_frontend_receipt() {
    let mut value = authority();
    value["backend_adapter_contract"] = json!("legacy-stable-readiness-b0-v1");
    value["backend_product_sha"] = json!("815c5eafb09d4b493319d255fb7ab88ddd02c8b6");
    value["backend_execution_sha"] = json!("c05114bcdf5c369cd74087db6317ce3c8f89bee8");
    assert!(!b0::matches(&value));
    value["frontend_sha"] = json!("0087ea2ecf62530d042b9e52f5c950fb34c66d78");
    assert!(b0::matches(&value));
    value["backend_execution_sha"] = json!("e".repeat(40));
    assert!(!b0::matches(&value));
}

#[test]
fn direct_verify_requires_bound_authority_and_preserves_exact_reply() {
    let root = std::env::current_dir().unwrap();
    let authority = authority();
    let make_document = |name, value: Value| document::Document {
        path: root.join(name),
        raw: serde_json::to_vec(&value).unwrap(),
        value,
    };
    let bindings = make_document(
        "bindings.json",
        json!({
            "record":{"status":"data_verified","data_verified_at":authority["data_verified_at"],
                "plan_hash":authority["plan_hash"],"plan":{"id":authority["restore_id"],"backup_id":authority["backup_id"],
                    "scope_id":authority["scope_id"],"frontend_sha":authority["frontend_sha"],
                    "api_ready_url":authority["api_endpoint"],"worker_ready_url":authority["worker_endpoint"]}},
            "manifest":{"id":authority["backup_id"],"source_sha":authority["backup_source_sha"]},
        }),
    );
    let backend = make_document("backend.json", json!({}));
    let frontend = make_document(
        "frontend.json",
        json!({"files":[{"path":"index.html"},{"path":".vite/manifest.json"}]}),
    );
    let paths = json!({"bindings":bindings.path,"backend_build":backend.path,"frontend_build":frontend.path});
    let digests = json!({"bindings":bindings.sha256(),"backend_build":backend.sha256(),"frontend_build":frontend.sha256()});
    let roots =
        json!({"backend_product":root,"backend_execution":root,"frontend":root.join("frontend")});
    let launch = make_document(
        "launch.json",
        json!({"request":{"authority":authority,"paths":paths,"digests":digests,"roots":roots}}),
    );
    let documents = RuntimeDocuments {
        bindings,
        backend,
        frontend,
        launch,
    };
    let mut runtime_paths = paths;
    runtime_paths["backend_product_root"] = roots["backend_product"].clone();
    runtime_paths["backend_execution_root"] = roots["backend_execution"].clone();
    runtime_paths["frontend_root"] = roots["frontend"].clone();
    let source = json!({"backend_product_sha":authority["backend_product_sha"],
        "backend_execution_sha":authority["backend_execution_sha"],"backend_adapter_contract":null,
        "frontend_sha":authority["frontend_sha"]});
    let mut receipt_source = source.clone();
    receipt_source["backup_source_sha"] = authority["backup_source_sha"].clone();
    let runtime = make_document(
        "runtime.json",
        json!({
            "restore":{"id":authority["restore_id"],"backup_id":authority["backup_id"],"scope_id":authority["scope_id"],
                "data_verified_at":authority["data_verified_at"],"plan_hash":authority["plan_hash"]},
            "endpoints":{"api":authority["api_endpoint"],"worker":authority["worker_endpoint"],"frontend":authority["frontend_endpoint"]},
            "paths":runtime_paths,"digests":digests,"source":receipt_source,
        }),
    );
    verify_authority(&runtime.value, &documents, &authority, source.clone()).unwrap();
    let mut different = authority;
    different["plan_hash"] = json!("e".repeat(64));
    assert!(verify_authority(&runtime.value, &documents, &different, source).is_err());

    let processes = json!({"api":{"identity":{"pid":42}}});
    let readiness = json!({"status":"ready","mysql":"up","redis":"up","object_storage":"up"});
    let descriptor =
        |input: &document::Document| json!({"path":input.path,"sha256":input.sha256()});
    assert_eq!(
        reply(&runtime, &documents, processes.clone(), readiness.clone()),
        json!({
            "format_version":1,"kind":"restore-runtime-verification","status":"verified",
            "runtime_receipt_sha256":runtime.sha256(),"bindings":descriptor(&documents.bindings),
            "build_receipts":{"backend":descriptor(&documents.backend),"frontend":descriptor(&documents.frontend),"launch":descriptor(&documents.launch)},
            "restore":runtime.value["restore"],"source":runtime.value["source"],"endpoints":runtime.value["endpoints"],
            "processes":processes,"api_readiness":readiness,
            "frontend":{"build_receipt_sha256":documents.frontend.sha256(),"files":documents.frontend.value["files"]},
        })
    );
}
