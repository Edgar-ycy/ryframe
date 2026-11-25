//! 复制后阶段在统一入口内持有真实存储、目标代次和完整复制账本保护。
use super::{
    business_binding::Completion,
    context::Context,
    diagnostic::{self, Phase},
};
use crate::recovery::fixture_control::source_pair::execution::HeldTargetSource;
use crate::{
    Result,
    cli::{PostCopyOperation, PostCopyOptions},
    recovery::clone::native::{
        bridge::scope::Scope,
        cache, json as document,
        lifecycle::{
            manifest,
            stage::{self, StageRequest, StageRunner},
        },
        resources::common,
        target_verify::guard::Generation,
    },
};
use serde_json::{Value, json};
use std::{cell::RefCell, path::Path};

pub(crate) fn run(backend: &Path, options: &PostCopyOptions) -> Result<Value> {
    if options.hold_originals
        && !PostCopyOperation::supports_held_originals(options.operation.as_str())
    {
        return Err("当前复制后操作不支持持有原件".into());
    }
    if options.operation == PostCopyOperation::RecoverSession {
        let path = options
            .producer_binding
            .as_deref()
            .ok_or("回收需要明确生产者收据描述文件")?;
        return super::recover::run(backend, &options.run_dir, "post-copy", path);
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
    Ok(summary(&value))
}

pub(in crate::recovery::clone::native) fn summary(value: &Value) -> Value {
    json!({"status":value["status"],"stage":value["stage"],"mode":value["mode"],
        "attempt":value["attempt"],"restore_qualified":value["restore_qualified"]})
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
            _ if self.options.request.is_none() => Value::Null,
            _ => return Err("此复制后阶段沿用已登记请求".into()),
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
        let mut scope = self.scope.borrow_mut();
        let scope = scope.as_mut().ok_or("复制后阶段没有来源保护")?;
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        let storage = cache::handoff::locks::Storage::acquire(
            request.backend,
            request.run_dir,
            &manifest.value,
        )?;
        let (value, completion) = diagnostic::context(
            request,
            Phase::Cleanup,
            super::ownership::with_target(request, &manifest.value, storage, |owner, storage| {
                let input = self.input.borrow();
                let input = input.as_ref().filter(|v| !v.is_null());
                #[cfg(windows)]
                if self.options.hold_originals {
                    let target = hold_post_copy_scope(request, &manifest.value, owner, scope)?;
                    return super::runtime::with_held_published_target(
                        request.backend,
                        &target,
                        |held| {
                            self.execute_with_context(request, owner, scope, storage, input, held)
                        },
                    );
                }
                self.execute_with_context(request, owner, scope, storage, input, None)
            }),
        )?;
        if let Some(completion) = completion {
            diagnostic::context(request, Phase::Business, completion.check(request.backend))?;
        }
        Ok(value)
    }

    fn execute_with_context<'stage>(
        &self,
        request: &StageRequest<'stage>,
        owner: &'stage Generation,
        scope: &'stage Scope,
        storage: &'stage Path,
        input: Option<&Value>,
        held: Option<&'stage HeldTargetSource>,
    ) -> Result<(Value, Option<Completion>)> {
        let context = Context::load_with_held_target(
            request,
            owner,
            scope,
            storage,
            input,
            held,
            self.options.hold_originals,
        )?;
        #[cfg(windows)]
        let context = {
            let mut context = context;
            if self.options.hold_originals {
                context.hold_plan()?;
            }
            context
        };
        let result = self.execute_stage(request, &context);
        super::files::combine(result, context.check_end())
    }

    fn execute_stage(
        &self,
        request: &StageRequest<'_>,
        context: &Context<'_>,
    ) -> Result<(Value, Option<Completion>)> {
        let without_completion = |value| (value, None);
        let (phase, result) = match self.options.operation {
            PostCopyOperation::Register => (
                Phase::Registration,
                context.register().map(without_completion),
            ),
            PostCopyOperation::Prepare => (
                Phase::Preparation,
                super::prepare::run(context).map(without_completion),
            ),
            PostCopyOperation::Amend => (
                Phase::Amendment,
                super::amend::run(context).map(without_completion),
            ),
            PostCopyOperation::Schedules | PostCopyOperation::Reconcile => {
                (Phase::Schedules, super::execution::run(context))
            }
            PostCopyOperation::Verify => (Phase::Business, super::execution::run(context)),
            PostCopyOperation::RecoverSession => return Err("回收须使用专用的历史保护入口".into()),
        };
        diagnostic::context(request, phase, result)
    }
}

#[cfg(windows)]
fn hold_post_copy_scope(
    request: &StageRequest<'_>,
    manifest: &Value,
    owner: &Generation,
    scope: &mut Scope,
) -> Result<Value> {
    use crate::recovery::clone::native::{export, target_verify};
    use target_verify::input::request::{SystemSourceObserver, execution_backend};

    if request.stage != "post-copy"
        || !PostCopyOperation::supports_held_originals(request.mode)
        || manifest["copy_stage"] != "source_to_seed"
    {
        return Err("持有原件必须属于受支持的 source_to_seed 复制后阶段".into());
    }
    let source =
        export::input::Context::for_copy(request.backend, request.run_dir, request.run_dir)?;
    let initialized = common::bound_file(request.backend, &manifest["initialized"], "初始化后像")?;
    let (_, target) =
        target_verify::published::owned_history(request.backend, &initialized, owner)?;
    let (execution, _) = execution_backend(
        request.backend,
        target.get("execution_backend"),
        &SystemSourceObserver,
    )?;
    scope.hold_windows_sources(&[&source.execution_backend, &execution])?;
    Ok(target)
}
