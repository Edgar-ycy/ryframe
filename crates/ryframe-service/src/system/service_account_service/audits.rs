use super::*;

impl ServiceAccountService {
    pub async fn list_access_audits(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<ServiceAccessAuditVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_enabled()?;
        let db = self.db.select_read(ReadConsistency::Eventual).connection;
        let base = service_access_audit::Entity::find()
            .filter(service_access_audit::Column::TenantId.eq(tenant_id));
        let total = base.clone().count(&db).await.map_err(database_error)?;
        let rows = base
            .order_by_desc(service_access_audit::Column::StartedAt)
            .offset(page.offset())
            .limit(page.page_size())
            .all(&db)
            .await
            .map_err(database_error)?;
        Ok(PageResult::new(
            rows.into_iter().map(ServiceAccessAuditVo::from).collect(),
            total,
            &page,
        ))
    }
}
