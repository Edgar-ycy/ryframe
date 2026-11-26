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
) -> AppResult<String> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(params.idempotency_key.as_bytes())
        .expect("HMAC accepts arbitrary Idempotency-Key lengths");
    mac.update(b"ryframe:tenant-provisioning:v3\0");
    update_fingerprint_field(&mut mac, params.tenant_id.as_bytes());
    update_fingerprint_field(&mut mac, params.name.as_bytes());
    update_optional_fingerprint_field(&mut mac, params.domain.as_deref());
    match params.expire_at {
        Some(value) => {
            mac.update(&[1]);
            let micros = i64::try_from(value.unix_timestamp_nanos().div_euclid(1_000))
                .map_err(|_| AppError::Validation("租户到期时间超出创建请求支持的范围".into()))?;
            mac.update(&micros.to_be_bytes());
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
    Ok(hex::encode(mac.finalize().into_bytes()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::{OffsetDateTime, UtcOffset};

    #[test]
    fn provisioning_v3_fingerprint_preserves_original_utc_microsecond_bytes() {
        let mut params = CreateTenantParams {
            idempotency_key: "tenant-expiry-test-key".into(),
            tenant_id: "tenant-a".into(),
            name: "Tenant A".into(),
            domain: Some("tenant.example.com".into()),
            expire_at: None,
            max_users: Some(100),
            max_roles: Some(20),
            max_storage_mb: Some(1024),
            max_requests_per_min: Some(1000),
            admin_username: "admin".into(),
            admin_password: "Fixture-A1!".into(),
            plan_version_id: 42,
            data_target_key: "primary".into(),
        };
        let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
        // 固定向量按旧版 HMAC 输入与 Chrono 微秒编码生成。
        for (nanos, expected) in [
            (
                None,
                "9115263352616fa82c97b8c5038c4a6ca024c8c0c03312e306227e7314694c91",
            ),
            (
                Some(-1),
                "a979b6700fff535db8a3a59c169dd0132ed593ae62740699e9a5536505893852",
            ),
            (
                Some(-1_999),
                "8d47b754418f52fcd4eb0de3f8d5aa814f2aefe66e7175a151e6dc5bf35dbfea",
            ),
            (
                Some(123_456_789),
                "3cae1c9436d4e1dc83ad102486f9fa40c094f5fcabca1282338421bfb8b61d0e",
            ),
        ] {
            params.expire_at =
                nanos.map(|value| OffsetDateTime::from_unix_timestamp_nanos(value).unwrap());
            assert_eq!(
                provisioning_switch_token(&params, 100, 20, 1024, 1000).unwrap(),
                expected
            );
            params.expire_at = params.expire_at.map(|value| value.to_offset(offset));
            assert_eq!(
                provisioning_switch_token(&params, 100, 20, 1024, 1000).unwrap(),
                expected
            );
        }
    }
}
