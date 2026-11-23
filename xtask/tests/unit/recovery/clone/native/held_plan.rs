use std::fs;

use super::{super as native, *};
use crate::clone_native_fixture::Fixture;

fn save(fixture: &Fixture) -> (PathBuf, Value) {
    document::write_new(&fixture.input, &fixture.value).unwrap();
    let wrapper = native::inspect_input(&fixture.root, &fixture.input)
        .unwrap()
        .wrapper;
    let plan = fixture.input.with_file_name("plan.json");
    document::write_new(&plan, &wrapper).unwrap();
    (plan, wrapper)
}

#[test]
fn held_plan_issues_same_result_and_protects_the_real_object_path() {
    let fixture = Fixture::new();
    let (plan, wrapper) = save(&fixture);
    let object = fixture.export.join("file.bin");
    let writer = fs::OpenOptions::new().write(true).open(&object).unwrap();
    assert!(HeldPlanObjects::open(&fixture.root, &plan, &wrapper).is_err());
    drop(writer);

    let window = HeldPlanObjects::open(&fixture.root, &plan, &wrapper).unwrap();
    assert_eq!(window.issue().unwrap(), wrapper);
    assert!(fs::write(&object, b"edit").is_err());
    assert!(fs::rename(&object, object.with_extension("old")).is_err());
    assert!(fs::rename(&fixture.export, fixture.export.with_extension("old")).is_err());
    window.check_end().unwrap();
    drop(window);

    fs::write(&object, b"edit").unwrap();
    assert!(native::issue(&fixture.root, &plan, &wrapper).is_err());
    assert!(HeldPlanObjects::open(&fixture.root, &plan, &wrapper).is_err());
}

#[test]
fn held_sql_files_deny_windows_writes_and_keep_the_full_pending_plan() {
    let fixture = fixture_with_pending_schedule();
    let (plan, wrapper) = save(&fixture);
    assert_eq!(
        wrapper["plan"]["pending_target_actions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let files = fixture.value["databases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dump| {
            fixture
                .export
                .join(dump["artifact"]["file"].as_str().unwrap())
        })
        .collect::<Vec<_>>();
    let original = files
        .iter()
        .map(|file| fs::read(file).unwrap())
        .collect::<Vec<_>>();
    let writer = fs::OpenOptions::new().write(true).open(&files[0]).unwrap();
    assert!(HeldPlanObjects::open(&fixture.root, &plan, &wrapper).is_err());
    drop(writer);

    let window = HeldPlanObjects::open(&fixture.root, &plan, &wrapper).unwrap();
    assert_eq!(
        native::issue(&fixture.root, &plan, &wrapper).unwrap(),
        wrapper
    );
    for (file, bytes) in files.iter().zip(&original) {
        assert!(fs::write(file, b"edit").is_err());
        assert!(fs::rename(file, file.with_extension("old")).is_err());
        assert_eq!(&fs::read(file).unwrap(), bytes);
    }
    assert_eq!(window.issue().unwrap(), wrapper);
    window.check_end().unwrap();
    drop(window);

    fs::write(&files[0], b"edit").unwrap();
    assert!(native::issue(&fixture.root, &plan, &wrapper).is_err());
    assert!(HeldPlanObjects::open(&fixture.root, &plan, &wrapper).is_err());
}

fn fixture_with_pending_schedule() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.add(
        "shared-control",
        "sys_job_schedule",
        &[
            ("id", json!(3)),
            ("tenant_id", json!("system")),
            ("name", json!("数据保留清理")),
            ("handler_key", json!("system.data_retention_cleanup")),
            ("cron_expression", json!("0 30 3 * * * *")),
            ("timezone", json!("UTC")),
            ("enabled", json!(1)),
            ("next_run_at", json!("2026-09-04 03:30:00")),
            ("version", json!(1)),
            ("del_flag", json!("0")),
            ("created_at", json!("2026-09-04 00:00:00")),
            ("updated_at", json!("2026-09-04 00:00:00")),
        ],
    );
    fixture.refresh();
    let catalog = native::catalog::load(&fixture.root).unwrap();
    let columns = native::sql::all_columns(&catalog.control, &catalog.tenant, true);
    let dump = fixture.value["databases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|dump| dump["key"] == "shared-control")
        .unwrap();
    let binding = input::bound_file(&fixture.export, &dump["artifact"]).unwrap();
    let mut digest = None;
    native::sql::for_each_bound_row(
        &fixture.export,
        &binding,
        &columns,
        Some("sys_job_schedule"),
        |_, row| {
            digest = Some(native::schedule::row_sha256(row)?);
            Ok(())
        },
    )
    .unwrap();
    fixture.value["target_schedule_actions"] = json!([{
        "action":"disable_schedule_via_api","tenant_id":"system","schedule_id":"3",
        "handler_key":"system.data_retention_cleanup","expected_version":1,
        "source_row_sha256":digest.unwrap(),
    }]);
    fixture
}

#[test]
fn held_plan_rejects_different_descriptors_roots_and_object_declarations() {
    let fixture = Fixture::new();
    let (plan, wrapper) = save(&fixture);
    let window = HeldPlanObjects::open(&fixture.root, &plan, &wrapper).unwrap();
    let original = &fixture.value["objects"];
    let artifact = &original[4]["entries"][0]["artifact"];
    assert!(window.bound_object(&fixture.export, artifact).is_ok());
    assert!(window.bound_object(&fixture.root, artifact).is_err());

    for (field, value) in [
        ("file", json!("different.bin")),
        ("bytes", json!(5)),
        ("sha256", json!("0".repeat(64))),
    ] {
        let mut changed = artifact.clone();
        changed[field] = value;
        assert!(window.bound_object(&fixture.export, &changed).is_err());
    }

    let mut changed = original.clone();
    changed[4]["entries"][0]["metadata"] = json!({"different":true});
    assert!(
        window
            .require_declaration(&fixture.export, &changed)
            .is_err()
    );
    changed[4]["entries"] = json!([]);
    assert!(
        window
            .require_declaration(&fixture.export, &changed)
            .is_err()
    );

    let mut repeated = original.clone();
    let entries = repeated[4]["entries"].as_array_mut().unwrap();
    entries.push(original[4]["entries"][0].clone());
    assert_eq!(declared_artifacts(&repeated).unwrap().len(), 1);
    repeated[4]["entries"][1]["artifact"]["sha256"] = json!("0".repeat(64));
    assert!(declared_artifacts(&repeated).is_err());
}

#[test]
fn held_database_plan_rejects_declared_sql_and_actual_catalogue_drift() {
    let fixture = Fixture::new();
    let (plan, wrapper) = save(&fixture);
    let window = HeldPlanObjects::open(&fixture.root, &plan, &wrapper).unwrap();

    let mut changed = fixture.value.clone();
    changed["databases"][0]["artifact"]["sha256"] = json!("0".repeat(64));
    let error = native::plan::create(&changed, &fixture.root, Some(&window))
        .err()
        .expect("SQL 声明变化必须拒绝复用")
        .to_string();
    assert!(error.contains("完整复制输入不同"), "{error}");

    let control = fixture.root.join("sql/ryframe_config.sql");
    let original = fs::read_to_string(&control).unwrap();
    let changed = original.replacen(
        "TEXT         NOT NULL                   COMMENT '公告内容'",
        "MEDIUMTEXT   NOT NULL                   COMMENT '公告内容'",
        1,
    );
    assert_ne!(changed, original);
    fs::write(&control, changed).unwrap();
    let error = window.issue().unwrap_err().to_string();
    assert!(error.contains("规范目录摘要变化"), "{error}");
    fs::write(control, original).unwrap();
    assert_eq!(window.issue().unwrap(), wrapper);
    window.check_end().unwrap();
    drop(window);

    let mut forged = wrapper;
    forged["plan"]["databases"][0]["preserve_target_ownership"] = json!(false);
    fs::write(&plan, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(HeldPlanObjects::open(&fixture.root, &plan, &forged).is_err());
}

#[test]
fn held_plan_still_verifies_unheld_evidence_and_exact_input_bytes() {
    let fixture = Fixture::new();
    let (plan, wrapper) = save(&fixture);
    let window = HeldPlanObjects::open(&fixture.root, &plan, &wrapper).unwrap();
    let descriptor = &fixture.value["evidence"]["source_stopped"];
    let file = fixture.export.join(descriptor["file"].as_str().unwrap());
    let original = fs::read(&file).unwrap();
    let mut changed = original.clone();
    changed.push(b' ');
    fs::write(&file, changed).unwrap();
    assert!(window.issue().is_err());
    fs::write(&file, original).unwrap();
    assert_eq!(window.issue().unwrap(), wrapper);
    let declaration = fs::read(&fixture.input).unwrap();
    let mut changed = declaration.clone();
    changed.push(b' ');
    fs::write(&fixture.input, changed).unwrap();
    assert!(window.issue().is_err());
    fs::write(&fixture.input, declaration).unwrap();
    window.check_end().unwrap();

    let mut bytes = fs::read(&plan).unwrap();
    bytes.push(b' ');
    fs::write(&plan, bytes).unwrap();
    assert!(window.issue().is_err());
    assert!(window.check_end().is_err());
}
