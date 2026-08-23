use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{ResourceError, ResourceExplanation, ResourceIr, StorageKind};

mod catalog;
mod format;
mod slice;

const MAX_GENERATED_LINES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetRoot {
    Backend,
    Frontend,
}

impl AssetRoot {
    pub fn label(self) -> &'static str {
        match self {
            Self::Backend => "backend",
            Self::Frontend => "frontend",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedAsset {
    pub resource: String,
    pub root: AssetRoot,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct GeneratedCatalog {
    pub assets: Vec<GeneratedAsset>,
    pub explanations: BTreeMap<String, ResourceExplanation>,
    pub(crate) resources: BTreeMap<String, ResourceIr>,
}

impl GeneratedCatalog {
    pub fn explanation(&self, resource: &str) -> Option<&ResourceExplanation> {
        self.explanations.get(resource)
    }
}

/// 按资源名确定排序并生成所有受管资产。
pub fn render_resources(resources: &[ResourceIr]) -> Result<GeneratedCatalog, ResourceError> {
    let mut ordered = resources.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.name.cmp(&right.name));
    for pair in ordered.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(ResourceError::new(
                format!("资源 `{}` 重复", pair[0].name),
                "每个资源只保留一个 TOML 清单",
            )
            .with_resource(&pair[0].name));
        }
    }
    let mut assets = Vec::new();
    for resource in &ordered {
        render_backend(resource, &mut assets);
        render_frontend(resource, &mut assets);
    }
    render_aggregates(&ordered, &mut assets);
    format::rust_assets(&mut assets)?;
    validate_assets(&assets)?;
    assets.sort_by(|left, right| {
        left.root
            .cmp(&right.root)
            .then(left.path.cmp(&right.path))
            .then(left.resource.cmp(&right.resource))
    });
    let explanations = ordered
        .iter()
        .map(|resource| {
            (
                resource.name.clone(),
                ResourceExplanation::from_resource(resource),
            )
        })
        .collect();
    let resources = ordered
        .into_iter()
        .map(|resource| (resource.name.clone(), resource.clone()))
        .collect();
    Ok(GeneratedCatalog {
        assets,
        explanations,
        resources,
    })
}

fn render_backend(resource: &ResourceIr, assets: &mut Vec<GeneratedAsset>) {
    let name = &resource.name;
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-application/src/generated/{name}/mod.rs"),
        format!(
            "{}pub mod fake;\npub mod model;\npub mod port;\npub mod service;\n\npub use fake::*;\npub use model::*;\npub use port::*;\npub use service::*;\n",
            rust_header(resource)
        ),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-application/src/generated/{name}/model.rs"),
        slice::application_model(resource, &rust_header(resource)),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-application/src/generated/{name}/port.rs"),
        slice::application_port(resource, &rust_header(resource)),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-application/src/generated/{name}/service.rs"),
        slice::application_service(resource, &rust_header(resource)),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-application/src/generated/{name}/fake.rs"),
        slice::application_fake(resource, &rust_header(resource)),
    );

    let storage_crate = match resource.storage {
        StorageKind::ControlRow => "ryframe-db",
        StorageKind::TenantData => "ryframe-tenant-db",
    };
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/{storage_crate}/src/generated/{name}/mod.rs"),
        format!(
            "{}pub mod entity;\nmod repository;\n{}\npub use repository::port;\n",
            rust_header(resource),
            if resource.bootstrap_migration {
                "pub mod migration;\n"
            } else {
                ""
            }
        ),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/{storage_crate}/src/generated/{name}/entity.rs"),
        slice::database_entity(resource, &rust_header(resource)),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/{storage_crate}/src/generated/{name}/repository.rs"),
        slice::database_repository(resource, &rust_header(resource)),
    );
    if resource.bootstrap_migration {
        push(
            assets,
            resource,
            AssetRoot::Backend,
            format!("crates/{storage_crate}/src/generated/{name}/migration.rs"),
            slice::migration(resource, &rust_header(resource)),
        );
    }

    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-api/src/generated/{name}/mod.rs"),
        format!(
            "{}pub mod dto;\npub mod handler;\npub mod openapi;\n",
            rust_header(resource)
        ),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-api/src/generated/{name}/dto.rs"),
        slice::api_dto(resource, &rust_header(resource)),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-api/src/generated/{name}/handler.rs"),
        slice::api_handler(resource, &rust_header(resource)),
    );
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-api/src/generated/{name}/openapi.rs"),
        slice::api_openapi(resource, &rust_header(resource)),
    );
}

fn render_frontend(resource: &ResourceIr, assets: &mut Vec<GeneratedAsset>) {
    let name = &resource.name;
    push(
        assets,
        resource,
        AssetRoot::Frontend,
        format!("src/generated/resources/{name}/api.ts"),
        slice::frontend_api(resource),
    );
    push(
        assets,
        resource,
        AssetRoot::Frontend,
        format!("src/generated/resources/{name}/fields.ts"),
        slice::frontend_fields(resource),
    );
    push(
        assets,
        resource,
        AssetRoot::Frontend,
        format!("src/generated/resources/{name}/page.vue"),
        slice::frontend_page(resource),
    );
    push(
        assets,
        resource,
        AssetRoot::Frontend,
        format!("src/generated/resources/{name}/index.ts"),
        format!(
            "{}export * from './api'\nexport * from './fields'\nexport {{ default as {}Page }} from './page.vue'\n",
            slash_header(resource),
            resource.pascal_name
        ),
    );
}

fn render_aggregates(resources: &[&ResourceIr], assets: &mut Vec<GeneratedAsset>) {
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
        .map(|resource| format!("pub mod {};", resource.name))
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
        "use std::sync::Arc;\n\n"
    } else {
        ""
    };
    let empty_body = if selected.is_empty() {
        format!("    let _ = ({parameter_name}, ports);\n")
    } else {
        String::new()
    };
    format!(
        "{header}{arc_import}use ryframe_application::generated::GeneratedPersistencePorts;\nuse sea_orm_migration::MigrationTrait;\n\n{modules}\n\npub mod entities {{\n{entity_exports}\n}}\n\npub fn register_ports({parameter_name}: {parameter_type}, ports: &mut GeneratedPersistencePorts) {{\n{empty_body}{registrations}\n}}\n\npub fn migrations() -> Vec<Box<dyn MigrationTrait>> {{\n    vec![\n{migrations}\n    ]\n}}\n"
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

fn render_frontend_fields(resource: &ResourceIr) -> String {
    let pascal = &resource.pascal_name;
    let form_fields = resource
        .fields
        .iter()
        .filter(|field| field.usage.create || field.usage.update)
        .collect::<Vec<_>>();
    let mut output = slash_header(resource);
    output.push_str("import {\n  defineFlatCrudResource,\n  type FlatCrudColumn,\n  type FlatCrudFormField,\n  type FlatCrudLabels,\n  type FlatCrudPermissions,\n  type FlatCrudQueryField,\n} from '@/components/business/flat-crud'\n");
    output.push_str(&format!(
        "import {{\n  create{pascal},\n  delete{pascal},\n  get{pascal},\n  list{pascal},\n  update{pascal},\n  type {pascal}CreateInput,\n  type {pascal}Query,\n  type {pascal}Record,\n  type {pascal}UpdateInput,\n}} from './api'\nimport {{ emptyPageResponse }} from '@/shared/http/types'\n\n"
    ));
    output.push_str(&format!("export interface {pascal}Form {{\n"));
    for field in &form_fields {
        output.push_str(&format!("  {}: {}\n", field.name, field.typescript_type));
    }
    output.push_str("}\n\n");
    output.push_str(&format!(
        "export interface {pascal}Presentation {{\n  columns: readonly FlatCrudColumn<{pascal}Record>[]\n  formFields: readonly FlatCrudFormField<{pascal}Form>[]\n  labels: FlatCrudLabels\n  permissions: FlatCrudPermissions\n  queryFields: readonly FlatCrudQueryField<{pascal}Query>[]\n  resource: ReturnType<typeof {name}Resource>\n}}\n\ntype Translate = (zhCN: string, en: string) => string\n\nexport function create{pascal}Presentation(\n  translate: Translate,\n  formatDate: (value: string) => string,\n): {pascal}Presentation {{\n",
        name = resource.name
    ));
    for field in resource
        .fields
        .iter()
        .filter(|field| !field.enum_values.is_empty())
    {
        output.push_str(&format!("  const {}Options = [\n", field.name));
        for (value, labels) in &field.enum_values {
            push_typescript_object(
                &mut output,
                &[
                    format!("label: translate({:?}, {:?})", labels.zh_cn, labels.en),
                    format!("value: {}", ts_enum_value(value, field.value_type)),
                ],
            );
        }
        output.push_str("  ] as const\n\n");
    }
    output.push_str(&format!(
        "  const permissions = {{\n    list: {:?},\n    create: {:?},\n    update: {:?},\n    remove: {:?},\n  }} as const\n\n",
        resource.access.permissions.list,
        resource.access.permissions.create,
        resource.access.permissions.update,
        resource.access.permissions.delete,
    ));
    output.push_str(&format!(
        "  const labels = {{\n    title: translate({:?}, {:?}),\n    add: translate(\"新增\", \"Add\"),\n    edit: translate(\"编辑\", \"Edit\"),\n    remove: translate(\"删除\", \"Delete\"),\n    actions: translate(\"操作\", \"Actions\"),\n    search: translate(\"搜索\", \"Search\"),\n    reset: translate(\"重置\", \"Reset\"),\n    confirm: translate(\"确定\", \"Confirm\"),\n    cancel: translate(\"取消\", \"Cancel\"),\n  }}\n\n",
        resource.labels.zh_cn, resource.labels.en
    ));
    output.push_str("  const queryFields = [\n");
    for field in resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && !matches!(field.widget, super::WidgetIr::Hidden))
    {
        let kind = if field.enum_values.is_empty() {
            "text"
        } else {
            "select"
        };
        let mut properties = vec![
            format!("key: {:?}", field.name),
            format!("kind: {kind:?}"),
            format!(
                "label: translate({:?}, {:?})",
                field.labels.zh_cn, field.labels.en
            ),
            format!(
                "placeholder: translate({:?}, {:?})",
                format!("请输入或选择{}", field.labels.zh_cn),
                format!("Enter or select {}", field.labels.en),
            ),
        ];
        if !field.enum_values.is_empty() {
            properties.push(format!("options: {}Options", field.name));
        }
        push_typescript_object(&mut output, &properties);
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudQueryField<{pascal}Query>[]\n\n"
    ));
    output.push_str("  const columns = [\n");
    for field in resource.fields.iter().filter(|field| field.usage.list) {
        let mut properties = vec![
            format!("key: {:?}", field.name),
            format!(
                "label: translate({:?}, {:?})",
                field.labels.zh_cn, field.labels.en
            ),
        ];
        if !field.enum_values.is_empty() {
            let positive = field.enum_values.keys().last().expect("枚举非空");
            properties.push("display: 'status'".into());
            properties.push(format!("options: {}Options", field.name));
            properties.push(format!(
                "positiveValue: {}",
                ts_enum_value(positive, field.value_type)
            ));
        } else if matches!(field.value_type, super::ValueType::DateTime) {
            properties.push("display: 'datetime'".into());
            properties.push("format: formatDate".into());
        }
        push_typescript_object(&mut output, &properties);
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudColumn<{pascal}Record>[]\n\n"
    ));
    output.push_str("  const formFields = [\n");
    for field in &form_fields {
        let mut properties = vec![
            format!("key: {:?}", field.name),
            format!("kind: {:?}", form_widget(field)),
            format!(
                "label: translate({:?}, {:?})",
                field.labels.zh_cn, field.labels.en
            ),
        ];
        if matches!(
            field.widget,
            super::WidgetIr::Text | super::WidgetIr::Textarea
        ) {
            properties.push(format!(
                "placeholder: translate({:?}, {:?})",
                format!("请输入{}", field.labels.zh_cn),
                format!("Enter {}", field.labels.en)
            ));
            if field.validation.required {
                properties.push(format!(
                    "requiredMessage: translate({:?}, {:?})",
                    format!("请输入{}", field.labels.zh_cn),
                    format!("Enter {}", field.labels.en)
                ));
            }
        }
        if matches!(field.widget, super::WidgetIr::Number) {
            if let Some(minimum) = field.validation.minimum {
                properties.push(format!("min: {minimum}"));
            }
            if let Some(maximum) = field.validation.maximum {
                properties.push(format!("max: {maximum}"));
            }
        }
        if !field.enum_values.is_empty() {
            properties.push(format!("options: {}Options", field.name));
        }
        if field.usage.create && !field.usage.update {
            properties.push("disabledOnEdit: true".into());
        }
        if field.usage.update && !field.usage.create {
            properties.push("editOnly: true".into());
        }
        push_typescript_object(&mut output, &properties);
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudFormField<{pascal}Form>[]\n\n  return {{\n    columns,\n    formFields,\n    labels,\n    permissions,\n    queryFields,\n    resource: {name}Resource(translate),\n  }}\n}}\n\n",
        name = resource.name
    ));

    let query_defaults = resource
        .fields
        .iter()
        .filter(|field| {
            field.usage.filter
                && !matches!(field.widget, super::WidgetIr::Hidden)
                && matches!(field.value_type, super::ValueType::String)
        })
        .map(|field| format!("      {}: '',\n", field.name))
        .collect::<String>();
    let empty_form = form_fields
        .iter()
        .map(|field| format!("      {}: {},\n", field.name, ts_default(field)))
        .collect::<String>();
    let edit_form = form_fields
        .iter()
        .map(|field| format!("      {}: record.{},\n", field.name, field.name))
        .collect::<String>();
    let create_input = resource
        .fields
        .iter()
        .filter(|field| field.usage.create)
        .map(|field| format!("      {}: form.{},\n", field.name, field.name))
        .collect::<String>();
    let update_input = resource
        .fields
        .iter()
        .filter(|field| field.usage.update)
        .map(|field| format!("      {}: form.{},\n", field.name, field.name))
        .collect::<String>();
    let record_id = resource
        .primary_key
        .iter()
        .rev()
        .find(|field| Some(field.as_str()) != resource.tenant_field.as_deref())
        .map(String::as_str)
        .unwrap_or("id");
    let record_label = resource
        .fields
        .iter()
        .find(|field| {
            field.name == "name"
                && field.value_type == super::ValueType::String
                && (field.usage.read || field.usage.list)
        })
        .or_else(|| {
            resource.fields.iter().find(|field| {
                field.value_type == super::ValueType::String
                    && (field.usage.list || field.usage.read)
            })
        })
        .map(|field| field.name.as_str())
        .unwrap_or(record_id);
    output.push_str(&format!(
        r#"function {name}Resource(translate: Translate) {{
  return defineFlatCrudResource<
    {pascal}Record,
    {pascal}Query,
    {pascal}Form,
    {pascal}CreateInput,
    {pascal}UpdateInput
  >({{
    key: {name:?},
    initialQuery: (): {pascal}Query => ({{
      page: 1,
      page_size: 10,
{query_defaults}    }}),
    emptyForm: (): {pascal}Form => ({{
{empty_form}    }}),
    editForm: record => ({{
{edit_form}    }}),
    createInput: form => ({{
{create_input}    }}),
    updateInput: form => ({{
{update_input}    }}),
    recordId: record => String(record.{record_id}),
    messages: {{
      addSuccess: translate("新增成功", "Created"),
      addTitle: translate("新增{zh}", "Add {en}"),
      deleteConfirm: record => translate(
        `确定删除 ${{String(record.{record_label})}} 吗？`,
        `Delete ${{String(record.{record_label})}}?`,
      ),
      deleteSuccess: translate("删除成功", "Deleted"),
      detailMissing: translate("资源不存在", "Resource not found"),
      editTitle: translate("编辑{zh}", "Edit {en}"),
      updateSuccess: translate("更新成功", "Updated"),
      warningTitle: translate("提示", "Warning"),
    }},
    adapter: {{
      async list(query, signal) {{
        const response = await list{pascal}({{ ...query }}, signal)
        return response.data ?? emptyPageResponse<{pascal}Record>(query)
      }},
      async detail(id, signal) {{
        const response = await get{pascal}(id, signal)
        if (!response.data) throw new Error(translate("资源不存在", "Resource not found"))
        return response.data
      }},
      async create(input) {{ await create{pascal}(input) }},
      async update(id, input) {{ await update{pascal}(id, input) }},
      async remove(id) {{ await delete{pascal}(id) }},
    }},
  }})
}}
"#,
        name = resource.name,
        zh = resource.labels.zh_cn,
        en = resource.labels.en,
    ));
    output
}

fn render_frontend_page(resource: &ResourceIr) -> String {
    format!(
        r#"{}<template>
  <FlatCrudPage
    v-model:dialog-visible="dialogVisible"
    v-model:form="form"
    v-model:page="page"
    v-model:page-size="pageSize"
    :columns="presentation.columns"
    :deleting-key="deletingKey"
    :dialog-title="dialogTitle"
    :editing="editing"
    :form-fields="presentation.formFields"
    :labels="presentation.labels"
    :loading="listQuery.isFetching.value"
    :permissions="presentation.permissions"
    :query="query"
    :query-fields="presentation.queryFields"
    :row-key="presentation.resource.recordId"
    :rows="listQuery.data.value?.items ?? []"
    :saving="saving"
    :total="listQuery.data.value?.total ?? 0"
    @add="add"
    @edit="edit"
    @page-change="changePage"
    @remove="remove"
    @reset="reset"
    @search="search"
    @submit="submit"
    @update:query="setQuery"
  >
    <template v-if="slots.actions" #actions>
      <slot
        name="actions"
        :can-export="canExport"
        :last-successful-query="lastSuccessfulQuery ?? null"
      />
    </template>
  </FlatCrudPage>
</template>

<script setup lang="ts">
import {{ computed }} from 'vue'
import {{ useI18n }} from 'vue-i18n'

import {{ FlatCrudPage, useFlatCrudResource }} from '@/components/business/flat-crud'
import {{ formatLocalizedDate }} from '@/i18n'
import type {{ {pascal}Query }} from './api'
import {{ create{pascal}Presentation }} from './fields'

const slots = defineSlots<{{
  actions?(props: {{ canExport: boolean; lastSuccessfulQuery: {pascal}Query | null }}): unknown
}}>()

const {{ locale }} = useI18n()
const translate = (zhCN: string, en: string) => locale.value.startsWith('zh') ? zhCN : en
const presentation = computed(() => create{pascal}Presentation(translate, formatLocalizedDate))
const {{
  add,
  canExport,
  changePage,
  deletingKey,
  dialogTitle,
  dialogVisible,
  edit,
  editing,
  form,
  lastSuccessfulQuery,
  listQuery,
  page,
  pageSize,
  query,
  remove,
  reset,
  search,
  setQuery,
  submit,
  saving,
}} = useFlatCrudResource(presentation.value.resource)
</script>
"#,
        html_header(resource),
        pascal = resource.pascal_name,
    )
}

fn push_typescript_object(output: &mut String, properties: &[String]) {
    output.push_str("    {\n");
    for property in properties {
        output.push_str("      ");
        output.push_str(property);
        output.push_str(",\n");
    }
    output.push_str("    },\n");
}

fn ts_default(field: &super::FieldIr) -> &'static str {
    match field.value_type {
        super::ValueType::String | super::ValueType::Date | super::ValueType::DateTime => "''",
        super::ValueType::I32 | super::ValueType::I64 | super::ValueType::Decimal => "0",
        super::ValueType::Bool => "false",
        super::ValueType::Json => "null",
    }
}

fn ts_enum_value(value: &str, value_type: super::ValueType) -> String {
    match value_type {
        super::ValueType::I32 | super::ValueType::I64 | super::ValueType::Decimal
            if value.parse::<f64>().is_ok() =>
        {
            value.to_owned()
        }
        super::ValueType::Bool if matches!(value, "true" | "false") => value.to_owned(),
        _ => format!("{value:?}"),
    }
}

fn form_widget(field: &super::FieldIr) -> &'static str {
    if !field.enum_values.is_empty() {
        "radio"
    } else {
        match field.widget {
            super::WidgetIr::Number => "number",
            _ => "text",
        }
    }
}

fn push(
    assets: &mut Vec<GeneratedAsset>,
    resource: &ResourceIr,
    root: AssetRoot,
    path: String,
    content: String,
) {
    assets.push(GeneratedAsset {
        resource: resource.name.clone(),
        root,
        path,
        content,
    });
}

fn validate_assets(assets: &[GeneratedAsset]) -> Result<(), ResourceError> {
    let mut paths = BTreeSet::new();
    for asset in assets {
        let key = (asset.root, asset.path.as_str());
        if !paths.insert(key) {
            return Err(ResourceError::new(
                format!("多个生成资产使用相同路径 `{}`", asset.path),
                "检查资源命名，保证路径唯一",
            )
            .with_resource(&asset.resource)
            .with_file(&asset.path));
        }
        if asset.content.lines().count() > MAX_GENERATED_LINES {
            return Err(ResourceError::new(
                format!("生成文件超过 {MAX_GENERATED_LINES} 行"),
                "按职责继续拆分生成文件，不要扩大单文件模板",
            )
            .with_resource(&asset.resource)
            .with_file(&asset.path));
        }
        if !asset.content.contains("@generated by RyFrame") {
            return Err(ResourceError::new(
                "生成文件缺少版本头",
                "使用统一 header renderer 生成文件头",
            )
            .with_resource(&asset.resource)
            .with_file(&asset.path));
        }
    }
    Ok(())
}

fn rust_header(resource: &ResourceIr) -> String {
    format!(
        "// @generated by RyFrame {}\n// resource: {}\n// source: {}\n// source-sha256: {}\n// 请勿手工修改；修改资源清单后重新生成。\n\n",
        crate::GENERATOR_VERSION,
        resource.name,
        resource.source_path,
        resource.source_hash,
    )
}

fn slash_header(resource: &ResourceIr) -> String {
    rust_header(resource)
}

fn html_header(resource: &ResourceIr) -> String {
    format!(
        "<!-- @generated by RyFrame {} | resource: {} | source: {} | source-sha256: {} -->\n<!-- 请勿手工修改；修改资源清单后重新生成。 -->\n",
        crate::GENERATOR_VERSION,
        resource.name,
        resource.source_path,
        resource.source_hash,
    )
}

fn aggregate_header(source: &str) -> String {
    format!(
        "// @generated by RyFrame {}\n// sources: {}\n// 请勿手工修改；修改资源清单后重新生成。\n\n",
        crate::GENERATOR_VERSION,
        source,
    )
}
