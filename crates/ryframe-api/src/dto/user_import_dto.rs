use serde::{Deserialize, Serialize};
use validator::Validate;

/// 用户导入数据
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct UserImportData {
    #[validate(length(min = 2, max = 64, message = "用户名长度为2-64个字符"))]
    pub username: String,

    #[validate(length(min = 1, max = 64, message = "昵称为1-64个字符"))]
    pub nickname: String,

    #[validate(email(message = "邮箱格式不正确"))]
    pub email: String,

    #[validate(length(max = 32, message = "手机号最多32个字符"))]
    pub phone: Option<String>,

    pub sex: Option<String>,
    pub dept_id: Option<String>,
    pub status: Option<String>,
    pub remark: Option<String>,
}

impl UserImportData {
    /// Excel 表头定义
    pub fn excel_headers() -> &'static [(&'static str, &'static str)] {
        &[
            ("username", "用户名"),
            ("nickname", "昵称"),
            ("email", "邮箱"),
            ("phone", "手机号"),
            ("sex", "性别"),
            ("dept_id", "部门ID"),
            ("status", "状态"),
            ("remark", "备注"),
        ]
    }
}

/// 用户导出数据
#[derive(Debug, Clone, Serialize)]
pub struct UserExportData {
    pub user_id: String,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub sex: String,
    pub dept_name: Option<String>,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: String,
}

impl UserExportData {
    /// Excel 表头定义
    pub fn excel_headers() -> &'static [(&'static str, &'static str)] {
        &[
            ("user_id", "用户ID"),
            ("username", "用户名"),
            ("nickname", "昵称"),
            ("email", "邮箱"),
            ("phone", "手机号"),
            ("sex", "性别"),
            ("dept_name", "部门"),
            ("status", "状态"),
            ("remark", "备注"),
            ("created_at", "创建时间"),
        ]
    }
}
