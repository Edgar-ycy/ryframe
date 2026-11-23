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

fn independent_work_fixture() -> Result<(tempfile::TempDir, PathBuf, Value)> {
    let fixture = temporary();
    let backend = crate::recovery::fixture_control::portable_path(&fixture.path().canonicalize()?);
    let local = backend.join(".local-tests");
    fs::create_dir(&local)?;
    let plan = json!({
        "id":"independent-b1",
        "target_side":"candidate",
        "work_dir":local.join("reference"),
    });
    Ok((fixture, backend, plan))
}

#[test]
fn independent_begin_work_rejects_existing_owner_and_active_preview() -> Result<()> {
    let (_fixture, backend, plan) = independent_work_fixture()?;
    let work = path(&plan["work_dir"])?;
    let window = crate::recovery::runtime::dataset_authority::reads::Window::direct()?;
    assert!(local::OwnedWork::claim(&backend, &plan).is_err());
    assert!(!work.exists());
    window.finish(Ok(()))?;
    drop(window);
    files::work(&json!({"plan":plan}), true)?;
    let owner = work.join("reference-owner.json");
    let before = fs::read(&owner)?;
    assert!(local::OwnedWork::claim(&backend, &plan).is_err());
    assert_eq!(fs::read(owner)?, before);
    assert!(!work.join(".reference-lock").exists());
    Ok(())
}

#[test]
fn independent_begin_work_rechecks_the_original_owner_and_live_lock() -> Result<()> {
    let (_fixture, backend, plan) = independent_work_fixture()?;
    let work = path(&plan["work_dir"])?;
    let claim = local::OwnedWork::claim(&backend, &plan)?;
    let owner_path = work.join("reference-owner.json");
    let owner = json!({
        "format_version":1, "id":plan["id"], "plan_sha256":native::digest::plan_hash(&plan)?,
    });
    assert_eq!(
        fs::read(&owner_path)?,
        inputs::document::canonical_bytes(&owner)?
    );
    claim.check(work, &plan)?;
    let directory_identity =
        native::resources::common::device_inode_for(work, &fs::metadata(work)?)?;
    let owner_identity =
        native::resources::common::device_inode_for(&owner_path, &fs::metadata(&owner_path)?)?;
    let original = inputs::document::read(&owner_path, &backend, "夹具原归属")?;
    assert_eq!(
        claim.binding(&backend, &plan)?,
        json!({
            "work_dir":work,
            "directory_identity":{"device":directory_identity.0,"inode":directory_identity.1},
            "owner":inputs::document::binding(&original),
            "owner_identity":{"device":owner_identity.0,"inode":owner_identity.1},
        })
    );
    let mut changed = plan.clone();
    changed["id"] = json!("other");
    assert!(claim.check(work, &changed).is_err());
    assert!(claim.binding(&backend, &changed).is_err());
    assert!(claim.check(&work.join("other"), &plan).is_err());

    let lock = work.join(".reference-lock");
    fs::rename(&lock, work.join("previous-reference-lock"))?;
    fs::create_dir(&lock)?;
    assert!(claim.check(work, &plan).is_err());
    assert!(claim.binding(&backend, &plan).is_err());
    drop(claim);
    assert!(lock.is_dir());
    assert!(local::OwnedWork::claim(&backend, &plan).is_err());
    Ok(())
}

#[test]
fn independent_begin_work_guards_against_owner_and_directory_replacement() -> Result<()> {
    let (_fixture, backend, plan) = independent_work_fixture()?;
    let work = path(&plan["work_dir"])?;
    let claim = local::OwnedWork::claim(&backend, &plan)?;
    let owner = work.join("reference-owner.json");
    let owner_bytes = fs::read(&owner)?;
    match fs::rename(&owner, work.join("previous-owner.json")) {
        Ok(()) => {
            fs::write(&owner, &owner_bytes)?;
            assert!(claim.check(work, &plan).is_err());
        }
        Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32)) => {
            claim.check(work, &plan)?;
        }
        Err(error) => return Err(error.into()),
    }
    drop(claim);

    let (_fixture, backend, plan) = independent_work_fixture()?;
    let work = path(&plan["work_dir"])?;
    let claim = local::OwnedWork::claim(&backend, &plan)?;
    match fs::rename(work, work.with_file_name("previous-reference")) {
        Ok(()) => {
            files::work(&json!({"plan":plan}), true)?;
            let _other = local::Lock::claim(work)?;
            assert!(claim.check(work, &plan).is_err());
        }
        Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32)) => {
            claim.check(work, &plan)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
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
fn independent_b1_reference_only_enters_complete_target_plan_without_creating_work() -> Result<()> {
    let directory = temporary();
    let backend = crate::workspace::root_dir();
    let plan = directory.path().join("independent.json");
    let work = directory.path().join("new-work");
    let missing = directory.path().join("missing-evidence.json");
    native::json::write_new(
        &plan,
        &json!({
            "format_version":1, "kind":"restore-reference-independent-b1-plan",
            "reference_plan":{"target_side":"candidate","work_dir":work},
            "arm_input":null, "source_export":null,
        }),
    )?;
    for (operation, extra) in [
        ("plan", json!({})),
        ("dataset", json!({})),
        (
            "backup",
            json!({"inventory":missing,"source_generation":missing,"source_export_result":missing}),
        ),
        (
            "copy",
            json!({"backup_root":missing,"copy_id":"isolated-copy"}),
        ),
        (
            "damage",
            json!({"backup_root":missing,"artifact":"databases/control.sql","missing":false}),
        ),
    ] {
        let mut request = raw_request(&backend, operation, operation != "plan");
        request["request"]["plan"] = json!(plan);
        request["request"]
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let error = run_local(&serde_json::to_string(&request)?)
            .unwrap_err()
            .to_string();
        assert!(error.contains("只允许完整目标计划操作"), "{error}");
    }

    let mut request = raw_request(&backend, "plan", false);
    request["request"]["plan"] = json!(plan);
    for field in [
        "backup_receipt",
        "comparison_sources",
        "arm_input",
        "fresh_target_verify",
        "product_plan",
    ] {
        request["request"][field] = json!(missing);
    }
    let error = run_local(&serde_json::to_string(&request)?)
        .unwrap_err()
        .to_string();
    assert!(error.contains("backup_receipt"), "{error}");
    assert!(!work.exists());
    assert!(!missing.exists());
    Ok(())
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
        assert!(inputs::domain::dataset_timeout(&json!({"timeout_seconds":timeout})).is_err());
    }
    for seconds in [1, 21_600, 604_800] {
        assert_eq!(
            inputs::domain::dataset_timeout(&json!({"timeout_seconds":seconds}))
                .unwrap()
                .as_secs(),
            seconds
        );
    }
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
    let original = fixture.root.join("manifest.json");
    let original_binding = native::resources::common::binding(&original).unwrap();
    for missing in [false, true] {
        let copied = fixture.copy(if missing {
            "bad-missing"
        } else {
            "bad-corrupt"
        });
        assert_eq!(copied["source_manifest"], original_binding);
        let root = PathBuf::from(copied["backup_root"].as_str().unwrap());
        let manifest = native::json::read(&root.join("manifest.json")).unwrap();
        files::damage(
            &json!({"work":fixture.plan["work_dir"],"root":root,"manifest":manifest,
            "relative":"databases/control.sql","missing":missing}),
        )
        .unwrap();
        assert!(inputs::inventory::verify_artifacts(&root, &manifest).is_err());
        inputs::inventory::verify_artifacts(&fixture.root, &fixture.manifest).unwrap();
        assert_eq!(
            native::resources::common::binding(&original).unwrap(),
            original_binding
        );
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
fn copy_refuses_a_caller_manifest_other_than_the_source_file() {
    let fixture = Fixture::new();
    let mut mismatched = fixture.manifest.clone();
    mismatched["id"] = json!("unrelated-original");
    let output = fixture.root.parent().unwrap().join("different-copy");
    assert!(
        files::copy(&json!({
            "work":fixture.plan["work_dir"],"root":fixture.root,"manifest":mismatched,
            "copy_id":"different-copy",
        }))
        .is_err()
    );
    assert!(!output.exists());
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
    assert!(
        model::bound_artifact(
            &fixture.root,
            &fixture.manifest,
            &json!("objects/uploads/one.bin"),
        )
        .is_err()
    );
}

#[test]
fn retention_uses_capture_time() {
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
    let hash = proof::product_hash(&product).unwrap();
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
        ("id", json!("_restore")),
        ("backup_id", json!("backup.one")),
        ("frontend_sha", json!("e".repeat(40))),
        ("fault_at", json!("2026-01-01T00:00:00Z")),
        ("api_ready_url", json!("http://127.0.0.1:3100/readyz")),
        ("worker_ready_url", json!("http://127.0.0.1:19201/readyz")),
    ] {
        let mut invalid = args.clone();
        invalid["record"]["plan"][field] = value;
        invalid["record"]["plan_hash"] =
            json!(proof::product_hash(&invalid["record"]["plan"]).unwrap());
        assert!(proof::record(&invalid).is_err());
    }
    let mut pair = args;
    pair["plan"]["target"]["databases"]
        .as_array_mut()
        .unwrap()
        .push(json!({"key":"tenant","server_uuid":"uuid-2","database":"target_tenant"}));
    pair["product_plan"]["databases"].as_array_mut().unwrap().push(
        json!({"source_key":"tenant","target_key":"tenant","server_uuid":"uuid-2","database":"target_tenant"}),
    );
    pair["record"]["plan"] = pair["product_plan"].clone();
    pair["record"]["plan_hash"] = json!(proof::product_hash(&pair["record"]["plan"]).unwrap());
    proof::record(&pair).unwrap();
    pair["record"]["plan"]["databases"]
        .as_array_mut()
        .unwrap()
        .reverse();
    pair["record"]["plan_hash"] = json!(proof::product_hash(&pair["record"]["plan"]).unwrap());
    assert!(
        proof::record(&pair)
            .unwrap_err()
            .to_string()
            .contains("不完全一致")
    );
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
        "runtime":{"verified_at":"2025-01-01T00:00:01Z"},
        "quiescence":{"observed_stopped_at":"2025-01-01T00:00:02Z"}});
    let mut args = args;
    for field in ["quiesced_at", "captured_at"] {
        args["inventory"][field] = json!("2025-01-01T00:00:03Z");
        args["exported"]["inventory"][field] = args["inventory"][field].clone();
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
    // 两份库存保持一致，使每个反例都命中同代来源的实际时间顺序检查。
    for (pointer, timestamp) in [
        ("/runtime/verified_at", "2025-01-01T00:00:03Z"),
        ("/quiescence/observed_stopped_at", "2025-01-01T00:00:04Z"),
        ("/inventory/quiesced_at", "2025-01-01T00:00:04Z"),
    ] {
        let mut invalid = args.clone();
        *invalid.pointer_mut(pointer).unwrap() = json!(timestamp);
        if let Some(field) = pointer.strip_prefix("/inventory/") {
            invalid["exported"]["inventory"][field] = json!(timestamp);
        }
        let error = proof::backup_source(&invalid).unwrap_err();
        assert!(
            error.to_string().contains("时间顺序不成立"),
            "{pointer}: {error}"
        );
    }
    let mut future = args.clone();
    for field in ["quiesced_at", "captured_at"] {
        future["inventory"][field] = json!("2999-01-01T00:00:00Z");
        future["exported"]["inventory"][field] = json!("2999-01-01T00:00:00Z");
    }
    let error = proof::backup_source(&future).unwrap_err();
    assert!(error.to_string().contains("时间顺序不成立"), "{error}");
}
