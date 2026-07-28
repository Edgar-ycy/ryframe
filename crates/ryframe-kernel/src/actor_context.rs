use crate::data_scope::{DataScope, DataScopeContext};
use serde::{Deserialize, Serialize};

/// 显式传入业务用例的已认证应用主体。
///
/// HTTP 认证只创建一次该值。服务据此进行租户、操作人和数据权限范围判断，
/// 而无需依赖请求本地状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorContext {
    pub user_id: i64,
    pub tenant_id: String,
    pub username: String,
    pub dept_id: Option<i64>,
    pub dept_path: Option<String>,
    pub data_scope: DataScope,
    pub custom_dept_ids: Vec<i64>,
    pub include_self: bool,
    pub is_super_admin: bool,
}

impl ActorContext {
    /// 构造供数据访问层使用的数据权限上下文。
    pub fn data_scope_context(&self) -> DataScopeContext {
        DataScopeContext {
            scope: self.data_scope.clone(),
            user_id: self.user_id,
            dept_id: self.dept_id,
            ancestors: self.dept_path.clone(),
            custom_dept_ids: self.custom_dept_ids.clone(),
            include_self: self.include_self,
        }
    }
}
