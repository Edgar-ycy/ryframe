use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use ryframe_config::TenantConfigTransferConfig;
use ryframe_core::repository::{PageResult, ValidatedPageQuery};
use ryframe_db::{
    CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository, DatabaseCluster, EnqueueBackgroundJob,
    FileRepository, TenantConfigTransferRepository,
    entities::{
        background_job, config, dept, dict_data, dict_type, menu, permission, post, role,
        role_dept, role_permission, tenant, tenant_config_bundle, tenant_config_lease,
        tenant_config_transfer, tenant_config_transfer_item, user, user_role,
    },
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_utils::{file_upload::UploadConfig, snowflake::try_next_snowflake_id};
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait,
    IntoActiveModel, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    sea_query::Expr,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::tenant_config_package::TENANT_CONFIG_PACKAGE_SCHEMA;
use super::{
    CONFIG_PACKAGE_BUCKET, DownloadedFile, FileService, ParsedTenantConfigPackage, PortableConfig,
    PortableDepartment, PortableDictData, PortableDictType, PortableMenu, PortablePermission,
    PortablePost, PortableRole, TenantConfigPackageLimits, TenantConfigPackageResources,
    UserService, parse_tenant_config_package,
};
use crate::{AuthorizationCache, JobHandler, JobQueue};

mod model;
mod plan;
mod resources;
mod stable_key;

pub use model::*;
use plan::*;
use resources::*;
use stable_key::*;
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
    db: DatabaseCluster,
    repository: Arc<TenantConfigTransferRepository>,
    queue: Arc<JobQueue>,
    user_service: Arc<UserService>,
    file_service: Arc<FileService>,
    authorization_cache: AuthorizationCache,
    target_catalog: TenantConfigTargetCatalog,
    config: TenantConfigTransferConfig,
}

impl TenantConfigTransferService {
    pub fn new(
        db: DatabaseCluster,
        queue: Arc<JobQueue>,
        user_service: Arc<UserService>,
        file_service: Arc<FileService>,
        authorization_cache: AuthorizationCache,
        target_catalog: TenantConfigTargetCatalog,
        config: TenantConfigTransferConfig,
    ) -> Self {
        Self {
            db,
            repository: Arc::new(TenantConfigTransferRepository),
            queue,
            user_service,
            file_service,
            authorization_cache,
            target_catalog,
            config,
        }
    }

    pub fn upload_config(&self) -> UploadConfig {
        UploadConfig {
            upload_dir: "config-packages".to_owned(),
            max_file_size: u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            allowed_extensions: vec!["zip".to_owned()],
        }
    }

    pub async fn request_package_export(
        &self,
        actor: &ActorContext,
        idempotency_key_hash: &str,
    ) -> AppResult<RequestTenantConfigBundleOutcome> {
        validate_sha256(idempotency_key_hash)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let operation = async {
            self.repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            let now = self.repository.database_utc_now(&transaction).await?;
            let tenant = tenant::Entity::find()
                .filter(tenant::Column::TenantId.eq(tenant_id))
                .one(&transaction)
                .await
                .map_err(database_error)?
                .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
            if let Some(bundle) = tenant_config_bundle::Entity::find()
                .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                .filter(tenant_config_bundle::Column::CreatedBy.eq(actor.user_id))
                .filter(tenant_config_bundle::Column::IdempotencyKeyHash.eq(idempotency_key_hash))
                .one(&transaction)
                .await
                .map_err(database_error)?
            {
                return Ok::<_, AppError>((bundle, false));
            }
            let proposed_bundle_id = try_next_snowflake_id()?;
            let trace = crate::trace_context::current_trace_context();
            let enqueued = self
                .queue
                .enqueue_in_transaction(
                    &transaction,
                    EnqueueBackgroundJob {
                        tenant_id: Some(tenant_id.to_owned()),
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: Some(self.max_runtime_seconds()?),
                        job_type: TENANT_CONFIG_EXPORT_JOB_TYPE.to_owned(),
                        payload: json!({ "bundle_id": proposed_bundle_id.to_string() }),
                        priority: -5,
                        available_at: now,
                        max_attempts: MAX_ATTEMPTS,
                        dedupe_key: Some(format!(
                            "{tenant_id}:{}:export:{idempotency_key_hash}",
                            actor.user_id
                        )),
                        traceparent: trace.traceparent,
                        tracestate: trace.tracestate,
                    },
                )
                .await?;
            let bundle = if enqueued.inserted {
                self.repository
                    .insert_bundle(
                        &transaction,
                        tenant_config_bundle::Model {
                            id: proposed_bundle_id,
                            tenant_id: tenant_id.to_owned(),
                            origin: tenant_config_bundle::Model::ORIGIN_GENERATED.to_owned(),
                            source_tenant_key: tenant_id.to_owned(),
                            source_tenant_name_snapshot: tenant.name,
                            package_schema_version: TENANT_CONFIG_PACKAGE_SCHEMA.to_owned(),
                            source_app_version: env!("CARGO_PKG_VERSION").to_owned(),
                            file_id: None,
                            sha256: None,
                            resource_counts: json!({}),
                            item_count: 0,
                            status: tenant_config_bundle::Model::STATUS_PENDING.to_owned(),
                            background_job_id: Some(enqueued.job.id),
                            idempotency_key_hash: Some(idempotency_key_hash.to_owned()),
                            created_by: actor.user_id,
                            error_summary: None,
                            expires_at: Some(
                                now + Duration::hours(i64::from(self.config.artifact_hours)),
                            ),
                            created_at: now,
                            updated_at: now,
                        },
                    )
                    .await?
            } else {
                tenant_config_bundle::Entity::find()
                    .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                    .filter(tenant_config_bundle::Column::BackgroundJobId.eq(enqueued.job.id))
                    .one(&transaction)
                    .await
                    .map_err(database_error)?
                    .ok_or_else(|| AppError::Conflict("配置包导出幂等记录尚未完成".into()))?
            };
            Ok::<_, AppError>((bundle, enqueued.inserted))
        }
        .await;
        match operation {
            Ok((bundle, inserted)) => {
                crate::commit_current_audit(transaction).await?;
                self.queue.notify_background_jobs().await;
                Ok(RequestTenantConfigBundleOutcome {
                    bundle: bundle.into(),
                    inserted,
                })
            }
            Err(error) => {
                transaction.rollback().await.map_err(database_error)?;
                Err(error)
            }
        }
    }

    pub async fn upload_package_and_create_transfer(
        &self,
        actor: &ActorContext,
        original_name: String,
        data: Vec<u8>,
        idempotency_key_hash: &str,
    ) -> AppResult<RequestTenantConfigTransferOutcome> {
        validate_sha256(idempotency_key_hash)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let parsed = parse_tenant_config_package(data.clone(), self.package_limits()).await?;
        if let Some((existing, bundle)) = self
            .find_uploaded_transfer_by_idempotency_and_bind_audit(
                tenant_id,
                actor.user_id,
                idempotency_key_hash,
                &parsed.package_sha256,
            )
            .await?
        {
            return Ok(RequestTenantConfigTransferOutcome {
                transfer: TenantConfigTransferVo::from_models(existing, &bundle)?,
                inserted: false,
            });
        }
        let uploaded = self
            .file_service
            .upload_config_package_unbound(
                tenant_id,
                &actor.username,
                original_name,
                data,
                u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            )
            .await?;
        let file_id = parse_file_id(&uploaded.file_id)?;
        let result = self
            .insert_uploaded_bundle_and_transfer(actor, file_id, parsed, idempotency_key_hash)
            .await;
        if match result.as_ref() {
            Ok(outcome) => !outcome.inserted,
            Err(_) => true,
        } {
            let _ = self
                .file_service
                .schedule_unreferenced_config_package_cleanup(tenant_id, file_id)
                .await;
        }
        result
    }

    async fn find_uploaded_transfer_by_idempotency_and_bind_audit(
        &self,
        tenant_id: &str,
        requested_by: i64,
        idempotency_key_hash: &str,
        package_sha256: &str,
    ) -> AppResult<Option<(tenant_config_transfer::Model, tenant_config_bundle::Model)>> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            self.repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            let existing = self
                .repository
                .find_transfer_by_idempotency_key(
                    &transaction,
                    tenant_id,
                    requested_by,
                    idempotency_key_hash,
                )
                .await?;
            let Some(existing) = existing else {
                return Ok::<_, AppError>(None);
            };
            ensure_transfer_request_identity(&existing, REQUEST_KIND_UPLOAD, package_sha256)?;
            let bundle = self
                .repository
                .lock_bundle_in_txn(&transaction, tenant_id, existing.bundle_id)
                .await?
                .ok_or_else(|| AppError::Conflict("幂等记录关联的配置包不存在".into()))?;
            if bundle.sha256.as_deref() != Some(package_sha256) {
                return Err(AppError::Conflict(
                    "Idempotency-Key 已用于其他配置包".into(),
                ));
            }
            Ok::<_, AppError>(Some((existing, bundle)))
        }
        .await;
        match result {
            Ok(Some((existing, bundle))) => {
                crate::commit_current_audit(transaction).await?;
                Ok(Some((existing, bundle)))
            }
            Ok(None) => {
                transaction.rollback().await.map_err(database_error)?;
                Ok(None)
            }
            Err(error) => {
                transaction.rollback().await.map_err(database_error)?;
                Err(error)
            }
        }
    }

    pub async fn create_transfer_from_package(
        &self,
        actor: &ActorContext,
        bundle_id: i64,
        idempotency_key_hash: &str,
    ) -> AppResult<RequestTenantConfigTransferOutcome> {
        validate_sha256(idempotency_key_hash)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let request_fingerprint =
            transfer_request_fingerprint(REQUEST_KIND_FROM_PACKAGE, bundle_id);
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let operation = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            if let Some(existing) = self
                .repository
                .find_transfer_by_idempotency_key(
                    &transaction,
                    tenant_id,
                    actor.user_id,
                    idempotency_key_hash,
                )
                .await?
            {
                ensure_transfer_request_identity(
                    &existing,
                    REQUEST_KIND_FROM_PACKAGE,
                    &request_fingerprint,
                )?;
                if existing.bundle_id != bundle_id {
                    return Err(AppError::Conflict(
                        "Idempotency-Key 已用于其他配置包".into(),
                    ));
                }
                let bundle = self
                    .repository
                    .lock_bundle_in_txn(&transaction, tenant_id, bundle_id)
                    .await?
                    .ok_or_else(|| AppError::Conflict("幂等记录关联的配置包不存在".into()))?;
                return Ok::<_, AppError>((existing, bundle, false));
            }
            let bundle = self
                .repository
                .lock_bundle_in_txn(&transaction, tenant_id, bundle_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置包不存在".into()))?;
            ensure_bundle_available(
                &bundle,
                self.repository.database_utc_now(&transaction).await?,
            )?;
            let transfer = self
                .repository
                .insert_transfer(
                    &transaction,
                    new_transfer_model(
                        tenant_id,
                        bundle.id,
                        idempotency_key_hash,
                        REQUEST_KIND_FROM_PACKAGE,
                        &request_fingerprint,
                        actor.user_id,
                        fence.configuration_version,
                        fence.authorization_epoch,
                        self.repository.database_utc_now(&transaction).await?,
                    )?,
                )
                .await?;
            Ok((transfer, bundle, true))
        }
        .await;
        match operation {
            Ok((transfer, bundle, inserted)) => {
                crate::commit_current_audit(transaction).await?;
                Ok(RequestTenantConfigTransferOutcome {
                    transfer: TenantConfigTransferVo::from_models(transfer, &bundle)?,
                    inserted,
                })
            }
            Err(error) => {
                transaction.rollback().await.map_err(database_error)?;
                Err(error)
            }
        }
    }

    pub async fn list_bundles(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigBundleVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let total = tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        let records = self
            .repository
            .list_bundles(self.db.write(), tenant_id, page.page_size(), page.offset())
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(PageResult::new(records, total, &page))
    }

    pub async fn get_bundle(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<TenantConfigBundleVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.repository
            .find_bundle_by_id(self.db.write(), tenant_id, id)
            .await?
            .map(Into::into)
            .ok_or_else(|| AppError::NotFound("配置包不存在".into()))
    }

    pub async fn download_bundle(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<DownloadedFile> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let bundle = self
            .repository
            .find_bundle_by_id(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置包不存在".into()))?;
        ensure_bundle_available(
            &bundle,
            self.repository.database_utc_now(self.db.write()).await?,
        )?;
        let file_id = bundle
            .file_id
            .ok_or_else(|| AppError::Conflict("配置包文件尚未生成".into()))?;
        self.file_service
            .download_config_package_internal(tenant_id, file_id)
            .await
    }

    pub async fn list_transfers(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigTransferVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let total = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        let records = self
            .repository
            .list_transfers(self.db.write(), tenant_id, page.page_size(), page.offset())
            .await?;
        let bundle_ids = records
            .iter()
            .map(|transfer| transfer.bundle_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let bundles = self
            .repository
            .find_bundles_by_ids(self.db.write(), tenant_id, &bundle_ids)
            .await?
            .into_iter()
            .map(|bundle| (bundle.id, bundle))
            .collect::<BTreeMap<_, _>>();
        let records = records
            .into_iter()
            .map(|transfer| {
                let bundle = bundles
                    .get(&transfer.bundle_id)
                    .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
                TenantConfigTransferVo::from_models(transfer, bundle)
            })
            .collect::<AppResult<Vec<_>>>()?;
        Ok(PageResult::new(records, total, &page))
    }

    pub async fn get_transfer(
        &self,
        actor: &ActorContext,
        id: i64,
    ) -> AppResult<TenantConfigTransferVo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transfer = self
            .repository
            .find_transfer_by_id(self.db.write(), tenant_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        let bundle = self
            .repository
            .find_bundle_by_id(self.db.write(), tenant_id, transfer.bundle_id)
            .await?
            .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
        TenantConfigTransferVo::from_models(transfer, &bundle)
    }

    pub async fn list_transfer_items(
        &self,
        actor: &ActorContext,
        transfer_id: i64,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<TenantConfigTransferItemVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.ensure_transfer_visible(tenant_id, transfer_id).await?;
        let base = tenant_config_transfer_item::Entity::find()
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id));
        let total = base
            .clone()
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        let records = base
            .order_by_asc(tenant_config_transfer_item::Column::Id)
            .limit(page.page_size())
            .offset(page.offset())
            .all(self.db.write())
            .await
            .map_err(database_error)?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(PageResult::new(records, total, &page))
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

    async fn insert_uploaded_bundle_and_transfer(
        &self,
        actor: &ActorContext,
        file_id: i64,
        parsed: ParsedTenantConfigPackage,
        idempotency_key_hash: &str,
    ) -> AppResult<RequestTenantConfigTransferOutcome> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let operation = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            if let Some(existing) = self
                .repository
                .find_transfer_by_idempotency_key(
                    &transaction,
                    tenant_id,
                    actor.user_id,
                    idempotency_key_hash,
                )
                .await?
            {
                ensure_transfer_request_identity(
                    &existing,
                    REQUEST_KIND_UPLOAD,
                    &parsed.package_sha256,
                )?;
                let existing_bundle = self
                    .repository
                    .lock_bundle_in_txn(&transaction, tenant_id, existing.bundle_id)
                    .await?
                    .ok_or_else(|| AppError::Conflict("幂等记录关联的配置包不存在".into()))?;
                if existing_bundle.sha256.as_deref() != Some(parsed.package_sha256.as_str()) {
                    return Err(AppError::Conflict(
                        "Idempotency-Key 已用于其他配置包".into(),
                    ));
                }
                return Ok::<_, AppError>((existing, existing_bundle, false));
            }
            let now = self.repository.database_utc_now(&transaction).await?;
            ensure_config_package_file_ready_in_txn(&transaction, tenant_id, file_id, now).await?;
            let bundle_id = try_next_snowflake_id()?;
            let counts = serde_json::to_value(&parsed.manifest.resource_counts)
                .map_err(internal_json_error)?;
            let bundle = self
                .repository
                .insert_bundle(
                    &transaction,
                    tenant_config_bundle::Model {
                        id: bundle_id,
                        tenant_id: tenant_id.to_owned(),
                        origin: tenant_config_bundle::Model::ORIGIN_UPLOADED.to_owned(),
                        source_tenant_key: parsed.manifest.source_tenant_key,
                        source_tenant_name_snapshot: parsed.manifest.source_tenant_name,
                        package_schema_version: parsed.manifest.schema,
                        source_app_version: parsed.manifest.source_app_version,
                        file_id: Some(file_id),
                        sha256: Some(parsed.package_sha256.clone()),
                        resource_counts: counts,
                        item_count: i32::try_from(parsed.manifest.item_count)
                            .map_err(|_| AppError::PayloadTooLarge("配置包项目数量超限".into()))?,
                        status: tenant_config_bundle::Model::STATUS_SUCCEEDED.to_owned(),
                        background_job_id: None,
                        idempotency_key_hash: None,
                        created_by: actor.user_id,
                        error_summary: None,
                        expires_at: Some(
                            now + Duration::hours(i64::from(self.config.artifact_hours)),
                        ),
                        created_at: now,
                        updated_at: now,
                    },
                )
                .await?;
            let transfer = self
                .repository
                .insert_transfer(
                    &transaction,
                    new_transfer_model(
                        tenant_id,
                        bundle_id,
                        idempotency_key_hash,
                        REQUEST_KIND_UPLOAD,
                        &parsed.package_sha256,
                        actor.user_id,
                        fence.configuration_version,
                        fence.authorization_epoch,
                        now,
                    )?,
                )
                .await?;
            Ok((transfer, bundle, true))
        }
        .await;
        match operation {
            Ok((transfer, bundle, inserted)) => {
                crate::commit_current_audit(transaction).await?;
                Ok(RequestTenantConfigTransferOutcome {
                    transfer: TenantConfigTransferVo::from_models(transfer, &bundle)?,
                    inserted,
                })
            }
            Err(error) => {
                transaction.rollback().await.map_err(database_error)?;
                Err(error)
            }
        }
    }

    async fn enqueue_transfer_operation(
        &self,
        actor: &ActorContext,
        transfer_id: i64,
        idempotency_key_hash: &str,
        job_type: &'static str,
        operation: TransferOperationRequest,
    ) -> AppResult<TenantConfigTransferVo> {
        validate_sha256(idempotency_key_hash)?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            self.repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            let mut transfer = self
                .repository
                .lock_transfer_in_txn(&transaction, tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if transfer.requested_by != actor.user_id {
                return Err(AppError::Authorization("只能操作本人创建的配置迁移".into()));
            }
            let now = self.repository.database_utc_now(&transaction).await?;
            let trace = crate::trace_context::current_trace_context();
            let enqueued = self
                .queue
                .enqueue_in_transaction(
                    &transaction,
                    EnqueueBackgroundJob {
                        tenant_id: Some(tenant_id.to_owned()),
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: Some(self.max_runtime_seconds()?),
                        job_type: job_type.to_owned(),
                        payload: json!({ "transfer_id": transfer_id.to_string() }),
                        priority: -5,
                        available_at: now,
                        max_attempts: MAX_ATTEMPTS,
                        dedupe_key: Some(format!(
                            "{tenant_id}:{}:{transfer_id}:{idempotency_key_hash}",
                            actor.user_id
                        )),
                        traceparent: trace.traceparent,
                        tracestate: trace.tracestate,
                    },
                )
                .await?;
            if !enqueued.inserted {
                if operation_job_id(&transfer, &operation) == Some(enqueued.job.id) {
                    validate_operation_replay_identity(&transfer, &operation)?;
                    let bundle = self
                        .repository
                        .find_bundle_by_id(&transaction, tenant_id, transfer.bundle_id)
                        .await?
                        .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
                    return Ok::<_, AppError>((transfer, bundle));
                }
                return Err(AppError::Conflict("幂等键已被其他配置迁移操作使用".into()));
            }
            validate_operation_request(&transfer, &operation)?;
            clear_superseded_dead_operation_jobs(&transaction, &mut transfer, &operation).await?;
            match operation {
                TransferOperationRequest::Preview => {
                    if enqueued.inserted {
                        transfer.status =
                            tenant_config_transfer::Model::STATUS_PREVIEW_PENDING.to_owned();
                        transfer.preview_background_job_id = Some(enqueued.job.id);
                        transfer.preview_calculated_at = None;
                        transfer.plan_hash = None;
                        transfer.error_summary = None;
                    } else if transfer.preview_background_job_id != Some(enqueued.job.id) {
                        return Err(AppError::Conflict("预览幂等键已被其他预览请求使用".into()));
                    }
                }
                TransferOperationRequest::Apply(command) => {
                    if transfer.plan_hash.as_deref() != Some(command.plan_hash.as_str())
                        || transfer.target_configuration_version
                            != command.target_configuration_version
                        || transfer.target_authorization_epoch != command.target_authorization_epoch
                    {
                        return Err(AppError::Conflict("预览结果已失效，请重新预览".into()));
                    }
                    if enqueued.inserted {
                        transfer.status =
                            tenant_config_transfer::Model::STATUS_APPLY_PENDING.to_owned();
                        transfer.apply_background_job_id = Some(enqueued.job.id);
                        transfer.error_summary = None;
                    } else if transfer.apply_background_job_id != Some(enqueued.job.id) {
                        return Err(AppError::Conflict("应用幂等键已被其他请求使用".into()));
                    }
                }
                TransferOperationRequest::Rollback => {
                    if enqueued.inserted {
                        transfer.status =
                            tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING.to_owned();
                        transfer.rollback_background_job_id = Some(enqueued.job.id);
                        transfer.error_summary = None;
                    } else if transfer.rollback_background_job_id != Some(enqueued.job.id) {
                        return Err(AppError::Conflict("回滚幂等键已被其他请求使用".into()));
                    }
                }
            }
            transfer.updated_at = now;
            let transfer = self
                .repository
                .update_transfer(&transaction, transfer)
                .await?;
            let bundle = self
                .repository
                .find_bundle_by_id(&transaction, tenant_id, transfer.bundle_id)
                .await?
                .ok_or_else(|| AppError::Internal("配置迁移关联的配置包不存在".into()))?;
            Ok::<_, AppError>((transfer, bundle))
        }
        .await;
        match result {
            Ok((transfer, bundle)) => {
                crate::commit_current_audit(transaction).await?;
                self.queue.notify_background_jobs().await;
                TenantConfigTransferVo::from_models(transfer, &bundle)
            }
            Err(error) => {
                transaction.rollback().await.map_err(database_error)?;
                Err(error)
            }
        }
    }

    async fn ensure_transfer_visible(&self, tenant_id: &str, transfer_id: i64) -> AppResult<()> {
        self.repository
            .find_transfer_by_id(self.db.write(), tenant_id, transfer_id)
            .await?
            .map(|_| ())
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))
    }

    fn package_limits(&self) -> TenantConfigPackageLimits {
        TenantConfigPackageLimits::from(&self.config)
    }

    fn max_runtime_seconds(&self) -> AppResult<i32> {
        i32::try_from(self.config.max_runtime_seconds)
            .map_err(|_| AppError::Config("配置迁移最大运行时间超出数据库范围".into()))
    }

    async fn execute_export(&self, job: &background_job::Model) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let bundle_id = payload_id(job, "bundle_id")?;
        let requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                self.bundle_requester(tenant_id, bundle_id).await?,
                PACKAGE_EXPORT_PERMISSION,
            )
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        self.repository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
            .await?;
        let mut bundle = self
            .repository
            .lock_bundle_in_txn(&transaction, tenant_id, bundle_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置包导出记录不存在".into()))?;
        if bundle.background_job_id != Some(job.id) {
            transaction.rollback().await.map_err(database_error)?;
            return Err(AppError::Conflict("配置包导出任务身份不匹配".into()));
        }
        if bundle.status == tenant_config_bundle::Model::STATUS_SUCCEEDED {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(());
        }
        bundle.status = tenant_config_bundle::Model::STATUS_RUNNING.to_owned();
        bundle.updated_at = self.repository.database_utc_now(&transaction).await?;
        self.repository.update_bundle(&transaction, bundle).await?;
        transaction.commit().await.map_err(database_error)?;

        // 在租户配置行锁保护下从同一事务读取全部资源，避免包内混合两个版本。
        let source_transaction = self.db.write().begin().await.map_err(database_error)?;
        let source_result = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(&source_transaction, tenant_id, None)
                .await?;
            let generated_at = self
                .repository
                .database_utc_now(&source_transaction)
                .await?;
            ensure_requester_snapshot_in_txn(
                &source_transaction,
                tenant_id,
                &requester,
                fence,
                generated_at,
            )
            .await?;
            let tenant = tenant::Entity::find()
                .filter(tenant::Column::TenantId.eq(tenant_id))
                .one(&source_transaction)
                .await
                .map_err(database_error)?
                .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
            let resources = load_resources_on(&source_transaction, tenant_id).await?;
            let resources = filter_exportable_resources(resources, &self.target_catalog)?;
            Ok::<_, AppError>((resources, tenant.name, generated_at))
        }
        .await;
        let (source_resources, source_tenant_name, generated_at) = match source_result {
            Ok(source) => {
                source_transaction.commit().await.map_err(database_error)?;
                source
            }
            Err(error) => {
                source_transaction
                    .rollback()
                    .await
                    .map_err(database_error)?;
                return Err(error);
            }
        };
        let generated = super::build_tenant_config_package(
            source_resources,
            tenant_id.to_owned(),
            source_tenant_name,
            env!("CARGO_PKG_VERSION").to_owned(),
            generated_at,
            self.package_limits(),
        )
        .await?;
        let uploaded = self
            .file_service
            .upload_config_package_unbound(
                tenant_id,
                "config-transfer-worker",
                format!(
                    "{}-{}.ryframe-config.zip",
                    tenant_id,
                    generated_at.format("%Y%m%d%H%M%S")
                ),
                generated.data,
                u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            )
            .await?;
        let file_id = parse_file_id(&uploaded.file_id)?;
        let final_requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                requester.actor.user_id,
                PACKAGE_EXPORT_PERMISSION,
            )
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let operation = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            let mut bundle = self
                .repository
                .lock_bundle_in_txn(&transaction, tenant_id, bundle_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置包导出记录不存在".into()))?;
            if bundle.background_job_id != Some(job.id) {
                return Err(AppError::Conflict("配置包导出任务已被替换".into()));
            }
            let now = self.repository.database_utc_now(&transaction).await?;
            ensure_config_package_file_ready_in_txn(&transaction, tenant_id, file_id, now).await?;
            ensure_requester_snapshot_in_txn(&transaction, tenant_id, &final_requester, fence, now)
                .await?;
            bundle.file_id = Some(file_id);
            bundle.sha256 = Some(generated.package_sha256);
            bundle.source_tenant_key = generated.manifest.source_tenant_key;
            bundle.source_tenant_name_snapshot = generated.manifest.source_tenant_name;
            bundle.package_schema_version = generated.manifest.schema;
            bundle.source_app_version = generated.manifest.source_app_version;
            bundle.resource_counts = serde_json::to_value(generated.manifest.resource_counts)
                .map_err(internal_json_error)?;
            bundle.item_count = i32::try_from(generated.manifest.item_count)
                .map_err(|_| AppError::PayloadTooLarge("配置包项目数量超限".into()))?;
            bundle.status = tenant_config_bundle::Model::STATUS_SUCCEEDED.to_owned();
            bundle.error_summary = None;
            bundle.expires_at = Some(now + Duration::hours(i64::from(self.config.artifact_hours)));
            bundle.updated_at = now;
            self.repository.update_bundle(&transaction, bundle).await
        }
        .await;
        match operation {
            Ok(_) => {
                if let Err(error) = transaction.commit().await.map_err(database_error) {
                    // COMMIT 响应丢失时结果可能已经持久化。引用保护会在已绑定成功时拒绝
                    // 清理，而在事务确实未提交时把孤儿文件纳入延迟回收。
                    let _ = self
                        .file_service
                        .schedule_unreferenced_config_package_cleanup(tenant_id, file_id)
                        .await;
                    return Err(error);
                }
                Ok(())
            }
            Err(error) => {
                transaction.rollback().await.map_err(database_error)?;
                let _ = self
                    .file_service
                    .schedule_unreferenced_config_package_cleanup(tenant_id, file_id)
                    .await;
                Err(error)
            }
        }
    }

    async fn execute_preview(&self, job: &background_job::Model) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let transfer_id = payload_id(job, "transfer_id")?;
        let transfer = self
            .repository
            .find_transfer_by_id(self.db.write(), tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        if transfer.preview_background_job_id != Some(job.id) {
            return Ok(());
        }
        let requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                transfer.requested_by,
                TRANSFER_PREVIEW_PERMISSION,
            )
            .await?;
        self.mark_transfer_running(
            tenant_id,
            transfer_id,
            job.id,
            TENANT_CONFIG_PREVIEW_JOB_TYPE,
            None,
        )
        .await?;
        let parsed = self
            .load_bundle_package(tenant_id, transfer.bundle_id)
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let operation = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, None)
                .await?;
            let mut current = self
                .repository
                .lock_transfer_in_txn(&transaction, tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if current.preview_background_job_id != Some(job.id)
                || current.status != tenant_config_transfer::Model::STATUS_PREVIEWING
            {
                return Ok::<_, AppError>(None);
            }
            let calculated_at = self.repository.database_utc_now(&transaction).await?;
            ensure_requester_snapshot_in_txn(
                &transaction,
                tenant_id,
                &requester,
                fence,
                calculated_at,
            )
            .await?;
            let target = load_resources_on(&transaction, tenant_id).await?;
            let plan = build_preview_plan(
                tenant_id,
                transfer_id,
                &parsed,
                &target,
                &self.target_catalog.page_routes,
                &self.target_catalog.api_permission_codes,
                fence.configuration_version,
                fence.authorization_epoch,
                calculated_at,
            )?;
            self.repository
                .replace_items_in_txn(&transaction, tenant_id, transfer_id, plan.items)
                .await?;
            current.status = tenant_config_transfer::Model::STATUS_PREVIEWED.to_owned();
            current.target_configuration_version = fence.configuration_version;
            current.target_authorization_epoch = fence.authorization_epoch;
            current.plan_hash = Some(plan.plan_hash);
            current.preview_calculated_at = Some(calculated_at);
            current.change_counts =
                serde_json::to_value(plan.counts).map_err(internal_json_error)?;
            current.error_summary = None;
            current.updated_at = calculated_at;
            Ok(Some(
                self.repository
                    .update_transfer(&transaction, current)
                    .await?,
            ))
        }
        .await;
        match operation {
            Ok(_) => transaction.commit().await.map_err(database_error),
            Err(error) => {
                transaction.rollback().await.map_err(database_error)?;
                Err(error)
            }
        }
    }

    async fn execute_apply(&self, job: &background_job::Model) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let transfer_id = payload_id(job, "transfer_id")?;
        let owner_token = Uuid::new_v4().to_string();
        let transfer = self
            .repository
            .find_transfer_by_id(self.db.write(), tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        if transfer.apply_background_job_id != Some(job.id) {
            return Ok(());
        }
        if transfer.status == tenant_config_transfer::Model::STATUS_APPLIED {
            return self.sync_committed_cache_state(tenant_id, &transfer).await;
        }
        let requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                transfer.requested_by,
                TRANSFER_APPLY_PERMISSION,
            )
            .await?;
        self.acquire_operation_lease(
            tenant_id,
            transfer_id,
            &owner_token,
            tenant_config_lease::Model::OPERATION_APPLY,
        )
        .await?;
        if let Err(error) = self
            .mark_transfer_running(
                tenant_id,
                transfer_id,
                job.id,
                TENANT_CONFIG_APPLY_JOB_TYPE,
                Some(&owner_token),
            )
            .await
        {
            let _ = self.release_operation_lease(tenant_id, &owner_token).await;
            return Err(error);
        }
        let parsed = match self
            .load_bundle_package(tenant_id, transfer.bundle_id)
            .await
        {
            Ok(parsed) => parsed,
            Err(error) => {
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                return Err(error);
            }
        };
        // 快照也必须在租约持有者的租户行锁下从同一事务读取。
        let snapshot_transaction = match self.db.write().begin().await.map_err(database_error) {
            Ok(transaction) => transaction,
            Err(error) => {
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                return Err(error);
            }
        };
        let snapshot_result = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(
                    &snapshot_transaction,
                    tenant_id,
                    Some(&owner_token),
                )
                .await?;
            let snapshot_time = self
                .repository
                .database_utc_now(&snapshot_transaction)
                .await?;
            ensure_requester_snapshot_in_txn(
                &snapshot_transaction,
                tenant_id,
                &requester,
                fence,
                snapshot_time,
            )
            .await?;
            let tenant = tenant::Entity::find()
                .filter(tenant::Column::TenantId.eq(tenant_id))
                .one(&snapshot_transaction)
                .await
                .map_err(database_error)?
                .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
            let target_resources = load_resources_on(&snapshot_transaction, tenant_id).await?;
            ensure_preview_identity(&transfer, &parsed, &target_resources, fence)?;
            let snapshot_resources =
                filter_exportable_resources(target_resources, &self.target_catalog)?;
            Ok::<_, AppError>((snapshot_resources, tenant.name, snapshot_time))
        }
        .await;
        let (snapshot_resources, snapshot_tenant_name, snapshot_time) = match snapshot_result {
            Ok(value) => {
                if let Err(error) = snapshot_transaction.commit().await.map_err(database_error) {
                    let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                    return Err(error);
                }
                value
            }
            Err(error) => {
                let rollback_result = snapshot_transaction
                    .rollback()
                    .await
                    .map_err(database_error);
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                rollback_result?;
                return Err(error);
            }
        };
        let snapshot = match super::build_tenant_config_package(
            snapshot_resources,
            tenant_id.to_owned(),
            snapshot_tenant_name,
            env!("CARGO_PKG_VERSION").to_owned(),
            snapshot_time,
            self.package_limits(),
        )
        .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                return Err(error);
            }
        };
        let snapshot_upload = match self
            .file_service
            .upload_config_package_unbound(
                tenant_id,
                "config-transfer-worker",
                format!("rollback-{transfer_id}.ryframe-config.zip"),
                snapshot.data,
                u64::try_from(self.config.max_package_bytes).unwrap_or(u64::MAX),
            )
            .await
        {
            Ok(uploaded) => uploaded,
            Err(error) => {
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                return Err(error);
            }
        };
        let snapshot_file_id = match parse_file_id(&snapshot_upload.file_id) {
            Ok(file_id) => file_id,
            Err(error) => {
                if let Ok(file_id) = snapshot_upload.file_id.parse::<i64>() {
                    let _ = self
                        .file_service
                        .schedule_unreferenced_config_package_cleanup(tenant_id, file_id)
                        .await;
                }
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                return Err(error);
            }
        };

        if let Err(error) = self.renew_operation_lease(tenant_id, &owner_token).await {
            let _ = self
                .file_service
                .schedule_unreferenced_config_package_cleanup(tenant_id, snapshot_file_id)
                .await;
            let _ = self.release_operation_lease(tenant_id, &owner_token).await;
            return Err(error);
        }

        let transaction = match self.db.write().begin().await.map_err(database_error) {
            Ok(transaction) => transaction,
            Err(error) => {
                let _ = self
                    .file_service
                    .schedule_unreferenced_config_package_cleanup(tenant_id, snapshot_file_id)
                    .await;
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                return Err(error);
            }
        };
        let operation = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, Some(&owner_token))
                .await?;
            let mut current = self
                .repository
                .lock_transfer_in_txn(&transaction, tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if current.apply_background_job_id != Some(job.id)
                || current.status != tenant_config_transfer::Model::STATUS_APPLYING
            {
                return Err(AppError::Conflict("配置应用任务已被替换".into()));
            }
            if fence.configuration_version != current.target_configuration_version
                || fence.authorization_epoch != current.target_authorization_epoch
            {
                return Err(AppError::Conflict("目标配置已变化，请重新预览".into()));
            }
            let mutation_time = self.repository.database_utc_now(&transaction).await?;
            ensure_config_package_file_ready_in_txn(
                &transaction,
                tenant_id,
                snapshot_file_id,
                mutation_time,
            )
            .await?;
            ensure_requester_snapshot_in_txn(
                &transaction,
                tenant_id,
                &requester,
                fence,
                mutation_time,
            )
            .await?;
            let target = load_resources_on(&transaction, tenant_id).await?;
            let plan = build_preview_plan(
                tenant_id,
                transfer_id,
                &parsed,
                &target,
                &self.target_catalog.page_routes,
                &self.target_catalog.api_permission_codes,
                fence.configuration_version,
                fence.authorization_epoch,
                mutation_time,
            )?;
            if current.plan_hash.as_deref() != Some(plan.plan_hash.as_str()) {
                return Err(AppError::Conflict("预览计划哈希已失效".into()));
            }
            if plan
                .counts
                .get(tenant_config_transfer_item::Model::ACTION_BLOCKED)
                .copied()
                .unwrap_or(0)
                > 0
                || plan
                    .counts
                    .get(tenant_config_transfer_item::Model::ACTION_CONFLICT)
                    .copied()
                    .unwrap_or(0)
                    > 0
            {
                return Err(AppError::Conflict("配置计划仍含冲突或阻断项".into()));
            }
            ensure_role_quota_for_plan_in_txn(&transaction, tenant_id, &plan.items).await?;
            apply_resources_in_transaction(
                &transaction,
                tenant_id,
                &parsed.resources,
                &plan.items,
                mutation_time,
            )
            .await?;
            let configuration_version = self
                .repository
                .increment_configuration_version_in_txn(&transaction, tenant_id)
                .await?;
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                .await?;
            let namespace_version = self
                .authorization_cache
                .record_namespace_version_in_transaction(
                    &transaction,
                    tenant_id,
                    CONFIG_CACHE_NAMESPACE,
                )
                .await?;
            let now = self.repository.database_utc_now(&transaction).await?;
            current.status = tenant_config_transfer::Model::STATUS_APPLIED.to_owned();
            current.snapshot_file_id = Some(snapshot_file_id);
            current.applied_configuration_version = Some(configuration_version);
            current.applied_authorization_epoch = Some(authorization_epoch);
            current.rollback_expires_at =
                Some(now + Duration::hours(i64::from(self.config.rollback_hours)));
            current.error_summary = None;
            current.updated_at = now;
            self.repository
                .update_transfer(&transaction, current)
                .await?;
            mark_plan_outcome(
                &transaction,
                tenant_id,
                transfer_id,
                tenant_config_transfer_item::Model::OUTCOME_APPLIED,
            )
            .await?;
            self.repository
                .release_lease_in_txn(&transaction, tenant_id, &owner_token)
                .await?;
            Ok::<_, AppError>((authorization_epoch, namespace_version))
        }
        .await;
        match operation {
            Ok((authorization_epoch, namespace_version)) => {
                if let Err(error) = transaction.commit().await.map_err(database_error) {
                    let _ = self
                        .file_service
                        .schedule_unreferenced_config_package_cleanup(tenant_id, snapshot_file_id)
                        .await;
                    let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                    return Err(error);
                }
                self.authorization_cache
                    .sync_tenant_epoch(tenant_id, authorization_epoch)
                    .await?;
                self.authorization_cache
                    .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
                    .await
            }
            Err(error) => {
                let rollback_result = transaction.rollback().await.map_err(database_error);
                let _ = self
                    .file_service
                    .schedule_unreferenced_config_package_cleanup(tenant_id, snapshot_file_id)
                    .await;
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                rollback_result?;
                Err(error)
            }
        }
    }

    async fn execute_rollback(&self, job: &background_job::Model) -> AppResult<()> {
        let tenant_id = job_tenant(job)?;
        let transfer_id = payload_id(job, "transfer_id")?;
        let owner_token = Uuid::new_v4().to_string();
        let transfer = self
            .repository
            .find_transfer_by_id(self.db.write(), tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        if transfer.rollback_background_job_id != Some(job.id) {
            return Ok(());
        }
        if transfer.status == tenant_config_transfer::Model::STATUS_ROLLED_BACK {
            return self.sync_committed_cache_state(tenant_id, &transfer).await;
        }
        let requester = self
            .user_service
            .resolve_current_authorization(
                tenant_id,
                transfer.requested_by,
                TRANSFER_ROLLBACK_PERMISSION,
            )
            .await?;
        let now = self.repository.database_utc_now(self.db.write()).await?;
        if transfer
            .rollback_expires_at
            .is_none_or(|expires_at| expires_at <= now)
        {
            return Err(AppError::Conflict("配置回滚窗口已过期".into()));
        }
        let snapshot_file_id = transfer
            .snapshot_file_id
            .ok_or_else(|| AppError::Conflict("配置回滚快照不存在".into()))?;
        let snapshot_file = self
            .file_service
            .download_config_package_internal(tenant_id, snapshot_file_id)
            .await?;
        let snapshot =
            parse_tenant_config_package(snapshot_file.data, self.package_limits()).await?;
        self.acquire_operation_lease(
            tenant_id,
            transfer_id,
            &owner_token,
            tenant_config_lease::Model::OPERATION_ROLLBACK,
        )
        .await?;
        if let Err(error) = self
            .mark_transfer_running(
                tenant_id,
                transfer_id,
                job.id,
                TENANT_CONFIG_ROLLBACK_JOB_TYPE,
                Some(&owner_token),
            )
            .await
        {
            let _ = self.release_operation_lease(tenant_id, &owner_token).await;
            return Err(error);
        }
        let transaction = match self.db.write().begin().await.map_err(database_error) {
            Ok(transaction) => transaction,
            Err(error) => {
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                return Err(error);
            }
        };
        let operation = async {
            let fence = self
                .repository
                .lock_tenant_configuration_in_txn(&transaction, tenant_id, Some(&owner_token))
                .await?;
            let mut current = self
                .repository
                .lock_transfer_in_txn(&transaction, tenant_id, transfer_id)
                .await?
                .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
            if current.rollback_background_job_id != Some(job.id)
                || current.status != tenant_config_transfer::Model::STATUS_ROLLING_BACK
            {
                return Err(AppError::Conflict("配置回滚任务已被替换".into()));
            }
            if Some(fence.configuration_version) != current.applied_configuration_version
                || Some(fence.authorization_epoch) != current.applied_authorization_epoch
            {
                return Err(AppError::Conflict(
                    "应用完成后配置已被修改，不能自动回滚".into(),
                ));
            }
            let rollback_time = self.repository.database_utc_now(&transaction).await?;
            ensure_requester_snapshot_in_txn(
                &transaction,
                tenant_id,
                &requester,
                fence,
                rollback_time,
            )
            .await?;
            ensure_rollback_references_safe(&transaction, tenant_id, transfer_id).await?;
            restore_snapshot_in_transaction(
                &transaction,
                tenant_id,
                &snapshot.resources,
                transfer_id,
                &self.target_catalog,
                rollback_time,
            )
            .await?;
            let configuration_version = self
                .repository
                .increment_configuration_version_in_txn(&transaction, tenant_id)
                .await?;
            let authorization_epoch = self
                .authorization_cache
                .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                .await?;
            let namespace_version = self
                .authorization_cache
                .record_namespace_version_in_transaction(
                    &transaction,
                    tenant_id,
                    CONFIG_CACHE_NAMESPACE,
                )
                .await?;
            current.status = tenant_config_transfer::Model::STATUS_ROLLED_BACK.to_owned();
            current.applied_configuration_version = Some(configuration_version);
            current.applied_authorization_epoch = Some(authorization_epoch);
            current.error_summary = None;
            current.updated_at = self.repository.database_utc_now(&transaction).await?;
            self.repository
                .update_transfer(&transaction, current)
                .await?;
            mark_plan_outcome(
                &transaction,
                tenant_id,
                transfer_id,
                tenant_config_transfer_item::Model::OUTCOME_ROLLED_BACK,
            )
            .await?;
            self.repository
                .release_lease_in_txn(&transaction, tenant_id, &owner_token)
                .await?;
            Ok::<_, AppError>((authorization_epoch, namespace_version))
        }
        .await;
        match operation {
            Ok((epoch, namespace_version)) => {
                if let Err(error) = transaction.commit().await.map_err(database_error) {
                    let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                    return Err(error);
                }
                self.authorization_cache
                    .sync_tenant_epoch(tenant_id, epoch)
                    .await?;
                self.authorization_cache
                    .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
                    .await
            }
            Err(error) => {
                let rollback_result = transaction.rollback().await.map_err(database_error);
                let _ = self.release_operation_lease(tenant_id, &owner_token).await;
                rollback_result?;
                Err(error)
            }
        }
    }

    async fn mark_transfer_running(
        &self,
        tenant_id: &str,
        transfer_id: i64,
        job_id: i64,
        job_type: &str,
        owner_token: Option<&str>,
    ) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        self.repository
            .lock_tenant_configuration_in_txn(&transaction, tenant_id, owner_token)
            .await?;
        let mut transfer = self
            .repository
            .lock_transfer_in_txn(&transaction, tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        match job_type {
            TENANT_CONFIG_PREVIEW_JOB_TYPE
                if transfer.preview_background_job_id == Some(job_id)
                    && matches!(
                        transfer.status.as_str(),
                        tenant_config_transfer::Model::STATUS_PREVIEW_PENDING
                            | tenant_config_transfer::Model::STATUS_PREVIEWING
                    ) =>
            {
                transfer.status = tenant_config_transfer::Model::STATUS_PREVIEWING.to_owned();
            }
            TENANT_CONFIG_APPLY_JOB_TYPE
                if transfer.apply_background_job_id == Some(job_id)
                    && matches!(
                        transfer.status.as_str(),
                        tenant_config_transfer::Model::STATUS_APPLY_PENDING
                            | tenant_config_transfer::Model::STATUS_APPLYING
                    ) =>
            {
                transfer.status = tenant_config_transfer::Model::STATUS_APPLYING.to_owned();
            }
            TENANT_CONFIG_ROLLBACK_JOB_TYPE
                if transfer.rollback_background_job_id == Some(job_id)
                    && matches!(
                        transfer.status.as_str(),
                        tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING
                            | tenant_config_transfer::Model::STATUS_ROLLING_BACK
                    ) =>
            {
                transfer.status = tenant_config_transfer::Model::STATUS_ROLLING_BACK.to_owned();
            }
            _ => {
                transaction.rollback().await.map_err(database_error)?;
                return Err(AppError::Conflict("配置迁移任务已被更新的操作取代".into()));
            }
        }
        transfer.updated_at = self.repository.database_utc_now(&transaction).await?;
        self.repository
            .update_transfer(&transaction, transfer)
            .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn bundle_requester(&self, tenant_id: &str, bundle_id: i64) -> AppResult<i64> {
        self.repository
            .find_bundle_by_id(self.db.write(), tenant_id, bundle_id)
            .await?
            .map(|bundle| bundle.created_by)
            .ok_or_else(|| AppError::NotFound("配置包导出记录不存在".into()))
    }

    async fn load_bundle_package(
        &self,
        tenant_id: &str,
        bundle_id: i64,
    ) -> AppResult<ParsedTenantConfigPackage> {
        let bundle = self
            .repository
            .find_bundle_by_id(self.db.write(), tenant_id, bundle_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置包不存在".into()))?;
        ensure_bundle_available(
            &bundle,
            self.repository.database_utc_now(self.db.write()).await?,
        )?;
        let file = self
            .file_service
            .download_config_package_internal(
                tenant_id,
                bundle
                    .file_id
                    .ok_or_else(|| AppError::Conflict("配置包文件不存在".into()))?,
            )
            .await?;
        let parsed = parse_tenant_config_package(file.data, self.package_limits()).await?;
        if bundle.sha256.as_deref() != Some(parsed.package_sha256.as_str()) {
            return Err(AppError::Conflict("配置包文件完整性校验失败".into()));
        }
        Ok(parsed)
    }

    async fn acquire_operation_lease(
        &self,
        tenant_id: &str,
        transfer_id: i64,
        owner_token: &str,
        operation: &str,
    ) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let now = self.repository.database_utc_now(&transaction).await?;
        self.repository
            .acquire_lease_in_txn(
                &transaction,
                tenant_config_lease::Model {
                    tenant_id: tenant_id.to_owned(),
                    owner_token: owner_token.to_owned(),
                    transfer_id,
                    operation: operation.to_owned(),
                    expires_at: now
                        + Duration::seconds(
                            i64::try_from(self.config.lease_seconds).unwrap_or(300),
                        ),
                    created_at: now,
                    updated_at: now,
                },
            )
            .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn release_operation_lease(&self, tenant_id: &str, owner_token: &str) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        self.repository
            .release_lease_in_txn(&transaction, tenant_id, owner_token)
            .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn renew_operation_lease(&self, tenant_id: &str, owner_token: &str) -> AppResult<()> {
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let operation = async {
            let now = self.repository.database_utc_now(&transaction).await?;
            let renewed = self
                .repository
                .renew_lease_in_txn(
                    &transaction,
                    tenant_id,
                    owner_token,
                    now + Duration::seconds(
                        i64::try_from(self.config.lease_seconds).unwrap_or(300),
                    ),
                )
                .await?;
            if !renewed {
                return Err(AppError::Conflict("配置迁移租约已失效".into()));
            }
            Ok::<_, AppError>(())
        }
        .await;
        match operation {
            Ok(()) => transaction.commit().await.map_err(database_error),
            Err(error) => {
                transaction.rollback().await.map_err(database_error)?;
                Err(error)
            }
        }
    }

    async fn sync_committed_cache_state(
        &self,
        tenant_id: &str,
        transfer: &tenant_config_transfer::Model,
    ) -> AppResult<()> {
        let epoch = transfer
            .applied_authorization_epoch
            .ok_or_else(|| AppError::Conflict("迁移终态缺少授权纪元".into()))?;
        let namespace_version = CacheNamespaceVersionRepository
            .find_version(self.db.write(), tenant_id, CONFIG_CACHE_NAMESPACE)
            .await?;
        self.authorization_cache
            .sync_tenant_epoch(tenant_id, epoch)
            .await?;
        self.authorization_cache
            .sync_namespace_version(tenant_id, CONFIG_CACHE_NAMESPACE, namespace_version)
            .await
    }
}

enum TransferOperationRequest {
    Preview,
    Apply(ApplyTenantConfigTransferCommand),
    Rollback,
}

async fn clear_superseded_dead_operation_jobs<C>(
    db: &C,
    transfer: &mut tenant_config_transfer::Model,
    operation: &TransferOperationRequest,
) -> AppResult<()>
where
    C: ConnectionTrait,
{
    let candidates = match operation {
        TransferOperationRequest::Preview => [
            transfer.apply_background_job_id,
            transfer.rollback_background_job_id,
        ],
        TransferOperationRequest::Apply(_) => [
            transfer.preview_background_job_id,
            transfer.rollback_background_job_id,
        ],
        TransferOperationRequest::Rollback => [
            transfer.preview_background_job_id,
            transfer.apply_background_job_id,
        ],
    }
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(());
    }
    let dead_ids = background_job::Entity::find()
        .filter(background_job::Column::Id.is_in(candidates))
        .filter(background_job::Column::TenantId.eq(&transfer.tenant_id))
        .filter(background_job::Column::Status.eq(background_job::Model::STATUS_DEAD))
        .all(db)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|job| job.id)
        .collect::<BTreeSet<_>>();

    // 新操作只废止其他类型的死信执行资格；成功任务指针、应用版本、快照和回滚窗口
    // 均予以保留，因此不会破坏合法回滚链或历史任务关联。
    match operation {
        TransferOperationRequest::Preview => {
            if transfer
                .apply_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.apply_background_job_id = None;
            }
            if transfer
                .rollback_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.rollback_background_job_id = None;
            }
        }
        TransferOperationRequest::Apply(_) => {
            if transfer
                .preview_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.preview_background_job_id = None;
            }
            if transfer
                .rollback_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.rollback_background_job_id = None;
            }
        }
        TransferOperationRequest::Rollback => {
            if transfer
                .preview_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.preview_background_job_id = None;
            }
            if transfer
                .apply_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.apply_background_job_id = None;
            }
        }
    }
    Ok(())
}

macro_rules! transfer_job_handler {
    ($name:ident, $job_type:expr, $method:ident) => {
        pub struct $name {
            service: Arc<TenantConfigTransferService>,
        }

        impl $name {
            pub fn new(service: Arc<TenantConfigTransferService>) -> Self {
                Self { service }
            }
        }

        #[async_trait]
        impl JobHandler for $name {
            fn job_type(&self) -> &'static str {
                $job_type
            }

            async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
                self.service.$method(job).await
            }

            fn should_dead_letter(&self, error: &AppError) -> bool {
                matches!(
                    error,
                    AppError::Validation(_)
                        | AppError::Authorization(_)
                        | AppError::NotFound(_)
                        | AppError::Conflict(_)
                        | AppError::PayloadTooLarge(_)
                )
            }
        }
    };
}

transfer_job_handler!(
    TenantConfigExportJobHandler,
    TENANT_CONFIG_EXPORT_JOB_TYPE,
    execute_export
);
transfer_job_handler!(
    TenantConfigPreviewJobHandler,
    TENANT_CONFIG_PREVIEW_JOB_TYPE,
    execute_preview
);
transfer_job_handler!(
    TenantConfigApplyJobHandler,
    TENANT_CONFIG_APPLY_JOB_TYPE,
    execute_apply
);
transfer_job_handler!(
    TenantConfigRollbackJobHandler,
    TENANT_CONFIG_ROLLBACK_JOB_TYPE,
    execute_rollback
);

async fn ensure_requester_snapshot_in_txn(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    requester: &super::user_service::CurrentAuthorization,
    fence: ryframe_db::TenantConfigurationFence,
    database_now: DateTime<Utc>,
) -> AppResult<()> {
    if requester.tenant.tenant_id != tenant_id
        || requester.tenant.authorization_epoch != fence.authorization_epoch
    {
        return Err(AppError::Conflict(
            "申请人授权在执行期间发生变化，请重新发起操作".into(),
        ));
    }
    let tenant = tenant::Entity::find()
        .filter(tenant::Column::TenantId.eq(tenant_id))
        .one(transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
    if !tenant.is_available(database_now) {
        return Err(AppError::Authorization("申请人的租户已停用或到期".into()));
    }
    let current_user = user::Entity::find_by_id(requester.actor.user_id)
        .filter(user::Column::TenantId.eq(tenant_id))
        .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
        .one(transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::NotFound("操作申请人不存在".into()))?;
    if !current_user.is_enabled()
        || current_user.authorization_version != requester.user.authorization_version
    {
        return Err(AppError::Authorization(
            "申请人账号或授权在执行期间发生变化".into(),
        ));
    }
    Ok(())
}

async fn ensure_role_quota_for_plan_in_txn(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    plan_items: &[tenant_config_transfer_item::Model],
) -> AppResult<()> {
    let create_count = plan_items
        .iter()
        .filter(|item| {
            item.resource_type == "role"
                && item.action == tenant_config_transfer_item::Model::ACTION_CREATE
        })
        .count() as u64;
    if create_count == 0 {
        return Ok(());
    }
    let tenant = tenant::Entity::find()
        .filter(tenant::Column::TenantId.eq(tenant_id))
        .one(transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
    let active_count = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .count(transaction)
        .await
        .map_err(database_error)?;
    let limit = u64::try_from(tenant.max_roles).unwrap_or_default();
    if limit > 0 && active_count.saturating_add(create_count) > limit {
        return Err(AppError::Validation(format!(
            "配置应用将使租户角色数超过上限（当前 {active_count}，新增 {create_count}，上限 {limit}）"
        )));
    }
    Ok(())
}

async fn apply_resources_in_transaction(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &TenantConfigPackageResources,
    plan_items: &[tenant_config_transfer_item::Model],
    now: DateTime<Utc>,
) -> AppResult<()> {
    let changed = plan_items
        .iter()
        .filter(|item| {
            matches!(
                item.action.as_str(),
                tenant_config_transfer_item::Model::ACTION_CREATE
                    | tenant_config_transfer_item::Model::ACTION_UPDATE
            )
        })
        .map(|item| {
            (
                item.resource_type.clone(),
                normalize_resource_stable_key(&item.resource_type, &item.stable_key),
            )
        })
        .collect::<BTreeSet<_>>();
    let existing_departments = dept::Entity::find()
        .filter(dept::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let mut department_ids = build_department_paths(&existing_departments)?
        .into_iter()
        .map(|(id, path)| (normalize_department_path(&path), id))
        .collect::<BTreeMap<_, _>>();
    let mut department_ancestors = existing_departments
        .iter()
        .map(|item| (item.id, item.ancestors.clone()))
        .collect::<BTreeMap<_, _>>();
    for item in &resources.departments {
        let stable_key = join_path(&item.path);
        if !changed.contains(&(
            "department".to_owned(),
            normalize_resource_stable_key("department", &stable_key),
        )) {
            continue;
        }
        let parent_path = &item.path[..item.path.len().saturating_sub(1)];
        let parent_id = if parent_path.is_empty() {
            None
        } else {
            Some(
                *department_ids
                    .get(&normalize_department_path(parent_path))
                    .ok_or_else(|| AppError::Conflict("部门父路径不存在".into()))?,
            )
        };
        let ancestors = parent_id.map_or_else(
            || "0".to_owned(),
            |id| {
                department_ancestors
                    .get(&id)
                    .map(|ancestors| format!("{ancestors},{id}"))
                    .unwrap_or_else(|| format!("0,{id}"))
            },
        );
        if let Some(id) = department_ids
            .get(&normalize_department_path(&item.path))
            .copied()
        {
            let mut model = dept::Entity::find_by_id(id)
                .one(transaction)
                .await
                .map_err(database_error)?
                .ok_or_else(|| AppError::Conflict("部门已不存在".into()))?;
            model.name = item.path.last().cloned().unwrap_or_default();
            model.parent_id = parent_id;
            model.ancestors = ancestors.clone();
            model.sort = item.sort;
            model.status = item.status.clone();
            model.remark = item.remark.clone();
            model.del_flag = dept::Model::DEL_FLAG_NORMAL.to_owned();
            model.updated_at = now;
            dept::ActiveModel::from(model)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
            department_ancestors.insert(id, ancestors);
        } else {
            let id = try_next_snowflake_id()?;
            dept::ActiveModel::from(dept::Model {
                id,
                tenant_id: tenant_id.to_owned(),
                name: item.path.last().cloned().unwrap_or_default(),
                parent_id,
                ancestors: ancestors.clone(),
                sort: item.sort,
                status: item.status.clone(),
                remark: item.remark.clone(),
                del_flag: dept::Model::DEL_FLAG_NORMAL.to_owned(),
                created_at: now,
                updated_at: now,
            })
            .insert(transaction)
            .await
            .map_err(database_error)?;
            department_ids.insert(normalize_department_path(&item.path), id);
            department_ancestors.insert(id, ancestors);
        }
    }

    upsert_simple_resources(transaction, tenant_id, resources, &changed, now).await?;
    upsert_permissions(
        transaction,
        tenant_id,
        &resources.permissions,
        &changed,
        now,
    )
    .await?;
    upsert_menus(transaction, tenant_id, &resources.menus, &changed, now).await?;
    upsert_roles_and_relations(
        transaction,
        tenant_id,
        &resources.roles,
        &department_ids,
        &changed,
        now,
    )
    .await
}

async fn upsert_simple_resources(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &TenantConfigPackageResources,
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    for item in &resources.posts {
        if !changed.contains(&("post".to_owned(), normalize_stable_key(&item.code))) {
            continue;
        }
        let existing = post::Entity::find()
            .filter(post::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.code) == normalize_stable_key(&item.code)
            });
        let model = post::Model {
            id: existing
                .as_ref()
                .map(|item| item.id)
                .unwrap_or(try_next_snowflake_id()?),
            tenant_id: tenant_id.to_owned(),
            name: item.name.clone(),
            code: item.code.clone(),
            sort: item.sort,
            status: item.status.clone(),
            remark: item.remark.clone(),
            del_flag: post::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(
            transaction,
            existing.is_some(),
            post::ActiveModel::from(model),
        )
        .await?;
    }
    for item in &resources.dict_types {
        if !changed.contains(&("dict_type".to_owned(), normalize_stable_key(&item.code))) {
            continue;
        }
        let existing = dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.code) == normalize_stable_key(&item.code)
            });
        let model = dict_type::Model {
            id: existing
                .as_ref()
                .map(|item| item.id)
                .unwrap_or(try_next_snowflake_id()?),
            tenant_id: tenant_id.to_owned(),
            name: item.name.clone(),
            code: item.code.clone(),
            status: item.status.clone(),
            remark: item.remark.clone(),
            del_flag: dict_type::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(
            transaction,
            existing.is_some(),
            dict_type::ActiveModel::from(model),
        )
        .await?;
    }
    for item in &resources.dict_data {
        let key = format!("{}:{}:{}", item.type_code.len(), item.type_code, item.value);
        if !changed.contains(&("dict_data".to_owned(), normalize_stable_key(&key))) {
            continue;
        }
        let existing = dict_data::Entity::find()
            .filter(dict_data::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.type_code) == normalize_stable_key(&item.type_code)
                    && normalize_stable_key(&candidate.value) == normalize_stable_key(&item.value)
            });
        let model = dict_data::Model {
            id: existing
                .as_ref()
                .map(|item| item.id)
                .unwrap_or(try_next_snowflake_id()?),
            tenant_id: tenant_id.to_owned(),
            type_code: item.type_code.clone(),
            label: item.label.clone(),
            value: item.value.clone(),
            sort: item.sort,
            status: item.status.clone(),
            css_class: item.css_class.clone(),
            remark: item.remark.clone(),
            del_flag: dict_data::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(
            transaction,
            existing.is_some(),
            dict_data::ActiveModel::from(model),
        )
        .await?;
    }
    for item in &resources.configs {
        if !changed.contains(&("config".to_owned(), normalize_stable_key(&item.key))) {
            continue;
        }
        if super::tenant_config_package::is_sensitive_config_key(&item.key) {
            return Err(AppError::Validation("敏感参数不能应用".into()));
        }
        let existing = config::Entity::find()
            .filter(config::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.key) == normalize_stable_key(&item.key)
            });
        let model = config::Model {
            id: existing
                .as_ref()
                .map(|item| item.id)
                .unwrap_or(try_next_snowflake_id()?),
            tenant_id: tenant_id.to_owned(),
            name: item.name.clone(),
            key: item.key.clone(),
            value: item.value.clone(),
            portable: true,
            remark: item.remark.clone(),
            del_flag: config::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(
            transaction,
            existing.is_some(),
            config::ActiveModel::from(model),
        )
        .await?;
    }
    Ok(())
}

async fn save_model<A>(
    transaction: &sea_orm::DatabaseTransaction,
    exists: bool,
    model: A,
) -> AppResult<()>
where
    A: ActiveModelTrait + ActiveModelBehavior + Send,
    <A as ActiveModelTrait>::Entity: EntityTrait,
    <<A as ActiveModelTrait>::Entity as EntityTrait>::Model: IntoActiveModel<A>,
{
    if exists {
        model
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)?;
    } else {
        model.insert(transaction).await.map_err(database_error)?;
    }
    Ok(())
}

async fn upsert_permissions(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &[PortablePermission],
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let existing = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let mut by_code = existing
        .into_iter()
        .map(|item| (normalize_stable_key(&item.code), item))
        .collect::<BTreeMap<_, _>>();
    let source_codes = resources
        .iter()
        .map(|item| normalize_stable_key(&item.code))
        .collect::<BTreeSet<_>>();
    let mut remaining = resources.iter().collect::<Vec<_>>();
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut deferred = Vec::new();
        for item in remaining {
            if permission_contains_wildcard(&item.code) || is_platform_only_permission(&item.code) {
                return Err(AppError::Validation(
                    "平台专用权限或超级通配权限不能应用".into(),
                ));
            }
            let parent_id = match item.parent_code.as_deref() {
                Some(parent_code) => match by_code.get(&normalize_stable_key(parent_code)) {
                    Some(parent) => Some(parent.id),
                    None if source_codes.contains(&normalize_stable_key(parent_code)) => {
                        deferred.push(item);
                        continue;
                    }
                    None => {
                        return Err(AppError::Conflict(format!(
                            "权限 {} 的父权限不存在",
                            item.code
                        )));
                    }
                },
                None => None,
            };
            if !changed.contains(&("permission".to_owned(), normalize_stable_key(&item.code))) {
                continue;
            }
            let old = by_code.get(&normalize_stable_key(&item.code)).cloned();
            if let Some(old) = &old
                && old.perm_type != item.permission_type
            {
                return Err(AppError::Conflict(format!(
                    "目标权限 {} 的类型与配置包不一致",
                    item.code
                )));
            }
            let model = permission::Model {
                id: old
                    .as_ref()
                    .map(|value| value.id)
                    .unwrap_or(try_next_snowflake_id()?),
                tenant_id: tenant_id.to_owned(),
                name: item.name.clone(),
                code: item.code.clone(),
                parent_id,
                perm_type: old
                    .as_ref()
                    .map(|value| value.perm_type.clone())
                    .unwrap_or_else(|| item.permission_type.clone()),
                icon: item.icon.clone(),
                sort: item.sort,
                status: item.status.clone(),
                created_at: old.as_ref().map(|value| value.created_at).unwrap_or(now),
                updated_at: now,
            };
            save_model(
                transaction,
                old.is_some(),
                permission::ActiveModel::from(model.clone()),
            )
            .await?;
            by_code.insert(normalize_stable_key(&item.code), model);
        }
        if deferred.len() == before {
            return Err(AppError::Conflict(
                "权限父子层级存在循环或缺少父权限".into(),
            ));
        }
        remaining = deferred;
    }
    Ok(())
}

async fn upsert_menus(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &[PortableMenu],
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let permissions = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let permission_ids = permissions
        .into_iter()
        .map(|item| (normalize_stable_key(&item.code), item.id))
        .collect::<BTreeMap<_, _>>();
    let existing = menu::Entity::find()
        .filter(menu::Column::TenantId.eq(tenant_id))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let existing_permission_codes = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|item| (item.id, item.code))
        .collect::<BTreeMap<_, _>>();
    let stable_keys = build_menu_stable_keys(&existing, &existing_permission_codes)?;
    let mut by_key = existing
        .into_iter()
        .filter_map(|item| {
            stable_keys
                .get(&item.id)
                .cloned()
                .map(|key| (normalize_stable_key(&key), item))
        })
        .collect::<BTreeMap<_, _>>();
    let source_keys = resources
        .iter()
        .map(|item| normalize_stable_key(&item.stable_key))
        .collect::<BTreeSet<_>>();
    let mut remaining = resources.iter().collect::<Vec<_>>();
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut deferred = Vec::new();
        for item in remaining {
            let parent_id = match item.parent_stable_key.as_deref() {
                Some(parent_key) => match by_key.get(&normalize_stable_key(parent_key)) {
                    Some(parent) if parent.menu_type == menu::Model::MENU_TYPE_BUTTON => {
                        return Err(AppError::Validation(format!(
                            "菜单 {} 不能将操作菜单作为父菜单",
                            item.stable_key
                        )));
                    }
                    Some(parent) => Some(parent.id),
                    None if source_keys.contains(&normalize_stable_key(parent_key)) => {
                        deferred.push(item);
                        continue;
                    }
                    None => {
                        return Err(AppError::Conflict(format!(
                            "菜单 {} 的父菜单不存在",
                            item.stable_key
                        )));
                    }
                },
                None => None,
            };
            let perm_id = item
                .permission_code
                .as_ref()
                .map(|code| {
                    permission_ids
                        .get(&normalize_stable_key(code))
                        .copied()
                        .ok_or_else(|| AppError::Conflict(format!("菜单引用的权限 {code} 不存在")))
                })
                .transpose()?;
            match item.menu_type.as_str() {
                menu::Model::MENU_TYPE_DIR => {
                    if item.route_key.is_none() {
                        return Err(AppError::Validation("目录菜单必须声明 route_key".into()));
                    }
                }
                menu::Model::MENU_TYPE_MENU => {
                    if item.route_key.is_none() || perm_id.is_none() {
                        return Err(AppError::Validation(
                            "页面菜单必须声明 route_key 并绑定权限".into(),
                        ));
                    }
                }
                menu::Model::MENU_TYPE_BUTTON => {
                    if item.route_key.is_some() || perm_id.is_none() || parent_id.is_none() {
                        return Err(AppError::Validation(
                            "操作菜单必须绑定权限和父菜单，且不能声明 route_key".into(),
                        ));
                    }
                }
                _ => return Err(AppError::Validation("配置包菜单类型不受支持".into())),
            }
            if !changed.contains(&("menu".to_owned(), normalize_stable_key(&item.stable_key))) {
                continue;
            }
            let old = by_key.get(&normalize_stable_key(&item.stable_key)).cloned();
            let model = menu::Model {
                id: old
                    .as_ref()
                    .map(|value| value.id)
                    .unwrap_or(try_next_snowflake_id()?),
                tenant_id: tenant_id.to_owned(),
                name: item.name.clone(),
                parent_id,
                menu_type: item.menu_type.clone(),
                perm_id,
                route_key: item.route_key.clone(),
                icon: item.icon.clone(),
                sort: item.sort,
                visible: item.visible,
                status: item.status.clone(),
                remark: item.remark.clone(),
                del_flag: menu::Model::DEL_FLAG_NORMAL.to_owned(),
                created_at: old.as_ref().map(|value| value.created_at).unwrap_or(now),
                updated_at: now,
            };
            save_model(
                transaction,
                old.is_some(),
                menu::ActiveModel::from(model.clone()),
            )
            .await?;
            by_key.insert(normalize_stable_key(&item.stable_key), model);
        }
        if deferred.len() == before {
            return Err(AppError::Conflict(
                "菜单父子层级存在循环或缺少父菜单".into(),
            ));
        }
        remaining = deferred;
    }
    Ok(())
}

async fn upsert_roles_and_relations(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &[PortableRole],
    department_ids: &BTreeMap<Vec<String>, i64>,
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let permission_ids = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|item| (normalize_stable_key(&item.code), item.id))
        .collect::<BTreeMap<_, _>>();
    for item in resources {
        if !changed.contains(&("role".to_owned(), normalize_stable_key(&item.code))) {
            continue;
        }
        if permission_contains_wildcard(&item.code)
            || item
                .permission_codes
                .iter()
                .any(|code| permission_contains_wildcard(code) || is_platform_only_permission(code))
        {
            return Err(AppError::Validation(
                "超级角色或通配权限不能通过配置包迁移".into(),
            ));
        }
        let old = role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.code) == normalize_stable_key(&item.code)
            });
        if old.as_ref().is_some_and(|value| value.is_super == 1) {
            return Err(AppError::Conflict("超级角色不能被配置包覆盖".into()));
        }
        let role_id = old
            .as_ref()
            .map(|value| value.id)
            .unwrap_or(try_next_snowflake_id()?);
        let model = role::Model {
            id: role_id,
            tenant_id: tenant_id.to_owned(),
            name: item.name.clone(),
            code: item.code.clone(),
            is_super: 0,
            data_scope: item.data_scope.clone(),
            status: item.status.clone(),
            sort: item.sort,
            remark: item.remark.clone(),
            del_flag: role::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: old.as_ref().map(|value| value.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(transaction, old.is_some(), role::ActiveModel::from(model)).await?;
        role_permission::Entity::delete_many()
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .filter(role_permission::Column::RoleId.eq(role_id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        let relations = item
            .permission_codes
            .iter()
            .map(|code| {
                let perm_id = permission_ids
                    .get(&normalize_stable_key(code))
                    .copied()
                    .ok_or_else(|| AppError::Conflict(format!("角色引用的权限 {code} 不存在")))?;
                Ok(role_permission::ActiveModel::from(role_permission::Model {
                    tenant_id: tenant_id.to_owned(),
                    role_id,
                    perm_id,
                }))
            })
            .collect::<AppResult<Vec<_>>>()?;
        if !relations.is_empty() {
            role_permission::Entity::insert_many(relations)
                .exec(transaction)
                .await
                .map_err(database_error)?;
        }
        role_dept::Entity::delete_many()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::RoleId.eq(role_id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        let departments = item
            .custom_department_paths
            .iter()
            .map(|path| {
                let dept_id = department_ids
                    .get(&normalize_department_path(path))
                    .copied()
                    .ok_or_else(|| {
                        AppError::Conflict(format!("角色引用的部门路径 {} 不存在", join_path(path)))
                    })?;
                Ok(role_dept::ActiveModel::from(role_dept::Model {
                    tenant_id: tenant_id.to_owned(),
                    role_id,
                    dept_id,
                }))
            })
            .collect::<AppResult<Vec<_>>>()?;
        if !departments.is_empty() {
            role_dept::Entity::insert_many(departments)
                .exec(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    Ok(())
}

async fn ensure_rollback_references_safe(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    transfer_id: i64,
) -> AppResult<()> {
    let created = load_created_item_keys(transaction, tenant_id, transfer_id).await?;
    let created_roles = created_keys(&created, "role").collect::<BTreeSet<_>>();
    let extra_role_ids = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter(|item| created_roles.contains(&normalize_stable_key(&item.code)))
        .map(|item| item.id)
        .collect::<Vec<_>>();
    if !extra_role_ids.is_empty()
        && user_role::Entity::find()
            .filter(user_role::Column::TenantId.eq(tenant_id))
            .filter(user_role::Column::RoleId.is_in(extra_role_ids))
            .count(transaction)
            .await
            .map_err(database_error)?
            > 0
    {
        return Err(AppError::Conflict(
            "应用创建的角色已经分配给用户，不能自动回滚".into(),
        ));
    }
    let created_departments = created_keys(&created, "department").collect::<BTreeSet<_>>();
    let current_models = dept::Entity::find()
        .filter(dept::Column::TenantId.eq(tenant_id))
        .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let paths = build_department_paths(&current_models)?;
    let extra_dept_ids = paths
        .into_iter()
        .filter_map(|(id, path)| {
            created_departments
                .contains(&normalize_resource_stable_key(
                    "department",
                    &join_path(&path),
                ))
                .then_some(id)
        })
        .collect::<Vec<_>>();
    if !extra_dept_ids.is_empty()
        && user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::DeptId.is_in(extra_dept_ids.clone()))
            .count(transaction)
            .await
            .map_err(database_error)?
            > 0
    {
        return Err(AppError::Conflict(
            "应用创建的部门已经被用户引用，不能自动回滚".into(),
        ));
    }
    if !extra_dept_ids.is_empty() {
        let migrated_role_keys = tenant_config_transfer_item::Entity::find()
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
            .filter(tenant_config_transfer_item::Column::ResourceType.eq("role"))
            .filter(tenant_config_transfer_item::Column::Action.is_in([
                tenant_config_transfer_item::Model::ACTION_CREATE,
                tenant_config_transfer_item::Model::ACTION_UPDATE,
            ]))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(|item| normalize_stable_key(&item.stable_key))
            .collect::<BTreeSet<_>>();
        let migrated_role_ids = role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .filter_map(|item| {
                migrated_role_keys
                    .contains(&normalize_stable_key(&item.code))
                    .then_some(item.id)
            })
            .collect::<BTreeSet<_>>();
        let unexpected_reference = role_dept::Entity::find()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::DeptId.is_in(extra_dept_ids))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .any(|relation| !migrated_role_ids.contains(&relation.role_id));
        if unexpected_reference {
            return Err(AppError::Conflict(
                "应用创建的部门已经被迁移范围外的角色数据范围引用，不能自动回滚".into(),
            ));
        }
    }
    Ok(())
}

async fn load_created_item_keys(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    transfer_id: i64,
) -> AppResult<BTreeMap<String, BTreeSet<String>>> {
    let items = tenant_config_transfer_item::Entity::find()
        .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
        .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
        .filter(
            tenant_config_transfer_item::Column::Action
                .eq(tenant_config_transfer_item::Model::ACTION_CREATE),
        )
        .all(transaction)
        .await
        .map_err(database_error)?;
    let mut created = BTreeMap::<String, BTreeSet<String>>::new();
    for item in items {
        let normalized_key = normalize_resource_stable_key(&item.resource_type, &item.stable_key);
        created
            .entry(item.resource_type)
            .or_default()
            .insert(normalized_key);
    }
    Ok(created)
}

fn created_keys<'a>(
    created: &'a BTreeMap<String, BTreeSet<String>>,
    resource_type: &str,
) -> impl Iterator<Item = String> + 'a {
    created.get(resource_type).into_iter().flatten().cloned()
}

async fn restore_snapshot_in_transaction(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    snapshot: &TenantConfigPackageResources,
    transfer_id: i64,
    target_catalog: &TenantConfigTargetCatalog,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let current = load_resources_on(transaction, tenant_id).await?;
    let mut descriptions = Vec::new();
    compare_resources(
        snapshot,
        &current,
        &target_catalog.page_routes,
        &target_catalog.api_permission_codes,
        &mut descriptions,
    )?;
    if descriptions.iter().any(|item| {
        matches!(
            item.action,
            tenant_config_transfer_item::Model::ACTION_BLOCKED
                | tenant_config_transfer_item::Model::ACTION_CONFLICT
        )
    }) {
        return Err(AppError::Conflict(
            "目标权限或路由目录已经变化，配置快照不能完整回滚".into(),
        ));
    }
    let plan_items = descriptions
        .into_iter()
        .map(|item| tenant_config_transfer_item::Model {
            id: 0,
            tenant_id: tenant_id.to_owned(),
            transfer_id: 0,
            resource_type: item.resource_type.to_owned(),
            stable_key: item.stable_key,
            display_name: item.display_name,
            action: item.action.to_owned(),
            outcome: tenant_config_transfer_item::Model::OUTCOME_PENDING.to_owned(),
            detail_code: item.detail_code.map(str::to_owned),
            detail: item.detail,
            created_at: now,
            updated_at: now,
        })
        .collect::<Vec<_>>();
    apply_resources_in_transaction(transaction, tenant_id, snapshot, &plan_items, now).await?;
    let created = load_created_item_keys(transaction, tenant_id, transfer_id).await?;

    let created_role_codes = created_keys(&created, "role").collect::<BTreeSet<_>>();
    let extra_roles = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::IsSuper.eq(0))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter(|item| created_role_codes.contains(&normalize_stable_key(&item.code)))
        .collect::<Vec<_>>();
    for item in extra_roles {
        role_permission::Entity::delete_many()
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .filter(role_permission::Column::RoleId.eq(item.id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        role_dept::Entity::delete_many()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::RoleId.eq(item.id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        let mut model = item;
        model.del_flag = role::Model::DEL_FLAG_DELETED.to_owned();
        model.updated_at = now;
        role::ActiveModel::from(model)
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)?;
    }

    let permission_codes = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|item| (item.id, item.code))
        .collect::<BTreeMap<_, _>>();
    let current_menus = menu::Entity::find()
        .filter(menu::Column::TenantId.eq(tenant_id))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let current_menu_keys = build_menu_stable_keys(&current_menus, &permission_codes)?;
    let created_menu_keys = created_keys(&created, "menu").collect::<BTreeSet<_>>();
    for mut item in current_menus {
        if current_menu_keys
            .get(&item.id)
            .is_some_and(|key| created_menu_keys.contains(&normalize_stable_key(key)))
        {
            item.del_flag = menu::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            menu::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }

    let created_permission_codes = created_keys(&created, "permission").collect::<BTreeSet<_>>();
    let extra_permissions = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter(|item| created_permission_codes.contains(&normalize_stable_key(&item.code)))
        .collect::<Vec<_>>();
    for item in extra_permissions {
        let referenced = role_permission::Entity::find()
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .filter(role_permission::Column::PermId.eq(item.id))
            .count(transaction)
            .await
            .map_err(database_error)?
            + menu::Entity::find()
                .filter(menu::Column::TenantId.eq(tenant_id))
                .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
                .filter(menu::Column::PermId.eq(item.id))
                .count(transaction)
                .await
                .map_err(database_error)?;
        if referenced > 0 {
            return Err(AppError::Conflict(
                "应用创建的权限仍被引用，不能自动回滚".into(),
            ));
        }
        permission::Entity::delete_by_id(item.id)
            .exec(transaction)
            .await
            .map_err(database_error)?;
    }

    soft_delete_simple_extras(transaction, tenant_id, &created, now).await?;
    soft_delete_department_extras(transaction, tenant_id, &created, now).await
}

async fn soft_delete_simple_extras(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    created: &BTreeMap<String, BTreeSet<String>>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let post_codes = created_keys(created, "post").collect::<BTreeSet<_>>();
    for mut item in post::Entity::find()
        .filter(post::Column::TenantId.eq(tenant_id))
        .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?
    {
        if post_codes.contains(&normalize_stable_key(&item.code)) {
            item.del_flag = post::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            post::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    let data_keys = created_keys(created, "dict_data").collect::<BTreeSet<_>>();
    for mut item in dict_data::Entity::find()
        .filter(dict_data::Column::TenantId.eq(tenant_id))
        .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?
    {
        let stable_key = format!("{}:{}:{}", item.type_code.len(), item.type_code, item.value);
        if data_keys.contains(&normalize_stable_key(&stable_key)) {
            item.del_flag = dict_data::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            dict_data::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    let type_codes = created_keys(created, "dict_type").collect::<BTreeSet<_>>();
    for mut item in dict_type::Entity::find()
        .filter(dict_type::Column::TenantId.eq(tenant_id))
        .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?
    {
        if type_codes.contains(&normalize_stable_key(&item.code)) {
            item.del_flag = dict_type::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            dict_type::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    let config_keys = created_keys(created, "config").collect::<BTreeSet<_>>();
    for mut item in config::Entity::find()
        .filter(config::Column::TenantId.eq(tenant_id))
        .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
        .filter(config::Column::Portable.eq(true))
        .all(transaction)
        .await
        .map_err(database_error)?
    {
        if config_keys.contains(&normalize_stable_key(&item.key)) {
            item.del_flag = config::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            config::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    Ok(())
}

async fn soft_delete_department_extras(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    created: &BTreeMap<String, BTreeSet<String>>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let created_paths = created_keys(created, "department").collect::<BTreeSet<_>>();
    let models = dept::Entity::find()
        .filter(dept::Column::TenantId.eq(tenant_id))
        .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let paths = build_department_paths(&models)?;
    let mut extras = models
        .into_iter()
        .filter_map(|model| {
            paths
                .get(&model.id)
                .filter(|path| {
                    created_paths.contains(&normalize_resource_stable_key(
                        "department",
                        &join_path(path),
                    ))
                })
                .map(|path| (path.len(), model))
        })
        .collect::<Vec<_>>();
    extras.sort_by_key(|item| std::cmp::Reverse(item.0));
    for (_, mut item) in extras {
        item.del_flag = dept::Model::DEL_FLAG_DELETED.to_owned();
        item.updated_at = now;
        dept::ActiveModel::from(item)
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)?;
    }
    Ok(())
}

async fn mark_plan_outcome(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    transfer_id: i64,
    outcome: &str,
) -> AppResult<()> {
    tenant_config_transfer_item::Entity::update_many()
        .col_expr(
            tenant_config_transfer_item::Column::Outcome,
            Expr::value(outcome),
        )
        .col_expr(
            tenant_config_transfer_item::Column::UpdatedAt,
            Expr::cust("UTC_TIMESTAMP(6)"),
        )
        .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
        .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
        .filter(
            tenant_config_transfer_item::Column::Action
                .ne(tenant_config_transfer_item::Model::ACTION_UNCHANGED),
        )
        .exec(transaction)
        .await
        .map_err(database_error)?;
    if outcome == tenant_config_transfer_item::Model::OUTCOME_APPLIED {
        tenant_config_transfer_item::Entity::update_many()
            .col_expr(
                tenant_config_transfer_item::Column::Outcome,
                Expr::value(tenant_config_transfer_item::Model::OUTCOME_SKIPPED),
            )
            .col_expr(
                tenant_config_transfer_item::Column::UpdatedAt,
                Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
            .filter(
                tenant_config_transfer_item::Column::Action
                    .eq(tenant_config_transfer_item::Model::ACTION_UNCHANGED),
            )
            .exec(transaction)
            .await
            .map_err(database_error)?;
    }
    Ok(())
}

fn ensure_preview_identity(
    transfer: &tenant_config_transfer::Model,
    source: &ParsedTenantConfigPackage,
    target: &TenantConfigPackageResources,
    fence: ryframe_db::TenantConfigurationFence,
) -> AppResult<()> {
    if transfer.target_configuration_version != fence.configuration_version
        || transfer.target_authorization_epoch != fence.authorization_epoch
    {
        return Err(AppError::Conflict("目标配置已经变化，请重新预览".into()));
    }
    let expected = sha256_json(&PlanHashInput {
        resources_sha256: &source.manifest.resources_sha256,
        target_resources_sha256: sha256_hex(&canonical_resources(target)?),
        target_configuration_version: fence.configuration_version,
        target_authorization_epoch: fence.authorization_epoch,
    })?;
    if transfer.plan_hash.as_deref() != Some(expected.as_str()) {
        return Err(AppError::Conflict("预览计划已经失效，请重新预览".into()));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> AppResult<()> {
    if value.len() == 64 && value.bytes().all(|value| value.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::Validation(
            "幂等键哈希必须是 64 位十六进制 SHA-256".into(),
        ))
    }
}

fn transfer_request_fingerprint(request_kind: &str, bundle_id: i64) -> String {
    sha256_hex(format!("{request_kind}:{bundle_id}").as_bytes())
}

fn ensure_transfer_request_identity(
    transfer: &tenant_config_transfer::Model,
    request_kind: &str,
    request_fingerprint: &str,
) -> AppResult<()> {
    if transfer.request_kind == request_kind && transfer.request_fingerprint == request_fingerprint
    {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "Idempotency-Key 已用于其他配置迁移请求".into(),
        ))
    }
}

fn parse_file_id(value: &str) -> AppResult<i64> {
    value
        .parse::<i64>()
        .map_err(|_| AppError::Internal("文件标识格式无效".into()))
}

/// 在业务引用写入前锁定内部文件，并恢复仍处于宽限期的去重文件。
///
/// 调用方必须先持有租户配置栅栏；文件锁用于和延迟清理声明串行化。
async fn ensure_config_package_file_ready_in_txn(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    file_id: i64,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let file = FileRepository
        .find_by_id_any_status_for_update(transaction, tenant_id, file_id)
        .await?
        .ok_or_else(|| AppError::Conflict("配置包文件不存在或已经清理".into()))?;
    if file.bucket != CONFIG_PACKAGE_BUCKET {
        return Err(AppError::Authorization("配置包文件存储边界不匹配".into()));
    }
    if file.upload_status == ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_READY {
        return Ok(());
    }
    if file.upload_status == ryframe_db::entities::sys_file::Model::UPLOAD_STATUS_CLEANUP
        && FileRepository
            .restore_file_for_reference_in_txn(
                transaction,
                tenant_id,
                file_id,
                CONFIG_PACKAGE_BUCKET,
                now,
            )
            .await?
    {
        return Ok(());
    }
    Err(AppError::Conflict(
        "配置包文件尚未就绪或清理宽限期已经结束".into(),
    ))
}

#[allow(clippy::too_many_arguments)]
fn new_transfer_model(
    tenant_id: &str,
    bundle_id: i64,
    idempotency_key_hash: &str,
    request_kind: &str,
    request_fingerprint: &str,
    requested_by: i64,
    configuration_version: i64,
    authorization_epoch: i32,
    now: DateTime<Utc>,
) -> AppResult<tenant_config_transfer::Model> {
    Ok(tenant_config_transfer::Model {
        id: try_next_snowflake_id()?,
        tenant_id: tenant_id.to_owned(),
        bundle_id,
        idempotency_key_hash: idempotency_key_hash.to_owned(),
        request_kind: request_kind.to_owned(),
        request_fingerprint: request_fingerprint.to_owned(),
        status: tenant_config_transfer::Model::STATUS_PREVIEW_READY.to_owned(),
        target_configuration_version: configuration_version,
        target_authorization_epoch: authorization_epoch,
        plan_hash: None,
        preview_calculated_at: None,
        preview_background_job_id: None,
        apply_background_job_id: None,
        rollback_background_job_id: None,
        snapshot_file_id: None,
        applied_configuration_version: None,
        applied_authorization_epoch: None,
        change_counts: json!({}),
        error_summary: None,
        requested_by,
        rollback_expires_at: None,
        created_at: now,
        updated_at: now,
    })
}

fn ensure_bundle_available(
    bundle: &tenant_config_bundle::Model,
    now: DateTime<Utc>,
) -> AppResult<()> {
    if bundle.status != tenant_config_bundle::Model::STATUS_SUCCEEDED {
        return Err(AppError::Conflict("配置包尚未生成成功".into()));
    }
    if bundle
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        return Err(AppError::Conflict("配置包已经过期".into()));
    }
    Ok(())
}

fn validate_operation_request(
    transfer: &tenant_config_transfer::Model,
    operation: &TransferOperationRequest,
) -> AppResult<()> {
    let valid = match operation {
        TransferOperationRequest::Preview => matches!(
            transfer.status.as_str(),
            tenant_config_transfer::Model::STATUS_PREVIEW_READY
                | tenant_config_transfer::Model::STATUS_PREVIEWED
                | tenant_config_transfer::Model::STATUS_FAILED
        ),
        TransferOperationRequest::Apply(_) => {
            transfer.status == tenant_config_transfer::Model::STATUS_PREVIEWED
        }
        TransferOperationRequest::Rollback => {
            transfer.status == tenant_config_transfer::Model::STATUS_APPLIED
        }
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "配置迁移当前状态不允许执行该操作".into(),
        ))
    }
}

fn validate_operation_replay_identity(
    transfer: &tenant_config_transfer::Model,
    operation: &TransferOperationRequest,
) -> AppResult<()> {
    if let TransferOperationRequest::Apply(command) = operation
        && (transfer.plan_hash.as_deref() != Some(command.plan_hash.as_str())
            || transfer.target_configuration_version != command.target_configuration_version
            || transfer.target_authorization_epoch != command.target_authorization_epoch)
    {
        return Err(AppError::Conflict(
            "Idempotency-Key 已用于其他配置应用请求".into(),
        ));
    }
    Ok(())
}

fn operation_job_id(
    transfer: &tenant_config_transfer::Model,
    operation: &TransferOperationRequest,
) -> Option<i64> {
    match operation {
        TransferOperationRequest::Preview => transfer.preview_background_job_id,
        TransferOperationRequest::Apply(_) => transfer.apply_background_job_id,
        TransferOperationRequest::Rollback => transfer.rollback_background_job_id,
    }
}

fn job_tenant(job: &background_job::Model) -> AppResult<&str> {
    job.tenant_id
        .as_deref()
        .ok_or_else(|| AppError::Validation("配置迁移任务缺少租户".into()))
}

fn payload_id(job: &background_job::Model, key: &str) -> AppResult<i64> {
    let value = job
        .payload
        .get(key)
        .ok_or_else(|| AppError::Validation("配置迁移任务载荷缺少资源标识".into()))?;
    match value {
        Value::String(value) => value
            .parse()
            .map_err(|_| AppError::Validation("配置迁移任务资源标识无效".into())),
        Value::Number(value) => value
            .as_i64()
            .ok_or_else(|| AppError::Validation("配置迁移任务资源标识无效".into())),
        _ => Err(AppError::Validation("配置迁移任务资源标识无效".into())),
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

fn internal_json_error(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(error.to_string())
}
