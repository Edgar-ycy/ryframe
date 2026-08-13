use serde::Serialize;

use crate::system::ServiceCapabilityDescriptor;

/// 编译期固定的 Agent 查询能力，不接受客户端传入 operation、路径或权限码。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    Capabilities,
    DirectoryUsers,
    DirectoryDepartments,
    DirectoryPosts,
    ReferenceDictionary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentCapabilityDescriptor {
    pub capability: AgentCapability,
    pub key: &'static str,
    pub operation_id: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub required_permission: &'static str,
    pub direct: bool,
    pub delegated: bool,
    pub cost: u32,
}

impl AgentCapability {
    pub const ALL: [Self; 5] = [
        Self::Capabilities,
        Self::DirectoryUsers,
        Self::DirectoryDepartments,
        Self::DirectoryPosts,
        Self::ReferenceDictionary,
    ];

    pub const fn descriptor(self) -> &'static AgentCapabilityDescriptor {
        match self {
            Self::Capabilities => &CAPABILITIES,
            Self::DirectoryUsers => &DIRECTORY_USERS,
            Self::DirectoryDepartments => &DIRECTORY_DEPARTMENTS,
            Self::DirectoryPosts => &DIRECTORY_POSTS,
            Self::ReferenceDictionary => &REFERENCE_DICTIONARY,
        }
    }
}

const CAPABILITIES: AgentCapabilityDescriptor = AgentCapabilityDescriptor {
    capability: AgentCapability::Capabilities,
    key: "capabilities.list",
    operation_id: "get_agent_v1_capabilities",
    method: "GET",
    path: "/api/v1/agent/v1/capabilities",
    // 能力发现本身不授予数据访问，认证后仅返回调用方当前真正可用的固定能力。
    required_permission: "",
    direct: true,
    delegated: true,
    cost: 1,
};

const DIRECTORY_USERS: AgentCapabilityDescriptor = AgentCapabilityDescriptor {
    capability: AgentCapability::DirectoryUsers,
    key: "directory.users.list",
    operation_id: "get_agent_v1_directory_users",
    method: "GET",
    path: "/api/v1/agent/v1/directory/users",
    required_permission: "system:user:list",
    direct: true,
    delegated: true,
    cost: 1,
};

const DIRECTORY_DEPARTMENTS: AgentCapabilityDescriptor = AgentCapabilityDescriptor {
    capability: AgentCapability::DirectoryDepartments,
    key: "directory.departments.list",
    operation_id: "get_agent_v1_directory_departments",
    method: "GET",
    path: "/api/v1/agent/v1/directory/departments",
    required_permission: "system:dept:list",
    direct: true,
    delegated: true,
    cost: 1,
};

const DIRECTORY_POSTS: AgentCapabilityDescriptor = AgentCapabilityDescriptor {
    capability: AgentCapability::DirectoryPosts,
    key: "directory.posts.list",
    operation_id: "get_agent_v1_directory_posts",
    method: "GET",
    path: "/api/v1/agent/v1/directory/posts",
    required_permission: "system:post:list",
    direct: true,
    delegated: true,
    cost: 1,
};

const REFERENCE_DICTIONARY: AgentCapabilityDescriptor = AgentCapabilityDescriptor {
    capability: AgentCapability::ReferenceDictionary,
    key: "reference.dictionaries.read",
    operation_id: "get_agent_v1_reference_dictionaries_by_type_code",
    method: "GET",
    path: "/api/v1/agent/v1/reference/dictionaries/{type_code}",
    required_permission: "system:dict:list",
    direct: true,
    delegated: true,
    cost: 1,
};

/// 管理端与委托创建共用这一份目录，避免权限与 Agent 路由发生漂移。
///
/// 能力发现不是可委托的数据能力，因此不进入管理端选择列表。
pub fn service_capability_descriptors() -> Vec<ServiceCapabilityDescriptor> {
    AgentCapability::ALL
        .into_iter()
        .map(SelfDescriptor::new)
        .filter(|descriptor| !descriptor.required_permission.is_empty())
        .map(|descriptor| ServiceCapabilityDescriptor {
            key: descriptor.key.to_owned(),
            permission: descriptor.required_permission.to_owned(),
            direct: descriptor.direct,
            delegated: descriptor.delegated,
        })
        .collect()
}

struct SelfDescriptor {
    key: &'static str,
    required_permission: &'static str,
    direct: bool,
    delegated: bool,
}

impl SelfDescriptor {
    fn new(capability: AgentCapability) -> Self {
        let descriptor = capability.descriptor();
        Self {
            key: descriptor.key,
            required_permission: descriptor.required_permission,
            direct: descriptor.direct,
            delegated: descriptor.delegated,
        }
    }
}
