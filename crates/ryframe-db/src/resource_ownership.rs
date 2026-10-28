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

/// 组合根保留命名锁会话时，只提交本次首次初始化的归属元数据。
pub async fn register_in_session(
    database: &mut sea_orm::DatabaseTransaction,
    scope_id: &str,
    kind: &str,
) -> Result<(), sea_orm::DbErr> {
    validate_marker_input(scope_id, kind)?;
    if database.is_open() {
        return Err(sea_orm::DbErr::Custom(
            "归属登记不能结束调用方的活跃事务".into(),
        ));
    }
    database
        .begin_mysql_root(
            Some(sea_orm::IsolationLevel::Serializable),
            Some(sea_orm::AccessMode::ReadWrite),
        )
        .await?;
    let result = async {
        database.query_all_raw(Statement::from_string(DbBackend::MySql,
            "SELECT resource_kind FROM ryframe_resource_ownership ORDER BY resource_kind FOR UPDATE")).await?;
        database.execute_raw(Statement::from_sql_and_values(DbBackend::MySql,
            "INSERT INTO ryframe_resource_ownership(resource_kind,scope_id,marker) VALUES (?,?,?)",
            [kind.into(),scope_id.into(),marker(scope_id,kind).into()])).await?;
        verify_resource_ownership(database,scope_id,kind).await
    }.await;
    if let Err(error) = result {
        let rollback = database.finish_mysql_root(false).await;
        return Err(sea_orm::DbErr::Custom(format!(
            "{error}；回滚结果={rollback:?}；保留现场，不自动重放"
        )));
    }
    database.finish_mysql_root(true).await.map_err(|error| {
        sea_orm::DbErr::Custom(format!(
            "归属提交结果未知：{error}；先只读核对归属及初始化账本，不自动重放"
        ))
    })
}
/// 正式离线初始化与在线登记使用相同显式 scope 和 marker 合同。
pub fn initialization_statements(
    scope_id: &str,
    kinds: &[&str],
) -> Result<Vec<String>, sea_orm::DbErr> {
    let mut statements = Vec::new();
    let mut predicates = vec![format!(
        "(SELECT COUNT(*) FROM ryframe_resource_ownership)={}",
        kinds.len()
    )];
    let mut unique = std::collections::BTreeSet::new();
    for kind in kinds {
        validate_marker_input(scope_id, kind)?;
        if !matches!(*kind, "control" | "tenant-data") || !unique.insert(*kind) {
            return Err(sea_orm::DbErr::Custom("初始化归属类型未知或重复".into()));
        }
        let marker = marker(scope_id, kind);
        statements.push(format!("INSERT INTO ryframe_resource_ownership(resource_kind,scope_id,marker) SELECT '{kind}','{scope_id}','{marker}' FROM ryframe_initialization_guard WHERE valid=1"));
        predicates.push(format!("(SELECT COUNT(*) FROM ryframe_resource_ownership WHERE CAST(resource_kind AS BINARY)=CAST('{kind}' AS BINARY) AND CAST(scope_id AS BINARY)=CAST('{scope_id}' AS BINARY) AND CAST(marker AS BINARY)=CAST('{marker}' AS BINARY))=1"));
    }
    statements.extend(
        crate::schema::SchemaAssertion {
            category: "resource_ownership",
            sql: predicates.join(" AND "),
        }
        .diagnostic_guard("framework", "final-ownership"),
    );
    Ok(statements)
}

pub fn validate_marker_input(scope_id: &str, resource_kind: &str) -> Result<(), sea_orm::DbErr> {
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
