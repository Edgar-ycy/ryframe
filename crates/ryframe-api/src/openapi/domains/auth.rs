use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::handlers::auth_handler::session::csrf,
        crate::handlers::auth_handler::login::login,
        crate::handlers::auth_handler::session::logout,
        crate::handlers::auth_handler::session::refresh,
        crate::handlers::auth_handler::context::context,
        crate::handlers::auth_handler::password_reset::complete_password_reset,
        crate::handlers::auth_handler::session::list_sessions,
        crate::handlers::auth_handler::session::revoke_session,
        crate::handlers::auth_handler::session::revoke_other_sessions,
        crate::handlers::auth_handler::ws_ticket::websocket_ticket,
        crate::handlers::captcha_handler::generate_captcha_handler,
        crate::handlers::captcha_handler::captcha_image_handler,
        crate::handlers::captcha_handler::verify_captcha_handler,
        crate::handlers::captcha_handler::get_captcha_config_handler,
        crate::handlers::profile_handler::get_profile,
        crate::handlers::profile_handler::update_profile,
        crate::handlers::profile_handler::change_password,
        crate::handlers::profile_handler::update_avatar
    ),
    components(schemas(
        crate::dto::auth_dto::LoginRequest,
        crate::dto::auth_dto::CompletePasswordResetRequest,
        crate::dto::auth_dto::LoginResponse,
        crate::dto::auth_dto::SessionUserVo,
        crate::dto::auth_dto::TenantBusinessDataContextVo,
        crate::dto::auth_dto::SessionContextVo,
        crate::dto::auth_dto::CsrfResponse,
        crate::dto::auth_dto::AuthSessionResponse,
        crate::dto::auth_dto::RevokeOtherSessionsResponse,
        crate::dto::empty_dto::EmptyRequestDto,
        crate::message_socket::WebSocketTicketResponse,
        crate::handlers::captcha_handler::CaptchaQuery,
        crate::handlers::captcha_handler::CaptchaResponse,
        crate::handlers::captcha_handler::CaptchaVerifyRequest,
        crate::handlers::captcha_handler::CaptchaVerifyResponse,
        crate::handlers::captcha_handler::CaptchaConfigResponse,
        crate::dto::public_dto::UserInfo,
        crate::dto::profile_dto::UpdateProfileRequest,
        crate::dto::profile_dto::ChangePasswordRequest,
        crate::dto::profile_dto::AvatarResponse,
        crate::dto::public_dto::UserProfileResponse
    ))
)]
struct AuthDoc;

pub(super) fn document() -> utoipa::openapi::OpenApi {
    AuthDoc::openapi()
}
