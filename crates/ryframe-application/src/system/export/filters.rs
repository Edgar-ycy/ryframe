use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use super::{EXPORT_STATUS_SUCCEEDED, RequestExportCommand};

/// 用户导出的规范化筛选条件。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UserExportFilter {
    username: Option<String>,
    phone: Option<String>,
    status: Option<String>,
    dept_id: Option<i64>,
}

impl UserExportFilter {
    pub fn new(
        username: Option<String>,
        phone: Option<String>,
        status: Option<String>,
        dept_id: Option<i64>,
    ) -> Self {
        Self {
            username: normalize_optional_text(username),
            phone: normalize_optional_text(phone),
            status: normalize_optional_text(status),
            dept_id,
        }
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub fn phone(&self) -> Option<&str> {
        self.phone.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub const fn dept_id(&self) -> Option<i64> {
        self.dept_id
    }

    pub const fn is_empty(&self) -> bool {
        self.username.is_none()
            && self.phone.is_none()
            && self.status.is_none()
            && self.dept_id.is_none()
    }
}

macro_rules! text_export_filter {
    ($name:ident { $($field:ident),+ $(,)? }) => {
        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(
                $field: Option<String>,
            )+
        }

        impl $name {
            pub fn new($($field: Option<String>),+) -> Self {
                Self {
                    $($field: normalize_optional_text($field)),+
                }
            }

            $(
                pub fn $field(&self) -> Option<&str> {
                    self.$field.as_deref()
                }
            )+

            pub const fn is_empty(&self) -> bool {
                $(self.$field.is_none())&&+
            }
        }
    };
}

text_export_filter!(RoleExportFilter { name, code, status });
text_export_filter!(PostExportFilter { name, code, status });
text_export_filter!(ConfigExportFilter { name, key });
text_export_filter!(DictTypeExportFilter { name, code, status });

/// 操作日志导出的规范化筛选条件。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperLogExportFilter {
    oper_name: Option<String>,
    status: Option<String>,
    begin_time: Option<DateTime<Utc>>,
    end_time: Option<DateTime<Utc>>,
}

impl OperLogExportFilter {
    pub fn new(
        oper_name: Option<String>,
        status: Option<String>,
        begin_time: Option<String>,
        end_time: Option<String>,
    ) -> AppResult<Self> {
        let (begin_time, end_time) = parse_export_time_range(begin_time, end_time)?;
        Ok(Self {
            oper_name: normalize_optional_text(oper_name),
            status: normalize_optional_text(status),
            begin_time,
            end_time,
        })
    }

    pub fn oper_name(&self) -> Option<&str> {
        self.oper_name.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub const fn begin_time(&self) -> Option<DateTime<Utc>> {
        self.begin_time
    }

    pub const fn end_time(&self) -> Option<DateTime<Utc>> {
        self.end_time
    }

    const fn is_empty(&self) -> bool {
        self.oper_name.is_none()
            && self.status.is_none()
            && self.begin_time.is_none()
            && self.end_time.is_none()
    }
}

/// 登录日志导出的规范化筛选条件。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoginLogExportFilter {
    user_name: Option<String>,
    status: Option<String>,
    begin_time: Option<DateTime<Utc>>,
    end_time: Option<DateTime<Utc>>,
}

impl LoginLogExportFilter {
    pub fn new(
        user_name: Option<String>,
        status: Option<String>,
        begin_time: Option<String>,
        end_time: Option<String>,
    ) -> AppResult<Self> {
        let (begin_time, end_time) = parse_export_time_range(begin_time, end_time)?;
        Ok(Self {
            user_name: normalize_optional_text(user_name),
            status: normalize_optional_text(status),
            begin_time,
            end_time,
        })
    }

    pub fn user_name(&self) -> Option<&str> {
        self.user_name.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub const fn begin_time(&self) -> Option<DateTime<Utc>> {
        self.begin_time
    }

    pub const fn end_time(&self) -> Option<DateTime<Utc>> {
        self.end_time
    }

    const fn is_empty(&self) -> bool {
        self.user_name.is_none()
            && self.status.is_none()
            && self.begin_time.is_none()
            && self.end_time.is_none()
    }
}

/// Worker 可严格解析的资源选择集合。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "resource", content = "filter", deny_unknown_fields)]
pub enum ExportSelection {
    #[serde(rename = "users")]
    Users(UserExportFilter),
    #[serde(rename = "roles")]
    Roles(RoleExportFilter),
    #[serde(rename = "posts")]
    Posts(PostExportFilter),
    #[serde(rename = "configs")]
    Configs(ConfigExportFilter),
    #[serde(rename = "dict-types")]
    DictTypes(DictTypeExportFilter),
    #[serde(rename = "operlogs")]
    OperLogs(OperLogExportFilter),
    #[serde(rename = "loginlogs")]
    LoginLogs(LoginLogExportFilter),
}

impl ExportSelection {
    pub const fn resource(&self) -> &'static str {
        match self {
            Self::Users(_) => "users",
            Self::Roles(_) => "roles",
            Self::Posts(_) => "posts",
            Self::Configs(_) => "configs",
            Self::DictTypes(_) => "dict-types",
            Self::OperLogs(_) => "operlogs",
            Self::LoginLogs(_) => "loginlogs",
        }
    }

    pub const fn is_empty(&self) -> bool {
        match self {
            Self::Users(filter) => filter.is_empty(),
            Self::Roles(filter) => filter.is_empty(),
            Self::Posts(filter) => filter.is_empty(),
            Self::Configs(filter) => filter.is_empty(),
            Self::DictTypes(filter) => filter.is_empty(),
            Self::OperLogs(filter) => filter.is_empty(),
            Self::LoginLogs(filter) => filter.is_empty(),
        }
    }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else if trimmed.len() == value.len() {
        Some(value)
    } else {
        Some(trimmed.to_owned())
    }
}

fn parse_export_time_range(
    begin_time: Option<String>,
    end_time: Option<String>,
) -> AppResult<crate::system::log_time_range::ParsedLogTimeRange> {
    crate::system::log_time_range::parse_log_time_range(begin_time.as_deref(), end_time.as_deref())
}

pub(super) const ROLE_HEADERS: &[(&str, &str)] = &[
    ("role_id", "角色 ID"),
    ("role_name", "角色名称"),
    ("role_code", "角色编码"),
    ("data_scope", "数据范围"),
    ("status", "状态"),
    ("sort", "排序"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];
pub(super) const POST_HEADERS: &[(&str, &str)] = &[
    ("post_id", "岗位 ID"),
    ("name", "岗位名称"),
    ("code", "岗位编码"),
    ("sort", "排序"),
    ("status", "状态"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];
pub(super) const CONFIG_HEADERS: &[(&str, &str)] = &[
    ("name", "参数名称"),
    ("key", "参数键名"),
    ("value", "参数键值"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];
pub(super) const DICT_TYPE_HEADERS: &[(&str, &str)] = &[
    ("name", "字典名称"),
    ("code", "字典类型"),
    ("status", "状态"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];
pub(super) const OPER_LOG_HEADERS: &[(&str, &str)] = &[
    ("title", "操作模块"),
    ("business_type", "业务类型"),
    ("oper_name", "操作人员"),
    ("oper_url", "请求地址"),
    ("oper_ip", "操作 IP"),
    ("status", "状态"),
    ("cost_time", "耗时(ms)"),
    ("oper_time", "操作时间"),
];
pub(super) const LOGIN_LOG_HEADERS: &[(&str, &str)] = &[
    ("user_name", "用户名"),
    ("ipaddr", "IP 地址"),
    ("login_location", "登录地点"),
    ("browser", "浏览器"),
    ("os", "操作系统"),
    ("status", "状态"),
    ("msg", "提示消息"),
    ("login_time", "登录时间"),
];

pub(super) const USER_HEADERS: &[(&str, &str)] = &[
    ("user_id", "用户 ID"),
    ("username", "用户名"),
    ("nickname", "昵称"),
    ("email", "邮箱"),
    ("phone", "手机号"),
    ("dept_name", "部门"),
    ("status", "状态"),
    ("remark", "备注"),
    ("created_at", "创建时间"),
];

pub fn validate_request_command(command: &RequestExportCommand) -> AppResult<()> {
    let permission_code = command.permission_code.as_str();
    if permission_code.trim().is_empty() || permission_code.len() > 128 {
        return Err(AppError::Validation(
            "导出请求 permission_code 长度必须介于 1 和 128 之间".into(),
        ));
    }
    if command.selection.is_empty() && !command.confirm_all {
        return Err(AppError::ExportAllConfirmationRequired(
            "导出全部匹配数据前必须二次确认".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_job_id(id: i64) -> AppResult<()> {
    if id <= 0 {
        return Err(AppError::Validation("导出任务 ID 必须是正整数".into()));
    }
    Ok(())
}

pub(super) fn export_file_location(
    tenant_id: &str,
    resource: &str,
    export_id: i64,
) -> (String, String) {
    let file_name = format!("{resource}-{export_id}.xlsx");
    let key = format!("{tenant_id}/exports/{file_name}");
    (file_name, key)
}

pub(super) const fn deterministic_export_file_id(export_id: i64) -> i64 {
    export_id
}

pub fn ensure_download_authorization_matches(
    stored_fingerprint: &str,
    current_fingerprint: &str,
) -> AppResult<()> {
    if !stored_fingerprint.is_empty() && stored_fingerprint == current_fingerprint {
        Ok(())
    } else {
        Err(AppError::Authorization(
            "导出申请后的授权或数据范围已变化，请重新创建导出任务".into(),
        ))
    }
}

pub fn should_delete_uncommitted_object(status: &str) -> bool {
    status != EXPORT_STATUS_SUCCEEDED
}
