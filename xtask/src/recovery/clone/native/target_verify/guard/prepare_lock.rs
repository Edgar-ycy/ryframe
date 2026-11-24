use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, path::Path};

use super::{Inputs, ProcessGuard, Role, common, digest};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const LOCK: &str = "initialize.lock";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::recovery::clone::native) struct Observation {
    target: (u64, u64),
    guard: (u64, u64),
    directory: (u64, u64),
}

pub(in crate::recovery::clone::native) fn observe(
    backend: &Path,
    target: &Path,
) -> Result<Observation> {
    let target_id = parent_identity(backend, target)?;
    let guard_id = guard_identity(target)?;
    let directory = empty_identity(&target.join(LOCK))?.ok_or("原目标空锁不存在")?;
    if parent_identity(backend, target)? != target_id || guard_identity(target)? != guard_id {
        return Err("观察期间目标目录或互斥文件身份变化".into());
    }
    Ok(Observation {
        target: target_id,
        guard: guard_id,
        directory,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::recovery::clone::native) enum Location {
    Target,
    Archived,
}

pub(in crate::recovery::clone::native) struct Recovery<'a> {
    backend: &'a Path,
    target: &'a Path,
    archive: &'a Path,
    observation: Observation,
    guard: ProcessGuard,
    roots: [Inputs; 2],
}

impl<'a> Recovery<'a> {
    pub(in crate::recovery::clone::native) fn acquire(
        backend: &'a Path,
        target: &'a Path,
        archive: &'a Path,
        observation: Observation,
    ) -> Result<Self> {
        let archive_root = archive.parent().ok_or("目标锁归档缺少父目录")?;
        if parent_identity(backend, archive_root)?.0 != observation.target.0
            || archive.starts_with(target)
        {
            return Err("目标锁归档必须位于目标树之外的同一卷".into());
        }
        validate_local_test_path(archive, backend, LocalTestPathKind::StateDirectory)?;
        let roots = [Inputs::directory(target)?, Inputs::directory(archive_root)?];
        let guard = ProcessGuard::acquire_existing_named(target, Role::Target.guard_name())?;
        let recovery = Self {
            backend,
            target,
            archive,
            observation,
            guard,
            roots,
        };
        recovery.location()?;
        Ok(recovery)
    }

    pub(in crate::recovery::clone::native) fn location(&self) -> Result<Location> {
        self.check_parents()?;
        let location = match (
            empty_identity(&self.target.join(LOCK))?,
            empty_identity(self.archive)?,
        ) {
            (Some(found), None) if found == self.observation.directory => Location::Target,
            (None, Some(found)) if found == self.observation.directory => Location::Archived,
            _ => return Err("目标锁或归档的身份与原空目录不符，保留现场".into()),
        };
        self.check_parents()?;
        Ok(location)
    }

    pub(in crate::recovery::clone::native) fn archive(&self) -> Result<()> {
        if self.location()? != Location::Target {
            return Err("原空锁已经移出，拒绝重复移动".into());
        }
        move_without_replacement(&self.target.join(LOCK), self.archive)?;
        if self.location()? != Location::Archived {
            return Err("移动后无法定位已登记的原空锁，保留现场".into());
        }
        Ok(())
    }

    fn check_parents(&self) -> Result<()> {
        for root in &self.roots {
            root.check()?;
        }
        self.guard
            .check_named(self.target, Role::Target.guard_name())?;
        if parent_identity(self.backend, self.target)? != self.observation.target
            || guard_identity(self.target)? != self.observation.guard
        {
            return Err("目标目录或原互斥文件身份已经变化".into());
        }
        Ok(())
    }
}

fn parent_identity(backend: &Path, path: &Path) -> Result<(u64, u64)> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingDirectory)?;
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}

pub(in crate::recovery::clone::native) fn guard_identity(target: &Path) -> Result<(u64, u64)> {
    let path = target.join(Role::Target.guard_name());
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file()
        || digest::linked(&metadata)
        || metadata.len() != 1
        || common::link_count_for(&path, &metadata)? != 1
    {
        return Err("目标互斥文件类型、长度或硬链接无效".into());
    }
    common::device_inode_for(&path, &metadata)
}

fn empty_identity(path: &Path) -> Result<Option<(u64, u64)>> {
    let metadata = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        value => value?,
    };
    if !metadata.is_dir() || digest::linked(&metadata) {
        return Err("目标锁必须为无链接的真实目录".into());
    }
    let identity = common::device_inode_for(path, &metadata)?;
    if fs::read_dir(path)?.next().transpose()?.is_some() {
        return Err("目标锁目录不是空目录，保留现场".into());
    }
    let after = fs::symlink_metadata(path)?;
    if !after.is_dir()
        || digest::linked(&after)
        || common::device_inode_for(path, &after)? != identity
    {
        return Err("目标锁在读取期间发生替换".into());
    }
    Ok(Some(identity))
}

#[cfg(windows)]
fn move_without_replacement(from: &Path, to: &Path) -> Result<()> {
    // 保留同一空目录作为落盘回执。原生 MoveFile 不覆盖已有目的节点。
    winsafe::MoveFile(&common::windows_path(from)?, &common::windows_path(to)?)?;
    Ok(())
}

#[cfg(not(windows))]
fn move_without_replacement(_from: &Path, _to: &Path) -> Result<()> {
    Err("当前目标空锁回收只支持 Windows".into())
}
