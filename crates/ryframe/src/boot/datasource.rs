use ryframe_common::AppError;
use ryframe_config::AppConfig;
use sea_orm::DatabaseConnection;

/// 数据源连接结果
pub struct DataSources {
    pub primary: DatabaseConnection,
    /// 额外数据源连接（db_1, db_2...）
    pub extras: Vec<DatabaseConnection>,
}

/// 连接所有数据源并执行健康检查 + 表校验
pub async fn connect(config: &AppConfig) -> Result<DataSources, AppError> {
    // 连接主库（connections[0]）
    let primary_config = &config.database.connections[0];
    let primary_db =
        ryframe_db::connection::connect_with_level(primary_config, config.database.sql_log_level)
            .await?;
    tracing::info!("数据源 'primary' 连接成功: {}", primary_config.database);

    // 连接额外数据源（connections[1..]），命名为 db_1, db_2...
    let mut extra_dbs = Vec::with_capacity(config.database.connections.len().saturating_sub(1));
    for (i, conn_config) in config.database.connections.iter().enumerate().skip(1) {
        let name = format!("db_{}", i);
        match ryframe_db::connection::connect_with_level(conn_config, config.database.sql_log_level)
            .await
        {
            Ok(db) => {
                tracing::info!("数据源 '{}' 连接成功: {}", name, conn_config.database);
                extra_dbs.push(db);
            }
            Err(e) => {
                tracing::warn!(
                    "数据源 '{}' ({}) 连接失败: {}，跳过",
                    name,
                    conn_config.database,
                    e
                );
            }
        }
    }

    tracing::info!("数据库连接初始化完成, 额外数据源: {} 个", extra_dbs.len());

    // 健康检查 primary
    ryframe_db::connection::ping(&primary_db).await?;

    // 检查所有必需表是否存在
    verify_tables(&primary_db).await?;

    Ok(DataSources {
        primary: primary_db,
        extras: extra_dbs,
    })
}

/// 检查必需表是否存在
async fn verify_tables(db: &DatabaseConnection) -> Result<(), AppError> {
    if let Err(missing) = ryframe_db::connection::check_tables(db).await {
        eprintln!("\n========================================");
        eprintln!("  数据库表缺失！请先执行建表 SQL：");
        eprintln!("    mysql -u root -p ryframe_config < sql/ryframe_config.sql");
        eprintln!("========================================");
        eprintln!("  缺失的表 ({} 张):", missing.len());
        for table in &missing {
            eprintln!("    - {}", table);
        }
        eprintln!("========================================\n");
        return Err(AppError::Internal(format!(
            "缺少 {} 张必需的数据表，请先执行 sql/ryframe_config.sql 初始化数据库",
            missing.len()
        )));
    }
    tracing::info!("数据库表检查通过 ({} 张表全部存在)", 19);
    Ok(())
}
