use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

pub(super) fn plain_directory_chain(path: &Path) -> Result<()> {
    let absolute = absolute(path)?;
    let mut prefix = PathBuf::new();
    // 按路径分量检查，避免先消去 .. 而漏查被跨过的链接祖先。
    for component in absolute.components() {
        prefix.push(component);
        if !prefix.has_root() {
            continue;
        }
        let metadata = fs::symlink_metadata(&prefix).map_err(|error| {
            io_error(
                &format!("检查配置目录 {} 的祖先", path.display()),
                &prefix,
                &error,
                "请创建缺失的普通目录，并检查祖先路径及读取权限",
            )
        })?;
        if linked(&metadata) || !metadata.is_dir() {
            return Err(format!(
                "检查配置目录 {} 失败：祖先 {} 实际为{}；请改用无链接、无重解析点的普通目录",
                path.display(),
                prefix.display(),
                kind(&metadata),
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn plain_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        io_error(
            "检查已登记配置文件类型",
            path,
            &error,
            "请提供普通文件并检查读取权限",
        )
    })?;
    if linked(&metadata) || !metadata.is_file() {
        return Err(format!(
            "检查已登记配置入口 {} 失败：实际为{}；请替换为无链接、无重解析点的普通文件",
            path.display(),
            kind(&metadata),
        )
        .into());
    }
    Ok(())
}

pub(super) fn new_directory_path(path: &Path) -> Result<PathBuf> {
    let absolute = absolute(path)?;
    let name = absolute.file_name().ok_or_else(|| {
        format!(
            "准备配置目标 {} 失败：缺少新目录名；请指定父目录下尚不存在的子目录",
            path.display(),
        )
    })?;
    if name == "." || name == ".." {
        return Err(format!(
            "准备配置目标 {} 失败：目录名不能为 . 或 ..；请指定新的子目录名",
            path.display()
        )
        .into());
    }
    let parent = absolute.parent().ok_or_else(|| {
        format!(
            "准备配置目标 {} 失败：缺少父目录；请指定已有普通父目录下的新目标",
            path.display(),
        )
    })?;
    plain_directory_chain(parent)
        .map_err(|error| format!("准备新配置目标 {} 失败：{error}", absolute.display(),))?;
    match fs::symlink_metadata(&absolute) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(absolute),
        Err(error) => Err(io_error(
            "检查新配置目标是否存在",
            &absolute,
            &error,
            "请检查目标路径和访问权限，然后指定尚不存在的目标目录",
        )
        .into()),
        Ok(metadata) => Err(format!(
            "准备配置目标 {} 失败：目标已存在，实际为{}；拒绝覆盖，请指定新的目标目录",
            absolute.display(),
            kind(&metadata),
        )
        .into()),
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err("确定配置路径失败：路径为空；请指定明确的源目录或新目标目录".into());
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                io_error(
                    "确定相对配置路径的当前工作目录",
                    path,
                    &error,
                    "请恢复可访问的当前工作目录，或改用明确的绝对路径",
                )
            })?
            .join(path)
    };
    if !absolute.is_absolute() {
        return Err(format!(
            "确定配置路径 {} 失败：无法相对当前工作目录确定绝对位置；请改用明确的绝对路径",
            path.display()
        )
        .into());
    }
    Ok(absolute)
}

pub(super) fn io_error(stage: &str, path: &Path, error: &std::io::Error, repair: &str) -> String {
    format!(
        "{stage}失败：路径 {}，error.kind={:?}，raw_os_error={:?}；{repair}",
        path.display(),
        error.kind(),
        error.raw_os_error(),
    )
}

fn kind(metadata: &fs::Metadata) -> &'static str {
    if linked(metadata) {
        "链接或重解析点"
    } else if metadata.is_dir() {
        "目录"
    } else if metadata.is_file() {
        "普通文件"
    } else {
        "特殊文件"
    }
}

#[cfg(windows)]
fn linked(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn linked(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
