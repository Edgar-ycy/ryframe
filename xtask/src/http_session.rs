//! 受限 JSON HTTP 会话；授权、进程控制和请求节奏由调用方注入。

use std::{collections::BTreeMap, future::Future, pin::Pin, time::Duration};

use reqwest::{Client, Request};
use serde_json::{Value, json};

#[path = "http_session/error.rs"]
mod error;
#[path = "http_session/preparation.rs"]
mod preparation;
#[path = "http_session/response.rs"]
mod response;
#[path = "http_session/tls.rs"]
mod tls;

pub(crate) use error::SessionError;
pub(crate) fn install_crypto_provider() -> Result<(), SessionError> {
    tls::install()
}
use preparation::{Configuration, Plan};

#[cfg(test)]
#[path = "../tests/internal/http_session.rs"]
mod tests;

pub(crate) struct RequestMetadata {
    pub operation: String,
    pub method: String,
    pub route: String,
}

pub type HookFuture<'a> = Pin<Box<dyn Future<Output = Result<(), SessionError>> + Send + 'a>>;

pub trait RequestHook: Send {
    fn before<'a>(&'a mut self, metadata: &'a RequestMetadata) -> HookFuture<'a>;
    fn complete<'a>(&'a mut self) -> HookFuture<'a>;
}

#[cfg(test)]
pub(crate) struct NoopHook;

#[cfg(test)]
impl RequestHook for NoopHook {
    fn before<'a>(&'a mut self, _metadata: &'a RequestMetadata) -> HookFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn complete<'a>(&'a mut self) -> HookFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

pub(crate) struct Session {
    client: Client,
    configuration: Configuration,
    cookies: BTreeMap<String, String>,
    access_token: Option<String>,
}

impl Session {
    pub(crate) fn new(
        config: &Value,
        catalog: BTreeMap<String, Value>,
        identity: &Value,
    ) -> Result<Self, SessionError> {
        let configuration = Configuration::new(config, catalog, identity)?;
        tls::install()?;
        let client = Client::builder()
            .tls_backend_rustls()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .referer(false)
            .timeout(configuration.timeout)
            .connect_timeout(configuration.timeout)
            .build()
            .map_err(|_| SessionError::new("client_configuration", None, None))?;
        Ok(Self {
            client,
            configuration,
            cookies: BTreeMap::new(),
            access_token: None,
        })
    }

    pub(crate) async fn login(&mut self, hook: &mut dyn RequestHook) -> Result<(), SessionError> {
        let password = self
            .configuration
            .password
            .clone()
            .ok_or_else(|| SessionError::new("missing_credentials", None, None))?;
        let step = json!({
            "operation": "post_auth_login",
            "body": {"username": self.configuration.username, "password": password},
        });
        // 先核验登录请求，避免不完整契约产生多余的匿名挑战请求。
        let plan = self.configuration.prepare(&step)?;
        self.access_token = None;
        self.cookies.clear();
        let csrf = self
            .configuration
            .prepare(&json!({"operation": "get_auth_csrf"}))?;
        self.perform(csrf, hook).await?;
        let result = self.perform(plan, hook).await?;
        self.access_token = Some(response::token(&result, "post_auth_login")?);
        Ok(())
    }

    pub(crate) async fn request(
        &mut self,
        step: &Value,
        hook: &mut dyn RequestHook,
    ) -> Result<Value, SessionError> {
        let plan = self.configuration.prepare(step)?;
        if matches!(
            plan.metadata.operation.as_str(),
            "post_auth_refresh" | "post_auth_logout"
        ) {
            let csrf = self
                .configuration
                .prepare(&json!({"operation": "get_auth_csrf"}))?;
            self.perform(csrf, hook).await?;
        }
        self.perform(plan, hook).await
    }

    pub(crate) fn token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    pub(crate) fn clear_access_token(&mut self) {
        self.access_token = None;
    }

    async fn perform(
        &mut self,
        plan: Plan,
        hook: &mut dyn RequestHook,
    ) -> Result<Value, SessionError> {
        let request = self.configuration.request(
            &self.client,
            &plan,
            &self.cookies,
            self.access_token.as_deref(),
        )?;
        let operation = plan.metadata.operation.as_str();
        hook.before(&plan.metadata)
            .await
            .map_err(|error| SessionError::new("hook_before", Some(operation), error.status))?;
        let result = self
            .exchange(request, operation, self.configuration.timeout)
            .await;
        let completion = hook
            .complete()
            .await
            .map_err(|error| SessionError::new("hook_complete", Some(operation), error.status));
        // 两侧均失败时保留原 HTTP 失败；完成回调仍恰好执行一次。
        match (result, completion) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    async fn exchange(
        &mut self,
        request: Request,
        operation: &str,
        timeout: Duration,
    ) -> Result<Value, SessionError> {
        tokio::time::timeout(timeout, async {
            let reply = self.client.execute(request).await.map_err(|error| {
                let category = if error.is_timeout() {
                    "request_timeout"
                } else {
                    "request_failed"
                };
                SessionError::new(category, Some(operation), None)
            })?;
            response::cookies(reply.headers(), &mut self.cookies, operation)?;
            let value = response::json(reply, operation).await?;
            if operation == "post_auth_refresh" {
                self.access_token = Some(response::token(&value, operation)?);
            }
            Ok(value)
        })
        .await
        .map_err(|_| SessionError::new("request_timeout", Some(operation), None))?
    }
}
