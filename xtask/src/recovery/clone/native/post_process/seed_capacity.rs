//! seed 业务及运行准备共用原目标代次和复制后调度证明。

use std::{cell::RefCell, path::Path};

use serde_json::{Value, json};

use super::{
    context::Context,
    execution, files,
    live::Live,
    ownership,
    producer::{self, QuotaSession},
    schedule_images,
};
use crate::{
    Result,
    cli::{SeedRuntimeOperation, SeedRuntimeOptions},
    recovery::clone::native::{
        bridge::scope::Scope,
        cache,
        export::transport::SystemTransport,
        json as document,
        lifecycle::{
            manifest,
            stage::{self, StageRequest, StageRunner},
            state,
        },
        resources::common,
        seed::{
            self,
            capacity::orchestration::{Port, Prepared, before_identities},
        },
        target_verify::guard::Inputs,
    },
};

pub(in crate::recovery::clone::native) fn selected(operation: SeedRuntimeOperation) -> bool {
    matches!(
        operation,
        SeedRuntimeOperation::QuotasPlan
            | SeedRuntimeOperation::QuotasApply
            | SeedRuntimeOperation::QuotasReconcile
            | SeedRuntimeOperation::IdentitiesApply
            | SeedRuntimeOperation::IdentitiesVerify
            | SeedRuntimeOperation::DepartmentsPlan
            | SeedRuntimeOperation::DepartmentsApply
            | SeedRuntimeOperation::DepartmentsReconcile
            | SeedRuntimeOperation::DepartmentsVerify
            | SeedRuntimeOperation::Prepare
            | SeedRuntimeOperation::Start
            | SeedRuntimeOperation::Close
    )
}

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    options: &SeedRuntimeOptions,
) -> Result<Value> {
    if !selected(options.operation)
        || options.request.is_some()
        || options.producer_binding.is_some()
    {
        return Err("seed 业务及运行准备沿用本 run 已发布登记和原身份计划".into());
    }
    if options.hold_originals
        && !SeedRuntimeOperation::supports_held_originals(options.operation.as_str())
    {
        return Err("当前 seed 操作不支持持有原件".into());
    }
    let runner = Runner {
        mode: options.operation.as_str(),
        scope: RefCell::new(None),
        #[cfg(windows)]
        hold_originals: options.hold_originals,
    };
    let result = stage::run_named(
        backend,
        &options.run_dir,
        "seed-runtime",
        runner.mode,
        &runner,
    )?;
    Ok(super::entry::summary(&result))
}

struct Runner {
    mode: &'static str,
    scope: RefCell<Option<Scope>>,
    #[cfg(windows)]
    hold_originals: bool,
}

impl StageRunner for Runner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.stage != "seed-runtime" || request.mode != self.mode {
            return Err("seed 操作不属于当前明确运行阶段".into());
        }
        let registration = seed::Registration::load(request.backend, request.run_dir, false)?;
        if request.mode.starts_with("quotas-") {
            before_identities(
                request.backend,
                request.run_dir,
                &registration.value,
                &state::load(request.run_dir, true)?,
            )?;
        }
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        for field in ["source_environment", "target_environment"] {
            common::environment(request.backend, &manifest.value[field])
                .map_err(|_| format!("seed 清单的 {field} 必须绑定唯一 environment 文本映射"))?;
        }
        let mut scope = self.scope.borrow_mut();
        if let Some(scope) = scope.as_ref() {
            scope.assert_unchanged()?;
        } else {
            *scope = Some(Scope::load(
                request.backend,
                document::array(&manifest.value["build_bridges"], "seed 构建桥接")?,
                &cache::runner::inherited_bridges(request.backend, &manifest.value)?,
            )?);
        }
        registration.assert_unchanged(request.backend, false)
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.preflight(request)?;
        let mut scope = self.scope.borrow_mut();
        let scope = scope.as_mut().ok_or("seed 缺少固定来源窗口")?;
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        let storage = cache::handoff::locks::Storage::acquire(
            request.backend,
            request.run_dir,
            &manifest.value,
        )?;
        ownership::with_target(request, &manifest.value, storage, |owner, storage| {
            #[cfg(windows)]
            if self.hold_originals {
                let target =
                    super::source_proofs::hold_scope(request, &manifest.value, owner, scope)?;
                return super::runtime::with_held_published_target(
                    request.backend,
                    &target,
                    |held| {
                        let mut context = Context::load_with_held_target(
                            request, owner, scope, storage, None, held, true,
                        )?;
                        context.hold_plan()?;
                        let result = execute(&context);
                        files::combine(result, context.check_end())
                    },
                );
            }
            let context = Context::load(request, owner, scope, storage, None)?;
            let result = execute(&context);
            files::combine(result, context.check())
        })
    }
}

fn execute(context: &Context<'_>) -> Result<Value> {
    let registration = seed::Registration::load(context.backend, context.run, false)?;
    let registration_path = common::common_file(
        context.backend,
        &context.run.join("seed-runtime.json"),
        "seed 登记",
    )?;
    let descriptor = common::binding(&registration_path)?;
    let held = Inputs::open([
        registration_path,
        common::bound_file(
            context.backend,
            &registration.value["identity_plan"],
            "seed 身份计划",
        )?,
    ])?;
    let guard = Guard {
        live: Live::load(context)?,
        current: seed::Current::load(context.backend, context.run)?,
        request: &registration.value,
        held,
    };
    registration.assert_unchanged(context.backend, false)?;
    if context.mode == "prepare" {
        return super::seed_prepare::execute(&guard, &registration.value, &descriptor);
    }
    if context.mode == "start" {
        return super::seed_start::execute(&guard, &registration.value);
    }
    if context.mode == "close" {
        return super::seed_close::execute(&guard, &registration.value);
    }
    if context.mode.starts_with("departments-") {
        return super::seed_department::execute(&guard, &registration.value, &descriptor);
    }
    if context.mode.starts_with("identities-") {
        return super::seed_identity::execute(&guard, &registration.value, &descriptor);
    }
    guard.quota()?;
    // 先验证全部已有配额意图和发布计划，再启动会话生产者。
    let plan = Prepared::load(
        context.backend,
        context.run,
        &context.output,
        context.number,
        &context.post.request,
        &registration.value,
        context.mode,
    )?;
    producer::with_quota(
        context,
        &registration.value,
        &descriptor,
        |session| {
            plan.run(&mut QuotaPort {
                guard: &guard,
                session,
            })
        },
        || guard.quota(),
    )
}

pub(super) struct Guard<'a, 'c> {
    pub(super) live: Live<'a, 'c>,
    current: seed::Current,
    request: &'a Value,
    held: Inputs,
}

impl Guard<'_, '_> {
    fn quota(&self) -> Result<()> {
        let context = self.live.context;
        before_identities(
            context.backend,
            context.run,
            self.request,
            &state::load(context.run, true)?,
        )?;
        self.full().map(|_| ())
    }

    pub(super) fn full(&self) -> Result<Vec<Value>> {
        self.check_binding()?;
        self.live.check()?;
        let schedules = execution::verified_schedules(&self.live)?;
        self.require_disabled()?;
        self.check_binding()?;
        Ok(schedules)
    }

    pub(super) fn quick(&self, schedules: &[Value]) -> Result<()> {
        self.check_binding()?;
        self.live.check_mutation()?;
        execution::require_schedule_stage(self.live.context, schedules)?;
        self.require_disabled()?;
        self.check_binding()
    }

    fn check_binding(&self) -> Result<()> {
        self.held.check()?;
        self.current.check(self.live.context.backend)?;
        self.held.check()
    }

    fn require_disabled(&self) -> Result<()> {
        schedule_images::require_disabled_schedules(&self.live.target(), &SystemTransport, &|| {
            self.live.checkpoint()
        })
    }

    pub(super) fn verify_administrator(&self) -> Result<()> {
        let live = &self.live;
        schedule_images::verify_administrator(
            &live.source(),
            &live.target(),
            &live.context.post.request["source_admin"],
            &SystemTransport,
            &|| live.checkpoint(),
        )
    }
}

struct QuotaPort<'a, 'b, 'c, 'd, 'e> {
    guard: &'a Guard<'b, 'c>,
    session: &'a mut QuotaSession<'d, 'e>,
}

impl Port for QuotaPort<'_, '_, '_, '_, '_> {
    fn check(&self) -> Result<()> {
        self.guard.quota()
    }

    fn verify_administrator(&mut self) -> Result<()> {
        self.guard.verify_administrator()
    }

    fn login(&mut self, tenant_ids: &Value) -> Result<Value> {
        self.session.login(tenant_ids)
    }

    fn get(&mut self, tenant_id: &str) -> Result<Value> {
        self.session.call("get", json!({"tenant_id":tenant_id}))
    }

    fn usage(&mut self, tenant_id: &str) -> Result<Value> {
        self.session.call("usage", json!({"tenant_id":tenant_id}))
    }

    fn update(&mut self, tenant_id: &str, body: &Value) -> Result<Value> {
        self.session
            .call("update", json!({"tenant_id":tenant_id,"body":body}))
    }
}
