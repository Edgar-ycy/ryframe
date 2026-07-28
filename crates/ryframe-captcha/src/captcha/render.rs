use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use rand::{Rng, RngExt};

use crate::{AppError, AppResult};

use super::glyph::{self, GLYPH_HEIGHT, GLYPH_WIDTH};

const GLYPH_SCALE: u32 = 4;
const INTERFERENCE_LINES: usize = 5;
const INTERFERENCE_POINTS: usize = 50;
const MAX_TEXT_CHARACTERS: usize = 12;
const MAX_IMAGE_DIMENSION: u32 = 1024;

pub(super) fn create_captcha_image(text: &str, width: u32, height: u32) -> AppResult<Vec<u8>> {
    let character_count = text.chars().count();
    if character_count == 0 || character_count > MAX_TEXT_CHARACTERS {
        return Err(AppError::Validation(format!(
            "验证码文本长度必须为 1-{MAX_TEXT_CHARACTERS} 个字符"
        )));
    }
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(AppError::Validation(format!(
            "验证码图片尺寸必须为 1-{MAX_IMAGE_DIMENSION} 像素"
        )));
    }
    if let Some(character) = text
        .chars()
        .find(|character| glyph::rows(*character).is_none())
    {
        return Err(AppError::Validation(format!(
            "验证码包含不支持的字符: {character}"
        )));
    }

    let character_count = u32::try_from(character_count)
        .map_err(|_| AppError::Validation("验证码字符数量无效".to_owned()))?;
    let cell_width = width / character_count;
    let glyph_width = GLYPH_WIDTH * GLYPH_SCALE;
    let glyph_height = GLYPH_HEIGHT * GLYPH_SCALE;
    if cell_width < glyph_width || height < glyph_height {
        return Err(AppError::Validation(
            "验证码图片尺寸不足以容纳文本".to_owned(),
        ));
    }

    let mut rng = rand::rng();
    let background = Rgb([
        rng.random_range(220..255),
        rng.random_range(220..255),
        rng.random_range(220..255),
    ]);
    let mut image = ImageBuffer::from_pixel(width, height, background);

    for _ in 0..INTERFERENCE_LINES {
        let color = interference_color(&mut rng);
        draw_line(
            &mut image,
            rng.random_range(0..width),
            rng.random_range(0..height),
            rng.random_range(0..width),
            rng.random_range(0..height),
            color,
        );
    }
    for _ in 0..INTERFERENCE_POINTS {
        image.put_pixel(
            rng.random_range(0..width),
            rng.random_range(0..height),
            interference_color(&mut rng),
        );
    }

    let x_offset = (cell_width - glyph_width) / 2;
    let y_offset = (height - glyph_height) / 2;
    for (index, character) in text.chars().enumerate() {
        let color = Rgb([
            rng.random_range(0..100),
            rng.random_range(0..100),
            rng.random_range(0..100),
        ]);
        draw_glyph(
            &mut image,
            glyph::rows(character).expect("glyphs were validated before rendering"),
            u32::try_from(index).unwrap_or_default() * cell_width + x_offset,
            y_offset,
            color,
        );
    }

    let mut buffer = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut buffer), ImageFormat::Png)
        .map_err(|error| AppError::Internal(format!("生成验证码图片失败: {error}")))?;
    Ok(buffer)
}

fn interference_color(rng: &mut impl Rng) -> Rgb<u8> {
    Rgb([
        rng.random_range(100..200),
        rng.random_range(100..200),
        rng.random_range(100..200),
    ])
}

fn draw_line(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    start_x: u32,
    start_y: u32,
    end_x: u32,
    end_y: u32,
    color: Rgb<u8>,
) {
    let (mut x, mut y) = (start_x as i32, start_y as i32);
    let (end_x, end_y) = (end_x as i32, end_y as i32);
    let delta_x = (end_x - x).abs();
    let delta_y = -(end_y - y).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let step_y = if y < end_y { 1 } else { -1 };
    let mut error = delta_x + delta_y;

    loop {
        if x >= 0 && y >= 0 && x < image.width() as i32 && y < image.height() as i32 {
            image.put_pixel(x as u32, y as u32, color);
        }
        if x == end_x && y == end_y {
            break;
        }
        let doubled_error = error * 2;
        if doubled_error >= delta_y {
            error += delta_y;
            x += step_x;
        }
        if doubled_error <= delta_x {
            error += delta_x;
            y += step_y;
        }
    }
}

fn draw_glyph(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    rows: [u8; GLYPH_HEIGHT as usize],
    origin_x: u32,
    origin_y: u32,
    color: Rgb<u8>,
) {
    for (row_index, row) in rows.into_iter().enumerate() {
        for column in 0..GLYPH_WIDTH {
            if row & (1u8 << (GLYPH_WIDTH - column - 1)) == 0 {
                continue;
            }
            for delta_x in 0..GLYPH_SCALE {
                for delta_y in 0..GLYPH_SCALE {
                    let x = origin_x + column * GLYPH_SCALE + delta_x;
                    let y = origin_y
                        + u32::try_from(row_index).unwrap_or_default() * GLYPH_SCALE
                        + delta_y;
                    if x < image.width() && y < image.height() {
                        image.put_pixel(x, y, color);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_math_symbol_uses_character_count_for_layout() {
        let image = create_captcha_image("1×2=?", 120, 40).unwrap();
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn invalid_render_inputs_return_errors_instead_of_panicking() {
        assert!(create_captcha_image("", 120, 40).is_err());
        assert!(create_captcha_image("ABCD", 0, 40).is_err());
        assert!(create_captcha_image("ABCD", 60, 20).is_err());
        assert!(create_captcha_image("A@CD", 120, 40).is_err());
    }
}
