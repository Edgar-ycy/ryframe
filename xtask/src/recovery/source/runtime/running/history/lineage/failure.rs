//! 固定启动前失败的全量证据读取；缺少任何原始输入都不能授权后继。
use super::{
    super::{super::builds, prelaunch},
    model,
};
use crate::recovery::source::evidence::{images, inventory, staging};
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        cache, digest, json as document,
        lifecycle::state,
        resources::{common, storage::registered},
        seed::protocol::guards,
    },
};
use serde_json::{Value, json};
use std::path::Path;

const HEAD: &str = "6b58001aa320e3fe7f1de62b100654b0f2a51edc";
const TREE: &str = "2c63b19a658f0b0a69bf3855f0cb343046c7539c";

#[path = "failure/inputs.rs"]
pub(in crate::recovery::source::evidence) mod inputs;

pub(in crate::recovery::source::evidence) fn inspect(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    external: Option<&Path>,
) -> Result<Value> {
    let archive = prelaunch::closed(backend, run, attempts)?;
    let tail = model("history", json!({"attempts":attempts,"archive":archive}))?["tail"].clone();
    let tail = document::array(&tail, "C68 完整历史尾")?;
    staging::validate_source(&archive["recovery"]["sources"])?;
    for row in tail {
        staging::validate_source(&row["sources"])?;
    }
    let length = tail.len();
    if length < 3 {
        return Err("C68 失败缺少完整相邻历史".into());
    }
    prelaunch::reregistration(backend, run, &tail[length - 2], &tail[length - 3])?;
    let failed = &tail[length - 1];
    if failed["sources"]["snapshot"]["head"] != HEAD || builds::git_tree(backend, HEAD)? != TREE {
        return Err("C68 谱系失败不属于已审计提交和整树".into());
    }
    let (controller, owner) = controller(run, failed)?;
    let failure = common::binding(&run.join("failure-0068.json"))?;
    model(
        "failure",
        json!({"failure":common::read_bound(backend,&failure,"C68 原失败")?,"controller":controller}),
    )?;
    common::require_recorded_stopped(&owner["identity"])?;
    let output = run.join("g0068");
    inputs::layout(backend, &output)?;
    let files = inventory::manifest(&output)?;
    if summary(&files)?
        != json!({"files":10233,"bytes":1081069032_u64,
        "sha256":"4cffdac03ff9d83566f56f7ea3114766fdc3acf86e566cd24944a8378567899b"})
    {
        return Err("C68 generation 完整文件清单变化".into());
    }
    for path in [
        run.join("results/0068.json"),
        run.join("seed-runtime/attempt-0068"),
    ] {
        inputs::missing(backend, &path)?;
    }
    let intent = common::binding(&output.join("intent.json"))?;
    model(
        "intent",
        json!({"intent":common::read_bound(backend,&intent,"C68 启动意图")?,
        "request":common::binding(&output.join("request.json"))?,"runtime":output.join("runtime")}),
    )?;
    let requests = inputs::requests(backend, &output, external)?;
    let source = source(backend, run, attempts, &requests["request"], tail)?;
    let observed = inputs::runtime(backend, &output, &requests["request"], &source)?;
    let before = common::binding(&output.join("before/image.json"))?;
    let image = images::verify(
        backend,
        &before,
        &observed["selected"],
        &source["request"],
        &requests["request"]["source_registration"],
    )?;
    let runtime = common::binding(&output.join("runtime/runtime.json"))?;
    let proof = json!({"attempt":digest::plan_hash(failed)?,"failure":failure,"controller":controller,
        "source":failed["sources"],"head":HEAD,"tree":TREE,"manifest":summary(&files)?,
        "request":requests["internal_descriptor"],"intent":intent,"runtime":runtime,"before":before});
    if self::controller(run, failed)? != (controller, owner)
        || inventory::manifest(&output)? != files
        || builds::git_tree(backend, HEAD)? != TREE
    {
        return Err("C68 失败、源码、目录或完整前像在核验期间变化".into());
    }
    for key in ["failure", "request", "intent", "runtime", "before"] {
        common::bound_file(backend, &proof[key], "C68 原始证据")?;
    }
    Ok(
        json!({"proof":proof,"failed":failed,"request":requests["request"],"historical_request":requests["internal"],
        "request_descriptor":requests["descriptor"],"source":source,"contract":observed["contract"],
        "selected":observed["selected"],"before":before,"image":image["image"],"records":tail}),
    )
}

fn source(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    request: &Value,
    tail: &[Value],
) -> Result<Value> {
    let selected = model(
        "select-source",
        json!({"attempts":attempts,"request":request}),
    )?;
    let record = &selected["rebind"];
    let rebound = super::super::stops::result(backend, run, record)?;
    guards::rebind_receipt(
        &json!({"value":rebound,"descriptor":request["source_registration"],
        "original":rebound["original_storage"],"prefix":selected["prefix"]}),
    )?;
    let mut source = cache::binding::source::load(backend, &request["review_successor"])?;
    guards::rebind::live::transition(
        backend,
        &json!({"original":rebound["original_storage"],"current":rebound["current_storage"]}),
    )?;
    let storage =
        registered::registered(backend, run, CloneSide::Target, Some(68))?.unwrap_or(Value::Null);
    let cache = cache::binding::query(
        backend,
        run,
        None,
        &json!(68),
        &json!([request["source_registration"], request["review_successor"]]),
    )?;
    model(
        "source",
        json!({"source":source,"directory":run,"request":request,"rebound":rebound,
        "storage":storage,"cache":cache,"cache_record":tail[tail.len()-3]}),
    )?;
    guards::rebind::live::transition(
        backend,
        &json!({"original":rebound["current_storage"],"current":storage}),
    )?;
    source["storage"]["storage"] = storage;
    source["source_rebind"] = request["source_rebind"].clone();
    Ok(source)
}

pub(super) fn controller(run: &Path, attempt: &Value) -> Result<(Value, Value)> {
    let mut running = attempt.clone();
    running["status"] = json!("running");
    running["finished_at"] = Value::Null;
    running["error_type"] = Value::Null;
    let number = attempt["number"].as_u64().ok_or("历史控制阶段序号无效")?;
    let (binding, owner) = state::controller_record(run, number, &running)?;
    document::exact(
        &owner,
        &["format_version", "identity", "directory", "manifest_sha256"],
        "历史控制器所有权",
    )?;
    common::validate_process_identity(&owner["identity"])?;
    if owner["format_version"].as_u64() != Some(1)
        || owner["directory"] != json!(run)
        || owner["manifest_sha256"] != common::binding(&run.join("manifest.json"))?["sha256"]
    {
        return Err("历史控制器不属于固定复制运行".into());
    }
    Ok((binding, owner))
}

pub(super) fn summary(files: &[Value]) -> Result<Value> {
    let bytes = files.iter().try_fold(0_u64, |sum, row| {
        sum.checked_add(row["bytes"].as_u64().ok_or("证据文件字节数无效")?)
            .ok_or("证据文件字节数溢出")
    })?;
    Ok(json!({"files":files.len(),"bytes":bytes,"sha256":digest::plan_hash(&json!(files))?}))
}
