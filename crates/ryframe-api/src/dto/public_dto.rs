use chrono::{DateTime, Utc};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use ryframe_http::api_path;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use ryframe_service::system::generator_service::{
    ColumnInfo as ServiceColumnInfo, GeneratedFile as ServiceGeneratedFile,
    TableInfo as ServiceTableInfo, WriteReport as ServiceWriteReport,
};
use ryframe_service::system::profile_service::UserProfileResponse as ServiceUserProfileResponse;
use ryframe_service::system::{
    ConfigVo as ServiceConfigVo, DeptTreeNode as ServiceDeptTreeNode, DeptVo as ServiceDeptVo,
    DictDataVo as ServiceDictDataVo, DictTypeVo as ServiceDictTypeVo,
    ExportJobVo as ServiceExportJobVo, LoginInfoVo as ServiceLoginInfoVo,
    MenuTreeNode as ServiceMenuTreeNode, MenuType as ServiceMenuType, MenuVo as ServiceMenuVo,
    NoticeVo as ServiceNoticeVo, OnlineUserVo as ServiceOnlineUserVo,
    OperLogVo as ServiceOperLogVo, OptionItem as ServiceOptionItem,
    OptionList as ServiceOptionList, PermissionSyncReport as ServicePermissionSyncReport,
    PermissionTreeNode as ServicePermissionTreeNode, PermissionType as ServicePermissionType,
    PermissionVo as ServicePermissionVo, PostVo as ServicePostVo,
    RoleBriefVo as ServiceRoleBriefVo, RoleVo as ServiceRoleVo, TenantVo as ServiceTenantVo,
    UploadResponse as ServiceUploadResponse, UserDetailVo as ServiceUserDetailVo,
    UserVo as ServiceUserVo,
};
use ryframe_service::{
    BackgroundJobQueueStats as ServiceBackgroundJobQueueStats,
    BackgroundJobVo as ServiceBackgroundJobVo, UserInfo as ServiceUserInfo,
};

/// 当前登录用户的公开信息。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UserInfo {
    pub id: String,
    pub tenant_id: String,
    pub tenant_name: String,
    pub dept_name: Option<String>,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub preferred_locale: Option<String>,
    pub roles: Vec<String>,
    pub perms: Vec<String>,
}

impl From<ServiceUserInfo> for UserInfo {
    fn from(value: ServiceUserInfo) -> Self {
        let ServiceUserInfo {
            id,
            tenant_id,
            tenant_name,
            dept_name,
            username,
            nickname,
            email,
            phone,
            avatar,
            preferred_locale,
            roles,
            perms,
        } = value;
        Self {
            id,
            tenant_id,
            tenant_name,
            dept_name,
            username,
            nickname,
            email,
            phone,
            avatar,
            preferred_locale,
            roles,
            perms,
        }
    }
}

/// 后台任务的公开视图，不包含内部载荷。
#[derive(Debug, Serialize, ToSchema)]
pub struct BackgroundJobVo {
    pub id: String,
    pub job_type: String,
    pub status: String,
    pub priority: i32,
    pub available_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub dedupe_key: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl From<ServiceBackgroundJobVo> for BackgroundJobVo {
    fn from(value: ServiceBackgroundJobVo) -> Self {
        let ServiceBackgroundJobVo {
            id,
            job_type,
            status,
            priority,
            available_at,
            attempts,
            max_attempts,
            lease_owner,
            lease_until,
            dedupe_key,
            last_error,
            created_at,
            updated_at,
            completed_at,
        } = value;
        Self {
            id,
            job_type,
            status,
            priority,
            available_at,
            attempts,
            max_attempts,
            lease_owner,
            lease_until,
            dedupe_key,
            last_error,
            created_at,
            updated_at,
            completed_at,
        }
    }
}

/// 后台任务队列统计。
#[derive(Debug, Serialize, ToSchema)]
pub struct BackgroundJobQueueStats {
    pub total: u64,
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub dead: u64,
    pub ready: u64,
}

impl From<ServiceBackgroundJobQueueStats> for BackgroundJobQueueStats {
    fn from(value: ServiceBackgroundJobQueueStats) -> Self {
        let ServiceBackgroundJobQueueStats {
            total,
            pending,
            running,
            succeeded,
            dead,
            ready,
        } = value;
        Self {
            total,
            pending,
            running,
            succeeded,
            dead,
            ready,
        }
    }
}

/// 参数配置响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigVo {
    pub id: String,
    pub name: String,
    pub key: String,
    pub value: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceConfigVo> for ConfigVo {
    fn from(value: ServiceConfigVo) -> Self {
        let ServiceConfigVo {
            id,
            name,
            key,
            value,
            remark,
            created_at,
        } = value;
        Self {
            id,
            name,
            key,
            value,
            remark,
            created_at,
        }
    }
}

/// 字典类型响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct DictTypeVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceDictTypeVo> for DictTypeVo {
    fn from(value: ServiceDictTypeVo) -> Self {
        let ServiceDictTypeVo {
            id,
            name,
            code,
            status,
            remark,
            created_at,
        } = value;
        Self {
            id,
            name,
            code,
            status,
            remark,
            created_at,
        }
    }
}

/// 字典数据响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct DictDataVo {
    pub id: String,
    pub type_code: String,
    pub label: String,
    pub value: String,
    pub sort: i32,
    pub status: String,
    pub css_class: Option<String>,
}

impl From<ServiceDictDataVo> for DictDataVo {
    fn from(value: ServiceDictDataVo) -> Self {
        let ServiceDictDataVo {
            id,
            type_code,
            label,
            value,
            sort,
            status,
            css_class,
        } = value;
        Self {
            id,
            type_code,
            label,
            value,
            sort,
            status,
            css_class,
        }
    }
}

/// 导出任务响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct ExportJobVo {
    pub id: String,
    pub resource: String,
    pub status: String,
    pub result_file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl From<ServiceExportJobVo> for ExportJobVo {
    fn from(value: ServiceExportJobVo) -> Self {
        let ServiceExportJobVo {
            id,
            resource,
            status,
            result_file_name,
            content_type,
            file_size,
            expires_at,
            error_message,
            created_at,
            updated_at,
            completed_at,
        } = value;
        Self {
            id,
            resource,
            status,
            result_file_name,
            content_type,
            file_size,
            expires_at,
            error_message,
            created_at,
            updated_at,
            completed_at,
        }
    }
}

/// 文件上传响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct UploadResponse {
    pub file_id: String,
    pub file_name: String,
    pub file_path: String,
    pub file_url: String,
}

/// 构造只能通过认证 API 下载的私有文件地址。
pub(crate) fn private_file_url(bucket: &str, path: &str) -> String {
    format!(
        "{}?bucket={}&path={}",
        api_path("common/file/download"),
        utf8_percent_encode(bucket, NON_ALPHANUMERIC),
        utf8_percent_encode(path, NON_ALPHANUMERIC),
    )
}

impl From<ServiceUploadResponse> for UploadResponse {
    fn from(value: ServiceUploadResponse) -> Self {
        let ServiceUploadResponse {
            file_id,
            bucket,
            file_name,
            file_path,
        } = value;
        Self {
            file_id,
            file_url: private_file_url(&bucket, &file_path),
            file_name,
            file_path,
        }
    }
}

/// 部门响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct DeptVo {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub ancestors: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceDeptVo> for DeptVo {
    fn from(value: ServiceDeptVo) -> Self {
        let ServiceDeptVo {
            id,
            name,
            parent_id,
            ancestors,
            sort,
            status,
            remark,
            created_at,
        } = value;
        Self {
            id,
            name,
            parent_id,
            ancestors,
            sort,
            status,
            remark,
            created_at,
        }
    }
}

/// 部门树节点。
#[derive(Debug, Serialize, ToSchema)]
pub struct DeptTreeNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub sort: i32,
    pub status: String,
    #[schema(no_recursion)]
    pub children: Vec<DeptTreeNode>,
}

impl From<ServiceDeptTreeNode> for DeptTreeNode {
    fn from(value: ServiceDeptTreeNode) -> Self {
        let ServiceDeptTreeNode {
            id,
            name,
            parent_id,
            sort,
            status,
            children,
        } = value;
        Self {
            id,
            name,
            parent_id,
            sort,
            status,
            children: children.into_iter().map(Self::from).collect(),
        }
    }
}

/// 登录日志响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct LoginInfoVo {
    pub id: String,
    pub user_name: String,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub status: String,
    pub msg: Option<String>,
    pub login_time: String,
}

impl From<ServiceLoginInfoVo> for LoginInfoVo {
    fn from(value: ServiceLoginInfoVo) -> Self {
        let ServiceLoginInfoVo {
            id,
            user_name,
            ipaddr,
            login_location,
            browser,
            os,
            status,
            msg,
            login_time,
        } = value;
        Self {
            id,
            user_name,
            ipaddr,
            login_location,
            browser,
            os,
            status,
            msg,
            login_time,
        }
    }
}

/// 菜单类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum MenuType {
    #[serde(rename = "M")]
    Directory,
    #[serde(rename = "C")]
    Page,
    #[serde(rename = "F")]
    Action,
}

impl From<MenuType> for ServiceMenuType {
    fn from(value: MenuType) -> Self {
        match value {
            MenuType::Directory => Self::Directory,
            MenuType::Page => Self::Page,
            MenuType::Action => Self::Action,
        }
    }
}

/// 菜单响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct MenuVo {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: String,
    pub perm_id: Option<String>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceMenuVo> for MenuVo {
    fn from(value: ServiceMenuVo) -> Self {
        let ServiceMenuVo {
            id,
            name,
            parent_id,
            menu_type,
            perm_id,
            route_key,
            icon,
            sort,
            visible,
            status,
            remark,
            created_at,
        } = value;
        Self {
            id,
            name,
            parent_id,
            menu_type,
            perm_id,
            route_key,
            icon,
            sort,
            visible,
            status,
            remark,
            created_at,
        }
    }
}

/// 菜单树节点。
#[derive(Debug, Serialize, ToSchema)]
pub struct MenuTreeNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: String,
    pub perm_id: Option<String>,
    pub perm_code: Option<String>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    #[schema(no_recursion)]
    pub children: Vec<MenuTreeNode>,
}

impl From<ServiceMenuTreeNode> for MenuTreeNode {
    fn from(value: ServiceMenuTreeNode) -> Self {
        let ServiceMenuTreeNode {
            id,
            name,
            parent_id,
            menu_type,
            perm_id,
            perm_code,
            route_key,
            icon,
            sort,
            visible,
            status,
            children,
        } = value;
        Self {
            id,
            name,
            parent_id,
            menu_type,
            perm_id,
            perm_code,
            route_key,
            icon,
            sort,
            visible,
            status,
            children: children.into_iter().map(Self::from).collect(),
        }
    }
}

/// 在线用户响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct OnlineUserVo {
    pub sid: String,
    pub username: String,
    pub dept_name: Option<String>,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub login_time: String,
    pub last_access_time: String,
}

impl From<ServiceOnlineUserVo> for OnlineUserVo {
    fn from(value: ServiceOnlineUserVo) -> Self {
        let ServiceOnlineUserVo {
            sid,
            username,
            dept_name,
            ipaddr,
            login_location,
            browser,
            os,
            login_time,
            last_access_time,
        } = value;
        Self {
            sid,
            username,
            dept_name,
            ipaddr,
            login_location,
            browser,
            os,
            login_time,
            last_access_time,
        }
    }
}

/// 操作日志响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct OperLogVo {
    pub id: String,
    pub title: String,
    pub business_type: String,
    pub method: String,
    pub request_method: String,
    pub oper_name: String,
    pub oper_url: String,
    pub oper_ip: String,
    pub oper_location: Option<String>,
    pub oper_param: Option<String>,
    pub json_result: Option<String>,
    pub status: String,
    pub error_msg: Option<String>,
    pub cost_time: i64,
    pub oper_time: String,
}

impl From<ServiceOperLogVo> for OperLogVo {
    fn from(value: ServiceOperLogVo) -> Self {
        let ServiceOperLogVo {
            id,
            title,
            business_type,
            method,
            request_method,
            oper_name,
            oper_url,
            oper_ip,
            oper_location,
            oper_param,
            json_result,
            status,
            error_msg,
            cost_time,
            oper_time,
        } = value;
        Self {
            id,
            title,
            business_type,
            method,
            request_method,
            oper_name,
            oper_url,
            oper_ip,
            oper_location,
            oper_param,
            json_result,
            status,
            error_msg,
            cost_time,
            oper_time,
        }
    }
}

/// 通知公告响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct NoticeVo {
    pub id: String,
    pub title: String,
    pub content_markdown: String,
    pub notice_type: Option<String>,
    pub status: String,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceNoticeVo> for NoticeVo {
    fn from(value: ServiceNoticeVo) -> Self {
        let ServiceNoticeVo {
            id,
            title,
            content_markdown,
            notice_type,
            status,
            created_by,
            created_at,
        } = value;
        Self {
            id,
            title,
            content_markdown,
            notice_type,
            status,
            created_by,
            created_at,
        }
    }
}

/// 选择器候选项。
#[derive(Debug, Serialize, ToSchema)]
pub struct OptionItem {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub disabled: bool,
}

impl From<ServiceOptionItem> for OptionItem {
    fn from(value: ServiceOptionItem) -> Self {
        let ServiceOptionItem {
            value,
            label,
            description,
            disabled,
        } = value;
        Self {
            value,
            label,
            description,
            disabled,
        }
    }
}

/// 有界选择器响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct OptionList {
    pub items: Vec<OptionItem>,
    pub has_more: bool,
}

impl From<ServiceOptionList> for OptionList {
    fn from(value: ServiceOptionList) -> Self {
        let ServiceOptionList { items, has_more } = value;
        Self {
            items: items.into_iter().map(OptionItem::from).collect(),
            has_more,
        }
    }
}

/// 岗位响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct PostVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ServicePostVo> for PostVo {
    fn from(value: ServicePostVo) -> Self {
        let ServicePostVo {
            id,
            name,
            code,
            sort,
            status,
            remark,
            created_at,
        } = value;
        Self {
            id,
            name,
            code,
            sort,
            status,
            remark,
            created_at,
        }
    }
}

/// 权限类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionType {
    Api,
    Menu,
}

impl PermissionType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Menu => "menu",
        }
    }
}

impl From<PermissionType> for ServicePermissionType {
    fn from(value: PermissionType) -> Self {
        match value {
            PermissionType::Api => Self::Api,
            PermissionType::Menu => Self::Menu,
        }
    }
}

/// 权限树节点。
#[derive(Debug, Serialize, ToSchema)]
pub struct PermissionTreeNode {
    pub id: String,
    pub name: String,
    pub code: String,
    pub parent_id: Option<String>,
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    #[schema(no_recursion)]
    pub children: Vec<PermissionTreeNode>,
}

impl From<ServicePermissionTreeNode> for PermissionTreeNode {
    fn from(value: ServicePermissionTreeNode) -> Self {
        let ServicePermissionTreeNode {
            id,
            name,
            code,
            parent_id,
            perm_type,
            icon,
            sort,
            status,
            children,
        } = value;
        Self {
            id,
            name,
            code,
            parent_id,
            perm_type,
            icon,
            sort,
            status,
            children: children.into_iter().map(Self::from).collect(),
        }
    }
}

/// 权限详情响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct PermissionVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub parent_id: Option<String>,
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

impl From<ServicePermissionVo> for PermissionVo {
    fn from(value: ServicePermissionVo) -> Self {
        let ServicePermissionVo {
            id,
            name,
            code,
            parent_id,
            perm_type,
            icon,
            sort,
            status,
            created_at,
        } = value;
        Self {
            id,
            name,
            code,
            parent_id,
            perm_type,
            icon,
            sort,
            status,
            created_at,
        }
    }
}

/// 权限同步结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct PermissionSyncReport {
    pub scanned: usize,
    pub existing: usize,
    pub created: usize,
    pub missing: Vec<String>,
}

impl From<ServicePermissionSyncReport> for PermissionSyncReport {
    fn from(value: ServicePermissionSyncReport) -> Self {
        let ServicePermissionSyncReport {
            scanned,
            existing,
            created,
            missing,
        } = value;
        Self {
            scanned,
            existing,
            created,
            missing,
        }
    }
}

/// 用户个人信息响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct UserProfileResponse {
    pub user_id: String,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub preferred_locale: Option<String>,
    pub dept_id: Option<String>,
    pub dept_name: Option<String>,
    pub status: String,
    pub remark: Option<String>,
    pub login_ip: Option<String>,
    pub login_date: Option<String>,
    pub created_at: String,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
}

impl From<ServiceUserProfileResponse> for UserProfileResponse {
    fn from(value: ServiceUserProfileResponse) -> Self {
        let ServiceUserProfileResponse {
            user_id,
            username,
            nickname,
            email,
            phone,
            avatar,
            preferred_locale,
            dept_id,
            dept_name,
            status,
            remark,
            login_ip,
            login_date,
            created_at,
            roles,
            permissions,
        } = value;
        Self {
            user_id,
            username,
            nickname,
            email,
            phone,
            avatar,
            preferred_locale,
            dept_id,
            dept_name,
            status,
            remark,
            login_ip,
            login_date,
            created_at,
            roles,
            permissions,
        }
    }
}

/// 角色响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct RoleVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub is_super: i8,
    pub data_scope: String,
    pub status: String,
    pub sort: i32,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dept_ids: Option<Vec<String>>,
}

impl From<ServiceRoleVo> for RoleVo {
    fn from(value: ServiceRoleVo) -> Self {
        let ServiceRoleVo {
            id,
            name,
            code,
            is_super,
            data_scope,
            status,
            sort,
            remark,
            created_at,
            dept_ids,
        } = value;
        Self {
            id,
            name,
            code,
            is_super,
            data_scope,
            status,
            sort,
            remark,
            created_at,
            dept_ids,
        }
    }
}

/// 租户响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantVo {
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
}

impl From<ServiceTenantVo> for TenantVo {
    fn from(value: ServiceTenantVo) -> Self {
        let ServiceTenantVo {
            tenant_id,
            name,
            domain,
            status,
            expire_at,
            max_users,
            max_roles,
            max_storage_mb,
            max_requests_per_min,
        } = value;
        Self {
            tenant_id,
            name,
            domain,
            status,
            expire_at,
            max_users,
            max_roles,
            max_storage_mb,
            max_requests_per_min,
        }
    }
}

/// 用户响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct UserVo {
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub status: String,
    pub dept_id: Option<String>,
    pub dept_name: Option<String>,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceUserVo> for UserVo {
    fn from(value: ServiceUserVo) -> Self {
        let ServiceUserVo {
            id,
            username,
            nickname,
            email,
            phone,
            avatar,
            status,
            dept_id,
            dept_name,
            remark,
            created_at,
        } = value;
        Self {
            id,
            username,
            nickname,
            email,
            phone,
            avatar,
            status,
            dept_id,
            dept_name,
            remark,
            created_at,
        }
    }
}

/// 用户关联的简要角色信息。
#[derive(Debug, Serialize, ToSchema)]
pub struct RoleBriefVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub is_super: i8,
}

impl From<ServiceRoleBriefVo> for RoleBriefVo {
    fn from(value: ServiceRoleBriefVo) -> Self {
        let ServiceRoleBriefVo {
            id,
            name,
            code,
            is_super,
        } = value;
        Self {
            id,
            name,
            code,
            is_super,
        }
    }
}

/// 用户详情响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct UserDetailVo {
    #[serde(flatten)]
    pub user: UserVo,
    pub roles: Vec<RoleBriefVo>,
}

impl From<ServiceUserDetailVo> for UserDetailVo {
    fn from(value: ServiceUserDetailVo) -> Self {
        let ServiceUserDetailVo { user, roles } = value;
        Self {
            user: user.into(),
            roles: roles.into_iter().map(RoleBriefVo::from).collect(),
        }
    }
}

/// 数据库表结构响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TableInfo {
    pub table_name: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
}

impl From<ServiceTableInfo> for TableInfo {
    fn from(value: ServiceTableInfo) -> Self {
        let ServiceTableInfo {
            table_name,
            comment,
            columns,
        } = value;
        Self {
            table_name,
            comment,
            columns: columns.into_iter().map(ColumnInfo::from).collect(),
        }
    }
}

/// 数据库列结构响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub rust_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_auto_increment: bool,
    pub comment: Option<String>,
}

impl From<ServiceColumnInfo> for ColumnInfo {
    fn from(value: ServiceColumnInfo) -> Self {
        let ServiceColumnInfo {
            name,
            data_type,
            rust_type,
            is_nullable,
            is_primary_key,
            is_unique,
            is_auto_increment,
            comment,
        } = value;
        Self {
            name,
            data_type,
            rust_type,
            is_nullable,
            is_primary_key,
            is_unique,
            is_auto_increment,
            comment,
        }
    }
}

/// 代码生成预览文件。
#[derive(Debug, Serialize, ToSchema)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

impl From<ServiceGeneratedFile> for GeneratedFile {
    fn from(value: ServiceGeneratedFile) -> Self {
        let ServiceGeneratedFile { path, content } = value;
        Self { path, content }
    }
}

/// 代码生成写入报告。
#[derive(Debug, Serialize, ToSchema)]
pub struct WriteReport {
    pub written: Vec<String>,
    pub skipped: Vec<String>,
}

impl From<ServiceWriteReport> for WriteReport {
    fn from(value: ServiceWriteReport) -> Self {
        let ServiceWriteReport { written, skipped } = value;
        Self { written, skipped }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn public_enums_keep_the_wire_values() {
        assert_eq!(
            serde_json::to_value(MenuType::Directory).unwrap(),
            json!("M")
        );
        assert_eq!(serde_json::to_value(MenuType::Page).unwrap(), json!("C"));
        assert_eq!(serde_json::to_value(MenuType::Action).unwrap(), json!("F"));
        assert_eq!(
            serde_json::to_value(PermissionType::Api).unwrap(),
            json!("api")
        );
        assert_eq!(
            serde_json::to_value(PermissionType::Menu).unwrap(),
            json!("menu")
        );
    }

    #[test]
    fn recursive_tree_conversion_keeps_the_json_shape() {
        let source = ServiceDeptTreeNode {
            id: "1".into(),
            name: "总部".into(),
            parent_id: None,
            sort: 1,
            status: "0".into(),
            children: vec![ServiceDeptTreeNode {
                id: "2".into(),
                name: "研发部".into(),
                parent_id: Some("1".into()),
                sort: 2,
                status: "0".into(),
                children: Vec::new(),
            }],
        };

        assert_eq!(
            serde_json::to_value(DeptTreeNode::from(source)).unwrap(),
            json!({
                "id": "1",
                "name": "总部",
                "parent_id": null,
                "sort": 1,
                "status": "0",
                "children": [{
                    "id": "2",
                    "name": "研发部",
                    "parent_id": "1",
                    "sort": 2,
                    "status": "0",
                    "children": []
                }]
            })
        );
    }

    #[test]
    fn user_detail_conversion_keeps_flattened_fields() {
        let created_at = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        let source = ServiceUserDetailVo {
            user: ServiceUserVo {
                id: "10".into(),
                username: "admin".into(),
                nickname: "管理员".into(),
                email: "admin@example.com".into(),
                phone: "13800000000".into(),
                avatar: None,
                status: "0".into(),
                dept_id: Some("20".into()),
                dept_name: Some("研发部".into()),
                remark: None,
                created_at,
            },
            roles: vec![ServiceRoleBriefVo {
                id: "30".into(),
                name: "管理员".into(),
                code: "admin".into(),
                is_super: 1,
            }],
        };

        let value = serde_json::to_value(UserDetailVo::from(source)).unwrap();
        assert_eq!(value["id"], "10");
        assert_eq!(value["username"], "admin");
        assert_eq!(value["roles"][0]["code"], "admin");
        assert!(value.get("user").is_none());
    }

    #[test]
    fn optional_public_fields_keep_the_omission_rules() {
        let options = ServiceOptionList {
            items: vec![ServiceOptionItem {
                value: "1".into(),
                label: "管理员".into(),
                description: None,
                disabled: false,
            }],
            has_more: false,
        };
        let option_value = serde_json::to_value(OptionList::from(options)).unwrap();
        assert!(option_value["items"][0].get("description").is_none());

        let role = ServiceRoleVo {
            id: "1".into(),
            name: "管理员".into(),
            code: "admin".into(),
            is_super: 1,
            data_scope: "1".into(),
            status: "0".into(),
            sort: 1,
            remark: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
            dept_ids: None,
        };
        let role_value = serde_json::to_value(RoleVo::from(role)).unwrap();
        assert!(role_value.get("dept_ids").is_none());
    }

    #[test]
    fn nested_upload_and_generator_contracts_are_stable() {
        let upload = ServiceUploadResponse {
            file_id: "42".into(),
            bucket: "uploads".into(),
            file_name: "报告.xlsx".into(),
            file_path: "2026/08/stable.xlsx".into(),
        };
        assert_eq!(
            serde_json::to_value(UploadResponse::from(upload)).unwrap(),
            json!({
                "file_id": "42",
                "file_name": "报告.xlsx",
                "file_path": "2026/08/stable.xlsx",
                "file_url": "/api/v1/common/file/download?bucket=uploads&path=2026%2F08%2Fstable%2Exlsx"
            })
        );

        let table = ServiceTableInfo {
            table_name: "sys_user".into(),
            comment: Some("用户表".into()),
            columns: vec![ServiceColumnInfo {
                name: "id".into(),
                data_type: "bigint".into(),
                rust_type: "i64".into(),
                is_nullable: false,
                is_primary_key: true,
                is_unique: true,
                is_auto_increment: false,
                comment: Some("主键".into()),
            }],
        };
        let table_value = serde_json::to_value(TableInfo::from(table)).unwrap();
        assert_eq!(table_value["table_name"], "sys_user");
        assert_eq!(table_value["columns"][0]["is_primary_key"], true);
    }
}
