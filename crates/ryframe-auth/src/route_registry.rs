use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use regex::Regex;
use ryframe_common::AppError;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::{jwt::Claims, permission::check_permission};

/// 参数化路由条目（含 `{id}` 等路径参数，需正则匹配）
#[derive(Clone, Debug)]
struct ParameterizedEntry {
    /// 编译后的路径正则 (例: `^system/users/[^/]+$`)
    regex: Regex,
    /// HTTP 方法
    method: String,
    /// 需要的权限码
    perm_code: String,
}

/// 路由权限注册表
///
/// 启动时从 `sys_permission` 表加载所有 API 权限的 path→method→code 映射。
/// 分两层索引以优化大量权限时的查找性能：
///
/// - **精确匹配层** (`HashMap`): 不含 `{param}` 的路径 → O(1) 查找
/// - **参数化匹配层** (`Vec`): 含 `{param}` 的路径 → 正则扫描（数量通常很少）
///
/// 在数千条权限下，绝大多数请求命中 O(1) 精确层，
/// 仅少数参数化路由（如 `/users/{id}`）走正则扫描。
#[derive(Clone)]
pub struct PermissionRouteRegistry {
    /// 精确路径匹配: (path, method) → perm_code
    exact: HashMap<(String, String), String>,
    /// 参数化路径匹配（含 `{param}` 的动态路径）
    parameterized: Vec<ParameterizedEntry>,
}

impl PermissionRouteRegistry {
    /// 创建一个空的注册表（用于测试，无任何权限路由映射）
    pub fn empty() -> Self {
        Self {
            exact: HashMap::new(),
            parameterized: Vec::new(),
        }
    }

    /// 从数据库加载权限路由映射表
    ///
    /// 只加载 `perm_type='api'` 且 `path` 和 `http_method` 均不为 NULL 的记录，
    /// 将路径中的 `{param}` 模式转为正则 `[^/]+`。
    pub async fn load_from_db(db: &DatabaseConnection) -> Result<Self, AppError> {
        use ryframe_db::entities::permission::{Column, Entity};

        let perms = Entity::find()
            .filter(Column::PermType.eq("api"))
            .filter(Column::Path.is_not_null())
            .filter(Column::HttpMethod.is_not_null())
            .filter(Column::Status.eq("1"))
            .all(db)
            .await
            .map_err(|e| AppError::Internal(format!("加载权限路由表失败: {}", e)))?;

        let mut exact: HashMap<(String, String), String> = HashMap::new();
        let mut parameterized = Vec::new();
        let has_param = Regex::new(r"\{[^}]+\}").unwrap();

        for p in perms {
            if let (Some(path), Some(method)) = (p.path, p.http_method) {
                let method = method.to_uppercase();
                if has_param.is_match(&path) {
                    // 参数化路径：编译为正则
                    let regex_str = Self::path_to_regex(&path);
                    let regex = Regex::new(&regex_str).map_err(|e| {
                        AppError::Internal(format!(
                            "权限路由正则编译失败 [{}] path={}: {}",
                            p.code, path, e
                        ))
                    })?;
                    parameterized.push(ParameterizedEntry {
                        regex,
                        method,
                        perm_code: p.code,
                    });
                } else {
                    // 精确路径：直接 HashMap 索引
                    exact.insert((path, method), p.code);
                }
            }
        }

        tracing::info!(
            "权限路由注册表加载完成: {} 精确匹配 + {} 参数化匹配",
            exact.len(),
            parameterized.len()
        );

        Ok(Self {
            exact,
            parameterized,
        })
    }

    /// 将路由路径模式转为正则表达式
    ///
    /// 例: `system/users/{id}` → `^system/users/[^/]+$`
    ///     `system/users/export` → `^system/users/export$`
    fn path_to_regex(path: &str) -> String {
        // 将 Axum 风格的 {param} 替换为 [^/]+
        let re = Regex::new(r"\{[^}]+\}").unwrap();
        let pattern = re.replace_all(path, r"[^/]+");
        // 如果原本没有路径参数，使用精确匹配；否则已替换为通配
        format!("^{}$", pattern)
    }

    /// 根据请求路径和方法查找需要的权限码
    ///
    /// 查找策略：
    /// 1. 先查精确匹配 HashMap → O(1)，覆盖 90%+ 的请求
    /// 2. 未命中则正则扫描参数化路由 → O(k)，k 通常 < 10
    ///
    /// 返回 `Some(perm_code)` 表示该路由需要权限，`None` 表示无需权限（公开访问）。
    pub fn find_permission(&self, relative_path: &str, method: &str) -> Option<&str> {
        let method_upper = method.to_uppercase();

        // 第 1 层：精确匹配 O(1)
        if let Some(perm) = self
            .exact
            .get(&(relative_path.to_string(), method_upper.clone()))
        {
            return Some(perm.as_str());
        }

        // 第 2 层：参数化路径正则扫描（数量极少，几十条以内）
        self.parameterized
            .iter()
            .filter(|e| e.method == method_upper)
            .filter(|e| e.regex.is_match(relative_path))
            .max_by_key(|e| e.regex.as_str().len())
            .map(|e| e.perm_code.as_str())
    }
}

/// 动态权限校验中间件
///
/// 从 `PermissionRouteRegistry` 查找当前请求路径 + 方法对应的权限码，
/// 若找到则校验用户 JWT Claims 中是否包含该权限。
///
/// 应在 `auth_middleware` 之后注册（依赖 Claims 在 extensions 中）。
pub async fn dynamic_permission_middleware(
    State(registry): State<Arc<PermissionRouteRegistry>>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    let path = request.uri().path();
    let method = request.method().as_str();

    // 去掉 /api/v1/ 前缀，得到相对路径
    let relative = path
        .strip_prefix("/api/v1/")
        .unwrap_or(path)
        .trim_start_matches('/');

    let required_perm = registry.find_permission(relative, method);

    match required_perm {
        Some(perm_code) => {
            let claims = request.extensions().get::<Claims>().ok_or_else(|| {
                AppError::Authentication("未认证，请先登录".into()).into_response()
            })?;

            check_permission(claims, perm_code).map_err(|e| e.into_response())?;
            Ok(next.run(request).await)
        }
        None => {
            // 路由未在权限表中配置 → 放行
            Ok(next.run(request).await)
        }
    }
}
