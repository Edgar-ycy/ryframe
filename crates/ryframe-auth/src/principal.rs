use std::ops::Deref;

use ryframe_kernel::ActorContext;
use serde::{Deserialize, Serialize};

/// 当前请求中一次解析完成的不可变认证身份。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestPrincipal {
    pub actor: ActorContext,
    /// 必须先修改初始密码，受限主体不能访问普通业务接口。
    pub password_change_required: bool,
    /// 构造此主体时核验的用户授权版本，不能用后来读取的新版本覆盖旧主体。
    pub user_authorization_version: i32,
    /// 构造此主体时核验的租户数据范围版本。
    pub data_scope_version: i64,
    /// 解析当前主体时使用的租户授权纪元，用于客户端检测授权变化。
    pub tenant_authorization_epoch: i32,
    /// 用户保存的语言偏好；请求头未指定可用语言时作为回退。
    pub preferred_locale: Option<String>,
    pub roles: Vec<String>,
    pub role_ids: Vec<i64>,
    pub permissions: Vec<String>,
    pub tenant_request_limit_per_minute: u32,
}

impl Deref for RequestPrincipal {
    type Target = ActorContext;

    fn deref(&self) -> &Self::Target {
        &self.actor
    }
}
