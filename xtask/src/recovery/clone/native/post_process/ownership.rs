//! 复制后阶段和 seed 登记独占同一目标代次、存储控制和已收尾复制账本。

use std::path::Path;

use serde_json::Value;

use super::files;
use crate::{
    Result,
    recovery::clone::native::{
        cache::handoff::locks::Storage,
        json as document,
        lifecycle::{lock::ProcessGuard, stage::StageRequest},
        resources::common,
        target_verify::guard::Generation,
    },
};

pub(in crate::recovery::clone::native) fn with_target<T>(
    request: &StageRequest<'_>,
    manifest: &Value,
    storage: Storage,
    execute: impl FnOnce(&Generation, &Path) -> Result<T>,
) -> Result<T> {
    let backend = request.backend;
    let result = (|| {
        let initialized = common::bound_file(backend, &manifest["initialized"], "初始化后像")?;
        let target = initialized.parent().ok_or("初始化目录缺失")?;
        let copy = common::local_directory(
            backend,
            Path::new(document::text(&manifest["copy_directory"], "复制目录")?),
            "复制目录",
        )?;
        let ledger = copy.join("ledger");
        let lock = ledger.join("clone.lock");
        files::absent(&lock)?;
        let owner = Generation::acquire(backend, target)?;
        let result = (|| {
            let guard = ProcessGuard::acquire_named(&ledger, "copy-ledger.guard")?;
            files::absent(&lock)?;
            let result = execute(&owner, &storage.directory);
            let checked = (|| {
                files::absent(&lock)?;
                guard.check()?;
                storage.check()
            })();
            files::combine(result, checked)
        })();
        files::combine(result, owner.finish(Ok(())))
    })();
    files::combine(result, storage.finish(Ok(())))
}
