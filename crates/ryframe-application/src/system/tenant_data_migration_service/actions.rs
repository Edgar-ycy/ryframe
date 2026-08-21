use super::*;

impl TenantDataMigrationService {
    pub async fn cancel(
        &self,
        actor: &ActorContext,
        migration_id: i64,
        command: MigrationActionCommand,
    ) -> AppResult<MigrationView> {
        ensure_platform_actor(actor)?;
        validate_idempotency_key(&command.idempotency_key)?;
        let key_hash = sha256_hex(&format!(
            "ryframe:tenant-data:cancel:v1:{migration_id}:{}",
            command.idempotency_key
        ));
        let mut snapshot = self
            .persistence
            .migration(migration_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户数据迁移不存在".into()))?;
        if snapshot.state == TenantDataMigrationRecord::STATE_CANCELLED {
            if snapshot.cancel_idempotency_key_hash.as_deref() == Some(&key_hash) {
                return self.migration_view(snapshot).await;
            }
            return Err(AppError::Conflict("迁移已经取消".into()));
        }
        if !snapshot.can_cancel() {
            return Err(AppError::Conflict(
                "迁移已进入 cutting_over 或更晚阶段，不能取消".into(),
            ));
        }

        let intent_now = self.persistence.database_now().await?;
        let intent_transaction = self.persistence.begin().await?;
        self.acquire_or_renew_operation_lease(&*intent_transaction, &snapshot, intent_now)
            .await?;
        let mut intent = intent_transaction.lock_migration(migration_id).await?;
        if intent.state == TenantDataMigrationRecord::STATE_CANCELLED {
            let same_key = intent.cancel_idempotency_key_hash.as_deref() == Some(&key_hash);
            intent_transaction
                .release_lease(&intent.tenant_id, &intent.switch_token)
                .await?;
            intent_transaction.commit().await?;
            if same_key {
                return self.migration_view(intent).await;
            }
            return Err(AppError::Conflict("迁移已经取消".into()));
        }
        if intent.error_code.is_some() {
            return Err(AppError::TenantOperationConflict(
                "迁移已由失败补偿取得仲裁权".into(),
            ));
        }
        if !intent.can_cancel() {
            return Err(AppError::Conflict("迁移已越过取消边界".into()));
        }
        match intent.cancel_idempotency_key_hash.as_deref() {
            Some(existing) if existing != key_hash => {
                return Err(AppError::Conflict("迁移已有其他取消请求".into()));
            }
            Some(_) => {}
            None => {
                intent.cancel_idempotency_key_hash = Some(key_hash.clone());
                intent.cancelled_by = Some(actor.user_id);
                intent.cancel_requested_at = Some(intent_now);
                intent.updated_at = intent_now;
                intent = intent_transaction.save_migration(intent).await?;
            }
        }
        let background_job_id = intent
            .background_job_id
            .ok_or_else(|| AppError::Conflict("迁移缺少权威后台任务".into()))?;
        if !self
            .queue
            .reactivate_linked_in_transaction(
                intent_transaction.background_jobs(),
                background_job_id,
                super::TENANT_DATA_MIGRATION_JOB_TYPE,
                "migration_id",
                migration_id,
                intent_now,
            )
            .await?
        {
            return Err(AppError::Conflict("迁移权威后台任务关联无效".into()));
        }
        intent_transaction.commit().await?;
        snapshot = intent;
        self.queue.notify_background_jobs().await;
        self.migration_view(snapshot).await
    }

    pub async fn finalize(
        &self,
        actor: &ActorContext,
        migration_id: i64,
        command: MigrationActionCommand,
    ) -> AppResult<MigrationView> {
        ensure_platform_actor(actor)?;
        validate_idempotency_key(&command.idempotency_key)?;
        let key_hash = sha256_hex(&format!(
            "ryframe:tenant-data:finalize:v1:{migration_id}:{}",
            command.idempotency_key
        ));
        let mut snapshot = self
            .persistence
            .migration(migration_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户数据迁移不存在".into()))?;
        if snapshot.state == TenantDataMigrationRecord::STATE_FINALIZED {
            if snapshot.finalize_idempotency_key_hash.as_deref() == Some(&key_hash) {
                return self.migration_view(snapshot).await;
            }
            return Err(AppError::Conflict("迁移已经完成保留期清理".into()));
        }
        if snapshot.state != TenantDataMigrationRecord::STATE_RETENTION_PENDING {
            return Err(AppError::Conflict("迁移尚未进入保留期清理阶段".into()));
        }
        let now = self.persistence.database_now().await?;
        if snapshot.retention_until.is_none_or(|until| until > now) {
            return Err(AppError::Conflict("源数据保留期尚未结束".into()));
        }
        let not_before = snapshot
            .activated_at
            .or(snapshot.succeeded_at)
            .ok_or_else(|| AppError::Conflict("迁移缺少激活时间".into()))?;
        if !self
            .persistence
            .has_validated_backup(&snapshot, not_before, now)
            .await?
        {
            return Err(AppError::Conflict(
                "当前目标缺少满足范围、代际、指纹和保留期要求的 validated backup".into(),
            ));
        }

        let intent_transaction = self.persistence.begin().await?;
        self.acquire_or_renew_operation_lease(&*intent_transaction, &snapshot, now)
            .await?;
        let mut intent = intent_transaction.lock_migration(migration_id).await?;
        if intent.state == TenantDataMigrationRecord::STATE_FINALIZED {
            let same_key = intent.finalize_idempotency_key_hash.as_deref() == Some(&key_hash);
            intent_transaction
                .release_lease(&intent.tenant_id, &intent.switch_token)
                .await?;
            intent_transaction.commit().await?;
            if same_key {
                return self.migration_view(intent).await;
            }
            return Err(AppError::Conflict("迁移已经完成保留期清理".into()));
        }
        if intent.state != TenantDataMigrationRecord::STATE_RETENTION_PENDING {
            return Err(AppError::Conflict("迁移状态不允许 finalize".into()));
        }
        match intent.finalize_idempotency_key_hash.as_deref() {
            Some(existing) if existing != key_hash => {
                return Err(AppError::Conflict("迁移已有其他 finalize 请求".into()));
            }
            _ => {}
        }
        if intent.retention_until.is_none_or(|until| until > now) {
            return Err(AppError::Conflict("源数据保留期尚未结束".into()));
        }
        let intent_not_before = intent
            .activated_at
            .or(intent.succeeded_at)
            .ok_or_else(|| AppError::Conflict("迁移缺少激活时间".into()))?;
        if !intent_transaction
            .has_validated_backup(&intent, intent_not_before, now)
            .await?
        {
            return Err(AppError::Conflict(
                "validated backup 不再满足 finalize 条件".into(),
            ));
        }
        if intent.finalize_idempotency_key_hash.is_none() {
            intent.finalize_idempotency_key_hash = Some(key_hash.clone());
            intent.finalized_by = Some(actor.user_id);
            intent.finalize_requested_at = Some(now);
            intent.updated_at = now;
            intent = intent_transaction.save_migration(intent).await?;
        }
        let background_job_id = intent
            .background_job_id
            .ok_or_else(|| AppError::Conflict("迁移缺少权威后台任务".into()))?;
        if !self
            .queue
            .reactivate_linked_in_transaction(
                intent_transaction.background_jobs(),
                background_job_id,
                super::TENANT_DATA_MIGRATION_JOB_TYPE,
                "migration_id",
                migration_id,
                now,
            )
            .await?
        {
            return Err(AppError::Conflict("迁移权威后台任务关联无效".into()));
        }
        intent_transaction.commit().await?;
        snapshot = intent;
        self.queue.notify_background_jobs().await;
        self.migration_view(snapshot).await
    }
}
