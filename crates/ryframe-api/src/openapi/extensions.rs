use std::collections::BTreeSet;

pub(super) fn merge_generated_document(openapi: &mut utoipa::openapi::OpenApi) {
    let mut generated = <crate::generated::GeneratedOpenApi as utoipa::OpenApi>::openapi();
    let generated_extensions = generated.extensions.take();
    openapi.merge(generated);
    if let Some(generated_extensions) = generated_extensions {
        let extensions = openapi.extensions.get_or_insert_default();
        let generated_extensions: std::collections::HashMap<_, _> = generated_extensions.into();
        for (name, value) in generated_extensions {
            assert!(
                extensions.insert(name.clone(), value).is_none(),
                "生成 OpenAPI 扩展与手写扩展冲突: {name}"
            );
        }
    }
}

pub(super) fn configure_security_schemes(openapi: &mut utoipa::openapi::OpenApi) {
    if let Some(components) = openapi.components.as_mut() {
        components.add_security_scheme(
            "bearer",
            utoipa::openapi::security::SecurityScheme::Http(utoipa::openapi::security::Http::new(
                utoipa::openapi::security::HttpAuthScheme::Bearer,
            )),
        );
        components.add_security_scheme(
            "refreshCookie",
            utoipa::openapi::security::SecurityScheme::ApiKey(
                utoipa::openapi::security::ApiKey::Cookie(
                    utoipa::openapi::security::ApiKeyValue::new("ryframe_refresh_token"),
                ),
            ),
        );
    }
}

pub(super) fn add_contract_extensions(openapi: &mut utoipa::openapi::OpenApi) {
    openapi
        .extensions
        .get_or_insert_default()
        .insert("x-ryframe-menu-routes".into(), menu_route_contract());
    openapi.extensions.get_or_insert_default().insert(
        "x-ryframe-product-capabilities".into(),
        product_capability_contract(),
    );
    openapi.extensions.get_or_insert_default().insert(
        "x-ryframe-password-policy".into(),
        password_policy_contract(),
    );
    openapi
        .extensions
        .get_or_insert_default()
        .insert("x-ryframe-notice-policy".into(), notice_policy_contract());
    openapi
        .extensions
        .get_or_insert_default()
        .insert("x-ryframe-api-prefix".into(), api_prefix_contract());
    openapi.extensions.get_or_insert_default().insert(
        "x-ryframe-permission-catalog".into(),
        permission_catalog_contract(),
    );
    openapi
        .extensions
        .get_or_insert_default()
        .insert("x-ryframe-route-contract".into(), route_contract());
    openapi.extensions.get_or_insert_default().insert(
        "x-ryframe-tenant-context-headers".into(),
        tenant_context_header_contract(),
    );
    openapi
        .extensions
        .get_or_insert_default()
        .insert("x-ryframe-product-errors".into(), product_error_contract());
}

fn menu_route_contract() -> serde_json::Value {
    let routes = crate::permission_catalog::menu_routes()
        .iter()
        .map(|menu| {
            serde_json::json!({
                "route_key": menu.route_key,
                "name": menu.name,
                "title_key": menu.title_key,
                "menu_type": menu.menu_type,
                "page_key": menu.page_key,
                "permission_code": menu.permission_code,
                "capability_code": menu.capability_code,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "version": 2,
        "routes": routes,
    })
}

fn product_capability_contract() -> serde_json::Value {
    let capabilities = ryframe_application::system::platform::CAPABILITY_CATALOG
        .iter()
        .map(|descriptor| {
            serde_json::json!({
                "code": descriptor.code,
                "dependencies": descriptor.dependencies,
                "conflicts": descriptor.conflicts,
                "route_keys": descriptor.route_keys,
                "permission_codes": descriptor.permission_codes,
                "default_admin_permissions": descriptor.default_admin_permissions,
                "deployment_dependencies": descriptor.deployment_dependencies,
                "deployment_available": true,
                "client_config_fields": descriptor.client_config_fields,
                "variants": descriptor.variants.iter().map(|variant| serde_json::json!({
                    "code": variant.code,
                    "schema_version": variant.schema_version,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "version": 1, "capabilities": capabilities })
}

fn password_policy_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "min_length": ryframe_auth::password::MIN_PASSWORD_LENGTH,
        "max_length": ryframe_auth::password::MAX_PASSWORD_LENGTH,
        "pattern": ryframe_auth::password::COMPLEXITY_PATTERN,
        "allowed_characters": "ascii_graphic",
        "required_classes": ["uppercase", "lowercase", "digit", "special"],
    })
}

fn notice_policy_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "content_markdown": {
            "min_utf8_bytes": crate::generated::notice::dto::CONTENT_MARKDOWN_MIN_UTF8_BYTES,
            "max_utf8_bytes": crate::generated::notice::dto::CONTENT_MARKDOWN_MAX_UTF8_BYTES,
        },
    })
}

fn api_prefix_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "value": crate::http::API_PREFIX,
    })
}

fn permission_catalog_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "codes": crate::permission_catalog::permission_codes(),
    })
}

fn route_contract() -> serde_json::Value {
    let bindings = crate::permission_catalog::route_capability_bindings();
    let mut endpoint_keys = BTreeSet::new();
    for descriptor in ryframe_application::system::platform::CAPABILITY_CATALOG {
        for permission in descriptor.permission_codes {
            assert!(
                bindings
                    .iter()
                    .any(|binding| binding.capability_code == descriptor.code
                        && binding.permission_code == Some(*permission)),
                "capability permission {permission} has no compiled route binding"
            );
        }
    }
    let routes = bindings
        .iter()
        .map(|binding| {
            assert!(
                binding.path == crate::http::API_PREFIX
                    || binding
                        .path
                        .starts_with(&format!("{}/", crate::http::API_PREFIX)),
                "route contract path must include the public API prefix: {}",
                binding.path
            );
            assert!(
                endpoint_keys.insert((binding.method, binding.path)),
                "route contract has duplicate method/path binding: {} {}",
                binding.method,
                binding.path
            );
            let descriptor = ryframe_application::system::platform::CAPABILITY_CATALOG
                .iter()
                .find(|descriptor| descriptor.code == binding.capability_code)
                .expect("route capability must exist in the compiled catalog");
            if let Some(permission_code) = binding.permission_code {
                assert!(
                    descriptor.permission_codes.contains(&permission_code),
                    "route capability permission is outside its descriptor"
                );
            }
            serde_json::json!({
                "source": binding.source,
                "handler": binding.handler,
                "method": binding.method,
                "path": binding.path,
                "permission_code": binding.permission_code,
                "capability_code": binding.capability_code,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "version": 1, "routes": routes })
}

fn tenant_context_header_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "headers": [
            "X-Authorization-Epoch",
            "X-Tenant-Runtime-Epoch",
            "X-Tenant-Data-Generation",
            "X-Tenant-Data-State"
        ]
    })
}

fn product_error_contract() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "errors": [
            {"error_key": "capability_unavailable", "status": 501},
            {"error_key": "tenant_capability_denied", "status": 403},
            {"error_key": "permission_denied", "status": 403},
            {"error_key": "stale_runtime_epoch", "status": 409},
            {"error_key": "stale_placement_generation", "status": 409},
            {"error_key": "tenant_operation_conflict", "status": 409},
            {"error_key": "tenant_data_maintenance", "status": 423, "retry_after": true},
            {"error_key": "tenant_data_target_unavailable", "status": 503, "retry_after": true}
        ]
    })
}
