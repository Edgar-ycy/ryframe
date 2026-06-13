//! SeaORM 数据库迁移模块
//!
//! 使用方式：
//! ```bash
//! # 执行所有待处理的迁移
//! cargo run --bin ryframe-migration -- up
//!
//! # 回滚最后一次迁移
//! cargo run --bin ryframe-migration -- down
//!
//! # 查看迁移状态
//! cargo run --bin ryframe-migration -- status
//!
//! # 重置数据库（回滚所有迁移后重新执行）
//! cargo run --bin ryframe-migration -- fresh
//! ```

pub mod m20260101_000001_init_tables;
pub mod m20260613_000002_add_http_method;
pub mod m20260613_000003_seed_permission_menu;

use sea_orm::{DatabaseConnection, DbErr, EntityTrait, QueryOrder};
use sea_orm_migration::prelude::*;

/// 所有迁移的集合
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260101_000001_init_tables::Migration),
            Box::new(m20260613_000002_add_http_method::Migration),
            Box::new(m20260613_000003_seed_permission_menu::Migration),
        ]
    }
}

// ============================================================
// 用于查询 seaql_migrations 表的轻量级实体
// ============================================================

mod seaql_migration_entity {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "seaql_migrations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub version: String,
        pub applied_at: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// 数据库迁移管理器
///
/// 提供迁移版本查询、状态检查、升降级等操作。
/// 封装 SeaORM MigratorTrait，提供更友好的编程接口。
///
/// # 示例
///
/// ```
/// use ryframe_db::migration::MigrationManager;
///
/// // 获取迁移名称列表（无需数据库连接）
/// let names = MigrationManager::migration_names();
/// assert!(!names.is_empty());
///
/// // 获取迁移总数
/// let count = MigrationManager::migration_count();
/// assert_eq!(count, names.len());
/// ```
///
/// 需要数据库连接的方法（需在运行时提供 `DatabaseConnection`）：
/// - `MigrationManager::is_up_to_date(db)`
/// - `MigrationManager::pending_count(db)`
/// - `MigrationManager::status(db)`
pub struct MigrationManager;

impl MigrationManager {
    /// 获取迁移状态列表
    ///
    /// 返回已应用的迁移名称列表。
    pub async fn status(db: &DatabaseConnection) -> Result<Vec<String>, DbErr> {
        let models = seaql_migration_entity::Entity::find()
            .order_by_asc(seaql_migration_entity::Column::Version)
            .all(db)
            .await?;
        Ok(models.into_iter().map(|m| m.version).collect())
    }

    /// 检查数据库是否已是最新版本
    pub async fn is_up_to_date(db: &DatabaseConnection) -> Result<bool, DbErr> {
        let pending = Self::pending_count(db).await?;
        Ok(pending == 0)
    }

    /// 获取待处理的迁移数量
    pub async fn pending_count(db: &DatabaseConnection) -> Result<usize, DbErr> {
        let applied = Self::status(db).await?;
        let total = Self::migration_count();
        Ok(total.saturating_sub(applied.len()))
    }

    /// 获取已应用的迁移数量
    pub async fn applied_count(db: &DatabaseConnection) -> Result<usize, DbErr> {
        let applied = Self::status(db).await?;
        Ok(applied.len())
    }

    /// 执行待处理的迁移
    pub async fn migrate_up(db: &DatabaseConnection, steps: Option<u32>) -> Result<(), DbErr> {
        Migrator::up(db, steps).await
    }

    /// 回滚迁移
    pub async fn migrate_down(db: &DatabaseConnection, steps: Option<u32>) -> Result<(), DbErr> {
        Migrator::down(db, steps).await
    }

    /// 获取所有迁移名称列表
    pub fn migration_names() -> Vec<String> {
        Migrator::migrations()
            .iter()
            .map(|m| m.name().to_string())
            .collect()
    }

    /// 获取迁移总数
    pub fn migration_count() -> usize {
        Migrator::migrations().len()
    }

    /// 打印迁移状态到终端（友好输出）
    pub async fn print_status(db: &DatabaseConnection) -> Result<(), DbErr> {
        Migrator::status(db).await
    }
}
