use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StableArtifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

pub(crate) fn stable_artifact(path: &Path, label: &str) -> Result<StableArtifact, String> {
    stable_artifact_with_hook(path, label, || {})
}

pub(crate) fn stable_artifact_with_hook(
    path: &Path,
    label: &str,
    after_open: impl FnOnce(),
) -> Result<StableArtifact, String> {
    let before = regular_path(path, label)?;
    let mut file = File::open(path).map_err(|_| format!("{label}无法读取"))?;
    let opened_before = file.metadata().map_err(|_| format!("{label}无法核验"))?;
    let opened =
        same_file::Handle::from_file(file.try_clone().map_err(|_| format!("{label}无法核验"))?)
            .map_err(|_| format!("{label}无法核验"))?;
    if opened != same_file::Handle::from_path(path).map_err(|_| format!("{label}无法核验"))?
        || !same_metadata(&before, &opened_before)
    {
        return Err(format!("{label}在打开前被替换"));
    }
    after_open();
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| format!("{label}无法读取"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let opened_after = file.metadata().map_err(|_| format!("{label}无法核验"))?;
    let after = regular_path(path, label)?;
    if opened != same_file::Handle::from_path(path).map_err(|_| format!("{label}无法核验"))?
        || !same_metadata(&opened_before, &opened_after)
        || !same_metadata(&opened_before, &after)
    {
        return Err(format!("{label}读取期间发生变化"));
    }
    Ok(StableArtifact {
        path: path.display().to_string(),
        bytes: opened_before.len(),
        sha256: hex::encode(digest.finalize()),
    })
}

fn same_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

fn regular_path(path: &Path, label: &str) -> Result<std::fs::Metadata, String> {
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
