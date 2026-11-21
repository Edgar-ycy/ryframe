use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    cli::CloneCommand,
    clone_native_fixture::{Fixture, TestRow, sha, snapshot, sql_literal},
    recovery::clone::native,
};
impl Fixture {
    fn write(&self) {
        fs::write(
            &self.input,
            serde_json::to_string_pretty(&self.value).unwrap(),
        )
        .unwrap();
    }

    fn plan(&self, name: &str) -> Result<PathBuf, String> {
        self.write();
        let output = self.root.join(".local-tests").join(name);
        native::run(
            &CloneCommand::Plan {
                input: self.input.clone(),
                output: output.clone(),
            },
            &self.root,
        )
        .map_err(|error| error.to_string())?;
        Ok(output)
    }

    fn verify(&self, plan: &Path) -> Result<(), String> {
        native::run(
            &CloneCommand::Verify {
                plan: plan.to_path_buf(),
            },
            &self.root,
        )
        .map_err(|error| error.to_string())
    }
}

#[path = "clone_native/catalog.rs"]
mod catalog;
#[path = "clone_native/jobs.rs"]
mod jobs;
#[path = "clone_native/plan_publication.rs"]
mod plan_publication;
#[path = "clone_native/sql.rs"]
mod sql;

#[test]
fn native_plan_and_verify_are_deterministic_and_read_only_after_write() {
    let fixture = Fixture::new();
    let plan = fixture.plan("plan.json").unwrap();
    let value: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    assert_eq!(value["plan"]["status"], "offline_verified");
    assert_eq!(value["plan"]["execution_authorized"], false);
    assert_eq!(value["plan"]["target_ready"], false);
    assert_eq!(value["plan"]["objects"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["plan"]["objects"][0]["target_key"],
        "clone-target/system/file.txt"
    );
    let second = fixture.plan("plan-again.json").unwrap();
    let second: Value = serde_json::from_slice(&fs::read(second).unwrap()).unwrap();
    assert_eq!(second["plan"]["plan_sha256"], value["plan"]["plan_sha256"]);
    let before = snapshot(&fixture.root);
    fixture.verify(&plan).unwrap();
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn duplicate_unknown_and_missing_json_fields_fail_closed() {
    let mut fixture = Fixture::new();
    for raw in [
        "{\"format_version\":1,\"format_version\":1}",
        "{\"database_time\":1,\"database_time\":2}",
        "{\"active\":{\"x\":0,\"x\":1}}",
    ] {
        fs::write(&fixture.input, raw).unwrap();
        assert!(
            native::run(
                &CloneCommand::Plan {
                    input: fixture.input.clone(),
                    output: fixture.root.join(".local-tests/duplicate.json"),
                },
                &fixture.root,
            )
            .unwrap_err()
            .to_string()
            .contains("重复")
        );
    }

    let mut missing = fixture.value.clone();
    missing.as_object_mut().unwrap().remove("copy_id");
    fixture.value = missing;
    assert!(fixture.plan("missing.json").is_err());

    let mut unknown = fixture.value.clone();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("unknown".into(), json!(true));
    fixture.value = unknown;
    assert!(fixture.plan("unknown.json").is_err());
}

#[test]
fn invalid_artifact_digest_and_size_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.value["evidence"]["source_stopped"]["sha256"] = json!("d".repeat(64));
    assert!(fixture.plan("bad-sha.json").is_err());

    fixture.value["evidence"]["source_stopped"]["sha256"] = json!(sha(b"{}"));
    fixture.value["evidence"]["source_stopped"]["bytes"] = json!(1);
    assert!(fixture.plan("bad-size.json").is_err());
}

#[test]
fn oversized_json_and_unclosed_job_relations_are_rejected() {
    let fixture = Fixture::new();
    fs::write(&fixture.input, vec![b' '; 16 * 1024 * 1024 + 1]).unwrap();
    assert!(
        native::run(
            &CloneCommand::Plan {
                input: fixture.input.clone(),
                output: fixture.root.join(".local-tests/oversized.json"),
            },
            &fixture.root,
        )
        .is_err()
    );

    let mut fixture = Fixture::new();
    fixture.add(
        "shared-control",
        "sys_background_job_attempt",
        &[
            ("job_id", json!(7)),
            ("sequence", json!(1)),
            ("outcome", json!("succeeded")),
            ("closed_at", json!("2026-09-01 00:00:00")),
            ("finished_at", json!("2026-09-01 00:00:00")),
        ],
    );
    fixture.refresh();
    assert!(fixture.plan("unclosed-job.json").is_err());
}

#[test]
fn source_target_overlap_and_non_new_target_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.value["target"]["scope_id"] = json!("clone-source");
    assert!(fixture.plan("same-scope.json").is_err());

    fixture.value["target"]["scope_id"] = json!("clone-target");
    fixture.value["target"]["databases"][0]["database"] =
        fixture.value["source"]["databases"][0]["database"].clone();
    assert!(fixture.plan("same-database.json").is_err());

    fixture.value["target"]["databases"][0]["database"] = json!("clone_target_shared_control");
    fixture.value["target"]["state"] = json!("existing");
    assert!(fixture.plan("not-new.json").is_err());
}

#[test]
fn malformed_buckets_keys_and_metadata_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.value["objects"].as_array_mut().unwrap().pop();
    assert!(fixture.plan("bucket.json").is_err());

    fixture = Fixture::new();
    fixture.value["objects"][4]["entries"][0]["key"] = json!("other/file");
    assert!(fixture.plan("key.json").is_err());

    fixture = Fixture::new();
    fixture.value["objects"][4]["entries"][0]["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("Expires");
    assert!(fixture.plan("metadata.json").is_err());
}

#[test]
fn incomplete_tables_and_abnormal_states_fail_before_plan_output() {
    let mut fixture = Fixture::new();
    let tables = fixture.value["databases"][0]["tables"]
        .as_object_mut()
        .unwrap();
    let first = tables.keys().next().unwrap().clone();
    tables.remove(&first);
    assert!(fixture.plan("columns.json").is_err());

    fixture = Fixture::new();
    set_row(
        &mut fixture,
        "shared-control",
        "sys_file",
        "upload_status",
        json!("cleanup"),
    );
    fixture.refresh();
    assert!(fixture.plan("state.json").is_err());

    fixture = Fixture::new();
    set_row(
        &mut fixture,
        "dedicated-a",
        "biz_tenant_fence",
        "state",
        json!("frozen"),
    );
    fixture.refresh();
    assert!(fixture.plan("fence.json").is_err());

    fixture = Fixture::new();
    set_row(
        &mut fixture,
        "shared-control",
        "sys_tenant_data_placement",
        "state",
        json!("frozen"),
    );
    fixture.refresh();
    assert!(fixture.plan("placement.json").is_err());
}

#[test]
fn active_schedule_without_exact_disposition_is_rejected() {
    let mut fixture = Fixture::new();
    fixture.add(
        "shared-control",
        "sys_job_schedule",
        &[
            ("id", json!(3)),
            ("tenant_id", json!("system")),
            ("handler_key", json!("system.data_retention_cleanup")),
            ("enabled", json!(1)),
            ("del_flag", json!("0")),
            ("version", json!(1)),
        ],
    );
    fixture.refresh();
    assert!(fixture.plan("schedule.json").is_err());
}

fn pending_schedule_disposition() -> (Fixture, usize) {
    let mut fixture = Fixture::new();
    let schedule_index = fixture.data["shared-control"].len();
    let row = fixture
        .add(
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
        )
        .clone();
    fixture.refresh();
    let sql_row = row
        .iter()
        .map(|(name, value)| {
            let value = match value {
                Value::Null => native::sql::SqlValue::Null,
                Value::Bool(value) => {
                    native::sql::SqlValue::Number(if *value { "1" } else { "0" }.into())
                }
                Value::Number(value) => native::sql::SqlValue::Number(value.to_string()),
                Value::String(value) => native::sql::SqlValue::Text(value.clone()),
                Value::Array(_) | Value::Object(_) => native::sql::SqlValue::Json(value.clone()),
            };
            (name.clone(), value)
        })
        .collect();
    let digest = native::schedule::row_sha256(&sql_row).unwrap();
    fixture.value["target_schedule_actions"] = json!([{
        "action": "disable_schedule_via_api",
        "tenant_id": "system",
        "schedule_id": "3",
        "handler_key": "system.data_retention_cleanup",
        "expected_version": 1,
        "source_row_sha256": digest,
    }]);
    fixture.refresh();
    (fixture, schedule_index)
}

#[test]
fn exact_schedule_disposition_is_bound_without_writing_resources() {
    let (mut fixture, schedule_index) = pending_schedule_disposition();
    let plan = fixture.plan("schedule-pending.json").unwrap();
    let value: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    assert_eq!(
        value["plan"]["status"],
        "offline_verified_pending_target_actions"
    );
    assert_eq!(
        value["plan"]["pending_target_actions"][0]["api_called"],
        false
    );
    assert_eq!(
        value["plan"]["pending_target_actions"][0]["body"],
        json!({"version": 1, "enabled": false})
    );

    fixture.value["copy_stage"] = json!("seed_to_arm");
    let pending = "arm-pending.json";
    let error = fixture.plan(pending).unwrap_err();
    assert!(error.contains("只有来源到 seed"), "{error}");
    assert!(
        !fixture
            .root
            .join(".local-tests")
            .join(pending)
            .try_exists()
            .unwrap()
    );

    // 用同一条调度模拟 seed 完成停用后的导出。
    let (_, row) = &mut fixture.data.get_mut("shared-control").unwrap()[schedule_index];
    row.insert("enabled".into(), json!(0));
    row.insert("next_run_at".into(), Value::Null);
    row.insert("version".into(), json!(2));
    row.insert("updated_at".into(), json!("2026-09-04 00:01:00"));
    fixture.value["target_schedule_actions"] = json!([]);
    fixture.refresh();
    let plan = fixture.plan("arm-disabled.json").unwrap();
    let value: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    assert_eq!(value["plan"]["status"], "offline_verified");
    assert_eq!(value["plan"]["pending_target_actions"], json!([]));

    let (_, row) = &mut fixture.data.get_mut("shared-control").unwrap()[schedule_index];
    row.insert("enabled".into(), json!(1));
    row.insert("tenant_id".into(), json!("clone-source-01"));
    fixture.refresh();
    let unmatched = "arm-unmatched.json";
    let error = fixture.plan(unmatched).unwrap_err();
    assert!(error.contains("精确匹配"), "{error}");
    assert!(
        !fixture
            .root
            .join(".local-tests")
            .join(unmatched)
            .try_exists()
            .unwrap()
    );
}

#[test]
fn schedule_action_protocol_preserves_order_and_rejects_id_and_shape_boundaries() {
    let action = json!({"action": "disable_schedule_via_api", "tenant_id": "system",
        "schedule_id": "9", "handler_key": "system.cleanup", "expected_version": 1,
        "source_row_sha256": "a".repeat(64)});
    let mut second = action.clone();
    second["schedule_id"] = json!("2");
    let input =
        json!({"copy_stage": "source_to_seed", "target_schedule_actions": [action, second]});
    assert_eq!(
        native::schedule::request(&input).unwrap(),
        input["target_schedule_actions"]
    );
    for id in ["0", "01", "-1", "1２", "1٢", "9223372036854775808"] {
        let mut invalid = input.clone();
        invalid["target_schedule_actions"][0]["schedule_id"] = json!(id);
        assert!(native::schedule::request(&invalid).is_err(), "{id}");
    }
    let mut maximum = input.clone();
    maximum["target_schedule_actions"][0]["schedule_id"] = json!(i64::MAX.to_string());
    assert!(native::schedule::request(&maximum).is_ok());
    for version in [json!(true), json!(0), json!(i64::MAX)] {
        let mut invalid = input.clone();
        invalid["target_schedule_actions"][0]["expected_version"] = version;
        assert!(native::schedule::request(&invalid).is_err());
    }
    let mut duplicate = input.clone();
    duplicate["target_schedule_actions"][1] = duplicate["target_schedule_actions"][0].clone();
    assert!(native::schedule::request(&duplicate).is_err());
    let mut extra_field = input.clone();
    extra_field["target_schedule_actions"][0]["unexpected"] = json!(true);
    assert!(native::schedule::request(&extra_field).is_err());
    let oversized = json!({"copy_stage": "source_to_seed", "target_schedule_actions":
        vec![input["target_schedule_actions"][0].clone(); 101]});
    assert!(native::schedule::request(&oversized).is_err());
    let mut wrong_stage = input.clone();
    wrong_stage["copy_stage"] = json!("seed_to_arm");
    assert!(native::schedule::request(&wrong_stage).is_err());
    let empty = json!({"copy_stage": "seed_to_arm", "target_schedule_actions": []});
    assert_eq!(native::schedule::request(&empty).unwrap(), json!([]));
    let mut extra = input.clone();
    extra["unexpected"] = json!(false);
    assert!(native::schedule::request(&extra).is_err());
    let mut missing = input;
    missing.as_object_mut().unwrap().remove("copy_stage");
    assert!(native::schedule::request(&missing).is_err());
}

#[test]
fn existing_outputs_and_outputs_inside_evidence_root_are_rejected() {
    let fixture = Fixture::new();
    fixture.write();
    let output = fixture.root.join(".local-tests/existing.json");
    fs::write(&output, b"keep").unwrap();
    assert!(
        native::run(
            &CloneCommand::Plan {
                input: fixture.input.clone(),
                output: output.clone(),
            },
            &fixture.root,
        )
        .is_err()
    );
    assert_eq!(fs::read(output).unwrap(), b"keep");

    assert!(
        native::run(
            &CloneCommand::Plan {
                input: fixture.input.clone(),
                output: fixture.export.join("plan.json"),
            },
            &fixture.root,
        )
        .is_err()
    );
}

#[test]
fn artifact_input_and_policy_drift_after_write_are_rejected() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan("plan.json").unwrap();
    fs::write(fixture.export.join("file.bin"), b"evil").unwrap();
    assert!(fixture.verify(&plan).is_err());
    fs::write(fixture.export.join("file.bin"), b"data").unwrap();

    fixture.value["copy_id"] = json!("clone-changed");
    fixture.write();
    assert!(fixture.verify(&plan).is_err());

    fixture.value["copy_id"] = json!("clone-case");
    fixture.write();
    let second = fixture.plan("second.json").unwrap();
    let wrapper: Value = serde_json::from_slice(&fs::read(&second).unwrap()).unwrap();
    fixture.value["copy_stage"] = json!("seed_to_arm");
    fixture.write();
    assert!(native::issue(&fixture.root, &second, &wrapper).is_err());

    let baseline = fixture
        .root
        .join("crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs");
    let mut source = fs::read_to_string(&baseline).unwrap();
    source.push_str(
        "\nCREATE TABLE IF NOT EXISTS `biz_future` (\n    `id` BIGINT\n) ENGINE=InnoDB;\n",
    );
    fs::write(baseline, source).unwrap();
    assert!(fixture.verify(&second).is_err());
}

#[cfg(unix)]
#[test]
fn linked_inputs_are_rejected() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    fixture.write();
    let link = fixture.root.join(".local-tests/linked-input.json");
    symlink(&fixture.input, &link).unwrap();
    assert!(
        native::run(
            &CloneCommand::Plan {
                input: link,
                output: fixture.root.join(".local-tests/linked-plan.json"),
            },
            &fixture.root,
        )
        .is_err()
    );
}

#[cfg(windows)]
#[test]
fn linked_inputs_are_rejected() {
    use std::os::windows::fs::symlink_file;

    let fixture = Fixture::new();
    fixture.write();
    let link = fixture.root.join(".local-tests/linked-input.json");
    if let Err(error) = symlink_file(&fixture.input, &link) {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return;
        }
        panic!("无法建立链接夹具：{error}");
    }
    assert!(
        native::run(
            &CloneCommand::Plan {
                input: link,
                output: fixture.root.join(".local-tests/linked-plan.json"),
            },
            &fixture.root,
        )
        .is_err()
    );
}

#[cfg(windows)]
#[test]
fn long_chinese_input_paths_are_supported() {
    let fixture = Fixture::new();
    let mut directory = fixture.root.join(".local-tests");
    for _ in 0..5 {
        directory.push(format!("长路径 空格 {}", "x".repeat(40)));
    }
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("复制 输入.json");
    fs::write(&input, serde_json::to_vec_pretty(&fixture.value).unwrap()).unwrap();
    assert!(input.as_os_str().len() > 260);
    native::run(
        &CloneCommand::Plan {
            input,
            output: fixture.root.join(".local-tests/long-plan.json"),
        },
        &fixture.root,
    )
    .unwrap();
}

fn set_row(fixture: &mut Fixture, key: &str, table: &str, field: &str, value: Value) {
    let row = fixture
        .data
        .get_mut(key)
        .unwrap()
        .iter_mut()
        .find(|(name, _)| name == table)
        .map(|(_, row)| row)
        .unwrap();
    row.insert(field.into(), value);
}
