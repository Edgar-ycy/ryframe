//! ryframe-db 测试公共模块
//!
//! 提供隔离的 MySQL 8.4 测试数据库与建表辅助函数。

mod test_database;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Schema};
pub use test_database::TestDatabase;

/// 创建独立 MySQL 数据库并建表。
pub async fn setup_test_db() -> TestDatabase {
    let db = connect_isolated_mysql().await;
    create_all_tables(&db).await;
    db
}

pub async fn connect_isolated_mysql() -> TestDatabase {
    TestDatabase::create("db").await
}

/// 为所有测试用到的实体创建表
async fn create_all_tables(db: &DatabaseConnection) {
    let backend = DatabaseBackend::MySql;
    let schema = Schema::new(backend);

    macro_rules! create {
        ($entity:path) => {
            let stmt = schema.create_table_from_entity($entity);
            db.execute(&stmt).await.expect("create table failed");
        };
    }

    create!(ryframe_db::entities::tenant::Entity);
    create!(ryframe_db::entities::config::Entity);
    create!(ryframe_db::entities::dept::Entity);
    create!(ryframe_db::entities::dict_type::Entity);
    create!(ryframe_db::entities::dict_data::Entity);
    create!(ryframe_db::entities::login_info::Entity);
    create!(ryframe_db::entities::notice::Entity);
    create!(ryframe_db::entities::oper_log::Entity);
    create!(ryframe_db::entities::permission::Entity);
    create!(ryframe_db::entities::post::Entity);
    create!(ryframe_db::entities::role::Entity);
    create!(ryframe_db::entities::menu::Entity);
    create!(ryframe_db::entities::user::Entity);
    create!(ryframe_db::entities::password_reset_request::Entity);
    create!(ryframe_db::entities::user_role::Entity);
    create!(ryframe_db::entities::role_permission::Entity);
    create!(ryframe_db::entities::role_dept::Entity);
    create!(ryframe_db::entities::sys_file::Entity);
}
