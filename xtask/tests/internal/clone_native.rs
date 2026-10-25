use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{cli::CloneCommand, recovery::clone::native, workspace};

static NEXT: AtomicUsize = AtomicUsize::new(1);
const UUID: &str = "67cd8d4a-fe90-11ef-98fd-5847ca786499";
type TestRow = Map<String, Value>;
type TestData = BTreeMap<String, Vec<(String, TestRow)>>;

struct Fixture {
    root: PathBuf,
    export: PathBuf,
    input: PathBuf,
    value: Value,
    data: TestData,
}

impl Fixture {
    fn new() -> Self {
        let (root, export, input) = fixture_paths();
        copy_catalog(&root);
        let evidence = initial_evidence(&export);
        let objects = initial_objects(&export);
        let mut data = BTreeMap::new();
        data.insert("shared-control".to_owned(), Vec::new());
        data.insert("dedicated-a".to_owned(), Vec::new());
        let mut fixture = Self {
            root,
            export,
            input,
            value: json!({
                "format_version": 1,
                "copy_id": "clone-case",
                "copy_stage": "source_to_seed",
                "target_schedule_actions": [],
                "app_env": "test",
                "artifact_root": "",
                "execution_backend": null,
                "source_snapshot": {
                    "head": "a".repeat(40),
                    "clean": false,
                    "worktree_sha256": "b".repeat(64),
                },
                "source": side("clone-source"),
                "target": {
                    "scope_id": "clone-target",
                    "object_endpoint": "http://127.0.0.1:29200",
                    "databases": side("clone-target")["databases"].clone(),
                    "state": "new_initialized",
                    "ever_started": false,
                },
                "evidence": evidence,
                "databases": [],
                "objects": objects,
            }),
            data,
        };
        fixture.value["artifact_root"] = json!(fixture.export);
        seed_initial_rows(&mut fixture);
        fixture.refresh();
        fixture
    }

    fn add(&mut self, key: &str, table: &str, values: &[(&str, Value)]) -> &mut Map<String, Value> {
        let templates = self.templates(key, table);
        let mut row = templates
            .into_iter()
            .map(|name| (name, Value::Null))
            .collect::<Map<_, _>>();
        for (name, value) in values {
            row.insert((*name).into(), value.clone());
        }
        let rows = self.data.get_mut(key).unwrap();
        rows.push((table.to_owned(), row));
        let (_, row) = rows.last_mut().unwrap();
        row
    }

    fn refresh(&mut self) {
        let control =
            table_columns(&fs::read_to_string(self.root.join("sql/ryframe_config.sql")).unwrap());
        let baseline = table_columns(
            &fs::read_to_string(self.root.join(
                "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
            ))
            .unwrap(),
        );
        let mut dumps = Vec::new();
        for (key, rows) in &self.data {
            let mut tables = BTreeMap::new();
            if key == "shared-control" {
                for table in control.keys() {
                    if ![
                        "ryframe_resource_ownership",
                        "sys_backup_set",
                        "sys_backup_resource",
                        "sys_restore_run",
                        "sys_tenant_data_backup_point",
                        "seaql_migrations",
                    ]
                    .contains(&table.as_str())
                    {
                        tables.insert(table.clone(), 0_u64);
                    }
                }
                for table in baseline
                    .keys()
                    .filter(|table| table.as_str() != "ryframe_resource_ownership")
                {
                    tables.insert(table.clone(), 0);
                }
            } else {
                for table in baseline
                    .keys()
                    .filter(|table| table.as_str() != "ryframe_resource_ownership")
                {
                    tables.insert(table.clone(), 0);
                }
            }
            let mut content = String::from("-- ryframe-reference-data-v1\n");
            for (table, row) in rows {
                *tables.get_mut(table).unwrap() += 1;
                content.push_str(&format!(
                    "INSERT INTO `{table}` ({}) VALUES ({});\n",
                    row.keys()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    row.values().map(sql_literal).collect::<Vec<_>>().join(",")
                ));
            }
            dumps.push(json!({
                "key": key,
                "tables": tables,
                "artifact": artifact(&self.export, &format!("{key}.sql"), content.as_bytes()),
            }));
        }
        self.value["databases"] = Value::Array(dumps);
    }

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

    fn templates(&self, key: &str, table: &str) -> Vec<String> {
        let mut result =
            table_columns(&fs::read_to_string(self.root.join("sql/ryframe_config.sql")).unwrap());
        if key != "shared-control" {
            result = BTreeMap::new();
        }
        if key == "shared-control" || matches!(table, "biz_tenant_fence" | "biz_tenant_target_slot")
        {
            let baseline = table_columns(
                &fs::read_to_string(self.root.join(
                    "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
                ))
                .unwrap(),
            );
            result.extend(baseline);
        }
        result.remove(table).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.root
                .starts_with(workspace::root_dir().join(".local-tests"))
        );
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn fixture_paths() -> (PathBuf, PathBuf, PathBuf) {
    let root = workspace::root_dir().join(format!(
        ".local-tests/clone-native-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let export = root.join(".local-tests/export");
    fs::create_dir_all(&export).unwrap();
    let input = root.join(".local-tests/input.json");
    (root, export, input)
}

fn copy_catalog(root: &Path) {
    for relative in [
        "sql/ryframe_config.sql",
        "crates/ryframe-tenant-db/src/generated/catalog.rs",
        "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
    ] {
        let target = root.join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(workspace::root_dir().join(relative), &target).unwrap();
    }
}

fn initial_evidence(export: &Path) -> Map<String, Value> {
    ["source_stopped", "target_initialized", "target_stopped"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                artifact(export, &format!("{name}.json"), b"{}"),
            )
        })
        .collect()
}

fn initial_objects(export: &Path) -> Vec<Value> {
    ["avatar", "config-packages", "exports", "imports", "uploads"]
        .into_iter()
        .map(|bucket| {
            let entries = if bucket == "uploads" {
                vec![json!({
                    "key": "clone-source/system/file.txt",
                    "artifact": artifact(export, "file.bin", b"data"),
                    "metadata": metadata(),
                })]
            } else {
                Vec::new()
            };
            json!({"bucket": bucket, "entries": entries})
        })
        .collect()
}

fn seed_initial_rows(fixture: &mut Fixture) {
    for (tenant, target) in [
        ("system", "shared-control"),
        ("clone-source-01", "dedicated-a"),
    ] {
        fixture.add(
            "shared-control",
            "sys_tenant",
            &[
                ("id", json!(if tenant == "system" { 1 } else { 2 })),
                ("tenant_id", json!(tenant)),
                ("name", json!(tenant)),
                ("status", json!("enabled")),
            ],
        );
        fixture.add(
            "shared-control",
            "sys_tenant_data_placement",
            &[
                ("tenant_id", json!(tenant)),
                ("current_target_key", json!(target)),
                ("placement_generation", json!(1)),
                ("switch_token", json!("token")),
                ("state", json!("active")),
            ],
        );
        fixture.add(
            target,
            "biz_tenant_fence",
            &[
                ("tenant_id", json!(tenant)),
                ("target_key", json!(target)),
                ("placement_generation", json!(1)),
                ("switch_token", json!("token")),
                ("state", json!("active")),
            ],
        );
    }
    fixture.add(
        "shared-control",
        "biz_tenant_target_slot",
        &[("slot_id", json!(1))],
    );
    fixture.add(
        "dedicated-a",
        "biz_tenant_target_slot",
        &[
            ("slot_id", json!(1)),
            ("tenant_id", json!("clone-source-01")),
            ("placement_generation", json!(1)),
            ("switch_token", json!("token")),
        ],
    );
    fixture.add(
        "shared-control",
        "sys_file",
        &[
            ("id", json!(1)),
            ("tenant_id", json!("system")),
            ("bucket", json!("uploads")),
            ("storage_path", json!("system/file.txt")),
            ("file_url", json!("uploads/system/file.txt")),
            ("file_size", json!(4)),
            ("file_sha256", json!(sha(b"data"))),
            ("upload_status", json!("ready")),
            ("del_flag", json!("0")),
        ],
    );
}

fn side(scope: &str) -> Value {
    let databases = BTreeMap::from([
        ("shared-control", ("combined", "shared")),
        ("dedicated-a", ("tenant", "dedicated")),
    ])
    .into_iter()
    .map(|(key, (kind, mode))| {
        let kinds = if kind == "combined" {
            &["control", "tenant-data"][..]
        } else {
            &["tenant-data"][..]
        };
        json!({
            "key": key,
            "kind": kind,
            "mode": mode,
            "server_uuid": UUID,
            "database": format!("{}_{}", scope.replace('-', "_"), key.replace('-', "_")),
            "schema_sha256": "c".repeat(64),
            "ownership": kinds.iter().map(|resource| json!({
                "resource_kind": resource,
                "scope_id": scope,
                "marker": format!("ryframe-owner:v1:{scope}:{resource}"),
            })).collect::<Vec<_>>(),
        })
    })
    .collect::<Vec<_>>();
    json!({
        "scope_id": scope,
        "object_endpoint": "http://127.0.0.1:29200",
        "databases": databases,
    })
}

fn artifact(root: &Path, name: &str, content: &[u8]) -> Value {
    let path = root.join(name);
    fs::write(&path, content).unwrap();
    json!({
        "file": name,
        "bytes": content.len(),
        "sha256": sha(content),
    })
}

fn metadata() -> Value {
    json!({
        "ContentType": "text/plain",
        "CacheControl": null,
        "ContentDisposition": null,
        "ContentEncoding": null,
        "ContentLanguage": null,
        "Expires": null,
        "WebsiteRedirectLocation": null,
        "Metadata": {},
    })
}

fn table_columns(source: &str) -> BTreeMap<String, Vec<String>> {
    let table =
        Regex::new(r"(?s)CREATE TABLE(?: IF NOT EXISTS)? `([a-z0-9_]+)` \((.*?)\n\s*\)\s*ENGINE=")
            .unwrap();
    let column = Regex::new(r"(?m)^\s*`([a-z0-9_]+)`\s+[A-Za-z]+").unwrap();
    table
        .captures_iter(source)
        .map(|part| {
            (
                part[1].to_owned(),
                column
                    .captures_iter(&part[2])
                    .map(|capture| capture[1].to_owned())
                    .collect(),
            )
        })
        .collect()
}

fn sql_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(value) => if *value { "1" } else { "0" }.into(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => quote(value),
        Value::Array(_) | Value::Object(_) => quote(&serde_json::to_string(value).unwrap()),
    }
}

fn quote(value: &str) -> String {
    format!(
        "'{}'",
        value
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

fn sha(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

#[path = "clone_native/catalog.rs"]
mod catalog;

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

#[test]
fn exact_schedule_disposition_is_bound_without_writing_resources() {
    let mut fixture = Fixture::new();
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

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                result.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    result
}
