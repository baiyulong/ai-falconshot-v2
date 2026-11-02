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

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use falcon_core::capture::{
    CaptureService, MonitorInfo, ScreenSnapshot, WindowExtras, WindowInfo, XcapCapture,
};
use falcon_core::frame::Frame;
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
///
/// `leg` became a `String` in P169 because the same value now crosses a process
/// boundary: the word a *later* run reads back is one no `&'static str` in this binary
/// wrote. A `String` costs this struct its `Copy`, and nothing reads it by copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastRegion {
    pub rect: PhysRect,
    pub leg: String,
}

/// 「重复」这一腿的名字，唯一一处拼写。`pin_last_region` 用它改写本进程的记忆，而
/// [`worth_persisting`] 用它拒绝改写盘上那一行 —— 理由见那枚函数。
pub const REPEAT_LEG: &str = "重复";

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

/// 盘上那一行值不值得写。它跟着 [`worth_remembering`] 的规矩，只多一条：**「重复」不写**。
///
/// 盘上那一行的职责是回答「用户上一次*截*的是什么」，而一次重复没有截新东西 —— 它读的就是
/// 这一行。把 `重复` 抄回去，第三趟进程就会说出 «重复重复的上一次区域»，而那句话里第一个
/// 「重复」已经不是在说用户的手势了。内存里那一格照旧被 `pin_last_region` 改成 `重复`
/// （P167 起就是这么定的，为的是让紧接着的第二句承认自己是重复），只是不落到盘上。
fn worth_persisting(last: &LastRegion) -> bool {
    worth_remembering(last.rect) && last.leg != REPEAT_LEG
}

/// Take note of a region that just became a picture: in this process, and — if the
/// §5.17 store is open ([`enable_region_store`]) — on disk for the process after it.
pub fn remember(rect: PhysRect, leg: &str) {
    if !worth_remembering(rect) {
        return;
    }
    let last = LastRegion {
        rect,
        leg: leg.to_string(),
    };
    *last_slot().lock().unwrap_or_else(PoisonError::into_inner) = Some(last.clone());
    if store_enabled() && worth_persisting(&last) {
        // A state write that the machine refused is *not* folded into the exit code:
        // §5.17 rule 3 is about the picture the call named, and that one landed. The
        // refused store shows up where it matters — the next `--last` reads no region
        // and answers 「还没有截图过」, so the loss is witnessed by the next process
        // rather than by a counter nobody outside this one can read.
        let _ = save_at(&state_path(), &encode_line(&last));
    }
}

/// What the last capture asked for, if **this process** has made one. Deliberately
/// memory-only: §5.3.5's 自定义截图 dialog and the autosave gauge read the region the
/// flow they belong to picked, and a file written by an unrelated earlier run would
/// answer a question nobody asked them.
pub fn last_region() -> Option<LastRegion> {
    last_slot()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// What §5.17's `--last` reads: this process's memory first, and only when it is empty
/// the one line the previous process left on disk.
fn last_region_across_processes() -> Option<LastRegion> {
    if let Some(seen) = last_region() {
        return Some(seen);
    }
    if DISK_MUTED.load(Ordering::SeqCst) {
        return None;
    }
    read_at(&state_path()).and_then(|text| decode_line(&text))
}

/// The flow is over and the memory goes with it - used by the doors that have to
/// start from "nothing has been captured yet".
///
/// Forgetting also mutes the disk for the rest of this process. A door that wipes the
/// memory to measure 「还没有截图过」 must not get a region back out of a file it never
/// wrote, or P169's fallback turns one of the oldest refusals into a silent success.
pub fn forget_region() {
    *last_slot().lock().unwrap_or_else(PoisonError::into_inner) = None;
    DISK_MUTED.store(true, Ordering::SeqCst);
}

// ─────────────────────────────────────── §5.17 的那一份跨进程状态（P169）

/// 量具与命令行钉落点用的环境变量，与 `FALCONSHOT_GUIDE_DIR` / `FALCONSHOT_CONFIG`
/// 同一族：给了就往那里读写，没给才问 `advanced.data_dir` 那一位主人。
pub const ENV_DIR: &str = "FALCONSHOT_LASTREGION_DIR";

/// 那份状态在盘上的名字。
pub const STATE_FILE: &str = "last_region.txt";

/// 这一趟进程许不许碰那份文件。关着的默认是**这一整轮的立足点**：遮罩、托盘、热键、
/// `--snap` 那三枚量具档、`cargo test` 与 `--selftest` 全都一次也不动盘，它们眼里的
/// 「上一次区域」自 P169 起仍是 P169 之前那一份进程内记忆。只有 `main` 在
/// [`cli::Decision::Run`] 那一支上打开它 —— 也就是 PRD §5.17 说的「脚本、快捷方式和第三方
/// 程序」真的点名了一枚截图动词的那一趟。
static STORE_ON: AtomicBool = AtomicBool::new(false);

/// [`forget_region`] 留下的那枚闸：本进程不再从盘上读上一次区域。
static DISK_MUTED: AtomicBool = AtomicBool::new(false);

pub fn enable_region_store() {
    STORE_ON.store(true, Ordering::SeqCst);
}

pub fn store_enabled() -> bool {
    STORE_ON.load(Ordering::SeqCst)
}

/// 那份文件住哪。落点的**那一条决定**在 [`dir_in`] 里，这里只负责去问两位主人 —— 单测碰得到
/// 前者，碰不到 env（`cargo test` 是多线程的，改进程环境是另一码事）。
pub fn state_path() -> PathBuf {
    let data = crate::settings::with(|s| falcon_core::config::data_dir(&s.config));
    dir_in(std::env::var_os(ENV_DIR).as_deref(), &data).join(STATE_FILE)
}

/// 环境变量给了非空就用它，否则用 `advanced.data_dir` 答的那枚目录。与 `guide.rs` 同一条规矩，
/// 拆成纯函数是为了让「门把落点换到自己那一枚目录里」这一件 §9.1 第十五条要求的事，有一次
/// 不带盘的证人。
fn dir_in(env: Option<&OsStr>, data: &Path) -> PathBuf {
    match env {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => data.to_path_buf(),
    }
}

/// 盘上那一行的写法：`x,y,w,h|leg`。四个整数那半**就是** [`rect_arg`] 的语法，不是第二份
/// 解析器 —— 于是 `w=0`、越界、少一个数这些写法天生读不回来，而多监视器上那个负数原点天生
/// 读得回来（P168 第 5 腿守的就是这一件）。
pub fn encode_line(last: &LastRegion) -> String {
    format!(
        "{},{},{},{}|{}",
        last.rect.x, last.rect.y, last.rect.w, last.rect.h, last.leg
    )
}

/// [`encode_line`] 的反向，只认**第一行**：一份被谁追加过点的文件读回的是当初那一行，
/// 而不是最后那一行。认不出的写法交回 `None`，也就是「还没有截图过」—— 存档坏了不冒充
/// 一次成功，也不许把一次真截图拖成崩溃。
pub fn decode_line(text: &str) -> Option<LastRegion> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let (nums, leg) = line.split_once('|')?;
    if leg.is_empty() {
        return None;
    }
    let rect = rect_arg(STATE_FILE, nums).ok()?;
    Some(LastRegion {
        rect,
        leg: leg.to_string(),
    })
}

/// 盘上那一行现在写着什么；不存在 / 读不开 = `None`（新用户，或上一趟没留下东西）。
fn read_at(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// 写那一行。先写同名 `.tmp` 再改名，与 `guide.rs` 的 `write_at` 同一条理由：一份读到一半
/// 被截断的存档比不写更坏 —— 它会教下一趟进程去重复一个从未存在过的矩形。
fn save_at(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let name = path
        .file_name()
        .ok_or_else(|| "这一枚路径连文件名都没有".to_string())?;
    let tmp = path.with_file_name(format!("{}.tmp", name.to_string_lossy()));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// One rectangle read again, on a frame taken **now**.
///
/// [`last_frame`]'s middle, with the rectangle handed in rather than remembered — the two
/// flows that ask for a re-read (§5.2.5's 重复上次截图 and §5.5.8's 刷新截图) differ in which
/// rectangle they name and in what they call the line, and they agree on everything else,
/// down to the one reason neither of them may use [`working_frame`]:
///
/// Every other read of the freeze exists to avoid re-photographing a desktop that has not
/// changed; a re-read is the one flow whose whole point is that the desktop *has* changed
/// under a region that has not. Reusing the cached frame would hand back the same pixels and
/// call it a second look.
///
/// `what` is the phrase the line opens with, because «重复窗口的上一次区域» and «刷新贴图 3»
/// are two sentences about two different things and neither is this function's to decide.
/// A region that no longer fits is clipped here and the clipping is said in that line, which
/// is what §5.2.5 asks for ("自动裁剪到有效范围并提示"); §5.5.8 refuses the same case before
/// reaching here, and [`falcon_core::refresh`] says why the two must not share an answer.
pub fn re_read(rect: &PhysRect, what: &str) -> Result<(Frame, PhysRect, String), String> {
    let f = freeze()?;
    let cap = f.snap.capture(rect).map_err(|e| e.to_string())?;
    Ok((
        cap.frame,
        cap.rect,
        format!(
            "{} {:?}：{} 的 {} ms 新冻结{}",
            what,
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

/// §5.2.5: the last region again, on a frame taken **now**.
///
/// `working_frame` is deliberately not used here — the reason is written on [`re_read`],
/// and it is the reason this function exists apart from the mask's ordinary reads: a repeat
/// that reused the cached freeze would return the picture the user already had.
///
/// The region it reads is [`last_region_across_processes`], i.e. this process's memory and,
/// when that is empty, the line the previous §5.17 run left on disk. That fallback is the
/// whole of P169: PRD §5.17 lists 截取上一次区域 among what a *script* may ask for, and a
/// script's call is always a new process — while `last_region`'s memory used to die with
/// each one, so `--last` could only ever answer 「还没有截图过」 from a terminal.
///
/// The frame owner behind both §5.17's output arms: [`pin_last_region`] pins what this
/// returns, and `-o clipboard` / `-o file` hand the same `(frame, rect, line)` to
/// [`crate::platform_windows::clip`] or [`crate::save`] without opening a window. It
/// does NOT `remember` — that stays in [`pin_last_region`], so a repeat that ends on the
/// clipboard does not re-leg the memory as "重复" out from under a pin that has to keep
/// the original geometry. The freeze here is deliberate and shared: one new photograph,
/// three possible destinations.
pub fn last_frame() -> Result<(Frame, PhysRect, String), String> {
    let last = last_region_across_processes().ok_or("还没有截图过：先把要重复的范围截出来")?;
    re_read(&last.rect, &format!("重复{}的上一次区域", last.leg))
}

/// §5.2.5: the last region again, as a pin — [`last_frame`]'s picture presented.
pub fn pin_last_region() -> Result<(PinId, String), String> {
    let (frame, rect, line) = last_frame()?;
    let id = state::with(|s| s.add_captured(frame, PhysPoint::new(rect.x, rect.y)))?;
    remember(rect, REPEAT_LEG);
    Ok((id, line))
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
/// as its own picture — the frame owner, with no window opened.
///
/// Split from [`pin_active_window`] so §5.17's `-o clipboard` / `-o file` can reach the
/// same DWM content the pin reaches. The refusal table is [`active_window_refusal`]'s and
/// runs before any pixels are asked for, so a protected or minimised foreground is a
/// clean `Err` on this path too. Like [`last_frame`] it does not `remember`: legging the
/// memory stays with the pin, so a clipboard copy of the active window cannot silently
/// change what a later §5.2.5 repeat pulls up.
pub fn active_frame() -> Result<(Frame, PhysRect, String), String> {
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
    let (frame, rect) = window_frame(hwnd as u32)?;
    let title = window_title(hwnd).unwrap_or_default();
    Ok((
        frame,
        rect,
        format!("活动窗口「{title}」[{}] {hwnd:#x}", app.unwrap_or_default()),
    ))
}

/// §5.2.6 as a pin — [`active_frame`]'s picture presented.
pub fn pin_active_window() -> Result<(PinId, String), String> {
    let (frame, rect, line) = active_frame()?;
    remember(rect, "窗口");
    let id = state::with(|s| s.add_captured(frame, PhysPoint::new(rect.x, rect.y)))?;
    Ok((id, line))
}

/// §5.3's output for one rectangle: stitched across monitors and clipped to the
/// desktop, as pixels rather than as a window.
///
/// The frame owner for a named rectangle: [`pin_rect`] presents it, and §5.17's
/// `-o clipboard` / `-o file` hand the same `(frame, rect)` to the clipboard or the
/// encoder. It `remember`s here (before any presenter runs) because this is the leg that
/// decides "the region that just became a picture" — a `--region` that lands on the
/// clipboard has earned a §5.2.5 repeat exactly as much as one that lands on the desktop.
/// The reuse of `working_frame` (a fresh freeze only if none is cached) is preserved
/// unchanged from the pin path, so `--snap`/`--rect` timing is not this function's to
/// alter.
///
/// P165 adds the third slot: core already knew the request did not fit (`Captured::clipped`,
/// `crates/core/src/capture.rs:140`) and this function used to drop that bool on the floor. It is the
/// difference between 「自动修正」 and 「提示」 in PRD §5.3.5's 校验规则, and only the presenter can say
/// which one the user got.
pub fn region_frame(want: &PhysRect) -> Result<(Frame, PhysRect, bool), String> {
    let snap = working_frame()?;
    let cap = snap.capture(want).map_err(|e| e.to_string())?;
    remember(cap.rect, "矩形");
    Ok((cap.frame, cap.rect, cap.clipped))
}

/// PRD §5.3.5's 「坐标超出屏幕时应提示或自动修正」 in one sentence: the clipping is the 自动修正 and has
/// always happened, and this is the 提示 half — the two rectangles named side by side, so a line that
/// reads `区域 …296×400…` for a request of `…400×400…` is not something the user has to diff by eye.
/// A request that fit keeps the old sentence exactly: a region that needed no correcting gets no
/// second half, which is what makes the second half mean something when it appears.
pub fn region_line(want: &PhysRect, got: &PhysRect, clipped: bool) -> String {
    if clipped {
        format!("区域 {got:?}（{}）", clip_note(want))
    } else {
        format!("区域 {got:?}")
    }
}

/// The 提示 half's one and only wording, so no presenter can paraphrase the accusation away. Every
/// caller that displays a clipped region reaches it, either inside [`region_line`]'s parentheses or,
/// for the leg whose sentence belongs to `save`, as a whole sentence of its own.
pub fn clip_note(want: &PhysRect) -> String {
    format!("你要的是 {want:?}，出桌面的那几边已裁掉")
}

/// [`clip_note`] as the file leg can attach it: `save` owns 「已保存到 …（截图 W×H 像素）。」 and the
/// correction has to arrive after that period, not inside it. Empty when nothing was clipped, so the
/// region that fit stays a one-sentence line — the second sentence is what makes it a report.
pub fn clip_tail(want: &PhysRect, clipped: bool) -> String {
    if clipped {
        format!("{}。", clip_note(want))
    } else {
        String::new()
    }
}

/// One rectangle, pinned where it was on screen — [`region_frame`]'s pixels presented.
///
/// Also the region `capture_repeat` will ask for next time (§5.2.5): what is
/// remembered is `cap.rect`, the crop that actually became a picture, rather than
/// `want`. A request that ran off the edge of the desktop has already been answered
/// once by clipping it, and repeating the answer is what the user saw.
///
/// It hands back the sentence too, for the same reason [`region_frame`] hands back the bool: every
/// presenter that used to build its own line had a `want` in scope and printed that, so a clipped
/// region reported a rectangle that never became a picture.
pub fn pin_rect(want: &PhysRect) -> Result<(PinId, String), String> {
    let (frame, rect, clipped) = region_frame(want)?;
    let id = state::with(|s| s.add_captured(frame, PhysPoint::new(rect.x, rect.y)))?;
    Ok((id, region_line(want, &rect, clipped)))
}

/// 一块抓出来的区域成为贴图时，说给它听的那一句的**唯一作者**。
///
/// P166 之前这一句长在 `main.rs` 的 `cli_capture_to_pin` 那一格里，连同它那半句「（关掉那扇窗
/// 即退出）」—— 那半句说的是命令行那一路只钉一张、钉完就等人关窗，不属于这一句的本分。对话框
/// （PRD 第 302 行那一枚「开始截图」）是这一句的第二位读者，而它要的正是前面那半句：谁钉的、
/// 钉在哪一块。把前面那半句抽到这里、尾巴留给调用方，两位读者说的就是同一句话，而「贴图 {id}
/// 已钉：」这几个字不会出现第二种写法。
///
/// P167 把这一句的读者从两位扩到五位：`cli_capture_to_pin` 的三臂（`--region` / `--last` /
/// `--active`）自此都过这里，`--last` 与 `--active` 此前把 `id` 丢在 `|(_, line)|` 里、终端上因此
/// 从来没说过刚钉的是哪一枚；`--snap` 那一档的打印行也从「自己另写一句 `snapped pin {id}`」改成
/// 递到这里来。`--paste` 不在这五位里：它钉的是剪贴板里来的一张，手里没有区域可描述，硬套这一句
/// 只会得到一枚冒号后面空着的句子。
pub fn pinned_line(id: PinId, line: &str) -> String {
    format!("贴图 {id} 已钉：{line}")
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
    // The line is dropped here and not because it does not matter: [`centre_rect`] builds its rect out
    // of the primary monitor's own bounds, so this leg cannot arrive at `pin_rect` with a region that
    // ran off the desktop. The two presenters that can take a typed rectangle print it.
    let (id, _) = pin_rect(&want)?;
    Ok((
        id,
        format!("{} ms, {:?} from {}", f.ms, want, snapshot_line(&f.snap)),
    ))
}

/// PRD 第 307 行「宽度和高度必须大于 0」这一句的**唯一作者**。
///
/// P165 把它写在 [`rect_arg`] 里面，那是对的 —— 那时命令行是它唯一的读者。P166 的「自定义截图」
/// 对话框（PRD 第 298-303 行）是第二位读者，而它手里已经是一拆开的四格整数、不是 `x,y,w,h` 那样
/// 一串字符：让对话框照着这一句再写一遍，就是 #281 的形状 —— 同一枚拒绝在两处各有措辞，改一处
/// 就留下另一处还在说旧话。所以这里收的是一枚**名字**（谁给的数）与那两个数，命令行进来的那串
/// 字符仍旧由 [`rect_arg`] 切完之后交进来，两句字节与 P165 记下的逐字相同。
pub fn size_guard(what: &str, w: i32, h: i32) -> Result<(), String> {
    if w <= 0 || h <= 0 {
        return Err(format!(
            "{what} 的宽度和高度必须大于 0（给的是 w={w}, h={h}）：0 或负数不是一块可截取的区域"
        ));
    }
    Ok(())
}

/// A rectangle in the shape `--rect x,y,w,h` / `--region x,y,w,h` carries.
///
/// PRD §5.3.5's 校验规则 asks 「宽度和高度必须大于 0」, and until P165 this function answered that by
/// rewriting the number: `n[2].max(1)` turned `--region 5,6,0,-40` into a real 1×1 picture and an exit
/// code of 0, so the typo produced a screenshot rather than an answer. A negative did worse than lie —
/// `-40 as u32` is 4294967256, and only the saturating `right()` kept it from looking like a width.
/// Refusing, and naming the flag that was typed and the pair that broke, is the reading the line asks
/// for; the caller no longer has to guess which of its two verbs the sentence belongs to.
pub fn rect_arg(flag: &str, raw: &str) -> Result<PhysRect, String> {
    let parsed: Option<Vec<i32>> = raw
        .split(',')
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().parse::<i32>().ok())
        .collect();
    let n = match parsed {
        Some(n) if n.len() == 4 => n,
        _ => {
            return Err(format!(
                "{flag} 后面要跟四个整数 x,y,w,h（例如 `{flag} 10,20,300,400`）"
            ));
        }
    };
    size_guard(flag, n[2], n[3])?;
    Ok(PhysRect::new(n[0], n[1], n[2] as u32, n[3] as u32))
}

/// A whole window as its own picture. This path does not use the freeze: a window
/// shot asks the DWM for that window's content, which is what makes it survive
/// being behind another window - and what makes its size drift (P5), hence the
/// re-crop inside `capture_window`.
///
/// The re-cropped bounds are what goes into §5.2.5's memory, not the window's
/// geometry as the shell reported it: the picture the user got is the region they
/// will ask to see again.
///
/// The DWM read itself, with no window opened and no memory written — [`pin_window`]
/// and [`active_frame`] are its two presenters, so §5.17's clipboard/file arms and a hot
/// key all ask the shell the same one question about a handle.
pub fn window_frame(hwnd: u32) -> Result<(Frame, PhysRect), String> {
    let cap = service().capture_window(hwnd).map_err(|e| e.to_string())?;
    Ok((cap.frame, cap.rect))
}

/// A whole window as its own pin — [`window_frame`]'s picture presented.
pub fn pin_window(hwnd: u32) -> Result<PinId, String> {
    let (frame, rect) = window_frame(hwnd)?;
    remember(rect, "窗口");
    state::with(|s| s.add_captured(frame, PhysPoint::new(rect.x, rect.y)))
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
            rect_arg("--rect", "10,20,300,400"),
            Ok(PhysRect::new(10, 20, 300, 400))
        );
        // Spaces are typed by humans; three numbers is a typo, and a typo that
        // silently becomes a rect is worse than a rect that is refused.
        assert_eq!(
            rect_arg("--region", " 10 , 20 , 300 , 400 "),
            Ok(PhysRect::new(10, 20, 300, 400))
        );
        for bad in ["10,20,300", "10,20,300,abc", ""] {
            let why = rect_arg("--region", bad).unwrap_err();
            assert!(
                why.contains("--region"),
                "{why} does not name the flag typed"
            );
            assert!(why.contains("四个整数"), "{why}");
            // The other verb must not be able to borrow this sentence.
            assert!(!why.contains("--rect"), "{why}");
        }
        assert!(rect_arg("--rect", "1,2,3,4,5").is_err());
    }

    /// §5.3.5 校验规则第一行「宽度和高度必须大于 0」，自 P165 起是一次拒绝而不是一次改写。
    /// The old behaviour is the thing this test exists to keep away: `5,6,0,-40` used to answer with a
    /// real 1×1 rectangle (and exit 0), which reads to the user as "that little square is what you
    /// asked for". A refusal has to name the pair that broke, or the user is hunting through four
    /// numbers they just typed.
    #[test]
    fn a_zero_or_negative_size_is_refused_by_the_name_that_was_typed() {
        for (flag, raw, w, h) in [
            ("--region", "5,6,0,-40", 0, -40),
            ("--region", "5,6,0,40", 0, 40),
            ("--rect", "5,6,3,0", 3, 0),
            ("--region", "5,6,-1,-1", -1, -1),
        ] {
            let why = rect_arg(flag, raw).unwrap_err();
            assert!(why.contains(flag), "{why} 没点名 {flag}");
            assert!(why.contains("必须大于 0"), "{why}");
            assert!(why.contains(&format!("w={w}")), "{why} 没念出 w={w}");
            assert!(why.contains(&format!("h={h}")), "{why} 没念出 h={h}");
        }
        // One is the smallest legal region, and it stays legal: the refusal is about 0 and below, not
        // about "suspiciously small", or a one-pixel pin would become impossible to ask for.
        assert_eq!(
            rect_arg("--region", "5,6,1,1"),
            Ok(PhysRect::new(5, 6, 1, 1))
        );
        assert!(rect_arg("--region", "5,6,1,0").is_err());
    }

    /// P169：盘上那一行的写法。`cargo test` 不许改进程环境（多线程），所以这里问的是**写法
    /// 本身** —— 谁写谁读、坏写法一律读不回东西；「这一趟落在哪枚目录」那一格归终端门。
    #[test]
    fn the_line_on_disk_is_the_same_grammar_as_a_region_argument() {
        let last = LastRegion {
            rect: PhysRect::new(10, 20, 300, 400),
            leg: "矩形".to_string(),
        };
        assert_eq!(encode_line(&last), "10,20,300,400|矩形");
        assert_eq!(decode_line("10,20,300,400|矩形"), Some(last.clone()));
        assert_eq!(decode_line(&encode_line(&last)), Some(last.clone()));
        // 尾随换行是新写的文件常带的那一枚,它不是第二行,也不改变第一行。
        assert_eq!(decode_line("10,20,300,400|矩形\n"), Some(last));
        // 多监视器上负数是合法的矩形（P168 第 5 腿守的就是这一件）,存档不许把它读丢。
        let neg = decode_line("-100,-50,400,300|窗口").unwrap();
        assert_eq!(neg.rect, PhysRect::new(-100, -50, 400, 300));
        assert_eq!(neg.leg, "窗口");
        for bad in [
            "",
            "   ",
            "10,20,300,400",
            "10,20,300|矩形",
            "10,20,0,40|矩形",
            "10,20,300,abc|矩形",
            "10,20,300,400|",
            "|矩形",
            "1,2,3,4,5|矩形",
            "x,y,w,h|矩形",
        ] {
            assert_eq!(decode_line(bad), None, "{bad} 被读成了一次上一次区域");
        }
        // 只认第一行：一份被谁追加过点的文件,读回的是当初那一行,不是最后那一行。
        let two = decode_line("10,20,300,400|矩形\n7,8,9,10|窗口").unwrap();
        assert_eq!(two.rect, PhysRect::new(10, 20, 300, 400));
        assert_eq!(two.leg, "矩形");
        // 名字里带 '|' 不改语法 —— 矩形那半不可能含竖线,所以竖线后面整段都是名字。
        let odd = decode_line("1,2,3,4|重复|窗口").unwrap();
        assert_eq!(odd.leg, "重复|窗口");
    }

    /// P169：哪一次 remember 值得让下一趟进程也看见。空矩形跟着内存那条老规矩走,而「重复」
    /// 这一腿不写盘 —— 它读的就是这一行,把"重复"自己的名字抄回去,第三趟进程就会说出
    /// «重复重复的上一次区域»,而那句里第一个「重复」已经不是在说用户的手势了。
    #[test]
    fn a_repeat_re_legs_the_memory_but_not_the_disk() {
        let rect = PhysRect::new(10, 20, 300, 400);
        for leg in ["矩形", "窗口", "遮罩选区"] {
            assert!(
                worth_persisting(&LastRegion {
                    rect,
                    leg: leg.to_string()
                }),
                "{leg} 那一腿截出来的区域该让下一趟进程也看得见"
            );
        }
        assert!(!worth_persisting(&LastRegion {
            rect,
            leg: REPEAT_LEG.to_string()
        }));
        assert!(!worth_persisting(&LastRegion {
            rect: PhysRect::new(10, 20, 0, 40),
            leg: "矩形".to_string()
        }));
        // 「重复」这一枚名字只有一处拼写,而上面那条规则的全部依据就是它。
        assert_eq!(REPEAT_LEG, "重复");
        assert_eq!(
            encode_line(&LastRegion {
                rect,
                leg: REPEAT_LEG.to_string()
            }),
            "10,20,300,400|重复"
        );
    }

    /// 落点那一条决定是纯的：[`ENV_DIR`] 给了非空就用它,否则用 `advanced.data_dir` 答的那枚
    /// 目录。这一格是 §9.1 第十五条的地基 —— 门换得了自己的落点,才谈得上一次也不碰用户那一份。
    #[test]
    fn a_door_can_point_the_state_somewhere_else() {
        let data = Path::new("C:/users/me/AppData/Roaming/ai-falconshot");
        assert_eq!(dir_in(None, data), data.to_path_buf());
        assert_eq!(dir_in(Some(OsStr::new("")), data), data.to_path_buf());
        assert_eq!(
            dir_in(Some(OsStr::new("D:/tmp/p169state_A")), data),
            PathBuf::from("D:/tmp/p169state_A")
        );
        // 换到哪一枚目录,那份文件的名字都不跟着换。
        assert_eq!(
            state_path().file_name().and_then(|n| n.to_str()),
            Some(STATE_FILE)
        );
    }

    /// 那份文件真的写得出去、读得回来,而半行存档不会冒充一次成功。写的是 `%TEMP%` 下本档自己
    /// 那一枚名字 —— 用户 `%APPDATA%\\ai-falconshot` 里那一份不是 `cargo test` 的草稿纸。
    #[test]
    fn a_saved_line_survives_a_second_reading() {
        let dir = std::env::temp_dir().join(format!("p169state_{}", std::process::id()));
        let file = dir.join(STATE_FILE);
        let last = LastRegion {
            rect: PhysRect::new(100, 120, 320, 240),
            leg: "矩形".to_string(),
        };
        save_at(&file, &encode_line(&last)).unwrap();
        assert_eq!(
            read_at(&file).and_then(|text| decode_line(&text)),
            Some(last.clone())
        );
        // 截断的存档读不回一枚矩形,而不是读回一枚"看起来对"的。
        std::fs::write(&file, "100,120,32").unwrap();
        assert_eq!(read_at(&file).and_then(|text| decode_line(&text)), None);
        // 没有文件 = 新用户,不是坏。
        std::fs::remove_file(&file).unwrap();
        assert_eq!(read_at(&file), None);
        assert!(std::fs::remove_dir(&dir).is_ok());
    }

    /// §5.3.5 校验规则第二行「坐标超出屏幕时应提示或自动修正」的两半，在这里各自有一句话可问：
    /// 裁没裁是 `clipped` 那一格，说了没有是这一格。`region_line` is a pure function precisely so the
    /// sentence can be graded without a desktop — the real-machine door then asks whether the clipped
    /// case ever reaches a user's terminal at all.
    #[test]
    fn a_clipped_region_says_what_was_asked_and_a_whole_one_says_nothing_more() {
        let want = PhysRect::new(1500, 900, 400, 400);
        let got = PhysRect::new(1500, 900, 36, 60);
        let clipped = region_line(&want, &got, true);
        assert!(clipped.starts_with("区域 "), "{clipped}");
        assert!(
            clipped.contains(&format!("{got:?}")),
            "{clipped} 没先说落下的那一格"
        );
        assert!(
            clipped.contains(&format!("{want:?}")),
            "{clipped} 没说你要的那一格"
        );
        assert!(clipped.contains("裁"), "{clipped} 没说这一格是被裁过的");
        // The order is not cosmetic: the picture the user got is what the line opens with.
        assert!(
            clipped.find(&format!("{got:?}")).unwrap()
                < clipped.find(&format!("{want:?}")).unwrap(),
            "{clipped}"
        );
        // A region that fit is not given a second half to read past.
        let same = PhysRect::new(10, 20, 300, 400);
        assert_eq!(region_line(&same, &same, false), format!("区域 {same:?}"));
        assert!(!region_line(&same, &same, false).contains("你要的是"));
        // The file leg's tail is the same wording with a period on it, and the same emptiness: one
        // phrase, two places that may say it. A tail that added something to a region that fit would
        // make 「已保存到 …」 ambiguous about whether anything moved.
        let tail = clip_tail(&want, true);
        assert!(
            tail.ends_with('。') && tail.starts_with("你要的是"),
            "{tail}"
        );
        assert_eq!(tail, format!("{}。", clip_note(&want)));
        assert!(
            tail.contains(&format!("{want:?}")),
            "{tail} 没点名要的那一格"
        );
        assert_eq!(clip_tail(&same, false), "");
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
