//! 分段停机结果覆盖全部已登记代次；不凭单个停止标志推断其余进程已关闭。
use super::resolution::cache_call;
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        digest, json as document,
        resources::{common, storage::registered},
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn result(backend: &Path, run: &Path, attempt: &Value) -> Result<Value> {
    let number = attempt["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("分段阶段序号无效")?;
    if common::bound_file(backend, &attempt["result"], "分段续作结果")?
        != run.join(format!("results/{number:04}.json"))
    {
        return Err("分段续作结果不属于相邻账本阶段".into());
    }
    common::read_bound(backend, &attempt["result"], "分段续作结果")
}

pub(super) fn cache(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    source: &Value,
    successor: &Value,
) -> Result<Value> {
    let value = result(backend, run, attempt)?;
    document::exact(
        &value,
        &[
            "status",
            "side",
            "request",
            "processes",
            "copy_usable",
            "remote_writes",
            "resources_deleted",
        ],
        "缓存停止结果",
    )?;
    let registration = cache_call(backend, run, "registration", json!({}))?;
    let request = &registration[0];
    let descriptor = &registration[1];
    let rows = document::array(&value["processes"], "缓存停止进程集合")?;
    if value["status"] != "cache_stopped"
        || value["side"] != "target"
        || value["request"] != *descriptor
        || value["copy_usable"] != false
        || value["remote_writes"].as_u64() != Some(0)
        || value["resources_deleted"] != false
        || rows.is_empty()
    {
        return Err("分段续作缺少完整缓存停止结果".into());
    }
    let generations = cache_call(
        backend,
        run,
        "starts",
        json!({"request":request,"descriptor":descriptor,
        "before":attempt["number"],"frozen":[source,successor]}),
    )?;
    let generations = document::array(&generations, "完整缓存创建代次")?;
    if rows.len() != generations.len() {
        return Err("缓存停止结果没有覆盖全部已登记创建代次".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for (row, generation) in rows.iter().zip(generations) {
        document::exact(row, &["attempt", "process"], "缓存停止代次")?;
        let number = row["attempt"].as_u64().ok_or("缓存停止代次序号无效")?;
        if row["attempt"] != generation[0]["number"] || !seen.insert(number) {
            return Err("缓存停止结果没有按顺序唯一覆盖全部已登记创建代次".into());
        }
        cache_process(
            backend,
            &row["process"],
            number,
            &generation[2],
            generations,
        )?;
    }
    Ok(json!({"request":descriptor,"result":value}))
}

fn cache_process(
    backend: &Path,
    process: &Value,
    number: u64,
    runtime: &Value,
    generations: &[Value],
) -> Result<()> {
    if process["state"] == "stopped" {
        document::exact(
            process,
            &[
                "state",
                "launcher_alive",
                "linux",
                "tools_sha256",
                "evidence",
            ],
            "历史缓存停止进程",
        )?;
        document::exact(
            &process["linux"],
            &["alive", "boot_id", "identity", "terminated"],
            "缓存 Linux 停止结果",
        )?;
        if process["launcher_alive"] != false
            || process["linux"]["alive"] != false
            || !process["linux"]["terminated"].is_boolean()
        {
            return Err("缓存停止结果仍包含存活进程".into());
        }
        let path = common::bound_file(backend, &process["evidence"], "缓存停止 predecessor")?;
        let mut allowed = false;
        for generation in generations {
            let root = Path::new(document::text(&generation[1], "缓存创建输出目录")?);
            allowed |= path == root.join(format!("predecessor-{number:04}.json"));
        }
        if !allowed {
            return Err("缓存停止结果引用其他运行或代次的 predecessor".into());
        }
        let proof = common::read_bound(backend, &process["evidence"], "缓存停止 predecessor")?;
        document::exact(
            &proof,
            &["process_sha256", "runtime_sha256", "observation"],
            "缓存 predecessor 证明",
        )?;
        let mut observation = proof["observation"].clone();
        observation
            .as_object_mut()
            .ok_or("缓存停止观察必须为对象")?
            .insert("evidence".into(), process["evidence"].clone());
        if observation != *process || proof["runtime_sha256"] != digest::plan_hash(runtime)? {
            return Err("缓存停止结果没有绑定该创建代次的冻结运行像".into());
        }
        return Ok(());
    }
    document::exact(
        process,
        &[
            "status",
            "alive",
            "boot_id",
            "identity",
            "resources_deleted",
            "runtime",
            "terminated",
        ],
        "当前缓存停止结果",
    )?;
    if process["status"] != "redis_process_stopped"
        || process["alive"] != false
        || process["resources_deleted"] != false
        || !process["terminated"].is_boolean()
        || process["runtime"] != *runtime
        || process["identity"] != runtime["linux_identity"]
        || process["boot_id"] != runtime["boot_id"]
    {
        return Err("缓存停止结果没有完整关闭当前代次".into());
    }
    Ok(())
}

pub(super) fn storage(backend: &Path, run: &Path, attempt: &Value) -> Result<Value> {
    let value = result(backend, run, attempt)?;
    document::exact(
        &value,
        &[
            "status",
            "side",
            "request",
            "processes",
            "reconciliation_required",
            "object_api_calls",
            "database_calls",
            "resources_deleted",
        ],
        "存储停止结果",
    )?;
    let (request, descriptor) = registered::registration(backend, run, CloneSide::Target)?;
    if value["status"] != "storage_stopped"
        || value["side"] != "target"
        || value["request"] != descriptor
        || value["reconciliation_required"] != true
        || value["object_api_calls"].as_u64() != Some(0)
        || value["database_calls"].as_u64() != Some(0)
        || value["resources_deleted"] != false
    {
        return Err("分段续作缺少零数据操作的完整存储停止结果".into());
    }
    let previous = registered::previous_attempts(
        backend,
        run,
        CloneSide::Target,
        &descriptor,
        Some(attempt["number"].as_u64().ok_or("存储停止序号无效")?),
    )?;
    let rows = document::array(&value["processes"], "存储停止进程集合")?;
    if previous.is_empty() || rows.len() != previous.len() {
        return Err("存储停止结果没有覆盖全部已登记创建代次".into());
    }
    for ((record, observed), row) in previous.iter().zip(rows) {
        document::exact(
            row,
            &[
                "attempt",
                "state",
                "identity",
                "terminated",
                "process_receipt",
            ],
            "存储停止代次",
        )?;
        if observed["state"] != "recorded"
            || row["attempt"] != record["number"]
            || row["state"] != "stopped"
            || row["identity"] != observed["identity"]
            || row["process_receipt"] != observed["process_receipt"]
            || !row["terminated"].is_boolean()
        {
            return Err("存储停止结果仍有未知或存活创建代次".into());
        }
        common::require_recorded_stopped(&row["identity"])?;
    }
    directory_identity(backend, &request["data_directory"])?;
    Ok(json!({"request":descriptor,"result":value}))
}

pub(super) fn directory_identity(backend: &Path, value: &Value) -> Result<()> {
    document::exact(value, &["path", "device", "inode"], "存储数据目录身份")?;
    let path = common::local_directory(
        backend,
        Path::new(document::text(&value["path"], "存储数据目录")?),
        "存储数据目录",
    )?;
    let (device, inode) = common::device_inode_for(&path, &std::fs::symlink_metadata(&path)?)?;
    if value["device"].as_u64() != Some(device) || value["inode"].as_u64() != Some(inode) {
        return Err("存储数据目录实际身份与登记不一致".into());
    }
    Ok(())
}
