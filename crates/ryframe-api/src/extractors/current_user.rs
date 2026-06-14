use axum::{extract::FromRequestParts, http::request::Parts};
use ryframe_auth::jwt::Claims;
use ryframe_common::{
    AppError,
    annotations::data_scope::{DataScope, DataScopeContext},
};

/// 当前登录用户上下文
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user_id: i64,
    pub username: String,
    pub roles: Vec<String>,
    /// 角色ID列表（用于菜单/权限过滤查询）
    pub role_ids: Vec<i64>,
    pub permissions: Vec<String>,
    pub dept_id: Option<i64>,
    /// 部门祖级路径（ancestors）
    pub dept_path: Option<String>,
    pub data_scope: DataScope,
    /// 自定义数据权限的部门ID列表
    pub custom_dept_ids: Vec<i64>,
    pub is_super_admin: bool,
}

impl CurrentUser {
    /// 构建 DataScopeContext（用于 Service 层数据过滤）
    pub fn to_data_scope_context(&self) -> DataScopeContext {
        DataScopeContext {
            scope: self.data_scope.clone(),
            user_id: self.user_id,
            dept_id: self.dept_id,
            ancestors: self.dept_path.clone(),
            custom_dept_ids: self.custom_dept_ids.clone(),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, AppError> {
        // 取 Claims（认证中间件已注入）
        let _claims = parts
            .extensions
            .get::<Claims>()
            .ok_or_else(|| AppError::Authentication("未认证".into()))?;

        // 取已注入的 CurrentUser 缓存（认证中间件预查时缓存）
        if let Some(cached) = parts.extensions.get::<CurrentUser>() {
            return Ok(cached.clone());
        }

        Err(AppError::Internal(
            "CurrentUser 未注入到 request extensions".into(),
        ))
    }
}
