use std::{collections::HashMap, sync::Arc};

use crate::{
    ControlDatabaseCluster, ReadConsistency, Repository, ServiceAccountRepository,
    ServiceCredentialRepository, ServiceDelegationRepository,
    entities::{
        service_account, service_credential, service_delegation, service_delegation_capability,
    },
};
use ryframe_kernel::{AppError, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect,
};

use super::account_record;

use ryframe_application::{
    PersistenceFuture,
    ports::service_accounts::{
        ServiceAccountDetailRecord, ServiceAccountReadPort, ServiceAccountRecord,
        ServiceCredentialRecord, ServiceDelegationRecord,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ServiceAccountReadPort> {
    Arc::new(DatabaseServiceAccountRead { database })
}

struct DatabaseServiceAccountRead {
    database: ControlDatabaseCluster,
}

impl ServiceAccountReadPort for DatabaseServiceAccountRead {
    fn list_accounts<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<ServiceAccountRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            let result = ServiceAccountRepository
                .find_by_page(&database, tenant_id, page)
                .await?;
            Ok(PageResult {
                records: result.records.into_iter().map(account_record).collect(),
                total: result.total,
                page: result.page,
                page_size: result.page_size,
            })
        })
    }

    fn account_detail<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountDetailRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let Some(account) = ServiceAccountRepository
                .find_by_id(&database, tenant_id, account_id)
                .await?
            else {
                return Ok(None);
            };
            let role_ids = ServiceAccountRepository
                .role_ids(&database, tenant_id, account_id)
                .await?;
            Ok(Some(ServiceAccountDetailRecord {
                account: account_record(account),
                role_ids,
            }))
        })
    }

    fn enabled_account_role_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<Vec<i64>>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let Some(account) = ServiceAccountRepository
                .find_by_id(&database, tenant_id, account_id)
                .await?
                .filter(service_account::Model::is_enabled)
            else {
                return Ok(None);
            };
            let role_ids = ServiceAccountRepository
                .role_ids(&database, tenant_id, account.id)
                .await?;
            Ok(Some(role_ids))
        })
    }

    fn enabled_account_credentials<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<Vec<ServiceCredentialRecord>>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let enabled = ServiceAccountRepository
                .find_by_id(&database, tenant_id, account_id)
                .await?
                .is_some_and(|account| account.is_enabled());
            if !enabled {
                return Ok(None);
            }
            let credentials = ServiceCredentialRepository
                .list_for_account(&database, tenant_id, account_id)
                .await?
                .into_iter()
                .map(credential_record)
                .collect();
            Ok(Some(credentials))
        })
    }

    fn delegations_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Vec<ServiceDelegationRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let rows = ServiceDelegationRepository
                .list_for_user(&database, tenant_id, user_id)
                .await?;
            delegations_with_capabilities(&database, rows).await
        })
    }

    fn list_delegations<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<ServiceDelegationRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let total = service_delegation::Entity::find()
                .filter(service_delegation::Column::TenantId.eq(tenant_id));
            let total = total.count(&database).await.map_err(database_error)?;
            let rows = service_delegation::Entity::find()
                .filter(service_delegation::Column::TenantId.eq(tenant_id))
                .order_by_desc(service_delegation::Column::CreatedAt)
                .offset(page.offset())
                .limit(page.page_size())
                .all(&database)
                .await
                .map_err(database_error)?;
            let records = delegations_with_capabilities(&database, rows).await?;
            Ok(PageResult::new(records, total, &page))
        })
    }
}

async fn delegations_with_capabilities(
    database: &DatabaseConnection,
    rows: Vec<service_delegation::Model>,
) -> ryframe_kernel::AppResult<Vec<ServiceDelegationRecord>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let delegation_ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let capabilities = service_delegation_capability::Entity::find()
        .filter(service_delegation_capability::Column::TenantId.eq(&rows[0].tenant_id))
        .filter(
            service_delegation_capability::Column::DelegationId
                .is_in(delegation_ids.iter().copied()),
        )
        .order_by_asc(service_delegation_capability::Column::DelegationId)
        .order_by_asc(service_delegation_capability::Column::CapabilityKey)
        .all(database)
        .await
        .map_err(database_error)?;
    let mut by_delegation = HashMap::<i64, Vec<String>>::new();
    for capability in capabilities {
        by_delegation
            .entry(capability.delegation_id)
            .or_default()
            .push(capability.capability_key);
    }
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let keys = by_delegation.remove(&row.id).unwrap_or_default();
        result.push(delegation_record(row, keys));
    }
    Ok(result)
}

fn credential_record(credential: service_credential::Model) -> ServiceCredentialRecord {
    ServiceCredentialRecord {
        id: credential.id,
        account_id: credential.account_id,
        key_id: credential.key_id,
        label: credential.label,
        status: credential.status,
        expires_at: credential.expires_at,
        last_used_at: credential.last_used_at,
        revoked_at: credential.revoked_at,
        created_at: credential.created_at,
    }
}

fn delegation_record(
    delegation: service_delegation::Model,
    capability_keys: Vec<String>,
) -> ServiceDelegationRecord {
    ServiceDelegationRecord {
        id: delegation.id,
        account_id: delegation.account_id,
        user_id: delegation.user_id,
        status: delegation.status,
        version: delegation.version,
        not_before: delegation.not_before,
        expires_at: delegation.expires_at,
        reason: delegation.reason,
        capability_keys,
        revoked_at: delegation.revoked_at,
        created_at: delegation.created_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
