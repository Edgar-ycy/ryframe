use std::{
    fmt::Write as _,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::{Result, process::process_is_running};

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

pub(super) fn copying_generation_name(name: &str) -> Option<&str> {
    name.strip_prefix('.')
        .and_then(|name| name.strip_suffix(".copying"))
        .filter(|name| {
            valid_generation_name(name)
                || name.strip_prefix('g').is_some_and(|value| {
                    value.bytes().any(|byte| byte.is_ascii_digit())
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || byte == b'-')
                })
        })
}

pub(super) fn session_is_active(session: &str) -> bool {
    session
        .strip_prefix("s-")
        .and_then(|value| value.split_once('-'))
        .and_then(|(pid, _)| pid.parse::<u32>().ok())
        .is_some_and(process_is_running)
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
    fs::create_dir_all(target)?;
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source_path, &target_path)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}

pub(super) fn directory_hash(directory: &Path) -> Result<String> {
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
        }
    }
    Ok(())
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
