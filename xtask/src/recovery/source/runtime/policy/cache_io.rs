//! Redis 适配器只采集实际响应；计划、版本、快照和原子命令由原生合同决定。
use super::super::super::cache;
use super::{documents, model};
use crate::Result;
use serde_json::{Value, json};

pub(super) fn keys(args: &Value) -> Result<Value> {
    documents::exact(args, &["plan"], "来源缓存键参数")?;
    Ok(json!(cache::keys(&args["plan"])?))
}

pub(super) fn read_plan(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["subject", "types", "tenant_epoch", "user_version"],
        "来源缓存读取参数",
    )?;
    let types = &args["types"];
    documents::exact(
        types,
        &["tenant_epoch", "user_version", "snapshot"],
        "来源缓存类型",
    )?;
    if types["tenant_epoch"] != "string"
        || types["user_version"] != "string"
        || !matches!(types["snapshot"].as_str(), Some("hash" | "none"))
    {
        return Err("来源授权缓存持久键缺失或快照类型无效".into());
    }
    let epoch = documents::text(&args["tenant_epoch"], "来源缓存代次")?;
    let version = args["subject"]["user_authorization_version"]
        .as_u64()
        .ok_or("来源授权版本无效")?;
    if !regex::Regex::new(r"^[1-9][0-9]{0,9}$")?.is_match(epoch)
        || args["user_version"].as_str() != Some(version.to_string().as_str())
    {
        return Err("来源授权缓存持久版本不同".into());
    }
    Ok(json!({"field":format!("{epoch}:{version}"),"snapshot_type":types["snapshot"]}))
}

pub(super) fn read_plans(args: &Value) -> Result<Value> {
    documents::exact(args, &["plan", "raw"], "来源缓存批量读取参数")?;
    let plan = documents::array(&args["plan"], "来源缓存计划")?;
    let raw = documents::array(&args["raw"], "来源缓存响应")?;
    if plan.len() != 11 || raw.len() != plan.len() {
        return Err("来源授权缓存前像没有完整主体集合".into());
    }
    plan.iter()
        .zip(raw)
        .map(|(subject, row)| {
            documents::exact(
                row,
                &["types", "tenant_epoch", "user_version"],
                "来源缓存版本响应",
            )?;
            read_plan(&json!({"subject":subject,"types":row["types"],
            "tenant_epoch":row["tenant_epoch"],"user_version":row["user_version"]}))
        })
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

fn observed_subject(subject: &Value, raw: &Value) -> Result<(Value, Vec<Value>)> {
    documents::exact(
        raw,
        &["types", "tenant_epoch", "user_version", "fields", "payload"],
        "来源缓存采集",
    )?;
    let selected = read_plan(&json!({"subject":subject,"types":raw["types"],
        "tenant_epoch":raw["tenant_epoch"],"user_version":raw["user_version"]}))?;
    let mut snapshot = json!({"key":subject["keys"]["snapshot"],"type":selected["snapshot_type"]});
    let mut field = json!("");
    let mut payload = json!("");
    if selected["snapshot_type"] == "hash" {
        if raw["fields"] != json!([selected["field"]]) {
            return Err("来源授权缓存快照字段不同".into());
        }
        field = selected["field"].clone();
        payload = raw["payload"].clone();
        let identity = model::snapshot(
            &json!({"payload":payload,"subject":subject,"tenant_epoch":raw["tenant_epoch"]}),
        )?;
        snapshot["field"] = field.clone();
        snapshot["bytes"] = identity["bytes"].clone();
        snapshot["sha256"] = identity["sha256"].clone();
    } else if raw["fields"] != json!([]) || raw["payload"] != "" {
        return Err("来源已过期缓存不应包含快照响应".into());
    }
    let row = json!({"tenant_id":subject["tenant_id"],"user_id":subject["user_id"],
        "user_authorization_version":subject["user_authorization_version"],
        "tenant_epoch":{"key":subject["keys"]["tenant_epoch"],"value":raw["tenant_epoch"]},
        "user_version":{"key":subject["keys"]["user_version"],"value":raw["user_version"]},"snapshot":snapshot});
    Ok((
        row,
        vec![
            raw["tenant_epoch"].clone(),
            raw["user_version"].clone(),
            selected["snapshot_type"].clone(),
            field,
            payload,
        ],
    ))
}

pub(super) fn observation(args: &Value) -> Result<Value> {
    documents::exact(args, &["plan", "raw"], "来源缓存观察参数")?;
    let plan = documents::array(&args["plan"], "来源缓存计划")?;
    let raw = documents::array(&args["raw"], "来源缓存响应")?;
    if plan.len() != raw.len() || plan.len() != 11 {
        return Err("来源授权缓存前像没有完整主体集合".into());
    }
    let mut before = Vec::new();
    let mut comparisons = Vec::new();
    for (subject, actual) in plan.iter().zip(raw) {
        let (row, values) = observed_subject(subject, actual)?;
        before.push(row);
        comparisons.extend(values);
    }
    Ok(json!({"before":before,"comparisons":comparisons}))
}

pub(super) struct Comparison {
    pub(super) keys: Vec<String>,
    pub(super) values: Vec<String>,
    pub(super) expired: Vec<String>,
}

pub(super) fn comparison(args: &Value) -> Result<Comparison> {
    documents::exact(
        args,
        &["observation", "comparisons"],
        "来源缓存清理命令参数",
    )?;
    let observation = &args["observation"];
    cache::observation(&json!({"value":observation,"subjects":observation["subjects"]}))?;
    let comparisons = documents::array(&args["comparisons"], "来源缓存比较值")?;
    if comparisons.len() != 55 || comparisons.iter().any(|value| !value.is_string()) {
        return Err("来源缓存清理比较值不完整".into());
    }
    let rows = documents::array(&observation["before"], "来源缓存前像")?;
    for (row, values) in rows.iter().zip(comparisons.as_chunks::<5>().0.iter()) {
        if values[0] != row["tenant_epoch"]["value"]
            || values[1] != row["user_version"]["value"]
            || values[2] != row["snapshot"]["type"]
        {
            return Err("来源缓存清理比较值与冻结前像不同".into());
        }
        if values[2] == "hash" {
            let identity = model::snapshot(
                &json!({"payload":values[4],"subject":row,"tenant_epoch":values[0]}),
            )?;
            if values[3] != row["snapshot"]["field"]
                || identity["bytes"] != row["snapshot"]["bytes"]
                || identity["sha256"] != row["snapshot"]["sha256"]
            {
                return Err("来源缓存清理快照与冻结前像不同".into());
            }
        } else if values[3] != "" || values[4] != "" {
            return Err("来源缓存清理过期快照比较值无效".into());
        }
    }
    let keys = cache::keys(&observation["plan"])?;
    let keys = keys
        .iter()
        .map(|key| documents::text(key, "来源缓存键").map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let values = comparisons
        .iter()
        .map(|value| documents::text(value, "来源缓存比较值").map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let expired = rows
        .iter()
        .filter(|row| row["snapshot"]["type"] == "none")
        .map(|row| documents::text(&row["snapshot"]["key"], "来源过期快照").map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    Ok(Comparison {
        keys,
        values,
        expired,
    })
}
