use super::{AssetRoot, GeneratedAsset, GeneratedCatalog, ResourceError, ResourceIr};

mod execute;
mod model;
mod ownership;
mod path;
mod plan;
mod schema;
mod transaction;

pub use execute::{write_resource, write_resources};
pub use model::{
    OwnershipEntry, OwnershipManifest, PlanAction, PlannedAsset, ResourceAssetPlan,
    ResourceWorkspace, SafeWriteReport,
};
pub use plan::{plan_all_resource_changes, plan_resource_assets, plan_resource_changes};

use ownership::{
    desired_entries, load_manifest, load_manifest_snapshot, manifests_equal, validate_workspace,
    verify_no_resource_removal, verify_owned_files, verify_unselected_resource_sources,
};
use path::{content_hash, display_path, extract_source_hash, target_path, validate_managed_path};
use plan::selected_assets;
use transaction::{
    ExpectedFile, InstalledFile, install_staged, move_to_backup, persist_recovery_directories,
    rollback, verify_expected_file, write_staged,
};

const MANIFEST_PATH: &str = "catalog/resources/.ownership.toml";
