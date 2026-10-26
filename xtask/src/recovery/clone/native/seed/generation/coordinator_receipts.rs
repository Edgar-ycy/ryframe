//! START、STOP 和有效来源请求的统一模型；不发布或修改外层证据。
use super::{RECOVER, STOP, attempts, matches_source, prefix_matches, reject, request};
use crate::{Result, recovery::clone::native::json as document};
use serde_json::{Value, json};

pub(crate) const START_FIELDS: &[&str] = &[
    "status",
    "request",
    "source_registration",
    "source_rebind",
    "review_successor",
    "current_storage",
    "history_length",
    "history_sha256",
    "generation_id",
    "intent",
    "runtime_evidence",
    "before",
    "running",
    "dataset_lineage",
    "coordinator_source",
    "remote_writes",
    "restore_qualified",
];
const RESULT_FIELDS: &[&str] = &[
    "status",
    "request",
    "source_registration",
    "source_rebind",
    "review_successor",
    "current_storage",
    "history_length",
    "history_sha256",
    "source_request",
    "generation_verified",
    "runtime_evidence",
    "before",
    "after",
    "remote_writes",
    "restore_qualified",
    "dataset_lineage",
    "start",
    "source_runtime",
    "running",
    "stop_before",
];

pub(super) fn source_request(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "source_request",
            "request",
            "fingerprint",
            "runtime",
            "processes",
            "producers_registry",
            "runtime_dir",
        ],
        "后继来源请求",
    )?;
    document::object(&args["source_request"], "原始来源请求")?;
    request(&args["request"])?;
    let mut value = args["source_request"].clone();
    for key in ["id", "backend_build", "maintenance_build"] {
        value[key] = args["request"][key].clone();
    }
    value["worktree_fingerprint"] = args["fingerprint"].clone();
    for key in ["runtime", "processes", "producers_registry"] {
        value[key] = args[key].clone();
    }
    document::object(&value["source"], "来源物理目标")?;
    value["source"]["runtime_dir"] = args["runtime_dir"].clone();
    Ok(value)
}

pub(super) fn running(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "receipt",
            "request",
            "source",
            "prefix",
            "coordinator_source",
            "generation_id",
            "request_copy",
            "attempts",
            "number",
            "live",
        ],
        "运行中生成收据",
    )?;
    let value = &args["receipt"];
    document::exact(value, START_FIELDS, "运行中生成收据")?;
    request(&args["request"])?;
    let live = args["live"].as_bool().ok_or("在线校验必须为布尔值")?;
    let number = args["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("START 序号无效")?;
    if live
        && attempts(&args["attempts"])?.iter().any(|row| {
            row["number"].as_u64().is_some_and(|n| n > number)
                && row["stage"] == "seed-runtime"
                && matches!(row["mode"].as_str(), Some(STOP | RECOVER))
                && row["status"] != "running"
        })
    {
        return Ok(reject("stopped"));
    }
    if value["status"] != "seed_source_generation_running"
        || value["generation_id"] != args["generation_id"]
        || value["coordinator_source"] != args["coordinator_source"]
        || !prefix_matches(value, &args["prefix"])?
        || !matches_source(value, &args["source"])
        || args["request"] != args["request_copy"]
        || [
            "source_registration",
            "source_rebind",
            "review_successor",
            "current_storage",
        ]
        .iter()
        .any(|key| args["request"][key] != value[key])
    {
        return Ok(reject("running"));
    }
    Ok(json!({"verified":true}))
}

pub(super) fn start(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "request",
            "descriptor",
            "coordinator_source",
            "prefix",
            "generation_id",
            "bindings",
        ],
        "START 发布模型",
    )?;
    request(&args["request"])?;
    let prefix = attempts(&args["prefix"])?;
    let bindings = document::exact(
        &args["bindings"],
        &[
            "intent",
            "runtime_evidence",
            "before",
            "running",
            "dataset_lineage",
        ],
        "START 文件证据",
    )?;
    let mut value = json!({"status":"seed_source_generation_running", "request":args["descriptor"],
        "coordinator_source":args["coordinator_source"],"history_length":prefix.len(),
        "history_sha256":crate::recovery::clone::native::digest::plan_hash(&args["prefix"] )?,
        "generation_id":args["generation_id"],"remote_writes":0,"restore_qualified":false});
    for key in [
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
    ] {
        value[key] = args["request"][key].clone();
    }
    for (key, binding) in bindings {
        value[key] = binding.clone();
    }
    Ok(value)
}

pub(super) fn published(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "receipt",
            "request",
            "source",
            "descriptor",
            "prefix",
            "source_receipt",
            "verified_receipt",
            "verified_source",
            "before",
            "running",
        ],
        "STOP 发布收据",
    )?;
    let value = &args["receipt"];
    document::exact(value, RESULT_FIELDS, "STOP 发布收据")?;
    // 请求已由 source-runtime 事实边界核验；此处只校验本收据消费的绑定。
    document::object(&args["request"], "已验证的来源请求")?;
    let source = document::object(&args["source"], "原 dataset 来源")?;
    if value["request"] != args["source_receipt"]["request"]
        || value["dataset_lineage"] != args["source_receipt"]["dataset_lineage"]
        || source
            .iter()
            .any(|(key, item)| args["verified_source"].get(key) != Some(item))
    {
        return Ok(reject("dataset"));
    }
    if value["status"] != "seed_source_generation_published"
        || value["source_registration"] != args["descriptor"]
        || value["source_rebind"] != args["source"]["source_rebind"]
        || value["current_storage"] != args["source"]["storage"]["storage"]
        || !prefix_matches(value, &args["prefix"])?
        || [
            "source_registration",
            "source_rebind",
            "review_successor",
            "current_storage",
        ]
        .iter()
        .any(|key| value[key] != args["request"][key])
    {
        return Ok(reject("published"));
    }
    if args["verified_receipt"]["source_generation"] != value["start"]
        || args["verified_receipt"]["dataset_lineage"] != value["dataset_lineage"]
        || value["before"] != args["before"]
        || value["running"] != args["running"]
    {
        return Ok(reject("lineage"));
    }
    Ok(json!({"verified":true}))
}

pub(super) fn generation_proof(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "effective",
            "runtime",
            "source_snapshot",
            "maintenance",
            "processes",
            "producers",
            "physical_binding",
            "generation",
            "runtime_physical",
        ],
        "后继 generation 证明",
    )?;
    let mut producers = args["producers"]
        .as_array()
        .ok_or("生产者必须为数组")?
        .clone();
    for producer in &producers {
        document::text(&producer["name"], "生产者名称")?;
    }
    producers.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let expected = json!({"request_sha256":crate::recovery::clone::native::digest::plan_hash(&args["effective"] )?,
        "runtime":args["runtime"],"source":args["source_snapshot"],
        "worktree_fingerprint":args["effective"]["worktree_fingerprint"],"maintenance":args["maintenance"],
        "processes":args["processes"],"operator_declared_producers":producers,"physical_binding":args["physical_binding"],
        "external_writers_discovered":false,"clone_verified":false,"restore_qualified":false});
    if args["generation"] != expected || args["runtime_physical"] != args["physical_binding"] {
        return Ok(reject("proof"));
    }
    Ok(json!({"verified":true}))
}
