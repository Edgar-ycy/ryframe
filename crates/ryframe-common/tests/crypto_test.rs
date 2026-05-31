use ryframe_common::utils::crypto::*;

#[test]
fn test_md5_and_base64() {
    assert_eq!(md5_hex("hello"), "5d41402abc4b2a76b9719d911017c592");
    assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");

    let encoded = base64_encode("Hello, World!");
    assert_eq!(base64_decode(&encoded).unwrap(), "Hello, World!");
    assert!(base64_decode("!!!invalid!!!").is_none());
}

#[test]
fn test_uuid() {
    let simple = uuid_v4_simple();
    assert_eq!(simple.len(), 32);
    assert!(!simple.contains('-'));

    let standard = uuid_v4();
    assert_eq!(standard.len(), 36);
    assert_ne!(uuid_v4(), uuid_v4());
}
