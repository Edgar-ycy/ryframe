use std::{borrow::Cow, path::Path, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::Response,
};
use percent_encoding::percent_decode_str;

use super::content::{Asset, RECEIPT, Site};

struct StaticSite {
    site: Site,
    production_asset_cache: bool,
}

pub(super) fn router(site: Site, production_asset_cache: bool) -> Router {
    Router::new()
        .fallback(respond)
        .with_state(Arc::new(StaticSite {
            site,
            production_asset_cache,
        }))
}

async fn respond(
    State(site): State<Arc<StaticSite>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let mut response = if method != Method::GET && method != Method::HEAD {
        let mut response = empty(StatusCode::METHOD_NOT_ALLOWED);
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        response
    } else {
        match decode_path(&uri) {
            Ok(relative) => serve_asset(&site, &method, &relative, &headers),
            Err(()) => empty(StatusCode::BAD_REQUEST),
        }
    };
    let headers = response.headers_mut();
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn serve_asset(
    site: &StaticSite,
    method: &Method,
    relative: &str,
    headers: &HeaderMap,
) -> Response {
    if relative == RECEIPT {
        return empty(StatusCode::NOT_FOUND);
    }
    let requested = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let direct = site.site.assets.get(requested);
    let asset = direct.or_else(|| {
        let leaf = requested.rsplit('/').next().unwrap_or(requested);
        if Path::new(leaf).extension().is_none() && accepts_html(headers) {
            return site.site.assets.get("index.html");
        }
        None
    });
    match asset {
        Some(asset) => {
            let mut response = success(asset, method == Method::HEAD);
            if site.production_asset_cache && direct.is_some() && production_asset(requested) {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                );
            }
            response
        }
        None => empty(StatusCode::NOT_FOUND),
    }
}

fn production_asset(relative: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        "js", "css", "woff", "woff2", "ttf", "svg", "png", "jpg", "jpeg", "webp", "ico",
    ];
    // 与 deploy/nginx/ryframe.conf 的生产资源扩展名一致。
    relative.starts_with("assets/")
        && Path::new(relative)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                EXTENSIONS
                    .iter()
                    .any(|expected| extension.eq_ignore_ascii_case(expected))
            })
}

fn decode_path(uri: &Uri) -> std::result::Result<Cow<'_, str>, ()> {
    if uri.scheme().is_some() || uri.authority().is_some() {
        return Err(());
    }
    let raw = uri.path().strip_prefix('/').ok_or(())?;
    let bytes = raw.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !bytes
                .get(index + 1..index + 3)
                .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit))
        {
            return Err(());
        }
    }
    let relative = percent_decode_str(raw).decode_utf8().map_err(|_| ())?;
    if relative.chars().any(|ch| ch == '\\' || ch.is_control())
        || (!relative.is_empty()
            && relative
                .split('/')
                .any(|part| matches!(part, "" | "." | "..")))
    {
        return Err(());
    }
    Ok(relative)
}

fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|entry| {
            let mut parts = entry.split(';');
            parts
                .next()
                .is_some_and(|media| media.trim().eq_ignore_ascii_case("text/html"))
                && parts.all(|parameter| {
                    parameter.split_once('=').is_none_or(|(name, value)| {
                        !name.trim().eq_ignore_ascii_case("q")
                            || value
                                .trim()
                                .parse::<f32>()
                                .is_ok_and(|q| q > 0.0 && q <= 1.0)
                    })
                })
        })
}

fn success(asset: &Asset, head: bool) -> Response {
    let body = if head {
        Body::empty()
    } else {
        Body::from(asset.bytes.clone())
    };
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, asset.media.clone());
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(asset.bytes.len()));
    response
}

fn empty(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    response
}
