use serde_json::json;

use super::super::execute;

#[test]
fn inventory_dispatch_keeps_strict_bounded_read_only_source_protocol() {
    let request = json!({
        "operation": "source-inventory",
        "write": false,
        "arguments": {
            "action": "fingerprints",
            "files": [{"path": "xtask/src/main.rs", "sha256": "a".repeat(64)}]
        }
    });
    assert_eq!(
        execute(&serde_json::to_vec(&request).unwrap()).unwrap()["product"]["files"],
        1
    );
    let mut changed = request.clone();
    changed["write"] = json!(true);
    assert!(execute(&serde_json::to_vec(&changed).unwrap()).is_err());
    let mut changed = request;
    changed["arguments"]["extra"] = json!(true);
    assert!(execute(&serde_json::to_vec(&changed).unwrap()).is_err());
    let duplicate_files = br#"{
        "operation": "source-inventory",
        "write": false,
        "arguments": {"action": "fingerprints", "files": [], "files": []}
    }"#;
    assert!(execute(duplicate_files).is_err());
    assert!(execute(&vec![b' '; 16 * 1024 * 1024 + 1]).is_err());
}
