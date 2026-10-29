//! 复用正式初始化的外部事实替身；核对清理资格、精确写入及未知结果不重放。
use super::{Fixture, Resources, retain_prepared};
use crate::{
    Result,
    recovery::clone::native::{
        json as document,
        target_verify::{
            fresh::{initialize, reconcile, registration::Registration},
            guard::Generation,
            storage::StorageInputs,
        },
    },
};
use serde_json::{Value, json};
use std::fs;

fn prepare(resources: &Resources<'_>) {
    let f = resources.fixture;
    retain_prepared(f);
    resources.preflight_state();
    fs::create_dir(f.target.join("reset-state")).unwrap();
    document::write_new(
        &f.target.join("initialize.started.json"),
        &json!({"status":"initializing"}),
    )
    .unwrap();
    document::write_new(
        &f.target.join("failure.json"),
        &json!({
        "status":"failed","stage":"create_and_reset","at":"2026-10-30T00:00:00Z",
        "error_type":"CalledProcessError","reason":"固定失败",
        "automatic_retry":false,"automatic_resource_cleanup":false,"fresh_target_initialized":false,
        "clone_verified":false,"restore_qualified":false}),
    )
    .unwrap();
    document::write_new(&f.target.join("reset-state/one.report.json"), &json!({
        "status":"failed","failed_phase":"preflight","phases":{
            "preflight":{"status":"failed"},"database":{"status":"pending"},
            "redis":{"status":"pending"},"objects":{"status":"pending"},"release":{"status":"complete"}}})).unwrap();
}

fn execute(resources: &Resources<'_>, drift: bool) -> Result<Value> {
    let f = resources.fixture;
    let registration = Registration::load(&f.backend, &f.local)?;
    let owner = Generation::acquire(&f.backend, &f.target)?;
    let mut environment = f.environment.clone();
    environment.insert("TEMP".into(), f.local.to_str().unwrap().into());
    let protected = initialize::inputs::protect(&registration, &f.request, &environment)?;
    let generation = || {
        let mut value = resources.generation()?;
        if drift {
            value.0["storage"]["unexpected"] = json!(true);
        }
        Ok(value)
    };
    let result = reconcile::cleanup(&initialize::window::Window {
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
        observe_generation: &generation,
    });
    owner.finish(result)
}

#[test]
fn preflight_reconciliation_publishes_plan_then_exact_cleanup_and_compatible_completion() {
    let f = Fixture::for_initialization();
    let resources = Resources::new(&f);
    prepare(&resources);
    let failure = fs::read(f.target.join("failure.json")).unwrap();
    let report = fs::read(f.target.join("reset-state/one.report.json")).unwrap();
    let result = execute(&resources, false).unwrap();
    assert_eq!(result["status"], "preflight_failure_reconciled");
    assert_eq!(result["automatic_retry"], false);
    assert_eq!(result["restore_qualified"], false);
    assert!(resources.created.borrow().is_empty());
    assert_eq!(resources.redis_image(), json!({}));
    assert_eq!(fs::read(f.target.join("failure.json")).unwrap(), failure);
    assert_eq!(
        fs::read(f.target.join("reset-state/one.report.json")).unwrap(),
        report
    );
    assert_eq!(
        document::read(&f.target.join("reconciliation-completed.json")).unwrap(),
        result
    );
    let sql = resources.sql.borrow();
    let dropped = sql
        .iter()
        .filter(|v| v.starts_with("DROP DATABASE"))
        .cloned()
        .collect::<Vec<_>>();
    let mut databases = f.request["target"]["databases"]
        .as_array()
        .unwrap()
        .iter()
        .collect::<Vec<_>>();
    databases.sort_by(|a, b| b["key"].as_str().cmp(&a["key"].as_str()));
    assert_eq!(
        dropped,
        databases
            .iter()
            .map(|db| format!("DROP DATABASE `{}`;", db["database"].as_str().unwrap()))
            .collect::<Vec<_>>()
    );
    drop(sql);
    assert_eq!(
        resources
            .redis_calls
            .borrow()
            .iter()
            .filter(|v| v[0] == "DEL")
            .count(),
        1
    );
    let public = serde_json::to_string(&result).unwrap();
    for secret in [
        "fixture-secret",
        "fixture-redis",
        "fixture-access",
        "Fixture!Admin123",
    ] {
        assert!(!public.contains(secret));
    }
    let writes = resources
        .sql
        .borrow()
        .iter()
        .filter(|v| v.starts_with("DROP DATABASE"))
        .count();
    assert!(execute(&resources, false).is_err());
    assert_eq!(
        resources
            .sql
            .borrow()
            .iter()
            .filter(|v| v.starts_with("DROP DATABASE"))
            .count(),
        writes
    );
}

#[test]
fn preflight_reconciliation_rejects_unsafe_or_ambiguous_before_images_without_writes() {
    for case in [
        "write-phase",
        "release",
        "two-reports",
        "failure",
        "nonempty",
        "objects",
        "sentinel",
        "drift",
        "plan-exists",
    ] {
        let f = Fixture::for_initialization();
        let resources = Resources::new(&f);
        prepare(&resources);
        let report = f.target.join("reset-state/one.report.json");
        match case {
            "write-phase" | "release" => {
                let mut value = document::read(&report).unwrap();
                let phase = if case == "release" {
                    "release"
                } else {
                    "database"
                };
                value["phases"][phase]["status"] = json!("running");
                fs::write(&report, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "two-reports" => {
                fs::copy(&report, f.target.join("reset-state/two.report.json")).unwrap();
            }
            "failure" => {
                let path = f.target.join("failure.json");
                let mut value = document::read(&path).unwrap();
                value["stage"] = json!("inventory");
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "nonempty" | "objects" => resources.fail.set(Some(case)),
            "sentinel" => resources.existing_sentinel(),
            "plan-exists" => {
                document::write_new(
                    &f.target.join("reconciliation-plan.json"),
                    &json!({"unknown":true}),
                )
                .unwrap();
            }
            "drift" => (),
            _ => unreachable!(),
        }
        let failure = fs::read(f.target.join("failure.json")).unwrap();
        assert!(execute(&resources, case == "drift").is_err(), "{case}");
        assert!(
            !f.target.join("reconciliation-completed.json").exists(),
            "{case}"
        );
        assert_eq!(resources.created.borrow().len(), 4, "{case}");
        assert!(
            resources
                .sql
                .borrow()
                .iter()
                .all(|v| !v.starts_with("DROP") && !v.starts_with("CREATE")),
            "{case}"
        );
        assert!(
            resources
                .redis_calls
                .borrow()
                .iter()
                .all(|v| matches!(v[0].as_str(), "SCAN" | "GET")),
            "{case}"
        );
        assert_eq!(fs::read(f.target.join("failure.json")).unwrap(), failure);
    }
}

#[test]
fn preflight_reconciliation_preserves_unknown_drop_and_blocks_replay() {
    let f = Fixture::for_initialization();
    let resources = Resources::new(&f);
    prepare(&resources);
    resources.fail.set(Some("drop"));
    assert!(execute(&resources, false).is_err());
    let plan = fs::read(f.target.join("reconciliation-plan.json")).unwrap();
    assert!(!f.target.join("reconciliation-completed.json").exists());
    assert_eq!(resources.created.borrow().len(), 3);
    assert!(execute(&resources, false).is_err());
    assert_eq!(resources.created.borrow().len(), 3);
    assert_eq!(
        fs::read(f.target.join("reconciliation-plan.json")).unwrap(),
        plan
    );
    assert_eq!(
        resources
            .sql
            .borrow()
            .iter()
            .filter(|v| v.starts_with("DROP DATABASE"))
            .count(),
        1
    );
}

#[test]
fn preflight_reconciliation_unconfirmed_del_keeps_plan_and_prevents_every_drop_or_replay() {
    for reply in ["del-numeric", "del-unconfirmed"] {
        let f = Fixture::for_initialization();
        let resources = Resources::new(&f);
        prepare(&resources);
        let failure = fs::read(f.target.join("failure.json")).unwrap();
        resources.fail.set(Some(reply));
        let error = execute(&resources, false).unwrap_err();
        assert!(error.to_string().contains("DEL 结果未确认"));
        assert!(error.to_string().contains("可能已写入"));
        assert!(error.to_string().contains("禁止重放"));
        let plan = fs::read(f.target.join("reconciliation-plan.json")).unwrap();
        assert!(!f.target.join("reconciliation-completed.json").exists());
        assert_eq!(resources.created.borrow().len(), 4);
        assert_eq!(resources.redis_image(), json!({}));
        assert!(execute(&resources, false).is_err());
        assert_eq!(
            fs::read(f.target.join("reconciliation-plan.json")).unwrap(),
            plan
        );
        assert_eq!(fs::read(f.target.join("failure.json")).unwrap(), failure);
        assert!(!resources.sql.borrow().iter().any(|v| v.starts_with("DROP")));
        assert_eq!(
            resources
                .redis_calls
                .borrow()
                .iter()
                .filter(|v| v[0] == "DEL")
                .count(),
            1
        );
    }
}
