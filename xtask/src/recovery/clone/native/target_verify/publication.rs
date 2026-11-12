use std::{
    error::Error,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde_json::Value;
use tempfile::NamedTempFile;

use crate::Result;

/// 完整序列化并同步临时收据后才发布，不覆盖已有最终路径。
pub(crate) fn publish(path: &Path, value: &Value) -> Result<()> {
    publish_with(
        path,
        value,
        |file, bytes| file.write_all(bytes),
        File::sync_all,
    )
}

pub(crate) fn publish_from(path: &Path, value: &Value, staging: &Path) -> Result<()> {
    publish_in(
        path,
        value,
        staging,
        "创建发布临时文件",
        |file, bytes| file.write_all(bytes),
        File::sync_all,
    )
}

/// 写入与同步钩子用于确定性核验发布前的失败和并发创建边界。
pub(crate) fn publish_with(
    path: &Path,
    value: &Value,
    writer: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    sync: impl FnOnce(&File) -> io::Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    publish_in(path, value, parent, "创建同目录临时文件", writer, sync)
}

fn publish_in(
    path: &Path,
    value: &Value,
    staging: &Path,
    creation: &'static str,
    writer: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    sync: impl FnOnce(&File) -> io::Result<()>,
) -> Result<()> {
    let mut encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| failure(path, "序列化完整 JSON", error))?;
    encoded.push(b'\n');
    let mut temporary =
        NamedTempFile::new_in(staging).map_err(|error| failure(path, creation, error))?;
    let prepared = (|| {
        writer(temporary.as_file_mut(), &encoded)
            .map_err(|error| failure(path, "写入临时文件", error))?;
        temporary
            .as_file_mut()
            .flush()
            .map_err(|error| failure(path, "刷新临时文件", error))?;
        sync(temporary.as_file()).map_err(|error| failure(path, "同步临时文件", error))
    })();
    if let Err(error) = prepared {
        return fail(temporary, error);
    }
    match temporary.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) => fail(error.file, failure(path, "发布最终文件", error.error)),
    }
}

#[derive(Debug)]
struct PublicationError {
    path: PathBuf,
    stage: &'static str,
    cause: Box<dyn Error>,
    cleanup: Option<io::Error>,
}

impl std::fmt::Display for PublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "目标复核收据{}失败（{}）：{}",
            self.stage,
            self.path.display(),
            self.cause
        )?;
        if let Some(error) = &self.cleanup {
            write!(formatter, "；关闭本次临时文件同时失败：{error}")?;
        }
        Ok(())
    }
}

impl Error for PublicationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

fn failure(path: &Path, stage: &'static str, cause: impl Error + 'static) -> PublicationError {
    PublicationError {
        path: path.to_path_buf(),
        stage,
        cause: Box::new(cause),
        cleanup: None,
    }
}

fn fail(temporary: NamedTempFile, mut error: PublicationError) -> Result<()> {
    error.cleanup = temporary.close().err();
    Err(Box::new(error))
}
