use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{DictDataVo, DictTypeVo};
use serde::{Deserialize, Serialize};
use validator::Validate;

use super::auth_handler::AppState;
use crate::dto::dict_dto::{
    CreateDictDataDto, CreateDictTypeDto, UpdateDictDataDto, UpdateDictTypeDto,
};
use crate::handler_utils::excel_response;
use crate::list_query;

list_query!(pub DictTypeListQuery {
    name: String,
    code: String,
    status: String,
});

pub fn dict_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_types))
        .merge(route!(list_types_no_page))
        .merge(route!(export_dict_types))
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
#[get("/types", "/types/list")]
#[perm("system:dict:list")]
#[utoipa::path(get, path = "/api/v1/system/dict/types", tag = "字典管理",
    responses((status = 200, description = "字典类型列表")), security(("bearer" = [])))]
async fn list_types(
    State(state): State<AppState>,
    Query(query): Query<DictTypeListQuery>,
) -> AppResult<Json<ApiPageResponse<DictTypeVo>>> {
    let page_query = PageQuery {
        page: query.page,
        page_size: query.page_size,
    };
    let page_result = state
        .dict_service
        .find_types_by_page(&state.db, page_query)
        .await?;
    Ok(Json(page_result.to_page_response("查询成功")))
}

/// 字典类型不分页查询
#[get("/types/listNoPage")]
#[perm("system:dict:list")]
#[utoipa::path(get, path = "/api/v1/system/dict/types/listNoPage", tag = "字典管理",
    responses((status = 200, description = "字典类型列表")),
    security(("bearer" = [])))]
async fn list_types_no_page(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<Vec<DictTypeVo>>>> {
    let page_query = PageQuery::all_records();
    let page_result = state
        .dict_service
        .find_types_by_page(&state.db, page_query)
        .await?;
    Ok(Json(ApiResponse::success(page_result.records)))
}

/// 创建字典类型
#[post("/types")]
#[perm("system:dict:add")]
#[utoipa::path(post, path = "/api/v1/system/dict/types", tag = "字典管理",
    request_body = CreateDictTypeDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create_type(
    State(state): State<AppState>,
    Json(dto): Json<CreateDictTypeDto>,
) -> AppResult<Json<ApiResponse<DictTypeVo>>> {
    dto.validate()?;
    state
        .dict_service
        .create_type(&state.db, &dto.name, &dto.code)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新字典类型
#[put("/types/{id}")]
#[perm("system:dict:edit")]
#[utoipa::path(put, path = "/api/v1/system/dict/types/{id}", tag = "字典管理",
    params(("id" = i64, Path)),
    request_body = UpdateDictTypeDto,
    responses((status = 200, description = "更新成功")),
    security(("bearer" = [])))]
async fn update_type(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDictTypeDto>,
) -> AppResult<Json<ApiResponse<DictTypeVo>>> {
    dto.validate()?;
    state
        .dict_service
        .update_type(&state.db, id, &dto.name, dto.status)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 删除字典类型
#[delete("/types/{id}")]
#[perm("system:dict:remove")]
#[utoipa::path(delete, path = "/api/v1/system/dict/types/{id}", tag = "字典管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
async fn delete_type(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.dict_service.delete_type(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

#[derive(Debug, Deserialize)]
struct ListDataQuery {
    type_code: String,
}

#[get("/data")]
#[perm("system:dict:list")]
async fn list_data(
    State(state): State<AppState>,
    Query(query): Query<ListDataQuery>,
) -> AppResult<Json<ApiResponse<Vec<DictDataVo>>>> {
    state
        .dict_service
        .find_data_by_type(&state.db, &query.type_code)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 通过字典类型编码查询字典数据
/// 查询字典数据
#[get("/data/type/{dict_type}")]
#[perm("system:dict:list")]
#[utoipa::path(get, path = "/api/v1/system/dict/data/type/{dict_type}", tag = "字典管理",
    params(("dict_type" = String, Path)), responses((status = 200, description = "字典数据")), security(("bearer" = [])))]
async fn list_data_by_type_path(
    State(state): State<AppState>,
    Path(dict_type): Path<String>,
) -> AppResult<Json<ApiResponse<Vec<serde_json::Value>>>> {
    let data = state
        .dict_service
        .find_data_by_type(&state.db, &dict_type)
        .await?;
    let items: Vec<serde_json::Value> = data
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "dictLabel": d.label,
                "dictValue": d.value,
                "cssClass": d.css_class,
            })
        })
        .collect();
    Ok(Json(ApiResponse::success(items)))
}

/// 创建字典数据
#[post("/data")]
#[perm("system:dict:add")]
#[utoipa::path(post, path = "/api/v1/system/dict/data", tag = "字典管理",
    request_body = CreateDictDataDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create_data(
    State(state): State<AppState>,
    Json(dto): Json<CreateDictDataDto>,
) -> AppResult<Json<ApiResponse<DictDataVo>>> {
    dto.validate()?;
    state
        .dict_service
        .create_data(
            &state.db,
            &dto.type_code,
            &dto.label,
            &dto.value,
            dto.sort.unwrap_or(0),
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新字典数据
#[put("/data/{id}")]
#[perm("system:dict:edit")]
#[utoipa::path(put, path = "/api/v1/system/dict/data/{id}", tag = "字典管理",
    params(("id" = i64, Path)),
    request_body = UpdateDictDataDto,
    responses((status = 200, description = "更新成功")),
    security(("bearer" = [])))]
async fn update_data(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateDictDataDto>,
) -> AppResult<Json<ApiResponse<DictDataVo>>> {
    dto.validate()?;
    state
        .dict_service
        .update_data(
            &state.db,
            id,
            &dto.label,
            &dto.value,
            dto.sort.unwrap_or(0),
            dto.status,
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 删除字典数据
#[delete("/data/{id}")]
#[perm("system:dict:remove")]
#[utoipa::path(delete, path = "/api/v1/system/dict/data/{id}", tag = "字典管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
async fn delete_data(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.dict_service.delete_data(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 字典类型导出数据
#[derive(Debug, Serialize)]
struct DictTypeExportData {
    pub name: String,
    pub code: String,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: String,
}

impl DictTypeExportData {
    fn excel_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("name", "字典名称"),
            ("code", "字典类型"),
            ("status", "状态"),
            ("remark", "备注"),
            ("created_at", "创建时间"),
        ]
    }
}

/// 导出字典类型
#[get("/types/export")]
#[perm("system:dict:export")]
async fn export_dict_types(State(state): State<AppState>) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let query = PageQuery::all_records();
    let page_result = state
        .dict_service
        .find_types_by_page(&state.db, query)
        .await?;
    let export_data: Vec<DictTypeExportData> = page_result
        .records
        .into_iter()
        .map(|t| DictTypeExportData {
            name: t.name,
            code: t.code,
            status: t.status,
            remark: t.remark,
            created_at: t.created_at.to_rfc3339(),
        })
        .collect();

    let bytes = ExcelExporter::export_to_bytes(
        &export_data,
        "字典类型",
        &DictTypeExportData::excel_headers(),
    )?;

    excel_response(bytes, "dict_types.xlsx")
}
