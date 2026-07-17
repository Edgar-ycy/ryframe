use axum::{
    Json, Router,
    extract::{Query, State},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_macro::{get, route};
use ryframe_service::system::LoginInfoVo;
use serde::Serialize;

use crate::dto::login_log_dto::{LoginLogFilterQuery, LoginLogPageQuery};
use crate::handler_utils::excel_response;
use crate::state::AppState;
use ryframe_auth::RequestPrincipal;

pub fn login_log_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(list_no_page))
        .merge(route!(export_login_logs))
        .with_state(state)
}

/// 登录日志列表
#[get("/")]
#[perm("system:logininfor:list")]
#[utoipa::path(get, path = "/api/v1/system/loginlogs", tag = "登录日志",
    params(LoginLogPageQuery),
    responses((status = 200, description = "日志列表", body = ApiPageResponse<LoginInfoVo>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<LoginLogPageQuery>,
) -> AppResult<Json<ApiPageResponse<LoginInfoVo>>> {
    state
        .services
        .login_info
        .find_by_page(&current_user, query.into_service_query())
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 登录日志不分页查询（返回全部数据）
#[get("/all")]
#[perm("system:logininfor:list")]
#[utoipa::path(get, path = "/api/v1/system/loginlogs/all", tag = "登录日志",
    params(LoginLogFilterQuery),
    responses((status = 200, description = "全部登录日志", body = ApiResponse<Vec<LoginInfoVo>>)), security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<LoginLogFilterQuery>,
) -> AppResult<Json<ApiResponse<Vec<LoginInfoVo>>>> {
    let logs = state
        .services
        .login_info
        .find_all(
            &current_user,
            query.into_service_query(ryframe_core::PageQuery::all_records()),
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
#[utoipa::path(get, path = "/api/v1/system/loginlogs/export", tag = "登录日志",
    params(LoginLogFilterQuery),
    responses((status = 200, description = "导出登录日志 Excel", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")), security(("bearer" = [])))]
async fn export_login_logs(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<LoginLogFilterQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let logs = state
        .services
        .login_info
        .find_all(
            &current_user,
            query.into_service_query(ryframe_core::PageQuery::all_records()),
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
