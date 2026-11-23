use std::{
    fs::{self, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    time::SystemTime,
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(PartialEq)]
pub(crate) struct Image {
    pub(crate) bytes: Vec<u8>,
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

pub(crate) fn diagnostic(path: &Path, stage: &str, reason: &str) -> String {
    format!(
        "阶段={stage}；路径={}；原因={reason}；请核对对应文件、字段、权限和隔离目录，保留现场，不自动重放",
        path.display()
    )
}

pub(crate) fn io_error(path: &Path, stage: &str, error: io::Error) -> String {
    diagnostic(
        path,
        stage,
        &format!(
            "IOkind={:?}；rawcode={:?}",
            error.kind(),
            error.raw_os_error()
        ),
    )
}

pub(crate) fn ordinary(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        !metadata.file_type().is_symlink()
            && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
    }
    #[cfg(not(windows))]
    {
        !metadata.file_type().is_symlink()
    }
}

pub(crate) fn directory_available(path: &Path) -> Result<bool, String> {
    if !path.is_absolute() {
        return Err(diagnostic(path, "检查目录", "必须是明确绝对路径"));
    }
    for part in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs::symlink_metadata(part) {
            Ok(metadata) if ordinary(&metadata) && metadata.is_dir() => {}
            Ok(_) => {
                return Err(diagnostic(
                    part,
                    "检查目录",
                    "不是普通目录或存在链接/重解析点",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_error(part, "检查目录", error)),
        }
    }
    Ok(true)
}

pub(crate) fn require_directory(path: &Path) -> Result<(), String> {
    if directory_available(path)? {
        Ok(())
    } else {
        Err(diagnostic(path, "检查目录", "必需目录不存在"))
    }
}

pub(crate) fn create_parents(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(diagnostic(path, "创建目录", "必须是绝对路径"));
    }
    for part in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if !directory_available(part)? {
            match fs::create_dir(part) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io_error(part, "创建受控父目录", error)),
            }
        }
        require_directory(part)?;
    }
    Ok(())
}

fn image(metadata: &Metadata, bytes: Vec<u8>) -> Image {
    Image {
        bytes,
        length: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
    }
}

pub(crate) fn read_optional(path: &Path) -> Result<Option<Image>, String> {
    require_directory(
        path.parent()
            .ok_or_else(|| diagnostic(path, "读取", "缺少父目录"))?,
    )?;
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(path, "读取前检查", error)),
    };
    if !ordinary(&before) || !before.is_file() || before.len() > MAX_BYTES {
        return Err(diagnostic(
            path,
            "读取前检查",
            "要求普通无链接文件且大小不超过16MiB",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0o400000);
    }
    let file = options
        .open(path)
        .map_err(|error| io_error(path, "打开文件", error))?;
    let held = file
        .metadata()
        .map_err(|error| io_error(path, "检查句柄", error))?;
    let expected = image(&before, Vec::new());
    if !ordinary(&held) || !held.is_file() || image(&held, Vec::new()) != expected {
        return Err(diagnostic(path, "检查句柄", "文件身份元数据已变化"));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(path, "有界读取", error))?;
    let after = fs::symlink_metadata(path).map_err(|error| io_error(path, "读取后检查", error))?;
    let held_after = file
        .metadata()
        .map_err(|error| io_error(path, "读取后检查句柄", error))?;
    if !ordinary(&after)
        || !after.is_file()
        || image(&after, Vec::new()) != expected
        || image(&held_after, Vec::new()) != expected
        || bytes.len() as u64 != before.len()
    {
        return Err(diagnostic(
            path,
            "读取后检查",
            "文件在捕获期间变化或超过大小限制",
        ));
    }
    require_directory(path.parent().expect("已检查父目录"))?;
    Ok(Some(image(&before, bytes)))
}

pub(crate) fn read_required(path: &Path) -> Result<Image, String> {
    read_optional(path)?.ok_or_else(|| diagnostic(path, "读取文件", "必需文件不存在"))
}

pub(crate) fn parse<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    path: &Path,
) -> Result<T, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        diagnostic(
            path,
            "UTF8解析",
            &format!(
                "无效编码；byte={}；error_len={:?}",
                error.valid_up_to(),
                error.error_len()
            ),
        )
    })?;
    toml::from_str(text).map_err(|error: toml::de::Error| {
        let reason = error.message();
        let category = if reason.contains("missing field") {
            "缺少必需字段"
        } else if reason.contains("unknown field") {
            "存在未允许的字段"
        } else if reason.contains("duplicate") {
            "重复键或表"
        } else if reason.contains("invalid type") {
            "字段类型不符"
        } else if reason.contains("expected") {
            "语法元素缺失或分隔符错误"
        } else {
            "TOML语法或字段值不符合契约"
        };
        diagnostic(
            path,
            "TOML解析",
            &format!(
                "{category}；byte/span={:?}；按允许字段修正，不输出原文",
                error.span()
            ),
        )
    })
}

pub(crate) fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    require_directory(
        path.parent()
            .ok_or_else(|| diagnostic(path, "发布", "缺少父目录"))?,
    )?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_error(path, "独占创建文件；已有目录/部分文件保留", error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error(path, "写入/持久化；目标已创建，部分文件保留", error))
}
