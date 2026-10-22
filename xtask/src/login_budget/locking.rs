use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use same_file::Handle;

use super::{
    BudgetError,
    paths::{self, Parents},
};

const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_POLL: Duration = Duration::from_millis(50);

pub(super) struct OwnedLock<'a> {
    path: PathBuf,
    held: Handle,
    token: Vec<u8>,
    parents: &'a Parents,
    released: bool,
    release_attempted: bool,
}

impl<'a> OwnedLock<'a> {
    pub(super) fn acquire(path: &Path, parents: &'a Parents) -> Result<Self, BudgetError> {
        Self::acquire_for(path, parents, LOCK_TIMEOUT)
    }

    fn acquire_for(
        path: &Path,
        parents: &'a Parents,
        timeout: Duration,
    ) -> Result<Self, BudgetError> {
        let path = paths::appended(path, ".lock");
        let started = Instant::now();
        loop {
            parents.verify()?;
            match paths::create_new(&path) {
                Ok(file) => return Self::initialize(path, file, parents),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    match fs::symlink_metadata(&path) {
                        Ok(metadata) => paths::ordinary(&metadata, u64::MAX)?,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(_) => return Err(BudgetError::Io),
                    }
                    let remaining = timeout.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        return Err(BudgetError::LockBusy);
                    }
                    thread::sleep(LOCK_POLL.min(remaining));
                }
                Err(_) => {
                    // Windows 对已存在目录可能返回 PermissionDenied；先保留精确的对象类型诊断。
                    if let Ok(metadata) = fs::symlink_metadata(&path) {
                        paths::ordinary(&metadata, u64::MAX)?;
                    }
                    return Err(BudgetError::Io);
                }
            }
        }
    }

    fn initialize(
        path: PathBuf,
        mut file: File,
        parents: &'a Parents,
    ) -> Result<Self, BudgetError> {
        let token = uuid::Uuid::new_v4().to_string().into_bytes();
        let held = Handle::from_file(file.try_clone().map_err(|_| BudgetError::Io)?)
            .map_err(|_| BudgetError::Io)?;
        // 初始化失败时不凭路径删除；未知锁保留供主控核验。
        file.write_all(&token).map_err(|_| BudgetError::LockOwner)?;
        file.sync_all().map_err(|_| BudgetError::LockOwner)?;
        let owned = Self {
            path,
            held,
            token,
            parents,
            released: false,
            release_attempted: false,
        };
        parents.verify()?;
        owned.verify()?;
        Ok(owned)
    }

    pub(super) fn verify(&self) -> Result<(), BudgetError> {
        self.parents.verify().map_err(|_| BudgetError::LockOwner)?;
        let metadata = fs::symlink_metadata(&self.path).map_err(|_| BudgetError::LockOwner)?;
        paths::ordinary(&metadata, self.token.len() as u64).map_err(|_| BudgetError::LockOwner)?;
        let current = File::open(&self.path).map_err(|_| BudgetError::LockOwner)?;
        paths::ordinary(
            &current.metadata().map_err(|_| BudgetError::LockOwner)?,
            self.token.len() as u64,
        )
        .map_err(|_| BudgetError::LockOwner)?;
        let current = Handle::from_file(current).map_err(|_| BudgetError::LockOwner)?;
        if current != self.held {
            return Err(BudgetError::LockOwner);
        }
        let mut token = Vec::new();
        current
            .as_file()
            .take(self.token.len() as u64 + 1)
            .read_to_end(&mut token)
            .map_err(|_| BudgetError::LockOwner)?;
        if token != self.token
            || self.held != Handle::from_path(&self.path).map_err(|_| BudgetError::LockOwner)?
        {
            return Err(BudgetError::LockOwner);
        }
        Ok(())
    }

    pub(super) fn release(&mut self) -> Result<(), BudgetError> {
        if !self.released {
            self.release_attempted = true;
            self.verify()?;
            fs::remove_file(&self.path).map_err(|_| BudgetError::LockOwner)?;
            self.released = true;
        }
        Ok(())
    }
}

impl Drop for OwnedLock<'_> {
    fn drop(&mut self) {
        if !self.released && !self.release_attempted {
            // 仅在身份和 token 仍匹配时清理，外部锁永不强抢。
            let _ = self.release();
        }
    }
}

#[cfg(test)]
#[path = "../../tests/internal/login_budget/locking.rs"]
mod tests;
