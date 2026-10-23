use super::{
    evidence,
    execution::{Context, IdentityFuture, IdentitySession},
    inspection,
    plan::{self, text},
};
use crate::{
    Result,
    http_session::{RequestHook, RequestMetadata, Session},
    session_pacing::PreparationHook,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) struct NativeContext {
    plan: Value,
    config: Value,
    catalog: BTreeMap<String, Value>,
    artifacts: PathBuf,
    baseline: Option<Value>,
}
impl NativeContext {
    pub(super) fn new(plan: &Value, root: &Path, mode: &str) -> Result<Self> {
        if std::env::var("APP_ENV").ok().as_deref() != Some("test") {
            return Err("身份执行要求 APP_ENV=test 与已绑定的隔离配置文件".into());
        }
        let backend = Path::new(text(&plan["environment"], "backend_dir")?);
        let catalog = crate::devex::runtime_configuration::operation_catalog(
            &crate::recovery::clone::native::json::read(&backend.join("openapi/openapi.json"))?,
        )?;
        let environment = &plan["environment"];
        let config = json!({
            "contract": {
                "timeout_ms": 120000,
                "pacing": environment["pacing"]["contract"]
            },
            "bindings": {
                "scope_id": environment["scope_id"],
                "api_url": environment["api_url"],
                "frontend_url": environment["frontend_url"],
                "pacing": environment["pacing"]["bindings"]
            }
        });
        Ok(Self {
            plan: plan.clone(),
            config,
            catalog,
            artifacts: root.join(format!("{mode}-{}", uuid::Uuid::new_v4())),
            baseline: None,
        })
    }
}
impl Context for NativeContext {
    fn password<'a>(&'a mut self, slot: &'a str) -> IdentityFuture<'a, String> {
        Box::pin(async move {
            let private = plan::load(
                &self.plan,
                Path::new(text(&self.plan["environment"], "backend_dir")?),
            )?;
            Ok(text(&private["passwords"], slot)?.to_owned())
        })
    }
    fn inspect<'a>(&'a mut self, phase: &'a str) -> IdentityFuture<'a, Value> {
        Box::pin(async move {
            let plan = self.plan.clone();
            let phase = phase.to_owned();
            let (value, config_dir) = tokio::task::spawn_blocking(move || {
                inspection::inspect(&plan, &phase).map_err(|error| error.to_string())
            })
            .await
            .map_err(|_| "身份环境检查线程未完成")?
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
            if let Some(baseline) = &self.baseline {
                if baseline["scope_id"] != value["scope_id"]
                    || baseline["server_uuid"] != value["server_uuid"]
                    || baseline["api_process"] != value["api_process"]
                {
                    return Err("身份准备期间 API 创建身份或资源绑定变化".into());
                }
            } else {
                fs::create_dir(&self.artifacts)?;
                evidence::new(&self.artifacts.join("environment.json"), &value)?;
                self.baseline = Some(value.clone());
            }
            self.config["bindings"]["app_config_dir"] = json!(config_dir);
            Ok(value)
        })
    }
    fn session<'a>(
        &'a mut self,
        identity: &'a Value,
    ) -> IdentityFuture<'a, Box<dyn IdentitySession>> {
        Box::pin(async move {
            if self.baseline.is_none() {
                return Err("身份会话必须先通过资源 ownership 核验".into());
            }
            let private = plan::load(
                &self.plan,
                Path::new(text(&self.plan["environment"], "backend_dir")?),
            )?;
            let mut identity = identity.clone();
            if let Some(slot) = identity["credential_slot"].as_str() {
                identity["password"] = private["passwords"][slot].clone();
            } else {
                let admin = if identity["tenant_id"] == "system" {
                    &private["system_admin"]
                } else {
                    &private["tenants"]
                        .as_array()
                        .ok_or("缺少租户身份配置")?
                        .iter()
                        .find(|tenant| tenant["tenant_id"] == identity["tenant_id"])
                        .ok_or("身份未登记")?["admin"]
                };
                for key in ["tenant_id", "username", "client_address"] {
                    if admin[key] != identity[key] {
                        return Err("身份会话与私有文件绑定不同".into());
                    }
                }
                identity["password"] = admin["password"].clone();
            }
            if identity["password"].as_str().is_none_or(str::is_empty) {
                return Err("身份文件配置缺少明确密码".into());
            }
            let directory = self
                .artifacts
                .join(format!("session-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&directory)?;
            let session = Session::new(&self.config, self.catalog.clone(), &identity)?;
            let hook = PreparationHook::new(
                Path::new(text(&self.plan["environment"], "backend_dir")?),
                &self.config,
                &identity,
                &directory,
                "test",
            )
            .await?;
            Ok(Box::new(NativeSession {
                session,
                hook,
                identity,
                config: self.config.clone(),
                catalog: self.catalog.clone(),
                login_attempted: false,
                closed: false,
            }) as Box<dyn IdentitySession>)
        })
    }
}
struct NativeSession {
    session: Session,
    hook: PreparationHook,
    identity: Value,
    config: Value,
    catalog: BTreeMap<String, Value>,
    login_attempted: bool,
    closed: bool,
}
impl IdentitySession for NativeSession {
    fn login(&mut self) -> IdentityFuture<'_, ()> {
        Box::pin(async {
            self.login_attempted = true;
            self.session.login(&mut self.hook).await?;
            Ok(())
        })
    }
    fn request<'a>(&'a mut self, step: &'a Value) -> IdentityFuture<'a, Value> {
        Box::pin(async move { Ok(self.session.request(step, &mut self.hook).await?) })
    }
    fn template<'a>(&'a mut self, path: &'a Path) -> IdentityFuture<'a, Value> {
        Box::pin(async move {
            let operation = self
                .catalog
                .get("get_system_users_import_template")
                .ok_or("契约缺少模板下载")?;
            if operation["method"] != "GET" {
                return Err("模板下载必须为 GET".into());
            }
            let route = text(operation, "path")?;
            let metadata = RequestMetadata {
                operation: "get_system_users_import_template".into(),
                method: "GET".into(),
                route: route.into(),
            };
            let token = self.session.token().ok_or("模板下载缺少当前会话")?;
            self.hook.before(&metadata).await?;
            let result = download(&self.config, &self.identity, token, route, path).await;
            let completed = self.hook.complete().await;
            let digest = match (result, completed) {
                (Err(error), _) => return Err(error),
                (Ok(_), Err(error)) => return Err(error.into()),
                (Ok(value), Ok(())) => value,
            };
            Ok(serde_json::to_value(
                crate::devex::import_samples::template_receipt(path, &digest)?,
            )?)
        })
    }
    fn close(&mut self) -> IdentityFuture<'_, ()> {
        Box::pin(async {
            if self.closed {
                return Ok(());
            }
            self.closed = true;
            let result = if self.login_attempted || self.session.token().is_some() {
                tokio::time::timeout(
                    Duration::from_secs(10),
                    self.session
                        .request(&json!({"operation":"post_auth_logout"}), &mut self.hook),
                )
                .await
                .map_err(|_| -> Box<dyn std::error::Error> { "身份退出结果未知".into() })
                .and_then(|value| value.map(|_| ()).map_err(Into::into))
            } else {
                Ok(())
            };
            let closed = self.hook.close().await;
            result.and(closed.map_err(Into::into))?;
            Ok(())
        })
    }
}
async fn download(
    config: &Value,
    identity: &Value,
    token: &str,
    route: &str,
    path: &Path,
) -> Result<String> {
    let api = url::Url::parse(text(&config["bindings"], "api_url")?)?;
    let url = api.join(route)?;
    if url.origin() != api.origin() {
        return Err("模板下载不得跨原点".into());
    }
    let client = reqwest::Client::builder()
        .tls_backend_rustls()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(120))
        .build()?;
    let mut response = client
        .get(url)
        .bearer_auth(token)
        .header("X-Tenant-Id", text(identity, "tenant_id")?)
        .header("X-Forwarded-For", text(identity, "client_address")?)
        .send()
        .await
        .map_err(|_| "真实模板下载连接失败")?;
    if !response.status().is_success() {
        return Err("真实模板下载失败".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "模板响应读取失败")? {
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            return Err("模板超过 16 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(evidence::hash(bytes))
}
