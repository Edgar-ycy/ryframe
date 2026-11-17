use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};

use super::model::{
    BaselineContract, DevexRunOptions, PairedArm, SaveConfigBinding, SaveConfigDirs, SaveConfigFile,
};

const COMMON_APP: &str = "app.toml";
const DEV_APP: &str = "app.dev.toml";
const CONFIG_FILES: [&str; 6] = [
    COMMON_APP,
    DEV_APP,
    "app.test.toml",
    "app.prod.toml",
    "tools.toml",
    "feature-matrix.json",
];

impl SaveConfigDirs {
    pub(super) fn select(
        run: &DevexRunOptions,
        contract: Option<BaselineContract>,
        baseline: Option<PathBuf>,
        candidate: Option<PathBuf>,
    ) -> std::result::Result<Option<Self>, String> {
        let required =
            contract.is_some_and(|item| item.requires_save_configuration(run.suite, &run.variant));
        match (required, baseline, candidate) {
            (true, Some(baseline), Some(candidate)) => {
                if !baseline.is_absolute() || !candidate.is_absolute() {
                    return Err("paired 的两个保存测量配置目录必须是绝对路径".into());
                }
                Ok(Some(Self {
                    baseline,
                    candidate,
                }))
            }
            (true, ..) => {
                Err("该保存场景要求成对提供 --base-config-dir 与 --candidate-config-dir".into())
            }
            (false, None, None) => Ok(None),
            (false, ..) => Err("配对配置目录仅用于共同工具链的非 cancellation 保存场景".into()),
        }
    }

    pub(super) fn bind(
        &self,
        source_roots: [&Path; 4],
    ) -> Result<(SaveConfigBinding, SaveConfigBinding)> {
        let baseline = canonical_directory(&self.baseline)?;
        let candidate = canonical_directory(&self.candidate)?;
        if overlaps(&baseline, &candidate)
            || source_roots
                .iter()
                .any(|root| overlaps(&baseline, root) || overlaps(&candidate, root))
        {
            return Err("两侧保存配置目录不得相互重叠或与被测源码根重叠".into());
        }
        let baseline = SaveConfigBinding::capture(baseline)?;
        if !baseline.files.contains_key(COMMON_APP) {
            return Err("B0 保存配置缺少必需的普通文件 app.toml".into());
        }
        Ok((baseline, SaveConfigBinding::capture(candidate)?))
    }
}

impl SaveConfigBinding {
    fn capture(directory: PathBuf) -> Result<Self> {
        let mut files = BTreeMap::new();
        for name in CONFIG_FILES {
            if let Some(file) = read_file(&directory.join(name))? {
                files.insert(name.to_owned(), file);
            }
        }
        if !files.contains_key(DEV_APP) {
            return Err("保存配置缺少必需的普通文件 app.dev.toml".into());
        }
        if canonical_directory(&directory)? != directory {
            return Err("保存配置目录在读取期间发生漂移".into());
        }
        Ok(Self { directory, files })
    }

    pub(super) fn verify(&self) -> Result<()> {
        let observed = Self::capture(canonical_directory(&self.directory)?)?;
        if observed != *self {
            return Err("保存测量配置集合或文件字节发生变化，拒绝混入样本".into());
        }
        Ok(())
    }

    pub(super) fn valid_report_shape(&self, arm: PairedArm) -> bool {
        self.directory
            .to_str()
            .is_some_and(|path| !path.is_empty() && !path.chars().any(char::is_control))
            && self.files.contains_key(DEV_APP)
            && (arm != PairedArm::Baseline || self.files.contains_key(COMMON_APP))
            && self.files.iter().all(|(name, file)| {
                CONFIG_FILES.contains(&name.as_str())
                    && super::runtime_native::configuration::digest(&file.sha256)
            })
    }

    pub(super) fn conflicts_with(&self, other: &Self) -> bool {
        overlaps(&self.directory, &other.directory)
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf> {
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)?;
    Ok(path.canonicalize()?)
}

fn read_file(path: &Path) -> Result<Option<SaveConfigFile>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    let contents = fs::read(path)?;
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    Ok(Some(SaveConfigFile {
        bytes: contents.len(),
        sha256: hex::encode(Sha256::digest(&contents)),
    }))
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
#[path = "../../tests/internal/devex_save_config.rs"]
mod tests;
