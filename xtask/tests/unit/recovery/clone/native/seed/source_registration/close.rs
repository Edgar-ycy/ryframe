use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::latest;
use crate::recovery::clone::native::{json as document, resources::common};

struct Fixture {
    _directory: TempDir,
    backend: PathBuf,
    run: PathBuf,
    runtime: PathBuf,
    handoff: Value,
    descriptor: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("source-close-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        let run = directory.path().join("run");
        let runtime = run.join("seed-runtime/runtime");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir(run.join("results")).unwrap();
        let handoff = json!({"scope_id":"seed-fixture"});
        document::write_new(&runtime.join("handoff.json"), &handoff).unwrap();
        let value = closed_result(&runtime);
        let path = run.join("results/0001.json");
        document::write_new(&path, &value).unwrap();
        let descriptor = common::binding(&path).unwrap();
        Self {
            _directory: directory,
            backend,
            run,
            runtime,
            handoff,
            descriptor,
        }
    }

    fn state(&self) -> Value {
        json!({"attempts":[{"number":1,"stage":"seed-runtime","mode":"close",
            "status":"passed","result":self.descriptor}]})
    }

    fn latest(&self, state: &Value, current: u64) -> crate::Result<Value> {
        latest(
            &self.backend,
            &self.run,
            state,
            &self.runtime,
            &self.handoff,
            current,
        )
    }
}

// 沿用 guards 的完整成功结果形状；不复制排空或关闭验证算法。
fn snapshot() -> Value {
    let active = "background_job_status background_job_lease background_job_completion \
        background_job_attempt outbox_event_status outbox_event_lease export_job_status \
        export_job_request export_job_delete user_import_job enabled_schedule schedule_execution \
        config_bundle config_transfer retention_run password_reset tenant_operation_lease \
        tenant_data_migration tenant_data_migration_item file_upload tenant_status";
    let terminal = "background_job_dead background_job_attempt_failed background_job_attempt_dead \
        background_job_attempt_deferred background_job_attempt_lease_expired outbox_event_dead \
        export_job_failed export_job_cancelled export_job_expired user_import_partial user_import_failed \
        user_import_cancelled schedule_skipped_misfire schedule_skipped_concurrency schedule_target_unavailable \
        schedule_invalid_configuration config_bundle_failed config_bundle_expired config_transfer_failed \
        retention_run_failed password_reset_expired";
    let counts = |names: &str| {
        names
            .split_whitespace()
            .map(|name| (name.to_owned(), json!(0)))
            .collect::<serde_json::Map<_, _>>()
    };
    json!({"database_time":"2026-09-05T00:00:00.000000Z",
        "active":counts(active),"terminal_non_success":counts(terminal)})
}

fn control(runtime: &Path, roles: &[&str], operation: &str) -> Value {
    json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":operation,
        "scope_id":"seed-fixture","runtime_directory":runtime,
        "producer_history":runtime.join("producer-history.json"),
        "processes":roles.iter().map(|role| (role.to_string(),
            json!({"state":"stopped","identity":null,"ready":false})))
            .collect::<serde_json::Map<_,_>>()})
}

fn closed_result(runtime: &Path) -> Value {
    let image = snapshot();
    let identities = json!({"api":{"pid":101,"started":"123","executable":"api.exe"},
        "worker":{"pid":102,"started":"124","executable":"worker.exe"}});
    json!({"format_version":1,"kind":"devex-clone-seed-close","status":"seed_runtime_closed",
        "phase":"verified","handoff":common::binding(&runtime.join("handoff.json")).unwrap(),
        "runtime_directory":runtime,"scope_id":"seed-fixture","process_identities":identities,
        "runtime_status":control(runtime,&["api","worker"],"status"),
        "api_stop":control(runtime,&["api"],"stop"),
        "drain":{"stable_seconds":2,"observations":3,"first_snapshot":image,"last_snapshot":image},
        "worker_stop":control(runtime,&["worker"],"stop"),
        "final_snapshot":image,"last_snapshot":null,
        "terminal_non_success":image["terminal_non_success"],
        "stopped_evidence":{"original":identities},"worker_preserved":false,
        "reason":null,"error_type":null,"outbox_drained":true,"direct_remote_writes":0,
        "worker_drain_writes":true,"restore_qualified":false})
}

#[test]
fn latest_failed_close_cannot_fall_back_and_current_close_is_excluded() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state["attempts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"number":2,
        "stage":"seed-runtime","mode":"close","status":"failed","result":null}));
    assert!(
        fixture
            .latest(&state, 3)
            .unwrap_err()
            .to_string()
            .contains("不能回退历史成功")
    );
    assert_eq!(fixture.latest(&state, 2).unwrap(), fixture.descriptor);
    assert_eq!(
        common::binding(&fixture.run.join("results/0001.json")).unwrap(),
        fixture.descriptor
    );
}

#[test]
fn any_unapproved_stage_after_close_rejects_regardless_of_status() {
    let fixture = Fixture::new();
    for status in ["passed", "failed", "running"] {
        for (stage, mode) in [
            ("post-copy", "verify"),
            ("seed-runtime", "identities-apply"),
        ] {
            let mut state = fixture.state();
            state["attempts"]
                .as_array_mut()
                .unwrap()
                .push(json!({"number":2,
                "stage":stage,"mode":mode,"status":status,"result":null}));
            assert!(
                fixture
                    .latest(&state, 3)
                    .unwrap_err()
                    .to_string()
                    .contains("未重新排空")
            );
        }
    }
    assert_eq!(
        common::binding(&fixture.run.join("results/0001.json")).unwrap(),
        fixture.descriptor
    );
}

#[test]
fn all_twenty_one_permitted_failed_records_leave_the_passed_close_usable() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    let allowed = [
        ("seed-runtime", "source-register"),
        ("seed-runtime", "source-rebind"),
        ("seed-runtime", "source-generation-start"),
        ("seed-runtime", "source-generation-stop"),
        ("seed-runtime", "source-generation-recover"),
        ("seed-runtime", "source-export"),
        ("seed-runtime", "source-export-reconcile"),
        ("seed-runtime", "arm-input"),
        ("seed-runtime", "stop"),
        ("seed-runtime", "recover"),
        ("seed-runtime", "recover-session"),
        ("storage-target", "restart"),
        ("storage-target", "status"),
        ("storage-target", "stop"),
        ("storage-target", "recover"),
        ("cache-target", "restart"),
        ("cache-target", "status"),
        ("cache-target", "stop"),
        ("cache-target", "recover"),
        ("cache-target", "reconcile"),
        ("cache-target", "resume"),
    ];
    for (index, (stage, mode)) in allowed.iter().enumerate() {
        state["attempts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"number":index+2,
            "stage":stage,"mode":mode,"status":"failed","result":null}));
    }
    let current = allowed.len() as u64 + 2;
    assert_eq!(fixture.latest(&state, current).unwrap(), fixture.descriptor);
}

#[test]
fn close_result_must_belong_to_exact_run_attempt_and_actual_handoff() {
    let fixture = Fixture::new();
    let original = fixture.run.join("results/0001.json");
    let value = document::read(&original).unwrap();
    for wrong in [
        fixture.run.join("results/0002.json"),
        fixture._directory.path().join("outside.json"),
    ] {
        document::write_new(&wrong, &value).unwrap();
        let mut state = fixture.state();
        state["attempts"][0]["result"] = common::binding(&wrong).unwrap();
        assert!(
            fixture
                .latest(&state, 2)
                .unwrap_err()
                .to_string()
                .contains("不属于固定 run attempt")
        );
    }
    let other = json!({"scope_id":"another-handoff"});
    document::write_replace(&fixture.runtime.join("handoff.json"), &other).unwrap();
    assert!(
        fixture
            .latest(&fixture.state(), 2)
            .unwrap_err()
            .to_string()
            .contains("交接内容")
    );
    assert_eq!(common::binding(&original).unwrap(), fixture.descriptor);
    document::write_replace(&fixture.runtime.join("handoff.json"), &fixture.handoff).unwrap();
    let mut changed = fs::read(&original).unwrap();
    changed.push(b'\n');
    fs::write(&original, &changed).unwrap();
    assert!(
        fixture
            .latest(&fixture.state(), 2)
            .unwrap_err()
            .to_string()
            .contains("登记摘要不一致")
    );
    assert_eq!(fs::read(&original).unwrap(), changed);
}
