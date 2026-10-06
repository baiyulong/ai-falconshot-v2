//! Bitmap encode/decode. The rule from plan §3.3 is absolute: **every** encoded
//! pixel goes through `image` here — `QImage::save` is banned because P5/P6
//! measured it writing nothing at all for TGA/GIF/TIFF.

use crate::frame::Frame;
use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("codec failed: {0}")]
    Codec(String),
    #[error("unsupported format {0:?}")]
    Unsupported(String),
    #[error("image is too large to decode: {0}x{1}")]
    TooLarge(u32, u32),
    #[error("transparent pixels would be lost in {0}; use PNG")]
    TransparencyLost(&'static str),
    #[error(transparent)]
    Frame(#[from] crate::frame::FrameError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Png,
    Jpg,
    Bmp,
    Tga,
    Ico,
    Tiff,
    Gif,
    WebP,
}

/// PRD §5.5.5 promises PNG/JPG/BMP as the save floor; §5.8.3 promises all seven
/// as imports. P5 measured every one of them round-tripping through `image`.
pub const SAVE_FORMATS: [Format; 6] = [
    Format::Png,
    Format::Jpg,
    Format::Bmp,
    Format::Tga,
    Format::Tiff,
    Format::WebP,
];
pub const IMPORT_FORMATS: [Format; 7] = [
    Format::Png,
    Format::Jpg,
    Format::Bmp,
    Format::Tga,
    Format::Ico,
    Format::Tiff,
    Format::Gif,
];

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::Bmp => "bmp",
            Format::Tga => "tga",
            Format::Ico => "ico",
            Format::Tiff => "tiff",
            Format::Gif => "gif",
            Format::WebP => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpg => "JPG",
            Format::Bmp => "BMP",
            Format::Tga => "TGA",
            Format::Ico => "ICO",
            Format::Tiff => "TIFF",
            Format::Gif => "GIF",
            Format::WebP => "WebP",
        }
    }

    pub fn media_type(self) -> &'static str {
        match self {
            Format::Png => "image/png",
            Format::Jpg => "image/jpeg",
            Format::Bmp => "image/bmp",
            Format::Tga => "image/x-tga",
            Format::Ico => "image/x-icon",
            Format::Tiff => "image/tiff",
            Format::Gif => "image/gif",
            Format::WebP => "image/webp",
        }
    }

    pub fn supports_alpha(self) -> bool {
        !matches!(
            self,
            Format::Jpg | Format::Bmp | Format::Tga | Format::Gif | Format::Ico
        )
    }

    pub fn from_ext(name: &str) -> Option<Format> {
        Some(match name.to_ascii_lowercase().as_str() {
            "png" => Format::Png,
            "jpg" | "jpeg" | "jpe" | "jfif" => Format::Jpg,
            "bmp" | "dib" => Format::Bmp,
            "tga" | "icb" | "vda" | "vst" => Format::Tga,
            "ico" | "cur" => Format::Ico,
            "tif" | "tiff" => Format::Tiff,
            "gif" => Format::Gif,
            "webp" => Format::WebP,
            _ => return None,
        })
    }

    fn to_image_format(self) -> ImageFormat {
        match self {
            Format::Png => ImageFormat::Png,
            Format::Jpg => ImageFormat::Jpeg,
            Format::Bmp => ImageFormat::Bmp,
            Format::Tga => ImageFormat::Tga,
            Format::Ico => ImageFormat::Ico,
            Format::Tiff => ImageFormat::Tiff,
            Format::Gif => ImageFormat::Gif,
            Format::WebP => ImageFormat::WebP,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodeOptions {
    pub format: Format,
    /// 1..=100, only meaningful for JPG and WebP.
    pub quality: u8,
    /// Fill instead of refusing when the format has no alpha channel.
    pub flatten_on_lossy: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            format: Format::Png,
            quality: 92,
            flatten_on_lossy: true,
        }
    }
}

pub const ICO_MAX_SIDE: u32 = 256;

/// Encode to bytes. JPG/WebP honour `quality`; ICO is downscaled first because
/// the encoder rejects anything above 256px (P5).
pub fn encode(frame: &Frame, opt: &EncodeOptions) -> Result<Vec<u8>, EncodeError> {
    let mut work = std::borrow::Cow::Borrowed(frame);
    let owned;
    if opt.format == Format::Ico && (frame.width > ICO_MAX_SIDE || frame.height > ICO_MAX_SIDE) {
        let s = (ICO_MAX_SIDE as f64 / frame.width.max(frame.height) as f64).min(1.0);
        owned = frame.resized(
            ((frame.width as f64 * s).round().max(1.0)) as u32,
            ((frame.height as f64 * s).round().max(1.0)) as u32,
            true,
        )?;
        work = std::borrow::Cow::Owned(owned);
    }
    let f = &*work;
    let has_alpha = opt.format.supports_alpha();
    if !has_alpha && f.has_transparency() && !opt.flatten_on_lossy {
        return Err(EncodeError::TransparencyLost(opt.format.label()));
    }
    let prepared = if has_alpha {
        std::borrow::Cow::Borrowed(f)
    } else if f.has_transparency() {
        std::borrow::Cow::Owned(flatten(f, [255, 255, 255, 255])?)
    } else {
        std::borrow::Cow::Borrowed(f)
    };
    let f = &*prepared;
    let mut buf = Vec::new();
    let dyn_img = DynamicImage::ImageRgba8(f.to_image());

    match opt.format {
        Format::Jpg => {
            let rgb = dyn_img.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, opt.quality.clamp(1, 100))
                .encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .map_err(codec)?;
        }
        Format::Ico => image::codecs::ico::IcoEncoder::new(&mut buf)
            .write_image(&f.pixels, f.width, f.height, ExtendedColorType::Rgba8)
            .map_err(codec)?,
        other => {
            let mut cursor = Cursor::new(&mut buf);
            if other == Format::WebP {
                // Screenshots want the lossless branch; `write_to` picks lossy.
                image::codecs::webp::WebPEncoder::new_lossless(&mut cursor)
                    .write_image(&f.pixels, f.width, f.height, ExtendedColorType::Rgba8)
                    .map_err(codec)?;
            } else {
                dyn_img
                    .write_to(&mut cursor, other.to_image_format())
                    .map_err(codec)?;
            }
        }
    }
    Ok(buf)
}

/// Replace alpha with a solid background, for formats that cannot store it.
pub fn flatten(frame: &Frame, background: [u8; 4]) -> Result<Frame, EncodeError> {
    let mut out = Frame::new(frame.width, frame.height)?;
    for i in 0..(frame.width as usize * frame.height as usize) {
        let p = &frame.pixels[i * 4..i * 4 + 4];
        let a = p[3] as u32;
        let mix = |c: u8, b: u8| -> u8 { ((c as u32 * a + b as u32 * (255 - a)) / 255) as u8 };
        out.pixels[i * 4] = mix(p[0], background[0]);
        out.pixels[i * 4 + 1] = mix(p[1], background[1]);
        out.pixels[i * 4 + 2] = mix(p[2], background[2]);
        out.pixels[i * 4 + 3] = 255;
    }
    Ok(out)
}

/// Largest accepted decode, as a guard against a hostile file (PRD §7.2).
pub const DECODE_MAX_PIXELS: u64 = 64 * 1024 * 1024;

pub fn decode(bytes: &[u8]) -> Result<Frame, EncodeError> {
    decode_as(bytes, None)
}

/// Decode with a format hint. `image` cannot sniff TGA — the format has no
/// leading signature — while PRD §5.8.3 promises TGA import from a file name,
/// so every file entry point passes the extension through here.
pub fn decode_as(bytes: &[u8], hint: Option<Format>) -> Result<Frame, EncodeError> {
    let (w, h) = opener(bytes, hint)?
        .into_dimensions()
        .map_err(|e| EncodeError::Codec(e.to_string()))?;
    if (w as u64) * (h as u64) > DECODE_MAX_PIXELS {
        return Err(EncodeError::TooLarge(w, h));
    }
    let img = opener(bytes, hint)?
        .decode()
        .map_err(|e| EncodeError::Codec(e.to_string()))?;
    Ok(Frame::from_image(img.to_rgba8()))
}

fn opener(bytes: &[u8], hint: Option<Format>) -> Result<ImageReader<Cursor<&[u8]>>, EncodeError> {
    let mut reader = ImageReader::new(Cursor::new(bytes));
    match hint {
        Some(fmt) => reader.set_format(fmt.to_image_format()),
        None => {
            reader = reader
                .with_guessed_format()
                .map_err(|e| EncodeError::Codec(e.to_string()))?;
        }
    }
    Ok(reader)
}

/// What the bytes themselves claim. `None` for a format without a signature.
pub fn detect_format(bytes: &[u8]) -> Option<Format> {
    let f = opener(bytes, None).ok()?.format()?;
    Some(match f {
        ImageFormat::Png => Format::Png,
        ImageFormat::Jpeg => Format::Jpg,
        ImageFormat::Bmp => Format::Bmp,
        ImageFormat::Tga => Format::Tga,
        ImageFormat::Ico => Format::Ico,
        ImageFormat::Tiff => Format::Tiff,
        ImageFormat::Gif => Format::Gif,
        ImageFormat::WebP => Format::WebP,
        _ => return None,
    })
}

pub fn decode_file(path: &std::path::Path) -> Result<Frame, EncodeError> {
    let bytes = std::fs::read(path)?;
    let hint = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(Format::from_ext);
    let mut frame = decode_as(&bytes, hint)?;
    if hint == Some(Format::Ico) && frame.width > ICO_MAX_SIDE {
        frame = frame.resized(ICO_MAX_SIDE, ICO_MAX_SIDE, true)?;
    }
    Ok(frame)
}

pub fn save(frame: &Frame, path: &std::path::Path, opt: &EncodeOptions) -> Result<(), EncodeError> {
    let bytes = encode(frame, opt)?;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    // Same tmp+rename discipline as config: a half-written PNG is worse than an
    // error dialog (PRD §7.2/§8.3).
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("img")
    ));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn codec<E: std::fmt::Display>(e: E) -> EncodeError {
    EncodeError::Codec(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;

    fn sample(w: u32, h: u32) -> Frame {
        let mut f = Frame::filled(w, h, [10, 200, 30, 255]).unwrap();
        for y in 0..h / 2 {
            f.fill_rect(
                &crate::geometry::PhysRect::new(0, y as i32, w, 1),
                [200, 20, 20, 128],
            );
        }
        f
    }

    #[test]
    fn all_promised_formats_round_trip() {
        for fmt in [
            Format::Png,
            Format::Jpg,
            Format::Bmp,
            Format::Tga,
            Format::Tiff,
            Format::Gif,
            Format::WebP,
            Format::Ico,
        ] {
            let f = sample(64, 48);
            let opt = EncodeOptions {
                format: fmt,
                ..Default::default()
            };
            let bytes = encode(&f, &opt).unwrap_or_else(|e| panic!("{fmt:?} encode: {e}"));
            assert!(!bytes.is_empty(), "{fmt:?} produced no bytes");
            let back =
                decode_as(&bytes, Some(fmt)).unwrap_or_else(|e| panic!("{fmt:?} decode: {e}"));
            assert_eq!(back.width, f.width, "{fmt:?} width");
            assert_eq!(back.height, f.height, "{fmt:?} height");
            // Sniffing covers every format but TGA, which carries no leading
            // signature — hence the extension hint on the import path.
            if fmt == Format::Tga {
                assert_eq!(detect_format(&bytes), None);
                assert!(decode(&bytes).is_err());
            } else {
                assert_eq!(detect_format(&bytes), Some(fmt), "{fmt:?} sniff");
                assert!(decode(&bytes).is_ok(), "{fmt:?} sniffed decode");
            }
            assert_eq!(Format::from_ext(fmt.ext()), Some(fmt));
        }
    }

    #[test]
    fn alpha_survives_png_and_dies_in_jpg() {
        let f = sample(32, 32);
        assert!(f.has_transparency());
        let png = encode(&f, &EncodeOptions::default()).unwrap();
        assert!(decode(&png).unwrap().has_transparency());
        let jpg = encode(
            &f,
            &EncodeOptions {
                format: Format::Jpg,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!decode(&jpg).unwrap().has_transparency());
        assert!(matches!(
            encode(
                &f,
                &EncodeOptions {
                    format: Format::Jpg,
                    flatten_on_lossy: false,
                    ..Default::default()
                }
            ),
            Err(EncodeError::TransparencyLost("JPG"))
        ));
    }

    #[test]
    fn ico_is_clamped_not_rejected() {
        let big = sample(600, 400);
        let bytes = encode(
            &big,
            &EncodeOptions {
                format: Format::Ico,
                ..Default::default()
            },
        )
        .unwrap();
        let back = decode(&bytes).unwrap();
        assert!(back.width <= ICO_MAX_SIDE && back.height <= ICO_MAX_SIDE);
    }

    #[test]
    fn jpg_quality_moves_the_size() {
        let noisy = {
            let mut f = Frame::new(96, 96).unwrap();
            for y in 0..96 {
                for x in 0..96 {
                    f.set(
                        x,
                        y,
                        [(x * 7) as u8, (y * 13) as u8, ((x + y) * 3) as u8, 255],
                    );
                }
            }
            f
        };
        let hi = encode(
            &noisy,
            &EncodeOptions {
                format: Format::Jpg,
                quality: 95,
                flatten_on_lossy: true,
            },
        )
        .unwrap();
        let lo = encode(
            &noisy,
            &EncodeOptions {
                format: Format::Jpg,
                quality: 20,
                flatten_on_lossy: true,
            },
        )
        .unwrap();
        assert!(hi.len() > lo.len(), "{} vs {}", hi.len(), lo.len());
    }

    #[test]
    fn garbage_is_reported_not_panicked() {
        assert!(decode(&[0, 1, 2, 3, 4, 5]).is_err());
        assert_eq!(detect_format(&[0, 1, 2]), None);
    }
}
