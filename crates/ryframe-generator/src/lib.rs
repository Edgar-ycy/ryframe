pub mod import;
pub mod naming;
pub mod resource;
mod schema;
mod type_mapping;

/// 生成器版本号；生成边界或端口签名变化时递增。
pub const GENERATOR_VERSION: &str = "1.0.0";

pub use resource::{
    AccessSpec, ApiSpec, AssetRoot, AuditSpec, DatabaseSpec, EnumValueSpec, ExplainNode,
    ExtensionSpec, FieldSpec, FieldUsageSpec, GeneratedAsset, GeneratedCatalog, IndexSpec,
    LabelsSpec, MenuSpec, OperationSpec, OwnershipEntry, OwnershipManifest, PermissionSpec,
    PlanAction, PlannedAsset, ResourceAssetPlan, ResourceError, ResourceExplanation,
    ResourceIdentitySpec, ResourceIr, ResourceProfile, ResourceSpec, ResourceWorkspace, RouteSpec,
    SafeWriteReport, SoftDeleteSpec, StorageKind, StorageSpec, ValidationSpec, ValueType,
    WidgetSpec, load_resource, normalize_resource, plan_resource_assets, plan_resource_changes,
    render_resources, write_resource, write_resources,
};
