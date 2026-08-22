use super::*;

impl ProductService {
    /// 强一致读取当前租户的已验证有效产品上下文，供认证上下文和安全门禁复用。
    pub async fn effective_context(&self, tenant_id: &str) -> AppResult<ProductContextVo> {
        let bundle = self
            .read
            .tenant_product(tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        self.context_from_snapshot(bundle)
    }

    /// 强一致读取运行时纪元；不得从旧授权快照推导。
    pub async fn runtime_epoch(&self, tenant_id: &str) -> AppResult<i64> {
        self.read
            .tenant_product(tenant_id)
            .await?
            .map(|bundle| bundle.runtime_epoch)
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    pub async fn session_context(&self, tenant_id: &str) -> AppResult<SessionProductContextVo> {
        let bundle = self
            .read
            .tenant_product(tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let authorization_epoch = bundle.authorization_epoch;
        let context = self.context_from_snapshot(bundle)?;
        Ok(SessionProductContextVo {
            authorization_epoch,
            runtime_epoch: context.runtime_epoch,
            capabilities: context
                .capabilities
                .into_iter()
                .filter(|capability| capability.enabled)
                .map(|capability| SessionCapabilityVo {
                    code: capability.capability_code,
                    variant: capability
                        .variant_code
                        .expect("enabled capability has a validated variant"),
                    schema_version: capability
                        .schema_version
                        .expect("enabled capability has a validated schema version"),
                    client_config: capability.config.unwrap_or_else(|| serde_json::json!({})),
                })
                .collect(),
        })
    }

    /// 返回当前会话必须在 RBAC 之前移除的 capability 路由。
    pub fn disabled_session_route_keys(&self, context: &SessionProductContextVo) -> Vec<String> {
        let enabled = context
            .capabilities
            .iter()
            .map(|capability| capability.code.as_str())
            .collect::<BTreeSet<_>>();
        CAPABILITY_CATALOG
            .iter()
            .filter(|descriptor| !enabled.contains(descriptor.code))
            .flat_map(|descriptor| descriptor.route_keys.iter())
            .map(|route_key| (*route_key).to_owned())
            .collect()
    }

    /// 强一致 Capability 门禁：部署缺依赖返回 501，租户未开通返回 403。
    pub async fn require_capability(
        &self,
        tenant_id: &str,
        capability_code: &str,
    ) -> AppResult<EffectiveCapabilityVo> {
        let context = self.effective_context(tenant_id).await?;
        ensure_available_capability(context, capability_code)
    }

    /// 校验调用方在同一事务中读取的产品快照。
    #[doc(hidden)]
    pub fn require_capability_snapshot(
        &self,
        snapshot: TenantProductSnapshot,
        capability_code: &str,
    ) -> AppResult<EffectiveCapabilityVo> {
        ensure_available_capability(self.context_from_snapshot(snapshot)?, capability_code)
    }

    pub async fn product_context(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
    ) -> AppResult<ProductContextVo> {
        ensure_platform_actor(actor)?;
        self.effective_context(tenant_id).await
    }
}

fn ensure_available_capability(
    context: ProductContextVo,
    capability_code: &str,
) -> AppResult<EffectiveCapabilityVo> {
    let capability = context
        .capabilities
        .into_iter()
        .find(|value| value.capability_code == capability_code)
        .ok_or_else(|| AppError::CapabilityUnavailable("能力未编译进当前部署".into()))?;
    if !capability.deployment_enabled {
        return Err(AppError::CapabilityUnavailable(format!(
            "当前部署不满足能力 {capability_code} 的基础设施依赖"
        )));
    }
    if !capability.entitled {
        return Err(AppError::TenantCapabilityDenied(format!(
            "当前租户未开通能力 {capability_code}"
        )));
    }
    Ok(capability)
}
