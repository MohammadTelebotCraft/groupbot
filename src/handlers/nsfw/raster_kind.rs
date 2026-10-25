#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RasterKind {
    Static,
    Animated,
    Unknown,
}

pub(super) fn raster_kind(prefix: &[u8], declared_size: usize) -> RasterKind {
    if declared_size == 0 || prefix.len() > declared_size {
        return RasterKind::Unknown;
    }
    if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png_kind(prefix, declared_size);
    }
    if prefix.starts_with(b"RIFF") {
        return webp_kind(prefix, declared_size);
    }
    RasterKind::Unknown
}

fn png_kind(bytes: &[u8], declared_size: usize) -> RasterKind {
    let mut at = 8usize;
    let mut first = true;
    loop {
        let Some(header) = bytes.get(at..at.saturating_add(8)) else {
            return RasterKind::Unknown;
        };
        let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let kind = &header[4..8];
        let Some(end) = at.checked_add(12).and_then(|at| at.checked_add(size)) else {
            return RasterKind::Unknown;
        };
        if end > declared_size || !kind.iter().all(u8::is_ascii_alphabetic) {
            return RasterKind::Unknown;
        }
        if first {
            if kind != b"IHDR" || size != 13 {
                return RasterKind::Unknown;
            }
            first = false;
        } else if kind == b"IHDR" {
            return RasterKind::Unknown;
        }
        if kind == b"IDAT" {
            return RasterKind::Static;
        }
        let Some(chunk) = bytes.get(at + 4..end - 4) else {
            return RasterKind::Unknown;
        };
        let Some(crc_bytes) = bytes.get(end - 4..end) else {
            return RasterKind::Unknown;
        };
        let crc = u32::from_be_bytes(crc_bytes.try_into().unwrap());
        if png_crc(chunk) != crc {
            return RasterKind::Unknown;
        }
        if kind == b"IHDR" {
            let width = u32::from_be_bytes(bytes[at + 8..at + 12].try_into().unwrap());
            let height = u32::from_be_bytes(bytes[at + 12..at + 16].try_into().unwrap());
            if width == 0 || height == 0 {
                return RasterKind::Unknown;
            }
        } else if kind == b"acTL" {
            if size != 8 || u32::from_be_bytes(bytes[at + 8..at + 12].try_into().unwrap()) == 0 {
                return RasterKind::Unknown;
            }
            return RasterKind::Animated;
        } else if kind == b"IEND" {
            return RasterKind::Unknown;
        }
        at = end;
    }
}

fn webp_kind(bytes: &[u8], declared_size: usize) -> RasterKind {
    let Some(header) = bytes.get(..20) else {
        return RasterKind::Unknown;
    };
    if &header[8..12] != b"WEBP" {
        return RasterKind::Unknown;
    }
    let riff_size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    if riff_size.checked_add(8) != Some(declared_size) {
        return RasterKind::Unknown;
    }
    let size = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
    let Some(end) = 20usize
        .checked_add(size)
        .and_then(|at| at.checked_add(size & 1))
    else {
        return RasterKind::Unknown;
    };
    if end > declared_size {
        return RasterKind::Unknown;
    }
    match &header[12..16] {
        b"VP8X" if size == 10 => {
            let Some(payload) = bytes.get(20..30) else {
                return RasterKind::Unknown;
            };
            if payload[0] & 0xc1 != 0 || payload[1..4] != [0, 0, 0] {
                return RasterKind::Unknown;
            }
            if payload[0] & 0x02 != 0 {
                RasterKind::Animated
            } else {
                RasterKind::Static
            }
        }
        b"VP8 " | b"VP8L" if size > 0 => RasterKind::Static,
        _ => RasterKind::Unknown,
    }
}

const CRC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut at = 0;
    while at < 256 {
        let mut value = at as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xedb8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[at] = value;
        at += 1;
    }
    table
};

fn png_crc(bytes: &[u8]) -> u32 {
    let mut value = !0u32;
    for &byte in bytes {
        value = (value >> 8) ^ CRC_TABLE[((value as u8) ^ byte) as usize];
    }
    !value
}

#[cfg(test)]
mod tests {
    use super::*;

    const APNG: &[u8] = include_bytes!("../../../tests/fixtures/media/two-frame.apng");
    const WEBP: &[u8] = include_bytes!("../../../tests/fixtures/media/two-frame.webp");

    fn hex(input: &str) -> Vec<u8> {
        (0..input.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&input[at..at + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn identifies_real_animated_and_static_containers() {
        assert_eq!(raster_kind(APNG, APNG.len()), RasterKind::Animated);
        assert_eq!(raster_kind(WEBP, WEBP.len()), RasterKind::Animated);
        assert_eq!(raster_kind(&WEBP[..30], WEBP.len()), RasterKind::Animated);

        let png = hex(concat!(
            "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de",
            "0000000c49444154789c63f8cfc0000003010100c9fe92ef",
            "0000000049454e44ae426082"
        ));
        assert_eq!(raster_kind(&png[..41], png.len()), RasterKind::Static);
        assert_eq!(raster_kind(&png, png.len()), RasterKind::Static);

        let webp = hex(concat!(
            "524946463800000057454250565038202c0000009001009d012a01000100",
            "02c04c25a00274ba00039800feee431fee6c738b7057ff6d0fff5a1ffeb43fe94000"
        ));
        assert_eq!(raster_kind(&webp[..20], webp.len()), RasterKind::Static);
        assert_eq!(raster_kind(&webp, webp.len()), RasterKind::Static);
    }

    #[test]
    fn late_apng_control_is_unknown_until_reached() {
        let split = 33;
        let payload = [b"note\0".as_slice(), &vec![b'x'; 8192]].concat();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        chunk.extend_from_slice(b"tEXt");
        chunk.extend_from_slice(&payload);
        chunk.extend_from_slice(&png_crc(&chunk[4..]).to_be_bytes());
        let mut late = APNG[..split].to_vec();
        late.extend_from_slice(&chunk);
        late.extend_from_slice(&APNG[split..]);

        assert_eq!(raster_kind(&late[..4096], late.len()), RasterKind::Unknown);
        assert_eq!(raster_kind(&late, late.len()), RasterKind::Animated);
    }

    #[test]
    fn truncated_and_malformed_headers_remain_unknown() {
        assert_eq!(raster_kind(&APNG[..31], APNG.len()), RasterKind::Unknown);
        assert_eq!(raster_kind(&WEBP[..19], WEBP.len()), RasterKind::Unknown);
        assert_eq!(raster_kind(&WEBP[..29], WEBP.len()), RasterKind::Unknown);
        assert_eq!(raster_kind(APNG, 20), RasterKind::Unknown);

        let mut bad_png = APNG.to_vec();
        bad_png[29] ^= 1;
        assert_eq!(raster_kind(&bad_png, bad_png.len()), RasterKind::Unknown);
        let mut bad_apng = APNG.to_vec();
        let actl = bad_apng
            .windows(4)
            .position(|bytes| bytes == b"acTL")
            .unwrap();
        bad_apng[actl + 12] ^= 1;
        assert_eq!(raster_kind(&bad_apng, bad_apng.len()), RasterKind::Unknown);

        let mut bad_webp = WEBP.to_vec();
        bad_webp[4] ^= 1;
        assert_eq!(raster_kind(&bad_webp, bad_webp.len()), RasterKind::Unknown);
    }
}
