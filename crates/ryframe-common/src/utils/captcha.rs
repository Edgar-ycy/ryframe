use image::{ImageBuffer, Rgb};
use rand::prelude::*;

use crate::{AppError, AppResult};

/// 验证码类型
#[derive(Debug, Clone)]
pub enum CaptchaType {
    /// 数字字母混合验证码
    Alphanumeric,
    /// 数学计算验证码（如 3+5=?）
    Math,
}

/// 验证码生成结果
#[derive(Debug, Clone)]
pub struct CaptchaResult {
    /// 验证码答案（用于校验）
    pub answer: String,
    /// 验证码图片（PNG 格式的字节数据）
    pub image_data: Vec<u8>,
}

/// 生成随机验证码文本
pub fn generate_text(length: usize) -> String {
    let chars = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // 去除易混淆字符
    let mut rng = rand::rng();
    (0..length)
        .map(|_| {
            let idx = rng.random_range(0..chars.len());
            chars.chars().nth(idx).unwrap()
        })
        .collect()
}

/// 生成数学验证码
pub fn generate_math() -> (String, String) {
    let mut rng = rand::rng();
    let a = rng.random_range(1..10);
    let b = rng.random_range(1..10);
    let operators = ['+', '-', '*'];
    let op = operators[rng.random_range(0..operators.len())];

    let (question, answer) = match op {
        '+' => (format!("{}+{}=?", a, b), (a + b).to_string()),
        '-' => {
            // 确保结果为正数
            let (max, min) = if a > b { (a, b) } else { (b, a) };
            (format!("{}-{}=?", max, min), (max - min).to_string())
        }
        '*' => (format!("{}×{}=?", a, b), (a * b).to_string()),
        _ => unreachable!(),
    };

    (question, answer)
}

/// 创建验证码图片
pub fn create_captcha_image(text: &str, width: u32, height: u32) -> AppResult<Vec<u8>> {
    let mut img = ImageBuffer::new(width, height);
    let mut rng = rand::rng();

    // 背景色
    let bg_color = Rgb([
        rng.random_range(220..255),
        rng.random_range(220..255),
        rng.random_range(220..255),
    ]);

    // 填充背景
    for x in 0..width {
        for y in 0..height {
            img.put_pixel(x, y, bg_color);
        }
    }

    // 添加干扰线
    for _ in 0..5 {
        let x1 = rng.random_range(0..width);
        let y1 = rng.random_range(0..height);
        let x2 = rng.random_range(0..width);
        let y2 = rng.random_range(0..height);
        let line_color = Rgb([
            rng.random_range(100..200),
            rng.random_range(100..200),
            rng.random_range(100..200),
        ]);
        draw_line(&mut img, x1, y1, x2, y2, line_color);
    }

    // 添加干扰点
    for _ in 0..50 {
        let x = rng.random_range(0..width);
        let y = rng.random_range(0..height);
        let point_color = Rgb([
            rng.random_range(100..200),
            rng.random_range(100..200),
            rng.random_range(100..200),
        ]);
        img.put_pixel(x, y, point_color);
    }

    // 绘制文字（简单实现，使用像素点模拟）
    let char_width = width / (text.len() as u32);
    for (i, ch) in text.chars().enumerate() {
        let x = (i as u32) * char_width + char_width / 4;
        let y = height / 4;
        let text_color = Rgb([
            rng.random_range(0..100),
            rng.random_range(0..100),
            rng.random_range(0..100),
        ]);
        draw_char(&mut img, ch, x, y, text_color);
    }

    // 转换为 PNG 字节数据
    let mut buffer = Vec::new();
    let dyn_img = image::DynamicImage::ImageRgb8(img);
    dyn_img
        .write_to(
            &mut std::io::Cursor::new(&mut buffer),
            image::ImageFormat::Png,
        )
        .map_err(|e| AppError::Internal(format!("生成验证码图片失败: {}", e)))?;

    Ok(buffer)
}

/// 绘制直线（Bresenham 算法）
fn draw_line(
    img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x1: u32,
    y1: u32,
    x2: u32,
    y2: u32,
    color: Rgb<u8>,
) {
    let mut x1 = x1 as i32;
    let mut y1 = y1 as i32;
    let x2 = x2 as i32;
    let y2 = y2 as i32;

    let dx = (x2 - x1).abs();
    let dy = -(y2 - y1).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        if x1 >= 0 && y1 >= 0 && x1 < img.width() as i32 && y1 < img.height() as i32 {
            img.put_pixel(x1 as u32, y1 as u32, color);
        }
        if x1 == x2 && y1 == y2 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x1 += sx;
        }
        if e2 <= dx {
            err += dx;
            y1 += sy;
        }
    }
}

/// 绘制字符（简化版，实际应使用字体渲染）
fn draw_char(img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, ch: char, x: u32, y: u32, color: Rgb<u8>) {
    // 简化实现：根据字符绘制简单的点阵图案
    // 实际生产环境应使用字体库（如 rusttype）
    let pattern = match ch {
        '0' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (4, 1),
            (0, 2),
            (4, 2),
            (0, 3),
            (4, 3),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        '1' => vec![(2, 0), (1, 1), (2, 1), (3, 1), (2, 2), (2, 3), (2, 4)],
        '2' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 1),
            (3, 2),
            (2, 2),
            (1, 3),
            (0, 4),
            (1, 4),
            (2, 4),
            (3, 4),
            (4, 4),
        ],
        '3' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 1),
            (3, 2),
            (2, 2),
            (4, 3),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        '4' => vec![
            (0, 0),
            (4, 0),
            (0, 1),
            (4, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2),
            (4, 2),
            (4, 3),
            (4, 4),
        ],
        '5' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (0, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2),
            (4, 3),
            (3, 4),
            (2, 4),
            (1, 4),
        ],
        '6' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2),
            (4, 2),
            (0, 3),
            (4, 3),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        '7' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (4, 1),
            (3, 2),
            (2, 3),
            (1, 4),
        ],
        '8' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (4, 1),
            (1, 2),
            (2, 2),
            (3, 2),
            (0, 3),
            (4, 3),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        '9' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (4, 1),
            (0, 2),
            (4, 2),
            (1, 3),
            (2, 3),
            (3, 3),
            (4, 3),
            (4, 4),
        ],
        'A' => vec![
            (2, 0),
            (1, 1),
            (3, 1),
            (0, 2),
            (4, 2),
            (0, 3),
            (4, 3),
            (0, 4),
            (4, 4),
        ],
        'B' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (0, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2),
            (0, 3),
            (0, 4),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        'C' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (0, 2),
            (0, 3),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        'D' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        'E' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (0, 3),
            (0, 4),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        'F' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (0, 3),
            (0, 4),
        ],
        'G' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (0, 2),
            (4, 2),
            (0, 3),
            (1, 4),
            (2, 4),
            (3, 4),
            (4, 3),
            (4, 4),
        ],
        'H' => vec![
            (0, 0),
            (4, 0),
            (0, 1),
            (4, 1),
            (0, 2),
            (4, 2),
            (0, 3),
            (4, 3),
            (0, 4),
            (1, 4),
            (2, 4),
            (3, 4),
            (4, 4),
        ],
        'J' => vec![
            (4, 0),
            (4, 1),
            (4, 2),
            (0, 3),
            (1, 3),
            (2, 3),
            (3, 3),
            (4, 3),
            (4, 4),
        ],
        'K' => vec![
            (0, 0),
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (1, 3),
            (2, 2),
            (3, 1),
            (4, 0),
            (1, 1),
            (2, 2),
            (3, 3),
            (4, 4),
        ],
        'L' => vec![
            (0, 0),
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (1, 4),
            (2, 4),
            (3, 4),
            (4, 4),
        ],
        'M' => vec![
            (0, 0),
            (4, 0),
            (0, 1),
            (4, 1),
            (0, 2),
            (4, 2),
            (1, 2),
            (2, 1),
            (3, 2),
            (0, 3),
            (4, 3),
            (0, 4),
            (4, 4),
        ],
        'N' => vec![
            (0, 0),
            (4, 0),
            (0, 1),
            (4, 1),
            (1, 1),
            (2, 2),
            (3, 3),
            (0, 2),
            (4, 2),
            (0, 3),
            (4, 3),
            (0, 4),
            (4, 4),
        ],
        'P' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (0, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2),
            (0, 3),
            (0, 4),
        ],
        'R' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (0, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2),
            (0, 3),
            (4, 3),
            (0, 4),
            (4, 4),
        ],
        'S' => vec![
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (1, 2),
            (2, 2),
            (3, 2),
            (4, 2),
            (3, 3),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        'T' => vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (2, 1),
            (2, 2),
            (2, 3),
            (2, 4),
        ],
        'U' => vec![
            (0, 0),
            (4, 0),
            (0, 1),
            (4, 1),
            (0, 2),
            (4, 2),
            (0, 3),
            (4, 3),
            (1, 4),
            (2, 4),
            (3, 4),
        ],
        'W' => vec![
            (0, 0),
            (4, 0),
            (0, 1),
            (4, 1),
            (0, 2),
            (4, 2),
            (1, 3),
            (3, 3),
            (2, 4),
        ],
        'X' => vec![
            (0, 0),
            (4, 0),
            (1, 1),
            (3, 1),
            (2, 2),
            (1, 3),
            (3, 3),
            (0, 4),
            (4, 4),
        ],
        'Y' => vec![(0, 0), (4, 0), (1, 1), (3, 1), (2, 2), (2, 3), (2, 4)],
        _ => vec![], // 其他字符不绘制
    };

    let scale = 4; // 每个点放大4倍
    for (px, py) in pattern {
        for dx in 0..scale {
            for dy in 0..scale {
                let draw_x = x + px * scale + dx;
                let draw_y = y + py * scale + dy;
                if draw_x < img.width() && draw_y < img.height() {
                    img.put_pixel(draw_x, draw_y, color);
                }
            }
        }
    }
}

/// 生成验证码
pub fn generate_captcha(captcha_type: CaptchaType) -> AppResult<CaptchaResult> {
    match captcha_type {
        CaptchaType::Alphanumeric => {
            let text = generate_text(4);
            let image_data = create_captcha_image(&text, 120, 40)?;
            Ok(CaptchaResult {
                answer: text,
                image_data,
            })
        }
        CaptchaType::Math => {
            let (question, answer) = generate_math();
            let image_data = create_captcha_image(&question, 120, 40)?;
            Ok(CaptchaResult { answer, image_data })
        }
    }
}
