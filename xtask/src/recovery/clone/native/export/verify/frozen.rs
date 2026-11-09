//! 停机导出的离线证明只消费原文件和保存的产物。
use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use super::{
    super::{capture, database::Models, evidence, input, inventory},
    SavedObjectInputs, reject_failure,
};
use crate::{
    Result,
    recovery::{
        clone::native::{catalog, digest, json as document, sql},
        inputs::document::{Document, bound_file},
        source::evidence::saved_object_captures,
    },
};

mod facts;
mod objects;

pub(crate) struct FrozenExport {
    pub(crate) request: Value,
    pub(crate) generation: Value,
    pub(crate) inventory: Value,
    pub(crate) captures: BTreeMap<String, BTreeMap<String, Value>>,
    pub(crate) proof_files: Value,
}

impl FrozenExport {
    /// 只复核原小型证明，SQL 和对象正文继续由绑定它们的步骤校验。
    pub(crate) fn check_proofs(&self, backend: &Path, descriptor: &Value) -> Result<()> {
        use crate::recovery::clone::native::resources::common;

        let export = common::bound_file(backend, descriptor, "原复制导出收据")?;
        let root = export.parent().ok_or("原复制导出缺少证据目录")?;
        reject_failure(root)?;
        for (name, binding) in document::object(&self.proof_files, "原导出小型证明")? {
            let path = common::common_file(backend, Path::new(name), "原导出小型证明")?;
            let actual = digest::file(&path)?;
            if binding["bytes"].as_u64() != Some(actual.bytes) || binding["sha256"] != actual.sha256
            {
                return Err("原导出小型证明在来源观察窗口内变化".into());
            }
            if path.file_name().is_some_and(|name| name == "capture.json") {
                reject_failure(path.parent().ok_or("原对象采集缺少证据目录")?)?;
            }
        }
        common::bound_file(backend, descriptor, "原复制导出收据")?;
        reject_failure(root)
    }
}

/// 复核指定冻结导出，不查询源端口、构建新产物或要求源码当前 HEAD。
pub(crate) fn verify(backend: &Path, descriptor: &Value) -> Result<FrozenExport> {
    let inputs = SavedObjectInputs::load(backend, descriptor)?;
    let bound = Evidence::load(&inputs)?;
    let execution = input::execution_backend(backend, &inputs.request)?;
    let catalog = Models::load_saved(&execution)?;
    let generation = facts::verify(&inputs, &bound.values, &catalog)?;
    let receipts = receipts(backend, &execution, &inputs.request, &generation)?;
    let proof_files = evidence::frozen_files(
        backend,
        &inputs.request["source"],
        &inputs.export,
        &inputs.root,
    )?;
    let authority = saved_object_captures(&json!({"backend":backend,"export":descriptor}))?;
    dumps(&inputs, &catalog)?;
    let captures = objects::verify(
        backend,
        &inputs.root,
        &inputs.export,
        &proof_files,
        &authority,
    )?;
    for receipt in receipts {
        receipt.assert_unchanged()?;
    }
    bound.check(&inputs)?;
    if catalog.fingerprint()? != Models::load_saved(&execution)?.fingerprint()?
        || proof_files
            != evidence::frozen_files(
                backend,
                &inputs.request["source"],
                &inputs.export,
                &inputs.root,
            )?
    {
        return Err("源 catalog 或完整原始小型证据在核验期间变化".into());
    }
    inputs.check()?;
    let captures = captures.finish()?;
    Ok(FrozenExport {
        request: inputs.request,
        generation,
        inventory: inputs.inventory,
        captures,
        proof_files,
    })
}

const EVIDENCE: &[&str] = &[
    "request",
    "generation-before",
    "generation-after",
    "inventory-before",
    "inventory-after",
    "inventory-before-targets",
    "inventory-after-targets",
    "schema-before",
    "schema-after",
];

struct Evidence {
    values: BTreeMap<&'static str, Value>,
}

impl Evidence {
    fn load(inputs: &SavedObjectInputs) -> Result<Self> {
        let bindings = document::exact(&inputs.export["evidence"], EVIDENCE, "原始导出证据")?;
        let values = EVIDENCE
            .iter()
            .map(|&name| {
                let path = evidence::bound_relative(
                    &inputs.root,
                    &bindings[name],
                    &format!("{name}.json"),
                )?;
                Ok((name, document::read(&path)?))
            })
            .collect::<Result<_>>()?;
        Ok(Self { values })
    }

    fn check(&self, inputs: &SavedObjectInputs) -> Result<()> {
        if Self::load(inputs)?.values != self.values {
            return Err("原始导出证据在核验期间变化".into());
        }
        Ok(())
    }
}

impl Models {
    fn load_saved(execution: &Path) -> Result<Self> {
        let loaded = catalog::load_ordered(execution, true)?;
        let (control, tenant) = loaded.full.ok_or("源导出缺少完整 schema catalog")?;
        // 让调用方的现有读取窗口同样保护 catalog 的实际文件。
        for path in loaded.sources {
            crate::recovery::runtime::dataset_authority::reads::file(&path)?;
        }
        Ok(Self {
            control: catalog::into_catalog(loaded.control),
            tenant: catalog::into_catalog(loaded.tenant),
            full_control: catalog::into_catalog(control),
            full_tenant: catalog::into_catalog(tenant),
        })
    }

    fn fingerprint(&self) -> Result<String> {
        digest::plan_hash(&json!({"control":self.control,"tenant":self.tenant}))
    }
}

fn receipts(
    backend: &Path,
    execution: &Path,
    request: &Value,
    generation: &Value,
) -> Result<[Document; 5]> {
    let (_, build) = bound_file(backend, &request["backend_build"], "源产品构建")?;
    let (_, maintenance) = bound_file(execution, &request["maintenance_build"], "源维护构建")?;
    let (_, runtime) = bound_file(backend, &request["runtime"], "源运行收据")?;
    let (_, api) = bound_file(backend, &request["processes"]["api"], "源 API 进程收据")?;
    let (_, worker) = bound_file(
        backend,
        &request["processes"]["worker"],
        "源 Worker 进程收据",
    )?;
    facts::receipts(
        generation,
        &build.value,
        &maintenance.value,
        &runtime.value,
        &api.value,
        &worker.value,
    )?;
    Ok([build, maintenance, runtime, api, worker])
}

fn dumps(inputs: &SavedObjectInputs, models: &Models) -> Result<()> {
    let source = &inputs.request["source"];
    let dumps = document::array(&inputs.export["databases"], "源数据库转储")?;
    let bound = facts::dumps(source, dumps, &inputs.inventory, models)?;
    for item in &bound {
        bound_dump(&inputs.root, item)?;
    }
    let objects = inventory::object_index(&inputs.inventory)?;
    let endpoint = document::text(&inputs.export["source"]["s3"]["endpoint"], "源对象端点")?;
    let state = capture::dump::validate_dump_state(
        source,
        endpoint,
        &inputs.root,
        models,
        dumps,
        &objects,
        capture::dump::PhysicalRules::Frozen,
    )?;
    facts::tenant_identifiers(&state.logical_tenants, source)?;
    if inputs.export["enabled_system_schedule_rows"] != json!(state.schedules) {
        return Err("源实际业务关系或调度处置与导出结果不同".into());
    }
    for item in bound {
        bound_dump(&inputs.root, item)?;
    }
    Ok(())
}

fn bound_dump(root: &Path, binding: &Value) -> Result<()> {
    let name = document::text(&binding["file"], "源 SQL 转储路径")?;
    evidence::bound_relative(root, binding, name)?;
    Ok(())
}
