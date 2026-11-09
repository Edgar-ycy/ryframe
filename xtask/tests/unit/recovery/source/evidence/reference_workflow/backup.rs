use super::*;
use native::{
    export::transport::{CommandOutput, CommandSpec, StdoutTarget},
    s3::{Operation, Request},
};
use std::{cell::RefCell, path::PathBuf, process::ExitStatus, time::Duration};

fn backend() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn backup_inventory_is_derived_from_unchanged_raw_input() {
    let plan = json!({"id":"new-backup"});
    let raw = json!({"source_sha":"a".repeat(40)});
    let source = || Ok(json!({"clean":true,"head":"a".repeat(40)}));
    let inventory = model::backup_inventory(&plan, &raw, source).unwrap();
    assert_eq!(inventory["id"], "new-backup");
    assert!(raw.get("id").is_none());
    assert!(
        model::backup_inventory(&plan, &inventory, || panic!("handwritten id read Git")).is_err()
    );
    for bad in [
        json!({"clean":false,"head":raw["source_sha"]}),
        json!({"clean":true,"head":"b".repeat(40)}),
    ] {
        assert!(model::backup_inventory(&plan, &raw, || Ok(bad)).is_err());
    }
}

#[test]
fn existing_pid_is_rejected_even_when_its_identity_differs() {
    let directory = super::super::tests::temporary();
    let side = json!({"runtime_dir":directory.path(),"scope_id":"source"});
    for (role, pid) in [("api", 123), ("worker", 124)] {
        native::json::write_new(
            &directory.path().join(format!("{role}.json")),
            &json!({"format_version":1,"role":role,"scope_id":"source","identity":{"pid":pid}}),
        )
        .unwrap();
    }
    let seen = RefCell::new(Vec::new());
    assert!(
        require_stopped(backend(), &side, |pid| {
            seen.borrow_mut().push(pid);
            Ok((pid == 124).then(|| json!({"pid":pid,"started":"new-process"})))
        })
        .is_err()
    );
    assert_eq!(seen.into_inner(), [123, 124]);
    require_stopped(backend(), &side, |_| Ok(None)).unwrap();
}

#[test]
fn changed_source_leaves_payloads_and_cannot_publish_manifest() {
    let fixture = super::super::tests::Fixture::new();
    let file = fixture.root.join("manifest.json");
    let original = fixture
        .root
        .parent()
        .unwrap()
        .join("original-manifest.json");
    fs::rename(&file, &original).unwrap();
    let source = json!({"source_export":{"original":1},"source_generation":{"same":true}});
    let mut inventory = fixture.manifest.clone();
    let artifacts = inventory
        .as_object_mut()
        .unwrap()
        .remove("artifacts")
        .unwrap();
    let publish_backup = |final_source: Value| {
        publish(
            backend(),
            &fixture.root,
            &fixture.plan,
            &inventory,
            artifacts.as_array().unwrap().clone(),
            &source,
            || Ok(final_source),
        )
    };
    let changed = json!({"source_export":{"original":2},"source_generation":{"same":true}});
    assert!(publish_backup(changed).is_err());
    assert!(!file.exists());
    assert!(fixture.root.join("databases/control.sql").is_file());
    let result = publish_backup(source.clone()).unwrap();
    let document = document::read(&file, backend(), "fixture").unwrap();
    assert_eq!(result["format_version"], 2);
    assert_eq!(result["kind"], "restore-reference-backup");
    assert_eq!(result["source_export"], source["source_export"]);
    assert_eq!(result["source_generation"], source["source_generation"]);
    assert_eq!(result["manifest"], document::binding(&document));
    assert_eq!(result["reference_plan"], fixture.plan);
    assert!(publish_backup(source.clone()).is_err());
    document.assert_unchanged().unwrap();
}

#[derive(Default)]
struct Fake {
    commands: RefCell<Vec<Vec<String>>>,
    gets: RefCell<Vec<(String, String, u64)>>,
    wrong_object: bool,
    fail_dump: bool,
}

impl Transport for Fake {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        let StdoutTarget::File(raw) = spec.stdout else {
            panic!("转储必须保留原始文件");
        };
        assert!(spec.stdin.is_none());
        assert_eq!(spec.timeout, Duration::from_secs(1800));
        assert!(
            !spec
                .environment
                .keys()
                .any(|key| key.eq_ignore_ascii_case("MYSQL_PWD"))
        );
        self.commands.borrow_mut().push(spec.args);
        fs::write(
            raw,
            "/*!40101 SET @OLD=1 */;\nINSERT INTO `sys_post` (`id`) VALUES (1);\n",
        )?;
        Ok(CommandOutput {
            status: exit_status(self.fail_dump),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    fn s3(&self, request: Request) -> Result<Value> {
        let Operation::Get {
            key,
            if_match: None,
            version: None,
            destination,
            max_bytes,
        } = request.operation
        else {
            panic!("参考备份只发送原 scope 的无条件 GET");
        };
        let bucket = request.bucket;
        self.gets
            .borrow_mut()
            .push((bucket.clone(), key.clone(), max_bytes));
        if key == "source/.ryframe-owner" {
            fs::write(
                destination,
                format!("ryframe-owner:v1:source:object-storage:{bucket}"),
            )?;
        } else {
            assert_eq!(key, "source/tenant/file.txt");
            fs::write(
                destination,
                if self.wrong_object {
                    b"other"
                } else {
                    b"bytes"
                },
            )?;
        }
        Ok(json!({"ContentType":"text/plain; charset=utf-8"}))
    }
}

fn exit_status(failed: bool) -> ExitStatus {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    #[cfg(unix)]
    let code = i32::from(failed) << 8;
    #[cfg(windows)]
    let code = u32::from(failed);
    ExitStatus::from_raw(code)
}

#[test]
fn reads_all_five_owners_and_bounded_payload_then_checks_same_length_hash() {
    use sha2::{Digest, Sha256};

    let fixture = super::super::tests::Fixture::new();
    let scope = &fixture.plan["source"];
    let inventory = json!({"objects":inputs::domain::BUCKETS.map(|bucket| {
        let entries = if bucket == "uploads" {
            vec![json!({"key":"source/tenant/file.txt","bytes":5,"sha256":hex::encode(Sha256::digest(b"bytes"))})]
        } else {
            vec![]
        };
        json!({"bucket":bucket,"entries":entries})
    })});
    for wrong in [false, true] {
        let fake = Fake {
            wrong_object: wrong,
            ..Fake::default()
        };
        let root = fixture.root.join(if wrong { "wrong" } else { "good" });
        fs::create_dir(&root).unwrap();
        let storage = objects::Storage {
            backend: backend(),
            source: scope,
            transport: &fake,
        };
        storage.verify_owners(&root).unwrap();
        let mut artifacts = Vec::new();
        let stored = storage.store_buckets(&root, &inventory, &mut artifacts);
        let calls = fake.gets.into_inner();
        assert_eq!(calls.len(), 6);
        assert!(
            calls[..5]
                .iter()
                .all(|(_, key, cap)| key == "source/.ryframe-owner" && *cap == 4096)
        );
        assert_eq!(
            calls[5],
            ("uploads".into(), "source/tenant/file.txt".into(), 5)
        );
        let index = root.join("objects/uploads/index.json");
        if wrong {
            assert!(stored.is_err());
            assert!(!index.exists());
            assert_eq!(fs::read_dir(index.parent().unwrap()).unwrap().count(), 1);
        } else {
            stored.unwrap();
            assert_eq!(artifacts.len(), 6);
            let entry = &native::json::read(&index).unwrap()["entries"][0];
            assert_eq!(entry["content_type"], "text/plain; charset=utf-8");
            assert_eq!(
                fs::read(root.join(entry["file"].as_str().unwrap())).unwrap(),
                b"bytes"
            );
        }
    }
}

fn database_inputs(fixture: &mut super::super::tests::Fixture) -> (PathBuf, PathBuf) {
    let work = fixture.root.parent().unwrap().to_owned();
    let defaults = path(&fixture.plan["source"]["databases"][0]["defaults_file"]).unwrap();
    fs::write(
        defaults,
        "[client]\nhost=127.0.0.1\nport=3307\nuser=fixture\npassword=fixture\n",
    )
    .unwrap();
    let hash = native::digest::file(defaults).unwrap().sha256;
    for side in ["source", "target"] {
        fixture.plan[side]["databases"][0]["defaults_sha256"] = json!(hash);
    }
    // 测试更换凭据文件后的计划使用单独新目录与新 owner。
    let fresh = work.join("backup-read");
    fixture.plan["work_dir"] = json!(fresh);
    super::super::files::work(&json!({"plan":fixture.plan}), true).unwrap();
    let runtime = path(&fixture.plan["source"]["runtime_dir"]).unwrap();
    fs::create_dir(runtime).unwrap();
    native::json::write_new(&runtime.join("api.json"), &json!({"fixture":true})).unwrap();
    let plan_path = work.join("dump-plan.json");
    native::json::write_new(&plan_path, &fixture.plan).unwrap();
    (fresh, plan_path)
}

#[test]
fn dump_passes_inventory_order_and_preserves_failed_raw_output() {
    let mut fixture = super::super::tests::Fixture::new();
    let (work, plan_path) = database_inputs(&mut fixture);
    let databases = BackupDatabases::capture(backend(), &plan_path, &fixture.plan, &work).unwrap();
    let database = &fixture.plan["source"]["databases"][0];
    let tables = ["sys_post", "other_table"];
    for fail_dump in [false, true] {
        let fake = Fake {
            fail_dump,
            ..Fake::default()
        };
        let file = work.join(if fail_dump {
            "failed.sql"
        } else {
            "complete.sql"
        });
        let result = databases.dump(&fake, database, &tables, &file);
        let commands = fake.commands.into_inner();
        assert_eq!(commands.len(), 1);
        assert_eq!(
            &commands[0][13..],
            ["source_control", "sys_post", "other_table"]
        );
        if fail_dump {
            assert!(result.is_err());
            assert!(file.with_extension("raw.sql").is_file());
            assert!(!file.exists());
            let diagnostic =
                native::json::read(&work.join("backup-mysqldump-control.json")).unwrap();
            assert_eq!(diagnostic["returncode"], 1);
        } else {
            result.unwrap();
            assert!(!file.with_extension("raw.sql").exists());
            assert_eq!(
                fs::read_to_string(file).unwrap(),
                "-- ryframe-reference-data-v1\nINSERT INTO `sys_post` (`id`) VALUES (1);\n"
            );
        }
    }
}

#[test]
fn native_dispatch_records_failure_before_live_reads_and_blocks_replay() {
    let mut fixture = super::super::tests::Fixture::new();
    let (work, plan_path) = database_inputs(&mut fixture);
    let inventory = work.join("raw.json");
    native::json::write_new(&inventory, &json!({"id":"handwritten"})).unwrap();
    let missing = work.join("unused.json");
    let raw = json!({"format_version":1,"kind":"ryframe-xtask-recovery-reference","request":{
        "format_version":1,"backend_dir":backend(),"operation":"backup","plan":plan_path,"write":true,
        "inventory":inventory,"source_generation":missing,"source_export_result":missing,
    }}).to_string();
    let first = super::super::run_local(&raw).unwrap_err();
    assert!(first.to_string().contains("不能手工补写"));
    let receipt = work.join("backup.json");
    let saved = native::json::read(&receipt).unwrap();
    assert_eq!(saved["status"], "failed");
    assert_eq!(saved["failure"]["error_type"], "ReferenceBackupFailure");
    assert!(!work.join("backup").exists());
    assert!(super::super::run_local(&raw).is_err());
    assert_eq!(native::json::read(&receipt).unwrap(), saved);
}
