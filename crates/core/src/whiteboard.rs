//! Whiteboard surfaces: PRD §5.4.4 (a blank background to draw on) and §5.4.5
//! (the same canvas with no background at all).
//!
//! Nothing here opens a window. The two sections are four decisions, and each one
//! is a value question that a machine can be graded on without a hand on a mouse:
//! what the words on the config file mean (§5.4.4 step 2), how big the board is
//! (§5.4.4 step 3 "与屏幕或指定区域匹配"), what the pixels of the resulting canvas
//! actually are, and what happens when a transparent board is exported to a format
//! that cannot carry alpha (§5.4.5's two rules).
//!
//! The canvas it hands out is an ordinary [`Frame`], which is the whole point: the
//! mask and the annotation layer already read `snap.monitors[..].frame`, so a
//! whiteboard is a capture whose pixels were made instead of grabbed.

use crate::colors;
use crate::encode::Format;
use crate::frame::{Frame, MAX_DIM};
use crate::geometry::PhysRect;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WhiteboardError {
    #[error("a whiteboard background needs a colour like \"#FFFFFF\" or \"rgb(255, 255, 255)\", or the word \"transparent\" - got {0:?}")]
    BadColour(String),
    #[error("a whiteboard size is \"screen\" or \"WxH\" in pixels - got {0:?}")]
    BadSize(String),
    #[error(transparent)]
    Frame(#[from] crate::frame::FrameError),
}

/// §5.4.4 step 2 versus §5.4.5 step 1: the two are one enum, because the entry
/// point is one tray item and the difference the user asked for is only whether
/// the canvas carries a background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Opaque background. The alpha byte is always 255 here: a half-transparent
    /// "colour card" is a different feature (§5.8.6), and letting a 3-digit hex
    /// with an alpha quietly become §5.4.5 would make the two entries the same
    /// one. Spell [`Self::Transparent`] as the word `transparent` instead.
    Solid([u8; 3]),
    /// §5.4.5: a canvas of alpha-0 pixels.
    Transparent,
}

/// The word that selects §5.4.5 on the config file and on the CLI.
pub const TRANSPARENT: &str = "transparent";

/// The word that selects the whole virtual screen for [`Size`].
pub const SCREEN: &str = "screen";

pub const DEFAULT_COLOUR: &str = "#FFFFFF";
pub const DEFAULT_SIZE: &str = SCREEN;

impl Surface {
    /// §5.4.4 step 2 "选择白板背景颜色": the colour is a string because that is
    /// what a config file and a toolbar field carry. Parsing is delegated to
    /// [`colors::parse`] - the same reader §5.4.2's picked colours and §5.8.6's
    /// colour cards use, so `#abc`, `#AABBCC`, `rgb()` and `hsl()` mean one thing
    /// in this product rather than two.
    pub fn parse(raw: &str) -> Result<Self, WhiteboardError> {
        let text = raw.trim();
        if text.eq_ignore_ascii_case(TRANSPARENT) {
            return Ok(Surface::Transparent);
        }
        match colors::parse(text) {
            Some([r, g, b, _]) => Ok(Surface::Solid([r, g, b])),
            None => Err(WhiteboardError::BadColour(raw.to_string())),
        }
    }

    pub fn is_transparent(self) -> bool {
        matches!(self, Surface::Transparent)
    }

    /// What the page and the tray call it.
    pub fn label(self) -> &'static str {
        match self {
            Surface::Transparent => "透明白板",
            Surface::Solid(_) => "白板",
        }
    }
}

/// §5.4.4 step 3: "与屏幕或指定区域匹配". Either match a real display, or be a
/// board of the requested pixel size - and in the second case the caller still
/// has to say where it goes, which is why this returns a rect and not a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Screen,
    Canvas { width: u32, height: u32 },
}

impl Size {
    pub fn parse(raw: &str) -> Result<Self, WhiteboardError> {
        let text = raw.trim();
        if text.eq_ignore_ascii_case(SCREEN) {
            return Ok(Size::Screen);
        }
        // One separator, both spellings: `1280x720` is what a toolbar field writes
        // and `1280,720` is what every other pair in this config uses.
        let sep = if text.contains('x') || text.contains('X') {
            'x'
        } else {
            ','
        };
        let mut parts = text.split(sep).map(|p| p.trim());
        let (Some(w), Some(h), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(WhiteboardError::BadSize(raw.to_string()));
        };
        let read = |v: &str| v.parse::<u32>().ok().filter(|n| *n > 0 && *n <= MAX_DIM);
        match (read(w), read(h)) {
            (Some(width), Some(height)) => Ok(Size::Canvas { width, height }),
            _ => Err(WhiteboardError::BadSize(raw.to_string())),
        }
    }

    /// The board's own rect. `screen` is answered by the caller's snapshot: this
    /// layer does not know what monitors exist, and inventing a default resolution
    /// here is how a config value ends up disagreeing with the display it runs on.
    pub fn rect(self, screen: PhysRect) -> PhysRect {
        match self {
            Size::Screen => screen,
            Size::Canvas { width, height } => PhysRect::new(screen.x, screen.y, width, height),
        }
    }

    pub fn text(self) -> String {
        match self {
            Size::Screen => SCREEN.to_string(),
            Size::Canvas { width, height } => format!("{width}x{height}"),
        }
    }
}

/// The canvas. Both arms are a plain `Frame`, so the annotation layer cannot tell
/// a whiteboard from a capture - except that the transparent one reports
/// `is_transparent` and would go through [`plan_export`].
pub fn canvas(surface: Surface, width: u32, height: u32) -> Result<Frame, WhiteboardError> {
    match surface {
        Surface::Solid([r, g, b]) => Ok(Frame::filled(width, height, [r, g, b, 255])?),
        Surface::Transparent => Ok(Frame::new(width, height)?),
    }
}

/// §5.4.5's two rules as one decision: "透明输出应优先使用 PNG" and "不支持透明度
/// 的输出格式应提醒用户背景将被填充".
///
/// `want` is `None` when nobody named a format - the config's `[output] format`
/// and an explicit extension both arrive as `Some`. Nobody naming a format lands on
/// PNG, which is what rule 1 asks for and also this product's existing default, so
/// the preference is a fact about the caller rather than a second rule here. A
/// named format is never rewritten: the user who typed `--format jpg` asked for
/// jpg, and the honest answer is the warning, not a different file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Export {
    pub format: Format,
    /// Set only when alpha is about to be lost: the colour that fills it, which is
    /// [`crate::encode::FLATTEN_FILL`] by construction, not a copy of it.
    pub filled_with: Option<[u8; 4]>,
    /// True when §5.4.5's second rule fired and the user has to be told. A solid
    /// board never has this set: nothing is lost.
    pub warn: bool,
}

pub fn plan_export(surface: Surface, want: Option<Format>) -> Export {
    let format = want.unwrap_or_default();
    let lossy = surface.is_transparent() && !format.supports_alpha();
    Export {
        format,
        filled_with: lossy.then_some(crate::encode::FLATTEN_FILL),
        warn: lossy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::FLATTEN_FILL;

    #[test]
    fn a_colour_word_becomes_the_two_surfaces() {
        assert_eq!(Surface::parse("transparent"), Ok(Surface::Transparent));
        assert_eq!(
            Surface::parse("TRANSPARENT"),
            Ok(Surface::Transparent),
            "the word is not case-sensitive, like \"screen\""
        );
        assert_eq!(Surface::parse("#FFF"), Ok(Surface::Solid([255, 255, 255])));
        assert_eq!(
            Surface::parse("rgb(12, 34, 56)"),
            Ok(Surface::Solid([12, 34, 56]))
        );
        assert_eq!(
            Surface::parse("  #11223344  "),
            Ok(Surface::Solid([0x11, 0x22, 0x33])),
            "a hex with alpha is still a solid board: the alpha byte is §5.8.6's business"
        );
        for bad in ["", "white", "#12", "rgb(1,2)", "transparently"] {
            assert!(
                matches!(Surface::parse(bad), Err(WhiteboardError::BadColour(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_size_is_the_screen_or_a_pair_of_numbers() {
        assert_eq!(Size::parse("screen"), Ok(Size::Screen));
        assert_eq!(Size::parse(" SCREEN "), Ok(Size::Screen));
        assert_eq!(
            Size::parse("1280x720"),
            Ok(Size::Canvas {
                width: 1280,
                height: 720
            })
        );
        assert_eq!(
            Size::parse("800,600"),
            Ok(Size::Canvas {
                width: 800,
                height: 600
            }),
            "the config's other pairs are comma-separated, so both spellings land here"
        );
        for bad in ["", "0x100", "100", "100x200x300", "abc", "70000x10"] {
            assert!(
                matches!(Size::parse(bad), Err(WhiteboardError::BadSize(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn screen_matches_and_canvas_is_placed_at_the_origin() {
        let screen = PhysRect::new(0, 0, 2560, 1440);
        assert_eq!(Size::Screen.rect(screen), screen);
        assert_eq!(
            Size::Canvas {
                width: 320,
                height: 200
            }
            .rect(screen),
            PhysRect::new(0, 0, 320, 200)
        );
        assert_eq!(Size::parse("640x360").unwrap().text(), "640x360");
    }

    #[test]
    fn a_solid_board_is_opaque_and_a_transparent_one_is_all_alpha_zero() {
        let solid = canvas(Surface::Solid([10, 20, 30]), 4, 3).expect("a board");
        assert_eq!(solid.width, 4);
        assert_eq!(solid.get(0, 0), [10, 20, 30, 255]);
        assert!(!solid.has_transparency());

        let clear = canvas(Surface::Transparent, 4, 3).expect("a board");
        assert_eq!(clear.get(3, 2), [0, 0, 0, 0]);
        assert!(
            clear.has_transparency(),
            "otherwise §5.4.5's export rule would never be reachable"
        );
        assert_eq!(clear.is_uniform(), Some([0, 0, 0, 0]));
    }

    #[test]
    fn a_board_too_big_to_hold_is_refused_not_silently_clipped() {
        assert!(matches!(
            canvas(Surface::Transparent, MAX_DIM + 1, 2),
            Err(WhiteboardError::Frame(_))
        ));
    }

    #[test]
    fn transparent_output_prefers_png_and_a_named_lossy_format_warns() {
        // Rule 1: no format named -> PNG, nothing to warn about. The preference
        // *is* the product default, so naming PNG by hand must land on one row.
        let e = plan_export(Surface::Transparent, None);
        assert_eq!(e.format, Format::Png);
        assert_eq!(e, plan_export(Surface::Transparent, Some(Format::Png)));
        assert!(!e.warn && e.filled_with.is_none());

        // A named format that carries alpha is honoured silently.
        for f in [Format::Png, Format::Tiff, Format::WebP] {
            let e = plan_export(Surface::Transparent, Some(f));
            assert_eq!(e.format, f, "§5.4.5 asks for PNG *preferred*, not forced");
            assert!(!e.warn, "{f:?} keeps alpha");
        }

        // Rule 2: a format with no alpha fills the background, and says so.
        for f in [
            Format::Jpg,
            Format::Bmp,
            Format::Tga,
            Format::Gif,
            Format::Ico,
        ] {
            let e = plan_export(Surface::Transparent, Some(f));
            assert_eq!(e.format, f, "the named format is never rewritten");
            assert!(e.warn && e.filled_with == Some(FLATTEN_FILL), "{f:?}");
        }

        // A solid board loses nothing, whichever format it goes to.
        for f in [Format::Jpg, Format::Png, Format::Gif] {
            let e = plan_export(Surface::Solid([1, 2, 3]), Some(f));
            assert_eq!(e.format, f);
            assert!(!e.warn && e.filled_with.is_none(), "{f:?}");
        }
    }

    #[test]
    fn the_fill_the_export_promises_is_the_fill_the_encoder_uses() {
        // The warning says "背景将被填充为白色"; that sentence is only true if the
        // encoder's own flattening colour is the same one.
        let frame = canvas(Surface::Transparent, 2, 2).expect("a board");
        let e = plan_export(Surface::Transparent, Some(Format::Jpg));
        let fill = e.filled_with.expect("a transparent jpg fills");
        let out = crate::encode::flatten(&frame, fill).expect("flatten");
        assert_eq!(out.get(0, 0), [255, 255, 255, 255]);
        assert_eq!(fill, FLATTEN_FILL);
    }
}
