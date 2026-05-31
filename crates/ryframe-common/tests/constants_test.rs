use ryframe_common::*;

#[test]
fn test_constants_valid() {
    const { assert!(DEFAULT_PAGE_SIZE > 0 && DEFAULT_PAGE_SIZE <= MAX_PAGE_SIZE) };
    assert!(CAPTCHA_KEY_PREFIX.starts_with(CACHE_KEY_PREFIX));
    assert!(TOKEN_PREFIX.ends_with(' '));
}
