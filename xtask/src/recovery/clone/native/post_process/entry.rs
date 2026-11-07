//! 固定请求三模式在统一阶段内持有真实存储、目标代次和完整复制账本保护。
use super::{
    context::Context,
    diagnostic::{self, Phase},
};
use crate::{
    Result,
    cli::{PostCopyOperation, PostCopyOptions},
    recovery::clone::native::{
        bridge::scope::Scope,
        cache, json as document,
        lifecycle::{
            lock::ProcessGuard,
            manifest,
            stage::{self, StageRequest, StageRunner},
        },
        resources::common,
        target_verify::guard::Generation,
    },
};
use serde_json::{Value, json};
use std::{cell::RefCell, path::Path};

pub(crate) fn selected(options: &PostCopyOptions) -> bool {
    matches!(
        options.operation,
        PostCopyOperation::Register | PostCopyOperation::Prepare | PostCopyOperation::Amend
    )
}

pub(crate) fn run(backend: &Path, options: &PostCopyOptions) -> Result<Value> {
    if !selected(options) {
        return Err("此原生入口仅处理登记、准备和环境修订".into());
    }
    let value = stage::run_named(
        backend,
        &options.run_dir,
        "post-copy",
        options.operation.as_str(),
        &Runner {
            options,
            scope: RefCell::new(None),
            input: RefCell::new(None),
        },
    )?;
    Ok(
        json!({"status":value["status"],"stage":value["stage"],"mode":value["mode"],
        "attempt":value["attempt"],"restore_qualified":value["restore_qualified"]}),
    )
}

struct Runner<'a> {
    options: &'a PostCopyOptions,
    scope: RefCell<Option<Scope>>,
    input: RefCell<Option<Value>>,
}

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.stage != "post-copy"
            || request.mode != self.options.operation.as_str()
            || request.run_dir != self.options.run_dir
            || self.options.producer_binding.is_some()
        {
            return Err("固定请求阶段参数或生产者绑定不符合正式入口".into());
        }
        let input = match self.options.operation {
            PostCopyOperation::Register | PostCopyOperation::Amend => {
                let path = self
                    .options
                    .request
                    .as_ref()
                    .ok_or("登记或修订需要显式请求文件")?;
                common::binding(&common::common_file(request.backend, path, "固定请求输入")?)?
            }
            PostCopyOperation::Prepare if self.options.request.is_none() => Value::Null,
            _ => return Err("prepare不能替换登记请求".into()),
        };
        let mut frozen = self.input.borrow_mut();
        if frozen.as_ref().is_some_and(|old| *old != input) {
            return Err("取得阶段锁前请求文件变化".into());
        }
        *frozen = Some(input);
        if self.options.operation == PostCopyOperation::Register
            && crate::recovery::clone::native::lifecycle::state::attempts(
                &crate::recovery::clone::native::lifecycle::state::load(request.run_dir, true)?,
            )?
            .iter()
            .any(|row| {
                row["stage"] == "post-copy"
                    && row["mode"] == "register"
                    && row["status"] == "passed"
            })
        {
            return Err("原始登记已有唯一成功结果；继续prepare，不重复发布原始登记".into());
        }
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        let mut scope = self.scope.borrow_mut();
        if let Some(scope) = scope.as_ref() {
            scope.assert_unchanged()?;
        } else {
            *scope = Some(Scope::load(
                request.backend,
                document::array(&manifest.value["build_bridges"], "复制后构建桥接")?,
                &cache::runner::inherited_bridges(request.backend, &manifest.value)?,
            )?);
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        diagnostic::context(request, Phase::Binding, self.execute_owned(request))
    }
}

impl Runner<'_> {
    fn execute_owned(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.preflight(request)?;
        request
            .attempt
            .filter(|v| *v > 0)
            .ok_or("缺少正式阶段控制器序号")?;
        let scope = self.scope.borrow();
        let scope = scope.as_ref().ok_or("复制后阶段没有来源保护")?;
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        let storage = cache::handoff::locks::Storage::acquire(
            request.backend,
            request.run_dir,
            &manifest.value,
        )?;
        let result = (|| {
            let initialized = common::bound_file(
                request.backend,
                &manifest.value["initialized"],
                "初始化后像",
            )?;
            let owner = Generation::acquire(
                request.backend,
                initialized.parent().ok_or("初始化目录缺失")?,
            )?;
            let result = (|| {
                let copy = common::local_directory(
                    request.backend,
                    Path::new(document::text(
                        &manifest.value["copy_directory"],
                        "复制目录",
                    )?),
                    "复制目录",
                )?;
                super::files::absent(&copy.join("ledger/clone.lock"))?;
                let ledger =
                    ProcessGuard::acquire_named(&copy.join("ledger"), "copy-ledger.guard")?;
                super::files::absent(&copy.join("ledger/clone.lock"))?;
                let input = self.input.borrow();
                let context = Context::load(
                    request,
                    &owner,
                    scope,
                    &storage.directory,
                    input.as_ref().filter(|v| !v.is_null()),
                )?;
                let result = match self.options.operation {
                    PostCopyOperation::Register => {
                        diagnostic::context(request, Phase::Registration, context.register())
                    }
                    PostCopyOperation::Prepare => diagnostic::context(
                        request,
                        Phase::Preparation,
                        super::prepare::run(&context),
                    ),
                    PostCopyOperation::Amend => {
                        diagnostic::context(request, Phase::Amendment, super::amend::run(&context))
                    }
                    _ => Err("未知固定请求模式".into()),
                };
                let checked = (|| {
                    context.check()?;
                    storage.check()?;
                    ledger.check()?;
                    scope.assert_unchanged()
                })();
                super::files::combine(result, checked)
            })();
            diagnostic::context(request, Phase::Cleanup, owner.finish(result))
        })();
        diagnostic::context(request, Phase::Cleanup, storage.finish(result))
    }
}
