use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::super::{
    copy_owner::{Owner, Role},
    digest,
    lifecycle::lock::ProcessGuard,
    resources::common,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
};

#[cfg(windows)]
type Handle = winsafe::guard::CloseHandleGuard<winsafe::HFILE>;
#[cfg(not(windows))]
type Handle = fs::File;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    bytes: u64,
    links: u64,
}

struct FrozenEntry {
    path: PathBuf,
    directory: bool,
    identity: Identity,
    sha256: Option<String>,
    handle: Mutex<Handle>,
}

impl FrozenEntry {
    fn open(path: &Path, directory: bool, allow_write: bool) -> Result<Self> {
        let before = path_identity(path, directory)?;
        let handle = open_handle(path, directory, allow_write, false)?;
        if handle_identity(&handle, directory)? != before
            || path_identity(path, directory)? != before
        {
            return Err("目标输入在打开保护句柄期间被替换".into());
        }
        let sha256 = if directory || allow_write {
            None
        } else {
            Some(digest::file(path)?.sha256)
        };
        let entry = Self {
            path: path.to_path_buf(),
            directory,
            identity: before,
            sha256,
            handle: Mutex::new(handle),
        };
        entry.check()?;
        Ok(entry)
    }

    fn check(&self) -> Result<()> {
        self.check_identity()?;
        if let Some(expected) = &self.sha256
            && digest::file(&self.path)?.sha256 != *expected
        {
            return Err("目标保护输入的文件摘要发生变化".into());
        }
        Ok(())
    }

    fn check_identity(&self) -> Result<()> {
        let handle = self.handle.lock().map_err(|_| "目标保护句柄检查被中断")?;
        if handle_identity(&handle, self.directory)? != self.identity
            || path_identity(&self.path, self.directory)? != self.identity
        {
            return Err("目标输入的内核身份、链接数量或文件内容发生变化".into());
        }
        Ok(())
    }
}

pub(crate) struct Inputs {
    entries: Vec<FrozenEntry>,
}

impl Inputs {
    pub(in crate::recovery::clone::native) fn directory(path: &Path) -> Result<Self> {
        Ok(Self {
            entries: vec![FrozenEntry::open(path, true, false)?],
        })
    }
    pub(crate) fn open(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self> {
        let entries = paths
            .into_iter()
            .map(|path| FrozenEntry::open(&path, false, false))
            .collect::<Result<_>>()?;
        Ok(Self { entries })
    }

    pub(crate) fn check(&self) -> Result<()> {
        for entry in &self.entries {
            entry.check()?;
        }
        Ok(())
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        // Windows 保护句柄不共享写入和删除权限，持有期间路径无法被改写或替换；
        // 阶段边界仍执行完整身份与摘要复核。Unix 没有同等共享模式，逐项核对身份。
        #[cfg(windows)]
        return Ok(());
        #[cfg(not(windows))]
        for entry in &self.entries {
            entry.check_identity()?;
        }
        #[cfg(not(windows))]
        Ok(())
    }
}

// 与 Python fresh/copy 控制器使用同一个内核 guard 和目录锁，不自动抢占残留锁。
pub(crate) struct Generation {
    root: FrozenEntry,
    lock: Option<FrozenEntry>,
    guard_file: FrozenEntry,
    process: ProcessGuard,
    copy: Option<CopyLock>,
}

struct CopyLock {
    owner: Owner,
    record: serde_json::Value,
    receipt: Option<FrozenEntry>,
}

impl Generation {
    pub(crate) fn acquire(backend: &Path, target: &Path) -> Result<Self> {
        Self::acquire_inner(backend, target, None)
    }

    pub(in crate::recovery::clone::native) fn acquire_copy(owner: Owner) -> Result<Self> {
        let backend = owner.ledger_binding().0.to_path_buf();
        let target = owner
            .lock_path(Role::Target)
            .parent()
            .ok_or("复制锁缺少固定目标目录")?
            .to_path_buf();
        Self::acquire_inner(&backend, &target, Some(owner))
    }

    fn acquire_inner(backend: &Path, target: &Path, owner: Option<Owner>) -> Result<Self> {
        validate_local_test_path(target, backend, LocalTestPathKind::ExistingDirectory)?;
        let root = FrozenEntry::open(target, true, false)?;
        let lock_path = target.join("initialize.lock");
        absent_lock(&lock_path)?;
        let guard_name = Role::Target.guard_name();
        let process = ProcessGuard::acquire_named(target, guard_name)?;
        let guard_file = FrozenEntry::open(&target.join(guard_name), false, true)?;
        if guard_file.identity.links != 1 {
            return Err("目标控制互斥文件不能存在额外硬链接".into());
        }
        absent_lock(&lock_path)?;
        let copy = match owner {
            Some(owner) => {
                let record = owner.claim_lock(Role::Target, &process)?;
                let receipt = FrozenEntry::open(&lock_path.join("owner.json"), false, false)?;
                Some(CopyLock {
                    owner,
                    record,
                    receipt: Some(receipt),
                })
            }
            None => {
                fs::create_dir(&lock_path)?;
                None
            }
        };
        let lock = FrozenEntry::open(&lock_path, true, false)?;
        let generation = Self {
            root,
            lock: Some(lock),
            guard_file,
            process,
            copy,
        };
        generation.check()?;
        Ok(generation)
    }

    pub(in crate::recovery::clone::native) fn copy_session(
        &self,
    ) -> Result<(&Owner, &ProcessGuard, &serde_json::Value)> {
        self.check()?;
        let copy = self.copy.as_ref().ok_or("当前目标锁没有登记复制会话")?;
        Ok((&copy.owner, &self.process, &copy.record))
    }

    pub(super) fn carries_copy_owner(&self) -> bool {
        self.copy.is_some()
    }

    pub(crate) fn check(&self) -> Result<()> {
        self.root.check()?;
        self.guard_file.check()?;
        self.process.check()?;
        self.lock
            .as_ref()
            .ok_or("目标初始化目录锁已释放")?
            .check()?;
        self.check_copy_lock()
    }

    fn check_copy_lock(&self) -> Result<()> {
        if let Some(copy) = &self.copy {
            copy.receipt
                .as_ref()
                .ok_or("复制锁持有人保护已释放")?
                .check()?;
            copy.owner
                .check_live_lock(Role::Target, &self.process, &copy.record)?;
        }
        Ok(())
    }

    pub(crate) fn protects(&self, target: &Path) -> Result<()> {
        if self.root.path != target {
            return Err("初始化持有者不属于当前目标目录".into());
        }
        self.check()
    }

    pub(in crate::recovery::clone::native) fn check_initialization_lock(&self) -> Result<()> {
        self.check()?;
        let lock = self.lock.as_ref().ok_or("目标初始化目录锁已释放")?;
        if self.copy.is_some() || fs::read_dir(&lock.path)?.next().transpose()?.is_some() {
            return Err("目标初始化锁含未知成员或属于复制控制器".into());
        }
        self.check()
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        self.lock.as_ref().ok_or("目标初始化目录锁已释放")?;
        self.process.check()?;
        // Windows 目录保护句柄不共享删除权限，目标根和初始化锁不能在本代次中被替换。
        // 阶段边界仍调用 check 完整核对所有内核身份。
        #[cfg(not(windows))]
        return self.check();
        #[cfg(windows)]
        self.check_copy_lock()
    }

    pub(crate) fn finish<T>(self, result: Result<T>) -> Result<T> {
        self.finish_publishing(result, |_| Ok(()))
    }

    pub(crate) fn finish_publishing<T>(
        mut self,
        result: Result<T>,
        publish: impl FnOnce(&T) -> Result<()>,
    ) -> Result<T> {
        let released = (|| {
            self.check()?;
            let lock = self.lock.take().ok_or("目标初始化目录锁已释放")?;
            let path = lock.path.clone();
            drop(lock);
            match &mut self.copy {
                Some(copy) => {
                    drop(copy.receipt.take().ok_or("复制锁持有人保护已释放")?);
                    copy.owner
                        .release_lock(Role::Target, &self.process, &copy.record)?;
                }
                None => fs::remove_dir(path)?,
            }
            self.after_unlock()
        })();
        match (result, released) {
            (Ok(value), Ok(())) => {
                self.after_unlock()?;
                let published = publish(&value);
                let checked = self.after_unlock();
                match (published, checked) {
                    (Ok(()), Ok(())) => Ok(value),
                    (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
                    (Err(error), Err(check)) => {
                        Err(format!("{error}；目标guard收尾同时失败：{check}").into())
                    }
                }
            }
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(primary), Err(release)) => Err(format!(
                "{primary}；目标锁释放同时失败：{release}；保留现场，不自动重放或清理"
            )
            .into()),
        }
    }
    fn after_unlock(&self) -> Result<()> {
        if self.lock.is_some() {
            return Err("初始化目录锁尚未释放，不能发布外层快照".into());
        }
        self.root.check()?;
        self.guard_file.check()?;
        self.process.check()?;
        if let Some(copy) = &self.copy
            && copy.owner.verify_lock(Role::Target)?.is_some()
        {
            return Err("登记目标锁尚未释放，不能发布复制结果".into());
        }
        Ok(())
    }
}

fn absent_lock(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("目标初始化锁已存在，不能并发执行或自动抢锁".into()),
    }
}

#[cfg(windows)]
fn path_identity(path: &Path, directory: bool) -> Result<Identity> {
    let kind = if directory {
        LocalTestPathKind::ExistingDirectory
    } else {
        LocalTestPathKind::ExistingFile
    };
    validate_absolute_path(path, kind)?;
    // 私有 guard 身份只比较各自真实句柄的原生表示，不与公共资源的完整卷号混用。
    let candidate = open_handle(path, directory, true, true)?;
    handle_identity(&candidate, directory)
}

#[cfg(not(windows))]
fn path_identity(path: &Path, directory: bool) -> Result<Identity> {
    let kind = if directory {
        LocalTestPathKind::ExistingDirectory
    } else {
        LocalTestPathKind::ExistingFile
    };
    validate_absolute_path(path, kind)?;
    let metadata = fs::symlink_metadata(path)?;
    let (device, inode) = common::device_inode_for(path, &metadata)?;
    let links = if directory {
        0
    } else {
        common::link_count_for(path, &metadata)?
    };
    if !directory && links == 0 {
        return Err("目标保护输入的硬链接数量无效".into());
    }
    Ok(Identity {
        device,
        inode,
        bytes: if directory { 0 } else { metadata.len() },
        links,
    })
}

#[cfg(windows)]
fn open_handle(
    path: &Path,
    directory: bool,
    allow_write: bool,
    metadata_only: bool,
) -> Result<Handle> {
    use winsafe::{self as w, co, prelude::*};
    let flags = co::FILE_FLAG::OPEN_REPARSE_POINT
        | if directory {
            co::FILE_FLAG::BACKUP_SEMANTICS
        } else {
            co::FILE_FLAG::default()
        };
    let (handle, _) = w::HFILE::CreateFile(
        &common::windows_path(path)?,
        if metadata_only {
            co::GENERIC::default()
        } else {
            co::GENERIC::READ
        },
        Some(if directory || allow_write {
            co::FILE_SHARE::READ | co::FILE_SHARE::WRITE
        } else {
            co::FILE_SHARE::READ
        }),
        None,
        co::DISPOSITION::OPEN_EXISTING,
        co::FILE_ATTRIBUTE::NORMAL,
        Some(flags),
        None,
        None,
    )?;
    Ok(handle)
}

#[cfg(windows)]
fn handle_identity(handle: &Handle, directory: bool) -> Result<Identity> {
    use winsafe::{co, prelude::*};
    let mut info = winsafe::BY_HANDLE_FILE_INFORMATION::default();
    handle.GetFileInformationByHandle(&mut info)?;
    if info.dwFileAttributes.has(co::FILE_ATTRIBUTE::REPARSE_POINT) {
        return Err("目标保护句柄不能指向链接或重解析点".into());
    }
    if info.dwFileAttributes.has(co::FILE_ATTRIBUTE::DIRECTORY) != directory {
        return Err("目标保护句柄的节点类型不符".into());
    }
    if !directory && info.nNumberOfLinks == 0 {
        return Err("目标保护输入的硬链接数量无效".into());
    }
    Ok(Identity {
        device: u64::from(info.dwVolumeSerialNumber),
        inode: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        bytes: if directory {
            0
        } else {
            (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow)
        },
        links: if directory {
            0
        } else {
            u64::from(info.nNumberOfLinks)
        },
    })
}

#[cfg(not(windows))]
fn open_handle(
    path: &Path,
    _directory: bool,
    _allow_write: bool,
    _metadata_only: bool,
) -> Result<Handle> {
    Ok(fs::File::open(path)?)
}

#[cfg(unix)]
fn handle_identity(handle: &Handle, directory: bool) -> Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = handle.metadata()?;
    Ok(Identity {
        device: metadata.dev(),
        inode: metadata.ino(),
        bytes: if directory { 0 } else { metadata.len() },
        links: if directory { 0 } else { metadata.nlink() },
    })
}

#[cfg(not(any(windows, unix)))]
fn handle_identity(_handle: &Handle, _directory: bool) -> Result<Identity> {
    Err("当前平台不能核验目标保护输入的内核身份".into())
}
