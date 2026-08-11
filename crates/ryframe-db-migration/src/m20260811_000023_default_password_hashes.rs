use sea_orm::{ConnectionTrait, DbBackend};
use sea_orm_migration::prelude::*;

/// 修复早期基线中与文档默认密码不匹配的系统用户哈希和登录状态。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom("默认密码修复仅支持 MySQL".into()));
        }

        manager
            .get_connection()
            .execute_unprepared(
                r#"UPDATE `sys_user`
                   SET `password_hash` = CASE `username`
                       WHEN 'admin' THEN '$argon2id$v=19$m=65536,t=3,p=4$O8qRRhiIVYjCHpUuwGWTSA$OO+ik8t1+N5a4PSipMbB71W/pfc3roAbq6mdIAgV1bA'
                       WHEN 'user' THEN '$argon2id$v=19$m=65536,t=3,p=4$LDHC7/MqBOozq1OQk24lDw$IDFTnrVNEFZgvpDI+9kzNZi3pKWEt8EIPi6qOKtmgrw'
                       ELSE `password_hash`
                   END,
                   `status` = CASE
                       WHEN `status` = 'must_reset_password' THEN '1'
                       ELSE `status`
                   END
                   WHERE `tenant_id` = 'system'
                     AND `username` IN ('admin', 'user')
                     AND `password_hash` = '$argon2id$v=19$m=65536,t=3,p=4$/jPTT9LsEpBD6BFpc2rddg$vogNJpv6lRqvcLOOSeZCOId88Fene5oRnWJwuDz5IUE'"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom("默认密码哈希修复不能自动回滚".into()))
    }
}
