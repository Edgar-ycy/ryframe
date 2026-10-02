//! 业务资源的 HTTP 传输层。

pub mod generated;

pub use ryframe_api::{RequestPrincipal, http};
use utoipa::OpenApi;

/// 返回框架 API 与业务资源 API 合并后的完整契约。
pub fn combined_openapi() -> utoipa::openapi::OpenApi {
    let mut document = ryframe_api::openapi::ApiDoc::openapi();
    let business = generated::GeneratedOpenApi::openapi();
    let mut resources = document
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("x-ryframe-crud-resources"))
        .and_then(|value| value.get("resources"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    resources.extend(
        business
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("x-ryframe-crud-resources"))
            .and_then(|value| value.get("resources"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    resources.sort_by(|left, right| {
        left.get("name")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("name").and_then(serde_json::Value::as_str))
    });
    document.merge(business);
    document.extensions.get_or_insert_default().insert(
        "x-ryframe-crud-resources".into(),
        serde_json::json!({ "version": 1, "resources": resources }),
    );
    document
}

#[doc(hidden)]
pub mod __macro_support {
    pub use ryframe_api::__macro_support::perm_route;
}

pub mod handler_utils {
    pub use ryframe_api::parse_id;
}
