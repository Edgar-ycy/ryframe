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
#[path = "../src/frontend_origin.rs"]
mod frontend_origin;
#[path = "../src/http_session.rs"]
mod http_session;
#[path = "../src/local_test_path.rs"]
mod local_test_path;
#[path = "../src/login_budget.rs"]
mod login_budget;
#[path = "../src/migration.rs"]
mod migration;
#[path = "../src/process.rs"]
mod process;
#[path = "support/process_liveness.rs"]
mod process_liveness;
#[path = "../src/rate_limit.rs"]
mod rate_limit;
#[path = "../src/recovery.rs"]
mod recovery;
#[path = "../src/release.rs"]
mod release;
#[path = "../src/resource.rs"]
mod resource;
#[path = "../src/session_pacing.rs"]
mod session_pacing;
#[path = "../src/source_edit.rs"]
mod source_edit;
#[path = "../src/source_scan.rs"]
mod source_scan;
#[path = "../src/watch.rs"]
mod watch;
#[path = "../src/workspace.rs"]
mod workspace;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[path = "../src/windows_acl.rs"]
mod windows_acl;

#[path = "internal/copy_session.rs"]
mod copy_session_tests;
#[path = "internal/login_budget.rs"]
mod login_budget_tests;
#[path = "internal/session_pacing.rs"]
mod session_pacing_tests;

#[path = "internal/job_evidence_model.rs"]
mod job_evidence_model_tests;
#[path = "internal/job_evidence_mysql.rs"]
mod job_evidence_mysql_tests;
#[path = "internal/job_evidence_process.rs"]
mod job_evidence_process_tests;
#[path = "internal/job_evidence_protocol.rs"]
mod job_evidence_protocol_tests;
#[path = "internal/job_evidence_wait.rs"]
mod job_evidence_wait_tests;

#[path = "../src/build.rs"]
mod build;
#[path = "../src/build_source.rs"]
mod build_source;
#[path = "../src/data.rs"]
mod data;

#[path = "internal/arm_target.rs"]
mod arm_target_tests;
#[path = "internal/backend_sources.rs"]
mod backend_sources_tests;
#[path = "internal/build.rs"]
mod build_tests;
#[path = "internal/check_architecture.rs"]
mod check_architecture_tests;
#[path = "internal/check_cancellation.rs"]
mod check_cancellation_tests;
#[path = "internal/check_cyclonedx.rs"]
mod check_cyclonedx_tests;
#[path = "internal/check_deployment_assets.rs"]
mod check_deployment_assets_tests;
#[path = "internal/check_permission_routes.rs"]
mod check_permission_routes_tests;
#[path = "internal/check_plan.rs"]
mod check_plan_tests;
#[path = "internal/check_policy_tasks.rs"]
mod check_policy_task_tests;
#[path = "internal/check_policy.rs"]
mod check_policy_tests;
#[path = "internal/check_prerelease.rs"]
mod check_prerelease_tests;
#[path = "internal/check_removed_identity.rs"]
mod check_removed_identity_tests;
#[path = "internal/check_snapshot.rs"]
mod check_snapshot_tests;
#[path = "internal/check_supply_chain.rs"]
mod check_supply_chain_tests;
#[path = "internal/check_test_harness.rs"]
mod check_test_harness_tests;
#[path = "internal/check.rs"]
mod check_tests;
#[path = "internal/check_trivy.rs"]
mod check_trivy_tests;
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
#[path = "internal/ci_tls.rs"]
mod ci_tls_tests;
#[path = "internal/ci_workflow.rs"]
mod ci_workflow_tests;
#[path = "internal/cli.rs"]
mod cli_tests;
#[path = "internal/clone_bridge.rs"]
mod clone_bridge_tests;
#[path = "internal/clone_cli.rs"]
mod clone_cli_tests;
#[path = "internal/clone_lifecycle.rs"]
mod clone_lifecycle_tests;
#[path = "internal/clone_native.rs"]
mod clone_native_tests;
#[cfg(windows)]
#[path = "internal/clone_process_identity.rs"]
mod clone_process_identity_tests;
#[path = "internal/clone_resources.rs"]
mod clone_resources_tests;
#[path = "internal/contract_source.rs"]
mod contract_source_tests;
#[path = "internal/contract_target.rs"]
mod contract_target_tests;
#[path = "internal/contract.rs"]
mod contract_tests;
#[path = "internal/data_cli.rs"]
mod data_cli_tests;
#[path = "internal/data.rs"]
mod data_tests;
#[path = "internal/dataset_prepare.rs"]
mod dataset_prepare_tests;
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
#[path = "internal/devex_cgroup.rs"]
mod devex_cgroup_tests;
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
#[path = "internal/export_native.rs"]
mod export_native_tests;
#[path = "internal/fixture_control.rs"]
mod fixture_control_tests;
#[path = "internal/fixture_dataset_control.rs"]
mod fixture_dataset_control_tests;
#[path = "internal/fixture_native.rs"]
mod fixture_native_tests;
#[path = "internal/fixture_prepare.rs"]
mod fixture_prepare_tests;
#[path = "internal/fixture_runtime.rs"]
mod fixture_runtime_tests;
#[path = "internal/import_samples_archive.rs"]
mod import_samples_archive_tests;
#[path = "internal/import_samples_fixture.rs"]
mod import_samples_fixture;
#[path = "internal/import_samples.rs"]
mod import_samples_tests;
#[path = "internal/language.rs"]
mod language_tests;
#[path = "internal/migration_history.rs"]
mod migration_history_tests;
#[path = "internal/migration.rs"]
mod migration_tests;
#[path = "internal/monitoring_native.rs"]
mod monitoring_native_tests;
#[path = "internal/monitoring_observation.rs"]
mod monitoring_observation_tests;
#[path = "internal/monitoring_runtime.rs"]
mod monitoring_runtime_tests;
#[path = "internal/monitoring_security.rs"]
mod monitoring_security_tests;
#[path = "internal/monitoring.rs"]
mod monitoring_tests;
#[path = "internal/performance_identities.rs"]
mod performance_identities_tests;
#[path = "internal/process.rs"]
mod process_tests;
#[path = "internal/python_suite.rs"]
mod python_suite_tests;
#[path = "internal/rate_limit_protocol.rs"]
mod rate_limit_protocol_tests;
#[path = "internal/rate_limit.rs"]
mod rate_limit_tests;
#[path = "internal/recovery_cli.rs"]
mod recovery_cli_tests;
#[path = "internal/recovery_inputs_backup.rs"]
mod recovery_inputs_backup_tests;
#[path = "internal/recovery_inputs_fixture.rs"]
mod recovery_inputs_fixture;
#[path = "internal/recovery_inputs_fresh.rs"]
mod recovery_inputs_fresh_tests;
#[path = "internal/recovery_inputs_product.rs"]
mod recovery_inputs_product_tests;
#[path = "internal/recovery_inputs_support.rs"]
mod recovery_inputs_support;
#[path = "internal/recovery_inputs.rs"]
mod recovery_inputs_tests;
#[path = "internal/recovery.rs"]
mod recovery_tests;
#[path = "internal/release_archive.rs"]
mod release_archive_tests;
#[path = "internal/release_ci.rs"]
mod release_ci_tests;
#[path = "internal/release_evidence.rs"]
mod release_evidence_tests;
#[path = "internal/release_source.rs"]
mod release_source_tests;
#[path = "internal/release.rs"]
mod release_tests;
#[path = "internal/release_workflow.rs"]
mod release_workflow_tests;
#[path = "internal/resource_gate_cargo_surface.rs"]
mod resource_gate_cargo_surface_tests;
#[path = "internal/resource_gate_replay_fixtures.rs"]
mod resource_gate_replay_fixtures;
#[path = "internal/resource_gate_replay.rs"]
mod resource_gate_replay_tests;
#[path = "internal/resource_gate.rs"]
mod resource_gate_tests;
#[cfg(feature = "resource")]
#[path = "internal/resource.rs"]
mod resource_tests;
#[path = "internal/runtime_configuration.rs"]
mod runtime_configuration_tests;
#[path = "internal/runtime_input.rs"]
mod runtime_input_tests;
#[path = "internal/runtime_pacing.rs"]
mod runtime_pacing_tests;
#[path = "internal/runtime_selection.rs"]
mod runtime_selection_tests;
#[path = "internal/runtime_source.rs"]
mod runtime_source_tests;
#[path = "internal/seed_source.rs"]
mod seed_source_tests;
#[cfg(windows)]
#[path = "internal/source_registration_cim.rs"]
mod source_registration_cim_tests;
#[path = "internal/target_authority_native.rs"]
mod target_authority_native;
#[path = "internal/target_configuration_fixture.rs"]
mod target_configuration_fixture;
#[path = "internal/target_database_native.rs"]
mod target_database_native;
#[path = "internal/target_evidence_aggregate.rs"]
mod target_evidence_aggregate_tests;
#[path = "internal/target_fresh_status.rs"]
mod target_fresh_status_tests;
#[path = "internal/target_guard_native.rs"]
mod target_guard_native_tests;
#[path = "internal/target_initialize_restart_fixture.rs"]
mod target_initialize_restart_fixture;
#[path = "internal/target_physical_file.rs"]
mod target_physical_file;
#[path = "internal/target_proof_native.rs"]
mod target_proof_native_tests;
#[path = "internal/target_publication_native.rs"]
mod target_publication_native_tests;
#[path = "internal/target_reset_native.rs"]
mod target_reset_native_tests;
#[path = "internal/target_resume_prefix.rs"]
mod target_resume_prefix_tests;
#[path = "internal/target_stage_protocol.rs"]
mod target_stage_protocol;
#[path = "internal/target_storage_model.rs"]
mod target_storage_model_tests;
#[path = "internal/target_storage_redis.rs"]
mod target_storage_redis;
#[path = "internal/target_storage_redis_state.rs"]
mod target_storage_redis_state;
#[path = "internal/target_verify_fixture.rs"]
mod target_verify_fixture;
#[path = "internal/target_verify_native.rs"]
mod target_verify_native_tests;
#[path = "internal/target_verify_published.rs"]
mod target_verify_published_tests;
#[path = "internal/watch.rs"]
mod watch_tests;
#[path = "internal/workspace.rs"]
mod workspace_tests;

#[path = "unit/recovery/clone/native/target_verify/prepare_history.rs"]
mod target_prepare_history_tests;
#[path = "unit/recovery/clone/native/target_verify/fresh/ports.rs"]
mod target_prepare_ports_tests;
#[path = "unit/recovery/clone/native/target_verify/fresh/registration.rs"]
mod target_prepare_registration_tests;

#[path = "internal/target_initialize_native.rs"]
mod target_initialize_native_tests;
