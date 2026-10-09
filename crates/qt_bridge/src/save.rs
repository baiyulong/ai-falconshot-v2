//! The one owner of "this picture, into that file, and what it cost" (§5.5.5).
//!
//! Two doors write a picture out: the mask's 另存为 (`mask::MaskState::save_out`) and
//! a pin's (`state::PinState::save_image`). Until P35 they spelled the same act twice,
//! and the two spellings had drifted: the pin took `..Default::default()` so
//! `[output] jpeg_quality` was inert on that half of the product (PRD 507 asks for a
//! configurable JPG quality, not a half-configurable one), it answered in English, and
//! it flattened transparency without saying so (PRD 506/447/521). A third door would
//! drift the same way, so the sentence, the format choice and the count live here and
//! the doors only say what a refusal costs *their* user.

use std::borrow::Cow;

use falcon_core::encode::{self, EncodeOptions};
use falcon_core::frame::Frame;

/// Why the write did not happen. The two arms need different tails from a caller:
/// a refused name is the user's own doing and stands alone, while a failed write
/// means the picture is still in memory and can be asked for again.
#[derive(Debug)]
pub enum Refusal {
    /// The name carries an extension this app cannot write. Nothing was touched.
    BadName(String),
    /// The encoder or the disk said no. The picture is still there.
    Write(String),
}

/// Turns what a dialog hands the product into the path it names.
///
/// `FileDialog.selectedFile` is a `QUrl`, and this Qt gives JS no way to convert one:
/// the member read out of the runtime is `undefined` at both the empty and the set
/// value (`toLocalFile`, `isFile` and `path` alike - P37 measured the `TypeError` that
/// `AnnotationToolbar.qml` and `PinWindow.qml` were about to throw on every accept), so
/// the only text a door can pass is the URL itself. Read as a path, `file:///C:/x.png`
/// is a *relative* path whose first component is `file:`, so the write would land
/// somewhere the user never named and the sentence would still say 已保存到.
pub fn local_path(typed: &str) -> Cow<'_, str> {
    let Some(rest) = typed.strip_prefix("file://") else {
        return Cow::Borrowed(typed);
    };
    // `file:///C:/…` has an empty host; `file://server/share/…` is UNC, and the two
    // need different repairs: one slash to drop, and a `\\server` to put back.
    let (host, path) = match rest.split_once('/') {
        Some(("", path)) => ("", path),
        Some((host, path)) => (host, path),
        None => ("", rest),
    };
    let decoded = percent_decode(path);
    // A drive path (`/C:/x`) loses its leading slash; every separator becomes
    // Windows', because that is the form the dialog's own text was written in.
    let body = decoded.trim_start_matches('/').replace('/', "\\");
    if host.is_empty() {
        Cow::Owned(body)
    } else {
        Cow::Owned(format!("\\\\{host}\\{body}"))
    }
}

/// One pass of `%XX`, so a name that legitimately contains a `%` stays itself:
/// `%2520` decodes to `%20`, not to a space.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &text[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // The bytes came out of a `&str` one third at a time, so a `%XX` can only have
    // landed mid-codepoint if the dialog wrote it that way; decode best-effort.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Writes `frame` to the name the user gave, and returns the sentence to say about it.
///
/// `subject` is what the picture is called *from the caller's side* - 选区 behind the
/// mask, 贴图 on a pin - and it appears in both the size clause and the transparency
/// clause. It is a parameter rather than fixed wording because a sentence that says
/// "选区" about a floating window is a sentence nobody checked, which is exactly how the
/// English one got there.
pub fn write(
    frame: &Frame,
    typed: &str,
    export: &EncodeOptions,
    subject: &str,
) -> Result<String, Refusal> {
    let named = local_path(typed);
    let path = std::path::Path::new(named.as_ref());
    // An extension is the user's choice; a bare name is the configured format, because
    // the dialog was showing a format while they typed.
    let (target, format) = match path.extension() {
        Some(ext) => (
            path.to_path_buf(),
            encode::Format::from_ext(&ext.to_string_lossy()),
        ),
        None => {
            let mut suffixed = path.as_os_str().to_os_string();
            suffixed.push(".");
            suffixed.push(export.format.ext());
            (std::path::PathBuf::from(suffixed), Some(export.format))
        }
    };
    let Some(format) = format else {
        // The converted name, because the `file:///` is the dialog's own writing and
        // not something the user recognises as the name they typed.
        return Err(Refusal::BadName(format!(
            "{named} 不是本程序会写的格式：用 .png、.jpg 或 .bmp。"
        )));
    };
    // Counted before the write, from the frame about to be encoded: asking the file
    // afterwards would be asking a JPG whether it kept an alpha channel. `!= 255` and
    // not `== 0`, because a half-transparent pixel is just as unsavable - and this is
    // the same predicate `Frame::has_transparency` uses.
    let filled = if format.supports_alpha() || !frame.has_transparency() {
        0
    } else {
        frame
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] != 255)
            .count()
    };
    let opt = EncodeOptions { format, ..*export };
    match encode::save(frame, &target, &opt) {
        // The frame's size, not the file's: `encode` rescales an ICO down to 256 before
        // it writes, and a sentence quoting the file would quote a picture the user
        // never looked at. The frame is true of every format.
        Ok(()) => Ok(format!(
            "已保存到 {}（{subject} {}×{} 像素）。{}",
            target.display(),
            frame.width,
            frame.height,
            if filled == 0 {
                String::new()
            } else {
                format!(
                    "{subject}里有 {filled} px 透明，{} 存不下，已填成白色；要保留透明请存成 PNG。",
                    format.label()
                )
            }
        )),
        Err(e) => Err(Refusal::Write(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use falcon_core::encode::Format;

    /// A directory under the OS temp dir, emptied first: a file the previous run of
    /// this test left behind would be read back as this one's answer to a write that
    /// never happened.
    fn sandbox(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-save-owner-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    /// A picture with something to lose on both counts: a texture that a JPG has to
    /// actually compress (a flat colour gives the same bytes at every quality, which
    /// would let an inert knob pass this leg) and every fourth pixel see-through, so a
    /// format without alpha has a number to report.
    fn textured(w: u32, h: u32) -> Frame {
        let mut f = Frame::new(w, h).expect("frame");
        for y in 0..h {
            for x in 0..w {
                let a = if (x + y) % 4 == 0 { 0 } else { 255 };
                f.set(
                    x,
                    y,
                    [(x * 7) as u8, (y * 13) as u8, ((x + y) * 3) as u8, a],
                );
            }
        }
        f
    }

    /// `[output] jpeg_quality` only reaches the file if this owner carries it, and the
    /// pin half of the product is the reason that sentence is worth writing: until P35
    /// it built its own options and the knob was inert behind a floating window. Core
    /// proves the *encoder* obeys quality; what is proved here is that the door does
    /// not throw the value away on the way.
    #[test]
    fn the_quality_it_is_handed_is_the_quality_it_writes() {
        let dir = sandbox("quality");
        let frame = textured(192, 144);
        let mut bytes = Vec::new();
        for quality in [30u8, 95u8] {
            let path = dir.join(format!("shot-q{quality}.jpg"));
            let said = write(
                &frame,
                &path.to_string_lossy(),
                &EncodeOptions {
                    quality,
                    ..Default::default()
                },
                "贴图",
            )
            .expect("write");
            assert!(said.contains("已保存到"), "{said}");
            bytes.push(std::fs::read(&path).expect("the file the sentence promised"));
        }
        assert_ne!(bytes[0], bytes[1], "both qualities wrote one file");
        assert_ne!(
            bytes[0].len(),
            bytes[1].len(),
            "quality 30 and quality 95 wrote the same size: {} bytes",
            bytes[0].len()
        );
    }

    /// The three sentences that were the pin door's absence: it answered in English and
    /// it flattened without saying so. The count is the whole point of the notice, so
    /// it is compared against what the frame actually held rather than against a
    /// number this test also happens to compute by eye.
    #[test]
    fn a_flattened_picture_names_its_loss_and_who_it_belongs_to() {
        let dir = sandbox("notice");
        let frame = textured(64, 64);
        let lost = frame
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] != 255)
            .count();
        assert!(lost > 0, "the test picture had nothing to lose");

        let pin = dir.join("pin.jpg");
        let said = write(
            &frame,
            &pin.to_string_lossy(),
            &EncodeOptions::default(),
            "贴图",
        )
        .expect("write");
        assert!(
            said.contains(&format!("贴图里有 {lost} px 透明")),
            "the pin's sentence did not name its own loss: {said}"
        );
        assert!(said.contains("填成白色") && said.contains("PNG"), "{said}");
        assert!(
            said.contains("贴图 64×64 像素"),
            "the size clause called a 贴图 something else: {said}"
        );

        // The same frame behind the mask's door: same loss, same number, different name
        // for the picture - which is why `subject` is an argument and not a string.
        let sel = dir.join("selection.jpg");
        let other = write(
            &frame,
            &sel.to_string_lossy(),
            &EncodeOptions::default(),
            "选区",
        )
        .expect("write");
        assert!(
            other.contains(&format!("选区里有 {lost} px 透明")),
            "the selection's count moved between the two doors: {other}"
        );
        assert!(
            !said.contains("选区") && !other.contains("贴图"),
            "{said} | {other}"
        );

        // And a format that keeps the alpha gets no lecture about losing it.
        let png = dir.join("pin.png");
        let kept = write(
            &frame,
            &png.to_string_lossy(),
            &EncodeOptions::default(),
            "贴图",
        )
        .expect("write");
        assert!(
            !kept.contains("透明"),
            "PNG was told it lost transparency: {kept}"
        );
    }

    /// A name typed without an extension is the configured format's, which on the pin
    /// half used to mean "whatever `Default` said" - PNG, always, however the file was
    /// named and whatever the user had set. The extension is the user's choice, so the
    /// fallback is the only place the setting can speak.
    #[test]
    fn a_bare_name_becomes_the_configured_format() {
        let frame = textured(48, 32);
        for (format, ext) in [
            (Format::Bmp, "bmp"),
            (Format::Tga, "tga"),
            (Format::Png, "png"),
        ] {
            let dir = sandbox("bare");
            let typed = dir.join("shot");
            let said = write(
                &frame,
                &typed.to_string_lossy(),
                &EncodeOptions {
                    format,
                    ..Default::default()
                },
                "贴图",
            )
            .expect("write");
            let named = dir.join(format!("shot.{ext}"));
            assert!(
                named.exists(),
                "{ext}: the bare name did not become {named:?}"
            );
            assert!(
                !typed.exists(),
                "{ext}: the name without one was written too"
            );
            assert!(said.contains(&format!("shot.{ext}")), "{ext}: {said}");
        }
    }

    /// The rungs that answer before a byte is written must not leave a file behind -
    /// and a refused name is the user's own doing, so it stands without the tail each
    /// caller adds about the picture still being there.
    #[test]
    fn a_name_that_is_not_a_format_touches_nothing() {
        let dir = sandbox("refusal");
        let odd = dir.join("shot.qqq");
        let err = write(
            &textured(16, 16),
            &odd.to_string_lossy(),
            &EncodeOptions::default(),
            "贴图",
        )
        .expect_err("a format this app cannot write was accepted");
        match err {
            Refusal::BadName(said) => assert!(said.contains("不是本程序会写的格式"), "{said}"),
            Refusal::Write(said) => panic!("a refused name came back as a failed write: {said}"),
        }
        assert!(!odd.exists(), "the refused name appeared on disk");
        assert_eq!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .filter_map(Result::ok)
                .count(),
            0,
            "a refusal left something in the sandbox"
        );
    }

    /// The other arm: the encoder said yes and the disk said no. What the caller needs
    /// from this one is the picture's name *and* the fact that nothing was consumed,
    /// which is why the two arms are an enum rather than one string.
    #[test]
    fn a_write_the_machine_refuses_is_not_reported_as_a_bad_name() {
        let dir = sandbox("blocked");
        let blocker = dir.join("shot.png");
        std::fs::write(&blocker, b"not a directory").expect("blocker");
        let err = write(
            &textured(16, 16),
            &dir.join("shot.png").join("inner.png").to_string_lossy(),
            &EncodeOptions::default(),
            "贴图",
        )
        .expect_err("a write into a file was accepted");
        match err {
            Refusal::Write(said) => assert!(!said.is_empty(), "a failure said nothing"),
            Refusal::BadName(said) => {
                panic!("a disk that refused is not a name that refused: {said}")
            }
        }
    }

    /// ICO is clamped to 256 px on the way out (`ICO_MAX_SIDE`), so the file and the
    /// picture are two different sizes. The sentence has to quote the one the user
    /// looked at; quoting the one the encoder settled for would tell them their pin
    /// shrank.
    #[test]
    fn the_size_named_is_the_picture_not_the_file() {
        let dir = sandbox("ico");
        let frame = textured(600, 400);
        let ico = dir.join("shot.ico");
        let said = write(
            &frame,
            &ico.to_string_lossy(),
            &EncodeOptions::default(),
            "贴图",
        )
        .expect("write");
        let back = encode::decode_file(&ico).expect("decode");
        assert!(
            back.width < frame.width && back.height < frame.height,
            "the file came back the size of the picture, so this test proved nothing: {:?}",
            (back.width, back.height)
        );
        assert!(
            said.contains("贴图 600×400 像素"),
            "the sentence quoted the file instead of the picture: {said}"
        );
    }

    /// The two QML doors hand a URL, because this Qt gives JS no way to turn one into
    /// a path (`toLocalFile` measured `undefined` at both the empty and the set value).
    /// A URL read as a path is a *relative* path whose first component is `file:`, so
    /// without this leg the dialog writes somewhere the user never named and still says
    /// 已保存到 - a wrong file with a confident sentence, which is the worst thing this
    /// owner can do.
    #[test]
    fn the_url_a_dialog_hands_over_lands_where_it_names() {
        let dir = sandbox("url");
        let plain = dir.join("shot.png");
        let typed = format!(
            "file:///{}/shot.png",
            dir.to_string_lossy().replace('\\', "/")
        );
        let said =
            write(&textured(24, 16), &typed, &EncodeOptions::default(), "贴图").expect("write");
        assert!(plain.exists(), "{typed} did not reach {plain:?}: {said}");
        assert!(
            said.contains(&plain.to_string_lossy().to_string()),
            "{said}"
        );
        assert!(!said.contains("file://"), "{said}");
        // Nothing was written at the literal text either: a second file would mean the
        // URL was treated as a name rather than converted.
        assert_eq!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .filter_map(Result::ok)
                .count(),
            1,
            "the URL form also appeared on disk as a name"
        );
    }

    /// A space in a folder name reaches the dialog as `%20`, so the decoding has to
    /// happen or the write lands in a directory that does not exist. One pass only:
    /// `%2520` is a name that contains `%20`, and a second pass would move that file.
    #[test]
    fn a_percent_in_the_name_is_read_once() {
        assert_eq!(
            local_path("file:///C:/My%20Docs/shot.png"),
            "C:\\My Docs\\shot.png"
        );
        assert_eq!(
            local_path("file:///C:/100%25/shot.png"),
            "C:\\100%\\shot.png"
        );
        assert_eq!(local_path("file:///C:/a%2520b.png"), "C:\\a%20b.png");
        // Not a hex pair after the `%` is the user's own punctuation, left alone.
        assert_eq!(local_path("file:///C:/100%zz.png"), "C:\\100%zz.png");
    }

    /// The shapes a `QUrl` can carry, and the one shape the Rust doors already use.
    #[test]
    fn a_path_that_is_already_one_is_carried_untouched() {
        let bare = r"C:\Users\me\Pictures\shot.png";
        assert!(
            matches!(local_path(bare), Cow::Borrowed(s) if s == bare),
            "a plain path was rewritten"
        );
        assert_eq!(local_path("shot.png"), "shot.png");
        // UNC: the host is part of the name, so it has to come back as `\\server\…`.
        assert_eq!(
            local_path("file://NAS/share/shot.png"),
            "\\\\NAS\\share\\shot.png"
        );
        // A scheme this app was never handed is not ours to reinterpret.
        assert_eq!(
            local_path("https://example.com/a.png"),
            "https://example.com/a.png"
        );
    }

    /// The refusal has to name the thing the user can find in the dialog's edit box,
    /// not the URL the property returned behind their back.
    #[test]
    fn a_refused_url_speaks_of_the_path_it_read() {
        let dir = sandbox("url-refusal");
        let odd = format!(
            "file:///{}/shot.qqq",
            dir.to_string_lossy().replace('\\', "/")
        );
        let err = write(&textured(8, 8), &odd, &EncodeOptions::default(), "贴图")
            .expect_err("a format this app cannot write was accepted");
        match err {
            Refusal::BadName(said) => {
                assert!(said.contains("不是本程序会写的格式"), "{said}");
                assert!(
                    !said.contains("file://"),
                    "the refusal quoted the dialog's url: {said}"
                );
                assert!(said.contains("shot.qqq"), "{said}");
            }
            Refusal::Write(said) => panic!("a refused name came back as a failed write: {said}"),
        }
    }
}
