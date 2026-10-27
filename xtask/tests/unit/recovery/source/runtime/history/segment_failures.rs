use super::*;
use crate::recovery::clone::native::{digest, lifecycle::state};

struct Fixture {
    directory: tempfile::TempDir,
    backend: std::path::PathBuf,
    row: Value,
}

impl Fixture {
    fn new() -> Self {
        Self::registered(
            crate::workspace::root_dir(),
            json!({"snapshot":{"head":"a".repeat(40)},"contract":"original-producer"}),
            "CalledProcessError",
        )
    }

    fn registered(backend: std::path::PathBuf, sources: Value, error: &str) -> Self {
        let local = backend.join(".local-tests");
        std::fs::create_dir_all(&local).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("source-historical-controller-")
            .tempdir_in(local)
            .unwrap();
        let run = directory.path();
        document::write_new(&run.join("manifest.json"), &json!({"fixture":true})).unwrap();
        state::initialize(run).unwrap();
        let (number, running) = state::begin(run, "storage-target", "restart", &sources).unwrap();
        let owner = json!({"format_version":1,"directory":run,
            "manifest_sha256":common::binding(&run.join("manifest.json")).unwrap()["sha256"],
            "identity":{"pid":424242,"started":"123456789",
                "executable":backend.join("fixture-controller.exe")}});
        std::fs::create_dir(run.join("run.lock")).unwrap();
        document::write_new(&run.join("run.lock/owner.json"), &owner).unwrap();
        state::bind_controller(run, number, &owner, &running).unwrap();
        state::finish(run, number, None, Some(error)).unwrap();
        let row = state::load(run, true).unwrap()["attempts"][0].clone();
        let controller = common::binding(&state::controller_path(run, number)).unwrap();
        document::write_new(
            &run.join("failure-0001.json"),
            &json!({"format_version":2,"kind":"devex-stage-failure","attempt":number,
                "stage":"storage-target","mode":"restart","error_type":error,
                "controller":controller,"operation_id":"clone.storage-target.restart"}),
        )
        .unwrap();
        Self {
            directory,
            backend,
            row,
        }
    }

    fn incident(&self) -> Result<(Value, Value)> {
        incident(
            &self.backend,
            self.directory.path(),
            &self.row,
            "restart",
            "clone.storage-target.restart",
            &[],
        )
    }
}

#[test]
fn finished_failure_uses_the_registered_running_controller_digest() {
    let fixture = Fixture::new();
    let run = fixture.directory.path();
    let controller = document::read(&state::controller_path(run, 1)).unwrap();
    assert_eq!(fixture.row["status"], "failed");
    assert!(!fixture.row["finished_at"].is_null());
    assert_ne!(
        controller["attempt_sha256"],
        digest::plan_hash(&fixture.row).unwrap()
    );
    let observed = fixture.incident().unwrap();
    assert_eq!(
        observed.0,
        common::binding(&run.join("failure-0001.json")).unwrap()
    );
    assert_eq!(
        observed.1,
        common::binding(&state::controller_path(run, 1)).unwrap()
    );
}

#[test]
fn historical_failure_rejects_source_owner_and_manifest_tampering() {
    for mutation in ["sources", "owner", "manifest"] {
        let mut fixture = Fixture::new();
        let run = fixture.directory.path();
        match mutation {
            "sources" => fixture.row["sources"]["contract"] = json!("other-producer"),
            "owner" => {
                let path = state::controller_path(run, 1);
                let mut controller = document::read(&path).unwrap();
                controller["owner"]["identity"]["pid"] = json!(1);
                document::write_replace(&path, &controller).unwrap();
                // 同步失败文件的外部绑定，确保拒绝来自所有者校验而非旧摘要。
                let failure = run.join("failure-0001.json");
                let mut value = document::read(&failure).unwrap();
                value["controller"] = common::binding(&path).unwrap();
                document::write_replace(&failure, &value).unwrap();
            }
            "manifest" => {
                document::write_replace(&run.join("manifest.json"), &json!({"fixture":false}))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(fixture.incident().is_err(), "未拒绝历史篡改：{mutation}");
    }
}

#[path = "restart_contract.rs"]
mod contract_tests;
