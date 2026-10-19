use std::{fs, panic::catch_unwind};

use super::Fixture;

#[test]
fn failed_fixture_keeps_complete_evidence_without_a_second_panic() {
    let fixture = Fixture::new("preserve-panic");
    let directory = fixture.directory.clone();
    let evidence = directory.join("original.bin");
    fs::write(&evidence, b"original\0failure\n").unwrap();
    let result = catch_unwind(move || {
        let _fixture = fixture;
        panic!("注入监控测试失败");
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&evidence).unwrap(), b"original\0failure\n");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn successful_fixture_still_cleans_its_owned_directory() {
    let fixture = Fixture::new("cleanup-success");
    let directory = fixture.directory.clone();
    fs::write(directory.join("owned.bin"), b"owned").unwrap();
    drop(fixture);
    assert!(!directory.exists());
}
