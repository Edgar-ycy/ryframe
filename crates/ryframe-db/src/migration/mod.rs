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

use sea_orm_migration::prelude::*;

/// 所有迁移的集合
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260101_000001_init_tables::Migration),
        ]
    }
}
