use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsumerContractPlan {
    pub(crate) mode: &'static str,
    pub(crate) backend_commit: String,
    pub(crate) backend_repository: String,
    pub(crate) require_pin: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceGraph {
    pub(crate) package_by_dir: BTreeMap<String, String>,
    pub(crate) reverse_dependencies: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FrontendProfile {
    Contract,
    Code,
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BackendSnapshotProfile {
    OpenApiContract,
    Mysql,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VerifySelection {
    pub(crate) full_reason: Option<String>,
    pub(crate) backend_packages: BTreeSet<String>,
    pub(crate) backend_snapshot_profiles: BTreeSet<BackendSnapshotProfile>,
    pub(crate) frontend_profiles: BTreeSet<FrontendProfile>,
    pub(super) reasons: Vec<String>,
}
