use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::super::super::{digest, export::transport::StdoutTarget, resources::common};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};

#[derive(Clone, Debug)]
pub(crate) struct CaptureError {
    kind: &'static str,
    reason: String,
}

impl CaptureError {
    fn integrity(reason: &str) -> Self {
        Self {
            kind: "StdoutIntegrityError",
            reason: reason.into(),
        }
    }

    fn io(error: std::io::Error) -> Self {
        Self {
            kind: "IoError",
            reason: format!("MySQL 原始输出句柄访问失败：{:?}", error.kind()),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        self.kind
    }
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}
impl std::error::Error for CaptureError {}

type CaptureResult<T> = std::result::Result<T, CaptureError>;

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    identity: (u64, u64),
    bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}

#[derive(Debug)]
pub(crate) struct Snapshot {
    pub(crate) raw: Vec<u8>,
    pub(crate) artifact: Value,
}

/// 原始输出只经此对象持有的句柄读取；路径只用于校验身份和登记证据。
pub(crate) struct VerifiedStdout {
    path: PathBuf,
    file: File,
    identity: (u64, u64),
    observed: Option<Value>,
    failure: Option<CaptureError>,
}

impl VerifiedStdout {
    pub(crate) fn create(path: &Path) -> Result<Self> {
        validate_absolute_path(path, LocalTestPathKind::OutputFile)
            .map_err(|_| "MySQL 输出必须使用无链接的明确绝对新路径")?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 1;
            options.share_mode(FILE_SHARE_READ);
        }
        let file = options.open(path).map_err(CaptureError::io)?;
        // Windows 的路径 ID 在原句柄禁止 WRITE/DELETE 共享的保护下读取。
        let identity = common::device_inode_for(path, &file.metadata()?)?;
        let output = Self {
            path: path.to_path_buf(),
            file,
            identity,
            observed: None,
            failure: None,
        };
        output.state()?;
        Ok(output)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn target(&self) -> Result<StdoutTarget> {
        Ok(StdoutTarget::HeldFile {
            path: self.path.clone(),
            file: self.file.try_clone()?,
        })
    }

    pub(crate) fn snapshot(&mut self) -> CaptureResult<Snapshot> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.read_snapshot();
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    fn read_snapshot(&mut self) -> CaptureResult<Snapshot> {
        self.file.flush().map_err(CaptureError::io)?;
        self.file.sync_all().map_err(CaptureError::io)?;
        let before = self.state()?;
        let raw = self.read_pass()?;
        let repeated = self.read_pass()?;
        let after = self.state()?;
        let sha256 = hex::encode(Sha256::digest(&raw));
        if before != after
            || raw.len() as u64 != before.bytes
            || repeated.len() != raw.len()
            || sha256 != hex::encode(Sha256::digest(&repeated))
        {
            return Err(CaptureError::integrity("MySQL 输出文件在读取期间变化"));
        }
        let artifact = json!({"path": self.path, "bytes": raw.len(), "sha256": sha256});
        if self
            .observed
            .as_ref()
            .is_some_and(|observed| observed != &artifact)
        {
            return Err(CaptureError::integrity(
                "MySQL 输出与已登记的外层命令快照不同",
            ));
        }
        self.observed = Some(artifact.clone());
        Ok(Snapshot { raw, artifact })
    }

    fn read_pass(&mut self) -> CaptureResult<Vec<u8>> {
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(CaptureError::io)?;
        let mut bytes = Vec::new();
        self.file
            .read_to_end(&mut bytes)
            .map_err(CaptureError::io)?;
        Ok(bytes)
    }

    fn state(&self) -> CaptureResult<FileState> {
        validate_absolute_path(&self.path, LocalTestPathKind::ExistingFile)
            .map_err(|_| CaptureError::integrity("MySQL 输出路径缺失、经过链接或越界"))?;
        let held = self.file.metadata().map_err(CaptureError::io)?;
        let path = fs::symlink_metadata(&self.path).map_err(CaptureError::io)?;
        if !held.is_file() || !path.is_file() || digest::linked(&held) || digest::linked(&path) {
            return Err(CaptureError::integrity("MySQL 输出必须保持为普通文件"));
        }
        let held = file_state(&self.path, &held)?;
        let path = file_state(&self.path, &path)?;
        if held != path || held.identity != self.identity {
            return Err(CaptureError::integrity(
                "MySQL 输出路径已不属于原始打开文件",
            ));
        }
        Ok(held)
    }
}

fn file_state(path: &Path, metadata: &Metadata) -> CaptureResult<FileState> {
    let identity = common::device_inode_for(path, metadata)
        .map_err(|_| CaptureError::integrity("MySQL 输出文件 ID 无法核验"))?;
    let links = common::link_count_for(path, metadata)
        .map_err(|_| CaptureError::integrity("MySQL 输出硬链接数量无法核验"))?;
    if links != 1 {
        return Err(CaptureError::integrity("MySQL 输出存在额外硬链接"));
    }
    Ok(FileState {
        identity,
        bytes: metadata.len(),
        modified: metadata.modified().map_err(CaptureError::io)?,
        #[cfg(unix)]
        changed: {
            use std::os::unix::fs::MetadataExt;
            (metadata.ctime(), metadata.ctime_nsec())
        },
    })
}
