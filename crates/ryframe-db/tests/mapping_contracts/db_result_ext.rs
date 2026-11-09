use ryframe_db::DbResultExt;
use ryframe_kernel::AppError;
use sea_orm::{DbErr, RuntimeErr, sqlx};
use sqlx::error::DatabaseError;

#[test]
fn original_database_message_is_preserved() {
    let error = Result::<(), _>::Err(std::io::Error::other("连接失败"))
        .db()
        .expect_err("错误应被映射");

    assert!(matches!(error, AppError::Database(message) if message == "连接失败"));

    for error in [
        DbErr::Custom("1451 foreign key constraint fails".into()),
        DbErr::Conn(RuntimeErr::Internal("连接失败".into())),
        DbErr::Exec(RuntimeErr::SqlxError(
            sqlx::Error::Io(std::io::Error::other("事务连接中断")).into(),
        )),
    ] {
        let message = error.to_string();
        let mapped = Result::<(), _>::Err(error).db().unwrap_err();
        assert!(matches!(mapped, AppError::Database(actual) if actual == message));
    }
}

#[test]
fn database_context_prefixes_the_original_message() {
    let error = Result::<(), _>::Err(std::io::Error::other("连接失败"))
        .db_context("数据库健康检查失败")
        .expect_err("错误应被映射");

    assert!(matches!(
        error,
        AppError::Database(message) if message == "数据库健康检查失败: 连接失败"
    ));
}

#[test]
fn database_conflicts_select_named_index_and_preserve_other_errors() {
    let named = Result::<(), _>::Err("Duplicate entry for key 'uk_tenant_code'")
        .db_conflicts(&[("uk_tenant_code", "岗位编码已存在")], "岗位已存在")
        .expect_err("唯一键错误应映射为冲突");
    assert!(matches!(named, AppError::Conflict(message) if message == "岗位编码已存在"));

    let fallback = Result::<(), _>::Err("1062 duplicate entry")
        .db_conflicts(&[], "岗位已存在")
        .expect_err("未知唯一键应使用资源级冲突");
    assert!(matches!(fallback, AppError::Conflict(message) if message == "岗位已存在"));

    let database = Result::<(), _>::Err("connection closed")
        .db_conflicts(&[], "岗位已存在")
        .expect_err("非唯一键错误应保留数据库文本");
    assert!(matches!(database, AppError::Database(message) if message == "connection closed"));
}

#[derive(Debug)]
struct ForeignKeyError;

impl std::fmt::Display for ForeignKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ForeignKeyError {}

impl DatabaseError for ForeignKeyError {
    fn message(&self) -> &str {
        "private SQL and field value; duplicate entry 1062 uk_tenant_code"
    }

    fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self
    }

    fn kind(&self) -> sqlx::error::ErrorKind {
        sqlx::error::ErrorKind::ForeignKeyViolation
    }
}

#[test]
fn structured_foreign_keys_are_safe_conflicts_through_all_database_mappings() {
    for query in [false, true] {
        let driver = sqlx::Error::Database(Box::new(ForeignKeyError));
        let runtime = RuntimeErr::SqlxError(driver.into());
        let error = if query {
            DbErr::Query(runtime)
        } else {
            DbErr::Exec(runtime)
        };
        let mapped = [
            Result::<(), _>::Err(error.clone()).db(),
            Result::<(), _>::Err(error.clone()).db_context("写入失败"),
            Result::<(), _>::Err(error)
                .db_conflicts(&[("uk_tenant_code", "岗位编码已存在")], "岗位已存在"),
        ];
        for result in mapped {
            assert!(matches!(result, Err(AppError::Conflict(message)) if message
                == "数据关联约束冲突，请先处理引用此记录的数据，并确认所引用的记录存在后重试"));
        }
    }
}
