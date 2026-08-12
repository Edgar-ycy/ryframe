use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use ryframe_service::system::{
    ConfigVo as ServiceConfigVo, DeptTreeNode as ServiceDeptTreeNode, DeptVo as ServiceDeptVo,
    DictDataVo as ServiceDictDataVo, DictTypeVo as ServiceDictTypeVo, NoticeVo as ServiceNoticeVo,
    OptionItem as ServiceOptionItem, OptionList as ServiceOptionList, PostVo as ServicePostVo,
    RoleVo as ServiceRoleVo, TenantVo as ServiceTenantVo,
};

/// 参数配置响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigVo {
    pub id: String,
    pub name: String,
    pub key: String,
    pub value: String,
    pub portable: bool,
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
            portable,
            remark,
            created_at,
        } = value;
        Self {
            id,
            name,
            key,
            value,
            portable,
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
