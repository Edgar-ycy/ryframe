use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Inputs, build, source_id};

struct Fixture {
    runtime: PathBuf,
    documents: Value,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(if cfg!(windows) {
            "D:/request-fixture"
        } else {
            "/request-fixture"
        });
        let runtime = root.join(".local-tests/runtime");
        let binding =
            |name: &str| json!({"path":root.join(name),"bytes":10,"sha256":"a".repeat(64)});
        let tool = |name: &str| json!({"path":root.join(name),"sha256":"b".repeat(64)});
        let runtime_binding =
            json!({"path":runtime.join("runtime.json"),"bytes":20,"sha256":"c".repeat(64)});
        let documents = json!({
            "manifest":{"id":"copy-run","ignored":"manifest-extra"},
            "original_request":{"source":{"scope_id":"original-scope"},"tools":{
                "mysql":tool("original-mysql.exe"),"mysqldump":tool("original-dump.exe")},
                "backend_build":binding("original-build.json"),"max_object_bytes":12345,"ignored":true},
            "target_request":{"target":{"scope_id":"seed-scope","s3":{"private":"target-secret"},
                "databases":[{"key":"target-db"}]},"tools":{"mysql":tool("target-mysql.exe")},
                "maintenance_build":binding("target-maintenance.json"),"ignored":true},
            "post_request":{"node":tool("post-node.exe"),"ignored":true},
            "runtime_binding":runtime_binding,
            "object_configuration":{"app_env":"test","directory":runtime.join("config"),"files":{},"sha256":"1".repeat(64)},
            "handoff":{"api_url":"http://127.0.0.1:18210","backend_build":binding("handoff-build.json")},
            "registry":{"scope_id":"seed-scope","runtime_sha256":"c".repeat(64)},
            "registry_binding":binding("registry.json"),
            "maintenance":{"source":{"worktree_fingerprint":format!("sha256:{}","d".repeat(64))}},
            "process_bindings":{"api":{"path":runtime.join("api.json"),"bytes":30,"sha256":"e".repeat(64)},
                "worker":{"path":runtime.join("worker.json"),"bytes":40,"sha256":"f".repeat(64)}}});
        Self { runtime, documents }
    }

    fn inputs(&self) -> Inputs<'_> {
        Inputs {
            manifest: &self.documents["manifest"],
            original_request: &self.documents["original_request"],
            target_request: &self.documents["target_request"],
            post_request: &self.documents["post_request"],
            runtime: &self.runtime,
            runtime_binding: &self.documents["runtime_binding"],
            object_configuration: &self.documents["object_configuration"],
            handoff: &self.documents["handoff"],
            registry: &self.documents["registry"],
            registry_binding: &self.documents["registry_binding"],
            maintenance: &self.documents["maintenance"],
            process_bindings: &self.documents["process_bindings"],
        }
    }
}

#[test]
fn short_boundary_and_long_source_ids_are_stable_without_character_conversion() {
    assert_eq!(source_id("copy-run").unwrap(), "copy-run-seed");
    assert_eq!(
        source_id(&"a".repeat(43)).unwrap(),
        format!("{}-seed", "a".repeat(43))
    );
    assert_eq!(
        source_id(&"a".repeat(44)).unwrap(),
        "seed-16849877c6c21ef0bfa68e4f6747300d"
    );
    assert_eq!(
        source_id(&"a".repeat(48)).unwrap(),
        "seed-97daac0ee9998dfcad6c9c0970da5ca4"
    );
    assert_ne!(
        source_id(&"a".repeat(48)).unwrap(),
        source_id(&"b".repeat(48)).unwrap()
    );
    for id in ["UPPER", "ab", "bad/id", "bad id", "租户", &"a".repeat(49)] {
        assert!(source_id(id).is_err(), "{id}");
    }
}

#[test]
fn request_has_exact_twelve_fields_and_uses_each_authoritative_source_without_mutation() {
    let fixture = Fixture::new();
    let before = fixture.documents.clone();
    let result = build(fixture.inputs()).unwrap();
    assert_eq!(
        result,
        json!({"format_version":1,"kind":"devex-clone-source-export","id":"copy-run-seed",
        "source":{"scope_id":"seed-scope","s3":{"configuration":fixture.documents["object_configuration"]},"databases":[{"key":"target-db"}],
            "runtime_dir":fixture.runtime,"api_url":"http://127.0.0.1:18210"},
        "tools":{"mysql":fixture.documents["target_request"]["tools"]["mysql"],
            "mysqldump":fixture.documents["original_request"]["tools"]["mysqldump"],"node":fixture.documents["post_request"]["node"]},
        "backend_build":fixture.documents["handoff"]["backend_build"],
        "maintenance_build":fixture.documents["target_request"]["maintenance_build"],
        "worktree_fingerprint":fixture.documents["maintenance"]["source"]["worktree_fingerprint"],
        "runtime":fixture.documents["runtime_binding"],"processes":fixture.documents["process_bindings"],
        "producers_registry":fixture.documents["registry_binding"],"max_object_bytes":12345})
    );
    assert_eq!(fixture.documents, before);
}

#[test]
fn registry_scope_runtime_digest_and_consumed_shapes_fail_closed_without_secrets() {
    for (pointer, replacement) in [
        ("/registry/scope_id", json!("wrong-scope")),
        ("/registry/runtime_sha256", json!("f".repeat(64))),
        ("/runtime_binding/bytes", json!(false)),
        ("/original_request/max_object_bytes", json!(true)),
        ("/maintenance/source/worktree_fingerprint", json!(23)),
        ("/post_request/node/sha256", json!("invalid")),
        ("/process_bindings/api/path", json!("relative.json")),
    ] {
        let mut fixture = Fixture::new();
        *fixture.documents.pointer_mut(pointer).unwrap() = replacement;
        let error = build(fixture.inputs()).unwrap_err().to_string();
        assert!(!error.contains("target-secret"), "{pointer}");
        assert!(!error.contains("manifest-extra"), "{pointer}");
    }
}
