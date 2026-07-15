use axum::{
    Json, Router,
    extract::{Query, State},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_macro::{get, route};
use ryframe_service::system::OperLogVo;
use serde::Serialize;

use super::auth_handler::AppState;
use crate::dto::oper_log_dto::OperLogPageQuery;
use crate::extractors::CurrentUser;
use crate::handler_utils::excel_response;

pub fn oper_log_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(list_no_page))
        .merge(route!(export_oper_logs))
        .with_state(state)
}

/// 操作日志列表
#[get("/", "/list")]
#[perm("system:operlog:list")]
#[utoipa::path(get, path = "/api/v1/system/operlogs", tag = "操作日志",
    responses((status = 200, description = "日志列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Query(query): Query<OperLogPageQuery>,
) -> AppResult<Json<ApiPageResponse<OperLogVo>>> {
    state
        .oper_log_service
        .find_by_page(
            &state.db,
            ryframe_core::PageQuery {
                page: query.page,
                page_size: query.page_size,
            },
            query.oper_name.as_deref(),
            query.status,
            query.begin_time.as_deref(),
            query.end_time.as_deref(),
            &current_user.to_data_scope_context(),
        )
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 操作日志不分页查询（返回全部数据）
#[get("/listNoPage")]
#[perm("system:operlog:list")]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Query(query): Query<OperLogPageQuery>,
) -> AppResult<Json<ApiResponse<Vec<OperLogVo>>>> {
    let logs = state
        .oper_log_service
        .find_all_filtered(
            &state.db,
            query.oper_name.as_deref(),
            query.status,
            query.begin_time.as_deref(),
            query.end_time.as_deref(),
            &current_user.to_data_scope_context(),
        )
        .await?;
    Ok(Json(ApiResponse::success(logs)))
}

/// 操作日志导出数据
#[derive(Debug, Serialize)]
struct OperLogExportData {
    pub title: String,
    pub business_type: String,
    pub oper_name: String,
    pub oper_url: String,
    pub oper_ip: String,
    pub status: String,
    pub cost_time: i64,
    pub oper_time: String,
}

impl OperLogExportData {
    fn excel_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("title", "操作模块"),
            ("business_type", "业务类型"),
            ("oper_name", "操作人员"),
            ("oper_url", "请求地址"),
            ("oper_ip", "操作IP"),
            ("status", "状态"),
            ("cost_time", "耗时(ms)"),
            ("oper_time", "操作时间"),
        ]
    }
}

/// 导出操作日志
#[get("/export")]
#[perm("system:operlog:export")]
async fn export_oper_logs(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Query(query): Query<OperLogPageQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let logs = state
        .oper_log_service
        .find_all_filtered(
            &state.db,
            query.oper_name.as_deref(),
            query.status,
            query.begin_time.as_deref(),
            query.end_time.as_deref(),
            &current_user.to_data_scope_context(),
        )
        .await?;

    let export_data: Vec<OperLogExportData> = logs
        .into_iter()
        .map(|l| OperLogExportData {
            title: l.title,
            business_type: l.business_type,
            oper_name: l.oper_name,
            oper_url: l.oper_url,
            oper_ip: l.oper_ip,
            status: l.status,
            cost_time: l.cost_time,
            oper_time: l.oper_time,
        })
        .collect();

    let bytes = ExcelExporter::export_to_bytes(
        &export_data,
        "操作日志",
        &OperLogExportData::excel_headers(),
    )?;

    excel_response(bytes, "oper_logs.xlsx")
}
