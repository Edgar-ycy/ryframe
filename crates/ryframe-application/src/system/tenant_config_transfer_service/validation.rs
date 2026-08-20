use super::*;

pub(super) fn ensure_preview_identity(
    transfer: &tenant_config_transfer::Model,
    source: &ParsedTenantConfigPackage,
    target: &TenantConfigPackageResources,
    fence: ryframe_db::TenantConfigurationFence,
) -> AppResult<()> {
    if transfer.target_configuration_version != fence.configuration_version
        || transfer.target_authorization_epoch != fence.authorization_epoch
    {
        return Err(AppError::Conflict("目标配置已经变化，请重新预览".into()));
    }
    let expected = sha256_json(&PlanHashInput {
        source_package_sha256: &source.package_sha256,
        target_resources_sha256: sha256_hex(&canonical_resources(target)?),
        target_configuration_version: fence.configuration_version,
        target_authorization_epoch: fence.authorization_epoch,
    })?;
    if transfer.plan_hash.as_deref() != Some(expected.as_str()) {
        return Err(AppError::Conflict("预览计划已经失效，请重新预览".into()));
    }
    Ok(())
}

pub(super) fn validate_sha256(value: &str) -> AppResult<()> {
    if value.len() == 64 && value.bytes().all(|value| value.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::Validation(
            "幂等键哈希必须是 64 位十六进制 SHA-256".into(),
        ))
    }
}

pub(super) fn transfer_request_fingerprint(request_kind: &str, bundle_id: i64) -> String {
    sha256_hex(format!("{request_kind}:{bundle_id}").as_bytes())
}

pub(super) fn ensure_transfer_request_identity(
    transfer: &tenant_config_transfer::Model,
    request_kind: &str,
    request_fingerprint: &str,
) -> AppResult<()> {
    if transfer.request_kind == request_kind && transfer.request_fingerprint == request_fingerprint
    {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "Idempotency-Key 已用于其他配置迁移请求".into(),
        ))
    }
}

pub(super) fn parse_file_id(value: &str) -> AppResult<i64> {
    value
        .parse::<i64>()
        .map_err(|_| AppError::Internal("文件标识格式无效".into()))
}

/// 在业务引用写入前锁定内部文件，并恢复仍处于宽限期的去重文件。
///
/// 调用方必须先持有租户配置栅栏；文件锁用于和延迟清理声明串行化。
pub(super) async fn ensure_config_package_file_ready_in_txn(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    file_id: i64,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let file = FileRepository
        .find_by_id_any_status_for_update(transaction, tenant_id, file_id)
        .await?
        .ok_or_else(|| AppError::Conflict("配置包文件不存在或已经清理".into()))?;
    if file.bucket != CONFIG_PACKAGE_BUCKET {
        return Err(AppError::Authorization("配置包文件存储边界不匹配".into()));
    }
    if file.upload_status == ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_READY {
        return Ok(());
    }
    if file.upload_status == ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
        && FileRepository
            .restore_file_for_reference_in_txn(
                transaction,
                tenant_id,
                file_id,
                CONFIG_PACKAGE_BUCKET,
                now,
            )
            .await?
    {
        return Ok(());
    }
    Err(AppError::Conflict(
        "配置包文件尚未就绪或清理宽限期已经结束".into(),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn new_transfer_model(
    tenant_id: &str,
    bundle_id: i64,
    idempotency_key_hash: &str,
    request_kind: &str,
    request_fingerprint: &str,
    requested_by: i64,
    configuration_version: i64,
    authorization_epoch: i32,
    now: DateTime<Utc>,
) -> AppResult<tenant_config_transfer::Model> {
    Ok(tenant_config_transfer::Model {
        id: next_id()?,
        tenant_id: tenant_id.to_owned(),
        bundle_id,
        idempotency_key_hash: idempotency_key_hash.to_owned(),
        request_kind: request_kind.to_owned(),
        request_fingerprint: request_fingerprint.to_owned(),
        status: tenant_config_transfer::Model::STATUS_PREVIEW_READY.to_owned(),
        target_configuration_version: configuration_version,
        target_authorization_epoch: authorization_epoch,
        plan_hash: None,
        preview_calculated_at: None,
        preview_background_job_id: None,
        apply_background_job_id: None,
        rollback_background_job_id: None,
        snapshot_file_id: None,
        applied_configuration_version: None,
        applied_authorization_epoch: None,
        change_counts: json!({}),
        error_summary: None,
        requested_by,
        rollback_expires_at: None,
        created_at: now,
        updated_at: now,
    })
}

pub(super) fn ensure_bundle_available(
    bundle: &tenant_config_bundle::Model,
    now: DateTime<Utc>,
) -> AppResult<()> {
    if bundle.status != tenant_config_bundle::Model::STATUS_SUCCEEDED {
        return Err(AppError::Conflict("配置包尚未生成成功".into()));
    }
    if bundle
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        return Err(AppError::Conflict("配置包已经过期".into()));
    }
    Ok(())
}

pub(super) fn validate_operation_request(
    transfer: &tenant_config_transfer::Model,
    operation: &TransferOperationRequest,
) -> AppResult<()> {
    let valid = match operation {
        TransferOperationRequest::Preview => matches!(
            transfer.status.as_str(),
            tenant_config_transfer::Model::STATUS_PREVIEW_READY
                | tenant_config_transfer::Model::STATUS_PREVIEWED
                | tenant_config_transfer::Model::STATUS_FAILED
        ),
        TransferOperationRequest::Apply(_) => {
            transfer.status == tenant_config_transfer::Model::STATUS_PREVIEWED
        }
        TransferOperationRequest::Rollback => {
            transfer.status == tenant_config_transfer::Model::STATUS_APPLIED
        }
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "配置迁移当前状态不允许执行该操作".into(),
        ))
    }
}

pub(super) fn validate_operation_replay_identity(
    transfer: &tenant_config_transfer::Model,
    operation: &TransferOperationRequest,
) -> AppResult<()> {
    if let TransferOperationRequest::Apply(command) = operation
        && (transfer.plan_hash.as_deref() != Some(command.plan_hash.as_str())
            || transfer.target_configuration_version != command.target_configuration_version
            || transfer.target_authorization_epoch != command.target_authorization_epoch)
    {
        return Err(AppError::Conflict(
            "Idempotency-Key 已用于其他配置应用请求".into(),
        ));
    }
    Ok(())
}

pub(super) fn operation_job_id(
    transfer: &tenant_config_transfer::Model,
    operation: &TransferOperationRequest,
) -> Option<i64> {
    match operation {
        TransferOperationRequest::Preview => transfer.preview_background_job_id,
        TransferOperationRequest::Apply(_) => transfer.apply_background_job_id,
        TransferOperationRequest::Rollback => transfer.rollback_background_job_id,
    }
}

pub(super) fn job_tenant(job: &ClaimedBackgroundJob) -> AppResult<&str> {
    job.tenant_id
        .as_deref()
        .ok_or_else(|| AppError::Validation("配置迁移任务缺少租户".into()))
}

pub(super) fn payload_id(job: &ClaimedBackgroundJob, key: &str) -> AppResult<i64> {
    let value = job
        .payload
        .get(key)
        .ok_or_else(|| AppError::Validation("配置迁移任务载荷缺少资源标识".into()))?;
    match value {
        Value::String(value) => value
            .parse()
            .map_err(|_| AppError::Validation("配置迁移任务资源标识无效".into())),
        Value::Number(value) => value
            .as_i64()
            .ok_or_else(|| AppError::Validation("配置迁移任务资源标识无效".into())),
        _ => Err(AppError::Validation("配置迁移任务资源标识无效".into())),
    }
}

pub(super) fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

pub(super) fn internal_json_error(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(error.to_string())
}
