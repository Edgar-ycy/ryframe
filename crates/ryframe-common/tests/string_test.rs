use ryframe_common::utils::string::{random_string, to_camel_case, to_pascal_case, to_snake_case};

#[test]
fn test_to_snake_case() {
    assert_eq!(to_snake_case("UserName"), "user_name");
    assert_eq!(to_snake_case("HTTPResponse"), "http_response");
    assert_eq!(to_snake_case("already_snake"), "already_snake");
    assert_eq!(to_snake_case(""), "");
    assert_eq!(to_snake_case("A"), "a");
}

#[test]
fn test_to_camel_case() {
    assert_eq!(to_camel_case("user_name"), "userName");
    assert_eq!(to_camel_case("hello_world_test"), "helloWorldTest");
    assert_eq!(to_camel_case(""), "");
    assert_eq!(to_camel_case("already"), "already");
}

#[test]
fn test_to_pascal_case() {
    assert_eq!(to_pascal_case("user_name"), "UserName");
    assert_eq!(to_pascal_case("hello"), "Hello");
    assert_eq!(to_pascal_case(""), "");
}

#[test]
fn test_random_string() {
    let s = random_string(10);
    assert_eq!(s.len(), 10);
    assert!(s.chars().all(|c| c.is_alphanumeric()));

    let s2 = random_string(0);
    assert!(s2.is_empty());

    let a = random_string(20);
    let b = random_string(20);
    assert_ne!(a, b);
}
