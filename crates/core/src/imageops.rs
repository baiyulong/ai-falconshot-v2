//! Whole-image effects that the output chain and the annotation tools share:
//! mosaic, blur, rounded corners, and the border/shadow plate (§5.5.6, §5.5.7,
//! §5.7.9, §5.7.10).
//!
//! Every function here is pure: it reads a [`Frame`] and returns a new one, so
//! the same code serves the live preview (a brush stroke re-rasterised per
//! dirty rect) and the final export. Nothing calls `QImage::save` or any Qt
//! type — plan §3.3 keeps bitmap work on the Rust side.

use crate::encode::EncodeError;
use crate::frame::{blend_rgba, Frame, FrameError};
use crate::geometry::{PhysPoint, PhysRect};

/// What a pixelated block looks like: the mean of the block, hard edges.
pub fn mosaic(src: &Frame, block: u32) -> Result<Frame, FrameError> {
    if block <= 1 {
        return Ok(src.clone());
    }
    let mut out = Frame::new(src.width, src.height)?;
    for by in (0..src.height).step_by(block as usize) {
        for bx in (0..src.width).step_by(block as usize) {
            let bw = block.min(src.width - bx);
            let bh = block.min(src.height - by);
            let mut sum = [0u64; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    for (i, v) in src.get(x, y).iter().enumerate() {
                        sum[i] += *v as u64;
                    }
                }
            }
            let n = bw as u64 * bh as u64;
            let avg = [
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                // Alpha averages too: a half-transparent mosaic stays half
                // transparent instead of turning the holes opaque.
                (sum[3] / n) as u8,
            ];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    out.set(x, y, avg);
                }
            }
        }
    }
    Ok(out)
}

/// A single-pass box blur of radius `r`, sampled from the untouched source so
/// the result does not depend on iteration order. Edges clamp rather than
/// wrap: a screenshot has no pixels beyond its own border to borrow.
pub fn blur(src: &Frame, radius: u32) -> Result<Frame, FrameError> {
    if radius == 0 {
        return Ok(src.clone());
    }
    let mut out = Frame::new(src.width, src.height)?;
    for y in 0..src.height {
        for x in 0..src.width {
            let (x0, x1) = (
                x.saturating_sub(radius),
                x.saturating_add(radius).min(src.width - 1),
            );
            let (y0, y1) = (
                y.saturating_sub(radius),
                y.saturating_add(radius).min(src.height - 1),
            );
            let mut sum = [0u64; 4];
            for sy in y0..=y1 {
                for sx in x0..=x1 {
                    for (i, v) in src.get(sx, sy).iter().enumerate() {
                        sum[i] += *v as u64;
                    }
                }
            }
            let n = (x1 - x0 + 1) as u64 * (y1 - y0 + 1) as u64;
            out.set(
                x,
                y,
                [
                    (sum[0] / n) as u8,
                    (sum[1] / n) as u8,
                    (sum[2] / n) as u8,
                    (sum[3] / n) as u8,
                ],
            );
        }
    }
    Ok(out)
}

/// Mosaic or blur only inside `area`, leaving the rest of the frame as it was.
/// The area is clipped to the frame, so a stroke that hangs off the edge is
/// still correct.
pub fn effect_in(src: &Frame, area: &PhysRect, kind: Effect) -> Result<Frame, EncodeError> {
    let Some(hit) = area.intersection(&src.bounds()) else {
        return Ok(src.clone());
    };
    if hit.is_empty() {
        return Ok(src.clone());
    }
    let cropped = src.crop(&hit)?;
    let effected = match kind {
        Effect::Mosaic { block } => mosaic(&cropped, block)?,
        Effect::Blur { radius } => blur(&cropped, radius)?,
    };
    let mut out = src.clone();
    out.paste(&effected, PhysPoint::new(hit.x, hit.y));
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// §5.7.9 像素化: the block edge length in physical pixels.
    Mosaic { block: u32 },
    /// §5.7.10 模糊强度: the box radius in physical pixels.
    Blur { radius: u32 },
}

/// Composite `effect` over `base` weighted by `mask`'s alpha, with `mask` and
/// `effect` placed at `at`. This is how a feathered brush or a rounded mosaic
/// patch lands on the picture: the shape lives in the mask, the pixels in the
/// effect.
pub fn blend_masked(base: &mut Frame, effect: &Frame, mask: &Frame, at: PhysPoint) {
    let Some(hit) =
        PhysRect::new(at.x, at.y, effect.width, effect.height).intersection(&base.bounds())
    else {
        return;
    };
    for y in hit.y..hit.bottom() {
        for x in hit.x..hit.right() {
            let (dx, dy) = (x as u32, y as u32);
            let (sx, sy) = ((x - at.x) as u32, (y - at.y) as u32);
            if sx >= effect.width || sy >= effect.height || sx >= mask.width || sy >= mask.height {
                continue;
            }
            let m = mask.get(sx, sy)[3];
            if m == 0 {
                continue;
            }
            let dst = base.get(dx, dy);
            base.set(dx, dy, blend_rgba(dst, effect.get(sx, sy), m));
        }
    }
}

/// §5.5.6: what sits outside the rounded corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outside {
    /// The default, and the only shape that survives PNG.
    Transparent,
    /// Filled, which is what a JPG export has to use anyway (§8.2).
    Fill([u8; 4]),
}

/// Round the four corners by `radius` physical pixels. Corners are the only
/// place anything changes, so the copy is cheap and the centre is byte-exact.
pub fn rounded(src: &Frame, radius: u32, outside: Outside) -> Result<Frame, FrameError> {
    if radius == 0 {
        return Ok(src.clone());
    }
    let r = radius.min(src.width / 2).min(src.height / 2) as i32;
    if r <= 0 {
        return Ok(src.clone());
    }
    let mut out = match outside {
        Outside::Transparent => Frame::new(src.width, src.height)?,
        Outside::Fill(c) => Frame::filled(src.width, src.height, c)?,
    };
    out.pixels[..].copy_from_slice(&src.pixels[..]);
    for y in 0..r {
        for x in 0..r {
            // Distance from the arc centre, measured in pixel centres with the
            // corner's own square as the bounding box. Doubling everything keeps
            // the half-pixel exact: a pixel centre sits at (x+0.5, y+0.5) and
            // the arc centre at (r, r), so the comparison is
            // (2(r-x)-1)² + (2(r-y)-1)² > (2r)².
            let dx = 2 * (r - x) - 1;
            let dy = 2 * (r - y) - 1;
            if dx * dx + dy * dy > 4 * r * r {
                for (cx, cy) in [
                    (x as u32, y as u32),
                    (src.width - 1 - x as u32, y as u32),
                    (x as u32, src.height - 1 - y as u32),
                    (src.width - 1 - x as u32, src.height - 1 - y as u32),
                ] {
                    let clear = match outside {
                        Outside::Transparent => [0, 0, 0, 0],
                        Outside::Fill(c) => c,
                    };
                    out.set(cx, cy, clear);
                }
            }
        }
    }
    Ok(out)
}

/// §5.5.7 border and drop shadow, rendered into the exported image rather than
/// onto a window, so what the user sees is what the file contains.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plate {
    /// Line width drawn around the picture, in physical pixels.
    pub border_width: u32,
    pub border_color: [u8; 4],
    /// Extra transparent margin on every side, in physical pixels.
    pub shadow_margin: u32,
    /// How far the shadow fades inside that margin.
    pub shadow_radius: u32,
    pub shadow_color: [u8; 4],
}

impl Default for Plate {
    fn default() -> Self {
        Plate {
            border_width: 0,
            border_color: [0, 0, 0, 0],
            shadow_margin: 0,
            shadow_radius: 0,
            shadow_color: [0, 0, 0, 128],
        }
    }
}

impl Plate {
    /// Nothing to add: the export keeps its exact size and pixels.
    pub fn is_identity(&self) -> bool {
        self.border_width == 0 && self.shadow_margin == 0
    }

    pub fn extra(&self) -> u32 {
        // Only the shadow costs margin: the border is drawn over the picture's
        // own edge, so a plate with a border and no shadow keeps its size.
        self.shadow_margin
    }
}

/// Grow `src` by the plate's margins and draw shadow then border on it.
pub fn plate(src: &Frame, plate: &Plate) -> Result<Frame, FrameError> {
    if plate.is_identity() {
        return Ok(src.clone());
    }
    let grow = plate.extra();
    let out_w = src.width + grow * 2;
    let out_h = src.height + grow * 2;
    let mut out = Frame::new(out_w, out_h)?;
    if plate.shadow_margin > 0 {
        // A shadow is a distance field: how far a pixel sits outside the
        // picture, faded over `shadow_radius`. Cheaper and steadier than a
        // convolution, and it never bleeds past the margin.
        let fade = plate.shadow_radius.max(1).min(plate.shadow_margin) as i32;
        let alpha = plate.shadow_color[3] as f64 / 255.0;
        let (x0, y0) = (grow as i32, grow as i32);
        let (x1, y1) = (x0 + src.width as i32, y0 + src.height as i32);
        for y in 0..out_h {
            for x in 0..out_w {
                let outside = |p: i32, lo: i32, hi: i32| -> i32 {
                    if p < lo {
                        lo - p
                    } else if p >= hi {
                        p - hi + 1
                    } else {
                        0
                    }
                };
                let d = outside(x as i32, x0, x1).max(outside(y as i32, y0, y1));
                if d >= grow as i32 {
                    continue;
                }
                // d counts from the picture edge outward, so the edge is the
                // darkest pixel and the far side of the margin is clear.
                let t = (d as f64 / fade as f64).min(1.0);
                let a = (alpha * (1.0 - t) * 255.0) as u8;
                if a == 0 {
                    continue;
                }
                let mut c = plate.shadow_color;
                c[3] = a;
                out.set(x, y, c);
            }
        }
    }
    out.paste(src, PhysPoint::new(grow as i32, grow as i32));
    if plate.border_width > 0 {
        let b = plate.border_width;
        let inner = PhysRect::new(grow as i32, grow as i32, src.width, src.height);
        // The border sits on the picture's edge, inside the plate: it does not
        // resize the content, it draws over it.
        for i in 0..b {
            out.fill_rect_blend(
                &PhysRect::new(inner.x, inner.y + i as i32, inner.w, 1),
                plate.border_color,
            );
            out.fill_rect_blend(
                &PhysRect::new(inner.x, inner.bottom() - 1 - i as i32, inner.w, 1),
                plate.border_color,
            );
            out.fill_rect_blend(
                &PhysRect::new(inner.x + i as i32, inner.y, 1, inner.h),
                plate.border_color,
            );
            out.fill_rect_blend(
                &PhysRect::new(inner.right() - 1 - i as i32, inner.y, 1, inner.h),
                plate.border_color,
            );
        }
    }
    Ok(out)
}

/// §5.5.8: the region a refresh re-reads, clamped to a possibly changed
/// desktop. `None` means the area has gone off every screen and the user has
/// to pick again.
pub fn still_valid(area: &PhysRect, desktop: &PhysRect) -> Option<PhysRect> {
    desktop.intersection(area)
}

/// A brush shape mask: opaque inside the shape, transparent outside, with the
/// edge ramped over `feather` pixels so §5.7.10's "笔刷大小和形状" preview does
/// not look cut out with scissors.
pub fn shape_mask(
    width: u32,
    height: u32,
    shape: Shape,
    feather: u32,
) -> Result<Frame, FrameError> {
    let mut out = Frame::new(width, height)?;
    for y in 0..height {
        for x in 0..width {
            let (nx, ny) = (x as f64 + 0.5, y as f64 + 0.5);
            let inside = match shape {
                Shape::Rect => true,
                Shape::Circle => {
                    let rx = width as f64 / 2.0;
                    let ry = height as f64 / 2.0;
                    let dx = (nx - rx) / rx.max(1e-6);
                    let dy = (ny - ry) / ry.max(1e-6);
                    dx * dx + dy * dy <= 1.0
                }
            };
            let a = if feather == 0 || shape == Shape::Rect {
                if inside {
                    255.0
                } else {
                    0.0
                }
            } else {
                // Distance to the shape edge, in pixels, signed inward.
                let rx = width as f64 / 2.0;
                let ry = height as f64 / 2.0;
                let d = ((nx - rx) / rx).hypot((ny - ry) / ry);
                let edge = (1.0 - d) * rx.min(ry);
                (edge / feather as f64).clamp(0.0, 1.0) * 255.0
            };
            out.set(x, y, [0, 0, 0, a as u8]);
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Rect,
    Circle,
}

/// A 1 px hairline in the accent colour, used by the selection rectangle and
/// by `Plate` defaults; kept here so the QML side and the export side agree.
pub fn outline(src: &mut Frame, r: &PhysRect, color: [u8; 4], width: u32) {
    let w = width.max(1);
    for i in 0..w {
        let y0 = r.y + i as i32;
        let y1 = r.bottom() - 1 - i as i32;
        if y0 < r.bottom() && y0 >= 0 {
            src.fill_rect_blend(&PhysRect::new(r.x, y0, r.w, 1), color);
        }
        if y1 >= r.y && y1 < src.height as i32 {
            src.fill_rect_blend(&PhysRect::new(r.x, y1, r.w, 1), color);
        }
        let x0 = r.x + i as i32;
        let x1 = r.right() - 1 - i as i32;
        if x0 >= 0 {
            src.fill_rect_blend(&PhysRect::new(x0, r.y, 1, r.h), color);
        }
        if x1 < src.width as i32 {
            src.fill_rect_blend(&PhysRect::new(x1, r.y, 1, r.h), color);
        }
    }
}

/// Refuse a JPEG export whose transparency would silently turn black/white
/// (§5.5.6 rule, §8.2), unless the caller already flattened it onto a fill.
pub fn warns_about_transparency(
    frame: &Frame,
    format: crate::encode::Format,
) -> Result<(), EncodeError> {
    if !matches!(
        format,
        crate::encode::Format::Jpg | crate::encode::Format::Bmp | crate::encode::Format::Tga
    ) {
        return Ok(());
    }
    if frame.has_transparency() {
        return Err(EncodeError::TransparencyLost(format.ext()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::Format;

    /// 4x4 with a distinct value per pixel, opaque.
    fn gradient() -> Frame {
        let mut f = Frame::new(4, 4).unwrap();
        for y in 0..4 {
            for x in 0..4 {
                f.set(x, y, [(x + y * 4) as u8 * 16, 0, 0, 255]);
            }
        }
        f
    }

    #[test]
    fn mosaic_is_the_block_average_with_hard_edges() {
        let m = mosaic(&gradient(), 2).unwrap();
        // Block (0,0): pixels 0,1,4,5 → 0,16,64,80 → mean 40.
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(m.get(x, y), [40, 0, 0, 255], "{x},{y}");
        }
        // Block (2,0): 2,3,6,7 → 32,48,96,112 → mean 72.
        assert_eq!(m.get(2, 0), [72, 0, 0, 255]);
        assert_eq!(m.get(3, 1), [72, 0, 0, 255]);
        // A block at the edge that is cut short averages over what exists.
        let odd = Frame::new(3, 1).unwrap();
        let mut odd = odd;
        odd.set(0, 0, [10, 0, 0, 255]);
        odd.set(1, 0, [20, 0, 0, 255]);
        odd.set(2, 0, [33, 0, 0, 255]);
        let m = mosaic(&odd, 2).unwrap();
        assert_eq!(m.get(0, 0), [15, 0, 0, 255]);
        assert_eq!(m.get(2, 0), [33, 0, 0, 255]);
    }

    #[test]
    fn blur_averages_the_neighbourhood_and_keeps_uniforms() {
        let flat = Frame::filled(5, 5, [123, 45, 6, 255]).unwrap();
        assert_eq!(blur(&flat, 2).unwrap(), flat);
        let b = blur(&gradient(), 1).unwrap();
        // Centre of a 4x4 gradient at (1,1): the 2x2 block 0,1,4,5 plus the
        // pixels to the right and below, i.e. 3x3 around (1,1) clamped at the
        // far edge → mean of 9 values.
        let mut sum = 0u64;
        for y in 0..3 {
            for x in 0..3 {
                sum += (x + y * 4) as u64 * 16;
            }
        }
        assert_eq!(b.get(1, 1)[0], (sum / 9) as u8);
        // A corner only has four neighbours, and must not borrow a wrapped row:
        // the mean of 0, 16, 64 and 80.
        assert_eq!(b.get(0, 0)[0], 40);
        // A radius larger than the frame saturates the window instead of
        // wrapping the coordinate: every pixel sees the whole 4x4, mean 120.
        assert_eq!(
            blur(&gradient(), 1_000).unwrap().get(2, 1),
            [120, 0, 0, 255]
        );
        assert_eq!(blur(&gradient(), 0).unwrap(), gradient());
    }

    #[test]
    fn an_effect_lands_only_inside_its_area() {
        let src = gradient();
        let out = effect_in(
            &src,
            &PhysRect::new(0, 0, 2, 2),
            Effect::Mosaic { block: 2 },
        )
        .unwrap();
        assert_eq!(out.get(0, 0), [40, 0, 0, 255]);
        // Outside the area the original survives untouched.
        assert_eq!(out.get(3, 3), src.get(3, 3));
        // An area hanging off the edge is clipped, not rejected.
        let clipped =
            effect_in(&src, &PhysRect::new(3, 3, 8, 8), Effect::Blur { radius: 4 }).unwrap();
        assert_eq!(clipped.get(3, 3), src.get(3, 3));
        assert_eq!(clipped.width, 4);
    }

    #[test]
    fn a_mask_decides_how_much_of_the_effect_shows() {
        let mut base = Frame::new(2, 1).unwrap();
        base.set(0, 0, [0, 0, 0, 255]);
        base.set(1, 0, [0, 0, 0, 255]);
        let effect = Frame::filled(2, 1, [255, 0, 0, 255]).unwrap();
        let mut mask = Frame::new(2, 1).unwrap();
        mask.set(0, 0, [0, 0, 0, 255]);
        mask.set(1, 0, [0, 0, 0, 51]);
        blend_masked(&mut base, &effect, &mask, PhysPoint::new(0, 0));
        assert_eq!(base.get(0, 0), [255, 0, 0, 255]);
        // 51/255 of the red over black: a soft brush edge, not a hard one.
        let r = base.get(1, 0)[0];
        assert!((20..60).contains(&r), "{r}");
        // A mask placed outside the frame is a no-op, not a panic.
        blend_masked(&mut base, &effect, &mask, PhysPoint::new(40, 40));
        assert_eq!(base.get(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn rounded_corners_clear_the_arc_and_keep_the_middle() {
        let src = Frame::filled(10, 10, [10, 20, 30, 255]).unwrap();
        let r = rounded(&src, 3, Outside::Transparent).unwrap();
        assert_eq!(r.get(0, 0), [0, 0, 0, 0]);
        assert_eq!(r.get(9, 0), [0, 0, 0, 0]);
        assert_eq!(r.get(0, 9), [0, 0, 0, 0]);
        assert_eq!(r.get(3, 3), [10, 20, 30, 255]);
        assert_eq!(r.get(5, 0), [10, 20, 30, 255]);
        assert_eq!(r.get(0, 2), [10, 20, 30, 255]);
        // Filled corners are what a JPG needs.
        let f = rounded(&src, 3, Outside::Fill([255, 255, 255, 255])).unwrap();
        assert_eq!(f.get(0, 0), [255, 255, 255, 255]);
        assert_eq!(f.get(3, 3), [10, 20, 30, 255]);
        // radius 0 is an exact copy, and a radius over half the image is clamped.
        assert_eq!(rounded(&src, 0, Outside::Transparent).unwrap(), src);
        let big = rounded(&src, 99, Outside::Transparent).unwrap();
        assert_eq!(big.get(0, 0), [0, 0, 0, 0]);
        assert_eq!(big.get(5, 5), [10, 20, 30, 255]);
    }

    #[test]
    fn a_plate_adds_shadow_and_border_without_moving_the_picture() {
        let src = Frame::filled(20, 10, [0, 0, 0, 255]).unwrap();
        assert_eq!(plate(&src, &Plate::default()).unwrap(), src);

        let p = Plate {
            border_width: 1,
            border_color: [232, 17, 35, 255],
            shadow_margin: 4,
            shadow_radius: 4,
            shadow_color: [0, 0, 0, 160],
        };
        let out = plate(&src, &p).unwrap();
        assert_eq!((out.width, out.height), (28, 18));
        // The picture itself, shifted by the growth and ringed by the border.
        assert_eq!(out.get(5, 5), [0, 0, 0, 255]);
        assert_eq!(out.get(4, 4), [232, 17, 35, 255]);
        assert_eq!(out.get(23, 4), [232, 17, 35, 255]);
        assert_eq!(out.get(4, 13), [232, 17, 35, 255]);
        // The outermost ring is clear: a shadow must not fill its own margin.
        assert_eq!(out.get(0, 0), [0, 0, 0, 0]);
        assert_eq!(out.get(13, 0), [0, 0, 0, 0]);
        // Mid-margin is a partial alpha, and it grows towards the picture.
        let near = out.get(3, 8);
        let far = out.get(1, 8);
        assert!(near[3] > far[3], "{near:?} vs {far:?}");
        assert!(far[3] < 160 && near[3] < 160, "{far:?} {near:?}");
        assert_eq!(&near[..3], &[0, 0, 0]);
    }

    #[test]
    fn a_refresh_area_survives_a_smaller_desktop_only_if_it_still_shares_pixels() {
        let desktop = PhysRect::new(0, 0, 1000, 800);
        assert_eq!(
            still_valid(&PhysRect::new(10, 10, 100, 100), &desktop),
            Some(PhysRect::new(10, 10, 100, 100))
        );
        assert_eq!(
            still_valid(&PhysRect::new(950, 10, 100, 100), &desktop),
            Some(PhysRect::new(950, 10, 50, 100))
        );
        assert_eq!(
            still_valid(&PhysRect::new(2000, 10, 100, 100), &desktop),
            None
        );
    }

    #[test]
    fn a_feathered_circle_mask_ramps_its_edge() {
        let hard = shape_mask(11, 11, Shape::Circle, 0).unwrap();
        assert_eq!(hard.get(0, 0)[3], 0);
        assert_eq!(hard.get(5, 5)[3], 255);
        let soft = shape_mask(11, 11, Shape::Circle, 3).unwrap();
        assert_eq!(soft.get(0, 0)[3], 0);
        assert_eq!(soft.get(5, 5)[3], 255);
        // The ramp is monotone towards the centre, and it is exactly
        // `feather` pixels wide: 42 → 127 → 212 across the boundary, then
        // solid from the third pixel in.
        let (a, b, c) = (soft.get(0, 5)[3], soft.get(1, 5)[3], soft.get(2, 5)[3]);
        assert!(a > 0 && a < b && b < c && c < 255, "{a} {b} {c}");
        assert_eq!(soft.get(3, 5)[3], 255);
        // A rect is filled to its own edges: the brush shape is the frame.
        let rect = shape_mask(4, 4, Shape::Rect, 2).unwrap();
        assert_eq!(rect.get(0, 0)[3], 255);
    }

    #[test]
    fn an_outline_draws_inside_the_rect() {
        let mut f = Frame::new(5, 5).unwrap();
        outline(&mut f, &PhysRect::new(1, 1, 3, 3), [255, 0, 0, 255], 1);
        assert_eq!(f.get(1, 1), [255, 0, 0, 255]);
        assert_eq!(f.get(3, 3), [255, 0, 0, 255]);
        assert_eq!(f.get(2, 2), [0, 0, 0, 0]);
        assert_eq!(f.get(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn opaque_output_never_promises_transparency() {
        let mut f = Frame::new(2, 2).unwrap();
        f.set(0, 0, [1, 2, 3, 200]);
        assert!(warnings::first(&f, Format::Jpg).is_some());
        assert!(warnings::first(&f, Format::Png).is_none());
        let solid = Frame::filled(2, 2, [1, 2, 3, 255]).unwrap();
        for fmt in [Format::Jpg, Format::Bmp, Format::Tga, Format::Png] {
            assert!(warnings::first(&solid, fmt).is_none(), "{fmt:?}");
        }
    }

    mod warnings {
        use super::*;
        pub fn first(frame: &Frame, format: Format) -> Option<String> {
            warns_about_transparency(frame, format)
                .err()
                .map(|e| e.to_string())
        }
    }
}
