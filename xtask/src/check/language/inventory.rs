use std::{collections::BTreeSet, fs, path::Path};

use sha2::{Digest, Sha256};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};

use super::{
    git::{self, Reply},
    model::FileRecord,
    policy::{self, Policy},
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Inventory {
    pub(crate) head: Option<String>,
    pub(crate) eligible: BTreeSet<String>,
    pub(crate) ignored: BTreeSet<String>,
    pub(crate) deleted: BTreeSet<String>,
}

pub(crate) fn discover(
    root: &Path,
    git: &mut impl FnMut(&[&str]) -> Result<Reply>,
) -> Result<Inventory> {
    let toplevel = String::from_utf8(git::success(git(&["rev-parse", "--show-toplevel"])?)?)?;
    if fs::canonicalize(toplevel.trim_end_matches(['\n', '\r']))? != fs::canonicalize(root)? {
        return Err("语言报告必须从完整 Git 工作树根目录统计，不能缩小到子目录".into());
    }
    let reply = git(git::HEAD)?;
    let head = match reply.code {
        0 => {
            let sha = String::from_utf8(reply.stdout)?.trim().to_owned();
            if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("语言报告 Git HEAD 不是完整提交 SHA".into());
            }
            Some(sha)
        }
        1 if reply.stdout.is_empty() && reply.stderr.is_empty() => None,
        _ => {
            git::success(reply)?;
            return Err("语言报告无法读取 HEAD".into());
        }
    };
    let candidates = git::paths(&git::success(git(git::FILES)?)?)?;
    let ignored = git::paths(&git::success(git(git::IGNORED)?)?)?;
    let deleted = git::paths(&git::success(git(git::DELETED)?)?)?;
    let eligible = candidates
        .difference(&ignored)
        .filter(|path| !deleted.contains(*path))
        .cloned()
        .collect();
    Ok(Inventory {
        head,
        eligible,
        ignored,
        deleted,
    })
}

pub(crate) fn collect(
    root: &Path,
    policy: &Policy,
    inventory: &Inventory,
) -> Result<Vec<FileRecord>> {
    inventory
        .eligible
        .iter()
        .map(|relative| {
            let bytes = read_file(root, relative)?;
            policy::classify(policy, relative, &bytes)
        })
        .collect()
}

pub(crate) fn verify(root: &Path, files: &[FileRecord]) -> Result<()> {
    for file in files {
        let bytes = read_file(root, &file.path)?;
        if policy::digest(&bytes) != file.sha256 {
            return Err(format!("统计期间文件变化，拒绝发布混合快照：{}", file.path).into());
        }
    }
    Ok(())
}

pub(crate) fn fingerprint(files: &[FileRecord]) -> String {
    let mut hash = Sha256::new();
    for file in files {
        hash.update((file.path.len() as u64).to_le_bytes());
        hash.update(file.path.as_bytes());
        hash.update(file.bytes.to_le_bytes());
        hash.update(file.sha256.as_bytes());
    }
    policy::format_digest(hash.finalize())
}

fn read_file(root: &Path, relative: &str) -> Result<Vec<u8>> {
    let path = relative
        .split('/')
        .fold(root.to_path_buf(), |path, component| path.join(component));
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("语言报告文件边界无效 {relative:?}：{error}"))?;
    fs::read(&path).map_err(|error| format!("语言报告无法读取 {relative:?}：{error}").into())
}
