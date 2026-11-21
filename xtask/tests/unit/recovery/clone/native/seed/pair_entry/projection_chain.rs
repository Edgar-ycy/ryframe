use super::*;
use crate::recovery::{
    clone::native::{
        cache::handoff::locks::seed_runtime_guard,
        lifecycle::lock::{self, ProcessGuard, RunLock},
        seed::runtime_model,
    },
    fixture_control::portable_path,
};
use controller::{STOP_ID_START, archive_stopped_fixture, stopped_runtime};
use std::{fs, net::TcpListener};

const STOP_ID: &str = "22222222222222222222222222222222";

struct Fixture<'a> {
    backend: &'a Path,
    run: &'a Path,
    runtime: PathBuf,
    handoff: Value,
    contract: Value,
}

impl<'a> Fixture<'a> {
    fn new(backend: &'a Path, run: &'a Path, port: u16) -> Result<(Self, Value, Value)> {
        document::write_new(&run.join("manifest.json"), &json!({}))?;
        state::initialize(run)?;
        let runtime = run.join("seed-runtime").join("runtime");
        fs::create_dir_all(&runtime)?;
        let (environment, product, executor) = stopped_runtime(&runtime, port)?;
        let contract = document::read(&runtime.join("runtime.json"))?;
        let handoff = json!({"runtime":common::binding(&runtime.join("runtime.json"))?,
            "api_environment":environment,"scope_id":contract["scope_id"]});
        document::write_new(&runtime.join("handoff.json"), &handoff)?;
        let fixture = Self {
            backend,
            run,
            runtime,
            handoff,
            contract,
        };
        Ok((fixture, product, executor))
    }

    fn original_close(&self, executor: &Value, identity: &Value) -> Result<u64> {
        let (number, running) = state::begin(self.run, "seed-runtime", "close", &json!({}))?;
        let owner = json!({"format_version":1,"directory":self.run,"identity":executor,
            "manifest_sha256":state::manifest_binding(self.run)?["sha256"]});
        let lock = self.run.join("run.lock");
        fs::create_dir(&lock)?;
        document::write_new(&lock.join("owner.json"), &owner)?;
        state::bind_controller(self.run, number, &owner, &running)?;
        let (controller, _) = state::controller_record(self.run, number, &running)?;
        fs::create_dir(directory(self.run, number))?;
        let claim = claim_path(self.run, number, "stop", &[Role::Worker]);
        document::write_new(
            &claim,
            &json!({"format_version":1,"kind":"devex-seed-role-control-intent",
            "attempt":number,"controller":controller,"executor":executor,
            "handoff":common::binding(&self.runtime.join("handoff.json"))?,
            "operation":"stop","roles":["worker"]}),
        )?;
        document::write_new(
            &path(self.run, number, Role::Worker, StopKind::Explicit),
            &json!({"format_version":1,"kind":"devex-seed-role-stop-reservation",
                "intent":common::binding(&claim)?,"role":"worker","operation_id":STOP_ID,
                "start_id":STOP_ID_START,"identity":identity}),
        )?;
        state::interrupt_owned(self.run, &owner, Some(number), |_| Ok(()))?;
        lock::recover_dead(self.run, &owner, None)?;
        Ok(number)
    }

    fn recover_frame(&self) -> Result<(RunLock, Frame<'_>)> {
        let lock = RunLock::claim(self.run)?;
        let (number, running) = state::begin(self.run, "seed-runtime", "recover", &json!({}))?;
        state::bind_controller(self.run, number, &lock.owner, &running)?;
        let (controller, owner) = state::controller_record(self.run, number, &running)?;
        let handoff_path = self.runtime.join("handoff.json");
        let guard = seed_runtime_guard(self.backend, self.run)?;
        let protected = Inputs::open([
            self.run.join("manifest.json"),
            self.run.join("state.json"),
            self.run.join("run.lock/owner.json"),
            state::controller_path(self.run, number),
            handoff_path.clone(),
            self.runtime.join("runtime.json"),
            self.runtime.join("environment.json"),
        ])?;
        let frame = Frame {
            backend: self.backend,
            run: self.run,
            number,
            saved: state::load(self.run, false)?,
            owner,
            owner_pid: std::process::id(),
            executor: lock.owner["identity"].clone(),
            controller,
            runtime: runtime_model::Inputs {
                runtime: self.runtime.clone(),
                handoff: self.handoff.clone(),
                environment: json!({"APP_ENV":"test"}),
            },
            handoff: common::binding(&handoff_path)?,
            product: self.runtime.clone(),
            contract: self.contract.clone(),
            guard,
            protected,
        };
        frame.check()?;
        Ok((lock, frame))
    }

    fn start_history(&self, identity: &Value) -> Result<()> {
        let ready = document::text(&self.contract["worker_ready_url"], "测试 Worker 地址")?;
        let producer = Producer::new(
            self.backend,
            &self.runtime,
            &self.contract,
            Role::Worker,
            STOP_ID_START,
            ready,
        )?;
        let receipt = self.runtime.join("worker.json");
        let staged = self.runtime.join("worker-receipt-fixture.json");
        fs::rename(&receipt, &staged)?;
        producer.intent()?;
        fs::write(
            self.runtime.join(format!("worker-{STOP_ID_START}.log")),
            b"fixture",
        )?;
        fs::rename(staged, receipt)?;
        producer.started(identity)
    }
}

fn archive_bytes(path: &Path) -> Result<Vec<Vec<u8>>> {
    ["owner.json", "request.json", "result.json"]
        .into_iter()
        .map(|name| Ok(fs::read(path.join(name))?))
        .collect()
}

#[test]
fn interrupted_close_stop_projection_reuses_original_archive_without_rewriting_history()
-> Result<()> {
    let backend = crate::workspace::root_dir();
    let root = backend.join(".local-tests");
    fs::create_dir_all(&root)?;
    let temp = tempfile::tempdir_in(root)?;
    let run = portable_path(&temp.path().canonicalize()?);
    let _guard = ProcessGuard::acquire(&run)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let (fixture, identity, executor) =
        Fixture::new(&backend, &run, listener.local_addr()?.port())?;
    fixture.start_history(&identity)?;
    let close = fixture.original_close(&executor, &identity)?;
    let archived = archive_stopped_fixture(&fixture.runtime, &executor, STOP_ID)?;
    let archive = fixture.runtime.join(Role::Worker.archive()).join(STOP_ID);
    let original_archive = archive_bytes(&archive)?;
    let history = fixture.runtime.join("producer-history.json");
    let (first_lock, first) = fixture.recover_frame()?;
    let marker = directory(&run, first.number).join("close-stop-projection-worker.json");
    drop(listener);

    let (result, evidence) = resolve(&first, Role::Worker)?.expect("原停止归档应完成投影");
    let evidence = evidence.expect("原 Close 需要完整归档证据");
    assert_eq!(result, archived);
    assert_eq!(evidence["close_attempt"], close);
    let projected_history = fs::read(&history)?;
    let events = document::read(&history)?;
    assert_eq!(events["events"].as_array().map(Vec::len), Some(3));
    assert_eq!(events["events"][2]["event"], "stopped");
    assert_eq!(events["events"][2]["identity"], identity);
    assert_eq!(document::read(&marker)?["evidence"], evidence);
    let marker_bytes = fs::read(&marker)?;
    let interrupted = first.number;
    let orphan = state::result_path(&run, interrupted);
    drop(first);
    first_lock.release()?;
    let stage_result = json!({"status":"seed_runtime_recover","restore_qualified":false,
        "runtime":{"format_version":1,"kind":"devex-clone-runtime-lock-reconciliation",
            "remote_writes_reconciled":false,"roles":{"worker":result},
            "close_stop_evidence":{"worker":evidence}}});
    document::write_new(&orphan, &stage_result)?;
    let orphan_bytes = fs::read(&orphan)?;
    let saved = state::interrupt_owned(&run, &first_lock.owner, Some(interrupted), |_| Ok(()))?;
    let interrupted_row = &state::attempts(&saved)?[1];
    assert_eq!(interrupted_row["number"], interrupted);
    assert!(recover_interrupted(interrupted_row));

    let (second_lock, second) = fixture.recover_frame()?;
    assert_eq!(
        resolve(&second, Role::Worker)?,
        Some((result, Some(evidence)))
    );
    assert_eq!(fs::read(&history)?, projected_history);
    assert_eq!(archive_bytes(&archive)?, original_archive);
    assert_eq!(fs::read(&marker)?, marker_bytes);
    assert_eq!(fs::read(&orphan)?, orphan_bytes);
    assert!(!directory(&run, second.number).exists());
    drop(second);
    second_lock.release()
}
