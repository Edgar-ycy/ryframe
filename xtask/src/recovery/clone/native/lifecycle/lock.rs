use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{digest, json as json_io};
use super::{identity, manifest};
use crate::Result;

pub(crate) struct ProcessGuard {
    path: PathBuf,
    file: File,
    identity: MetadataIdentity,
}

pub(crate) struct RunLock {
    pub(crate) owner: Value,
    directory: PathBuf,
    identity: MetadataIdentity,
}

impl ProcessGuard {
    pub(crate) fn acquire(directory: &Path) -> Result<Self> {
        Self::acquire_named(directory, "run-control.guard")
    }

    pub(crate) fn acquire_named(directory: &Path, name: &str) -> Result<Self> {
        if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
            return Err("控制互斥文件名称无效".into());
        }
        let path = directory.join(name);
        let existing = fs::symlink_metadata(&path);
        let created = existing.is_err();
        if let Ok(metadata) = &existing
            && (!metadata.is_file() || digest::linked(metadata) || metadata.len() != 1)
        {
            return Err("控制互斥文件已被替换或改变，拒绝继续".into());
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("控制互斥文件无法安全打开，拒绝继续：{error}"))?;
        if created {
            file.write_all(&[0])?;
            file.sync_all()?;
        }
        file.try_lock().map_err(|error| -> String {
            match error {
                fs::TryLockError::WouldBlock => "控制或锁恢复正在执行，拒绝并发操作".into(),
                fs::TryLockError::Error(error) => {
                    format!("控制互斥文件加锁失败：{error}")
                }
            }
        })?;
        let identity = metadata_identity(
            &fs::symlink_metadata(&path)
                .map_err(|error| format!("控制互斥文件无法核验：{error}"))?,
        )?;
        let guard = Self {
            path,
            file,
            identity,
        };
        guard.check()?;
        Ok(guard)
    }

    pub(crate) fn check(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.path)
            .map_err(|error| format!("控制互斥文件已变化：{error}"))?;
        if !metadata.is_file()
            || digest::linked(&metadata)
            || metadata.len() != 1
            || metadata_identity(&metadata)? != self.identity
        {
            return Err("控制互斥文件已被替换或改变，拒绝继续".into());
        }
        let mut file = &self.file;
        file.seek(SeekFrom::Start(0))?;
        let mut marker = [0_u8; 1];
        file.read_exact(&mut marker)?;
        if marker != [0] {
            return Err("控制互斥文件内容无效，拒绝继续".into());
        }
        Ok(())
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl RunLock {
    pub(crate) fn claim(directory: &Path) -> Result<Self> {
        let path = directory.join("run.lock");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if digest::linked(&metadata) || !metadata.is_dir() => {
                return Err("复制运行控制锁类型无效，拒绝继续".into());
            }
            Ok(_) => return Err("复制运行目录被锁定或上次异常退出，禁止自动抢锁".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法核验复制运行控制锁：{error}").into()),
        }
        fs::create_dir(&path).map_err(|error| format!("无法创建复制运行控制锁：{error}"))?;
        let identity = metadata_identity(
            &fs::symlink_metadata(&path)
                .map_err(|error| format!("无法核验复制运行控制锁：{error}"))?,
        )?;
        let owner = json!({
            "format_version": 1,
            "identity": identity::current()?,
            "directory": text_path(directory)?,
            "manifest_sha256": digest::file(&directory.join("manifest.json"))?.sha256,
        });
        json_io::write_new(&path.join("owner.json"), &owner)?;
        let lock = Self {
            owner,
            directory: directory.to_path_buf(),
            identity,
        };
        lock.verify()?;
        Ok(lock)
    }

    pub(crate) fn verify(&self) -> Result<()> {
        verify_lock(&self.directory, &self.identity, &self.owner, true)
    }

    pub(crate) fn release(&self) -> Result<()> {
        verify_lock(&self.directory, &self.identity, &self.owner, true)?;
        let path = self.directory.join("run.lock");
        fs::remove_file(path.join("owner.json"))
            .map_err(|error| format!("控制锁清理身份变化，保留现场：{error}"))?;
        match fs::remove_dir(&path) {
            Ok(()) => Ok(()),
            Err(error) => {
                if fs::symlink_metadata(&path).is_ok_and(|metadata| {
                    !digest::linked(&metadata)
                        && metadata.is_dir()
                        && metadata_identity(&metadata).is_ok_and(|value| value == self.identity)
                }) && path
                    .read_dir()
                    .is_ok_and(|mut entries| entries.next().is_none())
                {
                    let _ = json_io::write_new(&path.join("owner.json"), &self.owner);
                }
                Err(format!("控制锁清理失败，保留现场：{error}").into())
            }
        }
    }
}

pub(crate) fn owner_observation(directory: &Path) -> Result<Option<(Value, Value)>> {
    let lock = directory.join("run.lock");
    match fs::symlink_metadata(&lock) {
        Ok(metadata) if metadata.is_dir() && !digest::linked(&metadata) => {}
        Ok(_) => return Err("控制锁目录或内容无法归属".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法核验控制锁：{error}").into()),
    }
    let names = lock
        .read_dir()?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    if names.as_slice() != [OsString::from("owner.json")] {
        return Err("控制锁目录或内容无法归属".into());
    }
    let path = lock.join("owner.json");
    let owner = json_io::read(&path)?;
    identity::validate(&owner, directory)?;
    Ok(Some((
        manifest::binding(&path, &digest::file(&path)?)?,
        owner,
    )))
}

pub(crate) fn verify_lock(
    directory: &Path,
    expected_identity: &MetadataIdentity,
    expected_owner: &Value,
    exact_contents: bool,
) -> Result<()> {
    let path = directory.join("run.lock");
    let metadata = fs::symlink_metadata(&path).map_err(|error| format!("控制锁已变化：{error}"))?;
    if digest::linked(&metadata)
        || !metadata.is_dir()
        || metadata_identity(&metadata)? != *expected_identity
    {
        return Err("验收控制锁身份变化，拒绝删除其他执行者的锁".into());
    }
    if exact_contents {
        let names = path
            .read_dir()?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        if names.as_slice() != [OsString::from("owner.json")] {
            return Err("验收控制锁内容变化，保留现场".into());
        }
    }
    if json_io::read(&path.join("owner.json"))? != *expected_owner {
        return Err("验收控制锁内容变化，保留现场".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MetadataIdentity(String);

#[cfg(unix)]
fn metadata_identity(metadata: &fs::Metadata) -> Result<MetadataIdentity> {
    use std::os::unix::fs::MetadataExt;
    Ok(MetadataIdentity(format!(
        "{}:{}",
        metadata.dev(),
        metadata.ino()
    )))
}

#[cfg(windows)]
fn metadata_identity(metadata: &fs::Metadata) -> Result<MetadataIdentity> {
    use std::os::windows::fs::MetadataExt;
    Ok(MetadataIdentity(metadata.creation_time().to_string()))
}

#[cfg(not(any(unix, windows)))]
fn metadata_identity(metadata: &fs::Metadata) -> Result<MetadataIdentity> {
    Ok(MetadataIdentity(metadata.len().to_string()))
}

fn text_path(path: &Path) -> Result<String> {
    manifest::path_text(path, "运行目录")
}
