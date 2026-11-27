//! 只有完整成功恢复收据才能封存原零启动失败；全部旧文件仍逐项读取。
use super::{number, origin, partial, proof, reregistration, revalidated};
use crate::recovery::source::evidence::images;
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document, resources::common, seed::generation::coordinator,
    },
};
use serde_json::{Value, json};
use std::path::Path;

const FIELDS: &[&str] = &[
    "status",
    "request",
    "failed_start",
    "failure_proof",
    "history_length",
    "history_sha256",
    "source_registration",
    "source_rebind",
    "review_successor",
    "current_storage",
    "coordinator_source",
    "runtime",
    "before",
    "after",
    "historical_image_compared",
    "remote_writes",
    "restore_qualified",
    "collection_failure",
];

pub(in crate::recovery::source::evidence) fn closed(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
) -> Result<Value> {
    let mut matches = Vec::new();
    for row in attempts.iter().filter(|row| {
        row["stage"] == "seed-runtime"
            && row["mode"] == "source-generation-recover"
            && row["status"] == "passed"
    }) {
        let value = common::read_bound(backend, &row["result"], "来源历史恢复结果")?;
        if is_closure(&value) {
            matches.push((row, value));
        }
    }
    match matches.as_slice() {
        [] => Ok(Value::Null),
        [(record, value)] if value["status"] != revalidated::STATUS => {
            read_original(backend, run, attempts, record, value)
        }
        [(first_record, first), (next_record, next)]
            if first["status"] == "seed_source_partial_prelaunch_closed"
                && next["status"] == revalidated::STATUS =>
        {
            let original = read_original(backend, run, attempts, first_record, first)?;
            read_revalidated(backend, run, attempts, next_record, next, &original)
        }
        _ => Err("原未启动尝试不能重复封存恢复，后继完整前像收口必须继承原封存".into()),
    }
}

fn is_closure(value: &Value) -> bool {
    matches!(
        value["status"].as_str(),
        Some("seed_source_prelaunch_closed" | "seed_source_partial_prelaunch_closed")
    ) || value["status"] == revalidated::STATUS
}

fn read_original(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    record: &Value,
    value: &Value,
) -> Result<Value> {
    document::exact(value, FIELDS, "原未启动封存收据")?;
    let prefix = attempts
        .iter()
        .take_while(|row| row["number"] != record["number"])
        .cloned()
        .collect::<Vec<_>>();
    let recovery_number = number(record)?;
    let (start, failure_proof, collection) =
        failure(backend, run, &prefix, recovery_number, value)?;
    if recovery_number
        != number(prefix.last().ok_or("未启动封存缺少原历史")?)?
            .checked_add(1)
            .ok_or("封存序号溢出")?
        || value["collection_failure"] != collection
        || value["failed_start"] != start["number"]
        || value["failure_proof"] != failure_proof
        || value["history_length"].as_u64() != Some(u64::try_from(prefix.len())?)
        || value["history_sha256"] != digest::plan_hash(&json!(prefix))?
        || value["coordinator_source"] != record["sources"]
        || value["historical_image_compared"] != false
        || value["remote_writes"].as_u64() != Some(0)
        || value["restore_qualified"] != false
        || common::bound_file(backend, &record["result"], "原封存结果")?
            != run.join(format!("results/{recovery_number:04}.json"))
    {
        return Err("原未启动封存没有绑定原失败及完整历史前缀".into());
    }
    frozen_request(backend, value, &prefix)?;
    let output = run.join(format!("seed-runtime/attempt-{recovery_number:04}"));
    if common::bound_file(backend, &value["runtime"], "原封存运行配置")?
        != output.join("runtime/runtime.json")
    {
        return Err("未启动封存配置不属于同一恢复 attempt".into());
    }
    let image = image(backend, &output, value, None)?;
    let index = prefix
        .iter()
        .position(|row| *row == start)
        .ok_or("封存原 START 不属于完整历史")?;
    let mut records = prefix[index..].to_vec();
    records.push(record.clone());
    Ok(json!({"start":start,"recovery":record,"records":records,"receipt":value,"image":image}))
}

fn read_revalidated(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    record: &Value,
    receipt: &Value,
    archive: &Value,
) -> Result<Value> {
    document::exact(receipt, FIELDS, "后继完整前像封存收据")?;
    let index = attempts
        .iter()
        .position(|row| row == record)
        .ok_or("后继封存不属于完整历史")?;
    let prefix = &attempts[..index];
    let recovery_number = number(record)?;
    let original = revalidated::load(
        backend,
        run,
        prefix,
        recovery_number,
        &receipt["request"],
        archive,
    )?;
    if receipt["failed_start"] != original.start["number"]
        || receipt["failure_proof"] != original.proof
        || receipt["before"] != original.before
        || !receipt["collection_failure"].is_null()
        || receipt["history_length"].as_u64() != Some(u64::try_from(prefix.len())?)
        || receipt["history_sha256"] != digest::plan_hash(&json!(prefix))?
        || receipt["coordinator_source"] != record["sources"]
        || receipt["historical_image_compared"] != true
        || receipt["remote_writes"].as_u64() != Some(0)
        || receipt["restore_qualified"] != false
        || common::bound_file(backend, &record["result"], "后继封存结果")?
            != run.join(format!("results/{recovery_number:04}.json"))
    {
        return Err("后继封存没有绑定原失败、完整前像和相邻历史".into());
    }
    frozen_request(backend, receipt, prefix)?;
    let output = run.join(format!("seed-runtime/attempt-{recovery_number:04}"));
    if common::bound_file(backend, &receipt["runtime"], "后继封存运行配置")?
        != output.join("runtime/runtime.json")
    {
        return Err("后继封存配置不属于同一恢复 attempt".into());
    }
    let verified_image = image(backend, &output, receipt, Some(original.image))?;
    let mut records = document::array(&archive["records"], "原封存历史")?.to_vec();
    records.extend([original.start.clone(), record.clone()]);
    Ok(
        json!({"start":original.start,"recovery":record,"records":records,"receipt":receipt,"image":verified_image}),
    )
}

fn failure(
    backend: &Path,
    run: &Path,
    prefix: &[Value],
    recovery_number: u64,
    receipt: &Value,
) -> Result<(Value, Value, Value)> {
    if receipt["status"] == "seed_source_partial_prelaunch_closed" {
        return partial::recovery_history(
            backend,
            run,
            prefix,
            recovery_number,
            &receipt["request"],
        );
    }
    let (start, collection) = origin(backend, run, prefix)?;
    let original = proof(backend, run, &start)?;
    Ok((start, original, collection))
}

fn image(backend: &Path, output: &Path, value: &Value, historical: Option<Value>) -> Result<Value> {
    let outer = common::read_bound(backend, &value["source_registration"], "原封存来源登记结果")?;
    let request = common::read_bound(backend, &outer["source_request"], "原封存来源请求")?;
    let mut selected = request["source"].clone();
    document::object(&selected, "原封存物理范围")?;
    selected["runtime_dir"] = json!(output.join("runtime"));
    let generation = common::read_bound(backend, &value["request"], "原封存生成请求")?;
    if generation["format_version"].as_u64() == Some(2) {
        selected["s3"]["configuration"] = generation["configuration"].clone();
    }
    let phases = if historical.is_some() {
        &["after"][..]
    } else {
        &["before", "after"][..]
    };
    let mut observed = historical.unwrap_or(Value::Null);
    for &phase in phases {
        if common::bound_file(backend, &value[phase], "原封存完整像")?
            != output.join(phase).join("image.json")
        {
            return Err("原封存完整像不属于同一恢复 attempt".into());
        }
        let image = images::verify(
            backend,
            &value[phase],
            &selected,
            &request,
            &value["source_registration"],
        )?;
        if phase == "after" && observed != image["image"] {
            return Err("未启动恢复采集前后完整像变化".into());
        }
        observed = image["image"].clone();
    }
    Ok(observed)
}

pub(in crate::recovery::source::evidence::running::history) fn starts(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
) -> Result<Vec<Value>> {
    let archive = closed(backend, run, attempts)?;
    let consumed = archive["records"].as_array().map_or(&[][..], Vec::as_slice);
    let mut starts = attempts
        .iter()
        .filter(|row| {
            row["stage"] == "seed-runtime"
                && row["mode"] == "source-generation-start"
                && !consumed.contains(row)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !archive.is_null()
        && starts.first().is_some_and(|row| {
            row["status"] == "failed"
                && row["result"].is_null()
                && row["error_type"] == "ValueError"
        })
    {
        let first = starts.first().ok_or("生成重试缺少原 START")?;
        let index = attempts
            .iter()
            .position(|row| row == first)
            .filter(|index| *index > 0)
            .ok_or("生成重试缺少相邻前序阶段")?;
        reregistration(backend, run, first, &attempts[index - 1])?;
        starts.remove(0);
    }
    let replay = super::super::lineage::authority(backend, run, attempts)?;
    if !replay.is_null() {
        starts.retain(|row| *row != replay["failed"]);
    }
    Ok(starts)
}

fn frozen_request(backend: &Path, value: &Value, prefix: &[Value]) -> Result<()> {
    let request = common::read_bound(backend, &value["request"], "原封存请求")?;
    coordinator::request(&request)?;
    for field in [
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
    ] {
        if value[field] != request[field] {
            return Err("未启动封存改变了原冻结请求".into());
        }
    }
    for (mode, field) in [
        ("source-register", "source_registration"),
        ("source-rebind", "source_rebind"),
    ] {
        let records = prefix
            .iter()
            .filter(|row| {
                row["stage"] == "seed-runtime" && row["mode"] == mode && row["status"] == "passed"
            })
            .collect::<Vec<_>>();
        if records.len() != 1 || records[0]["result"] != value[field] {
            return Err("未启动封存改变了唯一来源登记或重绑定".into());
        }
    }
    let rebound = common::read_bound(backend, &value["source_rebind"], "原封存重绑定")?;
    for field in ["source_registration", "review_successor", "current_storage"] {
        if value[field] != rebound[field] {
            return Err("未启动封存改变了存储或后继工具关系".into());
        }
    }
    Ok(())
}
