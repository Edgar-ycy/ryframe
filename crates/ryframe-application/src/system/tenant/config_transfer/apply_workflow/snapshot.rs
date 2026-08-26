use super::*;

impl TenantConfigTransferService {
    pub(super) async fn capture_rollback_snapshot(
        &self,
        prepared: &PreparedApply,
    ) -> AppResult<RollbackSnapshotSource> {
        let transaction = self.persistence.begin().await?;
        let operation = async {
            let fence = transaction
                .lock_tenant_configuration(&prepared.tenant_id, Some(&prepared.owner_token))
                .await?;
            self.product
                .ensure_capability_requirements_in_txn(
                    transaction.product(),
                    &prepared.tenant_id,
                    &prepared.package.manifest.required_capabilities,
                )
                .await?;
            let generated_at = transaction.database_now().await?;
            transaction
                .ensure_requester_snapshot(
                    &prepared.tenant_id,
                    requester_record(&prepared.requester),
                    fence,
                    generated_at,
                )
                .await?;
            let tenant_name = transaction.tenant_name(&prepared.tenant_id).await?;
            let target_resources = transaction.load_resources(&prepared.tenant_id).await?;
            ensure_preview_identity(
                &prepared.transfer,
                &prepared.package,
                &target_resources,
                fence,
            )?;
            let enabled_capabilities = self
                .product
                .enabled_capability_requirements_in_txn(transaction.product(), &prepared.tenant_id)
                .await?;
            let (resources, capabilities) = filter_exportable_resources(
                target_resources,
                &self.target_catalog,
                &enabled_capabilities,
            )?;
            Ok::<_, AppError>(RollbackSnapshotSource {
                resources,
                capabilities,
                tenant_name,
                generated_at,
            })
        }
        .await;
        match operation {
            Ok(source) => {
                transaction
                    .commit(crate::TransactionAuditMode::Skip)
                    .await?;
                Ok(source)
            }
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    pub(super) async fn build_and_upload_rollback_snapshot(
        &self,
        prepared: &PreparedApply,
        source: RollbackSnapshotSource,
    ) -> AppResult<super::super::lifecycle::RollbackSnapshotFile> {
        let snapshot = crate::system::build_tenant_config_package(
            Arc::clone(&self.archive),
            source.resources,
            source.capabilities,
            TenantConfigPackageSource {
                tenant_key: prepared.tenant_id.clone(),
                tenant_name: source.tenant_name,
                app_version: env!("CARGO_PKG_VERSION").to_owned(),
                generated_at: source.generated_at,
            },
            self.package_limits(),
        )
        .await?;
        let uploaded = self
            .file
            .upload_config_package_unbound(
                &prepared.tenant_id,
                "config-transfer-worker",
                format!("rollback-{}.ryframe-config.zip", prepared.transfer_id),
                snapshot.data,
                u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            )
            .await?;
        let file_id = match parse_file_id(&uploaded.file_id) {
            Ok(file_id) => file_id,
            Err(error) => {
                if let Ok(file_id) = uploaded.file_id.parse::<i64>() {
                    let _ = self
                        .file
                        .schedule_unreferenced_config_package_cleanup(&prepared.tenant_id, file_id)
                        .await;
                }
                return Err(error);
            }
        };
        Ok(super::super::lifecycle::RollbackSnapshotFile::new(
            self.clone(),
            prepared.tenant_id.clone(),
            file_id,
        ))
    }
}
