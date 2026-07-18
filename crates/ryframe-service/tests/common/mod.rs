//! ryframe-service 测试公共模块
//!
//! 提供隔离的 MySQL 8.4 测试数据库与建表辅助函数。

#[path = "../../../ryframe-db/tests/common/test_database.rs"]
mod test_database;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Schema};
pub use test_database::TestDatabase;

/// 创建独立 MySQL 数据库并建表。
pub async fn setup_test_db() -> TestDatabase {
    let db = TestDatabase::create("service").await;
    create_all_tables(&db).await;
    let tenant = ryframe_db::entities::tenant::Model {
        id: 1,
        tenant_id: "system".into(),
        name: "系统租户".into(),
        domain: None,
        status: ryframe_db::entities::tenant::Model::STATUS_NORMAL.into(),
        expire_at: None,
        max_users: 100,
        max_roles: 20,
        max_storage_mb: 1024,
        max_requests_per_min: 1000,
        session_version: 1,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let active: ryframe_db::entities::tenant::ActiveModel = tenant.into();
    ryframe_db::entities::tenant::Entity::insert(active)
        .exec(&db)
        .await
        .expect("seed system tenant failed");
    db
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
