use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_auth::RequestPrincipal;
use ryframe_core::ValidatedPageQuery;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::DictTypeListParams;
use serde::Deserialize;
use validator::Validate;

use crate::dto::dict_dto::{
    CreateDictDataDto, CreateDictTypeDto, DictOptionDto, UpdateDictDataDto, UpdateDictTypeDto,
};
use crate::dto::public_dto::{DictDataVo, DictTypeVo, ExportJobVo};
use crate::list_query;
use crate::state::AppState;
use crate::{dto::export_dto::ExportRequestDto, handlers::export_handler::request_export};

list_query!(pub DictTypeListQuery, DictTypeFilterQuery {
    name: String,
    code: String,
    status: String,
});

impl DictTypeFilterQuery {
    fn into_service_params(self, page: ValidatedPageQuery) -> DictTypeListParams {
        DictTypeListParams {
            page,
            name: self.name,
            code: self.code,
            status: self.status,
        }
    }
}

pub fn dict_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_types))
        .merge(route!(request_dict_type_export))
        .merge(route!(create_type))
        .merge(route!(update_type))
        .merge(route!(delete_type))
        .merge(route!(list_data))
        .merge(route!(list_data_by_type_path))
        .merge(route!(create_data))
        .merge(route!(update_data))
        .merge(route!(delete_data))
        .with_state(state)
}

/// 字典类型列表
#[get("/types")]
#[perm("system:dict:list")]
#[utoipa::path(get, path = "/api/v1/system/dict/types", tag = "字典管理",
    params(DictTypeListQuery),
    responses((status = 200, description = "字典类型列表", body = ApiPageResponse<DictTypeVo>)), security(("bearer" = [])))]
async fn list_types(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<DictTypeListQuery>,
) -> HttpResult<Json<ApiPageResponse<DictTypeVo>>> {
    let (page, filter) = query.into_parts(&state.config.pagination)?;
    let page_result = state
        .services
        .dict
        .find_types_by_page(&current_user, filter.into_service_params(page))
        .await?;
    Ok(Json(ApiPageResponse::new(
        page_result
            .records
            .into_iter()
            .map(DictTypeVo::from)
            .collect(),
        page_result.total,
        page_result.page,
        page_result.page_size,
        state.config.pagination.max_page_size,
        "查询成功",
    )))
}

/// 创建字典类型
#[post("/types")]
#[perm("system:dict:add")]
#[utoipa::path(post, path = "/api/v1/system/dict/types", tag = "字典管理",
    request_body = CreateDictTypeDto, responses((status = 200, description = "创建成功", body = ApiResponse<DictTypeVo>)), security(("bearer" = [])))]
async fn create_type(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateDictTypeDto>,
) -> HttpResult<Json<ApiResponse<DictTypeVo>>> {
    dto.validate()?;
    state
        .services
        .dict
        .create_type(&current_user, &dto.name, &dto.code)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 更新字典类型
#[put("/types/{id}")]
#[perm("system:dict:edit")]
#[utoipa::path(put, path = "/api/v1/system/dict/types/{id}", tag = "字典管理",
    params(("id" = String, Path)),
    request_body = UpdateDictTypeDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<DictTypeVo>)),
    security(("bearer" = [])))]
async fn update_type(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDictTypeDto>,
) -> HttpResult<Json<ApiResponse<DictTypeVo>>> {
    dto.validate()?;
    state
        .services
        .dict
        .update_type(&current_user, id, &dto.name, dto.status)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 删除字典类型
#[delete("/types/{id}")]
#[perm("system:dict:remove")]
#[utoipa::path(delete, path = "/api/v1/system/dict/types/{id}", tag = "字典管理",
    params(("id" = String, Path)),
    responses((status = 200, description = "删除成功", body = ryframe_http::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn delete_type(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<()>>> {
    state.services.dict.delete_type(&current_user, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
struct ListDataQuery {
    type_code: String,
}

#[get("/data")]
#[perm("system:dict:list")]
#[utoipa::path(get, path = "/api/v1/system/dict/data", tag = "字典管理",
    params(ListDataQuery),
    responses((status = 200, description = "字典数据列表", body = ApiResponse<Vec<DictDataVo>>)), security(("bearer" = [])))]
async fn list_data(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<ListDataQuery>,
) -> HttpResult<Json<ApiResponse<Vec<DictDataVo>>>> {
    state
        .services
        .dict
        .find_data_by_type(&current_user, &query.type_code)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|values| {
            Json(ApiResponse::success(
                values.into_iter().map(DictDataVo::from).collect(),
            ))
        })
}

/// 通过字典类型编码查询字典数据
/// 查询字典数据
#[get("/data/type/{dict_type}")]
#[perm("system:dict:list")]
#[utoipa::path(get, path = "/api/v1/system/dict/data/type/{dict_type}", tag = "字典管理",
    params(("dict_type" = String, Path)), responses((status = 200, description = "字典数据", body = ApiResponse<Vec<DictOptionDto>>)), security(("bearer" = [])))]
async fn list_data_by_type_path(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(dict_type): Path<String>,
) -> HttpResult<Json<ApiResponse<Vec<DictOptionDto>>>> {
    let data = state
        .services
        .dict
        .find_data_by_type(&current_user, &dict_type)
        .await?;
    let items = data
        .into_iter()
        .map(|item| DictOptionDto {
            label: item.label,
            value: item.value,
            css_class: item.css_class,
        })
        .collect();
    Ok(Json(ApiResponse::success(items)))
}

/// 创建字典数据
#[post("/data")]
#[perm("system:dict:add")]
#[utoipa::path(post, path = "/api/v1/system/dict/data", tag = "字典管理",
    request_body = CreateDictDataDto, responses((status = 200, description = "创建成功", body = ApiResponse<DictDataVo>)), security(("bearer" = [])))]
async fn create_data(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateDictDataDto>,
) -> HttpResult<Json<ApiResponse<DictDataVo>>> {
    dto.validate()?;
    state
        .services
        .dict
        .create_data(
            &current_user,
            &dto.type_code,
            &dto.label,
            &dto.value,
            dto.sort.unwrap_or(0),
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 更新字典数据
#[put("/data/{id}")]
#[perm("system:dict:edit")]
#[utoipa::path(put, path = "/api/v1/system/dict/data/{id}", tag = "字典管理",
    params(("id" = String, Path)),
    request_body = UpdateDictDataDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<DictDataVo>)),
    security(("bearer" = [])))]
async fn update_data(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDictDataDto>,
) -> HttpResult<Json<ApiResponse<DictDataVo>>> {
    dto.validate()?;
    state
        .services
        .dict
        .update_data(
            &current_user,
            id,
            &dto.label,
            &dto.value,
            dto.sort.unwrap_or(0),
            dto.status,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 删除字典数据
#[delete("/data/{id}")]
#[perm("system:dict:remove")]
#[utoipa::path(delete, path = "/api/v1/system/dict/data/{id}", tag = "字典管理",
    params(("id" = String, Path)),
    responses((status = 200, description = "删除成功", body = ryframe_http::ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn delete_data(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<()>>> {
    state.services.dict.delete_data(&current_user, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 创建字典类型异步导出任务。
#[post("/types/exports")]
#[perm("system:dict:export")]
#[utoipa::path(post, path = "/api/v1/system/dict/types/exports", tag = "字典管理",
    params(("Idempotency-Key" = String, Header, description = "幂等键")), request_body = ExportRequestDto,
    responses((status = 202, description = "字典类型导出任务已创建", body = ApiResponse<ExportJobVo>)), security(("bearer" = [])))]
async fn request_dict_type_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<ExportRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    request_export(
        state,
        current_user,
        headers,
        "dict-types",
        "system:dict:export",
        request.0,
    )
    .await
}
