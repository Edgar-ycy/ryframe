//! 已登记 post-copy 的唯一 target/API 启动分支；目标权限由同进程 Generation 持有。
use super::super::{
    bridge::scope::Scope,
    json as document,
    lifecycle::{
        lock::ProcessGuard,
        manifest,
        stage::{self, StageRequest, StageRunner},
    },
    resources::common,
    target_verify,
};
use crate::{
    Result,
    cli::{CloneRole, CloneRuntimeOperation, CloneRuntimeOptions, CloneSide},
};
use serde_json::Value;
use std::{cell::RefCell, path::Path};

mod control;
mod diagnostic;
pub(crate) use diagnostic::diagnostic;
mod guard;
mod history;
mod input;
pub(crate) mod locks;
mod observation;
pub(crate) mod registration;

pub(crate) fn selected(backend: &Path, options: &CloneRuntimeOptions) -> Result<bool> {
    if options.side != CloneSide::Target {
        return Ok(false);
    }
    common::local_directory(backend, &options.run_dir, "API交接运行目录")?;
    let post = options.run_dir.join("post-copy.json");
    match std::fs::symlink_metadata(&post) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
        Ok(_) => {
            common::common_file(backend, &post, "API交接登记")?;
            Ok(true)
        }
    }
}

pub(crate) fn run(backend: &Path, options: &CloneRuntimeOptions) -> Result<Value> {
    if !selected(backend, options)? {
        return Err("原生API交接只接受当前已登记post-copy目标控制".into());
    }
    if options.operation != CloneRuntimeOperation::Start {
        return stage::run_named(
            backend,
            &options.run_dir,
            "runtime-target",
            options.operation.as_str(),
            &observation::Runner { options },
        );
    }
    stage::run_named(
        backend,
        &options.run_dir,
        "runtime-target",
        "start",
        &Runner {
            options,
            scope: RefCell::new(None),
        },
    )
}

struct Runner<'a> {
    options: &'a CloneRuntimeOptions,
    scope: RefCell<Option<Scope>>,
}

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        self.require(request, false)?;
        guard::absent(&request.run_dir.join("seed-runtime.json"))?;
        if self.options.roles != [CloneRole::Api] {
            return Err("复制后交接只允许API，Worker必须保持停止".into());
        }
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        let mut scope = self.scope.borrow_mut();
        if let Some(scope) = scope.as_ref() {
            scope.assert_unchanged()?;
        } else {
            *scope = Some(Scope::load(
                request.backend,
                document::array(&manifest.value["build_bridges"], "API交接构建桥接")?,
                &super::runner::inherited_bridges(request.backend, &manifest.value)?,
            )?);
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        diagnostic::context(
            request.run_dir,
            request.attempt,
            diagnostic::Phase::Binding,
            self.execute_owned(request),
        )
    }
}

impl Runner<'_> {
    fn execute_owned(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.require(request, true)?;
        guard::absent(&request.run_dir.join("seed-runtime.json"))?;
        let scope = self.scope.borrow();
        let scope = scope.as_ref().ok_or("API交接未建立来源核验窗口")?;
        scope.assert_unchanged()?;
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        let storage = locks::Storage::acquire(request.backend, request.run_dir, &manifest.value)?;
        let result = (|| {
            let filename = common::bound_file(
                request.backend,
                &manifest.value["initialized"],
                "API交接初始化",
            )?;
            let target = filename.parent().ok_or("API交接初始化目录缺失")?;
            let copy = common::local_directory(
                request.backend,
                Path::new(document::text(
                    &manifest.value["copy_directory"],
                    "复制目录",
                )?),
                "复制目录",
            )?;
            let ledger = copy.join("ledger");
            guard::absent(&ledger.join("clone.lock"))?;
            let owner = target_verify::guard::Generation::acquire(request.backend, target)?;
            let result = (|| {
                let ledger_guard = ProcessGuard::acquire_named(&ledger, "copy-ledger.guard")?;
                guard::absent(&ledger.join("clone.lock"))?;
                let frame = diagnostic::context(
                    request.run_dir,
                    request.attempt,
                    diagnostic::Phase::Binding,
                    input::Frame::load(
                        request.backend,
                        request.run_dir,
                        request.attempt.ok_or("API交接阶段序号缺失")?,
                        &owner,
                        scope,
                    ),
                )?;
                diagnostic::context(
                    request.run_dir,
                    request.attempt,
                    diagnostic::Phase::Binding,
                    storage.check(),
                )?;
                let result = control::start(&frame, &owner, scope, &storage.directory);
                let checked = (|| {
                    storage.check()?;
                    ledger_guard.check()?;
                    scope.assert_unchanged()
                })();
                locks::combine(result, checked)
            })();
            diagnostic::context(
                request.run_dir,
                request.attempt,
                diagnostic::Phase::Cleanup,
                owner.finish(result),
            )
        })();
        diagnostic::context(
            request.run_dir,
            request.attempt,
            diagnostic::Phase::Cleanup,
            storage.finish(locks::combine(result, scope.assert_unchanged())),
        )
    }

    fn require(&self, request: &StageRequest<'_>, executing: bool) -> Result<()> {
        if request.stage != "runtime-target"
            || request.mode != "start"
            || request.run_dir != self.options.run_dir
            || self.options.side != CloneSide::Target
            || self.options.operation != CloneRuntimeOperation::Start
            || executing != request.attempt.is_some()
            || request.attempt == Some(0)
        {
            return Err("API交接必须绑定当前正式target/start控制器".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/cache/handoff.rs"]
mod tests;
