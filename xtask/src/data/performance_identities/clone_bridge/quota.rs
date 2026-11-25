use super::super::state;
use super::model;
use crate::{
    Result,
    data::performance_identities::execution::{Context, IdentitySession},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const FIELDS: [&str; 7] = [
    "name",
    "domain",
    "expire_at",
    "max_users",
    "max_roles",
    "max_storage_mb",
    "max_requests_per_min",
];

pub(super) struct Controller {
    mode: String,
    desired: BTreeMap<String, u64>,
    before: BTreeMap<String, Value>,
    written: BTreeSet<String>,
    session: Box<dyn IdentitySession>,
    identity: Value,
    failed: bool,
}

impl Controller {
    pub(super) async fn new(
        mode: String,
        plan: Value,
        mut context: Box<dyn Context>,
        request: &Value,
    ) -> Result<(Self, Value)> {
        model::exact(
            request,
            &[
                "id",
                "operation",
                "backend",
                "frontend",
                "config",
                "identity",
                "artifacts",
                "run_dir",
                "attempt",
                "tenant_ids",
                "producer_owner",
                "launch",
                "process",
            ],
            "配额初始化",
        )?;
        validate_configuration(&plan, request)?;
        let identity = request["identity"].clone();
        validate_identity(&plan, &identity)?;
        let mut session = context.session(&identity).await?;
        if let Err(error) = session.login().await {
            return super::super::execution::close(session.as_mut(), Err(error)).await;
        }
        let auth = match session
            .request(&json!({"operation":"get_auth_context"}))
            .await
        {
            Ok(value) => value,
            Err(error) => {
                return super::super::execution::close(session.as_mut(), Err(error)).await;
            }
        };
        let receipt = match model::authorization(
            &auth,
            &identity,
            &["tenant:list", "tenant:edit", "tenant:usage:list"],
            true,
        ) {
            Ok(value) => value,
            Err(error) => {
                return super::super::execution::close(session.as_mut(), Err(error)).await;
            }
        };
        let mut desired = BTreeMap::new();
        desired.insert(
            "system".into(),
            plan["environment"]["quota"]["system_max_users"]
                .as_u64()
                .ok_or("系统租户配额无效")?,
        );
        for tenant in plan["environment"]["tenants"]
            .as_array()
            .ok_or("配额租户列表无效")?
        {
            desired.insert(
                model::text(tenant, "tenant_id")?.to_owned(),
                plan["environment"]["quota"]["tenant_max_users"]
                    .as_u64()
                    .ok_or("普通租户配额无效")?,
            );
        }
        Ok((
            Self {
                mode,
                desired,
                before: BTreeMap::new(),
                written: BTreeSet::new(),
                session,
                identity,
                failed: false,
            },
            receipt,
        ))
    }

    pub(super) async fn execute(&mut self, request: &Value) -> Result<Value> {
        if self.failed {
            return Err("配额会话已经失败，禁止继续请求或重放".into());
        }
        let result = self.execute_once(request).await;
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    async fn execute_once(&mut self, request: &Value) -> Result<Value> {
        let operation = model::text(request, "operation")?;
        let fields = if operation == "update" {
            vec!["id", "operation", "tenant_id", "body"]
        } else {
            vec!["id", "operation", "tenant_id"]
        };
        model::exact(request, &fields, "配额操作")?;
        let tenant = model::text(request, "tenant_id")?;
        let desired = *self.desired.get(tenant).ok_or("配额租户未登记")?;
        let path = json!({"tenant_id":tenant});
        match operation {
            "get" => {
                let response = self
                    .session
                    .request(&json!({
                        "operation":"get_platform_tenants_by_tenant_id", "path":path
                    }))
                    .await?;
                validate_before(&response, tenant)?;
                self.before.insert(tenant.into(), response["data"].clone());
                Ok(response)
            }
            "usage" => Ok(self
                .session
                .request(&json!({
                    "operation":"get_platform_tenants_by_tenant_id_usage", "path":path
                }))
                .await?),
            "update" => self.update(request, tenant, desired, path).await,
            _ => Err("配额操作未登记".into()),
        }
    }

    async fn update(
        &mut self,
        request: &Value,
        tenant: &str,
        desired: u64,
        path: Value,
    ) -> Result<Value> {
        model::exact(&request["body"], &FIELDS, "配额更新体")?;
        let before = self.before.get(tenant).ok_or("配额更新缺少完整前像")?;
        if self.mode != "apply"
            || self.written.contains(tenant)
            || before["status"] != "enabled"
            || desired == 0
            || before["max_users"]
                .as_u64()
                .is_none_or(|value| value == 0 || desired <= value)
            || request["body"]["max_users"] != desired
            || FIELDS
                .iter()
                .any(|field| *field != "max_users" && request["body"][*field] != before[*field])
        {
            return Err("只允许一次增加登记配额并保持其余完整前像".into());
        }
        self.written.insert(tenant.into());
        let response = self
            .session
            .request(&json!({
                "operation":"put_platform_tenants_by_tenant_id", "path":path, "body":request["body"]
            }))
            .await?;
        let old = self
            .session
            .token()
            .ok_or("配额更新缺少当前令牌")?
            .to_owned();
        self.session
            .request(&json!({"operation":"post_auth_refresh"}))
            .await?;
        if self
            .session
            .token()
            .is_none_or(|value| value.is_empty() || value == old)
        {
            return Err("配额更新后 access token 未轮换".into());
        }
        let auth = self
            .session
            .request(&json!({"operation":"get_auth_context"}))
            .await?;
        let authorization = model::authorization(
            &auth,
            &self.identity,
            &["tenant:list", "tenant:edit", "tenant:usage:list"],
            true,
        )?;
        Ok(json!({"response":response,"authorization":authorization}))
    }

    pub(super) async fn close(&mut self) -> Result<()> {
        self.session.close().await
    }
}

fn validate_before(response: &Value, tenant: &str) -> Result<()> {
    let data = &response["data"];
    if data["tenant_id"] != tenant
        || FIELDS
            .iter()
            .filter(|field| field.starts_with("max_"))
            .any(|field| data[*field].as_u64().is_none())
    {
        return Err("配额前像租户或整数边界无效".into());
    }
    Ok(())
}

fn validate_identity(plan: &Value, identity: &Value) -> Result<()> {
    model::exact(
        identity,
        &["subject_id", "tenant_id", "username", "client_address"],
        "配额身份",
    )?;
    state::id(&identity["subject_id"]).map_err(|_| "配额主体 ID 无效")?;
    if identity["tenant_id"] != "system" {
        return Err("配额只能使用系统管理员".into());
    }
    for field in ["tenant_id", "username", "client_address"] {
        if identity[field] != plan["environment"]["system_admin"][field] {
            return Err("配额管理员不同于身份计划".into());
        }
    }
    Ok(())
}

fn validate_configuration(plan: &Value, request: &Value) -> Result<()> {
    let expected = json!({
        "contract":{"timeout_ms":120000,"pacing":plan["environment"]["pacing"]["contract"]},
        "bindings":{"scope_id":plan["environment"]["scope_id"],"api_url":plan["environment"]["api_url"],
            "frontend_url":plan["environment"]["frontend_url"],"pacing":plan["environment"]["pacing"]["bindings"]}
    });
    let tenants = std::iter::once("system")
        .chain(
            plan["environment"]["tenants"]
                .as_array()
                .ok_or("配额租户无效")?
                .iter()
                .map(|value| value["tenant_id"].as_str().unwrap_or("")),
        )
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if request["config"] != expected
        || request["tenant_ids"] != json!(tenants)
        || request["frontend"] != plan["environment"]["frontend_dir"]
    {
        return Err("配额端点、节奏或租户集合不同于身份计划".into());
    }
    Ok(())
}
