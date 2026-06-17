use ryframe_auth::password;
use ryframe_common::{
    AppError, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
    utils::snowflake,
};
use ryframe_core::{
    LoggedRepo, Repository,
    auto_fill::{AutoFill, FillContext},
    repository::{PageQuery, PageResult},
};
use ryframe_db::{
    DeptRepository, RoleRepository, UserRepository,
    entities::{role, user},
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, TransactionTrait};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct UserVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
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
            id: u.id.to_string(),
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
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub code: String,
}

impl From<role::Model> for RoleBriefVo {
    fn from(r: role::Model) -> Self {
        Self {
            id: r.id.to_string(),
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

/// 创建用户参数
pub struct CreateUserParams<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub nickname: &'a str,
    pub email: &'a str,
    pub phone: &'a str,
    pub dept_id: Option<i64>,
    pub role_ids: Option<Vec<i64>>,
    pub enable_pwd_complexity: bool,
}

/// 更新用户参数
pub struct UpdateUserParams<'a> {
    pub id: i64,
    pub nickname: &'a str,
    pub email: &'a str,
    pub phone: &'a str,
    pub dept_id: Option<i64>,
    pub status: String,
    pub role_ids: Option<Vec<i64>>,
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

    /// 带搜索条件和数据权限过滤的分页查询。
    #[allow(clippy::too_many_arguments)]
    pub async fn find_by_page_filtered_with_data_scope(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        username: Option<&str>,
        phone: Option<&str>,
        status: Option<&str>,
        dept_id: Option<i64>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<UserVo>> {
        let page = self
            .user_repo
            .find_by_page_filtered_with_data_scope(
                db,
                query.clone(),
                username,
                phone,
                status,
                dept_id,
                scope_ctx,
            )
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

    /// 分页查询用户列表（自动处理数据权限与过滤）
    ///
    /// Handler 只需传入当前用户 ID 和查询参数即可，本方法内部完成：
    /// 1. 查找用户信息 → 2. 查角色 → 3. 构建 DataScopeContext → 4. 路由查询
    #[allow(clippy::too_many_arguments)]
    pub async fn find_by_page_with_user_scope(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
        query: PageQuery,
        username: Option<&str>,
        phone: Option<&str>,
        status: Option<&str>,
        dept_id: Option<i64>,
    ) -> AppResult<PageResult<UserVo>> {
        let user = self
            .user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::Authentication("用户不存在".into()))?;

        let roles = self.role_repo.find_user_roles(db, user_id).await?;

        let scope_ctx = self
            .build_data_scope_context(db, user_id, user.dept_id, &roles)
            .await?;

        self.find_by_page_filtered_with_data_scope(
            db, query, username, phone, status, dept_id, &scope_ctx,
        )
        .await
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

    pub async fn ensure_user_accessible(
        &self,
        db: &DatabaseConnection,
        id: i64,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<user::Model> {
        self.user_repo
            .find_by_id_with_data_scope(db, id, scope_ctx)
            .await?
            .ok_or_else(|| AppError::Authorization("无权访问该用户数据".into()))
    }

    pub async fn find_by_id_with_roles_with_data_scope(
        &self,
        db: &DatabaseConnection,
        id: i64,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<Option<UserDetailVo>> {
        match self
            .user_repo
            .find_by_id_with_data_scope(db, id, scope_ctx)
            .await?
        {
            Some(u) => {
                let mut vo = UserVo::from(u.clone());
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

    pub async fn is_super_admin_user(&self, db: &DatabaseConnection, id: i64) -> AppResult<bool> {
        let roles = self.role_repo.find_user_roles_all_status(db, id).await?;
        Ok(roles.iter().any(|r| r.code == "admin"))
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        params: CreateUserParams<'_>,
    ) -> AppResult<UserVo> {
        let CreateUserParams {
            username,
            password,
            nickname,
            email,
            phone,
            dept_id,
            role_ids,
            enable_pwd_complexity,
        } = params;

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
        let mut new_user = user::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: "system".to_string(),
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
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        new_user.fill_on_insert(&FillContext::new());

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

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        params: UpdateUserParams<'_>,
    ) -> AppResult<UserVo> {
        let UpdateUserParams {
            id,
            nickname,
            email,
            phone,
            dept_id,
            status,
            role_ids,
        } = params;
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
        user.fill_on_update(&FillContext::new());

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
        user.fill_on_update(&FillContext::new());
        self.user_repo.update(db, user).await?;
        Ok(())
    }
}
