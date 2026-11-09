//! 参考恢复复用同一目标的两轮完整数据库库存与原始证据。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        bridge::scope::Scope,
        export::transport::Transport,
        json as document,
        lifecycle::manifest::overlaps,
        resources::common,
        target_verify::{
            database::{self, SourceBinding, model},
            evidence_protocol::inventory_history,
            guard::Inputs,
            storage::StorageInputs,
            tree,
        },
    },
};

pub(super) struct Capture<'a> {
    pub(super) storage: &'a StorageInputs<'a>,
    pub(super) reference: &'a Value,
    pub(super) initialized: &'a Value,
    pub(super) configuration: &'a Value,
    pub(super) scope: &'a Scope,
    pub(super) initialized_directory: &'a Path,
    pub(super) phase_root: &'a Path,
}

pub(super) struct Observed<'a> {
    pub(super) observations: BTreeMap<String, Value>,
    pub(super) receipt: Value,
    backend: &'a Path,
    request: &'a Value,
    scope: &'a Scope,
    initialized: Directory,
    phase: Directory,
    inventory: Directory,
    files: Value,
    original: Inputs,
    retained: Inputs,
}

pub(super) fn capture<'a>(
    spec: &Capture<'a>,
    transport: &impl Transport,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Observed<'a>> {
    checkpoint()?;
    let storage = spec.storage;
    let backend = storage.backend;
    let initialized = Directory::bind(backend, spec.initialized_directory)?;
    let phase = Directory::bind(backend, spec.phase_root)?;
    if overlaps(
        &fs::canonicalize(&initialized.path)?,
        &fs::canonicalize(&phase.path)?,
    ) {
        return Err("参考恢复库存目录与原初始化证据重叠".into());
    }
    let original = original_inputs(storage, spec.initialized, &initialized.path)?;
    let selected = selected_target(
        storage,
        spec.reference,
        spec.initialized,
        spec.configuration,
    )?;
    spec.scope.assert_unchanged()?;
    let directory = phase.path.join("inventory");
    let captured = database::capture_source_at(
        storage,
        &directory,
        transport,
        &checkpoint,
        &SourceBinding {
            side: CloneSide::Target,
            plan: None,
            selected: &selected,
            tools: &storage.request["tools"],
            maintenance: &storage.request["maintenance_build"],
            scope: spec.scope,
            configuration: Some(spec.configuration),
        },
    )?;
    let inventory = Directory::bind(backend, &directory)?;
    let files = Value::Array(captured.evidence_files);
    let retained = Inputs::open(tree::files(&directory, &files)?)?;
    let observed = Observed {
        observations: captured.observations,
        receipt: captured.receipt,
        backend,
        request: storage.request,
        scope: spec.scope,
        initialized,
        phase,
        inventory,
        files,
        original,
        retained,
    };
    observed.check(checkpoint)?;
    Ok(observed)
}

impl Observed<'_> {
    pub(super) fn check(&self, checkpoint: &dyn Fn() -> Result<()>) -> Result<()> {
        checkpoint()?;
        self.check_files()?;
        inventory_history::verify(
            self.backend,
            &self.initialized.path,
            &json!({
                "initial":{"receipt":self.receipt,"observations":self.observations},
                "request":self.request,"resumed":false,"inventory_directory":self.inventory.path,
            }),
        )?;
        self.scope.assert_unchanged()?;
        self.check_files()?;
        checkpoint()
    }

    fn check_files(&self) -> Result<()> {
        self.original.check()?;
        self.retained.checkpoint()?;
        for directory in [&self.initialized, &self.phase, &self.inventory] {
            directory.check(self.backend)?;
        }
        check_tree(self.backend, &self.inventory.path, &self.files)
    }
}

fn original_inputs(
    storage: &StorageInputs<'_>,
    initialized: &Value,
    directory: &Path,
) -> Result<Inputs> {
    let proof = directory.join("initialized.json");
    let request = directory.join("request.json");
    let held = Inputs::open([proof.clone(), request.clone()])?;
    if document::read(&proof)? != *initialized || document::read(&request)? != *storage.request {
        return Err("参考恢复数据库采集未使用原初始化与同一 arm 请求".into());
    }
    held.check()?;
    Ok(held)
}

fn selected_target(
    storage: &StorageInputs<'_>,
    reference: &Value,
    initialized: &Value,
    configuration: &Value,
) -> Result<Value> {
    let scope = storage.target_scope()?;
    let declared = storage.target();
    let target = &reference["target"];
    let initial = &initialized["generation"]["selected"];
    if initialized["status"] != "fresh_target_initialized"
        || initialized["controlled_generation_never_started"] != true
        || initialized["scope_id"] != scope
        || target["scope_id"] != scope
        || reference["target_side"] != storage.request["side"]
        || target["databases"] != declared["databases"]
        || target["s3"]["configuration"] != *configuration
        || ["endpoint", "region"]
            .into_iter()
            .any(|field| target["s3"][field] != declared["s3"][field])
        || ["runtime_dir", "api_url", "worker_ready_url", "frontend_url"]
            .into_iter()
            .any(|field| target[field].as_str().is_none() || target[field] != initial[field])
    {
        return Err("参考恢复数据库采集的目标 scope、库、配置或运行地址不同".into());
    }
    initial_resources(storage, initialized, scope)?;
    let mut selected = declared.clone();
    selected["runtime_dir"] = initial["runtime_dir"].clone();
    selected["api_url"] = initial["api_url"].clone();
    selected["s3"] = json!({"configuration":configuration});
    Ok(selected)
}

fn initial_resources(storage: &StorageInputs<'_>, initialized: &Value, scope: &str) -> Result<()> {
    let databases = model::databases(storage)?;
    let images = document::object(
        &initialized["inventory"]["observations"],
        "初始化四库完整像",
    )?;
    if images.len() != databases.len() {
        return Err("原初始化未绑定完整四库".into());
    }
    for database in databases.values() {
        let name = document::text(&database["database"], "登记目标物理库")?;
        let resource = json!({"kind":"database","scope_id":scope,
            "server_uuid":database["server_uuid"],"database":name});
        if images
            .get(name)
            .is_none_or(|image| image["resource"] != resource)
        {
            return Err("原初始化库存与同一目标物理库图谱不同".into());
        }
    }
    Ok(())
}

fn check_tree(backend: &Path, directory: &Path, files: &Value) -> Result<()> {
    if tree::snapshot(backend, directory, false)? != *files {
        return Err("参考恢复原库存文件树或字节变化".into());
    }
    Ok(())
}

struct Directory {
    path: PathBuf,
    identity: (u64, u64),
}

impl Directory {
    fn bind(backend: &Path, path: &Path) -> Result<Self> {
        let path = common::local_directory(backend, path, "参考恢复库存目录")?;
        let identity = common::device_inode_for(&path, &fs::symlink_metadata(&path)?)?;
        Ok(Self { path, identity })
    }

    fn check(&self, backend: &Path) -> Result<()> {
        if Self::bind(backend, &self.path)?.identity != self.identity {
            return Err("参考恢复库存目录身份变化".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/source/evidence/reference_workflow/restore/database_images.rs"]
mod tests;
