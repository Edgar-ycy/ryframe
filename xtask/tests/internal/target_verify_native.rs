use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::CloneStage,
    recovery::clone::native::{
        export::transport::{CommandOutput, CommandSpec, Transport},
        json as json_io,
        lifecycle::{
            stage::{self, StageRequest, StageRunner},
            state,
        },
        s3,
        target_verify::{
            self, authority,
            storage::{StorageInputs, StorageObserver},
        },
    },
};

use crate::target_verify_fixture as fixture;

use fixture::{Fixture, database::FakeTransport, snapshot};

fn preflight_with(
    backend: &Path,
    request: &StageRequest<'_>,
    observer: &impl authority::Observer,
) -> Result<()> {
    target_verify::input::validate_inputs(backend, request)?;
    let _ = authority::inspect(backend, request, observer)?;
    Ok(())
}

fn execute_with<T: Transport>(
    backend: &Path,
    request: &StageRequest<'_>,
    transport: &T,
    storage_observer: &impl StorageObserver,
    authority_observer: &impl authority::Observer,
) -> Result<Value> {
    let context = target_verify::input::TargetContext::load(backend, request)?;
    target_verify::execute_context(
        &context,
        request,
        transport,
        storage_observer,
        authority_observer,
    )
}

#[test]
fn target_verify_success_publishes_create_only_evidence() {
    let fixture = Fixture::new();
    let manifest_path = fixture.run.join("manifest.json");
    #[cfg(windows)]
    {
        let mut manifest = json_io::read(&manifest_path).unwrap();
        manifest["initialized"]["path"] = serde_json::json!(
            fixture
                .target
                .join("initialized.json")
                .to_str()
                .unwrap()
                .replace('\\', "/")
        );
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }
    let manifest = json_io::read(&manifest_path).unwrap();
    let before = snapshot(&fixture.target);
    let run_before = snapshot(&fixture.run);
    let transport = FakeTransport::new(&fixture, false, false);
    let authority = fixture.authority();
    let storage = fixture.storage_observer();
    let result = execute_with(
        &fixture.backend,
        &fixture.stage(),
        &transport,
        &storage,
        &authority,
    )
    .unwrap();
    assert_eq!(result["status"], "fresh_target_reverified");
    assert_eq!(result["remote_writes"], 0);
    for field in ["target_ready", "clone_verified", "restore_qualified"] {
        assert_eq!(result[field], false);
    }
    assert_eq!(result["initialized"], manifest["initialized"]);
    assert_eq!(snapshot(&fixture.target), before);
    let run_after = snapshot(&fixture.run);
    for (name, digest) in run_before {
        assert_eq!(run_after.get(&name), Some(&digest));
    }
    assert!(fixture.run.join("v0001/verify.json").is_file());
    assert!(!fixture.run.join("v0001/failure.json").exists());
    assert_eq!(transport.inventories.get(), 8);
    assert_eq!(transport.identities.get(), 16);
    assert_eq!(transport.ownerships.get(), 24);
    assert_eq!(authority.calls.get(), 2);
    storage.assert_complete();
    assert!(!fixture.target.join("initialize.lock").exists());
    assert!(
        execute_with(
            &fixture.backend,
            &fixture.stage(),
            &transport,
            &storage,
            &authority
        )
        .is_err()
    );
    assert_eq!(snapshot(&fixture.target), before);
}

#[test]
fn target_verify_preflight_is_read_only() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.backend);
    let authority = fixture.authority();
    preflight_with(&fixture.backend, &fixture.stage(), &authority).unwrap();
    assert_eq!(snapshot(&fixture.backend), before);
    assert!(!fixture.run.join("v0001").exists());
    assert_eq!(authority.calls.get(), 1);
}

#[cfg(windows)]
#[test]
fn live_target_verify_rejects_historical_redis_before_observation() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.backend);
    let expected = target_verify::storage::redis_layout::WINDOWS_REQUIRED;
    let preflight = target_verify::preflight(&fixture.backend, &fixture.stage()).unwrap_err();
    assert_eq!(preflight.to_string(), expected);
    let execute = target_verify::execute(&fixture.backend, &fixture.stage()).unwrap_err();
    assert_eq!(execute.to_string(), expected);
    assert_eq!(snapshot(&fixture.backend), before);
    assert!(!fixture.run.join("v0001").exists());
    assert!(!fixture.target.join("initialize.lock").exists());
}

#[test]
fn target_verify_rejects_inventory_drift_and_external_failure() {
    for scenario in [
        Scenario::Inventory,
        Scenario::External,
        Scenario::MissingInput,
        Scenario::Object,
        Scenario::Redis,
        Scenario::Unknown("password=fake-password token=fake-token"),
        Scenario::Unknown("输入路径不存在 password=fake-password token=fake-token"),
    ] {
        let fixture = Fixture::new();
        state::initialize(&fixture.run).unwrap();
        let before = snapshot(&fixture.target);
        let runner = FailureRunner {
            fixture: &fixture,
            transport: FakeTransport::new(
                &fixture,
                matches!(scenario, Scenario::Inventory),
                matches!(scenario, Scenario::External),
            ),
            storage: fixture.storage_observer(),
            scenario,
        };
        let error = stage::run(
            &fixture.backend,
            &fixture.run,
            CloneStage::TargetVerify,
            &runner,
        )
        .unwrap_err()
        .to_string();
        let path = fixture.run.join("v0001").join("failure.json");
        let failure = json_io::read(&path).unwrap();
        assert!(error.contains("TargetVerifyError"), "{error}");
        assert!(
            error.contains("verify_existing") && error.contains("未验收"),
            "{error}"
        );
        assert!(error.contains(path.to_str().unwrap()), "{error}");
        assert!(error.contains("clone status --run-dir") && error.contains("完整前后像"));
        let expected = match scenario {
            Scenario::Inventory => "两次完整库存不同，不能声明只读稳定",
            Scenario::External => "目标库存 CLI 执行失败，诊断已绑定",
            Scenario::MissingInput => "输入路径不存在",
            Scenario::Object => "目标对象 key、owner 或版本状态与初始化要求不同",
            Scenario::Redis => "目标 Redis namespace、owner 或 sentinel 与初始化不同",
            Scenario::Unknown(_) => "未分类的目标复核错误",
        };
        assert!(error.contains(expected), "{error}");
        assert!(failure["reason"].as_str().unwrap().contains(expected));
        if matches!(scenario, Scenario::Redis) {
            assert!(error.contains("新的隔离目标"), "{error}");
        }
        let public = format!("{error} {failure}");
        assert!(!public.contains("fake-password") && !public.contains("fake-token"));
        assert!(!fixture.run.join("v0001/verify.json").exists());
        assert_eq!(snapshot(&fixture.target), before);
        assert!(!fixture.target.join("initialize.lock").exists());
        for field in [
            "automatic_retry",
            "automatic_resource_cleanup",
            "fresh_target_initialized",
            "clone_verified",
            "restore_qualified",
        ] {
            assert_eq!(failure[field], false);
        }
        let ledger = state::load(&fixture.run, true).unwrap();
        assert_eq!(ledger["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(ledger["attempts"][0]["status"], "failed");
        assert_eq!(ledger["attempts"][0]["error_type"], "TargetVerifyError");
        assert!(ledger["attempts"][0]["result"].is_null());
        let outer = json_io::read(&fixture.run.join("failure-0001.json")).unwrap();
        assert_eq!(outer["error_type"], "TargetVerifyError");
        assert!(!fixture.run.join("results/0001.json").exists());
        assert!(!fixture.run.join("run.lock").exists());
        assert_eq!(
            runner.transport.inventories.get(),
            if matches!(scenario, Scenario::External) {
                1
            } else {
                8
            }
        );
    }
}

#[test]
fn target_verify_respects_target_generation_guard() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.target.join("initialize.lock")).unwrap();
    let before = snapshot(&fixture.target);
    let transport = FakeTransport::new(&fixture, false, false);
    let error = execute_with(
        &fixture.backend,
        &fixture.stage(),
        &transport,
        &fixture.storage_observer(),
        &fixture.authority(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("初始化锁"), "{error}");
    assert_eq!(transport.inventories.get(), 0);
    assert!(!fixture.run.join("v0001").exists());
    assert_eq!(snapshot(&fixture.target), before);
    assert!(fixture.target.join("initialized.json").is_file());
}

#[derive(Clone, Copy)]
enum Scenario {
    Inventory,
    External,
    MissingInput,
    Object,
    Redis,
    Unknown(&'static str),
}

struct FailureRunner<'a, O> {
    fixture: &'a Fixture,
    transport: FakeTransport<'a>,
    storage: O,
    scenario: Scenario,
}

impl<O: StorageObserver> StageRunner for FailureRunner<'_, O> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        preflight_with(&self.fixture.backend, request, &self.fixture.authority())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        let result = execute_with(
            &self.fixture.backend,
            request,
            self,
            self,
            &self.fixture.authority(),
        );
        let error = result.as_ref().unwrap_err();
        assert!(error.source().is_some());
        if let Scenario::Unknown(raw) = self.scenario {
            assert_eq!(error.source().unwrap().to_string(), raw);
            let public = format!("{error} {error:?}");
            assert!(!public.contains("fake-password") && !public.contains("fake-token"));
        }
        result
    }
}

impl<O> Transport for FailureRunner<'_, O> {
    fn run(&self, spec: CommandSpec) -> Result<CommandOutput> {
        self.transport.run(spec)
    }

    fn s3(&self, request: s3::Request) -> Result<Value> {
        match self.scenario {
            Scenario::MissingInput => Err("输入路径不存在".into()),
            Scenario::Unknown(raw) => Err(raw.into()),
            Scenario::Object if matches!(&request.operation, s3::Operation::List { .. }) => {
                Ok(json!({"IsTruncated": false, "Contents": []}))
            }
            _ => self.transport.s3(request),
        }
    }
}

impl<O: StorageObserver> StorageObserver for FailureRunner<'_, O> {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>> {
        self.storage.process_identity(pid)
    }

    fn arguments(&self, context: &StorageInputs<'_>, identity: &Value) -> Result<Vec<String>> {
        self.storage.arguments(context, identity)
    }

    fn listener(&self, context: &StorageInputs<'_>, identity: &Value, url: &str) -> Result<()> {
        self.storage.listener(context, identity, url)
    }

    fn wsl(&self, context: &StorageInputs<'_>, redis: &Value, args: &[String]) -> Result<String> {
        self.storage.wsl(context, redis, args)
    }

    fn redis(&self, context: &StorageInputs<'_>, redis: &Value, parts: &[&str]) -> Result<Value> {
        if matches!(self.scenario, Scenario::Redis) {
            match parts {
                ["GET", _] => return Ok(Value::Null),
                ["SCAN", ..] => return Ok(json!(["0", []])),
                _ => {}
            }
        }
        self.storage.redis(context, redis, parts)
    }
}
