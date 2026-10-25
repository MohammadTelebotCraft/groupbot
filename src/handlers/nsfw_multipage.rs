use super::{
    ANIMATED_BYTES_MAX, IMAGE_DECODE_BYTES_MAX, IMAGE_DIMENSION_MAX, IMAGE_PIXEL_MAX,
    MAX_ANIMATED_FRAMES, MAX_ANIMATED_SIDE, scaled_dimensions, vision_rgb,
};
use image::ImageDecoder;
use std::io::Cursor;

const MAX_SCAN_PIXELS: u64 = 64_000_000;

fn admit(decoder: &impl ImageDecoder, scanned: &mut u64) -> Option<()> {
    let (width, height) = decoder.dimensions();
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    if width == 0
        || height == 0
        || width > IMAGE_DIMENSION_MAX
        || height > IMAGE_DIMENSION_MAX
        || pixels > IMAGE_PIXEL_MAX
        || decoder.total_bytes() > IMAGE_DECODE_BYTES_MAX
    {
        return None;
    }
    *scanned = scanned.checked_add(pixels)?;
    (*scanned <= MAX_SCAN_PIXELS).then_some(())
}

fn frame(image: image::DynamicImage) -> image::RgbImage {
    let rgb = vision_rgb(image);
    let (width, height) = scaled_dimensions(rgb.width(), rgb.height(), MAX_ANIMATED_SIDE);
    if rgb.dimensions() == (width, height) {
        rgb
    } else {
        image::imageops::resize(&rgb, width, height, image::imageops::FilterType::Triangle)
    }
}

pub(super) fn decode_ico_frames(bytes: Vec<u8>) -> Option<Vec<image::RgbImage>> {
    if bytes.len() > ANIMATED_BYTES_MAX || bytes.get(..4)? != b"\0\0\x01\0" {
        return None;
    }
    let count = usize::from(u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?));
    if count == 0 || count > MAX_ANIMATED_FRAMES {
        return None;
    }
    let directory_end = 6usize.checked_add(count.checked_mul(16)?)?;
    bytes.get(..directory_end)?;
    let mut frames = Vec::with_capacity(count);
    let mut scanned = 0;
    for at in 0..count {
        let entry = bytes.get(6 + at * 16..6 + (at + 1) * 16)?;
        let length =
            usize::try_from(u32::from_le_bytes(entry.get(8..12)?.try_into().ok()?)).ok()?;
        let offset =
            usize::try_from(u32::from_le_bytes(entry.get(12..16)?.try_into().ok()?)).ok()?;
        if length == 0 || offset < directory_end {
            return None;
        }
        let payload = bytes.get(offset..offset.checked_add(length)?)?;
        if payload.starts_with(b"\x89PNG\r\n\x1a\n")
            && super::raster_kind_full(payload) != Some(false)
        {
            return None;
        }
        let mut single = Vec::with_capacity(22usize.checked_add(length)?);
        single.extend_from_slice(b"\0\0\x01\0\x01\0");
        single.extend_from_slice(&entry[..12]);
        single.extend_from_slice(&22u32.to_le_bytes());
        single.extend_from_slice(payload);
        let decoder = image::codecs::ico::IcoDecoder::new(Cursor::new(single)).ok()?;
        admit(&decoder, &mut scanned)?;
        frames.push(frame(image::DynamicImage::from_decoder(decoder).ok()?));
    }
    Some(frames)
}

fn skip_pnm_separators(bytes: &[u8], at: &mut usize) {
    while let Some(&byte) = bytes.get(*at) {
        if byte.is_ascii_whitespace() {
            *at += 1;
        } else if byte == b'#' {
            *at = bytes[*at..]
                .iter()
                .position(|&candidate| candidate == b'\n')
                .map_or(bytes.len(), |end| *at + end + 1);
        } else {
            break;
        }
    }
}

pub(super) fn decode_pnm_frames(bytes: Vec<u8>) -> Option<Vec<image::RgbImage>> {
    if bytes.len() > ANIMATED_BYTES_MAX {
        return None;
    }
    let mut cursor = Cursor::new(bytes.as_slice());
    let mut frames = Vec::new();
    let mut scanned = 0;
    loop {
        let mut at = usize::try_from(cursor.position()).ok()?;
        skip_pnm_separators(&bytes, &mut at);
        if at == bytes.len() {
            return (!frames.is_empty()).then_some(frames);
        }
        if frames.len() == MAX_ANIMATED_FRAMES
            || !matches!(bytes.get(at..at + 2), Some([b'P', b'1'..=b'7']))
        {
            return None;
        }
        cursor.set_position(at as u64);
        let decoder = image::codecs::pnm::PnmDecoder::new(&mut cursor).ok()?;
        admit(&decoder, &mut scanned)?;
        frames.push(frame(image::DynamicImage::from_decoder(decoder).ok()?));
        if usize::try_from(cursor.position()).ok()? <= at {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ico_two() -> Vec<u8> {
        let png = |side, color| {
            let pixels = image::RgbaImage::from_pixel(side, side, image::Rgba(color));
            let mut out = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(pixels)
                .write_to(&mut out, image::ImageFormat::Png)
                .unwrap();
            out.into_inner()
        };
        let images = [png(16, [255, 0, 0, 0]), png(32, [0, 0, 255, 255])];
        let mut ico = b"\0\0\x01\0\x02\0".to_vec();
        let mut offset = 6 + 16 * images.len();
        for (side, bytes) in [16u8, 32].into_iter().zip(&images) {
            ico.extend_from_slice(&[side, side, 0, 0]);
            ico.extend_from_slice(&1u16.to_le_bytes());
            ico.extend_from_slice(&32u16.to_le_bytes());
            ico.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            ico.extend_from_slice(&(offset as u32).to_le_bytes());
            offset += bytes.len();
        }
        for bytes in images {
            ico.extend_from_slice(&bytes);
        }
        ico
    }

    #[test]
    fn ico_scores_every_entry_and_flattens_hidden_rgb() {
        let frames = decode_ico_frames(ico_two()).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].dimensions(), (16, 16));
        assert_eq!(frames[0].get_pixel(0, 0).0, [128, 128, 128]);
        assert_eq!(frames[1].dimensions(), (32, 32));
        assert_eq!(frames[1].get_pixel(0, 0).0, [0, 0, 255]);
    }

    #[test]
    fn ico_rejects_a_broken_later_entry() {
        let mut bytes = ico_two();
        bytes[6 + 16 + 8..6 + 16 + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_ico_frames(bytes).is_none());
    }

    #[test]
    fn ico_rejects_an_animated_png_entry() {
        let payload = include_bytes!("../../tests/fixtures/media/two-frame.apng");
        let mut bytes = b"\0\0\x01\0\x01\0".to_vec();
        bytes.extend_from_slice(&[16, 16, 0, 0]);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&22u32.to_le_bytes());
        bytes.extend_from_slice(payload);
        assert!(decode_ico_frames(bytes).is_none());
    }

    #[test]
    fn pnm_scores_each_subtype_and_flattens_pam_alpha() {
        let mut bytes = b"P3\n1 1\n255\n0 255 0\nP6\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[255, 0, 0]);
        bytes.extend_from_slice(
            b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
        );
        bytes.extend_from_slice(&[255, 0, 0, 0]);
        let frames = decode_pnm_frames(bytes).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].get_pixel(0, 0).0, [0, 255, 0]);
        assert_eq!(frames[1].get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(frames[2].get_pixel(0, 0).0, [128, 128, 128]);
    }

    #[test]
    fn pnm_rejects_trailing_junk_truncation_and_excess_frames() {
        let one = b"P6\n1 1\n255\n\xff\0\0";
        let mut junk = one.to_vec();
        junk.extend_from_slice(b"junk");
        assert!(decode_pnm_frames(junk).is_none());
        let mut truncated = one.to_vec();
        truncated.extend_from_slice(b"P6\n1 1\n255\n\xff");
        assert!(decode_pnm_frames(truncated).is_none());
        assert!(decode_pnm_frames(one.repeat(MAX_ANIMATED_FRAMES + 1)).is_none());
    }
}
