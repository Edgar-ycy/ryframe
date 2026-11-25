//! seed 配额桥接沿用正式控制器和旧动作的完整阶段守卫。

use super::super::registration;
use super::{
    Context, Kind, Producer, RPC_TIMEOUT, SeedLaunch, Session, common, document, files,
    finish_session,
};
use crate::{
    Result,
    recovery::clone::native::{lifecycle::state, seed},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(in crate::recovery::clone::native) fn with_quota<T>(
    context: &Context<'_>,
    request: &Value,
    registration: &Value,
    body: impl FnOnce(&mut QuotaSession<'_, '_>) -> Result<T>,
    final_guard: impl FnOnce() -> Result<()>,
) -> Result<T> {
    let kind = kind(context.mode)?;
    let launch = SeedLaunch::load(context, request, registration)?;
    let initialization = Initialization::from_plan(&launch.identity_plan, &context.post.request)?;
    let mut quota = QuotaSession {
        kind,
        initialization,
        session: Session::new(Producer::start(context, kind, Some(launch))?),
    };
    let result = body(&mut quota);
    // 业务请求之间仍由正文执行原阶段守卫。关闭或等待子进程自行注销最多 10 秒，
    // 之后执行调用方的原完整阶段后验，再回收已登记的 Job 和管道。
    finish_session(
        quota.session,
        result,
        ["quota-logout", "quota-final-guard", "quota-producer-close"],
        final_guard,
    )
}

pub(in crate::recovery::clone::native) struct QuotaSession<'a, 'r> {
    kind: Kind,
    initialization: Initialization,
    session: Session<'a, 'r>,
}

impl QuotaSession<'_, '_> {
    pub(in crate::recovery::clone::native) fn login(
        &mut self,
        tenant_ids: &Value,
    ) -> Result<Value> {
        let init = &self.initialization;
        if *tenant_ids != json!(init.tenants) {
            return Err("配额登录租户集合或顺序不同于绑定身份计划".into());
        }
        let auth = self.session.initialize(
            &init.frontend,
            &init.configuration,
            &init.identity,
            Some(tenant_ids),
        )?;
        if let Err(error) = authorization(&auth, &init.identity) {
            self.session.ready = false;
            return Err(error);
        }
        Ok(auth)
    }

    pub(in crate::recovery::clone::native) fn call(
        &mut self,
        operation: &str,
        values: Value,
    ) -> Result<Value> {
        if !self.session.ready {
            return Err("配额会话尚未明确授权".into());
        }
        check_operation(self.kind, &self.initialization.tenants, operation, &values)?;
        self.session.rpc(operation, values, RPC_TIMEOUT)
    }
}

impl<'a> SeedLaunch<'a> {
    pub(super) fn load(
        context: &Context<'_>,
        request: &'a Value,
        descriptor: &'a Value,
    ) -> Result<Self> {
        let path = common::bound_file(context.backend, descriptor, "seed 生产者登记")?;
        if registration::resolve(
            context.backend,
            context.run,
            "seed-runtime",
            descriptor,
            Some(context.number),
            false,
        )? != *request
        {
            return Err("生产者登记不属于当前正式 seed 阶段".into());
        }
        let identity_plan =
            common::read_bound(context.backend, &request["identity_plan"], "配额身份计划")?;
        let mut inputs = BTreeSet::from([path]);
        if descriptor["path"] != json!(context.run.join("seed-runtime.json")) {
            let successor = common::read_bound(context.backend, descriptor, "后继身份工具登记")?;
            files::bindings(context.backend, &successor, &mut inputs)?;
        }
        for value in [request, &identity_plan] {
            files::bindings(context.backend, value, &mut inputs)?;
        }
        for name in ["runtime.json", "api.json"] {
            inputs.insert(common::common_file(
                context.backend,
                &context.post.runtime.join(name),
                "配额原 API 运行收据",
            )?);
        }
        Ok(Self {
            request,
            descriptor,
            identity_plan,
            inputs,
        })
    }

    pub(super) fn check(&self, context: &Context<'_>) -> Result<()> {
        let saved = state::load(context.run, true)?;
        let attempt = state::running_attempt(&saved)?.ok_or("配额生产者没有运行中的正式阶段")?;
        if attempt["number"].as_u64() != Some(context.number)
            || attempt["stage"] != "seed-runtime"
            || attempt["mode"] != context.mode
            || self.request["post_copy"] != context.post.descriptor
            || registration::resolve(
                context.backend,
                context.run,
                "seed-runtime",
                self.descriptor,
                Some(context.number),
                false,
            )? != *self.request
        {
            return Err("配额生产者不属于当前正式 seed 登记和阶段".into());
        }
        let original = seed::Registration::load(context.backend, context.run, false)?;
        if seed::identity::inputs(context.backend, context.run, &original.value, &context.post)?
            != self.identity_plan
        {
            return Err("配额生产者的身份计划在绑定期间变化".into());
        }
        original.assert_unchanged(context.backend, false)?;
        Ok(())
    }
}

pub(super) fn kind(mode: &str) -> Result<Kind> {
    match mode {
        "quotas-plan" => Ok(Kind::QuotaPlan),
        "quotas-apply" => Ok(Kind::QuotaApply),
        "quotas-reconcile" => Ok(Kind::QuotaReconcile),
        _ => Err("配额生产者只接受规划、应用或核对正式阶段".into()),
    }
}

pub(super) struct Initialization {
    pub(super) frontend: Value,
    pub(super) configuration: Value,
    pub(super) identity: Value,
    pub(super) tenants: Vec<String>,
}

impl Initialization {
    pub(super) fn from_plan(plan: &Value, post: &Value) -> Result<Self> {
        let environment = &plan["environment"];
        let frontend = &environment["frontend_dir"];
        let administrator = &post["source_admin"];
        if frontend != &post["frontend_root"]
            || frontend.as_str().is_none()
            || administrator["tenant_id"] != "system"
            || ["tenant_id", "username", "client_address"]
                .iter()
                .any(|field| administrator[*field] != environment["system_admin"][*field])
        {
            return Err("配额登录前端或系统管理员不同于固定身份计划".into());
        }
        let subject = document::text(&administrator["subject_id"], "配额系统管理员 ID")?;
        let mut tenants = vec!["system".to_owned()];
        for tenant in document::array(&environment["tenants"], "配额身份计划租户")? {
            tenants.push(document::text(&tenant["tenant_id"], "配额租户 ID")?.to_owned());
        }
        if tenants.len() != 11 || tenants.iter().collect::<BTreeSet<_>>().len() != 11 {
            return Err("配额会话必须绑定系统和十个固定租户".into());
        }
        Ok(Self {
            frontend: frontend.clone(),
            configuration: json!({
                "contract":{"timeout_ms":120000,"pacing":environment["pacing"]["contract"]},
                "bindings":{"scope_id":environment["scope_id"],"api_url":environment["api_url"],
                    "frontend_url":environment["frontend_url"],"pacing":environment["pacing"]["bindings"]},
            }),
            identity: json!({"subject_id":subject,"tenant_id":administrator["tenant_id"],
                "username":administrator["username"],"client_address":administrator["client_address"]}),
            tenants,
        })
    }
}

pub(super) fn check_operation(
    kind: Kind,
    tenants: &[String],
    operation: &str,
    values: &Value,
) -> Result<()> {
    let fields: &[&str] = match operation {
        "get" | "usage" => &["tenant_id"],
        "update" if kind == Kind::QuotaApply => &["tenant_id", "body"],
        _ => return Err("当前配额阶段不允许此请求".into()),
    };
    document::exact(values, fields, "配额会话请求")?;
    let tenant = document::text(&values["tenant_id"], "配额请求租户")?;
    if !tenants.iter().any(|value| value == tenant) {
        return Err("配额请求不属于绑定身份计划的租户".into());
    }
    if operation == "update" {
        document::object(&values["body"], "配额更新体")?;
    }
    Ok(())
}

pub(super) fn authorization(value: &Value, identity: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "subject_id",
            "tenant_id",
            "username",
            "is_super_admin",
            "permissions",
        ],
        "配额登录授权",
    )?;
    let permissions = document::array(&value["permissions"], "配额授权权限")?;
    let has = |name: &str| permissions.iter().any(|permission| permission == name);
    if ["subject_id", "tenant_id", "username"]
        .iter()
        .any(|field| value[*field] != identity[*field])
        || value["tenant_id"] != "system"
        || !value["is_super_admin"].is_boolean()
        || permissions.iter().any(|permission| !permission.is_string())
        || (value["is_super_admin"] != true
            && !has("*")
            && !["tenant:list", "tenant:edit", "tenant:usage:list"]
                .into_iter()
                .all(has))
    {
        return Err("配额授权未确认原系统管理员和必需权限".into());
    }
    Ok(())
}
