use std::collections::HashSet;

use ryframe_common::utils::snowflake::{Snowflake, next_snowflake_id};

#[test]
fn test_snowflake_creation_and_validation() {
    assert!(Snowflake::new(0).is_ok());
    assert!(Snowflake::new(1023).is_ok());
    assert!(Snowflake::new(-1).is_err());
    assert!(Snowflake::new(1024).is_err());
}

#[test]
fn test_snowflake_uniqueness_and_extraction() {
    let sf = Snowflake::new(42).unwrap();
    let mut ids = HashSet::new();
    for _ in 0..10000 {
        let id = sf.next_id();
        assert!(id > 0);
        assert!(ids.insert(id), "重复ID: {}", id);
    }

    let id = sf.next_id();
    assert_eq!(Snowflake::extract_worker_id(id), 42);

    let ts = Snowflake::extract_timestamp(id);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!((ts - now).abs() < 10000);

    let sf2 = Snowflake::new(1).unwrap();
    assert_ne!(sf.next_id(), sf2.next_id());

    assert_ne!(next_snowflake_id(), next_snowflake_id());
}
