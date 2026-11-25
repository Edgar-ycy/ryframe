//! 只在一次持有原件的 prepare 历史读取内记录实际消费的证据。
use std::{
    fs::{self, DirEntry, ReadDir},
    io,
    path::Path,
    time::SystemTime,
};

use crate::Result;

#[cfg(windows)]
#[path = "history/windows.rs"]
mod windows;
#[cfg(windows)]
pub(in crate::recovery) use windows::{Fence, file};

pub(in crate::recovery) fn is_active() -> bool {
    #[cfg(windows)]
    return windows::is_active();
    #[cfg(not(windows))]
    false
}

#[cfg_attr(all(test, windows), track_caller)]
pub(super) fn require_explicit_facts(kind: &str) -> Result<()> {
    if is_active() {
        let reason = format!("prepare 存储历史缺少完整的 {kind} 保护");
        #[cfg(all(test, windows))]
        let reason = format!("{reason}（调用点 {}）", std::panic::Location::caller());
        return Err(reason.into());
    }
    Ok(())
}

pub(in crate::recovery) enum Entries {
    #[cfg(windows)]
    Native(Box<ReadDir>),
    #[cfg(not(windows))]
    Native(ReadDir),
    #[cfg(windows)]
    Held(std::vec::IntoIter<DirEntry>),
}

impl Entries {
    pub(super) fn native(entries: ReadDir) -> Self {
        #[cfg(windows)]
        let entries = Box::new(entries);
        Self::Native(entries)
    }
}

impl Iterator for Entries {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Native(entries) => entries.next(),
            #[cfg(windows)]
            Self::Held(entries) => entries.next().map(Ok),
        }
    }
}

pub(in crate::recovery) fn entries(path: &Path) -> Result<Entries> {
    #[cfg(windows)]
    if windows::is_active() {
        let entries = windows::entries(path)?.ok_or("prepare 存储历史丢失同次目录枚举证据")?;
        return Ok(Entries::Held(entries.into_iter()));
    }
    Ok(Entries::native(fs::read_dir(path)?))
}

pub(super) fn optional_file(path: &Path) -> Result<bool> {
    #[cfg(windows)]
    if windows::is_active() {
        windows::optional_file(path)?.ok_or("prepare 存储历史丢失同次可选文件证据")?;
        return Ok(true);
    }
    let _ = path;
    Ok(false)
}

pub(in crate::recovery) fn try_exists(path: &Path) -> Result<bool> {
    #[cfg(windows)]
    if windows::is_active() {
        return windows::exists(path)?
            .ok_or_else(|| "prepare 存储历史丢失同次路径存在性证据".into());
    }
    Ok(path.try_exists()?)
}

pub(in crate::recovery) fn exists(path: &Path) -> Result<bool> {
    #[cfg(windows)]
    if windows::is_active() {
        return try_exists(path);
    }
    Ok(path.exists())
}

pub(in crate::recovery) fn modified(path: &Path) -> Result<SystemTime> {
    #[cfg(windows)]
    if windows::is_active() {
        return windows::modified(path)?
            .ok_or_else(|| "prepare 存储历史丢失同次修改时间证据".into());
    }
    Ok(fs::metadata(path)?.modified()?)
}

pub(in crate::recovery) fn directory(path: &Path) -> Result<bool> {
    #[cfg(windows)]
    return windows::directory(path);
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(false)
    }
}
