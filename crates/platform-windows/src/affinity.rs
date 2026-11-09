//! §5.2.1 步 2 后半支（`[capture] freeze_frame` 关着 = 背景该是**活的**）所需的那一只手。
//!
//! 这条路上的第一道闸不是画法，也不是定时器，而是本仓头顶那一条规矩（`crates/qt_bridge/src/
//! capture.rs` 头上那句：整块屏幕只在任何遮罩窗存在**之前**读一次，之后再 `BitBlt` 一次，拍进
//! 图里的就是截图工具自己）。实时背景要么把这条规矩破掉，要么让「拍进来」这件事不可能发生 ——
//! 后者是 Win32 有的一格：`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`。它说的是「这扇窗
//! 只在该它的显示器上被看见」，屏幕上照旧有它，而屏幕读取里没有它。
//!
//! 这一格只做两件事：把那一句**说给系统**，再把系统此刻说的数**读回来**。它不判「实时该不该开」，
//! 也不判「读回来的那张图里还有没有自己」—— 那是调用方的事，而在这里永远只是数字。

/// 系统此刻给一扇窗定的档位，收成自己的枚举而不是转发的整数：`WDA_*` 那一族将来还会加档，
/// 多出的一档不许在这里被读成「没档」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Affinity {
    /// 默认档：这扇窗在屏幕读取里就在。
    Plain,
    /// 屏幕读取里看不见，屏幕上看得见。
    Excluded,
    /// 别的档（`WDA_MONITOR` 那一类），原样交回系统那个数。
    Other(u32),
}

/// 此刻的档位。`None` = 系统没答（窗没了、调用失败），它与 [`Affinity::Plain`] 不是一件事：
/// 把「问不出」读成「没档」，就等于替用户宣布这扇窗会被拍进图里。
#[cfg(windows)]
pub fn current(hwnd: u32) -> Option<Affinity> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };

    let mut raw = 0u32;
    // SAFETY: `raw` is a writable u32 this call fills, and the handle is the caller's.
    unsafe { GetWindowDisplayAffinity(crate::proc::to_hwnd(u64::from(hwnd)), &mut raw) }.ok()?;
    Some(classify(raw, WDA_NONE.0, WDA_EXCLUDEFROMCAPTURE.0))
}

/// 非 Windows 那一臂：没有这一格，`None` = 没有答案，而不是「等于没档」。
#[cfg(not(windows))]
pub fn current(_: u32) -> Option<Affinity> {
    None
}

/// 把这扇窗从屏幕读取里摘出去，然后**读回**系统定的档位。
///
/// 交回的是读回来的那个数，不是「调用成功了」：`SetWindowDisplayAffinity` 应下而档位没生效
/// （远台会话、策略挡着）是这一族里唯一会让实时背景把工具自己拍进图里的形状，而拦下它靠的
/// 就是这一格。设不成也交回此刻的读数 —— 调用方要的从来是「现在能不能不被拍进去」。
#[cfg(windows)]
pub fn exclude(hwnd: u32) -> Option<Affinity> {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};

    // SAFETY: the handle is the caller's, and the flag is one of the API's own constants.
    let _ = unsafe {
        SetWindowDisplayAffinity(crate::proc::to_hwnd(u64::from(hwnd)), WDA_EXCLUDEFROMCAPTURE)
    };
    current(hwnd)
}

#[cfg(not(windows))]
pub fn exclude(_: u32) -> Option<Affinity> {
    None
}

/// 收回那一句，回到默认档。一场流程结束（或实时那一臂被放弃）时该叫这一手，而不是留着：
/// 留着的话这扇窗在**别的程序**的录屏与投屏里也不出现，而那不再是本产品要的事。
#[cfg(windows)]
pub fn restore(hwnd: u32) -> Option<Affinity> {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_NONE};

    // SAFETY: as above; `WDA_NONE` is the API's own default flag.
    let _ = unsafe { SetWindowDisplayAffinity(crate::proc::to_hwnd(u64::from(hwnd)), WDA_NONE) };
    current(hwnd)
}

#[cfg(not(windows))]
pub fn restore(_: u32) -> Option<Affinity> {
    None
}

/// 系统那个数 → [`Affinity`]。拆出来是为了这一手不需要 `windows` 也不必开一扇窗就能被测：
/// 认不出的一档必须走 [`Affinity::Other`]，不许被兜底臂读成默认档。
fn classify(raw: u32, none: u32, excluded: u32) -> Affinity {
    if raw == none {
        Affinity::Plain
    } else if raw == excluded {
        Affinity::Excluded
    } else {
        Affinity::Other(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 那三个数的形状：`WDA_NONE = 0`、`WDA_EXCLUDEFROMCAPTURE = 17`（`0x11`）。这里钉的不是
    /// 系统给的数，是**这一手的读法**：认不出的一档要留成 `Other`。
    #[test]
    fn an_unknown_tier_is_not_read_as_the_default() {
        assert_eq!(classify(0, 0, 17), Affinity::Plain);
        assert_eq!(classify(17, 0, 17), Affinity::Excluded);
        assert_eq!(classify(1, 0, 17), Affinity::Other(1));
        assert_eq!(classify(18, 0, 17), Affinity::Other(18));
    }

    /// 问一扇根本不存在的窗：交回的是「没答」，不是「默认档」。这一手在无桌面上唯一能测的
    /// 那一半。
    #[test]
    fn a_window_that_is_not_there_is_not_answered_as_plain() {
        assert_eq!(current(0), None);
        assert_eq!(exclude(0), None);
        assert_eq!(restore(0), None);
    }
}

/// 一扇真的窗、一次真的屏幕读取：这一格量的是**这台机器上屏幕读取到底听不听这一句**，而那是
/// 整个实时背景分支的前提，不是可以推理的东西。跑法与 [`crate::uia`] 那枚 `#[ignore]` 的实测
/// 同一族：`cargo test -p platform-windows -- --ignored`。
///
/// 屏幕上会闪过一块洋红，一瞬。它必须在**屏幕正显示着本进程那块桌面**的时候跑：锁屏期间本进程摆的窗
/// 屏幕上没有，于是这一格读回来的是「一枚也不差」—— 而那个数是那次锁屏的读数，不是这台机器的答案。
/// 跑之前先看印出来的 `屏幕上此刻显示的那一扇窗` 那一行；跑之后也是它先答「这一格的读数作不作数」。
/// 别用 `input=` 那一格判锁屏：Windows 11 的锁屏由 `LockApp` 挂在**同一块** `Default` 桌面上，锁着时
/// `input_desktop()` 照旧答 `Default`（2026-11-06 在本机实测：`input=Some("Default")` 而
/// `foreground` 的标题是 `Windows Default Lock Screen`）。
#[cfg(windows)]
#[test]
#[ignore = "creates a window on the real desktop and photographs it"]
fn the_probe_reads_what_exclusion_does_to_a_screen_grab() {
    use falcon_core::capture::{FrameSource, XcapCapture};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::CreateSolidBrush;
    use windows::Win32::Graphics::Gdi::UpdateWindow;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// 类过程序就是 `DefWindowProcW`，而 `WNDPROC` 要的是 `extern "system"` 的那一枚指针，
    /// 直接递进去是一个 Rust 函数项 —— 包一层，别的什么都不做。
    unsafe extern "system" fn probe_proc(
        w: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(w, msg, wparam, lparam)
    }

    const SIDE: i32 = 200;
    let magenta = [255u8, 0, 255, 255];

    // 这一趟进程先声明它懂 DPI，`CreateWindowExW` 那四个数才是物理像素那一空间的数。真产品那条
    // 腿由 Qt 在启动时声明，不需要这一手。
    let _ = unsafe { SetProcessDPIAware() };

    let primary = XcapCapture
        .monitors()
        .expect("no monitor")
        .into_iter()
        .find(|m| m.primary)
        .expect("no primary monitor");
    // 摆在主屏里面一点，四边都不出屏。
    let at = POINT {
        x: primary.bounds.x + 300,
        y: primary.bounds.y + 300,
    };
    // **整屏**读，不在小窗上算坐标：这一格要答的是「屏幕读取里这扇窗在不在」，而这一块在不在是
    // 扫出来的，不是我按 `CreateWindowExW` 那四个数推出来的 —— 那四个数属于哪一个坐标空间，正是
    // 这一台 3072×1920 @200% 的机器上最容易算错的一格，而算错了的读数看起来和「不认这一句」一模一样。
    let read = || {
        // 顺手量一手：实时背景那一臂的刷新节拍直接落在「整块屏幕读一次要多久」上，而这一格的读数
        // 只说这台机器此刻答得出多少毫秒 —— 它是那一拍的依据，不是产品里那句 `re_read` 报的数。
        let clock = std::time::Instant::now();
        let frame = XcapCapture
            .capture_monitor(&primary.id)
            .expect("screen read failed");
        println!("整屏读一次 = {} ms", clock.elapsed().as_millis());
        frame
    };
    // 这一张里所有洋红像素的外接矩形；`None` = 屏幕上这一块没有这扇窗。
    let magenta_box = |f: &falcon_core::frame::Frame| -> Option<(u32, u32, u32, u32, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        let mut hits = 0usize;
        for y in 0..f.bounds().h {
            let row = f.row(y);
            for x in 0..f.bounds().w {
                let p = &row[(x as usize) * 4..(x as usize) * 4 + 4];
                if p == magenta {
                    hits += 1;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (hits > 0).then(|| (x0, y0, x1, y1, hits))
    };

    let before = read();
    let name: Vec<u16> = "FalconshotAffinityProbe\0".encode_utf16().collect();
    let class = PCWSTR(name.as_ptr());
    let instance = HINSTANCE(unsafe { GetModuleHandleW(None) }.expect("no module").0);
    let mut cls: WNDCLASSEXW = Default::default();
    cls.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
    cls.style = CS_HREDRAW | CS_VREDRAW;
    cls.lpfnWndProc = Some(probe_proc);
    cls.hInstance = instance;
    cls.hbrBackground = unsafe { CreateSolidBrush(COLORREF(0x00FF_00FF)) };
    cls.lpszClassName = class;
    // 登记失败只有一种形状：这一趟进程里已经有同名类（跑过一遍的 `--ignored`）。那一格是同一份代码
    // 登记的同一块洋红刷，`CreateWindowExW` 拿到的还是这一格要的窗，所以读数不作数与否不由这一句判。
    let _ = unsafe { RegisterClassExW(&cls) };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            class,
            class,
            WS_POPUP,
            at.x,
            at.y,
            SIDE,
            SIDE,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .expect("probe window refused");

    let show_and_read = |hwnd: HWND| -> falcon_core::frame::Frame {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
            for _ in 0..8 {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
        }
        read()
    };

    let shown = show_and_read(hwnd);
    let seen = magenta_box(&shown);

    // 两整张图之间不一样的像素有几枚、外接框在哪、样个颜色 —— 这一手是为了把「这扇窗没画洋红」
    // 与「这台机器的屏幕读取里根本没有这扇窗」分开：前者差出来的那一块就在窗那个位置，后者一枚也不差。
    let diff = |a: &falcon_core::frame::Frame, b: &falcon_core::frame::Frame| -> (usize, Option<(u32, u32, u32, u32)>) {
        let (w, h) = (a.bounds().w.min(b.bounds().w), a.bounds().h.min(b.bounds().h));
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        let mut hits = 0usize;
        for y in 0..h {
            for x in 0..w {
                if a.get(x, y) != b.get(x, y) {
                    hits += 1;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (hits, (hits > 0).then(|| (x0, y0, x1, y1)))
    };
    let (changed, where_changed) = diff(&before, &shown);

    // 这一手不为判档：它问的是「这条读取腿此刻装的是什么」—— 一整块纯色，还是十六个采点各读回什么。
    // 「窗出现前后一枚也不差」这一句要和它放在一起读：底是纯黑与底是桌面，对「这条腿读的不是屏幕」
    // 与「这一台的屏幕上此刻没有东西在变」是两种不同的说法，而它们要的处理不是一种。
    let sample = |f: &falcon_core::frame::Frame| -> (Option<[u8; 4]>, Vec<[u8; 4]>) {
        let (w, h) = (f.bounds().w.max(1), f.bounds().h.max(1));
        (
            f.is_uniform(),
            (0..16).map(|i| f.get(i * w / 16, i * h / 16)).collect(),
        )
    };

    // 这一格再换一个证人：`PrintWindow` 问的是「这扇窗自己画得出什么」，它不经过桌面合成。
    // 洋红在这里有、在屏幕读取里没有 ⇒ 窗会画，是合成那一条路把这台机器的桌面递给了别人。
    let printed = crate::print::print_window(hwnd.0 as u64, SIDE as u32, SIDE as u32, 0);

    // 这一格要先知道自己站在哪块桌面上、这扇窗此刻在不在屏幕上。读数在换档**之前**取：
    // `IsWindowVisible` 与 `GetWindowRect` 问的是此刻的窗，而此刻它还没被摘出去。
    let (station, visible, rect) = {
        let mut r = windows::Win32::Foundation::RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut r) };
        (
            crate::desktop::station_summary(),
            unsafe { IsWindowVisible(hwnd) }.as_bool(),
            r,
        )
    };
    // 第三格证人，与上面两格不是一件事：每一个会话都有自己的 `WinSta0\Default`，所以
    // `station=` / `input=` 那两行都写着 `Default` 时，本进程照样可能站在**没接到屏幕**的那一块
    // 桌面上；而 DWM 没在合成时，GDI 那条读取腿读回来的那张桌面里根本不会有普通窗。这两种读数
    // 都「不作数」，但挡的是两件不同的事，末尾那一句分岔靠的就是它。
    let reach = crate::desktop::display_reach();
    // 第四格，也是这一台机器上真正挡路的那一格：**屏幕上此刻显示的是哪一扇窗**。锁屏期间本进程照旧
    // 挂在 `WinSta0\Default`，而 Windows 11 的锁屏由 `LockApp` 挂在同一块 `Default` 桌面上，所以
    // `input_desktop()` 照旧答 `Default` —— 那一格分不开「锁着」与「没锁」（它的文档里那句「能分开」
    // 是错的，本轮已改）。锁着时屏幕上不是本进程摆窗的那一张，任何一条读取腿读回来的都是一张**不动的
    // 锁屏**：洋红窗一枚也不差，与「这条腿不跟桌面走」是同一个数，而它俩要的处理完全不一样。
    let foreground = crate::proc::foreground_window().map(|fg| {
        (
            fg,
            crate::proc::pid_of_window(fg).and_then(crate::proc::process_name),
            crate::proc::window_title(fg),
        )
    });
    let on_lock_screen = foreground
        .as_ref()
        .is_some_and(|(_, name, title)| {
            let name = name.clone().unwrap_or_default().to_lowercase();
            let title = title.clone().unwrap_or_default().to_lowercase();
            name.contains("lockapp")
                || name.contains("logonui")
                || title.contains("lock screen")
                || title.contains("锁屏")
        });
    let painted = printed
        .as_ref()
        .map(|f| (f.bounds(), f.is_uniform(), f.get(2, 2)));

    let readback = exclude(hwnd.0 as u64 as u32);
    unsafe {
        // 换档要一次重组成：DWM 不保证把已经排好的那一帧重画，`SWP_FRAMECHANGED` 就是「重算一次」。
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    let after = show_and_read(hwnd);
    let gone = magenta_box(&after);
    // 收尾这一手不是礼貌：档留着不收回，这扇窗在**别的程序**的录屏与投屏里也不出现，而那不是本产品
    // 要的事。收完再毁窗 —— 毁了就问不出来了。
    let restored = restore(hwnd.0 as u64 as u32);
    let _ = unsafe { DestroyWindow(hwnd) };

    // 一扇窗、两次读取、三份证据：先把数印全，再按证据的次序问，让最先挡路的那一句说清它挡的是什么。
    println!(
        "station={station} input={:?} visible={visible} rect={rect:?} monitor={:?} id={}",
        crate::desktop::input_desktop(),
        primary.bounds,
        primary.id
    );
    println!("这一趟进程站的那一块桌面 = {}", reach.summary());
    println!("屏幕上此刻显示的那一扇窗 = {foreground:?} 锁屏={on_lock_screen}");
    println!("PrintWindow（不经桌面合成，问这扇窗自己画得出什么）= {painted:?}");
    println!("这条读取腿在窗出现之前装的是 = {:?}", sample(&before));
    println!("这条读取腿在窗出现之后装的是 = {:?}", sample(&shown));
    println!("magenta seen = {seen:?}");
    println!(
        "whole-screen diff before→shown = {changed} 枚，外接框 {where_changed:?}，那里读回来 {:?}",
        where_changed.map(|(x, y, _, _)| shown.get(x, y))
    );
    println!("affinity readback = {readback:?}");
    println!("magenta after exclusion = {gone:?}");
    println!("affinity after restore = {restored:?}");
    // 那扇窗原来占着的那一块，现在读回来是什么样子：底下那层露出来（与窗出现之前同一块最像），
    // 还是一块纯黑（档生效了，可屏幕读取里留的是黑洞），还是照旧是洋红（这一句没被听）。
    if let Some((x0, y0, x1, y1, _)) = seen {
        let w = x1 - x0 + 1;
        let h = y1 - y0 + 1;
        let same_as_before = (0..h.min(40)).all(|dy| {
            (0..w.min(40)).all(|dx| {
                after.get(x0 + dx, y0 + dy) == before.get(x0 + dx, y0 + dy)
                    || after.get(x0 + dx, y0 + dy) == magenta
            })
        });
        let block: Vec<[u8; 4]> = (0..16)
            .map(|i| after.get(x0 + i * (w / 16).max(1), y0 + i * (h / 16).max(1)))
            .collect();
        println!("the hole now looks like: 底下那层露出来={same_as_before} 前十六采点={block:?}");
    }

    // ① 这扇窗自己画得出那一整块洋红吗？这一句不经过桌面合成，所以它是「窗是活的」这一格的证人。
    // 画不出 ⇒ 后面每一句量的都是一扇没画的窗，先修这一格再谈别的。
    assert_eq!(
        painted.map(|(_, uniform, _)| uniform),
        Some(Some(magenta)),
        "PrintWindow 都交不回一块纯洋红（{painted:?}）：这扇窗根本没画，屏幕读取里没有它不是档的功劳"
    );
    // ② 这条屏幕读取腿跟着桌面走吗？窗出现前后整块屏幕一枚也不差，而窗确实会画 ⇒ 读回来的不是
    // 此刻屏幕上那一张。这一格挡在档之前，而它挡的到底是哪一件，由四种读数分开说（#453：四种失败
    // 不并成一句），处理也不是一种：
    //   a) 屏幕上此刻显示的不是本进程那块桌面（锁屏正是这一格）—— 读数不作数，解锁后再跑一次；
    //   b) 本进程不站在屏幕上那一块桌面 —— 读数不作数，要到接到物理控制台的那一个会话里再跑；
    //   c) DWM 没在合成 —— 这条读取腿读的是合成好的那一张，普通窗永远进不去，这一台做不成实时
    //      背景，而「换一次解锁」不解决它；
    //   d) 上面三格都好，读数照旧一枚也不差 —— 这条读取腿本身不跟桌面走。
    // 四种都不是「档生效了」的证据：连桌面上多出来的一整块洋红都读不进来时，「这扇窗被摘出去了」
    // 与「这条腿读的是别的东西」是同一个数。
    if changed == 0 {
        if on_lock_screen {
            panic!(
                "摆一扇 {SIDE}×{SIDE} 的洋红窗、PrintWindow 确认它画得出来，而整块屏幕的读取在窗出现前后一枚也不\
                 差：屏幕上此刻显示的不是本进程那块桌面，而是锁屏（foreground={foreground:?}）。这一格的读数不作\
                 数 —— 那一张不动的图是锁屏的读数，不是这台机器的答案，而 `input=` 那一格在这一台分不开它（Win\
                 dows 11 的锁屏由 LockApp 挂在同一块 Default 桌面上，所以它照旧答 Default）。解锁之后再跑一次；\
                 实时背景的前提在那一次才算测过。"
            );
        }
        if reach.same_session() != Some(true) {
            panic!(
                "摆一扇 {SIDE}×{SIDE} 的洋红窗、PrintWindow 确认它画得出来，而整块屏幕的读取在窗出现前后一枚也\
                 不差。这一趟进程不站在屏幕上那一块桌面（{}）：station/input 那两格分不开这一件，因为每个会话都\
                 有自己的 WinSta0\\Default。这一格的读数不作数，要到接到物理控制台的那一个会话里再跑一次。",
                reach.summary()
            );
        }
        if reach.composition != Some(true) {
            panic!(
                "摆一扇 {SIDE}×{SIDE} 的洋红窗、PrintWindow 确认它画得出来，而整块屏幕的读取在窗出现前后一枚也\
                 不差。本进程就站在屏幕上那一块桌面（{}），而 DWM 没在合成：GDI 那条读取腿读回来的是合成好的那一\
                 张，普通窗只画在自己的 DC 上，那张图里永远不会有它们。换一次解锁不解决这一格，而实时背景在这台\
                 机器上做不成 —— 那张被读的底本来就是死的。",
                reach.summary()
            );
        }
        panic!(
            "摆一扇 {SIDE}×{SIDE} 的洋红窗、PrintWindow 确认它画得出来，本进程站在屏幕上那一块桌面且 DWM 在合成\
             （{}），屏幕上显示的也不是锁屏（foreground={foreground:?}），而整块屏幕的读取在窗出现前后一枚也不差：\
             这条读取腿本身不跟着桌面走。「这条读取腿装的是」那两行说的是它装的是哪一种（纯黑 = 读的是空，桌面样 =\
             读的是不走动的那一张）。实时背景的前提在这一台测不出来。",
            reach.summary()
        );
    }
    // ③ 尺子读得出来的那一块，得是窗所在的那一块：外接框在不在 `CreateWindowExW` 交出去的那个位置，
    // 就是那四个数属于哪一个坐标空间的答案 —— 这一台 3072×1920 @200% 的机器上最容易算错的一格。
    let (x0, y0, x1, y1, hits) = seen.unwrap_or_else(|| {
        panic!(
            "整块屏幕在窗出现前后差了 {changed} 枚，可一个洋红像素都没有（外接框 {where_changed:?}）：\
             差出来的不是这扇窗，下面每一句都不作数"
        )
    });
    assert!(
        hits > (SIDE as usize) * (SIDE as usize) / 2,
        "那扇 {SIDE}×{SIDE} 的窗只读出 {hits} 个洋红像素（外接框 ({x0},{y0})-({x1},{y1})，摆在 ({},{})）：\
         这一格摆的地方和读的地方不是一块",
        at.x,
        at.y
    );
    // ④ 系统收下这一句了吗？交回的是**读回来的档位**，不是「调用成功了」。
    assert_eq!(readback, Some(Affinity::Excluded), "系统没把这一句收下");
    // ⑤ 收下之后，屏幕读取里这扇窗没了没？这是整个实时背景分支的前提那一格。
    assert_eq!(
        gone, None,
        "档已生效而屏幕读取里这扇窗照旧在（{gone:?}）—— 这一句对这条读取腿没用，实时背景不能靠它"
    );
    // ⑥ 收回的那一句也照旧算数：留着不收回会影响别的程序的录屏。
    assert_eq!(restored, Some(Affinity::Plain), "档没收回来");
}
