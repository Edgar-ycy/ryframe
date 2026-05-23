use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use ryframe_common::AppResult;
use ryframe_core::PageResult;
use ryframe_service::system::LoginInfoVo;
use serde::Serialize;

use super::auth_handler::AppState;
use crate::dto::login_log_dto::LoginLogPageQuery;

pub fn login_log_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list))
        .route("/export", get(export_login_logs))
        .route("/clean", axum::routing::delete(clean))
        .with_state(state)
}

/// 登录日志列表
#[utoipa::path(get, path = "/api/v1/system/loginlogs", tag = "登录日志",
    responses((status = 200, description = "日志列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    Query(query): Query<LoginLogPageQuery>,
) -> AppResult<Json<PageResult<LoginInfoVo>>> {
    state
        .login_info_service
        .find_by_page(
            &state.db,
            ryframe_core::PageQuery {
                page: query.page,
                page_size: query.page_size,
            },
            query.user_name.as_deref(),
            query.status,
            query.begin_time.as_deref(),
            query.end_time.as_deref(),
        )
        .await
        .map(Json)
}

/// 清空登录日志
#[utoipa::path(delete, path = "/api/v1/system/loginlogs/clean", tag = "登录日志",
    responses((status = 200, description = "清空成功")), security(("bearer" = [])))]
async fn clean(State(state): State<AppState>) -> AppResult<Json<serde_json::Value>> {
    let count = state.login_info_service.clean(&state.db).await?;
    Ok(Json(
        serde_json::json!({"message": format!("成功清空 {} 条登录日志", count)}),
    ))
}

/// 登录日志导出数据
#[derive(Debug, Serialize)]
struct LoginLogExportData {
    pub user_name: String,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub status: String,
    pub msg: Option<String>,
    pub login_time: String,
}

impl LoginLogExportData {
    fn excel_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("user_name", "用户名"),
            ("ipaddr", "IP地址"),
            ("login_location", "登录地点"),
            ("browser", "浏览器"),
            ("os", "操作系统"),
            ("status", "状态"),
            ("msg", "提示消息"),
            ("login_time", "登录时间"),
        ]
    }
}

/// 导出登录日志
async fn export_login_logs(
    State(state): State<AppState>,
    Query(query): Query<LoginLogPageQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let logs = state
        .login_info_service
        .find_all_filtered(
            &state.db,
            query.user_name.as_deref(),
            query.status,
            query.begin_time.as_deref(),
            query.end_time.as_deref(),
        )
        .await?;

    let export_data: Vec<LoginLogExportData> = logs
        .into_iter()
        .map(|l| LoginLogExportData {
            user_name: l.user_name,
            ipaddr: l.ipaddr,
            login_location: l.login_location,
            browser: l.browser,
            os: l.os,
            status: l.status,
            msg: l.msg,
            login_time: l.login_time,
        })
        .collect();

    let bytes = ExcelExporter::export_to_bytes(
        &export_data,
        "登录日志",
        &LoginLogExportData::excel_headers(),
    )?;

    let response = axum::response::Response::builder()
        .status(200)
        .header(
            "Content-Type",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .header(
            "Content-Disposition",
            "attachment; filename=login_logs.xlsx",
        )
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ryframe_common::AppError::Internal(format!("构建响应失败: {}", e)))?;

    Ok(response)
}
