use std::{fs, path::PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;

pub(super) fn temporary() -> tempfile::TempDir {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/reference-coordination-native");
    fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}

pub(super) struct Fixture {
    _directory: tempfile::TempDir,
    pub(super) plan: Value,
    pub(super) root: PathBuf,
    pub(super) manifest: Value,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = temporary();
        let work = directory.path().join("reference 中文");
        let root = work.join("backup");
        let mut plan = json!({"id":"restore-case","work_dir":work,"target_side":"base",
            "source":{"scope_id":"source","databases":[]},
            "target":{"scope_id":"target","s3":{"endpoint":"http://127.0.0.1:9000"},"databases":[]}});
        for side in ["source", "target"] {
            plan[side]["databases"] = json!([{"key":"control","kind":"combined","mode":"shared",
                "database":format!("{side}_control"),"server_uuid":"uuid-1"}]);
        }
        formalize(&mut plan, directory.path());
        files::work(&json!({"plan":plan}), true).unwrap();
        fs::create_dir(&root).unwrap();
        let observed = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
            .unwrap()
            .to_rfc3339_opts(SecondsFormat::AutoSi, true);
        let mut manifest = json!({"id":"restore-case","scope_id":"source","source_sha":"a".repeat(40),
            "quiesced_at":observed,"captured_at":observed,
            "control_schema_fingerprint":"a".repeat(16),"tenant_schema_fingerprint":"b".repeat(64),
            "databases":[{"key":"control","kind":"combined",
                "shared":true,"database":"source_control","server_uuid":"uuid-1",
                "placements":[{"tenant_id":"tenant","generation":1,"switch_token":"initial"}],
                "tables":[{"table":"sys_post","rows":1,"sha256":"b".repeat(64)}]}],"objects":[],"artifacts":[]});
        let sql = root.join("databases/control.sql");
        fs::create_dir(sql.parent().unwrap()).unwrap();
        fs::write(
            &sql,
            "-- ryframe-reference-data-v1\nINSERT INTO `sys_post` (`id`) VALUES (1);\n",
        )
        .unwrap();
        let mut artifacts = vec![artifact(&root, "databases/control.sql", "db:control")];
        let mut objects = Vec::new();
        for bucket in inputs::domain::BUCKETS {
            let folder = root.join(format!("objects/{bucket}"));
            fs::create_dir_all(&folder).unwrap();
            let mut entries = Vec::new();
            let mut index = Vec::new();
            if bucket == "uploads" {
                fs::write(folder.join("one.bin"), b"restored-file").unwrap();
                let entry = json!({"key":"source/tenant/file.txt","bytes":13,"sha256":hex::encode(Sha256::digest(b"restored-file"))});
                entries.push(entry.clone());
                let mut value = entry;
                value["file"] = json!("objects/uploads/one.bin");
                value["content_type"] = json!("text/plain");
                index.push(value);
                artifacts.push(artifact(
                    &root,
                    "objects/uploads/one.bin",
                    "objects:uploads",
                ));
            }
            let relative = format!("objects/{bucket}/index.json");
            native::json::write_new(&root.join(&relative), &json!({"entries":index})).unwrap();
            artifacts.push(artifact(&root, &relative, &format!("objects:{bucket}")));
            objects.push(json!({"bucket":bucket,"prefix":"source/","entries":entries}));
        }
        manifest["artifacts"] = json!(artifacts);
        manifest["objects"] = json!(objects);
        native::json::write_new(&root.join("manifest.json"), &manifest).unwrap();
        Self {
            _directory: directory,
            plan,
            root,
            manifest,
        }
    }

    fn copy(&self, id: &str) -> Value {
        files::copy(&json!({"work":self.plan["work_dir"],"root":self.root,"manifest":self.manifest,"copy_id":id})).unwrap()
    }

    fn receipt(&self) -> Value {
        receipts::start(&json!({"plan":self.plan,"command":"backup","copy_id":null,"root":null,"receipt":null,"bindings":null})).unwrap()
    }
}

fn artifact(root: &std::path::Path, relative: &str, resource: &str) -> Value {
    let digest = native::digest::file(&root.join(relative)).unwrap();
    json!({"relative_path":relative,"resource":resource,"bytes":digest.bytes,"sha256":digest.sha256})
}

pub(super) fn formalize(plan: &mut Value, root: &std::path::Path) {
    let tool = root.join("tool.exe");
    fs::write(&tool, b"fixture-tool").unwrap();
    let defaults = root.join("client.cnf");
    fs::write(
        &defaults,
        "[client]\nhost=127.0.0.1\nuser=fixture\npassword=fixture\n",
    )
    .unwrap();
    let config = root.join("config");
    fs::create_dir(&config).unwrap();
    let settings = config.join("app.test.toml");
    fs::write(
        &settings,
        "[object_storage]\nendpoint='http://127.0.0.1:9000'\nregion='us-east-1'\naccess_key='fixture-access'\nsecret_key='fixture-secret'\n",
    )
    .unwrap();
    let digest = native::digest::file(&settings).unwrap();
    let mut binding = json!({"app_env":"test","directory":config,"files":{"app.test.toml":{"bytes":digest.bytes,"sha256":digest.sha256}}});
    binding["sha256"] = json!(native::digest::plan_hash(&binding).unwrap());
    plan["format_version"] = json!(1);
    let sha = native::digest::file(&tool).unwrap().sha256;
    plan["tools"] =
        json!({"mysql":{"path":tool,"sha256":sha},"mysqldump":{"path":tool,"sha256":sha}});
    for side in ["source", "target"] {
        plan[side]["runtime_dir"] = json!(root.join(side));
        plan[side]["api_url"] = json!("http://127.0.0.1:3000");
        plan[side]["s3"] = json!({"endpoint":"http://127.0.0.1:9000","region":"us-east-1","configuration":binding});
        let database = &mut plan[side]["databases"][0];
        database["defaults_file"] = json!(defaults);
        database["defaults_sha256"] = json!(native::digest::file(&defaults).unwrap().sha256);
    }
    plan["target"]["worker_ready_url"] = json!("http://127.0.0.1:3001/readyz");
    plan["target"]["frontend_url"] = json!("http://127.0.0.1:4174");
}

fn raw_request(root: &std::path::Path, operation: &str, writes: bool) -> Value {
    json!({"format_version":1,"kind":"ryframe-xtask-recovery-reference","request":{
        "backend_dir":root,"format_version":1,"operation":operation,"plan":root.join("plan.json"),"write":writes}})
}

fn parsed(value: &Value) -> Result<Value> {
    protocol::request(&json!({"raw":serde_json::to_string(value)?}))
}

#[test]
fn request_rejects_unknown_cross_family_duplicate_invalid_version_and_parent_jump() {
    let directory = temporary();
    let mut valid = raw_request(directory.path(), "check-existing", false);
    valid["request"]["side"] = json!("source");
    assert_eq!(parsed(&valid).unwrap()["side"], "source");
    for (field, value) in [
        ("other", json!(true)),
        ("copy_id", json!("other")),
        ("side", json!("other")),
        ("write", json!(true)),
        ("format_version", json!(true)),
        (
            "plan",
            json!(directory.path().join("evidence/../plan.json")),
        ),
    ] {
        let mut invalid = valid.clone();
        invalid["request"][field] = value;
        assert!(parsed(&invalid).is_err(), "{field}");
    }
    let raw = serde_json::to_string(&valid).unwrap().replace(
        "\"side\":\"source\"",
        "\"side\":\"source\",\"side\":\"target\"",
    );
    assert!(protocol::request(&json!({"raw":raw})).is_err());
    assert!(protocol::request(&json!({"raw":"x".repeat(65_537)})).is_err());
    assert!(protocol::request(&json!({"raw":"{\"private\": invalid}"})).is_err());
}

#[test]
fn every_operation_requires_exact_paths_modes_and_write_before_io() {
    let directory = temporary();
    for operation in [
        "plan",
        "check-dataset",
        "dataset",
        "backup",
        "restore",
        "copy",
        "damage",
    ] {
        let writes = matches!(
            operation,
            "dataset" | "backup" | "restore" | "copy" | "damage"
        );
        let mut invalid = raw_request(directory.path(), operation, writes);
        invalid["request"]["side"] = json!("source");
        assert!(parsed(&invalid).is_err(), "{operation}");
        invalid["request"].as_object_mut().unwrap().remove("side");
        invalid["request"]["write"] = json!(!writes);
        assert!(parsed(&invalid).is_err());
    }
    let mut backup = raw_request(directory.path(), "backup", true);
    for field in ["inventory", "source_generation", "source_export_result"] {
        backup["request"][field] = json!(directory.path().join(field));
    }
    assert!(parsed(&backup).is_ok());
    for field in ["inventory", "source_generation", "source_export_result"] {
        let mut missing = backup.clone();
        missing["request"].as_object_mut().unwrap().remove(field);
        assert!(parsed(&missing).is_err());
    }
    let mut restore = raw_request(directory.path(), "restore", true);
    for field in [
        "backup_root",
        "record",
        "target_plan",
        "runtime_registration",
    ] {
        restore["request"][field] = json!(directory.path().join(field));
    }
    assert!(parsed(&restore).is_ok());
    restore["request"]
        .as_object_mut()
        .unwrap()
        .remove("target_plan");
    assert!(parsed(&restore).is_err());
    assert!(!directory.path().join("plan.json").exists());
}

#[test]
fn dataset_protocol_is_bounded_exact_and_credential_free() {
    let directory = temporary();
    let value = protocol::dataset(&json!({"backend":directory.path(),"plan":directory.path().join("plan"),"preflight":directory.path().join("preflight")})).unwrap();
    let parsed: Value = serde_json::from_str(value.as_str().unwrap()).unwrap();
    assert_eq!(parsed.as_object().unwrap().len(), 8);
    assert_eq!(parsed["kind"], "ryframe-xtask-recovery-dataset-prepare");
    assert_eq!(parsed["side"], "source");
    assert_eq!(parsed["write"], true);
    assert!(parsed["verify_existing"].is_null());
    assert!(!value.as_str().unwrap().contains("secret"));
    assert!(
        protocol::dataset(&json!({"backend":"relative","plan":"relative","preflight":"relative"}))
            .is_err()
    );
    for timeout in [
        Value::Null,
        json!(0),
        json!(-1),
        json!(604801),
        json!(true),
        json!(1.5),
        json!("21600"),
    ] {
        assert!(
            super::super::reference::request(
                "reference-dataset-timeout",
                &json!({"plan":{"dataset":{"timeout_seconds":timeout}}})
            )
            .is_err()
        );
    }
    assert_eq!(
        super::super::reference::request(
            "reference-dataset-timeout",
            &json!({"plan":{"dataset":{"timeout_seconds":21600}}})
        )
        .unwrap(),
        21600
    );
}

#[test]
fn work_is_idempotent_read_only_verify_creates_nothing_and_owner_drift_is_rejected() {
    let fixture = Fixture::new();
    let args = json!({"plan":fixture.plan});
    let owner = fixture.root.parent().unwrap().join("reference-owner.json");
    let before = fs::read(&owner).unwrap();
    files::work(&args, false).unwrap();
    files::work(&args, true).unwrap();
    assert_eq!(fs::read(owner).unwrap(), before);
    let mut changed = fixture.plan.clone();
    changed["id"] = json!("another");
    assert!(files::work(&json!({"plan":changed}), true).is_err());
    changed["work_dir"] = json!(fixture._directory.path().join("absent"));
    assert!(files::work(&json!({"plan":changed}), false).is_err());
    assert!(!fixture._directory.path().join("absent").exists());
}

#[test]
fn independent_copy_damage_detects_missing_corrupt_and_preserves_original() {
    let fixture = Fixture::new();
    for missing in [false, true] {
        let copied = fixture.copy(if missing {
            "bad-missing"
        } else {
            "bad-corrupt"
        });
        let root = PathBuf::from(copied["backup_root"].as_str().unwrap());
        let manifest = native::json::read(&root.join("manifest.json")).unwrap();
        files::damage(
            &json!({"work":fixture.plan["work_dir"],"root":root,"manifest":manifest,
            "relative":"databases/control.sql","missing":missing}),
        )
        .unwrap();
        assert!(inputs::inventory::verify_artifacts(&root, &manifest).is_err());
        inputs::inventory::verify_artifacts(&fixture.root, &fixture.manifest).unwrap();
    }
    assert!(files::copy(&json!({"work":fixture.plan["work_dir"],"root":fixture.root,"manifest":fixture.manifest,"copy_id":"restore-case"})).is_err());
    assert!(
        files::damage(
            &json!({"work":fixture.plan["work_dir"],"root":fixture.root,"manifest":fixture.manifest,
        "relative":"databases/control.sql","missing":false})
        )
        .is_err()
    );
}

#[test]
fn damage_rejects_shared_hardlinks_and_unknown_artifacts() {
    let fixture = Fixture::new();
    let copied = fixture.copy("hardlink");
    let root = PathBuf::from(copied["backup_root"].as_str().unwrap());
    let manifest = native::json::read(&root.join("manifest.json")).unwrap();
    let target = root.join("databases/control.sql");
    fs::remove_file(&target).unwrap();
    fs::hard_link(fixture.root.join("databases/control.sql"), &target).unwrap();
    for relative in [
        "databases/control.sql",
        "manifest.json",
        "../backup/databases/control.sql",
    ] {
        assert!(
            files::damage(
                &json!({"work":fixture.plan["work_dir"],"root":root,"manifest":manifest,
            "relative":relative,"missing":false})
            )
            .is_err()
        );
    }
    inputs::inventory::verify_artifacts(&fixture.root, &fixture.manifest).unwrap();
}

#[test]
fn receipt_is_create_only_tamper_evident_and_has_one_terminal_outcome() {
    let fixture = Fixture::new();
    let receipt = fixture.receipt();
    assert!(receipts::start(&json!({"plan":fixture.plan,"command":"backup","copy_id":null,"root":null,"receipt":null,"bindings":null})).is_err());
    let args =
        json!({"receipt":receipt,"result":{"done":true},"failure":null,"resource_images":null});
    let complete = receipts::finish(&args).unwrap();
    assert_eq!(complete["status"], "completed");
    assert!(receipts::finish(&args).is_err());
    let other = Fixture::new();
    let receipt = other.receipt();
    fs::write(
        receipt["path"].as_str().unwrap(),
        "{\"status\":\"running\"}",
    )
    .unwrap();
    assert!(
        receipts::finish(
            &json!({"receipt":receipt,"result":{"done":true},"failure":null,"resource_images":null})
        )
        .is_err()
    );
}

#[test]
fn failed_receipt_preserves_resource_images_and_never_contains_success_or_retry() {
    let fixture = Fixture::new();
    let receipt = fixture.receipt();
    let failure = json!({"error_type":"TimeoutExpired","message":"外部调用结果未知","stderr":"password=[REDACTED]"});
    let value = receipts::finish(&json!({"receipt":receipt,"result":null,"failure":failure,"resource_images":{"before":{"sha256":"a"}}})).unwrap();
    assert_eq!(value["status"], "failed");
    assert!(value.get("result").is_none());
    assert_eq!(value["resource_images"]["before"]["sha256"], "a");
    assert!(value.get("automatic_retry_permitted").is_none());
}

#[test]
fn resource_preflight_checks_all_artifacts_before_returning_ordered_tasks() {
    let fixture = Fixture::new();
    let args = json!({"plan":fixture.plan,"root":fixture.root,"manifest":fixture.manifest});
    let tasks = model::resources(&args).unwrap();
    assert_eq!(tasks[0]["kind"], "database");
    assert_eq!(tasks[0]["tables"], json!(["sys_post"]));
    assert_eq!(tasks[1]["key"], "target/tenant/file.txt");
    fs::write(fixture.root.join("objects/uploads/one.bin"), b"tampered").unwrap();
    assert!(model::resources(&args).is_err());
    assert!(model::bound_artifact(&json!({"root":fixture.root,"manifest":fixture.manifest,"relative":"objects/uploads/one.bin"})).is_err());
}

#[test]
fn retention_uses_capture_time_and_dataset_drift_cannot_publish_success() {
    let fixture = Fixture::new();
    let mut inventory = fixture.manifest.clone();
    let captured =
        DateTime::from_timestamp_micros((Utc::now() - Duration::days(2)).timestamp_micros())
            .unwrap();
    let captured_text = json!(captured.to_rfc3339_opts(SecondsFormat::AutoSi, true));
    inventory["quiesced_at"] = captured_text.clone();
    inventory["captured_at"] = captured_text;
    let value = model::manifest(&json!({"plan":fixture.plan,"inventory":inventory,"artifacts":fixture.manifest["artifacts"],"root":fixture.root})).unwrap();
    assert_eq!(
        value["retention_until"],
        (captured + Duration::days(7)).to_rfc3339_opts(SecondsFormat::AutoSi, true)
    );
    let completed = DateTime::parse_from_rfc3339(value["completed_at"].as_str().unwrap()).unwrap();
    assert_eq!(
        value["completed_at"],
        completed
            .to_utc()
            .to_rfc3339_opts(SecondsFormat::AutoSi, true)
    );
    assert!(completed.timestamp_subsec_nanos().is_multiple_of(1_000));
    let args = json!({"preflight":{"same":true},"actual_preflight":{"same":true},"source":{"head":"a"},
        "actual_source":{"head":"a"},"tools":{"revision":1},"actual_tools":{"revision":1},"result":{"records":100000}});
    assert_eq!(model::dataset_result(&args).unwrap()["records"], 100000);
    for field in ["actual_preflight", "actual_source", "actual_tools"] {
        let mut drift = args.clone();
        drift[field] = json!({"changed":true});
        assert!(model::dataset_result(&drift).is_err());
    }
}

#[test]
fn backup_inventory_requires_schema_and_well_formed_placements() {
    let fixture = Fixture::new();
    inputs::inventory::validate(&fixture.plan, &fixture.manifest).unwrap();
    for (pointer, changed) in [
        ("/control_schema_fingerprint", json!("A".repeat(16))),
        ("/tenant_schema_fingerprint", Value::Null),
        ("/databases/0/placements", Value::Null),
        ("/databases/0/placements/0/generation", json!(0)),
        ("/databases/0/tables/0/rows", json!(-1)),
        ("/quiesced_at", json!("9999-01-01T00:00:00Z")),
    ] {
        let mut manifest = fixture.manifest.clone();
        *manifest.pointer_mut(pointer).unwrap() = changed;
        assert!(
            inputs::inventory::validate(&fixture.plan, &manifest).is_err(),
            "{pointer}"
        );
    }
    let mut duplicate = fixture.manifest.clone();
    duplicate["databases"][0]["placements"] = json!([
        {"tenant_id":"tenant","generation":1,"switch_token":"initial"},
        {"tenant_id":"tenant","generation":2,"switch_token":"second"}
    ]);
    assert!(inputs::inventory::validate(&fixture.plan, &duplicate).is_err());
}

#[test]
fn public_local_summary_copy_and_damage_use_the_native_lifecycle() {
    let fixture = Fixture::new();
    let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let plan = fixture._directory.path().join("plan.json");
    native::json::write_new(&plan, &fixture.plan).unwrap();
    let mut request = raw_request(backend, "plan", false);
    request["request"]["plan"] = json!(plan);
    let summary = run_local(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(summary["id"], "restore-case");
    assert!(!summary.to_string().contains("fixture-secret"));
    assert!(
        !fixture
            .root
            .parent()
            .unwrap()
            .join(".reference-lock")
            .exists()
    );
    request["request"]["operation"] = json!("copy");
    request["request"]["write"] = json!(true);
    request["request"]["backup_root"] = json!(fixture.root);
    request["request"]["copy_id"] = json!("public-copy");
    let copied = run_local(&serde_json::to_string(&request).unwrap()).unwrap();
    request["request"]
        .as_object_mut()
        .unwrap()
        .remove("copy_id");
    request["request"]["operation"] = json!("damage");
    request["request"]["backup_root"] = copied["backup_root"].clone();
    request["request"]["artifact"] = json!("databases/control.sql");
    request["request"]["missing"] = json!(false);
    assert_eq!(
        run_local(&serde_json::to_string(&request).unwrap()).unwrap()["injected"],
        "corrupt"
    );
    let failed = run_local(&serde_json::to_string(&request).unwrap());
    assert!(failed.is_err());
    inputs::inventory::verify_artifacts(&fixture.root, &fixture.manifest).unwrap();
    assert!(
        !fixture
            .root
            .parent()
            .unwrap()
            .join(".reference-lock")
            .exists()
    );
}

#[path = "../../../dataset_prepare/reference_schema.rs"]
mod reference_dataset_schema;

#[test]
fn product_hash_has_native_field_order_and_record_rejects_changed_embedded_inputs() {
    let fixture = Fixture::new();
    let captured = fixture.manifest["captured_at"].clone();
    let product = json!({"id":"restore-run","backup_id":"restore-case","scope_id":"target",
        "fault_at":captured,"databases":[{"source_key":"control","target_key":"control","server_uuid":"uuid-1","database":"target_control"}],
        "object_endpoint":"http://127.0.0.1:9000","object_prefix":"target/",
        "api_ready_url":"http://127.0.0.1:3000/readyz","worker_ready_url":"http://127.0.0.1:9200/readyz","frontend_sha":"f".repeat(40)});
    let hash = proof::hash_request(&json!({"value":product})).unwrap();
    let serialized = format!(
        "{{\"id\":\"restore-run\",\"backup_id\":\"restore-case\",\"scope_id\":\"target\",\"fault_at\":{},\"databases\":[{{\"source_key\":\"control\",\"target_key\":\"control\",\"server_uuid\":\"uuid-1\",\"database\":\"target_control\"}}],\"object_endpoint\":\"http://127.0.0.1:9000\",\"object_prefix\":\"target/\",\"api_ready_url\":\"http://127.0.0.1:3000/readyz\",\"worker_ready_url\":\"http://127.0.0.1:9200/readyz\",\"frontend_sha\":\"{}\"}}",
        serde_json::to_string(&captured).unwrap(),
        "f".repeat(40)
    );
    assert_eq!(hash, hex::encode(Sha256::digest(serialized.as_bytes())));
    let record = json!({"plan":product,"plan_hash":hash,"status":"running","started_at":Utc::now().to_rfc3339(),
        "data_verified_at":null,"completed_at":null,"recovered_at":captured,"failure":null});
    let args = json!({"plan":fixture.plan,"manifest":fixture.manifest,"record":record,"product_plan":product});
    proof::record(&args).unwrap();
    for (field, value) in [
        ("extra", json!(true)),
        ("plan_hash", json!("f".repeat(64))),
        ("status", json!("succeeded")),
        ("failure", json!("failed")),
        ("data_verified_at", json!("2026-01-01T00:00:00Z")),
        ("completed_at", captured),
        ("started_at", json!("2000-01-01T00:00:00Z")),
        ("started_at", json!("2999-01-01T00:00:00Z")),
        ("started_at", json!("2026-01-01T00:00:00")),
        ("recovered_at", json!("2000-01-01T00:00:00Z")),
    ] {
        let mut invalid = args.clone();
        invalid["record"][field] = value;
        assert!(proof::record(&invalid).is_err(), "{field}");
    }
    for (field, value) in [
        ("id", json!("other")),
        ("frontend_sha", json!("e".repeat(40))),
        ("fault_at", json!("2026-01-01T00:00:00Z")),
        ("api_ready_url", json!("http://127.0.0.1:3100/readyz")),
        ("worker_ready_url", json!("http://127.0.0.1:19201/readyz")),
    ] {
        let mut invalid = args.clone();
        invalid["record"]["plan"][field] = value;
        invalid["record"]["plan_hash"] =
            proof::hash_request(&json!({"value":invalid["record"]["plan"]})).unwrap();
        assert!(proof::record(&invalid).is_err());
    }
}

#[test]
fn backup_source_requires_same_inventory_build_configuration_generation_and_ordered_times() {
    let fixture = Fixture::new();
    let descriptor =
        json!({"path":fixture.root.join("generation.json"),"bytes":1,"sha256":"a".repeat(64)});
    let source = json!({"head":"a".repeat(40),"clean":true});
    let request = json!({"source":fixture.plan["source"],"tools":{"mysqldump":{"sha256":"b".repeat(64)}},
        "backend_build":{"path":fixture.root.join("build.json"),"bytes":1,"sha256":"c".repeat(64)}});
    let generation = json!({"source":source,"processes":{"api":{"pid":20}},"runtime":{"scope_id":"source"},"physical_binding":{"scope_id":"source"}});
    let mut inventory = fixture.manifest.clone();
    inventory.as_object_mut().unwrap().remove("id");
    let mut plan = fixture.plan.clone();
    plan["tools"] = request["tools"].clone();
    let args = json!({"plan":plan,"inventory":fixture.manifest,
        "identity":{"source_generation":descriptor},"source":{"source_generation":descriptor,"request":request,"generation":generation},
        "exported":{"inventory":inventory,"request":request,"generation":generation},
        "build":{"sources":{"full":{"source":{"snapshot":source}}}},"descriptor":descriptor,
        "runtime":{"verified_at":(Utc::now()-Duration::seconds(2)).to_rfc3339()},
        "quiescence":{"observed_stopped_at":(Utc::now()-Duration::seconds(1)).to_rfc3339()}});
    // 固定采集时间必须晚于实际停止观察。
    let mut args = args;
    let captured = Utc::now().to_rfc3339();
    for field in ["quiesced_at", "captured_at"] {
        args["inventory"][field] = json!(captured);
        args["exported"]["inventory"][field] = json!(captured);
    }
    proof::backup_source(&args).unwrap();
    for pointer in [
        "/exported/inventory/source_sha",
        "/exported/inventory/databases/0/tables/0/rows",
        "/exported/request/source/scope_id",
        "/exported/request/tools/mysqldump/sha256",
        "/exported/request/backend_build/sha256",
        "/exported/generation/source/head",
        "/exported/generation/processes/api/pid",
        "/exported/generation/runtime/scope_id",
        "/exported/generation/physical_binding/scope_id",
        "/source/source_generation",
        "/quiescence/observed_stopped_at",
    ] {
        let mut invalid = args.clone();
        *invalid.pointer_mut(pointer).unwrap() = json!("changed");
        assert!(proof::backup_source(&invalid).is_err(), "{pointer}");
    }
    let mut future = args.clone();
    for field in ["quiesced_at", "captured_at"] {
        future["inventory"][field] = json!("2999-01-01T00:00:00Z");
        future["exported"]["inventory"][field] = json!("2999-01-01T00:00:00Z");
    }
    assert!(proof::backup_source(&future).is_err());
}
