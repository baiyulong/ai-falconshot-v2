//! The one bitmap type that crosses module boundaries: tight RGBA8, row-major.
//!
//! `image::RgbaImage` is used at the edges (decode/encode), but products of the
//! capture path stay in `Frame` so the annotation rasterizer and the FFI layer
//! agree on a layout without pulling `image` generics everywhere.

use crate::geometry::PhysRect;
use image::RgbaImage;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum FrameError {
    #[error("region {0:?} is outside the {1}x{2} frame")]
    OutOfBounds(PhysRect, u32, u32),
    #[error("frame is too large: {0}x{1}")]
    TooLarge(u32, u32),
    #[error("pixel count {pixels} does not match {width}x{height}")]
    LengthMismatch {
        pixels: usize,
        width: usize,
        height: usize,
    },
}

/// Refuse absurd allocations before they reach `Vec` (PRD §7.2).
pub const MAX_DIM: u32 = 16_384;
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

fn pixel_count(width: u32, height: u32) -> Result<u64, FrameError> {
    let n = (width as u64) * (height as u64);
    if width > MAX_DIM || height > MAX_DIM || n > MAX_PIXELS {
        return Err(FrameError::TooLarge(width, height));
    }
    Ok(n)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Result<Self, FrameError> {
        let n = pixel_count(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0u8; (n * 4) as usize],
        })
    }

    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Result<Self, FrameError> {
        let n = pixel_count(width, height)?;
        let mut pixels = vec![0u8; (n * 4) as usize];
        pixels.as_chunks_mut::<4>().0.fill(rgba);
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, FrameError> {
        if pixels.len() != (width as usize) * (height as usize) * 4 {
            return Err(FrameError::LengthMismatch {
                pixels: pixels.len(),
                width: width as usize,
                height: height as usize,
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn from_image(img: RgbaImage) -> Self {
        let (width, height) = img.dimensions();
        Self {
            width,
            height,
            pixels: img.into_raw(),
        }
    }

    pub fn to_image(&self) -> RgbaImage {
        // `from_raw` cannot fail here: every constructor enforces len == w*h*4.
        image::ImageBuffer::from_raw(self.width, self.height, self.pixels.clone())
            .expect("frame length invariant violated")
    }

    pub fn row(&self, y: u32) -> &[u8] {
        let s = self.width as usize * 4;
        &self.pixels[y as usize * s..y as usize * s + s]
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = &self.pixels[i..i + 4];
        [p[0], p[1], p[2], p[3]]
    }

    pub fn set(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[i..i + 4].copy_from_slice(&rgba);
    }

    pub fn bounds(&self) -> PhysRect {
        PhysRect::new(0, 0, self.width, self.height)
    }

    pub fn rect(&self, at: crate::geometry::PhysPoint) -> PhysRect {
        PhysRect::new(at.x, at.y, self.width, self.height)
    }

    /// PRD §7.2: a single decode failure must not take the process down, so
    /// every pixel reader goes through bounds checks here.
    pub fn crop(&self, r: &PhysRect) -> Result<Self, FrameError> {
        let x = r.x.max(0) as u32;
        let y = r.y.max(0) as u32;
        if r.x < 0 || r.y < 0 || x + r.w > self.width || y + r.h > self.height {
            return Err(FrameError::OutOfBounds(*r, self.width, self.height));
        }
        if r.w == 0 || r.h == 0 {
            return Err(FrameError::OutOfBounds(*r, self.width, self.height));
        }
        let src_stride = self.width as usize * 4;
        let dst_stride = r.w as usize * 4;
        let mut out = vec![0u8; dst_stride * r.h as usize];
        for row in 0..r.h as usize {
            let s = (y as usize + row) * src_stride + x as usize * 4;
            out[row * dst_stride..(row + 1) * dst_stride]
                .copy_from_slice(&self.pixels[s..s + dst_stride]);
        }
        Ok(Self {
            width: r.w,
            height: r.h,
            pixels: out,
        })
    }

    /// Paste `src` with its top-left at `at`; parts falling outside are dropped.
    /// Returns the region actually written, in destination coordinates.
    pub fn paste(&mut self, src: &Frame, at: crate::geometry::PhysPoint) -> Option<PhysRect> {
        let dst = PhysRect::new(at.x, at.y, src.width, src.height);
        let hit = dst.intersection(&self.bounds())?;
        let stride = self.width as usize * 4;
        let src_stride = src.width as usize * 4;
        for row in 0..hit.h {
            let dy = (hit.y + row as i32) as usize;
            let sy = (hit.y - at.y + row as i32) as usize;
            let dstart = dy * stride + hit.x as usize * 4;
            let sstart = sy * src_stride + (hit.x - at.x) as usize * 4;
            self.pixels[dstart..dstart + hit.w as usize * 4]
                .copy_from_slice(&src.pixels[sstart..sstart + hit.w as usize * 4]);
        }
        Some(hit)
    }

    /// Source-over blend of a whole frame, used by the annotation overlay so a
    /// partially transparent layer composites correctly.
    pub fn blend_over(&mut self, src: &Frame, at: crate::geometry::PhysPoint, opacity: f64) {
        let a = opacity.clamp(0.0, 1.0);
        if a == 0.0 {
            return;
        }
        let dst = PhysRect::new(at.x, at.y, src.width, src.height);
        let Some(hit) = dst.intersection(&self.bounds()) else {
            return;
        };
        let (hx, hy) = (hit.x as u32, hit.y as u32);
        let (ox, oy) = ((hit.x - at.x) as u32, (hit.y - at.y) as u32);
        for y in 0..hit.h {
            for x in 0..hit.w {
                let s = src.get(x + ox, y + oy);
                if s[3] == 0 {
                    continue;
                }
                let dp = self.get(x + hx, y + hy);
                self.set(x + hx, y + hy, blend_rgba(dp, s, (s[3] as f64 * a) as u8));
            }
        }
    }

    pub fn fill_rect(&mut self, r: &PhysRect, rgba: [u8; 4]) {
        let Some(hit) = r.intersection(&self.bounds()) else {
            return;
        };
        let (hx, hy) = (hit.x as u32, hit.y as u32);
        for y in 0..hit.h {
            for x in 0..hit.w {
                self.set(x + hx, y + hy, rgba);
            }
        }
    }

    pub fn fill_rect_blend(&mut self, r: &PhysRect, rgba: [u8; 4]) {
        let Some(hit) = r.intersection(&self.bounds()) else {
            return;
        };
        let (hx, hy) = (hit.x as u32, hit.y as u32);
        for y in 0..hit.h {
            for x in 0..hit.w {
                let p = self.get(x + hx, y + hy);
                self.set(x + hx, y + hy, blend_rgba(p, rgba, rgba[3]));
            }
        }
    }

    pub fn has_transparency(&self) -> bool {
        self.pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 255)
    }

    /// Fast uniformity test used to decide "is this screenshot blank", and by
    /// the pixel-gauge checks in the spike-derived tests.
    pub fn is_uniform(&self) -> Option<[u8; 4]> {
        if self.pixels.is_empty() {
            return None;
        }
        let first = [
            self.pixels[0],
            self.pixels[1],
            self.pixels[2],
            self.pixels[3],
        ];
        if self.pixels.as_chunks::<4>().0.iter().all(|p| *p == first) {
            Some(first)
        } else {
            None
        }
    }

    pub fn flipped_horizontal(&self) -> Self {
        let mut out = self.clone();
        for y in 0..self.height {
            for x in 0..self.width {
                out.set(x, y, self.get(self.width - 1 - x, y));
            }
        }
        out
    }

    pub fn flipped_vertical(&self) -> Self {
        let mut out = self.clone();
        for y in 0..self.height {
            for x in 0..self.width {
                out.set(x, y, self.get(x, self.height - 1 - y));
            }
        }
        out
    }

    /// 90° clockwise (PRD §5.9.5 uses 90° as its basic unit).
    pub fn rotated_90_cw(&self) -> Self {
        let mut out = Self::new(self.height, self.width).expect("swap of a valid frame");
        for y in 0..self.height {
            for x in 0..self.width {
                out.set(self.height - 1 - y, x, self.get(x, y));
            }
        }
        out
    }

    pub fn rotated_90_ccw(&self) -> Self {
        let mut out = Self::new(self.height, self.width).expect("swap of a valid frame");
        for y in 0..self.height {
            for x in 0..self.width {
                out.set(y, self.width - 1 - x, self.get(x, y));
            }
        }
        out
    }

    /// Scale to exactly `width` x `height`. Both flavours answer the size they
    /// were asked for: `image`'s `resize` keeps the source aspect ratio and can
    /// hand back a frame smaller than requested, which a caller laying a frame
    /// over a picture region would then read past the end of.
    pub fn resized(&self, width: u32, height: u32, smooth: bool) -> Result<Self, FrameError> {
        if width == 0 || height == 0 {
            return Err(FrameError::TooLarge(width, height));
        }
        pixel_count(width, height)?;
        if !smooth {
            return Ok(self.nearest_resized(width, height));
        }
        let img = image::DynamicImage::ImageRgba8(self.to_image()).resize_exact(
            width,
            height,
            image::imageops::FilterType::Lanczos3,
        );
        Ok(Self::from_image(img.to_rgba8()))
    }

    /// Pixel-level scaling (PRD §5.9.2 offers both flavours): integer-ish
    /// nearest neighbour, no filtering.
    pub fn nearest_resized(&self, width: u32, height: u32) -> Self {
        let mut out = Self {
            width,
            height,
            pixels: vec![0u8; (width as usize) * (height as usize) * 4],
        };
        for y in 0..height {
            let sy = ((y as u64 * self.height as u64) / height as u64).min(self.height as u64 - 1)
                as u32;
            for x in 0..width {
                let sx = ((x as u64 * self.width as u64) / width as u64).min(self.width as u64 - 1)
                    as u32;
                out.set(x, y, self.get(sx, sy));
            }
        }
        out
    }

    pub fn thumbnail(&self, max: u32) -> Result<Self, FrameError> {
        let longest = self.width.max(self.height).max(1);
        let s = (max.min(longest) as f64) / longest as f64;
        if s >= 1.0 {
            return Ok(self.clone());
        }
        self.resized(
            ((self.width as f64) * s).round().max(1.0) as u32,
            ((self.height as f64) * s).round().max(1.0) as u32,
            true,
        )
    }

    pub fn scaled_by(&self, factor: f64) -> Result<Self, FrameError> {
        if !factor.is_finite() || factor <= 0.0 {
            return Ok(self.clone());
        }
        let w = ((self.width as f64 * factor).round().max(1.0) as u64).min(MAX_DIM as u64) as u32;
        let h = ((self.height as f64 * factor).round().max(1.0) as u64).min(MAX_DIM as u64) as u32;
        self.resized(w, h, true)
    }
}

/// `src` over `dst` with explicit alpha in 0..=255.
pub fn blend_rgba(dst: [u8; 4], src: [u8; 4], alpha: u8) -> [u8; 4] {
    if alpha == 0 {
        return dst;
    }
    if alpha == 255 && src[3] == 255 {
        return src;
    }
    let sa = (src[3] as u32 * alpha as u32) / 255;
    if sa == 0 {
        return dst;
    }
    let da = 255 - sa;
    let mix = |s: u8, d: u8| -> u8 { ((s as u32 * sa + d as u32 * da) / 255) as u8 };
    [
        mix(src[0], dst[0]),
        mix(src[1], dst[1]),
        mix(src[2], dst[2]),
        (sa + (dst[3] as u32 * da) / 255).min(255) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysPoint;

    #[test]
    fn crop_is_bounds_checked() {
        let f = Frame::filled(10, 10, [1, 2, 3, 4]).unwrap();
        assert_eq!(f.crop(&PhysRect::new(2, 2, 5, 5)).unwrap().width, 5);
        assert!(f.crop(&PhysRect::new(8, 8, 5, 5)).is_err());
        assert!(f.crop(&PhysRect::new(-1, 0, 5, 5)).is_err());
        assert!(f.crop(&PhysRect::new(0, 0, 0, 5)).is_err());
    }

    #[test]
    fn paste_clips_at_the_edges() {
        let mut dst = Frame::new(10, 10).unwrap();
        let src = Frame::filled(6, 6, [255, 0, 0, 255]).unwrap();
        assert_eq!(
            dst.paste(&src, PhysPoint::new(7, 7)),
            Some(PhysRect::new(7, 7, 3, 3))
        );
        assert_eq!(dst.get(9, 9), [255, 0, 0, 255]);
        assert_eq!(dst.get(6, 6), [0, 0, 0, 0]);
        assert_eq!(dst.paste(&src, PhysPoint::new(20, 20)), None);
    }

    #[test]
    fn nearest_resize_keeps_hard_edges() {
        let mut f = Frame::new(2, 2).unwrap();
        f.set(0, 0, [255, 0, 0, 255]);
        let g = f.nearest_resized(4, 4);
        assert_eq!(g.get(1, 1), [255, 0, 0, 255]);
        assert_eq!(g.get(2, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn rotation_is_lossless_and_invertible() {
        let mut f = Frame::new(3, 2).unwrap();
        f.set(0, 0, [9, 9, 9, 255]);
        let r = f.rotated_90_cw();
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(r.get(1, 0), [9, 9, 9, 255]);
        assert_eq!(r.rotated_90_ccw(), f);
    }

    #[test]
    fn blend_matches_source_over() {
        assert_eq!(
            blend_rgba([0, 0, 0, 255], [255, 255, 255, 255], 128)[0],
            128
        );
        assert_eq!(
            blend_rgba([10, 10, 10, 255], [200, 0, 0, 0], 255),
            [10, 10, 10, 255]
        );
    }
}
