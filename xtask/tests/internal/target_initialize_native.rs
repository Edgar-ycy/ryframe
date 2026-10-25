//! 正式编排与精确历史模型的离线行为；真实CLI/CIM/服务验收由主控单独执行。
#[path = "target_initialize_fixture.rs"]
mod fixture;
use crate::{
    Result,
    recovery::clone::native::{
        json as document,
        target_verify::{
            fresh::{initialize, registration::Registration},
            guard::Generation,
            published,
            storage::StorageInputs,
        },
    },
    target_verify_fixture::{Fixture, snapshot},
};
use fixture::{Resources, retain_prepared};
use serde_json::{Value, json};
use std::fs;

fn execute(resources: &Resources<'_>, resuming: bool) -> Result<Value> {
    let f = resources.fixture;
    let registration = Registration::load(&f.backend, &f.local)?;
    let proof = initialize::prepared(&registration)?;
    let mut attempt = if resuming {
        Some(initialize::resume::Attempt::new(
            &registration,
            initialize::resume::state(&registration, &proof, false)?,
        )?)
    } else {
        None
    };
    let owner = Generation::acquire(&f.backend, &f.target)?;
    let mut environment = f.environment.clone();
    environment.insert("TEMP".into(), f.local.to_str().unwrap().into());
    let protected = initialize::inputs::protect(&registration, &f.request, &environment)?;
    let observe_generation = || resources.generation();
    let window = initialize::window::Window {
        registration: &registration,
        inputs: StorageInputs {
            backend: &f.backend,
            request: &f.request,
            environment: &environment,
            observation_dir: &f.target,
        },
        transport: resources,
        observer: resources,
        protected: &protected,
        owner: &owner,
        observe_generation: &observe_generation,
    };
    let mut stage = "initialize_binding";
    let result = if let Some(attempt) = attempt.as_mut() {
        initialize::resume::execute(&window, &proof, attempt)
    } else {
        initialize::initial::execute(&window, &proof, &mut stage)
    };
    let result = owner.finish_publishing(result, |(_, files)| {
        initialize::publish(&registration, files, &proof)
    });
    initialize::finish(&registration, resuming, attempt.as_mut(), stage, result)
}

#[test]
fn initialization_publishes_four_databases_ten_migrations_and_complete_readonly_inventory() {
    let f = Fixture::for_initialization();
    let resources = Resources::new(&f);
    retain_prepared(&f);
    let value = execute(&resources, false).unwrap();
    assert_eq!(value["status"], "fresh_target_initialized");
    assert_eq!(value["restore_qualified"], false);
    assert_eq!(resources.created.borrow().len(), 4);
    let calls = resources.calls.borrow();
    let migrations = calls
        .iter()
        .filter(|v| {
            v.get(3)
                .is_some_and(|s| s == "control" || s == "tenant-data")
        })
        .collect::<Vec<_>>();
    assert_eq!(migrations.len(), 10);
    assert_eq!(&migrations[0][3..], ["control", "up"]);
    assert_eq!(&migrations[1][3..], ["control", "verify"]);
    assert_eq!(
        calls
            .iter()
            .filter(|v| v.get(3).is_some_and(|s| s == "target-inventory"))
            .count(),
        8
    );
    assert!(
        !calls
            .iter()
            .flatten()
            .any(|v| v == "--all" || v == "--write")
    );
    drop(calls);
    let before = snapshot(&f.local);
    let initialized = f.target.join("initialized.json");
    assert_eq!(
        published::unlocked_history(&f.backend, &initialized)
            .unwrap()
            .1,
        f.request
    );
    assert_eq!(snapshot(&f.local), before);
    let target_before = snapshot(&f.target);
    assert!(initialize::execute(&f.backend, &f.local, false).is_err());
    assert_eq!(snapshot(&f.target), target_before);
    assert_eq!(resources.created.borrow().len(), 4);
}

#[test]
fn unknown_create_keeps_intent_and_rejects_retry_without_cleanup_or_confirmation() {
    let f = Fixture::for_initialization();
    let resources = Resources::new(&f);
    retain_prepared(&f);
    resources.fail.set(Some("create"));
    assert!(execute(&resources, false).is_err());
    assert_eq!(resources.created.borrow().len(), 1);
    let key = f.request["target"]["databases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["key"].as_str().unwrap())
        .min()
        .unwrap();
    let intent = f.target.join(format!("create-{key}.intent.json"));
    let original = fs::read(&intent).unwrap();
    assert!(
        !f.target
            .join(format!("create-{key}.confirmed.json"))
            .exists()
    );
    assert!(!f.target.join("initialized.json").exists());
    assert!(!resources.reset.get());
    let before = snapshot(&f.target);
    assert!(initialize::execute(&f.backend, &f.local, false).is_err());
    assert!(initialize::execute(&f.backend, &f.local, true).is_err());
    assert_eq!(snapshot(&f.target), before);
    assert_eq!(fs::read(intent).unwrap(), original);
    assert!(
        !resources
            .calls
            .borrow()
            .iter()
            .flatten()
            .any(|v| v.contains("DROP"))
    );
}

#[test]
fn inventory_resume_never_replays_reset_or_migrations_and_keeps_failed_raw_material() {
    let f = Fixture::for_initialization();
    let resources = Resources::new(&f);
    retain_prepared(&f);
    resources.fail.set(Some("inventory"));
    assert!(execute(&resources, false).is_err());
    assert_eq!(
        document::read(&f.target.join("failure.json")).unwrap()["error_type"],
        "InventoryCaptureError"
    );
    let failure = fs::read(f.target.join("failure.json")).unwrap();
    let failed = snapshot(&f.target.join("inventory-initial"));
    let count = resources.calls.borrow().len();
    let value = execute(&resources, true).unwrap();
    assert_eq!(value["resumed"], true);
    for call in &resources.calls.borrow()[count..] {
        assert!(
            !call
                .get(3)
                .is_some_and(|v| v == "execute" || v == "control" || v == "tenant-data")
        );
    }
    assert_eq!(fs::read(f.target.join("failure.json")).unwrap(), failure);
    assert_eq!(snapshot(&f.target.join("inventory-initial")), failed);
    published::unlocked_history(&f.backend, &f.target.join("initialized.json")).unwrap();
    let before = snapshot(&f.target);
    assert!(initialize::execute(&f.backend, &f.local, true).is_err());
    assert_eq!(snapshot(&f.target), before);
}

#[test]
fn migration_resume_starts_at_missing_verify_and_rejects_unknown_tree_before_commands() {
    let f = Fixture::for_initialization();
    let resources = Resources::new(&f);
    retain_prepared(&f);
    resources.fail.set(Some("migrate-control-verify"));
    assert!(execute(&resources, false).is_err());
    let count = resources.calls.borrow().len();
    let registration = Registration::load(&f.backend, &f.local).unwrap();
    let proof = initialize::prepared(&registration).unwrap();
    let state = initialize::resume::state(&registration, &proof, false).unwrap();
    assert_eq!(state["next_index"], 1);
    let unknown = f.target.join("unknown-write");
    fs::write(&unknown, b"unowned").unwrap();
    assert!(initialize::resume::state(&registration, &proof, false).is_err());
    assert_eq!(resources.calls.borrow().len(), count);
    assert_eq!(fs::read(&unknown).unwrap(), b"unowned");
    fs::remove_file(unknown).unwrap();
    let value = execute(&resources, true).unwrap();
    assert_eq!(value["resumed"], true);
    let calls = resources.calls.borrow();
    let resumed = calls[count..]
        .iter()
        .filter(|v| {
            v.get(3)
                .is_some_and(|s| s == "control" || s == "tenant-data")
        })
        .collect::<Vec<_>>();
    assert_eq!(resumed.len(), 9);
    assert_eq!(&resumed[0][3..], ["control", "verify"]);
    assert!(
        !resumed
            .iter()
            .any(|v| v.get(3).is_some_and(|v| v == "execute"))
    );
    drop(calls);
    published::unlocked_history(&f.backend, &f.target.join("initialized.json")).unwrap();
}

#[test]
fn changed_source_configuration_or_frozen_tool_cannot_reach_first_write() {
    for changed in ["configuration", "tool", "source"] {
        let f = Fixture::for_initialization();
        let resources = Resources::new(&f);
        retain_prepared(&f);
        let path = match changed {
            "configuration" => f.local.join("config/app.toml"),
            "tool" => f.local.join("mysql"),
            _ => f.backend.join("crates/ryframe/Cargo.toml"),
        };
        let original = fs::read(&path).unwrap();
        let mut bytes = original.clone();
        bytes.extend_from_slice(b"\n# drift\n");
        fs::write(&path, bytes).unwrap();
        assert!(execute(&resources, false).is_err(), "{changed}");
        assert!(resources.created.borrow().is_empty());
        assert!(!resources.reset.get());
        assert!(!f.target.join("initialize.started.json").exists());
        fs::write(path, original).unwrap();
    }
}

#[test]
fn failed_resumed_write_is_bound_as_unknown_and_cannot_be_replayed() {
    let f = Fixture::for_initialization();
    let resources = Resources::new(&f);
    retain_prepared(&f);
    resources.fail.set(Some("migrate-control-verify"));
    assert!(execute(&resources, false).is_err());
    resources
        .fail
        .set(Some("migrate-tenant-data---target-dedicated-a-up"));
    assert!(execute(&resources, true).is_err());
    let confirmation = fs::read_dir(&f.target)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("resume-initialize-")
                && p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .ends_with(".failure.json")
        })
        .unwrap();
    let failure = document::read(&confirmation).unwrap();
    assert_eq!(
        failure["unknown_write_operation"],
        "tenant-data-dedicated-a-up"
    );
    assert_eq!(failure["phase"], "command-running");
    let before = snapshot(&f.target);
    let calls = resources.calls.borrow().len();
    assert!(execute(&resources, true).is_err());
    assert_eq!(resources.calls.borrow().len(), calls);
    assert_eq!(snapshot(&f.target), before);
}

fn interrupted_before_dedicated_verify(resources: &Resources<'_>) {
    retain_prepared(resources.fixture);
    resources
        .fail
        .set(Some("migrate-tenant-data---target-dedicated-a-verify"));
    assert!(execute(resources, false).is_err());
}
fn migration_commands(resources: &Resources<'_>) -> Vec<Vec<String>> {
    resources
        .calls
        .borrow()
        .iter()
        .filter(|v| {
            v.get(3)
                .is_some_and(|v| v == "control" || v == "tenant-data")
        })
        .cloned()
        .collect()
}
fn attempt_failure(f: &Fixture) -> (std::path::PathBuf, Value) {
    let paths = fs::read_dir(&f.target)
        .unwrap()
        .map(|v| v.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_str().unwrap();
            name.starts_with("resume-initialize-") && name.ends_with(".failure.json")
        })
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 1);
    (paths[0].clone(), document::read(&paths[0]).unwrap())
}

#[test]
fn registered_restart_rebinds_markers_once_before_remaining_migrations() {
    let f = Fixture::for_recovery();
    let resources = Resources::new(&f);
    interrupted_before_dedicated_verify(&resources);
    resources.registered_restart();
    let result = execute(&resources, true).unwrap();
    assert_eq!(result["status"], "fresh_target_initialized");
    assert_eq!(resources.eval_calls(), 1);
    let initialized = document::read(&f.target.join("initialized.json")).unwrap();
    assert_eq!(
        initialized["redis"]["owner"],
        "ryframe-owner:v1:seed-scope:redis"
    );
    assert_eq!(
        initialized["redis"]["sentinel"],
        f.request["reset"]["sentinel_value"]
    );
    let commands = migration_commands(&resources);
    assert_eq!(
        commands
            .iter()
            .filter(|v| v.get(4).is_some_and(|v| v == "up"))
            .count(),
        5
    );
    assert_eq!(
        commands
            .iter()
            .filter(|v| v[3..] == ["control", "up"])
            .count(),
        1
    );
    assert_eq!(
        commands
            .iter()
            .filter(|v| v[3..] == ["tenant-data", "up", "--target", "dedicated-a"])
            .count(),
        1
    );
    published::unlocked_history(&f.backend, &f.target.join("initialized.json")).unwrap();
}

#[test]
fn registered_restart_with_existing_sentinel_rejects_before_marker_intent() {
    let f = Fixture::for_recovery();
    let resources = Resources::new(&f);
    interrupted_before_dedicated_verify(&resources);
    resources.registered_restart();
    resources.existing_sentinel();
    let before = migration_commands(&resources);
    assert!(execute(&resources, true).is_err());
    assert_eq!(resources.eval_calls(), 0);
    assert_eq!(migration_commands(&resources), before);
    assert!(!fs::read_dir(&f.target).unwrap().any(|v| {
        v.unwrap()
            .file_name()
            .to_str()
            .unwrap()
            .starts_with("resume-redis-markers-")
    }));
    assert_eq!(
        resources.redis_image()[f.request["reset"]["sentinel_key"].as_str().unwrap()],
        "unexpected"
    );
}

#[test]
fn marker_write_with_lost_reply_keeps_unknown_result_and_never_replays() {
    let f = Fixture::for_recovery();
    let resources = Resources::new(&f);
    interrupted_before_dedicated_verify(&resources);
    resources.registered_restart();
    resources.lose_eval_reply.set(true);
    let commands = migration_commands(&resources);
    assert!(execute(&resources, true).is_err());
    assert_eq!(resources.eval_calls(), 1);
    assert_eq!(migration_commands(&resources), commands);
    let (_, failure) = attempt_failure(&f);
    assert_eq!(failure["unknown_write_operation"], "redis-marker-rebind");
    let marker = document::read(std::path::Path::new(
        failure["active_operation"]["failure"]["path"]
            .as_str()
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(marker["unknown_result"], true);
    assert_eq!(
        resources.redis_image()["ryframe:{seed-scope}:.ryframe-owner"],
        "ryframe-owner:v1:seed-scope:redis"
    );
    assert_eq!(
        resources.redis_image()[f.request["reset"]["sentinel_key"].as_str().unwrap()],
        f.request["reset"]["sentinel_value"]
    );
    let before = snapshot(&f.target);
    let calls = resources.calls.borrow().clone();
    let redis_calls = resources.redis_calls.borrow().clone();
    assert!(execute(&resources, true).is_err());
    assert_eq!(*resources.calls.borrow(), calls);
    assert_eq!(*resources.redis_calls.borrow(), redis_calls);
    assert_eq!(resources.eval_calls(), 1);
    assert_eq!(snapshot(&f.target), before);
}

#[test]
fn successful_write_with_failed_confirmation_keeps_receipt_and_unknown_write() {
    let f = Fixture::for_recovery();
    let resources = Resources::new(&f);
    interrupted_before_dedicated_verify(&resources);
    resources.confirmation_failure.set(true);
    assert!(execute(&resources, true).is_err());
    let (_, failure) = attempt_failure(&f);
    assert_eq!(
        failure["unknown_write_operation"],
        "tenant-data-dedicated-b-up"
    );
    assert_eq!(failure["phase"], "operation-confirmation-publish");
    assert_eq!(
        failure["confirmed_operations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["tenant-data-dedicated-a-verify"]
    );
    let receipts = failure["active_operation"]["command_receipts"]
        .as_array()
        .unwrap();
    assert_eq!(receipts.len(), 1);
    let path = std::path::Path::new(receipts[0]["path"].as_str().unwrap());
    assert_eq!(
        crate::recovery::clone::native::resources::common::binding(path).unwrap(),
        receipts[0]
    );
    assert_eq!(document::read(path).unwrap()["returncode"], 0);
    assert!(!resources.failed_publications.borrow()[0].exists());
    let before = snapshot(&f.target);
    let calls = resources.calls.borrow().clone();
    let redis_calls = resources.redis_calls.borrow().clone();
    assert!(execute(&resources, true).is_err());
    assert_eq!(*resources.calls.borrow(), calls);
    assert_eq!(*resources.redis_calls.borrow(), redis_calls);
    assert_eq!(snapshot(&f.target), before);
}

fn assert_interrupted_receipts_are_bound(
    f: &Fixture,
    resources: &Resources<'_>,
    proof: &Value,
    failure: &Value,
) {
    let registration = Registration::load(&f.backend, &f.local).unwrap();
    let intent = document::read(std::path::Path::new(
        failure["intent"]["path"].as_str().unwrap(),
    ))
    .unwrap();
    let names = intent["files_before"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["path"].as_str())
        .filter(|v| v.starts_with("migrate-tenant-data---target-dedicated-a-verify-"))
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 1);
    let original = f.target.join(names[0]);
    let bytes = fs::read(&original).unwrap();
    let before = snapshot(&f.target);
    let calls = resources.calls.borrow().clone();
    let mut changed = bytes.clone();
    changed.push(b'\n');
    fs::write(&original, &changed).unwrap();
    assert!(initialize::resume::state(&registration, proof, false).is_err());
    fs::write(&original, &bytes).unwrap();
    let new_path = resources.failed_publications.borrow()[0].clone();
    assert!(!new_path.exists());
    for body in [b"{".as_slice(), bytes.as_slice()] {
        fs::write(&new_path, body).unwrap();
        assert!(initialize::resume::state(&registration, proof, false).is_err());
        assert_eq!(fs::read(&new_path).unwrap(), body);
        fs::remove_file(&new_path).unwrap();
    }
    assert_eq!(*resources.calls.borrow(), calls);
    assert_eq!(snapshot(&f.target), before);
}

#[test]
fn readonly_publication_failure_allows_one_bound_successor_without_marker_replay() {
    let f = Fixture::for_recovery();
    let resources = Resources::new(&f);
    interrupted_before_dedicated_verify(&resources);
    let original = snapshot(&f.target);
    resources.registered_restart();
    resources.receipt_failures.set(1);
    assert!(execute(&resources, true).is_err());
    let (path, failure) = attempt_failure(&f);
    let failure_bytes = fs::read(&path).unwrap();
    assert_eq!(failure["phase"], "command-receipt-publish");
    assert_eq!(failure["unknown_write_operation"], Value::Null);
    assert_eq!(failure["remote_write_operations_confirmed"], 1);
    assert_eq!(failure["active_operation"]["command_receipts"], json!([]));
    assert_eq!(failure["os_error"]["filename_role"], "command_receipt");
    assert!(
        resources.failed_publications.borrow()[0]
            .to_string_lossy()
            .chars()
            .count()
            >= 260
    );
    let registration = Registration::load(&f.backend, &f.local).unwrap();
    let proof = initialize::prepared(&registration).unwrap();
    assert_interrupted_receipts_are_bound(&f, &resources, &proof, &failure);
    let result = execute(&resources, true).unwrap();
    assert_eq!(result["status"], "fresh_target_initialized");
    assert_eq!(resources.eval_calls(), 1);
    assert_eq!(fs::read(path).unwrap(), failure_bytes);
    let final_files = snapshot(&f.target);
    for (path, bytes) in original {
        assert_eq!(final_files.get(&path), Some(&bytes));
    }
    published::unlocked_history(&f.backend, &f.target.join("initialized.json")).unwrap();
}

#[test]
fn second_readonly_publication_failure_blocks_third_attempt_without_commands() {
    let f = Fixture::for_recovery();
    let resources = Resources::new(&f);
    interrupted_before_dedicated_verify(&resources);
    resources.registered_restart();
    resources.receipt_failures.set(2);
    assert!(execute(&resources, true).is_err());
    assert!(execute(&resources, true).is_err());
    assert_eq!(resources.failed_publications.borrow().len(), 2);
    assert_eq!(resources.eval_calls(), 1);
    let calls = resources.calls.borrow().clone();
    let redis_calls = resources.redis_calls.borrow().clone();
    let before = snapshot(&f.target);
    assert!(execute(&resources, true).is_err());
    assert_eq!(*resources.calls.borrow(), calls);
    assert_eq!(*resources.redis_calls.borrow(), redis_calls);
    assert_eq!(resources.eval_calls(), 1);
    assert_eq!(snapshot(&f.target), before);
}
