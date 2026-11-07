use image::{Rgba, RgbaImage};
use qrcode::{EcLevel, QrCode};

#[derive(Clone, Copy)]
pub(super) struct Logo {
    pub left: u32,
    pub top: u32,
    pub side: u32,
}

pub(super) struct Rendered {
    pub pixels: RgbaImage,
    pub logo: Option<Logo>,
}

/// Render the exact pairing payload locally; the logo never changes its data.
pub(super) fn render(payload: &str) -> Option<Rendered> {
    let (code, branded) = QrCode::with_error_correction_level(payload, EcLevel::H)
        .map(|code| (code, true))
        // Preserve pairing if a future payload exceeds H's smaller capacity.
        .or_else(|_| QrCode::new(payload).map(|code| (code, false)))
        .ok()?;
    // Four clear modules on every side are the QR quiet zone. Integer-sized
    // modules avoid interpolation at the image's native display size.
    let modules = code.width();
    let side = modules + 8;
    let scale = (280 / side).max(2);
    let width = side * scale;
    let mut pixels = RgbaImage::from_pixel(width as u32, width as u32, Rgba([255; 4]));
    for y in 0..modules {
        for x in 0..modules {
            if code[(x, y)] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        pixels.put_pixel(
                            ((x + 4) * scale + dx) as u32,
                            ((y + 4) * scale + dy) as u32,
                            Rgba([0, 0, 0, 255]),
                        );
                    }
                }
            }
        }
    }

    let logo = if branded {
        // Keep the white cutout below a fifth of the code's side (~3% of its
        // area). High error correction protects the covered data modules.
        let badge_modules = ((modules * 18 / 100) | 1).max(3);
        let badge = badge_modules * scale;
        let start = (width - badge) / 2;
        for y in start..start + badge {
            for x in start..start + badge {
                pixels.put_pixel(x as u32, y as u32, Rgba([255; 4]));
            }
        }
        Some(Logo {
            left: (start + scale) as u32,
            top: (start + scale) as u32,
            side: ((badge_modules - 2) * scale) as u32,
        })
    } else {
        None
    };
    Some(Rendered { pixels, logo })
}
