use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{ActorContext, AppError, AppResult, PageResult, ValidatedPageQuery};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    AuthorizationCache, PepperKeyring, ServiceAccountPolicy,
    ports::authorization::AuthorizationMirrorTransaction,
    ports::service_accounts::{
        ServiceAccountAuditReadPort, ServiceAccountAuthorizationReadPort, ServiceAccountReadPort,
        ServiceAccountRecord, ServiceAccountWritePort, ServiceAccountWriteTransaction,
        ServiceCredentialRecord, ServiceCredentialWriteRecord, ServiceDelegationRecord,
        ServiceDelegationWriteRecord,
    },
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

pub use capabilities::common_capabilities;
use capabilities::{validate_capabilities, validate_dept};
pub use model::*;
use validation::*;

pub struct ServiceAccountReadDependencies {
    pub accounts: Arc<dyn ServiceAccountReadPort>,
    pub authorization: Arc<dyn ServiceAccountAuthorizationReadPort>,
    pub audits: Arc<dyn ServiceAccountAuditReadPort>,
}

pub struct ServiceAccountService {
    config: ServiceAccountPolicy,
    keyring: Arc<PepperKeyring>,
    capabilities: Vec<ServiceCapabilityDescriptor>,
    read: Arc<dyn ServiceAccountReadPort>,
    write: Arc<dyn ServiceAccountWritePort>,
    authorization_cache: AuthorizationCache,
    authorization_read: Arc<dyn ServiceAccountAuthorizationReadPort>,
    audit_read: Arc<dyn ServiceAccountAuditReadPort>,
}

impl ServiceAccountService {
    pub fn new(
        write: Arc<dyn ServiceAccountWritePort>,
        config: ServiceAccountPolicy,
        keyring: Arc<PepperKeyring>,
        capabilities: Vec<ServiceCapabilityDescriptor>,
        authorization_cache: AuthorizationCache,
        reads: ServiceAccountReadDependencies,
    ) -> AppResult<Self> {
        validate_capabilities(&capabilities)?;
        Ok(Self {
            config,
            keyring,
            capabilities,
            read: reads.accounts,
            write,
            authorization_cache,
            authorization_read: reads.authorization,
            audit_read: reads.audits,
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
