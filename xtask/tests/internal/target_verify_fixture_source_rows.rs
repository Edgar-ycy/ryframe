use std::{cell::RefCell, collections::BTreeMap, fs, path::Path};

use super::super::{database_name, snapshot};
use super::{FakeTransport, Fixture, KEYS, SCOPE, bound, descriptor, raw, write};
use crate::{
    cli::CloneSide,
    recovery::clone::native::{
        bridge::scope::Scope,
        copy_transfer::observe_database::{Side, Window},
        json as document, maintenance,
        s3::app_configuration,
        target_verify::{
            database::{self, SourceBinding, model, protocol::TestInputs},
            storage::StorageInputs,
        },
    },
};
use serde_json::{Value, json};

const TARGET_SCOPE: &str = "copy-row-target";

#[test]
fn source_inventory_rejects_changed_saved_rows_without_rebinding_the_export() {
    let mut fixture = Fixture::new();
    let work = fixture.local.join("copy-source-rows");
    fs::create_dir(&work).unwrap();
    let configuration = bind_source(&mut fixture);
    let scope = Scope::load(&fixture.backend, &[], &[]).unwrap();
    let (target_request, initialized) = saved_target(&fixture, &work);
    let (export, projection) = source_images(&fixture);
    let export_path = fixture.local.join("copy-source-export-inventory.json");
    let session_path = work.join("session.json");
    write(&export_path, &export);
    write(&session_path, &json!({"source_observation":projection}));
    let original_export = fs::read(&export_path).unwrap();
    let first_export = document::read(&export_path).unwrap();
    let session = document::read(&session_path).unwrap();
    let window = Window {
        source: side(
            &fixture,
            &fixture.request,
            CloneSide::Source,
            &scope,
            &configuration,
        ),
        target: side(
            &fixture,
            &target_request,
            CloneSide::Target,
            &scope,
            &configuration,
        ),
        export_inventory: &first_export,
        initialized: &initialized,
        frozen_source_observation: Some(&session["source_observation"]),
    };
    let first_dir = work.join("original-source-inventory");
    let first_transport = FakeTransport::new(&fixture, false, false);
    let baseline = window
        .baseline(&first_dir, &first_transport, &|| Ok(()))
        .unwrap();
    assert_eq!(baseline.source_observation(), projection);
    assert_eq!(baseline.initialized().len(), KEYS.len());
    assert_eq!(
        baseline.initialized_receipt(),
        &initialized["inventory"]["receipt"]
    );
    let first = assert_source_capture(&first_dir, &first_transport);
    let first_snapshot = snapshot(&first_dir);

    // 只改原会话保存的行像，两轮实时采集继续返回相同的完整来源。
    let mut changed = session.clone();
    let saved =
        &mut changed["source_observation"]["shared-control"]["tables"]["sys_tenant"]["sha256"];
    let different = json!("f".repeat(64));
    assert!(saved.as_str().is_some());
    assert_ne!(*saved, different);
    *saved = different;
    document::write_replace(&session_path, &changed).unwrap();
    let changed_bytes = fs::read(&session_path).unwrap();
    let reread = document::read(&session_path).unwrap();
    let same_export = document::read(&export_path).unwrap();
    let continued = Window {
        export_inventory: &same_export,
        frozen_source_observation: Some(&reread["source_observation"]),
        ..window
    };
    let second_dir = work.join("continued-source-inventory");
    let second_transport = FakeTransport::new(&fixture, false, false);
    let Err(error) = continued.baseline(&second_dir, &second_transport, &|| Ok(())) else {
        panic!("冻结来源行像发生变化时必须拒绝重新绑定");
    };
    assert_eq!(error.to_string(), "源完整业务及保留表与冻结复制会话不同");
    assert_eq!(assert_source_capture(&second_dir, &second_transport), first);
    assert_eq!(snapshot(&first_dir), first_snapshot);
    assert_eq!(fs::read(&export_path).unwrap(), original_export);
    assert_eq!(fs::read(&session_path).unwrap(), changed_bytes);
}

#[test]
fn late_maintenance_receipt_drift_revokes_only_its_inventory_publication() {
    let mut fixture = Fixture::new();
    let configuration = bind_source(&mut fixture);
    let review = document::read(&fixture.local.join("review.json")).unwrap();
    for field in ["runtime_dir", "api_url"] {
        fixture.request["target"][field] = review["scopes"]["seed"][field].clone();
    }
    let output = fixture.local.join("late-maintenance-inventory");
    let inventory = output.join("inventory.json");
    let args = json!({
        "evidence_root":fixture.backend, "backend":fixture.backend, "side":"source",
        "plan":{"source":fixture.request["target"],"tools":fixture.request["tools"]},
        "maintenance":fixture.request["maintenance_build"], "configuration":configuration,
        "app_env":"test", "output":output,
    });
    let input = TestInputs::load(&args, &fixture.backend).unwrap();
    let scope = Scope::load(&fixture.backend, &[], &[]).unwrap();
    let mut source = side(
        &fixture,
        &fixture.request,
        CloneSide::Source,
        &scope,
        &configuration,
    );
    source.binding.plan = Some(&args["plan"]);

    let maintenance = fixture.local.join("maintenance/build.json");
    let original_maintenance = fs::read(&maintenance).unwrap();
    let mut changed_maintenance = original_maintenance.clone();
    let last = changed_maintenance.last_mut().unwrap();
    assert_eq!(*last, b'\n');
    *last = b' ';
    let original_receipt: Value = serde_json::from_slice(&original_maintenance).unwrap();
    let published = RefCell::new(None);
    let transport = FakeTransport::new(&fixture, false, false);
    let result = database::capture_source_at(
        &source.storage,
        &output,
        &transport,
        &|| {
            if inventory.try_exists()? {
                assert_source_capture(&output, &transport);
                assert!(published.replace(Some(snapshot(&output))).is_none());
                fs::write(&maintenance, &changed_maintenance)?;
                assert_eq!(document::read(&maintenance)?, original_receipt);
            }
            input.verify(&args, &fixture.backend)?;
            scope.assert_unchanged()
        },
        &source.binding,
    );

    let Err(error) = result else {
        panic!("发布后的维护收据变化必须使采集失败");
    };
    assert_eq!(error.to_string(), "库存维护构建与登记摘要不一致");
    assert_eq!(transport.inventories.get(), 8);
    assert_eq!(transport.identities.get(), 16);
    assert_eq!(transport.ownerships.get(), 24);
    assert_eq!(fs::read(&maintenance).unwrap(), changed_maintenance);

    let mut before = published.into_inner().expect("必须观察到真实库存发布");
    assert!(before.remove(Path::new("inventory.json")).is_some());
    let mut after = snapshot(&output);
    assert!(after.remove(Path::new("failure.json")).is_some());
    assert_eq!(after, before);
    assert_eq!(
        document::read(&output.join("failure.json")).unwrap(),
        json!({"format_version":1, "status":"side_inventory_failed", "stage":"publish",
            "error_type":"RustError", "remote_writes":0, "target_ready":false, "clone_verified":false})
    );
}

fn bind_source(fixture: &mut Fixture) -> Value {
    let build = fixture.local.join("maintenance/build.json");
    let mut receipt = document::read(&build).unwrap();
    receipt["toolchain"] =
        maintenance::toolchain(&fixture.backend, &maintenance::DefaultRunner).unwrap();
    document::write_replace(&build, &receipt).unwrap();
    fixture.request["maintenance_build"] = bound(&build);
    let dir = Path::new(fixture.request["config_dir"].as_str().unwrap());
    let configuration =
        app_configuration::configuration_value(&fixture.backend, dir, "test").unwrap();
    fixture.request["target"]["s3"] = json!({"configuration":configuration});
    configuration
}

fn side<'a>(
    fixture: &'a Fixture,
    request: &'a Value,
    side: CloneSide,
    scope: &'a Scope,
    configuration: &'a Value,
) -> Side<'a> {
    Side {
        storage: StorageInputs {
            backend: &fixture.backend,
            request,
            environment: &fixture.environment,
            observation_dir: &fixture.local,
        },
        binding: SourceBinding {
            side,
            plan: None,
            selected: &request["target"],
            tools: &request["tools"],
            maintenance: &request["maintenance_build"],
            scope,
            configuration: Some(configuration),
        },
    }
}

fn saved_target(fixture: &Fixture, work: &Path) -> (Value, Value) {
    let original = document::read(&fixture.target.join("initialized.json")).unwrap();
    let mut target = fixture.request["target"].clone();
    target["scope_id"] = json!(TARGET_SCOPE);
    let mut images = BTreeMap::new();
    // baseline 只验证目标的预存初像，采集命令仅发往来源替身。
    for database in target["databases"].as_array_mut().unwrap() {
        let mut image =
            original["inventory"]["observations"][database["database"].as_str().unwrap()].clone();
        let name = database_name("copy_row_target", database["key"].as_str().unwrap());
        database["database"] = json!(name);
        image["resource"]["database"] = json!(name);
        image["resource"]["scope_id"] = json!(TARGET_SCOPE);
        image["ownership"] = json!(model::expected_owners(
            TARGET_SCOPE,
            database["kind"] == "combined"
        ));
        images.insert(name, image);
    }
    let inventory = work.join("target-initial-inventory.json");
    write(
        &inventory,
        &json!({"scope_id":TARGET_SCOPE,"observations":images}),
    );
    let initialized = json!({
        "status":original["status"], "scope_id":TARGET_SCOPE,
        "controlled_generation_never_started":original["controlled_generation_never_started"],
        "inventory":{"receipt":bound(&inventory),"observations":images},
    });
    let request = json!({
        "target":target, "tools":fixture.request["tools"],
        "maintenance_build":fixture.request["maintenance_build"],
    });
    (request, initialized)
}

fn source_images(fixture: &Fixture) -> (Value, Value) {
    let mut databases = Vec::new();
    let mut projection = BTreeMap::new();
    for declaration in fixture.request["target"]["databases"].as_array().unwrap() {
        let key = declaration["key"].as_str().unwrap();
        let raw = raw(fixture, key);
        let owners = model::expected_owners(SCOPE, declaration["kind"] == "combined");
        let image =
            model::observation(&fixture.backend, &raw, declaration, SCOPE, &owners).unwrap();
        projection.insert(
            key,
            json!({"tables":image["tables"],"preserved":image["preserved"]}),
        );
        databases.push(raw["target"]["database"].clone());
    }
    let control = raw(fixture, "shared-control");
    let export = json!({
        "scope_id":SCOPE, "control_schema_fingerprint":control["control_schema_fingerprint"],
        "tenant_schema_fingerprint":control["tenant_schema_fingerprint"], "databases":databases,
    });
    (export, json!(projection))
}

fn assert_source_capture(directory: &Path, transport: &FakeTransport<'_>) -> Value {
    assert_eq!(transport.inventories.get(), 8);
    assert_eq!(transport.identities.get(), 16);
    assert_eq!(transport.ownerships.get(), 24);
    let receipt = document::read(&directory.join("inventory.json")).unwrap();
    assert_eq!(receipt["side"], "source");
    assert_eq!(receipt["scope_id"], SCOPE);
    assert_eq!(receipt["status"], "side_inventory_captured");
    assert_eq!(receipt["remote_writes"], 0);
    assert_eq!(receipt["inventories"].as_object().unwrap().len(), 8);
    for key in KEYS {
        let files = ["before", "after"].map(|phase| {
            let name = format!("{phase}-target-{key}.json");
            let path = directory.join(&name);
            assert_eq!(receipt["inventories"][name.as_str()], descriptor(&path));
            fs::read(path).unwrap()
        });
        assert_eq!(files[0], files[1]);
    }
    receipt
}
