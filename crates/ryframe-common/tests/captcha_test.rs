use ryframe_common::utils::captcha::{CaptchaType, generate_captcha};

#[test]
fn test_generate_captcha_both_types() {
    let (answer, image) = generate_captcha(CaptchaType::Alphanumeric)
        .unwrap()
        .into_parts();
    assert_eq!(answer.len(), 4);
    assert_eq!(&image[0..4], &[137, 80, 78, 71]);

    let (answer, image) = generate_captcha(CaptchaType::Math).unwrap().into_parts();
    assert!(answer.parse::<i64>().is_ok());
    assert_eq!(&image[0..4], &[137, 80, 78, 71]);
}
