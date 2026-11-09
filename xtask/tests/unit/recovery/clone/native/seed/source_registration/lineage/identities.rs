use super::Identities;
use serde_json::{Value, json};

fn identity(pid: u64, started: &str, executable: &str) -> Value {
    json!({"pid":pid,"started":started,
        "executable":crate::workspace::root_dir().join(executable)})
}

#[test]
fn complete_occurrences_merge_once_and_keep_numeric_creation_and_authoritative_runtime_order() {
    let mut records = Identities::default();
    let older = identity(20, "9", "older.exe");
    let newer = identity(20, "10", "newer.exe");
    let first = identity(3, "12345678901234567890123456789012", "first.exe");
    let root = crate::workspace::root_dir().join(".local-tests/Straße-中文");
    records.producer(&newer, 11, "existing").unwrap();
    records
        .runtime(&older, &root.join("seed-runtime/runtime"), 2, "worker", 9)
        .unwrap();
    records
        .runtime(&older, &root.join("post-copy/runtime"), 0, "worker", 3)
        .unwrap();
    records
        .runtime(&older, &root.join("post-copy/runtime"), 0, "api", 2)
        .unwrap();
    records.producer(&older, 12, "session").unwrap();
    records.producer(&older, 2, "identity-verify").unwrap();
    records.producer(&older, 2, "identity-verify").unwrap();
    records.producer(&first, 1, "existing").unwrap();
    let result = records.into_value();
    assert_eq!(result.as_array().unwrap().len(), 3);
    assert_eq!(result[0]["identity"], first);
    assert_eq!(result[1]["identity"], older);
    assert_eq!(result[2]["identity"], newer);
    assert_eq!(
        result[1]["occurrences"],
        json!([
            {"kind":"node","attempt":2,"producer_kind":"identity-verify"},
            {"kind":"node","attempt":12,"producer_kind":"session"},
            {"kind":"runtime","runtime_directory":root.join("post-copy/runtime"),"role":"api","sequence":2},
            {"kind":"runtime","runtime_directory":root.join("post-copy/runtime"),"role":"worker","sequence":3},
            {"kind":"runtime","runtime_directory":root.join("seed-runtime/runtime"),"role":"worker","sequence":9}
        ])
    );
}

#[test]
fn conflicting_executables_and_occurrence_locations_cannot_hide_behind_equal_keys() {
    let mut records = Identities::default();
    let original = identity(20, "10", "original.exe");
    records.producer(&original, 1, "existing").unwrap();
    assert!(
        records
            .producer(&identity(20, "10", "other.exe"), 2, "session")
            .is_err()
    );
    let root = crate::workspace::root_dir();
    records
        .runtime(&original, &root.join("one"), 0, "api", 1)
        .unwrap();
    assert!(
        records
            .runtime(&original, &root.join("two"), 0, "api", 1)
            .is_err()
    );
    assert_eq!(
        records.into_value()[0]["occurrences"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn malformed_creation_fields_are_rejected_without_rounding_or_truncating() {
    let original = identity(20, "10", "original.exe");
    for (field, value) in [
        ("pid", json!(true)),
        ("pid", json!(1)),
        ("started", json!("01")),
        ("started", json!("1２")),
        ("started", json!("1".repeat(33))),
        ("executable", json!("relative.exe")),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        assert!(
            Identities::default()
                .producer(&changed, 1, "existing")
                .is_err()
        );
    }
}
