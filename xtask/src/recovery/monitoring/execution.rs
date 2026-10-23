use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{Result, recovery::clone::native::digest};

use super::{contract::STAGING_DIRECTORY, staging};

struct ProtectedFile {
    path: PathBuf,
    receipt: Value,
    _stream: File,
}

/// 在整个执行阶段保留文件句柄，Windows 同时禁止其他进程写入和删除。
pub(crate) struct ProtectedInputs {
    files: Vec<ProtectedFile>,
    credential_security: Option<(PathBuf, Value)>,
}

impl ProtectedInputs {
    pub(crate) fn open(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self> {
        let mut files = Vec::new();
        for path in paths {
            let before = staging::file_receipt(&path, "execution-input")?;
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(1);
            }
            let stream = options.open(&path)?;
            let metadata = stream.metadata()?;
            let (device, inode) =
                crate::recovery::clone::native::resources::common::device_inode_for(
                    &path, &metadata,
                )?;
            if metadata.len() != before["bytes"]
                || device != before["device"]
                || inode != before["inode"]
            {
                return Err("监控执行输入在锁定期间被替换".into());
            }
            files.push(ProtectedFile {
                path,
                receipt: before,
                _stream: stream,
            });
        }
        let guard = Self {
            files,
            credential_security: None,
        };
        guard.assert_unchanged()?;
        Ok(guard)
    }

    pub(crate) fn binding(binding: &Value) -> Result<Self> {
        let credential = required_path(&binding["credential"])?;
        let manifest = required_path(&binding["staging"])?;
        let root = manifest.parent().ok_or("监控 staging 缺少目录")?;
        if root.file_name().and_then(|name| name.to_str()) != Some(STAGING_DIRECTORY) {
            return Err("监控 staging 必须使用固定目录".into());
        }
        let value = crate::recovery::clone::native::json::read(&manifest)?;
        let mut paths = vec![
            credential,
            required_path(&binding["environment"])?,
            manifest.clone(),
            required_path(&binding["coordinator"]["executable"])?,
        ];
        for row in value["files"]
            .as_array()
            .ok_or("监控 staging 缺少文件清单")?
        {
            let relative = row["path"].as_str().ok_or("监控 staging 文件路径无效")?;
            staging::validate_relative(relative)?;
            paths.push(root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR)));
        }
        let mut guard = Self::open(paths)?;
        let path = required_path(&binding["credential"])?;
        guard.credential_security =
            Some((path.clone(), super::permissions::security_receipt(&path)?));
        guard.assert_unchanged()?;
        Ok(guard)
    }

    pub(crate) fn assert_unchanged(&self) -> Result<()> {
        for file in &self.files {
            let metadata = fs::symlink_metadata(&file.path)?;
            if digest::linked(&metadata)
                || staging::file_receipt(&file.path, "execution-input")? != file.receipt
            {
                return Err("监控执行输入在执行期间发生变化".into());
            }
        }
        if let Some((path, expected)) = &self.credential_security
            && super::permissions::security_receipt(path)? != *expected
        {
            return Err("监控凭据权限在执行期间发生变化".into());
        }
        Ok(())
    }

    pub(crate) fn finish<T>(&self, result: Result<T>) -> Result<T> {
        match (result, self.assert_unchanged()) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Err(primary), Err(check)) => {
                Err(format!("{primary}；执行输入复核同时失败：{check}").into())
            }
        }
    }
}

fn required_path(value: &Value) -> Result<PathBuf> {
    Ok(Path::new(value["path"].as_str().ok_or("监控输入缺少路径")?).to_path_buf())
}
