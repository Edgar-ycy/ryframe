use crate::RequestPrincipal;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use ryframe_application::system::{
    BackupPointListParams, CreateMigrationCommand, DataTargetListParams, MigrationActionCommand,
    MigrationPreviewRequest,
};
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post, route};
use validator::Validate;

use crate::{
    dto::tenant_data_dto::{
        BackupPointListQuery, BackupPointView, CreateMigrationDto, DataPlacementView,
        DataTargetDetail, DataTargetListQuery, DataTargetSummary, MigrationListQuery,
        MigrationPreview, MigrationPreviewDto, MigrationView,
    },
    state::AppState,
};

pub fn tenant_data_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_targets))
        .merge(route!(target_detail))
        .merge(route!(backup_points))
        .merge(route!(placement))
        .merge(route!(preview_migration))
        .merge(route!(create_migration))
        .merge(route!(list_tenant_migrations))
        .merge(route!(migration_detail))
        .merge(route!(cancel_migration))
        .merge(route!(finalize_migration))
        .with_state(state)
}

#[get("/data-targets")]
#[perm("tenant:data-placement:view")]
#[utoipa::path(get, path = "/api/v1/platform/data-targets", tag = "租户数据放置",
    params(DataTargetListQuery),
    responses((status = 200, body = ApiPageResponse<DataTargetSummary>),
        (status = 403, description = "非 system 租户或缺少权限")), security(("bearer" = [])))]
pub(crate) async fn list_targets(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Query(query): Query<DataTargetListQuery>,
) -> HttpResult<Json<ApiPageResponse<DataTargetSummary>>> {
    let page = query.validate_page()?;
    let values = state
        .services
        .tenant_data_migration
        .list_targets_with_context(
            &principal,
            DataTargetListParams {
                eligible_for: query.eligible_for.map(|value| value.as_str().to_owned()),
                tenant_id: query.tenant_id.clone(),
                q: query.q.clone(),
            },
        )
        .await?;
    let total = u64::try_from(values.len())
        .map_err(|_| AppError::Internal("数据目标列表超出分页计数范围".into()))?;
    let offset = usize::try_from(page.offset())
        .map_err(|_| AppError::Validation("分页偏移超出服务器范围".into()))?;
    let page_size = usize::try_from(page.page_size())
        .map_err(|_| AppError::Validation("分页大小超出服务器范围".into()))?;
    let values = values
        .into_iter()
        .skip(offset)
        .take(page_size)
        .map(DataTargetSummary::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiPageResponse::page(
        values,
        total,
        page.page(),
        page.page_size(),
        DataTargetListQuery::max_page_size(),
    )))
}

#[get("/data-targets/{target_key}")]
#[perm("tenant:data-placement:view")]
#[utoipa::path(get, path = "/api/v1/platform/data-targets/{target_key}", tag = "租户数据放置",
    params(("target_key" = String, Path)),
    responses((status = 200, body = ApiResponse<DataTargetDetail>),
        (status = 404, description = "目标不存在")), security(("bearer" = [])))]
pub(crate) async fn target_detail(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(target_key): Path<String>,
) -> HttpResult<Json<ApiResponse<DataTargetDetail>>> {
    let value = state
        .services
        .tenant_data_migration
        .target_detail(&principal, &target_key)
        .await?;
    Ok(Json(ApiResponse::success(DataTargetDetail::try_from(
        value,
    )?)))
}

#[get("/data-targets/{target_key}/backup-points")]
#[perm("tenant:data-backup:list")]
#[utoipa::path(get, path = "/api/v1/platform/data-targets/{target_key}/backup-points", tag = "租户数据备份",
    params(("target_key" = String, Path), BackupPointListQuery),
    responses((status = 200, body = ApiResponse<Vec<BackupPointView>>),
        (status = 404, description = "目标不存在")), security(("bearer" = [])))]
pub(crate) async fn backup_points(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(target_key): Path<String>,
    Query(query): Query<BackupPointListQuery>,
) -> HttpResult<Json<ApiResponse<Vec<BackupPointView>>>> {
    let values = state
        .services
        .tenant_data_migration
        .backup_points(
            &principal,
            &target_key,
            BackupPointListParams {
                tenant_id: query.tenant_id,
                limit: query.limit.unwrap_or(100),
            },
        )
        .await?
        .into_iter()
        .map(BackupPointView::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiResponse::success(values)))
}

#[get("/tenants/{tenant_id}/data-placement")]
#[perm("tenant:data-placement:view")]
#[utoipa::path(get, path = "/api/v1/platform/tenants/{tenant_id}/data-placement", tag = "租户数据放置",
    params(("tenant_id" = String, Path)),
    responses((status = 200, body = ApiResponse<DataPlacementView>),
        (status = 404, description = "placement 不存在")), security(("bearer" = [])))]
pub(crate) async fn placement(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(tenant_id): Path<String>,
) -> HttpResult<Json<ApiResponse<DataPlacementView>>> {
    let value = state
        .services
        .tenant_data_migration
        .placement(&principal, &tenant_id)
        .await?;
    Ok(Json(ApiResponse::success(DataPlacementView::try_from(
        value,
    )?)))
}

#[post("/tenants/{tenant_id}/data-migration-previews")]
#[perm("tenant:data-migration:create")]
#[utoipa::path(post, path = "/api/v1/platform/tenants/{tenant_id}/data-migration-previews", tag = "租户数据迁移",
    params(("tenant_id" = String, Path)), request_body = MigrationPreviewDto,
    responses((status = 200, body = ApiResponse<MigrationPreview>),
        (status = 409, description = "placement generation 已变化"),
        (status = 423, description = "租户数据维护中；响应含 Retry-After"),
        (status = 503, description = "目标不可用；响应含 Retry-After")), security(("bearer" = [])))]
pub(crate) async fn preview_migration(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(tenant_id): Path<String>,
    Json(dto): Json<MigrationPreviewDto>,
) -> HttpResult<Json<ApiResponse<MigrationPreview>>> {
    dto.validate()?;
    let value = state
        .services
        .tenant_data_migration
        .preview(
            &principal,
            &tenant_id,
            MigrationPreviewRequest {
                target_key: dto.target_key,
                expected_placement_generation: parse_generation(
                    &dto.expected_placement_generation,
                )?,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(MigrationPreview::from(value))))
}

#[post("/tenants/{tenant_id}/data-migrations")]
#[perm("tenant:data-migration:create")]
#[utoipa::path(post, path = "/api/v1/platform/tenants/{tenant_id}/data-migrations", tag = "租户数据迁移",
    params(("tenant_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = CreateMigrationDto,
    responses((status = 200, body = ApiResponse<MigrationView>),
        (status = 409, description = "计划、代际、幂等键或租约冲突"),
        (status = 423, description = "租户数据维护中；响应含 Retry-After"),
        (status = 503, description = "目标不可用；响应含 Retry-After")), security(("bearer" = [])))]
pub(crate) async fn create_migration(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(tenant_id): Path<String>,
    headers: HeaderMap,
    Json(dto): Json<CreateMigrationDto>,
) -> HttpResult<Json<ApiResponse<MigrationView>>> {
    dto.validate()?;
    let value = state
        .services
        .tenant_data_migration
        .create(
            &principal,
            &tenant_id,
            CreateMigrationCommand {
                target_key: dto.target_key,
                expected_placement_generation: parse_generation(
                    &dto.expected_placement_generation,
                )?,
                plan_hash: dto.plan_hash,
                idempotency_key: idempotency_key(&headers)?,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(MigrationView::try_from(value)?)))
}

#[get("/tenants/{tenant_id}/data-migrations")]
#[perm("tenant:data-migration:list")]
#[utoipa::path(get, path = "/api/v1/platform/tenants/{tenant_id}/data-migrations", tag = "租户数据迁移",
    params(("tenant_id" = String, Path), MigrationListQuery),
    responses((status = 200, body = ApiResponse<Vec<MigrationView>>)), security(("bearer" = [])))]
pub(crate) async fn list_tenant_migrations(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(tenant_id): Path<String>,
    Query(query): Query<MigrationListQuery>,
) -> HttpResult<Json<ApiResponse<Vec<MigrationView>>>> {
    let values = state
        .services
        .tenant_data_migration
        .migrations_for_tenant(&principal, &tenant_id, query.limit.unwrap_or(20))
        .await?
        .into_iter()
        .map(MigrationView::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiResponse::success(values)))
}

#[get("/tenant-data-migrations/{migration_id}")]
#[perm("tenant:data-migration:list")]
#[utoipa::path(get, path = "/api/v1/platform/tenant-data-migrations/{migration_id}", tag = "租户数据迁移",
    params(("migration_id" = String, Path)),
    responses((status = 200, body = ApiResponse<MigrationView>),
        (status = 404, description = "迁移不存在")), security(("bearer" = [])))]
pub(crate) async fn migration_detail(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(migration_id): Path<String>,
) -> HttpResult<Json<ApiResponse<MigrationView>>> {
    let value = state
        .services
        .tenant_data_migration
        .migration(&principal, parse_id(&migration_id, "migration_id")?)
        .await?;
    Ok(Json(ApiResponse::success(MigrationView::try_from(value)?)))
}

#[post("/tenant-data-migrations/{migration_id}/cancel")]
#[perm("tenant:data-migration:cancel")]
#[utoipa::path(post, path = "/api/v1/platform/tenant-data-migrations/{migration_id}/cancel", tag = "租户数据迁移",
    params(("migration_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = ApiResponse<MigrationView>),
        (status = 404, description = "迁移不存在"),
        (status = 409, description = "已经越过取消边界或幂等键冲突")), security(("bearer" = [])))]
pub(crate) async fn cancel_migration(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(migration_id): Path<String>,
    headers: HeaderMap,
) -> HttpResult<Json<ApiResponse<MigrationView>>> {
    let value = state
        .services
        .tenant_data_migration
        .cancel(
            &principal,
            parse_id(&migration_id, "migration_id")?,
            MigrationActionCommand {
                idempotency_key: idempotency_key(&headers)?,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(MigrationView::try_from(value)?)))
}

#[post("/tenant-data-migrations/{migration_id}/finalize")]
#[perm("tenant:data-migration:finalize")]
#[utoipa::path(post, path = "/api/v1/platform/tenant-data-migrations/{migration_id}/finalize", tag = "租户数据迁移",
    params(("migration_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = ApiResponse<MigrationView>),
        (status = 404, description = "迁移不存在"),
        (status = 409, description = "保留期、备份资格或幂等键冲突")), security(("bearer" = [])))]
pub(crate) async fn finalize_migration(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(migration_id): Path<String>,
    headers: HeaderMap,
) -> HttpResult<Json<ApiResponse<MigrationView>>> {
    let value = state
        .services
        .tenant_data_migration
        .finalize(
            &principal,
            parse_id(&migration_id, "migration_id")?,
            MigrationActionCommand {
                idempotency_key: idempotency_key(&headers)?,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(MigrationView::try_from(value)?)))
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, AppError> {
    headers
        .get("Idempotency-Key")
        .ok_or_else(|| AppError::Validation("缺少 Idempotency-Key 请求头".into()))?
        .to_str()
        .map(str::to_owned)
        .map_err(|_| AppError::Validation("Idempotency-Key 必须是有效 ASCII".into()))
}

fn parse_generation(value: &str) -> Result<i64, AppError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::Validation("expected_placement_generation 无效".into()))
}

fn parse_id(value: &str, field: &str) -> Result<i64, AppError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::Validation(format!("{field} 无效")))
}
