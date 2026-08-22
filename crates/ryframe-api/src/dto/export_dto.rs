use serde::{Deserialize, Serialize};
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

/// 单删与批删共用的导出记录删除命令。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteExportJobsDto {
    #[schema(min_items = 1, max_items = 100, value_type = Vec<String>)]
    pub ids: Vec<String>,
}

impl DeleteExportJobsDto {
    pub fn into_ids(self) -> AppResult<Vec<i64>> {
        self.ids
            .into_iter()
            .map(|id| {
                id.parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or_else(|| AppError::Validation("导出任务 ID 必须是正整数".into()))
            })
            .collect()
    }
}

/// 服务端已受理的导出记录删除结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct ExportDeletionAcceptedDto {
    #[schema(value_type = Vec<String>)]
    pub accepted_ids: Vec<String>,
    pub accepted_count: u64,
    pub removed_unread_count: u64,
}

impl From<ryframe_application::system::ExportDeletionResult> for ExportDeletionAcceptedDto {
    fn from(result: ryframe_application::system::ExportDeletionResult) -> Self {
        Self {
            accepted_ids: result
                .accepted_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            accepted_count: result.accepted_count,
            removed_unread_count: result.removed_unread_count,
        }
    }
}
