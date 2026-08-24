use hmac::{Hmac, KeyInit, Mac};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sha2::Sha256;

use super::{CreateTenantParams, SYSTEM_TENANT_ID};

const MIN_TENANT_USERS: i32 = 1;
const MIN_TENANT_ROLES: i32 = 2;

pub(super) fn ensure_platform_admin(actor: &ActorContext) -> AppResult<()> {
    crate::validated_tenant_id(actor)?;
    if actor.tenant_id != SYSTEM_TENANT_ID {
        return Err(AppError::Authorization(
            "仅 system 租户中已获相应 RBAC 权限的操作员可以管理租户".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_tenant_limits(
    max_users: i32,
    max_roles: i32,
    max_storage_mb: i64,
    max_requests_per_min: i32,
) -> AppResult<()> {
    if max_users != 0 && max_users < MIN_TENANT_USERS {
        return Err(AppError::Validation(format!(
            "用户额度不能低于 {MIN_TENANT_USERS}"
        )));
    }
    if max_roles != 0 && max_roles < MIN_TENANT_ROLES {
        return Err(AppError::Validation(format!(
            "角色额度不能低于 {MIN_TENANT_ROLES}"
        )));
    }
    if max_storage_mb < 0 {
        return Err(AppError::Validation("存储额度不能为负数".into()));
    }
    if max_requests_per_min < 0 {
        return Err(AppError::Validation("每分钟请求额度不能为负数".into()));
    }
    Ok(())
}

pub(super) fn validate_data_target_key(value: &str) -> AppResult<()> {
    if value == value.trim() && crate::runtime_policy::is_valid_tenant_target_key(value) {
        Ok(())
    } else {
        Err(AppError::Validation(
            "data_target_key 必须为 2–64 位 ASCII 字母、数字、下划线或连字符，且首尾必须是字母或数字"
                .into(),
        ))
    }
}

pub(super) fn validate_idempotency_key(value: &str) -> AppResult<()> {
    if (16..=128).contains(&value.len()) && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        Ok(())
    } else {
        Err(AppError::Validation(
            "Idempotency-Key 必须为 16 到 128 个可见 ASCII 字符".into(),
        ))
    }
}

pub(super) fn provisioning_switch_token(
    params: &CreateTenantParams,
    max_users: i32,
    max_roles: i32,
    max_storage_mb: i64,
    max_requests_per_minute: i32,
) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(params.idempotency_key.as_bytes())
        .expect("HMAC accepts arbitrary Idempotency-Key lengths");
    mac.update(b"ryframe:tenant-provisioning:v3\0");
    update_fingerprint_field(&mut mac, params.tenant_id.as_bytes());
    update_fingerprint_field(&mut mac, params.name.as_bytes());
    update_optional_fingerprint_field(&mut mac, params.domain.as_deref());
    match params.expire_at {
        Some(value) => {
            mac.update(&[1]);
            mac.update(&value.timestamp_micros().to_be_bytes());
        }
        None => mac.update(&[0]),
    }
    mac.update(&max_users.to_be_bytes());
    mac.update(&max_roles.to_be_bytes());
    mac.update(&max_storage_mb.to_be_bytes());
    mac.update(&max_requests_per_minute.to_be_bytes());
    update_fingerprint_field(&mut mac, params.admin_username.as_bytes());
    mac.update(&params.plan_version_id.to_be_bytes());
    update_fingerprint_field(&mut mac, params.data_target_key.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn update_optional_fingerprint_field(mac: &mut Hmac<Sha256>, value: Option<&str>) {
    match value {
        Some(value) => {
            mac.update(&[1]);
            update_fingerprint_field(mac, value.as_bytes());
        }
        None => mac.update(&[0]),
    }
}

fn update_fingerprint_field(mac: &mut Hmac<Sha256>, value: &[u8]) {
    mac.update(&(value.len() as u64).to_be_bytes());
    mac.update(value);
}
