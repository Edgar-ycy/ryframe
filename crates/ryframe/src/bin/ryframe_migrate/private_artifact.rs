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
    let current = std::env::current_dir()?;
    let root = current
        .ancestors()
        .find(|path| path.join("Cargo.toml").is_file() && path.join("crates/ryframe").is_dir())
        .ok_or("请在后端 Cargo Workspace 内执行")?;
    let private = root.join(".local-tests");
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let relative = strip_workspace_prefix(&path, &private);
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
        || relative.is_none_or(|tail| tail.as_os_str().is_empty())
    {
        return Err("私有凭据或计划仅允许位于当前后端 .local-tests 下，且路径不能包含 ..".into());
    }
    for ancestor in path.ancestors().take_while(|path| {
        !strip_workspace_prefix(path, root).is_some_and(|tail| tail.as_os_str().is_empty())
    }) {
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
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
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
    if let (Component::Prefix(left), Component::Prefix(right)) = (left, right) {
        use std::path::Prefix;
        return match (left.kind(), right.kind()) {
            (
                Prefix::Disk(left) | Prefix::VerbatimDisk(left),
                Prefix::Disk(right) | Prefix::VerbatimDisk(right),
            ) => left == right,
            (
                Prefix::UNC(left_server, left_share) | Prefix::VerbatimUNC(left_server, left_share),
                Prefix::UNC(right_server, right_share)
                | Prefix::VerbatimUNC(right_server, right_share),
            ) => left_server == right_server && left_share == right_share,
            _ => left == right,
        };
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
