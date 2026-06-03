use crate::{naming, schema::TableInfo};

pub fn render_service(table: &TableInfo, _module: &str) -> String {
    let struct_name = naming::to_pascal_case(&table.table_name);
    let snake = naming::to_snake_case(&table.table_name);
    let pk_type = crate::schema::get_pk_type(table);

    // 生成 Model → Vo 字段映射
    let model_to_vo_fields: String = table
        .columns
        .iter()
        .map(|c| {
            let fn_name = naming::to_snake_case(&c.name);
            format!("            {}: m.{},", fn_name, fn_name)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // 生成 DTO → Model 字段映射（时间字段用 Default::default() 占位，由 AutoFill 填充）
    let dto_to_model_fields: String = table
        .columns
        .iter()
        .map(|c| {
            let fn_name = naming::to_snake_case(&c.name);
            if fn_name == "id"
                || fn_name == "created_at"
                || fn_name == "updated_at"
                || fn_name == "create_time"
                || fn_name == "update_time"
            {
                format!("            {}: Default::default(),", fn_name)
            } else {
                format!("            {}: dto.{},", fn_name, fn_name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // 生成 apply_dto_updates 字段更新
    let update_fields: String = table
        .columns
        .iter()
        .filter(|c| {
            let fn_name = naming::to_snake_case(&c.name);
            fn_name != "id"
                && fn_name != "created_at"
                && fn_name != "updated_at"
                && fn_name != "create_time"
                && fn_name != "update_time"
        })
        .map(|c| {
            let fn_name = naming::to_snake_case(&c.name);
            format!("    active.{} = sea_orm::Set(dto.{});", fn_name, fn_name)
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"use chrono;
use ryframe_common::{{AppResult}};
use ryframe_core::auto_fill::FillContext;
use sea_orm::DatabaseConnection;
use std::sync::Arc;

use crate::dto::{snake}_dto::*;
use super::super::repository::{snake}_repo::{snake_name}Repository;

pub struct {snake_name}ServiceImpl {{
    db: Arc<DatabaseConnection>,
    repo: {snake_name}Repository,
}}

impl {snake_name}ServiceImpl {{
    pub fn new(db: DatabaseConnection) -> Self {{
        Self {{
            db: Arc::new(db),
            repo: {snake_name}Repository,
        }}
    }}

    /// 分页列表查询
    pub async fn list(
        &self,
        page_query: &ryframe_core::PageQuery,
    ) -> AppResult<ryframe_core::PageResult<{snake_name}Vo>> {{
        let result = self.repo.find_by_page(&self.db, page_query.clone()).await?;
        let vos: Vec<{snake_name}Vo> = result
            .records
            .into_iter()
            .map(|m| m.into())
            .collect();
        Ok(ryframe_core::PageResult {{
            records: vos,
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        }})
    }}

    /// 根据 ID 查询
    pub async fn find_by_id(
        &self,
        id: {pk_type},
    ) -> AppResult<Option<{snake_name}Vo>> {{
        let entity = self.repo.find_by_id(&self.db, id).await?;
        Ok(entity.map(|m| m.into()))
    }}

    /// 创建记录
    pub async fn create(
        &self,
        dto: Create{snake_name}Dto,
    ) -> AppResult<{snake_name}Vo> {{
        let mut model: {snake}::Model = dto.into();
        model.fill_on_insert(&FillContext::new());
        let saved = self.repo.insert(&self.db, model).await?;
        Ok(saved.into())
    }}

    /// 更新记录
    pub async fn update(
        &self,
        id: {pk_type},
        dto: Update{snake_name}Dto,
    ) -> AppResult<{snake_name}Vo> {{
        let existing = self
            .repo
            .find_by_id(&self.db, id)
            .await?
            .ok_or_else(|| ryframe_common::AppError::NotFound("记录不存在".into()))?;
        let mut active: {snake}::ActiveModel = existing.into();
        apply_dto_updates(&mut active, dto);
        let updated = active.update(&*self.db).await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(updated.into())
    }}

    /// 删除记录
    pub async fn delete(&self, id: {pk_type}) -> AppResult<()> {{
        self.repo.delete(&self.db, id).await
    }}
}}

/// 将 DTO 更新字段应用到 ActiveModel
fn apply_dto_updates(
    active: &mut {snake}::ActiveModel,
    dto: Update{snake_name}Dto,
) {{
{update_fields}
}}

// 实现 Model → Vo 转换
impl From<{snake}::Model> for {snake_name}Vo {{
    fn from(m: {snake}::Model) -> Self {{
        Self {{
{model_to_vo_fields}
        }}
    }}
}}

// 实现 DTO → Model 转换
impl From<Create{snake_name}Dto> for {snake}::Model {{
    fn from(dto: Create{snake_name}Dto) -> Self {{
        Self {{
{dto_to_model_fields}
        }}
    }}
}}
"#,
        snake_name = struct_name,
        snake = snake,
        pk_type = pk_type,
        model_to_vo_fields = model_to_vo_fields,
        dto_to_model_fields = dto_to_model_fields,
        update_fields = update_fields,
    )
}
