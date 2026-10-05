//! 基于资源清单的离线生成核心。
//!
//! 资源清单只描述稳定、重复的平面 CRUD 信息。复杂 SQL、页面布局、状态机和跨资源
//! 事务必须留在普通手写代码中，避免把业务流程藏进 DSL。

mod error;
mod explain;
mod ir;
mod render;
mod spec;
mod writer;

pub use error::ResourceError;
pub use explain::{ExplainNode, ResourceExplanation};
pub use ir::{
    FieldIr, IndexIr, PermissionIr, RelationIr, RelationKind, ResourceIr, ResourceProfile,
    StorageKind, ValueType, WidgetIr,
};
pub(crate) use render::business::render_business_resource;
pub use render::{AssetRoot, GeneratedAsset, GeneratedCatalog, render_resources};
pub use spec::{
    AccessSpec, ApiSpec, AuditSpec, DatabaseSpec, EnumValueSpec, ExtensionSpec, FieldSpec,
    FieldUsageSpec, IndexSpec, LabelsSpec, MenuSpec, OperationSpec, PermissionSpec, RelationSpec,
    ResourceIdentitySpec, ResourceSpec, RouteSpec, SoftDeleteSpec, StorageSpec, ValidationSpec,
    WidgetSpec,
};
pub use writer::{
    OwnershipEntry, OwnershipManifest, PlanAction, PlannedAsset, ResourceAssetPlan,
    ResourceWorkspace, SafeWriteReport, plan_all_resource_changes, plan_resource_assets,
    plan_resource_changes, write_resource, write_resources,
};

use std::{fs, path::Path};

use sha2::{Digest, Sha256};

/// 读取并校验一个资源清单，再转换为确定排序的 IR。
pub fn load_resource(path: impl AsRef<Path>) -> Result<ResourceIr, ResourceError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|error| {
        ResourceError::file(
            path,
            format!("无法读取资源清单：{error}"),
            "确认文件存在且当前用户具有读取权限",
        )
    })?;
    let source_path = portable_path(path);
    let source_hash = hex::encode(Sha256::digest(normalized_source(&source).as_bytes()));
    let spec = ResourceSpec::parse(&source, &source_path)?;
    normalize_resource(spec, source_path, source_hash)
}

/// 将已经反序列化的清单转换为稳定 IR，适用于导入器和测试。
pub fn normalize_resource(
    spec: ResourceSpec,
    source_path: impl Into<String>,
    source_hash: impl Into<String>,
) -> Result<ResourceIr, ResourceError> {
    ir::normalize(spec, source_path.into(), source_hash.into())
}

fn normalized_source(source: &str) -> String {
    source.replace("\r\n", "\n").trim_end().to_owned() + "\n"
}

fn portable_path(path: &Path) -> String {
    let portable = path.to_string_lossy().replace('\\', "/");
    if let Some(index) = portable.rfind("/catalog/") {
        portable[index + 1..].to_owned()
    } else {
        portable
    }
}
