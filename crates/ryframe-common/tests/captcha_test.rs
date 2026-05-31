use ryframe_common::utils::captcha::{CaptchaType, generate_captcha, generate_math, generate_text};

#[test]
fn test_generate_text_and_math() {
    let text = generate_text(4);
    assert_eq!(text.len(), 4);
    for ch in text.chars() {
        assert!("ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(ch));
    }

    let (question, answer) = generate_math();
    assert!(question.contains("=?"));
    assert!(answer.parse::<i64>().is_ok());
}

#[test]
fn test_generate_captcha_both_types() {
    let alpha = generate_captcha(CaptchaType::Alphanumeric).unwrap();
    assert_eq!(alpha.answer.len(), 4);
    assert_eq!(&alpha.image_data[0..4], &[137, 80, 78, 71]);

    let math = generate_captcha(CaptchaType::Math).unwrap();
    assert!(math.answer.parse::<i64>().is_ok());
    assert_eq!(&math.image_data[0..4], &[137, 80, 78, 71]);
}
