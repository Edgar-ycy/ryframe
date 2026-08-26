use chrono::{DateTime, Utc};
use ryframe_application::ports::tenants::{ProvisionTenantRecord, TenantProvisioningIdentity};
use ryframe_kernel::AppResult;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseTransaction};

use super::TenantProvisioningRepository;
use crate::{
    DbResultExt,
    entities::{
        role,
        tenant::{self, provision_request as tenant_provision_request},
        user, user_role,
    },
    repositories::cache_namespace_version_repo::{
        CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository,
    },
};

impl TenantProvisioningRepository {
    pub async fn initialize_identity_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        record: &ProvisionTenantRecord,
    ) -> AppResult<TenantProvisioningIdentity> {
        let now = Utc::now();
        insert_tenant_and_request(transaction, record, now).await?;
        CacheNamespaceVersionRepository
            .insert_initial_in_transaction(
                transaction,
                &record.tenant_id,
                CONFIG_CACHE_NAMESPACE,
                now,
            )
            .await?;
        let ids = IdentityIds::allocate()?;
        insert_default_roles(transaction, &record.tenant_id, &ids, now).await?;
        insert_admin_user(transaction, record, &ids, now).await?;
        Ok(TenantProvisioningIdentity {
            tenant_id: record.tenant_id.clone(),
            admin_role_id: ids.admin_role_id,
            user_role_id: ids.user_role_id,
            provisioned_at: now,
        })
    }
}

struct IdentityIds {
    admin_role_id: i64,
    user_role_id: i64,
    admin_user_id: i64,
}

impl IdentityIds {
    fn allocate() -> AppResult<Self> {
        Ok(Self {
            admin_role_id: crate::next_id()?,
            user_role_id: crate::next_id()?,
            admin_user_id: crate::next_id()?,
        })
    }
}

async fn insert_tenant_and_request(
    transaction: &DatabaseTransaction,
    record: &ProvisionTenantRecord,
    now: DateTime<Utc>,
) -> AppResult<()> {
    tenant::ActiveModel {
        id: ActiveValue::Set(crate::next_id()?),
        tenant_id: ActiveValue::Set(record.tenant_id.clone()),
        name: ActiveValue::Set(record.name.clone()),
        domain: ActiveValue::Set(record.domain.clone()),
        status: ActiveValue::Set(tenant::Model::STATUS_PROVISIONING.to_owned()),
        expire_at: ActiveValue::Set(record.expire_at),
        max_users: ActiveValue::Set(record.max_users),
        max_roles: ActiveValue::Set(record.max_roles),
        max_storage_mb: ActiveValue::Set(record.max_storage_mb),
        max_requests_per_min: ActiveValue::Set(record.max_requests_per_minute),
        session_version: ActiveValue::Set(1),
        authorization_epoch: ActiveValue::Set(1),
        runtime_epoch: ActiveValue::Set(1),
        configuration_version: ActiveValue::Set(0),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(transaction)
    .await
    .db()?;
    tenant_provision_request::ActiveModel {
        tenant_id: ActiveValue::Set(record.tenant_id.clone()),
        request_token: ActiveValue::Set(record.provisioning_request_token.clone()),
        admin_password_hash: ActiveValue::Set(record.admin_password_hash.clone()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(transaction)
    .await
    .db()?;
    Ok(())
}

async fn insert_default_roles(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    ids: &IdentityIds,
    now: DateTime<Utc>,
) -> AppResult<()> {
    default_role(
        tenant_id,
        ids.admin_role_id,
        DefaultRoleSpec {
            name: "租户管理员",
            code: "tenant_admin",
            data_scope: role::Model::DATA_SCOPE_ALL,
            sort: 1,
            remark: "创建租户时自动初始化",
        },
        now,
    )
    .insert(transaction)
    .await
    .db()?;
    default_role(
        tenant_id,
        ids.user_role_id,
        DefaultRoleSpec {
            name: "租户普通用户",
            code: "tenant_user",
            data_scope: role::Model::DATA_SCOPE_SELF,
            sort: 0,
            remark: "租户初始化的只读角色",
        },
        now,
    )
    .insert(transaction)
    .await
    .db()?;
    Ok(())
}

struct DefaultRoleSpec {
    name: &'static str,
    code: &'static str,
    data_scope: &'static str,
    sort: i32,
    remark: &'static str,
}

fn default_role(
    tenant_id: &str,
    id: i64,
    spec: DefaultRoleSpec,
    now: DateTime<Utc>,
) -> role::ActiveModel {
    role::ActiveModel {
        id: ActiveValue::Set(id),
        tenant_id: ActiveValue::Set(tenant_id.to_owned()),
        name: ActiveValue::Set(spec.name.to_owned()),
        code: ActiveValue::Set(spec.code.to_owned()),
        is_super: ActiveValue::Set(0),
        data_scope: ActiveValue::Set(spec.data_scope.to_owned()),
        status: ActiveValue::Set(role::Model::STATUS_NORMAL.to_owned()),
        sort: ActiveValue::Set(spec.sort),
        remark: ActiveValue::Set(Some(spec.remark.to_owned())),
        del_flag: ActiveValue::Set(role::Model::DEL_FLAG_NORMAL.to_owned()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
}

async fn insert_admin_user(
    transaction: &DatabaseTransaction,
    record: &ProvisionTenantRecord,
    ids: &IdentityIds,
    now: DateTime<Utc>,
) -> AppResult<()> {
    user::ActiveModel {
        id: ActiveValue::Set(ids.admin_user_id),
        tenant_id: ActiveValue::Set(record.tenant_id.clone()),
        username: ActiveValue::Set(record.admin_username.clone()),
        password_hash: ActiveValue::Set(record.admin_password_hash.clone()),
        nickname: ActiveValue::Set("租户管理员".into()),
        email: ActiveValue::Set(String::new()),
        phone: ActiveValue::Set(String::new()),
        avatar: ActiveValue::Set(None),
        avatar_file_id: ActiveValue::Set(None),
        preferred_locale: ActiveValue::Set(None),
        status: ActiveValue::Set(user::Model::STATUS_NORMAL.into()),
        authorization_version: ActiveValue::Set(1),
        dept_id: ActiveValue::Set(None),
        remark: ActiveValue::Set(None),
        login_ip: ActiveValue::Set(None),
        login_date: ActiveValue::Set(None),
        del_flag: ActiveValue::Set(user::Model::DEL_FLAG_NORMAL.into()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(transaction)
    .await
    .db()?;
    user_role::ActiveModel {
        tenant_id: ActiveValue::Set(record.tenant_id.clone()),
        user_id: ActiveValue::Set(ids.admin_user_id),
        role_id: ActiveValue::Set(ids.admin_role_id),
    }
    .insert(transaction)
    .await
    .db()?;
    Ok(())
}
