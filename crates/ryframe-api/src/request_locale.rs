use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use ryframe_auth::RequestPrincipal;
use ryframe_i18n::{Locale, negotiate_locale};

/// 当前请求协商出的语言，可由处理器读取并用于渲染本地化文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLocale(pub Locale);

/// 根据请求头和已认证用户偏好协商语言，并为响应补充语言相关缓存头。
///
/// 此中间件可以在认证前后各执行一次：认证后的内层执行会依据用户偏好更新语言，
/// 外层执行只在响应尚未设置语言头时补齐公共端点的默认值。
pub async fn request_locale_middleware(mut request: Request, next: Next) -> Response {
    let accept_language = request
        .headers()
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok());
    let preferred_locale = request
        .extensions()
        .get::<RequestPrincipal>()
        .and_then(|principal| principal.preferred_locale.as_deref());
    let locale = negotiate_locale(accept_language, preferred_locale);
    request.extensions_mut().insert(RequestLocale(locale));

    let mut response = next.run(request).await;
    response
        .headers_mut()
        .entry(header::CONTENT_LANGUAGE)
        .or_insert_with(|| HeaderValue::from_static(locale.as_str()));
    ensure_vary_accept_language(response.headers_mut());
    response
}

fn ensure_vary_accept_language(headers: &mut HeaderMap) {
    let already_varies = headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case("accept-language"));
    if !already_varies {
        headers.append(header::VARY, HeaderValue::from_static("Accept-Language"));
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        extract::Extension,
        http::{Request, StatusCode, header},
        middleware,
        routing::get,
    };
    use tower::ServiceExt;

    use super::{RequestLocale, request_locale_middleware};
    use ryframe_i18n::Locale;

    #[tokio::test]
    async fn middleware_negotiates_locale_and_adds_response_header() {
        async fn handler(Extension(RequestLocale(locale)): Extension<RequestLocale>) -> StatusCode {
            assert_eq!(locale, Locale::EnUs);
            StatusCode::NO_CONTENT
        }

        let app = Router::new()
            .route("/", get(handler))
            .layer(middleware::from_fn(request_locale_middleware));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ACCEPT_LANGUAGE, "en-GB,en;q=0.9")
                    .body(axum::body::Body::empty())
                    .expect("请求格式"),
            )
            .await
            .expect("路由响应");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_LANGUAGE),
            Some(&axum::http::HeaderValue::from_static("en-US"))
        );
        let vary = response
            .headers()
            .get_all(header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>();
        assert_eq!(vary, vec!["Accept-Language"]);
    }

    #[tokio::test]
    async fn middleware_preserves_existing_vary_values_without_duplicates() {
        async fn handler() -> impl axum::response::IntoResponse {
            ([(header::VARY, "Accept-Encoding")], StatusCode::NO_CONTENT)
        }

        let app = Router::new()
            .route("/", get(handler))
            .layer(middleware::from_fn(request_locale_middleware))
            .layer(middleware::from_fn(request_locale_middleware));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .expect("请求格式"),
            )
            .await
            .expect("路由响应");
        let vary = response
            .headers()
            .get_all(header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>();

        assert_eq!(vary, vec!["Accept-Encoding", "Accept-Language"]);
    }
}
