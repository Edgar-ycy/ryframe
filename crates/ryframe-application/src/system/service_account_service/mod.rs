use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Duration, Utc};
use ryframe_adapters::snowflake;
use ryframe_adapters::{PageResult, Repository, ValidatedPageQuery};
use ryframe_config::{PepperKeyring, ServiceAccountsConfig};
use ryframe_db::{
    ControlDatabaseCluster, DataRetentionRepository, PermissionRepository, ReadConsistency,
    RoleRepository, ServiceAccountLock, ServiceAccountRepository, ServiceCredentialRepository,
    ServiceDelegationRepository, UserRepository,
    entities::{
        role, service_access_audit, service_account, service_account_role, service_credential,
        service_delegation, service_delegation_capability,
    },
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, TransactionTrait,
    sea_query::{Expr, LockType},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    AuthorizationCache,
    service_identity_secret::{IssuedApiKey, IssuedDelegationToken},
};

const IDEMPOTENCY_DOMAIN: &[u8] = b"ryframe/idempotency-key/v1\0";
const FINGERPRINT_DOMAIN: &[u8] = b"ryframe/request-fingerprint/v1\0";

mod accounts;
mod audits;
mod capabilities;
mod credentials;
mod delegations;
mod model;
mod roles;
mod support;
mod validation;

use capabilities::*;
pub use model::*;
use validation::*;
pub struct ServiceAccountService {
    db: ControlDatabaseCluster,
    config: ServiceAccountsConfig,
    keyring: Arc<PepperKeyring>,
    capabilities: Vec<ServiceCapabilityDescriptor>,
    account_repo: ServiceAccountRepository,
    credential_repo: ServiceCredentialRepository,
    delegation_repo: ServiceDelegationRepository,
    role_repo: RoleRepository,
    permission_repo: PermissionRepository,
    user_repo: UserRepository,
    authorization_cache: AuthorizationCache,
}

impl ServiceAccountService {
    pub fn new(
        db: ControlDatabaseCluster,
        config: ServiceAccountsConfig,
        keyring: Arc<PepperKeyring>,
        capabilities: Vec<ServiceCapabilityDescriptor>,
        authorization_cache: AuthorizationCache,
    ) -> AppResult<Self> {
        config.validate().map_err(AppError::Config)?;
        validate_capabilities(&capabilities)?;
        Ok(Self {
            db,
            config,
            keyring,
            capabilities,
            account_repo: ServiceAccountRepository,
            credential_repo: ServiceCredentialRepository,
            delegation_repo: ServiceDelegationRepository,
            role_repo: RoleRepository,
            permission_repo: PermissionRepository,
            user_repo: UserRepository,
            authorization_cache,
        })
    }

    pub(super) fn ensure_enabled(&self) -> AppResult<()> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(AppError::ServiceUnavailable("服务账号功能未启用".into()))
        }
    }
}
