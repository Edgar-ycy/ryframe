//! 受限 JSON HTTP 会话；授权、进程控制和请求节奏由调用方注入。

use std::{collections::BTreeMap, time::Duration};

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
use preparation::{Configuration, FORCE_LOGOUT_OPERATION, Plan};

#[cfg(test)]
#[path = "../tests/internal/http_session.rs"]
mod tests;

pub(crate) struct RequestMetadata {
    pub operation: String,
    pub method: String,
    pub route: String,
}

#[async_trait::async_trait]
pub trait RequestHook: Send {
    async fn before(&mut self, metadata: &RequestMetadata) -> Result<(), SessionError>;
    async fn complete(&mut self) -> Result<(), SessionError>;
}

#[cfg(test)]
pub(crate) struct NoopHook;

#[cfg(test)]
#[async_trait::async_trait]
impl RequestHook for NoopHook {
    async fn before(&mut self, _metadata: &RequestMetadata) -> Result<(), SessionError> {
        Ok(())
    }

    async fn complete(&mut self) -> Result<(), SessionError> {
        Ok(())
    }
}

pub(crate) struct Session {
    client: Client,
    configuration: Configuration,
    cookies: BTreeMap<String, String>,
    access_token: Option<String>,
}

/// 仅记录本次 DELETE 的发送边界与服务端响应，不包含会话凭据或响应正文。
#[derive(Debug)]
pub(crate) struct ForceLogoutOutcome {
    pub(crate) result: Result<(), SessionError>,
    pub(crate) request_started: bool,
    pub(crate) status: Option<u16>,
    pub(crate) request_id: Option<String>,
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
        self.perform(csrf, hook, None).await?;
        let result = self.perform(plan, hook, None).await?;
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
            self.perform(csrf, hook, None).await?;
        }
        self.perform(plan, hook, None).await
    }

    pub(crate) async fn force_logout_online_session(
        &mut self,
        sid: &str,
        idempotency_key: &str,
        hook: &mut dyn RequestHook,
    ) -> ForceLogoutOutcome {
        let mut outcome = ForceLogoutOutcome {
            result: Ok(()),
            request_started: false,
            status: None,
            request_id: None,
        };
        let result = async {
            let plan = self
                .configuration
                .prepare_force_logout(sid, idempotency_key)?;
            if self.access_token.is_none() {
                return Err(SessionError::new(
                    "missing_credentials",
                    Some(FORCE_LOGOUT_OPERATION),
                    None,
                ));
            }
            let csrf = self
                .configuration
                .prepare(&json!({"operation": "get_auth_csrf"}))?;
            self.perform(csrf, hook, None).await?;
            let reply = self.perform(plan, hook, Some(&mut outcome)).await?;
            if outcome.status != Some(200)
                || outcome.request_id.is_none()
                || reply["code"].as_u64() != Some(200)
            {
                return Err(SessionError::new(
                    "invalid_response",
                    Some(FORCE_LOGOUT_OPERATION),
                    outcome.status,
                ));
            }
            Ok(())
        }
        .await;
        outcome.result = result;
        outcome
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
        observation: Option<&mut ForceLogoutOutcome>,
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
            .exchange(request, operation, self.configuration.timeout, observation)
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
        mut observation: Option<&mut ForceLogoutOutcome>,
    ) -> Result<Value, SessionError> {
        tokio::time::timeout(timeout, async {
            if let Some(observation) = observation.as_mut() {
                observation.request_started = true;
            }
            let reply = self.client.execute(request).await.map_err(|error| {
                let category = if error.is_timeout() {
                    "request_timeout"
                } else {
                    "request_failed"
                };
                SessionError::new(category, Some(operation), None)
            })?;
            if let Some(observation) = observation {
                observation.status = Some(reply.status().as_u16());
                observation.request_id = response::request_id(reply.headers());
            }
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
