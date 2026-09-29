use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::Result;

pub(super) const CRUD_RESOURCE_ARTIFACT: &str = "src/api/generated/crudResources.ts";
pub(super) const CANDIDATE_MARKER: &str = "openapi/candidate.json";

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub(crate) path: PathBuf,
    pub(crate) content: Option<Vec<u8>>,
}

pub(super) struct ContractLock {
    path: PathBuf,
    identity: Vec<u8>,
}

impl ContractLock {
    pub(super) fn acquire(frontend_dir: &Path) -> Result<Self> {
        let path = frontend_dir.join("openapi/.xtask-contract.lock");
        let parent = path.parent().ok_or("契约锁文件缺少父目录")?;
        fs::create_dir_all(parent)?;
        let identity = format!("pid={}\nnonce={}\n", process::id(), nonce()?).into_bytes();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "无法获取契约同步锁 {}：{error}。若没有其他 cargo xtask generate api --write 正在运行，请删除该残留锁文件",
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

impl Drop for ContractLock {
    fn drop(&mut self) {
        if fs::read(&self.path).is_ok_and(|content| content.as_slice() == self.identity.as_slice())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) struct StagingFrontend {
    pub(super) path: PathBuf,
}

pub(crate) trait ContractFileOperations {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
}

pub(super) struct RealContractFileOperations;

impl ContractFileOperations for RealContractFileOperations {
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

impl Drop for StagingFrontend {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(super) fn nonce() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间早于 Unix epoch：{error}"))?
        .as_nanos())
}

pub(crate) fn sha256_hex(content: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(content);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}
