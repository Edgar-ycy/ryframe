use super::*;

/// 配置迁移租约必须显式完成或释放；析构仅在异常退出时安排尽力补偿。
pub(super) struct OperationLease {
    service: TenantConfigTransferService,
    tenant_id: String,
    owner_token: String,
    active: bool,
}

impl OperationLease {
    /// 最终写事务已经原子删除租约时，显式结束本地生命周期。
    pub(super) fn finish(&mut self) {
        self.active = false;
    }

    /// 在最终写事务之前失败时，显式释放数据库租约。
    pub(super) async fn release(&mut self) -> AppResult<()> {
        let result = self
            .service
            .release_operation_lease(&self.tenant_id, &self.owner_token)
            .await;
        if result.is_ok() {
            self.active = false;
        }
        result
    }
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let service = self.service.clone();
        let tenant_id = self.tenant_id.clone();
        let owner_token = self.owner_token.clone();
        runtime.spawn(async move {
            let _ = service
                .release_operation_lease(&tenant_id, &owner_token)
                .await;
        });
    }
}

impl TenantConfigTransferService {
    pub(super) async fn mark_transfer_running(
        &self,
        tenant_id: &str,
        transfer_id: i64,
        job_id: i64,
        job_type: &str,
        owner_token: Option<&str>,
    ) -> AppResult<()> {
        let transaction = self.persistence.begin().await?;
        transaction
            .lock_tenant_configuration(tenant_id, owner_token)
            .await?;
        let mut transfer = transaction
            .lock_transfer(tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        match job_type {
            TENANT_CONFIG_PREVIEW_JOB_TYPE
                if transfer.preview_background_job_id == Some(job_id)
                    && matches!(
                        transfer.status.as_str(),
                        TenantConfigTransferRecord::STATUS_PREVIEW_PENDING
                            | TenantConfigTransferRecord::STATUS_PREVIEWING
                    ) =>
            {
                transfer.status = TenantConfigTransferRecord::STATUS_PREVIEWING.to_owned();
            }
            TENANT_CONFIG_APPLY_JOB_TYPE
                if transfer.apply_background_job_id == Some(job_id)
                    && matches!(
                        transfer.status.as_str(),
                        TenantConfigTransferRecord::STATUS_APPLY_PENDING
                            | TenantConfigTransferRecord::STATUS_APPLYING
                    ) =>
            {
                transfer.status = TenantConfigTransferRecord::STATUS_APPLYING.to_owned();
            }
            TENANT_CONFIG_ROLLBACK_JOB_TYPE
                if transfer.rollback_background_job_id == Some(job_id)
                    && matches!(
                        transfer.status.as_str(),
                        TenantConfigTransferRecord::STATUS_ROLLBACK_PENDING
                            | TenantConfigTransferRecord::STATUS_ROLLING_BACK
                    ) =>
            {
                transfer.status = TenantConfigTransferRecord::STATUS_ROLLING_BACK.to_owned();
            }
            _ => {
                transaction.rollback().await?;
                return Err(AppError::Conflict("配置迁移任务已被更新的操作取代".into()));
            }
        }
        transfer.updated_at = transaction.database_now().await?;
        transaction.update_transfer(transfer).await?;
        transaction.commit().await
    }

    pub(super) async fn bundle_requester(&self, tenant_id: &str, bundle_id: i64) -> AppResult<i64> {
        self.persistence
            .find_bundle(tenant_id, bundle_id)
            .await?
            .map(|bundle| bundle.created_by)
            .ok_or_else(|| AppError::NotFound("配置包导出记录不存在".into()))
    }

    pub(super) async fn load_bundle_package(
        &self,
        tenant_id: &str,
        bundle_id: i64,
    ) -> AppResult<ParsedTenantConfigPackage> {
        let bundle = self
            .persistence
            .find_bundle(tenant_id, bundle_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置包不存在".into()))?;
        ensure_bundle_available(&bundle, self.persistence.database_now().await?)?;
        let file = self
            .file
            .download_config_package_internal(
                tenant_id,
                bundle
                    .file_id
                    .ok_or_else(|| AppError::Conflict("配置包文件不存在".into()))?,
            )
            .await?;
        let parsed = parse_tenant_config_package(
            Arc::clone(&self.archive),
            file.data,
            self.package_limits(),
        )
        .await?;
        if bundle.sha256.as_deref() != Some(parsed.package_sha256.as_str()) {
            return Err(AppError::Conflict("配置包文件完整性校验失败".into()));
        }
        Ok(parsed)
    }

    pub(super) async fn acquire_operation_lease(
        &self,
        tenant_id: &str,
        transfer_id: i64,
        owner_token: &str,
        operation: &str,
    ) -> AppResult<OperationLease> {
        let transaction = self.persistence.begin().await?;
        let now = transaction.database_now().await?;
        transaction
            .acquire_lease(TenantConfigOperationLeaseRecord {
                tenant_id: tenant_id.to_owned(),
                owner_token: owner_token.to_owned(),
                operation: operation.to_owned(),
                resource_type: "tenant_config_transfer".to_owned(),
                resource_id: transfer_id.to_string(),
                expires_at: now
                    + Duration::seconds(i64::try_from(self.config.lease_seconds).unwrap_or(300)),
                created_at: now,
                updated_at: now,
            })
            .await?;
        transaction.commit().await?;
        Ok(OperationLease {
            service: self.clone(),
            tenant_id: tenant_id.to_owned(),
            owner_token: owner_token.to_owned(),
            active: true,
        })
    }

    pub(super) async fn release_operation_lease(
        &self,
        tenant_id: &str,
        owner_token: &str,
    ) -> AppResult<()> {
        let transaction = self.persistence.begin().await?;
        transaction.release_lease(tenant_id, owner_token).await?;
        transaction.commit().await
    }

    pub(super) async fn renew_operation_lease(
        &self,
        tenant_id: &str,
        owner_token: &str,
    ) -> AppResult<()> {
        let transaction = self.persistence.begin().await?;
        let operation = async {
            let now = transaction.database_now().await?;
            let renewed = transaction
                .renew_lease(
                    tenant_id,
                    owner_token,
                    now + Duration::seconds(
                        i64::try_from(self.config.lease_seconds).unwrap_or(300),
                    ),
                )
                .await?;
            if !renewed {
                return Err(AppError::Conflict("配置迁移租约已失效".into()));
            }
            Ok::<_, AppError>(())
        }
        .await;
        match operation {
            Ok(()) => transaction.commit().await,
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    pub(super) async fn sync_committed_cache_state(
        &self,
        tenant_id: &str,
        transfer: &TenantConfigTransferRecord,
    ) -> AppResult<()> {
        let epoch = transfer
            .applied_authorization_epoch
            .ok_or_else(|| AppError::Conflict("迁移终态缺少授权纪元".into()))?;
        let namespace_version = self.persistence.cache_namespace_version(tenant_id).await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, epoch)
            .await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await
    }
}
