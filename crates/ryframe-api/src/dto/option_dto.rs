use crate::http::HttpResult;
use ryframe_kernel::{AppError, PaginationPolicy};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

pub const MAX_OPTION_QUERY_CHARS: usize = 64;

/// 角色和用户选择器共用的查询参数。
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct OptionQuery {
    /// 按名称或稳定编码做前缀搜索；首尾空白会被移除。
    #[param(max_length = 64)]
    pub q: Option<String>,
    /// 返回上限；省略时使用服务端默认分页大小。
    #[param(minimum = 1)]
    pub limit: Option<u64>,
}

pub struct ResolvedOptionQuery {
    pub q: Option<String>,
    pub limit: u64,
}

impl OptionQuery {
    pub fn resolve(self, policy: PaginationPolicy) -> HttpResult<ResolvedOptionQuery> {
        policy.validate()?;
        let q = self
            .q
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if q.as_ref()
            .is_some_and(|value| value.chars().count() > MAX_OPTION_QUERY_CHARS)
        {
            return Err(AppError::Validation(format!(
                "q 最多包含 {MAX_OPTION_QUERY_CHARS} 个字符"
            ))
            .into());
        }
        let limit = self.limit.unwrap_or(policy.default_page_size());
        if limit == 0 || limit > policy.max_page_size() {
            return Err(AppError::Validation(format!(
                "limit 必须在 1 到 {} 之间",
                policy.max_page_size()
            ))
            .into());
        }
        Ok(ResolvedOptionQuery { q, limit })
    }
}
