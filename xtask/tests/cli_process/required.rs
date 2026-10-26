use super::invoke;

#[test]
fn required_file_rejects_ambiguous_malformed_or_unbounded_input_without_echo() {
    let directory = super::temporary_directory();
    let file = directory.path().join("required.json");
    let sentinel = "PRIVATE-REQUIRED-SENTINEL";
    let samples = [
        b"".to_vec(),
        vec![0xff],
        vec![b' '; 1024 * 1024 + 1],
        br#"{"event":"push","needs":{}}"#.to_vec(),
        br#"{"event":"push","action":null,"needs":{},"extra":1}"#.to_vec(),
        br#"{"event":"push","event":"push","action":null,"needs":{}}"#.to_vec(),
        br#"{"event":"push","action":null,"action":null,"needs":{}}"#.to_vec(),
        br#"{"event":"push","action":null,"needs":{"plan":{"result":"success"},"plan":{"result":"success"}}}"#.to_vec(),
        br#"{"event":"push","action":null,"needs":{"plan":{"result":"success","outputs":{"x":"a","x":"b"}}}}"#.to_vec(),
        br#"{"event":"push","action":null,"needs":{"plan":{"result":"success","extra":1}}}"#.to_vec(),
        br#"{"event":"push","action":false,"needs":{}}"#.to_vec(),
        format!(r#"{{"event":"push","action":null,"needs":{{"plan":{{"result":"{sentinel}"}}}}}}"#).into_bytes(),
        format!(r#"{{"event":"push","action":null,"needs":{{}},"{sentinel}":1}}"#).into_bytes(),
    ];
    for bytes in samples {
        std::fs::write(&file, bytes).unwrap();
        let result = invoke(&["check", "ci", "required", "--input", file.to_str().unwrap()]);
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&result.stderr).contains(sentinel));
    }
    for arguments in [
        vec!["check", "ci", "required", "--input", "relative.json"],
        vec![
            "check",
            "ci",
            "required",
            "--input",
            file.to_str().unwrap(),
            "--event",
            "push",
        ],
        vec![
            "check",
            "ci",
            "required",
            "--input",
            file.to_str().unwrap(),
            "--input",
            file.to_str().unwrap(),
        ],
    ] {
        let result = invoke(&arguments);
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
    }
    std::fs::remove_file(&file).unwrap();
    let missing = invoke(&["check", "ci", "required", "--input", file.to_str().unwrap()]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
}
