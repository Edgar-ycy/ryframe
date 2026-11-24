use super::*;
use serde_json::json;

fn database(resource: char, tables: char) -> Value {
    json!({"kind":"database","resource_sha256":resource.to_string().repeat(64),"schema_sha256":"a".repeat(64),"tables_sha256":tables.to_string().repeat(64),"preserved_sha256":"b".repeat(64)})
}
fn object(exists: bool) -> Value {
    json!({"kind":"object","resource_sha256":"f".repeat(64),"exists":exists,"bytes":if exists {json!(42)}else{Value::Null},
        "sha256":if exists {json!("a".repeat(64))}else{Value::Null},"metadata_sha256":if exists {json!("b".repeat(64))}else{Value::Null}})
}
fn created() -> Value {
    json!({"type":"created","plan_sha256":"a".repeat(64),"generation_sha256":"b".repeat(64)})
}
fn opened(id: &str, mode: &str) -> Value {
    json!({"type":"opened","session":id,"mode":mode})
}
fn intent(id: &str, before: &Value, after: &Value) -> Value {
    json!({"type":"intent","step_id":id,"before":before,"after":after,"artifact_sha256":"c".repeat(64)})
}
fn observation(kind: &str, id: &str, observed: &Value) -> Value {
    json!({"type":kind,"step_id":id,"observed":observed})
}
fn closed(outcome: &str) -> Value {
    json!({"type":"closed","outcome":outcome,"completed_at":null})
}

#[test]
fn unknown_reconcile_before_resume_and_confirm_preserve_full_step() {
    let before = database('c', 'd');
    let after = database('c', 'e');
    let intent = intent("copy-one", &before, &after);
    let mut events = vec![created(), opened("session-one", "apply"), intent.clone()];
    assert_eq!(
        project(&events).unwrap().steps["copy-one"]["phase"],
        "unknown"
    );
    events.extend([
        opened("session-two", "reconcile"),
        observation("reconciled", "copy-one", &before),
        closed("partial"),
    ]);
    let state = project(&events).unwrap();
    assert_eq!(state.steps["copy-one"]["phase"], "reconciled_before");
    assert_eq!(
        state.session.unwrap().touched,
        BTreeSet::from(["copy-one".to_owned()])
    );
    events.extend([
        opened("session-three", "resume"),
        observation("resumed", "copy-one", &before),
    ]);
    let state = project(&events).unwrap();
    assert_eq!(state.steps["copy-one"]["phase"], "unknown");
    assert_eq!(state.steps["copy-one"]["attempt"], 2);
    events.push(observation("confirmed", "copy-one", &after));
    let state = project(&events).unwrap();
    let mut expected = intent;
    expected["phase"] = json!("confirmed");
    expected["attempt"] = json!(2);
    expected["session"] = json!("session-three");
    assert_eq!(state.steps["copy-one"], expected);
}

#[test]
fn independent_reconciliation_never_authorizes_implicit_write_or_confirmation() {
    let before = database('c', 'd');
    let after = database('c', 'e');
    let base = vec![
        created(),
        opened("session-one", "apply"),
        intent("copy-one", &before, &after),
        opened("session-two", "reconcile"),
    ];
    for event in [
        intent("copy-two", &object(false), &object(true)),
        observation("confirmed", "copy-one", &after),
        observation("resumed", "copy-one", &before),
    ] {
        let mut events = base.clone();
        events.push(event);
        assert!(project(&events).is_err());
    }
    let mut matched = base.clone();
    matched.push(observation("reconciled", "copy-one", &after));
    let state = project(&matched).unwrap();
    assert_eq!(state.steps["copy-one"]["phase"], "confirmed");
    assert_eq!(state.steps["copy-one"]["attempt"], 1);
    let mut events = base;
    events.push(observation("reconciled", "copy-one", &before));
    let before_reconciled = project(&events).unwrap();
    assert_eq!(before_reconciled.steps["copy-one"]["attempt"], 1);
    events.push(observation("reconciled", "copy-one", &after));
    assert!(project(&events).is_err());
}

#[test]
fn unknown_steps_physical_conflicts_and_foreign_confirmations_are_rejected() {
    let before = database('c', 'd');
    let after = database('c', 'e');
    let mut base = vec![
        created(),
        opened("session-one", "apply"),
        intent("copy-one", &before, &after),
    ];
    let mut next = base.clone();
    next.push(intent("copy-two", &object(false), &object(true)));
    assert!(project(&next).is_err());
    let mut wrong = base.clone();
    wrong.push(observation("confirmed", "copy-one", &before));
    assert!(project(&wrong).is_err());
    let mut foreign = base.clone();
    foreign.extend([
        closed("partial"),
        opened("session-two", "apply"),
        observation("confirmed", "copy-one", &after),
    ]);
    assert!(project(&foreign).is_err());
    base.push(observation("confirmed", "copy-one", &after));
    base.push(intent("copy-two", &before, &after));
    assert!(project(&base).is_err());
}

#[test]
fn partial_sealed_and_unfinished_sessions_keep_their_boundaries() {
    for outcome in ["open", "failed", "publishing", "partial"] {
        let mut base = vec![created(), opened("session-one", "apply")];
        if outcome == "publishing" {
            base.push(json!({"type":"closed","outcome":"publishing","completed_at":"2026-10-24T08:00:00+08:00"}));
        } else if outcome != "open" {
            base.push(closed(outcome));
        }
        for mode in ["apply", "resume", "reconcile"] {
            let mut events = base.clone();
            events.push(opened("session-two", mode));
            assert_eq!(
                project(&events).is_ok(),
                outcome == "partial" || mode == "reconcile",
                "{outcome}/{mode}"
            );
        }
        base.push(opened("session-one", "reconcile"));
        assert!(project(&base).is_err());
    }
}

#[test]
fn exact_images_prevent_overwrite_ambiguous_or_changed_database_identity() {
    let before = database('c', 'd');
    let after = database('c', 'e');
    assert!(image_pair(&before, &after).is_ok());
    for field in ["schema_sha256", "preserved_sha256", "resource_sha256"] {
        let mut wrong = after.clone();
        wrong[field] = json!("f".repeat(64));
        assert!(image_pair(&before, &wrong).is_err());
    }
    assert!(image_pair(&before, &before).is_err());
    assert!(image_pair(&object(false), &object(true)).is_ok());
    let mut overwritten = object(true);
    overwritten["sha256"] = json!("c".repeat(64));
    assert!(image_pair(&object(true), &overwritten).is_err());
    for (field, value) in [
        ("exists", json!(0)),
        ("bytes", json!(true)),
        ("sha256", Value::Null),
    ] {
        let mut wrong = object(true);
        wrong[field] = value;
        assert!(image(&wrong).is_err());
    }
    let mut absent = object(false);
    absent["bytes"] = json!(0);
    assert!(image(&absent).is_err());
    let mut extra = after;
    extra["unexpected"] = json!(true);
    assert!(image(&extra).is_err());
}

#[test]
fn unknown_missing_extra_events_and_invalid_completion_times_are_rejected() {
    assert!(project(&[opened("session-one", "apply")]).is_err());
    assert!(project(&[created(), json!({"type":"future-event"})]).is_err());
    assert!(project(&[created(), json!({"type":"fault","stage":"lock_release"})]).is_err());
    let base = vec![created(), opened("session-one", "apply")];
    for event in [
        json!({"type":"opened","session":"session-two","mode":"reconcile","extra":true}),
        json!({"type":"intent","step_id":"copy-one"}),
        json!({"type":"fault","stage":"unknown"}),
        json!({"type":"closed","outcome":"publishing","completed_at":"2026-10-24T08:00:00"}),
        json!({"type":"closed","outcome":"partial","completed_at":"2026-10-24T08:00:00Z"}),
    ] {
        let mut events = base.clone();
        events.push(event);
        assert!(project(&events).is_err());
    }
    for time in [
        "2026-10-24T08:00:00.123456+08:00",
        "20261024 080000Z",
        "2026-W43-6T08:00:00+00:00",
        "2026-W43-080000+00:00",
    ] {
        let mut events = base.clone();
        events.push(json!({"type":"closed","outcome":"publishing","completed_at":time}));
        let state = project(&events).unwrap();
        assert_eq!(state.session.unwrap().completed_at, time);
    }
    let mut fault = base;
    fault.extend([
        closed("partial"),
        json!({"type":"fault","stage":"receipt_write"}),
    ]);
    assert_eq!(project(&fault).unwrap().session.unwrap().outcome, "failed");
}

#[test]
fn completion_time_preserves_calendar_boundaries() {
    let project_completion = |completed_at| {
        project(&[
            created(),
            opened("session-one", "apply"),
            json!({"type":"closed","outcome":"publishing","completed_at":completed_at}),
        ])
    };
    for completed_at in [
        "0001-W01-1T00:00:00Z",
        "2019-W01-1T00:00:00Z",
        "2020-W53-7T23:59:59-23:59:59",
        "2020W537T235959Z",
        "9999-W52-5T23:59:59Z",
        "20000229T000000Z",
    ] {
        let state = project_completion(completed_at).unwrap();
        assert_eq!(state.session.unwrap().completed_at, completed_at);
    }
    for (completed_at, reason) in [
        ("0000-W01-1T00:00:00Z", "账本完成日期无效"),
        ("9999-W52-6T00:00:00Z", "账本完成日期无效"),
        ("2021-W53-1T00:00:00Z", "账本完成日期无效"),
        ("2020W538T000000Z", "账本完成日期无效"),
        ("1900-02-29T00:00:00Z", "账本完成日期无效"),
        ("2000-00-01T00:00:00Z", "账本完成日期无效"),
        ("2000-02-29T23:59:60Z", "账本完成时间无效"),
        ("2000-02-29T00:00:00+24:00:00", "账本完成时区无效"),
    ] {
        let error = project_completion(completed_at).unwrap_err();
        assert_eq!(error.to_string(), reason, "{completed_at}");
    }
}

#[test]
fn reconciliation_mismatch_blocks_other_resume_and_attempt_count_is_checked() {
    let one = database('c', 'd');
    let one_after = database('c', 'e');
    let two = object(false);
    let two_after = object(true);
    let mut events = vec![
        created(),
        opened("session-one", "apply"),
        intent("copy-one", &one, &one_after),
        observation("confirmed", "copy-one", &one_after),
        intent("copy-two", &two, &two_after),
        opened("session-two", "reconcile"),
        observation("reconciled", "copy-one", &one),
        observation("reconciled", "copy-two", &database('f', 'd')),
        closed("partial"),
        opened("session-three", "resume"),
    ];
    events.push(observation("resumed", "copy-one", &one));
    assert!(project(&events).is_err());
    let mut state = project(&[
        created(),
        opened("session-one", "apply"),
        intent("copy-one", &one, &one_after),
        opened("session-two", "reconcile"),
        observation("reconciled", "copy-one", &one),
        closed("partial"),
        opened("session-three", "resume"),
    ])
    .unwrap();
    state.steps["copy-one"]["attempt"] = json!(u64::MAX);
    assert!(step::apply(&mut state, &observation("resumed", "copy-one", &one)).is_err());
}
