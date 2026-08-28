use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{ResourceError, ResourceExplanation, ResourceIr, StorageKind, ValueType};

mod aggregate;
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
    let mut ordered = resources.to_vec();
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
    validate_relation_targets(&ordered)?;
    let ordered = ordered.iter().collect::<Vec<_>>();
    let mut assets = Vec::new();
    for resource in &ordered {
        render_backend(resource, &ordered, &mut assets);
        render_frontend(resource, &mut assets);
    }
    aggregate::render(&ordered, &mut assets);
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

fn render_backend(
    resource: &ResourceIr,
    resources: &[&ResourceIr],
    assets: &mut Vec<GeneratedAsset>,
) {
    render_application(resource, assets);
    render_database(resource, resources, assets);
    render_api(resource, assets);
}

fn render_application(resource: &ResourceIr, assets: &mut Vec<GeneratedAsset>) {
    let name = &resource.name;
    push(
        assets,
        resource,
        AssetRoot::Backend,
        format!("crates/ryframe-application/src/generated/{name}/mod.rs"),
        format!(
            "{}#[cfg(feature = \"test-support\")]\npub mod fake;\npub mod model;\npub mod port;\npub mod service;\n\n#[cfg(feature = \"test-support\")]\npub use fake::*;\npub use model::*;\npub use port::*;\npub use service::*;\n",
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
}

fn render_database(
    resource: &ResourceIr,
    resources: &[&ResourceIr],
    assets: &mut Vec<GeneratedAsset>,
) {
    let name = &resource.name;
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
            "{}#[cfg(feature = \"repositories\")]\npub mod entity;\n#[cfg(feature = \"repositories\")]\nmod repository;\n{}\n#[cfg(feature = \"repositories\")]\npub use repository::port;\n",
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
        slice::database_repository(resource, resources, &rust_header(resource)),
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
}

fn render_api(resource: &ResourceIr, assets: &mut Vec<GeneratedAsset>) {
    let name = &resource.name;
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

fn validate_relation_targets(resources: &[ResourceIr]) -> Result<(), ResourceError> {
    for resource in resources {
        for relation in &resource.relations {
            let target = resources
                .iter()
                .find(|candidate| candidate.name == relation.target_resource)
                .ok_or_else(|| {
                    ResourceError::new(
                        format!(
                            "关系 `{}` 引用了未声明资源 `{}`",
                            relation.name, relation.target_resource
                        ),
                        "在 catalog/resources 中加入目标资源清单，或修正 target_resource",
                    )
                    .with_resource(&resource.name)
                    .with_file(&resource.source_path)
                })?;
            if target.storage != resource.storage {
                return Err(ResourceError::new(
                    format!(
                        "关系 `{}` 跨越了 {:?} 与 {:?} 存储",
                        relation.name, resource.storage, target.storage
                    ),
                    "生成关系只允许同一存储边界；跨存储读取放入手写查询切片",
                )
                .with_resource(&resource.name)
                .with_field(&relation.local_field)
                .with_file(&resource.source_path));
            }
            let target_id = target
                .fields
                .iter()
                .find(|field| field.name == "id")
                .expect("flat_crud 目标资源已经校验 id");
            if target_id.value_type != ValueType::I64 || target_id.nullable {
                return Err(ResourceError::new(
                    format!("关系 `{}` 的目标 id 不是非空 i64", relation.name),
                    "关系目标必须是已通过 flat_crud 校验的标准资源",
                )
                .with_resource(&resource.name)
                .with_field(&relation.local_field)
                .with_file(&resource.source_path));
            }
        }
    }
    Ok(())
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
        format!("src/generated/resources/{name}/registration.ts"),
        slice::frontend_registration(resource),
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
