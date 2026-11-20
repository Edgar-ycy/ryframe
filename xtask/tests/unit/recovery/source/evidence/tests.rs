use serde_json::{Value, json};

use super::{execute, model};

#[test]
fn copy_generation_accepts_only_the_read_only_registered_run() {
    let mut request = json!({"operation":"copy-source-generation","write":true,
        "arguments":{"backend":"relative","run_dir":"relative"}});
    let reject = |value: &Value| {
        execute(&serde_json::to_vec(value).unwrap())
            .unwrap_err()
            .to_string()
    };
    assert!(reject(&request).contains("写入授权"));
    request["write"] = json!(false);
    request["arguments"]["source_request"] = json!({"forged":true});
    assert!(reject(&request).contains("字段"));
    request["arguments"]
        .as_object_mut()
        .unwrap()
        .remove("source_request");
    assert!(reject(&request).contains("绝对路径"));
}

#[test]
fn full_stack_source_facts_are_read_only_and_reject_undeclared_inputs() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let marker = directory.path().join("original");
    std::fs::write(&marker, b"unchanged").unwrap();
    let request = json!({"operation":"full-stack-source-evidence","write":false,
        "arguments":{"backend":backend,"directory":directory.path(),"ci":{"run_id":null,"attempt":null}}});
    assert_eq!(
        execute(&serde_json::to_vec(&request).unwrap()).unwrap(),
        Value::Null
    );
    let mut changed = request.clone();
    changed["write"] = json!(true);
    assert!(execute(&serde_json::to_vec(&changed).unwrap()).is_err());
    let mut changed = request.clone();
    changed["arguments"]["source"] = json!({"forged":true});
    assert!(execute(&serde_json::to_vec(&changed).unwrap()).is_err());
    let mut changed = request.clone();
    changed["arguments"]["ci"]["run_id"] = json!(1);
    assert!(execute(&serde_json::to_vec(&changed).unwrap()).is_err());
    let mut changed = request;
    changed["arguments"]["backend"] = json!("relative-backend");
    assert!(execute(&serde_json::to_vec(&changed).unwrap()).is_err());
    assert_eq!(std::fs::read(marker).unwrap(), b"unchanged");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn full_stack_binaries_read_requires_exact_roles_and_ordinary_absolute_paths() {
    use std::fs;

    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let manifest = directory.path().join("binaries.json");
    let mut binaries = json!({});
    for role in [
        "ryframe",
        "ryframe-worker",
        "ryframe-reset",
        "ryframe-migrate",
    ] {
        let path = directory.path().join(format!("{role} 产物.exe"));
        fs::write(&path, role.as_bytes()).unwrap();
        binaries[role] = json!(path);
    }
    let original = serde_json::to_string(&binaries).unwrap();
    fs::write(&manifest, &original).unwrap();
    let request = json!({"operation":"full-stack-binaries-read","write":false,
        "arguments":{"directory":directory.path()}});
    let call = |request: &Value| execute(&serde_json::to_vec(request).unwrap());
    let reject = |request: &Value, expected: &str| {
        let error = call(request).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    };
    assert_eq!(call(&request).unwrap(), binaries);
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 5);

    let mut changed = request.clone();
    changed["write"] = json!(true);
    reject(&changed, "写入授权");
    let mut changed = request.clone();
    changed["arguments"]["output"] = json!(manifest);
    reject(&changed, "字段");
    let mut changed = request.clone();
    changed["arguments"]["directory"] = json!("relative");
    reject(&changed, "绝对路径");
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original);

    let duplicate = format!(
        "{},\"ryframe\":{}}}",
        original.strip_suffix('}').unwrap(),
        binaries["ryframe"]
    );
    fs::write(&manifest, &duplicate).unwrap();
    reject(&request, "重复 JSON 字段");
    assert_eq!(fs::read_to_string(&manifest).unwrap(), duplicate);

    let mut missing = binaries.clone();
    missing.as_object_mut().unwrap().remove("ryframe-reset");
    let raw = serde_json::to_string(&missing).unwrap();
    fs::write(&manifest, &raw).unwrap();
    reject(&request, "字段缺失");
    assert_eq!(fs::read_to_string(&manifest).unwrap(), raw);

    for (path, expected) in [
        (json!("relative.exe"), "绝对路径"),
        (json!(directory.path().join("missing.exe")), "路径不存在"),
        (json!(directory.path()), "普通文件"),
    ] {
        let mut changed = binaries.clone();
        changed["ryframe-migrate"] = path;
        let raw = serde_json::to_string(&changed).unwrap();
        fs::write(&manifest, &raw).unwrap();
        reject(&request, expected);
        assert_eq!(fs::read_to_string(&manifest).unwrap(), raw);
    }
}

#[test]
fn native_source_reader_accepts_published_local_pair_and_rejects_rebound_commit() {
    use crate::recovery::{
        clone::native::json as documents, fixture_control::source_pair,
        full_stack::build::tests::Fixture,
    };
    use std::fs;

    let fixture = Fixture::new();
    let backend = &fixture.context.root;
    fs::write(backend.join(".git/info/exclude"), b"/.local-tests/\n").unwrap();
    let business = fixture.business();
    let directory = backend.join(".local-tests/reference-fixture/runtime");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("source-pair.json");
    let pair = source_pair::publish(backend, &path).unwrap();
    let original = fs::read(&path).unwrap();
    assert_eq!(pair["sources"], business["sources"]);
    assert!(source_pair::publish(backend, &path).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);

    let request = serde_json::to_vec(
        &json!({"operation":"full-stack-source-evidence","write":false,
        "arguments":{"backend":backend,"directory":directory,
            "ci":{"run_id":"unrelated-ci-run","attempt":"99"}}}),
    )
    .unwrap();
    let observed = execute(&request).unwrap();
    let source = &observed["source_pair"];
    assert_eq!(source["kind"], "reference-fixture-source-pair");
    assert_eq!(source["sources"], pair["sources"]);
    assert_eq!(source["fixture_receipt"], pair["fixture_receipt"]);
    assert!(source.get("run_id").is_none() && source.get("attempt").is_none());
    assert_eq!(fs::read(&path).unwrap(), original);

    let mut rebound = pair;
    rebound["sources"]["backend"]["head"] = json!("c".repeat(40));
    documents::write_replace(&path, &rebound).unwrap();
    let rejected = fs::read(&path).unwrap();
    let error = execute(&request).unwrap_err().to_string();
    assert!(error.contains("未绑定当前 fixture 收据和来源"), "{error}");
    assert_eq!(fs::read(path).unwrap(), rejected);
}

#[test]
fn inventory_capture_requires_local_write_authorization_and_keeps_failed_inputs() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("inventory-port-")
        .tempdir_in(local)
        .unwrap();
    let output = directory.path().join("inventory");
    let marker = directory.path().join("original.json");
    std::fs::write(&marker, "original-input").unwrap();
    let mut request = json!({"operation":"copy-side-inventory-capture","write":false,
        "arguments":{"evidence_root":backend,"backend":backend,"output":output,"side":"source",
            "plan":{},"maintenance":null,"configuration":null,"app_env":"prod"}});
    assert!(execute(&serde_json::to_vec(&request).unwrap()).is_err());
    assert!(!output.exists());
    request["write"] = json!(true);
    request["arguments"]["sql"] = json!("DELETE FROM unrelated");
    assert!(execute(&serde_json::to_vec(&request).unwrap()).is_err());
    assert!(!output.exists());
    request["arguments"].as_object_mut().unwrap().remove("sql");
    assert!(execute(&serde_json::to_vec(&request).unwrap()).is_err());
    let failure = crate::recovery::clone::native::json::read(&output.join("failure.json")).unwrap();
    assert_eq!(failure["remote_writes"], 0);
    assert_eq!(failure["stage"], "inputs");
    assert!(!output.join("inventory.json").exists());
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "original-input");
    let original_failure = std::fs::read(output.join("failure.json")).unwrap();
    assert!(execute(&serde_json::to_vec(&request).unwrap()).is_err());
    assert_eq!(
        std::fs::read(output.join("failure.json")).unwrap(),
        original_failure
    );
}

#[test]
fn reboot_observation_protocol_rejects_write_authorization_and_caller_supplied_facts() {
    let mut request = json!({"operation":"source-generation-reboot-observation","write":true,
        "arguments":{"backend":"missing","directory":"missing"}});
    assert!(
        execute(&serde_json::to_vec(&request).unwrap())
            .unwrap_err()
            .to_string()
            .contains("写入授权")
    );
    request["write"] = json!(false);
    request["arguments"]["boot"] = json!({"boot_ticks":1});
    assert!(
        execute(&serde_json::to_vec(&request).unwrap())
            .unwrap_err()
            .to_string()
            .contains("字段")
    );
}

#[expect(
    dead_code,
    reason = "消费者契约须导入完整产品键工厂；其中三个租户 namespace 函数不属于本授权缓存契约"
)]
#[path = "../../../../../../crates/ryframe/src/boot/authorization_cache_keyspace.rs"]
mod product_authorization_keyspace;

#[path = "business_catalog.rs"]
mod business_catalog;
#[path = "dataset_authority.rs"]
mod dataset_authority;

#[path = "../../../../../src/recovery/source/evidence/tests/staging.rs"]
mod staging;

#[path = "../../../../../src/recovery/source/evidence/tests/source_inventory.rs"]
mod source_inventory;

fn subjects() -> Value {
    Value::Array((0..11).map(|index| json!({
        "tenant_id":if index == 0 {"system".into()} else {format!("origin-unit-{index:02}")},
        "user_id":(9_007_199_254_740_993_u64 + index).to_string(),
        "user_authorization_version":index,
    })).collect())
}

fn result_request() -> Value {
    let subjects = subjects();
    let lineage = json!({"verification":{"scope_id":"seed-unit"},
        "scopes":{"origin_tenant_scope_id":"origin-unit"},
        "scale":{"tenants":11,"post_samples":33,"business_objects":256},
        "tenants":subjects.as_array().unwrap().iter().enumerate().map(|(index, subject)| json!({
            "tenant_id":subject["tenant_id"],"username":format!("user-{index}")
        })).collect::<Vec<_>>()});
    json!({"lineage":lineage,"start":{"sha256":"a".repeat(64)},"lineage_descriptor":{"sha256":"b".repeat(64)},
        "value":{"format_version":1,"kind":"restore-source-existing-verification",
            "status":"source_existing_data_verified","scope_id":"seed-unit",
            "origin_tenant_scope_id":"origin-unit",
            "lineage_sha256":crate::recovery::clone::native::digest::plan_hash(&lineage).unwrap(),
            "actions":{"business":"read_only","objects":"read_only","session":"login_logout"},
            "restore_success":false,"subjects":subjects,"tenants":11,"posts":33,"files":256,
            "source_generation_sha256":"a".repeat(64),"lineage_file_sha256":"b".repeat(64)}})
}

#[test]
fn exact_result_binds_all_eleven_subjects_start_lineage_and_business_counts() {
    let request = result_request();
    assert_eq!(model::result(&request).unwrap(), request["value"]);
    for (path, value) in [
        ("/value/subjects/1/tenant_id", json!("other")),
        ("/value/subjects/1/user_id", json!("9223372036854775808")),
        ("/value/subjects/1/user_id", json!("01")),
        ("/value/subjects/1/user_authorization_version", json!(-1)),
        (
            "/value/subjects/1/user_authorization_version",
            json!(2147483648_u64),
        ),
        ("/value/source_generation_sha256", json!("f".repeat(64))),
        ("/value/lineage_file_sha256", json!("f".repeat(64))),
        ("/value/posts", json!(32)),
        ("/value/format_version", json!(true)),
    ] {
        let mut changed = request.clone();
        *changed.pointer_mut(path).unwrap() = value;
        assert!(model::result(&changed).is_err(), "{path}");
    }
    let mut changed = request;
    changed["value"]["secret"] = json!("do-not-leak");
    assert!(model::result(&changed).is_err());
}

#[test]
fn cache_plan_requires_exact_namespace_unique_subjects_and_33_registered_keys() {
    let args = json!({"namespace":"ryframe:{seed-unit}:","subjects":subjects()});
    let plan = model::cache_plan(&args).unwrap();
    assert_eq!(plan.as_array().unwrap().len(), 11);
    assert_eq!(
        plan[1]["keys"]["snapshot"],
        "ryframe:{seed-unit}:authorization:{origin-unit-01}:user:9007199254740994:snapshots:v4"
    );
    // 产品 RedisNamespace::key 的物理规则由 adapters 独立契约覆盖：只剥离一次逻辑 ryframe:。
    // 此处直接调用产品纯键函数，避免来源计划与自身错误 fixture 相互证明。
    let namespace = args["namespace"].as_str().unwrap();
    for row in plan.as_array().unwrap() {
        let tenant = row["tenant_id"].as_str().unwrap();
        let user = row["user_id"].as_str().unwrap().parse::<i64>().unwrap();
        let logical = [
            (
                "tenant_epoch",
                product_authorization_keyspace::tenant_epoch_key(tenant),
            ),
            (
                "user_version",
                product_authorization_keyspace::user_version_key(tenant, user),
            ),
            (
                "snapshot",
                product_authorization_keyspace::snapshot_hash_key(tenant, user),
            ),
        ];
        assert_eq!(row["keys"].as_object().unwrap().len(), 3);
        for (role, key) in logical {
            let physical = format!("{namespace}{}", key.strip_prefix("ryframe:").unwrap());
            assert_eq!(row["keys"][role], physical, "{role}");
        }
    }
    for namespace in ["", "ryframe:seed-unit:", "ryframe:{bad scope}:"] {
        let mut changed = args.clone();
        changed["namespace"] = json!(namespace);
        assert!(model::cache_plan(&changed).is_err());
    }
    let mut changed = args.clone();
    changed["subjects"][1] = changed["subjects"][0].clone();
    assert!(model::cache_plan(&changed).is_err());
    changed = args;
    changed["subjects"].as_array_mut().unwrap().pop();
    assert!(model::cache_plan(&changed).is_err());
}

fn database(key: &str, login_rows: u64, hash: &str) -> Value {
    json!({"binding":{"key":key},"target":{"database":{"key":key,
        "placements":[{"tenant_id":"system"}],"tables":[
            {"table":"sys_login_info","rows":login_rows,"sha256":hash.repeat(64)},
            {"table":"sys_oper_log","rows":7,"sha256":"a".repeat(64)},
            {"table":"sys_outbox_event","rows":0,"sha256":"a".repeat(64)},
            {"table":"sys_post","rows":100000,"sha256":"a".repeat(64)}]},"preserved_tables":[]}})
}

fn images() -> Value {
    let before = json!({"databases":{
        "shared-control":database("shared-control",5,"a"),"shared":database("shared",5,"a"),
        "dedicated-a":database("dedicated-a",5,"a"),"dedicated-b":database("dedicated-b",5,"a")},
        "schema":[],"objects":{"uploads":{}},"owners":[],"redis":{"keys":["owner"]},"storage":{}});
    let mut after = before.clone();
    after["databases"]["shared-control"] = database("shared-control", 16, "b");
    json!({"before":before,"after":after})
}

#[test]
fn write_effects_accept_only_eleven_new_logins_with_zero_other_mutations() {
    let args = images();
    assert_eq!(
        model::write_effects(&args).unwrap(),
        json!({
        "api_requests":{"login":11,"logout":11},
        "database_rows":{"sys_login_info":11,"sys_oper_log":0,"sys_outbox_event":0},
        "authorization_cache":{"created_and_removed_keys":33},"business_mutations":0,"object_mutations":0})
    );
    for (path, value) in [
        ("/after/objects/uploads", json!({"unknown":{}})),
        ("/after/redis/keys", json!(["owner", "unknown"])),
        ("/after/schema", json!(["drift"])),
        ("/after/owners", json!(["drift"])),
        ("/after/storage", json!({"endpoint":"changed"})),
        (
            "/after/databases/shared/target/database/tables/3/rows",
            json!(1),
        ),
        (
            "/after/databases/shared-control/target/database/tables/1/sha256",
            json!("b".repeat(64)),
        ),
        (
            "/after/databases/shared-control/target/database/tables/0/rows",
            json!(15),
        ),
        (
            "/after/databases/shared-control/target/preserved_tables",
            json!([{"table":"sys_login_info"}]),
        ),
        (
            "/after/databases/shared-control/target/database/placements",
            json!([]),
        ),
    ] {
        let mut changed = args.clone();
        *changed.pointer_mut(path).unwrap() = value;
        assert!(model::write_effects(&changed).is_err(), "{path}");
    }
}

fn login_request() -> Value {
    let result = result_request();
    let rows = result["lineage"]["tenants"].as_array().unwrap().iter().enumerate().map(|(index, tenant)| json!({
        "id":(101+index).to_string(),"tenant_id":tenant["tenant_id"],"user_name":format!("user-{index}"),
        "ipaddr":format!("198.18.20.{}",index+1),"login_location":null,"browser":"Native",
        "os":null,"status":"1","msg":null,"login_time":"2026-09-12T01:00:01.000000",
    })).collect::<Vec<_>>();
    json!({"old_before":"6f6c64","old_after":"6f6c64","upper_id":100,"new_rows":rows,
        "lineage":result["lineage"],"subjects":subjects(),
        "started_at":"2026-09-12T01:00:00+00:00","completed_at":"2026-09-12T01:00:02Z"})
}

#[test]
fn login_audit_preserves_old_bytes_and_binds_each_new_row_identity_time_and_order() {
    let args = login_request();
    let value = model::login_audit(&args).unwrap();
    assert_eq!(value["upper_id"], "100");
    assert_eq!(value["new_rows"], args["new_rows"]);
    for (path, value) in [
        ("/old_after", json!("changed")),
        ("/upper_id", json!(true)),
        ("/new_rows/0/tenant_id", json!("other")),
        ("/new_rows/0/status", json!("0")),
        (
            "/new_rows/0/login_time",
            json!("2026-09-12T01:00:03.000000"),
        ),
        ("/new_rows/0/id", json!("102")),
        ("/new_rows/0/id", json!("0101")),
        ("/new_rows/0/browser", json!(1)),
        ("/started_at", json!("2026-09-12T01:00:00")),
    ] {
        let mut changed = args.clone();
        *changed.pointer_mut(path).unwrap() = value;
        assert!(model::login_audit(&changed).is_err(), "{path}");
    }
}

#[test]
fn source_evidence_protocol_rejects_duplicates_unknown_fields_and_nonstandard_values() {
    for bytes in [
        b"{\"operation\":\"cache-plan\",\"operation\":\"result\",\"arguments\":{}}".as_slice(),
        b"{\"operation\":\"unknown\",\"arguments\":{}}",
        b"{\"operation\":\"result\",\"arguments\":{},\"extra\":true}",
        b"{\"operation\":NaN,\"arguments\":{}}",
        b"[]",
        b"",
    ] {
        assert!(execute(bytes).is_err());
    }
    for arguments in [
        json!({"backend":"unused","request":{},"extra":true}),
        json!({"backend":7,"request":{}}),
        json!({"backend":"unused"}),
    ] {
        let request = json!({"operation":"source-reference-projection", "write":false,
            "arguments":arguments});
        assert!(execute(&serde_json::to_vec(&request).unwrap()).is_err());
    }
}

#[test]
fn source_evidence_protocol_requires_exact_write_authorization_before_dispatch() {
    let request = json!({"operation":"cache-plan","write":false,
        "arguments":{"namespace":"ryframe:{seed-unit}:","subjects":subjects()}});
    assert_eq!(
        execute(&serde_json::to_vec(&request).unwrap())
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        11
    );
    for (operation, write) in [
        ("cache-plan", json!(true)),
        ("staging-create", json!(false)),
        ("staging-create", json!(1)),
        ("source-reference-projection", json!(true)),
        ("reference-sql-stream-create", json!(false)),
        ("reference-sql-stream-close", json!(false)),
        ("reference-sql-stream-read", json!(true)),
    ] {
        let request = json!({"operation":operation,"write":write,"arguments":{}});
        assert!(
            execute(&serde_json::to_vec(&request).unwrap())
                .unwrap_err()
                .to_string()
                .contains("写入授权")
        );
    }
}

#[test]
fn copy_step_ids_preserve_ledger_hashes_and_fail_closed_on_inexact_vectors() {
    let mut request = json!({"operation":"reference-copy-step-ids","write":false,
        "arguments":{"databases":[{"key":"shared-control"},{"key":"dedicated-1"}],
            "objects":[{"bucket":"uploads","target_key":"side/目录/a.json"},
                {"bucket":"exports","target_key":"side/目录/a.json"}]}});
    let call = |value: &Value| execute(&serde_json::to_vec(value).unwrap());
    // 原逐项摘要协议输出的固定 ID，可直接与既有账本键比较。
    assert_eq!(
        call(&request).unwrap(),
        json!({"databases":["database-075921cce671fc36b9360281d8b631a2",
                            "database-2e23488b489f7e19b53a25393ac35d28"],
               "objects":["object-87c0fda0cfaf8d51466fd3b5320075ef",
                          "object-a19e6131c2dc22a819ac47fcde9093d4"]})
    );

    for field in ["databases", "objects"] {
        let mut duplicated = request.clone();
        let items = duplicated["arguments"][field].as_array_mut().unwrap();
        let repeated = items[0].clone();
        items.push(repeated);
        assert!(
            call(&duplicated)
                .unwrap_err()
                .to_string()
                .contains("重复或碰撞")
        );
    }
    for malformed in [
        json!({"databases":[]}),
        json!({"databases":[{"key":false}],"objects":[]}),
        json!({"databases":[],"objects":[{"bucket":"uploads","key":"side/a"}]}),
        json!({"databases":[],"objects":[{"bucket":"uploads","target_key":"side/a","artifact":{}}]}),
    ] {
        let mut invalid = request.clone();
        invalid["arguments"] = malformed;
        assert!(call(&invalid).is_err());
    }
    request["arguments"] = json!({"databases":[],"objects":[]});
    assert_eq!(
        call(&request).unwrap(),
        json!({"databases":[],"objects":[]})
    );
    request["write"] = json!(true);
    assert!(call(&request).unwrap_err().to_string().contains("写入授权"));
}
