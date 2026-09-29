use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{Result, cli::MigrationScope};

use super::plan::migration_directory;

#[derive(Debug)]
pub(crate) struct PlannedWrite {
    pub(crate) path: PathBuf,
    pub(crate) expected: Option<Vec<u8>>,
    pub(crate) content: Vec<u8>,
}

pub(crate) trait FileOperations {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
}

pub(super) struct RealFileOperations;

impl FileOperations for RealFileOperations {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::rename(source, target)
    }

    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::hard_link(source, target)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

pub(super) struct MigrationLock {
    path: PathBuf,
    identity: Vec<u8>,
}

impl MigrationLock {
    pub(super) fn acquire(root: &Path, scope: MigrationScope) -> Result<Self> {
        let directory = migration_directory(root, scope);
        if !directory.is_dir() {
            return Err(format!("迁移目录不存在：{}", directory.display()).into());
        }
        let path = directory.join(".xtask-migration.lock");
        let identity =
            format!("pid={}\nnonce={}\n", process::id(), transaction_nonce()?).into_bytes();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "无法获取迁移写入锁 {}：{error}。若确认没有其他 cargo xtask data migrate new 正在运行，请检查迁移事务残留后再删除该锁",
                    path.display()
                )
            })?;
        if let Err(error) = file.write_all(&identity).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error.into());
        }
        Ok(Self { path, identity })
    }
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        if fs::read(&self.path).is_ok_and(|content| content.as_slice() == self.identity.as_slice())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) fn transaction_nonce() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间早于 Unix epoch：{error}"))?
        .as_nanos())
}
