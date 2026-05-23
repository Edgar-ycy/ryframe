use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_service::system::OperLogVo;
use serde::Serialize;

use super::auth_handler::AppState;
use crate::dto::oper_log_dto::OperLogPageQuery;

pub fn oper_log_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list))
        .route("/list", get(list))
        .route("/listNoPage", get(list_no_page))
        .route("/export", get(export_oper_logs))
        .route("/clean", axum::routing::delete(clean))
        .with_state(state)
}

/// 操作日志列表
#[utoipa::path(get, path = "/api/v1/system/operlogs", tag = "操作日志",
    responses((status = 200, description = "日志列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
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
        )
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 操作日志不分页查询（返回全部数据）
async fn list_no_page(
    State(state): State<AppState>,
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
        )
        .await?;
    Ok(Json(ApiResponse::success(logs)))
}

/// 清空操作日志
#[utoipa::path(delete, path = "/api/v1/system/operlogs/clean", tag = "操作日志",
    responses((status = 200, description = "清空成功")), security(("bearer" = [])))]
async fn clean(State(state): State<AppState>) -> AppResult<Json<ApiResponse<()>>> {
    let count = state.oper_log_service.clean(&state.db).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "成功清空 {} 条操作日志",
        count
    ))))
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
async fn export_oper_logs(
    State(state): State<AppState>,
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

    let response = axum::response::Response::builder()
        .status(200)
        .header(
            "Content-Type",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .header("Content-Disposition", "attachment; filename=oper_logs.xlsx")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ryframe_common::AppError::Internal(format!("构建响应失败: {}", e)))?;

    Ok(response)
}
