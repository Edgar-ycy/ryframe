//! 固定参考恢复的原始输入，执行前预留同侧收据和三相证据路径。
use std::{
    fs,
    io::ErrorKind,
    ops::Deref,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{path, proof};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, same_local_test_file, validate_local_test_path},
    recovery::{
        clone::native::target_verify::guard::Inputs,
        fixture_control::portable_path,
        inputs::{
            backup,
            chain::Chain,
            document::{self, Document},
            target_plan,
        },
        monitoring::execution::ProtectedInputs,
        source::evidence::comparison,
    },
};

pub(super) struct TargetSnapshot {
    pub(super) root: PathBuf,
    pub(super) chain: Chain,
    pub(super) target: Document,
    pub(super) manifest: Document,
    pub(super) registration: Document,
    pub(super) product_plan: Document,
    protected: [ProtectedInputs; 3],
    provenance: Inputs,
}

impl TargetSnapshot {
    pub(super) fn read(backend: &Path, request: &Value, read_only: bool) -> Result<Self> {
        let snapshot = Self::load(backend, request)?;
        snapshot.verify_source(backend, read_only)?;
        Ok(snapshot)
    }

    fn load(backend: &Path, request: &Value) -> Result<Self> {
        let registration_path = validate_local_test_path(
            path(&request["runtime_registration"])?,
            backend,
            LocalTestPathKind::ExistingFile,
        )?;
        let registration_path = portable_path(&fs::canonicalize(registration_path)?);
        let registration_guard = ProtectedInputs::open([registration_path.clone()])?;
        let registration = document::read(&registration_path, backend, "恢复运行登记")?;
        let selected = Selected::read(backend, request, &registration)?;
        let (chain, verified, product_plan) =
            target_plan::verify(backend, selected.reference, &selected.target.path)?;
        if verified != selected.target.value {
            return Err("恢复目标计划在读取与核验之间发生变化".into());
        }
        let provenance = hold_provenance(&chain.comparison_document, &product_plan)?;
        let manifest_binding = &chain.backup.result["manifest"];
        let manifest_path = document::binding_shape(manifest_binding, "正式备份清单")?.path;
        let backup_guard =
            ProtectedInputs::open([chain.backup_document.path.clone(), manifest_path])?;
        let (_, manifest) = document::bound_file(backend, manifest_binding, "正式备份清单")?;
        if manifest.value != chain.backup.manifest {
            return Err("备份清单在读取与核验之间发生变化".into());
        }
        let root = backup_root(
            backend,
            path(&request["backup_root"])?,
            &chain.backup.result,
            &manifest.path,
        )?;
        Ok(Self {
            root,
            chain,
            target: selected.target,
            manifest,
            registration,
            product_plan,
            protected: [registration_guard, selected.guard, backup_guard],
            provenance,
        })
    }

    fn verify_source(&self, backend: &Path, read_only: bool) -> Result<()> {
        backup::verify_frozen_source(backend, &self.chain.backup)?;
        // 仅持原运行守卫的执行态调用允许比较入口重建 B0 适配证据。
        comparison::verify(backend, &self.chain.comparison_document.value, read_only)?;
        self.provenance.check()?;
        self.assert_unchanged()
    }

    pub(super) fn assert_unchanged(&self) -> Result<()> {
        check_provenance(&self.provenance)?;
        for guard in &self.protected {
            guard.assert_unchanged()?;
        }
        for document in [
            &self.chain.reference,
            &self.target,
            &self.chain.backup_document,
            &self.manifest,
            &self.registration,
        ] {
            document.assert_unchanged()?;
        }
        Ok(())
    }

    pub(super) fn check_runtime(&self, checkpoint: &Value) -> Result<()> {
        check_runtime(&self.registration, &self.target, checkpoint)?;
        self.assert_unchanged()
    }
}

pub(super) struct Snapshot {
    selected: TargetSnapshot,
    pub(super) record: Document,
    pub(super) begin_receipt: Document,
    record_guard: ProtectedInputs,
}

impl Deref for Snapshot {
    type Target = TargetSnapshot;

    fn deref(&self) -> &Self::Target {
        &self.selected
    }
}

impl Snapshot {
    pub(super) fn read(backend: &Path, request: &Value, read_only: bool) -> Result<Self> {
        let (record, record_guard) = running_record(backend, request)?;
        let selected = TargetSnapshot::load(backend, request)?;
        proof::record(&json!({
            "plan":selected.chain.reference.value,"manifest":selected.manifest.value,
            "record":record.value,"product_plan":selected.product_plan.value,
        }))?;
        let begin_receipt = super::begin::verify_record(backend, &selected, &record)?;
        selected.verify_source(backend, read_only)?;
        let snapshot = Self {
            selected,
            record,
            begin_receipt,
            record_guard,
        };
        snapshot.check_record()?;
        Ok(snapshot)
    }

    fn check_record(&self) -> Result<()> {
        self.record_guard.assert_unchanged()?;
        self.record.assert_unchanged()?;
        self.begin_receipt.assert_unchanged()
    }

    pub(super) fn assert_unchanged(&self) -> Result<()> {
        self.selected.assert_unchanged()?;
        self.check_record()
    }

    pub(super) fn bindings(&self) -> Value {
        json!({"target_side":self.target.value["target_side"],"target_plan":document::binding(&self.target),
            "backup_receipt":document::binding(&self.chain.backup_document),"record":document::binding(&self.record),
            "begin_receipt":document::binding(&self.begin_receipt),
            "runtime_registration":document::binding(&self.registration)})
    }

    pub(super) fn check_runtime(&self, checkpoint: &Value) -> Result<()> {
        self.selected.check_runtime(checkpoint)?;
        self.check_record()
    }

    pub(super) fn check_final(&self) -> Result<()> {
        self.provenance.check()?;
        self.assert_unchanged()
    }

    pub(super) fn output(&self, backend: &Path) -> Result<PathBuf> {
        output(
            backend,
            &self.chain.reference.value,
            &self.target.value,
            &self.root,
        )
    }
}

fn hold_provenance(comparison: &Document, product: &Document) -> Result<Inputs> {
    let held = Inputs::open([comparison.path.clone(), product.path.clone()])?;
    // 防止在正式计划验证和保护句柄打开之间接受新字节。
    comparison.assert_unchanged()?;
    product.assert_unchanged()?;
    held.check()?;
    Ok(held)
}

fn check_provenance(held: &Inputs) -> Result<()> {
    // Windows 的句柄阻止写入和替换，写入检查点无需反复读取完整来源清单。
    #[cfg(windows)]
    return held.checkpoint();
    #[cfg(not(windows))]
    held.check()
}

struct Selected {
    reference: Document,
    target: Document,
    guard: ProtectedInputs,
}

impl Selected {
    fn read(backend: &Path, request: &Value, registration: &Document) -> Result<Self> {
        let reference_binding = &registration.value["reference_plan"];
        let target_binding = &registration.value["target_plan"];
        let reference_path = registered_path(backend, path(&request["plan"])?, reference_binding)?;
        let target_path = registered_path(backend, path(&request["target_plan"])?, target_binding)?;
        let guard = ProtectedInputs::open([reference_path, target_path])?;
        let (_, reference) = document::bound_file(backend, reference_binding, "登记参考计划")?;
        let (_, target) = document::bound_file(backend, target_binding, "登记目标计划")?;
        Ok(Self {
            reference,
            target,
            guard,
        })
    }
}

fn running_record(backend: &Path, request: &Value) -> Result<(Document, ProtectedInputs)> {
    let path = validate_local_test_path(
        path(&request["record"])?,
        backend,
        LocalTestPathKind::ExistingFile,
    )?;
    let guard = ProtectedInputs::open([path.clone()])?;
    let record = document::read(&path, backend, "产品恢复运行记录")?;
    Ok((record, guard))
}

fn registered_path(backend: &Path, selected: &Path, descriptor: &Value) -> Result<PathBuf> {
    let binding = document::binding_shape(descriptor, "恢复登记的计划文件")?;
    if !same_local_test_file(backend, selected, &binding.path)? {
        return Err("恢复命令必须使用登记的同一计划文件".into());
    }
    Ok(binding.path)
}

fn check_runtime(registration: &Document, target: &Document, checkpoint: &Value) -> Result<()> {
    if *checkpoint
        != json!({"registration":document::binding(registration),"target_plan":document::binding(target)})
    {
        return Err("恢复守卫检查点未绑定同一登记和目标计划".into());
    }
    Ok(())
}

fn backup_root(
    backend: &Path,
    selected: &Path,
    result: &Value,
    manifest: &Path,
) -> Result<PathBuf> {
    let registered = path(&result["backup_root"])?;
    for directory in [selected, registered] {
        validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
    }
    if fs::canonicalize(selected)? != fs::canonicalize(registered)?
        || manifest != registered.join("manifest.json")
    {
        return Err("恢复只能消费目标计划绑定的同一正式备份目录".into());
    }
    Ok(registered.to_owned())
}

fn output(backend: &Path, reference: &Value, target: &Value, root: &Path) -> Result<PathBuf> {
    let side = match reference["target_side"].as_str() {
        Some(side @ ("base" | "candidate")) if target["target_side"] == side => side,
        _ => return Err("恢复收据必须绑定同一 base 或 candidate 目标侧".into()),
    };
    let work = path(&reference["work_dir"])?;
    let output = work.join(format!("restore-{side}.json"));
    validate_local_test_path(&output, backend, LocalTestPathKind::OutputFile)?;
    let initialized = path(&target["fresh_target"]["initialized"]["path"])?;
    let initialized = initialized
        .parent()
        .ok_or("fresh-target 初始化证据缺少父目录")?;
    if inside_existing_root(work, root)? || inside_existing_root(work, initialized)? {
        return Err("恢复输出不能覆盖备份或 fresh-target 初始化证据".into());
    }
    // 预检发生在 work 创建前，三个相位目录此时只要求路径安全且尚不存在。
    for phase in ["before", "after", "failure-after"] {
        let directory = work.join(format!("restore-{side}-{phase}"));
        validate_local_test_path(&directory, backend, LocalTestPathKind::OutputFile)?;
    }
    Ok(output)
}

fn inside_existing_root(work: &Path, root: &Path) -> Result<bool> {
    let root = fs::canonicalize(root)?;
    for path in work.ancestors() {
        match fs::canonicalize(path) {
            Ok(existing) => return Ok(existing.starts_with(&root)),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("恢复工作目录没有可信的现有父目录".into())
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/source/evidence/reference_workflow/restore/inputs.rs"]
mod tests;
