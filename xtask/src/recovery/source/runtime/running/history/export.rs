//! 失败导出的采用仅消费既有结果，不创建或修复收据。
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common},
};
use serde_json::Value;
use std::path::Path;

pub(super) fn reconciles(
    backend: &Path,
    run: &Path,
    state: &Value,
    failed: &Value,
    current: &Value,
    source: &Value,
) -> Result<bool> {
    if failed["stage"] != "seed-runtime"
        || failed["mode"] != "source-export"
        || failed["status"] != "failed"
    {
        return Ok(false);
    }
    let number = failed["number"].as_u64().ok_or("失败导出序号无效")?;
    let rows = document::array(&state["attempts"], "完整导出采用历史")?
        .iter()
        .filter(|row| {
            row["number"].as_u64().is_some_and(|n| n > number)
                && row["stage"] == "seed-runtime"
                && row["mode"] == "source-export-reconcile"
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Ok(false);
    }
    let terminal = rows
        .iter()
        .filter(|row| {
            row["status"] == "passed" || row["number"] == *current && row["status"] == "running"
        })
        .copied()
        .collect::<Vec<_>>();
    if terminal.len() != 1
        || rows.last() != terminal.first()
        || rows
            .iter()
            .any(|row| !terminal.contains(row) && row["status"] != "failed")
    {
        return Ok(false);
    }
    let mut values = Vec::new();
    for row in rows {
        if row["result"].is_null() {
            continue;
        }
        let number = row["number"].as_u64().ok_or("导出采用序号无效")?;
        let path = run.join(format!("results/{number:04}.json"));
        if common::binding(&path)? != row["result"] {
            return Err("seed export 采用结果发生变化".into());
        }
        let value = common::read_bound(backend, &row["result"], "导出采用结果")?;
        result_shape(&value)?;
        if value["status"] != "seed_source_export_published"
            || value["origin_attempt"] != failed["number"]
            || value["source_registration"] != *source
            || value["remote_writes"].as_u64() != Some(0)
            || value["restore_qualified"] != false
        {
            return Ok(false);
        }
        values.push(value);
    }
    Ok(values.windows(2).all(|pair| pair[0] == pair[1]))
}

fn result_shape(value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "status",
            "origin_attempt",
            "origin_intent",
            "source_registration",
            "source_rebind",
            "source_generation",
            "review_successor",
            "source_request",
            "source_storage",
            "export",
            "summary",
            "remote_writes",
            "restore_qualified",
        ],
        "导出采用结果",
    )?;
    document::exact(
        &value["summary"],
        &[
            "export_sha256",
            "generation_sha256",
            "proof_files_sha256",
            "logical_inventory_sha256",
            "databases",
            "objects",
        ],
        "导出采用摘要",
    )?;
    Ok(())
}
