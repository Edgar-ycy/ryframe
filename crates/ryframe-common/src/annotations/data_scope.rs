use serde::{Deserialize, Serialize};

/// 数据权限范围
///
/// 控制用户在查询时的行级可见范围，对应 sys_role.data_scope 字段：
/// - All:           data_scope='1' 全部数据
/// - Custom:        data_scope='2' 自定义部门数据
/// - Dept:          data_scope='3' 本部门数据
/// - DeptAndChildren: data_scope='4' 本部门及以下数据
/// - SelfOnly:      data_scope='5' 仅本人数据
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataScope {
    /// 全部数据权限（超级管理员）
    All,
    /// 自定义数据权限（根据 sys_role_dept 表动态决定）
    Custom,
    /// 本部门数据
    Dept,
    /// 本部门及以下数据
    DeptAndChildren,
    /// 仅本人数据
    SelfOnly,
}

impl DataScope {
    /// 从数据库 CHAR(1) 值转换为枚举
    pub fn from_db_value(v: &str) -> Self {
        match v {
            "1" => DataScope::All,
            "2" => DataScope::Custom,
            "3" => DataScope::Dept,
            "4" => DataScope::DeptAndChildren,
            "5" => DataScope::SelfOnly,
            _ => DataScope::SelfOnly, // 默认最严格
        }
    }

    /// 转为数据库 CHAR(1) 值
    pub fn to_db_value(&self) -> &str {
        match self {
            DataScope::All => "1",
            DataScope::Custom => "2",
            DataScope::Dept => "3",
            DataScope::DeptAndChildren => "4",
            DataScope::SelfOnly => "5",
        }
    }
}

/// 数据权限上下文
///
/// 在 Handler 层从 CurrentUser 中提取后传入 Service 层。
/// Service 层调用 `build_condition` 构建 SeaORM 过滤条件。
#[derive(Debug, Clone)]
pub struct DataScopeContext {
    pub scope: DataScope,
    pub user_id: i64,
    pub dept_id: Option<i64>,
    /// 部门祖级路径，形如 "0,1,2"，用于 DeptAndChildren 的 LIKE 匹配
    pub ancestors: Option<String>,
    /// 自定义权限的部门 ID 列表（从 sys_role_dept 表查出）
    pub custom_dept_ids: Vec<i64>,
}

impl DataScopeContext {
    /// 创建超级管理员上下文（DataScope::All，不过滤）
    pub fn super_admin(user_id: i64) -> Self {
        Self {
            scope: DataScope::All,
            user_id,
            dept_id: None,
            ancestors: None,
            custom_dept_ids: vec![],
        }
    }

    /// 构建 SeaORM SQL 条件片段
    ///
    /// `dept_alias` — 被查询表的部门列名（如 `"dept_id"` 或 `"sys_user"."dept_id"`）
    /// `user_id_col` — 被查询表的用户ID列名（如 `"id"` 或 `"sys_user"."id"`）
    ///
    /// 返回 `None` 表示 DataScope::All（不加任何条件）。
    /// 返回 `Some(sql_string)` 表示需要追加到 WHERE 子句的条件。
    pub fn build_sql_condition(&self, dept_alias: &str, user_id_col: &str) -> Option<String> {
        match &self.scope {
            DataScope::All => None,
            DataScope::SelfOnly => Some(format!("{} = {}", user_id_col, self.user_id)),
            DataScope::Dept => {
                match self.dept_id {
                    Some(did) => Some(format!("{} = {}", dept_alias, did)),
                    None => Some("1 = 0".to_string()), // 无部门则看不到任何数据
                }
            }
            DataScope::DeptAndChildren => {
                // 查本部门 + 所有 ancestors 包含本部门路径的子部门
                // 使用子查询: dept_id IN (SELECT id FROM sys_dept WHERE id = X OR ancestors LIKE 'X,%')
                match self.dept_id {
                    Some(did) => Some(format!(
                        "{} IN (SELECT id FROM sys_dept WHERE id = {} OR ancestors LIKE CONCAT((SELECT ancestors FROM sys_dept WHERE id = {}), ',{}%'))",
                        dept_alias, did, did, did
                    )),
                    None => Some("1 = 0".to_string()),
                }
            }
            DataScope::Custom => {
                if self.custom_dept_ids.is_empty() {
                    Some("1 = 0".to_string())
                } else {
                    let ids: Vec<String> = self
                        .custom_dept_ids
                        .iter()
                        .map(|id| id.to_string())
                        .collect();
                    Some(format!("{} IN ({})", dept_alias, ids.join(",")))
                }
            }
        }
    }

    /// 取多个角色中最宽松的数据权限
    ///
    /// 优先级: All > Custom > DeptAndChildren > Dept > SelfOnly
    /// 如果有任一角色为 All，则整体为 All。
    /// Custom 时合并所有角色的自定义部门ID。
    pub fn merge(scopes: Vec<DataScopeContext>) -> DataScopeContext {
        if scopes.is_empty() {
            return DataScopeContext {
                scope: DataScope::SelfOnly,
                user_id: 0,
                dept_id: None,
                ancestors: None,
                custom_dept_ids: vec![],
            };
        }

        let user_id = scopes[0].user_id;
        let dept_id = scopes[0].dept_id;
        let ancestors = scopes[0].ancestors.clone();

        // 优先级排序
        fn priority(s: &DataScope) -> u8 {
            match s {
                DataScope::All => 5,
                DataScope::Custom => 4,
                DataScope::DeptAndChildren => 3,
                DataScope::Dept => 2,
                DataScope::SelfOnly => 1,
            }
        }

        let best_scope = scopes.iter().max_by_key(|s| priority(&s.scope)).unwrap();

        let mut custom_dept_ids: Vec<i64> = vec![];
        for s in &scopes {
            if s.scope == DataScope::Custom {
                custom_dept_ids.extend(&s.custom_dept_ids);
            }
        }
        custom_dept_ids.sort();
        custom_dept_ids.dedup();

        DataScopeContext {
            scope: best_scope.scope.clone(),
            user_id,
            dept_id,
            ancestors,
            custom_dept_ids,
        }
    }
}
