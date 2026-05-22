use crate::naming;
use crate::schema::TableInfo;

pub fn render_service(table: &TableInfo, _module: &str) -> String {
    let struct_name = naming::to_pascal_case(&table.table_name);
    let snake = naming::to_snake_case(&table.table_name);

    format!(
        r#"use ryframe_common::AppResult;
use sea_orm::DatabaseConnection;

use crate::dto::{snake}_dto::*;

pub struct {struct_name}ServiceImpl {{
    // TODO: 注入 Repository
}}

impl {struct_name}ServiceImpl {{
    pub async fn list(&self, db: &DatabaseConnection) -> AppResult<Vec<{struct_name}Vo>> {{
        // TODO: 业务逻辑
        todo!("实现 {struct_name} 列表查询")
    }}

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        dto: Create{struct_name}Dto,
    ) -> AppResult<{struct_name}Vo> {{
        // TODO: 业务逻辑
        todo!("实现 {struct_name} 创建")
    }}

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        dto: Update{struct_name}Dto,
    ) -> AppResult<{struct_name}Vo> {{
        // TODO: 业务逻辑
        todo!("实现 {struct_name} 更新")
    }}

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {{
        // TODO: 业务逻辑
        todo!("实现 {struct_name} 删除")
    }}
}}
"#,
        struct_name = struct_name,
        snake = snake,
    )
}
