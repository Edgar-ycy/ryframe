use std::{collections::BTreeMap, fs, path::PathBuf};

use serde_json::{Value, json};

use super::{
    Facts, Files, Fixture, FixtureIo, Mode, OfflineFixture, Plan, database_writes, document,
    register_original_inputs, run_published_copy, snapshot,
};
use crate::recovery::clone::native::{
    self,
    lifecycle::{lock::RunLock, state},
    seed::source_registration::copied,
};

struct CompletedCopy {
    source: OfflineFixture,
    fixture: Fixture,
    run: PathBuf,
    manifest: Value,
    plan: Value,
    outer_result: PathBuf,
}

impl CompletedCopy {
    fn new() -> Self {
        let source = OfflineFixture::new();
        let initialized = json!({
            "inventory":{"observations":{"shared-control":{"kind":"database","tables":{},"preserved":{}}}}
        });
        let mut fixture = Fixture::in_root(&source.root, &initialized);
        let (run, manifest, declaration, proofs) =
            register_original_inputs(&mut fixture, &source, &initialized);
        let source_observation =
            json!({"shared-control":{},"shared":{},"dedicated-a":{},"dedicated-b":{}});
        let lock = RunLock::claim(&run).unwrap();
        let (number, attempt) = state::begin(&run, "copy", "run", &json!({})).unwrap();
        state::bind_controller(&run, number, &lock.owner, &attempt).unwrap();
        // 用正式准备替换测试夹具初建的空会话。
        fs::remove_file(fixture.copy.join("session.json")).unwrap();
        let prepared = Files::bind(&fixture.backend, &manifest, &manifest["source_export"])
            .unwrap()
            .prepare(Facts {
                source_generation: &json!({"stopped":true}),
                target_generation: &json!({"initialized":true}),
                source_observation: &source_observation,
                declaration: &declaration,
            })
            .unwrap();
        let plan = Plan::from_verified(&prepared.plan).unwrap();
        let mut io = FixtureIo::new(&plan);
        database_writes(&mut io, &plan.databases);
        // 数据库和对象动作只走原有隔离替身，计划、复制收据和账本实际落盘。
        let (result, ledger) =
            run_published_copy(&fixture, &prepared, &plan, Mode::New, &mut io).unwrap();
        assert_eq!(ledger["status"], "ledger_evidence_complete");
        assert_eq!(io.writes.len(), 3);
        proofs.checkpoint().unwrap();
        lock.release().unwrap();
        state::finish(&run, number, Some(&result), None).unwrap();
        let outer_result = state::result_path(&run, number);
        Self {
            source,
            fixture,
            run,
            manifest,
            plan: prepared.plan.clone(),
            outer_result,
        }
    }

    fn require(&self) -> crate::Result<Value> {
        copied::require(&self.fixture.backend, &self.run, &self.manifest)
    }

    fn saved_copy(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut saved = snapshot(&self.fixture.copy.join("ledger"));
        for path in [
            self.outer_result.clone(),
            self.fixture.copy.join("result.json"),
        ] {
            saved.insert(path.clone(), fs::read(path).unwrap());
        }
        saved
    }
}

#[test]
fn post_plan_reads_the_same_verified_plan_from_the_completed_copy() {
    let copy = CompletedCopy::new();
    let saved = copy.saved_copy();
    assert_eq!(copy.require().unwrap(), copy.plan);
    assert_eq!(copy.saved_copy(), saved);
}

#[test]
fn post_plan_rejects_payload_drift_after_valid_publication() {
    let copy = CompletedCopy::new();
    let saved = copy.saved_copy();
    assert_eq!(copy.require().unwrap(), copy.plan);
    let object = copy.source.export.join("file.bin");
    assert_eq!(fs::read(&object).unwrap(), b"data");
    fs::write(&object, b"evil").unwrap();

    assert_eq!(
        copy.require().unwrap_err().to_string(),
        "复制证据已变化或未完整写入"
    );
    assert_eq!(fs::read(object).unwrap(), b"evil");
    assert_eq!(copy.saved_copy(), saved);
}

#[test]
fn post_plan_rejects_a_newer_failed_copy_and_preserves_the_original() {
    let copy = CompletedCopy::new();
    let saved = copy.saved_copy();
    let lock = RunLock::claim(&copy.run).unwrap();
    let (number, attempt) = state::begin(&copy.run, "copy", "reconcile", &json!({})).unwrap();
    state::bind_controller(&copy.run, number, &lock.owner, &attempt).unwrap();
    let failure = "Interrupted";
    state::record_failure(&copy.run, number, "copy", "reconcile", failure).unwrap();
    lock.release().unwrap();
    state::finish(&copy.run, number, None, Some(failure)).unwrap();

    assert_eq!(
        copy.require().unwrap_err().to_string(),
        "最新复制阶段失败，不能回退早期成功"
    );
    assert_eq!(copy.saved_copy(), saved);
    assert!(!state::result_path(&copy.run, number).exists());
}

#[test]
fn post_plan_rejects_a_different_valid_plan_with_the_original_result_and_ledger() {
    let copy = CompletedCopy::new();
    let saved = copy.saved_copy();
    let output = &copy.fixture.copy;
    let input = output.join("replacement-input.json");
    let mut declaration = document::read(&output.join("input.json")).unwrap();
    declaration["copy_id"] = json!("alternate-copy");
    document::write_new(&input, &declaration).unwrap();
    let replacement = output.join("replacement-plan.json");
    native::plan(&copy.fixture.backend, &input, &replacement).unwrap();
    let wrapper = document::read(&replacement).unwrap();
    assert_ne!(wrapper["plan"]["plan_sha256"], copy.plan["plan_sha256"]);
    let plan = output.join("plan.json");
    document::write_replace(&plan, &wrapper).unwrap();
    let accepted = native::verify(&copy.fixture.backend, &plan).unwrap();
    assert_eq!(accepted["plan_sha256"], wrapper["plan"]["plan_sha256"]);

    assert_eq!(
        copy.require().unwrap_err().to_string(),
        "目标数据复制尚未发布有效的完整账本收据"
    );
    assert_eq!(document::read(&plan).unwrap(), wrapper);
    assert_eq!(copy.saved_copy(), saved);
}
