use sea_orm::{ConnectionTrait, DatabaseBackend, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

const SHA256_PRECONDITION_SQL: &str = "SELECT COUNT(*) FROM sys_file WHERE file_sha256 IS NULL \
     OR NOT REGEXP_LIKE(file_sha256, '^[0-9a-f]{64}$', 'c')";
const DELETE_FLAG_PRECONDITION_SQL: &str =
    "SELECT COUNT(*) FROM sys_file WHERE del_flag IS NULL OR del_flag NOT IN ('0', '2')";
const UPLOAD_STATE_PRECONDITION_SQL: &str = "SELECT COUNT(*) FROM sys_file WHERE upload_status IS NULL \
     OR upload_status NOT IN ('pending', 'ready', 'cleanup') \
     OR (upload_status = 'ready' AND \
         (reservation_token IS NOT NULL OR reservation_expires_at IS NOT NULL)) \
     OR (upload_status = 'pending' AND \
         (reservation_token IS NULL OR reservation_token = '' OR reservation_expires_at IS NULL)) \
     OR (upload_status = 'cleanup' AND reservation_expires_at IS NULL)";

/// 在维护命令完成后终结文件表，只保留 SHA-256 与 `upload_status` 状态机。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom("文件摘要终结迁移仅支持 MySQL".into()));
        }
        if !manager.has_table("sys_file").await? {
            return Err(DbErr::Custom(
                "缺少 sys_file，不能执行文件摘要终结迁移".into(),
            ));
        }
        for column in [
            "file_sha256",
            "upload_status",
            "reservation_token",
            "reservation_expires_at",
            "del_flag",
        ] {
            if !manager.has_column("sys_file", column).await? {
                return Err(DbErr::Custom(format!(
                    "sys_file.{column} 不存在；请先完成前置迁移"
                )));
            }
        }

        ensure_no_rows(
            manager,
            SHA256_PRECONDITION_SQL,
            "仍有文件缺少规范的小写 SHA-256；请先运行 ryframe-file-maintenance backfill-sha256",
        )
        .await?;
        ensure_no_rows(
            manager,
            DELETE_FLAG_PRECONDITION_SQL,
            "仍有旧上传预留或非法删除标记；请先运行 ryframe-file-maintenance drain-legacy-reservations",
        )
        .await?;
        ensure_no_rows(
            manager,
            UPLOAD_STATE_PRECONDITION_SQL,
            "上传状态与预留字段不满足最终状态机约束",
        )
        .await?;

        if manager
            .has_index("sys_file", "idx_file_upload_reservation")
            .await?
        {
            manager
                .drop_index(
                    Index::drop()
                        .name("idx_file_upload_reservation")
                        .table(Alias::new("sys_file"))
                        .to_owned(),
                )
                .await?;
        }

        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("sys_file"))
                    .modify_column(
                        ColumnDef::new(Alias::new("file_sha256"))
                            .char_len(64)
                            .not_null()
                            .comment("SHA-256 内容摘要"),
                    )
                    .modify_column(
                        ColumnDef::new(Alias::new("upload_status"))
                            .string_len(16)
                            .not_null()
                            .default("ready")
                            .comment("上传状态: pending/ready/cleanup"),
                    )
                    .modify_column(
                        ColumnDef::new(Alias::new("del_flag"))
                            .char_len(1)
                            .not_null()
                            .default("0")
                            .comment("文件状态: 0正常 2删除"),
                    )
                    .to_owned(),
            )
            .await?;

        if manager.has_column("sys_file", "file_md5").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_file"))
                        .drop_column(Alias::new("file_md5"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "文件摘要终结迁移是单向操作，不能恢复旧 MD5 或旧预留标记".into(),
        ))
    }
}

async fn ensure_no_rows(
    manager: &SchemaManager<'_>,
    sql: &str,
    message: &str,
) -> Result<(), DbErr> {
    let row = manager
        .get_connection()
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql.to_owned()))
        .await?
        .ok_or_else(|| DbErr::Custom(format!("迁移前置检查没有返回结果: {message}")))?;
    let count = i64::try_get_by_index(&row, 0)?;
    if count != 0 {
        return Err(DbErr::Custom(format!("{message}（不合规记录数: {count}）")));
    }
    Ok(())
}
