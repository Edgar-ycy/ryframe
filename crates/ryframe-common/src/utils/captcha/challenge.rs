use rand::{Rng, RngExt};

use super::CaptchaType;

pub(super) const ALPHANUMERIC_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
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

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    #[test]
    fn generated_text_uses_the_unambiguous_alphabet() {
        let mut rng = StdRng::seed_from_u64(7);
        let text = generate_text_with(&mut rng, 256);

        assert_eq!(text.len(), 256);
        assert!(
            text.bytes()
                .all(|character| ALPHANUMERIC_ALPHABET.contains(&character))
        );
    }

    #[test]
    fn generated_math_answers_match_the_displayed_expression() {
        let mut rng = StdRng::seed_from_u64(11);

        for _ in 0..256 {
            let challenge = generate_math_with(&mut rng);
            let expression = challenge.display.strip_suffix("=?").unwrap();
            let expected = if let Some((left, right)) = expression.split_once('+') {
                left.parse::<u32>().unwrap() + right.parse::<u32>().unwrap()
            } else if let Some((left, right)) = expression.split_once('-') {
                left.parse::<u32>().unwrap() - right.parse::<u32>().unwrap()
            } else {
                let (left, right) = expression.split_once('×').unwrap();
                left.parse::<u32>().unwrap() * right.parse::<u32>().unwrap()
            };

            assert_eq!(challenge.answer, expected.to_string());
        }
    }
}
