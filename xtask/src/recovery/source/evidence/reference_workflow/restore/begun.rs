//! 已成功开始且尚未进入写入阶段的独立 B1，只借用原现场证明。
use std::{fs, io::ErrorKind, path::Path};

use serde_json::{Value, json};

use super::{
    super::{local::OriginalWork, proof},
    begin::{BeginInputs, SealedBegin, independent_plan},
    combine, document,
    inputs::BoundBackup,
    path,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        inputs::{
            document::Document,
            independent_b1::Verified,
            target_plan::{evidence::Frame, independent},
        },
        runtime::dataset_authority::reads,
    },
};

/// 在调用方的读取窗口内重验原开始登记、工作目录和对同一原目标的投影。
pub(in crate::recovery) fn with_begun_independent_target<T>(
    backend: &Path,
    reference: &Document,
    target: &Document,
    registration: &Document,
    visit: impl FnOnce(&Verified<'_>) -> Result<T>,
) -> Result<T> {
    if !reads::is_active() {
        return Err("独立 B1 开始登记权威核验要求现有只读窗口".into());
    }
    let plan = independent_plan(reference, target)?;
    let directory = path(&plan["work_dir"])?;
    validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
    require_unapplied(directory)?;
    let (begin, running) = SealedBegin::open_recorded(backend, plan, &target.value)?;
    let work = OriginalWork::read_completed(backend, plan, begin.work())?;
    let check_work = |selected: &Path, projected: &Value| {
        if projected != plan {
            return Err("独立 B1 当前投影与原开始登记的参考计划不同".into());
        }
        work.check_completed(selected, projected)?;
        require_unapplied(selected)
    };
    verify_registration(registration, reference, target)?;
    let frame = Frame::published(backend, reference, target.clone())?;
    let (evidence, _) = independent::verify_owned(backend, &frame, &check_work)?;
    let [backup_receipt, _, _, _, product] = frame.evidence.as_slice() else {
        return Err("独立 B1 原开始登记缺少五项原目标计划证据".into());
    };
    let backup_root = path(&evidence.backup.result["backup_root"])?;
    let backup = BoundBackup::read(backend, backup_root, backup_receipt, &evidence.backup)?;
    let inputs = BeginInputs {
        root: &backup.root,
        reference,
        plan: evidence.reference_plan(),
        backup_receipt,
        target,
        manifest: &backup.manifest,
        registration,
        product_plan: product,
    };
    proof::running_record(
        inputs.plan,
        &inputs.manifest.value,
        &running.value,
        &product.value,
    )?;
    let images = begin.verify_images(backend, &inputs, &running)?;
    let check = || {
        evidence.check()?;
        frame.check()?;
        registration.assert_unchanged()?;
        running.assert_unchanged()?;
        backup.guard.assert_unchanged()?;
        backup.manifest.assert_unchanged()?;
        begin.check()?;
        images.check()?;
        check_work(directory, inputs.plan)
    };
    check()?;
    let result = visit(&evidence);
    combine(result, check())
}

fn verify_registration(
    registration: &Document,
    reference: &Document,
    target: &Document,
) -> Result<()> {
    let value = &registration.value;
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "reference_plan",
            "target_plan",
            "observation",
            "runtime_environment",
            "remote_writes",
        ],
        "独立 B1 原运行登记",
    )?;
    if value["format_version"] != 2
        || value["kind"] != "restore-runtime-registration"
        || value["remote_writes"] != 0
        || value["reference_plan"] != document::binding(reference)
        || value["target_plan"] != document::binding(target)
        || value["observation"]["ports_idle"] != json!(["api", "worker", "frontend"])
        || value["observation"]["runtime"]["entries"] != json!([])
    {
        return Err("独立 B1 原运行登记和静态停止证明未绑定同一参考计划及目标".into());
    }
    document::binding_shape(&value["runtime_environment"], "独立 B1 原运行私有环境绑定")?;
    registration.assert_unchanged()
}

fn require_unapplied(directory: &Path) -> Result<()> {
    for name in [
        "restore-candidate.json",
        "restore-candidate-before",
        "restore-candidate-after",
        "restore-candidate-failure-after",
    ] {
        let file = directory.join(name);
        match fs::symlink_metadata(&file) {
            Err(error) if error.kind() == ErrorKind::NotFound => reads::optional(&file)?,
            Err(error) => return Err(error.into()),
            Ok(_) => return Err("独立 B1 已出现写入阶段的原收据或前后像".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begun_independent_target_rejects_all_partial_apply_paths() -> Result<()> {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local)?;
        let temp = tempfile::tempdir_in(local)?;
        let directory = temp.path();
        require_unapplied(directory)?;
        let receipt = directory.join("restore-candidate.json");
        fs::write(&receipt, b"{}")?;
        assert!(require_unapplied(directory).is_err());
        fs::remove_file(receipt)?;
        for name in [
            "restore-candidate-before",
            "restore-candidate-after",
            "restore-candidate-failure-after",
        ] {
            let marker = directory.join(name);
            fs::create_dir(&marker)?;
            assert!(require_unapplied(directory).is_err(), "{name}");
            fs::remove_dir(marker)?;
        }
        require_unapplied(directory)
    }
}
