use super::*;
use ryframe_application::{DataScopeProjectionPublisher, ports::tenant_data::TenantDataFence};
use ryframe_db::{
    entities::tenant::{data_migration, data_migration_item, data_placement},
    repositories::{CreateTenantDataMigration, TenantDataRepository},
};
use ryframe_kernel::AppError;
use time::{OffsetDateTime, UtcOffset, macros::datetime};

const TOKEN: &str = "migration-projection";
const GENERATION: i64 = 3;
const MIGRATION_ID: i64 = 901;
const ITEM_ID: i64 = 902;
const PLACEMENT_UPDATED_AT: OffsetDateTime = datetime!(2026-08-21 06:32:03.123456 +05:30);
const MIGRATION_CREATED_AT: OffsetDateTime = datetime!(2026-08-21 10:35:06.234567 +05:30);
const MIGRATION_CUT_OVER_AT: OffsetDateTime = datetime!(2026-08-22 10:35:06.987654 +05:30);
const ITEM_VERIFIED_AT: OffsetDateTime = datetime!(2026-08-22 02:08:09.654321 -05:00);

pub(super) async fn exercise(
    control: &DatabaseConnection,
    target: &DatabaseConnection,
    router: &TenantDatabaseRouter,
) -> Result<(), String> {
    let target_key = router
        .targets()
        .metadata()
        .await
        .into_iter()
        .next()
        .ok_or("缺少测试目标")?
        .key;
    prepare_activation(control, target, &target_key).await?;
    verify_migration_timestamps(control).await?;
    verify_renewed_lease_time(control).await?;
    let publisher = ryframe_tenant_db::application_ports::data_scope_projection::publisher(
        ControlDatabaseCluster::single(control.clone()),
        router.clone(),
    );
    let fence = request(&target_key);
    let before = projection_image(target).await?;
    assert!(matches!(
        publisher.publish(&event(4)).await,
        Err(AppError::TenantDataMaintenance(..))
    ));
    assert_eq!(projection_image(target).await?, before);
    // 迁回旧目标时，旧代次派生投影可以在精确新围栏下重建。
    assert_eq!(
        publisher
            .publish_for_migration(fence)
            .await
            .map_err(message)?,
        4
    );
    let published = projection_image(target).await?;
    assert_ne!(published, before);
    assert_eq!(
        publisher
            .publish_for_migration(fence)
            .await
            .map_err(message)?,
        4
    );
    assert_eq!(projection_image(target).await?, published);
    require_maintenance(control).await?;
    reject_same_count_corruption(target, &*publisher, fence).await?;
    reject_control_drift(control, target, &*publisher, fence).await?;
    reject_target_drift(target, &*publisher, fence, target_key == TARGET).await?;
    verify_before_commit(control, target, &*publisher, fence).await?;
    require_maintenance(control).await
}

async fn verify_renewed_lease_time(control: &DatabaseConnection) -> Result<(), String> {
    use ryframe_application::{AuthorizationCache, TransactionAuditMode};
    use time::Duration;

    #[derive(FromQueryResult)]
    struct LeaseTime {
        expires_at: OffsetDateTime,
    }

    let store = ryframe_tenant_db::application_ports::tenant_data::tracking(
        ControlDatabaseCluster::single(control.clone()),
        AuthorizationCache::disabled(),
    );
    let now = store.database_now().await.map_err(message)?;
    assert_eq!(now.offset(), UtcOffset::UTC);
    let expires_at = (now + Duration::hours(24))
        .to_offset(UtcOffset::from_hms(5, 30, 0).expect("固定测试偏移有效"));
    let transaction = store.begin().await.map_err(message)?;
    assert!(
        transaction
            .renew_lease("system", TOKEN, expires_at)
            .await
            .map_err(message)?
    );
    transaction
        .commit(TransactionAuditMode::Skip)
        .await
        .map_err(message)?;
    let stored = LeaseTime::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT expires_at FROM sys_tenant_operation_lease WHERE tenant_id='system' AND owner_token=?",
        [TOKEN.into()],
    ))
    .one(control)
    .await
    .map_err(message)?
    .ok_or("找不到已续期的迁移租约")?;
    assert_eq!(stored.expires_at.offset(), UtcOffset::UTC);
    assert_eq!(stored.expires_at, expires_at);
    Ok(())
}

fn request(target_key: &str) -> TenantDataFence<'_> {
    TenantDataFence {
        tenant_id: "system",
        target_key,
        generation: GENERATION,
        switch_token: TOKEN,
    }
}

async fn prepare_activation(
    control: &DatabaseConnection,
    target: &DatabaseConnection,
    target_key: &str,
) -> Result<(), String> {
    prepare_control_rows(control, target_key).await?;
    mysql::execute(
        control,
        "UPDATE sys_tenant SET data_scope_version=4 WHERE tenant_id='system'",
    )
    .await?;
    control.execute_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "INSERT INTO sys_tenant_operation_lease \
         (tenant_id,owner_token,operation,resource_type,resource_id,expires_at,created_at,updated_at) \
         VALUES ('system',?,'tenant_data.migration','tenant_data_migration','901', \
          DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 1 HOUR),UTC_TIMESTAMP(6),UTC_TIMESTAMP(6))",
        [TOKEN.into()],
    )).await.map_err(message)?;
    mysql::execute(target, "UPDATE biz_tenant_fence SET placement_generation=3, switch_token='migration-projection', state='active' WHERE tenant_id='system'").await?;
    if target_key == TARGET {
        mysql::execute(target, "UPDATE biz_tenant_target_slot SET placement_generation=3, switch_token='migration-projection' WHERE slot_id=1").await?;
    }
    Ok(())
}

async fn prepare_control_rows(
    control: &DatabaseConnection,
    target_key: &str,
) -> Result<(), String> {
    let repository = TenantDataRepository;
    let transaction = control.begin().await.map_err(message)?;
    let mut placement = repository
        .lock_placement_in_txn(&transaction, "system")
        .await
        .map_err(message)?;
    placement.current_target_key = target_key.into();
    placement.placement_generation = GENERATION;
    placement.switch_token = TOKEN.into();
    placement.state = data_placement::Model::STATE_MAINTENANCE.into();
    placement.updated_at = PLACEMENT_UPDATED_AT;
    repository
        .save_placement_in_txn(&transaction, placement)
        .await
        .map_err(message)?;

    let (target_mode, target_kind) = if target_key == TARGET {
        ("dedicated", "mysql")
    } else {
        ("shared", "control")
    };
    let command = CreateTenantDataMigration {
        id: MIGRATION_ID,
        tenant_id: "system".into(),
        source_target_key: "old-target".into(),
        target_key: target_key.into(),
        source_target_mode: "dedicated".into(),
        source_target_kind: "mysql".into(),
        target_target_mode: target_mode.into(),
        target_target_kind: target_kind.into(),
        source_generation: 2,
        source_switch_token: "old-token".into(),
        target_generation: GENERATION,
        source_schema_fingerprint: "a".repeat(64),
        target_schema_fingerprint: "a".repeat(64),
        plan_hash: "b".repeat(64),
        create_idempotency_key_hash: "c".repeat(64),
        switch_token: TOKEN.into(),
        operator_id: 1,
        retention_hours: 168,
        now: MIGRATION_CREATED_AT,
    };
    let mut migration = repository
        .insert_migration_in_txn(&transaction, command)
        .await
        .map_err(message)?;
    migration.state = data_migration::Model::STATE_ACTIVATING.into();
    migration.cut_over_at = Some(MIGRATION_CUT_OVER_AT);
    migration.updated_at = MIGRATION_CUT_OVER_AT;
    repository
        .save_migration_in_txn(&transaction, migration)
        .await
        .map_err(message)?;

    let item = data_migration_item::Model {
        id: ITEM_ID,
        migration_id: MIGRATION_ID,
        table_name: "biz_order".into(),
        copy_order: 1,
        state: data_migration_item::Model::STATE_VERIFIED.into(),
        cursor_json: None,
        source_row_count: Some(0),
        target_row_count: Some(0),
        source_digest: None,
        target_digest: None,
        error_code: None,
        error_detail: None,
        copy_started_at: None,
        copied_at: None,
        verified_at: Some(ITEM_VERIFIED_AT),
        cleanup_state: data_migration_item::Model::CLEANUP_PENDING.into(),
        cleanup_row_count: 0,
        created_at: MIGRATION_CREATED_AT,
        updated_at: ITEM_VERIFIED_AT,
    };
    repository
        .insert_item(&transaction, item)
        .await
        .map_err(message)?;
    transaction.commit().await.map_err(message)
}

async fn verify_migration_timestamps(control: &DatabaseConnection) -> Result<(), String> {
    let repository = TenantDataRepository;
    let placement = repository
        .placement(control, "system")
        .await
        .map_err(message)?
        .ok_or("找不到迁移放置")?;
    let migration = repository
        .migration(control, MIGRATION_ID)
        .await
        .map_err(message)?
        .ok_or("找不到迁移主记录")?;
    let items = repository
        .items(control, MIGRATION_ID)
        .await
        .map_err(message)?;
    let [item] = items.as_slice() else {
        return Err("迁移只应包含一条测试表项".into());
    };
    assert_eq!(placement.updated_at, PLACEMENT_UPDATED_AT);
    assert_eq!(placement.updated_at.offset(), UtcOffset::UTC);
    assert_eq!(migration.prechecked_at, Some(MIGRATION_CREATED_AT));
    assert_eq!(migration.created_at, MIGRATION_CREATED_AT);
    assert_eq!(migration.cut_over_at, Some(MIGRATION_CUT_OVER_AT));
    assert_eq!(migration.updated_at, MIGRATION_CUT_OVER_AT);
    assert_eq!(migration.queued_at, None);
    assert_eq!(item.verified_at, Some(ITEM_VERIFIED_AT));
    assert_eq!(item.copied_at, None);
    let physical = rows(
        control,
        "SELECT CONCAT_WS('|', DATE_FORMAT(p.updated_at,'%Y-%m-%d %H:%i:%s.%f'), \
            DATE_FORMAT(m.prechecked_at,'%Y-%m-%d %H:%i:%s.%f'), \
            DATE_FORMAT(m.cut_over_at,'%Y-%m-%d %H:%i:%s.%f'), \
            DATE_FORMAT(i.verified_at,'%Y-%m-%d %H:%i:%s.%f')) AS item \
         FROM sys_tenant_data_placement p \
         JOIN sys_tenant_data_migration m ON m.tenant_id=p.tenant_id \
         JOIN sys_tenant_data_migration_item i ON i.migration_id=m.id \
         WHERE p.tenant_id='system' AND m.id=901 AND i.id=902",
    )
    .await?;
    assert_eq!(
        physical,
        [
            "2026-08-21 01:02:03.123456|2026-08-21 05:05:06.234567|2026-08-22 05:05:06.987654|2026-08-22 07:08:09.654321"
        ]
    );
    Ok(())
}

async fn reject_same_count_corruption(
    target: &DatabaseConnection,
    publisher: &dyn DataScopeProjectionPublisher,
    fence: TenantDataFence<'_>,
) -> Result<(), String> {
    for (damage, restore) in [
        (
            "UPDATE biz_data_scope_owner SET dept_id=10 WHERE tenant_id='system' AND user_id=1",
            "UPDATE biz_data_scope_owner SET dept_id=11 WHERE tenant_id='system' AND user_id=1",
        ),
        (
            "UPDATE biz_data_scope_dept SET ancestors='0,99' WHERE tenant_id='system' AND dept_id=11",
            "UPDATE biz_data_scope_dept SET ancestors='0,10' WHERE tenant_id='system' AND dept_id=11",
        ),
    ] {
        mysql::execute(target, damage).await?;
        let damaged = projection_image(target).await?;
        assert!(matches!(publisher.publish_for_migration(fence).await,
            Err(AppError::Conflict(ref detail)) if detail.contains("实际身份、部门行")));
        assert_eq!(projection_image(target).await?, damaged);
        mysql::execute(target, restore).await?;
        assert_eq!(
            publisher
                .publish_for_migration(fence)
                .await
                .map_err(message)?,
            4
        );
    }
    Ok(())
}

async fn reject_control_drift(
    control: &DatabaseConnection,
    target: &DatabaseConnection,
    publisher: &dyn DataScopeProjectionPublisher,
    fence: TenantDataFence<'_>,
) -> Result<(), String> {
    for (damage, restore, expected) in [
        (
            "UPDATE sys_tenant_data_placement SET state='active' WHERE tenant_id='system'",
            "UPDATE sys_tenant_data_placement SET state='maintenance' WHERE tenant_id='system'",
            "maintenance placement",
        ),
        (
            "UPDATE sys_tenant_data_placement SET placement_generation=4 WHERE tenant_id='system'",
            "UPDATE sys_tenant_data_placement SET placement_generation=3 WHERE tenant_id='system'",
            "maintenance placement",
        ),
        (
            "UPDATE sys_tenant_data_migration SET state='cutting_over' WHERE id=901",
            "UPDATE sys_tenant_data_migration SET state='activating' WHERE id=901",
            "activating",
        ),
        (
            "UPDATE sys_tenant_operation_lease SET owner_token='wrong' WHERE tenant_id='system'",
            "UPDATE sys_tenant_operation_lease SET owner_token='migration-projection' WHERE tenant_id='system'",
            "不属于当前迁移",
        ),
        (
            "UPDATE sys_tenant_operation_lease SET expires_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 HOUR) WHERE tenant_id='system'",
            "UPDATE sys_tenant_operation_lease SET expires_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 1 HOUR) WHERE tenant_id='system'",
            "已过期",
        ),
    ] {
        let before = projection_image(target).await?;
        mysql::execute(control, damage).await?;
        let error = publisher.publish_for_migration(fence).await.unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(projection_image(target).await?, before);
        mysql::execute(control, restore).await?;
    }
    Ok(())
}

async fn reject_target_drift(
    target: &DatabaseConnection,
    publisher: &dyn DataScopeProjectionPublisher,
    fence: TenantDataFence<'_>,
    dedicated: bool,
) -> Result<(), String> {
    for (damage, restore) in [
        (
            "UPDATE biz_tenant_fence SET state='frozen' WHERE tenant_id='system'",
            "UPDATE biz_tenant_fence SET state='active' WHERE tenant_id='system'",
        ),
        (
            "UPDATE biz_tenant_fence SET switch_token='wrong' WHERE tenant_id='system'",
            "UPDATE biz_tenant_fence SET switch_token='migration-projection' WHERE tenant_id='system'",
        ),
    ] {
        mysql::execute(target, damage).await?;
        let before = projection_image(target).await?;
        assert!(matches!(
            publisher.publish_for_migration(fence).await,
            Err(AppError::TenantDataMaintenance(..))
        ));
        assert_eq!(projection_image(target).await?, before);
        mysql::execute(target, restore).await?;
    }
    if dedicated {
        mysql::execute(
            target,
            "UPDATE biz_tenant_target_slot SET switch_token='wrong' WHERE slot_id=1",
        )
        .await?;
        let before = projection_image(target).await?;
        assert!(publisher.publish_for_migration(fence).await.is_err());
        assert_eq!(projection_image(target).await?, before);
        mysql::execute(
            target,
            "UPDATE biz_tenant_target_slot SET switch_token='migration-projection' WHERE slot_id=1",
        )
        .await?;
    }
    Ok(())
}

async fn verify_before_commit(
    control: &DatabaseConnection,
    target: &DatabaseConnection,
    publisher: &dyn DataScopeProjectionPublisher,
    fence: TenantDataFence<'_>,
) -> Result<(), String> {
    mysql::execute(
        control,
        "UPDATE sys_tenant SET data_scope_version=5 WHERE tenant_id='system'",
    )
    .await?;
    let before = projection_image(target).await?;
    target
        .execute_unprepared(
            "CREATE TRIGGER projection_test_corrupt_owner BEFORE INSERT ON biz_data_scope_owner \
         FOR EACH ROW SET NEW.dept_id = NULL",
        )
        .await
        .map_err(message)?;
    let error = publisher.publish_for_migration(fence).await.unwrap_err();
    assert!(matches!(error, AppError::Conflict(ref detail) if detail.contains("实际身份、部门行")));
    assert_eq!(projection_image(target).await?, before);
    target
        .execute_unprepared("DROP TRIGGER projection_test_corrupt_owner")
        .await
        .map_err(message)?;
    assert_eq!(
        publisher
            .publish_for_migration(fence)
            .await
            .map_err(message)?,
        5
    );
    Ok(())
}

async fn require_maintenance(control: &DatabaseConnection) -> Result<(), String> {
    mysql::require_count(
        control,
        "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM sys_tenant_data_placement \
         WHERE tenant_id='system' AND state='maintenance' AND placement_generation=3",
        1,
    )
    .await
}
