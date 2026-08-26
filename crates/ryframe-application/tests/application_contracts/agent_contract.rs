use std::collections::BTreeSet;

use chrono::{DateTime, Duration, TimeZone, Utc};
use ryframe_application::agent::{
    AgentAccountRecord, AgentAuthorizationSnapshot, AgentCredentialRecord, AgentDelegationRecord,
    AgentDepartmentSnapshot, AgentLimitHints, AgentRoleSnapshot, AgentRowScope, AgentTenantRecord,
    AgentUserSnapshot, resolve_account_row_scope,
};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 21, 8, 0, 0).unwrap()
}

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
fn tenant_expiration_is_fail_closed_at_boundary() {
    let current = now();
    let tenant = AgentTenantRecord {
        tenant_id: "tenant-a".into(),
        status: "enabled".into(),
        expire_at: Some(current),
        authorization_epoch: 1,
    };
    assert!(!tenant.is_available(current));
}

#[test]
fn deleted_account_is_not_enabled() {
    let account = AgentAccountRecord {
        id: 1,
        tenant_id: "tenant-a".into(),
        dept_id: None,
        status: "1".into(),
        deleted: true,
        authorization_version: 1,
    };
    assert!(!account.is_enabled());
}

#[test]
fn revoked_credential_is_not_usable() {
    let current = now();
    let credential = AgentCredentialRecord {
        id: 1,
        tenant_id: "tenant-a".into(),
        account_id: 2,
        key_id: "key-a".into(),
        secret_mac: vec![1],
        pepper_version: 1,
        status: "active".into(),
        expires_at: current + Duration::minutes(1),
        revoked_at: Some(current),
    };
    assert!(!credential.is_usable_at(current));
}

#[test]
fn delegation_time_window_is_enforced() {
    let current = now();
    let delegation = AgentDelegationRecord {
        id: 1,
        tenant_id: "tenant-a".into(),
        account_id: 2,
        user_id: 3,
        token_mac: vec![1],
        pepper_version: 1,
        status: "active".into(),
        version: 1,
        not_before: current + Duration::seconds(1),
        expires_at: current + Duration::minutes(1),
        revoked_at: None,
        capability_keys: BTreeSet::new(),
    };
    assert!(!delegation.is_usable_at(current));
    assert!(delegation.is_usable_at(current + Duration::seconds(1)));
    assert!(!delegation.is_usable_at(current + Duration::minutes(1)));
}

#[test]
fn deleted_super_role_does_not_grant_all_scope() {
    let authorization = snapshot(vec![AgentRoleSnapshot {
        id: 1,
        is_super: true,
        data_scope: "1".into(),
        status: "1".into(),
        deleted: true,
    }]);
    assert_eq!(
        resolve_account_row_scope(&authorization, Some(10)),
        AgentRowScope::Empty
    );
}

#[test]
fn department_children_scope_uses_application_snapshot() {
    let authorization = snapshot(vec![AgentRoleSnapshot {
        id: 1,
        is_super: false,
        data_scope: "4".into(),
        status: "1".into(),
        deleted: false,
    }]);
    assert_eq!(
        resolve_account_row_scope(&authorization, Some(10)),
        AgentRowScope::Departments(vec![10, 11])
    );
}

#[test]
fn deleted_user_is_not_enabled() {
    let user = AgentUserSnapshot {
        id: 1,
        dept_id: None,
        status: "1".into(),
        deleted: true,
        authorization_version: 1,
    };
    assert!(!user.is_enabled());
}

#[test]
fn effective_limits_use_bounded_default_or_account_override() {
    assert_eq!(
        AgentLimitHints {
            tenant_limit: 120,
            account_limit: None,
        }
        .effective_limits(u32::MAX),
        (120, i32::MAX)
    );
    assert_eq!(
        AgentLimitHints {
            tenant_limit: 120,
            account_limit: Some(30),
        }
        .effective_limits(60),
        (120, 30)
    );
}
