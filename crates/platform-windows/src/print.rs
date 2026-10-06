//! `PrintWindow` - the leg that asks a window to draw itself instead of reading
//! the desktop it happens to sit on.
//!
//! R13's matrix needs four independent ways to obtain pixels. GDI reads the
//! composited screen, WGC reads the desktop's duplicated frames, Qt reads the
//! screen through whichever backend Qt chose, and this one bypasses the screen
//! entirely by sending the window a paint request into a bitmap we own. A window
//! that paints straight to a swapchain, or that refuses to paint for a foreign
//! device context, comes back flat here while the desktop-reading legs see it
//! normally - and a window covered by another comes back *complete* here while
//! the desktop legs show the covering window. Both disagreements are the product
//! of this leg existing, which is why it is measured rather than assumed.

use falcon_core::frame::Frame;

/// `width`x`height` of `hwnd` as the window itself would paint it, with exactly
/// one `PrintWindow` flag set.
///
/// The flag is a parameter because that is the whole point of this leg: xcap's
/// window path (which the product otherwise uses) tries `2`, then `0`, then `4`
/// and finally falls back to a `BitBlt` of the window's rectangle. A call
/// through it can therefore succeed by *reading the desktop behind the window*
/// while being reported as a window capture, so it cannot answer "can this kind
/// of window be asked to paint itself". This function asks once, and answers
/// either with what came back or with nothing.
///
/// `None` is a result, not an error - it is this leg's answer to "can
/// `PrintWindow` produce pixels for this window at this size".
///
/// The `BOOL` return of `PrintWindow` is deliberately *not* the verdict: some
/// windows answer FALSE having painted correctly and others answer TRUE having
/// painted nothing, so the only claim this function can honour is whether a
/// bitmap actually arrived. A flat black frame is reported as `Some`, because a
/// black frame is what the leg saw.
#[cfg(windows)]
pub fn print_window(hwnd: u64, width: u32, height: u32, flags: u32) -> Option<Frame> {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetWindowDC,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};

    if width == 0 || height == 0 {
        return None;
    }
    let (w, h) = (width as i32, height as i32);
    let handle = crate::proc::to_hwnd(hwnd);

    // SAFETY: every handle below is created or fetched in this block and
    // released on every path out of it; the pixel buffer and the `BITMAPINFO`
    // that describes it are passed to `GetDIBits` together.
    unsafe {
        let screen = GetWindowDC(Some(handle));
        if screen.is_invalid() {
            return None;
        }
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            let _ = ReleaseDC(Some(handle), screen);
            return None;
        }
        let bmp = CreateCompatibleBitmap(screen, w, h);
        if bmp.is_invalid() {
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(Some(handle), screen);
            return None;
        }
        let old = SelectObject(mem, bmp.into());
        // `PW_RENDERFULLCONTENT` is what lets DirectComposition-backed windows
        // (every Qt Quick window among them) answer at all; without the flag an
        // otherwise perfectly visible window reads back flat - which is exactly
        // the disagreement the caller is here to measure, so the flag is passed
        // through rather than hard-wired.
        //
        // The returned `BOOL` is dropped on purpose: the honest verdict is
        // whether `GetDIBits` below produced lines.
        let _ = PrintWindow(handle, mem, PRINT_WINDOW_FLAGS(flags));
        // Deselect before reading. A bitmap still selected into a DC is
        // documented as undefined input for `GetDIBits`, and the observed
        // symptom is exactly the failure this leg must not confuse with "the
        // window is invisible": an all-zero read.
        let _ = SelectObject(mem, old);

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative height means top-down rows, so buffer byte 0 is the
                // top-left pixel - which is the layout `Frame` promises.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; width as usize * height as usize * 4];
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            height,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(Some(handle), screen);

        if lines == 0 {
            return None;
        }
        // A 32-bit DIB carries an alpha channel GDI does not fill, and
        // `PrintWindow` leaves it at zero - which would make the result
        // correctly captured and completely transparent. This leg answers "what
        // colour is the window", so the window is opaque.
        for px in buf.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            px[3] = 255;
        }
        Frame::from_rgba(width, height, buf).ok()
    }
}

#[cfg(not(windows))]
pub fn print_window(_: u64, _: u32, _: u32, _: u32) -> Option<Frame> {
    None
}
