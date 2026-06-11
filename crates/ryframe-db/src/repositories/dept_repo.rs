use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::entities::dept;

/// 部门树节点
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct DeptTreeNode {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub sort: i32,
    pub status: String,
    pub children: Vec<DeptTreeNode>,
}

pub struct DeptRepository;

#[async_trait]
impl Repository<dept::Model, i64> for DeptRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<dept::Model>> {
        dept::Entity::find_by_id(id)
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<dept::Model>> {
        crate::pagination::paginate(
            db,
            dept::Entity::find().filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL)),
            &query,
        )
        .await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: dept::Model) -> AppResult<dept::Model> {
        let active: dept::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: dept::Model) -> AppResult<dept::Model> {
        let active: dept::ActiveModel = entity.into();
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let active = dept::ActiveModel {
            id: ActiveValue::Unchanged(id),
            del_flag: ActiveValue::Set(dept::Model::DEL_FLAG_DELETED.to_string()),
            updated_at: ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl DeptRepository {
    /// 查询部门树
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<DeptTreeNode>> {
        let all = dept::Entity::find()
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .order_by_asc(dept::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(build_dept_tree(&all, None))
    }

    /// 检查是否有子部门
    pub async fn has_children(&self, db: &DatabaseConnection, parent_id: i64) -> AppResult<bool> {
        let exists = dept::Entity::find()
            .filter(dept::Column::ParentId.eq(parent_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(exists.is_some())
    }

    /// 根据父部门ID计算 ancestors 值
    ///
    /// 根部门(parent_id=None) 的 ancestors 为 "0"
    /// 子部门的 ancestors 为 "父部门ancestors,父部门id"
    pub async fn build_ancestors(
        &self,
        db: &DatabaseConnection,
        parent_id: Option<i64>,
    ) -> AppResult<String> {
        match parent_id {
            None => Ok("0".to_string()),
            Some(pid) => {
                let parent = self
                    .find_by_id(db, pid)
                    .await?
                    .ok_or_else(|| AppError::NotFound(format!("父部门 {} 不存在", pid)))?;
                Ok(format!("{},{}", parent.ancestors, pid))
            }
        }
    }

    /// 查询所有子部门ID（包含自身）
    pub async fn find_child_dept_ids(
        &self,
        db: &DatabaseConnection,
        dept_id: i64,
    ) -> AppResult<Vec<i64>> {
        // 先查自身
        let dept = self
            .find_by_id(db, dept_id)
            .await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        // 查所有 ancestors 以本部门路径开头的子部门
        let pattern = format!("{},{}%", dept.ancestors, dept_id);
        let children = dept::Entity::find()
            .filter(dept::Column::Ancestors.like(&pattern))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut ids = vec![dept_id];
        for child in children {
            ids.push(child.id);
        }
        Ok(ids)
    }

    /// 带搜索条件的查询（按名称、状态过滤）
    pub async fn find_filtered(
        &self,
        db: &DatabaseConnection,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<Vec<dept::Model>> {
        self.build_filtered_query(name, status)
            .order_by_asc(dept::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        name: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<dept::Model>> {
        crate::pagination::paginate(
            db,
            self.build_filtered_query(name, status)
                .order_by_asc(dept::Column::Sort),
            &query,
        )
        .await
    }

    /// 构建带搜索条件的查询（复用逻辑）
    fn build_filtered_query(
        &self,
        name: Option<&str>,
        status: Option<&str>,
    ) -> sea_orm::Select<dept::Entity> {
        let mut select =
            dept::Entity::find().filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL));
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            select = select.filter(dept::Column::Name.like(format!("%{}%", n)));
        }
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            select = select.filter(dept::Column::Status.eq(s));
        }
        select
    }
}

fn build_dept_tree(depts: &[dept::Model], parent_id: Option<i64>) -> Vec<DeptTreeNode> {
    depts
        .iter()
        .filter(|d| d.parent_id == parent_id)
        .map(|d| DeptTreeNode {
            id: d.id.to_string(),
            name: d.name.clone(),
            parent_id: d.parent_id.map(|p| p.to_string()),
            sort: d.sort,
            status: d.status.clone(),
            children: build_dept_tree(depts, Some(d.id)),
        })
        .collect()
}
