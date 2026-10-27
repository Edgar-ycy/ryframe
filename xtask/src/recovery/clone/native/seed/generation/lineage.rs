//! C68 失败封存与唯一相邻恢复的模型；只读判定不执行资源写入。
use crate::{
    Result,
    recovery::clone::native::{digest, json as document},
};
use serde_json::{Value, json};

#[path = "lineage_history.rs"]
mod history;
#[path = "lineage_receipts.rs"]
mod receipts;
#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/generation/lineage_tests.rs"]
mod tests;

const FAILED: u64 = 68;
const RECOVERY: u64 = 69;
const RETRY: u64 = 70;
const STATUS: &str = "seed_source_generation_replay_authorized";

pub(super) fn dispatch(value: &Value) -> Result<Value> {
    document::exact(value, &["action", "input"], "谱系恢复模型")?;
    let args = &value["input"];
    let result = match value["action"].as_str() {
        Some("pending") => history::pending(args),
        Some("history") => history::history(args),
        Some("select-source") => history::select_source(args),
        Some("source") => source(args),
        Some("request") => request(args),
        Some("reregister-request") => reregister_request(args),
        Some("request-source") => request_source(args),
        Some("failure") => receipts::failure(args),
        Some("intent") => receipts::intent(args),
        Some("authorize-stage") => history::authorize_stage(args),
        Some("authorize-product") => history::authorize_product(args),
        Some("replay-plan") => history::replay_plan(args),
        Some("receipt") => receipts::receipt(args),
        Some("verify-receipt") => receipts::verify(args),
        _ => return Err("未知谱系恢复模型操作".into()),
    };
    // 上述纯模型只生成固定错误文案，不回显请求、配置或底层异常。
    Ok(result.unwrap_or_else(|error| json!({"rejected":error.to_string()})))
}

fn reregister_request(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "previous",
            "unchanged",
            "backend_build",
            "maintenance_build",
        ],
        "谱系请求重新登记",
    )?;
    let previous = document::object(&args["previous"], "谱系原请求")?;
    let mut frozen = vec![
        "id",
        "review_successor",
        "execution_backend",
        "expected_backend_sha",
        "adapter_contract",
        "product_backend",
        "source_environment",
    ];
    if args["previous"]["format_version"].as_u64() == Some(2) {
        frozen.extend(["configuration", "execution_source"]);
    }
    let unchanged = document::exact(&args["unchanged"], &frozen, "谱系冻结字段")?;
    if unchanged
        .iter()
        .any(|(field, value)| previous.get(field) != Some(value))
    {
        return Err("C69 重新登记请求改变了构建收据以外的字段".into());
    }
    if args["backend_build"] == args["previous"]["backend_build"]
        || args["maintenance_build"] == args["previous"]["maintenance_build"]
    {
        return Err("C69 必须成对重新登记新的 API/Worker 与维护构建收据".into());
    }
    let mut request = args["previous"].clone();
    request["backend_build"] = args["backend_build"].clone();
    request["maintenance_build"] = args["maintenance_build"].clone();
    Ok(json!({"request":request}))
}

fn source(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "source",
            "directory",
            "request",
            "rebound",
            "storage",
            "cache",
            "cache_record",
        ],
        "谱系来源模型",
    )?;
    let source = &args["source"];
    let request = &args["request"];
    let rebound = &args["rebound"];
    if rebound["review_successor"] != request["review_successor"] {
        return Err("C68 重绑定没有冻结相同 C52、successor 与完整历史".into());
    }
    if source["directory"] != args["directory"]
        || source["review_successor"]["source_result"] != request["source_registration"]
        || source["review_successor_binding"] != request["review_successor"]
        || rebound["original_storage"] != source["storage"]["storage"]
    {
        return Err("C68 请求与原 C52 或 successor 不同".into());
    }
    let storage = &args["storage"];
    let cache = &args["cache"];
    if storage.is_null()
        || storage != &request["current_storage"]
        || storage["attempt"].as_u64() != Some(65)
        || cache.is_null()
        || cache["attempt"].as_u64() != Some(66)
        || cache["attempt"] != args["cache_record"]["number"]
        || cache["restart_result"] != args["cache_record"]["result"]
    {
        return Err("C68 请求没有绑定固定 C65 RustFS 与 C66 Redis 代次".into());
    }
    Ok(json!({"verified":true}))
}

fn request(args: &Value) -> Result<Value> {
    document::exact(args, &["previous", "request", "external"], "谱系请求比较")?;
    let request = document::object(&args["request"], "谱系当前请求")?;
    let previous = document::object(&args["previous"], "谱系内部请求")?;
    match (
        args["request"]["format_version"].as_u64(),
        args["previous"]["format_version"].as_u64(),
    ) {
        (Some(1), Some(1)) if args["request"]["kind"] == "devex-clone-seed-source-generation" => (),
        (Some(2), Some(2)) => {
            super::request(&args["request"])?;
            super::request(&args["previous"])?;
        }
        _ => return Err("C68 谱系恢复请求与失败代次不同".into()),
    }
    let external = document::boolean(&args["external"], "外部请求标志")?;
    let changes: Vec<_> = request
        .iter()
        .filter(|(key, value)| previous.get(*key) != Some(*value))
        .map(|(key, _)| key.as_str())
        .collect();
    if request.keys().ne(previous.keys())
        || external && changes != ["backend_build", "maintenance_build"]
    {
        return Err("C69 当前请求必须成对重新登记 API/Worker 与维护构建收据".into());
    }
    if !external && !changes.is_empty() {
        return Err("C68 内部请求在读取期间变化".into());
    }
    Ok(json!({"reregister":external}))
}

fn request_source(args: &Value) -> Result<Value> {
    document::exact(args, &["request", "source"], "谱系正式来源请求")?;
    let request = &args["request"];
    let source = &args["source"];
    super::request(request)?;
    if request["source_registration"] != source["review_successor"]["source_result"]
        || request["review_successor"] != source["review_successor_binding"]
        || request["source_rebind"] != source["source_rebind"]
        || request["current_storage"] != source["storage"]["storage"]
        || !source["source_generation"].is_null()
    {
        return Err("C68 谱系恢复请求不是重绑定后的首个正式生成请求".into());
    }
    Ok(json!({"verified":true}))
}
