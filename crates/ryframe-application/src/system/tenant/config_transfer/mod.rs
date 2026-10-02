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

use super::super::{
    file::{DownloadedFile, FileService, UploadPolicy},
    product::{CapabilityRequirement, ProductService},
    user::UserService,
};
use super::config_package::TENANT_CONFIG_PACKAGE_SCHEMA;
use super::config_package::{
    ParsedTenantConfigPackage, TenantConfigPackageLimits, TenantConfigPackageResources,
    TenantConfigPackageSource, parse_tenant_config_package,
};
use crate::{
    AuthorizationCache, ClaimedBackgroundJob, EnqueueJob, JobHandler, JobQueue,
    ports::tenant_config::{
        TenantConfigArchivePort, TenantConfigBundleRecord, TenantConfigOperationLeaseRecord,
        TenantConfigRequesterRecord, TenantConfigTransferItemRecord,
        TenantConfigTransferPersistencePort, TenantConfigTransferRecord,
        TenantConfigurationFenceRecord,
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

const PACKAGE_EXPORT_PERMISSION: &str = "platform:config-package:export";
const TRANSFER_PREVIEW_PERMISSION: &str = "platform:config-transfer:preview";
const TRANSFER_APPLY_PERMISSION: &str = "platform:config-transfer:apply";
const TRANSFER_ROLLBACK_PERMISSION: &str = "platform:config-transfer:rollback";
const MAX_ATTEMPTS: i32 = 3;
const REQUEST_KIND_UPLOAD: &str = "upload";
const REQUEST_KIND_FROM_PACKAGE: &str = "from_package";

/// 系统操作人与配置目标分开保存，避免借目标租户身份执行平台命令。
#[derive(Clone)]
pub struct TenantConfigScope {
    operator: ActorContext,
    target_tenant_id: String,
}

impl TenantConfigScope {
    pub fn new(operator: &ActorContext, target_tenant_id: &str) -> AppResult<Self> {
        if operator.tenant_id != "system" {
            return Err(AppError::Authorization("仅系统租户可以管理配置迁移".into()));
        }
        ryframe_kernel::TenantId::parse(target_tenant_id)?;
        Ok(Self {
            operator: operator.clone(),
            target_tenant_id: target_tenant_id.to_owned(),
        })
    }

    fn operator(&self) -> &ActorContext {
        &self.operator
    }

    fn tenant_id(&self) -> &str {
        &self.target_tenant_id
    }
}

#[derive(Clone)]
pub struct TenantConfigTransferService {
    persistence: Arc<dyn TenantConfigTransferPersistencePort>,
    queue: Arc<JobQueue>,
    user: Arc<UserService>,
    file: Arc<FileService>,
    product: Arc<ProductService>,
    authorization_cache: AuthorizationCache,
    target_catalog: TenantConfigTargetCatalog,
    config: crate::TenantConfigTransferPolicy,
    archive: Arc<dyn TenantConfigArchivePort>,
}

#[derive(Clone)]
pub struct TenantConfigTransferDependencies {
    pub persistence: Arc<dyn TenantConfigTransferPersistencePort>,
    pub queue: Arc<JobQueue>,
    pub user: Arc<UserService>,
    pub file: Arc<FileService>,
    pub product: Arc<ProductService>,
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
            user,
            file,
            product,
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
            user,
            file,
            product,
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
        scope: &TenantConfigScope,
        transfer_id: i64,
        idempotency_key_hash: &str,
    ) -> AppResult<TenantConfigTransferVo> {
        self.enqueue_transfer_operation(
            scope,
            transfer_id,
            idempotency_key_hash,
            TENANT_CONFIG_PREVIEW_JOB_TYPE,
            TransferOperationRequest::Preview,
        )
        .await
    }

    pub async fn request_apply(
        &self,
        scope: &TenantConfigScope,
        transfer_id: i64,
        command: ApplyTenantConfigTransferCommand,
    ) -> AppResult<TenantConfigTransferVo> {
        validate_sha256(&command.plan_hash)?;
        let idempotency_key_hash = command.idempotency_key_hash.clone();
        self.enqueue_transfer_operation(
            scope,
            transfer_id,
            &idempotency_key_hash,
            TENANT_CONFIG_APPLY_JOB_TYPE,
            TransferOperationRequest::Apply(command),
        )
        .await
    }

    pub async fn request_rollback(
        &self,
        scope: &TenantConfigScope,
        transfer_id: i64,
        idempotency_key_hash: &str,
    ) -> AppResult<TenantConfigTransferVo> {
        self.enqueue_transfer_operation(
            scope,
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
    requester: &crate::system::user::CurrentAuthorization,
) -> TenantConfigRequesterRecord {
    TenantConfigRequesterRecord {
        tenant_id: requester.tenant.tenant_id.clone(),
        user_id: requester.actor.user_id,
        tenant_authorization_epoch: requester.tenant.authorization_epoch,
        user_authorization_version: requester.user.authorization_version,
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use ryframe_kernel::DataScope;

    fn actor(tenant_id: &str) -> ActorContext {
        ActorContext {
            user_id: 7,
            tenant_id: tenant_id.into(),
            username: "平台操作人".into(),
            dept_id: None,
            dept_path: None,
            data_scope: DataScope::All,
            custom_dept_ids: Vec::new(),
            include_self: true,
            is_super_admin: true,
        }
    }

    #[test]
    fn target_scope_keeps_the_system_operator_identity() {
        let operator = actor("system");
        let scope = TenantConfigScope::new(&operator, "tenant-a").expect("系统租户可以选目标");
        assert_eq!(scope.operator().tenant_id, "system");
        assert_eq!(scope.operator().user_id, 7);
        assert_eq!(scope.tenant_id(), "tenant-a");
        assert!(TenantConfigScope::new(&actor("tenant-a"), "tenant-b").is_err());
        assert!(TenantConfigScope::new(&operator, "invalid tenant").is_err());
    }
}
