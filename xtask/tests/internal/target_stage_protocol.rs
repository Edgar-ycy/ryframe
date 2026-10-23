use std::path::Path;

use serde_json::Value;

use crate::recovery::clone::native::lifecycle::stage::{self, StageRequest};

fn request(root: &Path) -> StageRequest<'_> {
    StageRequest {
        backend: root,
        run_dir: root,
        stage: "target-verify",
        mode: "run",
        attempt: None,
    }
}

#[test]
fn authority_protocol_never_claims_a_writing_attempt() {
    let root = crate::workspace::root_dir();
    let value: Value =
        serde_json::from_str(&stage::protocol(&request(&root), "target-authority").unwrap())
            .unwrap();
    assert_eq!(value["request"]["operation"], "target-authority");
    assert_eq!(value.as_object().unwrap().len(), 4);
    assert_eq!(
        value["native_tool"],
        crate::recovery::clone::native_tool_binding().unwrap()
    );
    assert_eq!(value["request"]["write"], false);
    assert!(value["request"]["attempt"].is_null());
}

#[test]
fn authority_protocol_rejects_other_stages_modes_and_attempts() {
    let root = crate::workspace::root_dir();
    for (name, mode, attempt) in [
        ("export", "run", None),
        ("copy", "run", None),
        ("target-verify", "resume", None),
        ("target-verify", "reconcile", None),
        ("target-verify", "run", Some(1)),
        ("target-verify", "run", Some(0)),
    ] {
        let input = StageRequest {
            stage: name,
            mode,
            attempt,
            ..request(&root)
        };
        assert!(stage::protocol(&input, "target-authority").is_err());
    }
    assert!(stage::protocol(&request(&root), "unknown").is_err());
}

#[test]
fn writing_stage_protocol_requires_a_positive_attempt() {
    let root = crate::workspace::root_dir();
    assert!(stage::protocol(&request(&root), "run").is_err());
    assert!(
        stage::protocol(
            &StageRequest {
                attempt: Some(0),
                ..request(&root)
            },
            "run"
        )
        .is_err()
    );
    let input = StageRequest {
        attempt: Some(1),
        ..request(&root)
    };
    let value: Value = serde_json::from_str(&stage::protocol(&input, "run").unwrap()).unwrap();
    assert_eq!(value["request"]["attempt"], 1);
    assert_eq!(value["request"]["write"], true);
    assert!(stage::protocol(&input, "preflight").is_err());
}
