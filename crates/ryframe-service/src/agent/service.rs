use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use ryframe_auth::rbac;
use ryframe_config::{PepperKeyring, ServiceAccountsConfig};
use ryframe_core::RedisClient;
use ryframe_db::{
    AgentQueryRepository, AgentRowScope, DataRetentionRepository, DatabaseCluster,
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

const ACCESS_MODE_UNKNOWN: &str = "unknown";
const RESULT_DENIED: &str = service_access_audit::Model::RESULT_DENIED;
const RESULT_ERROR: &str = service_access_audit::Model::RESULT_ERROR;

#[derive(Clone)]
pub struct AgentService {
    db: DatabaseCluster,
    config: ServiceAccountsConfig,
    keyring: Arc<PepperKeyring>,
    limiter: AgentLimiter,
}

struct IdentityHint {
    credential: service_credential::Model,
    delegation: Option<service_delegation::Model>,
}

struct AuthorizedContext {
    tenant: ryframe_db::tenant::Model,
    account: service_account::Model,
    credential: service_credential::Model,
    delegation: Option<service_delegation::Model>,
    snapshot: ServiceAuthorizationSnapshot,
    delegation_capabilities: BTreeSet<String>,
    account_permissions: Vec<String>,
    user_permissions: Vec<String>,
    account_scope: SubjectScope,
    user_scope: Option<SubjectScope>,
}

impl AgentService {
    pub fn new(
        db: DatabaseCluster,
        redis: RedisClient,
        keyring: Arc<PepperKeyring>,
        config: ServiceAccountsConfig,
    ) -> AppResult<Self> {
        config.validate().map_err(AppError::Config)?;
        if !config.enabled {
            return Err(AppError::Config("服务账号功能未启用".into()));
        }
        Ok(Self {
            db,
            config,
            keyring,
            limiter: AgentLimiter::new(redis),
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

    async fn execute_inner(&self, request: &AgentRequest) -> AppResult<AgentSuccess> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.config.query_timeout_ms);
        let descriptor = request.capability.descriptor();
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
            self.audit_failure_bounded(request, None, RESULT_DENIED, reason, status)
                .await?;
            return Err(error);
        }
        if uuid::Uuid::parse_str(&request.request_id).is_err() {
            let error = AppError::Validation("请求 ID 无效".into());
            self.audit_failure_bounded(request, None, RESULT_DENIED, "validation", 400)
                .await?;
            return Err(error);
        }
        if let Some(message) = request.validation_error.as_ref() {
            self.audit_failure_bounded(request, None, RESULT_DENIED, "validation", 400)
                .await?;
            return Err(AppError::Validation(message.clone()));
        }
        if request.page == 0
            || request.page_size == 0
            || request.page_size > self.config.max_page_size
        {
            self.audit_failure_bounded(request, None, RESULT_DENIED, "validation", 400)
                .await?;
            return Err(AppError::Validation("Agent 分页参数超出允许范围".into()));
        }
        let parsed_key = match parse_authorization(request.authorization.as_deref()) {
            Ok(value) => value,
            Err(error) => {
                self.audit_failure_bounded(request, None, RESULT_DENIED, "invalid_credential", 401)
                    .await?;
                return Err(error);
            }
        };
        let parsed_delegation = match parse_delegation(request.delegation.as_deref()) {
            Ok(value) => value,
            Err(error) => {
                self.audit_failure_bounded(request, None, RESULT_DENIED, "invalid_credential", 401)
                    .await?;
                return Err(error);
            }
        };
        let hint = match before_deadline(
            deadline,
            self.identity_hint(&parsed_key, parsed_delegation.as_ref()),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                let (status, reason, result) = classify_pre_authorization_error(&error);
                self.audit_failure_bounded(request, None, result, reason, status)
                    .await?;
                return Err(error);
            }
        };
        let (tenant_limit, account_limit) =
            match before_deadline(deadline, self.limit_hints(&hint)).await {
                Ok(limits) => limits,
                Err(error) => {
                    let (status, reason, result) = classify_pre_authorization_error(&error);
                    self.audit_failure_bounded(request, Some(&hint), result, reason, status)
                        .await?;
                    return Err(error);
                }
            };
        let lease = match before_deadline(
            deadline,
            self.limiter.acquire(AgentLimitInput {
                ip: &request.client_ip.to_string(),
                tenant_id: &hint.credential.tenant_id,
                tenant_limit,
                account_id: hint.credential.account_id,
                account_limit,
                credential_id: hint.credential.id,
                represented_user_id: hint.delegation.as_ref().map(|item| item.user_id),
                capability_key: descriptor.key,
                capability_cost: descriptor.cost,
                default_limit: self.config.default_requests_per_minute,
                concurrency_limit: self.config.max_concurrent_queries,
                concurrency_ttl_ms: self.config.query_timeout_ms.saturating_add(1_000),
                owner: &request.request_id,
            }),
        )
        .await
        {
            Ok(lease) => lease,
            Err(error) => {
                let (status, reason) = match error {
                    AppError::RateLimited(_, _) => (429, "rate_limited"),
                    _ => (503, "rate_limit_unavailable"),
                };
                self.audit_failure_bounded(request, Some(&hint), RESULT_DENIED, reason, status)
                    .await?;
                return Err(error);
            }
        };
        let result = before_deadline(
            deadline,
            self.execute_locked(
                request,
                descriptor,
                &parsed_key,
                parsed_delegation.as_ref(),
                &hint,
            ),
        )
        .await;
        // 释放只用于提前回收并发槽位；请求主预算不能被 Redis 客户端超时配置延长。
        // 独立短任务失败时由租约 TTL 安全回收，且绝不覆盖已经确定的业务结果。
        std::mem::drop(tokio::spawn(async move {
            if tokio::time::timeout(Duration::from_millis(250), lease.release())
                .await
                .is_err()
            {
                tracing::warn!("Agent 并发租约释放超时，将由 TTL 自动回收");
            }
        }));
        match result {
            Ok(success) => Ok(success),
            Err(error) => {
                let (status, reason, result) = classify_error(&error);
                self.audit_failure_bounded(request, Some(&hint), result, reason, status)
                    .await?;
                Err(error)
            }
        }
    }

    async fn limit_hints(&self, hint: &IdentityHint) -> AppResult<(i32, i32)> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let tenant = ServiceAccountRepository
                .lock_tenant_in_txn(
                    &transaction,
                    &hint.credential.tenant_id,
                    ServiceAccountLock::Share,
                )
                .await?;
            let account = ServiceAccountRepository
                .find_by_id_in_txn(
                    &transaction,
                    &tenant.tenant_id,
                    hint.credential.account_id,
                    ServiceAccountLock::Share,
                )
                .await?;
            Ok::<_, AppError>((
                tenant.max_requests_per_min,
                account.map_or_else(
                    || i32::try_from(self.config.default_requests_per_minute).unwrap_or(i32::MAX),
                    |item| item.max_requests_per_minute,
                ),
            ))
        }
        .await;
        let _ = transaction.rollback().await;
        result
    }

    async fn identity_hint(
        &self,
        parsed_key: &ParsedApiKey,
        parsed_delegation: Option<&ParsedDelegation>,
    ) -> AppResult<IdentityHint> {
        let credential = ServiceCredentialRepository
            .find_hint_by_key_id(self.db.write(), &parsed_key.key_id)
            .await?
            .ok_or_else(invalid_credential)?;
        let delegation = if let Some(parsed) = parsed_delegation {
            let candidates = self
                .keyring
                .iter()
                .map(|(_, pepper)| parsed.mac(pepper))
                .collect::<AppResult<Vec<_>>>()?;
            Some(
                ServiceDelegationRepository
                    .find_by_mac_candidates(self.db.write(), &candidates)
                    .await?
                    .ok_or_else(invalid_credential)?,
            )
        } else {
            None
        };
        Ok(IdentityHint {
            credential,
            delegation,
        })
    }

    async fn execute_locked(
        &self,
        request: &AgentRequest,
        descriptor: &AgentCapabilityDescriptor,
        parsed_key: &ParsedApiKey,
        parsed_delegation: Option<&ParsedDelegation>,
        hint: &IdentityHint,
    ) -> AppResult<AgentSuccess> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let tenant = ServiceAccountRepository
                .lock_tenant_in_txn(
                    &transaction,
                    &hint.credential.tenant_id,
                    ServiceAccountLock::Share,
                )
                .await
                .map_err(mask_missing_identity)?;
            let now = DataRetentionRepository
                .database_utc_now(&transaction)
                .await?;
            if !tenant.is_available(now) {
                return Err(invalid_credential());
            }
            let account = ServiceAccountRepository
                .find_by_id_in_txn(
                    &transaction,
                    &tenant.tenant_id,
                    hint.credential.account_id,
                    ServiceAccountLock::Share,
                )
                .await?
                .filter(service_account::Model::is_enabled)
                .ok_or_else(invalid_credential)?;
            let credential = ServiceCredentialRepository
                .find_by_key_id_for_share(
                    &transaction,
                    &tenant.tenant_id,
                    account.id,
                    &parsed_key.key_id,
                )
                .await?
                .filter(|item| item.id == hint.credential.id && item.is_usable_at(now))
                .ok_or_else(invalid_credential)?;
            let pepper = self
                .keyring
                .get(credential.pepper_version)
                .ok_or_else(invalid_credential)?;
            if !parsed_key.verify(pepper, &credential.secret_mac)? {
                return Err(invalid_credential());
            }
            let (delegation, delegation_capabilities) = match parsed_delegation {
                Some(parsed) => {
                    let hinted = hint.delegation.as_ref().ok_or_else(invalid_credential)?;
                    if hinted.tenant_id != tenant.tenant_id || hinted.account_id != account.id {
                        return Err(invalid_credential());
                    }
                    let delegation = ServiceDelegationRepository
                        .find_by_id_for_share(&transaction, &tenant.tenant_id, hinted.id)
                        .await?
                        .filter(|item| item.is_usable_at(now))
                        .ok_or_else(invalid_credential)?;
                    let delegation_pepper = self
                        .keyring
                        .get(delegation.pepper_version)
                        .ok_or_else(invalid_credential)?;
                    if !parsed.verify(delegation_pepper, &delegation.token_mac)? {
                        return Err(invalid_credential());
                    }
                    let capabilities = ServiceDelegationRepository
                        .capability_keys_for_share(&transaction, &tenant.tenant_id, delegation.id)
                        .await?
                        .into_iter()
                        .collect();
                    (Some(delegation), capabilities)
                }
                None => (None, BTreeSet::new()),
            };
            let snapshot = ServiceAuthorizationRepository
                .lock_snapshot_in_txn(
                    &transaction,
                    &tenant.tenant_id,
                    account.id,
                    delegation.as_ref().map(|item| item.user_id),
                )
                .await?;
            validate_subjects(&snapshot, delegation.is_some())?;
            let account_permissions = subject_permissions(&snapshot, &snapshot.account_role_ids);
            let user_permissions = subject_permissions(&snapshot, &snapshot.user_role_ids);
            let account_scope = resolve_account_scope(&snapshot, account.dept_id);
            let user_scope = delegation.as_ref().map(|_| resolve_user_scope(&snapshot));
            let authorized = AuthorizedContext {
                tenant,
                account,
                credential,
                delegation,
                snapshot,
                delegation_capabilities,
                account_permissions,
                user_permissions,
                account_scope,
                user_scope,
            };
            self.ensure_capability_authorized(descriptor, &authorized)?;
            let query = self.query(&transaction, request, &authorized).await?;
            let body = encode_success(request, &query.data, self.config.max_response_bytes)?;
            let completed_at = DataRetentionRepository
                .database_utc_now(&transaction)
                .await?;
            ServiceAccessAuditRepository
                .insert(
                    &transaction,
                    success_audit(
                        request,
                        descriptor,
                        &authorized,
                        query.reason_code,
                        query.row_count,
                        body.len(),
                        completed_at,
                        &self.keyring,
                    )?,
                )
                .await?;
            let principal = AgentPrincipal {
                tenant_id: authorized.tenant.tenant_id,
                account_id: authorized.account.id,
                credential_id: authorized.credential.id,
                delegation_id: authorized.delegation.as_ref().map(|item| item.id),
                represented_user_id: authorized.delegation.as_ref().map(|item| item.user_id),
                access_mode: if authorized.delegation.is_some() {
                    AgentAccessMode::Delegated
                } else {
                    AgentAccessMode::Direct
                },
            };
            Ok((body, principal))
        }
        .await;
        match result {
            Ok((body, principal)) => {
                transaction.commit().await.map_err(database_error)?;
                Ok(AgentSuccess { body, principal })
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    fn ensure_capability_authorized(
        &self,
        descriptor: &AgentCapabilityDescriptor,
        context: &AuthorizedContext,
    ) -> AppResult<()> {
        let delegated = context.delegation.is_some();
        if (delegated && !descriptor.delegated) || (!delegated && !descriptor.direct) {
            return Err(AppError::Authorization("Agent 能力不可用".into()));
        }
        if descriptor.required_permission.is_empty() {
            return Ok(());
        }
        let account_allowed =
            rbac::has_permission(&context.account_permissions, descriptor.required_permission);
        let user_allowed = !delegated
            || rbac::has_permission(&context.user_permissions, descriptor.required_permission);
        let delegated_allowed =
            !delegated || context.delegation_capabilities.contains(descriptor.key);
        if account_allowed && user_allowed && delegated_allowed {
            Ok(())
        } else {
            Err(AppError::Authorization("Agent 能力不可用".into()))
        }
    }

    async fn query(
        &self,
        transaction: &sea_orm::DatabaseTransaction,
        request: &AgentRequest,
        context: &AuthorizedContext,
    ) -> AppResult<QueryResult> {
        let page = request.page;
        let page_size = request.page_size;
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let user_dept = context.snapshot.user.as_ref().and_then(|user| user.dept_id);
        match request.capability {
            AgentCapability::Capabilities => {
                let items = AgentCapability::ALL
                    .into_iter()
                    .filter(|capability| *capability != AgentCapability::Capabilities)
                    .filter(|capability| {
                        self.ensure_capability_authorized(capability.descriptor(), context)
                            .is_ok()
                    })
                    .filter(|capability| capability_has_rows(*capability, context, user_dept))
                    .map(|capability| {
                        let descriptor = capability.descriptor();
                        AgentCapabilityVo {
                            key: descriptor.key,
                            method: descriptor.method,
                            path: descriptor.path,
                        }
                    })
                    .collect::<Vec<_>>();
                Ok(QueryResult::new(items.len(), json_value(items)?))
            }
            AgentCapability::DirectoryUsers => {
                let scope = users_scope(
                    &context.account_scope,
                    context.user_scope.as_ref(),
                    user_dept,
                );
                let result = AgentQueryRepository
                    .users_page(
                        transaction,
                        &context.tenant.tenant_id,
                        &scope,
                        offset,
                        page_size,
                    )
                    .await?;
                let department_names = context
                    .snapshot
                    .departments
                    .iter()
                    .map(|department| (department.id, department.name.clone()))
                    .collect::<BTreeMap<_, _>>();
                let items = result
                    .records
                    .into_iter()
                    .map(|user| AgentUserVo {
                        id: user.id.to_string(),
                        username: user.username,
                        nickname: user.nickname,
                        dept_name: user
                            .dept_id
                            .and_then(|id| department_names.get(&id).cloned()),
                        status: user.status,
                    })
                    .collect::<Vec<_>>();
                let data = AgentPage::new(
                    items,
                    page,
                    page_size,
                    result.total,
                    self.config.max_page_size,
                );
                Ok(QueryResult::page(data.items.len(), data, &scope)?)
            }
            AgentCapability::DirectoryDepartments => {
                let scope = departments_scope(
                    &context.account_scope,
                    context.user_scope.as_ref(),
                    user_dept,
                );
                let result = AgentQueryRepository
                    .departments_page(
                        transaction,
                        &context.tenant.tenant_id,
                        &scope,
                        offset,
                        page_size,
                    )
                    .await?;
                let items = result
                    .records
                    .into_iter()
                    .map(|department| AgentDepartmentVo {
                        id: department.id.to_string(),
                        name: department.name,
                        parent_id: department.parent_id.map(|id| id.to_string()),
                        status: department.status,
                    })
                    .collect::<Vec<_>>();
                let data = AgentPage::new(
                    items,
                    page,
                    page_size,
                    result.total,
                    self.config.max_page_size,
                );
                Ok(QueryResult::page(data.items.len(), data, &scope)?)
            }
            AgentCapability::DirectoryPosts => {
                if !both_all(context) {
                    return QueryResult::empty_page(page, page_size, self.config.max_page_size);
                }
                let result = AgentQueryRepository
                    .posts_page(transaction, &context.tenant.tenant_id, offset, page_size)
                    .await?;
                let items = result
                    .records
                    .into_iter()
                    .map(|post| AgentPostVo {
                        id: post.id.to_string(),
                        code: post.code,
                        name: post.name,
                        status: post.status,
                    })
                    .collect::<Vec<_>>();
                let row_count = items.len();
                let data = AgentPage::new(
                    items,
                    page,
                    page_size,
                    result.total,
                    self.config.max_page_size,
                );
                Ok(QueryResult::new(row_count, json_value(data)?))
            }
            AgentCapability::ReferenceDictionary => {
                if !both_all(context) {
                    return QueryResult::empty_dictionary(
                        request.type_code.clone().unwrap_or_default(),
                        page,
                        page_size,
                        self.config.max_page_size,
                    );
                }
                let type_code = validate_type_code(request.type_code.as_deref())?;
                let result = AgentQueryRepository
                    .dictionary_by_type_code_page(
                        transaction,
                        &context.tenant.tenant_id,
                        type_code,
                        offset,
                        page_size,
                    )
                    .await?;
                let Some(result) = result else {
                    return Err(AppError::NotFound("字典类型不存在".into()));
                };
                let items = result
                    .records
                    .into_iter()
                    .map(|item| AgentDictionaryItemVo {
                        label: item.label,
                        value: item.value,
                        sort: item.sort,
                    })
                    .collect::<Vec<_>>();
                let row_count = items.len();
                let data = AgentDictionaryVo {
                    type_code: result.dict_type.code,
                    items,
                    page,
                    page_size,
                    total: result.total,
                    total_pages: result.total.div_ceil(page_size),
                    max_page_size: self.config.max_page_size,
                };
                Ok(QueryResult::new(row_count, json_value(data)?))
            }
        }
    }

    async fn audit_failure(
        &self,
        request: &AgentRequest,
        hint: Option<&IdentityHint>,
        result: &'static str,
        reason: &'static str,
        http_status: i32,
    ) -> AppResult<()> {
        let request_id = normalized_request_id(&request.request_id);
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let completed_at = DataRetentionRepository
            .database_utc_now(&transaction)
            .await?;
        let descriptor = request.capability.descriptor();
        let access_mode = if request.delegation.is_some() {
            "delegated"
        } else if hint.is_some() {
            "direct"
        } else {
            ACCESS_MODE_UNKNOWN
        };
        let active_pepper = self.keyring.active().1;
        let audit = service_access_audit::Model {
            id: snowflake::try_next_snowflake_id()?,
            request_id,
            tenant_id: hint.map(|item| item.credential.tenant_id.clone()),
            account_id: hint.map(|item| item.credential.account_id),
            credential_id: hint.map(|item| item.credential.id),
            delegation_id: hint.and_then(|item| item.delegation.as_ref().map(|row| row.id)),
            represented_user_id: hint
                .and_then(|item| item.delegation.as_ref().map(|row| row.user_id)),
            operation_id: descriptor.operation_id.into(),
            capability_key: descriptor.key.into(),
            required_permission: descriptor.required_permission.into(),
            access_mode: access_mode.into(),
            result: result.into(),
            reason_code: reason.into(),
            http_status,
            request_ip_digest: Some(keyed_hash(
                active_pepper,
                IP_DIGEST_DOMAIN,
                request.client_ip.to_string().as_bytes(),
            )?),
            user_agent_digest: request
                .user_agent
                .as_deref()
                .map(|value| keyed_hash(active_pepper, USER_AGENT_DIGEST_DOMAIN, value.as_bytes()))
                .transpose()?,
            row_count: None,
            response_bytes: None,
            tenant_epoch: None,
            account_authorization_version: None,
            user_authorization_version: None,
            delegation_version: hint
                .and_then(|item| item.delegation.as_ref().map(|row| row.version)),
            started_at: request.started_at,
            completed_at,
        };
        ServiceAccessAuditRepository
            .insert(&transaction, audit)
            .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn audit_failure_bounded(
        &self,
        request: &AgentRequest,
        hint: Option<&IdentityHint>,
        result: &'static str,
        reason: &'static str,
        http_status: i32,
    ) -> AppResult<()> {
        match tokio::time::timeout(
            Duration::from_millis(self.config.query_timeout_ms),
            self.audit_failure(request, hint, result, reason, http_status),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(AppError::ServiceUnavailable(
                "Agent 访问审计写入超时".into(),
            )),
        }
    }
}

struct QueryResult {
    data: serde_json::Value,
    row_count: usize,
    reason_code: &'static str,
}

impl QueryResult {
    fn new(row_count: usize, data: serde_json::Value) -> Self {
        Self {
            data,
            row_count,
            reason_code: if row_count == 0 {
                "data_scope_empty"
            } else {
                "ok"
            },
        }
    }

    fn page<T>(row_count: usize, data: T, scope: &AgentRowScope) -> AppResult<Self>
    where
        T: Serialize,
    {
        Ok(Self {
            data: json_value(data)?,
            row_count,
            reason_code: if matches!(scope, AgentRowScope::Empty) {
                "data_scope_empty"
            } else {
                "ok"
            },
        })
    }

    fn empty_page(page: u64, page_size: u64, max_page_size: u64) -> AppResult<Self> {
        Ok(Self {
            data: json_value(AgentPage::<AgentPostVo>::new(
                Vec::new(),
                page,
                page_size,
                0,
                max_page_size,
            ))?,
            row_count: 0,
            reason_code: "data_scope_empty",
        })
    }

    fn empty_dictionary(
        type_code: String,
        page: u64,
        page_size: u64,
        max_page_size: u64,
    ) -> AppResult<Self> {
        Ok(Self {
            data: json_value(AgentDictionaryVo {
                type_code,
                items: Vec::new(),
                page,
                page_size,
                total: 0,
                total_pages: 0,
                max_page_size,
            })?,
            row_count: 0,
            reason_code: "data_scope_empty",
        })
    }
}

fn validate_subjects(snapshot: &ServiceAuthorizationSnapshot, delegated: bool) -> AppResult<()> {
    let account_role_ids = snapshot
        .account_role_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if snapshot
        .roles
        .iter()
        .any(|role| account_role_ids.contains(&role.id) && role.is_super != 0)
    {
        return Err(AppError::Authorization("服务账号不能绑定超级角色".into()));
    }
    if delegated {
        let user = snapshot.user.as_ref().ok_or_else(invalid_credential)?;
        if !user.is_enabled() || user.del_flag != ryframe_db::user::Model::DEL_FLAG_NORMAL {
            return Err(invalid_credential());
        }
    }
    Ok(())
}

fn subject_permissions(
    snapshot: &ServiceAuthorizationSnapshot,
    subject_role_ids: &[i64],
) -> Vec<String> {
    let role_ids = subject_role_ids.iter().copied().collect::<BTreeSet<_>>();
    let active_role_ids = snapshot
        .roles
        .iter()
        .filter(|role| {
            role_ids.contains(&role.id)
                && role.status == role::Model::STATUS_NORMAL
                && role.del_flag == role::Model::DEL_FLAG_NORMAL
        })
        .map(|role| role.id)
        .collect::<BTreeSet<_>>();
    let permission_ids = snapshot
        .role_permissions
        .iter()
        .filter(|relation| active_role_ids.contains(&relation.role_id))
        .map(|relation| relation.perm_id)
        .collect::<BTreeSet<_>>();
    snapshot
        .permissions
        .iter()
        .filter(|permission| permission_ids.contains(&permission.id) && permission.status == "1")
        .map(|permission| permission.code.clone())
        .collect()
}

fn both_all(context: &AuthorizedContext) -> bool {
    context.account_scope.is_all() && context.user_scope.as_ref().is_none_or(SubjectScope::is_all)
}

fn capability_has_rows(
    capability: AgentCapability,
    context: &AuthorizedContext,
    user_dept: Option<i64>,
) -> bool {
    match capability {
        AgentCapability::Capabilities => true,
        AgentCapability::DirectoryUsers => !matches!(
            users_scope(
                &context.account_scope,
                context.user_scope.as_ref(),
                user_dept,
            ),
            AgentRowScope::Empty
        ),
        AgentCapability::DirectoryDepartments => !matches!(
            departments_scope(
                &context.account_scope,
                context.user_scope.as_ref(),
                user_dept,
            ),
            AgentRowScope::Empty
        ),
        AgentCapability::DirectoryPosts | AgentCapability::ReferenceDictionary => both_all(context),
    }
}

fn validate_type_code(type_code: Option<&str>) -> AppResult<&str> {
    let value = type_code
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 100)
        .ok_or_else(|| AppError::Validation("字典类型代码无效".into()))?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(AppError::Validation("字典类型代码无效".into()));
    }
    Ok(value)
}

fn encode_success(
    request: &AgentRequest,
    data: &serde_json::Value,
    max_bytes: usize,
) -> AppResult<Vec<u8>> {
    let value = serde_json::json!({
        "code": 200,
        "message": request.success_message,
        "data": data,
        "request_id": request.request_id,
        "error_key": null,
        "details": null,
    });
    let encoded = serde_json::to_vec(&value)
        .map_err(|_| AppError::Internal("序列化 Agent 响应失败".into()))?;
    if encoded.len() > max_bytes {
        return Err(AppError::PayloadTooLarge(
            "Agent 响应超过大小上限，请缩小分页".into(),
        ));
    }
    Ok(encoded)
}

#[allow(clippy::too_many_arguments)]
fn success_audit(
    request: &AgentRequest,
    descriptor: &AgentCapabilityDescriptor,
    context: &AuthorizedContext,
    reason_code: &'static str,
    row_count: usize,
    response_bytes: usize,
    completed_at: DateTime<Utc>,
    keyring: &PepperKeyring,
) -> AppResult<service_access_audit::Model> {
    let active_pepper = keyring.active().1;
    Ok(service_access_audit::Model {
        id: snowflake::try_next_snowflake_id()?,
        request_id: request.request_id.clone(),
        tenant_id: Some(context.tenant.tenant_id.clone()),
        account_id: Some(context.account.id),
        credential_id: Some(context.credential.id),
        delegation_id: context.delegation.as_ref().map(|item| item.id),
        represented_user_id: context.delegation.as_ref().map(|item| item.user_id),
        operation_id: descriptor.operation_id.into(),
        capability_key: descriptor.key.into(),
        required_permission: descriptor.required_permission.into(),
        access_mode: if context.delegation.is_some() {
            AgentAccessMode::Delegated.as_str()
        } else {
            AgentAccessMode::Direct.as_str()
        }
        .into(),
        result: service_access_audit::Model::RESULT_SUCCESS.into(),
        reason_code: reason_code.into(),
        http_status: 200,
        request_ip_digest: Some(keyed_hash(
            active_pepper,
            IP_DIGEST_DOMAIN,
            request.client_ip.to_string().as_bytes(),
        )?),
        user_agent_digest: request
            .user_agent
            .as_deref()
            .map(|value| keyed_hash(active_pepper, USER_AGENT_DIGEST_DOMAIN, value.as_bytes()))
            .transpose()?,
        row_count: Some(i32::try_from(row_count).unwrap_or(i32::MAX)),
        response_bytes: Some(i64::try_from(response_bytes).unwrap_or(i64::MAX)),
        tenant_epoch: Some(context.tenant.authorization_epoch),
        account_authorization_version: Some(context.account.authorization_version),
        user_authorization_version: context
            .snapshot
            .user
            .as_ref()
            .map(|user| user.authorization_version),
        delegation_version: context.delegation.as_ref().map(|item| item.version),
        started_at: request.started_at,
        completed_at,
    })
}

fn classify_error(error: &AppError) -> (i32, &'static str, &'static str) {
    match error {
        AppError::Authentication(_) => (401, "invalid_credential", RESULT_DENIED),
        AppError::Authorization(_) => (403, "capability_denied", RESULT_DENIED),
        AppError::NotFound(_) => (404, "not_found", RESULT_DENIED),
        AppError::Validation(_) => (400, "validation", RESULT_DENIED),
        AppError::PayloadTooLarge(_) => (413, "response_too_large", RESULT_ERROR),
        AppError::RateLimited(_, _) => (429, "rate_limited", RESULT_DENIED),
        AppError::Conflict(_) | AppError::RetryableConflict(_, _) => {
            (409, "conflict", RESULT_DENIED)
        }
        AppError::Database(_) => (503, "database_unavailable", RESULT_ERROR),
        AppError::ServiceUnavailable(message) if message == "Agent 查询超时" => {
            (503, "query_timeout", RESULT_ERROR)
        }
        AppError::ServiceUnavailable(_) => (503, "service_unavailable", RESULT_ERROR),
        AppError::Config(_) | AppError::Internal(_) => (500, "internal", RESULT_ERROR),
    }
}

fn classify_pre_authorization_error(error: &AppError) -> (i32, &'static str, &'static str) {
    match error {
        AppError::Database(_) => (503, "database_unavailable", RESULT_ERROR),
        AppError::ServiceUnavailable(message) if message == "Agent 查询超时" => {
            (503, "query_timeout", RESULT_ERROR)
        }
        AppError::ServiceUnavailable(_) => (503, "service_unavailable", RESULT_ERROR),
        AppError::Config(_) | AppError::Internal(_) => (500, "internal", RESULT_ERROR),
        _ => (401, "invalid_credential", RESULT_DENIED),
    }
}

fn mask_missing_identity(error: AppError) -> AppError {
    match error {
        AppError::NotFound(_) | AppError::Authentication(_) | AppError::Authorization(_) => {
            invalid_credential()
        }
        error => error,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

async fn before_deadline<T>(
    deadline: tokio::time::Instant,
    future: impl std::future::Future<Output = AppResult<T>>,
) -> AppResult<T> {
    match tokio::time::timeout_at(deadline, future).await {
        Ok(result) => result,
        Err(_) => Err(AppError::ServiceUnavailable("Agent 查询超时".into())),
    }
}

fn normalized_request_id(value: &str) -> String {
    uuid::Uuid::parse_str(value)
        .map(|request_id| request_id.to_string())
        .unwrap_or_else(|_| uuid::Uuid::now_v7().to_string())
}

fn json_value(value: impl Serialize) -> AppResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|_| AppError::Internal("序列化 Agent 数据失败".into()))
}
