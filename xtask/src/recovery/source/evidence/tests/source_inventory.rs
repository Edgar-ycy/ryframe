use serde_json::json;

use super::super::execute;

#[test]
fn inventory_dispatch_keeps_strict_bounded_read_only_source_protocol() {
    let request = json!({"operation":"source-inventory","write":false,
        "arguments":{"action":"domain","path":"xtask/src/main.rs"}});
    assert_eq!(
        execute(&serde_json::to_vec(&request).unwrap()).unwrap(),
        "product"
    );
    let mut changed = request.clone();
    changed["write"] = json!(true);
    assert!(execute(&serde_json::to_vec(&changed).unwrap()).is_err());
    let mut changed = request;
    changed["arguments"]["extra"] = json!(true);
    assert!(execute(&serde_json::to_vec(&changed).unwrap()).is_err());
    assert!(execute(br#"{"operation":"source-inventory","write":false,"arguments":{"action":"domain","path":"a","path":"b"}}"#).is_err());
    assert!(execute(&vec![b' '; 16 * 1024 * 1024 + 1]).is_err());
}
