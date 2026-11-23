//! 显式来源审计窗口；产品来源与本轮工具来源分开验证。
use std::{
    borrow::Cow,
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{build_source, digest, native_json as document, source, verify_build};
use crate::Result;

#[cfg(windows)]
use crate::recovery::fixture_control::source_pair::execution::HeldSourceRoot;
#[cfg(windows)]
use std::collections::{BTreeMap, btree_map::Entry};

#[path = "scope/registration.rs"]
mod registration;

#[derive(PartialEq, Eq)]
struct Registered {
    receipt_hash: String,
    build_path: PathBuf,
    build_sha256: String,
    build_identity: String,
    inventory: source::Inventory,
    #[cfg(windows)]
    audited: Option<source::Inventory>,
    root: PathBuf,
    inherited: bool,
}

pub(crate) struct Scope {
    root: PathBuf,
    current: source::Inventory,
    entries: Vec<(Value, bool, Registered)>,
    #[cfg(windows)]
    held: Option<HeldSources>,
}

impl Scope {
    /// 只读取本窗口已核验的唯一产品桥接，不为未登记构建推断所属根。
    pub(crate) fn registered_product_root(&self, path: &Path, receipt: &Value) -> Result<PathBuf> {
        self.assert_unchanged()?;
        let result = (|| {
            if receipt["kind"] != "restore-backend-build" || document::read(path)? != *receipt {
                return Err("产品根必须绑定实际API/Worker构建收据".into());
            }
            let sha = digest::file(path)?.sha256;
            let key = digest::plan_hash(receipt)?;
            let mut matches = self.entries.iter().filter(|(_, _, item)| {
                item.build_path == path && item.build_sha256 == sha && item.receipt_hash == key
            });
            let (_, _, entry) = matches.next().ok_or("产品构建没有精确匹配的已登记桥接")?;
            if matches.next().is_some() {
                return Err("产品构建所属桥接不唯一".into());
            }
            Ok(entry.root.clone())
        })();
        self.assert_unchanged()?;
        result
    }

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
            let value = registration::load(&root, &current, binding, inherited, capture_matches)?;
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
            #[cfg(windows)]
            held: None,
        };
        scope.assert_unchanged()?;
        Ok(scope)
    }

    pub(crate) fn assert_unchanged(&self) -> Result<()> {
        #[cfg(windows)]
        if let Some(held) = &self.held {
            HeldSources::require_independent_window()?;
            self.check_registrations(|root, expected| held.matches(root, expected))?;
            return held.checkpoint();
        }
        self.check_registrations(capture_matches)?;
        if source::capture(&self.root)? != self.current {
            return Err("阶段结束时完整源码变化，不能发布成功结果".into());
        }
        Ok(())
    }

    /// 按原字节和 Git 规则复核所有持有根，直到完整观察结束都保留原句柄。
    pub(crate) fn assert_unchanged_full(&self) -> Result<()> {
        #[cfg(windows)]
        if let Some(held) = &self.held {
            held.verify_full()?;
        }
        self.assert_unchanged()
    }

    /// 在原审计结束后持有协调根、非继承审计根及明确的独立执行根。
    #[cfg(windows)]
    pub(crate) fn hold_windows_sources(&mut self, execution_roots: &[&Path]) -> Result<()> {
        if self.held.is_some() {
            return Err("源码窗口已经持有，不能替换已核验的根".into());
        }
        let held = HeldSources::open(self, execution_roots)?;
        self.check_registrations(|root, expected| held.matches(root, expected))?;
        held.verify_full()?;
        self.held = Some(held);
        Ok(())
    }

    fn check_registrations(
        &self,
        matches_source: impl Fn(&Path, &source::Inventory) -> Result<bool>,
    ) -> Result<()> {
        for (binding, inherited, expected) in &self.entries {
            if registration::load(
                &self.root,
                &self.current,
                binding,
                *inherited,
                &matches_source,
            )? != *expected
            {
                return Err("阶段结束时构建桥接或冻结产物发生变化".into());
            }
        }
        Ok(())
    }

    fn observe_source(&self, root: &Path) -> Result<Cow<'_, source::Inventory>> {
        #[cfg(windows)]
        if let Some(held) = &self.held {
            HeldSources::require_independent_window()?;
            let source = held.get(root)?;
            source.handles.checkpoint()?;
            return Ok(Cow::Borrowed(&source.inventory));
        }
        source::capture(root).map(Cow::Owned)
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
            Some(self.observe_source(&execution_root)?)
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
                let current = external.as_deref().unwrap_or(&self.current);
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
            && self.observe_source(&execution_root)? != previous
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
        let fingerprints = super::verify_inventory_source(&inventory, receipt)?;
        if !registered.is_some_and(|item| item.inherited)
            && fingerprints["product"] != source::fingerprints(&self.current.files)?["product"]
        {
            return Err("维护构建的产品输入已经变化，必须重新编译".into());
        }
        Ok(Some(source::binding_value(&inventory.source)))
    }
}

fn capture_matches(root: &Path, expected: &source::Inventory) -> Result<bool> {
    Ok(&source::capture(root)? == expected)
}

#[cfg(windows)]
struct HeldSources(BTreeMap<PathBuf, HeldInventory>);

#[cfg(windows)]
struct HeldInventory {
    inventory: source::Inventory,
    handles: HeldSourceRoot,
}

#[cfg(windows)]
impl HeldSources {
    fn open(scope: &Scope, execution_roots: &[&Path]) -> Result<Self> {
        Self::require_independent_window()?;
        let mut inventories = BTreeMap::from([(scope.root.clone(), scope.current.clone())]);
        for (_, inherited, entry) in &scope.entries {
            if *inherited {
                continue;
            }
            let audited = entry.audited.as_ref().ok_or("待持有来源缺少实际审计库存")?;
            match inventories.entry(entry.root.clone()) {
                Entry::Occupied(previous) if previous.get() != audited => {
                    return Err("同一待持有源码根的实际审计库存不同".into());
                }
                Entry::Occupied(_) => {}
                Entry::Vacant(slot) => {
                    slot.insert(audited.clone());
                }
            }
        }
        for root in execution_roots {
            let root = registration::canonical(root)?;
            if let Entry::Vacant(slot) = inventories.entry(root) {
                let inventory = source::capture(slot.key())?;
                slot.insert(inventory);
            }
        }
        let roots = inventories
            .into_iter()
            .map(|(root, inventory)| {
                let handles = HeldSourceRoot::open_backend(&root, &inventory)?;
                Ok((root, HeldInventory { inventory, handles }))
            })
            .collect::<Result<_>>()?;
        Ok(Self(roots))
    }

    fn get(&self, root: &Path) -> Result<&HeldInventory> {
        self.0
            .get(root)
            .ok_or_else(|| "请求的执行源码根不在已持有的范围内".into())
    }

    fn matches(&self, root: &Path, expected: &source::Inventory) -> Result<bool> {
        Ok(&self.get(root)?.inventory == expected)
    }

    fn require_independent_window() -> Result<()> {
        if crate::recovery::runtime::dataset_authority::reads::is_active() {
            return Err("已持有来源不能替代另一静态读取窗口要求的原完整文件登记".into());
        }
        Ok(())
    }

    fn checkpoint(&self) -> Result<()> {
        for source in self.0.values() {
            source.handles.checkpoint()?;
        }
        Ok(())
    }

    fn verify_full(&self) -> Result<()> {
        Self::require_independent_window()?;
        self.checkpoint()?;
        for (root, source) in &self.0 {
            if !capture_matches(root, &source.inventory)? {
                return Err("持有期间原完整源码观察与已登记库存不一致".into());
            }
        }
        self.checkpoint()
    }
}
