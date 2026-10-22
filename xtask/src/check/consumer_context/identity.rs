//! 通过安全平台 API 持有并核验文件系统对象身份。

use std::{fs::File, path::Path};

use super::files::FILE_ERROR;
use crate::Result;

pub(super) struct Identity {
    #[cfg(unix)]
    handle: File,
    #[cfg(unix)]
    key: (u64, u64),
    #[cfg(windows)]
    handle: winsafe::guard::CloseHandleGuard<winsafe::HFILE>,
    #[cfg(windows)]
    key: (u32, u64),
    directory: bool,
}

impl Identity {
    pub(super) fn open(path: &Path, directory: bool) -> Result<Self> {
        super::files::raw_path(path)?;
        #[cfg(unix)]
        {
            let handle = File::open(path).map_err(|_| FILE_ERROR)?;
            Self::from_unix_handle(handle, directory)
        }
        #[cfg(windows)]
        {
            use winsafe::{co, prelude::kernel_Hfile};
            let path = path.to_str().ok_or(FILE_ERROR)?;
            let flags = co::FILE_FLAG::BACKUP_SEMANTICS | co::FILE_FLAG::OPEN_REPARSE_POINT;
            let (handle, _) = winsafe::HFILE::CreateFile(
                path,
                co::GENERIC::READ,
                Some(co::FILE_SHARE::READ | co::FILE_SHARE::WRITE),
                None,
                co::DISPOSITION::OPEN_EXISTING,
                co::FILE_ATTRIBUTE::NORMAL,
                Some(flags),
                None,
                None,
            )
            .map_err(|_| FILE_ERROR)?;
            let key = windows_key(&handle, directory)?;
            Ok(Self {
                handle,
                key,
                directory,
            })
        }
    }

    pub(super) fn for_file(path: &Path, file: &File) -> Result<Self> {
        #[cfg(unix)]
        {
            super::files::raw_path(path)?;
            Self::from_unix_handle(file.try_clone().map_err(|_| FILE_ERROR)?, false)
        }
        #[cfg(windows)]
        {
            // 调用方的标准句柄已禁止共享删除，安全包装器打开同一路径时对象不可替换。
            if !file.metadata().map_err(|_| FILE_ERROR)?.is_file() {
                return Err(FILE_ERROR.into());
            }
            Self::open(path, false)
        }
    }

    #[cfg(unix)]
    fn from_unix_handle(handle: File, directory: bool) -> Result<Self> {
        let metadata = handle.metadata().map_err(|_| FILE_ERROR)?;
        if metadata.is_dir() != directory || (!directory && !metadata.is_file()) {
            return Err(FILE_ERROR.into());
        }
        let key = unix_key(&metadata);
        Ok(Self {
            handle,
            key,
            directory,
        })
    }

    pub(super) fn verify(&self, path: &Path) -> Result<()> {
        super::files::no_links(path)?;
        #[cfg(unix)]
        let held = unix_key(&self.handle.metadata().map_err(|_| FILE_ERROR)?);
        #[cfg(windows)]
        let held = windows_key(&self.handle, self.directory)?;
        let current = Self::open(path, self.directory)?;
        if held != self.key || current.key != self.key {
            return Err(
                "consumer 文件身份漂移：保留未知对象与原持有句柄，不覆盖或清理替换对象。".into(),
            );
        }
        Ok(())
    }

    pub(super) fn same(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

#[cfg(unix)]
fn unix_key(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(windows)]
fn windows_key(handle: &winsafe::HFILE, directory: bool) -> Result<(u32, u64)> {
    use winsafe::{
        BY_HANDLE_FILE_INFORMATION, co,
        prelude::{NativeBitflag, kernel_Hfile},
    };
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    handle
        .GetFileInformationByHandle(&mut information)
        .map_err(|_| FILE_ERROR)?;
    if information
        .dwFileAttributes
        .has(co::FILE_ATTRIBUTE::REPARSE_POINT)
        || information
            .dwFileAttributes
            .has(co::FILE_ATTRIBUTE::DIRECTORY)
            != directory
    {
        return Err(FILE_ERROR.into());
    }
    Ok((
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}
