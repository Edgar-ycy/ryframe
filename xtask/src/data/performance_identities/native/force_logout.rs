use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Context, IdentitySession, NativeContext, NativeSession, VerifiedApi, plan, text};
use crate::{
    Result,
    data::performance_identities::{inspection::ApiProofReader, state},
    http_session::{RequestHook, RequestMetadata, SessionError},
};

const OPERATION: &str = "delete_system_online_by_sid";
const PERMISSION: &str = "monitor:online:force-logout";

pub(crate) struct AdminForceLogout<'a> {
    pub(crate) coordinator: &'a Path,
    pub(crate) run: &'a Path,
    pub(crate) attempt: u64,
    pub(crate) plan: &'a Value,
    pub(crate) artifacts: PathBuf,
    pub(crate) administrator: &'a Value,
    pub(crate) sid: &'a str,
    pub(crate) idempotency_key: &'a str,
}

#[derive(Debug, Default)]
pub(crate) struct AdminForceLogoutOutcome {
    pub(crate) admin_login_confirmed: bool,
    pub(crate) request_started: bool,
    pub(crate) status: Option<u16>,
    pub(crate) request_id: Option<String>,
    pub(crate) operation_confirmed: bool,
    pub(crate) admin_session_closed: bool,
    pub(crate) failure: Option<&'static str>,
}

impl AdminForceLogoutOutcome {
    fn failed(failure: &'static str) -> Self {
        Self {
            failure: Some(failure),
            ..Self::default()
        }
    }
}

pub(crate) async fn force_logout_as_system_admin(
    request: AdminForceLogout<'_>,
    api_proof: ApiProofReader,
    mut before_delete: impl FnMut() -> Result<()> + Send,
) -> Result<AdminForceLogoutOutcome> {
    let original_admin = &request.plan["environment"]["system_admin"];
    if request.administrator["tenant_id"] != "system"
        || state::id(&request.administrator["subject_id"]).is_err()
        || ["tenant_id", "username", "client_address"]
            .into_iter()
            .any(|key| request.administrator[key] != original_admin[key])
    {
        return Err("强制下线管理员与原身份计划不同".into());
    }
    let product = Path::new(text(&request.plan["environment"], "backend_dir")?);
    let environment =
        plan::load(request.plan, product).map_err(|_| "强制下线管理员私有配置绑定无效")?;
    let api = VerifiedApi::select(
        request.coordinator,
        request.run,
        request.attempt,
        &environment,
        api_proof,
    )
    .map_err(|_| "强制下线当前 API 证明无效")?
    .ok_or("强制下线缺少当前 API 证明")?;
    let mut context = NativeContext::with_api(request.plan, request.artifacts, Some(api.clone()))
        .map_err(|_| "强制下线身份会话配置无效")?;
    context
        .inspect("verify")
        .await
        .map_err(|_| "强制下线身份资源绑定检查失败")?;
    let mut administrator = context
        .bound_session(original_admin)
        .await
        .map_err(|_| "强制下线管理员会话准备失败")?;
    let mut outcome = delete(
        &mut administrator,
        request.administrator,
        request.sid,
        request.idempotency_key,
        || {
            api.observe(&environment)?;
            before_delete()
        },
    )
    .await;
    let close_confirmed = administrator.close().await.is_ok();
    // 登录失响时即使退出调用返回成功，仍无法证明原登录没有留下会话。
    outcome.admin_session_closed = outcome.admin_login_confirmed && close_confirmed;
    Ok(outcome)
}

async fn delete(
    session: &mut NativeSession,
    administrator: &Value,
    sid: &str,
    idempotency_key: &str,
    before_delete: impl FnMut() -> Result<()> + Send,
) -> AdminForceLogoutOutcome {
    if session.login().await.is_err() {
        return AdminForceLogoutOutcome::failed("admin_login_failed");
    }
    let failure = match session
        .request(&json!({"operation":"get_auth_context"}))
        .await
    {
        Ok(response) if authorized(&response, administrator) => None,
        Ok(_) => Some("admin_authorization_mismatch"),
        Err(_) => Some("admin_authorization_read_failed"),
    };
    if let Some(failure) = failure {
        return AdminForceLogoutOutcome {
            admin_login_confirmed: true,
            ..AdminForceLogoutOutcome::failed(failure)
        };
    }
    let mut hook = DeleteGuard {
        pacing: &mut session.hook,
        before_delete,
        entered: false,
        rejected: false,
    };
    let outcome = session
        .session
        .force_logout_online_session(sid, idempotency_key, &mut hook)
        .await;
    AdminForceLogoutOutcome {
        admin_login_confirmed: true,
        request_started: outcome.request_started,
        status: outcome.status,
        request_id: outcome.request_id,
        operation_confirmed: outcome.result.is_ok(),
        admin_session_closed: false,
        failure: if hook.rejected {
            Some("delete_guard_failed")
        } else {
            outcome.result.err().map(|error| error.category)
        },
    }
}

fn authorized(response: &Value, administrator: &Value) -> bool {
    let context = &response["data"];
    let user = &context["user"];
    let Some(permissions) = context["permissions"].as_array() else {
        return false;
    };
    user["id"] == administrator["subject_id"]
        && user["tenant_id"] == "system"
        && user["username"] == administrator["username"]
        && context["password_change_required"] == false
        && context["is_super_admin"].is_boolean()
        && permissions.iter().all(Value::is_string)
        && (context["is_super_admin"] == true
            || permissions.iter().any(|value| value == PERMISSION))
}

struct DeleteGuard<'a, F> {
    pacing: &'a mut dyn RequestHook,
    before_delete: F,
    entered: bool,
    rejected: bool,
}

#[async_trait::async_trait]
impl<F: FnMut() -> Result<()> + Send> RequestHook for DeleteGuard<'_, F> {
    async fn before(
        &mut self,
        metadata: &RequestMetadata,
    ) -> std::result::Result<(), SessionError> {
        self.pacing.before(metadata).await?;
        if metadata.operation == OPERATION {
            let duplicate = std::mem::replace(&mut self.entered, true);
            if duplicate || (self.before_delete)().is_err() {
                self.rejected = true;
                return Err(SessionError {
                    category: "hook_before",
                    operation: Some(OPERATION.into()),
                    status: None,
                });
            }
        }
        Ok(())
    }

    async fn complete(&mut self) -> std::result::Result<(), SessionError> {
        self.pacing.complete().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_session::NoopHook;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn force_logout_requires_the_bound_system_administrator() {
        let administrator = json!({"subject_id":"1","username":"fixture-admin"});
        let mut response = json!({"data":{
            "user":{"id":"1","tenant_id":"system","username":"fixture-admin"},
            "password_change_required":false,"is_super_admin":false,"permissions":[PERMISSION]
        }});
        assert!(authorized(&response, &administrator));
        response["data"]["permissions"] = json!(["monitor:online:list"]);
        assert!(!authorized(&response, &administrator));
        response["data"]["is_super_admin"] = json!(true);
        assert!(authorized(&response, &administrator));
        for field in ["id", "tenant_id", "username"] {
            let mut changed = response.clone();
            changed["data"]["user"][field] = json!("wrong");
            assert!(!authorized(&changed, &administrator));
        }
        response["data"]["password_change_required"] = Value::Null;
        assert!(!authorized(&response, &administrator));
    }

    #[tokio::test]
    async fn only_delete_calls_the_guard_and_guard_errors_stay_private() {
        let calls = AtomicUsize::new(0);
        let mut pacing = NoopHook;
        let mut guard = DeleteGuard {
            pacing: &mut pacing,
            before_delete: || {
                calls.fetch_add(1, Ordering::Relaxed);
                Err("private-receipt-details".into())
            },
            entered: false,
            rejected: false,
        };
        let mut metadata = RequestMetadata {
            operation: "get_auth_csrf".into(),
            method: "GET".into(),
            route: "/api/v1/auth/csrf".into(),
        };
        guard.before(&metadata).await.unwrap();
        guard.complete().await.unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        metadata.operation = OPERATION.into();
        metadata.method = "DELETE".into();
        metadata.route = "/api/v1/system/online/{sid}".into();
        let error = guard.before(&metadata).await.unwrap_err();
        assert!(!error.to_string().contains("private-receipt-details"));
        assert!(guard.rejected);
        assert!(guard.before(&metadata).await.is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
