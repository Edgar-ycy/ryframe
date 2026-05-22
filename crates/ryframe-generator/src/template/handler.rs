use crate::naming;
use crate::schema::TableInfo;

pub fn render_handler(table: &TableInfo, _module: &str) -> String {
    let struct_name = naming::to_pascal_case(&table.table_name);
    let snake = naming::to_snake_case(&table.table_name);

    format!(
        r#"use axum::{{extract::{{Path, State}}, routing::{{get, post, put, delete}}, Json, Router}};
use ryframe_common::AppResult;

use crate::dto::{snake}_dto::*;

// TODO: 替换 State<()> 中的 () 为你的 AppState 类型
// TODO: 在调用处使用 router.with_state(your_app_state)

pub fn {snake}_router() -> Router<()> {{
    Router::new()
        .route("/", get(list).post(create))
        .route("/{{id}}", put(update).delete(delete_entity))
}}

async fn list(
    State(_state): State<()>,
) -> AppResult<Json<Vec<{struct_name}Vo>>> {{
    todo!("实现 {struct_name} 列表 Handler")
}}

async fn create(
    State(_state): State<()>,
    Json(dto): Json<Create{struct_name}Dto>,
) -> AppResult<Json<{struct_name}Vo>> {{
    todo!("实现 {struct_name} 创建 Handler")
}}

async fn update(
    State(_state): State<()>,
    Path(id): Path<i64>,
    Json(dto): Json<Update{struct_name}Dto>,
) -> AppResult<Json<{struct_name}Vo>> {{
    todo!("实现 {struct_name} 更新 Handler")
}}

async fn delete_entity(
    State(_state): State<()>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {{
    todo!("实现 {struct_name} 删除 Handler")
}}
"#,
        struct_name = struct_name,
        snake = snake,
    )
}
