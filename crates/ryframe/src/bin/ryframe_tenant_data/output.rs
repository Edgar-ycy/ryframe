use ryframe_kernel::{AppError, AppResult};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub fn validate_new_output(output: &Path) -> AppResult<PathBuf> {
    let name = output
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| AppError::Validation("恢复记录输出必须是明确文件".into()))?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|_| AppError::Validation("恢复记录输出父目录必须已经存在".into()))?;
    if !parent.is_dir() {
        return Err(AppError::Validation("恢复记录输出父路径必须是目录".into()));
    }
    let target = parent.join(name);
    match target.symlink_metadata() {
        Ok(_) => Err(AppError::Validation(
            "恢复记录输出必须是不存在的新文件".into(),
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(target),
        Err(_) => Err(AppError::Validation("无法核对恢复记录输出路径".into())),
    }
}

fn canonical_bytes(value: &impl Serialize) -> AppResult<(Value, Vec<u8>)> {
    let canonical = serde_json::to_value(value)
        .map_err(|_| AppError::Internal("无法规范化恢复维护结果".into()))?;
    let mut bytes = serde_json::to_vec_pretty(&canonical)
        .map_err(|_| AppError::Internal("无法序列化恢复维护结果".into()))?;
    bytes.push(b'\n');
    Ok((canonical, bytes))
}

fn staging_file(target: &Path) -> AppResult<(PathBuf, File)> {
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Validation("恢复记录输出父目录无效".into()))?;
    let name = target
        .file_name()
        .ok_or_else(|| AppError::Validation("恢复记录输出文件名无效".into()))?
        .to_string_lossy();
    for _ in 0..64 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{name}.ryframe-{}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err(AppError::Internal("无法创建恢复记录临时文件".into()));
            }
        }
    }
    Err(AppError::Internal("无法取得唯一恢复记录临时文件".into()))
}

pub(crate) fn verify_published(
    path: &Path,
    expected: &Value,
    expected_bytes: &[u8],
) -> AppResult<()> {
    const MAX_BYTES: u64 = 16 * 1024 * 1024;
    let mut observed = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut observed))
        .map_err(|_| AppError::Internal("恢复记录写后无法重读".into()))?;
    if observed.len() as u64 > MAX_BYTES || observed != expected_bytes {
        return Err(AppError::Internal("恢复记录写后字节与规范结果不同".into()));
    }
    let parsed: Value = serde_json::from_slice(&observed)
        .map_err(|_| AppError::Internal("恢复记录写后格式无效".into()))?;
    if &parsed != expected {
        return Err(AppError::Internal("恢复记录写后内容与规范结果不同".into()));
    }
    Ok(())
}

pub fn publish_json(output: &Path, value: &impl Serialize) -> AppResult<PathBuf> {
    let target = validate_new_output(output)?;
    let (canonical, bytes) = canonical_bytes(value)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(AppError::Validation("恢复记录不能超过 16 MiB".into()));
    }
    let (staging, mut file) = staging_file(&target)?;
    let result = (|| {
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| AppError::Internal("恢复记录临时文件写入失败".into()))?;
        drop(file);
        verify_published(&staging, &canonical, &bytes)?;
        std::fs::hard_link(&staging, &target).map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                AppError::Validation("恢复记录输出已被并发创建，禁止覆盖".into())
            } else {
                AppError::Internal("恢复记录无法原子发布到目标路径".into())
            }
        })?;
        verify_published(&target, &canonical, &bytes).map_err(|_| {
            AppError::Internal("恢复记录已经创建但写后复核失败；必须保留并核对".into())
        })?;
        let observed = output.canonicalize().map_err(|_| {
            AppError::Internal("恢复记录已经创建但规范路径无法复核；必须保留并核对".into())
        })?;
        if observed != target {
            return Err(AppError::Internal(
                "恢复记录已经创建但规范路径发生变化；必须保留并核对".into(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = std::fs::remove_file(&staging)
        && result.is_ok()
    {
        eprintln!("恢复记录已成功发布，但临时硬链接清理失败：{error}");
    }
    result.map(|()| target)
}

pub fn publish_result<T: Serialize>(
    output: &Path,
    result: AppResult<T>,
    label: &str,
) -> AppResult<()> {
    let value = result?;
    let target = publish_json(output, &value)?;
    println!("{label}已写入不可覆盖的新文件：{}", target.display());
    Ok(())
}
