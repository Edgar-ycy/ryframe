//! 固定 seed 部门生产者和模板主体；业务写入继续由原阶段意图授权。

use super::{
    Context, Kind, Producer, RPC_TIMEOUT, SeedLaunch, Session, document, quota::Initialization,
    retain_cleanup,
};
use crate::Result;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

const SESSION_CLOSE: Duration = Duration::from_secs(210);
const PROCESS_CLOSE: Duration = Duration::from_secs(220);

pub(in crate::recovery::clone::native) fn with_department<T>(
    context: &Context<'_>,
    request: &Value,
    registration: &Value,
    body: impl FnOnce(&mut DepartmentSession<'_, '_>) -> Result<T>,
    final_guard: impl FnOnce() -> Result<()>,
) -> Result<T> {
    let kind = kind(context.mode)?;
    let launch = SeedLaunch::load(context, request, registration)?;
    let initialization = Initialization::from_plan(&launch.identity_plan, &context.post.request)?;
    let plan_id = document::text(
        &launch.identity_plan["environment"]["plan_id"],
        "部门身份计划编号",
    )?;
    let name = format!("dv-{plan_id}-import");
    let mut department = DepartmentSession {
        kind,
        initialization,
        name,
        session: Session::new(Producer::start(context, kind, Some(launch))?),
    };
    let result = body(&mut department);
    finish(department.session, result, final_guard)
}

fn finish<T>(
    mut session: Session<'_, '_>,
    result: Result<T>,
    final_guard: impl FnOnce() -> Result<()>,
) -> Result<T> {
    // 正文仍执行原阶段守卫。会话关闭及终止后的自动注销共用 210 秒，
    // 已登记 Job 和管道用同一 220 秒总截止时刻完成收尾。
    let started = Instant::now();
    let logout = if session.attempted_login {
        session.logout(started + SESSION_CLOSE)
    } else {
        Ok(())
    };
    let result = retain_cleanup(
        session.process.context,
        "department-logout",
        "ProducerLogoutError",
        result,
        logout,
    );
    let value = session.process.finish(
        result,
        "department-producer-close",
        Some(started + PROCESS_CLOSE),
    )?;
    final_guard()?;
    Ok(value)
}

pub(in crate::recovery::clone::native) struct DepartmentSession<'a, 'r> {
    kind: Kind,
    initialization: Initialization,
    name: String,
    session: Session<'a, 'r>,
}

impl DepartmentSession<'_, '_> {
    pub(in crate::recovery::clone::native) fn login(
        &mut self,
        tenant_ids: &Value,
        templates: &Value,
    ) -> Result<Value> {
        let init = &self.initialization;
        let expected = expected_receipt(
            &init.configuration["bindings"]["scope_id"],
            &self.name,
            &init.tenants[1..],
            tenant_ids,
            templates,
        )?;
        let mut request =
            self.session
                .initialization_fields(&init.frontend, &init.configuration, &init.identity);
        request["tenant_ids"] = tenant_ids.clone();
        request["template_identities"] = templates.clone();
        let receipt = self.session.send_initialization(request)?;
        if receipt != expected {
            self.session.ready = false;
            return Err("部门初始化收据未绑定原 scope、租户顺序、名称和模板主体".into());
        }
        Ok(receipt)
    }

    pub(in crate::recovery::clone::native) fn call(
        &mut self,
        operation: &str,
        values: Value,
    ) -> Result<Value> {
        if !self.session.ready {
            return Err("部门生产者尚未明确初始化".into());
        }
        check_operation(
            self.kind,
            &self.initialization.tenants[1..],
            operation,
            &values,
        )?;
        let result = self.session.rpc(operation, values, RPC_TIMEOUT)?;
        if let Err(error) = document::exact(&result, &["response", "authorization"], "部门操作响应")
        {
            self.session.ready = false;
            return Err(error);
        }
        Ok(result)
    }
}

pub(super) fn kind(mode: &str) -> Result<Kind> {
    let name = match mode {
        "departments-plan" => "department-plan",
        "departments-apply" => "department-apply",
        "departments-reconcile" => "department-reconcile",
        "departments-verify" => "department-verify",
        _ => return Err("部门生产者只接受规划、应用、核对或验证阶段".into()),
    };
    Ok(Kind::Department(name))
}

pub(super) fn expected_receipt(
    scope: &Value,
    name: &str,
    tenants: &[String],
    tenant_ids: &Value,
    templates: &Value,
) -> Result<Value> {
    let templates = document::array(templates, "部门模板身份")?;
    if tenants.len() != 10 || templates.len() != tenants.len() || *tenant_ids != json!(tenants) {
        return Err("部门初始化必须绑定身份计划的十个原槽位与模板主体".into());
    }
    let mut subjects = BTreeSet::new();
    let mut principals = Vec::with_capacity(templates.len());
    for (tenant, template) in tenants.iter().zip(templates) {
        document::exact(
            template,
            &[
                "subject_id",
                "tenant_id",
                "username",
                "credential_slot",
                "client_address",
            ],
            "部门模板身份",
        )?;
        let subject = document::text(&template["subject_id"], "部门模板主体")?;
        if template["tenant_id"] != *tenant || !subjects.insert(subject) {
            return Err("部门模板主体与原租户槽位不同或重复".into());
        }
        principals
            .push(json!({"subject_id":subject,"tenant_id":tenant,"username":template["username"]}));
    }
    Ok(
        json!({"scope_id":scope,"tenant_ids":tenant_ids,"department_name":name,
        "template_principals":principals}),
    )
}

pub(super) fn check_operation(
    kind: Kind,
    tenants: &[String],
    operation: &str,
    values: &Value,
) -> Result<()> {
    let fields: &[&str] = match operation {
        "list" => &["tenant_id", "name"],
        "template" => &["tenant_id"],
        "detail" => &["tenant_id", "department_id"],
        "create" if kind == Kind::Department("department-apply") => {
            &["tenant_id", "body", "idempotency_key"]
        }
        _ => return Err("当前部门阶段不允许此请求".into()),
    };
    document::exact(values, fields, "部门会话请求")?;
    let tenant = document::text(&values["tenant_id"], "部门请求租户")?;
    if !tenants.iter().any(|value| value == tenant) {
        return Err("部门请求不属于登记的普通租户".into());
    }
    Ok(())
}
