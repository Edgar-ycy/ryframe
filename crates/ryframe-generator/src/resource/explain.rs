use serde::Serialize;

use super::{AssetRoot, ResourceIr, StorageKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExplainNode {
    pub order: u8,
    pub layer: String,
    pub target: AssetRoot,
    pub file: String,
    pub symbol: String,
    pub responsibility: String,
}

/// 资源从前端页面到数据表的静态调用链。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceExplanation {
    pub resource: String,
    pub source: String,
    pub profile: String,
    pub storage: String,
    pub api_path: String,
    pub route_path: String,
    pub capability: Option<String>,
    pub permissions: Vec<String>,
    pub nodes: Vec<ExplainNode>,
    pub extension_notes: Vec<String>,
}

impl ResourceExplanation {
    pub fn from_resource(resource: &ResourceIr) -> Self {
        let name = &resource.name;
        let pascal = &resource.pascal_name;
        let storage_crate = match resource.storage {
            StorageKind::ControlRow => "ryframe-db",
            StorageKind::TenantData => "ryframe-tenant-db",
        };
        let nodes = vec![
            ExplainNode {
                order: 1,
                layer: "页面".into(),
                target: AssetRoot::Frontend,
                file: format!("src/generated/resources/{name}/page.vue"),
                symbol: "FlatCrudPage".into(),
                responsibility: "组合 FlatCrudPage 与强类型字段描述".into(),
            },
            ExplainNode {
                order: 2,
                layer: "API adapter".into(),
                target: AssetRoot::Frontend,
                file: format!("src/generated/resources/{name}/api.ts"),
                symbol: format!("list{pascal} / create{pascal} / update{pascal}"),
                responsibility: "提供资源专用、可追踪的 HTTP 调用".into(),
            },
            ExplainNode {
                order: 3,
                layer: "Handler".into(),
                target: AssetRoot::Backend,
                file: format!("crates/ryframe-api/src/generated/{name}/handler.rs"),
                symbol: "router".into(),
                responsibility: "解析 HTTP 输入并调用具体 Service".into(),
            },
            ExplainNode {
                order: 4,
                layer: "Service".into(),
                target: AssetRoot::Backend,
                file: format!("crates/ryframe-application/src/generated/{name}/service.rs"),
                symbol: format!("{pascal}Service"),
                responsibility: "承载业务规则与显式事务边界".into(),
            },
            ExplainNode {
                order: 5,
                layer: "Port".into(),
                target: AssetRoot::Backend,
                file: format!("crates/ryframe-application/src/generated/{name}/port.rs"),
                symbol: format!("{pascal}PersistencePort"),
                responsibility: "定义异步持久化能力，不暴露 SQL".into(),
            },
            ExplainNode {
                order: 6,
                layer: "Repository".into(),
                target: AssetRoot::Backend,
                file: format!("crates/{storage_crate}/src/generated/{name}/repository.rs"),
                symbol: "port".into(),
                responsibility: "实现 SQL 与存储映射".into(),
            },
            ExplainNode {
                order: 7,
                layer: "Table".into(),
                target: AssetRoot::Backend,
                file: format!("crates/{storage_crate}/src/generated/{name}/entity.rs"),
                symbol: resource.table.clone(),
                responsibility: "追加式迁移管理的物理数据表".into(),
            },
        ];
        let mut extension_notes = Vec::new();
        if !resource.backend_extensions.is_empty() {
            extension_notes.push(format!(
                "后端扩展键：{}；复杂行为由强类型扩展实现",
                resource
                    .backend_extensions
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !resource.frontend_extensions.is_empty() {
            extension_notes.push(format!(
                "前端扩展键：{}；页面特殊行为不进入通用 CRUD 内核",
                resource
                    .frontend_extensions
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !resource.relations.is_empty() {
            extension_notes.push(format!(
                "详情关系：{}；由查询端口在同一存储边界内装配",
                resource
                    .relations
                    .iter()
                    .map(|relation| format!(
                        "{}({} → {})",
                        relation.name, relation.local_field, relation.target_resource
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if extension_notes.is_empty() {
            extension_notes.push("该资源当前不需要手写扩展".into());
        }
        let mut permissions = Vec::new();
        for permission in resource.access.permissions.values() {
            if !permissions.iter().any(|existing| existing == permission) {
                permissions.push(permission.to_owned());
            }
        }
        Self {
            resource: resource.name.clone(),
            source: resource.source_path.clone(),
            profile: "flat_crud".into(),
            storage: match resource.storage {
                StorageKind::ControlRow => "control_row",
                StorageKind::TenantData => "tenant_data",
            }
            .into(),
            api_path: resource.api.path.clone(),
            route_path: resource.route.path.clone(),
            capability: resource.access.capability.clone(),
            permissions,
            nodes,
            extension_notes,
        }
    }

    pub fn render_text(&self) -> String {
        let mut output = format!(
            "资源：{}\n清单：{}\n画像：{}\n存储：{}\nAPI：{}\n路由：{}\n能力：{}\n权限：{}\n\n调用链：\n",
            self.resource,
            self.source,
            self.profile,
            self.storage,
            self.api_path,
            self.route_path,
            self.capability
                .as_deref()
                .unwrap_or("基础资源（不受套餐门禁）"),
            self.permissions.join(", "),
        );
        for node in &self.nodes {
            output.push_str(&format!(
                "  {}. {} → {} ({})\n     {}\n",
                node.order, node.layer, node.symbol, node.file, node.responsibility
            ));
        }
        output.push_str("\n扩展：\n");
        for note in &self.extension_notes {
            output.push_str(&format!("  - {note}\n"));
        }
        output
    }
}
