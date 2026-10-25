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
fn writing_stage_protocol_requires_a_positive_attempt() {
    let root = crate::workspace::root_dir();
    assert!(stage::protocol(&request(&root), "target-authority").is_err());
    assert!(stage::protocol(&request(&root), "unknown").is_err());
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
