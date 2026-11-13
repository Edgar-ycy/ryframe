use std::path::Path;

pub(super) fn same_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

pub(super) fn regular_path(path: &Path, label: &str) -> Result<std::fs::Metadata, String> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(format!("{label}必须是规范绝对文件路径"));
    }
    reject_link_ancestors(path, label)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| format!("{label}不存在"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{label}必须是普通文件"));
    }
    Ok(metadata)
}

fn reject_link_ancestors(path: &Path, label: &str) -> Result<(), String> {
    for ancestor in path.ancestors() {
        let Ok(metadata) = std::fs::symlink_metadata(ancestor) else {
            continue;
        };
        if metadata.file_type().is_symlink() || is_reparse(&metadata) {
            return Err(format!("{label}路径不能经过链接或重解析点"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(_: &std::fs::Metadata) -> bool {
    false
}
