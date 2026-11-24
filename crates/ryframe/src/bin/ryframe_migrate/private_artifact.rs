use std::{
    error::Error,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

pub(super) fn input(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let path = resolve(path)?;
    if !fs::metadata(&path)?.is_file() {
        return Err("私有计划必须是普通文件".into());
    }
    Ok(path)
}

pub(super) fn output(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let path = resolve(path)?;
    if path.exists() {
        return Err(format!(
            "目标文件已存在：{}；请选择新路径，避免覆盖已有初始化凭据或计划",
            path.display()
        )
        .into());
    }
    fs::create_dir_all(path.parent().ok_or("输出文件缺少父目录")?)?;
    Ok(path)
}

fn resolve(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    validate_spelling(path)?;
    let current = std::env::current_dir()?;
    let root = current
        .ancestors()
        .find(|path| path.join("Cargo.toml").is_file() && path.join("crates/ryframe").is_dir())
        .ok_or("请在后端 Cargo Workspace 内执行")?;
    let absolute = path.is_absolute();
    let path = if absolute {
        path.to_path_buf()
    } else {
        if path.has_root() || matches!(path.components().next(), Some(Component::Prefix(_))) {
            return Err("普通私有文件须使用检出目录之外的绝对路径".into());
        }
        root.join(path)
    };
    let maintenance = path.strip_prefix(root).is_ok_and(maintenance_file);
    if !absolute && !maintenance {
        return Err(
            "普通私有文件须使用检出目录之外的绝对路径；维护可使用 .local-tests 下的相对路径".into(),
        );
    }
    let physical = checked_path(&path, maintenance.then_some(root))?;
    let root = fs::canonicalize(root)?;
    // 只有核对到实际工作区目录中的 .local-tests，才能使用维护例外。
    if physical.strip_prefix(&root).is_ok_and(maintenance_file) {
        return Ok(path);
    }
    if !absolute || strip_workspace_prefix(&physical, &root).is_some() {
        return Err("私有文件须位于当前后端源码树之外；维护可使用当前源码树的 .local-tests".into());
    }
    for directory in physical.ancestors().skip(1) {
        reject_checkout(directory)?;
    }
    Ok(path)
}

fn validate_spelling(path: &Path) -> Result<(), Box<dyn Error>> {
    if path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("私有文件路径必须指定文件名且不能包含 ..".into());
    }
    #[cfg(windows)]
    for part in path.components() {
        use std::path::Prefix;
        match part {
            Component::Prefix(prefix)
                if !matches!(
                    prefix.kind(),
                    Prefix::Disk(_)
                        | Prefix::VerbatimDisk(_)
                        | Prefix::UNC(..)
                        | Prefix::VerbatimUNC(..)
                ) =>
            {
                return Err("私有文件路径不允许使用 Windows 设备命名空间".into());
            }
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if name.ends_with(['.', ' ']) || name.contains([':', '/']) {
                    return Err("私有文件路径不允许 Windows 尾点、尾空格或特殊路径语法".into());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn maintenance_file(relative: &Path) -> bool {
    relative
        .strip_prefix(".local-tests")
        .is_ok_and(|tail| !tail.as_os_str().is_empty())
}

fn checked_path(path: &Path, maintenance_root: Option<&Path>) -> Result<PathBuf, Box<dyn Error>> {
    let mut first_existing = None;
    for ancestor in path.ancestors() {
        if maintenance_root == Some(ancestor) {
            first_existing.get_or_insert(ancestor);
            break;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err("私有文件路径不允许符号链接".into());
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err("私有文件路径不允许重解析点".into());
                    }
                }
                if ancestor != path && !metadata.is_dir() {
                    return Err("私有文件路径的父级必须是目录".into());
                }
                first_existing.get_or_insert(ancestor);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let ancestor = first_existing.ok_or("私有文件路径没有可核验的父目录")?;
    let mut physical = fs::canonicalize(ancestor)?;
    let tail = path.strip_prefix(ancestor)?;
    if !tail.as_os_str().is_empty() {
        physical.push(tail);
    }
    Ok(physical)
}

fn reject_checkout(directory: &Path) -> Result<(), Box<dyn Error>> {
    match fs::symlink_metadata(directory.join(".git")) {
        Ok(_) => Err("私有文件须位于 Git 检出目录之外".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn strip_workspace_prefix<'a>(path: &'a Path, base: &Path) -> Option<&'a Path> {
    let mut actual = path.components();
    for expected in base.components() {
        if !same_component(actual.next()?, expected) {
            return None;
        }
    }
    Some(actual.as_path())
}

fn same_component(left: Component<'_>, right: Component<'_>) -> bool {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        match (left, right) {
            (Component::Prefix(left), Component::Prefix(right)) => {
                return match (left.kind(), right.kind()) {
                    (
                        Prefix::Disk(left) | Prefix::VerbatimDisk(left),
                        Prefix::Disk(right) | Prefix::VerbatimDisk(right),
                    ) => left.eq_ignore_ascii_case(&right),
                    (
                        Prefix::UNC(left_server, left_share)
                        | Prefix::VerbatimUNC(left_server, left_share),
                        Prefix::UNC(right_server, right_share)
                        | Prefix::VerbatimUNC(right_server, right_share),
                    ) => {
                        left_server.eq_ignore_ascii_case(right_server)
                            && left_share.eq_ignore_ascii_case(right_share)
                    }
                    _ => left == right,
                };
            }
            (Component::Normal(left), Component::Normal(right)) => {
                return left.eq_ignore_ascii_case(right);
            }
            _ => {}
        }
    }
    left == right
}

pub(super) fn publish(output: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = output.with_extension(format!("{}-{nonce}.partial", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temporary, output)?;
        Ok(())
    })();
    let cleanup = fs::remove_file(&temporary);
    result?;
    cleanup?;
    Ok(())
}
