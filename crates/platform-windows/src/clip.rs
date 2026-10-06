//! The Windows clipboard (PRD §5.8.2 paste-in, §5.9.16 copy-out).
//!
//! Two layers live here. The pure one converts between a `CF_DIB` and a
//! [`Frame`], and is unit-testable on any machine; the unsafe one is the Win32
//! clipboard, which is not. Splitting them matters because the byte layout of a
//! DIB is where this goes wrong if it goes wrong - and the symptom of a wrong
//! stride or a row order flip is a corrupted paste, not an error message.
//!
//! Plan §3.3's rule applies throughout: every bitmap byte is produced or
//! consumed by Rust's `image`. Qt is not in this path at all.

use std::path::PathBuf;

use falcon_core::clip::ClipFacts;
use falcon_core::encode::{self, EncodeOptions, Format};
use falcon_core::frame::Frame;
use thiserror::Error;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardOwner, GetOpenClipboardWindow, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_TIFF, CF_UNICODETEXT};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// One clipboard read, in the pieces `core::clip` decides between. Deciding is
/// not this layer's job - it is the part that gets tested.
#[derive(Clone, Debug, Default)]
pub struct Contents {
    /// Normalised to PNG so one decoder owns every bitmap that comes in.
    pub image_bytes: Option<Vec<u8>>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub files: Vec<PathBuf>,
}

impl Contents {
    pub fn facts(&self) -> ClipFacts<'_> {
        ClipFacts {
            image_bytes: self.image_bytes.as_deref(),
            html: self.html.as_deref(),
            text: self.text.as_deref(),
            files: &self.files,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.image_bytes.is_none()
            && self.html.is_none()
            && self.text.is_none()
            && self.files.is_empty()
    }
}

#[derive(Clone, Debug, Error)]
pub enum ClipError {
    /// Another window holds the clipboard. PRD §7.2 names this as a case to
    /// survive, and surviving it means *saying who is holding it*: a bare
    /// "Access is denied." is not something a user can act on, so the holder is
    /// looked up and carried in the message.
    #[error("剪贴板被占用：{0}")]
    Locked(String),
    /// Every other clipboard refusal. These strings go to the user (§8.1), which
    /// is why they read as sentences rather than as API names.
    #[error("剪贴板操作失败：{0}")]
    Win(String),
}

/// A DIB's `BITMAPINFOHEADER` is 40 bytes and its width/height sit at fixed
/// offsets in every header size Windows has shipped.
const DIB_HEADER_MIN: usize = 40;

/// `CF_DIB`: 40-byte header, then 32-bit rows bottom-up, uncompressed. `Frame`
/// is top-down RGBA; Windows wants bottom-up BGRA.
pub fn dib_from_rgba(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = Vec::with_capacity(DIB_HEADER_MIN + w * h * 4);
    out.extend_from_slice(&(DIB_HEADER_MIN as u32).to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    // Positive height is the bottom-up convention. Top-down (negative) is legal
    // and fewer readers handle it, so the rows are flipped instead.
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&[1, 0, 32, 0]); // planes = 1, bitcount = 32
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // resolution, palette counts: unspecified
    for y in (0..h).rev() {
        for x in 0..w {
            let i = (y * w + x) * 4;
            out.extend_from_slice(&[pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3]]);
        }
    }
    out
}

/// The geometry a DIB header claims, without touching its bits.
pub fn dib_size(dib: &[u8]) -> Option<(u32, i32)> {
    if dib.len() < DIB_HEADER_MIN {
        return None;
    }
    let header = u32::from_le_bytes(dib[0..4].try_into().ok()?);
    if (header as usize) < DIB_HEADER_MIN || dib.len() < header as usize {
        return None;
    }
    let w = u32::from_le_bytes(dib[4..8].try_into().ok()?);
    let h = i32::from_le_bytes(dib[8..12].try_into().ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

/// `CF_DIB`/`CF_DIBV5` bytes to PNG bytes, through `core::encode`.
///
/// There is no DIB decoder in this file on purpose: a DIB is a BMP without its
/// 14-byte file header, so the header is synthesised and `image`'s BMP reader -
/// which handles 8/16/24/32 bpp, palettes, bitfields and both row orders - does
/// the decoding. Re-encoding to PNG keeps `ClipFacts::image_bytes` one format for
/// the paste path to reason about.
pub fn png_from_dib(dib: &[u8]) -> Option<Vec<u8>> {
    // Length first: `dib_size` is the only place that checks it, and the header
    // read below indexes the slice.
    dib_size(dib)?;
    let header = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0u8; 4]);
    // Pixel data starts right after the header the DIB actually carries; a V5
    // header is copied verbatim, so its colour masks stay where they belong.
    bmp.extend_from_slice(&((14 + header) as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    let frame = encode::decode(&bmp).ok()?;
    let opt = EncodeOptions {
        format: Format::Png,
        ..Default::default()
    };
    encode::encode(&frame, &opt).ok()
}

/// Who is in the way. `GetOpenClipboardWindow` names a window that has the
/// clipboard open *right now* - a lock that ends when that window closes it -
/// and `GetClipboardOwner` names the window whose data is on it, which is the one
/// that matters when the refusal is not a lock but a permission: a lower
/// integrity process cannot open a clipboard owned by an elevated one, and the
/// only clue the user gets is this sentence.
fn holder() -> String {
    // SAFETY: both calls are read-only queries that return a window handle or
    // nothing; the handle is only ever passed to other queries.
    let hwnd = unsafe {
        GetOpenClipboardWindow()
            .ok()
            .filter(|h| !h.is_invalid())
            .or_else(|| GetClipboardOwner().ok().filter(|h| !h.is_invalid()))
    };
    let Some(hwnd) = hwnd else {
        return "未找到占用剪贴板的窗口".to_string();
    };

    let Some(pid) = crate::proc::pid_of_window(hwnd.0 as u64) else {
        return "未找到占用剪贴板的窗口".to_string();
    };
    // SAFETY: a query about our own process, no handles involved.
    if pid == unsafe { GetCurrentProcessId() } {
        // This is the one answer that must never be shown as if it were someone
        // else's fault: our own thread left the clipboard open, so this is a bug
        // in this program, not a competing app.
        return "本进程自己未关闭剪贴板（程序缺陷）".to_string();
    }

    let mut parts = Vec::new();
    if let Some(exe) = crate::proc::process_name(pid) {
        parts.push(exe);
    }
    if let Some(title) = crate::proc::window_title(hwnd.0 as u64) {
        parts.push(format!("窗口「{title}」"));
    }
    if parts.is_empty() {
        parts.push(format!("pid {pid}"));
    }
    parts.join(" ")
}

/// `OpenClipboard` fails outright while another process is mid-copy, which is
/// normal at human speed. Eight short tries covers it without letting a blocked
/// clipboard hang the app - and a refusal that survives all eight is the
/// permission case above, so it comes back named.
fn open() -> Result<(), ClipError> {
    let mut last = None;
    for attempt in 0..8 {
        // SAFETY: a null owner associates the clipboard with this thread.
        match unsafe { OpenClipboard(None) } {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                if attempt < 7 {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            }
        }
    }
    let detail = match last {
        Some(e) => format!("{}（{}）", holder(), e.message()),
        None => holder(),
    };
    Err(ClipError::Locked(detail))
}

/// The raw handle of one format, for the readers that take a handle rather than
/// bytes - `CF_HDROP` is the only one here.
///
/// # Safety: the clipboard must be open on this thread.
unsafe fn handle_of(format: u32) -> Option<HANDLE> {
    if !IsClipboardFormatAvailable(format).is_ok() {
        return None;
    }
    let handle = GetClipboardData(format).ok()?;
    (!handle.0.is_null()).then_some(handle)
}

/// A copy of one format's bytes. The system keeps owning the memory.
///
/// # Safety: the clipboard must be open on this thread.
unsafe fn bytes_of(format: u32) -> Option<Vec<u8>> {
    let handle = handle_of(format)?;
    let mem = HGLOBAL(handle.0);
    let size = GlobalSize(mem);
    let ptr = GlobalLock(mem) as *const u8;
    if ptr.is_null() || size == 0 {
        return None;
    }
    let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
    let _ = GlobalUnlock(mem);
    Some(bytes)
}

fn utf16_to_string(units: &[u16]) -> String {
    crate::proc::wide_to_string(units)
}

fn registered(name: &str) -> Option<u32> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is alive for the duration of the call.
    let id = unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) };
    (id != 0).then_some(id as u32)
}

/// Reads every piece the paste path cares about. Priority between them is
/// `core::clip::classify`'s decision, not this function's.
pub fn read() -> Result<Contents, ClipError> {
    let mut out = Contents::default();
    open()?;
    // Each read below is individually guarded rather than wrapped in one big
    // `unsafe` block: the invariant is that the clipboard is open on this thread,
    // and it is easier to see that holds per call than to re-derive it per line.
    {
        // PNG before the DIB, deliberately. Measured on `image` 0.25.10: a 32-bpp
        // `CF_DIB` decodes with its alpha forced to 255 whether it is BI_RGB or
        // BI_BITFIELDS - the format does not promise alpha, and readers that
        // honour it are a courtesy, not a guarantee. So a copy from one pin and a
        // paste into another has to travel on the leg that is lossless.
        let mut image = None;
        if let Some(png) = registered("PNG").and_then(|f| unsafe { bytes_of(f) }) {
            image = Some(png);
        }
        if image.is_none() {
            let dib = unsafe { bytes_of(CF_DIBV5.0 as u32) }
                .or_else(|| unsafe { bytes_of(CF_DIB.0 as u32) });
            image = dib.as_deref().and_then(png_from_dib);
        }
        if image.is_none() {
            image = unsafe { bytes_of(CF_TIFF.0 as u32) };
        }
        out.image_bytes = image;
        if let Some(raw) = unsafe { bytes_of(CF_UNICODETEXT.0 as u32) } {
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            out.text = Some(utf16_to_string(&units));
        }
        if let Some(html) = registered("HTML Format").and_then(|f| unsafe { bytes_of(f) }) {
            out.html = Some(String::from_utf8_lossy(&html).into_owned());
        }
        if let Some(hdrop) = unsafe { handle_of(CF_HDROP.0 as u32) } {
            out.files = unsafe { files_in_hdrop(HDROP(hdrop.0)) };
        }
        let _ = unsafe { CloseClipboard() };
    }
    Ok(out)
}

/// `CF_HDROP` is a `DROPFILES` block followed by a double-NUL-separated list of
/// wide paths; `DragQueryFileW` reads both, given the handle.
///
/// # Safety: `drop` must be a live `HDROP` from the clipboard, which this thread
/// holds open.
unsafe fn files_in_hdrop(drop: HDROP) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let count = DragQueryFileW(drop, u32::MAX, None);
    let mut files = Vec::with_capacity(count as usize);
    for i in 0..count {
        let mut buf = vec![0u16; 1024];
        let n = DragQueryFileW(drop, i, Some(&mut buf));
        if n == 0 {
            continue;
        }
        buf.truncate(n as usize);
        files.push(PathBuf::from(std::ffi::OsString::from_wide(&buf)));
    }
    files
}

/// Puts a bitmap on the clipboard as `CF_DIB` and as a registered `PNG`.
///
/// Both, because readers split two ways and neither set is empty: the classic
/// ones only ever ask for a DIB, and the modern ones prefer PNG. The DIB is
/// 32-bpp with the alpha bytes present, which some readers keep and some render
/// opaque - the format does not forbid either behaviour - so the copy that has to
/// be lossless (our own paste-back, and anything that reads PNG) travels on the
/// PNG leg.
pub fn write_image(frame: &Frame) -> Result<(), ClipError> {
    let dib = dib_from_rgba(&frame.pixels, frame.width, frame.height);
    let png = encode::encode(
        frame,
        &EncodeOptions {
            format: Format::Png,
            ..Default::default()
        },
    )
    .map_err(|e| ClipError::Win(e.to_string()))?;
    let mut items: Vec<(u32, &[u8])> = vec![(CF_DIB.0 as u32, &dib)];
    if let Some(png_format) = registered("PNG") {
        items.push((png_format, &png));
    }
    write_formats(&items)
}

/// `CF_UNICODETEXT`, which is what the text card (§5.11) copies.
pub fn write_text(text: &str) -> Result<(), ClipError> {
    let mut wide = Vec::with_capacity(text.len() * 2 + 2);
    for u in text.encode_utf16() {
        wide.extend_from_slice(&u.to_le_bytes());
    }
    wide.extend_from_slice(&[0, 0]);
    write_formats(&[(CF_UNICODETEXT.0 as u32, &wide)])
}

/// One `OpenClipboard` / `EmptyClipboard` / `SetClipboardData` transaction.
///
/// Each block is allocated moveable because that is what the clipboard contract
/// requires, and ownership passes to the system on success. A failure part-way
/// through frees only what is still ours.
fn write_formats(items: &[(u32, &[u8])]) -> Result<(), ClipError> {
    open()?;
    // SAFETY: clipboard open on this thread; `SetClipboardData` is given a
    // freshly allocated block per format and nothing is used after the close.
    unsafe {
        if EmptyClipboard().is_err() {
            let _ = CloseClipboard();
            return Err(ClipError::Win("剪贴板内容无法清空".into()));
        }
        for (format, bytes) in items {
            let mem = match GlobalAlloc(GMEM_MOVEABLE, bytes.len()) {
                Ok(m) => m,
                Err(e) => {
                    let _ = CloseClipboard();
                    return Err(ClipError::Win(e.message().to_string()));
                }
            };
            let dst = GlobalLock(mem) as *mut u8;
            if dst.is_null() {
                let _ = GlobalFree(Some(mem));
                let _ = CloseClipboard();
                return Err(ClipError::Win("剪贴板内存块无法锁定".into()));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
            let _ = GlobalUnlock(mem);
            if SetClipboardData(*format, Some(HANDLE(mem.0))).is_err() {
                let _ = GlobalFree(Some(mem));
                let _ = CloseClipboard();
                return Err(ClipError::Win("写入剪贴板失败".into()));
            }
        }
        let _ = CloseClipboard();
    }
    Ok(())
}

/// The formats currently on the clipboard, by name. `--selftest` prints this so
/// "the pin copied itself" is checked against what a paste target will ask for,
/// on the machine doing the copying.
pub fn formats() -> Result<Vec<String>, ClipError> {
    let mut out = Vec::new();
    open()?;
    // SAFETY: enumeration reads the format table only; no handle is dereferenced.
    unsafe {
        let mut format = EnumClipboardFormats(0);
        while format != 0 && out.len() < 64 {
            let mut buf = [0u16; 128];
            let n = GetClipboardFormatNameW(format, &mut buf);
            if n > 0 {
                out.push(utf16_to_string(&buf[..n as usize]));
            } else {
                // A standard format has no name of its own; the number is the
                // `CF_*` value, which is what a reader asks for.
                out.push(format!("#{}", format));
            }
            format = EnumClipboardFormats(format);
        }
        let _ = CloseClipboard();
    }
    Ok(out)
}

/// Whether this process can transact with the clipboard at all, right now.
///
/// The UI asks before it promises a copy, and `--selftest` asks first so that a
/// refusal names the app holding the clipboard instead of reading as a bug in
/// this code.
pub fn usable() -> Result<(), ClipError> {
    open()?;
    // SAFETY: paired with the open above; nothing is read or written between them.
    unsafe {
        let _ = CloseClipboard();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_2x1() -> Frame {
        Frame {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 0, 255, 0, 128],
        }
    }

    #[test]
    fn a_dib_header_is_little_endian_and_bottom_up() {
        let dib = dib_from_rgba(&frame_2x1().pixels, 2, 1);
        assert_eq!(&dib[0..4], &40u32.to_le_bytes());
        assert_eq!(&dib[4..8], &2u32.to_le_bytes());
        assert_eq!(&dib[8..12], &1u32.to_le_bytes());
        assert_eq!(dib[12..16], [1, 0, 32, 0]);
        assert_eq!(&dib[16..20], &0u32.to_le_bytes());
        assert_eq!(&dib[20..24], &8u32.to_le_bytes());
        assert_eq!(dib.len(), 40 + 8);
        // BGRA, and the alpha that arrived is the alpha that leaves.
        assert_eq!(&dib[40..44], &[0, 0, 255, 255]);
        assert_eq!(&dib[44..48], &[0, 255, 0, 128]);
    }

    #[test]
    fn a_dib_written_here_reads_back_with_the_right_channels_and_rows() {
        let src = frame_2x1();
        let dib = dib_from_rgba(&src.pixels, src.width, src.height);
        assert_eq!(dib_size(&dib), Some((2, 1)));
        let png = png_from_dib(&dib).expect("the DIB this file writes must decode");
        let back = encode::decode(&png).expect("png decodes");
        assert_eq!(back.width, 2);
        assert_eq!(back.height, 1);
        // The swap this file does on the way out (RGBA -> BGRA) is undone by the
        // reader, so red is red again and the bottom-up flip lands on row 0.
        assert_eq!(&back.pixels[0..4], &[255, 0, 0, 255]);
        // Alpha is the one thing a DIB does not carry: measured on `image`
        // 0.25.10, both BI_RGB and BI_BITFIELDS 32-bpp decode opaque. That is why
        // `read` takes PNG first and why `write_image` puts both on the clipboard.
        assert_eq!(&back.pixels[4..8], &[0, 255, 0, 255]);
    }

    #[test]
    fn png_is_the_leg_that_survives_a_copy_and_a_paste() {
        let src = frame_2x1();
        let png = encode::encode(
            &src,
            &EncodeOptions {
                format: Format::Png,
                ..Default::default()
            },
        )
        .expect("png");
        assert_eq!(encode::decode(&png).expect("decodes"), src);
    }

    #[test]
    fn a_top_down_dib_from_another_app_still_decodes() {
        // `-1` height is the top-down flag. `image` reads it; the synthesised BMP
        // header has to stay honest about where the pixels begin, which is the
        // only thing this function actually changes about an incoming DIB.
        let mut dib = dib_from_rgba(&frame_2x1().pixels, 2, 1);
        dib[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        assert_eq!(dib_size(&dib), Some((2, -1)));
        assert!(png_from_dib(&dib).is_some());
    }

    #[test]
    fn a_truncated_or_absurd_header_is_refused_not_paniced() {
        assert_eq!(dib_size(&[0u8; 8]), None);
        assert_eq!(dib_size(&[40, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]), None);
        assert!(png_from_dib(&[]).is_none());
        assert!(png_from_dib(&[0xff; 40]).is_none());
    }
}
