use ryframe_generator::naming::{to_camel_case, to_pascal_case, to_snake_case};

#[test]
fn test_naming_conversions() {
    assert_eq!(to_snake_case("UserName"), "user_name");
    assert_eq!(to_pascal_case("user_name"), "UserName");
    assert_eq!(to_camel_case("user_name"), "userName");
    assert_eq!(to_camel_case("UserName"), "userName");
}
