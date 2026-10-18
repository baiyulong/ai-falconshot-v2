//! PRD §5.4.3 - the system pointer as a small frame that can be stamped on a picture.
//!
//! Two legs, because they need different witnesses. [`compose`] is arithmetic over bytes:
//! it turns the buffer [`read`] painted into a tight RGBA frame, and it is graded with no
//! desktop at all. [`read`] is the Win32 leg - `GetCursorInfo` asks *which* pointer,
//! `GetIconInfo` asks its size and hotspot, `DrawIconEx` paints it into a 32-bit DIB this
//! module owns, and nothing here reads the screen.
//!
//! `Frame` promises **straight** alpha and [`falcon_core::frame::blend_rgba`] blends it as
//! straight, while GDI hands back **pre-multiplied** ARGB for the 32-bit cursors modern
//! Windows ships. Un-premultiplying inside [`compose`] is what stops a soft-edged arrow
//! from arriving as a dark smear; that rule is one of the arms of its test.
//!
//! A cursor is not an icon: it has a hotspot, so the point the user is aiming with sits
//! *inside* the bitmap rather than at its corner. [`read`] therefore hands back the screen
//! position of the picture's **top-left** (`pos - hotspot`) - the only coordinate the
//! stamping arithmetic can use, and the reason a rectangle planted around the pointer's
//! own position still lands where the arrow actually is.

use falcon_core::frame::Frame;
use falcon_core::geometry::PhysPoint;

/// The four bytes a pixel holds when `DrawIconEx` never touched it.
///
/// A 32-bit DIB section comes back zero-filled, and zero is a perfectly legal cursor
/// pixel (transparent black is what a hole in the arrow *is*). So "untouched" has to be
/// written into the buffer before the draw, and it is also handed to GDI as palette
/// entry 0 - the colour a monochrome cursor paints for its background - so both the
/// 1-bit and the 32-bit paths leave exactly this value where there is no pointer.
pub const SENTINEL: &[u8] = &[0x11, 0x22, 0x33, 0x00];

/// A pointer, where its picture's top-left sits on the screen, and whether the desktop
/// was actually showing it.
pub struct Readout {
    pub frame: Frame,
    pub at: PhysPoint,
    pub showing: bool,
}

/// The BGRA buffer GDI painted, as the tight straight-alpha RGBA frame the rest of the
/// project agrees on. `None` if the buffer cannot describe that many pixels.
///
/// Three rules, one per way a cursor arrives as the wrong picture:
/// - bytes still equal to [`SENTINEL`] are *nothing*, so they leave as fully transparent
///   rather than as a coloured square with a hole punched in the arrow;
/// - a painted pixel whose alpha byte is still zero is **opaque ink** - that is the
///   monochrome-cursor path, and reading it as transparent loses the arrow entirely;
/// - anything else is pre-multiplied and is divided back out.
pub fn compose(buf: &[u8], width: u32, height: u32) -> Option<Frame> {
    let n = (width as usize).checked_mul(height as usize)?;
    let need = n.checked_mul(4)?;
    if n == 0 || buf.len() < need {
        return None;
    }
    let mut out = vec![0u8; need];
    let srcs = buf[..need].as_chunks::<4>().0;
    let dsts = out.as_chunks_mut::<4>().0;
    for (src, dst) in srcs.iter().zip(dsts.iter_mut()) {
        if src.as_slice() == SENTINEL {
            continue;
        }
        let a = src[3];
        if a == 0 || a == 255 {
            // Either way there is nothing to divide out: 0 is the monochrome ink,
            // 255 is already opaque.
            dst.copy_from_slice(&[src[2], src[1], src[0], 255]);
        } else {
            let un = |c: u8| (((c as u32 * 255 + a as u32 / 2) / a as u32).min(255)) as u8;
            dst.copy_from_slice(&[un(src[2]), un(src[1]), un(src[0]), a]);
        }
    }
    Frame::from_rgba(width, height, out).ok()
}

/// The pointer as the desktop holds it right now, anchored at `pos`.
#[cfg(windows)]
pub fn read(pos: PhysPoint) -> Result<Readout, String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorInfo, CURSORINFO, CURSOR_SHOWING, HICON,
    };

    // SAFETY: `ci` is a value this call owns, its `cbSize` is the size of its own type
    // (which is how the API recognises the struct), and the returned handle names the
    // pointer the desktop is holding rather than anything this module allocated.
    let (frame, hx, hy, showing) = unsafe {
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        GetCursorInfo(&mut ci).map_err(|e| format!("GetCursorInfo: {e}"))?;
        // P119 #323：这两件事先前共用那一句「GetIconInfo/DrawIconEx 没画出一张图」。把手为空是
        // **桌面此刻没有可画的指针**（`GetCursorInfo` 交不出指针时把 `hCursor` 置空），那是机器
        // 状态而不是本模块的缺陷；把手非零而画不出图才是缺陷。分成两句，读到它的人才分得清该修
        // 哪一个 —— 而梯的 pw 格正是靠这一分才能把前者记成跳过、把后者留作失败。
        if ci.hCursor.0.is_null() {
            return Err("hCursor=NULL：桌面此刻没有可画的指针".to_string());
        }
        let (frame, hx, hy) = render(HICON(ci.hCursor.0))
            .ok_or("hCursor 非零而 GetIconInfo/DrawIconEx 没画出一张图")?;
        (frame, hx, hy, ci.flags.0 & CURSOR_SHOWING.0 != 0)
    };
    Ok(Readout {
        frame,
        at: PhysPoint::new(pos.x - hx, pos.y - hy),
        showing,
    })
}

/// The same question on a machine without Win32. The MVP is Windows-only, and this is the
/// answer the trait's `None` side is built from - never a guess about a pointer.
#[cfg(not(windows))]
pub fn read(_: PhysPoint) -> Result<Readout, String> {
    Err("this build has no Win32 cursor".to_string())
}

/// `icon` painted at its own size, with its hotspot.
///
/// The size comes from the cursor's own bitmaps rather than from `GetSystemMetrics`:
/// a monochrome cursor ships a single mask bitmap twice as tall as it is wide-ish (the
/// AND mask stacked on the XOR mask) and no colour bitmap at all, and that is exactly
/// the shape that used to make "the arrow is 32x64 of nothing" a plausible bug.
#[cfg(windows)]
fn render(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<(Frame, i32, i32)> {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP};
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    // SAFETY: `GetObjectW` is handed a bitmap handle that `GetIconInfo` just created for
    // this caller, and the out-parameter's size is the size of its own struct.
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(icon, &mut ii).ok()?;
        let colored = !ii.hbmColor.is_invalid();
        let probe = if colored { ii.hbmColor } else { ii.hbmMask };
        let mut bm = BITMAP::default();
        let got = GetObjectW(
            probe.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut c_void),
        );
        // Half the height for a monochrome cursor: that one bitmap is two masks stacked,
        // the AND on top of the XOR, and asking for the whole thing draws twice over.
        let h = if got as usize == std::mem::size_of::<BITMAP>() {
            if colored {
                bm.bmHeight
            } else {
                bm.bmHeight / 2
            }
        } else {
            0
        };
        let frame = if bm.bmWidth > 0 && h > 0 {
            paint(icon, bm.bmWidth, h)
        } else {
            None
        };
        // The two bitmaps `GetIconInfo` made belong to this caller even when the icon
        // handle itself is shared; leaking them is how a repeated capture eats memory.
        let _ = DeleteObject(ii.hbmMask.into());
        if colored {
            let _ = DeleteObject(ii.hbmColor.into());
        }
        let frame = frame?;
        Some((frame, ii.xHotspot as i32, ii.yHotspot as i32))
    }
}

/// One `DrawIconEx` into a 32-bit DIB section, painted over [`SENTINEL`] and composed.
#[cfg(windows)]
fn paint(icon: windows::Win32::UI::WindowsAndMessaging::HICON, w: i32, h: i32) -> Option<Frame> {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, RGBQUAD,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DrawIconEx, DI_NORMAL};

    let (w, h) = (w as u32, h as u32);
    let bytes = (w as usize) * (h as usize) * 4;

    // SAFETY: every object created here is released on every path out of the block; the
    // buffer `CreateDIBSection` hands back is exactly `bytes` long because the header
    // says 32 bits per pixel and a negative height (top-down rows, so byte 0 is the
    // top-left pixel - the layout `Frame` promises).
    unsafe {
        let mem = CreateCompatibleDC(None);
        if mem.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD {
                rgbBlue: SENTINEL[0],
                rgbGreen: SENTINEL[1],
                rgbRed: SENTINEL[2],
                rgbReserved: SENTINEL[3],
            }],
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            return None;
        };
        if bits.is_null() {
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            return None;
        }
        let buf = std::slice::from_raw_parts_mut(bits as *mut u8, bytes);
        for chunk in buf.as_chunks_mut::<4>().0 {
            chunk.copy_from_slice(SENTINEL);
        }
        let old = SelectObject(mem, bmp.into());
        let drawn = DrawIconEx(mem, 0, 0, icon, w as i32, h as i32, 0, None, DI_NORMAL);
        let answer = if drawn.is_ok() {
            compose(buf, w, h)
        } else {
            None
        };
        let _ = SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgra(b: u8, g: u8, r: u8, a: u8) -> [u8; 4] {
        [b, g, r, a]
    }

    /// [`compose`] 的四条规则各是一种错法：把没画过的地方当成不透明、把单色光标的墨当成
    /// 透明、把预乘当成直接 alpha、把 BGRA 与 RGBA 的通道序写反。
    #[test]
    fn the_sentinel_marks_what_the_pointer_never_covered() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(SENTINEL);
        buf.extend_from_slice(&bgra(0, 0, 0, 0));
        buf.extend_from_slice(&bgra(64, 64, 64, 128));
        buf.extend_from_slice(&bgra(1, 2, 3, 255));

        let f = compose(&buf, 2, 2).expect("四枚像素拼不出 2x2");
        assert_eq!(
            f.get(0, 0),
            [0, 0, 0, 0],
            "没画过的地方留下了一枚不透明的格子"
        );
        assert_eq!(
            f.get(1, 0),
            [0, 0, 0, 255],
            "alpha 字节还是 0 的墨被当成了透明"
        );
        assert_eq!(
            f.get(0, 1),
            [128, 128, 128, 128],
            "预乘的通道没有还原成直接 alpha"
        );
        assert_eq!(f.get(1, 1), [3, 2, 1, 255], "BGRA 到 RGBA 的通道序写反了");

        assert!(
            compose(&buf[..12], 2, 2).is_none(),
            "缓冲区差一格还硬拼成一帧"
        );
        assert!(compose(&[], 0, 0).is_none(), "零尺寸交回了一张图");
        assert!(
            compose(&buf, u32::MAX, u32::MAX).is_none(),
            "把尺寸换算成字节数时溢出（本机首跑正是在这里 panic 的）"
        );
    }

    /// 同一枚 DIB 腿,但**不需要指针也不需要桌面**:shell 自己那枚共享图标必须画得出图。
    /// 这一格红的读法是「这台机器上一条 GDI 像素都没画出来过」,而不是「光标不在」。
    #[cfg(windows)]
    #[test]
    fn a_shared_system_icon_paints_through_the_same_leg_as_the_pointer() {
        use windows::Win32::UI::WindowsAndMessaging::{
            LoadImageW, HICON, IDI_APPLICATION, IMAGE_ICON, LR_SHARED,
        };

        // SAFETY: `LR_SHARED` means the handle belongs to the shell and must not be
        // destroyed; the size arguments are zero because this asks for the system's own.
        let got = unsafe { LoadImageW(None, IDI_APPLICATION, IMAGE_ICON, 0, 0, LR_SHARED) }
            .expect("LoadImageW(IDI_APPLICATION)");
        let icon = HICON(got.0);
        let (frame, hx, hy) = render(icon).expect("shell 图标画不出图,那枚 DIB 腿就是空的");
        assert!(frame.width > 0 && frame.width <= 256, "{}", frame.width);
        assert!(frame.height > 0 && frame.height <= 256, "{}", frame.height);
        assert_eq!(
            frame.pixels.len(),
            frame.width as usize * frame.height as usize * 4
        );
        assert!(hx >= 0 && (hx as u32) < frame.width, "热点落在图之外");
        assert!(hy >= 0 && (hy as u32) < frame.height, "热点落在图之外");
        let opaque = frame
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] == 255)
            .count();
        assert!(opaque > 0, "DrawIconEx 一个字没画,而这一格照样交回了 Some");
    }

    /// 真指针那一臂:`read` 与产品走的 [`crate::win::WinExtras::cursor_image`] 必须同步。
    /// 断言写成双向,是因为桌面此刻亮不亮着指针不是这台机器能由测试决定的事——而**两者不一致**
    /// 才是缺陷（藏着的指针被画了,或者亮着的指针没被画）。
    ///
    /// 同一句 doc 先前还只是一句声明,而测试本体在 `expect` 上把它推翻了:P119 #323 起,桌面交不出
    /// 指针（`hCursor=NULL`）记成跳过,别的答不上仍旧算失败。
    #[cfg(windows)]
    #[test]
    fn the_pointer_leg_answers_for_a_point_it_was_handed() {
        use crate::win::WinExtras;
        use falcon_core::capture::WindowExtras;

        // 640x480 是**交进去的锚点**而不是读来的位置:`read` 自己问 `GetCursorInfo` 要哪一只
        // 指针,而位置用它手里那枚热点换算,所以这一格不需要碰用户的鼠标。
        let pos = PhysPoint::new(640, 480);
        let r = match read(pos) {
            Ok(r) => r,
            // 实证在 `p60_lad_p119d_pw.log`:17:3x 那一趟读回 NULL 把手、pw 格判红一格,同一枚
            // 测试 17:39 单独再跑就绿(`p119d_pw_single_rerun_b.txt` 逐字 `1 passed; 29 filtered
            // out`)。跳过只在 NULL 这一种形状上发生。
            Err(e) if e.starts_with("hCursor=NULL") => {
                println!("指针那一格跳过(机器状态,不是缺陷):{e}");
                return;
            }
            Err(e) => panic!("指针那一格答不上:{e}"),
        };
        let (hx, hy) = (pos.x - r.at.x, pos.y - r.at.y);
        assert!(
            hx >= 0 && (hx as u32) < r.frame.width,
            "交回的左上角不等于位置减热点"
        );
        assert!(
            hy >= 0 && (hy as u32) < r.frame.height,
            "交回的左上角不等于位置减热点"
        );
        let via_trait = WinExtras.cursor_image(pos);
        assert_eq!(
            via_trait.is_some(),
            r.showing,
            "藏着的指针被画了,或者亮着的指针没被画"
        );
        if let Some((frame, at)) = via_trait {
            assert_eq!(at, r.at, "同一枚指针经两扇门交回了两个位置");
            assert_eq!(
                frame.width, r.frame.width,
                "同一枚指针经两扇门交回了两种尺子"
            );
        }
    }
}
