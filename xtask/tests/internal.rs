#![allow(dead_code)]

use std::error::Error;

#[path = "../src/check.rs"]
mod check;
#[path = "../src/ci.rs"]
mod ci;
#[path = "../src/cli.rs"]
mod cli;
#[path = "../src/contract.rs"]
mod contract;
#[path = "../src/dev.rs"]
mod dev;
#[path = "../src/devex.rs"]
mod devex;
#[path = "../src/diff.rs"]
mod diff;
#[path = "../src/doctor.rs"]
mod doctor;
#[path = "../src/migration.rs"]
mod migration;
#[path = "../src/process.rs"]
mod process;
#[path = "../src/recovery.rs"]
mod recovery;
#[path = "../src/release.rs"]
mod release;
#[path = "../src/resource.rs"]
mod resource;
#[path = "../src/source_edit.rs"]
mod source_edit;
#[path = "../src/watch.rs"]
mod watch;
#[path = "../src/workspace.rs"]
mod workspace;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[path = "../src/build.rs"]
mod build;
#[path = "../src/data.rs"]
mod data;

#[path = "internal/build.rs"]
mod build_tests;
#[path = "internal/check_policy_tasks.rs"]
mod check_policy_task_tests;
#[path = "internal/check_policy.rs"]
mod check_policy_tests;
#[path = "internal/check_snapshot.rs"]
mod check_snapshot_tests;
#[path = "internal/check.rs"]
mod check_tests;
#[path = "internal/child_environment.rs"]
mod child_environment_tests;
#[path = "internal/ci.rs"]
mod ci_tests;
#[path = "internal/cli.rs"]
mod cli_tests;
#[path = "internal/contract.rs"]
mod contract_tests;
#[path = "internal/data.rs"]
mod data_tests;
#[path = "internal/dev_probe_control.rs"]
mod dev_probe_control_tests;
#[path = "internal/dev_process_control.rs"]
mod dev_process_control_tests;
#[path = "internal/dev_snapshot.rs"]
mod dev_snapshot_tests;
#[path = "internal/dev.rs"]
mod dev_tests;
#[path = "internal/devex_acceptance.rs"]
mod devex_acceptance_tests;
#[path = "internal/devex_cli.rs"]
mod devex_cli_tests;
#[path = "internal/devex_paths.rs"]
mod devex_paths_tests;
#[path = "internal/devex_sccache.rs"]
mod devex_sccache_tests;
#[path = "internal/devex.rs"]
mod devex_tests;
#[path = "internal/diff.rs"]
mod diff_tests;
#[path = "internal/doctor.rs"]
mod doctor_tests;
#[path = "internal/migration.rs"]
mod migration_tests;
#[path = "internal/process.rs"]
mod process_tests;
#[path = "internal/recovery.rs"]
mod recovery_tests;
#[path = "internal/resource_gate_cargo_surface.rs"]
mod resource_gate_cargo_surface_tests;
#[path = "internal/resource_gate.rs"]
mod resource_gate_tests;
#[cfg(feature = "resource")]
#[path = "internal/resource.rs"]
mod resource_tests;
#[path = "internal/watch.rs"]
mod watch_tests;
#[path = "internal/workspace.rs"]
mod workspace_tests;
