//! 只读核验同一正式备份发布者的完整独立副本。
use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::DateTime;
use serde_json::{Value, json};

use super::super::{artifact_shape, exact, files, local::Lock, native, path, text};
use super::inputs::TargetSnapshot;
use crate::{
    Result,
    cli::RestoreBeginCopy,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::inputs::{
        document::{self, Document},
        domain::{identifier, validate_plan},
        inventory::verify_artifacts,
    },
};
use native::target_verify::guard::Inputs;

pub(super) struct VerifiedCopy {
    pub(super) receipt: Document,
    pub(super) root: PathBuf,
    pub(super) manifest: Document,
}

pub(super) struct CopyWindow {
    pub(super) copy: VerifiedCopy,
    publisher: Document,
    lock: Option<Lock>,
}

impl CopyWindow {
    pub(super) fn open(
        backend: &Path,
        snapshot: &TargetSnapshot,
        target_work: &Path,
        options: &RestoreBeginCopy,
    ) -> Result<Self> {
        let publisher = document::read(&options.publisher_plan, backend, "原正式备份发布者")?;
        validate_plan(&publisher.value, backend)?;
        let work = files::work(&json!({"plan":publisher.value}), false)?;
        let work = path(&work)?;
        let lock = if work.canonicalize()? == target_work.canonicalize()? {
            None
        } else {
            Some(Lock::claim(work)?)
        };
        let copy = verify_copy(
            backend,
            &publisher,
            &snapshot.root,
            &snapshot.manifest,
            &options.id,
        )?;
        let window = Self {
            copy,
            publisher,
            lock,
        };
        window.check()?;
        Ok(window)
    }

    pub(super) fn check(&self) -> Result<()> {
        if let Some(lock) = &self.lock {
            lock.check()?;
        }
        self.publisher.assert_unchanged()?;
        self.copy.receipt.assert_unchanged()?;
        self.copy.manifest.assert_unchanged()
    }
}

pub(super) fn directory(
    normal: PathBuf,
    copy: &RestoreBeginCopy,
    original: &Value,
) -> Result<PathBuf> {
    identifier(&copy.id)?;
    if original["id"] == copy.id {
        return Err("失败副本必须有独立演练 ID".into());
    }
    let mut name = normal.into_os_string();
    name.push(format!("-copy-{}", copy.id));
    Ok(PathBuf::from(name))
}

/// 使用原正式备份发布者的参考计划查找副本。恢复侧参考计划可能属于其他目标侧。
fn verify_copy(
    backend: &Path,
    publisher_reference: &Document,
    original_root: &Path,
    original_manifest: &Document,
    copy_id: &str,
) -> Result<VerifiedCopy> {
    identifier(copy_id)?;
    let plan = &publisher_reference.value;
    let work = files::work(&json!({"plan": plan}), false)?;
    let work = path(&work)?;
    if original_root != work.join("backup")
        || original_manifest.path != original_root.join("manifest.json")
        || text(&original_manifest.value["id"])? != text(&plan["id"])?
        || text(&original_manifest.value["scope_id"])? != text(&plan["source"]["scope_id"])?
    {
        return Err("负例副本必须使用原正式备份发布者的根目录、清单和 scope".into());
    }
    let root = work.join(copy_id);
    if copy_id == text(&original_manifest.value["id"])? || root == original_root {
        return Err("失败演练必须使用独立备份ID和目录".into());
    }
    let roots = independent_roots(backend, original_root, &root)?;
    let receipt = document::read(
        &work.join(format!("copy-{copy_id}.json")),
        backend,
        "负例副本成功收据",
    )?;
    let manifest_path = root.join("manifest.json");
    let manifest = document::read(&manifest_path, backend, "负例副本清单")?;
    let documents = [publisher_reference, original_manifest, &receipt, &manifest];
    let guard = Inputs::open(documents.map(|document| document.path.clone()))?;
    verify_receipt(
        &receipt.value,
        plan,
        &document::binding(original_manifest),
        &root,
        &manifest_path,
    )?;
    verify_manifest(&original_manifest.value, &manifest.value, copy_id)?;
    artifact_shape(&manifest.value)?;
    verify_artifacts(&root, &manifest.value)?;
    if independent_roots(backend, original_root, &root)? != roots
        || path(&files::work(&json!({"plan": plan}), false)?)? != work
    {
        return Err("负例副本核验期间原根、副本根或发布者归属变化".into());
    }
    guard.check()?;
    for document in documents {
        document.assert_unchanged()?;
    }
    Ok(VerifiedCopy {
        receipt,
        root,
        manifest,
    })
}

fn independent_roots(backend: &Path, original: &Path, copied: &Path) -> Result<(PathBuf, PathBuf)> {
    for directory in [original, copied] {
        validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
    }
    let original = fs::canonicalize(original)?;
    let copied = fs::canonicalize(copied)?;
    if original.starts_with(&copied) || copied.starts_with(&original) {
        return Err("负例副本根目录与原正式备份根目录重叠".into());
    }
    Ok((original, copied))
}

fn verify_receipt(
    receipt: &Value,
    publisher_plan: &Value,
    original_manifest_binding: &Value,
    copy_root: &Path,
    copy_manifest: &Path,
) -> Result<()> {
    exact(
        receipt,
        &[
            "command",
            "plan_sha256",
            "started_at",
            "status",
            "completed_at",
            "result",
        ],
    )?;
    let started = DateTime::parse_from_rfc3339(text(&receipt["started_at"])?)?;
    let completed = DateTime::parse_from_rfc3339(text(&receipt["completed_at"])?)?;
    if receipt["command"] != "copy"
        || receipt["status"] != "completed"
        || receipt["plan_sha256"] != native::digest::plan_hash(publisher_plan)?
        || completed < started
    {
        return Err("负例必须使用同一正式备份发布者已经完成的复制收据".into());
    }
    let result = &receipt["result"];
    exact(
        result,
        &["manifest", "backup_root", "next", "source_manifest"],
    )?;
    text(&result["next"])?;
    if path(&result["backup_root"])? != copy_root
        || path(&result["manifest"])? != copy_manifest
        || result["source_manifest"] != *original_manifest_binding
    {
        return Err("复制收据的副本路径或原正式备份清单绑定不同".into());
    }
    Ok(())
}

fn verify_manifest(original: &Value, copied: &Value, copy_id: &str) -> Result<()> {
    if text(&original["id"])? == copy_id || text(&copied["id"])? != copy_id {
        return Err("负例副本清单必须具有本次独立复制ID".into());
    }
    let mut expected = original.clone();
    expected["id"] = json!(copy_id);
    if *copied != expected {
        return Err("负例副本清单除独立ID外必须与原正式备份清单精确相同".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_copy_requires_the_exact_publisher_source_and_fixed_paths() {
        let work = if cfg!(windows) {
            Path::new(r"D:\reference-copy-fixture")
        } else {
            Path::new("/reference-copy-fixture")
        };
        let copy_root = work.join("copy-a");
        let copy_manifest = copy_root.join("manifest.json");
        let original = json!({
            "path":work.join("backup/manifest.json"),
            "bytes":17,
            "sha256":"a".repeat(64),
        });
        let plan = json!({"id":"original","target_side":"base","work_dir":work});
        let receipt = json!({
            "command":"copy",
            "plan_sha256":native::digest::plan_hash(&plan).unwrap(),
            "started_at":"2026-11-23T00:00:00+00:00",
            "status":"completed",
            "completed_at":"2026-11-23T00:00:01+00:00",
            "result":{
                "manifest":copy_manifest,
                "backup_root":copy_root,
                "next":"登记副本",
                "source_manifest":original,
            },
        });
        let check = |receipt: &Value, plan: &Value| {
            verify_receipt(receipt, plan, &original, &copy_root, &copy_manifest)
        };
        assert!(check(&receipt, &plan).is_ok());

        let mut wrong_side = plan.clone();
        wrong_side["target_side"] = json!("candidate");
        assert!(check(&receipt, &wrong_side).is_err());
        for (pointer, replacement) in [
            ("/command", json!("backup")),
            ("/status", json!("failed")),
            ("/completed_at", json!("2026-11-22T23:59:59+00:00")),
            ("/result/backup_root", json!(work.join("copy-b"))),
            ("/result/manifest", json!(copy_root.join("renamed.json"))),
            (
                "/result/source_manifest/path",
                json!(work.join("other/manifest.json")),
            ),
            ("/result/source_manifest/bytes", json!(18)),
            ("/result/source_manifest/sha256", json!("b".repeat(64))),
        ] {
            let mut invalid = receipt.clone();
            *invalid.pointer_mut(pointer).unwrap() = replacement;
            assert!(check(&invalid, &plan).is_err(), "{pointer}");
        }
        let mut legacy = receipt.clone();
        legacy["result"]
            .as_object_mut()
            .unwrap()
            .remove("source_manifest");
        assert!(check(&legacy, &plan).is_err());
        legacy = receipt;
        legacy["result"]["unknown"] = json!(true);
        assert!(check(&legacy, &plan).is_err());
    }

    #[test]
    fn copy_manifest_can_change_only_to_its_own_id() {
        let original = json!({
            "id":"original","scope_id":"source",
            "artifacts":[{"sha256":"a".repeat(64),"bytes":1}],
        });
        let mut copy = original.clone();
        assert!(verify_manifest(&original, &copy, "original").is_err());
        assert!(verify_manifest(&original, &copy, "copy-a").is_err());
        copy["id"] = json!("copy-a");
        assert!(verify_manifest(&original, &copy, "copy-a").is_ok());
        copy["artifacts"][0]["bytes"] = json!(2);
        assert!(verify_manifest(&original, &copy, "copy-a").is_err());
    }
}
