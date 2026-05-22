use ryframe_common::{AppError, AppResult};
use ryframe_db::entities::dept;
use ryframe_db::DeptRepository;
use ryframe_db::repositories::dept_repo::DeptTreeNode;
use sea_orm::DatabaseConnection;
use ryframe_common::utils::snowflake;
use ryframe_core::Repository;

pub struct DeptServiceImpl {
    pub dept_repo: DeptRepository,
}

impl DeptServiceImpl {
    pub async fn find_tree(&self, db: &DatabaseConnection) -> AppResult<Vec<DeptTreeNode>> {
        self.dept_repo.find_tree(db).await
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        name: &str,
        parent_id: Option<i64>,
        sort: i32,
    ) -> AppResult<dept::Model> {
        // 自动计算 ancestors
        let ancestors = self.dept_repo.build_ancestors(db, parent_id).await?;

        let now = chrono::Utc::now();
        let new_dept = dept::Model {
            id: snowflake::next_snowflake_id(),
            name: name.to_string(),
            parent_id,
            ancestors,
            sort,
            status: dept::Model::STATUS_NORMAL.to_string(),
            remark: None,
            created_at: now,
            updated_at: now,
        };
        self.dept_repo.insert(db, new_dept).await
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        name: &str,
        parent_id: Option<i64>,
        sort: i32,
        status: String,
    ) -> AppResult<dept::Model> {
        let mut dept = self.dept_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        // 如果父部门变更，重新计算 ancestors
        if dept.parent_id != parent_id {
            dept.ancestors = self.dept_repo.build_ancestors(db, parent_id).await?;
        }

        dept.name = name.to_string();
        dept.parent_id = parent_id;
        dept.sort = sort;
        dept.status = status;
        dept.updated_at = chrono::Utc::now();

        self.dept_repo.update(db, dept).await
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        self.dept_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("部门不存在".into()))?;

        if self.dept_repo.has_children(db, id).await? {
            return Err(AppError::Validation("存在子部门，无法删除".into()));
        }

        self.dept_repo.delete(db, id).await
    }
}
