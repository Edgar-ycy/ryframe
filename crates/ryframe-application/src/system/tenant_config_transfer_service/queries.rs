use super::*;

impl TenantConfigTransferService {
    pub async fn list_bundles(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigBundleVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let total = tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        let records = self
            .repository
            .list_bundles(self.db.write(), tenant_id, page.page_size(), page.offset())
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(PageResult::new(records, total, &page))
    }

    pub async fn get_bundle(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<TenantConfigBundleVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.repository
            .find_bundle_by_id(self.db.write(), tenant_id, id)
            .await?
            .map(Into::into)
            .ok_or_else(|| AppError::NotFound("配置包不存在".into()))
    }

    pub async fn download_bundle(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<DownloadedFile> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let bundle = self
            .repository
            .find_bundle_by_id(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置包不存在".into()))?;
        ensure_bundle_available(
            &bundle,
            self.repository.database_utc_now(self.db.write()).await?,
        )?;
        let file_id = bundle
            .file_id
            .ok_or_else(|| AppError::Conflict("配置包文件尚未生成".into()))?;
        self.file_service
            .download_config_package_internal(tenant_id, file_id)
            .await
    }

    pub async fn list_transfers(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigTransferVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let total = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        let records = self
            .repository
            .list_transfers(self.db.write(), tenant_id, page.page_size(), page.offset())
            .await?;
        let bundle_ids = records
            .iter()
            .map(|transfer| transfer.bundle_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let bundles = self
            .repository
            .find_bundles_by_ids(self.db.write(), tenant_id, &bundle_ids)
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
        Ok(PageResult::new(records, total, &page))
    }

    pub async fn get_transfer(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<TenantConfigTransferVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transfer = self
            .repository
            .find_transfer_by_id(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        let bundle = self
            .repository
            .find_bundle_by_id(self.db.write(), tenant_id, transfer.bundle_id)
            .await?
            .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
        TenantConfigTransferVo::from_models(transfer, &bundle)
    }

    pub async fn list_transfer_items(
        &self,
        actor: &ActorContext,
        transfer_id: i64,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigTransferItemVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_transfer_visible(tenant_id, transfer_id).await?;
        let base = tenant_config_transfer_item::Entity::find()
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id));
        let total = base
            .clone()
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        let records = base
            .order_by_asc(tenant_config_transfer_item::Column::Id)
            .limit(page.page_size())
            .offset(page.offset())
            .all(self.db.write())
            .await
            .map_err(database_error)?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(PageResult::new(records, total, &page))
    }
}
