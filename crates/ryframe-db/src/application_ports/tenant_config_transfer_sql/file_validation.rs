use super::*;

/// 在业务引用写入前锁定内部文件，并恢复仍处于宽限期的去重文件。
///
/// 调用方必须先持有租户配置栅栏；文件锁用于和延迟清理声明串行化。
pub(crate) async fn ensure_config_package_file_ready_in_txn(
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
    if file.upload_status == crate::entities::sys_file::Model::UPLOAD_STATUS_READY {
        return Ok(());
    }
    if file.upload_status == crate::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
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
