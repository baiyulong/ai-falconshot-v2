//! The font leg: [`Glyphs`] answered by DirectWrite.
//!
//! [`raster`](falcon_core::annotation::raster) owns everything about a line of
//! annotation text that is not the shape of a glyph — wrapping, alignment, line
//! advance, the background box, the stroke around the letters — and asks this leg
//! for one thing: the coverage of one line, one byte per pixel. Coverage rather
//! than colour, because §5.7.11 step 4 sets 字体, 字号, 颜色, 描边 and 背景
//! independently of each other, and a leg that returns coloured pixels would force
//! core to undo that choice.
//!
//! DirectWrite, not a crate: the family named in [`Style::font_family`] is a
//! *system* font, and R9's licence row and the MSIX package both say a font file
//! must not be redistributed inside this one. The leg also gets script fallback for
//! free, which is what lets 中文 in an annotation render at all, and it is the same
//! engine Qt's own text would use, so a difference between the overlay and the
//! export cannot be blamed on two font engines.
//!
//! The answer is *measured* by `Glyphs::ink`, never assumed: a machine that
//! cannot make a factory, or a line that cannot be laid out, returns `None` and core
//! draws the background box without letters. That is the same answer `NoGlyphs`
//! gives everywhere, so this leg failing is a missing feature rather than a wrong
//! pixel.
//!
//! There is no cache: one call is one layout, measured at 0.7 ms for a 900-pixel line
//! and 4.2 ms for a 24-word paragraph wrapped (`.qoder/scratch/p25_glyphcache1.log`),
//! which is affordable because `paint` asks only for the elements inside its dirty
//! rect.

use std::ffi::c_void;
use std::sync::OnceLock;

use falcon_core::annotation::model::Style;
use falcon_core::annotation::raster::{Glyphs, Ink};
use windows::core::{implement, IUnknown, Ref, Result, BOOL, PCWSTR};
use windows::Win32::Graphics::DirectWrite::*;

/// One glyph run's coverage, in the pixel grid of the line it belongs to.
///
/// `alpha` is a `DWRITE_TEXTURE_CLEARTYPE_3x1` texture: three bytes per pixel. That
/// is the only texture DirectWrite will hand back for a natural rendering mode -
/// measured on this machine, `DWRITE_TEXTURE_ALIASED_1x1` answers an empty rectangle
/// for every mode above `DWRITE_RENDERING_MODE_ALIASED`, and `ALIASED` itself is the
/// bi-level text a screenshot annotation must not be. The three values are the
/// coverage of three subpixel positions, and they are *not* equal even for the
/// symmetric mode [`DrawGlyphRun`] asks for: 905 of the 2 560 samples of
/// "Annotation" at 24px differ. [`blit`] therefore takes the highest, which is what
/// a coverage field means - the pixel is inked if any part of it is - and the choice
/// keeps a one-pixel stem solid instead of averaging it to a half-tone.
struct Piece {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    alpha: Vec<u8>,
}

/// The buffer a line is blitted into, and the pieces that fill it.
struct Layout {
    width: u32,
    height: u32,
    /// How far the buffer's origin sits left of and above the layout's own origin,
    /// which is the negative of wherever the ink started outside the line box.
    shift: (i32, i32),
    pieces: Vec<Piece>,
}

/// The state the renderer callback is handed, since the callback's `&self` is
/// DirectWrite's object and not somewhere a `Vec` can live.
struct Collector<'a> {
    factory: &'a IDWriteFactory,
    pieces: Vec<Piece>,
}

#[implement(IDWriteTextRenderer)]
struct Raster;

#[allow(non_snake_case)]
impl IDWriteTextRenderer_Impl for Raster_Impl {
    fn DrawGlyphRun(
        &self,
        clientdrawingcontext: *const c_void,
        baselineoriginx: f32,
        baselineoriginy: f32,
        measuringmode: DWRITE_MEASURING_MODE,
        glyphrun: *const DWRITE_GLYPH_RUN,
        _glyphrundescription: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _clientdrawingeffect: Ref<IUnknown>,
    ) -> Result<()> {
        // SAFETY: `Draw` passes back exactly the pointer `measure` put into the
        // call, which points at a live `Collector` for the duration of the draw,
        // and `glyphrun` is valid for the duration of this callback.
        unsafe {
            let ctx = &mut *(clientdrawingcontext as *mut Collector);
            let run = &*glyphrun;
            let analysis = ctx.factory.CreateGlyphRunAnalysis(
                run,
                1.0,
                None,
                DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
                measuringmode,
                baselineoriginx,
                baselineoriginy,
            )?;
            let bounds = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_CLEARTYPE_3x1)?;
            let width = (bounds.right - bounds.left).max(0) as usize;
            let height = (bounds.bottom - bounds.top).max(0) as usize;
            if width == 0 || height == 0 {
                return Ok(());
            }
            let mut alpha = vec![0u8; width * 3 * height];
            analysis.CreateAlphaTexture(DWRITE_TEXTURE_CLEARTYPE_3x1, &bounds, &mut alpha)?;
            // The rectangle is relative to the point the layout is drawn at, not to
            // this run's baseline: measured on "Annotation" at 24px, the baseline
            // came back at y = 24.375 while the ink band was y = 5..25, which is the
            // letters standing *above* that baseline inside the line box. Adding the
            // baseline to it lands every glyph below the descender line.
            ctx.pieces.push(Piece {
                left: bounds.left,
                top: bounds.top,
                width,
                height,
                alpha,
            });
            Ok(())
        }
    }

    fn DrawUnderline(
        &self,
        _clientdrawingcontext: *const c_void,
        _baselineoriginx: f32,
        _baselineoriginy: f32,
        _underline: *const DWRITE_UNDERLINE,
        _clientdrawingeffect: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }

    fn DrawStrikethrough(
        &self,
        _clientdrawingcontext: *const c_void,
        _baselineoriginx: f32,
        _baselineoriginy: f32,
        _strikethrough: *const DWRITE_STRIKETHROUGH,
        _clientdrawingeffect: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }

    fn DrawInlineObject(
        &self,
        _clientdrawingcontext: *const c_void,
        _originx: f32,
        _originy: f32,
        _inlineobject: Ref<IDWriteInlineObject>,
        _issideways: BOOL,
        _isrighttoleft: BOOL,
        _clientdrawingeffect: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }
}

#[allow(non_snake_case)]
impl IDWritePixelSnapping_Impl for Raster_Impl {
    /// The positions are taken as DirectWrite lays them out and rounded once, in
    /// [`blit`]. Snapping here would move a run between one repaint of a dirty
    /// rect and the whole-canvas flatten of the same document, and the two are
    /// compared byte for byte.
    fn IsPixelSnappingDisabled(&self, _clientdrawingcontext: *const c_void) -> Result<BOOL> {
        Ok(BOOL::from(true))
    }

    fn GetCurrentTransform(
        &self,
        _clientdrawingcontext: *const c_void,
        transform: *mut DWRITE_MATRIX,
    ) -> Result<()> {
        // SAFETY: DirectWrite passes the address of a live `DWRITE_MATRIX`.
        unsafe {
            *transform = DWRITE_MATRIX {
                m11: 1.0,
                m12: 0.0,
                m21: 0.0,
                m22: 1.0,
                dx: 0.0,
                dy: 0.0,
            };
        }
        Ok(())
    }

    /// `Style::font_size` is a size in *physical* pixels, and the mask's canvas is
    /// already at device resolution, so one unit of the font is one pixel here.
    fn GetPixelsPerDip(&self, _clientdrawingcontext: *const c_void) -> Result<f32> {
        Ok(1.0)
    }
}

fn factory() -> Option<&'static IDWriteFactory> {
    static FACTORY: OnceLock<Option<IDWriteFactory>> = OnceLock::new();
    FACTORY
        // SAFETY: `DWriteCreateFactory` has no preconditions beyond the DLL being
        // present, and the shared factory is documented as safe to keep for the
        // life of the process.
        .get_or_init(|| unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok() })
        .as_ref()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The locale an empty name means "the user's own", and DirectWrite takes the
/// parameter but not a null pointer: the factory answers `E_INVALIDARG` for one,
/// which is the shape of "no glyphs" this leg is supposed to reserve for a machine
/// with no font story.
fn default_locale() -> &'static [u16] {
    static LOCALE: [u16; 1] = [0];
    &LOCALE
}

/// Lay the line out once and collect every glyph run's coverage.
///
/// Split from [`Glyphs::ink`] because the interesting question about a buffer that
/// is *exactly* the size the layout reports is whether any ink falls outside it, and
/// that can only be answered while the pieces are still separate.
fn measure(line: &str, style: &Style) -> Option<Layout> {
    let factory = factory()?;
    let family = wide(&style.font_family);
    let text: Vec<u16> = line.encode_utf16().collect();
    unsafe {
        let format = factory
            .CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                style.font_size.max(1) as f32,
                PCWSTR(default_locale().as_ptr()),
            )
            .ok()?;
        // The box is core's business, not the font's: `wrap` measures a candidate
        // string with no width to break at and wraps it itself, so a layout that
        // wrapped or elided here would be measured against a different string than
        // the one it was asked about.
        let layout = factory
            .CreateTextLayout(&text, &format, f32::MAX, f32::MAX)
            .ok()?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut metrics).ok()?;

        let mut ctx = Collector {
            factory,
            pieces: Vec::new(),
        };
        let renderer: IDWriteTextRenderer = Raster.into();
        layout
            .Draw(
                Some(&mut ctx as *mut Collector as *const c_void),
                &renderer,
                0.0,
                0.0,
            )
            .ok()?;

        // The buffer is the line box the font reports, grown to whatever the runs
        // actually inked. `GetOverhangMetrics` is deliberately not consulted: on the
        // unbounded layout this leg asks for it answers
        // `right = bottom = -FLT_MAX` - that is "there was no box to overhang", not a
        // number that can be added to a size. The pieces DirectWrite just handed
        // back are the only trustworthy statement about the ink's extent, and taking
        // their union is what keeps an accent or a deep descender from being blitted
        // off the edge of a block that is one row too short.
        let advance = metrics
            .width
            .max(metrics.widthIncludingTrailingWhitespace)
            .ceil() as i32;
        let mut box_ = (0i32, 0i32, advance, metrics.height.ceil() as i32);
        for piece in &ctx.pieces {
            let (l, t) = (piece.left, piece.top);
            let (r, b) = (
                piece.left + piece.width as i32,
                piece.top + piece.height as i32,
            );
            box_ = (box_.0.min(l), box_.1.min(t), box_.2.max(r), box_.3.max(b));
        }
        Some(Layout {
            width: (box_.2 - box_.0).max(1) as u32,
            height: (box_.3 - box_.1).max(1) as u32,
            shift: (-box_.0, -box_.1),
            pieces: ctx.pieces,
        })
    }
}

/// Merge one run's coverage into the line's buffer, keeping the higher alpha.
fn blit(layout: &Layout) -> Vec<u8> {
    let mut coverage = vec![0u8; layout.width as usize * layout.height as usize];
    let (shift_x, shift_y) = layout.shift;
    for piece in &layout.pieces {
        for row in 0..piece.height {
            let y = piece.top + shift_y + row as i32;
            if y < 0 || y >= layout.height as i32 {
                continue;
            }
            for column in 0..piece.width {
                let x = piece.left + shift_x + column as i32;
                if x < 0 || x >= layout.width as i32 {
                    continue;
                }
                let at = y as usize * layout.width as usize + x as usize;
                let sub = row * piece.width * 3 + column * 3;
                let v = piece.alpha[sub]
                    .max(piece.alpha[sub + 1])
                    .max(piece.alpha[sub + 2]);
                if v > coverage[at] {
                    coverage[at] = v;
                }
            }
        }
    }
    coverage
}

/// The system's own font engine, answering for one line at a time.
#[derive(Debug, Default)]
pub struct DirectWrite;

impl Glyphs for DirectWrite {
    fn ink(&self, line: &str, style: &Style) -> Option<Ink> {
        let layout = measure(line, style)?;
        Some(Ink {
            width: layout.width,
            height: layout.height,
            coverage: blit(&layout),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(size: u32) -> Style {
        Style {
            font_size: size,
            ..Default::default()
        }
    }

    fn ink(line: &str, size: u32) -> Ink {
        DirectWrite
            .ink(line, &style(size))
            .unwrap_or_else(|| panic!("{line:?} at {size}px laid out no ink at all"))
    }

    /// The one thing a font leg can be wrong about and still return a plausible
    /// rectangle: it filled the box. Coverage that is a *shape* - some pixel fully
    /// inked, some column of the block empty - is what says glyphs were rasterised
    /// rather than a block laid down under them.
    #[test]
    fn a_line_comes_back_as_a_shape_not_a_filled_box() {
        let ink = ink("Annotation", 32);
        assert!(ink.width > 20 && ink.height > 20, "block: {ink:?}");
        let max = *ink.coverage.iter().max().unwrap();
        assert!(max >= 100, "no pixel is inked, max coverage {max}");
        let empty_column = (0..ink.width)
            .any(|x| (0..ink.height).all(|y| ink.at(x, y) == 0) && x > 0 && x + 1 < ink.width);
        assert!(empty_column, "every column carries ink: {ink:?}");
    }

    /// §5.7.11 step 4 sets 字号, and the only way core can honour it is if the block
    /// it is handed actually changes size.
    #[test]
    fn a_bigger_font_is_a_bigger_block() {
        let small = ink("Annotation", 16);
        let big = ink("Annotation", 48);
        assert!(
            big.width > small.width && big.height > small.height,
            "16px = {}x{}, 48px = {}x{}",
            small.width,
            small.height,
            big.width,
            big.height
        );
    }

    /// The family in `Style::font_family` is the one a Chinese annotation is written
    /// in. If script fallback did not happen, this is the leg that would silently
    /// draw nothing and blame the layout.
    #[test]
    fn chinese_lays_out() {
        let ink = ink("局部放大文本编号", 24);
        let max = *ink.coverage.iter().max().unwrap();
        assert!(max >= 100, "no stroke inked, max coverage {max}");
    }

    /// A machine without the configured family must still put letters on the screen:
    /// DirectWrite falls back, and the fallback is what the user sees rather than an
    /// empty box.
    #[test]
    fn an_unknown_family_still_lays_out() {
        let mut s = style(24);
        s.font_family = "No Such Family On Any Machine".into();
        let ink = DirectWrite
            .ink("Fallback", &s)
            .expect("an unknown family laid out nothing");
        let max = *ink.coverage.iter().max().unwrap();
        assert!(max >= 100, "max coverage {max}");
    }

    /// The first and last row of the block that carry ink, as a range.
    fn inked_rows(ink: &Ink) -> Option<(u32, u32)> {
        let rows: Vec<u32> = (0..ink.height)
            .filter(|y| (0..ink.width).any(|x| ink.at(x, *y) > 20))
            .collect();
        Some((*rows.first()?, *rows.last()?))
    }

    /// Where the ink sits is a separate fact from how big the block is, and it is the
    /// one a font leg can get wrong while still returning a plausible size: the
    /// alpha rectangle DirectWrite hands back is relative to the point the layout is
    /// drawn from, not to the run's baseline. Measured on "Annotation" at 24px, the
    /// baseline is at y = 24.375 inside a 31-row line box and the ink occupies rows
    /// 5..24; adding the baseline to the rectangle moves every letter below the
    /// descender line, into the last rows of the box.
    ///
    /// Capitals are the shape that cannot be mistaken for a descender: `MW` inks
    /// rows 6..24, so the box's bottom rows stay empty.
    #[test]
    fn capitals_sit_above_the_baseline_of_the_line_they_are_blitted_into() {
        let ink = ink("MW", 24);
        let (top, bottom) = inked_rows(&ink).expect("MW inked no row at all");
        assert!(
            top < ink.height / 3,
            "capitals start at row {top} of a {}-row block",
            ink.height
        );
        assert!(
            bottom + 5 < ink.height,
            "capitals reach row {bottom} of a {}-row block, which is where a descender belongs",
            ink.height
        );
    }

    /// The same placement claim read from the other end: a descender has to be in the
    /// bottom of the box. `ppp` at 24px inks down to row 30 of 31, which is also the
    /// row the font's own line box ends at - so this row is empty only if the
    /// descender was dropped on the way into the buffer.
    #[test]
    fn descenders_reach_the_bottom_of_the_line_box() {
        let ink = ink("ppp", 24);
        let (_, bottom) = inked_rows(&ink).expect("ppp inked no row at all");
        assert!(
            bottom + 1 >= ink.height,
            "the deepest ink is row {bottom} of {}",
            ink.height
        );
    }

    /// The line box as DirectWrite reports it, asked for here rather than read out of
    /// [`measure`]: the assertion below is that the block covers that box, and a
    /// number taken from the code under test would only be comparing it with itself.
    fn line_box(line: &str, size: u32) -> (u32, u32) {
        let factory = factory().expect("this machine has no DirectWrite");
        let family = wide(&style(size).font_family);
        let text: Vec<u16> = line.encode_utf16().collect();
        unsafe {
            let format = factory
                .CreateTextFormat(
                    PCWSTR(family.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size as f32,
                    PCWSTR(default_locale().as_ptr()),
                )
                .expect("the configured family laid out no format");
            let layout = factory
                .CreateTextLayout(&text, &format, f32::MAX, f32::MAX)
                .expect("the line laid out no layout");
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics).expect("no metrics");
            (
                metrics
                    .width
                    .max(metrics.widthIncludingTrailingWhitespace)
                    .ceil() as u32,
                metrics.height.ceil() as u32,
            )
        }
    }

    /// The buffer is the line box *or* whatever the ink needs, whichever is larger,
    /// and both halves are load-bearing:
    ///
    /// - never smaller, because `layout` takes the height as the line advance, so a
    ///   block trimmed to the letters of `i` would let the next line ride up into it;
    /// - never smaller than the pieces either, because a block a run sticks out of is
    ///   a block whose edge pixels `blit` throws away. `局部放大` at 24px inks 97
    ///   columns of a 96-column line box, so the overhang is real on this machine and
    ///   not a hypothetical.
    #[test]
    fn the_block_is_the_line_box_or_whatever_the_ink_needs() {
        for line in ["Annotation", "局部放大", "gjpqy", "MW({})`", "i", "ppp"] {
            let Some(laid) = measure(line, &style(24)) else {
                panic!("{line:?} laid out nothing");
            };
            let box_ = line_box(line, 24);
            assert!(
                laid.width >= box_.0 && laid.height >= box_.1,
                "{line:?}: block {}x{} is smaller than the line {box_:?}",
                laid.width,
                laid.height
            );
            for piece in &laid.pieces {
                let (x, y) = (piece.left + laid.shift.0, piece.top + laid.shift.1);
                assert!(
                    x >= 0
                        && y >= 0
                        && x + piece.width as i32 <= laid.width as i32
                        && y + piece.height as i32 <= laid.height as i32,
                    "{line:?}: a run at {x},{y} of {}x{} does not fit {}x{}",
                    piece.width,
                    piece.height,
                    laid.width,
                    laid.height
                );
            }
        }
    }

    /// A paragraph can hold an empty line, and core's `layout` asks for a block for
    /// it. Measured: the empty string lays out 1 wide and 31 tall with no coverage at
    /// all, so the blank line still advances by the font's own line height instead of
    /// collapsing the lines under it.
    #[test]
    fn an_empty_line_is_a_blank_line_the_same_height_as_the_font() {
        let ink = DirectWrite
            .ink("", &style(24))
            .expect("an empty line laid out nothing");
        assert_eq!(ink.width, 1);
        assert_eq!(*ink.coverage.iter().max().unwrap(), 0, "a blank line inked");
        assert_eq!(
            ink.height,
            line_box("", 24).1,
            "a blank line advanced {} rows at 24px",
            ink.height
        );
    }
}
