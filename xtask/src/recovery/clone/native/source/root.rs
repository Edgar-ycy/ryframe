//! 单次来源观察的具体根锚点，不在调用或请求之间共享。
use crate::{Result, recovery::runtime::dataset_authority::reads};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Root {
    original: PathBuf,
    absolute: PathBuf,
    canonical: PathBuf,
}

impl Root {
    pub(super) fn open(original: &Path) -> Result<Self> {
        let absolute = std::path::absolute(original)?;
        let canonical = observe(&absolute)?;
        Ok(Self {
            original: original.to_path_buf(),
            absolute,
            canonical,
        })
    }

    pub(super) fn original(&self) -> &Path {
        &self.original
    }

    pub(super) fn path(&self) -> &Path {
        &self.absolute
    }

    pub(super) fn canonical(&self) -> &Path {
        &self.canonical
    }

    fn verify(&self) -> Result<()> {
        if std::path::absolute(&self.original)? != self.absolute {
            return Err("来源根相对路径不再定位原绝对目录".into());
        }
        if observe(&self.absolute)? != self.canonical {
            return Err("来源根规范路径变化".into());
        }
        Ok(())
    }

    pub(super) fn finish<T>(&self, body: Result<T>) -> Result<T> {
        match (body, self.verify()) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(after)) => Err(format!("{error}；来源根后验同时失败：{after}").into()),
        }
    }
}

fn observe(path: &Path) -> Result<PathBuf> {
    if !fs::metadata(path)?.is_dir() {
        return Err("来源根必须是目录".into());
    }
    reads::directory(path)?;
    Ok(fs::canonicalize(path)?)
}
