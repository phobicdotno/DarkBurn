//! Resolution helpers and reading the DPI stored in image files.

/// Line interval (mm) for a DPI.
pub fn dpi_to_interval_mm(dpi: f64) -> f64 {
    25.4 / dpi
}

/// DPI for a line interval (mm).
pub fn interval_mm_to_dpi(interval_mm: f64) -> f64 {
    25.4 / interval_mm
}

/// Reads the horizontal and vertical resolution (dots per inch) stored in a PNG (`pHYs`),
/// JPEG (JFIF header) or BMP file. Returns `None` when the file has none.
pub fn read_dpi(bytes: &[u8]) -> Option<(f64, f64)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png_dpi(bytes);
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return jpeg_dpi(bytes);
    }
    if bytes.starts_with(b"BM") && bytes.len() >= 46 {
        let x = i32::from_le_bytes(bytes[38..42].try_into().ok()?);
        let y = i32::from_le_bytes(bytes[42..46].try_into().ok()?);
        if x > 0 && y > 0 {
            return Some((f64::from(x) * 0.0254, f64::from(y) * 0.0254));
        }
    }
    None
}

fn be32(b: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(..4)?.try_into().ok()?))
}

fn png_dpi(bytes: &[u8]) -> Option<(f64, f64)> {
    let mut i = 8;
    while i + 8 <= bytes.len() {
        let len = be32(&bytes[i..])? as usize;
        let kind = &bytes[i + 4..i + 8];
        let body = bytes.get(i + 8..i + 8 + len)?;
        if kind == b"pHYs" && len >= 9 {
            let (x, y, unit) = (be32(body)?, be32(&body[4..])?, body[8]);
            // Unit 1 is pixels per metre; unit 0 is an aspect ratio only.
            return (unit == 1 && x > 0 && y > 0).then(|| (f64::from(x) * 0.0254, f64::from(y) * 0.0254));
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        i += 12 + len;
    }
    None
}

fn jpeg_dpi(bytes: &[u8]) -> Option<(f64, f64)> {
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        let body = bytes.get(i + 4..i + 2 + len)?;
        if marker == 0xE0 && body.starts_with(b"JFIF\0") && body.len() >= 12 {
            let unit = body[7];
            let x = f64::from(u16::from_be_bytes([body[8], body[9]]));
            let y = f64::from(u16::from_be_bytes([body[10], body[11]]));
            return match unit {
                1 if x > 0.0 && y > 0.0 => Some((x, y)),
                2 if x > 0.0 && y > 0.0 => Some((x * 2.54, y * 2.54)),
                _ => None,
            };
        }
        if marker == 0xDA {
            return None;
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_conversions() {
        assert!((dpi_to_interval_mm(254.0) - 0.1).abs() < 1e-12);
        assert!((interval_mm_to_dpi(0.1) - 254.0).abs() < 1e-9);
    }

    #[test]
    fn png_phys() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0; 13 + 4]);
        png.extend_from_slice(&[0, 0, 0, 9]);
        png.extend_from_slice(b"pHYs");
        png.extend_from_slice(&11811u32.to_be_bytes());
        png.extend_from_slice(&11811u32.to_be_bytes());
        png.push(1);
        png.extend_from_slice(&[0; 4]);
        let (x, y) = read_dpi(&png).unwrap();
        assert!((x - 300.0).abs() < 0.01 && (y - 300.0).abs() < 0.01);
    }

    #[test]
    fn jpeg_jfif() {
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        jpg.extend_from_slice(b"JFIF\0");
        jpg.extend_from_slice(&[1, 2, 1, 0, 72, 0, 96, 0, 0]);
        assert_eq!(read_dpi(&jpg), Some((72.0, 96.0)));
        assert_eq!(read_dpi(b"nothing"), None);
    }

    #[test]
    fn png_written_by_image_crate_round_trips_through_decoder() {
        // A PNG without pHYs has no DPI.
        let img = image::GrayImage::new(2, 2);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        assert_eq!(read_dpi(buf.get_ref()), None);
    }
}
