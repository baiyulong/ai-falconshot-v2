//! Clipboard content classification (PRD §5.8.2, §8.1) plus the HTML subset a
//! text card is allowed to render.
//!
//! The platform layer reads the clipboard and hands the pieces over; deciding
//! *what the user meant* is done here, where it is testable.

use crate::colors;
use crate::encode::{self, Format};
use crate::frame::Frame;
use std::path::PathBuf;
use thiserror::Error;

/// Everything the paste path is allowed to look at.
#[derive(Clone, Debug, Default)]
pub struct ClipFacts<'a> {
    /// Encoded bitmap bytes (PNG/BMP/TIFF/…), as `CF_DIB`/`CF_DIBV5` after the
    /// platform layer normalises them, or `CF_PNG`/`CF_HTML` image data.
    pub image_bytes: Option<&'a [u8]>,
    /// The `CF_HTML` blob: the whole envelope a browser or Office puts on the clipboard,
    /// its own header and fragment markers included. [`html_fragment`] cuts it.
    pub html: Option<&'a str>,
    pub text: Option<&'a str>,
    /// `CF_HDROP` list.
    pub files: &'a [PathBuf],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Image,
    Files,
    Html,
    Colour,
    Text,
    /// Nothing the paste path knows how to turn into a pin (§8.1).
    Unsupported,
}

/// The decided pin content, with the payload the renderer needs.
#[derive(Clone, Debug, PartialEq)]
pub enum ClipPayload {
    Image(Frame),
    /// Image files to read lazily; the first one is the displayed pin.
    Files(Vec<PathBuf>),
    /// Already sanitized.
    Html(String),
    Colour([u8; 4]),
    Text(String),
    Empty,
}

/// The paste path's failures are shown to the user (§8.1, §8.2), so they are
/// worded, not just debug-printed.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ClipError {
    /// A bitmap was present but undecodable: say so instead of pinning a
    /// placeholder (PRD §5.8.4 rule, §8.2).
    #[error("剪贴板里的图片无法解码：{0}")]
    ImageDecode(String),
    #[error("{0} 不是图片文件")]
    NotAnImageFile(PathBuf),
}

/// The text the user actually selected, cut out of a `CF_HTML` blob.
///
/// Cut on the two marker comments rather than on `StartFragment:`/`EndFragment:`: those
/// numbers are byte offsets into the *original* encoding, and the platform reader has
/// already lossily converted the blob to `String` by the time anyone asks this question, so
/// trusting them would be trusting a coordinate space that no longer exists. The markers
/// are what the copying app puts around exactly the fragment, and a blob without them (a
/// hand-written payload, an app that does not write them) is returned as it came.
pub fn html_fragment(raw: &str) -> &str {
    const OPEN: &str = "<!--StartFragment-->";
    // `to_ascii_lowercase` leaves every non-ASCII byte alone, so the indices it produces
    // are still byte offsets into `raw` and the two slices below cannot split a character.
    let lower = raw.to_ascii_lowercase();
    let Some(found) = lower.find("<!--startfragment-->") else {
        return raw;
    };
    let from = found + OPEN.len();
    // A blob that opened the fragment and never closed it (a truncated copy) still has its
    // envelope in front of the text, and the envelope is the one thing not to show.
    let Some(close) = lower[from..].find("<!--endfragment-->") else {
        return &raw[from..];
    };
    &raw[from..from + close]
}

/// §5.8.2 priority: a real bitmap beats a file list, which beats markup, which
/// beats a colour value, which beats plain text.
pub fn classify(facts: &ClipFacts<'_>) -> Result<(ClipKind, ClipPayload), ClipError> {
    if let Some(bytes) = facts.image_bytes {
        if !bytes.is_empty() {
            let frame = encode::decode(bytes).map_err(|e| ClipError::ImageDecode(e.to_string()))?;
            return Ok((ClipKind::Image, ClipPayload::Image(frame)));
        }
    }

    let images: Vec<PathBuf> = facts
        .files
        .iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .and_then(Format::from_ext)
                .is_some()
        })
        .cloned()
        .collect();
    if !images.is_empty() {
        if facts.files.len() == images.len() {
            return Ok((ClipKind::Files, ClipPayload::Files(images)));
        }
        // A mixed drop (a picture plus a .txt) is ambiguous; taking the image
        // quietly would lose what the user selected.
        return Err(ClipError::NotAnImageFile(
            facts
                .files
                .iter()
                .find(|p| !images.contains(p))
                .cloned()
                .unwrap_or_default(),
        ));
    }

    if let Some(html) = facts.html {
        let trimmed = html_fragment(html).trim();
        if !trimmed.is_empty() {
            return Ok((ClipKind::Html, ClipPayload::Html(sanitize_html(trimmed))));
        }
    }

    if let Some(text) = facts.text {
        let t = text.trim();
        if t.is_empty() {
            return Ok((ClipKind::Unsupported, ClipPayload::Empty));
        }
        if let Some(c) = colors::parse(t) {
            return Ok((ClipKind::Colour, ClipPayload::Colour(c)));
        }
        return Ok((ClipKind::Text, ClipPayload::Text(text.to_string())));
    }

    Ok((ClipKind::Unsupported, ClipPayload::Empty))
}

/// Tags the text card renders. Anything not here is dropped with its markup,
/// keeping its inner text — so a copied `<script>` never reaches a renderer.
const ALLOWED_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "li",
    "ol",
    "p",
    "pre",
    "s",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

/// Attributes kept, per tag. `href`/`src` are checked for a safe scheme.
const ALLOWED_ATTRS: &[&str] = &["align", "colspan", "href", "rowspan", "valign", "width"];

/// The members of [`ALLOWED_TAGS`] that start a line in a browser, and so start one in a
/// card ([`html_text`]). Inline tags — `a`, `b`, `em`, `i`, `span`, `code`, `sub`, `sup`,
/// `s`, `u`, `strike` — are deliberately absent: their markup is dropped and their text
/// stays where it was, which is the whole of what "基本格式" can mean without a style run.
const BLOCK_TAGS: &[&str] = &[
    "blockquote",
    "dd",
    "div",
    "dl",
    "dt",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "ol",
    "p",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

/// Only `http(s)` and `mailto` ever reach a renderer; `data:` is excluded
/// because `data:text/html` is a payload, not a link.
const SAFE_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// Tags whose *contents* are text to a browser, so dropping the markup alone
/// would leak `alert(1)` into the card as readable text.
const RAW_TEXT_TAGS: &[&str] = &[
    "applet", "iframe", "noembed", "noscript", "object", "script", "style", "textarea", "title",
    "xmp",
];

pub fn sanitize_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if matches!(chars.get(i + 1), Some('!') | Some('?')) {
            i = skip_bang(&chars, i);
            continue;
        }
        let Some(close) = find_char(&chars[i + 1..], '>') else {
            // Unterminated tag: the rest is text, and the `<` must not survive
            // as markup.
            out.push_str("&lt;");
            out.extend(chars[i + 1..].iter().cloned());
            break;
        };
        let tag_end = i + 1 + close;
        let raw: String = chars[i + 1..tag_end].iter().collect();
        i = tag_end + 1;
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let closing = raw.starts_with('/');
        let body = raw.trim_start_matches('/').trim_start_matches('!');
        let name: String = body
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if !closing && RAW_TEXT_TAGS.contains(&name.as_str()) {
            i = skip_past_close(&chars, i, &name);
            continue;
        }
        if !ALLOWED_TAGS.contains(&name.as_str()) {
            continue;
        }
        if closing {
            out.push_str("</");
            out.push_str(&name);
            out.push('>');
            continue;
        }
        let self_closing = body.ends_with('/');
        out.push('<');
        out.push_str(&name);
        for (k, v) in parse_attrs(body) {
            if !ALLOWED_ATTRS.contains(&k.as_str()) {
                continue;
            }
            if (k == "href" || k == "src") && !scheme_is_safe(&v) {
                continue;
            }
            out.push_str(&format!(" {k}=\"{}\"", escape_attr(&v)));
        }
        out.push_str(if self_closing { "/>" } else { ">" });
    }
    out
}

/// §5.8.2's 「HTML 文本在安全范围内保留基本格式」, read as far as a card can draw it:
/// the markup's *line structure* survives, its styling does not.
///
/// The PRD names the ask without defining the line, and this is where it is drawn:
/// paragraphs, list items, headings, table rows and `<br>` each start a line, because
/// that is what a browser does and a card that flattened them would be a card that
/// turned a copied error log into one long word salad. Bold, colour, font and size are
/// *not* kept — keeping them would mean the card carries per-run styles, which §5.8.5
/// does not ask for and §5.7.11 already answers for the text you type yourself.
///
/// It runs on [`sanitize_html`]'s output or on raw markup alike: a tag it does not know
/// is dropped with its markup and keeps its inner text, and the raw-text tags
/// ([`RAW_TEXT_TAGS`]) lose their contents, so `alert(1)` never reaches the card as
/// readable text from a copied `<script>`. Entities come back to characters here — the
/// card is the last stop, so `&lt;` is a `<` to read rather than markup to fear.
///
/// Whitespace: outside `<pre>` every run of it becomes one space, because HTML's source
/// newlines are not the author's line breaks and a card that honoured them would break
/// every wrapped paragraph in a copied web page. Inside `<pre>` the source spacing is
/// the content, so it is kept — and the one place this stops short of perfect is the
/// final tidy, which trims trailing spaces and folds runs of blank lines everywhere,
/// including inside a `pre` that happens to hold three blank lines in a row.
pub fn html_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2 + 16);
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    let mut pre = 0usize;
    while i < chars.len() {
        if chars[i] != '<' {
            if chars[i].is_ascii_whitespace() && pre == 0 {
                if !matches!(out.chars().next_back(), Some('\n') | Some(' ')) {
                    out.push(' ');
                }
                i += 1;
                continue;
            }
            let run = decode_entity(&chars, &mut i);
            out.push_str(&run);
            continue;
        }
        if matches!(chars.get(i + 1), Some('!') | Some('?')) {
            i = skip_bang(&chars, i);
            continue;
        }
        let Some(close) = find_char(&chars[i + 1..], '>') else {
            // Unterminated tag: the rest is text, exactly as `sanitize_html` reads it.
            out.extend(chars[i + 1..].iter().filter(|c| !c.is_ascii_whitespace()));
            break;
        };
        let tag_end = i + 1 + close;
        let raw: String = chars[i + 1..tag_end].iter().collect();
        i = tag_end + 1;
        let body = raw.trim().trim_start_matches('/').trim_start_matches('!');
        let name: String = body
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let closing = raw.trim().starts_with('/');
        if name.is_empty() {
            continue;
        }
        if !closing && RAW_TEXT_TAGS.contains(&name.as_str()) {
            i = skip_past_close(&chars, i, &name);
            continue;
        }
        if name == "pre" {
            if closing {
                pre = pre.saturating_sub(1);
            } else {
                pre += 1;
            }
            push_break(&mut out);
            continue;
        }
        if closing && !BLOCK_TAGS.contains(&name.as_str()) {
            continue;
        }
        match name.as_str() {
            "br" | "hr" => push_break(&mut out),
            "li" if !closing => {
                push_break(&mut out);
                out.push_str("\u{2022} ");
            }
            _ if BLOCK_TAGS.contains(&name.as_str()) => push_break(&mut out),
            _ => {}
        }
    }
    tidy(&out)
}

/// One paragraph break, never two in a row from the walker itself: `<div><p>` opens two
/// blocks and a browser puts the text on one new line, not two blank ones.
fn push_break(out: &mut String) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Decode one source token at `*i`, advancing it past at least one character.
///
/// Entities are the reason this is a function rather than a `push`: `&amp;lt;` is the
/// text `&lt;`, so a decoded `&` must never be scanned again.
fn decode_entity(chars: &[char], i: &mut usize) -> String {
    let mut out = String::new();
    let j = *i;
    if chars[j] == '&' {
        if let Some(off) = find_char(&chars[j..], ';') {
            let body: String = chars[j + 1..j + off].iter().collect();
            let named = match body.as_str() {
                "amp" => Some("&"),
                "lt" => Some("<"),
                "gt" => Some(">"),
                "quot" => Some("\""),
                "apos" => Some("'"),
                "nbsp" => Some("\u{a0}"),
                _ => None,
            };
            let numeric = body.strip_prefix('#').and_then(|d| {
                let (hex, digits) = match d.strip_prefix('x').or_else(|| d.strip_prefix('X')) {
                    Some(rest) => (true, rest),
                    None => (false, d),
                };
                if digits.is_empty() {
                    return None;
                }
                u32::from_str_radix(digits, if hex { 16 } else { 10 })
                    .ok()
                    .and_then(char::from_u32)
                    .map(|c| c.to_string())
            });
            // A body nobody can read stays literal: `&copy` without a `;` is the
            // user's text, not a mistake to erase.
            if let Some(text) = named.map(str::to_string).or(numeric) {
                out.push_str(&text);
                *i = j + off + 1;
                return out;
            }
        }
    }
    out.push(chars[j]);
    *i = j + 1;
    out
}

/// Trim each line's trailing spaces, fold runs of blank lines to one, drop the leading
/// and trailing ones. `card::boxes` treats an empty line as a real gap, so this is what
/// keeps a copied page's markup indentation from turning into a card half text.
fn tidy(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end();
        if line.is_empty() && out.last().is_some_and(|l| l.is_empty()) {
            continue;
        }
        out.push(line.to_string());
    }
    while out.first().is_some_and(|l| l.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Drop a comment (`<!-- -->`) or other declaration (`<!doctype>`, `<?…>`) in
/// full, so a `>` inside comment text cannot come back as markup.
fn skip_bang(chars: &[char], i: usize) -> usize {
    if chars[i..].starts_with(&['<', '!', '-', '-']) {
        let from = (i + 4).min(chars.len());
        if let Some(off) = find_slice(&chars[from..], &['-', '-', '>']) {
            return from + off + 3;
        }
        return match find_char(&chars[from..], '>') {
            Some(off) => from + off + 1,
            None => chars.len(),
        };
    }
    let from = (i + 2).min(chars.len());
    match find_char(&chars[from..], '>') {
        Some(off) => from + off + 1,
        None => chars.len(),
    }
}

/// Index just past the first `</name>` at or after `i`, or the end of input.
fn skip_past_close(chars: &[char], start: usize, name: &str) -> usize {
    let n: Vec<char> = name.chars().collect();
    let mut i = start;
    while i < chars.len() {
        if chars[i] == '<' && chars.get(i + 1) == Some(&'/') {
            let j = i + 2;
            let hit = j + n.len() <= chars.len()
                && chars[j..j + n.len()]
                    .iter()
                    .zip(n.iter())
                    .all(|(a, b)| a.eq_ignore_ascii_case(b));
            if hit {
                let after = j + n.len();
                let boundary = match chars.get(after) {
                    None => true,
                    Some(c) => *c == '>' || *c == '/' || c.is_whitespace(),
                };
                if boundary {
                    return match find_char(&chars[after..], '>') {
                        Some(off) => after + off + 1,
                        None => chars.len(),
                    };
                }
            }
        }
        i += 1;
    }
    i
}

fn find_slice(hay: &[char], needle: &[char]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn find_char(hay: &[char], c: char) -> Option<usize> {
    hay.iter().position(|&x| x == c)
}

/// `key="value"` / `key='value'` / `key=value`, dropping anything whose name
/// starts with `on` (§7.5: no executable markup in a pasted card).
fn parse_attrs(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = body.chars().skip_while(|c| c.is_alphanumeric()).collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && !(chars[i].is_alphabetic() || chars[i] == '_') {
            i += 1;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-')
        {
            i += 1;
        }
        if start == i {
            break;
        }
        let key: String = chars[start..i].iter().collect();
        let key = key.to_ascii_lowercase();
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() || chars[i] != '=' {
            out.push((key, String::new()));
            continue;
        }
        i += 1; // '='
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let value = if matches!(chars.get(i), Some('"') | Some('\'')) {
            let quote = chars[i];
            i += 1;
            let v_start = i;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }
            let v: String = chars[v_start..i].iter().collect();
            i += 1;
            v
        } else {
            let v_start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            chars[v_start..i].iter().collect()
        };
        if !key.starts_with("on") {
            out.push((key, value));
        }
    }
    out
}

/// A URL with no scheme is relative (or a fragment) and stays; one with a
/// scheme must use an allow-listed scheme, which also kills `javascript:` and
/// the `java\tscript:` spellings that browsers still normalise.
fn scheme_is_safe(v: &str) -> bool {
    let v = v.trim();
    match v.find(':') {
        None => true,
        Some(i) => {
            let scheme = &v[..i];
            !scheme.is_empty()
                && scheme.chars().all(|c| c.is_ascii_alphanumeric())
                && SAFE_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
        }
    }
}

fn escape_attr(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_start_lines_and_inline_tags_do_not() {
        assert_eq!(
            html_text("<p>one</p><p>two</p>"),
            "one\ntwo".to_string(),
            "two paragraphs are two lines"
        );
        assert_eq!(
            html_text("<div>a<b>b</b>c</div>"),
            "abc".to_string(),
            "bold keeps its text where it was, with no space invented around the markup"
        );
        // The markup's own newlines are not the author's line breaks (§5.8.2's reading).
        assert_eq!(html_text("<p>\n  one\n  two\n</p>"), "one two".to_string());
        assert_eq!(html_text("x<br/>y"), "x\ny".to_string());
        assert_eq!(
            html_text("<ul><li>a<li>b</ul>"),
            "\u{2022} a\n\u{2022} b".to_string()
        );
        // Two cells are two lines; the `<table>`/`<tr>` wrapped around them adds no
        // blank ones, because `push_break` refuses a break beside a break.
        assert_eq!(
            html_text("<table><tr><td>1</td><td>2</td></tr></table>"),
            "1\n2".to_string()
        );
    }

    #[test]
    fn pre_keeps_the_spacing_that_was_its_content() {
        let got = html_text("<pre>  a\n  bb\n</pre>");
        assert_eq!(got, "  a\n  bb".to_string(), "indentation is the point");
        assert_eq!(
            html_text("<p>a  b</p>"),
            "a b".to_string(),
            "outside pre it collapses"
        );
    }

    #[test]
    fn entities_arrive_as_the_characters_the_user_read() {
        assert_eq!(html_text("<p>a &amp; b</p>"), "a & b".to_string());
        assert_eq!(html_text("<p>&lt;tag&gt;</p>"), "<tag>".to_string());
        assert_eq!(html_text("<p>&#65;&#x42;</p>"), "AB".to_string());
        assert_eq!(html_text("<p>1&nbsp;2</p>"), "1\u{a0}2".to_string());
        // A body nobody can read is the user's text, not a mistake to erase.
        assert_eq!(html_text("<p>&copy 2026</p>"), "&copy 2026".to_string());
        // Double-encoded markup stays text: it is decoded once and not re-scanned.
        assert_eq!(
            html_text("<p>&amp;lt;b&amp;gt;</p>"),
            "&lt;b&gt;".to_string()
        );
    }

    #[test]
    fn a_script_loses_its_contents_not_just_its_markup() {
        assert_eq!(
            html_text("<p>keep</p><script>alert(1)</script>"),
            "keep".to_string()
        );
        assert_eq!(
            html_text("<style>p{color:red}</style><p>body</p>"),
            "body".to_string()
        );
        assert_eq!(
            html_text("<!-- <p>hidden</p> --><p>shown</p>"),
            "shown".to_string()
        );
    }

    #[test]
    fn sanitised_html_reads_the_same_as_raw() {
        let raw = "<p onclick=\"x()\">a &amp; b<script>alert(1)</script></p>";
        assert_eq!(
            html_text(sanitize_html(raw).as_str()),
            html_text(raw),
            "the card reads one answer whichever door the markup came in through"
        );
    }

    #[test]
    fn blank_lines_fold_to_one_gap_and_the_edges_are_trimmed() {
        assert_eq!(
            html_text("<p></p><p></p><p></p><p>x</p>"),
            "x".to_string(),
            "empty blocks are not three blank lines of card"
        );
        assert_eq!(html_text("<p>a</p><p>b</p><p>c</p>"), "a\nb\nc".to_string());
        assert_eq!(html_text("   "), String::new());
    }

    fn png_bytes() -> Vec<u8> {
        let f = Frame::filled(4, 4, [1, 2, 3, 255]).unwrap();
        encode::encode(&f, &encode::EncodeOptions::default()).unwrap()
    }

    #[test]
    fn bitmap_wins_over_everything() {
        let files = [PathBuf::from("C:/a.png")];
        let facts = ClipFacts {
            image_bytes: Some(&png_bytes()),
            html: Some("<b>hi</b>"),
            text: Some("#ff0000"),
            files: &files,
        };
        let (kind, payload) = classify(&facts).unwrap();
        assert_eq!(kind, ClipKind::Image);
        match payload {
            ClipPayload::Image(f) => assert_eq!((f.width, f.height), (4, 4)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn priority_order_text_over_image_is_not_possible() {
        let files = [PathBuf::from("C:/a.png"), PathBuf::from("C:/b.png")];
        let facts = ClipFacts {
            html: Some("<b>x</b>"),
            text: Some("#ff0000"),
            files: &files,
            ..Default::default()
        };
        assert_eq!(classify(&facts).unwrap().0, ClipKind::Files);

        let facts = ClipFacts {
            html: Some("<b>x</b>"),
            text: Some("#ff0000"),
            ..Default::default()
        };
        assert_eq!(classify(&facts).unwrap().0, ClipKind::Html);

        let facts = ClipFacts {
            text: Some("  #ff0000 "),
            ..Default::default()
        };
        assert_eq!(
            classify(&facts).unwrap(),
            (ClipKind::Colour, ClipPayload::Colour([255, 0, 0, 255]))
        );

        let facts = ClipFacts {
            text: Some("hello"),
            ..Default::default()
        };
        assert_eq!(classify(&facts).unwrap().0, ClipKind::Text);

        // §8.1: nothing usable, and no blank window.
        let facts = ClipFacts::default();
        assert_eq!(
            classify(&facts).unwrap(),
            (ClipKind::Unsupported, ClipPayload::Empty)
        );
        let facts = ClipFacts {
            text: Some("   "),
            ..Default::default()
        };
        assert_eq!(
            classify(&facts).unwrap(),
            (ClipKind::Unsupported, ClipPayload::Empty)
        );
    }

    #[test]
    fn the_fragment_is_cut_on_the_markers_not_the_offsets() {
        // The offsets here are deliberately wrong (they point at the middle of `<html>`):
        // a card that trusted them would cut its own text at a byte that no longer exists.
        let raw = concat!(
            "Version:0.9\r\nStartFragment:00000001\r\nEndFragment:00000002\r\n",
            "<body><!--StartFragment--><p>kept</p><!--EndFragment--></body>"
        );
        assert_eq!(html_fragment(raw), "<p>kept</p>");
        // Non-ASCII before the markers is what makes the wrong cut dangerous: the
        // lowercasing that finds the marker must not shift the indices it reports.
        let raw = "Version:0.9 前缀中<!--StartFragment--><p>ok</p><!--EndFragment-->";
        assert_eq!(html_fragment(raw), "<p>ok</p>");
        // No markers, or one of them: the payload is the user's, returned as it came.
        assert_eq!(html_fragment("<p>one</p>"), "<p>one</p>");
        assert_eq!(
            html_fragment("<!--StartFragment--><p>one</p>"),
            "<p>one</p>"
        );
        assert_eq!(
            html_fragment("<!--EndFragment--><p>one</p>"),
            "<!--EndFragment--><p>one</p>"
        );
    }

    /// What the clipboard's `HTML Format` really carries is the CF_HTML *envelope*: the
    /// reader in `crates/platform-windows/src/clip.rs` hands the whole blob over, offsets
    /// and all. A card that opened with 「Version:0.9 StartHTML:…」 would be showing the
    /// bookkeeping of a format the user never saw or copied.
    #[test]
    fn the_cf_html_envelope_is_read_as_its_fragment() {
        let raw = concat!(
            "Version:0.9\r\nStartHTML:00000097\r\nEndHTML:00000226\r\n",
            "StartFragment:00000133\r\nEndFragment:00000172\r\n",
            "<html>\r\n<body>\r\n<!--StartFragment--><p>one</p><p>two</p>",
            "<!--EndFragment-->\r\n</body>\r\n</html>"
        );
        let facts = ClipFacts {
            html: Some(raw),
            ..Default::default()
        };
        let (kind, payload) = classify(&facts).unwrap();
        assert_eq!(kind, ClipKind::Html);
        let ClipPayload::Html(sanitized) = payload else {
            panic!("the envelope reached the paste path as something else");
        };
        assert_eq!(html_text(&sanitized), "one\ntwo".to_string(), "{sanitized}");

        // A payload with no fragment comments stays what it was: guessing where a
        // hand-written one ends is how a card loses a paragraph.
        let facts = ClipFacts {
            html: Some("<p>one</p>"),
            ..Default::default()
        };
        assert_eq!(
            classify(&facts).unwrap(),
            (ClipKind::Html, ClipPayload::Html("<p>one</p>".to_string()))
        );
    }

    #[test]
    fn mixed_file_drop_is_refused_not_guessed() {
        let files = [PathBuf::from("C:/a.png"), PathBuf::from("C:/notes.txt")];
        let facts = ClipFacts {
            files: &files,
            ..Default::default()
        };
        assert_eq!(
            classify(&facts),
            Err(ClipError::NotAnImageFile(PathBuf::from("C:/notes.txt")))
        );
    }

    #[test]
    fn undecodable_bitmap_reports_the_reason() {
        let facts = ClipFacts {
            image_bytes: Some(&[0u8, 1, 2, 3]),
            ..Default::default()
        };
        match classify(&facts) {
            Err(ClipError::ImageDecode(_)) | Err(ClipError::NotAnImageFile(_)) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn html_keeps_the_subset_and_drops_the_rest() {
        let src = "<p onclick=\"steal()\">Hello <b>world</b><script>alert(1)</script>\
                   <style>body{}</style><a href=\"javascript:alert(1)\">x</a>\
                   <a href=\"https://example.com\">ok</a><img src=\"a.png\">";
        let out = sanitize_html(src);
        assert!(out.contains("<p>Hello <b>world</b>"), "{out}");
        assert!(out.contains("<b>world</b>"));
        assert!(!out.contains("alert"), "{out}");
        assert!(!out.contains("onclick"), "{out}");
        assert!(!out.contains("javascript"), "{out}");
        assert!(!out.contains("<script"), "{out}");
        assert!(!out.contains("<style"), "{out}");
        // `img` is not on the allow list: the card must not fetch on the user's
        // behalf, which also honours §5.8.4's "no silent low-quality substitute".
        assert!(!out.contains("<img"), "{out}");
        assert!(!out.contains("a.png"), "{out}");
        assert!(out.contains("https://example.com"), "{out}");
        assert_eq!(
            out,
            "<p>Hello <b>world</b><a>x</a><a href=\"https://example.com\">ok</a>"
        );
    }

    #[test]
    fn html_filter_is_total_on_junk() {
        for src in [
            "",
            "<",
            "<b",
            "<b>unterminated",
            "<!--",
            "<!-- c -->after",
            "<!-- a > b -->after",
            "<!doctype html><p>hi</p>",
            "<?xml version=\"1.0\"?><p>hi</p>",
            "<<>>",
            "<a href=",
            "<a href='https://x' >t</a>",
            "<br/>",
            "< p >spacey</p >",
            "<script>x</scripty>y",
            "<SCRIPT>a</ScRiPt>b",
            "<h1>t</h1><textarea>keep</textarea>done",
        ] {
            let _ = sanitize_html(src);
        }
        assert_eq!(sanitize_html("<!-- c -->after"), "after");
        // A `>` inside comment text must not resurrect the rest as markup.
        assert_eq!(sanitize_html("<!-- a > b -->after"), "after");
        assert_eq!(sanitize_html("<b>x</b >"), "<b>x</b>");
        assert_eq!(sanitize_html("<br/>"), "<br/>");
        assert_eq!(
            sanitize_html("<a href='https://x' >t</a>"),
            "<a href=\"https://x\">t</a>"
        );
        assert_eq!(sanitize_html("<SCRIPT>a</ScRiPt>b"), "b");
        assert_eq!(sanitize_html("<!doctype html><p>hi</p>"), "<p>hi</p>");
        // Raw-text elements swallow their body; an unterminated one takes the rest.
        assert_eq!(sanitize_html("<p>t</p><script>a</script>b"), "<p>t</p>b");
        assert_eq!(sanitize_html("<script>x</scripty>y"), "");
        assert_eq!(sanitize_html("<textarea>keep</textarea>done"), "done");
    }

    #[test]
    fn attribute_values_cannot_break_out() {
        // A quote inside the value cannot smuggle an event handler through.
        let out = sanitize_html("<a href=\"http://x/\\\" onmouseover=1\">t</a>");
        assert!(!out.contains("onmouseover"), "{out}");

        // An attribute left open at the first `>`: whatever followed must not
        // come back as live markup.
        let out = sanitize_html("<td width=\"><script>\">t</td>");
        assert!(!out.contains("<script"), "{out}");
        assert!(!out.contains("<b"), "{out}");

        // Values are re-escaped, so a stray quote cannot end the attribute.
        let out = sanitize_html("<td width=a\"b>t</td>");
        assert_eq!(out, "<td width=\"a&quot;b\">t</td>");
        assert!(sanitize_html("<td width=a&b>t</td>").contains("&amp;"));
        assert_eq!(
            sanitize_html("<a href=\"data:text/html,x\">t</a>"),
            "<a>t</a>"
        );
    }
}
