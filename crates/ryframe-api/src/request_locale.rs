use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use ryframe_auth::RequestPrincipal;
use ryframe_kernel::Locale;

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

/// 优先按 `Accept-Language` 协商；没有可用值时再使用用户偏好；最终回退中文。
fn negotiate_locale(accept_language: Option<&str>, preferred_locale: Option<&str>) -> Locale {
    accept_language
        .and_then(parse_accept_language)
        .or_else(|| preferred_locale.and_then(Locale::parse))
        .unwrap_or(Locale::DEFAULT)
}

fn parse_accept_language(header: &str) -> Option<Locale> {
    let mut candidates = header
        .split(',')
        .filter_map(|item| {
            let mut parts = item.trim().split(';');
            let language = parts.next()?.trim();
            let quality = parts
                .find_map(|part| part.trim().strip_prefix("q="))
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            (quality > 0.0).then_some((Locale::parse(language), quality))
        })
        .filter_map(|(locale, quality)| locale.map(|locale| (locale, quality)))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.1.total_cmp(&left.1));
    candidates.first().map(|(locale, _)| *locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_negotiation_honors_quality_and_fallbacks() {
        assert_eq!(
            negotiate_locale(Some("zh-CN;q=0.5, en-US;q=0.9"), Some("zh-CN")),
            Locale::EnUs
        );
        assert_eq!(negotiate_locale(None, Some("en-GB")), Locale::EnUs);
        assert_eq!(negotiate_locale(Some("fr-FR"), None), Locale::DEFAULT);
    }
}
