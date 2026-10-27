//! 唯一已发布导出的历史、意图、封存摘要与安全身份投影。
use super::{common, digest, document, generation, path};
use crate::{
    Result,
    recovery::{
        clone::native::{cache, lifecycle::state},
        source::evidence::running,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn identity(backend: &Path, descriptor: &Value) -> Result<Value> {
    let value = common::read_bound(backend, descriptor, "共享导出发布")?;
    let initial = cache::binding::source::load(backend, &value["review_successor"])?;
    let run = path(&initial["directory"])?;
    let state = state::load(&run, true)?;
    let source = running::history::predecessor(backend, &run, &state)?;
    let source = generation::resolve(backend, &source, &state)?;
    verify(backend, &run, &state, descriptor, &value, &source)?;
    let identity = projection(descriptor, &value)?;
    if common::read_bound(backend, descriptor, "共享导出发布")? != value
        || state::load(&run, true)? != state
    {
        return Err("共享导出发布在只读核验期间变化".into());
    }
    Ok(identity)
}

pub(super) fn projection(descriptor: &Value, value: &Value) -> Result<Value> {
    let mut identity = json!({"result":descriptor,"origin_attempt":value["origin_attempt"],
        "source_registration":value["source_registration"],"source_rebind":value["source_rebind"],
        "source_generation":value["source_generation"],"review_successor":value["review_successor"],
        "source_request":value["source_request"],"export":value["export"],
        "export_sha256":value["summary"]["export_sha256"],"generation_sha256":value["summary"]["generation_sha256"],
        "logical_inventory_sha256":value["summary"]["logical_inventory_sha256"]});
    identity["identity_sha256"] = json!(digest::plan_hash(&identity)?);
    Ok(identity)
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
        "导出发布结果",
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
        "导出发布摘要",
    )?;
    Ok(())
}

fn verify(
    backend: &Path,
    run: &Path,
    state: &Value,
    descriptor: &Value,
    value: &Value,
    source: &Value,
) -> Result<()> {
    let rows = document::array(&state["attempts"], "完整导出历史")?
        .iter()
        .filter(|row| {
            row["stage"] == "seed-runtime"
                && matches!(
                    row["mode"].as_str(),
                    Some("source-export" | "source-export-reconcile")
                )
        })
        .collect::<Vec<_>>();
    let passed = rows
        .iter()
        .filter(|row| row["status"] == "passed")
        .copied()
        .collect::<Vec<_>>();
    if passed.len() != 1 || passed[0]["result"] != *descriptor {
        return Err("比较必须绑定唯一已发布 seed source-export 外层结果".into());
    }
    let record = passed[0];
    let number = record["number"].as_u64().ok_or("导出发布序号无效")?;
    if common::bound_file(backend, descriptor, "导出发布")?
        != run.join(format!("results/{number:04}.json"))
    {
        return Err("导出发布不属于固定 attempt".into());
    }
    history(&rows, record)?;
    result_shape(value)?;
    for (key, expected) in [
        ("status", json!("seed_source_export_published")),
        (
            "source_registration",
            source["review_successor"]["source_result"].clone(),
        ),
        ("source_rebind", source["source_rebind"].clone()),
        ("source_generation", source["source_generation"].clone()),
        (
            "review_successor",
            source["review_successor_binding"].clone(),
        ),
        ("source_request", source["source_request"].clone()),
        ("source_storage", source["storage"]["storage"].clone()),
        ("remote_writes", json!(0)),
        ("restore_qualified", json!(false)),
    ] {
        if value[key] != expected {
            return Err("导出发布改变了已冻结 C52、存储、代次或 successor".into());
        }
    }
    if source["source_rebind"].is_null() || source["source_generation"].is_null() {
        return Err("导出发布必须继承完整存储重绑定及发布代次".into());
    }
    let origins = rows
        .iter()
        .filter(|row| row["number"] == value["origin_attempt"] && row["mode"] == "source-export")
        .copied()
        .collect::<Vec<_>>();
    if origins.len() != 1
        || origins[0] != record
            && (origins[0]["status"] != "failed" || record["mode"] != "source-export-reconcile")
    {
        return Err("导出发布缺少唯一合法原导出阶段".into());
    }
    let origin = origins[0];
    intent(backend, run, origin, record, value)?;
    let exported = common::read_bound(backend, &value["export"], "已发布导出清单")?;
    if exported["request"] != value["source_request"]
        || exported["logical_inventory_sha256"] != value["summary"]["logical_inventory_sha256"]
        || json!(digest::plan_hash(&exported)?) != value["summary"]["export_sha256"]
    {
        return Err("导出清单与已完整复核的发布摘要不同".into());
    }
    reconciled(backend, run, &rows, value)?;
    Ok(())
}

fn history(rows: &[&Value], record: &Value) -> Result<()> {
    let valid = if record["mode"] == "source-export" {
        rows == [record]
    } else {
        rows.len() >= 2
            && rows.last() == Some(&record)
            && rows[0]["mode"] == "source-export"
            && rows[0]["status"] == "failed"
            && rows[1..rows.len() - 1]
                .iter()
                .all(|row| row["mode"] == "source-export-reconcile" && row["status"] == "failed")
    };
    if !valid {
        return Err("导出存在重复、未收尾或非法采用历史".into());
    }
    Ok(())
}

fn intent(backend: &Path, run: &Path, origin: &Value, record: &Value, value: &Value) -> Result<()> {
    let number = origin["number"].as_u64().ok_or("原导出序号无效")?;
    let filename = run.join(format!("export-{number:04}.intent.json"));
    if common::bound_file(backend, &value["origin_intent"], "原导出意图")? != filename
        || common::bound_file(backend, &value["export"], "原导出清单")?
            != run.join(format!("e{number:04}/export.json"))
    {
        return Err("导出与原固定 attempt 目录不同".into());
    }
    let saved = common::read_bound(backend, &value["origin_intent"], "原导出意图")?;
    let mut initial = origin.clone();
    for field in ["finished_at", "result", "error_type"] {
        initial[field] = Value::Null;
    }
    initial["status"] = json!("running");
    let (controller, owner) = state::controller_record(run, number, &initial)?;
    if owner["directory"] != json!(run)
        || owner["manifest_sha256"] != common::binding(&run.join("manifest.json"))?["sha256"]
    {
        return Err("导出恢复控制器不属于固定运行的原始 attempt".into());
    }
    let expected = json!({"format_version":1,"kind":"devex-clone-export-attempt","attempt":number,
        "output":run.join(format!("e{number:04}")),"manifest":common::binding(&run.join("manifest.json"))?,
        "controller":controller,"source_request":value["source_request"],"source_storage":value["source_storage"],
        "execution_source":origin["sources"],"remote_operations":"read_only"});
    if saved != expected {
        return Err("导出意图与固定 manifest、controller、存储或原执行源码不同".into());
    }
    let seal = run.join(format!("export-{number:04}.verified.json"));
    match std::fs::symlink_metadata(&seal) {
        Ok(_) => {
            let expected = json!({"format_version":1,"kind":"devex-clone-export-verified","attempt":number,
                "intent":value["origin_intent"],"export":value["export"],"source_storage":saved["source_storage"],"summary":value["summary"],"remote_writes":0});
            if document::read(&seal)? != expected {
                return Err("导出完整核验封存与发布摘要矛盾".into());
            }
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && record["mode"] == "source-export-reconcile" => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn reconciled(backend: &Path, run: &Path, rows: &[&Value], value: &Value) -> Result<()> {
    for row in rows {
        if row["mode"] != "source-export-reconcile" || row["result"].is_null() {
            continue;
        }
        let number = row["number"].as_u64().ok_or("导出采用序号无效")?;
        if common::bound_file(backend, &row["result"], "导出采用结果")?
            != run.join(format!("results/{number:04}.json"))
            || common::read_bound(backend, &row["result"], "导出采用结果")? != *value
        {
            return Err("导出发布与先前只读采用结果不同".into());
        }
    }
    Ok(())
}
