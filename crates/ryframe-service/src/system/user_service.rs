use ryframe_auth::password;
use ryframe_common::annotations::data_scope::{DataScope, DataScopeContext};
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult};
use ryframe_db::entities::{role, user};
use ryframe_db::{DeptRepository, RoleRepository, UserRepository};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use ryframe_common::utils::snowflake;
use ryframe_core::Repository;

#[derive(Debug, Serialize)]
pub struct UserVo {
    pub id: i64,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub status: String,
    pub dept_id: Option<i64>,
    pub dept_name: Option<String>,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<user::Model> for UserVo {
    fn from(u: user::Model) -> Self {
        Self {
            id: u.id,
            username: u.username,
            nickname: u.nickname,
            email: u.email,
            phone: u.phone,
            avatar: u.avatar,
            status: u.status,
            dept_id: u.dept_id,
            dept_name: None,
            remark: u.remark,
            created_at: u.created_at,
        }
    }
}

pub struct UserServiceImpl {
    pub user_repo: UserRepository,
    pub role_repo: RoleRepository,
    pub dept_repo: DeptRepository,
}

impl UserServiceImpl {
    pub async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<UserVo>> {
        let page = self.user_repo.find_by_page(db, query.clone()).await?;
        let mut records: Vec<UserVo> = page.records.into_iter().map(UserVo::from).collect();

        // 补充 dept_name
        for vo in &mut records {
            if let Some(dept_id) = vo.dept_id
                && let Ok(Some(dept)) = self.dept_repo.find_by_id(db, dept_id).await {
                    vo.dept_name = Some(dept.name);
                }
        }

        Ok(PageResult::new(records, page.total, &query))
    }

    /// 带数据权限过滤的分页查询
    ///
    /// 根据当前登录用户的数据范围自动过滤可见用户列表。
    pub async fn find_by_page_with_data_scope(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<UserVo>> {
        let page = self.user_repo.find_by_page_with_data_scope(db, query.clone(), scope_ctx).await?;
        let mut records: Vec<UserVo> = page.records.into_iter().map(UserVo::from).collect();

        // 补充 dept_name
        for vo in &mut records {
            if let Some(dept_id) = vo.dept_id
                && let Ok(Some(dept)) = self.dept_repo.find_by_id(db, dept_id).await {
                    vo.dept_name = Some(dept.name);
                }
        }

        Ok(PageResult::new(records, page.total, &query))
    }

    /// 构建用户的数据权限上下文
    ///
    /// 根据用户的所有角色合并数据权限范围，查询自定义部门列表。
    pub async fn build_data_scope_context(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
        dept_id: Option<i64>,
        roles: &[role::Model],
    ) -> AppResult<DataScopeContext> {
        // 超级管理员直接返回 All
        let is_admin = roles.iter().any(|r| r.code == "admin");
        if is_admin {
            return Ok(DataScopeContext::super_admin(user_id));
        }

        // 获取部门 ancestors
        let ancestors = if let Some(did) = dept_id {
            if let Ok(Some(dept)) = self.dept_repo.find_by_id(db, did).await {
                Some(dept.ancestors)
            } else {
                None
            }
        } else {
            None
        };

        // 合并所有角色的数据权限，取最宽松的
        let mut scopes = Vec::new();
        let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();
        let custom_dept_ids = self.role_repo.find_roles_dept_ids(db, &role_ids).await?;

        for role in roles {
            let scope = DataScope::from_db_value(&role.data_scope);
            scopes.push(DataScopeContext {
                scope,
                user_id,
                dept_id,
                ancestors: ancestors.clone(),
                custom_dept_ids: custom_dept_ids.clone(),
            });
        }

        if scopes.is_empty() {
            // 无角色 → 只能看自己
            return Ok(DataScopeContext {
                scope: DataScope::SelfOnly,
                user_id,
                dept_id,
                ancestors,
                custom_dept_ids: vec![],
            });
        }

        Ok(DataScopeContext::merge(scopes))
    }

    pub async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<UserVo>> {
        match self.user_repo.find_by_id(db, id).await? {
            Some(u) => {
                let mut vo = UserVo::from(u);
                if let Some(dept_id) = vo.dept_id
                    && let Ok(Some(dept)) = self.dept_repo.find_by_id(db, dept_id).await {
                        vo.dept_name = Some(dept.name);
                    }
                Ok(Some(vo))
            }
            None => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        db: &DatabaseConnection,
        username: &str,
        password: &str,
        nickname: &str,
        email: &str,
        phone: &str,
        dept_id: Option<i64>,
        role_ids: Option<Vec<i64>>,
    ) -> AppResult<UserVo> {
        // 检查用户名唯一
        if self.user_repo.find_by_username(db, username).await?.is_some() {
            return Err(AppError::Conflict("用户名已存在".into()));
        }

        let password_hash = password::hash(password)?;
        let now = chrono::Utc::now();
        let new_user = user::Model {
            id: snowflake::next_snowflake_id(),
            username: username.to_string(),
            password_hash,
            nickname: nickname.to_string(),
            email: email.to_string(),
            phone: phone.to_string(),
            avatar: None,
            status: user::Model::STATUS_NORMAL.to_string(),
            dept_id,
            remark: None,
            login_ip: None,
            login_date: None,
            created_at: now,
            updated_at: now,
        };

        let saved = self.user_repo.insert(db, new_user).await?;

        // 分配角色
        if let Some(role_ids) = role_ids
            && !role_ids.is_empty() {
                self.role_repo.assign_roles(db, saved.id, &role_ids).await?;
            }

        Ok(UserVo::from(saved))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        db: &DatabaseConnection,
        id: i64,
        nickname: &str,
        email: &str,
        phone: &str,
        dept_id: Option<i64>,
        status: String,
        role_ids: Option<Vec<i64>>,
    ) -> AppResult<UserVo> {
        let mut user = self.user_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        user.nickname = nickname.to_string();
        user.email = email.to_string();
        user.phone = phone.to_string();
        user.dept_id = dept_id;
        user.status = status;
        user.updated_at = chrono::Utc::now();

        let saved = self.user_repo.update(db, user).await?;

        // 更新角色
        if let Some(role_ids) = role_ids {
            self.role_repo.assign_roles(db, saved.id, &role_ids).await?;
        }

        Ok(UserVo::from(saved))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        // 检查用户存在
        self.user_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        self.user_repo.delete(db, id).await
    }

    pub async fn reset_password(
        &self,
        db: &DatabaseConnection,
        id: i64,
        new_password: &str,
    ) -> AppResult<()> {
        let mut user = self.user_repo.find_by_id(db, id).await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        user.password_hash = password::hash(new_password)?;
        user.updated_at = chrono::Utc::now();
        self.user_repo.update(db, user).await?;
        Ok(())
    }
}
