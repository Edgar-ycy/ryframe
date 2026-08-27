use std::{collections::BTreeSet, path::PathBuf, process::Child, time::Duration};

use crate::{
    watch::{ChangeBatch, SourceRevision},
    workspace::root_dir,
};

use super::{build::cleanup_binaries, services::stop_all};

pub(super) const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const WATCH_DEBOUNCE: Duration = Duration::from_millis(350);
pub(super) const LOOP_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ChangeKind {
    RuntimeConfig,
    IgnoredDevConfig,
    LocaleCatalog,
    ApiOnly,
    WorkerOnly,
    SharedRuntime,
    ControlPersistence,
    TenantPersistence,
    MigrationTool,
    ResourceManifest,
    GeneratorTool,
    BuildGraph,
    ToolSelf,
    UnknownBackend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactAction {
    Rebuild,
    ReuseLkg,
    NotNeeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationValidation {
    None,
    StandaloneControl,
    StandaloneTenant,
    StandaloneControlAndTenant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CycleControl {
    Continue,
    Superseded,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeResult {
    Ready,
    Superseded,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildPlan {
    pub(crate) source_revision: SourceRevision,
    pub(crate) api: ArtifactAction,
    pub(crate) worker: ArtifactAction,
    pub(crate) migrate: bool,
    pub(crate) restart_pair: bool,
    pub(crate) resource_check: bool,
    pub(crate) tool_self_changed: bool,
    pub(crate) reasons: BTreeSet<ChangeKind>,
    pub(crate) migration: MigrationValidation,
}

impl BuildPlan {
    pub(crate) fn initial(source_revision: SourceRevision) -> Self {
        Self {
            source_revision,
            api: ArtifactAction::Rebuild,
            worker: ArtifactAction::Rebuild,
            migrate: true,
            restart_pair: true,
            resource_check: false,
            tool_self_changed: false,
            reasons: BTreeSet::from([ChangeKind::BuildGraph]),
            migration: MigrationValidation::StandaloneControl,
        }
    }

    pub(crate) fn from_changes(batch: &ChangeBatch) -> Self {
        let reasons = batch
            .paths
            .iter()
            .map(|path| classify_change(path))
            .collect::<BTreeSet<_>>();
        let full = reasons.contains(&ChangeKind::BuildGraph)
            || reasons.contains(&ChangeKind::UnknownBackend);
        let shared = full
            || reasons.contains(&ChangeKind::SharedRuntime)
            || reasons.contains(&ChangeKind::ControlPersistence)
            || reasons.contains(&ChangeKind::TenantPersistence);
        let api_rebuild = shared
            || reasons.contains(&ChangeKind::ApiOnly)
            || reasons.contains(&ChangeKind::LocaleCatalog);
        let worker_rebuild = shared
            || reasons.contains(&ChangeKind::WorkerOnly)
            || reasons.contains(&ChangeKind::LocaleCatalog);
        let runtime_config = reasons.contains(&ChangeKind::RuntimeConfig);
        let restart_pair = api_rebuild || worker_rebuild || runtime_config;
        let migrate = full
            || reasons.contains(&ChangeKind::ControlPersistence)
            || reasons.contains(&ChangeKind::TenantPersistence)
            || reasons.contains(&ChangeKind::MigrationTool);
        let migration = if reasons.contains(&ChangeKind::TenantPersistence)
            && (reasons.contains(&ChangeKind::ControlPersistence) || full)
        {
            MigrationValidation::StandaloneControlAndTenant
        } else if reasons.contains(&ChangeKind::TenantPersistence) {
            MigrationValidation::StandaloneTenant
        } else if migrate {
            MigrationValidation::StandaloneControl
        } else {
            MigrationValidation::None
        };
        Self {
            source_revision: batch.revision,
            api: artifact_action(api_rebuild, restart_pair),
            worker: artifact_action(worker_rebuild, restart_pair),
            migrate,
            restart_pair,
            resource_check: reasons.contains(&ChangeKind::ResourceManifest)
                || reasons.contains(&ChangeKind::GeneratorTool),
            tool_self_changed: reasons.contains(&ChangeKind::ToolSelf),
            reasons,
            migration,
        }
    }

    pub(crate) fn is_noop(&self) -> bool {
        !self.restart_pair && !self.migrate && !self.resource_check && !self.tool_self_changed
    }
}

fn artifact_action(rebuild: bool, restart_pair: bool) -> ArtifactAction {
    if rebuild {
        ArtifactAction::Rebuild
    } else if restart_pair {
        ArtifactAction::ReuseLkg
    } else {
        ArtifactAction::NotNeeded
    }
}

pub(crate) fn classify_change(path: &str) -> ChangeKind {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    if matches!(
        path.as_str(),
        "cargo.toml" | "cargo.lock" | "build.rs" | "rust-toolchain" | "rust-toolchain.toml"
    ) || path.starts_with(".cargo/")
        || path.ends_with("/cargo.toml")
        || path.ends_with("/build.rs")
    {
        return ChangeKind::BuildGraph;
    }
    if path.starts_with("xtask/") {
        return ChangeKind::ToolSelf;
    }
    if matches!(
        path.as_str(),
        "config/app.prod.toml" | "config/feature-matrix.json"
    ) {
        return ChangeKind::IgnoredDevConfig;
    }
    if path.starts_with("config/") {
        return ChangeKind::RuntimeConfig;
    }
    if path.starts_with("locales/") {
        return ChangeKind::LocaleCatalog;
    }
    if path.starts_with("catalog/resources/") {
        return ChangeKind::ResourceManifest;
    }
    if path.starts_with("catalog/access") {
        return ChangeKind::ControlPersistence;
    }
    if path.starts_with("crates/ryframe-generator/") || path.starts_with("crates/ryframe-macro/") {
        return ChangeKind::GeneratorTool;
    }
    if path.starts_with("crates/ryframe-api/")
        || path == "crates/ryframe/src/main.rs"
        || path.starts_with("crates/ryframe/src/app/")
    {
        return ChangeKind::ApiOnly;
    }
    if path == "crates/ryframe/src/bin/ryframe_worker.rs" {
        return ChangeKind::WorkerOnly;
    }
    if path == "crates/ryframe/src/bin/ryframe_migrate.rs" {
        return ChangeKind::MigrationTool;
    }
    if path.starts_with("crates/ryframe-db/src/migration/") {
        return ChangeKind::ControlPersistence;
    }
    if path.starts_with("crates/ryframe-tenant-db/src/migration/") {
        return ChangeKind::TenantPersistence;
    }
    if [
        "crates/ryframe-application/",
        "crates/ryframe-kernel/",
        "crates/ryframe-auth/",
        "crates/ryframe-config/",
        "crates/ryframe-adapters/",
        "crates/ryframe-db/",
        "crates/ryframe-tenant-db/",
        "crates/ryframe/src/boot/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
    {
        return ChangeKind::SharedRuntime;
    }
    ChangeKind::UnknownBackend
}

#[derive(Debug, Clone)]
pub(super) struct Binaries {
    pub(super) source_revision: SourceRevision,
    pub(super) api: PathBuf,
    pub(super) worker: PathBuf,
    pub(super) generation_dir: PathBuf,
    pub(super) runtime_dir: PathBuf,
    pub(super) config_dir: PathBuf,
    pub(super) locales_dir: PathBuf,
}

pub(super) struct Services {
    pub(super) api: Child,
    pub(super) worker: Child,
    pub(super) binaries: Binaries,
}

pub(super) struct RunningProcesses {
    pub(super) services: Services,
    pub(super) vite: Child,
}

impl Drop for RunningProcesses {
    fn drop(&mut self) {
        let _ = stop_all(&mut self.services, &mut self.vite);
        let _ = cleanup_binaries(&root_dir(), &self.services.binaries);
    }
}

pub(super) enum BuildResult {
    Ready(Binaries),
    VerifiedNoRestart,
    Failed,
    Superseded,
    Cancelled,
}
