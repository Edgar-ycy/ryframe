use ryframe_common::AppError;
use ryframe_config::AppConfig;
use ryframe_db::DatabaseCluster;
use sea_orm::DatabaseConnection;

/// 连接主库、只读副本和所有命名业务数据源。
pub async fn connect(config: &AppConfig) -> Result<DatabaseCluster, AppError> {
    let primary_config = &config.database.primary;
    let primary =
        ryframe_db::connection::connect_with_level(primary_config, config.database.sql_log_level)
            .await
            .map_err(|error| AppError::Database(format!("主数据库连接失败: {error}")))?;
    ryframe_db::connection::ping(&primary)
        .await
        .map_err(|error| AppError::Database(format!("主数据库健康检查失败: {error}")))?;
    tracing::info!(
        database = %primary_config.database,
        driver = "mysql",
        "主数据库连接成功"
    );

    let mut replicas = Vec::with_capacity(config.database.replicas.len());
    for replica_config in &config.database.replicas {
        let replica = ryframe_db::connection::connect_with_level(
            &replica_config.connection,
            config.database.sql_log_level,
        )
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "只读副本 {} 连接失败: {error}",
                replica_config.name
            ))
        })?;
        ryframe_db::connection::ping(&replica)
            .await
            .map_err(|error| {
                AppError::Database(format!(
                    "只读副本 {} 健康检查失败: {error}",
                    replica_config.name
                ))
            })?;
        tracing::info!(
            replica = %replica_config.name,
            host = %replica_config.connection.host,
            "只读副本连接成功"
        );
        replicas.push((replica_config.name.clone(), replica));
    }

    let mut sources = Vec::with_capacity(config.database.sources.len());
    for source_config in &config.database.sources {
        let source = ryframe_db::connection::connect_with_level(
            &source_config.connection,
            config.database.sql_log_level,
        )
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "业务数据源 {} 连接失败: {error}",
                source_config.name
            ))
        })?;
        ryframe_db::connection::ping(&source)
            .await
            .map_err(|error| {
                AppError::Database(format!(
                    "业务数据源 {} 健康检查失败: {error}",
                    source_config.name
                ))
            })?;
        tracing::info!(
            source = %source_config.name,
            database = %source_config.connection.database,
            driver = "mysql",
            "业务数据源连接成功"
        );
        sources.push((source_config.name.clone(), source));
    }

    Ok(DatabaseCluster::with_sources(primary, replicas, sources))
}

/// 在主库迁移完成后校验整个数据库拓扑的业务表结构。
pub async fn verify_schema(cluster: &DatabaseCluster) -> Result<(), AppError> {
    verify_tables("primary", cluster.write()).await?;
    for (name, replica) in cluster.replicas() {
        verify_tables(name, replica).await?;
    }
    Ok(())
}

/// 校验列、索引和外键均与当前 Migrator 的 MySQL 指纹一致。
async fn verify_tables(node: &str, db: &DatabaseConnection) -> Result<(), AppError> {
    ryframe_db_migration::verify_current_schema(db)
        .await
        .map_err(|error| AppError::Internal(format!("数据库节点 {node} 结构校验失败: {error}")))?;
    tracing::info!(node, "数据库结构指纹校验通过");
    Ok(())
}
