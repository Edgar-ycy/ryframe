use super::*;

impl ServiceAccountService {
    pub async fn list_access_audits(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<ServiceAccessAuditVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let result = self.audit_read.list(tenant_id, page).await?;
        Ok(PageResult {
            records: result.records.into_iter().map(Into::into).collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }
}
