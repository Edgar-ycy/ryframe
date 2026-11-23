//! 正式参考恢复沿用已登记的运行与存储锁，保留每次写入和完整资源像。
use std::{path::Path, sync::Mutex};

use serde_json::{Map, Value, json};

use super::{files, local::Lock, model, native, path, receipts, text};
use crate::{
    Result,
    recovery::{
        fixture_control::source_pair::execution::HeldTargetSource,
        inputs::{
            RestoreChecks,
            document::{self, Document},
            inventory::{self, HeldArtifacts},
            reference,
        },
        runtime::restore_guard::registered_stopped_runtime,
    },
};
use native::{
    bridge::scope::Scope, cache::handoff::locks::Storage, copy_transfer::ReferenceTarget,
    export::transport::SystemTransport, s3,
};

#[path = "restore/begin.rs"]
mod begin;
#[path = "restore/begin_copy.rs"]
mod begin_copy;
#[path = "restore/begin_proof.rs"]
mod begin_proof;
#[path = "restore/database_images.rs"]
mod database_images;
#[path = "restore/expected_images.rs"]
mod expected_images;
#[path = "restore/images.rs"]
mod images;
#[path = "restore/independent.rs"]
mod independent;
#[path = "restore/inputs.rs"]
mod inputs;
#[path = "restore/object_images.rs"]
mod object_images;
#[path = "restore/object_write.rs"]
mod object_write;
#[path = "restore/writes.rs"]
mod writes;

pub(super) use begin::run as run_begin;
use inputs::Snapshot;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Before,
    After,
    FailureAfter,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
            Self::FailureAfter => "failure-after",
        }
    }
}

pub(super) fn run(backend: &Path, request: &Value) -> Result<Value> {
    let reference = document::read(path(&request["plan"])?, backend, "参考恢复计划")?;
    if reference::is_independent_b1(&reference.value) {
        return independent::run(backend, request);
    }
    let before = Snapshot::read(backend, request, true)?;
    let output = before.output(backend)?;
    let plan = &before.chain.reference.value;
    let work = files::work(&json!({"plan":plan}), true)?;
    let work = path(&work)?;
    let lock = Lock::claim(work)?;
    if before.output(backend)? != output {
        return Err("取得参考阶段锁前恢复输出位置变化".into());
    }
    before.assert_unchanged()?;
    let bindings = before.bindings();
    let started = receipts::start(&json!({
        "plan":plan,"command":"restore","copy_id":null,"root":before.root,
        "receipt":output,"bindings":bindings,
    }))?;
    let stage = Stage {
        backend,
        work,
        check_owner: &|| lock.check(),
        started: &started,
    };
    let mut evidence = Map::new();
    let result = registered_stopped_runtime(
        backend,
        &before.registration.path,
        &document::binding(&before.target),
        |checkpoint| stage.execute(request, &before, checkpoint, &mut evidence),
    );
    lock.check()?;
    finish(&started, result, evidence)
}

fn finish(started: &Value, result: Result<Value>, evidence: Map<String, Value>) -> Result<Value> {
    match result {
        Ok(value) => {
            receipts::finish(&json!({
                "receipt":started,"result":value,"failure":null,"resource_images":null,
            }))?;
            Ok(value)
        }
        Err(error) => {
            let images = if evidence.is_empty() {
                Value::Null
            } else {
                Value::Object(evidence)
            };
            let recorded = receipts::finish(&json!({
                "receipt":started,"result":null,"resource_images":images,
                "failure":{"error_type":"ReferenceRestoreFailure",
                    "message":"参考恢复未完成；保留原输入、写入诊断和资源像，禁止覆盖或自动重放"},
            }));
            if recorded.is_err() {
                return Err("参考恢复与失败收据发布均未确认；保留运行收据和原现场".into());
            }
            Err(error)
        }
    }
}

struct Stage<'a> {
    backend: &'a Path,
    work: &'a Path,
    check_owner: &'a (dyn Fn() -> Result<()> + Sync),
    started: &'a Value,
}

impl Stage<'_> {
    fn check(&self) -> Result<()> {
        (self.check_owner)()?;
        receipts::check_started(self.started)?;
        Ok(())
    }

    fn execute(
        &self,
        request: &Value,
        before: &Snapshot,
        checkpoint: &dyn Fn(RestoreChecks<'_>) -> Result<Value>,
        evidence: &mut Map<String, Value>,
    ) -> Result<Value> {
        let runtime = checkpoint(RestoreChecks::default())?;
        before.check_runtime(&runtime)?;
        self.check()?;
        let snapshot = Snapshot::read(self.backend, request, false)?;
        if snapshot.bindings() != before.bindings() {
            return Err("恢复执行前目标计划或运行记录发生变化".into());
        }
        let plan = &snapshot.chain.reference.value;
        let tasks = model::resources(&json!({
            "plan":plan,"root":snapshot.root,"manifest":snapshot.manifest.value,
        }))?;
        let arm = &snapshot.chain.arm;
        let manifest = &arm.manifest.value;
        let bridges =
            native::json::array(&arm.request.value["build_bridges"], "参考恢复原构建桥接")?;
        let scope = Scope::load(self.backend, bridges, &[])?;
        let storage = Storage::acquire(self.backend, self.work, manifest)?;
        let result = (|| {
            let target =
                ReferenceTarget::open(self.backend, manifest, &storage, self.work, &scope)?;
            let window = Execution {
                stage: self,
                inputs: CopyInputs {
                    plan,
                    root: &snapshot.root,
                    target: &snapshot.target,
                    manifest: &snapshot.manifest,
                },
                check_inputs: &|| snapshot.assert_unchanged(),
                target: &target,
                scope: &scope,
            };
            #[cfg(windows)]
            let backup = Some(HeldArtifacts::open(
                &snapshot.root,
                &snapshot.manifest.value,
            )?);
            #[cfg(not(windows))]
            let backup: Option<HeldArtifacts> = None;
            #[cfg(windows)]
            let source = hold_source(self.backend, &snapshot);
            #[cfg(not(windows))]
            let source: Result<Option<HeldTargetSource>> = Ok(None);
            let result = source.and_then(|source| {
                let checks = RestoreChecks {
                    backup: backup.as_ref(),
                    source: source.as_ref(),
                };
                let result = window.run(
                    &tasks,
                    &|checks| snapshot.check_runtime(&checkpoint(checks)?),
                    checks,
                    evidence,
                    || self.verify_final(request, &snapshot),
                );
                combine(
                    result,
                    source.as_ref().map_or(Ok(()), HeldTargetSource::check_end),
                )
            });
            combine(
                result,
                backup.as_ref().map_or(Ok(()), HeldArtifacts::check_end),
            )?;
            model::restore_result(&json!({
                "record":snapshot.record.value,"bindings":snapshot.bindings(),
                "runtime":runtime,"images":evidence,
            }))
        })();
        storage.finish(combine(result, scope.assert_unchanged()))
    }

    fn verify_final(&self, request: &Value, snapshot: &Snapshot) -> Result<()> {
        let final_input = Snapshot::read(self.backend, request, true)?;
        if final_input.bindings() != snapshot.bindings() {
            return Err("恢复完成复核时来源或输入绑定发生变化".into());
        }
        snapshot.check_final()
    }
}

#[cfg(windows)]
fn hold_source(backend: &Path, snapshot: &Snapshot) -> Result<Option<HeldTargetSource>> {
    let target = &snapshot.chain.arm.target;
    target
        .document
        .value
        .get("execution_backend")
        .filter(|value| value.get("execution_source").is_some())
        .map(|declared| {
            let coordinator =
                crate::recovery::fixture_control::portable_path(&backend.canonicalize()?);
            HeldTargetSource::open(&coordinator, declared, &target.execution_binding)
        })
        .transpose()
}

struct CopyInputs<'a> {
    plan: &'a Value,
    root: &'a Path,
    target: &'a Document,
    manifest: &'a Document,
}

struct Execution<'a> {
    stage: &'a Stage<'a>,
    inputs: CopyInputs<'a>,
    check_inputs: &'a (dyn Fn() -> Result<()> + Sync),
    target: &'a ReferenceTarget<'a>,
    scope: &'a Scope,
}

impl Execution<'_> {
    fn check(&self) -> Result<()> {
        self.stage.check()?;
        (self.check_inputs)()?;
        self.target.checkpoint()
    }

    fn run(
        &self,
        tasks: &Value,
        checkpoint: &dyn Fn(RestoreChecks<'_>) -> Result<()>,
        checks: RestoreChecks<'_>,
        evidence: &mut Map<String, Value>,
        verify_final: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let (before, after) = expected_images::expected_images(
            self.inputs.plan,
            &self.inputs.manifest.value,
            self.target.initialized(),
        )?;
        let runtime = || checkpoint(RestoreChecks::default());
        capture_around_write(
            |phase, evidence| {
                let expected = if phase == Phase::Before {
                    &before
                } else {
                    &after
                };
                self.capture(phase, expected, &runtime, evidence)
            },
            || {
                writes::Writes {
                    storage: self.target.inputs(),
                    work: self.stage.work,
                    backup_root: self.inputs.root,
                    plan: self.inputs.plan,
                    manifest: &self.inputs.manifest.value,
                }
                .run(tasks, &SystemTransport, s3::put_if_absent, &|| {
                    checkpoint(checks)?;
                    self.check()
                })?;
                inventory::verify_artifacts(self.inputs.root, &self.inputs.manifest.value)
            },
            evidence,
        )?;
        runtime()?;
        verify_final()?;
        self.check()
    }

    fn capture(
        &self,
        phase: Phase,
        expected: &Value,
        runtime: &dyn Fn() -> Result<()>,
        evidence: &mut Map<String, Value>,
    ) -> Result<()> {
        runtime()?;
        self.target.check()?;
        let serial = Mutex::new(());
        let observe = || {
            let _guard = serial.lock().map_err(|_| "参考恢复观察检查点中断")?;
            self.check()
        };
        images::capture(self, phase, expected, &observe, evidence)?;
        self.target.check()?;
        runtime()
    }
}

fn capture_around_write(
    capture: impl Fn(Phase, &mut Map<String, Value>) -> Result<()>,
    write: impl FnOnce() -> Result<()>,
    evidence: &mut Map<String, Value>,
) -> Result<()> {
    capture(Phase::Before, evidence)?;
    let result = write().and_then(|()| capture(Phase::After, evidence));
    match result {
        Err(error) if !evidence.contains_key(Phase::After.name()) => {
            combine(Err(error), capture(Phase::FailureAfter, evidence))
        }
        result => result,
    }
}

fn combine<T>(result: Result<T>, checked: Result<()>) -> Result<T> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(_)) => Err(format!("{error}；参考恢复后验也失败，保留现场").into()),
    }
}
