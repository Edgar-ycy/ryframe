use ryframe_auth::password;
use ryframe_auth::permission::resolve_user_permission_context;
use ryframe_common::{
    AppError, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
    utils::snowflake,
};
use ryframe_core::{
    LoggedRepo, RedisClient, Repository,
    auto_fill::{AutoFill, FillContext},
    cache::clear_user_permission_cache,
    repository::{PageQuery, PageResult},
};
use ryframe_db::{
    DeptRepository, PasswordResetRequestRepository, RoleRepository, UserRepository,
    entities::{password_reset_request, role, user},
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set, TransactionTrait};
use serde::Serialize;
use uuid::Uuid;

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

#[derive(Debug)]
pub struct PasswordResetRequestOutcome {
    pub request: password_reset_request::Model,
    pub token: String,
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
    pub is_super: i8,
}

impl From<role::Model> for RoleBriefVo {
    fn from(r: role::Model) -> Self {
        Self {
            id: r.id.to_string(),
            name: r.name,
            code: r.code,
            is_super: r.is_super,
        }
    }
}

pub struct UserServiceImpl {
    pub user_repo: LoggedRepo<UserRepository>,
    pub role_repo: LoggedRepo<RoleRepository>,
    pub dept_repo: LoggedRepo<DeptRepository>,
    pub redis: Option<RedisClient>,
}

/// 创建用户参数
pub struct CreateUserParams<'a> {
    pub username: &'a str,
    pub nickname: &'a str,
    pub email: &'a str,
    pub phone: &'a str,
    pub dept_id: Option<i64>,
    pub role_ids: Option<Vec<i64>>,
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
    async fn invalidate_permission_cache(&self, user_id: i64) {
        if let Some(redis) = &self.redis
            && let Err(error) =
                clear_user_permission_cache(redis, &ryframe_core::current_tenant_id(), user_id)
                    .await
        {
            tracing::warn!(user_id, %error, "failed to clear user permission cache");
        }
    }

    /// Replace all roles assigned to one user.
    pub async fn assign_roles(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
        role_ids: Vec<i64>,
    ) -> AppResult<()> {
        self.user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        self.validate_assignments(db, None, Some(&role_ids)).await?;

        let mut unique_role_ids = role_ids;
        unique_role_ids.sort_unstable();
        unique_role_ids.dedup();
        let txn = db
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        self.role_repo
            .assign_roles_in_txn(&txn, user_id, &unique_role_ids)
            .await?;
        txn.commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        self.invalidate_permission_cache(user_id).await;
        Ok(())
    }

    /// Resolve all current API permission codes for one user, using Redis first.
    pub async fn get_user_all_perms(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
    ) -> AppResult<Vec<String>> {
        Ok(resolve_user_permission_context(
            db,
            self.redis.as_ref(),
            &ryframe_core::current_tenant_id(),
            user_id,
        )
        .await?
        .permissions)
    }

    /// Return department IDs visible to a user under the merged role data scope.
    pub async fn get_user_access_dept_ids(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
    ) -> AppResult<Vec<i64>> {
        let user = self
            .user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        let roles = self.role_repo.find_user_roles(db, user_id).await?;
        let scope = self
            .build_data_scope_context(db, user_id, user.dept_id, &roles)
            .await?;

        let mut ids = match scope.scope {
            DataScope::All => self
                .dept_repo
                .find_filtered(db, None, None)
                .await?
                .into_iter()
                .map(|dept| dept.id)
                .collect(),
            DataScope::Custom => scope.custom_dept_ids,
            DataScope::Dept | DataScope::SelfOnly => scope.dept_id.into_iter().collect(),
            DataScope::DeptAndChildren => match scope.dept_id {
                Some(dept_id) => self.dept_repo.find_child_dept_ids(db, dept_id).await?,
                None => Vec::new(),
            },
        };
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    async fn validate_assignments(
        &self,
        db: &DatabaseConnection,
        dept_id: Option<i64>,
        role_ids: Option<&[i64]>,
    ) -> AppResult<()> {
        if let Some(dept_id) = dept_id
            && self.dept_repo.find_by_id(db, dept_id).await?.is_none()
        {
            return Err(AppError::Validation("部门不存在或不属于当前租户".into()));
        }
        if let Some(role_ids) = role_ids {
            let mut unique = role_ids.to_vec();
            unique.sort_unstable();
            unique.dedup();
            for role_id in unique {
                if self.role_repo.find_by_id(db, role_id).await?.is_none() {
                    return Err(AppError::Validation("角色不存在或不属于当前租户".into()));
                }
            }
        }
        Ok(())
    }

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
        let affected = self.user_repo.delete_many(db, ids).await?;
        for user_id in ids {
            self.invalidate_permission_cache(*user_id).await;
        }
        Ok(affected)
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
        self.user_repo.update_status(db, id, status).await?;
        self.invalidate_permission_cache(id).await;
        Ok(())
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
        let is_admin = roles.iter().any(|r| r.is_super == 1);
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
        let custom_role_ids: Vec<i64> = roles
            .iter()
            .filter(|role| role.data_scope == role::Model::DATA_SCOPE_CUSTOM)
            .map(|role| role.id)
            .collect();
        let custom_dept_ids = self
            .role_repo
            .find_roles_dept_ids(db, &custom_role_ids)
            .await?;

        for role in roles {
            let scope = DataScope::from_db_value(&role.data_scope);
            let scope_dept_ids = match scope {
                DataScope::Custom => custom_dept_ids.clone(),
                DataScope::Dept => dept_id.into_iter().collect(),
                DataScope::DeptAndChildren => match dept_id {
                    Some(dept_id) => self.dept_repo.find_child_dept_ids(db, dept_id).await?,
                    None => Vec::new(),
                },
                DataScope::All | DataScope::SelfOnly => Vec::new(),
            };
            scopes.push(DataScopeContext {
                scope,
                user_id,
                dept_id,
                ancestors: ancestors.clone(),
                custom_dept_ids: scope_dept_ids,
                include_self: false,
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
                include_self: true,
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
        Ok(roles.iter().any(|r| r.is_super == 1))
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        params: CreateUserParams<'_>,
    ) -> AppResult<UserVo> {
        ryframe_db::TenantRepository
            .ensure_user_quota(db, &ryframe_core::current_tenant_id())
            .await?;
        let CreateUserParams {
            username,
            nickname,
            email,
            phone,
            dept_id,
            role_ids,
        } = params;

        self.validate_assignments(db, dept_id, role_ids.as_deref())
            .await?;

        // 检查用户名唯一
        if self
            .user_repo
            .find_by_username(db, username)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("用户名已存在".into()));
        }

        let activation_secret = format!("pending:{}", Uuid::new_v4());
        let password_hash = password::hash(&activation_secret)?;
        let status = user::Model::STATUS_PENDING_ACTIVATION;
        let mut new_user = user::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: ryframe_core::current_tenant_id(),
            username: username.to_string(),
            password_hash,
            nickname: nickname.to_string(),
            email: email.to_string(),
            phone: phone.to_string(),
            avatar: None,
            status: status.to_string(),
            auth_version: 1,
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
        self.invalidate_permission_cache(saved.id).await;
        Ok(UserVo::from(saved))
    }

    pub async fn request_password_reset(
        &self,
        db: &DatabaseConnection,
        target_user_id: i64,
        requested_by: i64,
        reason: &str,
        request_ip: Option<String>,
    ) -> AppResult<PasswordResetRequestOutcome> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(AppError::Validation("密码重置原因不能为空".into()));
        }

        self.user_repo
            .find_by_id(db, target_user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        if self.is_super_admin_user(db, target_user_id).await? {
            return Err(AppError::Authorization("禁止操作超级管理员".into()));
        }

        let token = Uuid::new_v4().to_string();
        let mut request = password_reset_request::Model {
            id: snowflake::next_snowflake_id(),
            tenant_id: ryframe_core::current_tenant_id(),
            target_user_id,
            requested_by,
            reason: reason.to_string(),
            token_hash: password::hash(&token)?,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
            completed_at: None,
            request_ip,
            status: password_reset_request::Model::STATUS_PENDING.to_string(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        request.fill_on_insert(&FillContext::new());

        let request = PasswordResetRequestRepository.insert(db, request).await?;
        Ok(PasswordResetRequestOutcome { request, token })
    }

    pub async fn complete_password_reset(
        &self,
        db: &DatabaseConnection,
        request_id: i64,
        token: &str,
        new_password: &str,
        enable_pwd_complexity: bool,
    ) -> AppResult<i64> {
        let token = token.trim();
        if token.is_empty() {
            return Err(AppError::Validation("密码重置令牌不能为空".into()));
        }
        if enable_pwd_complexity {
            password::validate_complexity(new_password)?;
        }

        let mut reset_request = PasswordResetRequestRepository
            .find_by_id(db, request_id)
            .await?
            .ok_or_else(|| AppError::NotFound("密码重置请求不存在".into()))?;

        if reset_request.status != password_reset_request::Model::STATUS_PENDING
            || reset_request.completed_at.is_some()
        {
            return Err(AppError::Validation("密码重置请求已处理".into()));
        }

        if reset_request.expires_at <= chrono::Utc::now() {
            reset_request.status = password_reset_request::Model::STATUS_EXPIRED.to_string();
            reset_request.fill_on_update(&FillContext::new());
            PasswordResetRequestRepository
                .update(db, reset_request)
                .await?;
            return Err(AppError::Validation("密码重置请求已过期".into()));
        }

        if !password::verify(token, &reset_request.token_hash)? {
            return Err(AppError::Authentication("密码重置令牌无效".into()));
        }

        let mut target_user = self
            .user_repo
            .find_by_id(db, reset_request.target_user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        if self.is_super_admin_user(db, target_user.id).await? {
            return Err(AppError::Authorization("禁止操作超级管理员".into()));
        }

        let target_user_id = target_user.id;
        target_user.password_hash = password::hash(new_password)?;
        if target_user.status == user::Model::STATUS_PENDING_ACTIVATION
            || target_user.status == user::Model::STATUS_MUST_RESET_PASSWORD
        {
            target_user.status = user::Model::STATUS_NORMAL.to_string();
        }
        target_user.fill_on_update(&FillContext::new());

        reset_request.status = password_reset_request::Model::STATUS_COMPLETED.to_string();
        reset_request.completed_at = Some(chrono::Utc::now());
        reset_request.fill_on_update(&FillContext::new());

        let password_hash = target_user.password_hash.clone();
        let user_status = target_user.status.clone();
        let user_updated_at = target_user.updated_at;
        let request_status = reset_request.status.clone();
        let request_completed_at = reset_request.completed_at;
        let request_updated_at = reset_request.updated_at;

        let txn = db
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("开启事务失败: {}", e)))?;

        let mut user_active: user::ActiveModel = target_user.into();
        user_active.password_hash = Set(password_hash);
        user_active.status = Set(user_status);
        user_active.updated_at = Set(user_updated_at);
        if let Err(err) = user_active
            .update(&txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
        {
            let _ = txn.rollback().await;
            return Err(err);
        }

        let mut request_active: password_reset_request::ActiveModel = reset_request.into();
        request_active.status = Set(request_status);
        request_active.completed_at = Set(request_completed_at);
        request_active.updated_at = Set(request_updated_at);
        if let Err(err) = request_active
            .update(&txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
        {
            let _ = txn.rollback().await;
            return Err(err);
        }

        txn.commit()
            .await
            .map_err(|e| AppError::Database(format!("提交事务失败: {}", e)))?;
        Ok(target_user_id)
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
        self.validate_assignments(db, dept_id, role_ids.as_deref())
            .await?;
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
        self.invalidate_permission_cache(saved.id).await;
        Ok(UserVo::from(saved))
    }

    pub async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        // 检查用户存在
        self.user_repo
            .find_by_id(db, id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        self.user_repo.delete(db, id).await?;
        self.invalidate_permission_cache(id).await;
        Ok(())
    }
}
