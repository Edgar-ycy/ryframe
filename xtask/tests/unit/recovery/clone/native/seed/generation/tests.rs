use super::{archived_image, archived_request, builds, lineage_receipt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn request() -> Value {
    json!({"format_version":1,"kind":"devex-clone-seed-source-generation","id":"same",
        "source_registration":{},"source_rebind":{},"review_successor":{},"current_storage":{"attempt":53},
        "execution_backend":"execution","expected_backend_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "adapter_contract":null,"product_backend":null,"backend_build":{"old":"runtime"},
        "maintenance_build":{"old":"maintenance"},"source_environment":{}})
}

#[test]
fn frozen_request_accepts_only_complete_resource_resume_and_paired_builds() {
    let old = request();
    assert!(!archived_request(&old, &old, &Value::Null).unwrap());
    let segment = json!({"phase":"ready","storage":{"attempt":64},"cache":{"attempt":65}});
    let mut current = old.clone();
    current["current_storage"] = segment["storage"].clone();
    assert!(!archived_request(&old, &current, &segment).unwrap());
    current["backend_build"] = json!({"new":"runtime"});
    current["maintenance_build"] = json!({"new":"maintenance"});
    assert!(archived_request(&old, &current, &segment).unwrap());
    for field in old.as_object().unwrap().keys() {
        if ["current_storage", "backend_build", "maintenance_build"].contains(&field.as_str()) {
            continue;
        }
        let mut changed = current.clone();
        changed[field] = json!("changed");
        assert!(
            archived_request(&old, &changed, &segment).is_err(),
            "{field}"
        );
    }
    for field in ["backend_build", "maintenance_build"] {
        let mut changed = current.clone();
        changed[field] = old[field].clone();
        assert!(archived_request(&old, &changed, &segment).is_err());
    }
    for (field, value) in [
        ("phase", json!("storage-ready")),
        ("storage", Value::Null),
        ("cache", Value::Null),
    ] {
        let mut changed = segment.clone();
        changed[field] = value;
        assert!(archived_request(&old, &current, &changed).is_err());
    }
    current["unknown"] = true.into();
    assert!(archived_request(&old, &current, &segment).is_err());
}

fn images() -> (Value, Value, Value) {
    let old = json!({"databases":{"rows":100044},"schema":{"all":true},"objects":{"count":257},
        "owners":{"all":true},"redis":{"keys":["owner","sentinel"]},"storage":{
        "rustfs":{"identity":{"pid":53},"sha256":"same"},"redis":{
        "configuration":{"sha256":"same"},"distribution":"Ubuntu-24.04","executable":"/usr/bin/redis-server",
        "pid":54,"port":16390,"run_id":"old","sha256":"same","started":"old","wsl":{"sha256":"same"}}}});
    let mut segment = json!({"phase":"ready","storage":{"storage":{
        "identity":{"pid":64},"sha256":"same","process_receipt":{},"launch_receipt":{}}},
        "cache":{"redis":old["storage"]["redis"]}});
    segment["cache"]["redis"]["pid"] = 65.into();
    segment["cache"]["redis"]["started"] = "new".into();
    segment["cache"]["redis"]["run_id"] = "new".into();
    let mut current = old.clone();
    current["storage"] =
        json!({"rustfs":segment["storage"]["storage"],"redis":segment["cache"]["redis"]});
    (old, current, segment)
}

#[test]
fn full_image_rejects_data_metadata_ownership_and_redis_identity_drift() {
    let (old, current, segment) = images();
    archived_image(&old, &old, &Value::Null).unwrap();
    archived_image(&old, &current, &segment).unwrap();
    for field in ["databases", "schema", "objects", "owners", "redis"] {
        let mut changed = current.clone();
        changed[field] = json!({"changed":true});
        assert!(archived_image(&old, &changed, &segment).is_err(), "{field}");
    }
    for field in old["storage"]["redis"].as_object().unwrap().keys() {
        if ["pid", "started", "run_id"].contains(&field.as_str()) {
            continue;
        }
        let mut changed = segment.clone();
        changed["cache"]["redis"][field] = "changed".into();
        assert!(archived_image(&old, &current, &changed).is_err(), "{field}");
    }
    for delete in [false, true] {
        let mut changed = segment.clone();
        if delete {
            changed["cache"]["redis"]
                .as_object_mut()
                .unwrap()
                .remove("port");
        } else {
            changed["cache"]["redis"]["extra"] = true.into();
        }
        assert!(archived_image(&old, &current, &changed).is_err());
    }
}

#[test]
fn replay_image_and_reboot_are_separate_from_zero_write_resume() {
    let (old, current, mut segment) = images();
    segment["replay"] = json!({"image":current});
    archived_image(&old, &current, &segment).unwrap();
    segment["replay"]["image"]["objects"]["count"] = 258.into();
    assert!(archived_image(&old, &current, &segment).is_err());
    segment["reboot"] = json!({"authorized":true});
    assert!(archived_image(&old, &current, &segment).is_err());
}

fn runtime() -> Value {
    json!({"sources":{"full":{"source":{"snapshot":{"head":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "clean":true,"files":[]}}}},"build":{"commands":{"api":["cargo","api"],"worker":["cargo","worker"]},
        "profile":"dev","target":"windows","jobs":"4","toolchain":{"cargo":"fixed","rustc":"fixed"},
        "environment":{"variables":["PATH"],"sha256":"old"}},
        "artifacts":{"api":{"bytes":10,"sha256":"same"},"worker":{"bytes":20,"sha256":"same"}}})
}

#[test]
fn runtime_equivalence_normalizes_only_environment_digest() {
    let old = runtime();
    let mut current = old.clone();
    current["build"]["environment"]["sha256"] = "new".into();
    builds::equivalent_runtime(&old, &current, &request()).unwrap();
    for pointer in [
        "/artifacts/api/bytes",
        "/artifacts/worker/sha256",
        "/build/jobs",
        "/build/commands/api",
        "/build/toolchain/cargo",
        "/build/environment/variables",
        "/sources/full/source/snapshot/clean",
        "/sources/full/source/snapshot/files",
        "/sources/full/source/snapshot/head",
    ] {
        let mut changed = current.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("changed");
        assert!(
            builds::equivalent_runtime(&old, &changed, &request()).is_err(),
            "{pointer}"
        );
    }
    current["build"]["environment"]["extra"] = true.into();
    assert!(builds::equivalent_runtime(&old, &current, &request()).is_err());
}

fn maintenance(directory: &Path, digest: &str) -> Value {
    let runtime = runtime();
    let mut artifacts = serde_json::Map::new();
    for role in ["reset", "migrate", "tenant-data"] {
        artifacts.insert(
            role.into(),
            json!({"executable":directory.join(format!("ryframe-{role}.exe")),
            "cargo_executable":"fixed","command":["cargo","build",role],"bytes":30,"sha256":"same",
            "cargo_output":{"file":format!("{role}.jsonl"),"bytes":40,"sha256":digest},
            "cargo_log":{"file":format!("{role}.log"),"bytes":50,"sha256":digest}}),
        );
    }
    json!({"backend_root":"execution","source":runtime["sources"]["full"]["source"],
        "source_inventory":runtime["sources"]["full"],"toolchain":{"same":true},
        "target_directory":"same","artifacts":artifacts})
}

#[test]
fn maintenance_equivalence_normalizes_only_receipt_directory_and_log_content() {
    let root = PathBuf::from("fixture");
    let old_dir = root.join("old");
    let new_dir = root.join("new");
    let old = maintenance(&old_dir, "old");
    let current = maintenance(&new_dir, "new");
    let check = |value: &Value| {
        builds::equivalent_maintenance(
            &runtime(),
            &old,
            &old_dir.join("build.json"),
            value,
            &new_dir.join("build.json"),
            &request(),
        )
    };
    check(&current).unwrap();
    for pointer in [
        "/artifacts/reset/sha256",
        "/artifacts/migrate/command",
        "/artifacts/tenant-data/bytes",
        "/artifacts/reset/cargo_executable",
        "/artifacts/reset/cargo_output/file",
        "/toolchain/same",
        "/source/snapshot/head",
        "/backend_root",
        "/source_inventory/source/snapshot/clean",
    ] {
        let mut changed = current.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("changed");
        assert!(check(&changed).is_err(), "{pointer}");
    }
    let mut changed = current.clone();
    changed["artifacts"]["reset"]["executable"] = json!(old_dir.join("ryframe-reset.exe"));
    assert!(check(&changed).is_err());
    changed = current;
    changed["artifacts"]["reset"]["cargo_log"]["extra"] = true.into();
    assert!(check(&changed).is_err());
}

fn receipt() -> Value {
    let fields = [
        "status",
        "request",
        "failed_start",
        "failure_proof",
        "history_length",
        "history_sha256",
        "coordinator_source",
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
        "runtime",
        "before",
        "after",
        "evidence_manifest",
        "source_generation_published",
        "replay_allowed",
        "remote_writes",
        "restore_qualified",
    ];
    let mut value: Value = fields
        .into_iter()
        .map(|field| (field.to_owned(), Value::Null))
        .collect::<serde_json::Map<_, _>>()
        .into();
    for field in ["failed_start", "history_length", "remote_writes"] {
        value[field] = 0.into();
    }
    for field in [
        "source_generation_published",
        "replay_allowed",
        "restore_qualified",
    ] {
        value[field] = false.into();
    }
    value["evidence_manifest"] =
        json!({"files":10233,"bytes":1081069032u64,"sha256":"a".repeat(64)});
    value
}

#[test]
fn lineage_receipt_never_accepts_python_bool_integer_coercion_or_bad_digest() {
    let value = receipt();
    lineage_receipt(&value).unwrap();
    for field in ["failed_start", "history_length", "remote_writes"] {
        for invalid in [json!(false), json!(-1), json!("0"), json!(0.0)] {
            let mut changed = value.clone();
            changed[field] = invalid;
            assert!(lineage_receipt(&changed).is_err(), "{field}");
        }
    }
    for field in [
        "source_generation_published",
        "replay_allowed",
        "restore_qualified",
    ] {
        let mut changed = value.clone();
        changed[field] = 0.into();
        assert!(lineage_receipt(&changed).is_err());
    }
    for invalid in [
        json!({"files":0,"bytes":1,"sha256":"a".repeat(64)}),
        json!({"files":1,"bytes":true,"sha256":"a".repeat(64)}),
        json!({"files":1,"bytes":1,"sha256":"A".repeat(64)}),
        json!({"files":1,"bytes":1,"sha256":"a".repeat(63)}),
    ] {
        let mut changed = value.clone();
        changed["evidence_manifest"] = invalid;
        assert!(lineage_receipt(&changed).is_err());
    }
}
