//! 显式来源审计窗口；产品来源与本轮工具来源分开验证。
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{build_source, digest, native_json as document, source, verify_build};
use crate::Result;

#[path = "scope/registration.rs"]
mod registration;

#[derive(PartialEq, Eq)]
struct Registered {
    receipt_hash: String,
    build_identity: String,
    inventory: source::Inventory,
    root: PathBuf,
    inherited: bool,
}

pub(crate) struct Scope {
    root: PathBuf,
    current: source::Inventory,
    entries: Vec<(Value, bool, Registered)>,
}

impl Scope {
    pub(crate) fn product(
        &self,
        execution_root: &Path,
        path: &Path,
        receipt: &Value,
        fingerprint: &str,
    ) -> Result<Value> {
        if receipt["kind"] != "restore-backend-build" {
            return Err("产品来源验证只能接受 API/Worker 构建".into());
        }
        let key = digest::plan_hash(receipt)?;
        let nested = self
            .entries
            .iter()
            .any(|(_, _, item)| item.receipt_hash == key && item.root != self.root);
        let context = if registration::canonical(execution_root)? == self.root && !nested {
            Some(super::build_context::current(execution_root)?)
        } else {
            None
        };
        let source =
            self.verify_source(execution_root, path, receipt, fingerprint, context.as_ref())?;
        if let Some(context) = context
            && super::build_context::current(execution_root)? != context
        {
            return Err("来源验证期间当前构建环境或工具链变化".into());
        }
        self.assert_unchanged()?;
        Ok(source)
    }

    pub(crate) fn maintenance(
        &self,
        execution_root: &Path,
        path: &Path,
        receipt: &Value,
    ) -> Result<Value> {
        use super::maintenance::{DefaultRunner, toolchain};
        if receipt["kind"] != "devex-clone-tool-build" {
            return Err("维护来源验证只能接受维护工具构建".into());
        }
        let versions = toolchain(execution_root, &DefaultRunner)?;
        let fingerprint =
            document::text(&receipt["source"]["worktree_fingerprint"], "维护来源指纹")?;
        let source = self.verify_source(execution_root, path, receipt, fingerprint, None)?;
        if receipt["source"] != source || receipt["toolchain"] != versions {
            return Err("维护工具未绑定已验证的完整来源和当前工具链".into());
        }
        if toolchain(execution_root, &DefaultRunner)? != versions {
            return Err("维护来源验证期间工具链变化".into());
        }
        self.assert_unchanged()?;
        Ok(receipt.clone())
    }

    /// inherited 只由已验证的发布源历史提供，不能来自未验证的请求参数。
    pub(crate) fn load(root: &Path, bindings: &[Value], inherited: &[Value]) -> Result<Self> {
        let root = registration::root(root, root)?;
        let current = source::capture(&root)?;
        let mut entries = Vec::new();
        let mut receipts = BTreeSet::new();
        let mut identities = BTreeSet::new();
        for (binding, inherited) in bindings
            .iter()
            .map(|v| (v, false))
            .chain(inherited.iter().map(|v| (v, true)))
        {
            let value = registration::load(&root, &current, binding, inherited)?;
            if !receipts.insert(value.receipt_hash.clone())
                || !identities.insert(value.build_identity.clone())
            {
                return Err("同一构建不能重复登记来源桥接".into());
            }
            entries.push((binding.clone(), inherited, value));
        }
        let scope = Self {
            root,
            current,
            entries,
        };
        scope.assert_unchanged()?;
        Ok(scope)
    }

    pub(crate) fn assert_unchanged(&self) -> Result<()> {
        for (binding, inherited, expected) in &self.entries {
            if registration::load(&self.root, &self.current, binding, *inherited)? != *expected {
                return Err("阶段结束时构建桥接或冻结产物发生变化".into());
            }
        }
        if source::capture(&self.root)? != self.current {
            return Err("阶段结束时完整源码变化，不能发布成功结果".into());
        }
        Ok(())
    }

    /// current_context 由本次原生工具链观察生成；不能使用构建收据自证当前上下文。
    pub(crate) fn verify_source(
        &self,
        execution_root: &Path,
        path: &Path,
        receipt: &Value,
        fingerprint: &str,
        current_context: Option<&Value>,
    ) -> Result<Value> {
        let execution_root = registration::canonical(execution_root)?;
        let external = if execution_root != self.root {
            Some(source::capture(&execution_root)?)
        } else {
            None
        };
        let before = digest::file(path)?;
        if document::read(path)? != *receipt {
            return Err("来源构建收据与实际绑定文件不同".into());
        }
        verify_build(&execution_root, path, receipt)?;
        let original = self.reusable(&execution_root, receipt, current_context)?;
        let expected = build_source(receipt)?;
        let selected = match original {
            Some(value) => value,
            None => {
                let current = external.as_ref().unwrap_or(&self.current);
                let actual = source::binding_value(&current.source);
                if expected["snapshot"] != actual["snapshot"] {
                    return Err("实际源码与本侧精确构建快照不同".into());
                }
                actual
            }
        };
        if selected["worktree_fingerprint"].as_str() != Some(fingerprint) {
            return Err("请求没有绑定构建时的完整工作区指纹".into());
        }
        if digest::file(path)? != before || document::read(path)? != *receipt {
            return Err("来源验证期间构建收据变化；保留证据，禁止自动重放".into());
        }
        if let Some(previous) = external
            && source::capture(&execution_root)? != previous
        {
            return Err("独立执行后端的完整来源在核验期间变化".into());
        }
        self.assert_unchanged()?;
        Ok(selected)
    }

    fn reusable(
        &self,
        execution_root: &Path,
        receipt: &Value,
        current_context: Option<&Value>,
    ) -> Result<Option<Value>> {
        if registration::canonical(execution_root)? != self.root {
            return Ok(None);
        }
        let key = digest::plan_hash(receipt)?;
        let registered = self
            .entries
            .iter()
            .map(|(_, _, item)| item)
            .find(|item| item.receipt_hash == key);
        match receipt["kind"].as_str() {
            Some("restore-backend-build") => {
                let nested = registered.is_some_and(|item| item.root != self.root);
                if !nested
                    && !registered.is_some_and(|item| item.inherited)
                    && receipt["sources"]["product"]
                        != source::backend_domains(&self.current)?["product"]
                {
                    return Err("产品构建输入已经变化，必须重新编译".into());
                }
                if !nested && current_context != receipt.get("build") {
                    return Err("构建命令、工具链或有效环境已经变化，必须重新编译".into());
                }
                Ok(Some(build_source(receipt)?))
            }
            Some("devex-clone-tool-build") => self.maintenance_source(receipt, registered),
            _ => Err("来源复用只接受明确的后端产品或维护构建".into()),
        }
    }

    fn maintenance_source(
        &self,
        receipt: &Value,
        registered: Option<&Registered>,
    ) -> Result<Option<Value>> {
        let inventory = match registered {
            Some(item) => item.inventory.clone(),
            None => match receipt.get("source_inventory") {
                Some(value) => source::validate_inventory(value)?,
                None => return Ok(None),
            },
        };
        super::verify_inventory_source(&inventory, receipt)?;
        if !registered.is_some_and(|item| item.inherited)
            && source::fingerprints(&inventory.files)?["product"]
                != source::fingerprints(&self.current.files)?["product"]
        {
            return Err("维护构建的产品输入已经变化，必须重新编译".into());
        }
        Ok(Some(source::binding_value(&inventory.source)))
    }
}
