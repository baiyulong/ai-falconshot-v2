//! The Windows side of [`falcon_core::capture::WindowExtras`].
//!
//! `xcap` enumerates windows and grabs pixels, but it does not know the DWM's idea
//! of a window's visible rectangle, cannot name the process behind a handle on
//! every path, and has no UI Automation client. Those four gaps are this trait, and
//! this module is where the two that decide *what a user sees* are filled in.

use falcon_core::capture::WindowExtras;
use falcon_core::geometry::{PhysPoint, PhysRect};

pub struct WinExtras;

/// PRD §5.2.6 规则第一句里「圆角」那一格：系统给这个窗口选的圆角档位。
///
/// 这四个数就是 `DWMWA_WINDOW_CORNER_PREFERENCE` 交回来的整数（`DWMWCP_*`）。这里把它们收
/// 成自己的枚举，是为了让下面那把换算的尺子能在**没有桌面、没有 Win32** 的地方被测：单测
/// 递一个整数进来，不用去开一扇真的窗。
pub const CORNER_DEFAULT: i32 = 0;
pub const CORNER_DO_NOT_ROUND: i32 = 1;
pub const CORNER_ROUND: i32 = 2;
pub const CORNER_ROUND_SMALL: i32 = 3;

/// 标准档在 100% 缩放下的半径（**逻辑**像素）。
///
/// 说清楚来源：这两个数是 Windows 11 上普通窗口与最小化按钮那一类小窗的实践观察值，不是
/// 任何头文件里的常数 —— 系统没有把它做成可读的指标。所以这一格是**可改的**：真机上量出
/// 别的数，改这里，`corner_radius_physical` 的那些断言会跟着走。
pub const CORNER_ROUND_DIP: u32 = 8;
/// 小圆角档（`DWMWCP_ROUNDSMALL`）在同一缩放下的半径。
pub const CORNER_SMALL_DIP: u32 = 4;

/// 档位 → 逻辑半径。`DWMWCP_DEFAULT` 归到标准档，理由写在函数体旁边而不是文档里：
/// 它说的是「由系统决定」，而 Win11 对普通顶层窗口决定的就是标准圆角 —— 把它读成「不圆」
/// 会把绝大多数窗口当成方的，那一格就永远不生效了。
pub fn corner_radius_dip(preference: i32) -> u32 {
    match preference {
        CORNER_DO_NOT_ROUND => 0,
        CORNER_ROUND_SMALL => CORNER_SMALL_DIP,
        CORNER_ROUND | CORNER_DEFAULT => CORNER_ROUND_DIP,
        // 这一族只到 3；认不出的值不当成任何一档 —— 宁可不切，也不照着猜出来的半径切一刀。
        _ => 0,
    }
}

/// 逻辑半径 → 物理像素半径：按**这扇窗所在显示器**的 DPI 换算（96 = 100%）。
///
/// 向上取半（`+48` 再整除）而不是向下：圆角宁可多切一像素，也不要留下一个没切到的方角尖。
pub fn corner_radius_physical(dip: u32, dpi: u32) -> u32 {
    if dip == 0 {
        return 0;
    }
    let dpi = if dpi == 0 { 96 } else { dpi };
    (dip * dpi + 48) / 96
}

#[cfg(windows)]
impl WindowExtras for WinExtras {
    /// `DWMWA_EXTENDED_FRAME_BOUNDS`, in physical pixels.
    ///
    /// `GetWindowRect` - what `xcap` reports - includes the invisible resize border
    /// the DWM draws outside the content, so a window shot cropped from it carries a
    /// frame of shadow and desktop that the user never saw as part of the window.
    fn dwm_bounds(&self, hwnd: u32) -> Option<PhysRect> {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};

        let mut rect = RECT::default();
        // SAFETY: the out-parameter and its size are passed together, and the
        // handle names a window from this same enumeration.
        let ok = unsafe {
            DwmGetWindowAttribute(
                crate::proc::to_hwnd(u64::from(hwnd)),
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut RECT as *mut core::ffi::c_void,
                std::mem::size_of::<RECT>() as u32,
            )
        };
        if ok.is_err() {
            return None;
        }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(PhysRect::new(rect.left, rect.top, w as u32, h as u32))
    }

    /// PRD §5.3.4 - the wheel cycling through the element chain under the cursor.
    ///
    /// Not answered yet, and deliberately not guessed at: a wrong rect here would
    /// silently select the wrong region, which is worse than no element snapping.
    /// The UIA client (`IUIAutomation::ElementFromPoint`, then walking parents) is
    /// the work item that fills this in.
    fn element_chain(&self, _: PhysPoint) -> Vec<PhysRect> {
        Vec::new()
    }

    /// PRD §5.4.3 - the system cursor as a small frame, for "capture with cursor".
    ///
    /// The leg is [`crate::cursor`]: `GetCursorInfo` for which pointer, `GetIconInfo` for
    /// its size and hotspot, one `DrawIconEx` into a 32-bit DIB this process owns. It
    /// reads no screen pixels, so unlike the freeze it costs nothing and cannot catch the
    /// mask window in the picture.
    ///
    /// `None` here is **two** states and the caller cannot tell them apart: the desktop
    /// was not showing a pointer (`CURSOR_SHOWING` clear - a hidden pointer is not
    /// something the user saw, so it is not something the picture may claim), or the GDI
    /// leg could not produce a bitmap. Both are reported as "no overlay to stamp", which
    /// is why the door still has no witness for the key-on side.
    fn cursor_image(&self, at: PhysPoint) -> Option<(falcon_core::frame::Frame, PhysPoint)> {
        let r = crate::cursor::read(at).ok()?;
        r.showing.then_some((r.frame, r.at))
    }

    /// `(executable name, full path)` of the process owning `hwnd`.
    fn process_path(&self, hwnd: u32) -> Option<(String, String)> {
        crate::proc::process_paths(crate::proc::pid_of_window(u64::from(hwnd))?)
    }

    /// `GetWindowRect`, physical pixels - the window's **real** frame, wider than
    /// [`Self::dwm_bounds`] by exactly the invisible band the DWM draws its shadow in.
    ///
    /// Pure adapter: [`crate::proc::window_rect`] already asks, this only names the
    /// handle. §5.2.6's shadow side reads it to learn how much room the real shadow
    /// takes; the decision of what to do with that number is
    /// [`falcon_core::capture::window_shadow_margin`], which is unit-tested.
    ///
    /// **「physical」是有条件的**，而这一句是 `--ignored` 那一格在这台 200% 的机器上量出来的：
    /// `GetWindowRect` 交回的是**读它那个进程**的坐标系。本程序的 Qt 侧按 per-monitor v2 起，
    /// 于是它问到的就是物理像素、带宽有意义；而一个没声明 DPI 认知的进程（`cargo test` 的那个
    /// exe 就是这样，P… 那一跑印出 `visible=3072x1920 real=1536x960`）问到的是被系统虚拟过的
    /// 数 —— 那条「带」在这里成了**负数**。它不会画坏任何东西：[`falcon_core::capture::window_shadow_margin`]
    /// 把负数读成 0，于是边距退到 `[output] shadow_size` 或那枚常数，也就是「按窗口自己的阴影留」这一
    /// 读法在这一趟**退化成按配置留**。真机上要盯的就是这一格：阴影圈的宽度对不对得上这台机器
    /// 上窗口自己那条带，只有眼睛能确认。
    fn window_rect(&self, hwnd: u32) -> Option<PhysRect> {
        crate::proc::window_rect(u64::from(hwnd))
    }

    /// The radius this window's **own** corners are rounded at, in physical pixels.
    ///
    /// Two Win32 reads and one pure calculation: `DWMWA_WINDOW_CORNER_PREFERENCE` for
    /// which band, `GetDpiForWindow` for whose scale, [`corner_radius_physical`] to
    /// combine them. `None` = no answer, which §5.2.6 reads as "system not asked":
    /// a Windows 10 build rejects that attribute outright, and a shot taken there must
    /// not get a cut invented from a Win11 metric.
    fn window_corner_radius(&self, hwnd: u32) -> Option<u32> {
        use windows::Win32::Graphics::Dwm::{
            DwmGetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE,
        };

        let h = crate::proc::to_hwnd(u64::from(hwnd));
        let mut pref: i32 = 0;
        // SAFETY: the out-parameter and its size are passed together, and the handle
        // names a window from this same enumeration.
        let ok = unsafe {
            DwmGetWindowAttribute(
                h,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &mut pref as *mut i32 as *mut core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            )
        };
        if ok.is_err() {
            return None;
        }
        // SAFETY: a window handle read for its own DPI; 0 only means "fall back to 100%"
        // inside the pure helper, never "square".
        let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(h) };
        Some(corner_radius_physical(corner_radius_dip(pref), dpi))
    }

    /// PRD §5.3.2 规则第一句里「完全透明窗口」那一问的真答案：`GetLayeredWindowAttributes`。
    ///
    /// 这一手自本轮起才有牙齿。在那之前 [`WindowExtras::window_opacity`] 走的是 trait 上那个
    /// 默认实现（恒 `None` = 平台没说），于是
    /// [`falcon_core::capture::window_is_shootable`] 的 `opacity != Some(0)` 那一格在**生产路径
    /// 上永远不会命中**：整条排除规则只在递假把手的单测里成立。core 那几条断言钉的是判定表，
    /// 钉不到「这一问有没有人答」。
    ///
    /// 一次问数：不开窗、不读像素、不占 GDI 对象，所以
    /// [`falcon_core::capture::CaptureService::windows`] 每次枚举、每扇窗问一次也不贵（遮罩那一趟
    /// 用的是缓存候选表，见 `crates/qt_bridge/src/capture.rs` 的 `window_candidates`，不会每帧
    /// 再问一遍桌面）。
    ///
    /// 三格读数怎么收成那一枚 `Option<u8>` **不在这里第二次算** —— 判据是
    /// [`falcon_core::capture::layered_window_opacity`]，那张表在没有窗口的地方被测。这一手只
    /// 负责把 Win32 交回的三格原样递过去：那一问成功没有、`LWA_ALPHA` 置着没有、alpha 是多少。
    ///
    /// 四种窗各落在哪一格：普通窗（没有 `WS_EX_LAYERED`）与 `UpdateLayeredWindow` 逐像素那一类
    /// 都失败 ⇒ `None` ⇒ 放行；只有 `LWA_COLORKEY` 的成功 ⇒ `None`（整体 alpha 从来没被设过，
    /// 交回的数是没意义的）⇒ 放行；`LWA_ALPHA` 且 alpha = 0 ⇒ `Some(0)` ⇒ **排除**，那正是用户
    /// 说「它压根看不见」的那一扇。
    fn window_opacity(&self, hwnd: u32) -> Option<u8> {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetLayeredWindowAttributes, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA,
        };

        let mut key = windows::Win32::Foundation::COLORREF(0);
        let mut alpha: u8 = 0;
        let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
        // SAFETY: 三个出参都是本函数自己拥有的活内存，把手来自同一次窗口枚举。返回的是
        // `Result<()>`（`BOOL` 为假 ⇒ `Err`），按 `is_ok()` 读；「返回值不是成败」那一族陷阱写在
        // `CoInitializeEx` 上，与这一手无关。
        let answered = unsafe {
            GetLayeredWindowAttributes(
                crate::proc::to_hwnd(u64::from(hwnd)),
                Some(&mut key),
                Some(&mut alpha),
                Some(&mut flags),
            )
        }
        .is_ok();
        let uses_alpha = flags.contains(LWA_ALPHA);
        // `key` 只在「只有 color key」那一格有意义，而那一格判成「平台没说」，所以它不进判定。
        let _ = key;
        falcon_core::capture::layered_window_opacity(answered, uses_alpha, alpha)
    }
}

#[cfg(not(windows))]
impl WindowExtras for WinExtras {
    fn dwm_bounds(&self, _: u32) -> Option<PhysRect> {
        None
    }

    fn element_chain(&self, _: PhysPoint) -> Vec<PhysRect> {
        Vec::new()
    }

    fn cursor_image(&self, _: PhysPoint) -> Option<(falcon_core::frame::Frame, PhysPoint)> {
        None
    }

    fn process_path(&self, _: u32) -> Option<(String, String)> {
        None
    }

    /// No `GetWindowRect` to ask, so no measured band - and §5.2.6's shadow side then
    /// falls back to the configured margin, which is a decision core already tests.
    fn window_rect(&self, _: u32) -> Option<PhysRect> {
        None
    }

    /// No system rounding to read: "asked, and the answer is there is no answer".
    fn window_corner_radius(&self, _: u32) -> Option<u32> {
        None
    }

    /// 这一侧没有 `GetLayeredWindowAttributes` 可问 ⇒ 恒 `None` = 「平台没说这扇窗多透」，
    /// 于是 §5.3.2 的排除在这一侧**不生效**，这是答案而不是失败：把「问不出」写成「全透」会
    /// 把整张候选表清空。判定表本身在有窗口的地方也测得到，见
    /// [`falcon_core::capture::layered_window_opacity`] 的那几条断言。
    fn window_opacity(&self, _: u32) -> Option<u8> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5.2.6「按设置决定…圆角」中「系统窗口原始圆角」那半边的档位表：四档各有答案，
    /// 认不出的整数不当成任何一档。
    #[test]
    fn the_four_bands_each_answer_a_radius() {
        assert_eq!(
            corner_radius_dip(CORNER_DO_NOT_ROUND),
            0,
            "方角档切出了圆角"
        );
        assert_eq!(corner_radius_dip(CORNER_ROUND), CORNER_ROUND_DIP);
        assert_eq!(corner_radius_dip(CORNER_ROUND_SMALL), CORNER_SMALL_DIP);
        // DEFAULT 归到标准档：把它读成 0，绝大多数窗口就永远不会被切。
        assert_eq!(corner_radius_dip(CORNER_DEFAULT), CORNER_ROUND_DIP);
        // 越界的值：不猜。
        assert_eq!(corner_radius_dip(9), 0);
        assert_eq!(corner_radius_dip(-1), 0);
    }

    /// 半径是**物理**像素：同一档在 200% 屏上应当是 100% 屏上的两倍。这一步错了的话，
    /// 高 DPI 屏上的圆角会只切到一半，肉眼可见（真机上要盯的就是这一格）。
    #[test]
    fn the_radius_scales_with_the_windows_own_dpi() {
        assert_eq!(corner_radius_physical(CORNER_ROUND_DIP, 96), 8);
        assert_eq!(corner_radius_physical(CORNER_ROUND_DIP, 192), 16);
        assert_eq!(corner_radius_physical(CORNER_ROUND_DIP, 144), 12);
        // 小圆角档在 150% 上是 6；向上取半：125%（120）给 8*120/96 = 10 整。
        assert_eq!(corner_radius_physical(CORNER_SMALL_DIP, 120), 5);
        // 方角档不因缩放变圆，0 DPI 不炸。
        assert_eq!(corner_radius_physical(0, 192), 0);
        assert_eq!(corner_radius_physical(CORNER_ROUND_DIP, 0), 8);
    }

    /// §5.2.6 那一腿在**真人机上**的只读证人：这台机器的 DWM 此刻给前台窗口留了多宽的阴影带、
    /// 又把它自己的角切成了多大。core 那六条断言钉的是判定表与画法，钉不到「`CORNER_ROUND_DIP`
    /// = 8 这一枚观察值在这台机器上到底对不对」—— 那一格只有量一次才知道。
    ///
    /// 一个像素也不读、一个窗口也不开：两次 `DwmGetWindowAttribute` + 一次 `GetWindowRect` +
    /// 一次 `GetDpiForWindow`，全是问数。挂着 `#[ignore]`，同 [`crate::autostart`] 那一格规矩
    /// —— 它要前台真有一扇窗，而 CI/无人那一趟不该把「没有窗口」量成红。
    ///
    /// [`cargo test -p platform-windows -- --ignored`] 自己点名跑它，读它印出来的那一行。
    #[cfg(windows)]
    #[test]
    #[ignore = "读这台机器真实前台窗口的 DWM 读数（只问数，但要真人点头）"]
    fn the_real_window_answers_band_and_corner() {
        use falcon_core::capture::window_shadow_margin;

        let hwnd = u32::try_from(crate::proc::foreground_window().unwrap_or_default())
            .expect("前台没有窗口：这一趟连把手是谁都问不出来");
        let e = WinExtras;
        let visible = e.dwm_bounds(hwnd).expect("DWM 不答可见边");
        let real = e.window_rect(hwnd).expect("GetWindowRect 不答");
        let corner = e.window_corner_radius(hwnd);
        // 阴影圈的宽度由 core 那枚判定说，这里只把它此刻会得到什么念出来。
        let margin = window_shadow_margin(&visible, Some(&real), 0);
        println!(
            "[window witness] hwnd={hwnd:#x} visible=({},{}) {}x{} real=({},{}) {}x{} \
             margin={margin} corner={corner:?}（8 DIP 档在本机的物理读数应为 8*DPI/96）",
            visible.x, visible.y, visible.w, visible.h, real.x, real.y, real.w, real.h,
        );
        // 只有一格是断言：开着阴影**必须**让框变大。这条一旦在这台机器上不成立，
        // §5.2.6 的「按设置决定」就是一句空话，而这一格会先红。
        assert!(
            margin > 0,
            "阴影边距算出 0：开关两边会得到同一张尺子，那一格就没有读者了"
        );
        // 观察值的证人：系统答得出时，那枚数必须是**四档之一在这扇窗的 DPI 上**换算得到的。
        // 答出别的数，就是档位表或 DPI 那一读错了 —— 这一格量的正是「8/4 这两枚观察值在
        // 这台机器上还是不是 8/4」，而那一句 unit 问不到。
        if let Some(r) = corner {
            // SAFETY: 读这扇窗自己的 DPI，与 `window_corner_radius` 里那一问同一手。
            let dpi = unsafe {
                windows::Win32::UI::HiDpi::GetDpiForWindow(crate::proc::to_hwnd(u64::from(hwnd)))
            };
            let can = [0, 4, 8].map(|dip| corner_radius_physical(dip, dpi));
            assert!(
                can.contains(&r),
                "系统圆角答出 {r}，本机 DPI={dpi} 上四档只能给出 {can:?}：档位表或 DPI 读错了"
            );
            println!("[window witness] dpi={dpi} 四档在这台机器上是 {can:?}");
        }
    }
}
