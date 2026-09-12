use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalTestPathKind {
    ExistingFile,
    OutputFile,
    StateDirectory,
}

/// 核验一个绝对路径由当前后端 `.local-tests` 直接拥有，且现有路径段不经过链接。
pub(crate) fn validate_local_test_path(
    value: &Path,
    root: &Path,
    kind: LocalTestPathKind,
) -> Result<PathBuf, String> {
    validate_lexical_path(value)?;
    let boundary = root.join(".local-tests");
    let relative = value
        .strip_prefix(&boundary)
        .map_err(|_| "路径必须位于当前后端 .local-tests 的子目录".to_owned())?;
    if relative.as_os_str().is_empty() {
        return Err("路径不能是 .local-tests 根目录".to_owned());
    }
    let boundary_metadata = fs::symlink_metadata(&boundary).map_err(|error| {
        format!(
            "无法核验当前后端 .local-tests {}：{error}",
            boundary.display()
        )
    })?;
    if !boundary_metadata.is_dir() || is_link_like(&boundary_metadata) {
        return Err("当前后端 .local-tests 必须是真实目录，不能是链接或 junction".to_owned());
    }
    let canonical_boundary = fs::canonicalize(&boundary)
        .map_err(|error| format!("无法解析当前后端 .local-tests：{error}"))?;
    validate_existing_components(value, &boundary, &canonical_boundary)?;
    validate_target_kind(value, kind)?;
    Ok(value.to_path_buf())
}

fn validate_lexical_path(value: &Path) -> Result<(), String> {
    if !value.is_absolute() {
        return Err("路径必须是绝对路径".to_owned());
    }
    if value.as_os_str().is_empty()
        || value
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("路径不得为空或包含父目录、当前目录跳转".to_owned());
    }
    let text = value
        .to_str()
        .ok_or_else(|| "路径必须能表示为 UTF-8".to_owned())?;
    if text.contains(['\n', '\r', '\0']) {
        return Err("路径不得包含换行符或 NUL".to_owned());
    }
    Ok(())
}

fn validate_existing_components(
    value: &Path,
    boundary: &Path,
    canonical_boundary: &Path,
) -> Result<(), String> {
    let relative = value
        .strip_prefix(boundary)
        .map_err(|_| "路径必须位于当前后端 .local-tests 的子目录".to_owned())?;
    let mut current = boundary.to_path_buf();
    let mut missing_parent = false;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        current.push(component.as_os_str());
        if missing_parent {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if is_link_like(&metadata) {
                    return Err(format!(
                        "路径不能经过符号链接或 junction：{}",
                        current.display()
                    ));
                }
                if components.peek().is_some() && !metadata.is_dir() {
                    return Err(format!("路径的中间部分必须是目录：{}", current.display()));
                }
                let resolved = fs::canonicalize(&current)
                    .map_err(|error| format!("无法解析路径 {}：{error}", current.display()))?;
                if !resolved.starts_with(canonical_boundary) {
                    return Err("路径解析后越过当前后端 .local-tests 边界".to_owned());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing_parent = true;
            }
            Err(error) => {
                return Err(format!("无法核验路径 {}：{error}", current.display()));
            }
        }
    }
    Ok(())
}

fn validate_target_kind(value: &Path, kind: LocalTestPathKind) -> Result<(), String> {
    match fs::symlink_metadata(value) {
        Ok(metadata) => match kind {
            LocalTestPathKind::ExistingFile if metadata.is_file() => Ok(()),
            LocalTestPathKind::ExistingFile => Err("输入路径必须是现有普通文件".to_owned()),
            LocalTestPathKind::OutputFile => Err("输出文件已存在，拒绝覆盖".to_owned()),
            LocalTestPathKind::StateDirectory if metadata.is_dir() => Ok(()),
            LocalTestPathKind::StateDirectory => Err("账本路径必须是目录".to_owned()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match kind {
            LocalTestPathKind::ExistingFile => Err("输入文件不存在".to_owned()),
            LocalTestPathKind::OutputFile | LocalTestPathKind::StateDirectory => Ok(()),
        },
        Err(error) => Err(format!("无法核验目标路径 {}：{error}", value.display())),
    }
}

#[cfg(windows)]
fn is_link_like(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
