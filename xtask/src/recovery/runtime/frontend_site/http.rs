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

pub(super) fn router(site: Site) -> Router {
    Router::new().fallback(respond).with_state(Arc::new(site))
}

async fn respond(
    State(site): State<Arc<Site>>,
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
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn serve_asset(site: &Site, method: &Method, relative: &str, headers: &HeaderMap) -> Response {
    if relative == RECEIPT {
        return empty(StatusCode::NOT_FOUND);
    }
    let requested = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let asset = site.assets.get(requested).or_else(|| {
        let leaf = requested.rsplit('/').next().unwrap_or(requested);
        if Path::new(leaf).extension().is_none() && accepts_html(headers) {
            return site.assets.get("index.html");
        }
        None
    });
    match asset {
        Some(asset) => success(asset, method == Method::HEAD),
        None => empty(StatusCode::NOT_FOUND),
    }
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
