use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentRowScope {
    All,
    Departments(Vec<i64>),
    DepartmentsAndUser {
        department_ids: Vec<i64>,
        user_id: i64,
    },
    User(i64),
    Empty,
}

#[derive(Debug)]
pub struct AgentUserSnapshot {
    pub id: i64,
    pub dept_id: Option<i64>,
    pub status: String,
    pub deleted: bool,
    pub authorization_version: i32,
}

impl AgentUserSnapshot {
    const STATUS_NORMAL: &'static str = "1";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL && !self.deleted
    }
}

#[derive(Debug)]
pub struct AgentRoleSnapshot {
    pub id: i64,
    pub is_super: bool,
    pub data_scope: String,
    pub status: String,
    pub deleted: bool,
}

impl AgentRoleSnapshot {
    const STATUS_NORMAL: &'static str = "1";
    const DATA_SCOPE_ALL: &'static str = "1";
    const DATA_SCOPE_CUSTOM: &'static str = "2";
    const DATA_SCOPE_DEPT: &'static str = "3";
    const DATA_SCOPE_DEPT_AND_CHILD: &'static str = "4";
    const DATA_SCOPE_SELF: &'static str = "5";

    pub fn is_active(&self) -> bool {
        self.status == Self::STATUS_NORMAL && !self.deleted
    }
}

#[derive(Debug)]
pub struct AgentRolePermissionSnapshot {
    pub role_id: i64,
    pub permission_id: i64,
}

#[derive(Debug)]
pub struct AgentPermissionSnapshot {
    pub id: i64,
    pub code: String,
    pub status: String,
}

impl AgentPermissionSnapshot {
    const STATUS_NORMAL: &'static str = "1";

    pub fn is_active(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }
}

#[derive(Debug)]
pub struct AgentRoleDepartmentSnapshot {
    pub role_id: i64,
    pub department_id: i64,
}

#[derive(Debug)]
pub struct AgentDepartmentSnapshot {
    pub id: i64,
    pub name: String,
    pub ancestors: String,
}

#[derive(Debug)]
pub struct AgentAuthorizationSnapshot {
    pub user: Option<AgentUserSnapshot>,
    pub account_role_ids: Vec<i64>,
    pub user_role_ids: Vec<i64>,
    pub roles: Vec<AgentRoleSnapshot>,
    pub role_permissions: Vec<AgentRolePermissionSnapshot>,
    pub permissions: Vec<AgentPermissionSnapshot>,
    pub role_departments: Vec<AgentRoleDepartmentSnapshot>,
    pub departments: Vec<AgentDepartmentSnapshot>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SubjectScope {
    all: bool,
    departments: BTreeSet<i64>,
    self_user_id: Option<i64>,
}

impl SubjectScope {
    pub fn is_all(&self) -> bool {
        self.all
    }
}

pub(super) fn resolve_account_scope(
    snapshot: &AgentAuthorizationSnapshot,
    account_dept_id: Option<i64>,
) -> SubjectScope {
    resolve_scope(snapshot, &snapshot.account_role_ids, None, account_dept_id)
}

pub(super) fn resolve_user_scope(snapshot: &AgentAuthorizationSnapshot) -> SubjectScope {
    let user = snapshot.user.as_ref();
    resolve_scope(
        snapshot,
        &snapshot.user_role_ids,
        user.map(|item| item.id),
        user.and_then(|item| item.dept_id),
    )
}

fn resolve_scope(
    snapshot: &AgentAuthorizationSnapshot,
    subject_role_ids: &[i64],
    self_user_id: Option<i64>,
    subject_dept_id: Option<i64>,
) -> SubjectScope {
    let role_ids = subject_role_ids.iter().copied().collect::<BTreeSet<_>>();
    let custom_departments = snapshot.role_departments.iter().fold(
        BTreeMap::<i64, BTreeSet<i64>>::new(),
        |mut result, row| {
            result
                .entry(row.role_id)
                .or_default()
                .insert(row.department_id);
            result
        },
    );
    let mut result = SubjectScope::default();
    for role in snapshot
        .roles
        .iter()
        .filter(|item| role_ids.contains(&item.id) && item.is_active())
    {
        if role.is_super {
            result.all = true;
            result.departments.clear();
            result.self_user_id = None;
            return result;
        }
        match role.data_scope.as_str() {
            AgentRoleSnapshot::DATA_SCOPE_ALL => {
                result.all = true;
                result.departments.clear();
                result.self_user_id = None;
                return result;
            }
            AgentRoleSnapshot::DATA_SCOPE_CUSTOM => {
                result.departments.extend(
                    custom_departments
                        .get(&role.id)
                        .into_iter()
                        .flatten()
                        .copied(),
                );
            }
            AgentRoleSnapshot::DATA_SCOPE_DEPT => {
                result.departments.extend(subject_dept_id);
            }
            AgentRoleSnapshot::DATA_SCOPE_DEPT_AND_CHILD => {
                if let Some(dept_id) = subject_dept_id {
                    result
                        .departments
                        .extend(descendant_department_ids(&snapshot.departments, dept_id));
                }
            }
            AgentRoleSnapshot::DATA_SCOPE_SELF => {
                // 服务账号没有“本人”业务行，只有用户主体可以产生 Self 范围。
                result.self_user_id = self_user_id;
            }
            _ => {}
        }
    }
    result
}

fn descendant_department_ids(departments: &[AgentDepartmentSnapshot], root_id: i64) -> Vec<i64> {
    departments
        .iter()
        .filter(|department| {
            department.id == root_id
                || department
                    .ancestors
                    .split(',')
                    .filter_map(|value| value.trim().parse::<i64>().ok())
                    .any(|ancestor| ancestor == root_id)
        })
        .map(|department| department.id)
        .collect()
}

pub(super) fn users_scope(
    account: &SubjectScope,
    user: Option<&SubjectScope>,
    represented_user_dept: Option<i64>,
) -> AgentRowScope {
    let Some(user) = user else {
        return row_scope(account);
    };
    if account.all {
        return row_scope(user);
    }
    if user.all {
        return row_scope(account);
    }
    let departments = account
        .departments
        .intersection(&user.departments)
        .copied()
        .collect::<Vec<_>>();
    let represented_user = user.self_user_id.filter(|_| {
        represented_user_dept.is_some_and(|dept_id| account.departments.contains(&dept_id))
    });
    match (departments.is_empty(), represented_user) {
        (true, None) => AgentRowScope::Empty,
        (true, Some(user_id)) => AgentRowScope::User(user_id),
        (false, None) => AgentRowScope::Departments(departments),
        (false, Some(user_id)) => AgentRowScope::DepartmentsAndUser {
            department_ids: departments,
            user_id,
        },
    }
}

pub(super) fn departments_scope(
    account: &SubjectScope,
    user: Option<&SubjectScope>,
    represented_user_dept: Option<i64>,
) -> AgentRowScope {
    let account_departments = department_set(account, None);
    let user_departments = user.map(|scope| department_set(scope, represented_user_dept));
    match (account.all, user.map(SubjectScope::is_all)) {
        (true, None | Some(true)) => AgentRowScope::All,
        (true, Some(false)) => to_department_scope(user_departments.unwrap_or_default()),
        (false, None) => to_department_scope(account_departments),
        (false, Some(true)) => to_department_scope(account_departments),
        (false, Some(false)) => to_department_scope(
            account_departments
                .intersection(&user_departments.unwrap_or_default())
                .copied()
                .collect(),
        ),
    }
}

fn row_scope(scope: &SubjectScope) -> AgentRowScope {
    if scope.all {
        return AgentRowScope::All;
    }
    let departments = scope.departments.iter().copied().collect::<Vec<_>>();
    match (departments.is_empty(), scope.self_user_id) {
        (true, None) => AgentRowScope::Empty,
        (true, Some(user_id)) => AgentRowScope::User(user_id),
        (false, None) => AgentRowScope::Departments(departments),
        (false, Some(user_id)) => AgentRowScope::DepartmentsAndUser {
            department_ids: departments,
            user_id,
        },
    }
}

fn department_set(scope: &SubjectScope, self_department: Option<i64>) -> BTreeSet<i64> {
    let mut departments = scope.departments.clone();
    if scope.self_user_id.is_some() {
        departments.extend(self_department);
    }
    departments
}

fn to_department_scope(departments: BTreeSet<i64>) -> AgentRowScope {
    if departments.is_empty() {
        AgentRowScope::Empty
    } else {
        AgentRowScope::Departments(departments.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(roles: Vec<AgentRoleSnapshot>) -> AgentAuthorizationSnapshot {
        AgentAuthorizationSnapshot {
            user: None,
            account_role_ids: roles.iter().map(|role| role.id).collect(),
            user_role_ids: Vec::new(),
            roles,
            role_permissions: Vec::new(),
            permissions: Vec::new(),
            role_departments: Vec::new(),
            departments: vec![
                AgentDepartmentSnapshot {
                    id: 10,
                    name: "总部".into(),
                    ancestors: String::new(),
                },
                AgentDepartmentSnapshot {
                    id: 11,
                    name: "研发部".into(),
                    ancestors: "10".into(),
                },
            ],
        }
    }

    #[test]
    fn deleted_super_role_does_not_grant_all_scope() {
        let snapshot = snapshot(vec![AgentRoleSnapshot {
            id: 1,
            is_super: true,
            data_scope: AgentRoleSnapshot::DATA_SCOPE_ALL.into(),
            status: AgentRoleSnapshot::STATUS_NORMAL.into(),
            deleted: true,
        }]);

        let scope = resolve_account_scope(&snapshot, Some(10));

        assert!(!scope.is_all());
        assert!(scope.departments.is_empty());
    }

    #[test]
    fn department_children_scope_uses_application_snapshot() {
        let snapshot = snapshot(vec![AgentRoleSnapshot {
            id: 1,
            is_super: false,
            data_scope: AgentRoleSnapshot::DATA_SCOPE_DEPT_AND_CHILD.into(),
            status: AgentRoleSnapshot::STATUS_NORMAL.into(),
            deleted: false,
        }]);

        let scope = resolve_account_scope(&snapshot, Some(10));

        assert_eq!(scope.departments, BTreeSet::from([10, 11]));
    }

    #[test]
    fn deleted_user_is_not_enabled() {
        let user = AgentUserSnapshot {
            id: 1,
            dept_id: None,
            status: AgentUserSnapshot::STATUS_NORMAL.into(),
            deleted: true,
            authorization_version: 1,
        };

        assert!(!user.is_enabled());
    }
}
