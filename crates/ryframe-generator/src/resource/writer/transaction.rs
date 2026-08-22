use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{ResourceError, content_hash};

/// 回滚不完整时保留同卷事务目录，供人工从 `.backup` 恢复。
pub(super) fn persist_recovery_directories(
    backend_stage: tempfile::TempDir,
    frontend_stage: Option<tempfile::TempDir>,
) -> Vec<PathBuf> {
    let mut directories = vec![backend_stage.keep().join(".backup")];
    if let Some(frontend_stage) = frontend_stage {
        directories.push(frontend_stage.keep().join(".backup"));
    }
    directories
}

#[derive(Debug, Clone)]
pub(super) enum ExpectedFile {
    Absent,
    ContentHash(String),
    ExactBytes(Vec<u8>),
}

impl ExpectedFile {
    pub(super) fn exists(&self) -> bool {
        !matches!(self, Self::Absent)
    }
}

#[derive(Debug)]
pub(super) struct InstalledFile {
    pub(super) path: PathBuf,
    pub(super) content_hash: String,
}

pub(super) fn write_staged(
    path: &Path,
    content: &str,
    resource: &str,
) -> Result<(), ResourceError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ResourceError::file(
                parent,
                format!("无法创建临时目录：{error}"),
                "确认工作区可写且磁盘空间充足",
            )
            .with_resource(resource)
        })?;
    }
    fs::write(path, content).map_err(|error| {
        ResourceError::file(
            path,
            format!("无法写入临时生成文件：{error}"),
            "确认工作区可写且磁盘空间充足",
        )
        .with_resource(resource)
    })?;
    let written = fs::read(path).map_err(|error| {
        ResourceError::file(
            path,
            format!("无法回读临时生成文件：{error}"),
            "检查磁盘或安全软件状态",
        )
        .with_resource(resource)
    })?;
    if content_hash(&written) != content_hash(content.as_bytes()) {
        return Err(ResourceError::file(
            path,
            "临时文件回读哈希不一致",
            "检查磁盘可靠性后重试；未替换正式文件",
        )
        .with_resource(resource));
    }
    Ok(())
}

pub(super) fn verify_expected_file(
    path: &Path,
    expected: &ExpectedFile,
) -> Result<(), ResourceError> {
    match expected {
        ExpectedFile::Absent => {
            if path.exists() {
                return Err(ResourceError::file(
                    path,
                    "目标在生成期间被其他进程创建，compare-and-swap 已拒绝覆盖",
                    "保留新文件并重新预览；确认 ownership 后再重试",
                ));
            }
        }
        ExpectedFile::ContentHash(expected_hash) => {
            let bytes = fs::read(path).map_err(|error| {
                ResourceError::file(
                    path,
                    format!("受管文件在生成期间丢失或不可读：{error}"),
                    "从版本控制恢复文件并重新生成",
                )
            })?;
            if content_hash(&bytes) != *expected_hash {
                return Err(ResourceError::file(
                    path,
                    "受管文件在生成期间发生变化，compare-and-swap 已拒绝覆盖",
                    "保留并检查并发改动，从新的预览重新生成",
                ));
            }
        }
        ExpectedFile::ExactBytes(expected_bytes) => {
            let bytes = fs::read(path).map_err(|error| {
                ResourceError::file(
                    path,
                    format!("ownership manifest 在生成期间丢失或不可读：{error}"),
                    "恢复原 manifest 并重新生成",
                )
            })?;
            if bytes != *expected_bytes {
                return Err(ResourceError::file(
                    path,
                    "ownership manifest 在生成期间发生变化，compare-and-swap 已拒绝覆盖",
                    "保留新的 manifest，从新的预览重新生成",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn move_to_backup(target: &Path, backup: &Path) -> Result<(), ResourceError> {
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ResourceError::file(
                parent,
                format!("无法创建备份目录：{error}"),
                "关闭占用文件的程序后重试",
            )
        })?;
    }
    fs::rename(target, backup).map_err(|error| {
        ResourceError::file(
            target,
            format!("无法备份旧生成文件：{error}"),
            "关闭编辑器或安全软件对该文件的占用后重试",
        )
    })
}

pub(super) fn install_staged(
    staged: &Path,
    target: &Path,
    resource: &str,
) -> Result<(), ResourceError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ResourceError::file(
                parent,
                format!("无法创建正式生成目录：{error}"),
                "确认工作区可写且目录未被占用",
            )
            .with_resource(resource)
        })?;
    }
    fs::hard_link(staged, target).map_err(|error| {
        ResourceError::file(
            target,
            format!("无法以不覆盖方式安装生成文件：{error}"),
            "保留并检查并发创建的目标；关闭占用文件的程序后从新预览重试",
        )
        .with_resource(resource)
    })
}

pub(super) fn rollback(
    installed: &[InstalledFile],
    backups: &[(PathBuf, PathBuf)],
) -> Result<(), ResourceError> {
    let mut failures = Vec::new();
    for installed_file in installed.iter().rev() {
        match fs::read(&installed_file.path) {
            Ok(bytes) if content_hash(&bytes) == installed_file.content_hash => {
                if let Err(error) = fs::remove_file(&installed_file.path) {
                    failures.push(format!(
                        "无法删除 {}：{error}",
                        installed_file.path.display()
                    ));
                }
            }
            Ok(_) => failures.push(format!(
                "拒绝删除已被并发修改的 {}",
                installed_file.path.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!(
                "无法检查 {}：{error}",
                installed_file.path.display()
            )),
        }
    }
    for (target, backup) in backups.iter().rev() {
        if let Some(parent) = target.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            failures.push(format!("无法创建 {}：{error}", parent.display()));
            continue;
        }
        if target.exists() {
            failures.push(format!(
                "拒绝覆盖回滚期间出现的 {}，旧文件仍在 {}",
                target.display(),
                backup.display()
            ));
            continue;
        }
        if let Err(error) = fs::hard_link(backup, target) {
            failures.push(format!(
                "无法将 {} 恢复到 {}：{error}",
                backup.display(),
                target.display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ResourceError::new(
            failures.join("；"),
            "按列出的目标与备份路径人工恢复，并保留现场用于排查",
        ))
    }
}
