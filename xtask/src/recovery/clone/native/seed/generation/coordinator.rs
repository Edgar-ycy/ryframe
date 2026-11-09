//! 生成主编排的纯状态与收据规则；进程和服务采集仍由受锁适配器执行。
use crate::{
    Result,
    recovery::clone::native::{digest, input, json as document},
};
use serde_json::{Value, json};

#[path = "lineage.rs"]
mod lineage;
#[path = "coordinator_receipts.rs"]
pub(crate) mod receipts;
#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/generation/coordinator_tests.rs"]
mod tests;

pub(super) const START: &str = "source-generation-start";
pub(super) const STOP: &str = "source-generation-stop";
pub(super) const RECOVER: &str = "source-generation-recover";
const REQUEST: &[&str] = &[
    "format_version",
    "kind",
    "id",
    "source_registration",
    "review_successor",
    "source_rebind",
    "current_storage",
    "execution_backend",
    "expected_backend_sha",
    "adapter_contract",
    "product_backend",
    "backend_build",
    "maintenance_build",
    "source_environment",
];

pub(crate) fn dispatch(value: &Value) -> Result<Value> {
    document::exact(value, &["action", "input"], "生成编排模型")?;
    let args = &value["input"];
    match value["action"].as_str() {
        Some("request-schema") => {
            document::exact(args, &["request"], "来源请求schema参数")?;
            request(&args["request"])?;
            Ok(json!({"verified":true}))
        }
        Some("lineage") => lineage::dispatch(args),
        Some("preflight-plan") => preflight_plan(args),
        Some("preflight-finish") => preflight_finish(args),
        Some("inputs") => inputs(args),
        Some("input-stage") => input_stage(args),
        Some("select-rebind" | "select-start" | "select-published") => {
            select(value["action"].as_str().ok_or("生成选择操作无效")?, args)
        }
        Some("source-request") => receipts::source_request(args),
        Some("running") => receipts::running(args),
        Some("start-receipt") => receipts::start(args),
        Some("published") => receipts::published(args),
        Some("generation-proof") => receipts::generation_proof(args),
        Some("equal-images") => {
            document::exact(args, &["before", "after"], "生成完整像比较")?;
            document::object(&args["before"], "生成完整前像")?;
            document::object(&args["after"], "生成完整后像")?;
            Ok(json!({"equal":args["before"] == args["after"]}))
        }
        _ => Err("未知生成编排模型操作".into()),
    }
}

fn reject(reason: &str) -> Value {
    // 文案只由固定分类产生，不回显输入、配置内容或底层异常。
    let message = match reason {
        "replay" => "source-generation 只执行一次；失败必须核对完整前后像，不能重放启动",
        "resume" => "source-generation 失败未形成资源完整、零输出且唯一相邻的续作边界",
        "replay_request" => "C70 必须在登记阶段前绑定 C69 授权的同一正式请求",
        "rebind" => "source-generation 必须继承唯一已发布 storage rebind",
        "current" => "source-generation 不是同一账本的当前持锁阶段",
        "source" => "source-generation 没有精确继承同一 C52、successor 与当前存储",
        "stopped" => "源运行代次已执行停止或恢复，不能再次验证或重放",
        "running" => "运行中源未冻结同一请求、C52、存储或完整前缀",
        "dataset" => "source-generation 请求或原 dataset/C29 来源与同一 start 不同",
        "published" => "source-generation 未冻结同一 C52、重绑定和完整历史前缀",
        "lineage" => "源验证收据未绑定同一 start、原始前像和 dataset lineage",
        "proof" => "后继 generation 证明与有效请求、源码或 runtime 不同",
        "start" => "source verify 必须绑定同一账本唯一成功 start 的外层收据",
        "duplicate" => "source-generation 存在重复或失败重放",
        _ => "生成编排模型拒绝结果无效；禁止自动重放",
    };
    json!({"rejected":message})
}

fn select(action: &str, args: &Value) -> Result<Value> {
    document::exact(args, &["attempts"], "生成阶段选择")?;
    let values = attempts(&args["attempts"])?;
    let mode = match action {
        "select-rebind" => "source-rebind",
        "select-start" => START,
        _ => STOP,
    };
    let records: Vec<_> = values
        .iter()
        .filter(|row| row["stage"] == "seed-runtime" && row["mode"] == mode)
        .collect();
    if action == "select-published" && !records.iter().any(|row| row["status"] == "passed") {
        return Ok(json!({"record":null,"prefix":[]}));
    }
    if records.len() != 1 || records[0]["status"] != "passed" {
        return Ok(reject(match action {
            "select-rebind" => "rebind",
            "select-start" => "start",
            _ => "duplicate",
        }));
    }
    let record = records[0];
    let prefix: Vec<_> = values
        .iter()
        .take_while(|row| row["number"] != record["number"])
        .collect();
    Ok(json!({"record":record,"prefix":prefix}))
}

fn attempts(value: &Value) -> Result<&Vec<Value>> {
    let values = value.as_array().ok_or("生成历史必须为数组")?;
    let mut previous = 0;
    for record in values {
        let number = record["number"]
            .as_u64()
            .filter(|n| *n > previous)
            .ok_or("生成历史序号必须严格递增")?;
        previous = number;
        if record["stage"].as_str().is_none()
            || record["mode"].as_str().is_none()
            || !matches!(
                record["status"].as_str(),
                Some("passed" | "failed" | "running")
            )
        {
            return Err("生成历史阶段、模式或状态无效".into());
        }
    }
    Ok(values)
}

fn remaining<'a>(values: &'a [Value], ignored: &[Value]) -> Vec<&'a Value> {
    values
        .iter()
        .filter(|row| {
            row["stage"] == "seed-runtime"
                && matches!(
                    row["mode"].as_str(),
                    Some(START | STOP | RECOVER | "source-export" | "arm-input")
                )
                && !ignored.contains(row)
        })
        .collect()
}

fn preflight_plan(args: &Value) -> Result<Value> {
    document::exact(args, &["attempts", "archive"], "生成预检计划")?;
    let values = attempts(&args["attempts"])?;
    let archive = &args["archive"];
    let ignored = if archive.is_null() {
        &[][..]
    } else {
        archive["records"]
            .as_array()
            .ok_or("封存阶段集合无效")?
            .as_slice()
    };
    let pending = remaining(values, ignored);
    if !archive.is_null() && !pending.is_empty() {
        if pending[0]["mode"] != START
            || pending[0]["status"] != "failed"
            || pending.len() > 1 && archive.get("recovery").is_none()
        {
            return Ok(reject("replay"));
        }
        return Ok(json!({"needs_segment":true}));
    }
    if !pending.is_empty() {
        return Ok(reject("replay"));
    }
    Ok(json!({"needs_segment":false}))
}

fn preflight_finish(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &[
            "attempts",
            "archive",
            "segment",
            "request",
            "current_request",
        ],
        "生成续作边界",
    )?;
    let plan = preflight_plan(&json!({"attempts":args["attempts"],"archive":args["archive"]}))?;
    if plan.get("rejected").is_some() {
        return Ok(plan);
    }
    let values = attempts(&args["attempts"])?;
    let mut ignored = args["archive"]["records"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let segment = &args["segment"];
    if plan["needs_segment"] == true {
        if segment["phase"] != "ready" || segment["storage"].is_null() || segment["cache"].is_null()
        {
            return Ok(reject("resume"));
        }
        if !segment["replay"].is_null()
            && (args["request"].is_null()
                || args["request"] != segment["replay"]["request"]
                || args["current_request"] != args["request"])
        {
            return Ok(reject("replay_request"));
        }
        ignored.extend(
            segment["records"]
                .as_array()
                .ok_or("续作阶段集合无效")?
                .iter()
                .cloned(),
        );
    } else if !segment.is_null() {
        return Err("无续作计划却提交续作证据".into());
    }
    let pending = remaining(values, &ignored);
    if !pending.is_empty() {
        return Ok(reject(
            if !segment.is_null() && pending.len() == 1 && pending[0]["status"] == "failed" {
                "resume"
            } else {
                "replay"
            },
        ));
    }
    let rebound: Vec<_> = values
        .iter()
        .filter(|row| row["stage"] == "seed-runtime" && row["mode"] == "source-rebind")
        .collect();
    if rebound.len() != 1 || rebound[0]["status"] != "passed" {
        return Ok(reject("rebind"));
    }
    Ok(json!({"ready":true}))
}

pub(crate) fn request(value: &Value) -> Result<()> {
    let version = value["format_version"].as_u64();
    if version == Some(2) {
        let mut fields = REQUEST.to_vec();
        fields.extend(["configuration", "execution_source"]);
        document::exact(value, &fields, "source-generation v2 请求")?;
        document::exact(
            &value["configuration"],
            &["app_env", "directory", "files", "sha256"],
            "来源配置绑定",
        )?;
        if value["configuration"]["app_env"] != "test" {
            return Err("来源 v2 配置必须明确选择test".into());
        }
        if !value["execution_source"].is_null() {
            document::exact(
                &value["execution_source"],
                &["path", "bytes", "sha256"],
                "执行源码发布绑定",
            )?;
        }
    } else if version == Some(1) {
        document::exact(value, REQUEST, "历史 source-generation v1 请求")?;
    } else {
        return Err("source-generation 请求版本无效".into());
    }
    if value["kind"] != "devex-clone-seed-source-generation" {
        return Err("source-generation 请求类型无效".into());
    }
    input::name(document::text(&value["id"], "生成名称")?)?;
    Ok(())
}

pub(crate) fn current_request(value: &Value) -> Result<()> {
    request(value)?;
    if value["format_version"].as_u64() != Some(2) {
        return Err("新source-generation必须使用显式配置与来源绑定的v2请求；原v1历史只读验证及已登记资源安全收尾保持原合同".into());
    }
    Ok(())
}

fn inputs(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &["request", "source", "attempts", "number"],
        "生成当前输入",
    )?;
    let stage = input_stage(
        &json!({"request":args["request"],"attempts":args["attempts"],"number":args["number"]}),
    )?;
    if stage.get("rejected").is_some() {
        return Ok(stage);
    }
    if !args["source"]["source_generation"].is_null()
        || !matches_source(&args["request"], &args["source"])
    {
        return Ok(reject("source"));
    }
    Ok(stage)
}

fn input_stage(args: &Value) -> Result<Value> {
    document::exact(args, &["request", "attempts", "number"], "生成持锁阶段")?;
    current_request(&args["request"])?;
    let values = attempts(&args["attempts"])?;
    let number = args["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("生成当前序号无效")?;
    let last = values.last().ok_or("生成缺少当前阶段")?;
    if last["number"] != number
        || last["stage"] != "seed-runtime"
        || last["mode"] != START
        || last["status"] != "running"
    {
        return Ok(reject("current"));
    }
    Ok(json!({"prefix":&values[..values.len()-1]}))
}

fn matches_source(value: &Value, source: &Value) -> bool {
    value["source_registration"] == source["review_successor"]["source_result"]
        && value["source_rebind"] == source["source_rebind"]
        && value["review_successor"] == source["review_successor_binding"]
        && value["current_storage"] == source["storage"]["storage"]
}

fn prefix_matches(value: &Value, prefix: &Value) -> Result<bool> {
    let values = attempts(prefix)?;
    Ok(
        value["history_length"].as_u64() == Some(values.len() as u64)
            && value["history_sha256"] == digest::plan_hash(prefix)?
            && value["remote_writes"].as_u64() == Some(0)
            && value["restore_qualified"] == false,
    )
}
