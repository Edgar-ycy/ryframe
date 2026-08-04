use crate::{naming, schema::TableInfo, template};

pub fn render_handler(table: &TableInfo, base_name: &str) -> String {
    let struct_name = naming::to_pascal_case(base_name);
    let snake = naming::to_snake_case(base_name);
    let primary_key_type = template::primary_key(table).rust_type.as_str();
    let command_columns = template::command_columns(table).collect::<Vec<_>>();
    let create_command_fields = command_columns
        .iter()
        .map(|column| {
            let field_name = naming::safe_field_name(&column.name);
            format!("            {field_name}: dto.{field_name},")
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"// 此文件由 ryframe-generator v{generator_version} 自动生成。
use axum::{{
    Json, Router,
    extract::{{Path, Query, State}},
}};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{{ApiPageResponse, ApiResponse, HttpResult}};
use ryframe_kernel::AppError;
use ryframe_macro::{{delete, get, post, put, route}};
use ryframe_service::system::{{
    Create{struct_name}Command, {struct_name}Vo, Update{struct_name}Command,
}};
use validator::Validate;

use crate::dto::{snake}_dto::{{Create{struct_name}Dto, Update{struct_name}Dto}};
use crate::list_query;
use crate::state::AppState;

list_query!(pub {struct_name}ListQuery {{}});

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
    responses((status = 200, description = "分页列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<{struct_name}ListQuery>,
) -> HttpResult<Json<ApiPageResponse<{struct_name}Vo>>> {{
    let (page_query, _) = query.into_parts(&state.config.pagination)?;
    let page = state
        .services
        .{snake}
        .find_by_page(&current_user, page_query)
        .await?;
    Ok(Json(ApiPageResponse::page(
            page.records,
            page.total,
            page.page,
            page.page_size,
            state.config.pagination.max_page_size,
        )))
}}

#[get("/{{id}}")]
#[perm("system:{snake}:list")]
#[utoipa::path(get, path = "/api/v1/system/{snake}/{{id}}", tag = "{struct_name}",
    params(("id" = {primary_key_type}, Path)), responses((status = 200, description = "详情")),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<{primary_key_type}>,
) -> HttpResult<Json<ApiResponse<{struct_name}Vo>>> {{
    let value = state
        .services
        .{snake}
        .find_by_id(&current_user, id)
        .await?
        .ok_or_else(|| AppError::NotFound("记录不存在".into()))?;
    Ok(Json(ApiResponse::success(value)))
}}

#[post("/")]
#[perm("system:{snake}:add")]
#[utoipa::path(post, path = "/api/v1/system/{snake}", tag = "{struct_name}",
    request_body = Create{struct_name}Dto, responses((status = 200, description = "创建成功")),
    security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<Create{struct_name}Dto>,
) -> HttpResult<Json<ApiResponse<{struct_name}Vo>>> {{
    dto.validate()?;
    let command = Create{struct_name}Command {{
{create_command_fields}
    }};
    let value = state
        .services
        .{snake}
        .create(&current_user, command)
        .await?;
    Ok(Json(ApiResponse::success(value)))
}}

#[put("/{{id}}")]
#[perm("system:{snake}:edit")]
#[utoipa::path(put, path = "/api/v1/system/{snake}/{{id}}", tag = "{struct_name}",
    params(("id" = {primary_key_type}, Path)), request_body = Update{struct_name}Dto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<{primary_key_type}>,
    Json(dto): Json<Update{struct_name}Dto>,
) -> HttpResult<Json<ApiResponse<{struct_name}Vo>>> {{
    dto.validate()?;
    let command = Update{struct_name}Command {{
{create_command_fields}
    }};
    let value = state
        .services
        .{snake}
        .update(&current_user, id, command)
        .await?;
    Ok(Json(ApiResponse::success(value)))
}}

#[delete("/{{id}}")]
#[perm("system:{snake}:remove")]
#[utoipa::path(delete, path = "/api/v1/system/{snake}/{{id}}", tag = "{struct_name}",
    params(("id" = {primary_key_type}, Path)), responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<{primary_key_type}>,
) -> HttpResult<Json<ApiResponse<()>>> {{
    state.services.{snake}.delete(&current_user, id).await?;
    Ok(Json(ApiResponse::success_no_data()))
}}
"#,
        generator_version = crate::GENERATOR_VERSION,
    )
}
