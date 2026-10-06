//! Colour values: the formats PRD §5.4.2 promises to copy, and the parse side
//! that turns a clipboard string into a colour card (§5.8.6).
//!
//! Alpha is carried as `[u8; 4]` everywhere, matching `Frame`.

use serde::{Deserialize, Serialize};

/// How a picked colour is written out. The default is configurable (§5.4.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorFormat {
    #[default]
    HexUpper,
    HexLower,
    HexRgba,
    Rgb,
    Rgba,
    Hsl,
    Hsla,
}

impl ColorFormat {
    pub fn label(self) -> &'static str {
        match self {
            ColorFormat::HexUpper => "HEX",
            ColorFormat::HexLower => "hex",
            ColorFormat::HexRgba => "HEX+Alpha",
            ColorFormat::Rgb => "RGB",
            ColorFormat::Rgba => "RGBA",
            ColorFormat::Hsl => "HSL",
            ColorFormat::Hsla => "HSLA",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        // Config spells these snake_case (`hex_upper`); the UI spells them
        // `HEX+Alpha`, so both separators are folded away before matching.
        let key = name
            .trim()
            .to_ascii_lowercase()
            .replace(['_', '-', '+'], "");
        Some(match key.as_str() {
            "hex" | "hexupper" | "#rrggbb" => ColorFormat::HexUpper,
            "hexlower" => ColorFormat::HexLower,
            "hexa" | "hexrgba" | "#rrggbbaa" => ColorFormat::HexRgba,
            "rgb" => ColorFormat::Rgb,
            "rgba" => ColorFormat::Rgba,
            "hsl" => ColorFormat::Hsl,
            "hsla" => ColorFormat::Hsla,
            _ => return None,
        })
    }
}

pub fn format(rgba: [u8; 4], f: ColorFormat) -> String {
    let [r, g, b, a] = rgba;
    let alpha01 = a as f64 / 255.0;
    let (h, s, l) = rgb_to_hsl(r, g, b);
    match f {
        ColorFormat::HexUpper => format!("#{r:02X}{g:02X}{b:02X}"),
        ColorFormat::HexLower => format!("#{r:02x}{g:02x}{b:02x}"),
        ColorFormat::HexRgba => format!("#{r:02X}{g:02X}{b:02X}{a:02X}"),
        ColorFormat::Rgb => format!("rgb({r}, {g}, {b})"),
        ColorFormat::Rgba => format!("rgba({r}, {g}, {b}, {tri})", tri = trim2(alpha01)),
        // Hue in degrees, saturation/lightness as CSS percentages.
        ColorFormat::Hsl => format!("hsl({}, {}%, {}%)", h.round(), s.round(), l.round()),
        ColorFormat::Hsla => format!(
            "hsla({}, {}%, {}%, {})",
            h.round(),
            s.round(),
            l.round(),
            trim2(alpha01)
        ),
    }
}

/// `1` instead of `1.0`, `0.5` instead of `0.50` — what CSS users expect.
fn trim2(v: f64) -> String {
    format!("{}", (v * 100.0).round() / 100.0)
}

/// Parse a colour the way §5.8.6 defines "合法颜色值": `#RGB`, `#RGBA`,
/// `#RRGGBB`, `#RRGGBBAA`, `rgb()/rgba()` (integers or percentages) and
/// `hsl()/hsla()`. Bare numbers and colour names are deliberately not
/// accepted — pasting `255` must stay a text card.
pub fn parse(text: &str) -> Option<[u8; 4]> {
    let s = text.trim();
    if s.is_empty() || s.len() > 64 {
        return None;
    }
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    let (name, rest) = s.split_once('(')?;
    let inner = rest.strip_suffix(')')?;
    let args: Vec<&str> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|a| !a.trim().is_empty())
        .collect();
    match name.trim().to_ascii_lowercase().as_str() {
        "rgb" | "rgba" => parse_rgb(&args),
        "hsl" | "hsla" => parse_hsl(&args),
        _ => None,
    }
}

fn parse_hex(hex: &str) -> Option<[u8; 4]> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize, j: usize| u8::from_str_radix(&hex[i..j], 16).ok();
    let a = |v: &str| u8::from_str_radix(v, 16).ok();
    let expanded = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();
    match hex.len() {
        3 => Some([
            expanded(hex.chars().next()?)?,
            expanded(hex.chars().nth(1)?)?,
            expanded(hex.chars().nth(2)?)?,
            255,
        ]),
        4 => Some([
            expanded(hex.chars().next()?)?,
            expanded(hex.chars().nth(1)?)?,
            expanded(hex.chars().nth(2)?)?,
            expanded(hex.chars().nth(3)?)?,
        ]),
        6 => Some([byte(0, 2)?, byte(2, 4)?, byte(4, 6)?, 255]),
        8 => Some([byte(0, 2)?, byte(2, 4)?, byte(4, 6)?, a(&hex[6..8])?]),
        _ => None,
    }
}

/// `255`, `255.0` or `100%` per CSS Color 4.
fn channel(v: &str, max: f64) -> Option<f64> {
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%') {
        let p: f64 = p.trim().parse().ok()?;
        return Some((p.clamp(0.0, 100.0) / 100.0) * max);
    }
    let n: f64 = v.parse().ok()?;
    Some(n.clamp(0.0, max))
}

fn alpha(v: &str) -> Option<u8> {
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%') {
        let p: f64 = p.trim().parse().ok()?;
        return Some((p.clamp(0.0, 100.0) * 2.55).round() as u8);
    }
    if v.contains('/') {
        // `50% / 0.4` form is not worth the parser here.
        return None;
    }
    let n: f64 = v.parse().ok()?;
    Some((n.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn parse_rgb(args: &[&str]) -> Option<[u8; 4]> {
    if args.len() != 3 && args.len() != 4 {
        return None;
    }
    let r = channel(args[0], 255.0)?.round() as u8;
    let g = channel(args[1], 255.0)?.round() as u8;
    let b = channel(args[2], 255.0)?.round() as u8;
    let a = match args.len() {
        4 => alpha(args[3])?,
        _ => 255,
    };
    Some([r, g, b, a])
}

fn parse_hsl(args: &[&str]) -> Option<[u8; 4]> {
    if args.len() != 3 && args.len() != 4 {
        return None;
    }
    let h = {
        let v = args[0].trim().trim_end_matches("deg");
        let n: f64 = v.parse().ok()?;
        n.rem_euclid(360.0)
    };
    let s = channel(args[1], 100.0)?;
    let l = channel(args[2], 100.0)?;
    let a = match args.len() {
        4 => alpha(args[3])?,
        _ => 255,
    };
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Some([r, g, b, a])
}

/// `h` in degrees, `s`/`l` in percent. Returns 0..=255 channels.
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let sf = s / 100.0;
    let lf = l / 100.0;
    let c = (1.0 - (2.0 * lf - 1.0).abs()) * sf;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp {
        v if v < 1.0 => (c, x, 0.0),
        v if v < 2.0 => (x, c, 0.0),
        v if v < 3.0 => (0.0, c, x),
        v if v < 4.0 => (0.0, x, c),
        v if v < 5.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = lf - c / 2.0;
    let q = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (q(r1), q(g1), q(b1))
}

/// Returns `(hue degrees, saturation percent, lightness percent)`.
pub fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (rf, gf, bf) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let l = (max + min) / 2.0;
    if delta <= f64::EPSILON {
        return (0.0, 0.0, l * 100.0);
    }
    let s = delta / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == rf {
        60.0 * (((gf - bf) / delta).rem_euclid(6.0))
    } else if max == gf {
        60.0 * ((bf - rf) / delta + 2.0)
    } else {
        60.0 * ((rf - gf) / delta + 4.0)
    };
    (
        h.rem_euclid(360.0),
        (s * 100.0).clamp(0.0, 100.0),
        l * 100.0,
    )
}

/// Per-tool colour memory plus the reorderable swatch strip of §5.7.21/§5.7.22.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub colors: Vec<[u8; 4]>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            colors: [
                (0xE8, 0x11, 0x23),
                (0xFD, 0x7E, 0x14),
                (0xFF, 0xD7, 0x00),
                (0x2E, 0xCC, 0x71),
                (0x00, 0xAB, 0xD4),
                (0x1E, 0x90, 0xFF),
                (0x9B, 0x59, 0xB6),
                (0x2C, 0x3E, 0x50),
                (0xFF, 0xFF, 0xFF),
                (0x00, 0x00, 0x00),
            ]
            .iter()
            .map(|(r, g, b)| [*r, *g, *b, 255])
            .collect(),
        }
    }
}

impl Palette {
    pub fn contains(&self, c: &[u8; 4]) -> bool {
        self.colors.contains(c)
    }

    /// Add at the end, unless it is already there — a palette that grows on
    /// every click is unusable (§5.7.22).
    pub fn add(&mut self, c: [u8; 4]) -> bool {
        if self.contains(&c) {
            return false;
        }
        self.colors.push(c);
        true
    }

    pub fn remove(&mut self, index: usize) -> Option<[u8; 4]> {
        if index < self.colors.len() {
            Some(self.colors.remove(index))
        } else {
            None
        }
    }

    /// Drag-to-reorder (§5.7.22). Out-of-range targets clamp, so a stale index
    /// from a concurrent edit cannot panic the UI.
    pub fn move_to(&mut self, from: usize, to: usize) -> bool {
        if from >= self.colors.len() || self.colors.len() < 2 {
            return false;
        }
        let item = self.colors.remove(from);
        let to = to.min(self.colors.len());
        self.colors.insert(to, item);
        true
    }

    /// Import/export as a plain text list of colour values, one per line, so a
    /// shared palette file stays hand-editable (§5.7.22).
    pub fn to_text(&self) -> String {
        self.colors
            .iter()
            .map(|c| format(*c, ColorFormat::HexUpper))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn from_text(text: &str) -> Self {
        let mut p = Self { colors: Vec::new() };
        for line in text.lines() {
            if let Some(c) = parse(line) {
                p.add(c);
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_round_trip() {
        let c = [232, 17, 35, 255];
        assert_eq!(format(c, ColorFormat::HexUpper), "#E81123");
        assert_eq!(format(c, ColorFormat::HexLower), "#e81123");
        assert_eq!(format(c, ColorFormat::Rgb), "rgb(232, 17, 35)");
        for f in [
            ColorFormat::HexUpper,
            ColorFormat::HexRgba,
            ColorFormat::Rgb,
            ColorFormat::Rgba,
        ] {
            let text = format(c, f);
            let back = parse(&text).unwrap_or_else(|| panic!("{f:?} -> {text}"));
            assert_eq!(back, c, "{f:?}");
        }
        // HSL is written at integer percent/hue, so it only comes back close.
        let hsl = format(c, ColorFormat::Hsl);
        assert!(hsl.starts_with("hsl(") && hsl.ends_with("%)"), "{hsl}");
        let back = parse(&hsl).unwrap();
        for (a, b) in c.iter().zip(back.iter()) {
            assert!(a.abs_diff(*b) <= 8, "{hsl} -> {back:?} vs {c:?}");
        }
        let hsla = format([10, 20, 30, 128], ColorFormat::Hsla);
        assert_eq!(parse(&hsla).unwrap()[3], 128, "{hsla}");
    }

    #[test]
    fn alpha_appears_only_in_alpha_formats() {
        let c = [10, 20, 30, 128];
        assert_eq!(format(c, ColorFormat::HexUpper), "#0A141E");
        assert_eq!(format(c, ColorFormat::HexRgba), "#0A141E80");
        assert_eq!(format(c, ColorFormat::Rgba), "rgba(10, 20, 30, 0.5)");
        assert!(format(c, ColorFormat::Rgb).starts_with("rgb("));
    }

    #[test]
    fn parse_accepts_the_css_shapes() {
        assert_eq!(parse("#f00"), Some([255, 0, 0, 255]));
        assert_eq!(parse("#ff0000"), Some([255, 0, 0, 255]));
        assert_eq!(parse("  #FF000080 "), Some([255, 0, 0, 128]));
        assert_eq!(parse("rgb(255, 0, 0)"), Some([255, 0, 0, 255]));
        assert_eq!(parse("rgb(100%, 0%, 0%)"), Some([255, 0, 0, 255]));
        assert_eq!(parse("rgba(0, 255, 0, 1)"), Some([0, 255, 0, 255]));
        assert_eq!(parse("hsl(0, 100%, 50%)"), Some([255, 0, 0, 255]));
        assert_eq!(parse("hsl(120deg, 100%, 50%)"), Some([0, 255, 0, 255]));
        assert_eq!(parse("hsl(240 100% 50%)"), Some([0, 0, 255, 255]));
    }

    #[test]
    fn parse_rejects_what_is_not_a_colour() {
        for s in [
            "",
            " ",
            "255",
            "red",
            "#12345",
            "#12g456",
            "rgb(300, 0, 0",
            "hsl(0, 0%)",
            "rgba(0,0,0,0.5/1)",
            "#0A141E #0A141E",
            "192.168.0.1",
            "rgb(1,2,3,4,5)",
        ] {
            assert_eq!(parse(s), None, "{s:?} should not parse");
        }
    }

    #[test]
    fn hsl_conversion_is_stable_at_the_greys() {
        for v in [0u8, 64, 128, 192, 255] {
            let (h, s, l) = rgb_to_hsl(v, v, v);
            assert_eq!((h, s), (0.0, 0.0));
            assert!((l - v as f64 * 100.0 / 255.0).abs() < 0.6);
        }
        // Round trip across the wheel.
        for h in (0..360).step_by(15) {
            let (r, g, b) = hsl_to_rgb(h as f64, 80.0, 45.0);
            let (h2, s2, l2) = rgb_to_hsl(r, g, b);
            assert!((h2 - h as f64).abs() < 1.0, "{h} -> {h2}");
            assert!((s2 - 80.0).abs() < 1.5);
            assert!((l2 - 45.0).abs() < 1.5);
        }
    }

    #[test]
    fn palette_editing_rules() {
        let mut p = Palette::default();
        let n = p.colors.len();
        assert!(!p.add(p.colors[0]), "duplicate add must be refused");
        assert_eq!(p.colors.len(), n);
        assert!(p.add([1, 2, 3, 4]));
        assert_eq!(p.colors.len(), n + 1);
        assert!(p.move_to(n, 0));
        assert_eq!(p.colors[0], [1, 2, 3, 4]);
        assert!(p.move_to(0, 9_999), "out-of-range target clamps");
        assert_eq!(p.colors[p.colors.len() - 1], [1, 2, 3, 4]);
        assert_eq!(p.remove(9_999), None);
        assert_eq!(p.remove(n), Some([1, 2, 3, 4]));
        assert_eq!(p.colors.len(), n);
        assert_eq!(Palette::from_text(&p.to_text()).colors, p.colors);
        assert_eq!(
            Palette::from_text("nonsense\n#f00\n").colors,
            vec![[255, 0, 0, 255]]
        );
    }

    #[test]
    fn format_names_survive_config_round_trip() {
        for f in [
            ColorFormat::HexUpper,
            ColorFormat::HexRgba,
            ColorFormat::Rgba,
            ColorFormat::Hsl,
        ] {
            assert_eq!(ColorFormat::from_name(f.label()).unwrap_or(f), f);
        }
        assert_eq!(ColorFormat::from_name("bogus"), None);
    }
}
