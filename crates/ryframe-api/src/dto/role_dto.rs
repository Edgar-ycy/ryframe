use crate::{
    dto::option_dto::{OptionQuery, ResolvedOptionQuery},
    http::HttpResult,
};
use ryframe_application::system::RoleOptionPurpose;
use ryframe_config::PaginationConfig;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

/// 角色选项的使用场景。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoleOptionPurposeDto {
    UserAssignment,
    ServiceAccountAssignment,
}

impl From<RoleOptionPurposeDto> for RoleOptionPurpose {
    fn from(value: RoleOptionPurposeDto) -> Self {
        match value {
            RoleOptionPurposeDto::UserAssignment => Self::UserAssignment,
            RoleOptionPurposeDto::ServiceAccountAssignment => Self::ServiceAccountAssignment,
        }
    }
}

/// 角色选项查询参数；用途必须由调用方明确指定。
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct RoleOptionQuery {
    /// 角色选项的使用场景。
    pub purpose: RoleOptionPurposeDto,
    /// 按名称或稳定编码做前缀搜索；首尾空白会被移除。
    #[param(max_length = 64)]
    pub q: Option<String>,
    /// 返回上限；省略时使用服务端默认分页大小。
    #[param(minimum = 1)]
    pub limit: Option<u64>,
}

pub struct ResolvedRoleOptionQuery {
    pub purpose: RoleOptionPurpose,
    pub q: Option<String>,
    pub limit: u64,
}

impl RoleOptionQuery {
    pub fn resolve(self, policy: &PaginationConfig) -> HttpResult<ResolvedRoleOptionQuery> {
        let purpose = self.purpose.into();
        let ResolvedOptionQuery { q, limit } = OptionQuery {
            q: self.q,
            limit: self.limit,
        }
        .resolve(policy)?;
        Ok(ResolvedRoleOptionQuery { purpose, q, limit })
    }
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRoleDto {
    #[validate(length(min = 1, max = 50, message = "角色名称长度1-50"))]
    pub name: String,
    #[validate(length(min = 1, max = 50, message = "角色编码长度1-50"))]
    pub code: String,
    pub sort: Option<i32>,
    /// 数据范围: "1"全部 "2"自定义 "3"本部门 "4"本部门及以下 "5"仅本人
    pub data_scope: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRoleDto {
    #[validate(length(min = 1, message = "角色名称不能为空"))]
    pub name: String,
    pub sort: Option<i32>,
    pub status: String,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceRolePermissionsDto {
    #[serde(default)]
    pub perm_ids: Vec<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceRoleDataScopeDto {
    #[validate(custom(function = "validate_data_scope"))]
    pub data_scope: String,
    #[serde(default)]
    pub dept_ids: Vec<String>,
}

fn validate_data_scope(value: &str) -> Result<(), validator::ValidationError> {
    if matches!(value, "1" | "2" | "3" | "4" | "5") {
        Ok(())
    } else {
        Err(validator::ValidationError::new("invalid_data_scope"))
    }
}

#[cfg(test)]
mod tests {
    use axum::{extract::Query, http::Uri};
    use ryframe_application::system::RoleOptionPurpose;
    use ryframe_config::PaginationConfig;

    use super::{RoleOptionPurposeDto, RoleOptionQuery};

    fn parse_query(uri: &str) -> Result<RoleOptionQuery, axum::extract::rejection::QueryRejection> {
        let uri = uri.parse::<Uri>().expect("测试 URI 必须有效");
        Query::<RoleOptionQuery>::try_from_uri(&uri).map(|Query(query)| query)
    }

    #[test]
    fn role_option_query_accepts_both_explicit_purposes() {
        let user = parse_query("/?purpose=user_assignment").expect("用户分配用途应可解析");
        assert_eq!(user.purpose, RoleOptionPurposeDto::UserAssignment);
        assert_eq!(
            user.resolve(&PaginationConfig::default())
                .expect("用户分配用途应可转换")
                .purpose,
            RoleOptionPurpose::UserAssignment
        );

        let service =
            parse_query("/?purpose=service_account_assignment").expect("服务账号分配用途应可解析");
        assert_eq!(
            service.purpose,
            RoleOptionPurposeDto::ServiceAccountAssignment
        );
        assert_eq!(
            service
                .resolve(&PaginationConfig::default())
                .expect("服务账号分配用途应可转换")
                .purpose,
            RoleOptionPurpose::ServiceAccountAssignment
        );
    }

    #[test]
    fn role_option_query_rejects_missing_invalid_and_unknown_fields() {
        assert!(parse_query("/?q=admin").is_err());
        assert!(parse_query("/?purpose=role_code").is_err());
        assert!(parse_query("/?purpose=user_assignment&unexpected=true").is_err());
    }
}
