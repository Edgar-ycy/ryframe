use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use ryframe_application::system::tenant_config_transfer_service::compare_resources;
use ryframe_application::{
    TenantConfigRequesterRecord, TenantConfigTransferItemRecord, TenantConfigurationFenceRecord,
    next_id,
    system::{
        CONFIG_PACKAGE_BUCKET, PortableConfig, PortableDepartment, PortableDictData,
        PortableDictType, PortableMenu, PortablePermission, PortablePost, PortableRole,
        TenantConfigPackageResources, TenantConfigTargetCatalog,
    },
    tenant_config_stable_key::*,
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait,
    IntoActiveModel, PaginatorTrait, QueryFilter, QueryOrder, sea_query::Expr,
};

use crate::{
    FileRepository,
    entities::{
        config, dept, dict_data, dict_type, menu, permission, post, role, role_dept,
        role_permission, tenant, tenant_config_transfer_item, user, user_role,
    },
};

mod apply_resources;
mod file_validation;
mod resources;
mod rollback_resources;
mod workflow_support;

use resources::{build_department_paths, build_menu_stable_keys};

pub(super) use apply_resources::apply_resources_in_transaction;
pub(super) use file_validation::ensure_config_package_file_ready_in_txn;
pub(super) use resources::load_resources_on;
pub(super) use rollback_resources::{
    ensure_rollback_references_safe, restore_snapshot_in_transaction,
};
pub(super) use workflow_support::{
    ensure_requester_snapshot_in_txn, ensure_role_quota_for_plan_in_txn, mark_plan_outcome,
};

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
