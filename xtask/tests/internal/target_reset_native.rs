use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use crate::{
    recovery::clone::native::{
        json as json_io,
        target_verify::{evidence_protocol, reset},
    },
    workspace,
};

struct Fixture {
    backend: PathBuf,
    _directory: tempfile::TempDir,
    target: PathBuf,
    manifest: Value,
    hash: String,
}

impl Fixture {
    fn new() -> Self {
        let backend = workspace::root_dir();
        let parent = backend.join(".local-tests/target-reset-proof");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let target = directory.path().join("target");
        fs::create_dir_all(target.join("reset-state")).unwrap();
        let scope = "审阅目标";
        let manifest = json!({
            "environment": "test", "scope_id": "reset-proof",
            "code_sha": "a".repeat(40), "config_sha": "b".repeat(64),
            "credential_version": "explicit-fixture-only",
            "databases": [{"host": "127.0.0.1", "port": 3306, "database": "fixture"}],
            "redis": {"namespace": scope},
            "object_storage": {"prefixes": [{"bucket": "fixture", "prefix": "reset-proof/"}]},
        });
        let hash = "c".repeat(64);
        let phases: serde_json::Map<String, Value> = [
            "preflight",
            "object_storage",
            "redis",
            "databases",
            "control_baseline",
            "tenant_baselines",
            "verification",
            "release",
        ]
        .into_iter()
        .map(|phase| {
            (
                phase.into(),
                json!({
                    "status": "complete", "started_at": "2026-10-18T00:00:00Z",
                    "completed_at": "2026-10-18T00:01:00Z",
                }),
            )
        })
        .collect();
        let common = json!({
            "environment": manifest["environment"], "scope_id": manifest["scope_id"],
            "code_sha": manifest["code_sha"], "config_sha": manifest["config_sha"],
            "credential_version": manifest["credential_version"], "plan_hash": hash,
            "phases": phases, "resources": {"fixture": {"status": "complete"}},
        });
        let mut report = common.clone();
        report.as_object_mut().unwrap().extend(
            json!({
                "report_version": 2, "status": "completed", "failed_phase": null,
                "completed_at": "2026-10-18T00:01:00Z", "databases": ["127.0.0.1:3306/fixture"],
                "redis_namespace": scope, "object_prefixes": ["fixture:reset-proof/"],
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let mut ledger = common;
        ledger["ledger_version"] = json!(4);
        ledger["updated_at"] = json!("2026-10-18T00:01:00Z");
        let mut previous = ledger.clone();
        previous["updated_at"] = json!("2026-10-18T00:00:59Z");
        previous["phases"]["release"]["status"] = json!("running");
        previous["phases"]["release"]["completed_at"] = Value::Null;
        let fixture = Self {
            backend,
            _directory: directory,
            target,
            manifest,
            hash,
        };
        for (kind, value) in [
            ("report", report),
            ("ledger", ledger),
            ("previous", previous),
        ] {
            json_io::write_new(&fixture.path(kind), &value).unwrap();
        }
        fixture
    }

    fn path(&self, kind: &str) -> PathBuf {
        let stem = format!("test-reset-proof-{}", self.hash);
        let name = match kind {
            "report" => format!("{stem}.report.json"),
            "ledger" => format!("{stem}.ledger.json"),
            "previous" => format!(".{stem}.ledger.json.previous"),
            _ => panic!("未知测试证据类型"),
        };
        self.target.join("reset-state").join(name)
    }

    fn verify(&self) -> crate::Result<Value> {
        reset::completed(&self.backend, &self.target, &self.manifest, &self.hash)
    }

    fn mutate(&self, kind: &str, change: impl FnOnce(&mut Value)) {
        let path = self.path(kind);
        let mut value = json_io::read(&path).unwrap();
        change(&mut value);
        fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
}

#[test]
fn completed_reset_binds_exact_files_without_changing_evidence() {
    let fixture = Fixture::new();
    let paths = [
        fixture.path("report"),
        fixture.path("ledger"),
        fixture.path("previous"),
    ];
    let before = paths.each_ref().map(|path| fs::read(path).unwrap());
    let actual = fixture.verify().unwrap();
    let expected = json!({
        "report": {"file": format!("reset-state{}test-reset-proof-{}.report.json", std::path::MAIN_SEPARATOR, fixture.hash),
            "bytes": before[0].len(), "sha256": crate::recovery::clone::native::digest::file(&paths[0]).unwrap().sha256},
        "ledger": {"file": format!("reset-state{}test-reset-proof-{}.ledger.json", std::path::MAIN_SEPARATOR, fixture.hash),
            "bytes": before[1].len(), "sha256": crate::recovery::clone::native::digest::file(&paths[1]).unwrap().sha256},
        "completed_at": "2026-10-18T00:01:00Z", "status": "completed"
    });
    assert_eq!(actual, expected);
    assert_eq!(paths.each_ref().map(|path| fs::read(path).unwrap()), before);
    assert_eq!(actual["status"], "completed");
}

#[test]
fn reset_refuses_wrong_phase_set_and_missing_completion_times() {
    for kind in ["report", "ledger"] {
        for case in 0..3 {
            let fixture = Fixture::new();
            fixture.mutate(kind, |value| match case {
                0 => {
                    value["phases"].as_object_mut().unwrap().remove("release");
                }
                1 => {
                    value["phases"]["surprise"] = value["phases"]["release"].clone();
                }
                _ => {
                    value["phases"]["verification"]["completed_at"] = Value::Null;
                }
            });
            assert!(fixture.verify().is_err(), "{kind}/{case}");
        }
    }
}

#[test]
fn reset_refuses_reused_failed_and_foreign_reports() {
    for case in 0..6 {
        let fixture = Fixture::new();
        fixture.mutate("report", |value| match case {
            0 => value["status"] = json!("reused"),
            1 => value["failed_phase"] = json!("release"),
            2 => value["scope_id"] = json!("another-scope"),
            3 => value["resources"]["fixture"]["status"] = json!("running"),
            4 => value["report_version"] = json!(true),
            _ => value["plan_hash"] = json!("d".repeat(64)),
        });
        assert!(fixture.verify().is_err(), "case {case}");
    }
}

#[test]
fn reset_requires_exact_direct_release_preimage() {
    for case in 0..6 {
        let fixture = Fixture::new();
        fixture.mutate("previous", |value| match case {
            0 => value["phases"]["release"]["status"] = json!("complete"),
            1 => value["phases"]["release"]["completed_at"] = json!("2026-10-18T00:01:00Z"),
            2 => value["phases"]["verification"]["started_at"] = json!("changed"),
            3 => value["updated_at"] = json!("2099-10-18T00:01:00Z"),
            4 => value["resources"] = json!({}),
            _ => {
                value.as_object_mut().unwrap().remove("updated_at");
            }
        });
        assert!(fixture.verify().is_err(), "case {case}");
    }
}

#[test]
fn reset_requires_exact_reported_resources_and_previous_file() {
    for field in ["databases", "redis_namespace", "object_prefixes"] {
        let fixture = Fixture::new();
        fixture.mutate("report", |value| value[field] = json!([]));
        assert!(fixture.verify().is_err(), "{field}");
    }
    let fixture = Fixture::new();
    fs::remove_file(fixture.path("previous")).unwrap();
    assert!(fixture.verify().is_err());
}

#[test]
fn reset_refuses_duplicate_keys_and_invalid_plan_path_components() {
    let mut fixture = Fixture::new();
    for scope in ["../outside", "nested/child", "drive:relative", ""] {
        fixture.manifest["scope_id"] = json!(scope);
        assert!(fixture.verify().is_err(), "{scope}");
    }
    fixture.manifest["scope_id"] = json!("reset-proof");
    fs::write(fixture.path("previous"), br#"{"phases":{},"phases":{}}"#).unwrap();
    assert!(fixture.verify().is_err());
}

#[test]
fn native_reset_protocol_reuses_exact_completed_evidence() {
    let fixture = Fixture::new();
    let payload = json!({"format_version":1,"operation":"reset-completed",
        "backend":fixture.backend,"target":fixture.target,
        "manifest":fixture.manifest,"sha":fixture.hash});
    let result = evidence_protocol::execute(&serde_json::to_vec(&payload).unwrap()).unwrap();
    assert_eq!(result, fixture.verify().unwrap());
    fixture.mutate("previous", |value| {
        value["phases"]["release"]["status"] = json!("complete")
    });
    assert!(evidence_protocol::execute(&serde_json::to_vec(&payload).unwrap()).is_err());
}

#[test]
fn reset_plan_binds_full_command_original_text_and_digest() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let document = "{\n  \"scope_id\": \"审阅目标\"\n}";
    let hash = hex::encode(Sha256::digest(document.as_bytes()));
    let plan = json!({"manifest":{"scope_id":"审阅目标"},"plan_hash":hash});
    let path = fixture.target.join("plan.command.json");
    let valid = json!({"command":["reset.exe","plan"],"returncode":0,
        "error_type":null,"stdout":format!("{document}\nplan_hash={hash}\n"),"stderr":""});
    for case in 0..10 {
        let mut value = valid.clone();
        match case {
            0 => {}
            1 => value["returncode"] = json!(true),
            2 => value["returncode"] = json!(1),
            3 => value["error_type"] = json!("Interrupted"),
            4 => value["stderr"] = json!("warning"),
            5 => value["stdout"] = json!(format!("{document}\nplan_hash={hash}")),
            6 => value["stdout"] = json!(format!("{{\"scope_id\":\"other\"}}\nplan_hash={hash}\n")),
            7 => value["command"] = json!(["other.exe", "plan"]),
            8 => value["extra"] = json!(true),
            _ => {
                value["stdout"] = json!(format!(
                    "{{\"scope_id\":1,\"scope_id\":2}}\nplan_hash={hash}\n"
                ))
            }
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let observed =
            reset::plan_receipt(&fixture.backend, &fixture.target, &path, "reset.exe", &plan);
        assert_eq!(observed.is_ok(), case == 0, "case {case}");
    }
    let outside = fixture
        .target
        .parent()
        .unwrap()
        .join("foreign.command.json");
    fs::write(&outside, serde_json::to_vec(&valid).unwrap()).unwrap();
    assert!(
        reset::plan_receipt(
            &fixture.backend,
            &fixture.target,
            &outside,
            "reset.exe",
            &plan
        )
        .is_err()
    );
}
