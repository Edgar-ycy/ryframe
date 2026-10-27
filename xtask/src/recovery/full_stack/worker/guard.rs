//! 内核文件互斥；进程退出释放锁，固定文件本身不是存活证据。
use crate::{
    Result,
    recovery::clone::native::{digest, resources::common},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub(super) struct Guard {
    path: PathBuf,
    file: File,
    handle: same_file::Handle,
    directory: (u64, u64),
}
impl Guard {
    pub fn acquire(role: super::role::Role, directory: &Path) -> Result<Self> {
        super::operation::canonical(directory)?;
        let identity = common::device_inode_for(directory, &fs::symlink_metadata(directory)?)?;
        let path = directory.join(format!("{}-control.guard", role.name()));
        let mut file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(&[0])?;
                file.sync_all()?;
                file
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                validate(&path)?;
                OpenOptions::new().read(true).write(true).open(&path)?
            }
            Err(error) => return Err(error.into()),
        };
        file.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => "控制或锁恢复正在执行，拒绝并发操作".to_owned(),
            fs::TryLockError::Error(_) => "Worker内核互斥无法加锁".to_owned(),
        })?;
        let handle = same_file::Handle::from_file(file.try_clone()?)?;
        file.seek(SeekFrom::Start(0))?;
        let guard = Self {
            path,
            file,
            handle,
            directory: identity,
        };
        guard.check()?;
        Ok(guard)
    }
    pub fn check(&self) -> Result<()> {
        let directory = self.path.parent().ok_or("Worker互斥缺少父目录")?;
        super::operation::canonical(directory)?;
        if common::device_inode_for(directory, &fs::symlink_metadata(directory)?)? != self.directory
        {
            return Err("Worker互斥父目录身份发生变化".into());
        }
        validate(&self.path)?;
        if same_file::Handle::from_path(&self.path)? != self.handle {
            return Err("Worker互斥文件被替换".into());
        }
        let mut file = &self.file;
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = [1u8; 2];
        let count = file.read(&mut bytes)?;
        if count != 1 || bytes[0] != 0 || self.file.metadata()?.len() != 1 {
            return Err("Worker互斥文件内容无效".into());
        }
        Ok(())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
fn validate(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || digest::linked(&metadata)
        || metadata.len() != 1
        || common::link_count_for(path, &metadata)? != 1
    {
        return Err("Worker互斥文件不是独占普通文件".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn directory() -> tempfile::TempDir {
        let root = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }
    #[test]
    fn kernel_guard_excludes_second_controller_and_survives_normal_release() {
        let directory = directory();
        let first = Guard::acquire(super::super::role::Role::Worker, directory.path()).unwrap();
        assert!(Guard::acquire(super::super::role::Role::Worker, directory.path()).is_err());
        first.check().unwrap();
        drop(first);
        Guard::acquire(super::super::role::Role::Worker, directory.path())
            .unwrap()
            .check()
            .unwrap();
    }
    #[test]
    fn replaced_guard_is_detected_even_when_marker_and_timestamp_shape_match() {
        let directory = directory();
        let first = Guard::acquire(super::super::role::Role::Worker, directory.path()).unwrap();
        let path = directory.path().join("worker-control.guard");
        fs::rename(&path, directory.path().join("old.guard")).unwrap();
        fs::write(&path, [0]).unwrap();
        assert!(first.check().is_err());
    }
    #[test]
    fn hardlinked_or_malformed_guards_are_rejected_before_reading_runtime() {
        for bytes in [vec![], vec![0, 0], vec![1]] {
            let directory = directory();
            fs::write(directory.path().join("worker-control.guard"), bytes).unwrap();
            assert!(Guard::acquire(super::super::role::Role::Worker, directory.path()).is_err());
        }
        let directory = directory();
        let source = directory.path().join("source");
        fs::write(&source, [0]).unwrap();
        fs::hard_link(source, directory.path().join("worker-control.guard")).unwrap();
        assert!(Guard::acquire(super::super::role::Role::Worker, directory.path()).is_err());
    }
}
