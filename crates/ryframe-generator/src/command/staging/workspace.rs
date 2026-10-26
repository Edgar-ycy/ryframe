use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use super::{
    manifest::normalize_external_paths,
    snapshot::{FileChange, SourceSnapshot, safe_path},
};

pub(super) struct CandidateWorkspace {
    pub(super) source: SourceSnapshot,
    pub(super) baseline: SourceSnapshot,
    pub(super) path: PathBuf,
    external: Vec<SourceSnapshot>,
    directory: tempfile::TempDir,
}

impl CandidateWorkspace {
    pub(super) fn create(root: &Path, cargo: bool) -> Result<Self, String> {
        let source = SourceSnapshot::capture(root)?;
        let cache = safe_path(root, Path::new(".local-tests"))?;
        fs::create_dir_all(&cache).map_err(|error| format!("无法创建候选缓存：{error}"))?;
        let directory = tempfile::Builder::new()
            .prefix("business-generation-")
            .tempdir_in(&cache)
            .map_err(|error| format!("无法创建隔离候选工作区：{error}"))?;
        let path = directory.path().join("workspace");
        source.copy_to(&path)?;
        let external = if cargo {
            normalize_external_paths(&source, &path)?
        } else {
            Vec::new()
        };
        let baseline = SourceSnapshot::capture(&path)?;
        source.verify()?;
        Ok(Self {
            source,
            baseline,
            path,
            external,
            directory,
        })
    }

    pub(super) fn changes(&self, allowed: &BTreeSet<PathBuf>) -> Result<Vec<FileChange>, String> {
        let changes = self.baseline.changes(&self.path)?;
        for change in &changes {
            if !allowed.contains(&change.relative) {
                return Err(format!(
                    "候选生成修改了未登记目标：{}；本批未安装，检查生成器与构建脚本",
                    change.relative.display()
                ));
            }
        }
        Ok(changes)
    }

    pub(super) fn verify_external(&self) -> Result<(), String> {
        for source in &self.external {
            source.verify()?;
        }
        Ok(())
    }

    pub(super) fn close(self) -> Result<(), String> {
        let path = self.directory.path().to_path_buf();
        self.directory.close().map_err(|error| {
            format!(
                "候选工作区 {} 清理失败：{error}；只清理该登记缓存，不重复生成写入",
                path.display()
            )
        })
    }

    pub(super) fn retain(self, error: String) -> String {
        let path = self.directory.keep();
        format!(
            "{error}；候选现场保留于 {}；正式文件未安装，先核对候选及其恢复目录，不自动重放或删除",
            path.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_candidate_preserves_nested_recovery_and_success_closes_cache() {
        let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests");
        fs::create_dir_all(&cache).unwrap();
        let root = tempfile::Builder::new()
            .prefix("candidate-recovery-")
            .tempdir_in(cache)
            .unwrap();
        fs::write(root.path().join("model.rs"), b"original").unwrap();
        let candidate = CandidateWorkspace::create(root.path(), false).unwrap();
        let candidate_root = candidate.directory.path().to_path_buf();
        let backup = candidate
            .path
            .join(".ryframe-stage-failed/.backup/model.rs");
        fs::create_dir_all(backup.parent().unwrap()).unwrap();
        fs::write(&backup, b"before-image").unwrap();
        let error = candidate.retain("安装失败，回滚未完成".into());
        assert!(error.contains(candidate_root.to_str().unwrap()));
        assert_eq!(fs::read(&backup).unwrap(), b"before-image");
        assert_eq!(fs::read(root.path().join("model.rs")).unwrap(), b"original");

        let successful = CandidateWorkspace::create(root.path(), false).unwrap();
        let successful_root = successful.directory.path().to_path_buf();
        successful.close().unwrap();
        assert!(!successful_root.exists());
        assert_eq!(fs::read(&backup).unwrap(), b"before-image");
    }
}
