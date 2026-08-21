use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::next_id;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{ActorContext, AppError, AppResult, PageResult, ValidatedPageQuery};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::tenant_config_package::TENANT_CONFIG_PACKAGE_SCHEMA;
use super::{
    CapabilityRequirement, DownloadedFile, FileService, ParsedTenantConfigPackage, ProductService,
    TenantConfigPackageLimits, TenantConfigPackageResources, TenantConfigPackageSource,
    UploadPolicy, UserService, parse_tenant_config_package,
};
use crate::{
    AuthorizationCache, ClaimedBackgroundJob, EnqueueJob, JobHandler, JobQueue,
    ports::tenant_config::{
        TenantConfigArchivePort, TenantConfigBundleRecord, TenantConfigOperationLeaseRecord,
        TenantConfigRequesterRecord, TenantConfigTransferItemRecord,
        TenantConfigTransferPersistencePort, TenantConfigTransferRecord,
        TenantConfigTransferTransaction, TenantConfigurationFenceRecord,
    },
};

mod apply_workflow;
mod export_filter;
mod export_workflow;
mod job_handlers;
mod lifecycle;
mod model;
mod plan;
mod preview_workflow;
mod queries;
mod requests;
mod rollback_workflow;
mod validation;

use crate::tenant_config_stable_key::*;
use export_filter::*;
pub use job_handlers::*;
pub use model::*;
#[doc(hidden)]
pub use plan::compare_resources;
use plan::*;
use requests::TransferOperationRequest;
use validation::*;

const CONFIG_CACHE_NAMESPACE: &str = "config";
pub const TENANT_CONFIG_EXPORT_JOB_TYPE: &str = "system.tenant_config.export";
pub const TENANT_CONFIG_PREVIEW_JOB_TYPE: &str = "system.tenant_config.preview";
pub const TENANT_CONFIG_APPLY_JOB_TYPE: &str = "system.tenant_config.apply";
pub const TENANT_CONFIG_ROLLBACK_JOB_TYPE: &str = "system.tenant_config.rollback";

const PACKAGE_EXPORT_PERMISSION: &str = "system:config-package:export";
const TRANSFER_PREVIEW_PERMISSION: &str = "system:config-transfer:preview";
const TRANSFER_APPLY_PERMISSION: &str = "system:config-transfer:apply";
const TRANSFER_ROLLBACK_PERMISSION: &str = "system:config-transfer:rollback";
const MAX_ATTEMPTS: i32 = 3;
const REQUEST_KIND_UPLOAD: &str = "upload";
const REQUEST_KIND_FROM_PACKAGE: &str = "from_package";

#[derive(Clone)]
pub struct TenantConfigTransferService {
    persistence: Arc<dyn TenantConfigTransferPersistencePort>,
    queue: Arc<JobQueue>,
    user_service: Arc<UserService>,
    file_service: Arc<FileService>,
    product_service: Arc<ProductService>,
    authorization_cache: AuthorizationCache,
    target_catalog: TenantConfigTargetCatalog,
    config: crate::TenantConfigTransferPolicy,
    archive: Arc<dyn TenantConfigArchivePort>,
}

#[derive(Clone)]
pub struct TenantConfigTransferDependencies {
    pub persistence: Arc<dyn TenantConfigTransferPersistencePort>,
    pub queue: Arc<JobQueue>,
    pub user_service: Arc<UserService>,
    pub file_service: Arc<FileService>,
    pub product_service: Arc<ProductService>,
    pub authorization_cache: AuthorizationCache,
    pub archive: Arc<dyn TenantConfigArchivePort>,
}

#[derive(Clone)]
pub struct TenantConfigTransferSettings {
    pub target_catalog: TenantConfigTargetCatalog,
    pub config: crate::TenantConfigTransferPolicy,
}

impl TenantConfigTransferService {
    pub fn new(
        dependencies: TenantConfigTransferDependencies,
        settings: TenantConfigTransferSettings,
    ) -> Self {
        let TenantConfigTransferDependencies {
            persistence,
            queue,
            user_service,
            file_service,
            product_service,
            authorization_cache,
            archive,
        } = dependencies;
        let TenantConfigTransferSettings {
            target_catalog,
            config,
        } = settings;
        Self {
            persistence,
            queue,
            user_service,
            file_service,
            product_service,
            authorization_cache,
            target_catalog,
            config,
            archive,
        }
    }

    pub fn upload_policy(&self) -> UploadPolicy {
        UploadPolicy {
            max_file_size: u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            allowed_extensions: vec!["zip".to_owned()],
        }
    }

    pub async fn request_preview(
        &self,
        actor: &ActorContext,
        transfer_id: i64,
        idempotency_key_hash: &str,
    ) -> AppResult<TenantConfigTransferVo> {
        self.enqueue_transfer_operation(
            actor,
            transfer_id,
            idempotency_key_hash,
            TENANT_CONFIG_PREVIEW_JOB_TYPE,
            TransferOperationRequest::Preview,
        )
        .await
    }

    pub async fn request_apply(
        &self,
        actor: &ActorContext,
        transfer_id: i64,
        command: ApplyTenantConfigTransferCommand,
    ) -> AppResult<TenantConfigTransferVo> {
        validate_sha256(&command.plan_hash)?;
        let idempotency_key_hash = command.idempotency_key_hash.clone();
        self.enqueue_transfer_operation(
            actor,
            transfer_id,
            &idempotency_key_hash,
            TENANT_CONFIG_APPLY_JOB_TYPE,
            TransferOperationRequest::Apply(command),
        )
        .await
    }

    pub async fn request_rollback(
        &self,
        actor: &ActorContext,
        transfer_id: i64,
        idempotency_key_hash: &str,
    ) -> AppResult<TenantConfigTransferVo> {
        self.enqueue_transfer_operation(
            actor,
            transfer_id,
            idempotency_key_hash,
            TENANT_CONFIG_ROLLBACK_JOB_TYPE,
            TransferOperationRequest::Rollback,
        )
        .await
    }

    pub(super) fn package_limits(&self) -> TenantConfigPackageLimits {
        TenantConfigPackageLimits::from(&self.config)
    }

    pub(super) fn max_runtime_seconds(&self) -> AppResult<i32> {
        i32::try_from(self.config.max_runtime_seconds)
            .map_err(|_| AppError::Config("配置迁移最大运行时间超出数据库范围".into()))
    }
}

fn requester_record(
    requester: &crate::system::user_service::CurrentAuthorization,
) -> TenantConfigRequesterRecord {
    TenantConfigRequesterRecord {
        tenant_id: requester.tenant.tenant_id.clone(),
        user_id: requester.actor.user_id,
        tenant_authorization_epoch: requester.tenant.authorization_epoch,
        user_authorization_version: requester.user.authorization_version,
    }
}
