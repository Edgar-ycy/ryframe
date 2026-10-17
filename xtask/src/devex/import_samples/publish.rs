use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use uuid::Uuid;

use crate::Result;

use super::model::{MAX_TEMPLATE_BYTES, PublishedFile, sha256_hex, valid_sha256};

pub(crate) fn read_template_file(path: &Path, expected_sha256: &str) -> Result<Vec<u8>> {
    validate_existing_file(path)?;
    if !valid_sha256(expected_sha256) {
        return Err("模板SHA与固定契约不一致".into());
    }
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_TEMPLATE_BYTES {
        return Err("模板必须为显式绝对路径普通文件且不超过16MiB".into());
    }
    let mut file = File::open(path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() > MAX_TEMPLATE_BYTES {
        return Err("模板必须为显式绝对路径普通文件且不超过16MiB".into());
    }
    let mut content = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
    Read::by_ref(&mut file)
        .take(MAX_TEMPLATE_BYTES + 1)
        .read_to_end(&mut content)?;
    if content.len() as u64 > MAX_TEMPLATE_BYTES {
        return Err("模板必须为显式绝对路径普通文件且不超过16MiB".into());
    }
    if sha256_hex(&content) != expected_sha256 {
        return Err("模板SHA与固定契约不一致".into());
    }
    Ok(content)
}

pub(crate) fn create_new_directory(path: &Path) -> Result<()> {
    validate_absolute_path(path)?;
    let parent = path.parent().ok_or("样本目录必须使用明确绝对路径")?;
    validate_directory_chain(parent)?;
    if fs::symlink_metadata(path).is_ok() {
        return Err("样本目录已存在或父目录未登记，拒绝复用或覆盖".into());
    }
    fs::create_dir(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            "样本目录已存在或父目录未登记，拒绝复用或覆盖".into()
        } else {
            error.into()
        }
    })
}

pub(crate) fn publish_new(path: &Path, content: &[u8]) -> Result<PublishedFile> {
    validate_new_file_path(path)?;
    let parent = path.parent().ok_or("输出必须使用明确绝对路径")?;
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("输出必须使用明确绝对路径")?;
    let temporary = parent.join(format!(".{filename}.{}.tmp", Uuid::new_v4().simple()));
    let result: Result<()> = (|| {
        let mut stream = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        stream.write_all(content)?;
        stream.sync_all()?;
        drop(stream);
        fs::hard_link(&temporary, path)?;
        Ok(())
    })();
    let cleanup = fs::remove_file(&temporary);
    result?;
    if let Err(error) = cleanup
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("临时样本清理失败：{error}").into());
    }
    Ok(PublishedFile {
        path: path
            .to_str()
            .ok_or("输出路径必须能表示为 UTF-8")?
            .to_owned(),
        bytes: content.len(),
        sha256: sha256_hex(content),
    })
}

fn validate_existing_file(path: &Path) -> Result<()> {
    validate_absolute_path(path)?;
    let parent = path.parent().ok_or("模板必须为显式绝对路径普通文件")?;
    validate_directory_chain(parent)?;
    let metadata = fs::symlink_metadata(path)?;
    if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
        return Err("模板必须为显式绝对路径普通文件且不超过16MiB".into());
    }
    Ok(())
}

fn validate_new_file_path(path: &Path) -> Result<()> {
    validate_absolute_path(path)?;
    let parent = path.parent().ok_or("输出必须使用明确绝对路径")?;
    validate_directory_chain(parent)?;
    if fs::symlink_metadata(path).is_ok() {
        return Err("输出必须是不存在且父目录已创建的绝对路径".into());
    }
    Ok(())
}

fn validate_absolute_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("路径必须是显式绝对路径且不得包含路径跳转".into());
    }
    Ok(())
}

fn validate_directory_chain(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err("父目录必须使用明确绝对路径".into());
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)?;
        if is_link_or_reparse(&metadata) {
            return Err("父目录不得为链接或重解析点".into());
        }
        if !metadata.is_dir() {
            return Err("父目录已创建的绝对路径".into());
        }
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
