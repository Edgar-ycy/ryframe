use rand::{Rng, RngExt};

use super::CaptchaType;

// 移除在低分辨率字形下容易混淆的 B/8、S/5、G/6、Z/2 等字符。
pub(super) const ALPHANUMERIC_ALPHABET: &[u8] = b"ACDEFHJKLMNPQRTUVWXY3479";
const ALPHANUMERIC_LENGTH: usize = 4;

pub(super) struct Challenge {
    pub display: String,
    pub answer: String,
}

pub(super) fn generate_challenge(captcha_type: CaptchaType) -> Challenge {
    let mut rng = rand::rng();
    generate_challenge_with(&mut rng, captcha_type)
}

fn generate_challenge_with(rng: &mut impl Rng, captcha_type: CaptchaType) -> Challenge {
    match captcha_type {
        CaptchaType::Alphanumeric => {
            let answer = generate_text_with(rng, ALPHANUMERIC_LENGTH);
            Challenge {
                display: answer.clone(),
                answer,
            }
        }
        CaptchaType::Math => generate_math_with(rng),
    }
}

fn generate_text_with(rng: &mut impl Rng, length: usize) -> String {
    (0..length)
        .map(|_| {
            let index = rng.random_range(0..ALPHANUMERIC_ALPHABET.len());
            char::from(ALPHANUMERIC_ALPHABET[index])
        })
        .collect()
}

fn generate_math_with(rng: &mut impl Rng) -> Challenge {
    let left = rng.random_range(1..10);
    let right = rng.random_range(1..10);

    match rng.random_range(0..3) {
        0 => Challenge {
            display: format!("{left}+{right}=?"),
            answer: (left + right).to_string(),
        },
        1 => {
            let (larger, smaller) = if left >= right {
                (left, right)
            } else {
                (right, left)
            };
            Challenge {
                display: format!("{larger}-{smaller}=?"),
                answer: (larger - smaller).to_string(),
            }
        }
        _ => Challenge {
            display: format!("{left}×{right}=?"),
            answer: (left * right).to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    #[test]
    fn alphanumeric_challenge_uses_unambiguous_four_character_answer() {
        let mut rng = StdRng::seed_from_u64(7);
        let challenge = generate_challenge_with(&mut rng, CaptchaType::Alphanumeric);

        assert_eq!(challenge.display, challenge.answer);
        assert_eq!(challenge.answer.chars().count(), ALPHANUMERIC_LENGTH);
        assert!(
            challenge
                .answer
                .bytes()
                .all(|character| ALPHANUMERIC_ALPHABET.contains(&character))
        );
    }

    #[test]
    fn math_challenge_answer_matches_displayed_expression() {
        let mut rng = StdRng::seed_from_u64(19);
        let mut seen_operators = [false; 3];

        for _ in 0..32 {
            let challenge = generate_challenge_with(&mut rng, CaptchaType::Math);
            let expression = challenge
                .display
                .strip_suffix("=?")
                .expect("数学验证码应以等号和问号结尾");
            let (left, right, expected, operator_index) =
                if let Some((left, right)) = expression.split_once('+') {
                    let left = left.parse::<u32>().expect("加法左值应为数字");
                    let right = right.parse::<u32>().expect("加法右值应为数字");
                    (left, right, left + right, 0)
                } else if let Some((left, right)) = expression.split_once('-') {
                    let left = left.parse::<u32>().expect("减法左值应为数字");
                    let right = right.parse::<u32>().expect("减法右值应为数字");
                    assert!(left >= right, "减法验证码不应产生负数答案");
                    (left, right, left - right, 1)
                } else {
                    let (left, right) = expression
                        .split_once('×')
                        .expect("数学验证码应使用受支持的运算符");
                    let left = left.parse::<u32>().expect("乘法左值应为数字");
                    let right = right.parse::<u32>().expect("乘法右值应为数字");
                    (left, right, left * right, 2)
                };

            assert!((1..10).contains(&left));
            assert!((1..10).contains(&right));
            assert_eq!(
                challenge.answer.parse::<u32>().expect("答案应为数字"),
                expected
            );
            seen_operators[operator_index] = true;
        }

        assert!(seen_operators.into_iter().all(|seen| seen));
    }
}
