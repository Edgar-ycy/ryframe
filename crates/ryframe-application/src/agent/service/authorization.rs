use super::*;

impl AgentService {
    pub(super) async fn execute_locked(
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
            self.product_service
                .require_capability_in_txn(
                    &transaction,
                    &tenant.tenant_id,
                    SERVICE_ACCOUNTS_CAPABILITY,
                )
                .await?;
            let snapshot = crate::legacy_agent_snapshot::authorization_snapshot(
                ServiceAuthorizationRepository
                    .lock_snapshot_in_txn(
                        &transaction,
                        &tenant.tenant_id,
                        account.id,
                        delegation.as_ref().map(|item| item.user_id),
                    )
                    .await?,
            );
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
                    crate::legacy_agent_audit::model(success_audit(
                        request,
                        descriptor,
                        &authorized,
                        query.reason_code,
                        query.row_count,
                        body.len(),
                        completed_at,
                        &self.keyring,
                    )?),
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

    pub(super) fn ensure_capability_authorized(
        &self,
        descriptor: &AgentCapabilityDescriptor,
        context: &AuthorizedContext,
    ) -> AppResult<()> {
        let delegated = context.delegation.is_some();
        if (delegated && !descriptor.delegated) || (!delegated && !descriptor.direct) {
            return Err(AppError::PermissionDenied("Agent 能力不可用".into()));
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
            Err(AppError::PermissionDenied("Agent 能力不可用".into()))
        }
    }
}
