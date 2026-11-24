use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{files, inputs, model, native, path, protocol, receipts};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{resources::common, target_verify::guard::Inputs},
        fixture_control::portable_path,
        monitoring::execution::ProtectedInputs,
        runtime::dataset_authority::reads,
    },
};

pub(super) struct Lock {
    path: PathBuf,
    identity: (u64, u64),
    release_on_drop: bool,
}

const REFERENCE_LOCK: &str = ".reference-lock";

impl Lock {
    pub(super) fn claim(work: &Path) -> Result<Self> {
        let path = work.join(REFERENCE_LOCK);
        fs::create_dir(&path)?;
        let identity = Self::identity(&path)?;
        Ok(Self {
            path,
            identity,
            release_on_drop: true,
        })
    }

    pub(super) fn retain(&mut self) {
        self.release_on_drop = false;
    }

    pub(super) fn check(&self) -> Result<()> {
        if Self::identity(&self.path)? != self.identity
            || fs::read_dir(&self.path)?.next().transpose()?.is_some()
        {
            return Err("参考阶段锁的原目录身份或内容变化，保留现场".into());
        }
        Ok(())
    }

    fn identity(path: &Path) -> Result<(u64, u64)> {
        super::unlinked(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            return Err("参考阶段锁必须为普通目录".into());
        }
        native::resources::common::device_inode_for(path, &metadata)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // 只移除本阶段创建的空目录；异常现场或外来文件保持原样。
        if self.release_on_drop && self.check().is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

/// 持有当前参考阶段的目录、归属文件与独占锁。
pub(super) struct OwnedWork {
    directory: PathBuf,
    identity: (u64, u64),
    directory_guard: Inputs,
    owner_path: PathBuf,
    owner_bytes: Vec<u8>,
    owner_guard: ProtectedInputs,
    lock: Lock,
    reentry: Option<Reentry>,
}

struct Reentry {
    parent: Inputs,
    owner_identity: (u64, u64),
}

impl OwnedWork {
    pub(super) fn claim(backend: &Path, reference: &Value) -> Result<Self> {
        if reads::is_active() {
            return Err("独占创建独立 B1 工作目录前必须结束原缺席读取窗口".into());
        }
        if reference["target_side"] != "candidate" {
            return Err("独立 B1 工作目录只允许 candidate 侧开始登记".into());
        }
        inputs::domain::identifier(inputs::document::text(
            &reference["id"],
            "独立 B1 参考计划 ID",
        )?)?;
        let declared = path(&reference["work_dir"])?;
        let directory = inputs::reference::new_independent_work_dir(backend, declared)?;
        if directory != declared
            || cfg!(windows)
                && directory
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(['.', ' ']))
        {
            return Err("独立 B1 工作目录必须使用原规范路径".into());
        }
        let parent = directory.parent().ok_or("独立 B1 工作目录缺少父目录")?;
        let parent_identity = Lock::identity(parent)?;
        let parent_guard = Inputs::directory(parent)?;
        let owner = files::work_owner(reference)?;
        let owner_bytes = inputs::document::canonical_bytes(&owner)?;
        if Lock::identity(parent)? != parent_identity {
            return Err("独立 B1 工作目录创建前父目录身份变化".into());
        }
        fs::create_dir(&directory)?;
        let identity = Lock::identity(&directory)?;
        let directory_guard = Inputs::directory(&directory)?;
        parent_guard.check()?;
        if Lock::identity(&directory)? != identity
            || Lock::identity(parent)? != parent_identity
            || portable_path(&directory.canonicalize()?) != directory
        {
            return Err("独立 B1 工作目录创建时路径身份变化，保留现场".into());
        }
        let owner_path = directory.join("reference-owner.json");
        native::json::write_new(&owner_path, &owner)?;
        let owner_guard = ProtectedInputs::open([owner_path.clone()])?;
        let lock = Lock::claim(&directory)?;
        let claim = Self {
            directory,
            identity,
            directory_guard,
            owner_path,
            owner_bytes,
            owner_guard,
            lock,
            reentry: None,
        };
        claim.check(declared, reference)?;
        Ok(claim)
    }

    /// 重新取得原工作目录的恢复写入锁，沿用开始登记已保存的文件身份。
    pub(super) fn reenter(backend: &Path, reference: &Value, expected: &Value) -> Result<Self> {
        if reads::is_active() {
            return Err("重入独立 B1 工作目录前必须结束静态读取窗口".into());
        }
        let OriginalWork {
            directory,
            identity,
            directory_guard,
            owner,
            owner_guard,
            reentry,
        } = OriginalWork::read(backend, reference, expected)?;
        let lock = Lock::claim(&directory)?;
        let claimed = Self {
            directory,
            identity,
            directory_guard,
            owner_path: owner.path,
            owner_bytes: owner.raw,
            owner_guard,
            lock,
            reentry: Some(reentry),
        };
        if claimed.binding(backend, reference)? != *expected {
            return Err("独立 B1 恢复取得新锁后原工作目录归属变化".into());
        }
        Ok(claimed)
    }

    pub(super) fn check(&self, selected: &Path, reference: &Value) -> Result<()> {
        self.check_reentry()?;
        self.directory_guard.check()?;
        if selected != self.directory
            || path(&reference["work_dir"])? != self.directory
            || Lock::identity(selected)? != self.identity
        {
            return Err("独立 B1 工作目录与本控制器创建的目录不一致".into());
        }
        self.lock.check()?;
        self.owner_guard.assert_unchanged()?;
        reads::file(&self.owner_path)?;
        let expected = inputs::document::canonical_bytes(&files::work_owner(reference)?)?;
        if expected != self.owner_bytes || fs::read(&self.owner_path)? != self.owner_bytes {
            return Err("独立 B1 工作目录的原归属字节或计划摘要变化".into());
        }
        self.lock.check()?;
        self.owner_guard.assert_unchanged()?;
        self.directory_guard.check()?;
        self.check_reentry()
    }

    /// 保存本次占有的目录和归属文件身份，供下一阶段持新锁重验。
    pub(super) fn binding(&self, backend: &Path, reference: &Value) -> Result<Value> {
        self.check(&self.directory, reference)?;
        let owner_identity = self.owner_identity()?;
        let owner = inputs::document::read(&self.owner_path, backend, "原参考目录归属")?;
        if owner.path != self.owner_path || owner.raw != self.owner_bytes {
            return Err("独立 B1 工作目录的原归属路径或字节变化".into());
        }
        self.check(&self.directory, reference)?;
        if self.owner_identity()? != owner_identity {
            return Err("独立 B1 工作目录的原归属文件身份变化".into());
        }
        Ok(json!({
            "work_dir":self.directory,
            "directory_identity":{"device":self.identity.0,"inode":self.identity.1},
            "owner":inputs::document::binding(&owner),
            "owner_identity":{"device":owner_identity.0,"inode":owner_identity.1},
        }))
    }

    fn owner_identity(&self) -> Result<(u64, u64)> {
        common::device_inode_for(&self.owner_path, &owner_metadata(&self.owner_path)?)
    }

    fn check_reentry(&self) -> Result<()> {
        if let Some(reentry) = &self.reentry {
            reentry.parent.check()?;
            check_reentry_owner(&self.owner_path, reentry.owner_identity)?;
        }
        Ok(())
    }
}

/// 读取开始登记保存的原工作目录归属，可供写阶段重入或已完成阶段只读消费。
pub(super) struct OriginalWork {
    directory: PathBuf,
    identity: (u64, u64),
    directory_guard: Inputs,
    owner: inputs::document::Document,
    owner_guard: ProtectedInputs,
    reentry: Reentry,
}

impl OriginalWork {
    fn read(backend: &Path, reference: &Value, expected: &Value) -> Result<Self> {
        if reference["target_side"] != "candidate" {
            return Err("独立 B1 工作目录只允许 candidate 侧恢复".into());
        }
        inputs::domain::identifier(inputs::document::text(
            &reference["id"],
            "独立 B1 参考计划 ID",
        )?)?;
        let expected_fields = inputs::document::exact(
            expected,
            &["work_dir", "directory_identity", "owner", "owner_identity"],
            "开始登记原工作目录归属",
        )?;
        let declared = path(&reference["work_dir"])?;
        validate_local_test_path(declared, backend, LocalTestPathKind::ExistingDirectory)?;
        let directory = portable_path(&declared.canonicalize()?);
        let canonical = json!(directory);
        if reference["work_dir"] != canonical || expected_fields["work_dir"] != canonical {
            return Err("独立 B1 恢复必须使用开始登记的原规范工作目录".into());
        }
        let parent = directory.parent().ok_or("独立 B1 工作目录缺少父目录")?;
        let parent_guard = Inputs::directory(parent)?;
        let directory_guard = Inputs::directory(&directory)?;
        let identity = bound_identity(&expected_fields["directory_identity"], "原工作目录身份")?;
        let owner_identity = bound_identity(&expected_fields["owner_identity"], "原归属文件身份")?;
        let owner_descriptor =
            inputs::document::binding_shape(&expected_fields["owner"], "原工作目录归属文件")?;
        let owner_path = directory.join("reference-owner.json");
        if owner_descriptor.path != owner_path || Lock::identity(&directory)? != identity {
            return Err("独立 B1 恢复的原工作目录身份或归属文件路径不同".into());
        }
        let owner_guard = ProtectedInputs::open([owner_path.clone()])?;
        check_reentry_owner(&owner_path, owner_identity)?;
        let owner = inputs::document::read(&owner_path, backend, "原参考目录归属")?;
        let work = Self {
            directory,
            identity,
            directory_guard,
            owner,
            owner_guard,
            reentry: Reentry {
                parent: parent_guard,
                owner_identity,
            },
        };
        if inputs::document::binding(&work.owner) != expected_fields["owner"] {
            return Err("独立 B1 恢复的原归属文件字节或计划摘要不同".into());
        }
        work.check(declared, reference)?;
        Ok(work)
    }

    pub(super) fn read_completed(
        backend: &Path,
        reference: &Value,
        expected: &Value,
    ) -> Result<Self> {
        lock_absent(path(&reference["work_dir"])?)?;
        let work = Self::read(backend, reference, expected)?;
        work.check_completed(&work.directory, reference)?;
        Ok(work)
    }

    pub(super) fn check_completed(&self, selected: &Path, reference: &Value) -> Result<()> {
        lock_absent(&self.directory)?;
        self.check(selected, reference)?;
        lock_absent(&self.directory)
    }

    fn check(&self, selected: &Path, reference: &Value) -> Result<()> {
        self.reentry.parent.check()?;
        self.directory_guard.check()?;
        if selected != self.directory
            || path(&reference["work_dir"])? != self.directory
            || portable_path(&selected.canonicalize()?) != self.directory
            || Lock::identity(selected)? != self.identity
            || inputs::document::canonical_bytes(&files::work_owner(reference)?)? != self.owner.raw
        {
            return Err("独立 B1 恢复的原工作目录或计划归属不同".into());
        }
        self.owner_guard.assert_unchanged()?;
        self.owner.assert_unchanged()?;
        check_reentry_owner(&self.owner.path, self.reentry.owner_identity)?;
        self.directory_guard.check()?;
        self.reentry.parent.check()
    }
}

fn lock_absent(work: &Path) -> Result<()> {
    let lock = work.join(REFERENCE_LOCK);
    reads::optional(&lock)?;
    match fs::symlink_metadata(lock) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("独立 B1 恢复仍持有工作目录写入锁".into()),
    }
}

fn bound_identity(value: &Value, label: &str) -> Result<(u64, u64)> {
    let value = inputs::document::exact(value, &["device", "inode"], label)?;
    Ok((
        inputs::document::integer(&value["device"], label)?,
        inputs::document::integer(&value["inode"], label)?,
    ))
}

fn owner_metadata(owner: &Path) -> Result<fs::Metadata> {
    super::unlinked(owner)?;
    let metadata = fs::symlink_metadata(owner)?;
    if !metadata.is_file() {
        return Err("独立 B1 工作目录的归属必须为原普通文件".into());
    }
    Ok(metadata)
}

fn check_reentry_owner(owner: &Path, expected: (u64, u64)) -> Result<()> {
    let metadata = owner_metadata(owner)?;
    if common::device_inode_for(owner, &metadata)? != expected
        || common::link_count_for(owner, &metadata)? != 1
    {
        return Err("独立 B1 恢复的原归属文件身份或硬链接数量不同".into());
    }
    Ok(())
}

pub(super) fn run(raw: &str) -> Result<Value> {
    let request = protocol::request(&json!({"raw":raw}))?;
    let command = request["command"].as_str().ok_or("参考操作缺失")?;
    if matches!(command, "check-dataset" | "check-existing") {
        return super::readonly::run(&request);
    }
    if command == "restore" {
        return super::restore::run(path(&request["backend_dir"])?, &request);
    }
    let backend = path(&request["backend_dir"])?;
    let reference = inputs::document::read(path(&request["plan"])?, backend, "参考恢复计划")?;
    if inputs::reference::is_independent_b1(&reference.value) {
        if command != "plan" || request.as_object().is_some_and(|value| value.len() == 4) {
            return Err("独立 B1 参考计划只允许完整目标计划操作".into());
        }
        return super::plan::run(backend, &request, reference);
    }
    inputs::domain::validate_plan(&reference.value, backend)?;
    if command == "plan" {
        return if request.as_object().is_some_and(|value| value.len() == 4) {
            model::summary(&json!({"plan":reference.value}))
        } else {
            super::plan::run(backend, &request, reference)
        };
    }
    let plan = &reference.value;
    if command == "dataset" {
        return super::dataset::run(backend, &request, plan);
    }
    if !matches!(command, "backup" | "copy" | "damage") {
        return Err("参考文件操作无效".into());
    }
    let work = files::work(&json!({"plan":plan}), true)?;
    let _guard = Lock::claim(path(&work)?)?;
    let receipt = receipts::start(
        &json!({"plan":plan,"command":command,"copy_id":request["copy_id"],
        "root":request["backup_root"],"receipt":null,"bindings":null}),
    )?;
    let result = (|| {
        if command == "backup" {
            return super::backup::run(backend, &reference, path(&work)?, &request);
        }
        let root = path(&request["backup_root"])?;
        super::unlinked(root)?;
        let manifest = native::json::read(&root.join("manifest.json"))?;
        if command == "copy" {
            files::copy(
                &json!({"work":work,"root":root,"manifest":manifest,"copy_id":request["copy_id"]}),
            )
        } else {
            files::damage(
                &json!({"work":work,"root":root,"manifest":manifest,"relative":request["artifact"],"missing":request["missing"]}),
            )
        }
    })();
    match result {
        Ok(value) => {
            receipts::finish(
                &json!({"receipt":receipt,"result":value,"failure":null,"resource_images":null}),
            )?;
            Ok(value)
        }
        Err(error) => {
            let (kind, message) = if command == "backup" {
                (
                    "ReferenceBackupFailure",
                    "参考备份未完成；核对原产物和外部只读诊断，禁止覆盖或自动重放",
                )
            } else {
                (
                    "ReferenceLocalFailure",
                    "参考本地阶段失败；保留现场，禁止覆盖或自动重放",
                )
            };
            let published = receipts::finish(&json!({"receipt":receipt,"result":null,
                "failure":{"error_type":kind,"message":message},"resource_images":null}));
            if published.is_err() {
                return Err(
                    "参考本地阶段与失败收据发布均未确认；保留运行收据和现场，禁止自动重放".into(),
                );
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work_fixture() -> Result<(tempfile::TempDir, PathBuf, Value)> {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local)?;
        let fixture = tempfile::tempdir_in(local)?;
        let backend = portable_path(&fixture.path().canonicalize()?);
        let local = backend.join(".local-tests");
        fs::create_dir(&local)?;
        let reference = json!({
            "id":"independent-reentry",
            "target_side":"candidate",
            "work_dir":local.join("reference"),
        });
        Ok((fixture, backend, reference))
    }

    #[test]
    fn independent_restore_reentry_claims_a_new_lock_and_refuses_other_owners() -> Result<()> {
        let (_fixture, backend, reference) = work_fixture()?;
        let work = path(&reference["work_dir"])?;
        let owner_path = work.join("reference-owner.json");
        let lock_path = work.join(".reference-lock");
        let begin = OwnedWork::claim(&backend, &reference)?;
        let expected = begin.binding(&backend, &reference)?;
        let owner_bytes = fs::read(&owner_path)?;
        assert!(OwnedWork::reenter(&backend, &reference, &expected).is_err());
        assert!(OriginalWork::read_completed(&backend, &reference, &expected).is_err());
        begin.lock.check()?;
        drop(begin);
        assert!(!lock_path.try_exists()?);

        let proof = OriginalWork::read_completed(&backend, &reference, &expected)?;
        proof.check_completed(work, &reference)?;
        assert!(!lock_path.try_exists()?);
        assert_eq!(fs::read(&owner_path)?, owner_bytes);
        drop(proof);

        fs::create_dir(&lock_path)?;
        assert!(OwnedWork::reenter(&backend, &reference, &expected).is_err());
        assert!(lock_path.is_dir());
        fs::remove_dir(&lock_path)?;

        let restored = OwnedWork::reenter(&backend, &reference, &expected)?;
        assert_eq!(restored.binding(&backend, &reference)?, expected);
        restored.lock.check()?;
        assert!(OwnedWork::reenter(&backend, &reference, &expected).is_err());
        assert_eq!(fs::read(&owner_path)?, owner_bytes);
        drop(restored);
        assert!(!lock_path.try_exists()?);
        Ok(())
    }

    #[test]
    fn independent_restore_reentry_rejects_wrong_paths_and_invalid_bindings() -> Result<()> {
        let (_fixture, backend, reference) = work_fixture()?;
        let work = path(&reference["work_dir"])?;
        let begin = OwnedWork::claim(&backend, &reference)?;
        let expected = begin.binding(&backend, &reference)?;
        drop(begin);
        for (pointer, changed) in [
            ("/work_dir", json!(work.with_file_name("different"))),
            ("/directory_identity/device", json!(-1)),
            ("/owner/path", json!(work.join("other-owner.json"))),
            ("/owner/bytes", json!(1)),
            ("/owner/sha256", json!("0".repeat(64))),
            ("/owner_identity/inode", json!("not-an-integer")),
        ] {
            let mut replaced = expected.clone();
            *replaced.pointer_mut(pointer).unwrap() = changed;
            assert!(
                OwnedWork::reenter(&backend, &reference, &replaced).is_err(),
                "{pointer}"
            );
            assert!(!work.join(".reference-lock").try_exists()?);
        }
        let mut added = expected.clone();
        added["extra"] = json!(true);
        assert!(OwnedWork::reenter(&backend, &reference, &added).is_err());
        let mut missing = reference.clone();
        let absent = work.with_file_name("absent");
        missing["work_dir"] = json!(absent);
        assert!(OwnedWork::reenter(&backend, &missing, &expected).is_err());
        assert!(!absent.try_exists()?);
        Ok(())
    }

    #[test]
    fn independent_restore_reentry_rejects_equal_bytes_at_replaced_identities() -> Result<()> {
        let (_fixture, backend, reference) = work_fixture()?;
        let work = path(&reference["work_dir"])?;
        let owner = work.join("reference-owner.json");
        let begin = OwnedWork::claim(&backend, &reference)?;
        let expected = begin.binding(&backend, &reference)?;
        let bytes = fs::read(&owner)?;
        drop(begin);

        let linked = work.with_file_name("owner-hard-link.json");
        fs::hard_link(&owner, &linked)?;
        assert!(OwnedWork::reenter(&backend, &reference, &expected).is_err());
        assert!(OriginalWork::read_completed(&backend, &reference, &expected).is_err());
        fs::remove_file(&linked)?;

        fs::rename(&owner, work.join("old-reference-owner.json"))?;
        fs::write(&owner, &bytes)?;
        assert!(OwnedWork::reenter(&backend, &reference, &expected).is_err());
        assert!(OriginalWork::read_completed(&backend, &reference, &expected).is_err());
        assert!(!work.join(".reference-lock").try_exists()?);

        fs::rename(work, work.with_file_name("previous-work"))?;
        fs::create_dir(work)?;
        fs::write(&owner, &bytes)?;
        assert!(OwnedWork::reenter(&backend, &reference, &expected).is_err());
        assert!(!work.join(".reference-lock").try_exists()?);
        Ok(())
    }
}
