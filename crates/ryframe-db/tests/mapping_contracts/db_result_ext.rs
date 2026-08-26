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
