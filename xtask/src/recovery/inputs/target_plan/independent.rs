//! 独立 B1 目标计划的只读重投影；读取窗口由调用方持有。
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{
    Result,
    cli::RestoreInputSide,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::cache::binding::source::relationship::paths_overlap,
        fixture_control::portable_path,
        inputs::{
            backup,
            document::{self, Document},
            independent_b1::Verified,
            reference,
        },
        source::evidence::comparison,
    },
};

use super::{
    capture_independent_b1,
    evidence::{Frame, directory_identity},
};

pub(in crate::recovery) fn verify<'a>(
    backend: &'a Path,
    frame: &Frame<'a>,
    output: Option<&Path>,
) -> Result<(Verified<'a>, Value)> {
    frame.check()?;
    let [backup_file, comparison_file, arm, fresh, product] = frame.evidence.as_slice() else {
        return Err("参考目标计划必须绑定五项原证据".into());
    };
    let evidence = Verified::load(
        backend,
        frame.reference,
        [
            &backup_file.path,
            &comparison_file.path,
            &arm.path,
            &fresh.path,
        ],
        RestoreInputSide::Candidate,
        output,
    )?;
    let value = capture_independent_b1(backend, &evidence, product)?;
    require_readback(frame.target.as_ref(), &value)?;
    frame.check_bindings(&value)?;
    backup::verify_frozen_source(backend, &evidence.backup)?;
    comparison::verify(backend, &evidence.comparison_document.value, true)?;
    frame.verify_fresh_inventory(backend, &evidence.arm.target_request.document.value)?;
    evidence.check()?;
    frame.check()?;
    Ok((evidence, value))
}

pub(in crate::recovery) fn require_readback(
    selected: Option<&Document>,
    expected: &Value,
) -> Result<()> {
    if selected.is_some_and(|target| &target.value != expected) {
        return Err("独立 B1 正式目标计划与当前完整来源、ownership 或产品计划不同".into());
    }
    Ok(())
}

pub(in crate::recovery) fn registration_paths(
    evidence: &Verified<'_>,
    frame: &Frame<'_>,
    output: &Path,
    sidecar: &Path,
) -> Result<()> {
    let [backup, _, _, fresh, _] = frame.evidence.as_slice() else {
        return Err("恢复登记必须绑定五项原证据".into());
    };
    let backup_work = backup.path.parent().ok_or("备份收据缺少父目录")?;
    let fresh_observation = fresh.path.parent().ok_or("目标 verify 缺少父目录")?;
    let backup_root = Path::new(document::text(
        &evidence.backup.result["backup_root"],
        "正式备份根目录",
    )?);
    let initialized =
        document::binding_shape(&evidence.arm.observed_target().initialized, "原目标初始化")?;
    let initialized_parent = initialized.path.parent().ok_or("原目标初始化缺少父目录")?;
    let work = Path::new(document::text(
        &evidence.reference_plan()["work_dir"],
        "参考工作目录",
    )?);
    let protected = [
        canonical_location(backup_work)?,
        canonical_location(fresh_observation)?,
        canonical_location(backup_root)?,
        canonical_location(initialized_parent)?,
        canonical_location(work)?,
    ];
    let physical = [
        directory_identity(backup_root)?,
        directory_identity(initialized_parent)?,
    ];
    let destinations = registration_destinations(
        [output, sidecar],
        [work, evidence.arm.copy_directory()],
        &protected,
        &physical,
    )?;
    for path in destinations {
        evidence.arm.require_disjoint(&path)?;
    }
    Ok(())
}

fn registration_destinations(
    [output, sidecar]: [&Path; 2],
    reserved: [&Path; 2],
    protected: &[PathBuf],
    physical: &[(u64, u64)],
) -> Result<[PathBuf; 2]> {
    let destinations = [canonical_location(output)?, canonical_location(sidecar)?];
    for path in &destinations {
        for directory in reserved {
            reference::validate_publication_path(directory, path)?;
        }
        require_outside(path, protected)?;
        for ancestor in path.ancestors().skip(1) {
            if physical.contains(&directory_identity(ancestor)?) {
                return Err("独立 B1 登记输出和配置侧件不得位于原备份根或原目标初始化目录".into());
            }
        }
    }
    Ok(destinations)
}

fn require_outside(path: &Path, protected: &[PathBuf]) -> Result<()> {
    if protected
        .iter()
        .any(|original| paths_overlap(&path.to_string_lossy(), &original.to_string_lossy()))
    {
        return Err("独立 B1 登记输出和配置侧件须与原备份、fresh 观察及参考工作目录分离".into());
    }
    Ok(())
}

fn canonical_location(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().ok_or("独立 B1 登记路径缺少父目录")?;
    validate_absolute_path(parent, LocalTestPathKind::ExistingDirectory)?;
    let name = path.file_name().ok_or("独立 B1 登记路径缺少名称")?;
    Ok(portable_path(&parent.canonicalize()?).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_registration_keeps_output_and_sidecar_outside_protected_locations() -> Result<()>
    {
        let fixture = crate::recovery_inputs_fixture::Fixture::new();
        let work = fixture.root.join("prospective-work");
        let copy = fixture.root.join("prospective-copy");
        let output = fixture.root.join("registration.json");
        let sidecar = fixture.root.join("registration.json-configuration");
        let backup = fixture.backup_receipt.parent().unwrap();
        let fresh = fixture.fresh_verify.parent().unwrap();
        let backup_root = fixture.root.join("original-backup-root");
        let initialized_parent = fixture.root.join("original-initialized");
        std::fs::create_dir(&backup_root)?;
        std::fs::create_dir(&initialized_parent)?;
        assert!(!backup_root.starts_with(backup));
        assert!(!initialized_parent.starts_with(fresh));
        let protected = [canonical_location(backup)?, canonical_location(fresh)?];
        let physical = [
            directory_identity(&backup_root)?,
            directory_identity(&initialized_parent)?,
        ];
        let reserved = [work.as_path(), copy.as_path()];
        let destinations = |output: &Path, sidecar: &Path| {
            registration_destinations([output, sidecar], reserved, &protected, &physical)
        };
        destinations(&output, &sidecar)?;
        for collision in [
            &work,
            &copy,
            &backup.join("registration"),
            &fresh.join("registration"),
            &backup_root.join("registration"),
            &initialized_parent.join("registration"),
        ] {
            assert!(destinations(collision, &sidecar).is_err());
            assert!(destinations(&output, collision).is_err());
        }
        #[cfg(windows)]
        for alias in [
            "PROSPECTIVE-WORK.",
            "prospective-work ",
            "PROSPECTIVE-COPY.",
            "prospective-copy ",
        ] {
            let alias = fixture.root.join(alias);
            assert!(destinations(&alias, &sidecar).is_err());
            assert!(destinations(&output, &alias).is_err());
        }
        assert!(!work.exists() && !copy.exists() && !output.exists() && !sidecar.exists());
        Ok(())
    }
}
