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
#[path = "../src/local_test_path.rs"]
mod local_test_path;
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
#[path = "internal/check_cancellation.rs"]
mod check_cancellation_tests;
#[path = "internal/check_plan.rs"]
mod check_plan_tests;
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
#[path = "internal/ci_frontend_source.rs"]
mod ci_frontend_source_tests;
#[path = "internal/ci_required.rs"]
mod ci_required_tests;
#[path = "internal/ci_security_deployment.rs"]
mod ci_security_deployment_tests;
#[path = "internal/ci_security.rs"]
mod ci_security_tests;
#[path = "internal/ci.rs"]
mod ci_tests;
#[path = "internal/cli.rs"]
mod cli_tests;
#[path = "internal/contract_source.rs"]
mod contract_source_tests;
#[path = "internal/contract.rs"]
mod contract_tests;
#[path = "internal/data_cli.rs"]
mod data_cli_tests;
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
#[path = "internal/devex_cgroup_cli.rs"]
mod devex_cgroup_cli_tests;
#[path = "internal/devex_cli.rs"]
mod devex_cli_tests;
#[path = "internal/devex_memory.rs"]
mod devex_memory_tests;
#[path = "internal/devex_paths.rs"]
mod devex_paths_tests;
#[path = "internal/devex_provenance.rs"]
mod devex_provenance_tests;
#[path = "internal/devex_runtime.rs"]
mod devex_runtime_tests;
#[path = "internal/devex_sccache.rs"]
mod devex_sccache_tests;
#[path = "internal/devex.rs"]
mod devex_tests;
#[path = "internal/diff.rs"]
mod diff_tests;
#[path = "internal/doctor.rs"]
mod doctor_tests;
#[path = "internal/fixture_runtime.rs"]
mod fixture_runtime_tests;
#[path = "internal/migration.rs"]
mod migration_tests;
#[path = "internal/node_test_discovery.rs"]
mod node_tests_tests;
#[path = "internal/performance_identities.rs"]
mod performance_identities_tests;
#[path = "internal/process.rs"]
mod process_tests;
#[path = "internal/recovery_cli.rs"]
mod recovery_cli_tests;
#[path = "internal/recovery.rs"]
mod recovery_tests;
#[path = "internal/release.rs"]
mod release_tests;
#[path = "internal/resource_gate_cargo_surface.rs"]
mod resource_gate_cargo_surface_tests;
#[path = "internal/resource_gate.rs"]
mod resource_gate_tests;
#[cfg(feature = "resource")]
#[path = "internal/resource.rs"]
mod resource_tests;
#[path = "internal/seed_source.rs"]
mod seed_source_tests;
#[path = "internal/watch.rs"]
mod watch_tests;
#[path = "internal/workspace.rs"]
mod workspace_tests;
