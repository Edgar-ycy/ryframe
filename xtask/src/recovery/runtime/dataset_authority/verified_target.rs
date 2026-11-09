//! 已发布目标、备份及运行收据的完整共同权威。
use std::path::Path;

use serde_json::Value;

use super::{check, cross, inputs::Inputs, lineage, path, reads::Window, registration, schema};
use crate::{
    Result,
    cli::{RuntimeSources, RuntimeVerifyOptions},
    recovery::{
        inputs::{backup, document::Document, target_plan},
        runtime::live_verify,
        source::evidence::comparison,
    },
};

pub(super) struct VerifiedTarget {
    pub(super) runtime: Document,
    pub(super) target: Document,
    pub(super) documents: cross::RuntimeDocuments,
    pub(super) value: Value,
    pub(super) manifest: Value,
    pub(super) authority: Value,
    pub(super) lineage: Value,
}

impl VerifiedTarget {
    pub(super) fn read(
        backend: &Path,
        runtime_path: &Path,
        target_path: &Path,
        inputs: &mut Inputs,
    ) -> Result<Self> {
        let runtime = inputs.read(runtime_path)?;
        schema::runtime(&runtime.value)?;
        let target = inputs.read(target_path)?;
        let documents = cross::read(inputs, &runtime.value)?;
        let reference = registration::reference(inputs, &documents.launch.value, &target)?;
        let (chain, value, _) = target_plan::verify(backend, reference, &target.path)?;
        comparison::verify(backend, &chain.comparison_document.value, true)?;
        backup::verify_frozen_source(backend, &chain.backup)?;
        let authority = cross::verify(
            &runtime.value,
            &value,
            &chain.reference.value,
            &chain.backup.manifest,
            &documents,
        )?;
        let lineage = lineage::extract(inputs, &chain.backup.result)?;
        Ok(Self {
            runtime,
            target,
            documents,
            value,
            manifest: chain.backup.manifest,
            authority,
            lineage,
        })
    }

    pub(super) fn verify_live(&self, backend: &Path, window: &Window) -> Result<()> {
        let options = options(&self.runtime, &self.value)?;
        let expected = self.runtime.sha256();
        let live = live_verify::verify_live_generation(
            backend,
            &options,
            &self.authority,
            &expected,
            window,
        )?;
        check(
            live["runtime_receipt_sha256"] == expected,
            "未主动核验当前运行收据",
        )
    }
}

fn options(runtime: &Document, target: &Value) -> Result<RuntimeVerifyOptions> {
    let execution = &target["product_execution"];
    let roots = &execution["roots"];
    let source_backend = path(&roots["execution_backend"])?;
    let product_backend = path(&roots["source_backend"])?;
    let distinct = !super::commands::same_path(&source_backend, &product_backend)?;
    let adapter_contract = execution["adapter"]["contract"].as_str().map(str::to_owned);
    Ok(RuntimeVerifyOptions {
        sources: RuntimeSources {
            source_backend,
            source_frontend: path(&roots["frontend"])?,
            product_backend: distinct.then_some(product_backend),
            adapter_contract,
        },
        bindings: path(&runtime.value["paths"]["bindings"])?,
        receipt: runtime.path.clone(),
    })
}
