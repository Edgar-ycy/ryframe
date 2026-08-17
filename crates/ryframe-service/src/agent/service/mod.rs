use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use ryframe_auth::rbac;
use ryframe_config::{MultiTenancyConfig, PepperKeyring, ServiceAccountsConfig};
use ryframe_core::RedisClient;
use ryframe_db::{
    AgentQueryRepository, AgentRowScope, ControlDatabaseCluster, DataRetentionRepository,
    ServiceAccessAuditRepository, ServiceAccountLock, ServiceAccountRepository,
    ServiceAuthorizationRepository, ServiceAuthorizationSnapshot, ServiceCredentialRepository,
    ServiceDelegationRepository,
    entities::{
        role, service_access_audit, service_account, service_credential, service_delegation,
    },
};
use ryframe_kernel::{AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::TransactionTrait;
use serde::Serialize;

use super::{
    AgentAccessMode, AgentCapability, AgentCapabilityVo, AgentDepartmentVo, AgentDictionaryItemVo,
    AgentDictionaryVo, AgentPage, AgentPostVo, AgentPrincipal, AgentRequest, AgentSuccess,
    AgentUserVo,
    limiter::{AgentLimitInput, AgentLimiter},
    registry::AgentCapabilityDescriptor,
    scope::{
        SubjectScope, departments_scope, resolve_account_scope, resolve_user_scope, users_scope,
    },
};
use crate::service_identity_secret::{
    IP_DIGEST_DOMAIN, ParsedApiKey, ParsedDelegation, USER_AGENT_DIGEST_DOMAIN, invalid_credential,
    keyed_hash, parse_authorization, parse_delegation,
};
use crate::system::{ProductService, SERVICE_ACCOUNTS_CAPABILITY};

const ACCESS_MODE_UNKNOWN: &str = "unknown";
const RESULT_DENIED: &str = service_access_audit::Model::RESULT_DENIED;
const RESULT_ERROR: &str = service_access_audit::Model::RESULT_ERROR;

#[derive(Clone)]
pub struct AgentService {
    db: ControlDatabaseCluster,
    config: ServiceAccountsConfig,
    multi_tenancy: MultiTenancyConfig,
    keyring: Arc<PepperKeyring>,
    limiter: AgentLimiter,
    product_service: Arc<ProductService>,
}

pub(super) struct IdentityHint {
    pub(super) credential: service_credential::Model,
    pub(super) delegation: Option<service_delegation::Model>,
}

pub(super) struct AuthorizedContext {
    pub(super) tenant: ryframe_db::tenant::Model,
    pub(super) account: service_account::Model,
    pub(super) credential: service_credential::Model,
    delegation: Option<service_delegation::Model>,
    pub(super) snapshot: ServiceAuthorizationSnapshot,
    pub(super) delegation_capabilities: BTreeSet<String>,
    pub(super) account_permissions: Vec<String>,
    pub(super) user_permissions: Vec<String>,
    pub(super) account_scope: SubjectScope,
    pub(super) user_scope: Option<SubjectScope>,
}

mod audit;
mod authorization;
mod pipeline;
mod query;
mod response;
mod support;

use audit::*;
use query::*;
use response::*;
use support::*;

impl AgentService {
    pub fn new(
        db: ControlDatabaseCluster,
        redis: RedisClient,
        keyring: Arc<PepperKeyring>,
        config: ServiceAccountsConfig,
        multi_tenancy: MultiTenancyConfig,
        product_service: Arc<ProductService>,
    ) -> AppResult<Self> {
        config.validate().map_err(AppError::Config)?;
        if !config.enabled {
            return Err(AppError::Config("服务账号功能未启用".into()));
        }
        Ok(Self {
            db,
            config,
            multi_tenancy,
            keyring,
            limiter: AgentLimiter::new(redis),
            product_service,
        })
    }

    /// 执行固定能力。成功数据只有在查询与审计所在事务提交后才会返回。
    pub async fn execute(&self, request: AgentRequest) -> AppResult<AgentSuccess> {
        self.execute_inner(&request).await
    }

    /// 对未命中固定注册表的 Agent 路径执行最小审计。HTTP fallback 不得直接返回未审计的 404。
    pub async fn audit_unregistered(
        &self,
        request_id: String,
        client_ip: std::net::IpAddr,
        user_agent: Option<String>,
        started_at: DateTime<Utc>,
    ) -> AppResult<()> {
        let request = AgentRequest {
            capability: AgentCapability::Capabilities,
            authorization: None,
            delegation: None,
            page: 1,
            page_size: 1,
            type_code: None,
            request_id,
            client_ip,
            user_agent,
            success_message: String::new(),
            started_at,
            validation_error: None,
        };
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.config.query_timeout_ms);
        if let Err(error) = before_deadline(
            deadline,
            self.limiter.guard_pre_auth_ip(
                &request.client_ip.to_string(),
                self.config.default_requests_per_minute,
            ),
        )
        .await
        {
            let (status, reason) = match error {
                AppError::RateLimited(_, _) => (429, "rate_limited"),
                _ => (503, "rate_limit_unavailable"),
            };
            self.audit_failure_bounded(&request, None, RESULT_DENIED, reason, status)
                .await?;
            return Err(error);
        }
        self.audit_failure_bounded(&request, None, RESULT_DENIED, "route_not_registered", 404)
            .await
    }
}
