use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
};
use ryframe_auth::{RequestPrincipal, permission::check_permission};
use ryframe_http::{ApiEmptyResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post, put, route};
use ryframe_service::system::{MessageAudienceKind, MessageAudienceSelector, PublishMessageParams};
use validator::Validate;

use crate::{
    dto::message_dto::{AcknowledgeMessagesDto, MessageInboxQuery, PublishMessageDto},
    message_presenter::{
        MessageInboxPage, PublishedMessageVo, into_message_text, render_inbox, render_published,
    },
    request_locale::RequestLocale,
    state::AppState,
};

const DEFAULT_INBOX_LIMIT: u64 = 30;
const MAX_INBOX_LIMIT: u64 = 100;
const SYSTEM_TENANT_ID: &str = "system";

/// 消息中心路由。
pub fn message_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(inbox))
        .merge(route!(unread_count))
        .merge(route!(publish))
        .merge(route!(acknowledge))
        .merge(route!(mark_read))
        .merge(route!(mark_all_read))
        .with_state(state)
}

/// 获取当前用户的消息收件箱。
#[get("/")]
#[utoipa::path(get, path = "/api/v1/system/messages", tag = "消息中心",
    params(MessageInboxQuery),
    responses((status = 200, description = "消息收件箱", body = ApiResponse<MessageInboxPage>)),
    security(("bearer" = [])))]
async fn inbox(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Extension(RequestLocale(locale)): Extension<RequestLocale>,
    Query(query): Query<MessageInboxQuery>,
) -> HttpResult<Json<ApiResponse<MessageInboxPage>>> {
    let limit = query.limit.unwrap_or(DEFAULT_INBOX_LIMIT);
    if !(1..=MAX_INBOX_LIMIT).contains(&limit) {
        return Err(
            AppError::Validation(format!("limit 必须在 1 到 {MAX_INBOX_LIMIT} 之间")).into(),
        );
    }
    let cursor = parse_optional_id(query.cursor.as_deref(), "cursor")?;
    state
        .services
        .message
        .inbox(&current_user, cursor, limit, query.unread_only)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|page| render_inbox(page, &state.localizer, locale))
        .map(ApiResponse::success)
        .map(Json)
}

/// 获取当前用户未读消息数。
#[get("/unread-count")]
#[utoipa::path(get, path = "/api/v1/system/messages/unread-count", tag = "消息中心",
    responses((status = 200, description = "未读数量", body = ApiResponse<u64>)),
    security(("bearer" = [])))]
async fn unread_count(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<u64>>> {
    state
        .services
        .message
        .unread_count(&current_user)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(ApiResponse::success)
        .map(Json)
}

/// 发布一条消息并固化收件人快照。
#[post("/")]
#[perm("system:message:publish")]
#[utoipa::path(post, path = "/api/v1/system/messages", tag = "消息中心",
    request_body = PublishMessageDto,
    responses((status = 200, description = "发布结果", body = ApiResponse<PublishedMessageVo>)),
    security(("bearer" = [])))]
async fn publish(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Extension(RequestLocale(locale)): Extension<RequestLocale>,
    Json(dto): Json<PublishMessageDto>,
) -> HttpResult<Json<ApiResponse<PublishedMessageVo>>> {
    dto.validate()?;
    let (title, content) = dto.localized_content()?;
    let title = into_message_text(title, &state.localizer)?;
    let content = into_message_text(content, &state.localizer)?;
    let target_tenant_id = dto
        .tenant_id
        .as_deref()
        .unwrap_or(current_user.tenant_id.as_str());
    ensure_cross_tenant_publish_authority(&current_user, target_tenant_id)?;
    let audiences = dto
        .audiences
        .iter()
        .map(parse_audience)
        .collect::<HttpResult<Vec<_>>>()?;
    state
        .services
        .message
        .publish(
            &current_user,
            PublishMessageParams {
                tenant_id: dto.tenant_id,
                topic: dto.topic,
                title,
                content,
                severity: dto.severity,
                payload: dto.payload,
                source_type: dto.source_type,
                source_id: dto.source_id,
                audiences,
                expires_at: dto.expires_at,
            },
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|published| render_published(published, &state.localizer, locale))
        .map(ApiResponse::success)
        .map(Json)
}

/// 批量确认已接收的消息。
#[post("/ack")]
#[utoipa::path(post, path = "/api/v1/system/messages/ack", tag = "消息中心",
    request_body = AcknowledgeMessagesDto,
    responses((status = 200, description = "确认数量", body = ApiResponse<u64>)),
    security(("bearer" = [])))]
async fn acknowledge(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<AcknowledgeMessagesDto>,
) -> HttpResult<Json<ApiResponse<u64>>> {
    dto.validate()?;
    let ids = dto
        .ids
        .iter()
        .map(|id| parse_id(id, "ids"))
        .collect::<HttpResult<Vec<_>>>()?;
    let started = std::time::Instant::now();
    state
        .services
        .message
        .acknowledge(&current_user, &ids)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .inspect(|_| ryframe_middleware::metrics::observe_message_ack_latency(started.elapsed()))
        .map(ApiResponse::success)
        .map(Json)
}

/// 标记单条消息为已读。
#[put("/{id}/read")]
#[utoipa::path(put, path = "/api/v1/system/messages/{id}/read", tag = "消息中心",
    params(("id" = String, Path)),
    responses((status = 200, description = "已读", body = ApiEmptyResponse)),
    security(("bearer" = [])))]
async fn mark_read(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiEmptyResponse>> {
    let message_id = parse_id(&id, "id")?;
    let started = std::time::Instant::now();
    state
        .services
        .message
        .mark_read(&current_user, message_id)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .inspect(|_| ryframe_middleware::metrics::observe_message_ack_latency(started.elapsed()))
        .map(|_| Json(ApiEmptyResponse::success_no_data()))
}

/// 将当前用户全部未读消息标记为已读。
#[put("/read-all")]
#[utoipa::path(put, path = "/api/v1/system/messages/read-all", tag = "消息中心",
    responses((status = 200, description = "已读数量", body = ApiResponse<u64>)),
    security(("bearer" = [])))]
async fn mark_all_read(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<u64>>> {
    let started = std::time::Instant::now();
    state
        .services
        .message
        .mark_all_read(&current_user)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .inspect(|_| ryframe_middleware::metrics::observe_message_ack_latency(started.elapsed()))
        .map(ApiResponse::success)
        .map(Json)
}

fn parse_audience(
    dto: &crate::dto::message_dto::MessageAudienceDto,
) -> HttpResult<MessageAudienceSelector> {
    let kind = match dto.kind.as_str() {
        "tenant" => MessageAudienceKind::Tenant,
        "role" => MessageAudienceKind::Role,
        "user" => MessageAudienceKind::User,
        _ => {
            return Err(
                AppError::Validation("消息受众 kind 只能是 tenant、role 或 user".into()).into(),
            );
        }
    };
    let target_id = match kind {
        MessageAudienceKind::Tenant => match dto.target_id.as_deref() {
            None | Some("0") => 0,
            Some(_) => {
                return Err(
                    AppError::Validation("tenant 受众不能指定非零 target_id".into()).into(),
                );
            }
        },
        MessageAudienceKind::Role | MessageAudienceKind::User => {
            let id = dto
                .target_id
                .as_deref()
                .ok_or_else(|| AppError::Validation("角色和用户受众必须提供 target_id".into()))?;
            parse_id(id, "target_id")?
        }
    };
    Ok(MessageAudienceSelector { kind, target_id })
}

fn ensure_cross_tenant_publish_authority(
    current_user: &RequestPrincipal,
    target_tenant_id: &str,
) -> HttpResult<()> {
    if target_tenant_id == current_user.tenant_id {
        return Ok(());
    }
    if current_user.tenant_id != SYSTEM_TENANT_ID {
        return Err(AppError::Authorization(
            "只有 system 租户的平台管理员可以跨租户发布消息".into(),
        )
        .into());
    }
    check_permission(current_user, "platform:message:publish")
        .map_err(ryframe_http::HttpAppError::from)
}

fn parse_optional_id(value: Option<&str>, field: &str) -> HttpResult<Option<i64>> {
    value.map(|value| parse_id(value, field)).transpose()
}

fn parse_id(value: &str, field: &str) -> HttpResult<i64> {
    Ok(value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation(format!("{field} 必须是正整数 ID")))?)
}

#[cfg(test)]
mod tests {
    use ryframe_auth::RequestPrincipal;
    use ryframe_kernel::{ActorContext, DataScope};

    use super::ensure_cross_tenant_publish_authority;

    fn principal(
        tenant_id: &str,
        permissions: Vec<&str>,
        is_super_admin: bool,
    ) -> RequestPrincipal {
        RequestPrincipal {
            actor: ActorContext {
                user_id: 1,
                tenant_id: tenant_id.into(),
                username: "tester".into(),
                dept_id: None,
                dept_path: None,
                data_scope: DataScope::All,
                custom_dept_ids: Vec::new(),
                include_self: true,
                is_super_admin,
            },
            preferred_locale: None,
            roles: Vec::new(),
            role_ids: Vec::new(),
            permissions: permissions.into_iter().map(str::to_owned).collect(),
            tenant_request_limit_per_minute: 100,
        }
    }

    #[test]
    fn ordinary_tenant_cannot_publish_across_tenants_even_with_platform_permission() {
        let user = principal("tenant-a", vec!["platform:message:publish"], true);

        assert!(ensure_cross_tenant_publish_authority(&user, "tenant-b").is_err());
    }

    #[test]
    fn system_tenant_requires_platform_permission_for_cross_tenant_publish() {
        let authorized = principal("system", vec!["platform:message:publish"], false);
        let unauthorized = principal("system", Vec::new(), false);

        assert!(ensure_cross_tenant_publish_authority(&authorized, "tenant-a").is_ok());
        assert!(ensure_cross_tenant_publish_authority(&unauthorized, "tenant-a").is_err());
    }

    #[test]
    fn same_tenant_publish_does_not_require_platform_permission() {
        let user = principal("tenant-a", Vec::new(), false);

        assert!(ensure_cross_tenant_publish_authority(&user, "tenant-a").is_ok());
    }
}
