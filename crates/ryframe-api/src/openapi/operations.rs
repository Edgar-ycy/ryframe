pub(super) fn finalize_operations(openapi: &mut utoipa::openapi::OpenApi) {
    for (path, item) in &mut openapi.paths.paths {
        finalize_operation(&mut item.get, "get", path);
        finalize_operation(&mut item.post, "post", path);
        finalize_operation(&mut item.put, "put", path);
        finalize_operation(&mut item.delete, "delete", path);
        finalize_operation(&mut item.patch, "patch", path);
        finalize_operation(&mut item.options, "options", path);
        finalize_operation(&mut item.head, "head", path);
        finalize_operation(&mut item.trace, "trace", path);
    }
}

fn finalize_operation(
    operation: &mut Option<utoipa::openapi::path::Operation>,
    method: &str,
    path: &str,
) {
    set_operation_id(operation, method, path);
    let Some(operation) = operation else {
        return;
    };
    // 生成器的 tuple 参数语法无法声明 Header required；运行时 handler 已强制
    // 提取该值，这里把生成契约同步为必填，避免客户端生成可空调用签名。
    if method == "post"
        && path == "/api/v1/platform/tenants"
        && let Some(parameter) = operation.parameters.as_mut().and_then(|parameters| {
            parameters.iter_mut().find_map(|parameter| match parameter {
                utoipa::openapi::RefOr::T(parameter)
                    if parameter.name.eq_ignore_ascii_case("Idempotency-Key") =>
                {
                    Some(parameter)
                }
                _ => None,
            })
        })
    {
        parameter.required = utoipa::openapi::Required::True;
    }
    let bearer =
        utoipa::openapi::security::SecurityRequirement::new("bearer", std::iter::empty::<String>());
    if !operation
        .security
        .as_ref()
        .is_some_and(|requirements| requirements.contains(&bearer))
    {
        return;
    }
    for response in operation.responses.responses.values_mut() {
        let utoipa::openapi::RefOr::T(response) = response else {
            continue;
        };
        for (name, description) in [
            ("X-Authorization-Epoch", "本次响应所依据的租户授权纪元"),
            ("X-Tenant-Runtime-Epoch", "本次响应所依据的租户产品运行纪元"),
            (
                "X-Tenant-Data-Generation",
                "本次响应所依据的租户数据放置代次",
            ),
            ("X-Tenant-Data-State", "本次响应所依据的租户业务数据状态"),
        ] {
            let mut header = utoipa::openapi::header::Header::default();
            header.description = Some(description.to_owned());
            response
                .headers
                .entry(name.to_owned())
                .or_insert(utoipa::openapi::RefOr::T(header));
        }
    }
}

fn set_operation_id(
    operation: &mut Option<utoipa::openapi::path::Operation>,
    method: &str,
    path: &str,
) {
    let Some(operation) = operation else {
        return;
    };

    let normalized_path = path
        .strip_prefix(crate::http::API_PREFIX)
        .unwrap_or(path)
        .trim_start_matches('/')
        .split('/')
        .map(|segment| {
            segment
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
                .map_or_else(
                    || segment.replace('-', "_"),
                    |parameter| format!("by_{parameter}"),
                )
        })
        .collect::<Vec<_>>()
        .join("_");

    operation.operation_id = Some(format!("{method}_{normalized_path}"));
}
