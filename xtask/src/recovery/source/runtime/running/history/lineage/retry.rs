//! C69 已知失败只读采用；未知输出、重复后继或不完整前像均不生成授权。
use super::{
    super::{super::builds, completed, resolution},
    failure,
};
use crate::recovery::source::evidence::{inventory, staging};
use crate::{
    Result,
    recovery::clone::native::{json as document, lifecycle::state, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;

const HEAD: &str = "e57703a1ccd1282d703ec9ee79f22c235093d4eb";
const TREE: &str = "dfd6ecf6b041f51a5c3ea0f0e9d286f38349b9e4";

pub(super) fn authority(backend: &Path, run: &Path, attempts: &[Value]) -> Result<Option<Value>> {
    let Some((prefix, record, previous)) = incident(attempts)? else {
        return Ok(None);
    };
    let before = state_evidence(run, prefix)?;
    let request = completed::fixed(backend)?;
    let request_path = Path::new(document::text(&request["path"], "C69 固定请求路径")?);
    let facts = failure::inspect(backend, run, prefix, Some(request_path))?;
    if facts["failed"] != *previous {
        return Err("C69 失败授权没有绑定固定 C68".into());
    }
    staging::validate_source(&record["sources"])?;
    if record["sources"]["snapshot"]["head"] != HEAD
        || builds::git_tree(backend, HEAD)? != TREE
        || record["sources"]["snapshot"]["files"] != json!([])
        || record["sources"]["snapshot"]["patch_sha256"] != crate::contract::sha256_hex(b"")
        || record["sources"]["fingerprints"]["product"]
            != previous["sources"]["fingerprints"]["product"]
    {
        return Err("C69 失败授权改变了产品来源或不是固定干净提交".into());
    }
    let (controller, owner) = failure::controller(run, record)?;
    common::require_recorded_stopped(&owner["identity"])?;
    let failure = common::binding(&run.join("failure-0069.json"))?;
    let raw = common::read_bound(backend, &failure, "C69 原失败")?;
    if raw != expected_failure(&controller) {
        return Err("C69 失败栈不是完整后像后的固定环境复核位置".into());
    }
    absent(backend, run)?;
    let output = run.join("seed-runtime/attempt-0069");
    let runtime = common::binding(&output.join("runtime/runtime.json"))?;
    let after = common::binding(&output.join("after/image.json"))?;
    let manifest = json!({"files":10231,"bytes":1081072371_u64,
        "sha256":"fe2100c6190188e5a66ada28f8ce0359786c798aa4ea8a75aab43152b26d8635"});
    let observed = super::output(backend, run, &facts, &manifest, &runtime, &after)?;
    let repeated = failure::inspect(backend, run, prefix, Some(request_path))?;
    for field in [
        "proof",
        "failed",
        "request",
        "historical_request",
        "request_descriptor",
        "source",
        "contract",
        "before",
        "image",
        "records",
    ] {
        if repeated[field] != facts[field] {
            return Err("C69 核验期间 C68 失败、来源或完整前像变化".into());
        }
    }
    let proof = json!({"status":"seed_source_generation_replay_recovered","request":request,
        "failed_start":facts["failed"],"failed_authorization":record,"failure":failure,"controller":controller,
        "runtime":runtime,"after":after,"evidence_manifest":manifest,"remote_writes":0,"restore_qualified":false});
    if state_evidence(run, prefix)? != before
        || completed::fixed(backend)? != request
        || self::failure::controller(run, record)? != (controller, owner)
        || failure::summary(&inventory::manifest(&output)?)? != manifest
    {
        return Err("C69 失败授权的账本、请求或完整证据在核验期间变化".into());
    }
    for binding in [&failure, &runtime, &after] {
        common::bound_file(backend, binding, "C69 原始证据")?;
    }
    absent(backend, run)?;
    Ok(Some(
        json!({"record":record,"failed":facts["failed"],"records":[facts["failed"],record],
        "request":request,"receipt":null,"image":observed["image"],"failure":facts,"recovered_failure":proof}),
    ))
}

fn incident(attempts: &[Value]) -> Result<Option<(&[Value], &Value, &Value)>> {
    let matches = attempts
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row["number"].as_u64() == Some(69)
                && row["stage"] == "seed-runtime"
                && row["mode"] == "source-generation-recover"
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(None);
    }
    let [(index, record)] = matches.as_slice() else {
        return Err("C69 谱系恢复不能重复".into());
    };
    if record["status"] != "failed"
        || !record["result"].is_null()
        || record["error_type"] != "ValueError"
    {
        return Ok(None);
    }
    let previous = index
        .checked_sub(1)
        .and_then(|position| attempts.get(position))
        .ok_or("C69 失败授权缺少相邻 C68")?;
    if previous["number"].as_u64() != Some(68) {
        return Err("C69 失败授权不是 C68 后的唯一相邻阶段".into());
    }
    let suffix = &attempts[index + 1..];
    if !suffix.is_empty() && (suffix.len() != 1 || !pending(&suffix[0])) {
        return Err("C69 后只允许唯一未收尾的 C70 START".into());
    }
    Ok(Some((&attempts[..index + 1], record, previous)))
}

fn pending(row: &Value) -> bool {
    row["number"].as_u64() == Some(70)
        && row["stage"] == "seed-runtime"
        && row["mode"] == "source-generation-start"
        && row["status"] == "running"
        && row["finished_at"].is_null()
        && row["result"].is_null()
        && row["error_type"].is_null()
}

fn state_evidence(run: &Path, prefix: &[Value]) -> Result<Value> {
    let binding = common::binding(&run.join("state.json"))?;
    let value = state::load(run, true)?;
    let rows = state::attempts(&value)?;
    if rows != prefix {
        if rows.len() != prefix.len() + 1
            || &rows[..prefix.len()] != prefix
            || !pending(&rows[rows.len() - 1])
        {
            return Err("C69 只读授权没有绑定当前账本或相邻 C70 START".into());
        }
        resolution::owner(run)?;
    }
    Ok(json!({"binding":binding,"state":value}))
}

fn absent(backend: &Path, run: &Path) -> Result<()> {
    for path in [run.join("results/0069.json"), run.join("g0069")] {
        failure::inputs::missing(backend, &path)?;
    }
    Ok(())
}

fn expected_failure(controller: &Value) -> Value {
    let frames=[("devex_clone_run","execute",717),("devex_clone_seed_runtime","execute_seed",341),
        ("devex_clone_seed_generation_control","execute_recover",220),("devex_clone_seed_generation_lineage_recovery","authorize",475),
        ("devex_clone_seed_generation_lineage_recovery","lineage_failure",384),("devex_clone_seed_generation_lineage_recovery","_verify_request",280),
        ("devex_clone_seed_generation_runtime","registered_inputs",63),("devex_provenance","verify_source",174),
        ("source_fingerprints","reusable_artifact_source",349)].iter().map(|(file,function,line)|
        json!({"file":format!("scripts/{file}.py"),"function":function,"line":line})).collect::<Vec<_>>();
    json!({"format_version":1,"kind":"devex-stage-failure","attempt":69,"stage":"seed-runtime",
        "mode":"source-generation-recover","error_type":"ValueError","frames":frames,"controller":controller})
}
