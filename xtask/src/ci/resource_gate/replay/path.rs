use std::{
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) fn resolve_requested_path(root: &Path, value: &Path) -> Result<PathBuf, String> {
    let root = canonical_path(root)?;
    let requested = if value.is_absolute() {
        value.to_path_buf()
    } else {
        root.join(value)
    };
    if requested
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("replay 路径不得包含父目录或当前目录跳转".to_owned());
    }
    if !requested.is_absolute() {
        return Err("replay 路径必须是绝对路径".to_owned());
    }
    let mut existing = requested.as_path();
    let mut missing = Vec::<OsString>::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| format!("replay 路径缺少有效文件名：{}", requested.display()))?;
        missing.push(name.to_owned());
        existing = existing
            .parent()
            .ok_or_else(|| format!("replay 路径缺少现有父目录：{}", requested.display()))?;
    }
    let mut resolved = canonical_path(existing)
        .map_err(|error| format!("无法解析 replay 路径 {}：{error}", existing.display()))?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

pub(super) fn validate_inside(path: &Path, allowed: &Path, label: &str) -> Result<(), String> {
    if path == allowed || !path.starts_with(allowed) {
        return Err(format!(
            "{label} 必须位于 {} 的子路径，实际为 {}",
            allowed.display(),
            path.display()
        ));
    }
    Ok(())
}

pub(super) fn ensure_real_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !is_link_like(&metadata) => Ok(()),
        Ok(_) => Err(format!("replay 目录必须是普通目录：{}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .map_err(|error| format!("无法创建 replay 目录 {}：{error}", path.display()))?;
            let metadata = fs::symlink_metadata(path)
                .map_err(|error| format!("无法核验 replay 目录 {}：{error}", path.display()))?;
            if metadata.is_dir() && !is_link_like(&metadata) {
                Ok(())
            } else {
                Err(format!("replay 目录创建后不是普通目录：{}", path.display()))
            }
        }
        Err(error) => Err(format!("无法核验 replay 目录 {}：{error}", path.display())),
    }
}

pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    normalized_path(left) == normalized_path(right)
}

pub(super) fn normalized_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    {
        text.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        text
    }
}

pub(super) fn canonical_or_original(path: &Path) -> PathBuf {
    canonical_path(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn canonical_path(path: &Path) -> Result<PathBuf, String> {
    let resolved = fs::canonicalize(path)
        .map_err(|error| format!("无法解析路径 {}：{error}", path.display()))?;
    Ok(strip_verbatim_prefix(resolved))
}

#[cfg(windows)]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

#[cfg(not(windows))]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(windows)]
pub(super) fn is_link_like(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub(super) fn is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
