use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{NoticeListParams, NoticeVo};
use validator::Validate;

use crate::dto::notice_dto::{CreateNoticeDto, UpdateNoticeDto};
use crate::state::AppState;
use crate::{detail_body, list_query, remove_body};
use ryframe_auth::RequestPrincipal;

list_query!(pub NoticeListQuery, NoticeFilterQuery {
    title: String,
    notice_type: String,
    status: String,
});

impl NoticeFilterQuery {
    fn into_service_params(self, page: PageQuery) -> NoticeListParams {
        NoticeListParams {
            page,
            title: self.title,
            notice_type: self.notice_type,
            status: self.status,
        }
    }
}

pub fn notice_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(list_no_page))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .with_state(state)
}

/// 通知公告列表
#[get("/")]
#[perm("system:notice:list")]
#[utoipa::path(get, path = "/api/v1/system/notices", tag = "通知公告",
    params(NoticeListQuery),
    responses((status = 200, description = "公告列表", body = ApiPageResponse<NoticeVo>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<NoticeListQuery>,
) -> AppResult<Json<ApiPageResponse<NoticeVo>>> {
    let (page, filter) = query.into_parts();
    state
        .services
        .notice
        .find_by_page(&current_user, filter.into_service_params(page))
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 通知公告列表不分页查询（返回全部数据）
#[get("/all")]
#[perm("system:notice:list")]
#[utoipa::path(get, path = "/api/v1/system/notices/all", tag = "通知公告",
    params(NoticeFilterQuery),
    responses((status = 200, description = "公告列表", body = ApiResponse<Vec<NoticeVo>>)),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<NoticeFilterQuery>,
) -> AppResult<Json<ApiResponse<Vec<NoticeVo>>>> {
    state
        .services
        .notice
        .find_by_page(
            &current_user,
            query.into_service_params(PageQuery::all_records()),
        )
        .await
        .map(|p| Json(ApiResponse::success(p.records)))
}

/// 通知公告详情
#[get("/{id}")]
#[perm("system:notice:list")]
#[utoipa::path(get, path = "/api/v1/system/notices/{id}", tag = "通知公告",
    params(("id" = i64, Path)),
    responses((status = 200, description = "通知详情", body = ApiResponse<NoticeVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<NoticeVo>>> {
    detail_body!(state, current_user, id, notice, NoticeVo, "通知公告")
}

/// 创建通知公告
#[post("/")]
#[perm("system:notice:add")]
#[utoipa::path(post, path = "/api/v1/system/notices", tag = "通知公告",
    request_body = CreateNoticeDto, responses((status = 200, description = "创建成功", body = ApiResponse<NoticeVo>)), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateNoticeDto>,
) -> AppResult<Json<ApiResponse<NoticeVo>>> {
    dto.validate()?;
    state
        .services
        .notice
        .create(
            &current_user,
            &dto.title,
            &dto.content,
            dto.notice_type.as_deref(),
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新通知公告
#[put("/{id}")]
#[perm("system:notice:edit")]
#[utoipa::path(put, path = "/api/v1/system/notices/{id}", tag = "通知公告",
    params(("id" = i64, Path)),
    request_body = UpdateNoticeDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<NoticeVo>)),
    security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateNoticeDto>,
) -> AppResult<Json<ApiResponse<NoticeVo>>> {
    dto.validate()?;
    state
        .services
        .notice
        .update(
            &current_user,
            id,
            &dto.title,
            &dto.content,
            dto.notice_type.as_deref(),
            dto.status,
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 删除通知公告
#[delete("/{id}")]
#[perm("system:notice:remove")]
#[utoipa::path(delete, path = "/api/v1/system/notices/{id}", tag = "通知公告",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    remove_body!(state, current_user, id, notice)
}
