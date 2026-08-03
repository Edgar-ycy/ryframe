use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
};
use ryframe_core::ValidatedPageQuery;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_i18n::LocalizedText;
use ryframe_kernel::AppError;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{
    MessageAudienceKind, MessageAudienceSelector, NoticeListParams, PublishMessageParams,
};
use validator::Validate;

use crate::dto::notice_dto::{CreateNoticeDto, UpdateNoticeDto};
use crate::dto::public_dto::NoticeVo;
use crate::message_presenter::{PublishedMessageVo, into_message_text, render_published};
use crate::request_locale::RequestLocale;
use crate::state::AppState;
use crate::{detail_body, list_query, remove_body};
use ryframe_auth::RequestPrincipal;

list_query!(pub NoticeListQuery, NoticeFilterQuery {
    title: String,
    notice_type: String,
    status: String,
});

impl NoticeFilterQuery {
    fn into_service_params(self, page: ValidatedPageQuery) -> NoticeListParams {
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
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(publish_to_message_center))
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
) -> HttpResult<Json<ApiPageResponse<NoticeVo>>> {
    let (page, filter) = query.into_parts(&state.config.pagination)?;
    state
        .services
        .notice
        .find_by_page(&current_user, filter.into_service_params(page))
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|p| {
            Json(ApiPageResponse::new(
                p.records.into_iter().map(NoticeVo::from).collect(),
                p.total,
                p.page,
                p.page_size,
                state.config.pagination.max_page_size,
                "查询成功",
            ))
        })
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
) -> HttpResult<Json<ApiResponse<NoticeVo>>> {
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
) -> HttpResult<Json<ApiResponse<NoticeVo>>> {
    dto.validate()?;
    state
        .services
        .notice
        .create(
            &current_user,
            &dto.title,
            &dto.content_markdown,
            dto.notice_type.as_deref(),
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
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
) -> HttpResult<Json<ApiResponse<NoticeVo>>> {
    dto.validate()?;
    state
        .services
        .notice
        .update(
            &current_user,
            id,
            &dto.title,
            &dto.content_markdown,
            dto.notice_type.as_deref(),
            dto.status,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 将已发布公告显式投递到当前租户的消息中心。
///
/// 使用公告 ID 作为业务幂等键，因此重复点击不会再次创建收件人快照。
#[post("/{id}/publish-message")]
#[perm("system:message:publish")]
#[utoipa::path(post, path = "/api/v1/system/notices/{id}/publish-message", tag = "通知公告",
    params(("id" = i64, Path)),
    responses((status = 200, description = "消息中心发布结果", body = ApiResponse<PublishedMessageVo>)),
    security(("bearer" = [])))]
async fn publish_to_message_center(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Extension(RequestLocale(locale)): Extension<RequestLocale>,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<PublishedMessageVo>>> {
    let notice = state
        .services
        .notice
        .find_by_id(&current_user, id)
        .await?
        .ok_or_else(|| AppError::NotFound("通知公告不存在".into()))?;
    if notice.status != "1" {
        return Err(AppError::Validation("仅已发布的通知公告可以投递到消息中心".into()).into());
    }

    state
        .services
        .message
        .publish(
            &current_user,
            PublishMessageParams {
                tenant_id: None,
                topic: "notice".into(),
                title: into_message_text(
                    LocalizedText::Literal {
                        value: notice.title,
                    },
                    &state.localizer,
                )?,
                content: into_message_text(
                    LocalizedText::Literal {
                        value: notice.content_markdown,
                    },
                    &state.localizer,
                )?,
                severity: "info".into(),
                payload: Some(serde_json::json!({ "notice_id": id.to_string() })),
                source_type: Some("notice".into()),
                source_id: Some(id.to_string()),
                audiences: vec![MessageAudienceSelector {
                    kind: MessageAudienceKind::Tenant,
                    target_id: 0,
                }],
                expires_at: None,
            },
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|published| render_published(published, &state.localizer, locale))
        .map(ApiResponse::success)
        .map(Json)
}

/// 删除通知公告
#[delete("/{id}")]
#[perm("system:notice:remove")]
#[utoipa::path(delete, path = "/api/v1/system/notices/{id}", tag = "通知公告",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_http::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<()>>> {
    remove_body!(state, current_user, id, notice)
}
