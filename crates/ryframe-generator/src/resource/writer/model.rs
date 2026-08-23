use std::path::Path;

use serde::{Deserialize, Serialize};

use super::AssetRoot;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipManifest {
    pub format_version: u16,
    pub generator_version: String,
    #[serde(default)]
    pub entries: Vec<OwnershipEntry>,
}

impl Default for OwnershipManifest {
    fn default() -> Self {
        Self {
            format_version: 1,
            generator_version: crate::GENERATOR_VERSION.into(),
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipEntry {
    pub resource: String,
    pub root: AssetRoot,
    pub path: String,
    pub source_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_revision: Option<String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceWorkspace<'a> {
    pub backend_root: &'a Path,
    pub frontend_root: Option<&'a Path>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SafeWriteReport {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub written: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAction {
    Create,
    Update,
    Delete,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAsset {
    pub resource: String,
    pub root: AssetRoot,
    pub path: String,
    pub action: PlanAction,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceAssetPlan {
    pub assets: Vec<PlannedAsset>,
}
