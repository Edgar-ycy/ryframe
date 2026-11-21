use std::{cell::Cell, collections::BTreeMap, fs, path::PathBuf};

use crate::recovery::inputs::domain::{BUCKETS, TARGET_DATABASES};
use native::{
    bridge::scope::Scope,
    target_verify::{
        database::{Captured, model::expected_owners},
        storage::StorageInputs,
        tree,
    },
};

use super::super::{Lock, capture_around_write, finish, model, receipts};
use super::*;

struct Fixture {
    _directory: tempfile::TempDir,
    backend: PathBuf,
    work: PathBuf,
    args: Value,
}

impl Fixture {
    fn new(side: &str) -> Self {
        let directory = super::super::super::tests::temporary();
        let backend = crate::recovery::fixture_control::portable_path(
            &directory.path().canonicalize().unwrap(),
        );
        let local = backend.join(".local-tests");
        fs::create_dir(&local).unwrap();
        let work = local.join("restore");
        let plan = json!({"id":"restore-case","work_dir":work,"target_side":side});
        super::super::super::files::work(&json!({"plan":plan}), true).unwrap();
        let mut bindings = json!({"target_side":side});
        for key in [
            "target_plan",
            "backup_receipt",
            "record",
            "runtime_registration",
        ] {
            let path = local.join(format!("{key}.json"));
            native::json::write_new(&path, &json!({"fixture":key})).unwrap();
            bindings[key] = common::binding(&path).unwrap();
        }
        let args = json!({"plan":plan,"command":"restore","copy_id":null,
            "root":null,"receipt":work.join(format!("restore-{side}.json")),"bindings":bindings});
        Self {
            _directory: directory,
            backend,
            work,
            args,
        }
    }

    fn image(&self, phase: Phase, evidence: &mut Map<String, Value>, matches: bool) -> Result<()> {
        let side = self.args["plan"]["target_side"].as_str().unwrap();
        let directory = self.work.join(format!("restore-{side}-{}", phase.name()));
        record(&self.backend, &directory, phase, evidence, || {
            let value = json!({
                "phase": phase.name(),
                "matches_expected": matches,
                "image": {
                    "databases":{"target_control":{"rows": if phase == Phase::Before { 0 } else { 1 }}},
                    "objects":{"uploads":{"keys":["target/.ryframe-owner"]}},
                    "redis":{"owner":"target"},
                },
            });
            let binding = publish(&self.backend, &directory.join("image.json"), &value)?;
            Ok((matches, binding))
        })
    }
}

#[test]
fn unknown_write_records_observed_failure_after_and_refuses_replay() {
    let fixture = Fixture::new("base");
    let lock = Lock::claim(&fixture.work).unwrap();
    let started = receipts::start(&fixture.args).unwrap();
    let mut evidence = Map::new();
    let writes = Cell::new(0);
    let result = capture_around_write(
        |phase, evidence| {
            lock.check()?;
            receipts::check_started(&started)?;
            match phase {
                Phase::Before => assert_eq!(writes.get(), 0),
                Phase::FailureAfter => assert_eq!(writes.get(), 1),
                Phase::After => panic!("未知写入后不能采集成功后像"),
            }
            fixture.image(phase, evidence, phase == Phase::Before)
        },
        || {
            lock.check()?;
            receipts::check_started(&started)?;
            assert!(
                fixture
                    .work
                    .join("restore-base-before/image.json")
                    .is_file()
            );
            assert!(Lock::claim(&fixture.work).is_err());
            writes.set(writes.get() + 1);
            Err("数据库写入结果未知".into())
        },
        &mut evidence,
    );
    assert_eq!(writes.get(), 1);
    assert_eq!(
        finish(&started, result.map(|()| Value::Null), evidence)
            .unwrap_err()
            .to_string(),
        "数据库写入结果未知"
    );
    let path = fixture.work.join("restore-base.json");
    let original = fs::read(&path).unwrap();
    let saved = native::json::read(&path).unwrap();
    assert_eq!(saved["status"], "failed");
    assert!(saved.get("result").is_none());
    let images = saved["resource_images"].as_object().unwrap();
    assert_eq!(
        images.keys().map(String::as_str).collect::<Vec<_>>(),
        ["before", "failure-after"]
    );
    let (_, observed) = document::bound_file(
        &fixture.backend,
        &images["failure-after"],
        "参考失败后实际像",
    )
    .unwrap();
    assert_eq!(observed.value["matches_expected"], false);
    assert_eq!(
        observed.value["image"]["databases"]["target_control"]["rows"],
        1
    );
    assert!(!observed.path.with_file_name("failure.json").exists());
    drop(lock);
    assert!(receipts::start(&fixture.args).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn strict_success_mismatch_and_unreadable_failure_image_keep_failure_diagnostics() {
    let fixture = Fixture::new("base");
    let mut evidence = Map::new();
    assert!(fixture.image(Phase::After, &mut evidence, false).is_err());
    assert!(evidence.contains_key("after"));
    assert!(evidence.contains_key("after-failure"));

    let directory = fixture.work.join("restore-base-failure-after");
    let error = record(
        &fixture.backend,
        &directory,
        Phase::FailureAfter,
        &mut evidence,
        || Err("五桶归属不符".into()),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "五桶归属不符");
    assert!(!evidence.contains_key("failure-after"));
    assert_eq!(
        evidence["failure-after-failure"],
        common::binding(&directory.join("failure.json")).unwrap()
    );
}

#[test]
fn successful_copy_receipts_keep_the_selected_side_and_exact_input_bindings() {
    for side in ["base", "candidate"] {
        let fixture = Fixture::new(side);
        let lock = Lock::claim(&fixture.work).unwrap();
        let started = receipts::start(&fixture.args).unwrap();
        let mut evidence = Map::new();
        let writes = Cell::new(0);
        capture_around_write(
            |phase, evidence| {
                lock.check()?;
                receipts::check_started(&started)?;
                assert!(phase != Phase::FailureAfter);
                assert_eq!(writes.get(), usize::from(phase == Phase::After));
                fixture.image(phase, evidence, true)
            },
            || {
                lock.check()?;
                writes.set(writes.get() + 1);
                Ok(())
            },
            &mut evidence,
        )
        .unwrap();
        let bindings = &fixture.args["bindings"];
        let runtime = json!({
            "registration":bindings["runtime_registration"],
            "target_plan":bindings["target_plan"],
        });
        let result = model::restore_result(&json!({
            "record":{"plan":{"id":"restore-case"}}, "bindings":bindings,
            "runtime":runtime,"images":evidence,
        }))
        .unwrap();
        assert_eq!(result["status"], "external_copy_completed");
        assert_eq!(result["runtime"], runtime);
        let reply = finish(&started, Ok(result.clone()), evidence).unwrap();
        assert_eq!(reply, result);
        let path = fixture.work.join(format!("restore-{side}.json"));
        let terminal = native::json::read(&path).unwrap();
        assert_eq!(terminal["status"], "completed");
        assert_eq!(terminal["result"], result);
        assert!(terminal.get("resource_images").is_none());
        for (key, value) in bindings.as_object().unwrap() {
            assert_eq!(&terminal[key], value);
            assert_eq!(&terminal["result"][key], value);
        }
        for phase in ["before", "after"] {
            let image = fixture
                .work
                .join(format!("restore-{side}-{phase}/image.json"));
            assert_eq!(
                terminal["result"]["resource_images"][phase],
                common::binding(&image).unwrap()
            );
        }
        assert_eq!(writes.get(), 1);
        assert!(!fixture.work.join("restore.json").exists());
        let bytes = fs::read(&path).unwrap();
        drop(lock);
        assert!(receipts::start(&fixture.args).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn mismatched_before_image_prevents_writes_and_finishes_with_original_evidence() {
    let fixture = Fixture::new("base");
    let lock = Lock::claim(&fixture.work).unwrap();
    let started = receipts::start(&fixture.args).unwrap();
    let mut evidence = Map::new();
    let result = capture_around_write(
        |phase, evidence| {
            lock.check()?;
            receipts::check_started(&started)?;
            assert!(phase == Phase::Before);
            fixture.image(phase, evidence, false)
        },
        || panic!("前像不匹配时不得发送写请求"),
        &mut evidence,
    );
    assert!(finish(&started, result.map(|()| Value::Null), evidence).is_err());
    let receipt = native::json::read(&fixture.work.join("restore-base.json")).unwrap();
    assert_eq!(receipt["status"], "failed");
    assert!(receipt.get("result").is_none());
    let images = receipt["resource_images"].as_object().unwrap();
    assert_eq!(
        images.keys().map(String::as_str).collect::<Vec<_>>(),
        ["before", "before-failure"]
    );
    let (_, before) =
        document::bound_file(&fixture.backend, &images["before"], "实际前像").unwrap();
    assert_eq!(before.value["matches_expected"], false);
    assert!(!fixture.work.join("restore-base-after").exists());
    assert!(!fixture.work.join("restore-base-failure-after").exists());
}

const OBJECT_PROOFS: [(&str, &str); 2] = [
    ("object-uploads-0/observation.json", "object_present"),
    ("object-heads/verified.json", "target_object_heads_verified"),
];

struct JointFixture {
    restore: Fixture,
    target: document::Document,
    initialized_directory: PathBuf,
    initialized: Value,
    request: Value,
    reference: Value,
    configuration: Value,
}

impl JointFixture {
    fn new() -> Self {
        let restore = Fixture::new("base");
        let (_, target) = document::bound_file(
            &restore.backend,
            &restore.args["bindings"]["target_plan"],
            "原目标计划",
        )
        .unwrap();
        let configuration = json!({"app_env":"test","sha256":"a".repeat(64)});
        let databases = TARGET_DATABASES.map(|(key, kind, mode)| {
            json!({"key":key,"kind":kind,"mode":mode,"database":database_name(key),
                "server_uuid":"server-fixture"})
        });
        let storage = json!({"endpoint":"http://127.0.0.1:19001","region":"us-east-1"});
        let request = json!({"side":"base","target":{"scope_id":"target","databases":databases,"s3":storage}});
        let reference = json!({"target_side":"base","target":{
            "scope_id":"target","databases":databases,"runtime_dir":"fixture-runtime",
            "api_url":"http://127.0.0.1:19002","worker_ready_url":"http://127.0.0.1:19003/readyz",
            "frontend_url":"http://127.0.0.1:19004","s3":{
                "endpoint":storage["endpoint"],"region":storage["region"],"configuration":configuration
            }
        }});
        let initialized = json!({"status":"fresh_target_initialized","scope_id":"target",
            "controlled_generation_never_started":true,"generation":{"selected":reference["target"]},
            "inventory":{"observations":resource_image(Phase::Before)["databases"]}});
        let initialized_directory = restore.work.join("initialized");
        fs::create_dir(&initialized_directory).unwrap();
        native::json::write_new(
            &initialized_directory.join("initialized.json"),
            &initialized,
        )
        .unwrap();
        native::json::write_new(&initialized_directory.join("request.json"), &request).unwrap();
        Self {
            restore,
            target,
            initialized_directory,
            initialized,
            request,
            reference,
            configuration,
        }
    }

    fn directory(&self, phase: Phase) -> PathBuf {
        self.restore
            .work
            .join(format!("restore-base-{}", phase.name()))
    }

    fn capture(
        &self,
        scope: &Scope,
        phase: Phase,
        expected: &Value,
        actual: &Value,
        evidence: &mut Map<String, Value>,
        (during_redis, checkpoint): (impl FnOnce(&Path) -> Result<()>, &dyn Fn() -> Result<()>),
    ) -> Result<()> {
        let backend = &self.restore.backend;
        let directory = self.directory(phase);
        record(backend, &directory, phase, evidence, || {
            let storage = StorageInputs {
                backend,
                request: &self.request,
                environment: &BTreeMap::new(),
                observation_dir: &directory,
            };
            let inventory = directory.join("inventory");
            let captured = record_inventory(backend, &inventory, &actual["databases"])?;
            let database = database_images::Observed::bind(
                &database_images::Capture {
                    storage: &storage,
                    reference: &self.reference,
                    initialized: &self.initialized,
                    configuration: &self.configuration,
                    scope,
                    initialized_directory: &self.initialized_directory,
                    phase_root: &directory,
                },
                captured,
            )?;
            let object_proofs = if phase == Phase::After {
                OBJECT_PROOFS
                    .into_iter()
                    .map(|(name, status)| {
                        let proof = directory.join(name);
                        fs::create_dir(proof.parent().unwrap())?;
                        native::json::write_new(
                            &proof,
                            &json!({"status":status,"bucket":"uploads",
                                "key":"target/tenant/file.txt","sha256":"c".repeat(64)}),
                        )?;
                        common::binding(&proof)
                    })
                    .collect::<Result<Vec<_>>>()?
            } else {
                Vec::new()
            };
            Publication {
                backend,
                directory: &directory,
                target: &self.target,
                phase,
                expected,
            }
            .publish(
                Observations {
                    database,
                    objects: actual["objects"].clone(),
                    object_proofs,
                },
                || {
                    during_redis(&inventory.join("inventory.json"))?;
                    Ok(actual["redis"].clone())
                },
                checkpoint,
            )
        })
    }
}

fn database_name(key: &str) -> String {
    format!("target_{}", key.replace('-', "_"))
}

fn resource_image(phase: Phase) -> Value {
    let databases = TARGET_DATABASES
        .iter()
        .map(|(key, kind, _)| {
            let name = database_name(key);
            let tables = json!({"sys_post":{"rows":usize::from(phase == Phase::After),
                "sha256":if phase == Phase::After {"b".repeat(64)} else {"a".repeat(64)}}});
            let preserved = if *kind == "combined" {
                json!({"sys_restore_run":{"rows":0,"sha256":"d".repeat(64)}})
            } else {
                json!({})
            };
            let combined = *kind == "combined";
            let mut all_tables = vec!["sys_post"];
            if combined {
                all_tables.push("sys_restore_run");
            }
            let image = json!({
                "resource":{"kind":"database","scope_id":"target","database":name,
                    "server_uuid":"server-fixture"},
                "schema_sha256": if combined {"1".repeat(64)} else {"2".repeat(64)},
                "tables":tables,"preserved":preserved,"all_tables":all_tables,
                "ownership":expected_owners("target", combined),
            });
            (name, image)
        })
        .collect::<BTreeMap<_, _>>();
    let objects = BUCKETS
        .iter()
        .map(|bucket| {
            let mut keys = vec!["target/.ryframe-owner"];
            if phase == Phase::After && *bucket == "uploads" {
                keys.push("target/tenant/file.txt");
            }
            (
                *bucket,
                json!({"keys":keys,"owner":{"bytes":45,"sha256":"e".repeat(64)}}),
            )
        })
        .collect::<BTreeMap<_, _>>();
    json!({"databases":databases,"objects":objects,
        "redis":{"namespace":"target","owner":"target","sentinel":"original"}})
}

fn record_inventory(backend: &Path, directory: &Path, databases: &Value) -> Result<Captured> {
    fs::create_dir(directory)?;
    let binding = json!({"scope_id":"target","server_uuid":"server-fixture"});
    for file in ["binding-before.json", "binding-after.json"] {
        native::json::write_new(&directory.join(file), &binding)?;
    }
    let keys = TARGET_DATABASES.map(|(key, _, _)| key);
    let mut observations = Map::new();
    let mut inventories = Map::new();
    for key in keys {
        let name = database_name(key);
        let observed = &databases[&name];
        observations.insert(key.to_owned(), observed.clone());
        for round in ["before", "after"] {
            let name = format!("{round}-target-{key}.json");
            let file = directory.join(&name);
            native::json::write_new(&file, observed)?;
            let digest = native::digest::file(&file)?;
            inventories.insert(name, json!({"bytes":digest.bytes,"sha256":digest.sha256}));
        }
    }
    let inventory = json!({"format_version":1,"status":"side_inventory_captured","side":"target",
        "scope_id":"target","keys":keys,"inventories":inventories,"observations":observations,
        "binding_sha256":plan_hash(&binding)?,"remote_writes":0,
        "producer_stopped_proven":false,"fresh_target_proven":false,"target_ready":false,
        "clone_verified":false,"restore_qualified":false});
    let path = directory.join("inventory.json");
    native::json::write_new(&path, &inventory)?;
    let evidence = tree::snapshot(backend, directory, false)?;
    Ok(Captured {
        binding,
        inventory,
        receipt: common::binding(&path)?,
        observations: serde_json::from_value(databases.clone())?,
        evidence_files: serde_json::from_value(evidence)?,
    })
}

#[test]
fn complete_joint_before_and_after_bind_four_databases_five_buckets_and_redis() {
    let fixture = JointFixture::new();
    let scope = Scope::load(&crate::workspace::root_dir(), &[], &[]).unwrap();
    let writes = Cell::new(0);
    let mut evidence = Map::new();
    capture_around_write(
        |phase, evidence| {
            assert!(phase != Phase::FailureAfter);
            let expected = resource_image(phase);
            fixture.capture(
                &scope,
                phase,
                &expected,
                &expected,
                evidence,
                (
                    |_: &Path| {
                        assert_eq!(writes.get(), usize::from(phase == Phase::After));
                        Ok(())
                    },
                    &|| Ok(()),
                ),
            )
        },
        || {
            writes.set(writes.get() + 1);
            Ok(())
        },
        &mut evidence,
    )
    .unwrap();
    assert_eq!(writes.get(), 1);
    assert_eq!(evidence.len(), 2);
    for phase in [Phase::Before, Phase::After] {
        let backend = &fixture.restore.backend;
        let (_, image) = document::bound_file(backend, &evidence[phase.name()], "联合像").unwrap();
        let expected = resource_image(phase);
        assert_eq!(image.value["image"], expected);
        assert_eq!(image.value["matches_expected"], true);
        assert_eq!(
            image.value["expected_sha256"],
            plan_hash(&expected).unwrap()
        );
        assert_eq!(
            image.value["target_plan"],
            document::binding(&fixture.target)
        );
        let (_, inventory) =
            document::bound_file(backend, &image.value["inventory"], "原库存收据").unwrap();
        assert_eq!(inventory.value["inventories"].as_object().unwrap().len(), 8);
        let proofs = image.value["object_observations"].as_array().unwrap();
        assert_eq!(
            proofs.len(),
            OBJECT_PROOFS.len() * usize::from(phase == Phase::After)
        );
        for ((name, status), proof) in OBJECT_PROOFS.into_iter().zip(proofs) {
            let (_, object) = document::bound_file(backend, proof, "对象证明").unwrap();
            assert_eq!(object.path, fixture.directory(phase).join(name));
            assert_eq!(object.value["status"], status);
            assert_eq!(object.value["key"], "target/tenant/file.txt");
        }
    }
}

#[test]
fn joint_resource_drift_is_recorded_and_blocks_the_affected_phase() {
    let scope = Scope::load(&crate::workspace::root_dir(), &[], &[]).unwrap();
    for (phase, pointer, replacement) in [
        (
            Phase::Before,
            "/databases/target_dedicated_a/tables/sys_post/rows",
            json!(2),
        ),
        (
            Phase::Before,
            "/databases/target_dedicated_b/all_tables",
            json!(["sys_post", "unexpected_table"]),
        ),
        (
            Phase::Before,
            "/databases/target_shared/schema_sha256",
            json!("3".repeat(64)),
        ),
        (
            Phase::Before,
            "/databases/target_shared_control/ownership/0/marker",
            json!("changed"),
        ),
        (
            Phase::Before,
            "/databases/target_shared_control/preserved/sys_restore_run/sha256",
            json!("f".repeat(64)),
        ),
        (
            Phase::Before,
            "/objects/avatar/owner/sha256",
            json!("f".repeat(64)),
        ),
        (
            Phase::Before,
            "/objects/exports/keys",
            json!(["target/.ryframe-owner", "target/unexpected"]),
        ),
        (Phase::Before, "/redis/sentinel", json!("changed")),
        (
            Phase::After,
            "/databases/target_shared_control/preserved/sys_restore_run/rows",
            json!(1),
        ),
    ] {
        let fixture = JointFixture::new();
        let mut actual = resource_image(phase);
        *actual.pointer_mut(pointer).unwrap() = replacement;
        let mut evidence = Map::new();
        let writes = Cell::new(0);
        let error = capture_around_write(
            |stage, evidence| {
                let fresh = resource_image(stage);
                let observed = if stage == phase { &actual } else { &fresh };
                fixture.capture(
                    &scope,
                    stage,
                    &fresh,
                    observed,
                    evidence,
                    (|_: &Path| Ok(()), &|| Ok(())),
                )
            },
            || {
                writes.set(writes.get() + 1);
                Ok(())
            },
            &mut evidence,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("完整资源像与预期不同"),
            "{pointer}: {error}"
        );
        assert_eq!(writes.get(), usize::from(phase == Phase::After));
        assert!(evidence.contains_key(&format!("{}-failure", phase.name())));
        let (_, image) = document::bound_file(
            &fixture.restore.backend,
            &evidence[phase.name()],
            "漂移联合像",
        )
        .unwrap();
        assert_eq!(image.value["image"], actual);
        assert_eq!(image.value["matches_expected"], false);
    }
}

#[test]
fn inventory_drift_after_object_observation_keeps_only_the_failure_binding() {
    let fixture = JointFixture::new();
    let scope = Scope::load(&crate::workspace::root_dir(), &[], &[]).unwrap();
    let expected = resource_image(Phase::After);
    let mut evidence = Map::new();
    let error = fixture
        .capture(
            &scope,
            Phase::After,
            &expected,
            &expected,
            &mut evidence,
            (
                |receipt: &Path| {
                    assert!(
                        fixture
                            .directory(Phase::After)
                            .join("object-heads/verified.json")
                            .is_file()
                    );
                    let original = fs::read(receipt)?;
                    let mut edited = original.clone();
                    let version = b"\"format_version\": 1";
                    let offset = edited
                        .windows(version.len())
                        .position(|bytes| bytes == version)
                        .unwrap();
                    edited[offset + version.len() - 1] = b'2';
                    let changed = fs::write(receipt, &edited);
                    #[cfg(windows)]
                    {
                        assert_eq!(changed.unwrap_err().raw_os_error(), Some(32));
                        assert_eq!(fs::read(receipt)?, original);
                        fs::write(receipt.with_file_name("late.json"), b"{}")?;
                    }
                    #[cfg(not(windows))]
                    {
                        changed?;
                        assert_eq!(fs::read(receipt)?, edited);
                    }
                    Ok(())
                },
                &|| Ok(()),
            ),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("原库存文件树或字节变化"),
        "{error}"
    );
    assert_eq!(evidence.len(), 1);
    assert!(!fixture.directory(Phase::After).join("image.json").exists());
    let (_, failure) = document::bound_file(
        &fixture.restore.backend,
        &evidence["after-failure"],
        "失败收据",
    )
    .unwrap();
    assert_eq!(failure.value["error_type"], "ReferenceResourceImageFailure");
}

#[test]
fn object_failure_markers_prevent_acceptance_of_the_parent_image() {
    let scope = Scope::load(&crate::workspace::root_dir(), &[], &[]).unwrap();
    let phase = Phase::After;
    let expected = resource_image(phase);
    for (failed_proof, after_publication) in [(0, false), (1, false), (1, true)] {
        let fixture = JointFixture::new();
        let directory = fixture.directory(phase);
        let image = directory.join("image.json");
        let proofs = OBJECT_PROOFS.map(|(name, _)| directory.join(name));
        let child_failure = proofs[failed_proof].with_file_name("failure.json");
        let marker = json!({"status":"object_observation_failed"});
        let original = Cell::new(None);
        let during_redis = |_: &Path| {
            original.set(Some([fs::read(&proofs[0])?, fs::read(&proofs[1])?]));
            if !after_publication {
                native::json::write_new(&child_failure, &marker)?;
            }
            Ok(())
        };
        let checkpoint = || {
            if after_publication && image.is_file() {
                native::json::write_new(&child_failure, &marker)?;
            }
            Ok(())
        };
        let mut evidence = Map::new();
        fixture
            .capture(
                &scope,
                phase,
                &expected,
                &expected,
                &mut evidence,
                (during_redis, &checkpoint),
            )
            .unwrap_err();
        assert_eq!(image.is_file(), after_publication);
        assert_eq!(evidence.len(), 1);
        assert_eq!(
            evidence["after-failure"],
            common::binding(&directory.join("failure.json")).unwrap()
        );
        let (_, failure) = document::bound_file(
            &fixture.restore.backend,
            &evidence["after-failure"],
            "失败收据",
        )
        .unwrap();
        assert_eq!(failure.value["error_type"], "ReferenceResourceImageFailure");
        assert_eq!(native::json::read(&child_failure).unwrap(), marker);
        for (proof, bytes) in proofs.iter().zip(original.into_inner().unwrap()) {
            assert_eq!(fs::read(proof).unwrap(), bytes);
        }
    }
}
