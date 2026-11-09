//! 参考备份从已停止的同一来源重新读取真实数据，只在本地发布备份。
use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{inputs, model, native, path, proof, readonly::BackupDatabases, text};
use crate::Result;
use inputs::document::{self, Document};
use native::{
    export::transport::{SystemTransport, Transport},
    resources::common,
};

mod objects;

pub(super) fn run(backend: &Path, plan: &Value, work: &Path, request: &Value) -> Result<Value> {
    let raw = document::read(path(&request["inventory"])?, backend, "共享导出原始库存")?;
    let inventory = model::backup_inventory(plan, &raw.value, || {
        native::source::protocol::request(&json!({"action":"source-snapshot","root":backend}))
    })?;
    inputs::inventory::validate(plan, &inventory)?;
    let generation = document::read(
        path(&request["source_generation"])?,
        backend,
        "备份来源停止代次",
    )?;
    let published = document::read(
        path(&request["source_export_result"])?,
        backend,
        "共享导出发布",
    )?;
    let verify_source =
        || verify_source(backend, plan, &inventory, [&raw, &generation, &published]);
    let source = verify_source()?;
    let stopped = || require_stopped(backend, &plan["source"], common::process_identity);
    stopped()?;
    let databases = BackupDatabases::capture(backend, path(&request["plan"])?, plan, work)?;
    let storage = objects::Storage {
        backend,
        source: &plan["source"],
        transport: &SystemTransport,
    };
    databases.verify(&SystemTransport)?;
    storage.verify_owners(work)?;
    let root = work.join("backup");
    fs::create_dir(&root)?;
    let mut artifacts = dump_databases(&root, plan, &inventory, &databases, &SystemTransport)?;
    storage.store_buckets(&root, &inventory, &mut artifacts)?;
    publish(backend, &root, plan, &inventory, artifacts, &source, || {
        databases.check()?;
        stopped()?;
        verify_source()
    })
}

fn verify_source(
    backend: &Path,
    plan: &Value,
    inventory: &Value,
    documents: [&Document; 3],
) -> Result<Value> {
    let [_, generation, published] = documents;
    let verified = inputs::backup::verify_source(
        backend,
        plan,
        inventory,
        &document::binding(generation),
        &document::binding(published),
        true,
    )?;
    for document in documents {
        document.assert_unchanged()?;
    }
    Ok(verified)
}

fn require_stopped(
    backend: &Path,
    side: &Value,
    process: impl Fn(u32) -> Result<Option<Value>>,
) -> Result<()> {
    let runtime = path(&side["runtime_dir"])?;
    for role in ["api", "worker"] {
        let receipt = document::read(
            &runtime.join(format!("{role}.json")),
            backend,
            "备份进程登记",
        )?;
        let value = &receipt.value;
        if value["format_version"].as_u64() != Some(1)
            || value["role"] != role
            || value["scope_id"] != side["scope_id"]
        {
            return Err("备份进程收据的角色或隔离 scope 不匹配".into());
        }
        let pid = value["identity"]["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 1)
            .ok_or("备份进程收据 PID 无效")?;
        if process(pid)?.is_some() {
            return Err("备份期间已登记 API/Worker 必须停止；PID 重用也须重新确认".into());
        }
        receipt.assert_unchanged()?;
    }
    Ok(())
}

fn dump_databases(
    root: &Path,
    plan: &Value,
    inventory: &Value,
    databases: &BackupDatabases,
    transport: &impl Transport,
) -> Result<Vec<Value>> {
    let directory = root.join("databases");
    fs::create_dir(&directory)?;
    let declared = native::json::array(&plan["source"]["databases"], "备份数据库")?;
    native::json::array(&inventory["databases"], "备份数据库库存")?
        .iter()
        .map(|database| {
            let key = text(&database["key"])?;
            let selected = declared
                .iter()
                .find(|db| db["key"] == key)
                .ok_or("备份数据库未登记")?;
            let tables = native::json::array(&database["tables"], "备份数据库表")?
                .iter()
                .map(|table| text(&table["table"]))
                .collect::<Result<Vec<_>>>()?;
            let relative = format!("databases/{key}.sql");
            databases.dump(transport, selected, &tables, &root.join(&relative))?;
            artifact(root, &relative, &format!("db:{key}"))
        })
        .collect()
}

fn artifact(root: &Path, relative: &str, resource: &str) -> Result<Value> {
    let file = root.join(relative);
    super::unlinked(&file)?;
    let digest = native::digest::file(&file)?;
    Ok(
        json!({"relative_path":relative,"resource":resource,"bytes":digest.bytes,"sha256":digest.sha256}),
    )
}

fn publish(
    backend: &Path,
    root: &Path,
    plan: &Value,
    inventory: &Value,
    artifacts: Vec<Value>,
    source: &Value,
    verify_source: impl FnOnce() -> Result<Value>,
) -> Result<Value> {
    let manifest = model::manifest(
        &json!({"plan":plan,"inventory":inventory,"artifacts":artifacts,"root":root}),
    )?;
    if verify_source()? != *source {
        return Err("备份期间共享导出或来源绑定被替换".into());
    }
    // 活体复核可能耗时，发布前重新确认将被绑定的实际本地备份文件。
    inputs::inventory::verify_artifacts(root, &manifest)?;
    let file = root.join("manifest.json");
    super::unlinked(&file)?;
    native::json::write_new(&file, &manifest)?;
    let document = document::read(&file, backend, "正式备份清单")?;
    if document.value != manifest {
        return Err("正式备份清单在发布期间变化".into());
    }
    let value = json!({"format_version":2,"kind":"restore-reference-backup","reference_plan":plan,
        "manifest":document::binding(&document),"backup_root":root,"artifacts":artifacts.len(),
        "source_export":source["source_export"],"source_generation":source["source_generation"]});
    proof::backup_result(&json!({"value":value}))?;
    document.assert_unchanged()?;
    Ok(value)
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/source/evidence/reference_workflow/backup.rs"]
mod tests;
