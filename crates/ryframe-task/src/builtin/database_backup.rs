use async_trait::async_trait;
use ryframe_common::AppResult;

use crate::task_manager::{ScheduledTask, TaskContext};

/// 数据库备份定时任务
///
/// 每天 03:00 执行数据库备份，保留最近 7 天的备份文件。
/// 通过 SeaORM Entity 查询导出数据，生成 SQL 备份文件。
pub struct DatabaseBackupTask;

#[async_trait]
impl ScheduledTask for DatabaseBackupTask {
    fn name(&self) -> &str {
        "database_backup"
    }

    fn cron(&self) -> &str {
        "0 0 3 * * *"
    }

    fn description(&self) -> &str {
        "每天 03:00 备份数据库，保留最近 7 天的备份文件"
    }

    async fn execute(&self, ctx: &TaskContext) -> AppResult<String> {
        let backup_dir = std::path::PathBuf::from("backup");
        tokio::fs::create_dir_all(&backup_dir)
            .await
            .map_err(|e| ryframe_common::AppError::Internal(format!("创建备份目录失败: {}", e)))?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let backup_file = backup_dir.join(format!("db_backup_{}.sql", timestamp));

        let result = export_database(ctx, &backup_file).await;

        match result {
            Ok(size) => {
                let cleaned = clean_old_backups(&backup_dir).await?;
                Ok(format!(
                    "数据库备份完成: {} ({:.2} MB), 清理 {} 个旧备份",
                    backup_file.display(),
                    size as f64 / 1024.0 / 1024.0,
                    cleaned
                ))
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&backup_file).await;
                Err(e)
            }
        }
    }
}

/// 导出数据库到 SQL 文件（通过 Entity 查询）
async fn export_database(
    ctx: &TaskContext,
    backup_file: &std::path::Path,
) -> AppResult<u64> {
    use sea_orm::EntityTrait;

    let db = ctx.db.as_ref();
    let mut sql_content = String::new();

    sql_content.push_str(&format!(
        "-- RyFrame Database Backup\n-- Generated at: {}\n\n",
        chrono::Utc::now().to_rfc3339()
    ));

    let mut total_rows = 0u64;

    // 导出所有已知表
    macro_rules! export_table {
        ($entity:ty, $table_name:expr) => {
            match <$entity>::find().all(db).await {
                Ok(models) => {
                    let count = models.len();
                    total_rows += count as u64;
                    sql_content.push_str(&format!(
                        "-- Table: {} ({} rows)\n",
                        $table_name, count
                    ));
                    for model in &models {
                        if let Ok(json) = serde_json::to_string(model) {
                            sql_content.push_str(&format!("-- {}\n", json));
                        }
                    }
                    sql_content.push('\n');
                }
                Err(e) => {
                    sql_content.push_str(&format!(
                        "-- Table: {} (export failed: {})\n\n",
                        $table_name, e
                    ));
                }
            }
        };
    }

    export_table!(ryframe_db::user::Entity, "sys_user");
    export_table!(ryframe_db::role::Entity, "sys_role");
    export_table!(ryframe_db::menu::Entity, "sys_menu");
    export_table!(ryframe_db::dept::Entity, "sys_dept");
    export_table!(ryframe_db::post::Entity, "sys_post");
    export_table!(ryframe_db::dict_type::Entity, "sys_dict_type");
    export_table!(ryframe_db::dict_data::Entity, "sys_dict_data");
    export_table!(ryframe_db::config::Entity, "sys_config");
    export_table!(ryframe_db::notice::Entity, "sys_notice");
    export_table!(ryframe_db::job::Entity, "sys_job");

    sql_content.push_str(&format!("-- Total: {} rows exported\n", total_rows));

    tokio::fs::write(backup_file, sql_content.as_bytes())
        .await
        .map_err(|e| ryframe_common::AppError::Internal(format!("写入备份文件失败: {}", e)))?;

    let metadata = tokio::fs::metadata(backup_file)
        .await
        .map_err(|e| ryframe_common::AppError::Internal(format!("获取文件大小失败: {}", e)))?;

    Ok(metadata.len())
}

/// 清理 7 天前的旧备份文件
async fn clean_old_backups(backup_dir: &std::path::Path) -> AppResult<usize> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
    let mut cleaned = 0;

    let mut entries = tokio::fs::read_dir(backup_dir)
        .await
        .map_err(|e| ryframe_common::AppError::Internal(format!("读取备份目录失败: {}", e)))?;

    while let Some(entry) = entries.next_entry().await.map_err(|e| {
        ryframe_common::AppError::Internal(format!("遍历备份目录失败: {}", e))
    })? {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with("db_backup_") && name.ends_with(".sql")
            && let Ok(metadata) = tokio::fs::metadata(&path).await
            && let Ok(modified) = metadata.modified()
        {
            let modified_time: chrono::DateTime<chrono::Utc> = modified.into();
            if modified_time < cutoff
                && tokio::fs::remove_file(&path).await.is_ok()
            {
                cleaned += 1;
            }
        }
    }

    Ok(cleaned)
}
