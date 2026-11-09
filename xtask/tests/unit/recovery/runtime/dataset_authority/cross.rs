use super::*;

struct Fixture {
    bindings: Value,
    target: Value,
    reference: Value,
    manifest: Value,
}

impl Fixture {
    fn new() -> Self {
        let product = json!({
            "id":"restore-arm", "backup_id":"formal-backup", "scope_id":"arm-b1",
            "fault_at":"2026-11-09T11:00:00+08:00", "databases":[],
            "object_endpoint":"http://127.0.0.1:19000", "object_prefix":"arm-b1/",
            "api_ready_url":"http://127.0.0.1:17001/readyz",
            "worker_ready_url":"http://127.0.0.1:17002/readyz",
            "frontend_sha":"d".repeat(40),
        });
        let plan_hash = document::hash(&product).unwrap();
        let manifest = json!({
            "id":"formal-backup", "source_sha":"a".repeat(40),
            "captured_at":"2026-11-09T10:30:00+08:00", "objects":[],
        });
        Self {
            bindings: json!({
                "record":{
                    "plan":product, "plan_hash":plan_hash, "status":"data_verified",
                    "started_at":"2026-11-09T11:00:00+08:00",
                    "data_verified_at":"2026-11-09T11:12:00+08:00",
                    "completed_at":null, "recovered_at":null, "failure":null,
                },
                "manifest":manifest,
            }),
            target: json!({
                "product_plan":product, "product_plan_sha256":plan_hash,
                "product_execution":{
                    "backend_product_sha":"b".repeat(40), "backend_execution_sha":"b".repeat(40),
                    "frontend_sha":"d".repeat(40), "adapter":null,
                },
            }),
            reference: json!({"target":{"frontend_url":"http://127.0.0.1:17003"}}),
            manifest,
        }
    }

    fn authority(&self) -> Result<Value> {
        start_authority(
            &self.bindings,
            &self.target,
            &self.reference,
            &self.manifest,
        )
    }
}

#[test]
fn start_authority_preserves_the_complete_v2_projection_for_ordinary_and_b0() {
    let mut fixture = Fixture::new();
    let mut expected = json!({
        "format_version":2, "kind":"restore-runtime-authority",
        "restore_id":"restore-arm", "backup_id":"formal-backup", "scope_id":"arm-b1",
        "plan_hash":fixture.target["product_plan_sha256"],
        "data_verified_at":"2026-11-09T11:12:00+08:00",
        "backup_source_sha":"a".repeat(40),
        "backend_product_sha":"b".repeat(40), "backend_execution_sha":"b".repeat(40),
        "frontend_sha":"d".repeat(40), "backend_adapter_contract":null,
        "api_endpoint":"http://127.0.0.1:17001/readyz",
        "worker_endpoint":"http://127.0.0.1:17002/readyz",
        "frontend_endpoint":"http://127.0.0.1:17003",
    });
    assert_eq!(fixture.authority().unwrap(), expected);

    let execution = &mut fixture.target["product_execution"];
    execution["backend_execution_sha"] = json!("c".repeat(40));
    execution["adapter"] = json!({"contract":"legacy-stable-readiness-b0-v1"});
    expected["backend_execution_sha"] = json!("c".repeat(40));
    expected["backend_adapter_contract"] = json!("legacy-stable-readiness-b0-v1");
    assert_eq!(fixture.authority().unwrap(), expected);
}

#[test]
fn start_authority_rejects_changed_plan_manifest_status_backup_id_and_plan_digest() {
    let mut fixture = Fixture::new();
    fixture.bindings["record"]["plan"]["object_prefix"] = json!("different/");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("唯一备份")
    );

    let mut fixture = Fixture::new();
    fixture.bindings["manifest"]["captured_at"] = json!("2026-11-09T10:35:00+08:00");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("唯一备份")
    );

    let mut fixture = Fixture::new();
    fixture.bindings["record"]["status"] = json!("started");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("唯一备份")
    );

    let mut fixture = Fixture::new();
    fixture.manifest["id"] = json!("another-backup");
    fixture.bindings["manifest"] = fixture.manifest.clone();
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("唯一备份")
    );

    let mut fixture = Fixture::new();
    fixture.bindings["record"]["plan_hash"] = json!("e".repeat(64));
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("计划摘要与已核验产品计划不同")
    );
}

#[test]
fn start_authority_validates_time_frontend_adapter_and_distinct_ports() {
    let mut fixture = Fixture::new();
    fixture.bindings["record"]["data_verified_at"] = json!("2026-11-09T11:12:00");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("缺少时区")
    );

    let mut fixture = Fixture::new();
    fixture.target["product_execution"]["frontend_sha"] = json!("e".repeat(40));
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("前端执行源码")
    );

    let mut fixture = Fixture::new();
    fixture.target["product_execution"]["backend_execution_sha"] = json!("c".repeat(40));
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("适配关系无效")
    );

    let mut fixture = Fixture::new();
    fixture.reference["target"]["frontend_url"] = json!("http://127.0.0.1:17002");
    assert!(
        fixture
            .authority()
            .unwrap_err()
            .to_string()
            .contains("互异显式端口")
    );
}
