//! 绑定参考数据准备的原计划与协调工具来源。
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};

use super::super::{model, path, unlinked};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{digest, source},
        fixture_control::portable_path,
        inputs::{
            document::{self, Document},
            domain::dataset_timeout,
        },
        monitoring::execution::ProtectedInputs,
    },
};

const RUNNER: &str = "xtask/src/recovery/dataset_prepare.rs";

pub(super) struct Bound {
    pub(super) timeout: Duration,
    pub(super) preflight: Value,
    pub(super) tools_receipt: Value,
    pub(super) plan_path: PathBuf,
    pub(super) source: Value,
    backend: PathBuf,
    plan: Document,
    inventory: source::Inventory,
    protected: ProtectedInputs,
}

impl Bound {
    pub(super) fn capture(backend: &Path, request: &Value, plan: &Value) -> Result<Self> {
        unlinked(backend)?;
        let selected = validate_local_test_path(
            path(&request["plan"])?,
            backend,
            LocalTestPathKind::ExistingFile,
        )?;
        let backend = portable_path(&fs::canonicalize(backend)?);
        let plan_path = portable_path(&fs::canonicalize(&selected)?);
        let runner = backend.join(RUNNER);
        unlinked(&plan_path)?;
        unlinked(&runner)?;
        let protected = ProtectedInputs::open([plan_path.clone(), runner.clone()])?;
        let document = document::read(&plan_path, &backend, "参考数据准备计划")?;
        if &document.value != plan {
            return Err("参考数据准备计划与已核对的计划不同".into());
        }

        let args = json!({"plan": plan});
        let timeout = dataset_timeout(&plan["dataset"])?;
        let preflight = model::preflight(&args)?;
        let inventory = source::capture(&backend)?;
        let snapshot = source::binding_value(&inventory.source)["snapshot"].clone();
        let digest = digest::file(&runner)?;
        if !inventory
            .files
            .iter()
            .any(|file| file.path == RUNNER && file.sha256 == digest.sha256)
        {
            return Err("原生数据准备运行器与完整工具来源清单不同".into());
        }
        let tools_receipt = json!({
            "format_version": 1,
            "kind": "restore-reference-dataset-tools",
            "root": backend,
            "sources": source::backend_domains(&inventory)?,
            "runner": {"path": runner, "bytes": digest.bytes, "sha256": digest.sha256},
        });
        protected.assert_unchanged()?;
        document.assert_unchanged()?;
        Ok(Self {
            timeout,
            preflight,
            tools_receipt,
            plan_path,
            source: snapshot,
            backend,
            plan: document,
            inventory,
            protected,
        })
    }

    pub(super) fn check(&self) -> Result<()> {
        self.protected.assert_unchanged()?;
        if source::capture(&self.backend)? != self.inventory {
            return Err("参考数据准备期间源码或维护工具来源发生变化".into());
        }
        self.protected.assert_unchanged()?;
        self.plan.assert_unchanged()
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/source/evidence/reference_workflow/dataset/binding.rs"]
mod tests;
