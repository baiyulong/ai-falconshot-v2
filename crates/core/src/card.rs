//! §5.8.5 文本卡片 — a pasted block of text laid out as a picture.
//!
//! PRD §5.8.2 lists the shapes the paste key accepts, and one renderer answers for three
//! of them: 纯文本 becomes a card, HTML 文本 becomes a card that keeps its line
//! structure, and a 合法颜色值 becomes a card whose background *is* the colour with its
//! own value written on it (§5.8.6 step 3). Those are one job, so they are one module —
//! and until this file existed the paste door in `qt_bridge/src/state.rs` had to answer
//! the first two with
//! 「贴图还未实现（Phase 2）」 while its own colour arm said the swatch was waiting for
//! the text card's renderer.
//!
//! Three rules run through it:
//!
//! * The column width is the user's setting, the break points are the font's answer.
//!   Wrapping goes through [`raster::wrap`] — the same breaker the capture mask lays its
//!   own text with — because a card that broke its lines somewhere else would make
//!   `card.width` mean two different things depending on which door the text came in
//!   through. [`raster::text_extent`] cannot answer here: it measures the widest
//!   paragraph *unwrapped*, which is by construction wider than any column the setting
//!   names.
//! * Nothing is estimated. With no font ([`Glyphs`] answering `None`) the ink cannot be
//!   measured, and this module then does what [`crate::annotation::raster`] does in the
//!   same situation: the background is real and the lines are simply not there. An
//!   invented line height would be a number the very next paste, with a font, contradicts.
//! * The ink colour is derived, not configured. §5.8.5 names four settings — 宽度、字号、
//!   字体、背景颜色 — and a fifth that the user must keep consistent with the fourth by
//!   hand is how a card ends up with invisible text, which §7.4 forbids.

use crate::annotation::model::{Align, Style};
use crate::annotation::raster::{self, Glyphs, Ink};
use crate::colors;
use crate::config::Card;
use crate::frame::{blend_rgba, Frame};
use thiserror::Error;

/// A card that cannot be made, with the number the user needs to fix it.
///
/// Worded by the door that shows it (§8.1, §8.2), the same way
/// [`crate::clip::ClipError`] and [`crate::encode::EncodeError`] are: core states the
/// fact, the paste path says it in the user's language.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum CardError {
    /// The text needed more pixels than [`crate::frame`] will allocate — §7.2's guard on
    /// 超大图片, reached here by a paste long enough to trip it.
    #[error("too large for one card: {0}x{1}")]
    TooLarge(u32, u32),
    /// `card.background` is not a colour. [`Card`] cannot hold that after a
    /// [`crate::config::Config::validate`] pass, but a card built by hand or read from a
    /// file that has not been validated can, and guessing a background is how a user who
    /// typed one and got white ends up not knowing which of the two happened.
    #[error("background is not a colour: {0}")]
    Background(String),
}

/// The gap between the ink and the coloured edge: half the font size, pinned at both
/// ends. Unpinned it goes wrong in each direction — a card set to 96 px would spend
/// most of its configured width on air, and one set to 10 px would have no edge at all.
pub fn pad(card: &Card) -> u32 {
    (card.font_size / 2).clamp(4, 32)
}

/// The width the font is allowed to break at, after the padding has taken its share.
///
/// At least 1: a `card.width` of 160 with `pad` at 32 leaves 96, and a smaller card
/// could leave zero. Zero would ask [`raster::wrap`] to break every single word.
pub fn column(card: &Card) -> u32 {
    card.width.saturating_sub(pad(card) * 2).max(1)
}

/// The style this card lays its lines with. Left-aligned: §5.7.11 makes 对齐 a choice
/// about one annotation, and §5.8.5 names four settings about a card, of which alignment
/// is not one.
pub fn style(card: &Card, ink_colour: [u8; 4]) -> Style {
    Style {
        color: ink_colour,
        font_family: card.font_family.clone(),
        font_size: card.font_size,
        align: Align::Left,
        ..Default::default()
    }
}

/// The ink colour for a background, so §7.4's 「深色与浅色主题均应保证文本和控件对比度」
/// is a rule the card enforces rather than a second setting to keep consistent by hand.
///
/// The sRGB weights over the channels as they are: the question is which side of middle
/// grey a swatch falls on, not a colourimetric answer.
pub fn foreground(bg: [u8; 4]) -> [u8; 4] {
    let lum = 0.2126 * f64::from(bg[0]) + 0.7152 * f64::from(bg[1]) + 0.0722 * f64::from(bg[2]);
    if lum > 127.5 {
        [0, 0, 0, 255]
    } else {
        [0xff, 0xff, 0xff, 255]
    }
}

/// The card's own background, or the refusal that says which string was not a colour.
pub fn background(card: &Card) -> Result<[u8; 4], CardError> {
    colors::parse(&card.background).ok_or_else(|| CardError::Background(card.background.clone()))
}

/// §5.8.6 步骤 3 与步骤 5 的那一行**颜色值**：不透明写六位、带透明写八位。
///
/// Alpha is written rather than applied — a swatch painted with `#00000000` is a picture
/// of nothing at all, and the value line is what the user reads the colour off.
///
/// 这一手自 §5.8.6 步骤 4「用户可进入编辑模式修改颜色值」起有了第二只需要它的手（一格栅窗
/// 改完颜色要重写这一行），所以它住在卡片这一侧而不是贴图那一侧：两处各写一遍「alpha 是 255
/// 就省掉」就是两张卡片会对同一个颜色写下不同的字（#281）。
pub fn colour_text(rgba: [u8; 4]) -> String {
    if rgba[3] == 0xff {
        colors::format(rgba, colors::ColorFormat::HexUpper)
    } else {
        colors::format(rgba, colors::ColorFormat::HexRgba)
    }
}

/// §5.8.6: the `Card` a colour is drawn on — 背景**就是**那枚颜色，宽度、字号、字体仍来自
/// 用户那一刻的 `[card]`。
///
/// 交回的是整张卡而不是只换背景那一格，因为「颜色卡片长什么样」是一件事：贴图那一臂与
/// [`crate::pin::CardPin::card_of`] 都要它，写两遍就是两处可能走散的读法。
pub fn colour_card(base: &Card, rgba: [u8; 4]) -> Card {
    let mut card = base.clone();
    // 色块是**不透明**的那一枚：`#RRGGBB` 是背景那一格唯一的写法，而带 alpha 的那一行写在
    // [`colour_text`] 上 —— 一次编辑换掉的两半共用这一枚函数，才不会出现块与字各走一头。
    card.background = colors::format(rgba, colors::ColorFormat::HexUpper);
    card
}

/// 档的闸对**这一张候选卡片**的回答：夹过的那一份，与闸自己那句「这一格越界了、夹成这一枚数」。
///
/// §5.8.5 步骤 4 之后四枚旋钮可以只落在一张卡片身上，可那两枚数的界只有一名作者
/// （[`crate::config::Config::validate`] 里 `card.width` / `card.font_size` 那两行）。这一手
/// 把候选塞进一份出厂的档、让那一行自己夹、再把夹回来的那一份连它的抱怨一起交回 —— 界在桥或
/// 页上再抄一遍就是第二个作者，而第二位与第一位在下一轮改界那天一定会走散（#281）。
///
/// 出厂那份档的每一枚键本来就在界内（`config.rs` 的
/// `card_keys_are_clamped_and_its_background_repaired` 钉着「the shipped card needs no
/// repair」），所以交回的抱怨只可能来自传进来的这一张卡片。
pub fn gate(card: &Card) -> (Card, Vec<String>) {
    let mut probe = crate::config::Config::default();
    probe.card = card.clone();
    let said = probe.validate();
    (probe.card, said)
}

/// The line boxes of `text`, top-down, each with the y it sits at inside the card.
///
/// Private because it is the middle of two answers that must not disagree: [`size`] and
/// [`render`] both go through it, so a card cannot measure one height and paint another.
///
/// A background nobody can parse yields no boxes rather than a guess at one; [`render`]
/// checks it first and so reports the refusal instead of drawing an empty card.
fn boxes(text: &str, card: &Card, glyphs: &dyn Glyphs) -> Vec<(Ink, u32)> {
    let bg = match background(card) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let style = style(card, foreground(bg));
    let col = column(card);
    let mut out = Vec::new();
    let mut y = pad(card);
    for para in text.split('\n') {
        for seg in raster::wrap(para, col, &style, glyphs) {
            let Some(ink) = glyphs.ink(&seg, &style) else {
                continue;
            };
            let h = ink.height.max(1);
            out.push((ink, y));
            y += h;
        }
    }
    out
}

/// How many lines the card laid out — the number a door prints instead of the pixels.
pub fn line_count(text: &str, card: &Card, glyphs: &dyn Glyphs) -> usize {
    boxes(text, card, glyphs).len()
}

/// The box a card of `text` will occupy: the configured width, and as tall as its lines
/// plus the padding. A caller that has to clamp a window asks this before pixels exist.
///
/// Height is never less than `pad * 2`, which is the card with no font at all: still a
/// real background, just one with nothing measured to sit in it.
pub fn size(text: &str, card: &Card, glyphs: &dyn Glyphs) -> (u32, u32) {
    let lines: u32 = boxes(text, card, glyphs)
        .iter()
        .map(|(ink, _)| ink.height.max(1))
        .sum();
    (card.width, pad(card) * 2 + lines)
}

/// Lay `text` out on a card and return the picture.
pub fn render(text: &str, card: &Card, glyphs: &dyn Glyphs) -> Result<Frame, CardError> {
    let bg = background(card)?;
    let (w, h) = size(text, card, glyphs);
    let mut frame = Frame::filled(w, h, bg).map_err(|_| CardError::TooLarge(w, h))?;
    let ink_colour = foreground(bg);
    let edge = pad(card);
    for (ink, top) in boxes(text, card, glyphs) {
        for row in 0..ink.height {
            let y = top + row;
            if y >= h {
                break;
            }
            for col in 0..ink.width {
                let v = ink.at(col, row);
                if v == 0 {
                    continue;
                }
                let x = edge + col;
                if x >= w {
                    continue;
                }
                let dst = frame.get(x, y);
                frame.set(x, y, blend_rgba(dst, ink_colour, v));
            }
        }
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::raster::NoGlyphs;

    fn card(width: u32) -> Card {
        Card {
            width,
            ..Default::default()
        }
    }

    /// A font that answers with a solid block whose width is one unit per byte and whose
    /// height is the font size: enough to make a break point observable without one.
    struct Blocks;

    impl Glyphs for Blocks {
        fn ink(&self, line: &str, style: &Style) -> Option<Ink> {
            let w = line.len() as u32 * 10;
            let h = style.font_size;
            Some(Ink::solid(w, h))
        }
    }

    #[test]
    fn no_font_still_paints_the_background() {
        let c = card(200);
        let f = render("hello", &c, &NoGlyphs).expect("card");
        assert_eq!((f.width, f.height), (200, pad(&c) * 2));
        assert_eq!(f.get(0, 0), [0xff, 0xff, 0xff, 0xff]);
        assert_eq!(line_count("hello", &c, &NoGlyphs), 0);
    }

    #[test]
    fn padding_is_half_the_font_within_two_pins() {
        assert_eq!(pad(&card(200)), 9); // 18 / 2
        let small = Card {
            font_size: crate::annotation::model::FONT_MIN,
            ..Default::default()
        };
        assert_eq!(pad(&small), 5); // 10 / 2, above the floor
        let big = Card {
            font_size: 96,
            ..Default::default()
        };
        assert_eq!(pad(&big), 32); // pinned
    }

    #[test]
    fn column_never_collapses_to_zero() {
        let narrow = card(160);
        assert_eq!(column(&narrow), 160 - pad(&narrow) * 2);
        assert!(column(&card(1)) >= 1);
    }

    #[test]
    fn a_long_paragraph_wraps_inside_the_column() {
        let c = card(200);
        let col = column(&c);
        let text = "word ".repeat(40);
        let n = line_count(&text, &c, &Blocks);
        assert!(n > 1, "one line for a 40-word paste: {n}");
        // Each block is 10 px per byte, so no line may carry more bytes than the column.
        for (ink, _) in boxes(&text, &c, &Blocks) {
            assert!(ink.width <= col + 10, "{} over {col}", ink.width);
        }
        assert_eq!(size(&text, &c, &Blocks).1, pad(&c) * 2 + n as u32 * 18);
    }

    #[test]
    fn a_hard_break_starts_a_new_line_without_wrapping() {
        let c = card(2000);
        assert_eq!(line_count("a\nb\nc", &c, &Blocks), 3);
        assert_eq!(line_count("", &c, &Blocks), 1);
    }

    #[test]
    fn ink_is_chosen_by_the_background_that_sits_under_it() {
        assert_eq!(foreground([0xff, 0xff, 0xff, 255]), [0, 0, 0, 255]);
        assert_eq!(foreground([0x10, 0x10, 0x10, 255]), [0xff, 0xff, 0xff, 255]);
        // Mid grey on the warm side of the weights, and its opposite.
        assert_eq!(foreground([0x00, 0xff, 0x00, 255]), [0, 0, 0, 255]);
        assert_eq!(foreground([0x80, 0x00, 0x00, 255]), [0xff, 0xff, 0xff, 255]);
    }

    #[test]
    fn the_ink_lands_on_the_card_rather_than_the_air() {
        let c = card(200);
        let f = render("i", &c, &Blocks).expect("card");
        // One 10x18 block at (pad, pad): its corner is ink, one pixel outside is not.
        let edge = pad(&c) as usize;
        let at = |x: usize, y: usize| f.get(x as u32, y as u32);
        assert_eq!(at(edge, edge), [0, 0, 0, 255]);
        assert_eq!(at(edge - 1, edge), [0xff, 0xff, 0xff, 255]);
        assert_eq!(at(edge + 9, edge + 17), [0, 0, 0, 255]);
    }

    #[test]
    fn an_unreadable_background_is_refused_not_guessed() {
        let c = Card {
            background: "not-a-colour".into(),
            ..Default::default()
        };
        assert_eq!(
            render("x", &c, &Blocks),
            Err(CardError::Background("not-a-colour".into()))
        );
        assert_eq!(size("x", &c, &Blocks).1, pad(&c) * 2);
    }

    #[test]
    fn a_paste_too_tall_for_one_frame_is_refused_with_the_size() {
        let c = Card {
            width: 16_384,
            ..Default::default()
        };
        // 16 384 px wide × past the 16 384 px side guard: 2 001 lines of 18 px each.
        let text = "aaaaaaaaaa\n".repeat(2_000);
        match render(&text, &c, &Blocks) {
            Err(CardError::TooLarge(w, h)) => assert!(w > 0 && h > 0, "{w}x{h}"),
            other => panic!("expected a size refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_default_card_is_a_light_background_with_dark_ink() {
        let c = Card::default();
        let bg = background(&c).expect("default colour parses");
        assert_eq!(bg, [0xff, 0xff, 0xff, 0xff]);
        assert_eq!(foreground(bg), [0, 0, 0, 255]);
    }

    /// §5.8.5 步骤 4 落在**一枚卡片**上之后仍要过同一道闸，而这一格判的是「过的是同一道闸」
    /// 而不是「界是这两枚数」：读回的是 [`crate::config::Config::validate`] 自己的夹法与它自己
    /// 那句抱怨，界若被谁在别处抄了第二份，这一格读回的夹值就会与档那一头不一致。
    #[test]
    fn the_gate_clamps_a_card_with_the_settings_own_numbers() {
        let shipped = Card::default();
        let (kept, said) = gate(&shipped);
        assert_eq!(kept, shipped, "出厂那张卡不该被改一个字：{said:?}");
        assert!(said.is_empty(), "出厂那张卡被抱怨了：{said:?}");

        let wild = Card {
            width: 1_000_000,
            font_size: 500,
            ..Default::default()
        };
        let (clamped, said) = gate(&wild);
        assert_eq!(
            (clamped.width, clamped.font_size),
            (4096, crate::annotation::model::FONT_MAX),
            "越界的两格没按档的那两枚界夹回来：{clamped:?}（抱怨：{said:?}）"
        );
        for needle in ["card.width", "card.font_size"] {
            assert!(
                said.iter().any(|w| w.contains(needle)),
                "闸没有点名 {needle}，状态栏就无从可说：{said:?}"
            );
        }

        // 下面那枚界也是同一道闸的读数：一张 1 px 宽的卡排不出任何一行字。
        let (thin, said) = gate(&Card {
            width: 1,
            font_size: 0,
            ..Default::default()
        });
        assert_eq!(
            (thin.width, thin.font_size),
            (160, crate::annotation::model::FONT_MIN),
            "贴着地板的两格没被夹到地板上：{thin:?}（抱怨：{said:?}）"
        );
    }

    /// 步骤 5「卡片颜色和显示文本同步更新」的前提是这两半各只有一个写法：色块那格是不透明的
    /// `#RRGGBB`，那一行在不透明时与它同字、带 alpha 时多出 alpha 那两位。
    #[test]
    fn a_colour_writes_its_swatch_and_its_value_line_from_one_reading() {
        let red = [0xe8, 0x11, 0x23, 0xff];
        assert_eq!(colour_text(red), "#E81123");
        assert_eq!(colour_text([0x12, 0x34, 0x56, 0x80]), "#12345680");

        let base = Card {
            width: 640,
            ..Default::default()
        };
        let card = colour_card(&base, red);
        assert_eq!(card.background, "#E81123", "色块没换成那枚颜色：{card:?}");
        assert_eq!(card.width, base.width, "宽度该仍来自 [card]：{card:?}");
        assert_eq!(
            (card.font_size, &card.font_family),
            (base.font_size, &base.font_family),
            "字号与字体该仍来自 [card]：{card:?}"
        );
        assert_eq!(
            card.background,
            colour_text(red),
            "不透明时色块与那一行写的是同一串字，否则同步这一步要用户自己对两次账"
        );
    }
}
