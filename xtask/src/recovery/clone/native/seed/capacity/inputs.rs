//! 固定本次实际读取的文件及动作目录；结束时重验，不修复或覆盖证据。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{json as document, resources::common},
};

#[derive(PartialEq, Eq)]
struct File {
    binding: Value,
    identity: (u64, u64),
}

#[derive(PartialEq, Eq)]
struct Directory {
    identity: (u64, u64),
    children: Vec<(String, (u64, u64))>,
}

pub(super) struct Reads<'a> {
    pub(super) backend: &'a Path,
    files: BTreeMap<PathBuf, File>,
    directories: BTreeMap<PathBuf, Option<Directory>>,
}

impl<'a> Reads<'a> {
    pub(super) fn new(backend: &'a Path) -> Self {
        Self {
            backend,
            files: BTreeMap::new(),
            directories: BTreeMap::new(),
        }
    }

    pub(super) fn binding(&mut self, path: &Path) -> Result<Value> {
        let current = file(self.backend, path)?;
        let value = current.binding.clone();
        if self
            .files
            .get(path)
            .is_some_and(|previous| *previous != current)
        {
            return Err("配额证据在重复读取期间变化；保留现场，禁止自动重放".into());
        }
        self.files.insert(path.to_path_buf(), current);
        Ok(value)
    }

    pub(super) fn read(&mut self, path: &Path) -> Result<Value> {
        let before = self.binding(path)?;
        let value = document::read(path)?;
        if self.binding(path)? != before {
            return Err("配额证据在读取期间变化".into());
        }
        Ok(value)
    }

    pub(super) fn bound(&mut self, descriptor: &Value) -> Result<Value> {
        let path = common::bound_file(self.backend, descriptor, "配额绑定证据")?;
        let value = self.read(&path)?;
        common::bound_file(self.backend, descriptor, "配额绑定证据")?;
        Ok(value)
    }

    pub(super) fn directories(&mut self, root: &Path) -> Result<Vec<PathBuf>> {
        let current = directory(self.backend, root)?;
        let children = current
            .as_ref()
            .map(|value| {
                value
                    .children
                    .iter()
                    .map(|(name, _)| root.join(name))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(previous) = self.directories.insert(root.to_path_buf(), current)
            && Some(&previous) != self.directories.get(root)
        {
            return Err("配额动作目录在重复读取期间变化".into());
        }
        Ok(children)
    }

    pub(super) fn check(&self) -> Result<()> {
        for (path, expected) in &self.files {
            if file(self.backend, path)? != *expected {
                return Err("配额文件绑定或身份变化；保留现场，禁止自动重放".into());
            }
        }
        for (path, expected) in &self.directories {
            if directory(self.backend, path)? != *expected {
                return Err("配额动作目录变化；保留现场，禁止自动重放".into());
            }
        }
        Ok(())
    }
}

fn file(backend: &Path, path: &Path) -> Result<File> {
    common::common_file(backend, path, "配额证据")?;
    let identity = common::device_inode_for(path, &fs::symlink_metadata(path)?)?;
    let binding = common::binding(path)?;
    common::common_file(backend, path, "配额证据")?;
    if common::device_inode_for(path, &fs::symlink_metadata(path)?)? != identity
        || common::binding(path)? != binding
    {
        return Err("配额文件在摘要读取期间变化".into());
    }
    Ok(File { binding, identity })
}

fn directory(backend: &Path, root: &Path) -> Result<Option<Directory>> {
    validate_local_test_path(root, backend, LocalTestPathKind::StateDirectory)?;
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    common::local_directory(backend, root, "配额动作根目录")?;
    let identity = common::device_inode_for(root, &fs::symlink_metadata(root)?)?;
    let mut paths = fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    let mut children = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let expected = format!("{:04}", index + 1);
        if path.file_name().and_then(|name| name.to_str()) != Some(expected.as_str()) {
            return Err("配额动作目录不连续或含未知文件".into());
        }
        common::local_directory(backend, path, "配额动作目录")?;
        children.push((
            expected,
            common::device_inode_for(path, &fs::symlink_metadata(path)?)?,
        ));
    }
    if common::device_inode_for(root, &fs::symlink_metadata(root)?)? != identity {
        return Err("配额动作目录在枚举期间被替换".into());
    }
    Ok(Some(Directory { identity, children }))
}
