use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entities::dept;

/// 部门树节点
#[derive(Debug, serde::Serialize)]
pub struct DeptTreeNode {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub sort: i32,
    pub status: String,
    pub children: Vec<DeptTreeNode>,
}

pub struct DeptRepository;

#[async_trait]
impl Repository<dept::Model, i64> for DeptRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<dept::Model>> {
        dept::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<dept::Model>> {
        crate::pagination::paginate(db, dept::Entity::find(), &query).await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: dept::Model) -> AppResult<dept::Model> {
        let active: dept::ActiveModel = entity.into();
        active.insert(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: dept::Model) -> AppResult<dept::Model> {
        let active: dept::ActiveModel = entity.into();
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        dept::Entity::delete_by_id(id).exec(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl DeptRepository {
    /// 查询部门树
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<DeptTreeNode>> {
        let all = dept::Entity::find()
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
                let parent = self.find_by_id(db, pid).await?
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
        let dept = self.find_by_id(db, dept_id).await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        // 查所有 ancestors 以本部门路径开头的子部门
        let pattern = format!("{},{}%", dept.ancestors, dept_id);
        let children = dept::Entity::find()
            .filter(dept::Column::Ancestors.like(&pattern))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut ids = vec![dept_id];
        for child in children {
            ids.push(child.id);
        }
        Ok(ids)
    }
}

fn build_dept_tree(depts: &[dept::Model], parent_id: Option<i64>) -> Vec<DeptTreeNode> {
    depts.iter()
        .filter(|d| d.parent_id == parent_id)
        .map(|d| DeptTreeNode {
            id: d.id,
            name: d.name.clone(),
            parent_id: d.parent_id,
            sort: d.sort,
            status: d.status.clone(),
            children: build_dept_tree(depts, Some(d.id)),
        })
        .collect()
}
