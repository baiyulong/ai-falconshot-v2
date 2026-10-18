//! The frozen screen, and the paths from a rectangle of it to a pin.
//!
//! One rule drives this whole module: the whole screen is captured **before** any
//! mask window exists, and every later read - the selection crop, the colour pick,
//! the magnifier, "capture again with the mask still up" - reads from that one
//! snapshot (plan §4-M1). A second `BitBlt` after the mask appears is how a
//! screenshot ends up containing the screenshot tool.
//!
//! The snapshot is cached on purpose. Freezing 3072x1920 measured 142-155 ms (P1/P5),
//! so the mask path cannot afford to re-read the screen per gesture, and the
//! magnifier would rebuild it on every mouse move.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use falcon_core::capture::{
    CaptureService, MonitorInfo, ScreenSnapshot, WindowExtras, WindowInfo, XcapCapture,
};
use falcon_core::geometry::{PhysPoint, PhysRect};
use falcon_core::pin::PinId;
use platform_windows::proc::{
    cursor_pos, foreground_window, is_minimized, pid_of_window, process_name, window_at_point,
    window_title,
};
use platform_windows::win::WinExtras;

use crate::state;

/// The capture service, with the Windows answers filled in behind `WindowExtras`.
///
/// Built per call rather than cached: it is two boxes and a pid, and every
/// enumeration has to be a fresh look at the desktop anyway - monitors get
/// unplugged and windows get closed between one question and the next.
pub fn service() -> CaptureService<WinExtras> {
    CaptureService::with_extras(Box::new(XcapCapture), Box::new(WinExtras))
}

/// How many times this process has frozen the screen, and how many times it has
/// gone to ask about the pointer.
///
/// These two numbers exist for one question, and it is not a question about pixels:
/// §5.4.3 的指针**在哪一刻**被读走。P106 之前它在 [`crate::mask::MaskState::commit`] 那道门上
/// 问，也就是「用户按下完成之后才去看指针此刻在哪」；而底图早在 [`freeze`] 就冻住了。读数时刻
/// 挪到冻结这一格之后，「一次流程问了几回」从跟着 commit 走变成跟着 freeze 走——这两枚计数器
/// 就是让门能在运行期把这条区别读出来，而不是要人来相信 doc。
///
/// 键开没开**不影响这两枚数**：[`cursor_overlay`] 在分岔之前就自增，因为它数的是「代码在哪一格
/// 去问」。键关着时那一问当场返回 `None`、一次平台调用也不发，所以门读到的 `cursor_reads` 是
/// 时刻的证据，不是像素的证据；键开那一侧的像素证人仍是 §10 第 106 项 ④ 欠着的第 28 格。
static FREEZE_COUNT: AtomicU64 = AtomicU64::new(0);
static CURSOR_READS: AtomicU64 = AtomicU64::new(0);

pub fn freeze_count() -> u64 {
    FREEZE_COUNT.load(Ordering::Relaxed)
}

pub fn cursor_reads() -> u64 {
    CURSOR_READS.load(Ordering::Relaxed)
}

/// §5.4.3 `include_cursor` 的**唯一读者**（P65 给整段 `[capture]` 建了快照读者，12 枚键里
/// 这一枚到今天才有第二个调用点 —— 而它要的是一条**分岔**，不是一次 clone）。
///
/// 两臂各说各的：键关着 ⇒ 直接 `None`，**一次平台调用都不发**（默认那一侧写在
/// `crates/core/src/config.rs:187`，多数用户从来不知道自己关着它）；键开着 ⇒ 问指针此刻
/// 在哪，再把这个答案交给平台的 [`WindowExtras::cursor_image`]。第二臂自 P106 起是**真臂**
/// （`crates/platform-windows/src/cursor.rs`：`GetCursorInfo` 问哪一只、`GetIconInfo` 问尺子
/// 与热点、一枚 `DrawIconEx` 画进本进程自己的 32 位 DIB），它交回 `None` 还剩两种原因：桌面
/// 此刻没亮着指针（`CURSOR_SHOWING` 没置上），或者那一趟 GDI 画不出图 —— 这一句写在这里而不
/// 是等门去猜，因为门能观察到的只有缝合，观察不到这三者里少了哪一格。
///
/// 交回的是 `(图, 图的左上角落在屏幕上的物理坐标)`，**不做任何裁剪**：选区在哪、贴去哪，
/// 归 [`crate::mask::MaskState::stamp_cursor`] 那一格的算术，那里能不开窗就验。
///
/// **调用它的是 [`freeze`]，不是 commit 那道门**（P107，结掉 §10 第 107 项 ⑨ 记的 #197）：
/// 指针的答案属于它被读走时那一枚画面，所以画面到手之后当场问一次、和画面并存进同一枚槽。
/// 判据是 PRD 那两条规则——第 419 行「指针热点位置应与实际屏幕位置一致」里的「实际位置」只能
/// 是**图里那块屏幕**，用户按下完成时指针已经走开的话，走开之后那个位置在图上根本没有对应物；
/// 第 420 行「延时截图应捕获倒计时结束时的指针状态」把时刻点名点得更死，而倒计时结束正是冻结
/// 发生的那一格。第 415 行「将当前鼠标指针合成」的「当前」在本轮读成「当前这张画面」，另一种
/// 读法（此刻）与 419/420 冲突，冲突时取那两条规则而不是取流程叙述里的一个形容词。
pub fn cursor_overlay() -> Option<(falcon_core::frame::Frame, PhysPoint)> {
    CURSOR_READS.fetch_add(1, Ordering::Relaxed);
    if !crate::settings::capture_cfg().include_cursor {
        return None;
    }
    let p = cursor_pos()?;
    WinExtras.cursor_image(p)
}

/// The monitors as the platform enumerates them: physical rects in virtual-desktop
/// coordinates, each with its own ratio, primary first.
///
/// No pixels are read, so this is cheap enough to call while a pin is being placed
/// rather than only while a frame is being frozen. It is also the only source of a
/// per-screen ratio that does not depend on where a window has ended up (§10 第 52 项 ①).
pub fn monitor_table() -> Result<Vec<MonitorInfo>, String> {
    service().monitors().map_err(|e| e.to_string())
}

/// A freeze, with how long it took. The timing is part of the product's promise
/// (plan P1: hot key to mask visible inside 150 ms), so it is measured here rather
/// than assumed.
pub struct Frozen {
    pub snap: ScreenSnapshot,
    pub ms: u64,
}

/// What one freeze owns: the pixels, and the pointer as it stood while those pixels
/// were read.
///
/// The two halves live in one value on purpose. §5.4.3 的覆盖层属于「它被读走时那一枚画面」，
/// 而这句话只要还是靠两处赋值挨得近来维持，就还是一次改码能拆开的东西：槽里放两枚 `Option`，
/// 忘了清其中一枚的 `clear()`、或者只写其中一枚的 `freeze()`，都能让画面与指针各说各话。
/// 合在一枚槽、一次锁、一次写入之后，那种走神不再有位置。
#[derive(Clone, Debug)]
pub struct Freeze {
    pub snap: ScreenSnapshot,
    pub cursor: Option<(falcon_core::frame::Frame, PhysPoint)>,
}

static FROZEN: OnceLock<Mutex<Option<Freeze>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Freeze>> {
    FROZEN.get_or_init(|| Mutex::new(None))
}

/// Step 1 of every capture flow. Also replaces any previous freeze: a stale
/// snapshot is what a "capture the screen as it was two minutes ago" bug looks
/// like, so a new flow always takes a new frame (PRD §8.5 makes the same point the
/// other way round - when the resolution changes, the old frame is invalid).
///
/// 指针那一问就在这一格里、画面到手之后当场问一次（P107 挪过来的，判据写在
/// [`cursor_overlay`] 头上）。问在 `?` 之后：一次失败的冻结不许留下任何新东西，槽里
/// 上一场的画面与指针一起还在原位——因为「只有画面更新了、指针还是上一场的」正是这一轮
/// 要关掉的那族 bug，而它不该从错误分支里进来。
pub fn freeze() -> Result<Frozen, String> {
    let started = std::time::Instant::now();
    let snap = service().freeze().map_err(|e| e.to_string())?;
    let ms = started.elapsed().as_millis() as u64;
    let cursor = cursor_overlay();
    FREEZE_COUNT.fetch_add(1, Ordering::Relaxed);
    *slot().lock().unwrap_or_else(PoisonError::into_inner) = Some(Freeze {
        cursor,
        snap: snap.clone(),
    });
    Ok(Frozen { snap, ms })
}

/// The frame the current flow is working from, if there is one.
pub fn current() -> Option<ScreenSnapshot> {
    slot()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|f| f.snap.clone())
}

/// The pointer that belongs to the frame currently in the slot, if there is one.
///
/// `None` 有三种读法，而这一格分不开它们——门只能数缝合，代码只能留证据：键关着（默认那一侧，
/// [`cursor_overlay`] 的第一臂）；桌面此刻没亮着指针；或者压根还没冻过画面。前两种在一场
/// 正常流程里长得一样。
///
/// 它读的是槽，不是桌面：从 [`freeze`] 到 [`crate::mask::MaskState::commit`] 之间指针走没走开，
/// 都不该改变交回的那一枚答案，而那正是 §5.4.3 第 419 行要的。
pub fn frozen_cursor() -> Option<(falcon_core::frame::Frame, PhysPoint)> {
    slot()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|f| f.cursor.clone())
}

/// The flow is over; the pixels go away with it — and so does the pointer read that
/// belonged to them, because the two are one value here.
pub fn clear() {
    *slot().lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Freeze if nothing is frozen yet, otherwise reuse the frame the user is looking at.
fn working_frame() -> Result<ScreenSnapshot, String> {
    match current() {
        Some(snap) => Ok(snap),
        None => freeze().map(|f| f.snap),
    }
}

/// The last region this process turned into a picture, and the leg that asked for it.
///
/// §5.2.5's 重复上次截图 needs a rectangle, not a frame: the picture has to be a fresh
/// look at the same place, so the memory outlives the freeze cache while the pixels do
/// not. `leg` is kept because the sentence this action prints is about *what* is being
/// repeated - the same three hundred pixels the user dragged out, or the same window
/// bounds - and a bare rect cannot tell those two apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastRegion {
    pub rect: PhysRect,
    pub leg: &'static str,
}

static LAST: OnceLock<Mutex<Option<LastRegion>>> = OnceLock::new();

fn last_slot() -> &'static Mutex<Option<LastRegion>> {
    LAST.get_or_init(|| Mutex::new(None))
}

/// The one decision the memory makes: has this rect earned a repeat.
///
/// An empty rect is a flow that never selected anything, and storing it would make
/// the next `capture_repeat` answer `Empty` - a bug report about the crop when the
/// truth is "nothing has been captured yet".
fn worth_remembering(rect: PhysRect) -> bool {
    !rect.is_empty()
}

/// Take note of a region that just became a picture.
pub fn remember(rect: PhysRect, leg: &'static str) {
    if !worth_remembering(rect) {
        return;
    }
    *last_slot().lock().unwrap_or_else(PoisonError::into_inner) = Some(LastRegion { rect, leg });
}

/// What the last capture asked for, if this process has made one.
pub fn last_region() -> Option<LastRegion> {
    *last_slot().lock().unwrap_or_else(PoisonError::into_inner)
}

/// The flow is over and the memory goes with it - used by the doors that have to
/// start from "nothing has been captured yet".
pub fn forget_region() {
    *last_slot().lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// §5.2.5: the last region again, on a frame taken **now**.
///
/// `working_frame` is deliberately not used here. Every other read of the freeze
/// exists to avoid re-photographing a desktop that has not changed; a repeat is the
/// one flow whose whole point is that the desktop *has* changed under a region that
/// has not. Reusing the cached frame would hand back the same pixels and call it a
/// second capture.
///
/// A region that no longer fits - a monitor unplugged, the resolution changed - is
/// clipped by `ScreenSnapshot::capture` and the clipping is said out loud, which is
/// what PRD §5.2.5 asks for ("自动裁剪到有效范围并提示").
pub fn pin_last_region() -> Result<(PinId, String), String> {
    let last = last_region().ok_or("还没有截图过：先把要重复的范围截出来")?;
    let f = freeze()?;
    let cap = f.snap.capture(&last.rect).map_err(|e| e.to_string())?;
    let id = state::with(|s| s.add(cap.frame, PhysPoint::new(cap.rect.x, cap.rect.y)))?;
    remember(cap.rect, "重复");
    Ok((
        id,
        format!(
            "重复{}的上一次区域 {:?}：{} 的 {} ms 新冻结{}",
            last.leg,
            cap.rect,
            f.snap.backend,
            f.ms,
            if cap.clipped {
                "（越界，已裁剪到当前桌面）"
            } else {
                ""
            }
        ),
    ))
}

/// Why `hwnd` cannot be §5.2.6's 活动窗口, or `None` when it can.
///
/// Split out from [`pin_active_window`] because the three readings that feed it -
/// which process owns the window, whether the shell draws it minimised, what the
/// session is showing - are each a separate Win32 answer that can fail separately,
/// while the *decision* is a table this function can be handed in a test with no
/// desktop behind it.
///
/// The order is the order of confidence: an unreadable owner is refused before
/// "minimised", because a handle the shell will not explain is not evidence that any
/// of the later readings mean anything.
pub fn active_window_refusal(
    owner: Option<u32>,
    self_pid: u32,
    minimized: bool,
    app: Option<&str>,
) -> Option<String> {
    let Some(pid) = owner else {
        return Some(
            "前台窗口查不到它的进程：受保护的进程或别的会话就是这样，这一趟不猜它是谁".to_string(),
        );
    };
    if pid == self_pid {
        return Some("前台是 Falconshot 自己：贴图截贴图只会得到一张图里的图".to_string());
    }
    if minimized {
        return Some("活动窗口最小化了：桌面上没有它的画面可截（§5.2.6）".to_string());
    }
    if let Some(app) = app {
        if is_lock(app) {
            return Some(format!("会话锁屏，前台是 {app}：那不是用户正在用的窗口"));
        }
    }
    None
}

/// `capture_active_window` (§5.2.6): the window the shell is giving keystrokes to,
/// as its own picture.
///
/// This leg does not read the freeze. A window shot goes to the DWM for that
/// window's own content, which is what makes it survive being partly behind
/// something else - and what makes the mask's frozen frame irrelevant here.
pub fn pin_active_window() -> Result<(PinId, String), String> {
    let hwnd = foreground_window().ok_or("前台没有窗口：这一趟连焦点在谁那里都问不出来")?;
    let owner = pid_of_window(hwnd);
    let app = owner.and_then(process_name);
    if let Some(why) = active_window_refusal(
        owner,
        std::process::id(),
        is_minimized(hwnd),
        app.as_deref(),
    ) {
        return Err(why);
    }
    let id = pin_window(hwnd as u32)?;
    let title = window_title(hwnd).unwrap_or_default();
    Ok((
        id,
        format!("活动窗口「{title}」[{}] {hwnd:#x}", app.unwrap_or_default()),
    ))
}

/// §5.3's output for one rectangle: stitched across monitors, clipped to the
/// desktop, and pinned where it was on screen.
///
/// Also the region `capture_repeat` will ask for next time (§5.2.5): what is
/// remembered is `cap.rect`, the crop that actually became a picture, rather than
/// `want`. A request that ran off the edge of the desktop has already been answered
/// once by clipping it, and repeating the answer is what the user saw.
pub fn pin_rect(want: &PhysRect) -> Result<PinId, String> {
    let snap = working_frame()?;
    let cap = snap.capture(want).map_err(|e| e.to_string())?;
    remember(cap.rect, "矩形");
    state::with(|s| s.add(cap.frame, PhysPoint::new(cap.rect.x, cap.rect.y)))
}

/// §5.3.2 - the window under a point, as the user sees it: topmost in z-order.
pub fn window_at(p: PhysPoint) -> Option<WindowInfo> {
    service().window_at(p)
}

/// §5.3.3 - the window the pointer is over right now.
pub fn window_under_cursor() -> Option<WindowInfo> {
    window_at(cursor_pos()?)
}

/// The middle of the primary monitor, in physical pixels - what `--snap` grabs
/// when it is not told a rectangle. The point of the flag is to prove that real
/// screen pixels reach a real window, not to stand in for a selection UI that does
/// not exist yet.
pub fn centre_rect(width: u32, height: u32) -> Result<PhysRect, String> {
    let snap = working_frame()?;
    let m = snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .or_else(|| snap.monitors.first())
        .ok_or("没有可截取的显示器")?;
    let b = m.info.bounds;
    Ok(PhysRect::new(
        b.x + (b.w as i32 - width as i32) / 2,
        b.y + (b.h as i32 - height as i32) / 2,
        width,
        height,
    ))
}

/// `--snap`: freeze, take the middle of the primary monitor, pin it. Returns the
/// new pin and the sentence describing the frame it came from.
pub fn snap_centre(width: u32, height: u32) -> Result<(PinId, String), String> {
    let f = freeze()?;
    let want = centre_rect(width, height)?;
    let id = pin_rect(&want)?;
    Ok((
        id,
        format!("{} ms, {:?} from {}", f.ms, want, snapshot_line(&f.snap)),
    ))
}

/// A rectangle in the shape `--rect x,y,w,h` carries.
pub fn rect_arg(raw: &str) -> Option<PhysRect> {
    let n: Vec<i32> = raw
        .split(',')
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().parse::<i32>().ok())
        .collect::<Option<Vec<_>>>()?;
    if n.len() != 4 {
        return None;
    }
    Some(PhysRect::new(
        n[0],
        n[1],
        n[2].max(1) as u32,
        n[3].max(1) as u32,
    ))
}

/// A whole window as its own picture. This path does not use the freeze: a window
/// shot asks the DWM for that window's content, which is what makes it survive
/// being behind another window - and what makes its size drift (P5), hence the
/// re-crop inside `capture_window`.
///
/// The re-cropped bounds are what goes into §5.2.5's memory, not the window's
/// geometry as the shell reported it: the picture the user got is the region they
/// will ask to see again.
pub fn pin_window(hwnd: u32) -> Result<PinId, String> {
    let cap = service().capture_window(hwnd).map_err(|e| e.to_string())?;
    remember(cap.rect, "窗口");
    state::with(|s| s.add(cap.frame, PhysPoint::new(cap.rect.x, cap.rect.y)))
}

/// `monitor id  WxH @x,y scale` per line, plus the union. Printed by `--capture`
/// and asserted on by the selftest, because "the enumeration is wrong" and "the
/// enumeration is right but the crop is wrong" need different fixes.
pub fn monitors_line() -> String {
    match service().monitors() {
        Ok(ms) => ms
            .iter()
            .map(|m| {
                format!(
                    "{} {}x{}@{},{} scale={:.2}{}",
                    m.id,
                    m.bounds.w,
                    m.bounds.h,
                    m.bounds.x,
                    m.bounds.y,
                    m.scale.ratio(),
                    if m.primary { " primary" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(" | "),
        Err(e) => format!("error: {e}"),
    }
}

/// The point a screen-reading row takes as the desktop's own: the centre of the virtual
/// bounds.
///
/// §10 第 42 项 ② made this a reading a row quotes rather than a detail of one log line:
/// a `readable` row has to say *what the desktop was* when it declined to answer, and the
/// answer it prints has to be the same coordinate [`snapshot_line`] prints, or the two
/// sentences describe two different screens.
pub fn centre_point(snap: &ScreenSnapshot) -> PhysPoint {
    PhysPoint::new(
        snap.virtual_bounds.x + snap.virtual_bounds.w as i32 / 2,
        snap.virtual_bounds.y + snap.virtual_bounds.h as i32 / 2,
    )
}

/// What the desktop is at [`centre_point`], already formatted, so a row that quotes it and
/// the line that prints it cannot disagree about how the reading is written.
pub fn centre_pixel(snap: &ScreenSnapshot) -> String {
    format!("{:?}", snap.color_at(centre_point(snap)))
}

/// One line describing a snapshot: backend, monitors, union, colour-pick probe.
pub fn snapshot_line(snap: &ScreenSnapshot) -> String {
    format!(
        "backend={} monitors={} virtual={}x{}@{},{} pixel@centre={}",
        snap.backend,
        snap.monitors.len(),
        snap.virtual_bounds.w,
        snap.virtual_bounds.h,
        snap.virtual_bounds.x,
        snap.virtual_bounds.y,
        centre_pixel(snap),
    )
}

/// Whether a process name belongs to the workstation lock.
///
/// A locked session puts a full-screen `LockApp` window, backed by `LogonUI`, above every
/// other window and nothing short of unlocking moves it. Naming that is what turns a
/// screen-reading row's SKIP into a statement about the session rather than a bug report
/// about the capture path - which since §10 第 41 项 ⑤ is *not* an excuse to explain a
/// row away by it: P40 read better ink on the locked run than on the unlocked one, so the
/// lock is one input parameter of a row, not a verdict over a round.
pub(crate) fn is_lock(app: &str) -> bool {
    let a = app.to_ascii_lowercase();
    a.contains("lockapp") || a.contains("logonui")
}

/// What the desktop is showing at its own centre right now.
///
/// §10 第 41 项 ④: `--r13` has been able to say 会话已锁屏 since P39 and the other three
/// gauges said nothing at all, so a run of `--ink` alone left no scene behind it and the
/// only witness was a `tasklist` in a batch file - which was broken (`//FI`, the Git-Bash
/// spelling) until this round. The reading lives here, next to the capture paths, because
/// it is an input of every row that reads the screen and not a property of one report.
///
/// No pixels are grabbed: the point is the primary monitor's centre and the answer is which
/// window the shell puts there. A `--pin` run does not look at the desktop (§10 第 41 项 ⑧)
/// and still gets to say what was there.
///
/// [`Scene::hwnd`] is the reading and not a word in the sentence: a row that graded "the
/// line does not begin with 没有" would be grading this file's own phrasing, which is the
/// shape §10 第 41 项 ⑥ refuses - 判接线的格，对面必须是操作系统.
pub struct Scene {
    pub line: String,
    pub hwnd: Option<u64>,
}

pub fn desktop_scene() -> Scene {
    let refused = |why: String| Scene {
        line: format!("没有桌面读数：{why}"),
        hwnd: None,
    };
    let monitors = match service().monitors() {
        Ok(m) => m,
        Err(e) => return refused(format!("monitors() 报 {e}")),
    };
    let Some(m) = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
    else {
        return refused(format!("枚举到 {} 个显示器", monitors.len()));
    };
    let centre = PhysPoint::new(
        m.bounds.x + m.bounds.w as i32 / 2,
        m.bounds.y + m.bounds.h as i32 / 2,
    );
    match window_at_point(centre) {
        None => refused(format!("({},{}) 处桌面没有任何窗口", centre.x, centre.y)),
        Some(hwnd) => {
            let app = pid_of_window(hwnd)
                .and_then(process_name)
                .unwrap_or_else(|| "未知进程".to_string());
            let title = window_title(hwnd).unwrap_or_default();
            Scene {
                line: format!(
                    "({},{}) 上是 {app}「{title}」 hwnd=0x{hwnd:x} 会话锁屏={}",
                    centre.x,
                    centre.y,
                    is_lock(&app)
                ),
                hwnd: Some(hwnd),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §10 第 41 项 ④: the rule that turns a screen row's refusal into a statement about the
    /// session is two substrings, and until this round those two substrings lived inside one
    /// report's `map(|(app, _)| …)` closure - so the other three gauges could not name the
    /// same thing without writing the rule a second time. A name a test can hit is what keeps
    /// them honest, and the case list is the shell's own spelling, not this file's: the name
    /// comes back from `QueryFullProcessImageNameW` in whatever case the exe's own directory
    /// entry has.
    #[test]
    fn a_lock_is_named_by_the_process_and_not_by_a_pixel() {
        assert!(is_lock("LockApp.exe"));
        assert!(is_lock("LOCKAPP.EXE"));
        assert!(is_lock("LogonUI.exe"));
        assert!(!is_lock("falconshot-desktop.exe"));
        assert!(!is_lock("explorer.exe"));
        // The desktop's own window, which is what a normal unlocked session answers here.
        assert!(!is_lock("Progman"));
        assert!(!is_lock(""));
    }

    #[test]
    fn a_rect_argument_is_four_numbers_and_nothing_else() {
        assert_eq!(
            rect_arg("10,20,300,400"),
            Some(PhysRect::new(10, 20, 300, 400))
        );
        // Spaces are typed by humans; three numbers is a typo, and a typo that
        // silently becomes a rect is worse than a rect that is refused.
        assert_eq!(
            rect_arg(" 10 , 20 , 300 , 400 "),
            Some(PhysRect::new(10, 20, 300, 400))
        );
        assert_eq!(rect_arg("10,20,300"), None);
        assert_eq!(rect_arg("10,20,300,abc"), None);
        assert_eq!(rect_arg(""), None);
    }

    #[test]
    fn a_zero_sized_rect_becomes_one_pixel_rather_than_nothing() {
        // An empty rect is refused by `ScreenSnapshot::capture` with `Empty`, so a
        // `--rect 0,0,0,0` would read as "the desktop is broken". Clamping to one
        // pixel keeps the diagnostic about the region the user named.
        assert_eq!(rect_arg("5,6,0,-40"), Some(PhysRect::new(5, 6, 1, 1)));
    }

    /// §5.2.5's memory keeps the region of a capture that happened, not of a flow
    /// that gave up. One pixel is a picture; zero is not.
    #[test]
    fn only_a_region_with_pixels_earns_a_repeat() {
        assert!(worth_remembering(PhysRect::new(0, 0, 1, 1)));
        assert!(!worth_remembering(PhysRect::new(10, 20, 0, 400)));
        assert!(!worth_remembering(PhysRect::default()));
    }

    /// §5.2.6's refusals, one per reading, and in the order the readings are trusted.
    /// The desktop is not needed for any of them: this is the table the leg consults
    /// after it has asked the shell its three questions.
    #[test]
    fn the_active_window_is_refused_for_the_reason_the_shell_gave() {
        let me = 4_100;
        // A handle the shell will not explain outranks everything else, because the
        // later readings are readings about a window this run cannot even name.
        let why = active_window_refusal(None, me, true, Some("LockApp.exe")).unwrap();
        assert!(why.contains("查不到它的进程"), "{why}");
        // Our own window is the loop the PRD's 活动窗口 is not about: a pin of a pin.
        let why =
            active_window_refusal(Some(me), me, false, Some("falconshot-desktop.exe")).unwrap();
        assert!(why.contains("Falconshot 自己"), "{why}");
        // Minimized beats the lock reading, and both beat acceptance - a locked
        // session's foreground window is minimised as far as this run can tell, and the
        // sentence the user reads should be the one that is certainly true.
        let why = active_window_refusal(Some(4_200), me, true, Some("LockApp.exe")).unwrap();
        assert!(why.contains("最小化"), "{why}");
        let why = active_window_refusal(Some(4_200), me, false, Some("LogonUI.exe")).unwrap();
        assert!(why.contains("锁屏"), "{why}");
        assert_eq!(
            active_window_refusal(Some(4_200), me, false, Some("notepad.exe")),
            None
        );
        // Not knowing the process *name* is not a refusal: `is_lock` is one of the
        // checks, and a window that answers its pid but not its exe is still a window.
        assert_eq!(
            active_window_refusal(Some(4_200), me, false, None),
            None,
            "查不到名字不是查不到窗口"
        );
    }

    /// The refusal rule is the shell's spelling, not this file's: the two names the
    /// lock window is answered under are the ones [`is_lock`] is written for.
    #[test]
    fn a_lock_is_the_third_reading_not_a_fourth_question() {
        assert!(
            active_window_refusal(Some(4_200), 4_100, false, Some("LockApp.exe"))
                .unwrap()
                .contains("LockApp.exe")
        );
    }
}
