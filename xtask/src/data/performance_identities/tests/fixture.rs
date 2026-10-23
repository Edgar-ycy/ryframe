use super::super::{
    evidence,
    execution::{Context, IdentityFuture, IdentitySession},
    plan,
};
use crate::Result;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

pub(super) struct Fixture {
    pub(super) directory: PathBuf,
    pub(super) plan: Value,
    pub(super) context: FakeContext,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let directory = backend
            .join(".local-tests")
            .join(format!("identity-native-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(directory.join("runtime")).unwrap();
        fs::write(directory.join("environment.md"), "独立测试").unwrap();
        for name in ["runtime.json", "api.json"] {
            fs::write(directory.join("runtime").join(name), "{}").unwrap();
        }
        let connection = |database| json!({"host":"127.0.0.1","port":3306,"username":"fixture","password":"Fixture-Database!123","database":database,"tls_mode":"required"});
        let admin = |tenant: &str, index| json!({"tenant_id":tenant,"username":"admin","password":"Fixture-Admin!123","client_address":format!("198.18.30.{index}")});
        let environment = json!({"format_version":1,"plan_id":"native","scope_id":"native-fixture","backend_dir":backend,"frontend_dir":backend.parent().unwrap().join("ryframe-vue3"),"api_url":"http://127.0.0.1:18999/","frontend_url":"http://127.0.0.1:4199/",
            "environment_document":{"path":directory.join("environment.md"),"sha256":evidence::hash("独立测试")},"runtime":{"directory":directory.join("runtime"),"receipt_sha256":evidence::hash("{}"),"api_process_sha256":evidence::hash("{}")},
            "database":{"python":backend.join("python-fixture"),"mysql_client":backend.join("mysql-fixture"),"server_uuid":"11111111-1111-1111-1111-111111111111","control":connection("identity_control"),"targets":[{"key":"shared","mode":"shared","connection":connection("identity_shared")}]},
            "system_admin":admin("system",1),"tenants":(1..=10).map(|index|json!({"slot":format!("tenant-{index:02}"),"tenant_id":format!("fixture-{index}"),"target_key":"shared","admin":admin(&format!("fixture-{index}"), index+1)})).collect::<Vec<_>>(),"passwords":{"system":"Fixture-User!123","tenant":"Fixture-User!123"},"quota":{"system_max_users":1000,"tenant_max_users":1000,"import_headroom_per_tenant":100},
            "pacing":{"contract":{"authority_sha256":"a".repeat(64),"request_interval_ms":1000,"operation_intervals_ms":{"post_auth_login":12250},"sample_prepare_wait_ms":60250},"bindings":{"python":backend.join("python-fixture"),"login_budget_state":directory.join("budget.json")}}});
        let source = directory.join("private.json");
        evidence::new(&source, &environment).unwrap();
        let plan = plan::create(&environment, &backend, &source).unwrap();
        let context = FakeContext {
            data: Rc::new(RefCell::new(FakeData::default())),
            plan: plan.clone(),
        };
        Self {
            directory,
            plan,
            context,
        }
    }
    pub(super) fn root(&self) -> PathBuf {
        self.directory.join("state")
    }
    pub(super) fn ledger(&self) -> Value {
        evidence::parse(&fs::read(self.root().join("ledger.json")).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[derive(Default)]
pub(super) struct FakeData {
    pub(super) roles: BTreeMap<String, Value>,
    pub(super) users: BTreeMap<String, Value>,
    pub(super) permissions: BTreeMap<String, Value>,
    pub(super) resets: BTreeMap<String, String>,
    pub(super) writes: usize,
    pub(super) sessions: usize,
    pub(super) closed: usize,
    pub(super) inspections: usize,
    pub(super) fail_inspection: Option<usize>,
    pub(super) drift_inspection: Option<usize>,
    pub(super) fail_operation: Option<String>,
    pub(super) fail_close_at: Option<usize>,
    pub(super) bad_fragment: bool,
    pub(super) bad_ordinary: bool,
    pub(super) different_template: bool,
}
pub(super) struct FakeContext {
    pub(super) data: Rc<RefCell<FakeData>>,
    pub(super) plan: Value,
}
impl Context for FakeContext {
    fn password<'a>(&'a mut self, slot: &'a str) -> IdentityFuture<'a, String> {
        Box::pin(async move {
            let private = plan::load(&self.plan, &crate::workspace::root_dir())?;
            Ok(private["passwords"][slot].as_str().unwrap().to_owned())
        })
    }
    fn inspect<'a>(&'a mut self, _: &'a str) -> IdentityFuture<'a, Value> {
        Box::pin(async move {
            let mut data = self.data.borrow_mut();
            data.inspections += 1;
            if data.fail_inspection == Some(data.inspections) {
                return Err("测试 ownership 失败".into());
            }
            Ok(
                json!({"scope_id":self.plan["environment"]["scope_id"],"server_uuid":self.plan["environment"]["database"]["server_uuid"],"api_process":{"generation":if data.drift_inspection==Some(data.inspections){2}else{1}}}),
            )
        })
    }
    fn session<'a>(
        &'a mut self,
        identity: &'a Value,
    ) -> IdentityFuture<'a, Box<dyn IdentitySession>> {
        Box::pin(async move {
            self.data.borrow_mut().sessions += 1;
            let group = self.plan["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|group| group["tenant_id"] == identity["tenant_id"])
                .unwrap()
                .clone();
            Ok(Box::new(FakeSession {
                data: self.data.clone(),
                identity: identity.clone(),
                group,
                closed: false,
            }) as Box<dyn IdentitySession>)
        })
    }
}
struct FakeSession {
    data: Rc<RefCell<FakeData>>,
    identity: Value,
    group: Value,
    closed: bool,
}
impl IdentitySession for FakeSession {
    fn login(&mut self) -> IdentityFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn request<'a>(&'a mut self, step: &'a Value) -> IdentityFuture<'a, Value> {
        Box::pin(async move { self.respond(step).map(|data| json!({"data":data})) })
    }
    fn template<'a>(&'a mut self, path: &'a Path) -> IdentityFuture<'a, Value> {
        Box::pin(async move {
            fs::write(path, b"isolated-template").unwrap();
            let department =
                if self.data.borrow().different_template && self.group["slot"] == "tenant-02" {
                    "不同部门"
                } else {
                    "总部/业务部"
                };
            Ok(
                json!({"template_sha256":evidence::hash("isolated-template"),"department_path":department,"department_sha256":evidence::hash(department)}),
            )
        })
    }
    fn template_data(&mut self) -> IdentityFuture<'_, Value> {
        Box::pin(async { Err("测试身份流程不读取内存模板".into()) })
    }
    fn token(&self) -> Option<&str> {
        Some("fixture-token")
    }
    fn close(&mut self) -> IdentityFuture<'_, ()> {
        Box::pin(async {
            assert!(!self.closed);
            self.closed = true;
            let mut data = self.data.borrow_mut();
            data.closed += 1;
            if data.fail_close_at == Some(data.closed) {
                return Err("测试 close 证据失败".into());
            }
            Ok(())
        })
    }
}
impl FakeSession {
    fn respond(&self, step: &Value) -> Result<Value> {
        let mut data = self.data.borrow_mut();
        let operation = step["operation"].as_str().unwrap();
        let id = step["path"]["id"].as_str().unwrap_or("");
        let tenant = self.group["tenant_id"].as_str().unwrap();
        let result = match operation {
            "get_auth_csrf" => json!({}),
            "get_auth_context" => self.auth(&data)?,
            "get_system_roles" => json!({"total":data.roles.values().filter(|value|value["tenant_id"]==tenant).count()}),
            "get_system_users" => json!({"total":data.users.values().filter(|value|value["tenant_id"]==tenant).count()}),
            "get_system_perms_tree" => json!(self.group["permissions"].as_array().unwrap().iter().enumerate().map(|(index,code)|json!({"id":format!("{}",10000+index),"code":code,"status":"1","children":[]})).collect::<Vec<_>>()),
            "get_system_roles_by_id" => data.roles.get(id).ok_or("测试角色缺失")?.clone(),
            "get_system_roles_by_id_permissions" => data.permissions.get(id).ok_or("测试权限缺失")?.clone(),
            "get_system_users_by_id" => data.users.get(id).ok_or("测试用户缺失")?.clone(),
            _ => self.mutate(&mut data, step, tenant)?,
        };
        if data.fail_operation.as_deref() == Some(operation) {
            data.fail_operation = None;
            return Err("测试响应丢失".into());
        }
        Ok(result)
    }
    fn auth(&self, data: &FakeData) -> Result<Value> {
        if self.identity["username"] == "admin" {
            return Ok(
                json!({"user":self.identity,"password_change_required":false,"is_super_admin":true}),
            );
        }
        let user = data
            .users
            .values()
            .find(|value| {
                value["username"] == self.identity["username"]
                    && value["tenant_id"] == self.identity["tenant_id"]
            })
            .ok_or("测试登录用户缺失")?;
        Ok(
            json!({"user":user,"password_change_required":user["password_change_required"],"is_super_admin":data.bad_ordinary,"roles":[self.group["role_code"]],"permissions":self.group["permissions"]}),
        )
    }
    fn mutate(&self, data: &mut FakeData, step: &Value, tenant: &str) -> Result<Value> {
        data.writes += 1;
        let id = format!("{}", data.writes + 100);
        let target = step["path"]["id"].as_str().unwrap_or("");
        match step["operation"].as_str().unwrap() {
            "post_system_roles" => {
                let value = json!({"id":id,"tenant_id":tenant,"code":step["body"]["code"],"is_super":0,"status":"1","data_scope":"1"});
                data.roles.insert(id, value.clone());
                Ok(value)
            }
            "put_system_roles_by_id_permissions" => {
                data.permissions
                    .insert(target.into(), step["body"]["perm_ids"].clone());
                Ok(Value::Null)
            }
            "post_system_users" => {
                let value = json!({"id":id,"tenant_id":tenant,"username":step["body"]["username"],"status":"1","password_change_required":true,"roles":[{"id":step["body"]["role_ids"][0],"is_super":0}]});
                data.users.insert(id, value.clone());
                Ok(value)
            }
            "post_system_users_by_id_password_reset_requests" => {
                data.resets.insert(id.clone(), target.into());
                Ok(
                    json!({"request_id":id,"expires_at":"2099-01-01T00:00:00Z","reset_url":format!("http://127.0.0.1:4199/reset-password#request_id={id}&tenant_id={tenant}&token=fixture-secret{}",if data.bad_fragment{"&extra=invalid"}else{""})}),
                )
            }
            "post_auth_password_reset_complete" => {
                let request = step["body"]["request_id"].as_str().unwrap();
                let user = data.resets.get(request).ok_or("测试重置请求缺失")?.clone();
                data.users.get_mut(&user).unwrap()["password_change_required"] = json!(false);
                Ok(Value::Null)
            }
            _ => Err("测试遇到未声明操作".into()),
        }
    }
}
