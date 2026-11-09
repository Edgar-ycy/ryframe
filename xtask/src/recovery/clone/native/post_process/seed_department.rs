//! 部门阶段沿用同一 seed 控制窗口和原 API/Worker 守卫。

use std::cell::RefCell;

use serde_json::Value;

use super::{
    producer::{self, DepartmentSession},
    seed_capacity::Guard,
};
use crate::{
    Result,
    recovery::clone::native::department::{
        self,
        orchestration::{Port, Prepared},
    },
};

pub(super) fn execute(
    guard: &Guard<'_, '_>,
    request: &Value,
    registration: &Value,
) -> Result<Value> {
    let context = guard.live.context;
    let checks = Checks {
        guard,
        request,
        schedules: RefCell::new(None),
    };
    if context.mode == "departments-plan" {
        let adopted = department::adopt_plan::run(
            context.backend,
            context.run,
            &context.output,
            context.number,
            request,
            &guard.live.department_target()?,
            &mut |prior| checks.check(prior, false),
            &mut || guard.verify_administrator(),
        )?;
        if let Some(value) = adopted {
            return Ok(value);
        }
    }
    let prepared = Prepared::load(
        context.backend,
        context.run,
        &context.output,
        context.number,
        request,
        context.mode,
    )?;
    producer::with_department(
        context,
        request,
        registration,
        |session| {
            prepared.run(&mut DepartmentPort {
                checks: &checks,
                session,
            })
        },
        || {
            let prior = department::read_prerequisite(context.backend, context.run)?;
            checks.check(Some(&prior), false)
        },
    )
}

struct Checks<'a, 'b, 'c> {
    guard: &'a Guard<'b, 'c>,
    request: &'a Value,
    schedules: RefCell<Option<Vec<Value>>>,
}

impl Checks<'_, '_, '_> {
    fn check(&self, prerequisite: Option<&Value>, quick: bool) -> Result<()> {
        let context = self.guard.live.context;
        if quick {
            let prior = prerequisite.ok_or("部门写入临界区缺少身份前提")?;
            department::require_prerequisite(context.backend, context.run, self.request, prior)?;
            let schedules = self.schedules.borrow();
            self.guard
                .quick(schedules.as_deref().ok_or("部门临界区缺少原完整调度证明")?)?;
        } else {
            *self.schedules.borrow_mut() = Some(self.guard.full()?);
        }
        if let Some(prior) = prerequisite {
            department::require_prerequisite(context.backend, context.run, self.request, prior)?;
        }
        Ok(())
    }
}

struct DepartmentPort<'a, 'b, 'c, 'd, 'e, 'f> {
    checks: &'a Checks<'b, 'c, 'd>,
    session: &'a mut DepartmentSession<'e, 'f>,
}

impl Port for DepartmentPort<'_, '_, '_, '_, '_, '_> {
    fn check(&self, prerequisite: Option<&Value>, quick: bool) -> Result<()> {
        self.checks.check(prerequisite, quick)
    }

    fn verify_administrator(&mut self) -> Result<()> {
        self.checks.guard.verify_administrator()
    }

    fn login(&mut self, tenant_ids: &Value, templates: &Value) -> Result<Value> {
        self.session.login(tenant_ids, templates)
    }

    fn call(&mut self, operation: &str, values: Value) -> Result<Value> {
        self.session.call(operation, values)
    }
}
