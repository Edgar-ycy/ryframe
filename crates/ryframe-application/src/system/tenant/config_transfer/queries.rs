use super::*;

impl TenantConfigTransferService {
    pub async fn list_bundles(
        &self,
        scope: &TenantConfigScope,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigBundleVo>> {
        let tenant_id = scope.tenant_id();
        let page = self.persistence.bundle_page(tenant_id, page).await?;
        Ok(PageResult {
            records: page.records.into_iter().map(Into::into).collect(),
            total: page.total,
            page: page.page,
            page_size: page.page_size,
        })
    }

    pub async fn get_bundle(
        &self,
        scope: &TenantConfigScope,
        id: i64,
    ) -> AppResult<TenantConfigBundleVo> {
        let tenant_id = scope.tenant_id();
        self.persistence
            .find_bundle(tenant_id, id)
            .await?
            .map(Into::into)
            .ok_or_else(|| AppError::NotFound("配置包不存在".into()))
    }

    pub async fn download_bundle(
        &self,
        scope: &TenantConfigScope,
        id: i64,
    ) -> AppResult<DownloadedFile> {
        let tenant_id = scope.tenant_id();
        let bundle = self
            .persistence
            .find_bundle(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置包不存在".into()))?;
        ensure_bundle_available(&bundle, self.persistence.database_now().await?)?;
        let file_id = bundle
            .file_id
            .ok_or_else(|| AppError::Conflict("配置包文件尚未生成".into()))?;
        self.file
            .download_config_package_internal(tenant_id, file_id)
            .await
    }

    pub async fn list_transfers(
        &self,
        scope: &TenantConfigScope,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigTransferVo>> {
        let tenant_id = scope.tenant_id();
        let page = self.persistence.transfer_page(tenant_id, page).await?;
        let records = page.records;
        let bundle_ids = records
            .iter()
            .map(|transfer| transfer.bundle_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let bundles = self
            .persistence
            .find_bundles(tenant_id, &bundle_ids)
            .await?
            .into_iter()
            .map(|bundle| (bundle.id, bundle))
            .collect::<BTreeMap<_, _>>();
        let records = records
            .into_iter()
            .map(|transfer| {
                let bundle = bundles
                    .get(&transfer.bundle_id)
                    .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
                TenantConfigTransferVo::from_models(transfer, bundle)
            })
            .collect::<AppResult<Vec<_>>>()?;
        Ok(PageResult {
            records,
            total: page.total,
            page: page.page,
            page_size: page.page_size,
        })
    }

    pub async fn get_transfer(
        &self,
        scope: &TenantConfigScope,
        id: i64,
    ) -> AppResult<TenantConfigTransferVo> {
        let tenant_id = scope.tenant_id();
        let transfer = self
            .persistence
            .find_transfer(tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        let bundle = self
            .persistence
            .find_bundle(tenant_id, transfer.bundle_id)
            .await?
            .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
        TenantConfigTransferVo::from_models(transfer, &bundle)
    }

    pub async fn list_transfer_items(
        &self,
        scope: &TenantConfigScope,
        transfer_id: i64,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigTransferItemVo>> {
        let tenant_id = scope.tenant_id();
        self.ensure_transfer_visible(tenant_id, transfer_id).await?;
        let page = self
            .persistence
            .item_page(tenant_id, transfer_id, page)
            .await?;
        Ok(PageResult {
            records: page.records.into_iter().map(Into::into).collect(),
            total: page.total,
            page: page.page,
            page_size: page.page_size,
        })
    }
}
