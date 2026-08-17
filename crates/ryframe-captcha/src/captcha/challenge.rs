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
    match captcha_type {
        CaptchaType::Alphanumeric => {
            let answer = generate_text_with(&mut rng, ALPHANUMERIC_LENGTH);
            Challenge {
                display: answer.clone(),
                answer,
            }
        }
        CaptchaType::Math => generate_math_with(&mut rng),
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
