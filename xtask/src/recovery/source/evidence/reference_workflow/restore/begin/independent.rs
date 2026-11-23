use std::{cell::Cell, path::Path};

use serde_json::Value;

use super::super::super::local::OwnedWork;
use super::super::inputs::{self, BoundBackup, RegisteredSelection};
use super::{
    Begin, BeginInputs, Maintenance, Result, Scope, Storage, combine, common, document, finish,
    output_file, path, receipt,
};
use crate::{
    cli::RestoreBeginOptions,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{
            copy_transfer::{IndependentTargetBinding, ReferenceTarget},
            target_verify::guard::Inputs,
        },
        inputs::{
            reference::CheckWorkOwner,
            target_plan::{evidence::Frame, independent},
        },
        runtime::{
            dataset_authority::reads::Window,
            restore_guard::registered_stopped_independent_runtime, verify_independent_projection,
        },
    },
};

pub(super) fn run(
    backend: &Path,
    options: &RestoreBeginOptions,
    request: &Value,
    selection: RegisteredSelection,
) -> Result<Value> {
    if options.copy.is_some() {
        return Err("独立 B1 当前只允许同一正式备份，不能登记失败副本".into());
    }
    let snapshot = Snapshot::read(backend, request, selection)?;
    let inputs = snapshot.inputs();
    let work = path(&inputs.plan["work_dir"])?;
    let maintenance = Maintenance::open(&options.maintenance_root, &options.maintenance_build)?;
    let directory = receipt::directory(&inputs)?;
    let output = output_file(backend, work, &inputs, &directory, &options.output)?;
    verify_output_name(work, &directory, &output)?;
    validate_local_test_path(&directory, backend, LocalTestPathKind::NewDirectory)?;
    snapshot.check()?;
    let claim = OwnedWork::claim(backend, inputs.plan)?;
    let work_binding = claim.binding(backend, inputs.plan)?;
    let directory_created = Cell::new(false);
    let owner = |selected: &Path, reference: &Value| claim.check(selected, reference);
    let active = Begin {
        backend,
        work,
        output: &output,
        directory: &directory,
        inputs: &inputs,
        maintenance: &maintenance,
        directory_created: &directory_created,
        copy: None,
        work_binding: Some(&work_binding),
    };
    let result = (|| {
        output_file(backend, work, &inputs, &directory, &output)?;
        registered_stopped_independent_runtime(
            backend,
            &inputs.registration.path,
            &document::binding(inputs.target),
            &owner,
            |stopped| {
                run_with_storage(&active, &snapshot, &|| {
                    owner(work, inputs.plan)?;
                    snapshot.check_runtime(&stopped()?)
                })
            },
        )
    })();
    finish_claimed(&active, result, &|| owner(work, inputs.plan))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_begin_output_rejects_claim_paths_before_work_exists() {
        let work = Path::new("new-work");
        let evidence = work.join("restore-begin-candidate");
        assert!(verify_output_name(work, &evidence, &work.join("running.json")).is_ok());
        for name in [
            "reference-owner.json",
            ".reference-lock",
            "restore-begin-candidate",
            "restore-candidate.json",
            "restore-candidate-before",
            "restore-candidate-after",
            "restore-candidate-failure-after",
        ] {
            assert!(verify_output_name(work, &evidence, &work.join(name)).is_err());
        }
        assert!(
            verify_output_name(work, &evidence, &work.join(".reference-lock/running.json"))
                .is_err()
        );
        #[cfg(windows)]
        for name in [
            "REFERENCE-OWNER.JSON. ",
            ".REFERENCE-LOCK.",
            "running.json:other",
            "RESTORE-CANDIDATE.JSON.",
            "RESTORE-CANDIDATE-BEFORE ",
            "restore-candidate-after.",
            "restore-candidate-failure-after:other",
        ] {
            assert!(verify_output_name(work, &evidence, &work.join(name)).is_err());
        }
    }
}

pub(super) fn verify_output_name(work: &Path, directory: &Path, output: &Path) -> Result<()> {
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("独立 B1 恢复运行记录须有有效文件名")?;
    let receipt = directory.file_name().and_then(|name| name.to_str());
    let aliases = |reserved: &str| {
        if cfg!(windows) {
            name.trim_end_matches(['.', ' '])
                .eq_ignore_ascii_case(reserved)
        } else {
            name == reserved
        }
    };
    if output.parent() != Some(work)
        || cfg!(windows) && name.contains(':')
        || aliases("reference-owner.json")
        || aliases(".reference-lock")
        || receipt.is_some_and(aliases)
        || [
            "restore-candidate.json",
            "restore-candidate-before",
            "restore-candidate-after",
            "restore-candidate-failure-after",
        ]
        .into_iter()
        .any(aliases)
    {
        return Err("独立 B1 恢复运行记录须为参考工作目录下非保留名的新文件".into());
    }
    Ok(())
}

fn finish_claimed(
    active: &Begin<'_>,
    result: Result<(receipt::Intent, document::Document)>,
    owner: &dyn Fn() -> Result<()>,
) -> Result<Value> {
    if let Err(error) = owner() {
        return match result {
            Ok(_) => Err(error),
            Err(cause) => combine(Err(cause), Err(error)),
        };
    }
    let was_registered = result.is_ok();
    let result = if !was_registered && !active.directory_created.get() {
        let prepared =
            common::ensure_new_directory(active.backend, active.directory, "正式登记证据目录");
        if prepared.is_ok() {
            active.directory_created.set(true);
        }
        combine(result, prepared)
    } else {
        result
    };
    let outcome = finish(
        active.backend,
        active.directory,
        active.directory_created,
        result,
    );
    if was_registered && outcome.is_err() {
        let sealed = (|| {
            owner()?;
            if !active.directory.join("result.json").try_exists()? {
                receipt::record_unconfirmed(active.backend, active.directory)?;
            }
            Ok(())
        })();
        return combine(outcome, sealed);
    }
    outcome
}

fn run_with_storage(
    active: &Begin<'_>,
    snapshot: &Snapshot,
    stopped: &dyn Fn() -> Result<()>,
) -> Result<(receipt::Intent, document::Document)> {
    let scope = Scope::load(active.backend, &snapshot.build_bridges, &[])?;
    let storage = Storage::acquire_independent_target(
        active.backend,
        active.work,
        &snapshot.binding.registration,
        &snapshot.binding.generation,
    )?;
    let result = (|| {
        let target = ReferenceTarget::open_independent(
            active.backend,
            snapshot.binding.clone(),
            &storage,
            active.work,
            &scope,
        )?;
        let check = || {
            stopped()?;
            target.check()?;
            active.maintenance.check()
        };
        active.prepare(&target, &check)
    })();
    storage.finish(combine(result, scope.assert_unchanged()))
}

pub(super) struct Snapshot {
    selection: RegisteredSelection,
    backup: BoundBackup,
    backup_receipt: document::Document,
    product_plan: document::Document,
    plan: Value,
    pub(super) binding: IndependentTargetBinding,
    pub(super) build_bridges: Vec<Value>,
    provenance: Inputs,
}

impl Snapshot {
    fn read(backend: &Path, request: &Value, selection: RegisteredSelection) -> Result<Self> {
        let window = Window::direct()?;
        let result = Self::from_plan(backend, request, selection, None);
        window.finish(result)
    }

    pub(super) fn from_plan(
        backend: &Path,
        request: &Value,
        selection: RegisteredSelection,
        owner: Option<&CheckWorkOwner<'_>>,
    ) -> Result<Self> {
        let frame = Frame::published(
            backend,
            &selection.selected.reference,
            selection.selected.target.clone(),
        )?;
        let (evidence, _) = match owner {
            Some(owner) => independent::verify_owned(backend, &frame, owner)?,
            None => independent::verify(backend, &frame, None)?,
        };
        verify_independent_projection(
            backend,
            &selection.selected.reference,
            &selection.selected.target,
            &selection.registration,
            evidence.reference_plan(),
            &evidence.arm.target_request,
        )?;
        let [backup_receipt, comparison, _, _, product_plan] = frame.evidence.as_slice() else {
            return Err("独立 B1 开始登记缺少五项原目标计划证据".into());
        };
        let backup = BoundBackup::read(
            backend,
            path(&request["backup_root"])?,
            backup_receipt,
            &evidence.backup,
        )?;
        let observed = evidence.arm.observed_target();
        let binding = IndependentTargetBinding {
            initialized: observed.initialized.clone(),
            registration: observed.registration.clone(),
            initialized_files: observed.files.clone(),
            environment: observed.environment.clone(),
            generation: observed.generation.clone(),
            state: observed.state.clone(),
        };
        let plan = evidence.reference_plan().clone();
        let build_bridges = evidence.arm.build_bridges()?.to_vec();
        let provenance = inputs::hold_provenance(comparison, product_plan)?;
        let backup_receipt = backup_receipt.clone();
        let product_plan = product_plan.clone();
        evidence.check()?;
        drop(evidence);
        frame.check()?;
        drop(frame);
        let snapshot = Self {
            selection,
            backup,
            backup_receipt,
            product_plan,
            plan,
            binding,
            build_bridges,
            provenance,
        };
        snapshot.check()?;
        Ok(snapshot)
    }

    pub(super) fn inputs(&self) -> BeginInputs<'_> {
        BeginInputs {
            root: &self.backup.root,
            reference: &self.selection.selected.reference,
            plan: &self.plan,
            backup_receipt: &self.backup_receipt,
            target: &self.selection.selected.target,
            manifest: &self.backup.manifest,
            registration: &self.selection.registration,
            product_plan: &self.product_plan,
        }
    }

    pub(super) fn check(&self) -> Result<()> {
        self.provenance.check()?;
        let inputs = self.inputs();
        for guard in [
            &self.selection.registration_guard,
            &self.selection.selected.guard,
            &self.backup.guard,
        ] {
            guard.assert_unchanged()?;
        }
        for document in [
            inputs.reference,
            inputs.target,
            inputs.registration,
            inputs.backup_receipt,
            inputs.manifest,
            inputs.product_plan,
        ] {
            document.assert_unchanged()?;
        }
        Ok(())
    }

    pub(super) fn check_runtime(&self, checkpoint: &Value) -> Result<()> {
        inputs::check_runtime(
            &self.selection.registration,
            &self.selection.selected.target,
            checkpoint,
        )?;
        self.check()
    }
}
