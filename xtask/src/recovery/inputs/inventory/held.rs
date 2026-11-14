//! 持有正式备份的原目录及产物；只有 Windows 原生共享模式允许复用期间的字节证据。
use std::path::Path;
#[cfg(windows)]
use std::{collections::BTreeSet, path::PathBuf};

use serde_json::Value;

#[cfg(windows)]
use super::{verify_artifacts, visit_artifacts};
use crate::Result;
#[cfg(windows)]
use crate::recovery::clone::native::target_verify::guard::BackupInputs;

pub(in crate::recovery) struct HeldArtifacts {
    #[cfg(windows)]
    root: PathBuf,
    #[cfg(windows)]
    manifest: Value,
    #[cfg(windows)]
    inputs: BackupInputs,
    #[cfg(not(windows))]
    impossible: std::convert::Infallible,
}

impl HeldArtifacts {
    #[cfg(windows)]
    pub(in crate::recovery) fn open(root: &Path, manifest: &Value) -> Result<Self> {
        let mut directories = BTreeSet::new();
        let mut files = Vec::new();
        visit_artifacts(root, manifest, |path, _, _| {
            for directory in path.ancestors().skip(1).take_while(|path| *path != root) {
                directories.insert(directory.to_path_buf());
            }
            files.push(path);
            Ok(())
        })?;
        // 父目录先于文件持有；持有后以原清单和普通文件规则复验全部原字节。
        let inputs = BackupInputs::open(root, directories, files)?;
        let held = Self {
            root: root.to_path_buf(),
            manifest: manifest.clone(),
            inputs,
        };
        held.check_end()?;
        Ok(held)
    }

    #[cfg(windows)]
    pub(in crate::recovery) fn verify(&self, root: &Path, manifest: &Value) -> Result<()> {
        if root != self.root || manifest != &self.manifest {
            return Err("恢复检查点没有绑定同一正式备份根与完整清单".into());
        }
        self.inputs.checkpoint()
    }

    #[cfg(not(windows))]
    pub(in crate::recovery) fn verify(&self, _root: &Path, _manifest: &Value) -> Result<()> {
        match self.impossible {}
    }

    #[cfg(windows)]
    pub(in crate::recovery) fn check_end(&self) -> Result<()> {
        self.inputs.check_identities()?;
        let bytes = verify_artifacts(&self.root, &self.manifest);
        self.inputs.check_identities()?;
        bytes
    }

    #[cfg(not(windows))]
    pub(in crate::recovery) fn check_end(&self) -> Result<()> {
        match self.impossible {}
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use serde_json::{Value, json};

    use super::super::verify_artifacts;
    #[cfg(windows)]
    use super::HeldArtifacts;
    use crate::recovery::inputs::domain::{BUCKETS, file_sha256};

    fn temporary() -> tempfile::TempDir {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }

    fn fixture(root: &Path) -> Value {
        let mut artifacts = Vec::new();
        for bucket in BUCKETS {
            let relative = format!("objects/{bucket}/index.json");
            let path = root.join(&relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"data").unwrap();
            artifacts.push(json!({
                "relative_path": relative,
                "resource": format!("objects:{bucket}"),
                "bytes": 4,
                "sha256": file_sha256(&path).unwrap(),
            }));
        }
        json!({"databases": [], "artifacts": artifacts})
    }

    #[test]
    fn ordinary_verification_requires_every_resource_and_exact_original_bytes() {
        let root = temporary();
        let manifest = fixture(root.path());
        verify_artifacts(root.path(), &manifest).unwrap();
        let mut missing = manifest.clone();
        missing["artifacts"].as_array_mut().unwrap().pop();
        assert!(verify_artifacts(root.path(), &missing).is_err());
        fs::write(root.path().join("objects/uploads/index.json"), b"drft").unwrap();
        assert!(verify_artifacts(root.path(), &manifest).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn held_backup_denies_existing_writers_and_new_writes_and_path_replacement() {
        let temporary = temporary();
        let root = temporary.path().join("backup");
        let manifest = fixture(&root);
        let path = root.join("objects/uploads/index.json");
        let writer = fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(HeldArtifacts::open(&root, &manifest).is_err());
        drop(writer);

        let mut held = HeldArtifacts::open(&root, &manifest).unwrap();
        let mut different = manifest.clone();
        different["id"] = json!("other");
        assert!(held.verify(&root, &different).is_err());
        assert!(held.verify(temporary.path(), &manifest).is_err());
        assert!(fs::write(&path, b"drft").is_err());
        assert!(fs::rename(&path, path.with_extension("old")).is_err());
        let parent = path.parent().unwrap();
        assert!(fs::rename(parent, parent.with_extension("old")).is_err());
        assert!(fs::rename(&root, temporary.path().join("moved")).is_err());
        held.verify(&root, &manifest).unwrap();
        held.check_end().unwrap();
        held.manifest["artifacts"][0]["sha256"] = json!("0".repeat(64));
        let error = held.check_end().unwrap_err().to_string();
        assert!(error.contains("大小或摘要被篡改"), "{error}");
        assert!(fs::write(&path, b"drft").is_err());
        drop(held);
        fs::write(&path, b"drft").unwrap();
        assert!(verify_artifacts(&root, &manifest).is_err());
        assert!(HeldArtifacts::open(&root, &manifest).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn held_backup_rejects_link_count_drift_when_the_volume_allows_it() {
        let root = temporary();
        let manifest = fixture(root.path());
        let path = root.path().join("objects/uploads/index.json");
        let alias = root.path().join("alias.json");
        fs::hard_link(&path, &alias).unwrap();
        let held = HeldArtifacts::open(root.path(), &manifest).unwrap();
        held.verify(root.path(), &manifest).unwrap();

        if fs::hard_link(&path, root.path().join("new-alias.json")).is_ok() {
            assert!(held.verify(root.path(), &manifest).is_err());
            assert!(held.check_end().is_err());
        } else {
            held.verify(root.path(), &manifest).unwrap();
            held.check_end().unwrap();
        }
    }
}
