use super::*;
use crate::DbResultExt;

#[async_trait::async_trait]
impl TenantConfigTransferPersistencePort for DatabaseTenantConfigTransferPersistence {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn bundle_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ryframe_kernel::ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<TenantConfigBundleRecord>> {
        let total = tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
            .count(self.database.write())
            .await
            .db()?;
        let records = TenantConfigTransferRepository
            .list_bundles(
                self.database.write(),
                tenant_id,
                page.page_size(),
                page.offset(),
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(PageResult::new(records, total, &page))
    }

    async fn transfer_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ryframe_kernel::ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<TenantConfigTransferRecord>> {
        let total = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .count(self.database.write())
            .await
            .db()?;
        let records = TenantConfigTransferRepository
            .list_transfers(
                self.database.write(),
                tenant_id,
                page.page_size(),
                page.offset(),
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(PageResult::new(records, total, &page))
    }

    async fn item_page<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        page: ryframe_kernel::ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<TenantConfigTransferItemRecord>> {
        let query = tenant_config_transfer_item::Entity::find()
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id));
        let total = query.clone().count(self.database.write()).await.db()?;
        let records = query
            .order_by_asc(tenant_config_transfer_item::Column::Id)
            .limit(page.page_size())
            .offset(page.offset())
            .all(self.database.write())
            .await
            .db()?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(PageResult::new(records, total, &page))
    }

    async fn find_bundle<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigBundleRecord>> {
        TenantConfigTransferRepository
            .find_bundle_by_id(self.database.write(), tenant_id, id)
            .await
            .map(|record| record.map(Into::into))
    }

    async fn find_bundles<'a>(
        &'a self,
        tenant_id: &'a str,
        ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<TenantConfigBundleRecord>> {
        TenantConfigTransferRepository
            .find_bundles_by_ids(self.database.write(), tenant_id, ids)
            .await
            .map(|records| records.into_iter().map(Into::into).collect())
    }

    async fn find_transfer<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>> {
        TenantConfigTransferRepository
            .find_transfer_by_id(self.database.write(), tenant_id, id)
            .await
            .map(|record| record.map(Into::into))
    }

    async fn find_transfer_by_background_job(
        &self,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>> {
        TenantConfigTransferRepository
            .find_transfer_by_background_job(self.database.write(), background_job_id)
            .await
            .map(|record| record.map(Into::into))
    }

    async fn find_transfer_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        requested_by: i64,
        idempotency_key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>> {
        TenantConfigTransferRepository
            .find_transfer_by_idempotency_key(
                self.database.write(),
                tenant_id,
                requested_by,
                idempotency_key_hash,
            )
            .await
            .map(|record| record.map(Into::into))
    }

    async fn items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<TenantConfigTransferItemRecord>> {
        TenantConfigTransferRepository
            .list_items(self.database.write(), tenant_id, transfer_id)
            .await
            .map(|records| records.into_iter().map(Into::into).collect())
    }

    async fn cache_namespace_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<i64> {
        CacheNamespaceVersionRepository
            .find_version(self.database.write(), tenant_id, CONFIG_CACHE_NAMESPACE)
            .await
    }

    async fn load_resources<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<ryframe_application::system::TenantConfigPackageResources> {
        super::super::transfer_sql::load_resources_on(self.database.write(), tenant_id).await
    }

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn TenantConfigTransferTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseTenantConfigTransferTransaction {
            transaction: transaction.into(),
        }) as Box<dyn TenantConfigTransferTransaction>)
    }
}
