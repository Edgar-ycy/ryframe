use super::{ResourceError, spec::*};

mod access;
mod contract;
mod field;
mod model;
mod normalize;
mod permission;
mod reference;
mod schema;
mod syntax;
mod value;

pub use model::{
    AccessIr, ApiIr, AuditIr, FieldIr, FieldUsageIr, IndexIr, LabelsIr, MenuIr, PermissionIr,
    RelationIr, RelationKind, ResourceIr, ResourceProfile, RouteIr, SoftDeleteIr, StorageKind,
    ValidationIr, ValueType, WidgetIr,
};

use access::{validate_api_and_access, validate_extensions};
use contract::{validate_generation_contract, validate_storage};
use field::field_error;
use field::{validate_field, validate_labels};
pub(super) use normalize::normalize;
use normalize::{operations, permissions};
use permission::is_permission;
use reference::ensure_field_reference;
use reference::validate_references;
use schema::normalize_schema;
use syntax::is_snake_identifier;
use syntax::{is_operation_symbol, is_safe_route, is_safe_symbol};
use value::{validate_enum_key, validate_field_value, validate_ir_value};
