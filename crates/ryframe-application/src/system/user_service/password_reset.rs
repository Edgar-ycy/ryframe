use ryframe_adapters::{
    Repository,
    auto_fill::{AutoFill, FillContext},
};
use ryframe_auth::password;
use ryframe_db::{
    PasswordResetRequestRepository, TenantRepository,
    entities::{password_reset_request, user_role},
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
    sea_query::{Expr, LockType},
};
use uuid::Uuid;

use super::{PasswordResetRequestOutcome, UserService};

impl UserService {
    pub async fn request_password_reset(
        &self,
        actor: &ActorContext,
        target_user_id: i64,
        reason: &str,
        request_ip: Option<String>,
    ) -> AppResult<PasswordResetRequestOutcome> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(AppError::Validation("密码重置原因不能为空".into()));
        }
        self.ensure_user_accessible(actor, target_user_id).await?;
        self.ensure_not_super_admin_user(actor, target_user_id)
            .await?;

        let token = Uuid::new_v4().to_string();
        let database_now = PasswordResetRequestRepository
            .database_utc_now(self.db.write())
            .await?;
        let mut request = password_reset_request::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: tenant_id.to_owned(),
            target_user_id,
            requested_by: actor.user_id,
            reason: reason.to_owned(),
            token_hash: password::hash(&token)?,
            expires_at: database_now + chrono::Duration::hours(24),
            completed_at: None,
            request_ip,
            status: password_reset_request::Model::STATUS_PENDING.into(),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        request.fill_on_insert(&FillContext::new())?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, tenant_id)
            .await?;
        self.lock_manageable_user_in_txn(actor, &transaction, target_user_id)
            .await?;
        let request = password_reset_request::ActiveModel::from(request)
            .insert(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        crate::commit_current_audit(transaction).await?;
        Ok(PasswordResetRequestOutcome { request, token })
    }

    pub async fn complete_password_reset_request(
        &self,
        tenant_id: &str,
        request_id: i64,
        token: &str,
        new_password: &str,
    ) -> AppResult<i64> {
        ryframe_adapters::validate_explicit_tenant(tenant_id)?;
        self.complete_password_reset(tenant_id, request_id, token, new_password)
            .await
    }

    async fn complete_password_reset(
        &self,
        tenant_id: &str,
        request_id: i64,
        token: &str,
        new_password: &str,
    ) -> AppResult<i64> {
        let token = token.trim();
        if token.is_empty() {
            return Err(AppError::Validation("密码重置令牌不能为空".into()));
        }
        password::validate_complexity(new_password)?;

        let reset_request = PasswordResetRequestRepository
            .find_by_id(self.db.write(), tenant_id, request_id)
            .await?
            .ok_or_else(|| AppError::NotFound("密码重置请求不存在".into()))?;
        if reset_request.status != password_reset_request::Model::STATUS_PENDING
            || reset_request.completed_at.is_some()
        {
            return Err(AppError::Validation("密码重置请求已处理".into()));
        }
        let evaluated_at = PasswordResetRequestRepository
            .database_utc_now(self.db.write())
            .await?;
        // 昂贵的密码散列留在事务外执行；过期请求进入事务后以当前读原子落为 expired。
        // 令牌、用户与角色仍会在持锁后重新校验，事务外检查仅用于尽早拒绝明显无效请求。
        let password_hash = if reset_request.expires_at > evaluated_at {
            if !password::verify(token, &reset_request.token_hash)? {
                return Err(AppError::Authentication("密码重置令牌无效".into()));
            }
            let target_user = self
                .user_repo
                .find_by_id(self.db.write(), tenant_id, reset_request.target_user_id)
                .await?
                .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
            let roles = self
                .role_repo
                .find_user_roles_all_status(self.db.write(), tenant_id, target_user.id)
                .await?;
            if roles.iter().any(|role| role.is_super == 1) {
                return Err(AppError::Authorization("禁止操作超级管理员".into()));
            }
            Some((password::hash(new_password)?, target_user))
        } else {
            None
        };
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启事务失败: {error}")))?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, tenant_id)
            .await?;
        let current_request = password_reset_request::Entity::find_by_id(request_id)
            .filter(password_reset_request::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::NotFound("密码重置请求不存在".into()))?;
        let completed_at = PasswordResetRequestRepository
            .database_utc_now(&transaction)
            .await?;
        if current_request.status != password_reset_request::Model::STATUS_PENDING
            || current_request.completed_at.is_some()
        {
            return Err(AppError::Validation("密码重置请求已处理".into()));
        }
        if current_request.expires_at <= completed_at {
            let mut expired_request = current_request;
            expired_request.status = password_reset_request::Model::STATUS_EXPIRED.into();
            expired_request.updated_at = completed_at;
            password_reset_request::ActiveModel::from(expired_request)
                .reset_all()
                .update(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            crate::commit_current_audit(transaction).await?;
            return Err(AppError::Validation("密码重置请求已过期".into()));
        }
        if !password::verify(token, &current_request.token_hash)? {
            return Err(AppError::Authentication("密码重置令牌无效".into()));
        }

        let user_id = current_request.target_user_id;
        let current_user = ryframe_db::entities::user::Entity::find_by_id(user_id)
            .filter(ryframe_db::entities::user::Column::TenantId.eq(tenant_id))
            .filter(
                ryframe_db::entities::user::Column::DelFlag
                    .eq(ryframe_db::entities::user::Model::DEL_FLAG_NORMAL),
            )
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        let (password_hash, target_user) = password_hash
            .ok_or_else(|| AppError::Conflict("密码重置请求状态已发生变化，请重新提交".into()))?;
        if current_request.target_user_id != target_user.id
            || current_user.authorization_version != target_user.authorization_version
            || current_user.status != target_user.status
        {
            return Err(AppError::Conflict(
                "用户认证状态已发生变化，请重新发起密码重置".into(),
            ));
        }
        // 关系与角色都使用当前读，并固定按角色 ID 升序锁定，确保与角色替换路径
        // 共享 tenant -> reset request -> user -> roles 的单向锁序。
        let role_ids = user_role::Entity::find()
            .filter(user_role::Column::TenantId.eq(tenant_id))
            .filter(user_role::Column::UserId.eq(user_id))
            .order_by_asc(user_role::Column::RoleId)
            .lock(LockType::Update)
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .into_iter()
            .map(|relation| relation.role_id)
            .collect::<Vec<_>>();
        let current_roles = if role_ids.is_empty() {
            Vec::new()
        } else {
            ryframe_db::entities::role::Entity::find()
                .filter(ryframe_db::entities::role::Column::TenantId.eq(tenant_id))
                .filter(
                    ryframe_db::entities::role::Column::DelFlag
                        .eq(ryframe_db::entities::role::Model::DEL_FLAG_NORMAL),
                )
                .filter(ryframe_db::entities::role::Column::Id.is_in(role_ids.iter().copied()))
                .order_by_asc(ryframe_db::entities::role::Column::Id)
                .lock(LockType::Update)
                .all(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?
        };
        if current_roles.len() != role_ids.len() {
            return Err(AppError::Conflict(
                "用户角色状态已发生变化，请重新发起密码重置".into(),
            ));
        }
        if current_roles.iter().any(|role| role.is_super == 1) {
            return Err(AppError::Authorization("禁止操作超级管理员".into()));
        }

        let next_status = if matches!(
            current_user.status.as_str(),
            ryframe_db::entities::user::Model::STATUS_PENDING_ACTIVATION
                | ryframe_db::entities::user::Model::STATUS_MUST_RESET_PASSWORD
        ) {
            ryframe_db::entities::user::Model::STATUS_NORMAL.to_owned()
        } else {
            current_user.status.clone()
        };
        let consumed = PasswordResetRequestRepository
            .complete_pending_in_txn(&transaction, tenant_id, request_id, completed_at)
            .await?;
        if !consumed {
            return Err(AppError::Validation("密码重置请求已处理或已过期".into()));
        }

        // 仅更新认证字段。守卫已观察到的状态，避免并发的管理员操作或密码修改被覆盖。
        let update_result =
            guarded_password_update(&current_user, password_hash, next_status, completed_at)
                .exec(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        if update_result.rows_affected != 1 {
            return Err(AppError::Conflict(
                "用户认证状态已发生变化，请重新发起密码重置".into(),
            ));
        }
        let authorization_version = current_user.authorization_version.saturating_add(1);
        self.authorization_cache
            .record_user_mirror_update_in_transaction(
                &transaction,
                tenant_id,
                user_id,
                authorization_version,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.authorization_cache
            .sync_user_versions(tenant_id, &[(user_id, authorization_version)])
            .await?;
        Ok(user_id)
    }
}

fn guarded_password_update(
    target_user: &ryframe_db::entities::user::Model,
    password_hash: String,
    next_status: String,
    updated_at: chrono::DateTime<chrono::Utc>,
) -> sea_orm::UpdateMany<ryframe_db::entities::user::Entity> {
    ryframe_db::entities::user::Entity::update_many()
        .col_expr(
            ryframe_db::entities::user::Column::PasswordHash,
            Expr::value(password_hash),
        )
        .col_expr(
            ryframe_db::entities::user::Column::AuthorizationVersion,
            Expr::col(ryframe_db::entities::user::Column::AuthorizationVersion).add(1),
        )
        .col_expr(
            ryframe_db::entities::user::Column::Status,
            Expr::value(next_status),
        )
        .col_expr(
            ryframe_db::entities::user::Column::UpdatedAt,
            Expr::value(updated_at),
        )
        .filter(ryframe_db::entities::user::Column::Id.eq(target_user.id))
        .filter(ryframe_db::entities::user::Column::TenantId.eq(target_user.tenant_id.as_str()))
        .filter(
            ryframe_db::entities::user::Column::DelFlag
                .eq(ryframe_db::entities::user::Model::DEL_FLAG_NORMAL),
        )
        .filter(ryframe_db::entities::user::Column::Status.eq(target_user.status.as_str()))
        .filter(
            ryframe_db::entities::user::Column::AuthorizationVersion
                .eq(target_user.authorization_version),
        )
}
