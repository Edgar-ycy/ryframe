//! 首次写入仅在完整prepare、实际空前像与同一generation一致时执行。
use super::super::super::super::{
    digest, export::transport::Transport, json as document, resources::common,
};
use super::super::super::{
    creation_plan, evidence_creation, evidence_reset, resume_baseline, resume_observations,
    resume_prefix, tree,
};
use super::super::{failure, receipts};
use super::{operations, reset_configuration::Snapshot, window::Window};
use crate::Result;
use crate::utc_timestamp::now_rfc3339;
use serde_json::{Value, json};

pub(crate) fn execute<T: Transport>(
    w: &Window<'_, T>,
    proof: &Value,
    stage: &mut &str,
) -> Result<(Value, Value)> {
    let (baseline, original, snapshot, prepared_binding) = preflight(w, proof)?;
    let target = w.inputs.observation_dir;
    receipts::write(
        &target.join("initialize.started.json"),
        &json!({"at":now_rfc3339(),"generation_sha256":digest::plan_hash(&original)?}),
    )?;
    *stage = "create_and_reset";
    let reset = operations::initialize(w, &original, &snapshot)?;
    *stage = "inventory";
    let inventory = failure::capture(super::super::super::database::capture_at(
        &w.inputs,
        &target.join("inventory-initial"),
        w.transport,
        &|| w.checkpoint(),
    ))?;
    let initial = json!({"receipt":inventory.receipt,"observations":inventory.observations});
    creation_plan::execute(
        w.inputs.backend,
        target,
        "initial-inventory-shape",
        &json!({"databases":w.inputs.request["target"]["databases"],"images":initial["observations"]}),
    )?;
    let value = publish(w, &original, &reset, &initial, &prepared_binding)?;
    let files = validate(
        w,
        &baseline,
        &original,
        &initial,
        &inventory.evidence_files,
        &value,
    )?;
    snapshot.check()?;
    Ok((value, files))
}

fn preflight<T: Transport>(
    w: &Window<'_, T>,
    proof: &Value,
) -> Result<(Value, Value, Snapshot, Value)> {
    let target = w.inputs.observation_dir;
    let prepared_binding = common::binding(&target.join("prepare.json"))?;
    let prepared = document::read(&target.join("prepare.json"))?;
    let baseline = resume_baseline::prepared(
        w.inputs.backend,
        target,
        proof,
        &w.registration.value["request"],
    )?;
    let mut actual = tree::snapshot(w.inputs.backend, target, true)?;
    actual
        .as_array_mut()
        .ok_or("初始化前像不是文件清单")?
        .retain(|v| v["path"] != "copy-target.guard");
    if actual != baseline {
        return Err("首次初始化目录含有prepare之外的文件，禁止接管".into());
    }
    let original = w.generation()?.0;
    if prepared["generation"] != original {
        return Err("prepare之后来源或存储代次变化，拒绝首次写入".into());
    }
    let databases = document::array(&w.inputs.request["target"]["databases"], "四目标")?
        .iter()
        .map(|db| super::super::resources::database_absence(&w.inputs, w.transport, db))
        .collect::<Result<Vec<_>>>()?;
    let absence = json!({"databases":databases,"objects":w.objects(false)?,"redis":w.redis_state(&original,false,false)?});
    if prepared["absence"] != absence {
        return Err("prepare之后目标空前像变化，禁止接管".into());
    }
    let snapshot = Snapshot::prepare(w.inputs.backend, target, w.inputs.request, &original)?;
    operations::reset_plan(w, &original, "reset-plan")?;
    snapshot.check()?;
    Ok((baseline, original, snapshot, prepared_binding))
}
fn publish<T: Transport>(
    w: &Window<'_, T>,
    original: &Value,
    reset: &Value,
    initial: &Value,
    prepared_binding: &Value,
) -> Result<Value> {
    let target = w.inputs.observation_dir;
    let objects = w.objects(true)?;
    let redis = w.redis_state(original, true, true)?;
    w.unchanged(original)?;
    common::bound_file(
        w.inputs.backend,
        &w.registration.value["request"],
        "初始化固定请求",
    )?;
    if common::binding(&target.join("prepare.json"))? != *prepared_binding {
        return Err("初始化期间prepare证据变化".into());
    }
    let value = super::super::super::resume::transition::execute(
        w.inputs.backend,
        target,
        "initialized",
        &json!({
        "request":w.inputs.request,"generation":original,"reset":reset,"inventory":initial,"objects":objects,"redis":redis,"at":now_rfc3339()}),
    )?;
    receipts::write(&target.join("initialized-candidate.json"), &value)?;
    receipts::write(&target.join("initialized.json"), &value)?;
    Ok(value)
}
fn validate<T: Transport>(
    w: &Window<'_, T>,
    baseline: &Value,
    original: &Value,
    initial: &Value,
    inventory_files: &[Value],
    result: &Value,
) -> Result<Value> {
    let target = w.inputs.observation_dir;
    let creation = evidence_creation::creation(w.inputs.backend, target, w.inputs.request)?;
    let reset = evidence_reset::aggregate(w.inputs.backend, target, w.inputs.request, original)?;
    let operations = resume_prefix::operations(
        w.inputs
            .maintenance_executable("migrate")?
            .to_str()
            .ok_or("迁移工具路径无效")?,
        w.inputs.request,
    )?;
    let completed =
        resume_prefix::prefix(w.inputs.backend, target, &operations["operations"], true)?;
    let mut required =
        evidence_creation::paths(target, &creation, &reset, &completed["completed"])?;
    required.remove("failure.json");
    required.extend(super::claims::inventory(
        w,
        "inventory-initial",
        initial,
        inventory_files,
    )?);
    required.extend([
        "initialized-candidate.json".into(),
        "initialized.json".into(),
    ]);
    let files = tree::snapshot(w.inputs.backend, target, true)?;
    resume_observations::delta(
        w.inputs.backend,
        target,
        &files,
        baseline,
        w.inputs.request,
        resume_observations::Delta {
            required: &required,
            rounds: 12 + document::array(&completed["completed"], "完整迁移前缀")?.len(),
            initialized: Some(&result["objects"]),
        },
    )?;
    Ok(files)
}
pub(super) fn failure(
    registration: &super::super::registration::Registration,
    stage: &str,
    error: &(dyn std::error::Error + 'static),
) -> Result<()> {
    let root = registration.target.join("failure.json");
    let path = if root.try_exists()? {
        registration
            .target
            .join(format!("failure-{}.json", uuid::Uuid::new_v4().simple()))
    } else {
        root
    };
    let mut body = receipts::failure(stage);
    body["error_type"] = json!(failure::category(error));
    body["reason"] =
        json!("初始化未完成；核对同目录完整证据和固定输入，只使用明确可续作操作，不自动清理或重放");
    receipts::write(&path, &body)
}
