use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
use image::{ImageEncoder, Rgba, RgbaImage};

const FONT: &[u8] = include_bytes!("../../assets/NotoColorEmoji.ttf");

const FONT_SIZE: f32 = 109.0;
const SIZE: (u32, u32) = (320, 200);

pub fn render(emoji: &str) -> Option<Vec<u8>> {
    let mut fonts = FontSystem::new();
    fonts.db_mut().load_font_data(FONT.to_vec());
    let mut cache = SwashCache::new();

    let mut buffer = Buffer::new(&mut fonts, Metrics::new(FONT_SIZE, FONT_SIZE * 1.2));
    let mut buffer = buffer.borrow_with(&mut fonts);
    buffer.set_size(Some(SIZE.0 as f32), Some(SIZE.1 as f32));
    buffer.set_text(
        emoji,
        &Attrs::new().family(Family::Name("Noto Color Emoji")),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(true);

    let mut canvas = RgbaImage::from_pixel(SIZE.0, SIZE.1, Rgba([245, 245, 245, 255]));
    let mut drew = false;
    buffer.draw(&mut cache, Color::rgb(0, 0, 0), |x, y, w, h, colour| {
        if colour.a() == 0 {
            return;
        }
        drew = true;
        for dy in 0..h {
            for dx in 0..w {
                let (px, py) = (x + dx as i32 + 100, y + dy as i32 + 20);
                if px < 0 || py < 0 || px >= SIZE.0 as i32 || py >= SIZE.1 as i32 {
                    continue;
                }

                let base = canvas.get_pixel(px as u32, py as u32).0;
                let a = colour.a() as u32;
                let mix = |top: u8, bottom: u8| {
                    ((top as u32 * a + bottom as u32 * (255 - a)) / 255) as u8
                };
                canvas.put_pixel(
                    px as u32,
                    py as u32,
                    Rgba([
                        mix(colour.r(), base[0]),
                        mix(colour.g(), base[1]),
                        mix(colour.b(), base[2]),
                        255,
                    ]),
                );
            }
        }
    });
    if !drew {
        return None;
    }

    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(canvas.as_raw(), SIZE.0, SIZE.1, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_real_image() {
        let png = render("🍎").expect("apple should render");
        assert!(png.len() > 1_000, "suspiciously small png: {}", png.len());
        assert_eq!(&png[1..4], b"PNG");
    }
}
