use super::{
    ANIMATED_BYTES_MAX, ANIMATED_SIGNATURE_SIDE, IMAGE_DECODE_BYTES_MAX, IMAGE_DIMENSION_MAX,
    IMAGE_PIXEL_MAX, MAX_ANIMATED_FRAMES, MAX_ANIMATED_SIDE, MAX_NATIVE_GIF_SCAN_FRAMES,
    decode_still, sample_novel_indices, scaled_dimensions,
};
use std::io::Cursor;
use tiff::tags::Tag;

const MAX_TIFF_SCAN_PIXELS: u64 = 64_000_000;

fn page_offsets(bytes: &[u8]) -> Option<Vec<u64>> {
    if bytes.len() > ANIMATED_BYTES_MAX
        || image::guess_format(bytes).ok()? != image::ImageFormat::Tiff
    {
        return None;
    }
    let mut limits = tiff::decoder::Limits::default();
    limits.decoding_buffer_size = IMAGE_DECODE_BYTES_MAX as usize;
    limits.intermediate_buffer_size = IMAGE_DECODE_BYTES_MAX as usize;
    let mut decoder = tiff::decoder::Decoder::new(Cursor::new(bytes))
        .ok()?
        .with_limits(limits);
    let mut pages = Vec::new();
    let mut pixels = 0_u64;
    loop {
        if pages.len() == MAX_NATIVE_GIF_SCAN_FRAMES {
            return None;
        }
        let (width, height) = decoder.dimensions().ok()?;
        let page_pixels = u64::from(width).checked_mul(u64::from(height))?;
        if width == 0
            || height == 0
            || width > IMAGE_DIMENSION_MAX
            || height > IMAGE_DIMENSION_MAX
            || page_pixels > IMAGE_PIXEL_MAX
        {
            return None;
        }
        pixels = pixels.checked_add(page_pixels)?;
        if pixels > MAX_TIFF_SCAN_PIXELS {
            return None;
        }
        if decoder.find_tag(Tag::SubIfd).ok()?.is_some() {
            return None;
        }
        let color = decoder.colortype().ok()?;
        let extras = decoder
            .find_tag_unsigned_vec::<u16>(Tag::ExtraSamples)
            .ok()?;
        match color {
            tiff::ColorType::RGBA(_) | tiff::ColorType::GrayA(_) => {
                if extras.as_deref() != Some(&[2]) {
                    return None;
                }
            }
            _ if extras.is_some() => return None,
            _ => {}
        }
        if !matches!(
            color,
            tiff::ColorType::Gray(1 | 8 | 16)
                | tiff::ColorType::GrayA(8 | 16)
                | tiff::ColorType::RGB(8 | 16 | 32)
                | tiff::ColorType::RGBA(8 | 16 | 32)
                | tiff::ColorType::CMYK(8 | 16)
        ) {
            return None;
        }
        pages.push(decoder.ifd_pointer()?.0);
        if !decoder.more_images() {
            break;
        }
        decoder.next_image().ok()?;
    }
    (!pages.is_empty()).then_some(pages)
}

fn select_page(bytes: &mut [u8], offset: u64) -> Option<()> {
    match bytes.get(..4)? {
        b"II*\0" => bytes
            .get_mut(4..8)?
            .copy_from_slice(&u32::try_from(offset).ok()?.to_le_bytes()),
        b"MM\0*" => bytes
            .get_mut(4..8)?
            .copy_from_slice(&u32::try_from(offset).ok()?.to_be_bytes()),
        _ => return None,
    }
    Some(())
}

pub(super) fn decode_frames(mut bytes: Vec<u8>) -> Option<Vec<image::RgbImage>> {
    let pages = page_offsets(&bytes)?;
    let mut signatures = Vec::with_capacity(pages.len());
    for &offset in &pages {
        select_page(&mut bytes, offset)?;
        let page = decode_still(&bytes).ok()?;
        signatures.push(
            image::imageops::resize(
                &page,
                ANIMATED_SIGNATURE_SIDE,
                ANIMATED_SIGNATURE_SIDE,
                image::imageops::FilterType::Nearest,
            )
            .into_raw(),
        );
    }
    let wanted = sample_novel_indices(&signatures, MAX_ANIMATED_FRAMES);
    let mut frames = Vec::with_capacity(wanted.len());
    for at in wanted {
        select_page(&mut bytes, pages[at])?;
        let page = decode_still(&bytes).ok()?;
        let (width, height) = scaled_dimensions(page.width(), page.height(), MAX_ANIMATED_SIDE);
        frames.push(if page.dimensions() == (width, height) {
            page
        } else {
            image::imageops::resize(&page, width, height, image::imageops::FilterType::Triangle)
        });
    }
    (!frames.is_empty()).then_some(frames)
}
