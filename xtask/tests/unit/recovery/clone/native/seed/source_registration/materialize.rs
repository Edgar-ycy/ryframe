//! 仅核验正式文档投影和两轮比较；不构造或冒充完整 anchors/quiet 成功链。
use super::*;
use std::{collections::BTreeSet, fs};

fn fields(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn files() -> Files {
    Files {
        lineage: json!({"evidence":"lineage-binding"}),
        storage: json!({"evidence":"storage-binding"}),
        registry: json!({"evidence":"registry-binding"}),
        request: json!({"evidence":"request-binding"}),
        generation: json!({"evidence":"generation-binding"}),
    }
}

#[test]
fn materialize_documents_preserve_python_schema_and_keep_private_inputs_out() {
    let run = crate::workspace::root_dir().join(".local-tests/schema-only-run");
    let mut anchors = json!({"storage":{"evidence":"storage"},"lineage":{"evidence":"lineage"}});
    for name in PUBLIC_FIELDS {
        anchors[name] = json!({"evidence":name});
    }
    anchors["post"] = json!({"private":"not-a-public-anchor"});
    anchors["private"] = json!({"secret_key":"must-not-be-published"});
    let files = files();
    let storage = storage_document(&run, &anchors).unwrap();
    assert_eq!(
        fields(&storage),
        BTreeSet::from([
            "format_version",
            "kind",
            "run_directory",
            "run_manifest",
            "side",
            "storage"
        ])
    );
    assert_eq!(storage["storage"], anchors["storage"]);
    let registration = registration_document(&run, &anchors, &files).unwrap();
    let mut expected = BTreeSet::from([
        "format_version",
        "kind",
        "run_directory",
        "producer_lineage",
        "source_storage",
        "source_request",
        "generation_verified",
        "remote_writes",
        "outbox_drained",
        "restore_qualified",
    ]);
    expected.extend(PUBLIC_FIELDS);
    assert_eq!(fields(&registration), expected);
    for name in PUBLIC_FIELDS {
        assert_eq!(registration[name], anchors[name]);
    }
    assert_eq!(registration["producer_lineage"], files.lineage);
    assert_eq!(registration["source_request"], files.request);
    let result = result_document(&files, json!({"evidence":"registration-binding"}));
    assert_eq!(
        fields(&result),
        BTreeSet::from([
            "status",
            "registration",
            "source_request",
            "source_storage",
            "generation_verified",
            "remote_writes",
            "outbox_drained",
            "restore_qualified"
        ])
    );
    assert_eq!(result["status"], "seed_source_registered");
    assert_eq!(result["remote_writes"], 0);
    assert_eq!(result["outbox_drained"], true);
    assert_eq!(result["restore_qualified"], false);
    for document in [&storage, &registration, &result] {
        assert!(!document.to_string().contains("must-not-be-published"));
        assert!(!document.to_string().contains("not-a-public-anchor"));
    }
}

#[test]
fn materialize_rejects_any_public_anchor_drift_and_other_successful_registration() {
    let original = json!({"lineage":{"identities":[1,2,3]},"storage":{"generation":7},"run_manifest":{"sha256":"original"}});
    compare_anchors(&original, &original).unwrap();
    for field in ["lineage", "storage", "run_manifest"] {
        let mut changed = original.clone();
        changed[field] = json!({"changed":true});
        assert!(compare_anchors(&original, &changed).is_err(), "{field}");
    }
    let mut saved = json!({"attempts":[{"number":4,"stage":"seed-runtime","mode":"source-register","status":"failed"}]});
    reject_prior(&saved, 5).unwrap();
    saved["attempts"][0]["status"] = json!("passed");
    assert!(reject_prior(&saved, 5).is_err());
    assert!(reject_prior(&saved, 3).is_err());
    reject_prior(&saved, 4).unwrap();
    assert!(reject_prior(&saved, 0).is_err());
}

#[test]
fn materialize_rechecks_complete_registry_request_values_and_written_bindings() {
    let parent = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&parent).unwrap();
    let backend = tempfile::Builder::new()
        .prefix("materialize-documents-")
        .tempdir_in(parent)
        .unwrap();
    let directory = backend.path().join(".local-tests/evidence");
    fs::create_dir_all(&directory).unwrap();
    let registry =
        json!({"processes":[{"identity":{"pid":42,"started":"10","executable":"recorded"}}]});
    let request =
        json!({"source":{"s3":{"secret_key":"private-source-request"}},"max_object_bytes":2097152});
    let registry_path = directory.join("producers.json");
    let request_path = directory.join("source-request.json");
    fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
    fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
    let files = Files {
        registry: file_binding(backend.path(), &registry_path).unwrap(),
        request: file_binding(backend.path(), &request_path).unwrap(),
        ..files()
    };
    compare_documents(
        backend.path(),
        &files,
        &registry,
        &registry,
        &request,
        &request,
    )
    .unwrap();
    assert!(
        compare_documents(
            backend.path(),
            &files,
            &registry,
            &json!({"processes":[]}),
            &request,
            &request
        )
        .is_err()
    );
    let mut changed = request.clone();
    changed["max_object_bytes"] = json!(1);
    assert!(
        compare_documents(
            backend.path(),
            &files,
            &registry,
            &registry,
            &request,
            &changed
        )
        .is_err()
    );
    let original = fs::read(&registry_path).unwrap();
    fs::write(&registry_path, [&original[..], b"\n"].concat()).unwrap();
    assert!(
        compare_documents(
            backend.path(),
            &files,
            &registry,
            &registry,
            &request,
            &request
        )
        .is_err()
    );
    fs::write(&registry_path, original).unwrap();
    fs::write(&request_path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(
        compare_documents(
            backend.path(),
            &files,
            &registry,
            &registry,
            &request,
            &request
        )
        .is_err()
    );
    assert_eq!(
        fs::read(&request_path).unwrap(),
        serde_json::to_vec(&changed).unwrap()
    );
}
