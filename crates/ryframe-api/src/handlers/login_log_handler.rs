use axum::{
    Json, Router,
    extract::{Query, State},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_macro::{get, route};
use ryframe_service::system::LoginInfoVo;
use serde::Serialize;

use super::auth_handler::AppState;
use crate::dto::login_log_dto::LoginLogPageQuery;
use crate::extractors::CurrentUser;
use crate::handler_utils::excel_response;

pub fn login_log_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(list_no_page))
        .merge(route!(export_login_logs))
        .with_state(state)
}

/// 登录日志列表
#[get("/", "/list")]
#[perm("system:logininfor:list")]
#[utoipa::path(get, path = "/api/v1/system/loginlogs", tag = "登录日志",
    responses((status = 200, description = "日志列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Query(query): Query<LoginLogPageQuery>,
) -> AppResult<Json<ApiPageResponse<LoginInfoVo>>> {
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
            &current_user.to_data_scope_context(),
        )
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 登录日志不分页查询（返回全部数据）
#[get("/listNoPage")]
#[perm("system:logininfor:list")]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Query(query): Query<LoginLogPageQuery>,
) -> AppResult<Json<ApiResponse<Vec<LoginInfoVo>>>> {
    let logs = state
        .login_info_service
        .find_all_filtered(
            &state.db,
            query.user_name.as_deref(),
            query.status,
            query.begin_time.as_deref(),
            query.end_time.as_deref(),
            &current_user.to_data_scope_context(),
        )
        .await?;
    Ok(Json(ApiResponse::success(logs)))
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
#[get("/export")]
#[perm("system:logininfor:export")]
async fn export_login_logs(
    State(state): State<AppState>,
    current_user: CurrentUser,
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
            &current_user.to_data_scope_context(),
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

    excel_response(bytes, "login_logs.xlsx")
}
