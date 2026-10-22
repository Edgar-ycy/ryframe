use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, TryGetable};

/// 初始化调用方提供的哈希；数据库层不读取密码或选择密码算法。
pub struct BootstrapHashes {
    admin: String,
    user: String,
}

impl BootstrapHashes {
    pub fn new(admin: String, user: String) -> Result<Self, DbErr> {
        for value in [&admin, &user] {
            if !value.starts_with("$argon2id$")
                || value.len() > 255
                || value
                    .bytes()
                    .any(|byte| !byte.is_ascii_graphic() || matches!(byte, b'\'' | b'\\'))
            {
                return Err(DbErr::Custom(
                    "初始化必须提供有效的 Argon2id 密码哈希".into(),
                ));
            }
        }
        Ok(Self { admin, user })
    }

    pub(super) fn render(&self, statement: &str) -> String {
        statement
            .replace("${RYFRAME_ADMIN_PASSWORD_HASH}", &self.admin)
            .replace("${RYFRAME_USER_PASSWORD_HASH}", &self.user)
    }
}

/// 在首次 DDL 前验证凭据是否齐备；已有正常用户不会被再次初始化覆盖。
pub(super) async fn require_credentials_for_new_users<C: ConnectionTrait + ?Sized>(
    db: &C,
    hashes: Option<&BootstrapHashes>,
) -> Result<(), DbErr> {
    if hashes.is_some() {
        return Ok(());
    }
    let row = db.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'sys_user'".to_owned(),
    )).await?.ok_or_else(missing_credentials)?;
    if i64::try_get_by_index(&row, 0)? == 0 {
        return Err(missing_credentials());
    }
    let row = db.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT COUNT(*) FROM sys_user WHERE tenant_id = 'system' AND ((id = 1 AND username = 'admin') OR (id = 2 AND username = 'user'))".to_owned(),
    )).await?.ok_or_else(missing_credentials)?;
    if i64::try_get_by_index(&row, 0)? != 2 {
        return Err(missing_credentials());
    }
    Ok(())
}

fn missing_credentials() -> DbErr {
    DbErr::Custom("首次初始化缺少管理员密码；在所选配置文件的 [bootstrap] 中填写 admin_password，再执行 cargo migrate -- control up；自定义目录使用 --config-dir <目录>。尚未执行初始化 DDL".into())
}
