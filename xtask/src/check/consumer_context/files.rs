//! 上下文证据文件的路径、内容与持有句柄核验。

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::identity::Identity;
use crate::Result;

pub(super) const FILE_ERROR: &str =
    "consumer 文件核验失败：检查明确路径的普通文件、无链接父目录及读取权限，保留当前证据后重试。";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub(super) path: String,
    pub(super) sha256: String,
}

#[derive(Serialize)]
pub(super) struct Binding {
    pub(super) path: PathBuf,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

pub(crate) struct HeldFile {
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
    file: File,
    identity: Identity,
    parent: Identity,
}

impl HeldFile {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let (mut file, identity) = open_data_file(path)?;
        let parent = Identity::open(path.parent().ok_or(FILE_ERROR)?, true)?;
        identity.verify(path)?;
        let (bytes, sha256) = fingerprint(&mut file)?;
        Ok(Self {
            path: path.to_owned(),
            bytes,
            sha256,
            file,
            identity,
            parent,
        })
    }

    pub(super) fn from_created(path: &Path, file: File) -> Result<Self> {
        let identity = Identity::for_file(path, &file)?;
        drop(file);
        let held = Self::open(path)?;
        if !identity.same(&held.identity) {
            return Err(FILE_ERROR.into());
        }
        Ok(held)
    }

    pub(crate) fn read(&mut self, limit: u64) -> Result<Vec<u8>> {
        if self.bytes > limit {
            return Err(FILE_ERROR.into());
        }
        self.file.seek(SeekFrom::Start(0)).map_err(|_| FILE_ERROR)?;
        let mut bytes = Vec::new();
        (&mut self.file)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| FILE_ERROR)?;
        if bytes.len() as u64 != self.bytes || hash(&bytes) != self.sha256 {
            return Err(FILE_ERROR.into());
        }
        Ok(bytes)
    }

    pub(crate) fn verify(&self) -> Result<()> {
        self.parent.verify(self.path.parent().ok_or(FILE_ERROR)?)?;
        self.identity.verify(&self.path)?;
        let held = Identity::for_file(&self.path, &self.file)?;
        if !held.same(&self.identity) {
            return Err(FILE_ERROR.into());
        }
        let (mut file, identity) = open_data_file(&self.path)?;
        identity.verify(&self.path)?;
        let (bytes, sha256) = fingerprint(&mut file)?;
        if !self.identity.same(&identity) || bytes != self.bytes || sha256 != self.sha256 {
            return Err(
                "consumer 文件内容漂移：保留证据并使用一致的源码、配置和绑定重新执行。".into(),
            );
        }
        Ok(())
    }

    pub(super) fn binding(&self) -> Result<Binding> {
        if self.bytes == 0 || self.bytes > 9_007_199_254_740_991 {
            return Err(FILE_ERROR.into());
        }
        Ok(Binding {
            path: self.path.clone(),
            bytes: self.bytes,
            sha256: self.sha256.clone(),
        })
    }

    pub(super) fn verify_binary(&mut self) -> Result<()> {
        #[cfg(windows)]
        {
            let mut header = [0_u8; 64];
            self.file.seek(SeekFrom::Start(0)).map_err(|_| FILE_ERROR)?;
            self.file.read_exact(&mut header).map_err(|_| FILE_ERROR)?;
            let offset =
                u32::from_le_bytes([header[60], header[61], header[62], header[63]]) as u64;
            if &header[..2] != b"MZ" || offset < 64 || offset + 4 > self.bytes {
                return Err(FILE_ERROR.into());
            }
            let mut signature = [0_u8; 4];
            self.file
                .seek(SeekFrom::Start(offset))
                .map_err(|_| FILE_ERROR)?;
            self.file
                .read_exact(&mut signature)
                .map_err(|_| FILE_ERROR)?;
            if &signature != b"PE\0\0" {
                return Err(FILE_ERROR.into());
            }
        }
        Ok(())
    }
}

pub(super) fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(super) fn digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn open_data_file(path: &Path) -> Result<(File, Identity)> {
    no_links(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = options.open(path).map_err(|_| FILE_ERROR)?;
    if !file.metadata().map_err(|_| FILE_ERROR)?.is_file() {
        return Err(FILE_ERROR.into());
    }
    let identity = Identity::for_file(path, &file)?;
    Ok((file, identity))
}

fn fingerprint(file: &mut File) -> Result<(u64, String)> {
    file.seek(SeekFrom::Start(0)).map_err(|_| FILE_ERROR)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| FILE_ERROR)?;
        if count == 0 {
            return Ok((bytes, hex::encode(hasher.finalize())));
        }
        bytes += count as u64;
        hasher.update(&buffer[..count]);
    }
}

pub(super) fn no_links(path: &Path) -> Result<()> {
    raw_path(path)?;
    if !path.is_absolute() {
        return Err(FILE_ERROR.into());
    }
    for component in path.ancestors() {
        let metadata = fs::symlink_metadata(component).map_err(|_| FILE_ERROR)?;
        if metadata.file_type().is_symlink() {
            return Err(FILE_ERROR.into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(FILE_ERROR.into());
            }
        }
    }
    Ok(())
}

pub(super) fn exists(path: &Path) -> Result<bool> {
    raw_path(path)?;
    if !path.is_absolute() {
        return Err(FILE_ERROR.into());
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {
            no_links(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            for ancestor in path.ancestors().skip(1) {
                match fs::symlink_metadata(ancestor) {
                    Ok(metadata) => {
                        if !metadata.is_dir() {
                            return Err(FILE_ERROR.into());
                        }
                        no_links(ancestor)?;
                        return Ok(false);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => return Err(FILE_ERROR.into()),
                }
            }
            Err(FILE_ERROR.into())
        }
        Err(_) => Err(FILE_ERROR.into()),
    }
}

pub(super) fn directory(path: &Path) -> Result<()> {
    if exists(path)? {
        if !path.is_dir() {
            return Err(FILE_ERROR.into());
        }
        return Ok(());
    }
    let parent = path.parent().ok_or(FILE_ERROR)?;
    directory(parent)?;
    fs::create_dir(path).map_err(|_| FILE_ERROR)?;
    no_links(path)
}

pub(super) fn absolute_directory(path: &Path) -> Result<PathBuf> {
    raw_path(path)?;
    let absolute = std::path::absolute(path).map_err(|_| FILE_ERROR)?;
    no_links(&absolute)?;
    if !absolute.is_dir() {
        return Err(FILE_ERROR.into());
    }
    let canonical = fs::canonicalize(&absolute).map_err(|_| FILE_ERROR)?;
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = canonical.components();
        if let Some(Component::Prefix(prefix)) = components.next()
            && let Prefix::VerbatimDisk(drive) = prefix.kind()
        {
            let mut ordinary = PathBuf::from(format!("{}:", char::from(drive)));
            for component in components {
                ordinary.push(component.as_os_str());
            }
            if fs::canonicalize(&ordinary).map_err(|_| FILE_ERROR)? != canonical {
                return Err(FILE_ERROR.into());
            }
            return Ok(ordinary);
        }
    }
    Ok(canonical)
}

/// 用户选择可包含父跳转，但每个原始目录前缀必须先通过核验。
pub(crate) fn selected_frontend_directory(
    backend_root: &Path,
    selection: &Path,
) -> Result<PathBuf> {
    let text = selection.to_str().ok_or(FILE_ERROR)?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(FILE_ERROR.into());
    }
    let backend_root = absolute_directory(backend_root)?;
    let mut steps = Vec::new();
    if !selection.is_absolute() {
        steps.extend(backend_root.components());
    }
    steps.extend(selection.components());
    let mut steps = steps.into_iter();
    let mut current = PathBuf::new();
    match steps.next().ok_or(FILE_ERROR)? {
        std::path::Component::Prefix(prefix) => {
            current.push(prefix.as_os_str());
            let Some(root @ std::path::Component::RootDir) = steps.next() else {
                return Err(FILE_ERROR.into());
            };
            current.push(root.as_os_str());
        }
        root @ std::path::Component::RootDir => current.push(root.as_os_str()),
        _ => return Err(FILE_ERROR.into()),
    }
    selected_directory_prefix(&current)?;
    for component in steps {
        match component {
            std::path::Component::Normal(name) if name == std::ffi::OsStr::new("..") => {
                selected_parent(&mut current)?;
            }
            std::path::Component::ParentDir => selected_parent(&mut current)?,
            std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => {
                current.push(name);
                selected_directory_prefix(&current)?;
            }
            _ => return Err(FILE_ERROR.into()),
        }
    }
    absolute_directory(&current)
}

fn selected_directory_prefix(path: &Path) -> Result<()> {
    no_links(path)?;
    for ancestor in path.ancestors() {
        if !fs::symlink_metadata(ancestor)
            .map_err(|_| FILE_ERROR)?
            .is_dir()
        {
            return Err(FILE_ERROR.into());
        }
    }
    Ok(())
}

fn selected_parent(current: &mut PathBuf) -> Result<()> {
    selected_directory_prefix(current)?;
    if !current.pop() {
        return Err(FILE_ERROR.into());
    }
    selected_directory_prefix(current)
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    raw_path(path)?;
    no_links(path.parent().ok_or(FILE_ERROR)?)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| FILE_ERROR)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| FILE_ERROR)?;
    Ok(())
}

pub(super) fn raw_path(path: &Path) -> Result<()> {
    let text = path.to_str().ok_or(FILE_ERROR)?;
    if text.is_empty()
        || text.chars().any(char::is_control)
        || text.split(['/', '\\']).any(|component| component == "..")
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(FILE_ERROR.into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/internal/consumer_context/file_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/internal/consumer_context/selected_frontend_tests.rs"]
mod selected_frontend_tests;
