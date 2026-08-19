use sea_orm::{
    ConnectionTrait, DbBackend, Statement, TransactionSession, TransactionTrait, TryGetable,
};

pub const RESOURCE_OWNERSHIP_TABLE: &str = "ryframe_resource_ownership";

/// 原子声明物理 MySQL schema 的 scope 所有权；已有记录只能与期望值完全一致。
pub async fn ensure_resource_ownership<C>(
    database: &C,
    scope_id: &str,
    resource_kind: &str,
) -> Result<(), sea_orm::DbErr>
where
    C: ConnectionTrait + TransactionTrait,
{
    validate_marker_input(scope_id, resource_kind)?;
    let transaction = database.begin().await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT IGNORE INTO `ryframe_resource_ownership` \
             (`resource_kind`, `scope_id`, `marker`) VALUES (?, ?, ?)",
            [
                resource_kind.into(),
                scope_id.into(),
                marker(scope_id, resource_kind).into(),
            ],
        ))
        .await?;
    verify_resource_ownership(&transaction, scope_id, resource_kind).await?;
    transaction.commit().await
}

/// 只读校验物理 MySQL schema 的 scope 所有权。
pub async fn verify_resource_ownership<C>(
    database: &C,
    scope_id: &str,
    resource_kind: &str,
) -> Result<(), sea_orm::DbErr>
where
    C: ConnectionTrait,
{
    validate_marker_input(scope_id, resource_kind)?;
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT `scope_id`, `marker` FROM `ryframe_resource_ownership` \
             WHERE `resource_kind` = ? LIMIT 1",
            [resource_kind.into()],
        ))
        .await?
        .ok_or_else(|| {
            sea_orm::DbErr::Custom(format!(
                "MySQL resource ownership marker is missing for {resource_kind}"
            ))
        })?;
    let actual_scope = String::try_get_by_index(&row, 0)?;
    let actual_marker = String::try_get_by_index(&row, 1)?;
    if actual_scope != scope_id || actual_marker != marker(scope_id, resource_kind) {
        return Err(sea_orm::DbErr::Custom(format!(
            "MySQL resource ownership marker mismatch for {resource_kind}"
        )));
    }
    Ok(())
}

pub fn marker(scope_id: &str, resource_kind: &str) -> String {
    format!("ryframe-owner:v1:{scope_id}:{resource_kind}")
}

fn validate_marker_input(scope_id: &str, resource_kind: &str) -> Result<(), sea_orm::DbErr> {
    if scope_id.is_empty()
        || scope_id.len() > 48
        || resource_kind.is_empty()
        || resource_kind.len() > 32
        || !scope_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
        || !resource_kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(sea_orm::DbErr::Custom(
            "invalid MySQL resource ownership marker input".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{marker, validate_marker_input};

    #[test]
    fn marker_is_stable_and_inputs_are_bounded() {
        assert_eq!(
            marker("test-a", "tenant-data"),
            "ryframe-owner:v1:test-a:tenant-data"
        );
        assert!(validate_marker_input("test-a", "control").is_ok());
        assert!(validate_marker_input("Test", "control").is_err());
        assert!(validate_marker_input("test", "tenant_data").is_err());
    }
}
