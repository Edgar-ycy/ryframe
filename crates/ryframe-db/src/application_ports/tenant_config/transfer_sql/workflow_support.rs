use super::*;

pub(crate) async fn ensure_requester_snapshot_in_txn(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    requester: TenantConfigRequesterRecord,
    fence: TenantConfigurationFenceRecord,
    database_now: DateTime<Utc>,
) -> AppResult<()> {
    if requester.tenant_id != tenant_id
        || requester.tenant_authorization_epoch != fence.authorization_epoch
    {
        return Err(AppError::Conflict(
            "申请人授权在执行期间发生变化，请重新发起操作".into(),
        ));
    }
    let tenant = tenant::Entity::find()
        .filter(tenant::Column::TenantId.eq(tenant_id))
        .one(transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
    if !tenant.is_available(database_now) {
        return Err(AppError::Authorization("申请人的租户已停用或到期".into()));
    }
    let current_user = user::Entity::find_by_id(requester.user_id)
        .filter(user::Column::TenantId.eq(tenant_id))
        .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
        .one(transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::NotFound("操作申请人不存在".into()))?;
    if !current_user.is_enabled()
        || current_user.authorization_version != requester.user_authorization_version
    {
        return Err(AppError::Authorization(
            "申请人账号或授权在执行期间发生变化".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_role_quota_for_plan_in_txn(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    plan_items: &[TenantConfigTransferItemRecord],
) -> AppResult<()> {
    let create_count = plan_items
        .iter()
        .filter(|item| {
            item.resource_type == "role"
                && item.action == TenantConfigTransferItemRecord::ACTION_CREATE
        })
        .count() as u64;
    if create_count == 0 {
        return Ok(());
    }
    let tenant = tenant::Entity::find()
        .filter(tenant::Column::TenantId.eq(tenant_id))
        .one(transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
    let active_count = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .count(transaction)
        .await
        .map_err(database_error)?;
    let limit = u64::try_from(tenant.max_roles).unwrap_or_default();
    if limit > 0 && active_count.saturating_add(create_count) > limit {
        return Err(AppError::Validation(format!(
            "配置应用将使租户角色数超过上限（当前 {active_count}，新增 {create_count}，上限 {limit}）"
        )));
    }
    Ok(())
}

pub(crate) async fn mark_plan_outcome(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    transfer_id: i64,
    outcome: &str,
) -> AppResult<()> {
    tenant_config_transfer_item::Entity::update_many()
        .col_expr(
            tenant_config_transfer_item::Column::Outcome,
            Expr::value(outcome),
        )
        .col_expr(
            tenant_config_transfer_item::Column::UpdatedAt,
            Expr::cust("UTC_TIMESTAMP(6)"),
        )
        .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
        .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
        .filter(
            tenant_config_transfer_item::Column::Action
                .ne(TenantConfigTransferItemRecord::ACTION_UNCHANGED),
        )
        .exec(transaction)
        .await
        .map_err(database_error)?;
    if outcome == TenantConfigTransferItemRecord::OUTCOME_APPLIED {
        tenant_config_transfer_item::Entity::update_many()
            .col_expr(
                tenant_config_transfer_item::Column::Outcome,
                Expr::value(TenantConfigTransferItemRecord::OUTCOME_SKIPPED),
            )
            .col_expr(
                tenant_config_transfer_item::Column::UpdatedAt,
                Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
            .filter(
                tenant_config_transfer_item::Column::Action
                    .eq(TenantConfigTransferItemRecord::ACTION_UNCHANGED),
            )
            .exec(transaction)
            .await
            .map_err(database_error)?;
    }
    Ok(())
}
