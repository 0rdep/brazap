//! Draws the unread-count badge shown over the application icon in the taskbar.

pub const SIZE: usize = 32;

const RED: [f32; 3] = [242.0, 63.0, 67.0];

/// 5x7 glyphs for `0`-`9` and `+`, one row per entry, most significant bit first.
#[rustfmt::skip]
const GLYPHS: [[u8; 7]; 11] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
    [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
];

/// The text a count is shown as: the number up to 9, then `9+`.
pub fn label(count: u32) -> String {
    if count > 9 {
        "9+".into()
    } else {
        count.to_string()
    }
}

/// A red disc with the count in white, as straight-alpha RGBA.
pub fn render(count: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; SIZE * SIZE * 4];
    let center = SIZE as f32 / 2.0;
    let radius = center - 0.5;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let coverage = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            let at = (y * SIZE + x) * 4;
            pixels[at] = RED[0] as u8;
            pixels[at + 1] = RED[1] as u8;
            pixels[at + 2] = RED[2] as u8;
            pixels[at + 3] = (coverage * 255.0) as u8;
        }
    }
    let text = label(count);
    let glyphs = text
        .bytes()
        .map(|c| if c == b'+' { 10 } else { (c - b'0') as usize })
        .collect::<Vec<_>>();
    let scale = if glyphs.len() > 1 { 2 } else { 3 };
    let gap = scale;
    let width = glyphs.len() * 5 * scale + (glyphs.len() - 1) * gap;
    let left = (SIZE - width) / 2;
    let top = (SIZE - 7 * scale) / 2;
    for (index, glyph) in glyphs.into_iter().enumerate() {
        let origin = left + index * (5 * scale + gap);
        for (row, bits) in GLYPHS[glyph].iter().enumerate() {
            for column in 0..5 {
                if bits >> (4 - column) & 1 == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = origin + column * scale + dx;
                        let y = top + row * scale + dy;
                        let at = (y * SIZE + x) * 4;
                        pixels[at..at + 3].fill(255);
                    }
                }
            }
        }
    }
    pixels
}
