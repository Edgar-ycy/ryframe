/// 用户导入模板和 Worker 共同使用的行结构。
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct UserImportData {
    #[serde(alias = "用户名")]
    #[validate(length(min = 2, max = 64, message = "用户名长度必须为 2-64 个字符"))]
    pub username: String,
    #[serde(alias = "昵称")]
    #[validate(length(min = 1, max = 64, message = "昵称长度必须为 1-64 个字符"))]
    pub nickname: String,
    #[serde(alias = "邮箱")]
    #[validate(email(message = "邮箱格式不正确"))]
    pub email: String,
    #[serde(alias = "手机号")]
    #[validate(length(max = 32, message = "手机号最多 32 个字符"))]
    pub phone: Option<String>,
    #[serde(alias = "部门完整路径")]
    pub department_path: Option<String>,
}

pub(super) struct ParsedImportRow<T> {
    pub(super) row_number: usize,
    pub(super) value: Result<T, String>,
}

impl UserImportData {
    pub const fn excel_headers() -> &'static [(&'static str, &'static str)] {
        &[
            ("username", "用户名"),
            ("nickname", "昵称"),
            ("email", "邮箱"),
            ("phone", "手机号"),
            ("department_path", "部门完整路径"),
        ]
    }
}

/// 面向管理端的异步导入任务安全视图。
#[derive(Clone, Debug, Serialize)]
pub struct UserImportJobVo {
    pub id: String,
    pub source_name: String,
    pub requester_username: Option<String>,
    pub duplicate_policy: String,
    pub status: String,
    pub total_rows: i32,
    pub processed_rows: i32,
    pub success_count: i32,
    pub skipped_count: i32,
    pub failure_count: i32,
    pub cancel_requested: bool,
    pub report_available: bool,
    pub last_error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<UserImportJobRecord> for UserImportJobVo {
    fn from(job: UserImportJobRecord) -> Self {
        Self {
            id: job.id.to_string(),
            source_name: job.source_name_snapshot,
            requester_username: None,
            duplicate_policy: job.duplicate_policy,
            status: job.status,
            total_rows: job.total_rows,
            processed_rows: job.processed_rows,
            success_count: job.success_count,
            skipped_count: job.skipped_count,
            failure_count: job.failure_count,
            cancel_requested: job.cancel_requested,
            report_available: job.error_report_file_id.is_some(),
            last_error: job.last_error,
            started_at: job.started_at,
            completed_at: job.completed_at,
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

pub(super) fn job_vo_with_requester<T>(
    job: T,
    requester_username: Option<String>,
) -> UserImportJobVo
where
    T: Into<UserImportJobVo>,
{
    let mut view = job.into();
    view.requester_username = requester_username;
    view
}

/// 面向管理端的导入异常行安全视图。
#[derive(Clone, Debug, Serialize)]
pub struct UserImportRowVo {
    pub row_number: i32,
    pub username: String,
    pub outcome: String,
    pub code: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

impl From<UserImportRowRecord> for UserImportRowVo {
    fn from(row: UserImportRowRecord) -> Self {
        Self {
            row_number: row.row_number,
            username: row.username,
            outcome: row.outcome,
            code: row.code,
            message: row.message,
            created_at: row.created_at,
        }
    }
}

/// 幂等创建导入任务的结果。
pub struct RequestUserImportOutcome {
    pub job: UserImportJobVo,
    pub inserted: bool,
}

/// 创建异步导入任务的受控输入。
pub struct RequestUserImportCommand {
    pub idempotency_key_hash: String,
    pub source_file_id: i64,
    pub source_name: String,
    pub source_sha256: String,
}

/// 用户导入列表查询。
pub struct UserImportListParams {
    pub page: ValidatedPageQuery,
    pub status: Option<String>,
}
