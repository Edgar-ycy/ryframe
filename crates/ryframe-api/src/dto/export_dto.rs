use serde::Deserialize;
use utoipa::ToSchema;

use ryframe_application::system::{
    ConfigExportFilter, DictTypeExportFilter, ExportSelection, LoginLogExportFilter,
    OperLogExportFilter, PostExportFilter, RoleExportFilter, UserExportFilter,
};
use ryframe_kernel::{AppError, AppResult};

macro_rules! export_request_dto {
    ($request:ident, $filter:ident) => {
        #[derive(Debug, Deserialize, ToSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $request {
            pub filter: $filter,
            pub confirm_all: bool,
        }
    };
}

/// 用户导出的筛选条件。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UserExportFilterDto {
    pub username: Option<String>,
    pub phone: Option<String>,
    pub status: Option<String>,
    /// Snowflake ID 使用字符串传输，避免 JavaScript 精度丢失。
    pub dept_id: Option<String>,
}

/// 角色导出的筛选条件。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleExportFilterDto {
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

/// 岗位导出的筛选条件。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PostExportFilterDto {
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

/// 参数配置导出的筛选条件。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigExportFilterDto {
    pub name: Option<String>,
    pub key: Option<String>,
}

/// 字典类型导出的筛选条件。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DictTypeExportFilterDto {
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

/// 操作日志导出的筛选条件。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OperLogExportFilterDto {
    pub oper_name: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}

/// 登录日志导出的筛选条件。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginLogExportFilterDto {
    pub user_name: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}

export_request_dto!(UserExportRequestDto, UserExportFilterDto);
export_request_dto!(RoleExportRequestDto, RoleExportFilterDto);
export_request_dto!(PostExportRequestDto, PostExportFilterDto);
export_request_dto!(ConfigExportRequestDto, ConfigExportFilterDto);
export_request_dto!(DictTypeExportRequestDto, DictTypeExportFilterDto);
export_request_dto!(OperLogExportRequestDto, OperLogExportFilterDto);
export_request_dto!(LoginLogExportRequestDto, LoginLogExportFilterDto);

impl UserExportRequestDto {
    pub fn into_selection(self) -> AppResult<(ExportSelection, bool)> {
        let filter = self.filter;
        let dept_id = parse_optional_id(filter.dept_id, "部门ID")?;
        Ok((
            ExportSelection::Users(UserExportFilter::new(
                filter.username,
                filter.phone,
                filter.status,
                dept_id,
            )),
            self.confirm_all,
        ))
    }
}

impl RoleExportRequestDto {
    pub fn into_selection(self) -> (ExportSelection, bool) {
        let filter = self.filter;
        (
            ExportSelection::Roles(RoleExportFilter::new(
                filter.name,
                filter.code,
                filter.status,
            )),
            self.confirm_all,
        )
    }
}

impl PostExportRequestDto {
    pub fn into_selection(self) -> (ExportSelection, bool) {
        let filter = self.filter;
        (
            ExportSelection::Posts(PostExportFilter::new(
                filter.name,
                filter.code,
                filter.status,
            )),
            self.confirm_all,
        )
    }
}

impl ConfigExportRequestDto {
    pub fn into_selection(self) -> (ExportSelection, bool) {
        let filter = self.filter;
        (
            ExportSelection::Configs(ConfigExportFilter::new(filter.name, filter.key)),
            self.confirm_all,
        )
    }
}

impl DictTypeExportRequestDto {
    pub fn into_selection(self) -> (ExportSelection, bool) {
        let filter = self.filter;
        (
            ExportSelection::DictTypes(DictTypeExportFilter::new(
                filter.name,
                filter.code,
                filter.status,
            )),
            self.confirm_all,
        )
    }
}

impl OperLogExportRequestDto {
    pub fn into_selection(self) -> AppResult<(ExportSelection, bool)> {
        let filter = self.filter;
        Ok((
            ExportSelection::OperLogs(OperLogExportFilter::new(
                filter.oper_name,
                filter.status,
                filter.begin_time,
                filter.end_time,
            )?),
            self.confirm_all,
        ))
    }
}

impl LoginLogExportRequestDto {
    pub fn into_selection(self) -> AppResult<(ExportSelection, bool)> {
        let filter = self.filter;
        Ok((
            ExportSelection::LoginLogs(LoginLogExportFilter::new(
                filter.user_name,
                filter.status,
                filter.begin_time,
                filter.end_time,
            )?),
            self.confirm_all,
        ))
    }
}

fn parse_optional_id(value: Option<String>, label: &str) -> AppResult<Option<i64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i64>()
        .map(Some)
        .map_err(|_| AppError::Validation(format!("无效的{label}: {value}")))
}

/// 取消导出任务的显式命令体。
///
/// 保留空对象而不是省略请求体，使写操作契约保持一致，并为后续增加取消原因等字段预留空间。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelExportJobDto {}

/// 确认当前用户已经实际看到的导出完成或失败通知。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkExportNotificationsReadDto {
    #[schema(min_items = 1, max_items = 100, value_type = Vec<String>)]
    pub ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_export_requests_require_strict_envelope() {
        macro_rules! assert_contract {
            ($request:ty, $filter:expr) => {{
                let valid = serde_json::json!({"filter": $filter, "confirm_all": false});
                serde_json::from_value::<$request>(valid).expect("统一包络应可解析");

                let unknown = serde_json::json!({
                    "filter": $filter,
                    "confirm_all": false,
                    "legacy": true
                });
                assert!(serde_json::from_value::<$request>(unknown).is_err());

                let old_shape = $filter;
                assert!(serde_json::from_value::<$request>(old_shape).is_err());
            }};
        }

        assert_contract!(UserExportRequestDto, serde_json::json!({}));
        assert_contract!(RoleExportRequestDto, serde_json::json!({}));
        assert_contract!(PostExportRequestDto, serde_json::json!({}));
        assert_contract!(ConfigExportRequestDto, serde_json::json!({}));
        assert_contract!(DictTypeExportRequestDto, serde_json::json!({}));
        assert_contract!(OperLogExportRequestDto, serde_json::json!({}));
        assert_contract!(LoginLogExportRequestDto, serde_json::json!({}));
    }

    #[test]
    fn export_filters_reject_pagination_and_unknown_fields() {
        let page = serde_json::json!({
            "filter": {"name": "ops", "page": 2},
            "confirm_all": false
        });
        assert!(serde_json::from_value::<RoleExportRequestDto>(page).is_err());

        let page_size = serde_json::json!({
            "filter": {"name": "ops", "page_size": 100},
            "confirm_all": false
        });
        assert!(serde_json::from_value::<RoleExportRequestDto>(page_size).is_err());

        let wrong_log_field = serde_json::json!({
            "filter": {"name": "operator"},
            "confirm_all": false
        });
        assert!(serde_json::from_value::<OperLogExportRequestDto>(wrong_log_field).is_err());
    }

    #[test]
    fn mapping_preserves_zero_and_rejects_invalid_time_before_enqueue() {
        let user: UserExportRequestDto = serde_json::from_value(serde_json::json!({
            "filter": {"username": " ", "dept_id": "0", "status": "0"},
            "confirm_all": false
        }))
        .expect("用户请求应可解析");
        let (selection, _) = user.into_selection().expect("数值零应可映射");
        assert!(!selection.is_empty());

        let log: OperLogExportRequestDto = serde_json::from_value(serde_json::json!({
            "filter": {
                "oper_name": "operator",
                "begin_time": "2026-08-20T04:00:00Z",
                "end_time": "2026-08-20T03:00:00Z"
            },
            "confirm_all": false
        }))
        .expect("DTO 解析不应隐藏时间错误");
        assert!(matches!(log.into_selection(), Err(AppError::Validation(_))));
    }
}
