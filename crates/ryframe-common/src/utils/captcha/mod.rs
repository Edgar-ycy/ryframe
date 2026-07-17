//! Captcha challenge generation and PNG rendering.

mod challenge;
mod glyph;
mod render;

use std::fmt;

use crate::AppResult;

use challenge::generate_challenge;
use render::create_captcha_image;

const CAPTCHA_WIDTH: u32 = 120;
const CAPTCHA_HEIGHT: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaType {
    Alphanumeric,
    Math,
}

#[derive(Clone)]
pub struct CaptchaResult {
    answer: String,
    image_data: Vec<u8>,
}

impl CaptchaResult {
    pub fn into_parts(self) -> (String, Vec<u8>) {
        (self.answer, self.image_data)
    }
}

impl fmt::Debug for CaptchaResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptchaResult")
            .field("answer", &"[REDACTED]")
            .field("image_bytes", &self.image_data.len())
            .finish()
    }
}

pub fn generate_captcha(captcha_type: CaptchaType) -> AppResult<CaptchaResult> {
    let challenge = generate_challenge(captcha_type);
    let image_data = create_captcha_image(&challenge.display, CAPTCHA_WIDTH, CAPTCHA_HEIGHT)?;
    Ok(CaptchaResult {
        answer: challenge.answer,
        image_data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_redacts_the_answer() {
        let result = CaptchaResult {
            answer: "SECRET".to_owned(),
            image_data: vec![1, 2, 3],
        };

        let output = format!("{result:?}");
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("SECRET"));
    }
}
