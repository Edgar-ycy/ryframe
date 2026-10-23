use super::{department, quota};
use crate::Result;
use crate::data::performance_identities::{
    evidence,
    execution::{Context, IdentitySession},
};
use base64::Engine;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    path::Path,
    rc::Rc,
};

#[derive(Default)]
struct Observed {
    requests: Vec<Value>,
    closed: usize,
    fail_close: bool,
}

struct Script {
    identity: Value,
    responses: BTreeMap<String, VecDeque<Value>>,
    template: Value,
}

struct FakeContext {
    scripts: VecDeque<Script>,
    observed: Rc<RefCell<Observed>>,
}

#[async_trait::async_trait(?Send)]
impl Context for FakeContext {
    async fn password(&mut self, _: &str) -> Result<String> {
        Err("测试上下文不提供密码".into())
    }

    async fn inspect(&mut self, _: &str) -> Result<Value> {
        Err("测试上下文不检查外部服务".into())
    }

    async fn session(&mut self, identity: &Value) -> Result<Box<dyn IdentitySession>> {
        let script = self.scripts.pop_front().ok_or("缺少测试会话")?;
        if script.identity != *identity {
            return Err("测试会话身份顺序不匹配".into());
        }
        Ok(Box::new(FakeSession {
            responses: script.responses,
            template: script.template,
            token: "first-token".into(),
            observed: self.observed.clone(),
        }) as Box<dyn IdentitySession>)
    }
}

struct FakeSession {
    responses: BTreeMap<String, VecDeque<Value>>,
    template: Value,
    token: String,
    observed: Rc<RefCell<Observed>>,
}

#[async_trait::async_trait(?Send)]
impl IdentitySession for FakeSession {
    async fn login(&mut self) -> Result<()> {
        Ok(())
    }

    async fn request(&mut self, step: &Value) -> Result<Value> {
        self.observed.borrow_mut().requests.push(step.clone());
        let operation = step["operation"].as_str().ok_or("测试操作缺失")?;
        if operation == "post_auth_refresh" {
            self.token = "second-token".into();
        }
        self.responses
            .get_mut(operation)
            .and_then(VecDeque::pop_front)
            .ok_or_else(|| format!("测试操作没有响应：{operation}").into())
    }

    async fn template(&mut self, _: &Path) -> Result<Value> {
        Err("测试不写模板文件".into())
    }

    async fn template_data(&mut self) -> Result<Value> {
        Ok(self.template.clone())
    }

    fn token(&self) -> Option<&str> {
        Some(&self.token)
    }

    async fn close(&mut self) -> Result<()> {
        self.observed.borrow_mut().closed += 1;
        if self.observed.borrow().fail_close {
            Err("测试关闭失败".into())
        } else {
            Ok(())
        }
    }
}

#[path = "conversation_tests.rs"]
mod conversation_tests;

#[test]
fn quota_controller_keeps_full_before_image_rotates_token_and_blocks_replay() {
    runtime().block_on(async {
        let plan = plan();
        let identity = system_identity(&plan);
        let before = json!({
            "tenant_id":"system", "status":"enabled", "name":"System", "domain":null,
            "expire_at":null, "max_users":100, "max_roles":10,
            "max_storage_mb":1000, "max_requests_per_min":500
        });
        let body = json!({
            "name":"System", "domain":null, "expire_at":null, "max_users":200,
            "max_roles":10, "max_storage_mb":1000, "max_requests_per_min":500
        });
        let auth = authorization(
            &identity,
            true,
            &["tenant:list", "tenant:edit", "tenant:usage:list"],
        );
        let observed = Rc::new(RefCell::new(Observed::default()));
        let context = FakeContext {
            scripts: VecDeque::from([script(
                identity.clone(),
                [
                    ("get_auth_context", vec![auth.clone(), auth]),
                    (
                        "get_platform_tenants_by_tenant_id",
                        vec![json!({"data":before})],
                    ),
                    (
                        "put_platform_tenants_by_tenant_id",
                        vec![json!({"code":200})],
                    ),
                    ("post_auth_refresh", vec![json!({"code":200})]),
                ],
                Value::Null,
            )]),
            observed: observed.clone(),
        };
        let request = initialization(&plan, identity, true);
        let (mut controller, receipt) =
            quota::Controller::new("apply".into(), plan, Box::new(context), &request)
                .await
                .unwrap();
        assert_eq!(receipt["subject_id"], "1");
        controller
            .execute(&json!({"id":2,"operation":"get","tenant_id":"system"}))
            .await
            .unwrap();
        let update = json!({"id":3,"operation":"update","tenant_id":"system","body":body});
        assert!(controller.execute(&update).await.is_ok());
        assert!(controller.execute(&update).await.is_err());
        controller.close().await.unwrap();
        assert_eq!(observed.borrow().closed, 1);
    });
}

#[test]
fn department_controller_uses_fixed_tenant_subject_and_validates_template_bytes() {
    runtime().block_on(async {
        let plan = plan();
        let admin = plan["groups"][1]["admin"].clone();
        let template = template_identities(&plan)[0].clone();
        let name = "dv-process-import";
        let record = json!({"id":"20","name":name,"parent_id":null,"ancestors":"0",
            "sort":0,"status":"1","remark":null,"created_at":"2026-01-01T00:00:00Z"});
        let bytes = b"xlsx";
        let template_response = json!({
            "base64":base64::engine::general_purpose::STANDARD.encode(bytes),
            "bytes":bytes.len(),
            "media_type":"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "sha256":evidence::hash(bytes),
        });
        let owner_auth = authorization(&admin, false, &["system:dept:list", "system:dept:add"]);
        let template_auth = authorization(&template, false, &["system:user-import:add"]);
        let observed = Rc::new(RefCell::new(Observed::default()));
        let context = FakeContext {
            scripts: VecDeque::from([
                script(
                    admin,
                    [
                        ("get_auth_context", vec![owner_auth]),
                        ("get_system_depts", vec![empty_departments()]),
                        ("post_system_depts", vec![json!({"code":200,"data":record})]),
                    ],
                    Value::Null,
                ),
                script(
                    template,
                    [("get_auth_context", vec![template_auth])],
                    template_response,
                ),
            ]),
            observed: observed.clone(),
        };
        let request = initialization(&plan, system_identity(&plan), false);
        let (mut controller, receipt) =
            department::Controller::new("apply".into(), plan, Box::new(context), &request).unwrap();
        assert_eq!(receipt["template_principals"].as_array().unwrap().len(), 10);
        controller
            .execute(&json!({"id":2,"operation":"list","tenant_id":"tenant-1","name":name}))
            .await
            .unwrap();
        controller
            .execute(&json!({"id":3,"operation":"create","tenant_id":"tenant-1",
                "body":{"name":name,"parent_id":null,"sort":0},
                "idempotency_key":"12345678-1234-4abc-8def-1234567890ab"}))
            .await
            .unwrap();
        controller
            .execute(&json!({"id":4,"operation":"template","tenant_id":"tenant-1"}))
            .await
            .unwrap();
        controller.close().await.unwrap();
        assert_eq!(observed.borrow().closed, 2);
    });
}

fn script<const N: usize>(
    identity: Value,
    responses: [(&str, Vec<Value>); N],
    template: Value,
) -> Script {
    Script {
        identity,
        responses: responses
            .into_iter()
            .map(|(key, values)| (key.into(), values.into()))
            .collect(),
        template,
    }
}

fn authorization(identity: &Value, super_admin: bool, permissions: &[&str]) -> Value {
    let subject = identity
        .get("subject_id")
        .cloned()
        .unwrap_or_else(|| json!("2"));
    json!({"data":{"user":{"id":subject,"tenant_id":identity["tenant_id"],
        "username":identity["username"]},"is_super_admin":super_admin,"permissions":permissions}})
}

fn empty_departments() -> Value {
    json!({"code":200,"data":{"items":[],"max_page_size":100,"page":1,
        "page_size":100,"total":0,"total_pages":0}})
}

fn initialization(plan: &Value, identity: Value, quota: bool) -> Value {
    let tenants = plan["environment"]["tenants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tenant| tenant["tenant_id"].clone());
    let tenant_ids = if quota {
        std::iter::once(json!("system"))
            .chain(tenants)
            .collect::<Vec<_>>()
    } else {
        tenants.collect::<Vec<_>>()
    };
    let mut value = json!({
        "id":1,"operation":"initialize","backend":"fixture","frontend":plan["environment"]["frontend_dir"],
        "config":{"contract":{"timeout_ms":120000,"pacing":plan["environment"]["pacing"]["contract"]},
            "bindings":{"scope_id":plan["environment"]["scope_id"],"api_url":plan["environment"]["api_url"],
                "frontend_url":plan["environment"]["frontend_url"],"pacing":plan["environment"]["pacing"]["bindings"]}},
        "identity":identity,"artifacts":"fixture","run_dir":"fixture","attempt":1,
        "tenant_ids":tenant_ids,"producer_owner":"12345678-1234-4abc-8def-1234567890ab",
        "launch":{},"process":{}
    });
    if !quota {
        value["template_identities"] = json!(template_identities(plan));
    }
    value
}

fn template_identities(plan: &Value) -> Vec<Value> {
    plan["groups"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .enumerate()
        .map(|(index, group)| {
            let user = &group["users"][0];
            json!({"subject_id":(index + 10).to_string(),"tenant_id":group["tenant_id"],
                "username":user["username"],"credential_slot":user["credential_slot"],
                "client_address":user["client_address"]})
        })
        .collect()
}

fn system_identity(plan: &Value) -> Value {
    json!({"subject_id":"1","tenant_id":plan["environment"]["system_admin"]["tenant_id"],
        "username":plan["environment"]["system_admin"]["username"],
        "client_address":plan["environment"]["system_admin"]["client_address"]})
}

fn plan() -> Value {
    let tenants = (1..=10)
        .map(|index| {
            json!({"slot":format!("tenant-{index:02}"),"tenant_id":format!("tenant-{index}"),
            "admin":{"tenant_id":format!("tenant-{index}"),"username":"admin",
                "client_address":format!("198.18.30.{}", index + 1)}})
        })
        .collect::<Vec<_>>();
    let mut groups = vec![json!({"slot":"system","kind":"system","tenant_id":"system",
        "admin":{"tenant_id":"system","username":"admin","client_address":"198.18.30.1"},
        "permissions":[],"users":[]})];
    for (index, tenant) in tenants.iter().enumerate() {
        groups.push(
            json!({"slot":tenant["slot"],"kind":"tenant","tenant_id":tenant["tenant_id"],
            "admin":tenant["admin"],"permissions":["system:user-import:add"],
            "users":[{"user_slot":format!("tenant-{:02}-user-001",index + 1),
                "username":format!("user-{}",index + 1),"credential_slot":"tenant",
                "client_address":format!("198.18.40.{}",index + 1)}]}),
        );
    }
    json!({"environment":{"plan_id":"process","scope_id":"fixture","backend_dir":"fixture",
        "frontend_dir":"frontend","api_url":"http://127.0.0.1:18999/","frontend_url":"http://127.0.0.1:4199/",
        "system_admin":{"tenant_id":"system","username":"admin","client_address":"198.18.30.1"},
        "quota":{"system_max_users":200,"tenant_max_users":100},"tenants":tenants,
        "pacing":{"contract":{},"bindings":{}}},"groups":groups})
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
