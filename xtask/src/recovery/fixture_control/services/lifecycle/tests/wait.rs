use super::*;

#[test]
fn reused_pid_counts_as_original_exit_and_no_signal_is_sent() {
    let mut runtime = FakeRuntime::new();
    runtime.observation = expected_stopped();
    runtime.identities = VecDeque::from(["reused", "missing"]);
    let services = runtime.fixture.services.clone();
    assert_eq!(
        close::closed(&mut runtime, &services).unwrap(),
        expected_stopped()
    );
    assert_eq!(runtime.pauses, 0);
    assert!(!runtime.events.iter().any(|event| event == "tree-stop"));
}

#[test]
fn transient_identity_denial_is_retried_with_one_shared_deadline() {
    let mut runtime = FakeRuntime::new();
    runtime.observation = expected_stopped();
    runtime.identities = VecDeque::from(["denied", "missing", "missing", "missing"]);
    let services = runtime.fixture.services.clone();
    assert!(close::closed(&mut runtime, &services).is_ok());
    assert_eq!(runtime.pauses, 1);
    runtime.identities = std::iter::repeat_n("denied", 300).collect();
    let before = runtime.now();
    assert!(close::closed(&mut runtime, &services).is_err());
    assert_eq!(runtime.now() - before, Duration::from_secs(5));
}

#[test]
fn other_identity_errors_and_observation_denial_are_never_swallowed() {
    let mut runtime = FakeRuntime::new();
    runtime.observation = expected_stopped();
    runtime.identities = VecDeque::from(["other-error"]);
    let services = runtime.fixture.services.clone();
    assert!(close::closed(&mut runtime, &services).is_err());
    assert_eq!(runtime.pauses, 0);
    runtime.fail = Some("observe".into());
    assert!(close::closed(&mut runtime, &services).is_err());
    assert_eq!(runtime.pauses, 0);
}

#[test]
fn closed_identity_with_running_members_does_not_gain_shutdown_proof() {
    let mut runtime = FakeRuntime::new();
    let services = runtime.fixture.services.clone();
    assert!(close::closed(&mut runtime, &services).is_err());
}

#[test]
fn malformed_private_requests_do_not_echo_credentials_or_start_resources() {
    for input in [br#"{"format_version":1,"format_version":2,"password":"private-marker"}"#.to_vec(),
        serde_json::to_vec(&json!({"format_version":1,"backend":"private-marker","operation":"restart","review":"private-marker","environment":"private-marker","owner":null,"write":false})).unwrap(),
        vec![b'X';4*1024*1024+1]] {
        let (code,response)=protocol::execute(&input);assert_eq!(code,2);assert!(!response.to_string().contains("private-marker"));
    }
}
