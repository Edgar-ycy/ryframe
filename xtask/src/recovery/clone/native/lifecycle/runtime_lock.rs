//! 旧运行控制锁的目录与 owner 核验。
use super::{
    super::{digest, json as document, resources::common},
    identity,
};
use crate::{
    Result,
    cli::CloneRuntimeOperation,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::fixture_control::read_json,
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Original {
    directory: PathBuf,
    path: PathBuf,
    inode: (u64, u64),
    runtime_inode: (u64, u64),
    digest: digest::FileDigest,
    owner: Value,
}

impl Original {
    pub(super) fn load(directory: &Path) -> Result<Self> {
        let original = Self::read(directory)?;
        original.check()?;
        Ok(original)
    }

    fn read(directory: &Path) -> Result<Self> {
        validate_absolute_path(directory, LocalTestPathKind::ExistingDirectory)?;
        let path = directory.join("worker-control.lock");
        let inode = lock_inode(&path)?;
        let (digest, owner) = owner_file(&path)?;
        document::exact(
            &owner,
            &[
                "format_version",
                "kind",
                "identity",
                "runtime_directory",
                "operation",
                "token",
            ],
            "原外层控制器",
        )?;
        common::validate_process_identity(&owner["identity"])?;
        if owner["format_version"].as_u64() != Some(1)
            || owner["kind"] != "devex-clone-runtime-lock"
            || !owner["operation"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
            || !owner["token"].as_str().is_some_and(|value| {
                value.len() == 32
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err("原外层控制器owner形状或登记运行目录不符，不能回收".into());
        }
        let runtime_inode = same_runtime(&owner["runtime_directory"], directory)?;
        Ok(Self {
            directory: directory.to_owned(),
            path,
            inode,
            runtime_inode,
            digest,
            owner,
        })
    }

    fn check_files(&self) -> Result<()> {
        validate_absolute_path(&self.path, LocalTestPathKind::ExistingDirectory)?;
        let metadata = fs::symlink_metadata(&self.path)?;
        let names = fs::read_dir(&self.path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        if digest::linked(&metadata)
            || common::device_inode_for(&self.path, &metadata)? != self.inode
            || names != [OsString::from("owner.json")]
            || owner_file(&self.path)? != (self.digest.clone(), self.owner.clone())
        {
            return Err("外层锁目录身份、原 owner 或文件集合变化，保留现场".into());
        }
        if same_runtime(&self.owner["runtime_directory"], &self.directory)? != self.runtime_inode {
            return Err("外层控制器登记运行目录身份在核对期间变化".into());
        }
        Ok(())
    }

    pub(super) fn check(&self) -> Result<Option<Value>> {
        self.check_files()?;
        let pid = u32::try_from(
            self.owner["identity"]["pid"]
                .as_u64()
                .ok_or("原控制器 PID 无效")?,
        )?;
        #[cfg(windows)]
        let actual =
            crate::recovery::fixture_control::services::lifecycle::observe_recovery_process(pid)?;
        #[cfg(not(windows))]
        let actual = common::process_identity(pid)?;
        if actual.as_ref() == Some(&self.owner["identity"]) {
            return Err("控制器仍在运行，禁止释放运行控制锁".into());
        }
        if actual.as_ref().is_some_and(|value| {
            creation(&value["started"]) == creation(&self.owner["identity"]["started"])
        }) {
            return Err("原控制器同一创建代次的映像有差异，禁止释放运行控制锁".into());
        }
        Ok(actual)
    }

    pub(super) fn publish(&self, actual: &Option<Value>) -> Result<Value> {
        let value = json!({"format_version":1,"kind":"devex-clone-runtime-lock-reconciliation",
            "owner":self.owner,"observed_identity":actual,"remote_writes_reconciled":false});
        let receipt = self.directory.join(format!(
            "controller-reconcile-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        document::write_new(&receipt, &value)?;
        Ok(value)
    }

    pub(super) fn remove(self, actual: &Option<Value>) -> Result<()> {
        if &self.check()? != actual {
            return Err("外层锁owner进程观察在恢复发布期间变化，保留现场".into());
        }
        self.remove_files()
    }

    fn remove_files(self) -> Result<()> {
        fs::remove_file(self.path.join("owner.json"))?;
        fs::remove_dir(&self.path)?;
        Ok(())
    }
}

/// 启停调用方持有 runtime-control.guard，外层锁只在显式释放时清理。
pub(super) struct Active(Original);

impl Active {
    pub(super) fn claim(directory: &Path, operation: CloneRuntimeOperation) -> Result<Self> {
        if !matches!(
            operation,
            CloneRuntimeOperation::Start | CloneRuntimeOperation::Stop
        ) {
            return Err("外层运行控制锁只接受启动和停止".into());
        }
        let runtime = directory.to_str().ok_or("外层控制器运行目录必须是 UTF-8")?;
        let owner = json!({"format_version":1,"kind":"devex-clone-runtime-lock",
            "identity":identity::current()?,"runtime_directory":runtime,"operation":operation.as_str(),
            "token":uuid::Uuid::new_v4().simple().to_string()});
        let runtime_inode = same_runtime(&owner["runtime_directory"], directory)?;
        let path = directory.join("worker-control.lock");
        fs::create_dir(&path)
            .map_err(|error| format!("无法独占创建外层运行控制锁，保留现场：{error}"))?;
        let inode = lock_inode(&path)?;
        if same_runtime(&owner["runtime_directory"], directory)? != runtime_inode {
            return Err("创建外层锁期间运行目录身份变化，保留现场".into());
        }
        document::write_new(&path.join("owner.json"), &owner)?;
        let original = Original::read(directory)?;
        if original.inode != inode
            || original.runtime_inode != runtime_inode
            || original.owner != owner
        {
            return Err("创建外层锁期间目录身份或 owner 变化，保留现场".into());
        }
        let active = Self(original);
        active.check()?;
        Ok(active)
    }

    pub(super) fn check(&self) -> Result<()> {
        self.0.check_files()?;
        if self.0.owner["identity"] != identity::current()? {
            return Err("外层运行控制锁不属于当前创建身份，保留现场".into());
        }
        Ok(())
    }

    pub(super) fn release(self) -> Result<()> {
        self.check()?;
        self.0.remove_files()
    }
}

fn lock_inode(path: &Path) -> Result<(u64, u64)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}

fn creation(value: &Value) -> Option<&str> {
    value.as_str().map(|value| value.trim_start_matches('0'))
}

fn owner_file(lock: &Path) -> Result<(digest::FileDigest, Value)> {
    let path = lock.join("owner.json");
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    if fs::symlink_metadata(&path)?.len() > 64 * 1024 {
        return Err("原外层控制器 owner 超过 64 KiB".into());
    }
    let before = digest::file(&path)?;
    let owner = read_json(&path, "原外层控制器 owner", 64 * 1024)?;
    if digest::file(&path)? != before {
        return Err("原控制器 owner 在读取期间变化，保留现场".into());
    }
    Ok((before, owner))
}

/// 仅比较路径位置和内核目录身份，保留原 owner 字节和路径写法。
fn same_runtime(recorded: &Value, selected: &Path) -> Result<(u64, u64)> {
    let original = recorded
        .as_str()
        .ok_or("原外层控制器owner运行目录必须是文本绝对路径")?;
    let recorded = Path::new(original);
    validate_absolute_path(recorded, LocalTestPathKind::ExistingDirectory)?;
    validate_absolute_path(selected, LocalTestPathKind::ExistingDirectory)?;
    if selected.to_str() != Some(original) {
        return Err("原 owner 运行目录不同于登记的规范路径".into());
    }
    let recorded_inode = common::device_inode_for(recorded, &fs::symlink_metadata(recorded)?)?;
    let selected_inode = common::device_inode_for(selected, &fs::symlink_metadata(selected)?)?;
    if recorded.canonicalize()? != selected.canonicalize()? || recorded_inode != selected_inode {
        return Err("原外层控制器登记运行目录不是同一受管真实目录".into());
    }
    validate_absolute_path(recorded, LocalTestPathKind::ExistingDirectory)?;
    validate_absolute_path(selected, LocalTestPathKind::ExistingDirectory)?;
    if common::device_inode_for(recorded, &fs::symlink_metadata(recorded)?)? != selected_inode
        || common::device_inode_for(selected, &fs::symlink_metadata(selected)?)? != selected_inode
    {
        return Err("原外层控制器登记运行目录身份在核对期间变化".into());
    }
    Ok(selected_inode)
}
