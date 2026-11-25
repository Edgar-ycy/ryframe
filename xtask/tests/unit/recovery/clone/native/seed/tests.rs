use super::*;
use serde_json::json;
use std::fs;

struct Fixture {
    backend: PathBuf,
    run: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let run = backend
            .join(".local-tests")
            .join(format!("seed-native-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&run).unwrap();
        json::write_new(&run.join("manifest.json"), &json!({"fixture":true})).unwrap();
        state::initialize(&run).unwrap();
        let fixture = Self { backend, run };
        let registration = json!({
            "format_version":1,"kind":"devex-clone-seed-runtime",
            "run_manifest":common::binding(&fixture.run.join("manifest.json")).unwrap(),
            "post_copy":{},"post_verify":{},"identity_plan":{},"identity_state":"fixture",
            "identity_tool":{},"api_process":{},"node":{}
        });
        json::write_new(&fixture.run.join("seed-runtime.json"), &registration).unwrap();
        fixture.register();
        fixture
    }

    fn register(&self) {
        let (number, _) = state::begin(&self.run, "seed-runtime", "register", &json!({})).unwrap();
        state::finish(
            &self.run,
            number,
            Some(&json!({
                "status":"seed_runtime_registered",
                "registration":common::binding(&self.run.join("seed-runtime.json")).unwrap()
            })),
            None,
        )
        .unwrap();
    }

    fn load(&self, cleanup: bool) -> Result<Registration> {
        Registration::load(&self.backend, &self.run, cleanup)
    }

    fn update(&self, name: &str, change: impl FnOnce(&mut Value)) {
        let path = self.run.join(name);
        let mut value = json::read(&path).unwrap();
        change(&mut value);
        json::write_replace(&path, &value).unwrap();
    }

    fn rebind_registration(&self) {
        let registration = common::binding(&self.run.join("seed-runtime.json")).unwrap();
        self.update("results/0001.json", |result| {
            result["registration"] = registration
        });
        let result = common::binding(&self.run.join("results/0001.json")).unwrap();
        self.update("state.json", |state| {
            state["attempts"][0]["result"] = result
        });
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}

#[test]
fn registered_seed_matches_published_run_and_does_not_write_runtime_or_ledger() {
    let fixture = Fixture::new();
    let before = fs::read(fixture.run.join("state.json")).unwrap();
    for cleanup in [false, true] {
        let proof = fixture.load(cleanup).unwrap();
        assert_eq!(proof.value["kind"], "devex-clone-seed-runtime");
        proof.assert_unchanged(&fixture.backend, cleanup).unwrap();
    }
    assert_eq!(before, fs::read(fixture.run.join("state.json")).unwrap());
    assert!(!fixture.run.join("seed-runtime").exists());
    assert!(!fixture.run.join("run.lock").exists());
}

#[test]
fn seed_register_rejects_missing_input_without_stage_writes() {
    use crate::cli::{CloneCommand, SeedRuntimeOperation, SeedRuntimeOptions};
    let fixture = Fixture::new();
    let before = fs::read(fixture.run.join("state.json")).unwrap();
    let options = SeedRuntimeOptions {
        run_dir: fixture.run.clone(),
        operation: SeedRuntimeOperation::Register,
        hold_originals: false,
        request: None,
        producer_binding: None,
    };
    let command = CloneCommand::SeedRuntime(options);
    assert!(
        crate::recovery::clone::native::run(&command, &fixture.backend)
            .unwrap_err()
            .to_string()
            .contains("seed 登记需要明确 --request")
    );
    assert_eq!(before, fs::read(fixture.run.join("state.json")).unwrap());
    assert!(!fixture.run.join("run.lock").exists());
}

#[test]
fn seed_mutations_and_recovery_reject_invalid_inputs_without_writes() {
    use crate::cli::{CloneCommand, SeedRuntimeOperation, SeedRuntimeOptions};
    let fixture = Fixture::new();
    let before = fs::read(fixture.run.join("state.json")).unwrap();
    for operation in [
        SeedRuntimeOperation::QuotasPlan,
        SeedRuntimeOperation::QuotasApply,
        SeedRuntimeOperation::QuotasReconcile,
        SeedRuntimeOperation::IdentitiesApply,
        SeedRuntimeOperation::IdentitiesVerify,
        SeedRuntimeOperation::DepartmentsPlan,
        SeedRuntimeOperation::DepartmentsApply,
        SeedRuntimeOperation::DepartmentsReconcile,
        SeedRuntimeOperation::DepartmentsVerify,
        SeedRuntimeOperation::Prepare,
        SeedRuntimeOperation::RecoverSession,
    ] {
        let command = CloneCommand::SeedRuntime(SeedRuntimeOptions {
            run_dir: fixture.run.clone(),
            operation,
            hold_originals: false,
            request: Some(fixture.run.join("manifest.json")),
            producer_binding: None,
        });
        let error = crate::recovery::clone::native::run(&command, &fixture.backend)
            .unwrap_err()
            .to_string();
        assert!(error.contains(match operation {
            SeedRuntimeOperation::RecoverSession => "回收需要明确生产者收据描述文件",
            _ => "seed 业务及运行准备沿用本 run 已发布登记和原身份计划",
        }));
    }
    assert_eq!(before, fs::read(fixture.run.join("state.json")).unwrap());
    assert!(!fixture.run.join("run.lock").exists());
}

#[test]
fn latest_failed_or_running_registration_blocks_normal_access_but_cleanup_uses_published() {
    for failed in [false, true] {
        let fixture = Fixture::new();
        let (number, _) =
            state::begin(&fixture.run, "seed-runtime", "register", &json!({})).unwrap();
        if failed {
            state::finish(&fixture.run, number, None, Some("ValueError")).unwrap();
        }
        assert!(fixture.load(false).is_err());
        assert!(fixture.load(true).is_ok());
    }
}

#[test]
fn register_result_must_bind_current_raw_registration_and_fixed_run() {
    for (key, value) in [
        ("status", json!("seed_runtime_prepared")),
        (
            "registration",
            json!({"path":"other","bytes":1,"sha256":"a".repeat(64)}),
        ),
    ] {
        let fixture = Fixture::new();
        fixture.update("results/0001.json", |result| result[key] = value);
        let result_binding = common::binding(&fixture.run.join("results/0001.json")).unwrap();
        fixture.update("state.json", |state| {
            state["attempts"][0]["result"] = result_binding
        });
        assert!(fixture.load(false).is_err());
        assert!(fixture.load(true).is_err());
    }
}

#[test]
fn request_schema_type_run_and_original_bytes_fail_closed() {
    for (key, value) in [
        ("format_version", json!(true)),
        ("format_version", json!("1")),
        ("kind", json!("unrelated")),
        ("run_manifest", json!({})),
        ("unknown", json!("Private-Seed-Secret")),
    ] {
        let fixture = Fixture::new();
        fixture.update("seed-runtime.json", |request| request[key] = value);
        // 即使重写外层摘要，schema 与 run 校验仍须阻断。
        fixture.rebind_registration();
        for cleanup in [false, true] {
            let message = fixture.load(cleanup).err().unwrap().to_string();
            assert!(!message.contains("Private-Seed-Secret"));
        }
    }
}

#[test]
fn missing_duplicate_or_partially_written_json_does_not_authorize_seed() {
    for raw in [
        "{",
        "{}",
        "{\"format_version\":1,\"format_version\":1}",
        "[]",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.run.join("seed-runtime.json"), raw).unwrap();
        fixture.rebind_registration();
        assert!(fixture.load(false).is_err());
        assert!(fixture.load(true).is_err());
    }
    let fixture = Fixture::new();
    fs::remove_file(fixture.run.join("seed-runtime.json")).unwrap();
    assert!(fixture.load(true).is_err());
}

#[test]
fn cleanup_skips_unrelated_damaged_results_but_never_its_own_result() {
    let fixture = Fixture::new();
    let (number, _) = state::begin(&fixture.run, "seed-runtime", "prepare", &json!({})).unwrap();
    state::finish(
        &fixture.run,
        number,
        Some(&json!({"private":"not logged"})),
        None,
    )
    .unwrap();
    fs::write(fixture.run.join("results/0002.json"), "changed").unwrap();
    assert!(fixture.load(false).is_err());
    assert!(fixture.load(true).is_ok());
    fs::write(fixture.run.join("results/0001.json"), "changed").unwrap();
    assert!(fixture.load(true).is_err());
}

#[test]
fn proof_rechecks_registration_state_manifest_and_published_result_after_authorization() {
    for name in [
        "seed-runtime.json",
        "state.json",
        "manifest.json",
        "results/0001.json",
    ] {
        let fixture = Fixture::new();
        let proof = fixture.load(false).unwrap();
        fs::write(fixture.run.join(name), "{\"secret\":\"do-not-log\"}").unwrap();
        let failure = proof
            .assert_unchanged(&fixture.backend, false)
            .unwrap_err()
            .to_string();
        assert!(!failure.contains("do-not-log"));
    }
}

#[test]
fn latest_excludes_only_requested_current_attempt_and_rejects_unknown_result_states() {
    let state = json!({"attempts":[
        {"number":1,"stage":"seed-runtime","mode":"prepare","status":"passed"},
        {"number":2,"stage":"seed-runtime","mode":"prepare","status":"failed"},
        {"number":3,"stage":"seed-runtime","mode":"prepare","status":"running"}
    ]});
    assert!(latest(&state, "seed-runtime", &["prepare"], None, false).is_err());
    assert!(latest(&state, "seed-runtime", &["prepare"], Some(3), false).is_err());
    assert_eq!(
        latest(&state, "seed-runtime", &["prepare"], None, true).unwrap()["number"],
        1
    );
    assert_eq!(
        latest(&state, "seed-runtime", &["prepare"], Some(3), true).unwrap()["number"],
        1
    );
    assert!(latest(&state, "seed-runtime", &["start"], None, true).is_err());
}
