use crate::{naming, schema::TableInfo, template};

pub fn render_handler(table: &TableInfo, base_name: &str) -> String {
    let struct_name = naming::to_pascal_case(base_name);
    let snake = naming::to_snake_case(base_name);
    let business_primary_keys = template::business_primary_keys(table);
    let key_path = business_primary_keys
        .iter()
        .map(|column| format!("/{{{}}}", naming::safe_field_name(&column.name)))
        .collect::<String>();

    format!(
        r#"// 此文件由 ryframe-generator v{generator_version} 自动生成。
// 租户数据边界：API 传输
use axum::{{
    Json, Router,
    extract::{{Path, Query, State}},
}};
use ryframe_application::business::{struct_name}Vo;
use ryframe_kernel::AppError;
use ryframe_macro::{{delete, get, post, put, route}};
use validator::Validate;

use crate::dto::business::{snake}_dto::{{
    Create{struct_name}Dto, {struct_name}KeyDto, {struct_name}ListDto, Update{struct_name}Dto,
}};
use crate::state::AppState;
use crate::{{ApiPageResponse, ApiResponse, HttpResult, RequestPrincipal}};

pub fn {snake}_router(state: AppState) -> Router {{
    Router::new()
        .merge(route!(list))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .with_state(state)
}}

#[get("/")]
#[perm("system:{snake}:list")]
#[utoipa::path(get, path = "/api/v1/system/{snake}", tag = "{struct_name}",
    params({struct_name}ListDto),
    responses((status = 200, description = "分页列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(dto): Query<{struct_name}ListDto>,
) -> HttpResult<Json<ApiPageResponse<{struct_name}Vo>>> {{
    let query = dto.into_query(
        state.config.pagination.default_page_size,
        state.config.pagination.max_page_size,
    )?;
    let page = state.services.{snake}.find_by_page(&current_user, query).await?;
    Ok(Json(ApiPageResponse::page(
        page.records,
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}}

#[get("{key_path}")]
#[perm("system:{snake}:list")]
#[utoipa::path(get, path = "/api/v1/system/{snake}{key_path}", tag = "{struct_name}",
    params({struct_name}KeyDto), responses((status = 200, description = "详情")),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(dto): Path<{struct_name}KeyDto>,
) -> HttpResult<Json<ApiResponse<{struct_name}Vo>>> {{
    let key = dto.into();
    let value = state
        .services
        .{snake}
        .find_by_id(&current_user, &key)
        .await?
        .ok_or_else(|| AppError::NotFound("记录不存在".into()))?;
    Ok(Json(ApiResponse::success(value)))
}}

#[post("/")]
#[perm("system:{snake}:add")]
#[utoipa::path(post, path = "/api/v1/system/{snake}", tag = "{struct_name}",
    request_body = Create{struct_name}Dto,
    responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<Create{struct_name}Dto>,
) -> HttpResult<Json<ApiResponse<{struct_name}Vo>>> {{
    dto.validate()?;
    let value = state.services.{snake}.create(&current_user, dto.into()).await?;
    Ok(Json(ApiResponse::success(value)))
}}

#[put("{key_path}")]
#[perm("system:{snake}:edit")]
#[utoipa::path(put, path = "/api/v1/system/{snake}{key_path}", tag = "{struct_name}",
    params({struct_name}KeyDto), request_body = Update{struct_name}Dto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(key_dto): Path<{struct_name}KeyDto>,
    Json(dto): Json<Update{struct_name}Dto>,
) -> HttpResult<Json<ApiResponse<{struct_name}Vo>>> {{
    dto.validate()?;
    let key = key_dto.into();
    let value = state
        .services
        .{snake}
        .update(&current_user, &key, dto.into())
        .await?;
    Ok(Json(ApiResponse::success(value)))
}}

#[delete("{key_path}")]
#[perm("system:{snake}:remove")]
#[utoipa::path(delete, path = "/api/v1/system/{snake}{key_path}", tag = "{struct_name}",
    params({struct_name}KeyDto), responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(dto): Path<{struct_name}KeyDto>,
) -> HttpResult<Json<ApiResponse<()>>> {{
    let key = dto.into();
    state.services.{snake}.delete(&current_user, &key).await?;
    Ok(Json(ApiResponse::success_no_data()))
}}
"#,
        generator_version = crate::GENERATOR_VERSION,
    )
}
