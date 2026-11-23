use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::{Value, json};

use super::super::{digest, json as json_io, resources::common};
use super::{identity, manifest};
use crate::Result;

pub(crate) struct ProcessGuard {
    path: PathBuf,
    file: File,
    handle: same_file::Handle,
    parent: (u64, u64),
    marker: Mutex<()>,
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

    pub(crate) fn acquire_existing(directory: &Path) -> Result<Self> {
        Self::acquire_existing_named(directory, "run-control.guard")
    }

    pub(crate) fn acquire_existing_named(directory: &Path, name: &str) -> Result<Self> {
        if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
            return Err("控制互斥文件名称无效".into());
        }
        let parent = guard_parent(directory)?;
        let path = directory.join(name);
        guard_metadata(&path, 1)?;
        let existing = same_file::Handle::from_path(&path)?;
        let file = guard_options().open(&path)?;
        let guard = Self::lock_open_file(path, file, parent, false)?;
        if guard.handle != existing {
            return Err("控制互斥文件已被替换或改变，拒绝继续".into());
        }
        Ok(guard)
    }

    pub(crate) fn require_existing_named_held(directory: &Path, name: &str) -> Result<()> {
        if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
            return Err("控制互斥文件名称无效".into());
        }
        let parent = guard_parent(directory)?;
        let path = directory.join(name);
        guard_metadata(&path, 1)?;
        let file = guard_options().open(&path)?;
        let handle = same_file::Handle::from_file(file.try_clone()?)?;
        match file.try_lock() {
            Err(fs::TryLockError::WouldBlock) => {}
            Err(fs::TryLockError::Error(error)) => return Err(error.into()),
            Ok(()) => return Err("原控制器尚未持有对应互斥文件".into()),
        }
        guard_metadata(&path, 1)?;
        if guard_parent(directory)? != parent || same_file::Handle::from_path(&path)? != handle {
            return Err("观察期间互斥文件身份变化".into());
        }
        Ok(())
    }

    pub(crate) fn acquire_named(directory: &Path, name: &str) -> Result<Self> {
        if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
            return Err("控制互斥文件名称无效".into());
        }
        let parent = guard_parent(directory)?;
        let path = directory.join(name);
        let (file, created) = match guard_options().create_new(true).open(&path) {
            Ok(file) => (file, true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                guard_metadata(&path, 1)?;
                (guard_options().open(&path)?, false)
            }
            Err(error) => return Err(error.into()),
        };
        Self::lock_open_file(path, file, parent, created)
    }

    fn lock_open_file(
        path: PathBuf,
        file: File,
        parent: (u64, u64),
        created: bool,
    ) -> Result<Self> {
        let handle = same_file::Handle::from_file(file.try_clone()?)?;
        file.try_lock().map_err(|error| -> String {
            match error {
                fs::TryLockError::WouldBlock => "控制或锁恢复正在执行，拒绝并发操作".into(),
                fs::TryLockError::Error(error) => {
                    format!("控制互斥文件加锁失败：{error}")
                }
            }
        })?;
        let mut guard = Self {
            path,
            file,
            handle,
            parent,
            marker: Mutex::new(()),
        };
        if created {
            guard.verify_identity(0)?;
            guard.file.write_all(&[0])?;
            guard.file.sync_all()?;
        }
        guard.check()?;
        Ok(guard)
    }

    pub(crate) fn check_named(&self, directory: &Path, name: &str) -> Result<()> {
        if self.path.file_name().and_then(|value| value.to_str()) != Some(name)
            || guard_parent(directory)? != self.parent
        {
            return Err("控制互斥不属于明确父目录和名称".into());
        }
        self.check()
    }

    fn verify_identity(&self, size: u64) -> Result<()> {
        let parent = self.path.parent().ok_or("控制互斥缺少父目录")?;
        let metadata = self.file.metadata()?;
        guard_metadata(&self.path, size)?;
        if !metadata.is_file()
            || metadata.len() != size
            || guard_parent(parent)? != self.parent
            || same_file::Handle::from_path(&self.path)? != self.handle
        {
            return Err("控制互斥文件已被替换或改变，拒绝继续".into());
        }
        Ok(())
    }

    pub(crate) fn check(&self) -> Result<()> {
        self.verify_identity(1)?;
        // 多个只读对象工作线程共享同一内核 guard 的文件游标。
        let _marker = self.marker.lock().map_err(|_| "控制互斥标记检查被中断")?;
        let mut file = &self.file;
        file.seek(SeekFrom::Start(0))?;
        let mut marker = [0_u8; 1];
        file.read_exact(&mut marker)?;
        if marker != [0] {
            return Err("控制互斥文件内容无效，拒绝继续".into());
        }
        self.verify_identity(1)
    }
}

fn guard_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(OPEN_REPARSE_POINT);
    }
    options
}

fn guard_parent(directory: &Path) -> Result<(u64, u64)> {
    crate::local_test_path::validate_absolute_path(
        directory,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    common::device_inode_for(directory, &fs::symlink_metadata(directory)?)
}

fn guard_metadata(path: &Path, size: u64) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || digest::linked(&metadata)
        || metadata.len() != size
        || common::link_count_for(path, &metadata)? != 1
    {
        return Err("控制互斥文件不是独占普通文件或大小已改变".into());
    }
    Ok(())
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
    if !has_only_owner(&lock)? {
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

pub(crate) type LockSnapshot = Option<((u64, u64), Value, Value)>;

pub(crate) fn lock_snapshot(run: &Path) -> Result<LockSnapshot> {
    let observed = owner_observation(run)?;
    observed
        .map(|(descriptor, owner)| {
            let path = run.join("run.lock");
            Ok((
                common::device_inode_for(&path, &fs::symlink_metadata(&path)?)?,
                descriptor,
                owner,
            ))
        })
        .transpose()
}

pub(crate) fn check_recovery_snapshot(directory: &Path, expected: &LockSnapshot) -> Result<()> {
    if lock_snapshot(directory)? != *expected {
        return Err("恢复期间控制锁身份变化，保留现场".into());
    }
    Ok(())
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
    if exact_contents && !has_only_owner(&path)? {
        return Err("验收控制锁内容变化，保留现场".into());
    }
    if json_io::read(&path.join("owner.json"))? != *expected_owner {
        return Err("验收控制锁内容变化，保留现场".into());
    }
    Ok(())
}

pub(crate) fn recover_dead(
    directory: &Path,
    expected_owner: &Value,
    original: Option<&LockSnapshot>,
) -> Result<()> {
    let path = directory.join("run.lock");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if original.is_some_and(|snapshot| snapshot.is_some()) {
                return Err("恢复期间控制锁身份变化，保留现场".into());
            }
            return Ok(());
        }
        Err(error) => return Err(format!("无法核验待恢复控制锁：{error}").into()),
    };
    if digest::linked(&metadata) || !metadata.is_dir() {
        return Err("待恢复控制锁类型无效，保留现场".into());
    }
    let directory_id = common::device_inode_for(&path, &metadata)?;
    if !has_only_owner(&path)? {
        return Err("验收控制锁内容变化，保留现场".into());
    }
    let pid = expected_owner["identity"]["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("待恢复控制器 PID 无效")?;
    if super::identity::process(pid)?.is_some() {
        return Err("控制进程仍存在或 PID 已复用，拒绝回收".into());
    }
    let owner_path = path.join("owner.json");
    let file = open_recovery_owner(&owner_path)?;
    let handle = same_file::Handle::from_file(file.try_clone()?)?;
    let mut reader = bounded_owner_reader(file);
    let mut raw = Vec::new();
    reader.read_to_end(&mut raw)?;
    let (_, file_digest) = reader.finish();
    if file_digest.bytes > json_io::MAX_JSON_BYTES {
        return Err("控制锁持有人收据超过读取上限".into());
    }
    let source = std::str::from_utf8(&raw).map_err(|_| "控制锁持有人收据必须是 UTF-8")?;
    if json_io::parse_fragment(source, "控制锁持有人收据")? != *expected_owner {
        return Err("验收控制锁内容变化，保留现场".into());
    }
    if let Some(snapshot) = original {
        let binding = manifest::binding(&owner_path, &file_digest)?;
        if snapshot.as_ref().is_none_or(|(id, descriptor, owner)| {
            *id != directory_id || descriptor != &binding || owner != expected_owner
        }) {
            return Err("恢复期间控制锁身份变化，保留现场".into());
        }
    }
    let file = open_recovery_owner(&owner_path)?;
    if same_file::Handle::from_file(file.try_clone()?)? != handle {
        return Err("恢复期间控制锁身份变化，保留现场".into());
    }
    let mut reader = bounded_owner_reader(file);
    io::copy(&mut reader, &mut io::sink())?;
    if reader.finish().1 != file_digest
        || !same_directory(&path, directory_id)?
        || !has_only_owner(&path)?
        || same_file::Handle::from_file(open_recovery_owner(&owner_path)?)? != handle
    {
        return Err("恢复期间控制锁身份变化，保留现场".into());
    }
    drop(handle);
    fs::remove_file(&owner_path)
        .map_err(|error| format!("控制锁 owner 清理失败，保留现场：{error}"))?;
    if let Err(error) = fs::remove_dir(&path) {
        let failure = format!("控制锁目录清理失败，保留现场：{error}");
        return match restore_owner_bytes(&path, &owner_path, directory_id, &raw) {
            Ok(()) => Err(failure.into()),
            Err(restore) => Err(format!("{failure}；控制锁原持有人收据恢复失败：{restore}").into()),
        };
    }
    Ok(())
}

fn has_only_owner(path: &Path) -> Result<bool> {
    let names = path
        .read_dir()?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(names.as_slice() == [OsString::from("owner.json")])
}

fn open_recovery_owner(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || digest::linked(&metadata)
        || common::link_count_for(path, &metadata)? != 1
    {
        return Err("控制锁持有人收据必须是无链接的独立普通文件".into());
    }
    digest::open(path)
}

fn bounded_owner_reader(file: File) -> digest::HashingIo<io::Take<File>> {
    digest::HashingIo::new(file.take(json_io::MAX_JSON_BYTES + 1))
}

fn same_directory(path: &Path, expected: (u64, u64)) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.is_dir()
        && !digest::linked(&metadata)
        && common::device_inode_for(path, &metadata)? == expected)
}

pub(in crate::recovery::clone::native) fn restore_owner_bytes(
    path: &Path,
    owner_path: &Path,
    original_id: (u64, u64),
    raw: &[u8],
) -> Result<()> {
    if !same_directory(path, original_id)? || fs::read_dir(path)?.next().transpose()?.is_some() {
        return Err("锁目录类型、成员或物理身份已变化，保留现场".into());
    }
    let mut temp = tempfile::NamedTempFile::new_in(path)?;
    temp.write_all(raw)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(owner_path)
        .map_err(|error| error.error)?;
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
