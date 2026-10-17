use std::sync::Arc;

use ryframe_application::{DataScopeProjectionEvent, DataScopeProjectionPublisher};
use ryframe_db::{ControlDatabaseCluster, DbResultExt};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    AccessMode, ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, IsolationLevel,
    Statement, TransactionTrait, Value,
};
use sha2::{Digest, Sha256};

use crate::TenantDatabaseRouter;

use super::map_error;

const PAGE_SIZE: u64 = 1000;

#[derive(Debug, FromQueryResult)]
struct TenantVersionRow {
    data_scope_version: i64,
}

#[derive(Debug, FromQueryResult)]
struct OwnerRow {
    id: i64,
    dept_id: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct DeptRow {
    id: i64,
    ancestors: String,
}

#[derive(Debug, FromQueryResult)]
struct ProjectionRow {
    source_version: i64,
    build_token: String,
    placement_generation: i64,
    switch_token: String,
    digest: String,
    owner_count: i64,
    dept_count: i64,
    ready: bool,
}

/// 从控制库权威身份事实构建租户库最小数据范围投影。
pub struct TenantDataScopeProjectionPublisher {
    control: ControlDatabaseCluster,
    router: TenantDatabaseRouter,
}

pub fn publisher(
    control: ControlDatabaseCluster,
    router: TenantDatabaseRouter,
) -> Arc<dyn DataScopeProjectionPublisher> {
    Arc::new(TenantDataScopeProjectionPublisher { control, router })
}

#[async_trait::async_trait]
impl DataScopeProjectionPublisher for TenantDataScopeProjectionPublisher {
    async fn publish(&self, event: &DataScopeProjectionEvent) -> AppResult<()> {
        let control = self
            .control
            .write()
            .begin_with_config(
                Some(IsolationLevel::RepeatableRead),
                Some(AccessMode::ReadOnly),
            )
            .await
            .db()?;
        let result = self.publish_with_control_snapshot(&control, event).await;
        let finish = control.commit().await.db();
        result.and(finish)
    }
}

impl TenantDataScopeProjectionPublisher {
    async fn publish_with_control_snapshot(
        &self,
        control: &DatabaseTransaction,
        event: &DataScopeProjectionEvent,
    ) -> AppResult<()> {
        let version = TenantVersionRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT `data_scope_version` FROM `sys_tenant` WHERE `tenant_id` = ? LIMIT 1 FOR SHARE",
            [event.tenant_id.clone().into()],
        ))
        .one(control)
        .await
        .db()?
        .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        if version.data_scope_version < event.source_version {
            return Err(AppError::Conflict("数据范围版本早于事件版本".into()));
        }
        let source_version = version.data_scope_version;
        let session = self
            .router
            .resolve(&event.tenant_id)
            .await
            .map_err(map_error)?;
        let target = session.begin_write().await.map_err(map_error)?;
        let result = self
            .publish_to_target(control, &target, event, source_version)
            .await;
        match result {
            Ok(()) => target.commit().await.db(),
            Err(error) => {
                let _ = target.rollback().await;
                Err(error)
            }
        }
    }

    async fn publish_to_target(
        &self,
        control: &DatabaseTransaction,
        target: &DatabaseTransaction,
        event: &DataScopeProjectionEvent,
        source_version: i64,
    ) -> AppResult<()> {
        let placement = session_identity(target, &event.tenant_id).await?;
        let metadata = load_projection(target, &event.tenant_id).await?;
        if let Some(metadata) = &metadata
            && metadata.source_version > source_version
        {
            return Ok(());
        }
        if let Some(metadata) = &metadata
            && metadata.source_version == source_version
            && metadata.ready
            && metadata.placement_generation == placement.generation
            && metadata.switch_token == placement.switch_token
            && valid_digest(&metadata.digest)
            && projection_rows_match(target, &event.tenant_id, metadata).await?
        {
            return Ok(());
        }
        let token = build_token(
            &event.tenant_id,
            source_version,
            &placement.target_key,
            placement.generation,
            &placement.switch_token,
        );
        clear_build(target, &event.tenant_id, &token).await?;
        let (owner_count, dept_count, digest) =
            copy_projection(control, target, &event.tenant_id, &token).await?;
        save_metadata(
            target,
            &event.tenant_id,
            &ProjectionPublication {
                source_version,
                token: &token,
                placement: &placement,
                digest: &digest,
                owner_count,
                dept_count,
            },
        )
        .await?;
        remove_old_builds(target, &event.tenant_id, &token).await?;
        Ok(())
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

struct ProjectionPlacement {
    target_key: String,
    generation: i64,
    switch_token: String,
}

struct ProjectionPublication<'a> {
    source_version: i64,
    token: &'a str,
    placement: &'a ProjectionPlacement,
    digest: &'a str,
    owner_count: i64,
    dept_count: i64,
}

async fn session_identity(
    target: &DatabaseTransaction,
    tenant_id: &str,
) -> AppResult<ProjectionPlacement> {
    let row = ProjectionPlacementRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT `target_key`, `placement_generation`, `switch_token` \
         FROM `biz_tenant_fence` WHERE `tenant_id` = ? LIMIT 1",
        [tenant_id.to_string().into()],
    ))
    .one(target)
    .await
    .db()?
    .ok_or_else(|| AppError::TenantDataMaintenance("租户数据围栏不存在".into(), 5))?;
    Ok(ProjectionPlacement {
        target_key: row.target_key,
        generation: row.placement_generation,
        switch_token: row.switch_token,
    })
}

#[derive(Debug, FromQueryResult)]
struct ProjectionPlacementRow {
    target_key: String,
    placement_generation: i64,
    switch_token: String,
}

async fn load_projection(
    target: &DatabaseTransaction,
    tenant_id: &str,
) -> AppResult<Option<ProjectionRow>> {
    ProjectionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT `source_version`, `build_token`, `placement_generation`, `switch_token`, \
         `digest`, `owner_count`, `dept_count`, `ready` \
         FROM `biz_data_scope_projection` WHERE `tenant_id` = ? LIMIT 1 FOR UPDATE",
        [tenant_id.to_string().into()],
    ))
    .one(target)
    .await
    .db()
}

async fn projection_rows_match(
    target: &DatabaseTransaction,
    tenant_id: &str,
    metadata: &ProjectionRow,
) -> AppResult<bool> {
    let owner_count = count_rows(
        target,
        "biz_data_scope_owner",
        tenant_id,
        &metadata.build_token,
    )
    .await?;
    let dept_count = count_rows(
        target,
        "biz_data_scope_dept",
        tenant_id,
        &metadata.build_token,
    )
    .await?;
    Ok(owner_count == metadata.owner_count && dept_count == metadata.dept_count)
}

async fn count_rows(
    target: &DatabaseTransaction,
    table: &str,
    tenant_id: &str,
    token: &str,
) -> AppResult<i64> {
    let sql = format!(
        "SELECT CAST(COUNT(*) AS SIGNED) AS `count` FROM `{table}` \
         WHERE `tenant_id` = ? AND `build_token` = ?"
    );
    CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        sql,
        [tenant_id.to_string().into(), token.to_string().into()],
    ))
    .one(target)
    .await
    .db()?
    .map(|row| row.count)
    .ok_or_else(|| AppError::Database("投影行数查询未返回结果".into()))
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

async fn clear_build(target: &DatabaseTransaction, tenant_id: &str, token: &str) -> AppResult<()> {
    for table in ["biz_data_scope_owner", "biz_data_scope_dept"] {
        let sql = format!("DELETE FROM `{table}` WHERE `tenant_id` = ? AND `build_token` = ?");
        target
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                sql,
                [tenant_id.to_string().into(), token.to_string().into()],
            ))
            .await
            .db()?;
    }
    Ok(())
}

async fn copy_projection(
    control: &DatabaseTransaction,
    target: &DatabaseTransaction,
    tenant_id: &str,
    token: &str,
) -> AppResult<(i64, i64, String)> {
    let mut hasher = Sha256::new();
    let owner_count = copy_owners(control, target, tenant_id, token, &mut hasher).await?;
    let dept_count = copy_departments(control, target, tenant_id, token, &mut hasher).await?;
    Ok((owner_count, dept_count, hex::encode(hasher.finalize())))
}

async fn copy_owners(
    control: &DatabaseTransaction,
    target: &DatabaseTransaction,
    tenant_id: &str,
    token: &str,
    hasher: &mut Sha256,
) -> AppResult<i64> {
    let mut cursor = 0_i64;
    let mut count = 0_i64;
    loop {
        let rows = OwnerRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT `id`, `dept_id` FROM `sys_user` \
             WHERE `tenant_id` = ? AND `del_flag` = '0' AND `id` > ? \
             ORDER BY `id` LIMIT ?",
            [
                tenant_id.to_string().into(),
                cursor.into(),
                PAGE_SIZE.into(),
            ],
        ))
        .all(control)
        .await
        .db()?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            hasher.update(format!("u:{}:{:?}\n", row.id, row.dept_id));
        }
        insert_owner_batch(target, tenant_id, token, &rows).await?;
        count += i64::try_from(rows.len()).unwrap_or(i64::MAX);
        cursor = rows.last().map(|row| row.id).unwrap_or(cursor);
        if rows.len() < usize::try_from(PAGE_SIZE).unwrap_or(usize::MAX) {
            break;
        }
    }
    Ok(count)
}

async fn copy_departments(
    control: &DatabaseTransaction,
    target: &DatabaseTransaction,
    tenant_id: &str,
    token: &str,
    hasher: &mut Sha256,
) -> AppResult<i64> {
    let mut cursor = 0_i64;
    let mut count = 0_i64;
    loop {
        let rows = DeptRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT `id`, `ancestors` FROM `sys_dept` \
             WHERE `tenant_id` = ? AND `del_flag` = '0' AND `id` > ? \
             ORDER BY `id` LIMIT ?",
            [
                tenant_id.to_string().into(),
                cursor.into(),
                PAGE_SIZE.into(),
            ],
        ))
        .all(control)
        .await
        .db()?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            hasher.update(format!("d:{}:{}\n", row.id, row.ancestors));
        }
        insert_dept_batch(target, tenant_id, token, &rows).await?;
        count += i64::try_from(rows.len()).unwrap_or(i64::MAX);
        cursor = rows.last().map(|row| row.id).unwrap_or(cursor);
        if rows.len() < usize::try_from(PAGE_SIZE).unwrap_or(usize::MAX) {
            break;
        }
    }
    Ok(count)
}

async fn insert_owner_batch(
    target: &DatabaseTransaction,
    tenant_id: &str,
    token: &str,
    rows: &[OwnerRow],
) -> AppResult<()> {
    let placeholders = std::iter::repeat_n("(?, ?, ?, ?)", rows.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO `biz_data_scope_owner` \
         (`tenant_id`, `build_token`, `user_id`, `dept_id`) VALUES {placeholders}"
    );
    let mut values = Vec::with_capacity(rows.len().saturating_mul(4));
    for row in rows {
        values.extend([
            tenant_id.to_string().into(),
            token.to_string().into(),
            row.id.into(),
            row.dept_id
                .map(Value::from)
                .unwrap_or_else(|| Value::from(None::<i64>)),
        ]);
    }
    target
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            values,
        ))
        .await
        .db()?;
    Ok(())
}

async fn insert_dept_batch(
    target: &DatabaseTransaction,
    tenant_id: &str,
    token: &str,
    rows: &[DeptRow],
) -> AppResult<()> {
    let placeholders = std::iter::repeat_n("(?, ?, ?, ?)", rows.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO `biz_data_scope_dept` \
         (`tenant_id`, `build_token`, `dept_id`, `ancestors`) VALUES {placeholders}"
    );
    let mut values = Vec::with_capacity(rows.len().saturating_mul(4));
    for row in rows {
        values.extend([
            tenant_id.to_string().into(),
            token.to_string().into(),
            row.id.into(),
            row.ancestors.clone().into(),
        ]);
    }
    target
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            values,
        ))
        .await
        .db()?;
    Ok(())
}

async fn save_metadata(
    target: &DatabaseTransaction,
    tenant_id: &str,
    publication: &ProjectionPublication<'_>,
) -> AppResult<()> {
    target
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO `biz_data_scope_projection` \
             (`tenant_id`, `source_version`, `build_token`, `placement_generation`, \
              `switch_token`, `digest`, `owner_count`, `dept_count`, `ready`, `published_at`) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, CURRENT_TIMESTAMP(6)) \
             ON DUPLICATE KEY UPDATE \
              `source_version` = VALUES(`source_version`), \
              `build_token` = VALUES(`build_token`), \
              `placement_generation` = VALUES(`placement_generation`), \
              `switch_token` = VALUES(`switch_token`), \
              `digest` = VALUES(`digest`), `owner_count` = VALUES(`owner_count`), \
              `dept_count` = VALUES(`dept_count`), `ready` = 1, \
              `published_at` = CURRENT_TIMESTAMP(6)",
            [
                tenant_id.to_string().into(),
                publication.source_version.into(),
                publication.token.to_string().into(),
                publication.placement.generation.into(),
                publication.placement.switch_token.clone().into(),
                publication.digest.to_string().into(),
                publication.owner_count.into(),
                publication.dept_count.into(),
            ],
        ))
        .await
        .db()?;
    Ok(())
}

async fn remove_old_builds(
    target: &DatabaseTransaction,
    tenant_id: &str,
    token: &str,
) -> AppResult<()> {
    for table in ["biz_data_scope_owner", "biz_data_scope_dept"] {
        let sql = format!("DELETE FROM `{table}` WHERE `tenant_id` = ? AND `build_token` <> ?");
        target
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                sql,
                [tenant_id.to_string().into(), token.to_string().into()],
            ))
            .await
            .db()?;
    }
    Ok(())
}

fn build_token(
    tenant_id: &str,
    source_version: i64,
    target_key: &str,
    generation: i64,
    switch_token: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ryframe:data-scope-projection:v1\0");
    hasher.update(tenant_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(source_version.to_be_bytes());
    hasher.update(target_key.as_bytes());
    hasher.update(b"\0");
    hasher.update(generation.to_be_bytes());
    hasher.update(switch_token.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{build_token, valid_digest};

    #[test]
    fn build_token_binds_version_target_and_placement() {
        let baseline = build_token("tenant-a", 7, "business-shared", 3, "switch-a");
        assert_eq!(
            baseline,
            build_token("tenant-a", 7, "business-shared", 3, "switch-a")
        );
        for changed in [
            build_token("tenant-b", 7, "business-shared", 3, "switch-a"),
            build_token("tenant-a", 8, "business-shared", 3, "switch-a"),
            build_token("tenant-a", 7, "business-dedicated", 3, "switch-a"),
            build_token("tenant-a", 7, "business-shared", 4, "switch-a"),
            build_token("tenant-a", 7, "business-shared", 3, "switch-b"),
        ] {
            assert_ne!(baseline, changed);
        }
        assert_eq!(baseline.len(), 64);
    }

    #[test]
    fn projection_digest_must_be_lowercase_sha256() {
        assert!(valid_digest(&"a".repeat(64)));
        assert!(!valid_digest(&"A".repeat(64)));
        assert!(!valid_digest(&"g".repeat(64)));
        assert!(!valid_digest("short"));
    }
}
