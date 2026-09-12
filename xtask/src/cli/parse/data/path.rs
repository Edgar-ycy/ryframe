use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::workspace::root_dir;

use super::super::super::model::CliError;

#[derive(Clone, Copy)]
pub(super) enum DataPathRole {
    ControlledInputFile,
    ExternalDirectory,
    ControlledNewOutput,
}

pub(super) fn validate_data_path(
    path: PathBuf,
    option: &str,
    role: DataPathRole,
) -> Result<PathBuf, CliError> {
    if !path.is_absolute() {
        return Err(CliError::new(format!("{option} 必须是绝对路径")));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(CliError::new(format!("{option} 不得包含父目录跳转")));
    }
    let boundary = match role {
        DataPathRole::ControlledInputFile | DataPathRole::ControlledNewOutput => {
            let boundary = root_dir().join(".local-tests");
            if path == boundary || !path.starts_with(&boundary) {
                return Err(CliError::new(format!(
                    "{option} 必须位于当前后端 .local-tests 的子目录"
                )));
            }
            Some(boundary)
        }
        DataPathRole::ExternalDirectory => None,
    };
    if let Some(boundary) = boundary {
        reject_linked_components(&path, &boundary, option)?;
    } else {
        reject_linked_ancestors(&path, option)?;
    }
    validate_role(&path, option, role)?;
    Ok(path)
}

fn reject_linked_ancestors(path: &Path, option: &str) -> Result<(), CliError> {
    for current in path.ancestors() {
        inspect_component(current, option)?;
    }
    Ok(())
}

fn reject_linked_components(path: &Path, boundary: &Path, option: &str) -> Result<(), CliError> {
    let mut current = boundary.to_path_buf();
    inspect_component(&current, option)?;
    for component in path
        .strip_prefix(boundary)
        .expect("调用方已经核对数据路径边界")
        .components()
    {
        current.push(component.as_os_str());
        inspect_component(&current, option)?;
    }
    Ok(())
}

fn inspect_component(path: &Path, option: &str) -> Result<(), CliError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link_or_reparse(&metadata) => Err(CliError::new(format!(
            "{option} 不得经过符号链接或重解析点：{}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::new(format!(
            "无法核验 {option} 路径 {}：{error}",
            path.display()
        ))),
    }
}

fn validate_role(path: &Path, option: &str, role: DataPathRole) -> Result<(), CliError> {
    match role {
        DataPathRole::ControlledInputFile => match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(()),
            _ => Err(CliError::new(format!("{option} 必须是已存在的真实文件"))),
        },
        DataPathRole::ExternalDirectory => match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            _ => Err(CliError::new(format!("{option} 必须是已存在的真实目录"))),
        },
        DataPathRole::ControlledNewOutput => {
            match fs::symlink_metadata(path) {
                Ok(_) => {
                    return Err(CliError::new(format!("{option} 必须指向不存在的新文件")));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(CliError::new(format!(
                        "无法核验 {option} 输出路径 {}：{error}",
                        path.display()
                    )));
                }
            }
            let parent = path
                .parent()
                .ok_or_else(|| CliError::new(format!("{option} 的父目录必须已存在")))?;
            match fs::symlink_metadata(parent) {
                Ok(metadata) if metadata.is_dir() => Ok(()),
                Ok(_) => Err(CliError::new(format!("{option} 的父路径必须是目录"))),
                Err(error) => Err(CliError::new(format!(
                    "无法核验 {option} 父目录 {}：{error}",
                    parent.display()
                ))),
            }
        }
    }
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
