use crate::data_scope::{DataScope, DataScopeContextRef};
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
    /// 本次认证看到的租户数据范围投影版本；查询必须与目标投影精确一致。
    pub data_scope_version: i64,
    pub is_super_admin: bool,
}

impl ActorContext {
    /// 借用数据访问层所需的数据权限字段。
    pub fn data_scope_context(&self) -> DataScopeContextRef<'_> {
        DataScopeContextRef {
            scope: self.data_scope.clone(),
            user_id: self.user_id,
            dept_id: self.dept_id,
            ancestors: self.dept_path.as_deref(),
            custom_dept_ids: &self.custom_dept_ids,
            include_self: self.include_self,
            data_scope_version: self.data_scope_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ActorContext, DataScope};

    #[test]
    fn data_scope_view_borrows_actor_buffers_and_preserves_scope() {
        let actor = ActorContext {
            user_id: 17,
            tenant_id: "tenant".into(),
            username: "tester".into(),
            dept_id: Some(33),
            dept_path: Some("0,22,33".into()),
            data_scope: DataScope::Custom,
            custom_dept_ids: (1..=4096).collect(),
            include_self: true,
            data_scope_version: 42,
            is_super_admin: false,
        };

        let scope = actor.data_scope_context();
        assert!(std::ptr::eq(
            scope.custom_dept_ids,
            actor.custom_dept_ids.as_slice()
        ));
        assert!(std::ptr::eq(
            scope.ancestors.unwrap(),
            actor.dept_path.as_deref().unwrap(),
        ));
        assert_eq!(scope.scope, actor.data_scope);
        assert_eq!(scope.user_id, actor.user_id);
        assert_eq!(scope.dept_id, actor.dept_id);
        assert_eq!(scope.include_self, actor.include_self);
        assert_eq!(scope.data_scope_version, actor.data_scope_version);
    }
}
