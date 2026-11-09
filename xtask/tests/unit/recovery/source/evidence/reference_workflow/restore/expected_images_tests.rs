use super::*;
use crate::recovery::inputs::domain::TARGET_DATABASES;

const COMBINED_SCHEMA: &str = "1a3730075616e16755e3466b3e693e1b167d218056c83bbd85d8f791d55e1fd7";
const TENANT_SCHEMA: &str = "cb1566749611a94867c5d26ecadd0cf6e5d53debe607c7573ddb89b40187ffb2";

struct Fixture {
    plan: Value,
    manifest: Value,
    initialized: Value,
}

impl Fixture {
    fn new() -> Self {
        let mut sources = Vec::new();
        let mut targets = Vec::new();
        let mut backup = Vec::new();
        let mut observations = BTreeMap::new();
        for (index, (key, kind, _)) in TARGET_DATABASES.into_iter().enumerate() {
            let source = format!("source_{index}");
            let target = format!("target_{index}");
            sources.push(
                json!({"key":key,"database":source,"kind":kind,"server_uuid":"source-server"}),
            );
            targets.push(
                json!({"key":key,"database":target,"kind":kind,"server_uuid":"target-server"}),
            );
            backup.push(json!({"key":key,"database":source,"kind":kind,"server_uuid":"source-server",
                "tables":[table("sys_post", 10 + index, "b"), table("seaql_tenant_data_migrations", 2, "e")]}));
            observations.insert(target, database(index, kind));
        }
        backup.reverse();
        let mut objects = BTreeMap::new();
        let mut buckets = Vec::new();
        for bucket in BUCKETS {
            objects.insert(
                bucket,
                json!({"keys":["target/.ryframe-owner"],
                "owner":{"bytes":41,"sha256":"f".repeat(64)}}),
            );
            let entries = if bucket == "uploads" {
                vec![
                    json!({"key":"source/tenant/z.txt","bytes":1,"sha256":"b".repeat(64)}),
                    json!({"key":"source/tenant/a.txt","bytes":2,"sha256":"c".repeat(64)}),
                ]
            } else {
                Vec::new()
            };
            buckets.push(json!({"bucket":bucket,"prefix":"source/","entries":entries}));
        }
        Self {
            plan: json!({"source":{"scope_id":"source","databases":sources},
                "target":{"scope_id":"target","databases":targets}}),
            manifest: json!({"scope_id":"source","control_schema_fingerprint":"c".repeat(16),
                "tenant_schema_fingerprint":"d".repeat(64),"databases":backup,"objects":buckets}),
            initialized: json!({"inventory":{"observations":observations},"objects":objects,
                "redis":{"keys":["target-owner"],"owner":"target","sentinel":"original"}}),
        }
    }

    fn project(&self) -> Result<(Value, Value)> {
        expected_images(&self.plan, &self.manifest, &self.initialized)
    }
}

fn table(name: &str, rows: usize, hash: &str) -> Value {
    json!({"table":name,"rows":rows,"sha256":hash.repeat(64)})
}

fn database(index: usize, kind: &str) -> Value {
    let mut preserved = BTreeMap::from([
        (
            "ryframe_resource_ownership",
            json!({"rows":if kind == "combined" {2} else {1},"sha256":"d".repeat(64)}),
        ),
        (
            "seaql_tenant_data_migrations",
            json!({"rows":1,"sha256":"c".repeat(64)}),
        ),
    ]);
    if kind == "combined" {
        for name in ["sys_backup_set", "sys_backup_resource", "sys_restore_run"] {
            preserved.insert(name, json!({"rows":0,"sha256":"e".repeat(64)}));
        }
    }
    let all = preserved
        .keys()
        .copied()
        .chain(["sys_post"])
        .collect::<BTreeSet<_>>();
    json!({"resource":{"kind":"database","scope_id":"target","server_uuid":"target-server",
            "database":format!("target_{index}")},
        "schema_sha256":if kind == "combined" {COMBINED_SCHEMA} else {TENANT_SCHEMA},
        "tables":{"sys_post":{"rows":0,"sha256":"a".repeat(64)}},"preserved":preserved,
        "all_tables":all,"ownership":[{"scope_id":"target","marker":format!("owner-{index}")}]})
}

#[test]
fn projects_complete_images_by_logical_database_key_and_preserves_original_owners() {
    let fixture = Fixture::new();
    let (before, after) = fixture.project().unwrap();
    let initial = &fixture.initialized;
    assert_eq!(
        before,
        json!({"databases":initial["inventory"]["observations"],
        "objects":initial["objects"],"redis":initial["redis"]})
    );
    let mut expected = before;
    for index in 0..4 {
        let name = format!("target_{index}");
        expected["databases"][&name]["tables"]["sys_post"] =
            json!({"rows":10+index,"sha256":"b".repeat(64)});
        expected["databases"][&name]["preserved"]["seaql_tenant_data_migrations"] =
            json!({"rows":2,"sha256":"e".repeat(64)});
    }
    expected["objects"]["uploads"]["keys"] = json!([
        "target/.ryframe-owner",
        "target/tenant/a.txt",
        "target/tenant/z.txt"
    ]);
    assert_eq!(after, expected);
}

#[test]
fn rejects_missing_table_wrong_schema_and_mismatched_physical_database() {
    let mut missing = Fixture::new();
    missing.manifest["databases"][0]["tables"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(
        missing
            .project()
            .unwrap_err()
            .to_string()
            .contains("全部非保留表")
    );

    let mut schema = Fixture::new();
    schema.initialized["inventory"]["observations"]["target_2"]["schema_sha256"] =
        json!(COMBINED_SCHEMA);
    assert!(schema.project().unwrap_err().to_string().contains("schema"));

    let mut physical = Fixture::new();
    physical.manifest["databases"][0]["database"] = json!("different_source");
    assert!(
        physical
            .project()
            .unwrap_err()
            .to_string()
            .contains("物理身份")
    );
}

#[test]
fn rejects_object_scope_owner_collision_and_duplicate_keys() {
    let mut scope = Fixture::new();
    scope.manifest["objects"][0]["entries"][0]["key"] = json!("source-other/tenant/file.txt");
    assert!(scope.project().unwrap_err().to_string().contains("scope"));

    let mut owner = Fixture::new();
    owner.manifest["objects"][0]["entries"][0]["key"] = json!("source/.ryframe-owner");
    assert!(owner.project().unwrap_err().to_string().contains("owner"));

    let mut duplicate = Fixture::new();
    duplicate.manifest["objects"][0]["entries"][1]["key"] = json!("source/tenant/z.txt");
    assert!(
        duplicate
            .project()
            .unwrap_err()
            .to_string()
            .contains("key 重复")
    );
}
