use ryframe_auth::password;

#[test]
fn test_hash_and_verify() {
    let hashed = password::hash("my_password").unwrap();
    assert!(password::verify("my_password", &hashed).unwrap());
    assert!(!password::verify("wrong_password", &hashed).unwrap());
}

#[test]
fn dummy_verification_is_valid_and_never_matches_an_arbitrary_password() {
    password::warm_dummy_hash();
    assert!(!password::verify_or_dummy("not-the-dummy-secret", None).unwrap());
}
