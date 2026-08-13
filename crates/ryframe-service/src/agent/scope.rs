use std::collections::{BTreeMap, BTreeSet};

use ryframe_db::{
    AgentRowScope, ServiceAuthorizationSnapshot,
    entities::{dept, role},
};

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
    snapshot: &ServiceAuthorizationSnapshot,
    account_dept_id: Option<i64>,
) -> SubjectScope {
    resolve_scope(snapshot, &snapshot.account_role_ids, None, account_dept_id)
}

pub(super) fn resolve_user_scope(snapshot: &ServiceAuthorizationSnapshot) -> SubjectScope {
    let user = snapshot.user.as_ref();
    resolve_scope(
        snapshot,
        &snapshot.user_role_ids,
        user.map(|item| item.id),
        user.and_then(|item| item.dept_id),
    )
}

fn resolve_scope(
    snapshot: &ServiceAuthorizationSnapshot,
    subject_role_ids: &[i64],
    self_user_id: Option<i64>,
    subject_dept_id: Option<i64>,
) -> SubjectScope {
    let role_ids = subject_role_ids.iter().copied().collect::<BTreeSet<_>>();
    let custom_departments = snapshot.role_departments.iter().fold(
        BTreeMap::<i64, BTreeSet<i64>>::new(),
        |mut result, row| {
            result.entry(row.role_id).or_default().insert(row.dept_id);
            result
        },
    );
    let mut result = SubjectScope::default();
    for role in snapshot.roles.iter().filter(|item| {
        role_ids.contains(&item.id)
            && item.status == role::Model::STATUS_NORMAL
            && item.del_flag == role::Model::DEL_FLAG_NORMAL
    }) {
        if role.is_super != 0 {
            result.all = true;
            result.departments.clear();
            result.self_user_id = None;
            return result;
        }
        match role.data_scope.as_str() {
            role::Model::DATA_SCOPE_ALL => {
                result.all = true;
                result.departments.clear();
                result.self_user_id = None;
                return result;
            }
            role::Model::DATA_SCOPE_CUSTOM => {
                result.departments.extend(
                    custom_departments
                        .get(&role.id)
                        .into_iter()
                        .flatten()
                        .copied(),
                );
            }
            role::Model::DATA_SCOPE_DEPT => {
                result.departments.extend(subject_dept_id);
            }
            role::Model::DATA_SCOPE_DEPT_AND_CHILD => {
                if let Some(dept_id) = subject_dept_id {
                    result
                        .departments
                        .extend(descendant_department_ids(&snapshot.departments, dept_id));
                }
            }
            role::Model::DATA_SCOPE_SELF => {
                // 服务账号没有“本人”业务行，只有用户主体可以产生 Self 范围。
                result.self_user_id = self_user_id;
            }
            _ => {}
        }
    }
    result
}

fn descendant_department_ids(departments: &[dept::Model], root_id: i64) -> Vec<i64> {
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
