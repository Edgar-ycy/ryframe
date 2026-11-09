//! 复制窗口内的双侧四库观察与冻结前后像。数据库访问仅使用已有只读库存端口。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    super::{
        catalog, digest,
        export::transport::Transport,
        input::digest_shape,
        json as document,
        lifecycle::manifest::overlaps,
        resources::common,
        target_verify::{
            database::{self, Captured, SourceBinding, model},
            storage::StorageInputs,
        },
    },
    images::DatabaseBasis,
};
use crate::{
    Result,
    cli::CloneSide,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::inputs::document::binding_shape,
};

pub(crate) struct Side<'a> {
    pub(crate) storage: StorageInputs<'a>,
    pub(crate) binding: SourceBinding<'a>,
}

/// 原始导出、初始化与可选的既有会话都由外层绑定；checkpoint 负责实际文件和锁。
pub(crate) struct Window<'a> {
    pub(crate) source: Side<'a>,
    pub(crate) target: Side<'a>,
    pub(crate) export_inventory: &'a Value,
    pub(crate) initialized: &'a Value,
    pub(crate) frozen_source_observation: Option<&'a Value>,
}

pub(crate) struct Baseline {
    source: Observed,
    initialized: BTreeMap<String, Value>,
    initialized_receipt: Value,
}

pub(crate) struct Observed {
    pub(crate) observations: BTreeMap<String, Value>,
    pub(crate) receipt: Value,
    pub(crate) evidence_files: Vec<Value>,
}

pub(crate) struct ObservedPair {
    pub(crate) source: Observed,
    pub(crate) target: Observed,
}

impl Window<'_> {
    pub(crate) fn baseline(
        &self,
        source_output: &Path,
        transport: &impl Transport,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Baseline> {
        checkpoint()?;
        self.check_sides()?;
        let (initialized, initialized_receipt) = self.initialized_images()?;
        let source = self.capture_source(source_output, transport, checkpoint)?;
        checkpoint()?;
        Ok(Baseline {
            source,
            initialized,
            initialized_receipt,
        })
    }

    pub(crate) fn observe_source(
        &self,
        baseline: &Baseline,
        source_output: &Path,
        transport: &impl Transport,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Observed> {
        checkpoint()?;
        self.check_sides()?;
        let source = self.capture_source(source_output, transport, checkpoint)?;
        baseline.require_source(&source)?;
        checkpoint()?;
        Ok(source)
    }

    pub(crate) fn observe(
        &self,
        baseline: &Baseline,
        source_output: &Path,
        target_output: &Path,
        transport: &impl Transport,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<ObservedPair> {
        checkpoint()?;
        self.check_sides()?;
        let (initialized, receipt) = self.initialized_images()?;
        if baseline.initialized != initialized || baseline.initialized_receipt != receipt {
            return Err("原初始化库存与本次复制初像不同".into());
        }
        let source_output = self.source.output(source_output)?;
        let target_output = self.target.output(target_output)?;
        if overlaps(&source_output, &target_output) {
            return Err("两侧库存必须各自占有独立的全新证据目录".into());
        }
        let source = self.capture_source(&source_output, transport, checkpoint)?;
        baseline.require_source(&source)?;
        let target = self.target.capture(&target_output, transport, checkpoint)?;
        checkpoint()?;
        Ok(ObservedPair { source, target })
    }

    fn capture_source(
        &self,
        output: &Path,
        transport: &impl Transport,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Observed> {
        let captured = self.source.capture(output, transport, checkpoint)?;
        source_matches_export(
            &captured.observations,
            &model::databases(&self.source.storage)?,
            self.source.storage.target_scope()?,
            self.export_inventory,
        )?;
        if let Some(frozen) = self.frozen_source_observation
            && source_projection(&captured.observations) != *frozen
        {
            return Err("源完整业务及保留表与冻结复制会话不同".into());
        }
        Ok(captured)
    }

    fn check_sides(&self) -> Result<()> {
        let source = &self.source;
        let target = &self.target;
        if source.binding.side != CloneSide::Source
            || target.binding.side != CloneSide::Target
            || source.storage.backend != target.storage.backend
            || source.binding.configuration.is_none()
            || target.binding.configuration.is_none()
            || source.storage.request["tools"]["mysql"] != target.storage.request["tools"]["mysql"]
        {
            return Err("复制库存必须明确绑定同一协调工作树、两侧配置和同一 MySQL 工具".into());
        }
        document::object(&source.storage.request["target"], "源四库采集配置")?;
        document::object(&target.storage.request["target"], "目标四库采集配置")?;
        if source.storage.target_scope()? == target.storage.target_scope()? {
            return Err("复制库存必须观察两侧独立 scope".into());
        }
        Ok(())
    }

    fn initialized_images(&self) -> Result<(BTreeMap<String, Value>, Value)> {
        let scope = self.target.storage.target_scope()?;
        let declared = model::databases(&self.target.storage)?;
        initialized_images(self.initialized, &declared, scope)
    }
}

impl Side<'_> {
    fn output(&self, output: &Path) -> Result<PathBuf> {
        let output = validate_local_test_path(
            output,
            self.storage.backend,
            LocalTestPathKind::NewDirectory,
        )?;
        common::local_directory(
            self.storage.backend,
            output.parent().ok_or("库存证据目录缺少父目录")?,
            "库存证据父目录",
        )?;
        Ok(output)
    }

    fn capture(
        &self,
        output: &Path,
        transport: &impl Transport,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Observed> {
        checkpoint()?;
        let output = self.output(output)?;
        let declared = model::databases(&self.storage)?;
        let scope = self.storage.target_scope()?;
        // source 绑定模式也适用于复制后的目标，按 require_fresh=false 采集两次完整库存。
        let captured = database::capture_source_at(
            &self.storage,
            &output,
            transport,
            &checkpoint,
            &self.binding,
        )
        .and_then(|captured| Observed::from_capture(captured, &declared, scope, self.binding.side));
        match (captured, checkpoint()) {
            (Ok(observed), Ok(())) => Ok(observed),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(check)) => {
                Err(format!("{error}；库存采集收尾复核同时失败：{check}").into())
            }
        }
    }
}

impl Baseline {
    pub(crate) fn source(&self) -> &Observed {
        &self.source
    }

    pub(crate) fn initialized(&self) -> &BTreeMap<String, Value> {
        &self.initialized
    }

    pub(crate) fn initialized_receipt(&self) -> &Value {
        &self.initialized_receipt
    }

    pub(crate) fn source_observation(&self) -> Value {
        source_projection(&self.source.observations)
    }

    pub(crate) fn basis<'a>(
        &'a self,
        key: &str,
        export_artifact_sha256: &'a str,
    ) -> Result<DatabaseBasis<'a>> {
        digest_shape(&json!(export_artifact_sha256))?;
        Ok(DatabaseBasis {
            source: self
                .source
                .observations
                .get(key)
                .ok_or("未登记的来源逻辑库")?,
            initialized: self.initialized.get(key).ok_or("未登记的初始化逻辑库")?,
            artifact_sha256: export_artifact_sha256,
        })
    }

    pub(crate) fn verify_complete_target(&self, observed: &Observed) -> Result<()> {
        if !observed.observations.keys().eq(self.initialized.keys()) {
            return Err("复制目标后像缺少或额外包含逻辑库".into());
        }
        for (key, initial) in &self.initialized {
            let source = self
                .source
                .observations
                .get(key)
                .ok_or("来源初像缺少逻辑库")?;
            let mut expected = initial.clone();
            expected["tables"] = source["tables"].clone();
            if observed.observations[key] != expected {
                return Err("最终目标完整业务、保留表或 ownership 与来源及初始化像不同".into());
            }
        }
        Ok(())
    }

    fn require_source(&self, observed: &Observed) -> Result<()> {
        if observed.observations != self.source.observations {
            return Err("复制期间源完整业务表、保留表或数据库身份发生变化".into());
        }
        Ok(())
    }
}

impl Observed {
    fn from_capture(
        captured: Captured,
        declared: &BTreeMap<String, Value>,
        scope: &str,
        side: CloneSide,
    ) -> Result<Self> {
        let physical = serde_json::to_value(&captured.observations)?;
        let observations = index_physical(&physical, declared, scope)?;
        if captured.inventory["side"] != side.as_str()
            || captured.inventory["scope_id"] != scope
            || captured.inventory["observations"] != serde_json::to_value(&observations)?
        {
            return Err("两轮库存原始收据与同侧数据库完整像不同".into());
        }
        Ok(Self {
            observations,
            receipt: captured.receipt,
            evidence_files: captured.evidence_files,
        })
    }
}

fn initialized_images(
    initialized: &Value,
    declared: &BTreeMap<String, Value>,
    scope: &str,
) -> Result<(BTreeMap<String, Value>, Value)> {
    if initialized["status"] != "fresh_target_initialized"
        || initialized["scope_id"] != scope
        || initialized["controlled_generation_never_started"].as_bool() != Some(true)
    {
        return Err("目标库存必须来自同侧已验证的原初始化".into());
    }
    let inventory = &initialized["inventory"];
    let receipt = &inventory["receipt"];
    binding_shape(receipt, "原初始化库存收据")?;
    let observations = index_physical(&inventory["observations"], declared, scope)?;
    Ok((observations, receipt.clone()))
}

fn index_physical(
    images: &Value,
    declared: &BTreeMap<String, Value>,
    scope: &str,
) -> Result<BTreeMap<String, Value>> {
    let images = document::object(images, "物理库名索引的完整观察")?;
    let mut names = BTreeSet::new();
    let mut logical = BTreeMap::new();
    if declared.len() != images.len() || declared.len() != 4 {
        return Err("库存必须完整覆盖本侧四个独立物理库".into());
    }
    for (key, database) in declared {
        let name = document::text(&database["database"], "登记物理库")?;
        let image = images.get(name).ok_or("库存缺少精确登记物理库")?;
        if !names.insert(name) {
            return Err("库存物理库索引不是唯一映射".into());
        }
        check_image(image, database, scope)?;
        logical.insert(key.clone(), image.clone());
    }
    Ok(logical)
}

fn check_image(image: &Value, database: &Value, scope: &str) -> Result<()> {
    document::exact(
        image,
        &[
            "resource",
            "schema_sha256",
            "tables",
            "preserved",
            "all_tables",
            "ownership",
        ],
        "复制数据库完整像",
    )?;
    let combined = database["kind"] == "combined";
    let resource = json!({"kind":"database", "scope_id":scope,
        "server_uuid":database["server_uuid"],"database":database["database"]});
    if image["resource"] != resource
        || image["ownership"] != json!(model::expected_owners(scope, combined))
    {
        return Err("数据库库存必须保留真实物理身份和完整 owner".into());
    }
    digest_shape(&image["schema_sha256"])?;
    let tables = image_names(&image["tables"])?;
    let kept = image_names(&image["preserved"])?;
    let mut allowed = catalog::excluded_tables();
    allowed.extend(["seaql_tenant_data_migrations", "ryframe_business_migration"]);
    let names = document::array(&image["all_tables"], "完整物理表集")?;
    let all = names
        .iter()
        .map(|entry| document::text(entry, "完整物理表名"))
        .collect::<Result<BTreeSet<_>>>()?;
    if kept.iter().any(|name| !allowed.contains(name))
        || tables.iter().any(|name| allowed.contains(name))
        || !kept.contains("ryframe_resource_ownership")
        || image["preserved"]["ryframe_resource_ownership"]["rows"].as_u64()
            != Some(if combined { 2 } else { 1 })
        || !tables.is_disjoint(&kept)
        || all.len() != names.len()
        || all != tables.union(&kept).copied().collect::<BTreeSet<_>>()
    {
        return Err("数据库库存业务表、保留表或完整表集不一致".into());
    }
    Ok(())
}

fn image_names(image: &Value) -> Result<BTreeSet<&str>> {
    let image = document::object(image, "数据库逐表像")?;
    for value in image.values() {
        document::exact(value, &["rows", "sha256"], "数据库逐表摘要")?;
        if value["rows"].as_u64().is_none() {
            return Err("数据库逐表像缺少有效行数".into());
        }
        digest_shape(&value["sha256"])?;
    }
    Ok(image.keys().map(String::as_str).collect())
}

fn source_matches_export(
    observations: &BTreeMap<String, Value>,
    declared: &BTreeMap<String, Value>,
    scope: &str,
    inventory: &Value,
) -> Result<()> {
    let fingerprints = model::fingerprints(inventory)?;
    let entries = document::array(&inventory["databases"], "原导出完整四库")?;
    if inventory["scope_id"] != scope
        || entries.len() != declared.len()
        || entries.len() != observations.len()
    {
        return Err("原导出库存不覆盖当前同 scope 的完整源四库".into());
    }
    let mut seen = BTreeSet::new();
    for entry in entries {
        let key = document::text(&entry["key"], "原导出逻辑库")?;
        let database = declared.get(key).ok_or("原导出库存包含未知逻辑库")?;
        let observed = observations.get(key).ok_or("当前源库存缺少原导出逻辑库")?;
        if !seen.insert(key)
            || ["kind", "server_uuid", "database"]
                .iter()
                .any(|field| entry[*field] != database[*field])
            || entry["shared"].as_bool() != Some(database["mode"] == "shared")
        {
            return Err("原导出库存的数据库类型、物理身份或逻辑映射不同".into());
        }
        let mut schema = fingerprints.clone();
        if database["kind"] != "combined" {
            schema["control_schema_fingerprint"] = Value::Null;
        }
        if observed["schema_sha256"] != digest::plan_hash(&schema)?
            || model::table_map(&entry["tables"])?
                .iter()
                .any(|(table, expected)| {
                    observed["tables"]
                        .get(table)
                        .or_else(|| observed["preserved"].get(table))
                        != Some(expected)
                })
        {
            return Err("源当前行像或编译 schema 与原导出库存不同".into());
        }
    }
    Ok(())
}

fn source_projection(images: &BTreeMap<String, Value>) -> Value {
    Value::Object(
        images
            .iter()
            .map(|(key, image)| {
                (
                    key.clone(),
                    json!({"tables":image["tables"],"preserved":image["preserved"]}),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/observe_database.rs"]
mod tests;
