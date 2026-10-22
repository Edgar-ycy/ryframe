use std::{
    fmt::Write as _,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::Result;

pub(super) fn io_error(
    stage: &str,
    path: &Path,
    error: std::io::Error,
) -> Box<dyn std::error::Error> {
    format!("{stage}失败：路径 {}，error.kind={:?}，raw_os_error={:?}；请检查普通路径、权限和磁盘空间，保留部分现场后重试",path.display(),error.kind(),error.raw_os_error()).into()
}

pub(super) fn named_directories(parent: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut directories = Vec::new();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            directories.push((
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            ));
        }
    }
    directories.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(directories)
}

pub(super) fn valid_session_name(name: &str) -> bool {
    numeric_pair(name, "s-")
}

pub(super) fn valid_generation_name(name: &str) -> bool {
    numeric_pair(name, "g-")
}

fn numeric_pair(name: &str, prefix: &str) -> bool {
    let Some(value) = name.strip_prefix(prefix) else {
        return false;
    };
    let Some((left, right)) = value.split_once('-') else {
        return false;
    };
    !left.is_empty()
        && !right.is_empty()
        && left.bytes().all(|byte| byte.is_ascii_digit())
        && right.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

pub(super) fn plain_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

pub(super) fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(format!("运行输入目录不存在：{}", source.display()).into());
    }
    require_plain_tree(source)?;
    fs::create_dir(target)?;
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source_path, &target_path)?;
        } else if entry.file_type()?.is_file() {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target_path)?;
            std::io::copy(&mut fs::File::open(&source_path)?, &mut output)?;
        } else {
            return Err(format!(
                "拒绝复制非普通运行输入 {}；请移除链接或特殊文件",
                source_path.display()
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn directory_hash(directory: &Path) -> Result<String> {
    require_plain_tree(directory)?;
    let mut files = Vec::new();
    collect_files(directory, directory, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (relative, path) in files {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0]);
    }
    hex_digest(digest.finalize())
}

pub(super) fn file_hash(path: &Path) -> Result<String> {
    require_plain_file(path)?;
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    hex_digest(digest.finalize())
}

fn hex_digest(bytes: impl IntoIterator<Item = u8>) -> Result<String> {
    let bytes = bytes.into_iter();
    let mut output = String::with_capacity(bytes.size_hint().0 * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}")?;
    }
    Ok(output)
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<(String, PathBuf)>) -> Result<()> {
    require_plain_ancestors(directory)?;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, files)?;
        } else if entry.file_type()?.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, path));
        } else {
            return Err(format!(
                "运行输入 {} 包含链接或特殊文件；请恢复普通文件快照",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn require_plain_file(path: &Path) -> Result<()> {
    require_plain_ancestors(path)?;
    if !plain_file(path) {
        return Err(format!("{} 不是普通文件；请恢复已登记产物", path.display()).into());
    }
    Ok(())
}

pub(super) fn require_plain_tree(path: &Path) -> Result<()> {
    require_plain_ancestors(path)?;
    if !plain_directory(path) {
        return Err(format!("{} 不是普通目录；请恢复已登记输入", path.display()).into());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        require_plain_ancestors(&child)?;
        if entry.file_type()?.is_dir() {
            require_plain_tree(&child)?;
        } else if !entry.file_type()?.is_file() {
            return Err(format!("{} 不是普通文件；请移除链接或特殊输入", child.display()).into());
        }
    }
    Ok(())
}

fn require_plain_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(|error| {
            format!(
                "检查快照路径 {} 失败：error.kind={:?}, raw_os_error={:?}；请核对路径和访问权限",
                ancestor.display(),
                error.kind(),
                error.raw_os_error()
            )
        })?;
        if linked(&metadata) {
            return Err(format!(
                "快照路径 {} 含链接或重解析点；请指定普通路径",
                ancestor.display()
            )
            .into());
        }
    }
    Ok(())
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

pub(super) fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

pub(super) fn nonce() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间早于 Unix epoch：{error}"))?
        .as_nanos())
}

pub(super) fn create_plain_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => require_plain_ancestors(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("创建快照目录缺少普通父路径")?;
            create_plain_directory(parent)?;
            fs::create_dir(path).map_err(|error|format!("创建快照目录 {} 失败：error.kind={:?}, raw_os_error={:?}；请核对路径及访问权限",path.display(),error.kind(),error.raw_os_error()))?;
            Ok(())
        }
        Err(error) => Err(format!(
            "检查快照目录 {} 失败：error.kind={:?}, raw_os_error={:?}；请核对目录及访问权限",
            path.display(),
            error.kind(),
            error.raw_os_error()
        )
        .into()),
    }
}
