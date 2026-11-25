use std::{cell::RefCell, process::ExitStatus};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;

use super::*;
use crate::clone_native_fixture::Fixture as SqlFixture;
use native::{
    export::{capture, database, transport::StdoutTarget},
    input::BUCKETS,
    s3::{Operation, Request},
    schedule, sql,
};

const DATABASE: &str = "shared-control";
const SCHEDULE: &str = "sys_job_schedule";
const SQL_TIME: &str = "2026-09-01 00:00:00.123456";
const OBJECT_KEY: &str = "fixture-source/system/file.txt";
const CLEANUPS: [(&str, &str); 3] = [
    ("system.export_result_cleanup", "system.export.cleanup"),
    (
        "system.message_retention_cleanup",
        "system.message.retention",
    ),
    (
        "system.data_retention_cleanup",
        "system.data_retention.cleanup",
    ),
];

#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Owner(String),
    InventoryBefore,
    InventoryAfter,
    ObjectHead,
    ObjectGet,
    ScheduleChanged,
}

struct Script {
    output: PathBuf,
    columns: String,
    raw_dump: String,
    changed_dump: Option<String>,
    inventory: Value,
    observations: RefCell<Vec<Observation>>,
}

impl Script {
    fn new(context: &Context, mut rows: SqlFixture, change_schedule: bool) -> Self {
        let models = database::models(context).unwrap();
        rows.refresh();
        let source_dump = rows.export.join(format!("{DATABASE}.sql"));
        let raw_dump = fs::read_to_string(&source_dump)
            .unwrap()
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n");
        let inventory = database_inventory(context, &models, &rows);
        let changed_dump = change_schedule.then(|| {
            let (table, row) = rows
                .data
                .get_mut(DATABASE)
                .unwrap()
                .iter_mut()
                .find(|(table, _)| table == SCHEDULE)
                .unwrap();
            assert_eq!(table, SCHEDULE);
            assert_eq!(row.insert("version".into(), json!(2)), Some(json!(1)));
            rows.refresh();
            fs::read_to_string(&source_dump).unwrap()
        });
        let mut columns = models.full_control;
        columns.extend(models.full_tenant);
        for ledger in ["seaql_migrations", "seaql_tenant_data_migrations"] {
            columns
                .entry(ledger.into())
                .or_default()
                .insert("version".into(), "VARCHAR".into());
        }
        let columns = columns
            .iter()
            .flat_map(|(table, columns)| {
                columns
                    .iter()
                    .map(move |(column, kind)| format!("{table}\t{column}\t{kind}\n"))
            })
            .collect();
        Self {
            output: context.output(),
            columns,
            raw_dump,
            changed_dump,
            inventory,
            observations: RefCell::new(Vec::new()),
        }
    }

    fn runtime_inventory(&self, command: &CommandSpec) {
        let when = argument(command, "--quiesced-at");
        let objects = BUCKETS
            .iter()
            .map(|bucket| {
                let entries = if *bucket == "uploads" {
                    json!([{"key":OBJECT_KEY,"bytes":4,"sha256":sha(b"data")}])
                } else {
                    json!([])
                };
                json!({"bucket":bucket,"prefix":"fixture-source/","entries":entries})
            })
            .collect::<Vec<_>>();
        write_json(
            Path::new(argument(command, "--output")),
            &json!({
                "scope_id":"fixture-source","source_sha":argument(command, "--source-sha"),
                "quiesced_at":when,"captured_at":when,
                "control_schema_fingerprint":"a".repeat(16),"tenant_schema_fingerprint":"b".repeat(64),
                "databases":[self.inventory],"objects":objects,
            }),
        );
    }

    fn observe(&self, observation: Observation) {
        self.observations.borrow_mut().push(observation);
    }

    fn finished_business_object(&self, path: &Path) -> crate::Result<()> {
        if let Some(dump) = &self.changed_dump
            && path.starts_with(self.output.join("objects"))
            && path.ends_with("owner-after-uploads.bin")
        {
            assert!(path.with_file_name("object.bin").is_file());
            fs::write(self.output.join("databases/shared-control.sql"), dump)?;
            self.observe(Observation::ScheduleChanged);
        }
        Ok(())
    }
}

impl Transport for Script {
    fn run(&self, command: CommandSpec) -> crate::Result<CommandOutput> {
        assert_eq!(command.work_dir, self.output);
        let stdout = match command.label.as_str() {
            "数据库 identity 核验" => format!("{UUID}\tfixture_shared_control"),
            "数据库 ownership 核验" => ["control", "tenant-data"]
                .map(|kind| {
                    format!("{kind}\tfixture-source\tryframe-owner:v1:fixture-source:{kind}\n")
                })
                .concat(),
            "源 schema 查询" => self.columns.clone(),
            "源数据库转储" => {
                let StdoutTarget::File(path) = command.stdout else {
                    panic!("转储必须写入新的原始 SQL 文件");
                };
                assert_eq!(path, self.output.join("databases/shared-control.raw.sql"));
                fs::write(path, &self.raw_dump)?;
                String::new()
            }
            "inventory-before" | "inventory-after" => {
                self.observe(if command.label == "inventory-before" {
                    Observation::InventoryBefore
                } else {
                    Observation::InventoryAfter
                });
                self.runtime_inventory(&command);
                String::new()
            }
            "inventory-before-target-shared-control" | "inventory-after-target-shared-control" => {
                assert_eq!(argument(&command, "--target"), DATABASE);
                write_json(
                    Path::new(argument(&command, "--output")),
                    &json!({
                        "scope_id":"fixture-source","control_schema_fingerprint":"a".repeat(16),
                        "tenant_schema_fingerprint":"b".repeat(64),
                        "target":{"database":self.inventory,"preserved_tables":[]},
                    }),
                );
                String::new()
            }
            "migrate-before-control" | "migrate-after-control" => {
                assert_eq!(command.args[2..], ["control", "verify"]);
                String::new()
            }
            "migrate-before-target-shared-control" | "migrate-after-target-shared-control" => {
                assert_eq!(
                    command.args[2..],
                    ["tenant-data", "verify", "--target", DATABASE]
                );
                String::new()
            }
            _ => panic!("出现未登记的导出命令：{}", command.label),
        };
        Ok(CommandOutput {
            status: ExitStatus::from_raw(0),
            stdout: stdout.into_bytes(),
            stderr: Vec::new(),
        })
    }

    fn s3(&self, request: Request) -> crate::Result<Value> {
        let after = match &request.operation {
            Operation::Get {
                key, destination, ..
            } if key == "fixture-source/.ryframe-owner" => {
                self.observe(Observation::Owner(request.bucket.clone()));
                Some(destination.clone())
            }
            Operation::Get { key, if_match, .. } => {
                assert_eq!(
                    (request.bucket.as_str(), key.as_str()),
                    ("uploads", OBJECT_KEY)
                );
                assert_eq!(if_match.as_deref(), Some("\"fixture-etag\""));
                self.observe(Observation::ObjectGet);
                None
            }
            Operation::Head { key, .. } => {
                assert_eq!(
                    (request.bucket.as_str(), key.as_str()),
                    ("uploads", OBJECT_KEY)
                );
                self.observe(Observation::ObjectHead);
                None
            }
            _ => panic!("导出不能访问此对象操作"),
        };
        let response = FakeS3 { wrong_body: false }.s3(request)?;
        if let Some(destination) = after {
            self.finished_business_object(&destination)?;
        }
        Ok(response)
    }
}

fn argument<'a>(command: &'a CommandSpec, name: &str) -> &'a str {
    &command
        .args
        .windows(2)
        .find(|pair| pair[0] == name)
        .unwrap()[1]
}

fn database_inventory(context: &Context, models: &database::Models, rows: &SqlFixture) -> Value {
    let observed = &rows.data[DATABASE];
    let tables = models
        .full_control
        .keys()
        .filter(|name| {
            ![
                "ryframe_resource_ownership",
                "sys_backup_set",
                "sys_backup_resource",
                "sys_restore_run",
            ]
            .contains(&name.as_str())
        })
        .chain(models.tenant.keys())
        .map(|table| {
            let count = observed.iter().filter(|(name, _)| name == table).count();
            json!({"table":table,"rows":count,"sha256":"c".repeat(64)})
        })
        .collect::<Vec<_>>();
    let database = &context.source()["databases"][0];
    json!({
        "key":database["key"],"kind":database["kind"],"server_uuid":database["server_uuid"],
        "database":database["database"],"shared":true,"tables":tables,
        "placements":[{"tenant_id":"system","generation":1,"switch_token":"token"}],
    })
}

fn context(fixture: &Fixture) -> Context {
    let directory = fixture.local.join("schedule-input");
    fs::create_dir(&directory).unwrap();
    let request_file = directory.join("source-request.json");
    let build = directory.join("backend-build.json");
    write_json(
        &build,
        &json!({"sources":{"full":{"source":{"snapshot":{"head":"a".repeat(40)}}}}}),
    );
    let maintenance = directory.join("maintenance.json");
    for tool in ["migrate", "tenant-data"] {
        fs::write(directory.join(tool), tool).unwrap();
    }
    write_json(
        &maintenance,
        &json!({"artifacts":{
            "migrate":{"executable":directory.join("migrate")},
            "tenant-data":{"executable":directory.join("tenant-data")}
        }}),
    );
    let mut request: Value =
        serde_json::from_slice(&fs::read(fixture.local.join("source-request.json")).unwrap())
            .unwrap();
    request["backend_build"] = binding(&build);
    request["maintenance_build"] = binding(&maintenance);
    write_json(&request_file, &request);
    let manifest_file = directory.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(fixture.local.join("manifest.json")).unwrap()).unwrap();
    manifest["source_request"] = binding(&request_file);
    manifest["copy_directory"] = json!(directory.join("copy"));
    fs::create_dir(directory.join("copy")).unwrap();
    write_json(&manifest_file, &manifest);
    let run = fixture.local.join("schedule-run");
    native::run(
        &CloneCommand::Init {
            manifest: manifest_file,
            run_dir: run.clone(),
        },
        &fixture.backend,
    )
    .unwrap();
    let stage = StageRequest {
        backend: &fixture.backend,
        run_dir: &run,
        stage: "export",
        mode: "run",
        attempt: Some(1),
    };
    let context = Context::load(&fixture.backend, &stage).unwrap();
    fs::create_dir(context.output()).unwrap();
    context
}

fn add_cleanup(rows: &mut SqlFixture, index: usize, job_type: &str) {
    let handler = CLEANUPS[index].0;
    let schedule = 101 + index;
    let job = 201 + index;
    rows.add(
        DATABASE,
        SCHEDULE,
        &[
            ("id", json!(schedule)),
            ("tenant_id", json!("system")),
            ("handler_key", json!(handler)),
            ("enabled", json!(1)),
            ("del_flag", json!("0")),
            ("version", json!(1)),
            ("next_run_at", json!("2026-09-02 00:00:00")),
            ("created_at", json!(SQL_TIME)),
            ("updated_at", json!(SQL_TIME)),
        ],
    );
    rows.add(
        DATABASE,
        "sys_background_job",
        &[
            ("id", json!(job)),
            ("tenant_id", Value::Null),
            ("job_type", json!(job_type)),
            ("status", json!("succeeded")),
            ("completed_at", json!("2026-09-01 00:00:01.000001")),
            ("schedule_id", json!(schedule)),
            ("scheduled_for", json!(SQL_TIME)),
            (
                "payload",
                json!({"schedule_id":schedule.to_string(),"trigger_kind":"scheduled",
            "scheduled_for":"2026-09-01T00:00:00.123456Z"}),
            ),
        ],
    );
    rows.add(
        DATABASE,
        "sys_job_schedule_execution",
        &[
            ("id", json!(301 + index)),
            ("tenant_id", json!("system")),
            ("schedule_id", json!(schedule)),
            ("background_job_id", json!(job)),
            ("outcome", json!("enqueued")),
            ("trigger_kind", json!("scheduled")),
            ("scheduled_for", json!(SQL_TIME)),
        ],
    );
}

fn run(context: &Context, script: &Script) -> crate::Result<Value> {
    capture::capture_data(
        context,
        script,
        &|| Ok(json!({"source":{"head":"a".repeat(40)}})),
        &|| Ok(()),
    )
}

fn system_rows() -> SqlFixture {
    let mut rows = SqlFixture::new();
    rows.data
        .get_mut(DATABASE)
        .unwrap()
        .retain(|(_, row)| row.get("tenant_id").and_then(Value::as_str) != Some("clone-source-01"));
    rows
}

fn assert_no_publication(output: &Path) {
    assert!(output.join("inventory-before.json").is_file());
    assert!(!output.join("inventory-after.json").exists());
    assert!(!output.join("inventory-after.command.json").exists());
    assert!(!output.join("export.json").exists());
}

#[test]
fn source_export_rejects_unreviewed_cleanup_before_business_objects() {
    let fixture = Fixture::new();
    let context = context(&fixture);
    let mut rows = system_rows();
    add_cleanup(&mut rows, 0, "unreviewed.cleanup");
    let script = Script::new(&context, rows, false);
    let error = run(&context, &script).unwrap_err().to_string();
    assert!(
        error.contains("全局调度父任务仅允许精确 system 维护目标"),
        "{error}"
    );
    let expected = BUCKETS
        .iter()
        .map(|bucket| Observation::Owner((*bucket).to_owned()))
        .chain([Observation::InventoryBefore])
        .collect::<Vec<_>>();
    assert_eq!(*script.observations.borrow(), expected);
    for bucket in BUCKETS {
        let owner = context.output().join(format!("owner-before-{bucket}.bin"));
        assert_eq!(
            fs::read_to_string(owner).unwrap(),
            format!("ryframe-owner:v1:fixture-source:object-storage:{bucket}")
        );
    }
    assert!(
        context
            .output()
            .join("databases/shared-control.sql")
            .is_file()
    );
    assert!(!context.output().join("objects").exists());
    assert_no_publication(&context.output());
}

#[test]
fn source_export_keeps_three_cleanup_schedules_across_business_objects() {
    let fixture = Fixture::new();
    let context = context(&fixture);
    let mut rows = system_rows();
    for (index, (_, job_type)) in CLEANUPS.iter().enumerate() {
        add_cleanup(&mut rows, index, job_type);
    }
    let script = Script::new(&context, rows, false);
    let published = run(&context, &script).unwrap();
    let output = context.output();
    let path = output.join("export.json");
    assert_eq!(Path::new(published["path"].as_str().unwrap()), path);
    let expected_binding = binding(&path);
    assert_eq!(published["bytes"], expected_binding["bytes"]);
    assert_eq!(published["sha256"], expected_binding["sha256"]);
    let export: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(export["status"], "source_export_captured");
    assert_eq!(export["clone_verified"], false);
    assert_eq!(export["restore_qualified"], false);
    assert_eq!(export["remote_writes"], 0);
    let models = database::models(&context).unwrap();
    let columns = sql::all_columns(&models.control, &models.tenant, true);
    let mut expected = Vec::new();
    sql::for_each_row(
        &output.join("databases/shared-control.sql"),
        &columns,
        Some(SCHEDULE),
        |_, row| {
            let (handler, _) = CLEANUPS[expected.len()];
            let id = (101 + expected.len()).to_string();
            assert_eq!(row.get("version").and_then(sql::SqlValue::integer), Some(1));
            expected.push(
                json!({"tenant_id":"system","schedule_id":id,"handler_key":handler,
            "expected_version":1,"source_row_sha256":schedule::row_sha256(row)?}),
            );
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(expected.len(), CLEANUPS.len());
    assert_eq!(export["enabled_system_schedule_rows"], json!(expected));
    assert!(output.join("inventory-before.json").is_file());
    assert!(output.join("inventory-after.json").is_file());
    let observed = script.observations.borrow();
    let sequence = observed
        .iter()
        .filter(|entry| !matches!(entry, Observation::Owner(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        sequence,
        [
            &Observation::InventoryBefore,
            &Observation::ObjectHead,
            &Observation::ObjectGet,
            &Observation::ObjectHead,
            &Observation::InventoryAfter,
        ]
    );
}

#[test]
fn source_export_rejects_changed_schedule_before_final_inventory() {
    let fixture = Fixture::new();
    let context = context(&fixture);
    let mut rows = system_rows();
    add_cleanup(&mut rows, 0, CLEANUPS[0].1);
    let script = Script::new(&context, rows, true);
    let error = run(&context, &script).unwrap_err().to_string();
    assert!(error.contains("源调度行在对象抓取前后变化"), "{error}");
    let output = context.output();
    assert_eq!(
        fs::read_to_string(output.join("databases/shared-control.sql")).unwrap(),
        *script.changed_dump.as_ref().unwrap()
    );
    let object = format!(
        "objects/{}",
        sha(format!("uploads:{OBJECT_KEY}").as_bytes())
    );
    assert_eq!(
        fs::read(output.join(&object).join("object.bin")).unwrap(),
        b"data"
    );
    assert!(output.join(object).join("capture.json").is_file());
    let observed = script.observations.borrow();
    let sequence = observed
        .iter()
        .filter(|entry| !matches!(entry, Observation::Owner(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        sequence,
        [
            &Observation::InventoryBefore,
            &Observation::ObjectHead,
            &Observation::ObjectGet,
            &Observation::ObjectHead,
            &Observation::ScheduleChanged,
        ]
    );
    assert_no_publication(&output);
}
