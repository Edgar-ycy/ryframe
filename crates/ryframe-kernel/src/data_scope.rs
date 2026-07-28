use serde::{Deserialize, Serialize};

/// 数据权限范围。
///
/// 控制用户在查询时的行级可见范围，对应 `sys_role.data_scope` 字段：
/// - `All`：`data_scope='1'`，全部数据。
/// - `Custom`：`data_scope='2'`，自定义部门数据。
/// - `Dept`：`data_scope='3'`，本部门数据。
/// - `DeptAndChildren`：`data_scope='4'`，本部门及以下数据。
/// - `SelfOnly`：`data_scope='5'`，仅本人数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataScope {
    /// 全部数据权限（超级管理员）。
    All,
    /// 自定义数据权限（根据 `sys_role_dept` 表动态决定）。
    Custom,
    /// 本部门数据。
    Dept,
    /// 本部门及以下数据。
    DeptAndChildren,
    /// 仅本人数据。
    SelfOnly,
}

impl DataScope {
    /// 从数据库 `CHAR(1)` 值转换为枚举。
    pub fn from_db_value(value: &str) -> Self {
        match value {
            "1" => Self::All,
            "2" => Self::Custom,
            "3" => Self::Dept,
            "4" => Self::DeptAndChildren,
            "5" => Self::SelfOnly,
            _ => Self::SelfOnly,
        }
    }

    /// 转换为数据库 `CHAR(1)` 值。
    pub fn to_db_value(&self) -> &str {
        match self {
            Self::All => "1",
            Self::Custom => "2",
            Self::Dept => "3",
            Self::DeptAndChildren => "4",
            Self::SelfOnly => "5",
        }
    }
}

/// 数据权限上下文。
///
/// 从已认证的 `RequestPrincipal` 中提取后传入服务层。服务层可调用
/// [`Self::build_sql_condition`] 构建 SQL 过滤条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataScopeContext {
    pub scope: DataScope,
    pub user_id: i64,
    pub dept_id: Option<i64>,
    /// 部门祖级路径，形如 `"0,1,2"`，用于 `DeptAndChildren` 的 LIKE 匹配。
    pub ancestors: Option<String>,
    /// 自定义权限的部门 ID 列表（从 `sys_role_dept` 表查询）。
    pub custom_dept_ids: Vec<i64>,
    /// 多角色合并后是否还需要包含“仅本人”范围。
    pub include_self: bool,
}

impl DataScopeContext {
    /// 创建超级管理员上下文（`DataScope::All`，不添加过滤条件）。
    pub fn super_admin(user_id: i64) -> Self {
        Self {
            scope: DataScope::All,
            user_id,
            dept_id: None,
            ancestors: None,
            custom_dept_ids: vec![],
            include_self: false,
        }
    }

    /// 构建 SQL 条件片段。
    ///
    /// `dept_alias` 是被查询表的部门列名，例如 `"dept_id"` 或
    /// `"sys_user"."dept_id"`；`user_id_col` 是被查询表的用户 ID 列名。
    ///
    /// 返回 `None` 表示 `DataScope::All`，无需添加条件；返回 `Some` 则表示
    /// 需要追加到 WHERE 子句的条件。
    pub fn build_sql_condition(&self, dept_alias: &str, user_id_col: &str) -> Option<String> {
        match &self.scope {
            DataScope::All => None,
            DataScope::SelfOnly => Some(format!("{} = {}", user_id_col, self.user_id)),
            DataScope::Dept => match self.dept_id {
                Some(dept_id) => Some(format!("{} = {}", dept_alias, dept_id)),
                None => Some("1 = 0".to_string()),
            },
            DataScope::DeptAndChildren => match self.dept_id {
                Some(dept_id) => Some(format!(
                    "{} IN (SELECT id FROM sys_dept WHERE id = {} OR ancestors LIKE CONCAT((SELECT ancestors FROM sys_dept WHERE id = {}), ',{}%'))",
                    dept_alias, dept_id, dept_id, dept_id
                )),
                None => Some("1 = 0".to_string()),
            },
            DataScope::Custom => {
                if self.custom_dept_ids.is_empty() && !self.include_self {
                    return Some("1 = 0".to_string());
                }

                let ids = self
                    .custom_dept_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>();
                let dept_condition =
                    (!ids.is_empty()).then(|| format!("{} IN ({})", dept_alias, ids.join(",")));

                match (dept_condition, self.include_self) {
                    (Some(departments), true) => Some(format!(
                        "({} OR {} = {})",
                        departments, user_id_col, self.user_id
                    )),
                    (Some(departments), false) => Some(departments),
                    (None, true) => Some(format!("{} = {}", user_id_col, self.user_id)),
                    (None, false) => Some("1 = 0".to_string()),
                }
            }
        }
    }

    /// 合并多个角色中最宽松的数据权限。
    ///
    /// 优先级为 `All > Custom > DeptAndChildren > Dept > SelfOnly`。任一角色为
    /// `All` 时，整体即为 `All`；`Custom` 会合并所有角色的自定义部门 ID。
    pub fn merge(scopes: Vec<Self>) -> Self {
        if scopes.is_empty() {
            return Self {
                scope: DataScope::SelfOnly,
                user_id: 0,
                dept_id: None,
                ancestors: None,
                custom_dept_ids: vec![],
                include_self: true,
            };
        }

        let user_id = scopes[0].user_id;
        let dept_id = scopes[0].dept_id;
        let ancestors = scopes[0].ancestors.clone();

        if scopes.iter().any(|item| item.scope == DataScope::All) {
            return Self::super_admin(user_id);
        }

        let mut custom_dept_ids = Vec::new();
        for scope in &scopes {
            match scope.scope {
                DataScope::Custom | DataScope::DeptAndChildren => {
                    custom_dept_ids.extend(&scope.custom_dept_ids);
                }
                DataScope::Dept => {
                    if let Some(dept_id) = scope.dept_id {
                        custom_dept_ids.push(dept_id);
                    }
                }
                DataScope::All | DataScope::SelfOnly => {}
            }
        }
        custom_dept_ids.sort();
        custom_dept_ids.dedup();

        let include_self = scopes.iter().any(|item| item.scope == DataScope::SelfOnly);
        Self {
            scope: if custom_dept_ids.is_empty() {
                DataScope::SelfOnly
            } else {
                DataScope::Custom
            },
            user_id,
            dept_id,
            ancestors,
            custom_dept_ids,
            include_self,
        }
    }
}
