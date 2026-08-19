use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_application::system::{
    ApplyProductChangeCommand, CapabilityOverrideInput, CapabilitySnapshotInput,
    CreateProductPlanCommand, CreateProductPlanVersionCommand, ProductChangeTarget,
    UpdateProductPlanCommand, UpdateProductPlanVersionCommand,
};
use ryframe_auth::rbac;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post, put, route};
use validator::Validate;

use crate::{
    RequestPrincipal,
    dto::product_dto::{
        CapabilityCatalogVo, CapabilityOverrideDto, CapabilitySnapshotDto, CreateProductPlanDto,
        CreateProductPlanVersionDto, ProductChangeApplyDto, ProductChangePreviewDto,
        ProductChangePreviewVo, ProductContextVo, ProductPlanPageQuery, ProductPlanVersionVo,
        ProductPlanVo, UpdateProductPlanDto, into_json_object,
    },
    state::AppState,
};

pub fn product_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(capabilities))
        .merge(route!(list_plans))
        .merge(route!(plan_detail))
        .merge(route!(create_plan))
        .merge(route!(update_plan))
        .merge(route!(list_versions))
        .merge(route!(create_version))
        .merge(route!(update_version))
        .merge(route!(publish_version))
        .merge(route!(retire_version))
        .merge(route!(tenant_context))
        .merge(route!(preview_tenant_change))
        .merge(route!(apply_tenant_change))
        .with_state(state)
}

#[get("/capabilities")]
#[perm("platform:product-plan:list")]
#[utoipa::path(get, path = "/api/v1/platform/capabilities", tag = "产品能力",
    responses((status = 200, body = ApiResponse<Vec<CapabilityCatalogVo>>)), security(("bearer" = [])))]
pub(crate) async fn capabilities(
    State(state): State<AppState>,
    principal: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<Vec<CapabilityCatalogVo>>>> {
    let values = state
        .services
        .product
        .capability_catalog(&principal)?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(ApiResponse::success(values)))
}

#[get("/product-plans")]
#[perm("platform:product-plan:list")]
#[utoipa::path(get, path = "/api/v1/platform/product-plans", tag = "产品套餐",
    params(ProductPlanPageQuery),
    responses((status = 200, body = ApiPageResponse<ProductPlanVo>)), security(("bearer" = [])))]
pub(crate) async fn list_plans(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Query(query): Query<ProductPlanPageQuery>,
) -> HttpResult<Json<ApiPageResponse<ProductPlanVo>>> {
    list_plans_page(state, principal, query).await
}

async fn list_plans_page(
    state: AppState,
    principal: RequestPrincipal,
    query: ProductPlanPageQuery,
) -> HttpResult<Json<ApiPageResponse<ProductPlanVo>>> {
    let page = query.validate_page()?;
    let values = state.services.product.list_plans(&principal).await?;
    let total = u64::try_from(values.len())
        .map_err(|_| AppError::Internal("产品套餐列表超出分页计数范围".into()))?;
    let offset = usize::try_from(page.offset())
        .map_err(|_| AppError::Validation("分页偏移超出服务器范围".into()))?;
    let page_size = usize::try_from(page.page_size())
        .map_err(|_| AppError::Validation("分页大小超出服务器范围".into()))?;
    let items = values
        .into_iter()
        .skip(offset)
        .take(page_size)
        .map(ProductPlanVo::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiPageResponse::page(
        items,
        total,
        page.page(),
        page.page_size(),
        ProductPlanPageQuery::max_page_size(),
    )))
}

#[get("/product-plans/{plan_id}")]
#[perm("platform:product-plan:list")]
#[utoipa::path(get, path = "/api/v1/platform/product-plans/{plan_id}", tag = "产品套餐",
    params(("plan_id" = String, Path)), responses((status = 200, body = ApiResponse<ProductPlanVo>)), security(("bearer" = [])))]
pub(crate) async fn plan_detail(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(plan_id): Path<String>,
) -> HttpResult<Json<ApiResponse<ProductPlanVo>>> {
    let value = state
        .services
        .product
        .plan(&principal, parse_positive_id(&plan_id, "plan_id")?)
        .await?;
    Ok(Json(ApiResponse::success(ProductPlanVo::try_from(value)?)))
}

#[post("/product-plans")]
#[perm("platform:product-plan:add")]
#[utoipa::path(post, path = "/api/v1/platform/product-plans", tag = "产品套餐",
    request_body = CreateProductPlanDto,
    responses((status = 200, body = ApiResponse<ProductPlanVo>), (status = 400, description = "参数无效"), (status = 403, description = "缺少平台套餐创建权限"), (status = 409, description = "套餐 key 冲突")), security(("bearer" = [])))]
pub(crate) async fn create_plan(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Json(dto): Json<CreateProductPlanDto>,
) -> HttpResult<Json<ApiResponse<ProductPlanVo>>> {
    dto.validate()?;
    let value = state
        .services
        .product
        .create_plan(
            &principal,
            CreateProductPlanCommand {
                key: dto.key,
                name: dto.name,
                description: dto.description,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(ProductPlanVo::try_from(value)?)))
}

#[put("/product-plans/{plan_id}")]
#[perm("platform:product-plan:edit")]
#[utoipa::path(put, path = "/api/v1/platform/product-plans/{plan_id}", tag = "产品套餐",
    params(("plan_id" = String, Path)), request_body = UpdateProductPlanDto,
    responses((status = 200, body = ApiResponse<ProductPlanVo>), (status = 400, description = "参数无效"), (status = 403, description = "缺少平台套餐编辑权限"), (status = 404, description = "套餐不存在"), (status = 409, description = "套餐状态冲突")), security(("bearer" = [])))]
pub(crate) async fn update_plan(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(plan_id): Path<String>,
    Json(dto): Json<UpdateProductPlanDto>,
) -> HttpResult<Json<ApiResponse<ProductPlanVo>>> {
    dto.validate()?;
    let value = state
        .services
        .product
        .update_plan(
            &principal,
            parse_positive_id(&plan_id, "plan_id")?,
            UpdateProductPlanCommand {
                name: dto.name,
                description: dto.description,
                status: dto.status.as_str().to_owned(),
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(ProductPlanVo::try_from(value)?)))
}

#[get("/product-plans/{plan_id}/versions")]
#[perm("platform:product-plan:list")]
#[utoipa::path(get, path = "/api/v1/platform/product-plans/{plan_id}/versions", tag = "产品套餐",
    params(("plan_id" = String, Path)), responses((status = 200, body = ApiResponse<Vec<ProductPlanVersionVo>>)), security(("bearer" = [])))]
pub(crate) async fn list_versions(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(plan_id): Path<String>,
) -> HttpResult<Json<ApiResponse<Vec<ProductPlanVersionVo>>>> {
    let values = state
        .services
        .product
        .versions(&principal, parse_positive_id(&plan_id, "plan_id")?)
        .await?
        .into_iter()
        .map(ProductPlanVersionVo::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiResponse::success(values)))
}

#[post("/product-plans/{plan_id}/versions")]
#[perm("platform:product-plan:add")]
#[utoipa::path(post, path = "/api/v1/platform/product-plans/{plan_id}/versions", tag = "产品套餐",
    params(("plan_id" = String, Path)), request_body = CreateProductPlanVersionDto,
    responses((status = 200, body = ApiResponse<ProductPlanVersionVo>), (status = 400, description = "能力快照无效"), (status = 403, description = "缺少平台套餐创建权限"), (status = 404, description = "套餐不存在"), (status = 409, description = "版本冲突")), security(("bearer" = [])))]
pub(crate) async fn create_version(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(plan_id): Path<String>,
    Json(dto): Json<CreateProductPlanVersionDto>,
) -> HttpResult<Json<ApiResponse<ProductPlanVersionVo>>> {
    dto.validate()?;
    let value = state
        .services
        .product
        .create_version(
            &principal,
            parse_positive_id(&plan_id, "plan_id")?,
            CreateProductPlanVersionCommand {
                name: dto.name,
                description: dto.description,
                capabilities: snapshots(dto.capabilities),
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(ProductPlanVersionVo::try_from(
        value,
    )?)))
}

#[put("/product-plans/{plan_id}/versions/{version}/draft")]
#[perm("platform:product-plan:edit")]
#[utoipa::path(put, path = "/api/v1/platform/product-plans/{plan_id}/versions/{version}/draft", tag = "产品套餐",
    params(("plan_id" = String, Path), ("version" = i32, Path)), request_body = CreateProductPlanVersionDto,
    responses((status = 200, body = ApiResponse<ProductPlanVersionVo>), (status = 400, description = "能力快照无效"), (status = 403, description = "缺少平台套餐编辑权限"), (status = 404, description = "套餐版本不存在"), (status = 409, description = "版本非 draft")), security(("bearer" = [])))]
pub(crate) async fn update_version(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path((plan_id, version)): Path<(String, i32)>,
    Json(dto): Json<CreateProductPlanVersionDto>,
) -> HttpResult<Json<ApiResponse<ProductPlanVersionVo>>> {
    dto.validate()?;
    let value = state
        .services
        .product
        .update_version(
            &principal,
            parse_positive_id(&plan_id, "plan_id")?,
            version,
            UpdateProductPlanVersionCommand {
                name: dto.name,
                description: dto.description,
                capabilities: snapshots(dto.capabilities),
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(ProductPlanVersionVo::try_from(
        value,
    )?)))
}

#[post("/product-plans/{plan_id}/versions/{version}/publish")]
#[perm("platform:product-plan:publish")]
#[utoipa::path(post, path = "/api/v1/platform/product-plans/{plan_id}/versions/{version}/publish", tag = "产品套餐",
    params(("plan_id" = String, Path), ("version" = i32, Path)),
    responses((status = 200, body = ApiResponse<ProductPlanVersionVo>), (status = 400, description = "能力依赖、冲突或 schema 无效"), (status = 403, description = "缺少发布权限"), (status = 404, description = "套餐版本不存在"), (status = 409, description = "版本非 draft"), (status = 501, description = "部署依赖不可用")), security(("bearer" = [])))]
pub(crate) async fn publish_version(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path((plan_id, version)): Path<(String, i32)>,
) -> HttpResult<Json<ApiResponse<ProductPlanVersionVo>>> {
    let value = state
        .services
        .product
        .publish_version(&principal, parse_positive_id(&plan_id, "plan_id")?, version)
        .await?;
    Ok(Json(ApiResponse::success(ProductPlanVersionVo::try_from(
        value,
    )?)))
}

#[post("/product-plans/{plan_id}/versions/{version}/retire")]
#[perm("platform:product-plan:publish")]
#[utoipa::path(post, path = "/api/v1/platform/product-plans/{plan_id}/versions/{version}/retire", tag = "产品套餐",
    params(("plan_id" = String, Path), ("version" = i32, Path)),
    responses((status = 200, body = ApiResponse<ProductPlanVersionVo>), (status = 403, description = "缺少发布权限"), (status = 404, description = "套餐版本不存在"), (status = 409, description = "版本非 published 或仍被竞态分配")), security(("bearer" = [])))]
pub(crate) async fn retire_version(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path((plan_id, version)): Path<(String, i32)>,
) -> HttpResult<Json<ApiResponse<ProductPlanVersionVo>>> {
    let value = state
        .services
        .product
        .retire_version(&principal, parse_positive_id(&plan_id, "plan_id")?, version)
        .await?;
    Ok(Json(ApiResponse::success(ProductPlanVersionVo::try_from(
        value,
    )?)))
}

#[get("/tenants/{tenant_id}/product-context")]
#[perm("tenant:product:view")]
#[utoipa::path(get, path = "/api/v1/platform/tenants/{tenant_id}/product-context", tag = "产品能力",
    params(("tenant_id" = String, Path)), responses((status = 200, body = ApiResponse<ProductContextVo>)), security(("bearer" = [])))]
pub(crate) async fn tenant_context(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(tenant_id): Path<String>,
) -> HttpResult<Json<ApiResponse<ProductContextVo>>> {
    let value = state
        .services
        .product
        .product_context(&principal, &tenant_id)
        .await?;
    Ok(Json(ApiResponse::success(ProductContextVo::try_from(
        value,
    )?)))
}

#[post("/tenants/{tenant_id}/product-change-previews")]
#[perm("tenant:product:assign")]
#[utoipa::path(post, path = "/api/v1/platform/tenants/{tenant_id}/product-change-previews", tag = "产品能力",
    params(("tenant_id" = String, Path)), request_body = ProductChangePreviewDto,
    responses((status = 200, body = ApiResponse<ProductChangePreviewVo>), (status = 400, description = "目标套餐、override 或 schema 无效"), (status = 403, description = "缺少套餐分配或能力覆盖权限"), (status = 404, description = "租户或套餐版本不存在"), (status = 409, description = "目标版本不可分配"), (status = 501, description = "部署能力不可用")), security(("bearer" = [])))]
pub(crate) async fn preview_tenant_change(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(tenant_id): Path<String>,
    Json(dto): Json<ProductChangePreviewDto>,
) -> HttpResult<Json<ApiResponse<ProductChangePreviewVo>>> {
    dto.validate()?;
    let capability_override_allowed = has_override_permission(&principal);
    let value = state
        .services
        .product
        .preview_change(
            &principal,
            &tenant_id,
            change_target(dto.plan_version_id, dto.overrides)?,
            capability_override_allowed,
        )
        .await?;
    Ok(Json(ApiResponse::success(
        ProductChangePreviewVo::try_from(value)?,
    )))
}

#[post("/tenants/{tenant_id}/product-changes")]
#[perm("tenant:product:assign")]
#[utoipa::path(post, path = "/api/v1/platform/tenants/{tenant_id}/product-changes", tag = "产品能力",
    params(("tenant_id" = String, Path)), request_body = ProductChangeApplyDto,
    responses((status = 200, body = ApiResponse<ProductContextVo>), (status = 400, description = "目标套餐、override 或 schema 无效"), (status = 403, description = "缺少套餐分配或能力覆盖权限"), (status = 404, description = "租户或套餐版本不存在"), (status = 409, description = "runtime_epoch、计划哈希、租约或版本状态冲突"), (status = 501, description = "部署能力不可用")), security(("bearer" = [])))]
pub(crate) async fn apply_tenant_change(
    State(state): State<AppState>,
    principal: RequestPrincipal,
    Path(tenant_id): Path<String>,
    Json(dto): Json<ProductChangeApplyDto>,
) -> HttpResult<Json<ApiResponse<ProductContextVo>>> {
    dto.validate()?;
    let capability_override_allowed = has_override_permission(&principal);
    let runtime_epoch = parse_positive_id(&dto.preview_runtime_epoch, "preview_runtime_epoch")?;
    let target = change_target(dto.plan_version_id, dto.overrides)?;
    let value = state
        .services
        .product
        .apply_change(
            &principal,
            &tenant_id,
            ApplyProductChangeCommand {
                target,
                preview_runtime_epoch: runtime_epoch,
                plan_hash: dto.plan_hash,
                reason: dto.reason,
                capability_override_allowed,
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(ProductContextVo::try_from(
        value,
    )?)))
}

fn snapshots(values: Vec<CapabilitySnapshotDto>) -> Vec<CapabilitySnapshotInput> {
    values
        .into_iter()
        .map(|value| CapabilitySnapshotInput {
            capability_code: value.capability_code,
            variant_code: value.variant_code,
            schema_version: value.schema_version,
            config: into_json_object(value.config),
        })
        .collect()
}

fn change_target(
    plan_version_id: String,
    values: Vec<CapabilityOverrideDto>,
) -> Result<ProductChangeTarget, AppError> {
    Ok(ProductChangeTarget {
        plan_version_id: parse_positive_id(&plan_version_id, "plan_version_id")?,
        overrides: values
            .into_iter()
            .map(|value| CapabilityOverrideInput {
                capability_code: value.capability_code,
                enabled: value.enabled,
                variant_code: value.variant_code,
                schema_version: value.schema_version,
                config: into_json_object(value.config),
            })
            .collect(),
    })
}

fn has_override_permission(principal: &RequestPrincipal) -> bool {
    principal.is_super_admin
        || rbac::has_permission(&principal.permissions, "tenant:capability:override")
}

fn parse_positive_id(value: &str, field: &str) -> Result<i64, AppError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::Validation(format!("{field} 必须是正整数十进制字符串")))
}
