use sea_orm::{ConnectionTrait, DbBackend, DbErr, FromQueryResult, Statement};

/// 用于迁移目标去重的物理身份；按服务器大小写规则规范化库名。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, FromQueryResult)]
pub struct DatabaseIdentity {
    pub server_uuid: String,
    pub database: String,
}

pub async fn physical_identity<C: ConnectionTrait>(
    database: &C,
) -> Result<DatabaseIdentity, DbErr> {
    if database.get_database_backend() != DbBackend::MySql {
        return Err(DbErr::Custom("迁移目标身份仅支持 MySQL".into()));
    }
    let identity = DatabaseIdentity::find_by_statement(Statement::from_string(DbBackend::MySql,
        "SELECT LOWER(@@server_uuid) AS server_uuid, CASE WHEN @@lower_case_table_names=0 THEN DATABASE() ELSE LOWER(DATABASE()) END AS `database`"))
        .one(database).await?.ok_or_else(|| DbErr::Custom("无法读取明确数据库的物理身份".into()))?;
    if identity.server_uuid.is_empty() || identity.database.is_empty() {
        return Err(DbErr::Custom(
            "迁移目标必须包含 MySQL 实例身份与明确数据库".into(),
        ));
    }
    Ok(identity)
}
