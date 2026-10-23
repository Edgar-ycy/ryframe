//! 失败栈、启动意图和零写入恢复收据；所有值来自已绑定采集。
use super::*;

pub(super) fn failure(args: &Value) -> Result<Value> {
    document::exact(args, &["failure", "controller"], "谱系固定失败栈")?;
    let frames: Vec<_> = [
        ("devex_clone_run", "execute", 717),
        ("devex_clone_seed_runtime", "execute_seed", 341),
        ("devex_clone_seed_generation", "execute_generation", 369),
        ("restore_source_lineage", "derive_dataset_lineage", 405),
        ("restore_source_lineage", "_dataset_facts", 107),
        ("restore_reference_plan", "validate_plan", 68),
    ].iter().map(|(file,function,line)| json!({"file":format!("scripts/{file}.py"),"function":function,"line":line})).collect();
    let expected = json!({"format_version":1,"kind":"devex-stage-failure","attempt":FAILED,
        "stage":"seed-runtime","mode":"source-generation-start","error_type":"ValueError",
        "frames":frames,"controller":args["controller"]});
    if args["failure"] != expected {
        return Err("C68 失败栈没有证明 runtime.start 前的固定谱系校验位置".into());
    }
    Ok(json!({"verified":true}))
}

pub(super) fn intent(args: &Value) -> Result<Value> {
    document::exact(args, &["intent", "request", "runtime"], "谱系启动意图")?;
    let intent = &args["intent"];
    document::exact(
        intent,
        &[
            "format_version",
            "kind",
            "request",
            "runtime_directory",
            "operations",
        ],
        "C68 启动意图",
    )?;
    let operations = document::exact(
        &intent["operations"],
        &["api", "worker"],
        "C68 双角色 operation",
    )?;
    if intent["format_version"].as_u64() != Some(1)
        || intent["kind"] != "seed-source-runtime-intent"
        || intent["request"] != args["request"]
        || intent["runtime_directory"] != args["runtime"]
        || operations
            .values()
            .any(|v| v.as_str().is_none_or(|s| s.chars().count() != 32))
        || operations["api"] == operations["worker"]
    {
        return Err("C68 启动意图与固定请求、目录或双角色 operation 不同".into());
    }
    Ok(json!({"operations":operations}))
}

pub(super) fn receipt(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "facts",
            "prefix",
            "coordinator",
            "runtime",
            "after",
            "manifest",
        ],
        "谱系恢复收据构造",
    )?;
    let facts = &args["facts"];
    let request = &facts["request"];
    let prefix = document::array(&args["prefix"], "谱系恢复完整前缀")?;
    Ok(json!({"receipt":{
        "status":STATUS,"request":facts["request_descriptor"],"failed_start":FAILED,
        "failure_proof":facts["proof"],"history_length":prefix.len(),"history_sha256":digest::plan_hash(&args["prefix"])?,
        "coordinator_source":args["coordinator"],"source_registration":request["source_registration"],
        "source_rebind":request["source_rebind"],"review_successor":request["review_successor"],"current_storage":request["current_storage"],
        "runtime":args["runtime"],"before":facts["before"],"after":args["after"],"evidence_manifest":args["manifest"],
        "source_generation_published":false,"replay_allowed":true,"remote_writes":0,"restore_qualified":false
    }}))
}

pub(super) fn verify(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &["receipt", "facts", "prefix", "record"],
        "谱系恢复收据核验",
    )?;
    let value = &args["receipt"];
    super::super::super::lineage_receipt(value)?;
    let record = &args["record"];
    let expected = receipt(
        &json!({"facts":args["facts"],"prefix":args["prefix"],"coordinator":record["sources"],
        "runtime":value["runtime"],"after":value["after"],"manifest":value["evidence_manifest"]}),
    )?;
    if value != &expected["receipt"]
        || record["sources"]["fingerprints"]["product"]
            != args["facts"]["failed"]["sources"]["fingerprints"]["product"]
    {
        return Err("C69 谱系恢复没有绑定 C68、完整历史或同一产品来源".into());
    }
    Ok(json!({"verified":true}))
}
