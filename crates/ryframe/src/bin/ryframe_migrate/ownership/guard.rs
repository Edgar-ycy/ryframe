use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, Statement,
    TransactionTrait, TryGetable,
};

const PREFIXES: [&str; 2] = ["ryframe:migration:", "ryframe:tenant-data-migration:"];

/// 同一会话固定顺序持有两侧既有迁移锁，覆盖资格判定、DDL、业务迁移及最终归属登记。
pub(super) struct Guard {
    session: Option<DatabaseTransaction>,
    pub identity: ryframe_db::connection::DatabaseIdentity,
}

impl Guard {
    pub async fn acquire(db: &DatabaseConnection) -> Result<Self, DbErr> {
        ryframe_tenant_db::migration::verify_mysql_80(db).await?;
        let identity = ryframe_db::connection::physical_identity(db).await?;
        let session = db.begin().await?;
        let mut guard = Self {
            session: Some(session),
            identity,
        };
        // 取得命名锁前建立保留会话生命周期，错误连接不得携带命名锁回池。
        guard.session_mut().finish_mysql_root(false).await?;
        guard.session_mut().begin_mysql_root(None, None).await?;
        for prefix in PREFIXES {
            lock(guard.session(), prefix, true).await?;
        }
        if ryframe_db::connection::physical_identity(guard.session()).await? != guard.identity {
            return Err(DbErr::Custom(
                "取得迁移互斥后物理身份变化，尚未开始初始化".into(),
            ));
        }
        Ok(guard)
    }
    pub fn session(&self) -> &DatabaseTransaction {
        self.session.as_ref().expect("迁移会话尚未关闭")
    }
    pub fn session_mut(&mut self) -> &mut DatabaseTransaction {
        self.session.as_mut().expect("迁移会话尚未关闭")
    }
    pub async fn close(mut self) -> Result<(), DbErr> {
        if self.session().is_open() {
            self.session_mut().finish_mysql_root(false).await?;
        }
        let mut result = Ok(());
        for prefix in PREFIXES.into_iter().rev() {
            let released = lock(self.session(), prefix, false).await;
            if result.is_ok() {
                result = released;
            }
        }
        let session = self.session.take().expect("迁移会话尚未关闭");
        drop(session);
        result
    }
}

async fn lock<C: ConnectionTrait>(db: &C, prefix: &str, acquire: bool) -> Result<(), DbErr> {
    let function = if acquire { "GET_LOCK" } else { "RELEASE_LOCK" };
    let timeout = if acquire { ",60" } else { "" };
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            format!("SELECT {function}(SHA2(CONCAT('{prefix}',DATABASE()),256){timeout})"),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("迁移互斥无结果".into()))?;
    if Option::<i64>::try_get_by_index(&row, 0)? != Some(1) {
        return Err(DbErr::Custom(
            "迁移互斥获取或释放失败；保留现场，不自动重放".into(),
        ));
    }
    Ok(())
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(mut session) = self.session.take() {
            // 取消后只在事务已结束的可用会话释放命名锁；失效会话依靠丢弃连接断连释放。
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                drop(runtime.spawn(async move {
                    let settled =
                        !session.is_open() || session.finish_mysql_root(false).await.is_ok();
                    if settled {
                        for prefix in PREFIXES.into_iter().rev() {
                            let _ = lock(&session, prefix, false).await;
                        }
                    }
                    drop(session);
                }));
            }
        }
    }
}
