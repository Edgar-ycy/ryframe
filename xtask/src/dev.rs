//! 本地开发监督器的命令入口。

use std::{error::Error, fmt};

pub(crate) const TOOL_SELF_CHANGED_EXIT_CODE: i32 = 75;

#[derive(Debug)]
pub(crate) struct ToolSelfChanged;

impl fmt::Display for ToolSelfChanged {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("xtask 自身已变化，请重新运行 `cargo dev`")
    }
}

impl Error for ToolSelfChanged {}

pub(crate) fn tool_self_changed_error() -> Box<dyn Error> {
    Box::new(ToolSelfChanged)
}

pub(crate) fn failure_exit_code(error: &(dyn Error + 'static)) -> Option<i32> {
    error
        .downcast_ref::<ToolSelfChanged>()
        .map(|_| TOOL_SELF_CHANGED_EXIT_CODE)
}

#[path = "dev/build.rs"]
mod build;
#[path = "dev/command.rs"]
mod command;
#[path = "dev/config.rs"]
mod config;
#[path = "dev/health.rs"]
mod health;
#[path = "dev/measure.rs"]
mod measure;
#[path = "dev/model.rs"]
mod model;
#[path = "dev/orchestrator.rs"]
mod orchestrator;
#[path = "dev/runtime_secrets.rs"]
mod runtime_secrets;
#[path = "dev/services.rs"]
mod services;
#[path = "dev/snapshot.rs"]
mod snapshot;

#[allow(unused_imports)]
pub(crate) use measure::run as measure_once;
#[allow(unused_imports)]
pub(crate) use orchestrator::{CandidateProbeDisposition, candidate_probe_disposition, run};

#[allow(unused_imports)]
pub(crate) use build::{
    BuildContext, DEV_API_FEATURES, StepResult, build_candidate, run_migration_validation,
};
#[allow(unused_imports)]
pub(crate) use command::{RuntimeInputPaths, api_command, worker_command};
#[allow(unused_imports)]
pub(crate) use health::{
    available_ports, combine_failures, start_worker_after_api_ready,
    wait_probe_services_ready_until_controlled, wait_services_ready_until,
    wait_services_ready_until_controlled,
};
#[allow(unused_imports)]
pub(crate) use measure::{
    RESULT_FILE_NAME, ReadyKind, SaveCase, SaveMeasurement, SaveMeasurementContract,
    read_measurement, read_measurement_with_contract, ready_kind,
};
#[allow(unused_imports)]
pub(crate) use model::{
    ArtifactAction, Binaries, BuildPlan, BuildResult, ChangeKind, ChangeOutcome, CycleControl,
    MigrationValidation, ProbeResult, classify_change,
};
#[allow(unused_imports)]
pub(crate) use runtime_secrets::{
    RuntimeSecrets, runtime_config_matches_snapshot, snapshot_config_tree,
};
#[allow(unused_imports)]
pub(crate) use services::switch_services_with_rollback;
#[allow(unused_imports)]
pub(crate) use snapshot::DevSession;
