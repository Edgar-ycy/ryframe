use super::*;

impl AgentService {
    pub(super) async fn execute_inner(&self, request: &AgentRequest) -> AppResult<AgentSuccess> {
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
        if !self.multi_tenancy.allows_tenant(&hint.credential.tenant_id) {
            let error = invalid_credential();
            self.audit_failure_bounded(
                request,
                Some(&hint),
                RESULT_DENIED,
                "invalid_credential",
                401,
            )
            .await?;
            return Err(error);
        }
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
        drop(tokio::spawn(async move {
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
        let hints = self
            .identity
            .limit_hints(&hint.credential.tenant_id, hint.credential.account_id)
            .await?;
        Ok(hints.effective_limits(self.config.default_requests_per_minute))
    }

    async fn identity_hint(
        &self,
        parsed_key: &ParsedApiKey,
        parsed_delegation: Option<&ParsedDelegation>,
    ) -> AppResult<IdentityHint> {
        let credential = self
            .identity
            .credential_hint(&parsed_key.key_id)
            .await?
            .ok_or_else(invalid_credential)?;
        let delegation = if let Some(parsed) = parsed_delegation {
            let candidates = self
                .keyring
                .iter()
                .map(|(_, pepper)| parsed.mac(pepper))
                .collect::<AppResult<Vec<_>>>()?;
            Some(
                self.identity
                    .delegation_hint(&candidates)
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
}
