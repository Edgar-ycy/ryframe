use super::*;

#[test]
fn pair_uses_canonical_order_and_rejects_ambiguous_roles() -> Result<()> {
    assert_eq!(
        ordered(&[Role::Worker, Role::Api])?,
        &[Role::Api, Role::Worker]
    );
    assert_eq!(ordered(&[Role::Worker])?, &[Role::Worker]);
    assert_eq!(ordered(&[Role::Api])?, &[Role::Api]);
    for invalid in [
        vec![],
        vec![Role::Worker, Role::Worker],
        vec![Role::Frontend],
    ] {
        assert!(ordered(&invalid).is_err());
    }
    assert!(
        distinct_endpoints(
            "http://127.0.0.1:18080/readyz",
            "http://127.0.0.1:18080/readyz"
        )
        .is_err()
    );
    distinct_endpoints(
        "http://127.0.0.1:18080/readyz",
        "http://127.0.0.1:18081/readyz",
    )
}

#[test]
fn stopped_and_unready_results_keep_the_original_control_shape() {
    let runtime = Path::new("fixture");
    let processes = json!({
        "api":{"state":"running","identity":{"fixture":"api"},"ready":false},
        "worker":{"state":"stopped","identity":null,"ready":false},
    });
    let receipt = summary(
        "status",
        &json!({"scope_id":"fixture"}),
        runtime,
        processes.clone(),
        None,
    );
    assert_eq!(
        receipt,
        json!({"format_version":1,"kind":"devex-clone-runtime-control",
        "operation":"status","scope_id":"fixture","runtime_directory":runtime,
        "processes":processes,"producer_history":null})
    );
}

#[test]
fn only_current_started_identity_selects_original_role_token() -> Result<()> {
    let old = json!({"fixture":"old"});
    let current = json!({"fixture":"current"});
    let mut history = json!({"events":[
        {"event":"started","role":"api","token":"older","identity":old},
        {"event":"started","role":"api","token":"current","identity":current},
        {"event":"started","role":"worker","token":"worker","identity":{"fixture":"worker"}}
    ]});
    assert_eq!(
        process::active_token(Some(&history), Role::Api, &current)?,
        "current"
    );
    assert!(process::active_token(Some(&history), Role::Api, &old).is_err());
    assert!(process::active_token(None, Role::Api, &current).is_err());
    history["events"]
        .as_array_mut()
        .unwrap()
        .push(json!({"event":"stopped","role":"api","identity":current}));
    assert!(process::active_token(Some(&history), Role::Api, &current).is_err());
    Ok(())
}
