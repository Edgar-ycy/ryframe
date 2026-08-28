use super::super::{ResourceIr, StorageKind};
use super::{AssetRoot, GeneratedAsset, aggregate_header, catalog};

pub(super) fn render(resources: &[&ResourceIr], assets: &mut Vec<GeneratedAsset>) {
    let source = resources
        .iter()
        .map(|resource| format!("{}:{}", resource.source_path, resource.source_hash))
        .collect::<Vec<_>>()
        .join(",");
    let aggregate = aggregate_header(&source);
    assets.push(GeneratedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Backend,
        path: "crates/ryframe-application/src/generated/mod.rs".into(),
        content: render_application_mod(resources, &aggregate),
    });
    assets.push(GeneratedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Backend,
        path: "crates/ryframe-application/src/generated/services.rs".into(),
        content: render_generated_services(resources, &aggregate),
    });
    for (crate_name, storage) in [
        ("ryframe-db", StorageKind::ControlRow),
        ("ryframe-tenant-db", StorageKind::TenantData),
    ] {
        assets.push(GeneratedAsset {
            resource: "__catalog__".into(),
            root: AssetRoot::Backend,
            path: format!("crates/{crate_name}/src/generated/mod.rs"),
            content: render_storage_mod(resources, storage, &aggregate),
        });
    }
    assets.push(GeneratedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Backend,
        path: "crates/ryframe-api/src/generated/mod.rs".into(),
        content: render_api_mod(resources, &aggregate),
    });
    assets.push(GeneratedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Backend,
        path: "crates/ryframe-api/src/generated/router.rs".into(),
        content: render_generated_router(resources, &aggregate),
    });
    assets.push(GeneratedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Backend,
        path: "crates/ryframe-api/src/generated/openapi.rs".into(),
        content: render_generated_openapi(resources, &aggregate),
    });
    let frontend_exports = resources
        .iter()
        .map(|resource| format!("export * from './{}'", resource.name))
        .collect::<Vec<_>>()
        .join("\n");
    assets.push(GeneratedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Frontend,
        path: "src/generated/resources/index.ts".into(),
        content: format!("{aggregate}{frontend_exports}\n"),
    });
    let crud_resources = resources
        .iter()
        .map(|resource| {
            format!(
                "        super::{}::openapi::crud_resource_metadata(),",
                resource.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    assets.push(GeneratedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Backend,
        path: "crates/ryframe-api/src/generated/crud_resources.rs".into(),
        content: format!(
            "{aggregate}pub const CRUD_RESOURCES_VERSION: u16 = 1;\n\n/// 返回 OpenAPI `x-ryframe-crud-resources` 的静态安全元数据。\npub fn crud_resources_extension() -> serde_json::Value {{\n    serde_json::json!({{\n        \"version\": CRUD_RESOURCES_VERSION,\n        \"resources\": [\n{crud_resources}\n        ]\n    }})\n}}\n"
        ),
    });
    assets.push(GeneratedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Backend,
        path: "catalog/access.generated.toml".into(),
        content: catalog::access_catalog(resources, &source),
    });
}

fn render_application_mod(resources: &[&ResourceIr], header: &str) -> String {
    let modules = resources
        .iter()
        .map(|resource| format!("pub mod {};", resource.name))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{header}pub mod services;\n\n{modules}\n\npub use services::{{GeneratedPersistencePorts, GeneratedServices}};\n"
    )
}

fn render_generated_services(resources: &[&ResourceIr], header: &str) -> String {
    if resources.is_empty() {
        return format!(
            "{header}use ryframe_kernel::AppResult;\n\n#[derive(Default)]\npub struct GeneratedPersistencePorts {{}}\n\n#[derive(Clone, Default)]\npub struct GeneratedServices {{}}\n\nimpl GeneratedServices {{\n    pub fn try_new(ports: GeneratedPersistencePorts) -> AppResult<Self> {{\n        let _ = ports;\n        Ok(Self {{}})\n    }}\n}}\n"
        );
    }
    let imports = resources
        .iter()
        .map(|resource| {
            format!(
                "use super::{}::{{{}PersistencePort, {}Service}};",
                resource.name, resource.pascal_name, resource.pascal_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let ports = resources
        .iter()
        .map(|resource| {
            format!(
                "    pub {}: Option<Arc<dyn {}PersistencePort>>,",
                resource.name, resource.pascal_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let services = resources
        .iter()
        .map(|resource| {
            format!(
                "    pub {}: Arc<{}Service>,",
                resource.name, resource.pascal_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let builds = resources
        .iter()
        .map(|resource| {
            format!(
                "            {name}: Arc::new({pascal}Service::new(ports.{name}.take().ok_or_else(|| AppError::Config(\"生成资源 {name} 缺少持久化端口\".into()))?)),",
                name = resource.name,
                pascal = resource.pascal_name,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{header}use std::sync::Arc;\n\nuse ryframe_kernel::{{AppError, AppResult}};\n\n{imports}\n\n#[derive(Default)]\npub struct GeneratedPersistencePorts {{\n{ports}\n}}\n\n#[derive(Clone)]\npub struct GeneratedServices {{\n{services}\n}}\n\nimpl GeneratedServices {{\n    pub fn try_new(mut ports: GeneratedPersistencePorts) -> AppResult<Self> {{\n        Ok(Self {{\n{builds}\n        }})\n    }}\n}}\n"
    )
}

fn render_storage_mod(resources: &[&ResourceIr], storage: StorageKind, header: &str) -> String {
    let selected = resources
        .iter()
        .copied()
        .filter(|resource| resource.storage == storage)
        .collect::<Vec<_>>();
    let modules = selected
        .iter()
        .map(|resource| {
            let feature_gate = if resource.bootstrap_migration {
                ""
            } else {
                "#[cfg(feature = \"repositories\")]\n"
            };
            format!("{feature_gate}pub mod {};", resource.name)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let entity_exports = selected
        .iter()
        .map(|resource| {
            format!(
                "    pub use super::{name}::entity as {name};",
                name = resource.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (parameter_type, parameter_name) = match storage {
        StorageKind::ControlRow => ("crate::ControlDatabaseCluster", "database"),
        StorageKind::TenantData => ("Arc<crate::TenantDatabaseRouter>", "router"),
    };
    let registrations = selected
        .iter()
        .enumerate()
        .map(|(index, resource)| {
            let argument = if index + 1 == selected.len() {
                parameter_name.to_owned()
            } else {
                format!("{parameter_name}.clone()")
            };
            format!(
                "    ports.{name} = Some({name}::port({argument}));",
                name = resource.name,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let migrations = selected
        .iter()
        .filter(|resource| resource.bootstrap_migration)
        .map(|resource| format!("        Box::new({}::migration::Migration),", resource.name))
        .collect::<Vec<_>>()
        .join("\n");
    let arc_import = if storage == StorageKind::TenantData {
        "#[cfg(feature = \"repositories\")]\nuse std::sync::Arc;\n\n"
    } else {
        ""
    };
    let empty_body = if selected.is_empty() {
        format!("    let _ = ({parameter_name}, ports);\n")
    } else {
        String::new()
    };
    format!(
        "{header}{arc_import}#[cfg(feature = \"repositories\")]\nuse ryframe_application::generated::GeneratedPersistencePorts;\nuse sea_orm_migration::MigrationTrait;\n\n{modules}\n\n#[cfg(feature = \"repositories\")]\npub mod entities {{\n{entity_exports}\n}}\n\n#[cfg(feature = \"repositories\")]\npub fn register_ports({parameter_name}: {parameter_type}, ports: &mut GeneratedPersistencePorts) {{\n{empty_body}{registrations}\n}}\n\npub fn migrations() -> Vec<Box<dyn MigrationTrait>> {{\n    vec![\n{migrations}\n    ]\n}}\n"
    )
}

fn render_api_mod(resources: &[&ResourceIr], header: &str) -> String {
    let modules = resources
        .iter()
        .map(|resource| format!("pub mod {};", resource.name))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{header}pub mod crud_resources;\npub mod openapi;\npub mod router;\n\n{modules}\n\npub use crud_resources::crud_resources_extension;\npub use openapi::GeneratedOpenApi;\npub use router::generated_router;\n"
    )
}

fn render_generated_router(resources: &[&ResourceIr], header: &str) -> String {
    let nests = resources
        .iter()
        .map(|resource| {
            let path = resource.api.path.trim_start_matches("/api/v1/system");
            format!(
                "        .nest({path:?}, super::{name}::handler::router(Arc::clone(&services.{name}), pagination))",
                name = resource.name,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let arc_import = if resources.is_empty() {
        ""
    } else {
        "use std::sync::Arc;\n\n"
    };
    let empty = if resources.is_empty() {
        "    let _ = (services, pagination);\n"
    } else {
        ""
    };
    format!(
        "{header}{arc_import}use axum::Router;\nuse ryframe_application::generated::GeneratedServices;\nuse ryframe_kernel::PaginationPolicy;\n\npub fn generated_router(services: &GeneratedServices, pagination: PaginationPolicy) -> Router {{\n{empty}    Router::new()\n{nests}\n}}\n"
    )
}

fn render_generated_openapi(resources: &[&ResourceIr], header: &str) -> String {
    let Some(first) = resources.first() else {
        return format!(
            "{header}pub struct GeneratedOpenApi;\n\nimpl utoipa::OpenApi for GeneratedOpenApi {{\n    fn openapi() -> utoipa::openapi::OpenApi {{\n        let mut document = utoipa::openapi::OpenApi::default();\n        document.extensions.get_or_insert_default().insert(\n            \"x-ryframe-crud-resources\".into(),\n            super::crud_resources::crud_resources_extension(),\n        );\n        document\n    }}\n}}\n"
        );
    };
    let merges = resources
        .iter()
        .skip(1)
        .map(|resource| {
            format!(
                "        document.merge(<super::{name}::openapi::{pascal}OpenApi as utoipa::OpenApi>::openapi());",
                name = resource.name,
                pascal = resource.pascal_name,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{header}pub struct GeneratedOpenApi;\n\nimpl utoipa::OpenApi for GeneratedOpenApi {{\n    fn openapi() -> utoipa::openapi::OpenApi {{\n        let mut document = <super::{first_name}::openapi::{first_pascal}OpenApi as utoipa::OpenApi>::openapi();\n{merges}\n        document.extensions.get_or_insert_default().insert(\n            \"x-ryframe-crud-resources\".into(),\n            super::crud_resources::crud_resources_extension(),\n        );\n        document\n    }}\n}}\n",
        first_name = first.name,
        first_pascal = first.pascal_name,
    )
}
