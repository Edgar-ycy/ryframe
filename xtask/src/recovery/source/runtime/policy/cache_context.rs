//! 从同一生产者与源代次文件派生清理绑定；不接受调用方自报身份。
use super::{audit, documents, producer, receipts};
use crate::{
    Result,
    recovery::clone::native::{digest, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;

fn read(backend: &Path, descriptor: &Value) -> Result<Value> {
    common::read_bound(backend, descriptor, "缓存清理原始绑定")
}

fn target(backend: &Path, start: &Value) -> Result<(Value, Value)> {
    let published = read(backend, &start["source_registration"])?;
    let registration = read(backend, &published["registration"])?;
    let manifest = read(backend, &registration["run_manifest"])?;
    let initialized = common::bound_file(backend, &manifest["initialized"], "原 seed 初始化")?;
    let parent = initialized.parent().ok_or("原初始化缺少目录")?;
    let prepare_binding = common::binding(&parent.join("prepare.json"))?;
    let prepare = read(backend, &prepare_binding)?;
    let request = read(backend, &prepare["request"])?;
    let initialization = read(backend, &manifest["initialized"])?;
    if manifest["copy_stage"] != "source_to_seed"
        || initialization["status"] != "fresh_target_initialized"
        || initialization["scope_id"] != request["target"]["scope_id"]
        || request["side"] != "seed"
        || initialization["prepare_sha256"] != prepare_binding["sha256"]
        || prepare["request_sha256"] != digest::plan_hash(&request)?
        || documents::read(&parent.join("request.json"))? != request
    {
        return Err("缓存清理原 seed 请求或初始化绑定已变化；保留现场核对".into());
    }
    Ok((
        request,
        json!({"source_registration":start["source_registration"],"registration":published["registration"],"run_manifest":registration["run_manifest"],
        "initialized":manifest["initialized"],"prepare":prepare_binding,"request":prepare["request"]}),
    ))
}

pub(super) fn load(backend: &Path, directory: &Path) -> Result<(Value, Value)> {
    common::local_directory(backend, directory, "缓存清理 verification")?;
    if directory
        .file_name()
        .is_none_or(|name| name != "verification")
    {
        return Err("缓存尝试只属于固定 verification 目录".into());
    }
    let producers = receipts::producer_bindings(directory)?;
    let intent = read(backend, &producers["intent"])?;
    let start_path = common::bound_file(backend, &intent["source_generation"], "缓存源代次")?;
    let start = read(backend, &intent["source_generation"])?;
    let run = start_path
        .parent()
        .and_then(Path::parent)
        .ok_or("源代次收据缺少 run")?;
    let generation = documents::text(&start["generation_id"], "源 generation ID")?;
    if !generation.starts_with('g')
        || !generation[1..].bytes().all(|byte| byte.is_ascii_digit())
        || generation.len() < 5
        || run.join(generation).join("verification") != directory
    {
        return Err("缓存清理目录不属于原源代次；不得跨生产者执行".into());
    }
    let generation_request = read(backend, &start["request"])?;
    if generation_request["source_registration"] != start["source_registration"] {
        return Err("源代次请求登记不同".into());
    }
    let (request, target_bindings) = target(backend, &start)?;
    let lineage = read(backend, &intent["dataset_lineage"])?;
    let configuration = read(backend, &intent["configuration"])?;
    let registered = producer::registration(&json!({"backend":backend,"directory":directory,
        "start":intent["source_generation"],"lineage":intent["dataset_lineage"],"tool":intent["executable"],
        "argv":intent["argv"],"environment_sha256":intent["environment_sha256"],
        "coordinator_source":intent["coordinator_source"],"execution_sha":intent["execution_sha"],"configuration":configuration}))?;
    let completed = producer::completion(&json!({"backend":backend,"directory":directory,
        "intent":registered["intent"],"producer":registered["producer"]}))?;
    let output = read(backend, &producers["stdout"])?;
    let result = super::super::super::model::result(&json!({"value":output,"lineage":lineage,
        "start":intent["source_generation"],"lineage_descriptor":intent["dataset_lineage"]}))?;
    common::scope_id(documents::text(
        &request["target"]["scope_id"],
        "缓存来源 scope",
    )?)?;
    if result["scope_id"] != request["target"]["scope_id"] {
        return Err("来源生产者 scope 不属于原 seed 请求".into());
    }
    if completed["exit_code"] != 0 {
        return Err("缓存清理要求原生产者成功完成；不得重放生产者".into());
    }
    let login = common::binding(&directory.join("audit/login-audit.json"))?;
    audit::stored(
        &json!({"backend":backend,"directory":directory,"descriptor":login,
        "lineage":lineage,"subjects":result["subjects"],"started_at":intent["started_at"]}),
    )?;
    super::super::super::inventory::request(
        &json!({"backend":backend,"directory":directory,"inputs":intent["inputs"]}),
        true,
    )?;
    Ok((
        request.clone(),
        json!({"source_generation":intent["source_generation"],"generation_request":start["request"],
        "target":target_bindings,"request_sha256":digest::plan_hash(&request)?,"producer":producers,
        "configuration":intent["configuration"],"login_audit":login,"dataset_lineage":intent["dataset_lineage"],
        "scope_id":request["target"]["scope_id"],"subjects":result["subjects"]}),
    ))
}
