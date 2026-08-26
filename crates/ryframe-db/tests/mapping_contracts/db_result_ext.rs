use ryframe_db::DbResultExt;
use ryframe_kernel::AppError;

#[test]
fn original_database_message_is_preserved() {
    let error = Result::<(), _>::Err(std::io::Error::other("连接失败"))
        .db()
        .expect_err("错误应被映射");

    assert!(matches!(error, AppError::Database(message) if message == "连接失败"));
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
