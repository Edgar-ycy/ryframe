use std::{fs, path::PathBuf};

use crate::Result;

pub(super) fn reject_contract_recovery_artifacts(paths: &[PathBuf]) -> Result<()> {
    let mut artifacts = Vec::new();
    let mut parents = Vec::new();
    for path in paths {
        let parent = path
            .parent()
            .ok_or_else(|| format!("契约文件没有父目录：{}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("契约文件名不是 UTF-8：{}", path.display()))?;
        let prefix = format!(".{name}.xtask-");
        parents.push(parent.to_path_buf());
        let entries = match fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let candidate = entry?.path();
            if candidate
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    value
                        .strip_prefix(&prefix)
                        .is_some_and(is_contract_file_artifact)
                })
            {
                artifacts.push(candidate);
            }
        }
    }
    parents.sort();
    parents.dedup();
    for parent in parents {
        let entries = match fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let candidate = entry?.path();
            if candidate
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|value| value.strip_prefix(".xtask-contract-transaction-"))
                .is_some_and(is_contract_identity)
            {
                artifacts.push(candidate);
            }
        }
    }
    artifacts.sort();
    artifacts.dedup();
    if artifacts.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "检测到上次契约事务未完整结束，已拒绝继续写入：{}；请根据 backup 恢复或确认目标已完整写入后再清理这些精确文件",
            artifacts
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("；")
        )
        .into())
    }
}

fn is_contract_file_artifact(value: &str) -> bool {
    let mut parts = value.split('-');
    matches!(parts.next(), Some("new" | "backup"))
        && parts.next().is_some_and(is_contract_number)
        && parts.next().is_some_and(is_contract_number)
        && parts.all(is_contract_number)
}

fn is_contract_identity(value: &str) -> bool {
    let mut parts = value.split('-');
    parts.next().is_some_and(is_contract_number)
        && parts.next().is_some_and(is_contract_number)
        && parts.next().is_none()
}

fn is_contract_number(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}
