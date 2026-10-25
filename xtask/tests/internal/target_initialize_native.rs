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
use serde_json::Value;
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
