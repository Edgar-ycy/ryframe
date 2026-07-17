use crate::annotations::data_scope::{DataScope, DataScopeContext};

/// Authenticated application actor passed explicitly into business use cases.
///
/// HTTP authentication creates this value once. Services use it for tenant,
/// operator and data-scope decisions without depending on request-local state.
#[derive(Debug, Clone, PartialEq, Eq)]
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
