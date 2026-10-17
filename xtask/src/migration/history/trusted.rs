use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::{
    git::head_file,
    lock::parse_lock,
    model::{
        CONTROL_BASELINE, LOCK_RELATIVE_PATH, LockDocument, LockedFile, TENANT_BASELINE,
        lock_identity, sha256_bytes,
    },
};

pub(super) fn verify_bootstrap_lock(
    root: &Path,
    entries: &[LockedFile],
    head_paths: &BTreeSet<String>,
    trusted_ref: &str,
) -> Vec<String> {
    if !root.join(".git").exists() {
        return Vec::new();
    }
    let by_path = entries
        .iter()
        .map(|entry| (entry.relative.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut errors = Vec::new();
    for relative in head_paths {
        let Ok((_storage, target)) = lock_identity(relative) else {
            continue;
        };
        if target != CONTROL_BASELINE && target != TENANT_BASELINE {
            errors.push(format!(
                "首次引入 migrations.lock.toml 时发现 HEAD 已有未冻结历史迁移：{relative}；只能从未提交的新追加迁移开始冻结"
            ));
            continue;
        }
        let Some(entry) = by_path.get(relative.as_str()) else {
            errors.push(format!(
                "首次引入 migrations.lock.toml 必须完整冻结 HEAD 基线源码：{relative}"
            ));
            continue;
        };
        match head_file(root, relative, trusted_ref) {
            Ok(Some(blob)) if sha256_bytes(&blob) == entry.expected_sha256 => {}
            Ok(_) => errors.push(format!(
                "首次 lock 中的基线 hash 与受信 HEAD 不一致：{relative}；不能同时修改历史源码和 lock"
            )),
            Err(error) => errors.push(error),
        }
    }
    errors
}

pub(super) fn verify_trusted_head_lock(
    root: &Path,
    document: &LockDocument,
    entries: &[LockedFile],
    head_paths: &BTreeSet<String>,
    trusted_ref: &str,
) -> Vec<String> {
    let head_content = match head_file(root, LOCK_RELATIVE_PATH, trusted_ref) {
        Ok(content) => content,
        Err(error) => return vec![error],
    };
    let Some(head_content) = head_content else {
        return verify_bootstrap_lock(root, entries, head_paths, trusted_ref);
    };
    let (head_document, head_entries, mut errors) =
        parse_lock(root, &format!("HEAD:{LOCK_RELATIVE_PATH}"), &head_content);
    let Some(head_document) = head_document else {
        errors.push("Git HEAD 中的迁移冻结清单无效，拒绝以工作树内容覆盖".to_owned());
        return errors;
    };
    if !errors.is_empty() {
        errors.push("Git HEAD 中的迁移冻结清单无效，拒绝以工作树内容覆盖".to_owned());
        return errors;
    }
    if document.frozen_at != head_document.frozen_at {
        errors.push("migrations.lock.toml 的 frozen_at 已进入 HEAD 后不得修改".to_owned());
    }
    if entries.len() < head_entries.len() {
        errors.push("migrations.lock.toml 不得删除 Git HEAD 中的冻结条目".to_owned());
        return errors;
    }
    for (index, head_entry) in head_entries.iter().enumerate() {
        let current = &entries[index];
        if lock_signature(current) != lock_signature(head_entry) {
            errors.push(format!(
                "migrations.lock.toml 的 HEAD 条目 #{} 被修改或重排：{}；冻结清单只能在末尾追加",
                index + 1,
                head_entry.relative
            ));
        }
        match head_file(root, &head_entry.relative, trusted_ref) {
            Ok(Some(blob)) if sha256_bytes(&blob) == head_entry.expected_sha256 => {}
            Ok(_) => errors.push(format!(
                "Git HEAD 中的冻结源码与受信 lock 不一致：{}",
                head_entry.relative
            )),
            Err(error) => errors.push(error),
        }
    }
    let trusted_targets = head_entries
        .iter()
        .map(|entry| (entry.storage.as_str(), entry.target.as_str()))
        .collect::<BTreeSet<_>>();
    for entry in &entries[head_entries.len()..] {
        if head_paths.contains(&entry.relative) {
            errors.push(format!(
                "拒绝给已存在于 HEAD 的历史迁移补录 lock：{}；只能冻结当前工作树中新建且尚未提交的迁移",
                entry.relative
            ));
        }
        if trusted_targets.contains(&(entry.storage.as_str(), entry.target.as_str())) {
            errors.push(format!(
                "新增 lock 条目复用了受信迁移目标 {}/{}：{}；已冻结迁移不得增加源码文件",
                entry.storage, entry.target, entry.relative
            ));
        }
    }
    errors
}

fn lock_signature(entry: &LockedFile) -> (&str, &str, &str, &str) {
    (
        &entry.relative,
        &entry.expected_sha256,
        &entry.storage,
        &entry.target,
    )
}
