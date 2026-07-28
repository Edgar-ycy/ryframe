use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

const USER_TABLE: &str = "sys_user";
const FILE_TABLE: &str = "sys_file";
const AVATAR_FILE_COLUMN: &str = "avatar_file_id";
const AVATAR_FILE_INDEX: &str = "idx_user_avatar_file";
const AVATAR_FILE_FOREIGN_KEY: &str = "fk_user_avatar_file";

/// 为头像建立指向文件元数据的稳定关系。
///
/// 历史头像只保存 URL，迁移后保留空关联；下一次头像更新会写入可计数的文件 ID，
/// 使异步回收能够在删除对象前确认没有用户继续引用它。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table(USER_TABLE).await? || !manager.has_table(FILE_TABLE).await? {
            return Ok(());
        }
        if !manager.has_column(USER_TABLE, AVATAR_FILE_COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(USER_TABLE))
                        .add_column(
                            ColumnDef::new(Alias::new(AVATAR_FILE_COLUMN))
                                .big_integer()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }
        if !manager.has_index(USER_TABLE, AVATAR_FILE_INDEX).await? {
            manager
                .create_index(
                    Index::create()
                        .name(AVATAR_FILE_INDEX)
                        .table(Alias::new(USER_TABLE))
                        .col(Alias::new(AVATAR_FILE_COLUMN))
                        .to_owned(),
                )
                .await?;
        }
        if !foreign_key_exists(manager, AVATAR_FILE_FOREIGN_KEY).await? {
            manager
                .create_foreign_key(
                    ForeignKey::create()
                        .name(AVATAR_FILE_FOREIGN_KEY)
                        .from(Alias::new(USER_TABLE), Alias::new(AVATAR_FILE_COLUMN))
                        .to(Alias::new(FILE_TABLE), Alias::new("id"))
                        .on_update(ForeignKeyAction::Cascade)
                        .on_delete(ForeignKeyAction::Restrict)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table(USER_TABLE).await? {
            return Ok(());
        }
        if foreign_key_exists(manager, AVATAR_FILE_FOREIGN_KEY).await? {
            manager
                .drop_foreign_key(
                    ForeignKey::drop()
                        .name(AVATAR_FILE_FOREIGN_KEY)
                        .table(Alias::new(USER_TABLE))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_index(USER_TABLE, AVATAR_FILE_INDEX).await? {
            manager
                .drop_index(
                    Index::drop()
                        .name(AVATAR_FILE_INDEX)
                        .table(Alias::new(USER_TABLE))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column(USER_TABLE, AVATAR_FILE_COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(USER_TABLE))
                        .drop_column(Alias::new(AVATAR_FILE_COLUMN))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

async fn foreign_key_exists(manager: &SchemaManager<'_>, name: &str) -> Result<bool, DbErr> {
    let row = manager
        .get_connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM information_schema.TABLE_CONSTRAINTS \
             WHERE CONSTRAINT_SCHEMA = DATABASE() AND TABLE_NAME = ? \
             AND CONSTRAINT_NAME = ? AND CONSTRAINT_TYPE = 'FOREIGN KEY'",
            [USER_TABLE.into(), name.into()],
        ))
        .await?;
    Ok(row
        .map(|row| i64::try_get_by_index(&row, 0))
        .transpose()?
        .unwrap_or_default()
        > 0)
}
