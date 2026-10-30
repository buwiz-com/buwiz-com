//! Files into what the readers take: a photo decoded upright and scaled
//! down, a digital PDF's text layer. All in memory.

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};

use super::extract::Line;
use super::{Bitmap, PDF_TEXT, Text};

/// Longest side a photo is scaled down to. An ID card that fills a phone
/// photo still has legible text at this size, and the engine's memory
/// stays well under 1 GB.
pub const MAX_SIDE: u32 = 1600;
/// PDF pages read; a 2303 is one page.
const PDF_PAGES: usize = 3;

/// A JPEG, PNG or WebP photo, turned upright as its EXIF orientation says
/// and scaled so its longest side is at most [`MAX_SIDE`].
pub fn photo(bytes: &[u8]) -> Result<Bitmap, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(unreadable)?;
    // A small file can claim a huge picture; refuse before allocating it.
    let mut limits = Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    let orientation = decoder.orientation().map_err(unreadable)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    image.apply_orientation(orientation);
    if image.width() > MAX_SIDE || image.height() > MAX_SIDE {
        image = image.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle);
    }
    let rgb = image.into_rgb8();
    Ok(Bitmap { width: rgb.width(), height: rgb.height(), rgb: rgb.into_raw() })
}

fn unreadable<E>(_: E) -> String {
    "the photo could not be opened".to_owned()
}

/// The text layer of a digital PDF, line by line. A scan has none.
pub fn pdf_text(bytes: &[u8]) -> Result<Text, String> {
    // The PDF parser panics on some malformed files.
    let pages = catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text_from_mem_by_pages(bytes)))
        .map_err(|_| "the PDF could not be opened".to_owned())?
        .map_err(|_| "the PDF could not be opened".to_owned())?;
    let lines: Vec<Line> = pages
        .iter()
        .take(PDF_PAGES)
        .flat_map(|page| page.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Line::new(line, 1.0))
        .collect();
    let letters = lines.iter().flat_map(|line| line.text.chars()).filter(|ch| ch.is_alphanumeric()).count();
    if letters < 20 {
        return Err("PDF scan not supported yet".to_owned());
    }
    Ok(Text { engine: PDF_TEXT, version: "pdf-extract 0.12", lines })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgb, RgbImage};

    fn png(image: &RgbImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(image.as_raw(), image.width(), image.height(), image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    #[test]
    fn photos_are_scaled_down_keeping_their_shape() {
        let big = RgbImage::from_pixel(4000, 2000, Rgb([200, 200, 200]));
        let bitmap = photo(&png(&big)).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (1600, 800));
        assert_eq!(bitmap.rgb.len(), 1600 * 800 * 3);
        let small = RgbImage::from_pixel(640, 400, Rgb([0, 0, 0]));
        assert_eq!(photo(&png(&small)).map(|bitmap| bitmap.width), Ok(640));
    }

    #[test]
    fn photos_turn_upright_from_exif() {
        // A JPEG 40 wide and 20 high whose EXIF says "rotate 90° clockwise".
        let mut jpeg = Vec::new();
        let image = RgbImage::from_pixel(40, 20, Rgb([10, 20, 30]));
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .write_image(image.as_raw(), 40, 20, image::ExtendedColorType::Rgb8)
            .unwrap();
        let exif: &[u8] = &[
            b'E', b'x', b'i', b'f', 0, 0, b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0,
            0, 0, 0, 0, 0,
        ];
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
        app1.extend_from_slice(exif);
        let mut rotated = jpeg[..2].to_vec();
        rotated.extend_from_slice(&app1);
        rotated.extend_from_slice(&jpeg[2..]);
        let bitmap = photo(&rotated).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (20, 40));
    }

    /// A one-page PDF with `lines` in Helvetica, as a digital 2303 is.
    fn digital_pdf(lines: &[&str]) -> Vec<u8> {
        let mut stream = String::from("BT /F1 12 Tf 14 TL 72 760 Td\n");
        for line in lines {
            stream.push_str(&format!("({line}) Tj T*\n"));
        }
        stream.push_str("ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
        );
        pdf
    }

    #[test]
    fn digital_pdfs_give_their_text_layer() {
        let pdf = digital_pdf(&["Certificate of Registration", "TIN 123 456 789 00000", "Taxpayer Name: ABC TRADING"]);
        let text = pdf_text(&pdf).unwrap();
        assert_eq!(text.engine, PDF_TEXT);
        let (fields, _) = super::super::extract::extract(&text.lines);
        assert_eq!(fields.tin.as_deref(), Some("123-456-789-00000"), "{:?}", text.lines);
        assert_eq!(fields.name.as_deref(), Some("ABC TRADING"));
        assert_eq!(pdf_text(&digital_pdf(&[])).unwrap_err(), "PDF scan not supported yet");
    }

    #[test]
    fn bad_files_are_reasons_not_panics() {
        assert!(photo(b"\x89PNG\r\n\x1a\nnot really").is_err());
        assert!(pdf_text(b"%PDF-1.7\nscan").is_err());
    }
}
