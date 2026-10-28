use super::*;
use crate::recovery::{
    clone::native::{
        lifecycle::{
            lock::{ProcessGuard, RunLock},
            state,
        },
        resources::common,
        source,
    },
    source::evidence,
};
use serde_json::json;

struct Fixture {
    _guard: ProcessGuard,
    lock: RunLock,
    directory: tempfile::TempDir,
    backend: PathBuf,
    number: u64,
    successor: Value,
    published: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let directory = tempfile::Builder::new()
            .prefix("source-rebind-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        let run = directory.path();
        document::write_new(
            &run.join("manifest.json"),
            &json!({"copy_stage":"source_to_seed"}),
        )
        .unwrap();
        document::write_new(
            &run.join("successor.json"),
            &json!({"relationship":"frozen"}),
        )
        .unwrap();
        let successor = common::binding(&run.join("successor.json")).unwrap();
        state::initialize(run).unwrap();
        let (number, _) = state::begin(run, "seed-runtime", "source-register", &json!({})).unwrap();
        state::finish(run, number, Some(&json!({"published":true})), None).unwrap();
        let published = common::binding(&run.join("results/0001.json")).unwrap();
        let (number, _) = state::begin(run, "storage-target", "restart", &json!({})).unwrap();
        state::finish(run, number, Some(&json!({"storage":"restarted"})), None).unwrap();
        let guard = ProcessGuard::acquire(run).unwrap();
        let lock = RunLock::claim(run).unwrap();
        let (number, attempt) = state::begin(
            run,
            "seed-runtime",
            "source-rebind",
            &source::current(&backend).unwrap(),
        )
        .unwrap();
        state::bind_controller(run, number, &lock.owner, &attempt).unwrap();
        Self {
            _guard: guard,
            lock,
            directory,
            backend,
            number,
            successor,
            published,
        }
    }

    fn request(&self) -> StageRequest<'_> {
        StageRequest {
            backend: &self.backend,
            run_dir: self.directory.path(),
            stage: "seed-runtime",
            mode: "source-rebind",
            attempt: Some(self.number),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.lock.release().unwrap();
    }
}

#[test]
fn owned_rebind_history_consumes_current_proof_without_publishing_or_rewriting() {
    let fixture = Fixture::new();
    let run = fixture.directory.path();
    let before = std::fs::read(run.join("state.json")).unwrap();
    let owner = std::fs::read(run.join("run.lock/owner.json")).unwrap();
    let controller = Controller::load(&fixture.request(), &fixture.successor).unwrap();
    let observed = evidence::source_rebind_history(&controller, &fixture.published).unwrap();
    assert_eq!(observed["later"].as_array().unwrap().len(), 2);
    assert_eq!(observed["later"][1]["number"], fixture.number);
    assert_eq!(std::fs::read(run.join("state.json")).unwrap(), before);
    assert_eq!(
        std::fs::read(run.join("run.lock/owner.json")).unwrap(),
        owner
    );
    assert!(
        !run.join(format!("results/{:04}.json", fixture.number))
            .exists()
    );
    assert!(!run.join(format!("g{:04}", fixture.number)).exists());
}

#[test]
fn rebind_context_rejects_wrong_attempt_phase_controller_and_changed_request() {
    let fixture = Fixture::new();
    for number in [0, fixture.number - 1, fixture.number + 1] {
        let mut request = fixture.request();
        request.attempt = Some(number);
        assert!(Controller::load(&request, &fixture.successor).is_err());
    }
    let mut request = fixture.request();
    request.mode = "source-export";
    assert!(Controller::load(&request, &fixture.successor).is_err());
    let owner_path = fixture.directory.path().join("run.lock/owner.json");
    let owner_bytes = std::fs::read(&owner_path).unwrap();
    let mut owner = document::read(&owner_path).unwrap();
    owner["identity"]["started"] = json!("unregistered");
    std::fs::write(&owner_path, serde_json::to_vec(&owner).unwrap()).unwrap();
    assert!(Controller::load(&fixture.request(), &fixture.successor).is_err());
    std::fs::write(owner_path, owner_bytes).unwrap();
    let path = state::controller_path(fixture.directory.path(), fixture.number);
    let original = std::fs::read(&path).unwrap();
    let mut value = document::read(&path).unwrap();
    value["owner"]["identity"]["started"] = json!("unregistered");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(Controller::load(&fixture.request(), &fixture.successor).is_err());
    std::fs::write(path, original).unwrap();
    let controller = Controller::load(&fixture.request(), &fixture.successor).unwrap();
    let result = controller.checked(|| {
        std::fs::write(
            fixture.directory.path().join("successor.json"),
            b"{\"changed\":true}",
        )?;
        Ok(json!({"would_be_success":true}))
    });
    assert!(result.is_err());
    assert!(
        !fixture
            .directory
            .path()
            .join(format!("results/{:04}.json", fixture.number))
            .exists()
    );
}

#[test]
fn history_checks_full_prefix_and_rejects_ledger_drift_after_observation() {
    let fixture = Fixture::new();
    let controller = Controller::load(&fixture.request(), &fixture.successor).unwrap();
    let result = controller.checked(|| {
        let mut saved = state::load(fixture.directory.path(), true)?;
        saved["attempts"][0]["sources"] = json!({"changed":true});
        std::fs::write(
            fixture.directory.path().join("state.json"),
            serde_json::to_vec(&saved)?,
        )?;
        Ok(())
    });
    assert!(result.is_err());
    assert!(evidence::source_rebind_history(&controller, &fixture.published).is_err());
    assert!(
        !fixture
            .directory
            .path()
            .join(format!("results/{:04}.json", fixture.number))
            .exists()
    );
}

#[test]
fn owned_history_cannot_skip_an_unproven_cache_restart() {
    let fixture = Fixture::new();
    let path = fixture.directory.path().join("state.json");
    let mut saved = state::load(fixture.directory.path(), true).unwrap();
    // 保留真实已发布结果绑定，但把历史阶段改为缓存；缺少缓存创建证明必须拒绝。
    saved["attempts"][1]["stage"] = json!("cache-target");
    std::fs::write(path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let source_bytes = std::fs::read(fixture.directory.path().join("results/0001.json")).unwrap();
    let request_bytes = std::fs::read(fixture.directory.path().join("successor.json")).unwrap();
    let controller = Controller::load(&fixture.request(), &fixture.successor).unwrap();
    let error = evidence::source_rebind_history(&controller, &fixture.published).unwrap_err();
    let category = stage::failure_type(error.as_ref());
    assert_eq!(category, "SourceRebindHistoryRejected");
    let (_, guidance) = diagnostic(error.as_ref()).unwrap();
    assert!(guidance.contains("完整历史"));
    assert!(guidance.contains("禁止自动重放"));
    state::record_failure(
        fixture.directory.path(),
        fixture.number,
        "seed-runtime",
        "source-rebind",
        &category,
    )
    .unwrap();
    state::finish(
        fixture.directory.path(),
        fixture.number,
        None,
        Some(&category),
    )
    .unwrap();
    let failure = document::read(
        &fixture
            .directory
            .path()
            .join(format!("failure-{:04}.json", fixture.number)),
    )
    .unwrap();
    assert_eq!(failure["error_type"], category);
    assert_eq!(failure["operation_id"], "clone.seed-runtime.source-rebind");
    let finished = state::load(fixture.directory.path(), true).unwrap();
    assert_eq!(finished["attempts"][2]["status"], "failed");
    assert_eq!(finished["attempts"][2]["error_type"], category);
    assert_eq!(
        std::fs::read(fixture.directory.path().join("results/0001.json")).unwrap(),
        source_bytes
    );
    assert_eq!(
        std::fs::read(fixture.directory.path().join("successor.json")).unwrap(),
        request_bytes
    );
    assert!(
        !fixture
            .directory
            .path()
            .join(format!("results/{:04}.json", fixture.number))
            .exists()
    );
}

#[test]
fn rebind_postcondition_failure_keeps_primary_and_never_formats_private_errors() {
    let error = finish::<Value>(
        Err("private-primary".into()),
        Err("private-secondary".into()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("后置核验同时失败"));
    assert!(!error.to_string().contains("private-"));
    assert_eq!(error.source().unwrap().to_string(), "private-primary");
    assert_eq!(
        stage::failure_type(error.as_ref()),
        "SourceRebindObservationChanged"
    );
    assert!(!diagnostic(error.as_ref()).unwrap().1.contains("private-"));
}

#[test]
fn known_cim_failure_keeps_its_error_code_and_repair_guidance() {
    let cause = super::super::source_registration::quiet::CimUnavailable {
        code: Some(0x80041003_u32 as i32),
    };
    let error = classify::<Value>(Phase::Quiet, Err(Box::new(cause))).unwrap_err();
    let (category, message) = diagnostic(error.as_ref()).unwrap();
    assert_eq!(category, "SourceRebindQuietRejected");
    assert!(message.contains("0x80041003"));
    assert!(message.contains("ROOT\\CIMV2"));
    assert!(message.contains("Winmgmt"));
    assert!(message.contains("未证明历史进程已停止"));
    assert!(message.contains("禁止自动重放"));
}
