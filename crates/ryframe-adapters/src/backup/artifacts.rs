use ryframe_application::ports::backup::{BackupArtifactVerifier, BackupManifest};
use ryframe_kernel::{AppError, AppResult};
use std::{
    fs::Metadata,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub fn artifact_verifier(root: PathBuf) -> Arc<dyn BackupArtifactVerifier> {
    Arc::new(BackupFiles { root })
}

struct BackupFiles {
    root: PathBuf,
}

#[async_trait::async_trait]
impl BackupArtifactVerifier for BackupFiles {
    async fn artifacts(&self, manifest: &BackupManifest) -> AppResult<()> {
        reject_link_or_reparse(&self.root, &self.root).await?;
        let root = tokio::fs::canonicalize(&self.root)
            .await
            .map_err(|_| AppError::Validation("备份文件根目录不存在或不可读".into()))?;
        for artifact in &manifest.artifacts {
            let relative = Path::new(&artifact.relative_path);
            if relative.as_os_str().is_empty()
                || relative.is_absolute()
                || artifact.relative_path.contains(['\\', ':'])
                || artifact.relative_path.chars().any(char::is_control)
                || relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(AppError::Validation(
                    "备份文件必须使用不可越界的相对路径".into(),
                ));
            }
            let declared = self.root.join(relative);
            reject_link_or_reparse(&self.root, &declared).await?;
            let path = tokio::fs::canonicalize(&declared)
                .await
                .map_err(|_| AppError::Validation("备份清单中的文件缺失或不可读".into()))?;
            if !path.starts_with(&root) || path == root {
                return Err(AppError::Validation(
                    "备份文件或链接越过已登记根目录".into(),
                ));
            }
            let digest = crate::storage::file_digest(&path)
                .await
                .map_err(|_| AppError::Validation("备份文件无法完成流式校验".into()))?;
            if digest.bytes != artifact.bytes || digest.sha256 != artifact.sha256 {
                return Err(AppError::Validation(
                    "备份文件大小或 SHA-256 校验失败".into(),
                ));
            }
            reject_link_or_reparse(&self.root, &declared).await?;
            if tokio::fs::canonicalize(&declared).await.ok().as_ref() != Some(&path) {
                return Err(AppError::Validation(
                    "备份文件在流式校验期间发生替换".into(),
                ));
            }
        }
        Ok(())
    }
}

async fn reject_link_or_reparse(root: &Path, path: &Path) -> AppResult<()> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        let metadata = tokio::fs::symlink_metadata(candidate)
            .await
            .map_err(|_| AppError::Validation("备份文件路径缺失或不可读".into()))?;
        if is_link_or_reparse(&metadata) {
            return Err(AppError::Validation(
                "备份文件路径不能经过链接或重解析点".into(),
            ));
        }
        if candidate == root {
            return Ok(());
        }
        current = candidate.parent();
    }
    Err(AppError::Validation("备份文件路径越过已登记根目录".into()))
}

fn is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
