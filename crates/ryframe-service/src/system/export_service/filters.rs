use ryframe_db::entities::export_job;
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::RequestExportCommand;

/// 用户导出的可持久化筛选条件。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserExportFilters {
    pub username: Option<String>,
    pub phone: Option<String>,
    pub status: Option<String>,
    pub dept_id: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct RoleExportFilters {
    pub(super) name: Option<String>,
    pub(super) code: Option<String>,
    pub(super) status: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct PostExportFilters {
    pub(super) name: Option<String>,
    pub(super) code: Option<String>,
    pub(super) status: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ConfigExportFilters {
    pub(super) name: Option<String>,
    pub(super) key: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct DictTypeExportFilters {
    pub(super) name: Option<String>,
    pub(super) code: Option<String>,
    pub(super) status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LogExportFilters {
    pub(super) name: Option<String>,
    pub(super) status: Option<String>,
    pub(super) begin_time: Option<String>,
    pub(super) end_time: Option<String>,
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

pub(super) fn decode_export_filters<T: serde::de::DeserializeOwned>(
    request: Value,
    resource: &str,
) -> AppResult<T> {
    serde_json::from_value(request)
        .map_err(|error| AppError::Validation(format!("{resource} 导出筛选条件无效: {error}")))
}

#[derive(Serialize)]
pub(super) struct UserExportRow {
    pub(super) user_id: String,
    pub(super) username: String,
    pub(super) nickname: String,
    pub(super) email: String,
    pub(super) phone: String,
    pub(super) dept_name: Option<String>,
    pub(super) status: String,
    pub(super) remark: Option<String>,
    pub(super) created_at: String,
}

impl UserExportRow {
    pub(super) const fn headers() -> &'static [(&'static str, &'static str)] {
        &[
            ("user_id", "用户 ID"),
            ("username", "用户名"),
            ("nickname", "昵称"),
            ("email", "邮箱"),
            ("phone", "手机号"),
            ("dept_name", "部门"),
            ("status", "状态"),
            ("remark", "备注"),
            ("created_at", "创建时间"),
        ]
    }
}

pub(super) fn validate_request_command(command: &RequestExportCommand) -> AppResult<()> {
    for (name, value, maximum) in [
        ("resource", command.resource.as_str(), 64),
        ("permission_code", command.permission_code.as_str(), 128),
    ] {
        if value.trim().is_empty() || value.len() > maximum {
            return Err(AppError::Validation(format!(
                "导出请求 {name} 长度必须介于 1 和 {maximum} 之间"
            )));
        }
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

pub(super) fn ensure_download_authorization_matches(
    stored_fingerprint: Option<&str>,
    current_fingerprint: &str,
) -> AppResult<()> {
    if stored_fingerprint.is_some_and(|stored| stored == current_fingerprint) {
        Ok(())
    } else {
        Err(AppError::Authorization(
            "导出完成后的授权或数据范围已变化，请重新创建导出任务".into(),
        ))
    }
}

pub(super) fn should_delete_uncommitted_object(status: &str) -> bool {
    status != export_job::Model::STATUS_SUCCEEDED
}
