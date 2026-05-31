use ryframe_auth::password;

#[test]
fn test_hash_and_verify() {
    let hashed = password::hash("my_password").unwrap();
    assert!(password::verify("my_password", &hashed).unwrap());
    assert!(!password::verify("wrong_password", &hashed).unwrap());
}
