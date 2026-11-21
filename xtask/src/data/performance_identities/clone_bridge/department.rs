use super::model;
use crate::{
    Result,
    data::performance_identities::execution::{Context, IdentitySession},
};
use base64::Engine;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const TEMPLATE_PERMISSION: &str = "system:user-import:add";
const RECORD_FIELDS: [&str; 8] = [
    "ancestors",
    "created_at",
    "id",
    "name",
    "parent_id",
    "remark",
    "sort",
    "status",
];

struct Target {
    admin: Value,
    template: Value,
}

struct SessionEntry {
    session: Box<dyn IdentitySession>,
    authorization: Value,
}

pub(super) struct Controller {
    mode: String,
    name: String,
    context: Box<dyn Context>,
    targets: BTreeMap<String, Target>,
    sessions: BTreeMap<String, SessionEntry>,
    before: BTreeSet<String>,
    written: BTreeSet<String>,
    failed: bool,
}

impl Controller {
    pub(super) fn new(
        mode: String,
        plan: Value,
        context: Box<dyn Context>,
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
                "template_identities",
                "producer_owner",
                "launch",
                "process",
            ],
            "部门初始化",
        )?;
        validate_configuration(&plan, request)?;
        validate_system_identity(&plan, &request["identity"])?;
        let (targets, principals) = targets(&plan, &request["template_identities"])?;
        let name = department_name(&plan)?;
        let receipt = json!({
            "scope_id":plan["environment"]["scope_id"],
            "tenant_ids":request["tenant_ids"],
            "department_name":name,
            "template_principals":principals,
        });
        Ok((
            Self {
                mode,
                name,
                context,
                targets,
                sessions: BTreeMap::new(),
                before: BTreeSet::new(),
                written: BTreeSet::new(),
                failed: false,
            },
            receipt,
        ))
    }

    pub(super) async fn execute(&mut self, request: &Value) -> Result<Value> {
        if self.failed {
            return Err("部门会话已经失败，禁止继续请求或重放".into());
        }
        let result = self.execute_once(request).await;
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    async fn execute_once(&mut self, request: &Value) -> Result<Value> {
        let tenant = model::text(request, "tenant_id")?.to_owned();
        if !self.targets.contains_key(&tenant) {
            return Err("部门请求租户未登记".into());
        }
        match model::text(request, "operation")? {
            "list" => self.list(request, &tenant).await,
            "template" => self.template(request, &tenant).await,
            "detail" => self.detail(request, &tenant).await,
            "create" => self.create(request, &tenant).await,
            _ => Err("部门请求 operation 未登记".into()),
        }
    }

    async fn list(&mut self, request: &Value, tenant: &str) -> Result<Value> {
        model::exact(
            request,
            &["id", "operation", "tenant_id", "name"],
            "部门列表请求",
        )?;
        if request["name"] != self.name {
            return Err("部门列表名称不同于固定计划".into());
        }
        let name = self.name.clone();
        let (response, authorization) = {
            let entry = self.session(tenant, false).await?;
            let response = entry
                .session
                .request(&json!({
                    "operation":"get_system_depts", "query":{"page":1,"page_size":100}
                }))
                .await?;
            (response, entry.authorization.clone())
        };
        let desired = validate_list(&response, &name)?;
        if desired == 0 {
            self.before.insert(tenant.into());
        }
        Ok(json!({"response":response,"authorization":authorization}))
    }

    async fn template(&mut self, request: &Value, tenant: &str) -> Result<Value> {
        model::exact(request, &["id", "operation", "tenant_id"], "部门模板请求")?;
        let entry = self.session(tenant, true).await?;
        let response = entry.session.template_data().await?;
        validate_template(&response)?;
        Ok(json!({"response":response,"authorization":entry.authorization}))
    }

    async fn detail(&mut self, request: &Value, tenant: &str) -> Result<Value> {
        model::exact(
            request,
            &["id", "operation", "tenant_id", "department_id"],
            "部门详情请求",
        )?;
        let id = model::text(request, "department_id")?;
        model::decimal(id).ok_or("部门详情 ID 无效")?;
        let entry = self.session(tenant, false).await?;
        let response = entry
            .session
            .request(&json!({
                "operation":"get_system_depts_by_id", "path":{"id":id}
            }))
            .await?;
        validate_record(&response["data"])?;
        Ok(json!({"response":response,"authorization":entry.authorization}))
    }

    async fn create(&mut self, request: &Value, tenant: &str) -> Result<Value> {
        model::exact(
            request,
            &["id", "operation", "tenant_id", "body", "idempotency_key"],
            "部门创建请求",
        )?;
        let key = model::text(request, "idempotency_key")?;
        if self.mode != "apply"
            || self.written.contains(tenant)
            || !self.before.contains(tenant)
            || !uuid::Uuid::parse_str(key)
                .is_ok_and(|value| value.get_version_num() == 4 && value.to_string() == key)
            || request["body"] != json!({"name":self.name,"parent_id":null,"sort":0})
        {
            return Err("只允许空前像后的单次固定部门创建".into());
        }
        self.written.insert(tenant.into());
        let name = self.name.clone();
        let entry = self.session(tenant, false).await?;
        let response = entry
            .session
            .request(&json!({
                "operation":"post_system_depts", "body":request["body"], "idempotency_key":key
            }))
            .await?;
        validate_detail(
            &response,
            &name,
            response["data"]["id"].as_str().unwrap_or(""),
        )?;
        Ok(json!({"response":response,"authorization":entry.authorization}))
    }

    async fn session(&mut self, tenant: &str, template: bool) -> Result<&mut SessionEntry> {
        let key = format!("{}:{tenant}", if template { "template" } else { "owner" });
        if !self.sessions.contains_key(&key) {
            let target = self.targets.get(tenant).ok_or("部门租户未登记")?;
            let identity = if template {
                &target.template
            } else {
                &target.admin
            };
            let mut session = self.context.session(identity).await?;
            if let Err(error) = session.login().await {
                return super::super::execution::close(session.as_mut(), Err(error)).await;
            }
            let response = match session
                .request(&json!({"operation":"get_auth_context"}))
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    return super::super::execution::close(session.as_mut(), Err(error)).await;
                }
            };
            let required = if template {
                vec![TEMPLATE_PERMISSION]
            } else if self.mode == "apply" {
                vec!["system:dept:list", "system:dept:add"]
            } else {
                vec!["system:dept:list"]
            };
            let authorization = match model::authorization(&response, identity, &required, false) {
                Ok(value) => value,
                Err(error) => {
                    return super::super::execution::close(session.as_mut(), Err(error)).await;
                }
            };
            if template && authorization["is_super_admin"] != false {
                return super::super::execution::close(
                    session.as_mut(),
                    Err("部门模板主体不能是超级管理员".into()),
                )
                .await;
            }
            self.sessions.insert(
                key.clone(),
                SessionEntry {
                    session,
                    authorization,
                },
            );
        }
        self.sessions
            .get_mut(&key)
            .ok_or_else(|| "部门会话未建立".into())
    }

    pub(super) async fn close(&mut self) -> Result<()> {
        let mut first = None;
        for entry in self.sessions.values_mut() {
            if let Err(error) = entry.session.close().await
                && first.is_none()
            {
                first = Some(error);
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn targets(plan: &Value, identities: &Value) -> Result<(BTreeMap<String, Target>, Vec<Value>)> {
    let tenants = plan["environment"]["tenants"]
        .as_array()
        .ok_or("部门租户无效")?;
    let groups = plan["groups"].as_array().ok_or("部门身份组无效")?;
    let identities = identities.as_array().ok_or("部门模板身份无效")?;
    if tenants.len() != 10 || groups.len() != 11 || identities.len() != 10 {
        return Err("部门阶段必须精确绑定十个普通租户".into());
    }
    let mut result = BTreeMap::new();
    let mut principals = Vec::new();
    let mut subjects = BTreeSet::new();
    for index in 0..10 {
        let tenant = &tenants[index];
        let group = &groups[index + 1];
        let identity = &identities[index];
        let slot = format!("tenant-{:02}", index + 1);
        let user = group["users"]
            .as_array()
            .and_then(|users| users.first())
            .ok_or("部门模板用户缺失")?;
        model::exact(
            identity,
            &[
                "subject_id",
                "tenant_id",
                "username",
                "credential_slot",
                "client_address",
            ],
            "部门模板身份",
        )?;
        let subject = model::text(identity, "subject_id")?;
        if tenant["slot"] != slot
            || group["slot"] != slot
            || group["kind"] != "tenant"
            || group["tenant_id"] != tenant["tenant_id"]
            || group["admin"] != tenant["admin"]
            || !group["permissions"].as_array().is_some_and(|values| {
                values.iter().any(|value| value == TEMPLATE_PERMISSION)
                    && values.iter().all(|value| value != "*")
            })
            || identity["tenant_id"] != group["tenant_id"]
            || identity["username"] != user["username"]
            || identity["credential_slot"] != user["credential_slot"]
            || identity["client_address"] != user["client_address"]
            || model::decimal(subject).is_none()
            || !subjects.insert(subject.to_owned())
        {
            return Err("部门租户、身份组或模板主体不一致".into());
        }
        let tenant_id = model::text(tenant, "tenant_id")?.to_owned();
        if result
            .insert(
                tenant_id,
                Target {
                    admin: group["admin"].clone(),
                    template: identity.clone(),
                },
            )
            .is_some()
        {
            return Err("部门租户重复".into());
        }
        principals.push(json!({"subject_id":subject,"tenant_id":identity["tenant_id"],"username":identity["username"]}));
    }
    Ok((result, principals))
}

fn validate_system_identity(plan: &Value, identity: &Value) -> Result<()> {
    model::exact(
        identity,
        &["subject_id", "tenant_id", "username", "client_address"],
        "系统准备身份",
    )?;
    model::decimal(model::text(identity, "subject_id")?).ok_or("系统准备主体 ID 无效")?;
    for field in ["tenant_id", "username", "client_address"] {
        if identity[field] != plan["environment"]["system_admin"][field] {
            return Err("系统准备身份不同于计划".into());
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
    let tenants = plan["environment"]["tenants"]
        .as_array()
        .ok_or("部门租户无效")?
        .iter()
        .map(|value| value["tenant_id"].clone())
        .collect::<Vec<_>>();
    if request["config"] != expected
        || request["tenant_ids"] != json!(tenants)
        || request["frontend"] != plan["environment"]["frontend_dir"]
    {
        return Err("部门端点、节奏或租户集合不同于身份计划".into());
    }
    Ok(())
}

fn department_name(plan: &Value) -> Result<String> {
    let id = model::text(&plan["environment"], "plan_id")?;
    if id.len() < 3
        || id.len() > 32
        || !id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        })
    {
        return Err("部门名称不能由当前身份计划安全生成".into());
    }
    let name = format!("dv-{id}-import");
    if name.len() > 50 {
        return Err("部门名称超过 50 字节".into());
    }
    Ok(name)
}

fn validate_list(response: &Value, name: &str) -> Result<usize> {
    let data = &response["data"];
    model::exact(
        data,
        &[
            "items",
            "max_page_size",
            "page",
            "page_size",
            "total",
            "total_pages",
        ],
        "部门列表",
    )?;
    let items = data["items"].as_array().ok_or("部门列表 items 无效")?;
    let total = data["total"].as_u64().ok_or("部门列表 total 无效")?;
    let expected_pages = u64::from(total > 0);
    if response["code"] != 200
        || total != items.len() as u64
        || total > 100
        || data["page"] != 1
        || data["page_size"] != 100
        || data["max_page_size"] != 100
        || data["total_pages"] != expected_pages
    {
        return Err("部门列表不是完整第一页像".into());
    }
    let mut desired = 0;
    for item in items {
        validate_record(item)?;
        desired += usize::from(item["name"] == name);
    }
    if desired > 1 {
        return Err("部门列表包含重复固定名称".into());
    }
    Ok(desired)
}

fn validate_record(value: &Value) -> Result<()> {
    model::exact(value, &RECORD_FIELDS, "部门响应")?;
    model::decimal(model::text(value, "id")?).ok_or("部门 ID 无效")?;
    if value["name"].as_str().is_none_or(str::is_empty)
        || value["parent_id"]
            .as_str()
            .is_some_and(|id| model::decimal(id).is_none())
        || !value["parent_id"].is_null() && !value["parent_id"].is_string()
        || !value["ancestors"].is_string()
        || !value["sort"].is_i64()
        || !value["status"].is_string()
        || !(value["remark"].is_null() || value["remark"].is_string())
        || value["created_at"].as_str().is_none_or(str::is_empty)
    {
        return Err("部门响应字段无效".into());
    }
    Ok(())
}

fn validate_detail(response: &Value, name: &str, id: &str) -> Result<()> {
    model::decimal(id).ok_or("部门详情 ID 无效")?;
    let item = &response["data"];
    validate_record(item)?;
    if response["code"] != 200
        || item["id"] != id
        || item["name"] != name
        || !item["parent_id"].is_null()
        || item["ancestors"] != "0"
        || item["sort"] != 0
        || item["status"] != "1"
        || !item["remark"].is_null()
    {
        return Err("部门详情不同于固定后像".into());
    }
    Ok(())
}

fn validate_template(value: &Value) -> Result<()> {
    model::exact(
        value,
        &["base64", "bytes", "media_type", "sha256"],
        "部门模板响应",
    )?;
    let bytes = value["bytes"].as_u64().ok_or("部门模板字节数无效")?;
    let media_type = model::text(value, "media_type")?.to_ascii_lowercase();
    let sha256 = model::text(value, "sha256")?;
    if !(1..=512 * 1024).contains(&bytes)
        || !media_type
            .starts_with("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        || !model::sha256(sha256)
    {
        return Err("部门模板响应类型或边界无效".into());
    }
    let encoded = model::text(value, "base64")?;
    let content = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    if content.len() as u64 != bytes
        || base64::engine::general_purpose::STANDARD.encode(&content) != encoded
        || super::super::evidence::hash(&content) != sha256
    {
        return Err("部门模板响应内容与摘要不符".into());
    }
    Ok(())
}
