//! 只接纳早期正式记录的双仓来源，原收据始终只读。
use super::files;
use crate::{
    Result,
    cli::FullStackConfigOptions,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{resources::common, target_verify::guard::Inputs},
    release::archive::verify_source_pair_receipt,
};
use std::{collections::BTreeSet, ffi::OsString, fs, path::Path};

pub(super) struct Source {
    protected: Inputs,
    directory_identity: (u64, u64),
    pair: files::Image,
}

impl Source {
    pub(super) fn capture(options: &FullStackConfigOptions, common: &Path) -> Result<Self> {
        for root in [
            &options.runtime_dir,
            &options.source_backend,
            &options.source_frontend,
        ] {
            validate_absolute_path(root, LocalTestPathKind::ExistingDirectory)?;
        }
        exact_directory(&options.runtime_dir)?;
        let metadata = fs::symlink_metadata(&options.runtime_dir)?;
        let path = options.runtime_dir.join("source-pair.json");
        let pair = files::read_required(&path)?;
        let protected = Inputs::open([path, common.to_owned(), options.template.clone()])?;
        let source = Self {
            protected,
            pair,
            directory_identity: common::device_inode_for(&options.runtime_dir, &metadata)?,
        };
        source.check(options)?;
        Ok(source)
    }

    pub(super) fn check(&self, options: &FullStackConfigOptions) -> Result<()> {
        self.protected.check()?;
        for root in [&options.source_backend, &options.source_frontend] {
            validate_absolute_path(root, LocalTestPathKind::ExistingDirectory)?;
        }
        exact_directory(&options.runtime_dir)?;
        let metadata = fs::symlink_metadata(&options.runtime_dir)?;
        if common::device_inode_for(&options.runtime_dir, &metadata)? != self.directory_identity {
            return Err("运行目录身份变化；保留来源现场，禁止继续发布".into());
        }
        let path = options.runtime_dir.join("source-pair.json");
        if files::read_required(&path)? != self.pair {
            return Err("原 source-pair.json 字节或身份变化；禁止重写或继续发布".into());
        }
        let run = options
            .run_id
            .parse::<u64>()
            .ok()
            .filter(|v| *v > 0)
            .ok_or("run-id必须为正u64")?;
        let attempt = options
            .attempt
            .parse::<u64>()
            .ok()
            .filter(|v| *v > 0)
            .ok_or("attempt必须为正u64")?;
        let pair = verify_source_pair_receipt(
            &path,
            &options.source_backend,
            &options.source_frontend,
            run,
            attempt,
        )
        .map_err(
            |_| "原全栈来源与当前干净双仓或运行身份不匹配；核对原检出目录和Job身份，不改写旧证明",
        )?;
        if pair["backend_sha"].as_str() != Some(options.source_sha.as_str()) {
            return Err("source-sha与原source-pair后端提交不符；核对精确来源".into());
        }
        self.protected.check()?;
        exact_directory(&options.runtime_dir)?;
        let metadata = fs::symlink_metadata(&options.runtime_dir)?;
        if common::device_inode_for(&options.runtime_dir, &metadata)? != self.directory_identity {
            return Err("完整来源复核期间运行目录身份变化；保留现场".into());
        }
        if files::read_required(&path)? != self.pair {
            return Err("完整来源复核期间原收据变化；保留现场".into());
        }
        Ok(())
    }
}

fn exact_directory(path: &Path) -> Result<()> {
    files::require_directory(path)?;
    let entries = fs::read_dir(path)?
        .map(|item| Ok(item?.file_name()))
        .collect::<Result<BTreeSet<_>>>()?;
    if entries != BTreeSet::from([OsString::from("source-pair.json")]) {
        return Err(
            "运行目录只能包含早期记录的source-pair.json；未知产物、控制或链接必须保留并核对".into(),
        );
    }
    crate::local_test_path::validate_absolute_path(
        &path.join("source-pair.json"),
        LocalTestPathKind::ExistingFile,
    )?;
    Ok(())
}
