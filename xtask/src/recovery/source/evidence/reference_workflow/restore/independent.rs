//! 用同一独立 B1 开始登记的运行记录，在原工作目录持新锁完成外部恢复。
use std::path::Path;

use serde_json::{Map, Value, json};

use super::super::local::OwnedWork;
use super::{
    CopyInputs, Execution, ReferenceTarget, Result, Scope, Stage, Storage,
    begin::{ApplySnapshot, PendingApply},
    combine, document, finish,
    inputs::{self, RegisteredSelection},
    model, path, receipts,
};
use crate::recovery::{
    inputs::RestoreChecks, runtime::restore_guard::registered_stopped_independent_runtime,
};

pub(super) fn run(backend: &Path, request: &Value) -> Result<Value> {
    let selection = RegisteredSelection::read(backend, request)?;
    let pending = PendingApply::read(backend, request, selection)?;
    let claim = pending.claim(backend)?;
    let snapshot = pending.verify(backend, request, &claim)?;
    let plan = snapshot.plan();
    let work = path(&plan["work_dir"])?;
    let output = inputs::output(backend, plan, &snapshot.target().value, snapshot.root())?;
    let owner = |selected: &Path, reference: &Value| claim.check(selected, reference);
    let check_owner = || owner(work, plan);
    check_owner()?;
    snapshot.assert_unchanged()?;
    let started = receipts::start(&json!({
        "plan":plan,"command":"restore","copy_id":null,"root":snapshot.root(),
        "receipt":output,"bindings":snapshot.bindings(),
    }))?;
    let stage = Stage {
        backend,
        work,
        check_owner: &check_owner,
        started: &started,
    };
    let mut evidence = Map::new();
    let result = registered_stopped_independent_runtime(
        backend,
        &snapshot.registration().path,
        &document::binding(snapshot.target()),
        &owner,
        |checkpoint| {
            execute(
                &stage,
                request,
                &snapshot,
                &claim,
                checkpoint,
                &mut evidence,
            )
        },
    );
    check_owner()?;
    finish(&started, result, evidence)
}

fn execute(
    stage: &Stage<'_>,
    request: &Value,
    snapshot: &ApplySnapshot,
    claim: &OwnedWork,
    checkpoint: &dyn Fn() -> Result<Value>,
    evidence: &mut Map<String, Value>,
) -> Result<Value> {
    let runtime = checkpoint()?;
    snapshot.check_runtime(&runtime)?;
    stage.check()?;
    let plan = snapshot.plan();
    let manifest = snapshot.manifest();
    let tasks = model::resources(&json!({
        "plan":plan,"root":snapshot.root(),"manifest":manifest.value,
    }))?;
    let scope = Scope::load(stage.backend, snapshot.build_bridges(), &[])?;
    let binding = snapshot.binding();
    let storage = Storage::acquire_independent_target(
        stage.backend,
        stage.work,
        &binding.registration,
        &binding.generation,
    )?;
    let result = (|| {
        let target = ReferenceTarget::open_independent(
            stage.backend,
            binding.clone(),
            &storage,
            stage.work,
            &scope,
        )?;
        let window = Execution {
            stage,
            inputs: CopyInputs {
                plan,
                root: snapshot.root(),
                target: snapshot.target(),
                manifest,
            },
            check_inputs: &|| snapshot.assert_unchanged(),
            target: &target,
            scope: &scope,
        };
        window.run(
            &tasks,
            &|_| snapshot.check_runtime(&checkpoint()?),
            RestoreChecks::default(),
            evidence,
            || snapshot.check_final(stage.backend, request, claim),
        )?;
        model::restore_result(&json!({
            "record":snapshot.record().value,"bindings":snapshot.bindings(),
            "runtime":runtime,"images":evidence,
        }))
    })();
    storage.finish(combine(result, scope.assert_unchanged()))
}
