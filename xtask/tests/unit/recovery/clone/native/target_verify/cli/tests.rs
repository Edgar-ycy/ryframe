use super::*;

fn status(phase: &str, details: Value, fixture: Value, controller: Value) -> Value {
    status::project(&json!({"phase":phase,"details":details,"registration":null,"fixture":fixture,"controller":controller})).unwrap()
}

fn migration(mode: &str) -> Value {
    json!({"resumable":true,"mode":mode,"completed":[{"id":"control-up"},{"id":"control-verify"},{"id":"tenant-data-dedicated-a-up"}],
        "next_index":3,"operations":[{"id":"control-up","write":true},{"id":"control-verify","write":false},
            {"id":"tenant-data-dedicated-a-up","write":true},{"id":"tenant-data-dedicated-a-verify","write":false}]})
}

#[test]
fn migration_and_successor_status_preserve_exact_prefix_and_read_only_next_action() {
    for mode in ["migration", "migration-successor"] {
        let result = status(
            "initialize-resume",
            migration(mode),
            Value::Null,
            Value::Null,
        );
        assert_eq!(result["status"], "fresh_target_migration_resume_pending");
        assert_eq!(
            result["last_successful_stage"],
            "migration:tenant-data-dedicated-a-up"
        );
        assert_eq!(result["next_action"], "resume-initialize");
        assert_eq!(
            result["resume"],
            json!({"mode":mode,"completed_operations":["control-up","control-verify","tenant-data-dedicated-a-up"],
            "next_operation":"tenant-data-dedicated-a-verify","next_operation_is_read_only":true})
        );
        assert_eq!(result["restore_qualified"], false);
    }
}

#[test]
fn stopped_fixture_blocks_resume_without_discarding_verified_prefix() {
    let fixture = json!({"available":false,"reason":"夹具服务已正常关闭；须先执行 fixture services restart 再续作 fresh target"});
    let pending = status(
        "initialize-resume",
        migration("migration"),
        Value::Null,
        Value::Null,
    );
    let blocked = status(
        "initialize-resume",
        migration("migration"),
        fixture,
        Value::Null,
    );
    assert_eq!(blocked["resume"], pending["resume"]);
    assert_eq!(blocked["pending_stage"], "service_restart");
    assert_eq!(blocked["next_action"], Value::Null);
    assert_eq!(blocked["evidence_valid"], false);
}

#[test]
fn live_reused_missing_and_unverifiable_controller_never_authorize_replay() {
    for (missing, matches, action) in [
        (false, true, Value::Null),
        (false, false, Value::Null),
        (true, false, json!("recover")),
    ] {
        let control = json!({"process_missing":missing,"process_matches":matches,"run_directory":crate::workspace::root_dir(),
            "owner":{"path":crate::workspace::root_dir().join("owner.json"),"bytes":1,"sha256":"a".repeat(64)}});
        let result = status(
            "prepare-interrupted",
            Value::Null,
            Value::Null,
            control.clone(),
        );
        assert_eq!(result["next_action"], action);
        assert_eq!(result["pending_stage"], "controller");
        assert_eq!(result["controller"], control);
        let invalid = status("local-invalid", Value::Null, Value::Null, control);
        assert_eq!(invalid["next_action"], Value::Null);
        assert_eq!(invalid["evidence_valid"], false);
    }
    let result = status("prepared", Value::Null, Value::Null, json!("invalid"));
    assert_eq!(result["evidence_valid"], false);
    assert_eq!(result["pending_stage"], "reconciliation");
    assert_eq!(result["next_action"], Value::Null);
}

#[test]
fn stage_status_table_preserves_explicit_actions_and_fail_closed_paths() {
    for (phase, current, action) in [
        ("unregistered", "fresh_target_unregistered", Some("prepare")),
        ("prepared", "fresh_creation_prepared", Some("initialize")),
        (
            "prepare-publication",
            "fresh_target_prepare_publication_pending",
            Some("resume-prepare"),
        ),
        (
            "prepare-interrupted",
            "fresh_target_prepare_interrupted",
            Some("resume-prepare"),
        ),
        ("initialized", "fresh_target_initialized", Some("verify")),
    ] {
        let result = status(phase, Value::Null, Value::Null, Value::Null);
        assert_eq!(result["status"], current);
        assert_eq!(result["next_action"], json!(action));
        assert_eq!(result["evidence_valid"], true);
    }
    for phase in [
        "incomplete",
        "registration-invalid",
        "request-invalid",
        "fixture-invalid",
        "tree-invalid",
        "local-invalid",
        "initialized-failure",
    ] {
        let result = status(phase, Value::Null, Value::Null, Value::Null);
        assert_eq!(result["status"], "fresh_target_needs_reconciliation");
        assert_eq!(result["evidence_valid"], false);
        assert_eq!(result["next_action"], Value::Null);
    }
}

#[test]
fn dispatch_validates_all_stage_arguments_and_explicit_write_before_any_io() {
    for (operation, required) in [
        ("prepare", [true, true, true, false]),
        ("verify", [false, false, false, true]),
        ("initialize", [false; 4]),
        ("resume-prepare", [false; 4]),
        ("resume-initialize", [false; 4]),
        ("reconcile-preflight", [false; 4]),
        ("status", [false; 4]),
    ] {
        let write = operation != "status";
        let valid = json!({"operation":operation,"write":write,"supplied":required});
        assert_eq!(dispatch(&valid).unwrap()["operation"], operation);
        let mut denied = valid.clone();
        denied["write"] = json!(!write);
        assert!(dispatch(&denied).is_err());
        for index in 0..4 {
            let mut denied = valid.clone();
            denied["supplied"][index] = json!(!required[index]);
            assert!(dispatch(&denied).is_err());
        }
    }
    for value in [
        json!({"operation":"unknown","write":true,"supplied":[false,false,false,false]}),
        json!({"operation":"initialize","write":1,"supplied":[false,false,false,false]}),
        json!({"operation":"initialize","write":true,"supplied":[0,0,0,0]}),
    ] {
        assert!(dispatch(&value).is_err());
    }
}

#[test]
fn only_completed_target_storage_stages_can_be_appended() {
    let allowed = json!({"attempts":[{"stage":"storage-target","mode":"restart","status":"succeeded"},
        {"stage":"cache-target","mode":"reconcile","status":"succeeded"}]});
    assert_eq!(
        storage_append(&allowed).unwrap(),
        json!({"storage":true,"cache":true})
    );
    for item in [
        json!({"stage":"storage-source","mode":"restart","status":"succeeded"}),
        json!({"stage":"runtime-target","mode":"start","status":"succeeded"}),
        json!({"stage":"cache-target","mode":"restart","status":"running"}),
        json!({"stage":"storage-target","mode":"start","status":"failed"}),
    ] {
        assert!(storage_append(&json!({"attempts":[item]})).is_err());
    }
}
