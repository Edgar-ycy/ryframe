//! 恢复运行登记的内核互斥与可由现行 recover 读取的 owner。
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{REGISTRATION_GUARD, REGISTRATION_LOCK};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{
        clone::native::{
            json as strict,
            lifecycle::{identity, lock::ProcessGuard},
            resources::common,
        },
        fixture_control::portable_path,
        inputs::publish::publish_verified,
    },
};

#[path = "ownership/recover.rs"]
mod recover;

const OWNER_KIND: &str = "restore-runtime-registration-controller";

pub(in crate::recovery::runtime) struct Owner {
    guard: ProcessGuard,
    directory: PathBuf,
    directory_identity: (u64, u64),
    file_identity: (u64, u64),
    receipt: Value,
}

pub(in crate::recovery::runtime) fn control_root(root: &Path) -> Result<PathBuf> {
    Ok(portable_path(&root.join(".local-tests").canonicalize()?))
}

impl Owner {
    pub(in crate::recovery::runtime) fn with<T>(
        control: &Path,
        operation: &str,
        body: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        let guard = ProcessGuard::acquire_named(control, REGISTRATION_GUARD)?;
        Self::claim(control, operation, guard)?.run(body)
    }

    fn run<T>(&self, body: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let result = body(self);
        match (result, self.release()) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => {
                Err(format!("{error}；运行控制失败后 ownership 锁清理也失败：{cleanup}").into())
            }
        }
    }

    fn claim(control: &Path, operation: &str, guard: ProcessGuard) -> Result<Self> {
        guard.check_named(control, REGISTRATION_GUARD)?;
        let receipt = new_receipt(
            control,
            operation,
            &uuid::Uuid::new_v4().simple().to_string(),
        )?;
        let directory = control.join(REGISTRATION_LOCK);
        guard.check()?;
        fs::create_dir(&directory)
            .map_err(|error| format!("恢复运行控制正在执行或异常退出，保留原 owner：{error}"))?;
        let directory_identity = directory_id(&directory)?;
        let path = directory.join("owner.json");
        publish_verified(&path, &receipt, || {
            guard.check()?;
            Ok(directory_id(&directory)? == directory_identity)
        })?;
        let owner = Self {
            guard,
            directory,
            directory_identity,
            file_identity: file_id(&path)?,
            receipt,
        };
        owner.check()?;
        Ok(owner)
    }

    pub(in crate::recovery::runtime) fn control_directory(&self) -> Result<&Path> {
        self.directory
            .parent()
            .ok_or("恢复运行控制锁缺少父目录".into())
    }

    pub(in crate::recovery::runtime) fn check(&self) -> Result<()> {
        self.guard.check()?;
        if directory_id(&self.directory)? != self.directory_identity
            || identity::current()? != self.receipt["identity"]
        {
            return Err("恢复运行控制锁不再属于原创建身份".into());
        }
        only_owner_entry(&self.directory)?;
        let path = self.directory.join("owner.json");
        if file_id(&path)? != self.file_identity {
            return Err("恢复运行 owner 文件被替换，保留现场".into());
        }
        if parse_owner(&owner_bytes(&path)?)? != self.receipt
            || file_id(&path)? != self.file_identity
            || directory_id(&self.directory)? != self.directory_identity
        {
            return Err("恢复运行 owner 不同，保留现场".into());
        }
        self.guard.check()
    }

    fn release(&self) -> Result<()> {
        self.check()?;
        fs::remove_file(self.directory.join("owner.json"))?;
        fs::remove_dir(&self.directory)?;
        self.guard.check()
    }
}

fn new_receipt(control: &Path, operation: &str, token: &str) -> Result<Value> {
    Ok(json!({
        "format_version":1, "kind":OWNER_KIND,
        "identity":identity::current()?, "runtime_directory":control, "operation":operation,
        "token":token,
    }))
}

fn only_owner_entry(directory: &Path) -> Result<()> {
    let mut names = fs::read_dir(directory)?;
    let only_owner = names
        .next()
        .transpose()?
        .is_some_and(|entry| entry.file_name() == "owner.json");
    if !only_owner || names.next().is_some() {
        return Err("恢复运行控制锁内容变化，保留现场".into());
    }
    Ok(())
}

fn owner_bytes(path: &Path) -> Result<Vec<u8>> {
    // owner 由本临界区复核，不能登记到会持续持有文件的静态读取窗口。
    let stream = File::open(path)?;
    let handle = same_file::Handle::from_file(stream.try_clone()?)?;
    let mut bytes = Vec::new();
    stream.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 || same_file::Handle::from_path(path)? != handle {
        return Err("恢复运行 owner 超过 64 KiB 或在读取期间被替换".into());
    }
    Ok(bytes)
}

fn parse_owner(bytes: &[u8]) -> Result<Value> {
    strict::parse_fragment(std::str::from_utf8(bytes)?, "恢复运行 owner")
        .map_err(|_| "恢复运行 owner 必须是严格 JSON，保留原收据".into())
}

fn directory_id(path: &Path) -> Result<(u64, u64)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}

fn file_id(path: &Path) -> Result<(u64, u64)> {
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}
