use crate::{AgentRowScope as DatabaseAgentRowScope, ServiceAuthorizationSnapshot};

use ryframe_application::agent::{
    AgentAuthorizationSnapshot, AgentDepartmentSnapshot, AgentPermissionSnapshot,
    AgentRoleDepartmentSnapshot, AgentRolePermissionSnapshot, AgentRoleSnapshot, AgentRowScope,
    AgentUserSnapshot,
};

pub(crate) fn authorization_snapshot(
    snapshot: ServiceAuthorizationSnapshot,
) -> AgentAuthorizationSnapshot {
    AgentAuthorizationSnapshot {
        user: snapshot.user.map(|user| AgentUserSnapshot {
            id: user.id,
            dept_id: user.dept_id,
            status: user.status,
            deleted: user.del_flag != crate::user::Model::DEL_FLAG_NORMAL,
            authorization_version: user.authorization_version,
        }),
        account_role_ids: snapshot.account_role_ids,
        user_role_ids: snapshot.user_role_ids,
        roles: snapshot
            .roles
            .into_iter()
            .map(|role| AgentRoleSnapshot {
                id: role.id,
                is_super: role.is_super != 0,
                data_scope: role.data_scope,
                status: role.status,
                deleted: role.del_flag != crate::role::Model::DEL_FLAG_NORMAL,
            })
            .collect(),
        role_permissions: snapshot
            .role_permissions
            .into_iter()
            .map(|relation| AgentRolePermissionSnapshot {
                role_id: relation.role_id,
                permission_id: relation.perm_id,
            })
            .collect(),
        permissions: snapshot
            .permissions
            .into_iter()
            .map(|permission| AgentPermissionSnapshot {
                id: permission.id,
                code: permission.code,
                status: permission.status,
            })
            .collect(),
        role_departments: snapshot
            .role_departments
            .into_iter()
            .map(|relation| AgentRoleDepartmentSnapshot {
                role_id: relation.role_id,
                department_id: relation.dept_id,
            })
            .collect(),
        departments: snapshot
            .departments
            .into_iter()
            .map(|department| AgentDepartmentSnapshot {
                id: department.id,
                name: department.name,
                ancestors: department.ancestors,
            })
            .collect(),
    }
}

pub(crate) fn row_scope(scope: AgentRowScope) -> DatabaseAgentRowScope {
    match scope {
        AgentRowScope::All => DatabaseAgentRowScope::All,
        AgentRowScope::Departments(ids) => DatabaseAgentRowScope::Departments(ids),
        AgentRowScope::DepartmentsAndUser {
            department_ids,
            user_id,
        } => DatabaseAgentRowScope::DepartmentsAndUser {
            department_ids,
            user_id,
        },
        AgentRowScope::User(user_id) => DatabaseAgentRowScope::User(user_id),
        AgentRowScope::Empty => DatabaseAgentRowScope::Empty,
    }
}
