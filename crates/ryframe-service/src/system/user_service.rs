use ryframe_auth::password;
use ryframe_common::{
    AppError, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
    utils::snowflake,
};
use ryframe_core::{
    LoggedRepo, Repository,
    repository::{PageQuery, PageResult},
};
use ryframe_db::{
    DeptRepository, RoleRepository, UserRepository,
    entities::{role, user},
};
use ryframe_macro::datasource;
use sea_orm::{ActiveModelTrait, DatabaseConnection, TransactionTrait};
use serde::Serialize;

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

/// 用户详情视图对象（包含角色列表）
#[derive(Debug, Serialize)]
pub struct UserDetailVo {
    #[serde(flatten)]
    pub user: UserVo,
    pub roles: Vec<RoleBriefVo>,
}

/// 角色简要信息
#[derive(Debug, Serialize)]
pub struct RoleBriefVo {
    pub id: i64,
    pub name: String,
    pub code: String,
}

impl From<role::Model> for RoleBriefVo {
    fn from(r: role::Model) -> Self {
        Self {
            id: r.id,
            name: r.name,
            code: r.code,
        }
    }
}

pub struct UserServiceImpl {
    pub user_repo: LoggedRepo<UserRepository>,
    pub role_repo: LoggedRepo<RoleRepository>,
    pub dept_repo: LoggedRepo<DeptRepository>,
}

impl UserServiceImpl {
    /// 批量补充 dept_name
    async fn fill_dept_names(&self, db: &DatabaseConnection, records: &mut [UserVo]) {
        for vo in records.iter_mut() {
            if let Some(dept_id) = vo.dept_id
                && let Ok(Some(dept)) = self.dept_repo.find_by_id(db, dept_id).await
            {
                vo.dept_name = Some(dept.name);
            }
        }
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        username: Option<&str>,
        phone: Option<&str>,
        status: Option<&str>,
        dept_id: Option<i64>,
    ) -> AppResult<PageResult<UserVo>> {
        let page = self
            .user_repo
            .find_by_page_filtered(db, query.clone(), username, phone, status, dept_id)
            .await?;
        let mut records: Vec<UserVo> = page.records.into_iter().map(UserVo::from).collect();
        self.fill_dept_names(db, &mut records).await;
        Ok(PageResult::new(records, page.total, &query))
    }

    /// 批量删除用户
    pub async fn delete_many(&self, db: &DatabaseConnection, ids: &[i64]) -> AppResult<u64> {
        if ids.is_empty() {
            return Err(AppError::Validation("请选择要删除的用户".into()));
        }
        self.user_repo.delete_many(db, ids).await
    }

    /// 修改用户状态
    pub async fn change_status(
        &self,
        db: &DatabaseConnection,
        id: i64,
        status: String,
    ) -> AppResult<()> {
        // 检查用户存在
        self.user_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        self.user_repo.update_status(db, id, status).await
    }

    pub async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<UserVo>> {
        let page = self.user_repo.find_by_page(db, query.clone()).await?;
        let mut records: Vec<UserVo> = page.records.into_iter().map(UserVo::from).collect();
        self.fill_dept_names(db, &mut records).await;

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
        let page = self
            .user_repo
            .find_by_page_with_data_scope(db, query.clone(), scope_ctx)
            .await?;
        let mut records: Vec<UserVo> = page.records.into_iter().map(UserVo::from).collect();
        self.fill_dept_names(db, &mut records).await;

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
                self.fill_dept_names(db, std::slice::from_mut(&mut vo))
                    .await;
                Ok(Some(vo))
            }
            None => Ok(None),
        }
    }

    /// 查询用户详情（包含角色列表）
    pub async fn find_by_id_with_roles(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<UserDetailVo>> {
        match self.user_repo.find_by_id(db, id).await? {
            Some(u) => {
                let mut vo = UserVo::from(u);
                self.fill_dept_names(db, std::slice::from_mut(&mut vo))
                    .await;
                let roles = self.role_repo.find_user_roles(db, id).await?;
                let role_vos: Vec<RoleBriefVo> = roles.into_iter().map(RoleBriefVo::from).collect();
                Ok(Some(UserDetailVo {
                    user: vo,
                    roles: role_vos,
                }))
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
        enable_pwd_complexity: bool,
    ) -> AppResult<UserVo> {
        // 密码复杂度校验
        if enable_pwd_complexity {
            password::validate_complexity(password)?;
        }

        // 检查用户名唯一
        if self
            .user_repo
            .find_by_username(db, username)
            .await?
            .is_some()
        {
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
            del_flag: user::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: now,
            updated_at: now,
        };

        // 使用事务：插入用户 + 分配角色
        let txn = db
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("开启事务失败: {}", e)))?;

        let active: user::ActiveModel = new_user.into();
        let saved = match active
            .insert(&txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
        {
            Ok(s) => s,
            Err(err) => {
                let _ = txn.rollback().await;
                return Err(err);
            }
        };

        // 分配角色
        if let Some(role_ids) = &role_ids
            && !role_ids.is_empty()
            && let Err(err) = self
                .role_repo
                .assign_roles_in_txn(&txn, saved.id, role_ids)
                .await
        {
            let _ = txn.rollback().await;
            return Err(err);
        }

        txn.commit()
            .await
            .map_err(|e| AppError::Database(format!("提交事务失败: {}", e)))?;
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
        let mut user = self
            .user_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        user.nickname = nickname.to_string();
        user.email = email.to_string();
        user.phone = phone.to_string();
        user.dept_id = dept_id;
        user.status = status;
        user.updated_at = chrono::Utc::now();

        // 使用事务：更新用户 + 更新角色
        let txn = db
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("开启事务失败: {}", e)))?;

        let active: user::ActiveModel = user.into();
        let saved = match active
            .update(&txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
        {
            Ok(s) => s,
            Err(err) => {
                let _ = txn.rollback().await;
                return Err(err);
            }
        };

        // 更新角色
        if let Some(role_ids) = &role_ids
            && let Err(err) = self
                .role_repo
                .assign_roles_in_txn(&txn, saved.id, role_ids)
                .await
        {
            let _ = txn.rollback().await;
            return Err(err);
        }

        txn.commit()
            .await
            .map_err(|e| AppError::Database(format!("提交事务失败: {}", e)))?;
        Ok(UserVo::from(saved))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        // 检查用户存在
        self.user_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        self.user_repo.delete(db, id).await
    }

    pub async fn reset_password(
        &self,
        db: &DatabaseConnection,
        id: i64,
        new_password: &str,
        enable_pwd_complexity: bool,
    ) -> AppResult<()> {
        // 密码复杂度校验
        if enable_pwd_complexity {
            password::validate_complexity(new_password)?;
        }

        let mut user = self
            .user_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        user.password_hash = password::hash(new_password)?;
        user.updated_at = chrono::Utc::now();
        self.user_repo.update(db, user).await?;
        Ok(())
    }

    /// 从从库查询用户（演示 `#[datasource]` 多数据源注解）
    ///
    /// 注解 `#[datasource("replica_0")]` 使此方法内所有 `repo.db()` 调用
    /// 自动走第一个从库连接，无需显式传递 `&DatabaseConnection`。
    ///
    /// # 验证方法
    ///
    /// 1. 确保 `app.dev.toml` 中配置了 `[[database.connections]]`（至少 2 个）
    /// 2. 在两个库的 `sys_user` 表中插入不同数据
    /// 3. 调用本方法 → 返回的是从库的数据
    /// 4. 调用不带注解的 `find_by_page` → 返回的是主库的数据
    #[datasource("db_1")]
    pub async fn find_by_page_from_replica(
        &self,
        query: PageQuery,
    ) -> AppResult<PageResult<UserVo>> {
        let db = self.user_repo.db(); // ← 从 task-local 解析为 db_1 连接
        let page = self.user_repo.find_by_page(&db, query.clone()).await?;
        let mut records: Vec<UserVo> = page.records.into_iter().map(UserVo::from).collect();
        self.fill_dept_names(&db, &mut records).await;
        Ok(PageResult::new(records, page.total, &query))
    }

    /// 从命名数据源查询（演示多数据源切换）
    #[datasource("db_2")]
    pub async fn find_by_page_from_order_db(
        &self,
        query: PageQuery,
    ) -> AppResult<PageResult<UserVo>> {
        let db = self.user_repo.db(); // ← 从 task-local 解析为 db_2 连接
        let page = self.user_repo.find_by_page(&db, query.clone()).await?;
        let mut records: Vec<UserVo> = page.records.into_iter().map(UserVo::from).collect();
        self.fill_dept_names(&db, &mut records).await;
        Ok(PageResult::new(records, page.total, &query))
    }
}
