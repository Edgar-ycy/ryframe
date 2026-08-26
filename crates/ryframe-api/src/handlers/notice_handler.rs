use axum::{
    Json, Router,
    extract::{Extension, Path, State},
};
use ryframe_application::system::operations::{
    MessageAudienceKind, MessageAudienceSelector, PublishMessageParams,
};
use ryframe_kernel::{AppError, LocalizedText};
use ryframe_macro::{post, route};

use crate::RequestPrincipal;
use crate::http::{ApiResponse, HttpResult};
use crate::message_presenter::{PublishedMessageVo, into_message_text, render_published};
use crate::request_locale::RequestLocale;
use crate::state::AppState;

pub fn notice_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(publish_to_message_center))
        .with_state(state)
}

/// 将已发布公告显式投递到当前租户的消息中心。
///
/// 使用公告 ID 作为业务幂等键，因此重复点击不会再次创建收件人快照。
#[post("/{id}/publish-message")]
#[perm("system:message:publish")]
#[utoipa::path(post, path = "/api/v1/system/notices/{id}/publish-message", tag = "通知公告",
    params(("id" = String, Path)),
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
        .content
        .generated
        .notice
        .find_by_id(&current_user, id)
        .await?
        .ok_or_else(|| AppError::NotFound("通知公告不存在".into()))?;
    if notice.status != "1" {
        return Err(AppError::Validation("仅已发布的通知公告可以投递到消息中心".into()).into());
    }

    state
        .services
        .operations
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
        .map_err(crate::http::HttpAppError::from)
        .map(|published| render_published(published, &state.localizer, locale))
        .map(ApiResponse::success)
        .map(Json)
}
