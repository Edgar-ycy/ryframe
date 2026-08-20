use super::*;

impl AgentService {
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
            id: crate::next_id()?,
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

    pub(super) async fn audit_failure_bounded(
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

#[allow(clippy::too_many_arguments)]
pub(super) fn success_audit(
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
        id: crate::next_id()?,
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
