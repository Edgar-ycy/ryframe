use chrono::{TimeZone, Utc};
use ryframe_application::agent::AgentAccessAuditDraft;
use ryframe_db::{
    application_ports::{
        agent::audit_model,
        service_accounts::{
            account_model, account_record, credential_model, credential_write_record,
            delegation_model, delegation_write_record,
        },
    },
    entities::{service_account, service_credential, service_delegation},
};

#[test]
fn account_mapping_preserves_deleted_state() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let model = service_account::Model {
        id: 1,
        tenant_id: "tenant-a".into(),
        code: "billing".into(),
        name: "结算服务".into(),
        description: None,
        dept_id: None,
        status: service_account::Model::STATUS_DISABLED.into(),
        authorization_version: 2,
        max_requests_per_minute: 60,
        created_by: 3,
        del_flag: service_account::Model::DEL_FLAG_DELETED.into(),
        created_at: now,
        updated_at: now,
    };

    let record = account_record(model);
    assert!(record.deleted);
    assert_eq!(
        account_model(record).del_flag,
        service_account::Model::DEL_FLAG_DELETED
    );
}

#[test]
fn credential_mapping_preserves_secret_metadata() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let model = service_credential::Model {
        id: 1,
        tenant_id: "tenant-a".into(),
        account_id: 2,
        key_id: "key-a".into(),
        secret_mac: vec![1, 2],
        pepper_version: 3,
        label: "自动化".into(),
        status: service_credential::Model::STATUS_ACTIVE.into(),
        expires_at: now,
        last_used_at: None,
        created_by: 4,
        revoked_at: None,
        revoked_by: None,
        created_at: now,
        updated_at: now,
        idempotency_key_hash: vec![5, 6],
        request_fingerprint: vec![7, 8],
    };

    let record = credential_write_record(model);
    assert_eq!(record.secret_mac, [1, 2]);
    assert_eq!(record.request_fingerprint, [7, 8]);
    let restored = credential_model(record);
    assert_eq!(restored.pepper_version, 3);
    assert_eq!(restored.idempotency_key_hash, [5, 6]);
}

#[test]
fn delegation_mapping_preserves_capabilities_without_copying() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let model = service_delegation::Model {
        id: 1,
        tenant_id: "tenant-a".into(),
        account_id: 2,
        user_id: 3,
        token_mac: vec![1, 2],
        pepper_version: 4,
        status: service_delegation::Model::STATUS_ACTIVE.into(),
        version: 1,
        not_before: now,
        expires_at: now,
        reason: "排障".into(),
        created_by_user_id: 3,
        revoked_at: None,
        revoked_by: None,
        created_at: now,
        updated_at: now,
        idempotency_key_hash: vec![5, 6],
        request_fingerprint: vec![7, 8],
    };

    let record = delegation_write_record(model, vec!["system:user:list".into()]);
    assert_eq!(record.capability_keys, ["system:user:list"]);
    let mut record = record;
    let capability_keys = std::mem::take(&mut record.capability_keys);
    let restored = delegation_model(record);
    assert_eq!(restored.token_mac, [1, 2]);
    assert_eq!(capability_keys, ["system:user:list"]);
}

#[test]
fn account_mapping_preserves_read_state() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let record = account_record(service_account::Model {
        id: 1,
        tenant_id: "tenant-a".into(),
        code: "billing".into(),
        name: "结算服务".into(),
        description: None,
        dept_id: Some(2),
        status: service_account::Model::STATUS_NORMAL.into(),
        authorization_version: 3,
        max_requests_per_minute: 60,
        created_by: 4,
        del_flag: service_account::Model::DEL_FLAG_NORMAL.into(),
        created_at: now,
        updated_at: now,
    });

    assert!(record.is_enabled());
    assert_eq!(record.dept_id, Some(2));
    assert_eq!(record.authorization_version, 3);
}

#[test]
fn audit_mapping_preserves_security_metadata() {
    let started_at = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let completed_at = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 4).unwrap();
    let record = AgentAccessAuditDraft {
        id: 1,
        request_id: "request-a".into(),
        tenant_id: Some("tenant-a".into()),
        account_id: Some(2),
        credential_id: Some(3),
        delegation_id: Some(4),
        represented_user_id: Some(5),
        operation_id: "agent.users".into(),
        capability_key: "directory.users".into(),
        required_permission: "system:user:list".into(),
        access_mode: "delegated".into(),
        result: "success".into(),
        reason_code: "ok".into(),
        http_status: 200,
        request_ip_digest: Some(vec![6, 7]),
        user_agent_digest: Some(vec![8, 9]),
        row_count: Some(10),
        response_bytes: Some(11),
        tenant_epoch: Some(12),
        account_authorization_version: Some(13),
        user_authorization_version: Some(14),
        delegation_version: Some(15),
        started_at,
    }
    .complete(completed_at);

    let model = audit_model(record);
    assert_eq!(model.request_ip_digest, Some(vec![6, 7]));
    assert_eq!(model.user_agent_digest, Some(vec![8, 9]));
    assert_eq!(model.completed_at, completed_at);
    assert_eq!(model.delegation_version, Some(15));
}
