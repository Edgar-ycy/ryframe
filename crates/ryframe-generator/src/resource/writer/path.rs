use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::{AssetRoot, ResourceError, ResourceWorkspace};

pub(super) fn target_path(
    workspace: ResourceWorkspace<'_>,
    root: AssetRoot,
    relative: &str,
) -> Result<PathBuf, ResourceError> {
    validate_managed_path(root, relative)?;
    let base = match root {
        AssetRoot::Backend => workspace.backend_root,
        AssetRoot::Frontend => workspace.frontend_root.ok_or_else(|| {
            ResourceError::new(
                "需要写入前端资产，但没有提供前端工作区",
                "设置 ResourceWorkspace.frontend_root",
            )
            .with_file(relative)
        })?,
    };
    ensure_no_symlink_escape(base, relative)?;
    Ok(base.join(relative))
}

pub(super) fn validate_managed_path(root: AssetRoot, relative: &str) -> Result<(), ResourceError> {
    let path = Path::new(relative);
    if relative.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ResourceError::new(
            "生成路径不是安全的正斜杠相对路径",
            "移除盘符、反斜杠、空片段、`.` 与 `..`",
        )
        .with_file(relative));
    }
    let allowed = match root {
        AssetRoot::Backend => {
            relative == "catalog/access.generated.toml"
                || [
                    "crates/ryframe-application/src/generated/",
                    "crates/ryframe-db/src/generated/",
                    "crates/ryframe-api/src/generated/",
                    "crates/ryframe-tenant-db/src/generated/",
                ]
                .iter()
                .any(|prefix| relative.starts_with(prefix))
        }
        AssetRoot::Frontend => relative.starts_with("src/generated/resources/"),
    };
    if !allowed {
        return Err(ResourceError::new(
            "生成路径超出 ownership 白名单",
            "后端只写各 crate 的 src/generated 与 access.generated.toml；前端只写 src/generated/resources",
        )
        .with_file(relative));
    }
    Ok(())
}

fn ensure_no_symlink_escape(root: &Path, relative: &str) -> Result<(), ResourceError> {
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        ResourceError::file(
            root,
            format!("无法解析工作区根目录：{error}"),
            "确认路径存在且不是失效链接",
        )
    })?;
    let target = root.join(relative);
    let mut ancestor = target.parent().unwrap_or(root);
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| {
            ResourceError::file(
                &target,
                "生成路径没有有效父目录",
                "检查工作区根目录和生成路径",
            )
        })?;
    }
    let canonical_ancestor = fs::canonicalize(ancestor).map_err(|error| {
        ResourceError::file(
            ancestor,
            format!("无法解析生成目录：{error}"),
            "移除失效符号链接后重试",
        )
    })?;
    if !canonical_ancestor.starts_with(canonical_root) {
        return Err(ResourceError::file(
            &target,
            "生成路径通过链接逃逸出工作区",
            "移除生成目录中的外部符号链接",
        ));
    }
    Ok(())
}

pub(super) fn display_path(root: AssetRoot, path: &str) -> String {
    format!("{}:{path}", root.label())
}

pub(super) fn extract_source_hash(content: &str) -> String {
    if let Some((_, rest)) = content.split_once("source-sha256: ") {
        return rest
            .split(|character: char| character.is_whitespace() || character == '|')
            .next()
            .unwrap_or_default()
            .to_owned();
    }
    content_hash(content.as_bytes())
}

pub(super) fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
