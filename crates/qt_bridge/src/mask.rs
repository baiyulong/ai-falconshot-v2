//! The capture mask's live state: which screens it covers, the one frozen frame
//! each is showing, and where the hole is.
//!
//! Two rules decide the shape of this module.
//!
//! The first is [`crate::capture`]'s: the screen is read **once**, before any mask
//! window exists, and every later question - the crop, the colour pick, the
//! magnifier - is answered from that snapshot. So `open` takes the frames it
//! publishes rather than taking a capture of its own, and a flow that needs a
//! fresh picture calls `capture::freeze` first.
//!
//! The second is that a mask is *not* a pin. A pin is a picture the user keeps,
//! addressed by a `PinId`; a mask is a transient full-screen texture addressed by
//! a key, and there is one per screen rather than one per picture. That is why the
//! provider gets a second store keyed by string (`pinIdOf` reads anything it
//! cannot parse as -1, so negative ids are not addressable and a mask has no id at
//! all), and why the slots live here instead of in [`crate::state`].
//!
//! Geometry arrives in two spaces on purpose: the hole is in physical desktop
//! pixels, because that is the space the selection and the crop are decided in,
//! and each slot converts its part of it into the window-local
//! device-independent pixels QML binds to. On a 200% screen the conversion is
//! where a mask that looks right and crops wrong gets caught.

use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use falcon_core::capture::ScreenSnapshot;
use falcon_core::encode;
use falcon_core::geometry::{
    handle_at, nudge, nudge_edge, resize, Handle, PhysPoint, PhysRect, Scale, DEFAULT_HIT_SLOP,
};
use falcon_core::naming;

use crate::annotate;
use crate::capture;
use crate::mask_view::shim;

/// The six modes that run a gauge rather than a user. `main.rs` refuses one without its
/// number, and [`MaskState::gauge_run`] recognises all of them - and the two readers must
/// not be two copied lists, because a mode added to the first while the second still
/// names the earlier set would open an Explorer window no probe can close, silently, on a
/// machine the user is sitting at. `--hint` joined for exactly that reason: it is a door
/// that opens a window, and the door after it is the one that decides whether 落盘 goes to
/// Explorer.
pub(crate) const GAUGE_FLAGS: [&str; 6] =
    ["--probe", "--r13", "--mask", "--ink", "--pin", "--hint"];

/// How far the desktop is pushed toward black outside the selection. One number,
/// because QML draws it, the shader multiplies by it and `--mask` asserts a
/// readback against `1 - DIM` - three places that have to disagree by design.
pub const DIM: f64 = 0.6;

/// Which screen a confirmed selection came from, as an index into the pairs it is
/// given - `(monitor name, physical bounds)`, the two things the question reads.
///
/// §6.2 gives `history.db` one `monitor_id` column while §5.2.5 lets one drag cross
/// a screen boundary, so "which screen" is not a yes/no per slot but a widest
/// overlap. Ties go to the earlier slot, and the list is Qt's screen order, so the
/// answer is deterministic rather than whichever enumerate landed first.
///
/// `None` is not "the user picked nothing" - the caller only asks this once a hole
/// exists. It is "no slot I was given overlaps that hole", which happens when the
/// slots handed to the door are not the ones the desktop is made of (a hot unplug
/// between the freeze and the commit).
///
/// Pure for the same reason as P67's `tray_from` and P68's `history_open_in`: this
/// is the one branch of the history flow a `cargo test` can ask without freezing a
/// desktop, opening SQLite, or touching the disk.
pub fn monitor_slot_for(screens: &[(String, PhysRect)], hole: PhysRect) -> Option<usize> {
    let mut best: Option<(u64, usize)> = None;
    for (i, (_, bounds)) in screens.iter().enumerate() {
        let Some(over) = bounds.intersection(&hole) else {
            continue;
        };
        if best.map(|(area, _)| over.area() > area).unwrap_or(true) {
            best = Some((over.area(), i));
        }
    }
    best.map(|(_, i)| i)
}

/// The row [`monitor_slot_for`] says this hole owes, before anybody writes it.
///
/// `source` is [`falcon_core::history::Source::Region`] because this is the only door
/// a dragged selection confirms: the window-click family (`Source::Window`) commits
/// somewhere else and is not borrowed a label here.
///
/// `cancelled` is **not** answered here — the caller hands it in. P93 pinned it to
/// `false` and left §5.14.1's second rule ("a selection cancelled with Esc is still a
/// record, flagged rather than dropped") unread, because the door that confirms and the
/// ladder that escapes are two different callers of one writer. Pinning it here made
/// the escape ladder look unimplementable when it was only un-wired; the escape rung
/// now passes `true` and [`esc_owes_row`] decides whether it passes at all.
pub fn capture_meta_for<'a>(
    screens: &'a [(String, PhysRect)],
    hole: PhysRect,
    at_ms: i64,
    offset_min: i32,
    cancelled: bool,
) -> falcon_core::history::CaptureMeta<'a> {
    let monitor_id = monitor_slot_for(screens, hole)
        .map(|i| screens[i].0.as_str())
        .unwrap_or("");
    falcon_core::history::CaptureMeta {
        at_ms,
        offset_min,
        monitor_id,
        rect: hole,
        source: falcon_core::history::Source::Region,
        cancelled,
    }
}

/// Whether an Esc that just cleared a selection owes the history a row - §5.14.1's
/// second rule ("保存所有截图记录，包括被取消的截图") read off the two facts this
/// ladder actually has: what the switch said, and whether there was anything to cancel.
///
/// `enabled` is deliberately absent. That half of the door has had exactly one owner
/// since P68 ([`crate::settings::history_open_in`], which answers `Ok(None)` for a
/// disabled history), and [`MaskState::record_history`] walks through it; deciding here
/// too would be a second owner for one switch, and the two would diverge with nothing
/// readable to tell them apart.
///
/// Pure, and for the same reason as P93's two above it: this is the branch that decides
/// whether an Escape costs a disk write, and `cargo test` can ask it about all four
/// shapes without opening SQLite or freezing a desktop.
pub fn esc_owes_row(save_all: bool, had_selection: bool) -> bool {
    save_all && had_selection
}

/// A screen the mask covers.
#[derive(Clone, Debug)]
pub struct Slot {
    /// Index into Qt's screen list, which is what the QML `Instantiator` repeats.
    pub index: usize,
    /// The capture layer's name for the same monitor, for the report to line up.
    pub name: String,
    /// Provider key, `name-revision`.
    pub key: String,
    /// Physical bounds, from the snapshot - the space the hole lives in.
    pub bounds: PhysRect,
    /// The same screen in device-independent pixels, from Qt - the space a
    /// `Window`'s x/y/width/height live in.
    pub geom: PhysRect,
    pub scale: Scale,
    pub revision: u32,
    pub swaps: u32,
    /// Milliseconds from `open` to this window's first presented frame, which is
    /// where the GPU warm-up the spike measured at 62-117 ms shows up. `None`
    /// until a frame has been presented.
    pub first_swap_ms: Option<u64>,
    /// `ShaderEffect.status`, once QML has reported it. `None` means the item was
    /// never constructed, which is a different fact from `Error`.
    pub shader_status: Option<i32>,
}

/// What one mask window binds to. Plain fields so a test can read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaskViewData {
    /// False once the mask is closed - the window hides itself.
    pub live: bool,
    /// The user is looking at a mask right now.
    pub shown: bool,
    pub geom: PhysRect,
    /// The selection, in window-local DIP. Empty when the hole is on another
    /// screen, which is how a two-monitor mask draws nothing rather than a band.
    pub hole: PhysRect,
    pub scale: Scale,
    /// §5.7.11's 文本 field: the box the layer is holding a string in, in window-local
    /// DIP, and whether there is one at all. QML owns the field and Rust owns the box,
    /// because the number that sizes it comes from the font leg - and a field whose
    /// width is QML's guess about somebody else's text is how a typed line lands outside
    /// the box it was measured in.
    pub typing: PhysRect,
    pub typing_live: bool,
    /// The pen's 字号 in DIP, for the same reason and by the same division.
    pub font_dip: u32,
    pub revision: u32,
    pub swaps: u32,
    pub first_swap_ms: Option<u64>,
    pub shader_status: Option<i32>,
    /// §5.3.5 —— 这一格是"这一张遮罩要不要显示选区尺寸"，从 [`MaskState::show_dimensions`] 抄过来。
    /// 单独放在这里而不复用 [`MaskState::show_dimensions`]，是因为 QML 只拿 `MaskViewData` 而不
    /// 直接读 `MaskState`；跨这一层加一枚 bool 而不是新增一次 lock，成本与 [`Self::shown`] 同量级。
    pub show_dimensions: bool,
}

/// What the pointer took hold of, in physical desktop pixels.
#[derive(Clone, Copy, Debug)]
pub struct Grab {
    /// `None` is not "nothing held" - it is "a rectangle starts here", which is the
    /// only grip that can create a selection. `Some(Body)` moves the whole rect,
    /// and the other eight resize it (§5.3.1's "拖动边缘或角点调整选区").
    pub handle: Option<Handle>,
    /// The press point: the fixed corner of a new rectangle, and the reference for
    /// a move.
    pub anchor: PhysPoint,
    /// The rect the grab began from. Resizing from `start` rather than from the
    /// current hole is what stops a long drag from accumulating rounding.
    pub start: PhysRect,
}

#[derive(Default)]
pub struct MaskState {
    pub slots: Vec<Slot>,
    /// The selection in physical desktop pixels.
    pub hole: PhysRect,
    /// §5.7's annotations, drawn on top of this flow's frozen frames. Its base is
    /// the same snapshot the slots' textures came from, so ink and picture cannot
    /// be describing two different captures.
    pub ink: annotate::Layer,
    /// The pointer's grip since the last `press`, or `None` when it is not held.
    /// Kept here rather than in QML because the arithmetic it feeds - which edge
    /// moved, and how far - is the part that can be tested without a window.
    pub grab: Option<Grab>,
    /// §5.4.3 的那一枚光标覆盖层，形状是 `(图, 图左上角落在屏幕上的物理坐标)`。
    ///
    /// `None` 有两条来历，本轮**故意不分成两枚字段**：`[capture] include_cursor = false`
    /// （默认那一侧，`crates/core/src/config.rs:187`），和开着键而平台没答出这枚指针
    /// （`WinExtras::cursor_image` 走 `crates/platform-windows/src/cursor.rs`，那里 `None`
    /// 还剩「桌面此刻没亮着指针」与「GDI 那趟画不出图」两种）。分成两枚会把「键关着」写成
    /// 一种错误，而它其实是用户自己的选择。
    ///
    /// 还有一条 P106 装真臂时才露出来的口径，P107 关掉了：这一枚**不再是 `commit()` 那一刻读的**。
    /// 画面早在 [`crate::capture::freeze`] 就冻住了，而指针在屏幕上是走的 —— 在门上当场问，
    /// 贴上去的位置就是「按下完成时指针在哪」，而 §5.4.3 第 419 行「热点位置应与实际屏幕位置
    /// 一致」里的「实际位置」只能是图里那块屏幕，第 420 行更把延时那一臂的时刻点名成倒计时
    /// 结束。所以取的是**冻结那一格读到的**答案（[`crate::capture::frozen_cursor`]），门只
    /// 把它接进状态，不再自己问桌面。
    ///
    /// 它**存在状态里**而不是在 [`Self::delivered`] 现场问一次：那一道缝合被四扇门各问一次
    /// （复制 / 保存 / 快速保存 / 存完再钉图），而指针在屏幕上是会走的 —— 现场问 ⇒ 同一枚
    /// 选区交出四张长得不一样的图，正是这一整块代码从 P58 起一直在防的那族形状。
    pub cursor: Option<(falcon_core::frame::Frame, falcon_core::geometry::PhysPoint)>,
    /// True = one ShaderEffect pass; false = four dim `Rectangle`s, which is also
    /// what the `QT_QUICK_BACKEND=software` fallback has to be able to draw.
    pub shader: bool,
    pub shown: bool,
    /// Set at the end of `open`, so the swap timings mean "since the mask was
    /// asked for", not "since the process started".
    started: Option<Instant>,
    /// The whole desktop as the snapshot saw it, kept for §5.2.5's clipping and
    /// for `desktop()` - a fold over the slots would count the origin as desktop
    /// whenever the monitors do not start there.
    pub virtual_bounds: PhysRect,
    /// Pixels handed to Qt, and how long that took - the part of the 150 ms
    /// promise that is this module's to keep.
    pub pixels_ms: Option<u64>,
    /// What the last delivery attempt said, in the user's own language, and `""`
    /// when the flow has nothing to report.
    ///
    /// It lives in Rust rather than in a return value for the same reason the hole
    /// does: two mask windows read this one state, so a sentence only the clicked
    /// window knew is a sentence the other screen never shows. It is cleared by
    /// `open`, so a fresh capture cannot start out wearing the last one's answer.
    pub message: String,
    /// §5.5.5's export knobs, taken at `open` for the same reason the pens are: the
    /// settings file is read while the mask is being put up, not in front of a user
    /// who has already waited for a save. `format` is what a name typed without an
    /// extension becomes, and `quality` is what JPG is written at.
    ///
    /// `Default` is `{Png, 92, true}`, which is what a first run reads out of the file
    /// anyway - so a test that never calls `open` saves at the same numbers a user
    /// with no `config.toml` does, rather than at quality 0.
    pub export: encode::EncodeOptions,
    /// §5.5.4's 快速保存 naming rule and directory, taken at the same `open` and for the
    /// same reason as [`Self::export`] - and the two have to be taken together, because
    /// one file name is built out of both: `%p`/`%Y`/`%i` from here, the extension from
    /// `export.format`. Read apart, a settings edit that lands mid-flow would name a
    /// file from one state of the file and encode it from another.
    pub naming: crate::settings::Naming,
    /// §5.5.6's rounded corners and §5.5.7's shadow and border, taken at the same
    /// `open` as [`Self::export`] and [`Self::naming`] - the three decide one file
    /// together, and a picture rounded as of *now* and encoded as of when the mask
    /// opened would be one output built out of two settings states.
    ///
    /// [`Self::delivered`] is the only thing that reads it, which is the reason the
    /// clipboard and the file are the same picture: every door asks the one question.
    pub decor: falcon_core::imageops::Decoration,
    /// How many files this flow has already quick-saved, so `%i` counts the series the
    /// user is watching rather than the process's lifetime. Not incremented by an
    /// attempt the settings refused: the number means "how many of these are on disk".
    ///
    /// Public for the gauge, which reads it to name the file the *next* press owes: a
    /// run that hardcoded `quick-001` would go red the moment another leg pressed the
    /// door first, and the sequence is the thing being graded, not one number.
    pub quick_saves: u32,
    /// §5.5.4 step 4 的通知要说的是**那一个文件**，所以路径得留下来。
    ///
    /// [`crate::save::quick_to`] 本来就把它交回来了（`Ok((PathBuf, String))`），是这一
    /// 台上一轮把它扔掉的：门当时只需要那句话。通知要能点，点的是路径，所以这一轮
    /// 把它接住。`None` 是"这一张还没有落到任何地方"，不是"落到了某个说不出的地方"。
    pub landed: Option<std::path::PathBuf>,
    /// `[output] reveal_after_save`，在 `open` 上和 [`Self::naming`] 同一刻取。
    ///
    /// 快照而不是点击时现读：这条开关管的是"这次保存要不要顺手把目录打开"，它属于
    /// 那张图，不属于手指按下去的那一秒——用户在流程中途改了配置，改的是**下一张**。
    pub reveal_after_save: bool,
    /// `[output] auto_save` —— §5.5.3 step 2 的那一格"截图后自动保存"，与
    /// [`Self::reveal_after_save`]、[`Self::naming`] 同一刻取，理由也是同一条：它属于
    /// 那张图，不属于手指按下去的那一秒。
    ///
    /// 这一格的默认值是 `false`（`config` 的 `Default` 就写着），所以"新加一扇门"这件事
    /// 对任何已经存在的一次按下的字节数都是零改变 —— 键以前在档里没人读，现在它有且只有
    /// 一个读者，而那个读者在开关关着时一步也不动。
    pub auto_save: bool,
    /// `[output] auto_copy` —— §5.5.1 step 3 那句"系统按默认行为**复制**、保存、贴图或关闭"
    /// 里的那一"按默认行为"。取的一刻与 [`Self::auto_save`]、[`Self::naming`] 相同。
    ///
    /// 与 `auto_save` 相反的一格要写明：这一枚的 `Default` 是 `true`，所以接上读者的那一刻
    /// 起，一次 `Enter` 就多递一张图进剪贴板 —— PRD 说这是默认行为，而 P59 之前整个仓库
    /// 没有一处读它（第 58 项 ① 那四枚没读者的键里的第一枚）。`MaskState` 自己的
    /// `Default` 仍是 `false`，那是给单测留的：§9.1 第十条那句"测试不许为了证明自己能拒绝
    /// 而去写用户的剪贴板"，挡它的是这一格而不是别人的手摆。
    pub auto_copy: bool,
    /// `[output] reuse_last_file` —— §5.5.8 那句"用户可选择**覆盖原图**或生成新图"里的
    /// "覆盖原图"。取的一刻与 [`Self::auto_copy`]、[`Self::auto_save`] 同一场 `open`。
    ///
    /// 语义本轮只接**同一次会话内的**复用：`self.landed` 在 `open` 时被清空（第 325 行
    /// "上一张图的通知不能跟着这一张活"那句），所以跨 mask 会话的"上一份"归 §5.5.8 另半，
    /// 仍未接。开关联动的是**同一次 mask 里连按两次快速保存**这一条：一次会话里第一次
    /// 走命名、第二次直接写到第一次落下的路径；`quick_saves` 仍递增，但 `self.landed`
    /// 不变，因此 `landed` 一直是那一份。默认 `false` ⇒ 已存在的每一次按下的字节零改变。
    pub reuse_last_file: bool,
    /// `[capture] show_dimensions` —— §5.3.5 那句"实时显示选区尺寸"（PRD §5.3.5）。取的一刻与
    /// [`Self::reuse_last_file`]、[`Self::auto_copy`] 同一场 `open`。
    ///
    /// 出厂 `true`（config `Default`），所以这一枚接到读者的那一刻起，遮罩多了一行 W×H 文字 ——
    /// 与 [`Self::auto_copy`] 那格同形："不是新加一扇没人按的门，是把 PRD 写着的默认行为接上"。
    /// `MaskState` 自己的 `Default` 是 `false`（`#[derive(Default)]` 给 bool 的），所以已有的
    /// 每一- 张 `MaskViewData` 单测在没设这枚位之前读到 `false` 而不是 `true`，画面不新增文字，
    /// 断言不变。真机上这一格由 `open` 从配置文件里取。
    pub show_dimensions: bool,
    /// 这一张图被**默认行为**复制成功的次数。一次 `Enter` 最多 +1；失败不吃次数也不加。
    ///
    /// 单独一格而不是复用 [`Self::quick_saves`] 那一族的计数，是因为"开关开着却一次也没
    /// 递进去"与"一次也没按过"是两件事，而第 58 项 ③ 那条口径（门要印它真读到的那个字段）
    /// 要求门能问出这两件事的差别。
    pub auto_copies: u32,
    /// 默认行为那一次复制的话，成功失败都写这里而不写 [`Self::message`]：同一刻的自动保存
    /// 也要说话，而 §5.5.3 的规则"保存失败时……保留剪贴板内容"要求的正是**两句证据同时在
    /// 场**。`reveal_last` 为同一条理由先占过一格。
    pub auto_copy_last: String,
    /// 通知被**点**了几次。每次点击恰好一次，与它开没开成目录无关。
    pub reveal_clicks: u32,
    /// 落盘时因开关自己来的那几次。和上一行分开，是因为把两个数并成一个就叫不出
    /// "开关开着却一次没自动来"这种状态。
    pub reveal_auto: u32,
    /// shell **收下**了几次请求。`0` 不等于没点过，也不等于点错——它是"这个进程往
    /// 别人家的窗口里递过一次话"的唯一证据，门要问的就是这一格。
    pub reveal_acted: u32,
    /// 最近一次揭示的话，成功失败都写这里，不写 [`Self::message`]：那句话是"已保存到
    /// 某个路径（多大）"，通知的行要留着它，否则一句反馈会把另一句证据挤掉。
    pub reveal_last: String,
    /// 写之前那一档提醒 —— PRD **447** 说的是"应提醒用户背景**将**被填充"，而
    /// [`Self::message`] 那一行的"已保存到 … 已填成白色"是**事后**说的。
    ///
    /// 它单独占一格，是因为一句话的槽位里"将被"只存在几毫秒就被"已保存"顶掉 —— 那不
    /// 叫提醒过。空串是"这一档不丢透明"，不是"没提醒"；它是提醒而不是确认，所以写照常
    /// 发生（P39 ⑦ 那次产品决定的另一读，第 46 项 ①）。
    pub pre_notice: String,
    /// Bumped by every `open`, so a second capture in the same process gets keys
    /// nobody has asked for yet. A constant revision would reuse the previous
    /// frame's key, and `Image.cache: false` only re-requests when the URL changes:
    /// a "re-capture" would then show the old freeze, which is exactly the bug
    /// §5.2.4 cannot survive.
    next_revision: u32,
    pub problems: Vec<String>,
    /// §5.2–5.4：整段 `[capture]` 自 P65（第 65 项 ② 那一族的结法与 P64 的 `[pin]` 同形）
    /// 起由 [`crate::settings::capture_cfg`] 送进运行期 —— `open()` 里取一次快照，之后 12
    /// 枚键（`default_mode` / `include_cursor` / `freeze_frame` / `highlight_element`
    /// / `magnifier_radius` / `magnifier_zoom` / `magnifier_grid` / `magnifier_coordinates`
    /// / `magnifier_color_format` / `show_dimensions` / `delay_seconds` / `snap_to_window`）
    /// 都在 `self.capture.<键>` 那格等着被下一次改码接进 QML。P65 之前只有 `show_dimensions`
    /// 单独走 [`crate::settings::show_dimensions`]，其余 11 枚停在"键在档里没人读"。
    ///
    /// 那 11 枚自 P108 起是 10 枚：`delay_seconds` 有了行为读者 [`crate::delay::arm_from_config`]，
    /// 而它**不从这里读** —— 延时决定的是 `open()` 之前等多久，那一刻这枚快照还没取。留在本格的
    /// 那一份仍然是「这一场遮罩开着的时候配置长什么样」，两枚读者各说各的时刻，没有谁覆盖谁。
    ///
    /// 快照而不是逐字段 reader：`open()` 只跑一次，中途 `write_back` 不改本会话已经开着的
    /// 遮罩，和 P64 的 `PinState::new()` 同一形状。**运行期读数仍欠**（第 65 项 ②）——
    /// `--mask` 门本轮 0 趟，cargo 单测证的是"字段装得下 config"而不是"用户改
    /// `freeze_frame = false` 之后遮罩背景真的会跟着刷新"。
    pub capture: falcon_core::config::Capture,
    /// `[history]` 整段在这一场流程里的快照，P93 起。`Some` 只说"这一场有历史可记"，
    /// **不**替 `enabled` 作第二次主：那一分岔归 [`crate::settings::history_open_in`]
    /// （P68 建的那枚纯函数本体，本轮第一次被产品路径而不是量具路径走到）。
    ///
    /// 而 `MaskState` 自己的 `Default` 是 `None` —— 这一格挡的是**跑单测的那台机器**，
    /// 不是用户的开关：`commit()` 被四格已有的 cargo 单测直接调用
    /// （`commit_only_returns_a_rect_that_exists` 那一族，§9.1 第十条），门上一旦改成
    /// "读进程全局配置决定要不要开 SQLite"，一次 `cargo test` 就会往
    /// `%APPDATA%\ai-falconshot\history\` 里长出一份 `history.db`、一行记录和一张原图。
    /// 真机上这一格由 `open()` 从配置里取，出厂 `enabled = true` ⇒ §5.14.1 的
    /// "每次截图都存"自此生效 —— 与 [`Self::auto_copy`]、[`Self::show_dimensions`]
    /// "配置文件默认开着、单测默认关着"是同一形的一格。取的一刻与 [`Self::export`]、
    /// [`Self::naming`]、[`Self::capture`] 同一场 `open`。
    pub history: Option<falcon_core::config::History>,
    /// [`Self::history`] 那一行的落点：`[advanced] data_dir` 空 ⇒ `%APPDATA%\ai-falconshot\`。
    /// 空串是"说不出往哪儿写" ⇒ 与 `history=None` 同样不写，所以单测里那枚
    /// `PathBuf::default()` 与真机上那枚说不出的目录都走同一条腿。
    pub history_dir: std::path::PathBuf,
    /// 这一场往 `history.db` 里**成功**写进了几行。失败不吃次数也不加，与
    /// [`Self::auto_copies`] 同一口径：开着却一次没记成，与从没按过，是两件事。
    pub history_records: u32,
    /// 最近一次记成的行号。`None` 是"这一场还没有行"，不是"有一行但说不出号"。
    pub history_last_id: Option<i64>,
    /// 最近一次记录的话，成功失败都写这里而不写 [`Self::message`]：那句话留给落盘，
    /// 而 §5.5.3 的"保存失败时应提示原因"要求的正是**两句证据同时在场**
    /// （[`Self::auto_copy_last`]、[`Self::reveal_last`] 为同一条理由各占一格）。
    pub history_last: String,
    /// 这一场从库里**把某一行的图读回来、又落了一次盘**成功几次（§5.14.2 的第一条产品腿，
    /// P97 起）。单列一枚而不是复用 [`Self::history_records`]：那一枚数的是"写进去几行"，
    /// 这一枚数的是"读回来再写一次"，同一次会话里两件事各长一份文件，混在一起就分不出
    /// 哪一张是哪一件的证。
    pub history_replays: u32,
    /// [`Self::history_replays`] 那一条腿说的话，与 [`Self::history_last`] 同一口径：成功
    /// 失败都写这里而不占 [`Self::message`]——"这一场没历史可读"、"库里没有第 N 行"、
    /// "图读不回来"与"落盘没写成"是四件不同的事，各自要能被点名。
    pub history_replay_last: String,
    /// 这一场从库里**读回来的那一张清单**（§5.14.2 step 1「打开历史记录」，P98 起）：
    /// [`crate::settings::history_rows_at`] join 出来的那一串，界面那一头经
    /// [`crate::session::Session::history_rows`] 读的就是它。空串有两种来历（库里没有行、
    /// 或根本读不到），后者写在 [`Self::history_list_last`] 里而不在这串里。
    pub history_list: String,
    /// [`Self::history_list`] 那一条腿说的话：读到几行、或者**为什么**一行也读不到。与
    /// [`Self::history_last`]、[`Self::history_replay_last`] 同一口径——写进去几行、读出
    /// 来一张清单、把某一行读回来再落一次盘是三件不同的事，各占一格，谁也不覆盖谁。
    pub history_list_last: String,
    /// 这一场经 [`Self::delete_history`] **删掉的行数**（§5.14.4 的"单条、多条"那一档，P99 起）。
    /// 数的是**行**而不是**次数**，因为 §5.14.4 step 4 那句"释放相应磁盘空间"的对面是一个个文件：
    /// 一次点了三行、和三次各点一行，盘上少的东西一样多，而次数会把它说成两件不同的事。删掉 0 行
    /// 的那一次不吃计数 —— 与 [`Self::history_records`] 那条"失败不加"的口径同一个理由。
    pub history_deletes: u32,
    /// 这一场经 [`Self::clear_history`] 删掉的行数（§5.14.4 的"全部"那一档，P99 起）。单独一枚
    /// 而不并进 [`Self::history_deletes`]：那一条是**用户点名**的行（包括他点名的一枚锁定项），
    /// 这一条永远**不碰锁定项**，两格混在一个数里就等于把这条相反的口径抹平。
    pub history_clears: u32,
    /// [`Self::delete_history`] 那一条腿说的话，含它收回的字节数。与 [`Self::history_clear_last`]
    /// 各占一格的理由，同 [`Self::history_list_last`] 那段。
    pub history_delete_last: String,
    /// [`Self::clear_history`] 那一条腿说的话：删了几行、收回多少字节，或者**为什么**一行也没删
    /// ——「二次确认没点头」「这一场没有历史」「`enabled` 关着」「库打不开」是四件不同的事。
    pub history_clear_last: String,
    /// 这一场经 [`Self::lock_history`] **写成的次数**（§5.14.4 规则 2 的那一枚锁，P100 起）。
    /// 它与 [`Self::history_deletes`]、[`Self::history_clears`] 是三种相反的口径，各占一格：
    /// 前两格数的是**行少了多少**（本格动的是一枚布尔，库里一行也不少、盘上一份文件也不走），
    /// 所以把它并进任何一枚都会把那两格的读数凭空抬高。
    pub history_locks: u32,
    /// [`Self::lock_history`] 那一条腿说的话：锁上了还是解开了、或者**为什么**这次没跑成 ——
    /// 「这一场没有历史」「`enabled` 关着」「库打不开」「库里没有第 N 行」是四件不同的事。
    pub history_lock_last: String,
    /// 这一场经 [`Self::restore_history_selection`] **重新进入原选区**几次（§5.14.3 step 4 的
    /// 第一条产品腿，P102 起）。单独一枚而不并进 [`Self::history_replays`]：那一枚数的是
    /// "把图读回来再落一次盘"（§5.14.2），这一枚一次也没往盘上写，它动的只有遮罩的选区。
    ///
    /// P113 起这一枚是**两半的合计**：代码直接按的恢复与翻页按的（
    /// [`Self::flip_history_selection`]）都算一次"回进了原选区"，谁递的要分开问就读
    /// [`Self::history_flips`] 那一半，`history_restores - history_flips` 即另一半。
    pub history_restores: u32,
    /// [`Self::restore_history_selection`] 那一条腿说的话：交回的是哪一行、那一行当时标没标
    /// 「已取消」、以及**为什么**这次没跑成 ——「这一场没有历史」「`enabled` 关着」「库打不开」
    /// 「库里没有第 N 行」「那一行的原选区是空的」是五件不同的事。
    pub history_restore_last: String,
    /// 这一场经**翻页**那两条腿（§5.14.2 step 2，P113 起）回成了几次，由
    /// [`Self::flip_history_selection`] 加。它不是 [`Self::history_restores`] 的重复计数，而是
    /// 把那枚总数拆成"谁递的"的两半之一：翻页落进那一行的原选区走的**就是**
    /// [`Self::restore_history_selection`]（本条不新开"什么算回成了"那第二套），所以一次翻页同时
    /// 加两枚，而一次由代码直接按的恢复只加 `history_restores`。不拆的代价是实打实看见的：
    /// 量具上「界面挪走过两次」与「界面翻了两页」那两格读的是同一枚计数器，两条腿一旦都绿，
    /// 加数就再也分不出是谁的 —— 判据只能改成要求一个更大的总数，而那个总数对不上任何一件
    /// 单独的事。单列一枚就能各问各的。
    pub history_flips: u32,
    /// 这一场界面**当前停在**的那一行（§5.14.2 的翻页那半，P113 起）。
    ///
    /// 为什么不复用 [`Self::history_last_id`]：那一枚说的是"这一场最近**记成**了第几行"，
    /// 它是量具上"行数加一"那格的证人（`autosave_check.rs` 里明确写着量具不碰它），
    /// 把"停在第几行"叠上去，一次翻页就会伪造出一行记录。也不与它混着读：一屏上刚记完
    /// 第 7 行、用户随即翻回第 3 行，这时"下一行是谁"问的是 3 而不是 7。
    ///
    /// `None` 是"这一场界面没停在任何一行上"——它不等同于"库里只有一行"，也不等同于
    /// "上一行记成了第 0 行"。有值时它就是 [`crate::settings::neighbor_id_for`] 的锚点，
    /// 而翻页落到新行时本格跟着换成新行，所以一条方向按两下、中间各读一次锚点，
    /// 就能证明锚点不是"这条腿只记一次"的记法。
    pub history_viewed: Option<i64>,
    /// 这一场经 [`Self::copy_history_content`] **把某一行的图递回剪贴板**成功几次（§5.14.3 step 4
    /// 「恢复该截图内容」的第一条产品腿，P116 起）。不复用 [`Self::auto_copies`]：那一枚数的是
    /// 「一次确认顺手递出去」，是默认行为；这一枚数是**用户点名的一行**，两条腿的对面不同，
    /// 混在一枚里就再也问不出「这一场递出去的哪一张是历史」。也不与
    /// [`Self::history_replays`] 并一枚：那一枚往盘上写了一份文件，这一枚一份也不写。
    pub history_copies: u32,
    /// [`Self::copy_history_content`] 那一条腿说的话：递的是哪一行、那张图多大、以及**为什么**
    /// 这次没递成 ——「这一场没有历史」「`enabled` 关着」「库打不开」「库里没有第 N 行」「那一行
    /// 的图取不回来」「剪贴板被占用」是六件不同的事，各写各的。与 [`Self::history_restore_last`]
    /// 同一条口径：不占 [`Self::message`]，那句话留给落盘。
    pub history_copy_last: String,
}

impl MaskState {
    /// §5.5.8 **跨 mask 会话** reuse 那半的落点：把上一次会话 quick_out 成功留下的路径
    /// 抄进本会话的 `landed`。抽出来是因为 `open()` 要抓屏、单测不能抓屏（§9.1 第十条的
    /// headless 半边），而 `prev` 由调用方从 `settings::last_landed()` 里带进来 ——
    /// 这一句本身既不需要抓屏、也不动进程全局，所以三格判定可以在单测里当场摆出来。
    ///
    /// 三格都不抄的情形：`reuse_last_file=false`（用户没打开那一格，跨会话那半整段不该生效）、
    /// `prev` 是 `None`（进程刚起来、还没落过任何一张）、
    /// 那一份路径**已经不在了**（覆写一张不存在的文件比让用户重新挑一次更糟）。
    fn seed_landed_from(&mut self, prev: Option<std::path::PathBuf>) {
        if self.reuse_last_file {
            if let Some(prev) = prev {
                if prev.exists() {
                    self.landed = Some(prev);
                }
            }
        }
    }

    /// Freeze → publish → one slot per screen. Returns the slot count.
    ///
    /// A screen Qt lists that the snapshot does not have is a `problem` and no
    /// slot, not a silent skip: a mask that covers two of three monitors is the
    /// kind of bug that otherwise gets filed against the capture path.
    pub fn open(&mut self, snap: &ScreenSnapshot, shader: bool) -> Result<usize, String> {
        if snap.monitors.is_empty() {
            return Err("快照里没有任何显示器".into());
        }
        self.shader = shader;
        self.hole = Self::default_hole(snap);
        self.pixels_ms = None;
        self.message.clear();
        self.pre_notice.clear();

        let started = Instant::now();
        let screens = shim::screens();
        if screens.is_empty() {
            return Err("Qt 没有报告任何屏幕".into());
        }

        let mut used = vec![false; snap.monitors.len()];
        let mut slots = Vec::new();
        let revision = self.take_revision();
        for (index, screen) in screens.iter().enumerate() {
            let Some(frozen) = Self::match_monitor(snap, screen, &mut used, index) else {
                self.problems.push(format!(
                    "screen {} {}x{} dpr={:.2} has no captured monitor",
                    screen.name, screen.w, screen.h, screen.dpr
                ));
                continue;
            };
            let info = &snap.monitors[frozen].info;
            let frame = &snap.monitors[frozen].frame;
            let key = Self::frame_key(&info.id, revision);
            shim::store_raw(&key, &frame.pixels, frame.width, frame.height);
            slots.push(Slot {
                index,
                name: info.id.clone(),
                key,
                bounds: info.bounds,
                geom: PhysRect::new(
                    screen.x,
                    screen.y,
                    screen.w.max(1) as u32,
                    screen.h.max(1) as u32,
                ),
                scale: Scale::of(screen.dpr),
                revision,
                swaps: 0,
                first_swap_ms: None,
                shader_status: None,
            });
        }
        if slots.is_empty() {
            return Err("Qt 的屏幕和快照的显示器配不上对".into());
        }

        self.slots = slots;
        // The annotation layer takes its base from the same snapshot, after the
        // slots exist: one flow, one freeze, and the ink's clip is this flow's hole.
        self.ink.begin(snap, &self.slots);
        // §5.7.1's per-tool memory arrives from the settings file here, after the
        // flow has been cleared: `begin` resets the layer, so pens handed over before
        // it would be thrown away by the reset. This is the one place the hot key
        // touches the file, and it costs what `[settings] apply=` reports.
        crate::settings::apply_to(&mut self.ink);
        self.export = crate::settings::export();
        self.naming = crate::settings::naming();
        self.decor = crate::settings::decoration();
        self.quick_saves = 0;
        // 上一张图的通知不能跟着这一张活：一张已经落盘的路径留着，下一次点击就会去
        // 揭示一张和用户此刻的决定无关的文件。计数同理——它们说的是这一次流程。
        // 唯一让 landed 跨过这一行的，是下面 reuse_last_file 那一支 —— 它带的不是
        // "上一段流程留下的通知"，而是"用户自己打开的'覆盖原图'那一格"要求的
        // 上一份**已落盘且此刻仍在磁盘上**的路径。默认（reuse=false）走 landed=None 那条老腿，
        // 已存在每一次按下的字节零改变。
        self.landed = None;
        self.reveal_clicks = 0;
        self.reveal_auto = 0;
        self.reveal_acted = 0;
        self.reveal_last.clear();
        self.reveal_after_save = crate::settings::reveal_after_save();
        self.auto_save = crate::settings::auto_save();
        self.auto_copy = crate::settings::auto_copy();
        self.reuse_last_file = crate::settings::reuse_last_file();
        // §5.2–5.4：整段 `[capture]` 由 P65 加的这枚 reader 一次性取快照 ——
        // P64 之前这里是逐字段（`show_dimensions` 那一枚），其余 11 枚停在"键在档里没人读"。
        // `show_dimensions` 那一枚 reader 保留（它比 P65 早三轮落地，且它的 qproperty 路径
        // 单独走 `MaskViewData` 与 QML，不能在这一轮撤）。
        self.capture = crate::settings::capture_cfg();
        self.show_dimensions = crate::settings::show_dimensions();
        // §5.5.8 **跨 mask 会话**那半（第 63 项 ⑦）：reuse_last_file=true 时，把上一次会话
        // quick_out 成功留下的路径抄进 self.landed —— 于是本会话第一次 quick_out 覆写的
        // 就是那一份而不是新起一份 quick-00N。文件不在了就不抄（走命名分支），因为
        // 覆写一张不存在的文件比让用户重新挑一次更糟。抽成 `seed_landed_from`
        // 是因为 open() 要抓屏、单测不能抓屏（§9.1 第十条的 headless 半边）。
        self.seed_landed_from(crate::settings::last_landed());
        self.auto_copies = 0;
        self.auto_copy_last.clear();
        // §5.14.1：这一场的历史门在这一刻取整段 `[history]` 的快照加上落点，与
        // [`Self::export`]、[`Self::naming`]、[`Self::capture`] 同一场 `open`、同一理由
        // ——一张图（和它那一行台账）不能由两份配置状态拼出来。**这里不判 `enabled`**：
        // 那一分岔自 P68 起就只有一个主人（[`crate::settings::history_open_in`]），
        // 在这一行再 `then` 一次就会多养出第二个主人。
        // 计数与那句话同理随会话归零：它们说的是这一次流程。
        self.history = Some(crate::settings::history_cfg());
        self.history_dir = crate::settings::history_data_dir();
        self.history_records = 0;
        self.history_last_id = None;
        self.history_last.clear();
        self.history_replays = 0;
        self.history_replay_last.clear();
        // 清单那两枚同理随会话归零：上一场读到过几行，与这一场界面读不读得回来是两件事。
        self.history_list.clear();
        self.history_list_last.clear();
        // §5.14.4 那三格同理：上一场删掉/清掉/锁过的次数与这一场点没点名是两件事，而三句话
        // 各占一格的理由同 [`Self::history_delete_last`] 头上那段。
        self.history_deletes = 0;
        self.history_clears = 0;
        self.history_locks = 0;
        self.history_delete_last.clear();
        self.history_clear_last.clear();
        self.history_lock_last.clear();
        // §5.14.3 那一格同理：上一场把几行交回过原选区，与这一场点没点是两件事。
        self.history_restores = 0;
        self.history_restore_last.clear();
        // 锚点同理随会话归零（P113）：上一场界面停在第几行，与这一场从哪一页翻起是两件事，
        // 留着它会让新会话第一次 Ctrl+Down 落在上一场那一行的下一行上。
        self.history_viewed = None;
        // 翻页那枚计数同理（P113）：它拆的是「这一场被翻过几页」，上一场的页数不是这一场的加数。
        self.history_flips = 0;
        self.ink.set_hole(self.hole);
        self.virtual_bounds = snap.virtual_bounds;
        self.shown = true;
        self.pixels_ms = Some(started.elapsed().as_millis() as u64);
        self.started = Some(Instant::now());
        crate::state::stamp("opened");
        Ok(self.slots.len())
    }

    /// Claim the next frame revision, one-based and never repeated.
    ///
    /// Split out of `open` because the rest of `open` needs Qt (the screen list and
    /// the frame store), while this is the part whose mistake is invisible: with a
    /// constant revision a re-capture publishes the same provider key as the capture
    /// before it, the `Image`'s URL does not change, and the overlay keeps showing
    /// the old freeze while reporting a new one.
    fn take_revision(&mut self) -> u32 {
        let revision = self.next_revision.max(1);
        self.next_revision = revision + 1;
        revision
    }

    /// The provider key for one screen's frame at one revision.
    fn frame_key(name: &str, revision: u32) -> String {
        format!("{name}-{revision}")
    }

    /// The selection a mask opens with: the middle half of the primary monitor,
    /// or of the whole desktop if the enumeration names no primary. A fixed shape
    /// on purpose - the hole the user drags is M3's, and until then the only
    /// honest test of a hole is one whose size is known in advance.
    fn default_hole(snap: &ScreenSnapshot) -> PhysRect {
        let b = snap
            .monitors
            .iter()
            .find(|m| m.info.primary)
            .map(|m| m.info.bounds)
            .unwrap_or(snap.virtual_bounds);
        PhysRect::new(b.x + b.w as i32 / 4, b.y + b.h as i32 / 4, b.w / 2, b.h / 2)
    }

    /// Qt's screen `i` to a snapshot monitor, by physical size.
    ///
    /// Size rather than name because the two enumerations do not speak the same
    /// names (`DISPLAY1` against `m0-display1`) and index order is not promised by
    /// either. At mixed scale, DIP times one screen's dpr is that screen's physical
    /// size, which is enough to pair them; two identical monitors at the same scale
    /// fall back to index order, and a genuine mismatch is reported, not guessed at.
    fn match_monitor(
        snap: &ScreenSnapshot,
        screen: &shim::Screen,
        used: &mut [bool],
        index: usize,
    ) -> Option<usize> {
        let near = |a: i32, b: i32| (a - b).abs() <= 1;
        let phys = |dip: i32, ratio: f64| (dip as f64 * ratio).round() as i32;
        let mut candidates: Vec<usize> = Vec::new();
        for (i, m) in snap.monitors.iter().enumerate() {
            if used[i] {
                continue;
            }
            if near(phys(screen.w, screen.dpr), m.info.bounds.w as i32)
                && near(phys(screen.h, screen.dpr), m.info.bounds.h as i32)
            {
                candidates.push(i);
            }
        }
        if candidates.is_empty() {
            return None;
        }
        let chosen = candidates
            .iter()
            .copied()
            .find(|i| *i == index)
            .unwrap_or(candidates[0]);
        used[chosen] = true;
        Some(chosen)
    }

    /// The mask is over: take the textures out of Qt's process, all of them, since
    /// a 4K desktop is 24 MB per screen and a leak here is a leak the user sees.
    pub fn close(&mut self) {
        for slot in &self.slots {
            shim::drop_frame(&slot.key);
        }
        self.slots.clear();
        self.grab = None;
        self.shown = false;
        // The ink's canvases are the same size as the frames they sit on, so a flow
        // that ended without this leaked two 24 MB textures per screen, not one.
        self.ink.end();
        // What this flow taught the pens goes back to the settings file now, not at
        // exit: a user who signs out without the process ever running its teardown
        // still gets the pen they set last time. `end` keeps `styles` and drops
        // everything else, so reading the layer here is reading the answer, not the
        // draft.
        crate::settings::write_back(&self.ink);
    }

    /// The selection, in physical desktop pixels. Each slot re-reads its part of
    /// it on the next `reload`; making every window redraw without waiting for its
    /// own reload is the M3 selection flow's, not this one's.
    ///
    /// The annotation layer is told as well: its clip is this rect, and an ink pixel
    /// the user has just dragged out of the selection must stop being on screen.
    pub fn set_hole(&mut self, hole: PhysRect) {
        self.hole = hole;
        self.ink.set_hole(hole);
    }

    pub fn set_shader(&mut self, shader: bool) {
        self.shader = shader;
    }

    // ------------------------------------------------------------ selection
    //
    // The pointer arrives in window-local device-independent pixels because that is
    // what Qt Quick reports, and every answer here is in physical desktop pixels
    // because that is the space the crop happens in. One conversion, in
    // [`MaskState::desk_point`], is the whole boundary.

    /// A press inside an existing selection moves it rather than starting a new
    /// one; a press outside draws. PRD §5.3.1 only says a drag makes a rectangle
    /// and the edges adjust it, so this is this project's decision, and the reason
    /// is that a mis-drawn selection is cheaper to move than to re-draw.
    ///
    /// A *drawing* tool takes the press first, though - §5.7.1 step 4's 在选区内绘制
    /// is about the pen, not the selection - and then nothing of the selection is
    /// held: `grab` stays `None` so a later `drag` cannot move the hole out from
    /// under the stroke. Getting the selection's handles back is the arrow tool's
    /// (code 0 in [`crate::annotate::TOOLS`]), which is the same reason a real
    /// screenshot toolbar has one.
    pub fn press(&mut self, index: usize, x: f64, y: f64) -> Option<Handle> {
        let p = self.desk_point(index, x, y)?;
        if self.ink.press(p) {
            self.grab = None;
            return None;
        }
        let slop = self.hit_slop(index);
        let hit = if self.hole.is_empty() {
            None
        } else {
            handle_at(&self.hole, p, slop)
        };
        self.grab = Some(match hit {
            Some(h) => Grab {
                handle: Some(h),
                anchor: p,
                start: self.hole,
            },
            None => Grab {
                handle: None,
                anchor: p,
                start: PhysRect::default(),
            },
        });
        if hit.is_none() {
            // Nothing is drawn yet: a hole of zero size at the anchor reads as "no
            // selection" to the dim, so the desktop stays fully dark until the
            // pointer actually moves.
            self.set_hole(PhysRect::new(p.x, p.y, 0, 0));
        }
        hit
    }

    /// The pointer moved while held. A new rectangle is normalised from the anchor,
    /// so dragging up-and-left works as well as down-and-right.
    ///
    /// A stroke mid-drag is not a selection drag: the drawing tool already took the
    /// press, so the point goes to the ink and the hole is left alone.
    pub fn drag(&mut self, index: usize, x: f64, y: f64) {
        let Some(p) = self.desk_point(index, x, y) else {
            return;
        };
        if self.ink.dragging() {
            self.ink.drag(p);
            return;
        }
        let Some(g) = self.grab else { return };
        let bounds = self.desktop();
        self.set_hole(match g.handle {
            None => PhysRect::from_points(g.anchor, p),
            // Not `resize`'s `Body` branch: that puts the rectangle's origin under
            // the pointer, which is right for a tool you click to place and wrong
            // for a selection you grabbed in the middle - the rect would jump by
            // half its size on the first pixel of the drag.
            Some(Handle::Body) => g
                .start
                .offset(p.x - g.anchor.x, p.y - g.anchor.y)
                .fitted_into(&bounds),
            Some(h) => resize(&g.start, h, p, &bounds),
        });
    }

    /// The pointer let go: the selection's grab is dropped, and a half-drawn stroke
    /// becomes an object (or is discarded) in the layer.
    pub fn release(&mut self) {
        self.grab = None;
        if self.ink.dragging() {
            self.ink.release();
        }
    }

    /// A point clamped into the desktop, in physical pixels.
    fn desk_point(&self, index: usize, x: f64, y: f64) -> Option<PhysPoint> {
        let slot = self.slot(index)?;
        let r = slot.scale.ratio();
        let p = PhysPoint::new(
            slot.bounds.x + (x * r).round() as i32,
            slot.bounds.y + (y * r).round() as i32,
        );
        let d = self.desktop();
        Some(PhysPoint::new(
            p.x.clamp(d.x, d.right()),
            p.y.clamp(d.y, d.bottom()),
        ))
    }

    /// The grab radius is a distance a finger travels, so it is stated in
    /// device-independent pixels and this screen's scale turns it into physical
    /// ones. Five DIP is `falcon_core`'s `DEFAULT_HIT_SLOP`, the same number the
    /// annotation handles use.
    fn hit_slop(&self, index: usize) -> u32 {
        self.slot(index)
            .map(|s| s.scale.dip_to_phys_i(DEFAULT_HIT_SLOP as i32).max(1) as u32)
            .unwrap_or(DEFAULT_HIT_SLOP)
    }

    /// Arrow keys (§5.3.9). The step is this project's: one physical pixel, ten with
    /// a modifier, and `resize_edge` moves the bottom-right edge instead of the
    /// whole rect. Both helpers clamp to the desktop, which is the rule the PRD
    /// does fix - "不允许选区超出有效截图区域".
    pub fn nudge_by(&mut self, dx: i32, dy: i32, resize_edge: bool) {
        if self.hole.is_empty() {
            return;
        }
        let bounds = self.desktop();
        self.set_hole(if resize_edge {
            nudge_edge(&self.hole, Handle::Se, dx, dy, &bounds)
        } else {
            nudge(&self.hole, dx, dy, &bounds)
        });
    }

    /// Esc. `true` means this press consumed itself - on a half-typed 文本 box or on the
    /// selection;
    /// `false` means there was nothing to clear, so the caller cancels the capture.
    /// Two windows both receiving one Esc press would then clear-and-cancel in a
    /// single keystroke, which is why the ladder is a return value and not a
    /// property QML reads.
    ///
    /// 第三样，P95 起：§5.14.1 的第二条"用户可开启『保存所有截图记录』，包括被取消的
    /// 截图"在这道门上有了它的第一个落点。它排在 [`Self::grab`] 被放下**之前**，理由是
    /// [`Self::record_history`] 要经 [`Self::delivered`] 才拼得出画面 —— 先松手再记，
    /// 记的就只是一句"这一场的画面拼不出来"。是否该行由 [`esc_owes_row`] 判，而那格开关
    /// 是 P95 新建的 [`falcon_core::config::History::save_all`]：它默认为关，所以这一条
    /// 阶梯对没开启它的用户完全静默，与本轮之前逐字节一致。
    pub fn escape(&mut self) -> bool {
        // §5.7.11's box is the first rung, because it is the only thing in this ladder
        // that is not in any history yet: the typed string is not an object, so a rung
        // that un-selected instead would take the letters with it - and the selection
        // one rung down clears the box anyway, through `Layer::set_hole`.
        if self.ink.abandon_typing() {
            return true;
        }
        let had_selection = !self.hole.is_empty();
        if !had_selection {
            return false;
        }
        if esc_owes_row(
            self.history.as_ref().is_some_and(|c| c.save_all),
            had_selection,
        ) {
            self.record_history(self.hole, true);
        }
        self.grab = None;
        self.set_hole(PhysRect::default());
        true
    }

    /// §5.14.1 的那一行台账：一次确认的选区在这道门上先留下历史，才轮到复制与落盘。
    ///
    /// 顺序是**先记、再复制、再写文件**，与 P59 给 `auto_copy` 排的那条是同一个方向的
    /// 两种用法：那条说"保存失败时要留住别的东西"，所以留住的那一句要在失败之后还在场；
    /// 这一条说的是历史本身 —— 它是这场流程唯一一份"这次截过图"的凭据，排在写文件之前
    /// 才可能出现"文件没写成、而这次截图还在历史里"这种解释得通的状态；反过来做，一次
    /// 失败的写就让整场截图凭空消失，而 PRD 没有给任何一扇失败这种权力。
    ///
    /// 每一格失败都只写 [`Self::history_last`] 并且返回 `None`：**不动** [`Self::message`]、
    /// 也不让 [`Self::commit`] 少交回那枚 rect。SQLite 打不开、缩略图编不出、目录写不进，
    /// 都不构成把用户已经画好的选区退回遮罩里的理由。
    ///
    /// 记成之后立刻跑 [`falcon_core::history::History::cleanup`] —— §5.14.4 的那三条上限
    /// （条数 / 天数 / 容量）与 `exempt_locked` 那一格全在 [`falcon_core::history::Retention`]
    /// 里，而 `Retention::from` 早就是现成的：把清理排在新行写进去之后，用户刚截的那一枚
    /// 永远是最后一条被考虑淘汰的。清理失败不许吃掉已经记成的那一行，所以它只改那句话。
    ///
    /// `cancelled` 自 P95 起由调用方交进来，而不是在这里替两条腿代答一次：确认那条门传
    /// `false`，Esc 清掉选区那一格传 `true`。落进 SQLite 的那一格（`history.rs` 的
    /// `cancelled INTEGER NOT NULL`）与屏幕上那句话用的是同一个数 —— 一次记成之后又说出
    /// 另一回事，就是两份真相。
    fn record_history(&mut self, rect: PhysRect, cancelled: bool) -> Option<i64> {
        // `None` 是"这一场没有历史可记"：`MaskState` 自己的 `Default` 就是这一格，
        // 所以直接调 `commit()` 的那几格 cargo 单测一步也不碰 disk（§9.1 第十条）。
        // 落点说不出（空串）走同一条腿，而 `enabled` 那一侧不在这里判 —— 那是
        // [`crate::settings::history_open_in`] 的分岔本体，本轮不养第二个主人。
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            return None;
        };
        let Some(frame) = self.delivered() else {
            self.history_last = "历史没记：这一场的画面拼不出来。".to_string();
            return None;
        };
        let mut history = match crate::settings::history_open_in(&dir, &cfg) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.history_last = "历史没记：`[history] enabled` 是关的。".to_string();
                return None;
            }
            Err(e) => {
                self.history_last = format!("历史没记：打不开 {:?} 里的 history.db（{e}）。", dir);
                return None;
            }
        };
        let screens: Vec<(String, PhysRect)> = self
            .slots
            .iter()
            .map(|s| (s.name.clone(), s.bounds))
            .collect();
        let meta = capture_meta_for(
            &screens,
            rect,
            falcon_core::capture::now_ms(),
            platform_windows::clock::wall_clock().1,
            cancelled,
        );
        match history.record(&meta, &frame, &cfg) {
            Ok(id) => {
                self.history_records += 1;
                self.history_last_id = Some(id);
                // 两句话分开写：§5.14.3 要用户在历史里"找到标记为已取消的记录"，那一句
                // 只能在取消这一格说，不能事后靠 `history_last_id` 去猜是哪一条。行号
                // 照旧点名，因为 `#id` 是这一行在 SQLite 里的名字，与它取不取得成图无关。
                self.history_last = if cancelled {
                    format!(
                        "历史记了一行 #{id}（已取消）：{} 上 ({},{}) {}×{}。",
                        meta.monitor_id, rect.x, rect.y, rect.w, rect.h
                    )
                } else {
                    format!(
                        "历史记了一行 #{id}：{} 上 ({},{}) {}×{}。",
                        meta.monitor_id, rect.x, rect.y, rect.w, rect.h
                    )
                };
                if let Err(e) = history.cleanup(&falcon_core::history::Retention::from(&cfg)) {
                    self.history_last =
                        format!("{}，但 §5.14.4 的自动清理没跑成：{e}", self.history_last);
                }
                Some(id)
            }
            Err(e) => {
                self.history_last = format!("历史没记：{e}。截图本身没受影响。");
                None
            }
        }
    }

    /// §5.14.2 step 1「打开历史记录」的那一张清单，P98 起。
    ///
    /// [`Self::replay_history`] 是"读回来再落一次盘"那一条腿，它得先知道库里**有哪几行**；
    /// 这一条就是那一问的答案，也是 [`falcon_core::history::History::newest`] 在产品里的
    /// 第一位读者（P95 普查点名的那 11 枚仍为零的函数之一）。落点、`[history]` 那一段快照
    /// 与时间偏移都取本会话自己的：`captured_at` 存的是 epoch 毫秒，"这一行写于几点"要的
    /// 是这台机器当时的偏移，故每次读都问一次 [`platform_windows::clock::wall_clock`]，
    /// 而不在 `settings` 那一层养第二个主人。
    ///
    /// 三种答法分开：`[history] enabled` 关着 ⇒ 一句「没开」；库里没有行 ⇒ **空清单 + 零行
    /// 那句**（一次正常的空）；库打不开或那些行读不回 ⇒ 各点自己的名。把后两种都说成空，
    /// 等于让一枚坏掉的 history.db 读成一次正常清空——[`Self::replay_history`] 头上写的是
    /// 同一条口径。
    pub fn list_history(&mut self, limit: usize) -> String {
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_list.clear();
            self.history_list_last = "清单没读：这一场没有历史可读。".to_string();
            return String::new();
        };
        let (_epoch, offset_min) = platform_windows::clock::wall_clock();
        match crate::settings::history_rows_at(&dir, &cfg, limit, offset_min) {
            Ok(rows) => {
                let n = if rows.is_empty() {
                    0
                } else {
                    rows.split('|').count()
                };
                self.history_list = rows.clone();
                self.history_list_last = format!("读了 {dir:?} 里的历史清单：{n} 行。");
                rows
            }
            Err(why) => {
                self.history_list.clear();
                self.history_list_last = format!("清单没读：{why}。");
                String::new()
            }
        }
    }

    /// §5.14.2「从历史记录中……保存」的第一条**产品**腿，P97 起。
    ///
    /// 读侧前三轮的证人全在量具那一头（`autosave_check` 自己开一次 SQLite 问行数、问
    /// `cancelled`），产品自己一次也没把某一行的图**读回来**过 —— 而 §5.14.1 那句
    /// "历史记录应包含……截图内容"要的恰恰是这份图取得回来。这一条走的是反方向：
    /// [`falcon_core::history::History::get`] 取那一行 →
    /// [`falcon_core::history::History::frame`] 把它的原图从盘上解码（原图不在了就退到缩略
    /// 图，那是 `frame` 自己定的口径，本条不替它重定）→ [`crate::save::quick_to`] 那唯一
    /// 一道落盘缝合。命名规则用的是**同一份**（模板、前缀、碰撞），所以这一格落在盘上的
    /// 名字仍是用户自己的规则给的，而碰撞时由命名引擎换名，不会盖掉任何一张旧图。
    ///
    /// 三件事本条一件也不碰，各有自己的主人：[`Self::quick_saves`] 与 [`Self::landed`]
    /// （回放一次就把它们挪一下，等于让新加的腿去改 P58/P60 已经结清的那两格账，所以
    /// [`Self::history_replays`] 是另一枚计数器）、[`crate::settings::record_landed`]
    /// （那是 §5.5.8 `reuse_last_file` 的对面：一次跨会话的复用该接住的是用户自己按的那次
    /// 快速保存，不是一场回放）、[`Self::message`]（留给落盘那一句）。
    ///
    /// [`Self::pre_notice`] 是**例外**，也照 quick_out 的写法先设：一份带透明的图存成 jpg
    /// 时会被填白，P46 给这件事立的话不该因为图是从库里读出来的就少说一次。
    pub fn replay_history(&mut self, id: i64) -> Option<std::path::PathBuf> {
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_replay_last = "回放没跑：这一场没有历史可读。".to_string();
            return None;
        };
        let history = match crate::settings::history_open_in(&dir, &cfg) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.history_replay_last =
                    "回放没跑：`[history] enabled` 是关的，库里没有可读的东西。".to_string();
                return None;
            }
            Err(e) => {
                self.history_replay_last =
                    format!("回放没跑：打不开 {dir:?} 里的 history.db（{e}）。");
                return None;
            }
        };
        // 「库里没有第 N 行」与「那一行读不回来」是两个答案：前者是一次正常的空，后者是
        // 库出了问题，把它们都说成"没有"会把一枚坏掉的 history.db 讲成一次正常的翻页。
        let rec = match history.get(id) {
            Ok(Some(rec)) => rec,
            Ok(None) => {
                self.history_replay_last = format!("回放没跑：{dir:?} 的库里没有第 {id} 行。");
                return None;
            }
            Err(e) => {
                self.history_replay_last = format!("回放没跑：第 {id} 行读不回来（{e}）。");
                return None;
            }
        };
        let frame = match history.frame(&rec) {
            Ok(frame) => frame,
            Err(e) => {
                self.history_replay_last =
                    format!("回放没跑：第 {id} 行的图取不回来（{e}），库里那一行还在。");
                return None;
            }
        };
        let Some(to) = self.naming.directory() else {
            self.history_replay_last = format!(
                "回放没跑：第 {id} 行的图读回来了（{}×{}），却没有目录可写它。",
                frame.width, frame.height
            );
            return None;
        };
        let (epoch, offset_min) = platform_windows::clock::wall_clock();
        let facts = naming::NameFacts {
            width: frame.width,
            height: frame.height,
            // 那一行的 `monitor_id` 是记录里带着的（§5.14.1 的"屏幕"），所以 `%m` 在这里
            // 比一次现场快速保存更有底气：quick_out 交的是空串，等的是"哪一路能替这张图
            // 决定它来自哪块屏"那一问（§5.5.4 头上结转的那半）。
            monitor: &rec.monitor_id,
            tool: "",
        };
        self.pre_notice =
            crate::save::reminder_for_format(&frame, self.export.format, "历史那一行");
        let outcome = {
            let quick = crate::save::Quick {
                dir: &to,
                template: &self.naming.template,
                prefix: &self.naming.prefix,
                seq: self.history_replays + 1,
                collision: &self.naming.collision,
            };
            crate::save::quick_to(
                &frame,
                naming::LocalStamp::from_epoch(epoch, offset_min),
                facts,
                &quick,
                &self.export,
                "历史那一行",
            )
        };
        match outcome {
            Ok((path, said)) => {
                self.history_replays += 1;
                self.history_replay_last = format!("回放了历史第 {id} 行：{said}");
                Some(path)
            }
            Err(crate::save::Refusal::BadName(e)) => {
                self.history_replay_last = format!("回放没写成：{e}");
                None
            }
            Err(crate::save::Refusal::Write(e)) => {
                self.history_replay_last = format!("回放没写成：{e}。库里那一行还在。");
                None
            }
        }
    }

    /// §5.14.4「选择单条、多条记录 → 点击删除 → 系统释放相应磁盘空间」的第一条**产品**腿，
    /// P99 起。[`falcon_core::history::History::delete`] 自 §6.2 建库那天起只有单测读者：
    /// 写侧四轮（P93/P95/P96）与读侧两轮（P97/P98）之外，库里那一行的**去**一次也没发生过。
    ///
    /// 这一格要的记账面比"库里少一行"硬：§5.14.4 step 4 那句"释放相应磁盘空间"对面是文件系统，
    /// 所以 [`falcon_core::history::Freed`] 那两个字段（哪些行、多少字节）原样交回，本条一处也
    /// 不替它估 —— 字节是 `bytes_of` 现场问那份文件与那一行的缩略图 BLOB 加起来得来的，而
    /// `History::delete` 顺手跑的 `prune_days` 会把删空的日期文件夹也收走（那是 §5.14.4 自己
    /// 说的话，不是本条加的）。
    ///
    /// 一桩本条**故意不判**的事：`History::delete` 不看锁（`history.rs` 头上第二条口径讲得很
    /// 明白——手动删除是用户点名的，锁定项也在他的点名里），而 [`Self::clear_history`] 恰恰相反，
    /// 永远不碰锁定项。两条相反的口径各有主人：本条不在这里替 `clear` 挡一道，那里也不在那里
    /// 替 `delete` 挡一道。
    ///
    /// 三件本条一件也不碰，各有主人：[`Self::history_records`] 与 [`Self::history_last_id`]
    /// （这一场记了几行、和这一场最后记成哪一行，不该因为别人删了行而改动）、
    /// [`Self::history_list`]（那是上一次**读**回来的快照，本条不许假装它跟着库同步了 ——
    /// 界面要新清单就再走一次 [`Self::list_history`]）、[`Self::message`]（留给落盘那一句）。
    pub fn delete_history(&mut self, ids: &[i64]) -> Option<falcon_core::history::Freed> {
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_delete_last = "删除没跑：这一场没有历史可清。".to_string();
            return None;
        };
        // 空点名挡在**开库之前**。`History::delete(&[])` 会答一次"成功的零行"，而"用户一行也
        // 没选中"与"选中的都删成了"在盘上长得一样，在话里必须长得不一样；不去开库也就顺手证
        // 明了这一次连读都没读过。
        if ids.is_empty() {
            self.history_delete_last = "删除没跑：没有点名任何一行，一行也不删。".to_string();
            return None;
        }
        let history = match crate::settings::history_open_in(&dir, &cfg) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.history_delete_last =
                    "删除没跑：`[history] enabled` 是关的，库里没有可删的东西。".to_string();
                return None;
            }
            Err(e) => {
                self.history_delete_last =
                    format!("删除没跑：打不开 {dir:?} 里的 history.db（{e}）。");
                return None;
            }
        };
        let asked = ids.len();
        match history.delete(ids) {
            Ok(freed) => {
                let gone = freed.ids.len() as u32;
                self.history_deletes += gone;
                self.history_delete_last = if gone == 0 {
                    format!(
                        "删除没删成：点名的 {asked} 个行号在库里一个也没有，盘上也就没少一份文件。"
                    )
                } else {
                    let names = freed
                        .ids
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join("、");
                    format!("删了历史第 {names} 行，收回 {} 字节。", freed.bytes)
                };
                Some(freed)
            }
            Err(e) => {
                self.history_delete_last =
                    format!("删除没跑：{dir:?} 的库里那 {asked} 行删不成（{e}）。");
                None
            }
        }
    }

    /// §5.14.4 step 2 的"全部"那一档，加规则 1 的**二次确认**，P99 起。
    ///
    /// `confirmed` 是本条唯一的门栓，而 `false` 那一臂**连库都不打开**。这不是画蛇添足：一次
    /// 没点头的清空如果在盘上留下任何痕迹（`history.db-wal` 被新建、checkpoint 顺手回收了几行），
    /// 判据就只能靠"看起来没删"来证，而"根本没开库"是能当场数出来的。`history.rs` 头上那句
    /// "The 二次确认 belongs to the UI"说的是弹窗归界面；**它落到产品里的形状**就是这一枚
    /// `bool`——界面递 `false` 的那一刻，产品一行也不动。
    ///
    /// `include_locked` 在这里钉死 `false`，与 [`Self::delete_history`] 相反：PRD §5.14.4 的
    /// 规则第二条"不得删除用户明确收藏或锁定的记录"读的是一条口径，而不是一条只归自动清理的
    /// 例外。想连锁定项一起清掉的那个决定，本条不替用户做，也不留一个悄悄为真的参数。
    ///
    /// 不碰的账同 [`Self::delete_history`]，理由也同：[`Self::history_records`]、
    /// [`Self::history_last_id`]、[`Self::history_list`]、[`Self::message`]。
    pub fn clear_history(&mut self, confirmed: bool) -> Option<falcon_core::history::Freed> {
        if !confirmed {
            self.history_clear_last = "清空没跑：二次确认没点头，一行也不动。".to_string();
            return None;
        }
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_clear_last = "清空没跑：这一场没有历史可清。".to_string();
            return None;
        };
        let history = match crate::settings::history_open_in(&dir, &cfg) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.history_clear_last =
                    "清空没跑：`[history] enabled` 是关的，库里没有可清的东西。".to_string();
                return None;
            }
            Err(e) => {
                self.history_clear_last =
                    format!("清空没跑：打不开 {dir:?} 里的 history.db（{e}）。");
                return None;
            }
        };
        match history.clear_all(false) {
            Ok(freed) => {
                let gone = freed.ids.len() as u32;
                self.history_clears += gone;
                self.history_clear_last = if gone == 0 {
                    "清空跑成了而一行也没删：库里没有行，或剩下的全是锁定项。".to_string()
                } else {
                    format!(
                        "清空了历史 {gone} 行，收回 {} 字节；锁定项按 §5.14.4 那条规则留下。",
                        freed.bytes
                    )
                };
                Some(freed)
            }
            Err(e) => {
                self.history_clear_last = format!("清空没跑：{dir:?} 的库清不成（{e}）。");
                None
            }
        }
    }

    /// §5.14.4 规则 2 里那一枚**锁**在产品里的形状，P100 起。
    ///
    /// 规则第二条说的是"不得删除用户明确收藏或锁定的记录"。这句话有两半：被禁的那件事（删）
    /// 归 [`Self::clear_history`]，而**打上那一枚锁**这半至今没有产品腿 ——
    /// [`falcon_core::history::History::set_locked`] 从 P89 建库起只有 core 自己的单测和量具
    /// 的**前置**在调（P99 第 22 格为此当场开了一次库），界面递不出「收藏这一行」这个动作。
    /// 本条是那半第一次有个能被界面按的地方。
    ///
    /// 一格布尔，两臂都跑得通：`locked` 为真就是锁上、为假就是解开。解开也在本条里，因为
    /// "用户明确"那四个字必然包含"用户改主意"；把它留给另一枚函数就等于让界面去猜哪一臂
    /// 该由谁写。
    ///
    /// 与那两格的**相反口径**在这里也照原样留着：[`Self::delete_history`] 不看锁（用户点名
    /// 就算点名了锁定项），[`Self::clear_history`] 永远绕开锁定项，而本条**只写那一枚布尔、
    /// 一行也不少、一份文件也不走**。所以 [`Self::history_locks`] 单独一枚，一次也不并进
    /// [`Self::history_deletes`] 或 [`Self::history_clears`]。
    ///
    /// 不碰的账同 [`Self::delete_history`]，理由也同：[`Self::history_records`]、
    /// [`Self::history_last_id`]、[`Self::history_list`]、[`Self::message`]。库里那一行现在
    /// 到底 locked 没有，本条不替它下结论 —— 那是判据自己去问盘上的事（P99 的
    /// `locked_of` 就在问这一件）。
    pub fn lock_history(&mut self, id: i64, locked: bool) -> Option<()> {
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_lock_last = "锁没跑：这一场没有历史可锁。".to_string();
            return None;
        };
        let history = match crate::settings::history_open_in(&dir, &cfg) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.history_lock_last =
                    "锁没跑：`[history] enabled` 是关的，库里没有可锁的行。".to_string();
                return None;
            }
            Err(e) => {
                self.history_lock_last = format!("锁没跑：打不开 {dir:?} 里的 history.db（{e}）。");
                return None;
            }
        };
        match history.set_locked(id, locked) {
            Ok(()) => {
                self.history_locks += 1;
                self.history_lock_last = if locked {
                    format!("锁上了历史第 {id} 行，自动清理与清空都不许再动它。")
                } else {
                    format!("解开了历史第 {id} 行，它重新回到可被清理的那一批里。")
                };
                Some(())
            }
            Err(e) => {
                self.history_lock_last =
                    format!("锁没跑成：{dir:?} 的库里第 {id} 行改不动（{e}）。");
                None
            }
        }
    }

    /// §5.14.3 step 4「恢复该截图内容或**重新进入原选区**」的第一条产品腿（P102 起）：把历史
    /// 里那一行的原选区交回遮罩，让人回到当初画的那个框。
    ///
    /// 前面三节各自的腿都已经证明过"那一行确实带着自己的 rect"（[`Self::record_history`] 把
    /// [`Self::hole`] 原样记进去），而"回去"这一臂至今只能靠用户重画一遍 —— 本条就是那一臂。
    /// 落点只有 [`Self::set_hole`] 一处：标注层的 clip 跟着选区走是它自己早已立下的规矩
    /// （§5.7.1 的「在选区内绘制」靠的就是这条），本条不另起第二套，也不改那条。
    ///
    /// **读的一头，一条也不写**：库里那一行不少、盘上一份文件也不走，所以
    /// [`Self::history_records`]、[`Self::history_replays`]、[`Self::history_deletes`]、
    /// [`Self::history_clears`]、[`Self::history_locks`] 五枚计数一次也不加，加的是
    /// [`Self::history_restores`] 这枚新的。不并进 `history_replays` 的理由与 P97 把它单列
    /// 时同一个：那一条往盘上多落一张图，这一条一张也不落，混在一个数里就分不出哪张是哪件。
    ///
    /// 本条**不**拿 `cancelled` 当门槛：§5.14.3 那一节问的确实是标着「已取消」的那一条，但
    /// "把这个框交回遮罩"这个动作对没取消的行同样成立，把它做成前置检查就等于让界面去猜哪一
    /// 行"配"被恢复。那一行标没标取消，改为如实写进 [`Self::history_restore_last`] 那句话。
    ///
    /// 五格失败各说各话，与 [`Self::replay_history`] 同一口径：「这一场没有历史」「`enabled`
    /// 是关的」「库打不开」「库里没有第 N 行」「那一行的原选区是空的」——把坏库讲成"没有那一
    /// 行"会把一次故障说成一次正常翻页，而把空 rect 交回遮罩会等于**清掉**用户当前的选区，
    /// 那是 §5.14.3 那句话从来没有说过的动作。[`Self::message`] 一律不动，留给落盘那一句。
    pub fn restore_history_selection(&mut self, id: i64) -> Option<falcon_core::PhysRect> {
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_restore_last = "恢复没跑：这一场没有历史可读。".to_string();
            return None;
        };
        let history = match crate::settings::history_open_in(&dir, &cfg) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.history_restore_last =
                    "恢复没跑：`[history] enabled` 是关的，库里没有可读的行。".to_string();
                return None;
            }
            Err(e) => {
                self.history_restore_last =
                    format!("恢复没跑：打不开 {dir:?} 里的 history.db（{e}）。");
                return None;
            }
        };
        let rec = match history.get(id) {
            Ok(Some(rec)) => rec,
            Ok(None) => {
                self.history_restore_last = format!("恢复没跑：{dir:?} 的库里没有第 {id} 行。");
                return None;
            }
            Err(e) => {
                self.history_restore_last = format!("恢复没跑：第 {id} 行读不回来（{e}）。");
                return None;
            }
        };
        if rec.rect.is_empty() {
            self.history_restore_last = format!(
                "恢复没跑：第 {id} 行记的原选区是空的，交回去等于把当前这一框清掉，那不是恢复。"
            );
            return None;
        }
        self.set_hole(rec.rect);
        self.history_restores += 1;
        // 这一场界面自此**停在**这一行上：下一次翻页问的是它的左右，而不是这一场刚记成的
        // 那一行。放在 set_hole 之后、成功话之前 —— 失败的那五格一条也不走这一行，所以
        // "按了一下但什么都没换成"不会伪造出一个锚点。
        self.history_viewed = Some(id);
        self.history_restore_last = if rec.cancelled {
            format!(
                "重新进入历史第 {id} 行的原选区：{}（那一行标着已取消，正是 §5.14.3 要找回的那一条）。",
                rec.rect
            )
        } else {
            format!("重新进入历史第 {id} 行的原选区：{}。", rec.rect)
        };
        Some(rec.rect)
    }

    /// §5.14.2 step 2 翻页那两条腿的落点（P113）：把「翻到那一行」整件事交回
    /// [`Self::restore_history_selection`]，只在它**回成**之后多按一枚 [`Self::history_flips`]。
    ///
    /// 这一条不另开通路：什么算回成了、五格失败各说什么话、锚点怎么跟着走、盘上一份也不写 ——
    /// 全部由 [`Self::restore_history_selection`] 那一处说了算，本条只加一个数。翻页因此不会长成
    /// 第二套"回成"的定义（§5.14.2 的翻页与 §5.14.3 的重新进入原选区在产品里对的是同一件事）。
    ///
    /// 加数只挂在 `Some` 那一支：一次被拒的翻页（`neighbor_id_for` 给了行号而那一行恰不在库里、
    /// 或那一行的原选区是空的）连 [`Self::history_restores`] 也不加，本条因此同样不加 ——「按了
    /// 一下但什么都没换成」不算翻过一页。
    pub fn flip_history_selection(&mut self, id: i64) -> Option<falcon_core::PhysRect> {
        let back = self.restore_history_selection(id);
        if back.is_some() {
            self.history_flips += 1;
        }
        back
    }

    /// §5.14.3 step 4「恢复该截图内容**或**重新进入原选区」里**内容**那一半的第一条产品腿，P116 起；
    /// 同一张图也正是 §5.14.2 step 4「用户可重新复制、保存、标注或贴图」里**重新复制**那半的第一位
    /// 产品读者（[`falcon_core::history::History::frame`] 自 §6.2 建库起只有单测与
    /// [`Self::replay_history`] 两个读者）。
    ///
    /// 交回的是库里那一行**自己存着的那张图**，不是此刻屏幕上那块区域：唯一的来路是
    /// [`falcon_core::history::History::frame`]，本条一处也不碰 [`Self::delivered`]。这不是省了一步，
    /// 是这一格的定义 ——「恢复内容」要的正是那张已经不属于此刻那块桌面的图，走缝合那一位会把当前
    /// 屏幕当成历史递出去。P97 的 [`Self::replay_history`] 是同一件事落在**盘**上的形状，本条是它落在
    /// **剪贴板**上的形状，两枚计数各归各的正是为此。
    ///
    /// 三道梯的顺序抄 [`Self::copy_out`]：先把那一行读回来（读不回来就一个字也不动剪贴板），再探
    /// [`platform_windows::clip::usable`]（PRD §7.2 要的是占用者的名字，不是 `Access is denied.`），
    /// 最后才写。为什么不动 [`Self::hole`] 也不动 [`Self::history_viewed`]：这一格交给用户的是一张图，
    /// 屏幕上那只框还停在原处，锚点也就还停在原处 —— 把「复制了第 N 行」写成「界面停在第 N 行」，
    /// 一次复制就会伪造出一页翻页（[`Self::history_viewed`] 头上那条理由，与它不复用
    /// [`Self::history_last_id`] 是同一条）。
    ///
    /// **仍未落地的另一半，此处点名而不代办**：回标注编辑器。那一半要把遮罩上**看得见**的那张图也
    /// 换成这一行的图，而屏幕上的纹理来自 provider 的实时帧（[`Self::slots`] 与 `frame_key`），只换递出去的那一张、不换看见的那一张，会让「看到的」和「递出去的」
    /// 第二次分家 —— 那正是 [`Self::delivered`] 头上整块代码从 P58 起要防的形状。所以那一半连着 QML
    /// 一起做，本条不半搭。
    pub fn copy_history_content(&mut self, id: i64) -> bool {
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_copy_last = "内容没递：这一场没有历史可读。".to_string();
            return false;
        };
        let history = match crate::settings::history_open_in(&dir, &cfg) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.history_copy_last =
                    "内容没递：`[history] enabled` 是关的，库里没有可读的行。".to_string();
                return false;
            }
            Err(e) => {
                self.history_copy_last =
                    format!("内容没递：打不开 {dir:?} 里的 history.db（{e}）。");
                return false;
            }
        };
        let rec = match history.get(id) {
            Ok(Some(rec)) => rec,
            Ok(None) => {
                self.history_copy_last = format!("内容没递：{dir:?} 的库里没有第 {id} 行。");
                return false;
            }
            Err(e) => {
                self.history_copy_last = format!("内容没递：第 {id} 行读不回来（{e}）。");
                return false;
            }
        };
        // 「库里没有第 N 行」与「那一行在、图取不回来」是两件事：后者正是 §5.14.4 的自动清理或
        // 一份被删掉的原图会走到的那格，说成「没有这一行」会把一件坏了的事讲成一次正常的空。
        let frame = match history.frame(&rec) {
            Ok(frame) => frame,
            Err(e) => {
                self.history_copy_last =
                    format!("内容没递：第 {id} 行的图取不回来（{e}），库里那一行还在。");
                return false;
            }
        };
        if let Err(e) = platform_windows::clip::usable() {
            self.history_copy_last =
                format!("{e}——第 {id} 行的图没递出去，剪贴板上此刻那份东西没动。");
            return false;
        }
        match platform_windows::clip::write_image(&frame) {
            Ok(()) => {
                self.history_copies += 1;
                self.history_copy_last = if rec.cancelled {
                    format!(
                        "把历史第 {id} 行的图递回剪贴板：{}×{}（那一行标着已取消，正是 §5.14.3 要找回内容的那一条）。",
                        frame.width, frame.height
                    )
                } else {
                    format!(
                        "把历史第 {id} 行的图递回剪贴板：{}×{}。",
                        frame.width, frame.height
                    )
                };
                true
            }
            Err(e) => {
                self.history_copy_last = format!("{e}——第 {id} 行的图没递出去。");
                false
            }
        }
    }

    /// 翻页那两条腿该问谁的左右（§5.14.2，P113 起）：界面这一场停在第几行，就那一行为准；
    /// 没停过就用这一场最近**记成**的那一行；两者都没有 ⇒ `-1`，意思是"没有锚点可问"。
    ///
    /// 为什么回退到 [`Self::history_last_id`] 而不是直接 `None`：§5.14.2 写的是"用方向键在
    /// 记录间翻页"，而截图刚落地时界面本来就在那一行的位置上看它 —— 那一刻的"上一行"确实是
    /// 它的上一行。为什么这不是把两枚并成一枚：读的方向只有这一个（`history_viewed` 优先），
    /// 写的方向各归各格（记成走 `history_last_id`、翻页走 `history_viewed`），
    /// 所以"行数加一"那枚证人不受翻页影响。
    ///
    /// 纯读，不动任何状态，也不碰库 —— 库那一头归 [`crate::settings::neighbor_id_for`]。
    pub fn history_anchor(&self) -> i64 {
        self.history_viewed.or(self.history_last_id).unwrap_or(-1)
    }

    /// 这一场真正**开着**的那座库是哪一座：与 [`Self::restore_history_selection`]、
    /// [`Self::replay_history`] 用的是同一对 `(dir, cfg)`，即 [`Self::history`] 与
    /// [`Self::history_dir`] 这两枚**会话快照**。
    ///
    /// 这一枚 getter 存在的理由是它挡的不是重复而是**分岔**：`Session` 的翻页腿自己会去问
    /// `crate::settings::history_data_dir()`，而那是全局 config 的答案、不是这一场遮罩的答案。
    /// 两者在真实用户那一头永远相等（`open()` 就是从同一处取的快照），所以这条分岔在单测与
    /// 手工使用里都读不出来；而 `--autosave` 那档的 `retarget()` 只把 `MaskState` 这一对重钉进
    /// 沙盒 ⇒ P113 第一次跑门时，行号是从**另一座空库**问出来的，交回遮罩的那一句当场是
    /// 「库里没有第 3 行」。一行行号必须只能从它要被读回的那座库里取。
    ///
    /// `None` 说的是这一场没有可读的库（`history` 缺、或落点是空串）—— 与 [`Self::history_anchor`]
    /// 那枚 `-1` 各判各的：`-1` 是「还没有锚点可翻」，`None` 是「连可开的库都没有」。
    pub fn history_store(&self) -> Option<(std::path::PathBuf, falcon_core::config::History)> {
        let dir = (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone())?;
        Some((dir, self.history.clone()?))
    }

    /// Enter or a double-click. `None` when there is no selection to confirm - the
    /// mask stays up rather than inventing a crop the user never drew.
    ///
    /// A confirmed selection is also this process's answer to "where was the last
    /// one", which is what §5.2.5's 重复上次截图 reads. It is recorded here rather
    /// than where the pixels are written because the mask ends in a copy or a file,
    /// not in a pin: the region is the thing this flow produces.
    ///
    /// §5.5.1 step 3 说的是"按默认行为复制、保存、贴图或关闭"，而 §5.5.3 step 4 说的
    /// "完成截图后，系统自动生成文件"里的那个"完成"就是这一格：`Enter`、双击、和
    /// [output] auto_save 三样都在这道门上会合。写文件的一步不在这里另起一套，而是
    /// 交给 §5.5.4 已经在用的 [`Self::quick_out`] —— 同一张拼好的图、同一个命名规则、
    /// 同一句"已保存到 …"，所以开着自动保存的一次 `Enter` 和按一次快速保存写到的是同
    /// 一个地方。顺序也是定的：先记下选区，再落盘。一次失败的写不该让用户上一条选区
    /// 跟着丢，而 §5.5.3 的规则恰恰要求失败时把原因说清楚并且留下别的东西。
    /// 同一道门上也汇合的第三样，P59 起：`[output] auto_copy` —— §5.5.1 step 3 那句
    /// "系统按默认行为复制"。顺序是**先复制、后落盘**，理由写在 §5.5.3 的规则里："保存失败时
    /// 应提示原因，并**保留剪贴板内容**或临时文件"——一次已经递进剪贴板的复制，是那次失败的
    /// 写唯一还能留给用户的东西，反过来做就等于要求失败时留下一件从来没存在过的东西。
    ///
    /// 两格各说各的话：复制写 [`Self::auto_copy_last`]，保存写 [`Self::message`]，历史写
    /// [`Self::history_last`]。合并成一句的话会输掉一整格证据——"落盘成了而复制被系统拒了"
    /// 与"两句都成"在一条句子里长得一样。
    ///
    /// 同一道门汇合的第四样，P93 起：§5.14.1 的那一行历史。它排在**最前**，与复制/保存的
    /// 顺序理由相反但同源，写在 [`Self::record_history`] 头上。
    pub fn commit(&mut self) -> Option<PhysRect> {
        self.grab = None;
        let hole = (!self.hole.is_empty()).then_some(self.hole);
        if let Some(rect) = hole {
            capture::remember(rect, "遮罩选区");
            // §5.4.3：`include_cursor` 的那一问自 P107 起**不在这道门上**。这道门只把画面冻住
            // 那一格读到的答案接进状态（[`crate::capture::frozen_cursor`]），不再当场问桌面：
            // 覆盖层属于它被读走时那一枚画面，而指针在 `open` 与 `Enter` 之间是走的。取在
            // `remember` 之后这一件没变：`remember` 管的是「再截一次」要不要复用上一枚矩形，
            // 与指针谁也不挡谁。
            self.cursor = capture::frozen_cursor();
            self.record_history(rect, false);
            if self.auto_copy {
                self.auto_copy_out();
            }
            if self.auto_save {
                self.quick_out();
            }
        }
        hole
    }

    pub fn has_selection(&self) -> bool {
        !self.hole.is_empty()
    }

    /// §5.4.3 的算术本体：把一枚光标图按**选区左上角为原点**贴进一张已经拼好的图。
    ///
    /// `hole` 是屏幕坐标下的选区，`at` 是光标图左上角的屏幕坐标 ⇒ 目的坐标是
    /// `at - hole.origin`；选区外的部分由 [`falcon_core::frame::Frame::blend_over`] 自己的
    /// 裁剪处理，不在这里手写第二遍边界（手写那一遍是「半个光标」这类 bug 的来历）。
    ///
    /// `opacity` 是**调用方交进来的**而不是这里读的：这一格只管算术，键在
    /// [`crate::capture::cursor_overlay`] 那一头读。抽成关联函数而不是塞进 [`Self::delivered`]
    /// 的理由与 [`Self::grab`] 同一句 —— 这部分算术可以不开窗就验。
    pub fn stamp_cursor(
        mut raw: falcon_core::frame::Frame,
        img: &falcon_core::frame::Frame,
        hole: falcon_core::geometry::PhysRect,
        at: falcon_core::geometry::PhysPoint,
        opacity: f64,
    ) -> falcon_core::frame::Frame {
        raw.blend_over(
            img,
            falcon_core::geometry::PhysPoint::new(at.x - hole.x, at.y - hole.y),
            opacity,
        );
        raw
    }

    /// The stitched selection with §5.5.6's corner cut and §5.5.7's plate on it: the one
    /// picture every door hands over, so 复制, 保存, 快速保存 and 存完再钉图 cannot answer
    /// one selection with four different files.
    ///
    /// Both halves of the decoration are the identity when the settings say nothing,
    /// which is what keeps every door that already has a 判据 passing the same bytes it
    /// passed before this existed - the `[output]` keys had no reader, and now they have
    /// exactly one.
    ///
    /// `None` is [`crate::annotate::Layer::deliver`]'s "nothing to stitch" *plus* the one way a
    /// legal setting can still fail: a whole-desktop selection with a 96 px margin is
    /// arithmetic that can leave [`falcon_core::frame::MAX_DIM`] behind. The doors'
    /// sentences ("截图还在，可以再复制一次") stay true for both readings, because in each
    /// one the answer is that no picture left this flow.
    ///
    /// 光标那一格排在装饰**之前**、拼好的原图**之后**：§5.4.3 要的是"指针停在被截的画面
    /// 上"，而 §5.5.6/§5.5.7 的圆角与铭牌是**这张画面之外**的东西 —— 反过来做的话一枚贴着
    /// 选区边缘的光标会被圆角裁掉半只，而那不是用户看见过的样子。`self.cursor` 是 `None`
    /// 时这一格整个跳过，所以 §5.4.3 的默认那一侧（键关着）与本轮之前的字节完全同形。
    pub fn delivered(&self) -> Option<falcon_core::frame::Frame> {
        let mut raw = self.ink.deliver()?;
        if let Some((img, at)) = &self.cursor {
            raw = Self::stamp_cursor(raw, img, self.hole, *at, 1.0);
        }
        if self.decor.is_identity() {
            return Some(raw);
        }
        self.decor.apply(&raw).ok()
    }

    /// §5.5.2's 复制: the annotated selection onto the clipboard, with a sentence
    /// beside it either way.
    ///
    /// Three rungs, in this order, because each costs what the one above it says is
    /// worth paying. Nothing selected is the user's own doing and needs no probe. The
    /// probe (`clip::usable`) runs before any pixel is moved, because a clipboard
    /// another process owns is an environment fact the user can act on - and PRD §7.2
    /// asks for the holder's name rather than "Access is denied". Only then does the
    /// write happen, through [`crate::annotate::Layer::deliver`], which is the one
    /// place a selection is stitched: what lands on the clipboard here is byte-for-byte
    /// what §5.5.4's 保存 will write to a file.
    ///
    /// This does **not** end the flow. The PRD stops at "系统将图片写入剪贴板"
    /// (§5.5.2 step 3), and closing has a cost this project is not entitled to pay on
    /// the user's behalf: the freeze, the ink and the message all go with the mask, so
    /// a copy that closed the overlay would take its own answer with it. Ending the
    /// capture is 取消's and, from M4b, 完成的.
    pub fn copy_out(&mut self) -> bool {
        if !self.has_selection() {
            self.message = "还没有选区：先把要复制的范围拖出来。".to_string();
            return false;
        }
        let Some(frame) = self.delivered() else {
            self.message = "选区的画面拼不出来：截图还在，可以再复制一次。".to_string();
            return false;
        };
        if let Err(e) = platform_windows::clip::usable() {
            self.message = format!("{e}。截图还在，可以先关掉占用它的程序再复制。");
            return false;
        }
        match platform_windows::clip::write_image(&frame) {
            Ok(()) => {
                self.message = format!("已复制 {}×{} 像素。", frame.width, frame.height);
                true
            }
            Err(e) => {
                self.message = format!("{e}。截图还在，可以再复制一次。");
                false
            }
        }
    }

    /// §5.5.1 step 3 的默认行为那一次复制：同三道梯、同一个所有者，只是不占 [`Self::message`]。
    ///
    /// 为什么不直接叫 [`Self::copy_out`]：那一扇是**有人按的**，它把答案写进 `message`，而
    /// 一次没人按过的自动复制会把同一刻自动保存那句"已保存到 …"顶掉 —— §5.5.3 的规则要的
    /// 恰恰是两句证据同时在场。为什么不在这里再拼一次图：[`Self::delivered`] 是全仓库唯一
    /// 一处缝合，复制、保存、快速保存与存完再钉图答的是同一张图。
    ///
    /// 三道梯里少了第一道（"还没有选区"）不是漏了检查，是这一扇到不了那一格：唯一的调用者
    /// [`Self::commit`] 已经先答过 `hole.is_empty()`。剩下的两梯顺序与 `copy_out` 一致 ——
    /// 先探（`usable`，PRD §7.2 要的是占用者的名字），后写。
    fn auto_copy_out(&mut self) -> bool {
        let Some(frame) = self.delivered() else {
            self.auto_copy_last = "选区的画面拼不出来：默认行为没递任何东西。".to_string();
            return false;
        };
        if let Err(e) = platform_windows::clip::usable() {
            self.auto_copy_last = format!("{e}——默认行为没递东西，截图照旧在。");
            return false;
        }
        match platform_windows::clip::write_image(&frame) {
            Ok(()) => {
                self.auto_copies += 1;
                self.auto_copy_last =
                    format!("已按默认行为复制 {}×{} 像素。", frame.width, frame.height);
                true
            }
            Err(e) => {
                self.auto_copy_last = format!("{e}——默认行为没递东西，截图照旧在。");
                false
            }
        }
    }

    /// §5.5.5's 另存为: the annotated selection to a file, in the format the name the
    /// user typed says, through the dialog step 2 of that section asks for.
    ///
    /// The first three rungs are [`Self::copy_out`]'s, in the same order and for the
    /// same reasons - nothing selected is the user's own doing and needs no probe, and
    /// a selection that cannot be stitched is a refusal that has to be said *before*
    /// anything is written, not a file that turns out to be nothing. What is new is
    /// only what happens to the bytes: [`falcon_core::encode`] rather than
    /// `QImage::save` - P5's rule for every byte this app puts in a file - and
    /// `encode::save`'s own tmp+rename, because half a PNG on disk is worse than a
    /// refusal (PRD §8.3). The frame that goes in is the one `deliver()` stitched, so
    /// what 复制 put on the clipboard and what 保存 put on disk are the same picture
    /// rather than two answers to the same selection.
    ///
    /// The name, the format, the count and the sentence are [`crate::save`'s`] - the pin
    /// window's 另存为 reads through the same owner, so the two doors cannot say two
    /// different things about one picture again (P35: they had, and one of them said it
    /// in English while ignoring `[output] jpeg_quality`).
    ///
    /// The fourth sentence is the reason P34 exists. Three rows of the PRD ask
    /// for it - §5.5.5 line 506 ("包含透明背景时，应推荐 PNG"), §5.4.5's line 447
    /// ("不支持透明度的输出格式应提醒用户背景将被填充") and §5.5.6's line 521 - and all
    /// three are true of the same write, because `flatten_on_lossy` paints those pixels
    /// white and the file is then not the picture the user was looking at. The count
    /// travels with the sentence: "有透明区域" alone is boilerplate, and P33 measured
    /// what one selection's eraser actually costs (`11684 px`), so the number is the
    /// difference between a prompt and a shrug.
    ///
    /// The write still happens. The extension was the user's own choice in a file
    /// dialog, this mask has one message line and no modal, and refusing a legitimate
    /// format is a worse reading of "应提醒" than doing the thing and naming what it
    /// cost. A pre-write confirm rung - the other way to read those lines - is a
    /// decision this round records rather than makes.
    ///
    /// `chosen` is the extension text of the filter the dialog was showing when the name
    /// was typed, and it decides a name that carries no extension. PRD §5.5.5's third
    /// step has the user choosing 目录、文件名 **和格式** in that same dialog, so the
    /// filter outranks `[output] format` - the configured one is what is left when the
    /// dialog has offered no format to honour (§10 第 40 项 ②).
    ///
    /// Like 复制, this does not end the flow: the freeze, the ink and the sentence all
    /// go with the mask, and a save that closed the overlay would take the picture the
    /// user may still want to copy with it.
    /// [`Self::save_out`]'s first rung on its own, so the order is *callable*: the gauge
    /// asks what the notice said at the moment the file it warns about did not exist yet.
    /// Returns the sentence (empty when this format carries alpha).
    pub fn remind_before_save(&mut self, path: &str, chosen: &str) -> String {
        let Some(frame) = self.delivered() else {
            self.pre_notice.clear();
            return String::new();
        };
        self.pre_notice =
            crate::save::reminder(&frame, path, chosen, &self.export, "选区").unwrap_or_default();
        self.pre_notice.clone()
    }

    pub fn save_out(&mut self, path: &str, chosen: &str) -> bool {
        if !self.has_selection() {
            self.message = "还没有选区：先把要保存的范围拖出来。".to_string();
            return false;
        }
        let Some(frame) = self.delivered() else {
            self.message = "选区的画面拼不出来：截图还在，可以再保存一次。".to_string();
            return false;
        };
        // 写之前先说一次：PRD 447 的措辞是"将被填充"，而 [`crate::save::write`] 一回来
        // 就把那句话变成"已填成白色"。这一格是提醒，不是确认 —— 照写。
        self.pre_notice =
            crate::save::reminder(&frame, path, chosen, &self.export, "选区").unwrap_or_default();
        match crate::save::write(&frame, path, &self.export, "选区", chosen) {
            Ok(said) => {
                self.message = said;
                true
            }
            Err(crate::save::Refusal::BadName(e)) => {
                self.message = e;
                false
            }
            Err(crate::save::Refusal::Write(e)) => {
                self.message = format!("{e}。截图还在，可以再保存一次。");
                false
            }
        }
    }

    /// §5.5.4's 快速保存: the same picture [`Self::save_out`] writes, into the file the
    /// settings name, with no dialog in between.
    ///
    /// The first two rungs are `save_out`'s in the same order, and for the same reasons:
    /// nothing selected is the user's own doing and needs no probe, and a selection that
    /// cannot be stitched has to be refused *before* a file appears. What this door owns
    /// that `save_out` does not is the name: `[output] naming_template` and
    /// `naming_prefix`, this flow's 序号 for `%i`, the extension from `[output] format`,
    /// and a taken name settled by `[output] collision` - §5.5.3's three rules, which
    /// had an owner in [`falcon_core::naming`] and a full set of tests and, until this
    /// door, no caller anywhere in the product.
    ///
    /// The sentence is [`crate::save::write_decided`]'s, shared with the dialog door down
    /// to the transparency count: 同一句 is the whole reason §5.5.4's success reads like
    /// §5.5.5's rather than like a third thing, and P35 is why the sharing is done by one
    /// owner rather than by two doors agreeing to copy each other.
    ///
    /// The clock is read at the save rather than at `open`, because a name belongs to the
    /// write: a mask held open ten minutes and then quick-saved is filed under the minute
    /// it was saved at, not the minute the desktop was frozen. `wall_clock` gives the
    /// offset as well as the epoch second, which is [`naming::LocalStamp`]'s documented
    /// division of labour - core has no timezone database.
    ///
    /// §5.5.4 step 4 asks for a notification the user can *click* to open the file or its
    /// directory. The click is [`Self::reveal_landed`], and what it needs from this door
    /// is the path rather than the sentence - which is why [`Self::landed`] exists and
    /// why the `PathBuf` `quick_to` already returned is no longer dropped here.
    /// `[output] reveal_after_save` is the other half of that step: off (its `Default`)
    /// the file is only ever opened by a finger, on it lands here once per save, from the
    /// same code the click goes through. What is still carried is `%B`, empty until
    /// something other than a second monitor enumeration decides which screen a picture
    /// came from.
    pub fn quick_out(&mut self) -> bool {
        if !self.has_selection() {
            self.message = "还没有选区：先把要快速保存的范围拖出来。".to_string();
            return false;
        }
        let Some(frame) = self.delivered() else {
            self.message = "选区的画面拼不出来：截图还在，可以再保存一次。".to_string();
            return false;
        };
        let Some(dir) = self.naming.directory() else {
            self.message =
                "快速保存没有目录可写：这台机器说不出图片文件夹在哪里，请在设置里填 [output] save_dir。"
                    .to_string();
            return false;
        };
        let (epoch, offset_min) = platform_windows::clock::wall_clock();
        let facts = naming::NameFacts {
            width: frame.width,
            height: frame.height,
            monitor: "",
            tool: self.ink.tool().map(|kind| kind.key()).unwrap_or(""),
        };
        // §5.5.8 "覆盖原图" —— 同一次 mask 会话里的第二次及以后，直接写到上一次落下的那份
        // 文件；命名引擎不参与、`quick_saves` 仍然加一，但 `self.landed` 保持不动。
        if self.reuse_last_file {
            if let Some(prev) = self.landed.clone() {
                let format = self.export.format;
                self.pre_notice = crate::save::reminder_for_format(&frame, format, "选区");
                return match crate::save::write_decided(&frame, prev, format, &self.export, "选区")
                {
                    Ok(landed) => {
                        self.quick_saves += 1;
                        self.landed = Some(landed.0.clone());
                        self.message = landed.1;
                        crate::settings::record_landed(&landed.0);
                        if self.reveal_after_save {
                            self.reveal_auto();
                        }
                        true
                    }
                    Err(crate::save::Refusal::BadName(e)) => {
                        self.message = e;
                        false
                    }
                    Err(crate::save::Refusal::Write(e)) => {
                        self.message = format!("{e}。截图还在，可以再保存一次。");
                        false
                    }
                };
            }
        }
        let quick = crate::save::Quick {
            dir: &dir,
            template: &self.naming.template,
            prefix: &self.naming.prefix,
            seq: self.quick_saves + 1,
            collision: &self.naming.collision,
        };
        // 同一句提醒的另一扇门：快速保存没有对话框，也就没有"用户自己选了扩展名"这一步，
        // 而它照样可能把透明像素填白（`[output] format` = jpg）。措辞与上一格同一个所有者。
        self.pre_notice = crate::save::reminder_for_format(&frame, self.export.format, "选区");
        let landed = match crate::save::quick_to(
            &frame,
            naming::LocalStamp::from_epoch(epoch, offset_min),
            facts,
            &quick,
            &self.export,
            "选区",
        ) {
            Ok(landed) => landed,
            Err(crate::save::Refusal::BadName(e)) => {
                self.message = e;
                return false;
            }
            Err(crate::save::Refusal::Write(e)) => {
                self.message = format!("{e}。截图还在，可以再保存一次。");
                return false;
            }
        };
        self.quick_saves += 1;
        self.landed = Some(landed.0.clone());
        self.message = landed.1;
        crate::settings::record_landed(&landed.0);
        if self.reveal_after_save {
            self.reveal_auto();
        }
        true
    }

    /// PRD 491 step 4 的那一次点击：通知上按下去，把刚存的那张图在它自己的目录里亮出来。
    ///
    /// 计数在**动手之前**加，不在成功之后加：这一格要判的是"点了几下"与"递出去几次"
    /// 是不是同一个数，而一个失败了却不计数的点击会把两次真点击读成一次。
    pub fn reveal_landed(&mut self) -> bool {
        self.reveal_clicks += 1;
        self.reveal_now()
    }

    /// 这一次运行是不是量具。判据取 argv 而不是取某一道腿的记忆，因为要挡的那一步
    /// （[`platform_windows::reveal::select`]）在腿的下头：腿记不记得自己是腿，不该决定
    /// 用户的桌面会不会多一个窗。前缀匹配，所以 `--mask-soft` 与 `--mask` 同判。
    ///
    /// 第 46 项 ③ 那一档要能对着一句"我不是量具"作保，所以它是 `pub(crate)`：那一档的第一格
    /// 读的就是这个函数的答案，而不是自己 argv 里有没有那个字符串——两处各读一次，就会有一处
    /// 在名单被改动的下一趟变成谎话。
    pub(crate) fn gauge_run() -> bool {
        std::env::args().any(|a| GAUGE_FLAGS.iter().any(|name| a.starts_with(name)))
    }

    /// `[output] reveal_after_save` 开着时，落盘自己走的那一次。和点击分开计，因为
    /// "开关开着却一次也没自动来"要能被说出来。
    fn reveal_auto(&mut self) -> bool {
        self.reveal_auto += 1;
        self.reveal_now()
    }

    /// 两条路共用的那一步。问盘（[`platform_windows::reveal::plan`]）与开口
    /// （[`platform_windows::reveal::select`]）在 reveal 那一层就是分开的两个函数，
    /// 所以量具问前半段不碰用户的桌面，后半段才需要谁同意。
    ///
    /// 而"不碰"这一半自此不再靠各条腿自己安排：`select` 之前有一道 [`Self::gauge_run`]。
    /// 理由不是"问盘更干净"，是 §10 第 38 项 ① 那条收尾规则 —— 探针必须能关掉自己留下的
    /// 东西 —— 而资源管理器那个窗不归本进程管。开关是用户配置里的一个布尔，量具跑哪一档、
    /// `--ink` 那五行快速保存会把落盘那一步重复几次，都不是这一格能替用户决定的事；把它交给
    /// 腿的顺序去保证，等于保证一枚没人复核的配置行不会说话。
    fn reveal_now(&mut self) -> bool {
        let Some(path) = self.landed.clone() else {
            self.reveal_last = "这张图还没有落到任何地方：通知没有可以去打开的文件。".to_string();
            return false;
        };
        let target = match platform_windows::reveal::plan(&path) {
            Ok(target) => target,
            Err(e) => {
                self.reveal_last = e.to_string();
                return false;
            }
        };
        if Self::gauge_run() {
            self.reveal_last = format!(
                "未问：{} 里那张 {} 已经问出来了，但量具不开这个窗。",
                target.folder.display(),
                target
                    .item
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            );
            return false;
        }
        match platform_windows::reveal::select(&target) {
            Ok(()) => {
                self.reveal_acted += 1;
                self.reveal_last =
                    format!("已经在 {} 里把这张图选出来了。", target.folder.display());
                true
            }
            Err(e) => {
                self.reveal_last = e.to_string();
                false
            }
        }
    }

    /// PRD 433 / 459's 贴图, and §10 第 43 项 ①'s whole reason: the picture goes to a
    /// file *first* and the pin is made **from that file**, so what floats on screen
    /// afterwards is the artifact rather than a second reading of the selection.
    ///
    /// The rungs are [`Self::save_out`]'s through the write, then one more that
    /// `save_out` does not have: [`crate::state::PinState::add_from_file`] at the
    /// selection's own corner. That last step is why this door returns two acts rather
    /// than one verdict. A file that landed and a pin that refused is a state the user
    /// has to be told about - the picture exists, it is just not floating - and a door
    /// that answered `false` with only the pin's sentence would leave them disbelieving
    /// the file, which is the same shape of lie as the 保存 that reported 透明 as kept.
    ///
    /// The pin is placed so that the **picture** lands on `hole`'s top-left, because that
    /// is where the user's rectangle already is: 存完再钉图 that moves the picture is a
    /// different feature. §5.5.7 makes that sentence need arithmetic: a shadow is part of
    /// the file rather than of the window, so the canvas grows *outward* by
    /// [`falcon_core::imageops::Decoration::extra`] on every side and the file's own
    /// corner has to move out by the same number for the picture to stay put. The shift
    /// stops at the desktop's top-left, because a pin nobody can reach is a worse reading
    /// than a halo clipped at the edge.
    /// The physical point is the same space [`crate::capture::pin_rect`] spawns in, so
    /// the pin's geometry arrives by the route a capture-pinned rectangle already uses.
    pub fn save_then_pin(&mut self, path: &str, chosen: &str) -> bool {
        if !self.has_selection() {
            self.message = "还没有选区：先把要钉的范围拖出来。".to_string();
            return false;
        }
        let Some(frame) = self.delivered() else {
            self.message = "选区的画面拼不出来：截图还在，可以再保存一次。".to_string();
            return false;
        };
        self.pre_notice =
            crate::save::reminder(&frame, path, chosen, &self.export, "选区").unwrap_or_default();
        let landed = match crate::save::write_to(&frame, path, &self.export, "选区", chosen) {
            Ok((landed, said)) => (landed, said),
            Err(crate::save::Refusal::BadName(e)) => {
                self.message = e;
                return false;
            }
            Err(crate::save::Refusal::Write(e)) => {
                self.message = format!("{e}。截图还在，可以再保存一次。");
                return false;
            }
        };
        let (path_, said) = landed;
        let grow = self.decor.extra() as i32;
        let at = PhysPoint::new(
            (self.hole.x - grow).max(self.virtual_bounds.x),
            (self.hole.y - grow).max(self.virtual_bounds.y),
        );
        match crate::state::with(|s| s.add_from_file(&path_.to_string_lossy(), at)) {
            Ok(id) => {
                self.message = format!("{said} 也钉成了贴图 {id}。");
                true
            }
            // The file clause stays: the write succeeded, and a refusal that hides
            // that tells the user to do the whole thing again.
            Err(e) => {
                self.message = format!("{said}但贴图没有钉出来：{e}");
                false
            }
        }
    }

    /// §5.7.11's 文本, §5.7.5's 折线 and §5.7.13's 放大 finishers, in that order, in
    /// this one place: 双击 and `Enter` mean four things to this mask (close a box, close
    /// a line, place a copy, crop the picture) and only the layer knows which is pending.
    /// `true` is "the keystroke closed something of mine", which is what tells the caller
    /// the crop is not this press's - and a ladder spelled in QML is two ladders.
    ///
    /// 文本 goes first because it is the one holding the keyboard: while a box is open
    /// `Enter` is *its* key, and a ladder that asked 折线 first would still be right (the
    /// two gestures cannot both be pending, `select_tool` clears them) but would be a
    /// ladder whose order nobody could defend.
    pub fn finish_ink(&mut self) -> bool {
        if self.ink.finish_typing() {
            return true;
        }
        if self.ink.finish_polyline() {
            return true;
        }
        self.ink.finish_zoom()
    }

    /// The live grip as a stable number, so the cursor shape and the hit test cannot
    /// drift apart: 0 = nothing held, 1 = drawing a new rectangle, 2..9 in
    /// `Handle::all()` order (Nw, N, Ne, E, Se, S, Sw, W), 10 = the whole rectangle.
    ///
    /// A stroke being dragged answers `1` as well, because that is the number QML
    /// turns into a crosshair - and a pen *is* drawing at the pointer, whatever the
    /// selection is doing underneath.
    pub fn grip_code(&self) -> i32 {
        if self.ink.dragging() {
            return 1;
        }
        let Some(g) = self.grab.as_ref() else {
            return 0;
        };
        Self::code_of(g.handle)
    }

    /// Which grip a point *would* take, without taking it. QML asks this on hover,
    /// for the cursor shape, and it has to be the same arithmetic `press` runs -
    /// a pointer that lies about what it is about to grab is how a resize quietly
    /// becomes a re-draw.
    pub fn hit_test(&self, index: usize, x: f64, y: f64) -> i32 {
        if self.ink.tool().is_some() {
            return 1;
        }
        let Some(p) = self.desk_point(index, x, y) else {
            return 0;
        };
        if self.hole.is_empty() {
            return Self::code_of(None);
        }
        Self::code_of(handle_at(&self.hole, p, self.hit_slop(index)))
    }

    fn code_of(handle: Option<Handle>) -> i32 {
        match handle {
            None => 1,
            Some(Handle::Body) => 10,
            Some(h) => 2 + Handle::all().iter().position(|x| *x == h).unwrap_or(0) as i32,
        }
    }

    pub fn slot(&self, index: usize) -> Option<&Slot> {
        self.slots.iter().find(|s| s.index == index)
    }

    /// The hole as this window sees it.
    fn hole_for(&self, slot: &Slot) -> PhysRect {
        self.local_dip(slot, &self.hole)
    }

    /// A desktop rect as one window sees it: clipped to the screen, moved to the
    /// screen's origin, divided by its scale.
    ///
    /// The one place that turns a desktop number into a number a mask window can place
    /// an item at. The selection goes through it and so does §5.7.11's text box, because
    /// a second copy of those three steps is a second answer, and the two of them
    /// rounding differently is a field that sits beside its own box.
    fn local_dip(&self, slot: &Slot, desk: &PhysRect) -> PhysRect {
        let Some(local) = desk.intersection(&slot.bounds) else {
            return PhysRect::default();
        };
        PhysRect::new(
            slot.scale.phys_to_dip_i(local.x - slot.bounds.x),
            slot.scale.phys_to_dip_i(local.y - slot.bounds.y),
            slot.scale.phys_to_dip_i(local.w as i32) as u32,
            slot.scale.phys_to_dip_i(local.h as i32) as u32,
        )
    }

    pub fn view_data(&self, index: usize) -> MaskViewData {
        let Some(slot) = self.slot(index) else {
            return MaskViewData::default();
        };
        let typing = self
            .ink
            .typing_rect()
            // The field belongs to the window whose click opened it, and only one window
            // can hold a focus: a box that straddles a screen seam shows its clipped
            // piece on both screens, but the caret goes to the one that was clicked.
            .filter(|r| slot.bounds.contains(PhysPoint::new(r.x, r.y)))
            .map(|r| self.local_dip(slot, &r));
        let typing_live = typing.is_some();
        MaskViewData {
            live: true,
            shown: self.shown,
            geom: slot.geom,
            hole: self.hole_for(slot),
            scale: slot.scale,
            typing: typing.unwrap_or_default(),
            typing_live,
            font_dip: slot.scale.phys_to_dip_i(self.ink.font_px() as i32).max(1) as u32,
            revision: slot.revision,
            swaps: slot.swaps,
            first_swap_ms: slot.first_swap_ms,
            shader_status: slot.shader_status,
            show_dimensions: self.show_dimensions,
        }
    }

    /// One presented frame. Only the first is timed, and the clock starts at
    /// `open`, so this is the number the pre-warm work is measured against.
    pub fn note_swap(&mut self, index: usize) {
        let elapsed = self.started.map(|t| t.elapsed().as_millis() as u64);
        if let Some(slot) = self.slot_mut(index) {
            slot.swaps += 1;
            if slot.first_swap_ms.is_none() {
                slot.first_swap_ms = elapsed;
                // The process's very first presented frame, whenever it lands. With two
                // screens this is the earlier of the two, which is what a user sees; the
                // per-slot `first_swap_ms` beside it stays exact per window.
                crate::state::stamp("swap");
            }
        }
    }

    pub fn note_shader_status(&mut self, index: usize, status: i32) {
        if let Some(slot) = self.slot_mut(index) {
            slot.shader_status = Some(status);
        }
    }

    fn slot_mut(&mut self, index: usize) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| s.index == index)
    }

    /// The desktop the hole is clipped against, as the snapshot described it.
    pub fn desktop(&self) -> PhysRect {
        self.virtual_bounds
    }

    /// One line per slot, for `--mask` to print and assert against.
    pub fn line(&self) -> String {
        let parts: Vec<String> = self
            .slots
            .iter()
            .map(|s| {
                format!(
                    "{} dpr={:.2} dip={}x{}@{},{} hole={:?} swaps={} first={:?} shader={:?}",
                    s.name,
                    s.scale.ratio(),
                    s.geom.w,
                    s.geom.h,
                    s.geom.x,
                    s.geom.y,
                    self.hole_for(s),
                    s.swaps,
                    s.first_swap_ms,
                    s.shader_status
                )
            })
            .collect();
        parts.join(" | ")
    }

    /// The annotation layer's own line, for `--ink` and for the toolbar's tooltip:
    /// which tool holds the pointer, how many objects are in the document, how many
    /// nodes a polyline has been clicked but not finished with, what copy 放大 is
    /// holding before its second drag commits it, what 文本 box is waiting for its
    /// click-away, and what the last repaint cost in pixels and milliseconds. "增量栅格"
    /// is only a claim once the number of pixels it moved is next to it.
    pub fn ink_line(&self) -> String {
        format!(
            "tool={:?} hole={:?} poly={} text={} zoom={} {}",
            self.ink.tool(),
            self.hole,
            self.ink.polyline_nodes(),
            self.ink.typing_line(),
            self.ink.zoom_line(),
            self.ink.paint_line()
        )
    }
}

// ------------------------------------------------------------------- session

static MASK: OnceLock<Mutex<MaskState>> = OnceLock::new();

fn store() -> &'static Mutex<MaskState> {
    MASK.get_or_init(|| Mutex::new(MaskState::default()))
}

pub fn with<R>(f: impl FnOnce(&mut MaskState) -> R) -> R {
    let mut guard = store().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// A mask that opened: the frame it is showing, what freezing it cost, and how
/// many screens it covers.
pub struct Opened {
    pub snap: ScreenSnapshot,
    pub ms: u64,
    pub slots: usize,
}

/// The flow the hotkey will run: freeze, then cover every screen with its frame.
/// Returns the snapshot alongside the slot count, because the `--mask` report has
/// to compare what the mask shows against the same pixels the crop will take.
pub fn open_from_freeze(shader: bool) -> Result<Opened, String> {
    let frozen = capture::freeze()?;
    let slots = with(|m| m.open(&frozen.snap, shader))?;
    Ok(Opened {
        snap: frozen.snap,
        ms: frozen.ms,
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask_view::shim;
    use falcon_core::capture::{FrozenMonitor, MonitorInfo};
    use falcon_core::frame::Frame;

    fn monitor(
        id: &str,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        scale: f64,
        primary: bool,
    ) -> FrozenMonitor {
        FrozenMonitor {
            info: MonitorInfo {
                id: id.into(),
                name: id.into(),
                friendly: id.into(),
                bounds: PhysRect::new(x, y, w, h),
                scale: Scale::of(scale),
                primary,
                builtin: false,
            },
            frame: Frame::new(w, h).expect("frame"),
        }
    }

    fn snapshot(monitors: Vec<FrozenMonitor>) -> ScreenSnapshot {
        let virtual_bounds = monitors.iter().fold(PhysRect::default(), |acc, m| {
            if acc.is_empty() {
                m.info.bounds
            } else {
                acc.union(&m.info.bounds)
            }
        });
        ScreenSnapshot {
            monitors,
            virtual_bounds,
            ..Default::default()
        }
    }

    fn state_with(slots: Vec<Slot>, hole: PhysRect) -> MaskState {
        MaskState {
            slots,
            hole,
            ..Default::default()
        }
    }

    fn slot(index: usize, bounds: PhysRect, scale: f64) -> Slot {
        Slot {
            index,
            name: format!("m{index}"),
            key: format!("m{index}-1"),
            bounds,
            geom: PhysRect::new(
                bounds.x / scale as i32,
                bounds.y / scale as i32,
                (bounds.w as f64 / scale).round() as u32,
                (bounds.h as f64 / scale).round() as u32,
            ),
            scale: Scale::of(scale),
            revision: 1,
            swaps: 0,
            first_swap_ms: None,
            shader_status: None,
        }
    }

    /// The conversion a stretched-or-mis-cropped mask breaks: the hole arrives in
    /// desktop pixels and has to leave as this window's own, divided by this
    /// window's scale.
    #[test]
    fn a_hole_becomes_window_local_pixels() {
        let bounds = PhysRect::new(0, 0, 3072, 1920);
        let m = state_with(
            vec![slot(0, bounds, 2.0)],
            PhysRect::new(768, 480, 1536, 960),
        );
        assert_eq!(
            m.hole_for(m.slot(0).unwrap()),
            PhysRect::new(384, 240, 768, 480)
        );
    }

    /// Two screens, a hole on one of them: the other draws *nothing*, not a band
    /// at its edge - which is what an unclipped hole looks like on a second
    /// monitor to the right.
    #[test]
    fn a_hole_only_reaches_the_screen_it_is_on() {
        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 1.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let m = state_with(
            vec![left.clone(), right.clone()],
            PhysRect::new(2100, 100, 400, 300),
        );
        assert_eq!(m.hole_for(m.slot(0).unwrap()), PhysRect::default());
        assert_eq!(m.hole_for(&right), PhysRect::new(180, 100, 400, 300));
    }

    /// The seam case, and the reason the clip happens before the division: half
    /// of the selection is on each screen, and each window gets its own half.
    #[test]
    fn a_hole_across_the_seam_is_split_not_stretched() {
        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 1.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let m = state_with(
            vec![left.clone(), right.clone()],
            PhysRect::new(1900, 10, 40, 100),
        );
        assert_eq!(m.hole_for(&left), PhysRect::new(1900, 10, 20, 100));
        assert_eq!(m.hole_for(&right), PhysRect::new(0, 10, 20, 100));
    }

    /// `--mask` asserts against this shape, so it is decided here: the middle half
    /// of the *primary*, never of the whole desktop - a two-monitor desktop whose
    /// primary is the right-hand one must not open a hole over the other screen.
    #[test]
    fn the_default_hole_is_the_middle_half_of_the_primary() {
        let snap = snapshot(vec![
            monitor("m0", 0, 0, 1920, 1080, 1.0, false),
            monitor("m1", 1920, 0, 1920, 1080, 1.0, true),
        ]);
        assert_eq!(
            MaskState::default_hole(&snap),
            PhysRect::new(2400, 270, 960, 540)
        );
    }

    /// The pairing has to survive mixed scale, because that is the only reason a
    /// mask window and its frame could disagree about size: Qt answers in DIP, the
    /// snapshot in device pixels, and neither names the other's monitor.
    #[test]
    fn a_screen_is_matched_by_its_physical_size() {
        let snap = snapshot(vec![
            monitor("m0", 0, 0, 3072, 1920, 2.0, true),
            monitor("m1", 3072, 0, 1920, 1080, 1.0, false),
        ]);
        let screen = shim::Screen {
            name: "DISPLAY1".into(),
            x: 1536,
            y: 0,
            w: 1920,
            h: 1080,
            dpr: 1.0,
        };
        let mut used = vec![false; snap.monitors.len()];
        assert_eq!(
            MaskState::match_monitor(&snap, &screen, &mut used, 1),
            Some(1)
        );
        let mut used = vec![false; snap.monitors.len()];
        let hidpi = shim::Screen {
            name: "DISPLAY2".into(),
            x: 0,
            y: 0,
            w: 1536,
            h: 960,
            dpr: 2.0,
        };
        assert_eq!(
            MaskState::match_monitor(&snap, &hidpi, &mut used, 0),
            Some(0)
        );
        // A screen the capture path never saw stays unmatched, and the caller turns
        // that into a problem row rather than a mask covering the wrong pixels.
        let mut used = vec![false; snap.monitors.len()];
        let alien = shim::Screen {
            name: "DISPLAY3".into(),
            x: 0,
            y: 0,
            w: 800,
            h: 600,
            dpr: 1.0,
        };
        assert_eq!(MaskState::match_monitor(&snap, &alien, &mut used, 2), None);
    }

    // ------------------------------------------------------------ selection

    /// The same state a drag runs against, with the one field `state_with` leaves
    /// empty: `desktop()` reads `virtual_bounds`, and an empty desktop would clamp
    /// every point to the origin and make each of these tests pass for the wrong
    /// reason.
    fn selectable(slots: Vec<Slot>, hole: PhysRect, desktop: PhysRect) -> MaskState {
        MaskState {
            slots,
            hole,
            virtual_bounds: desktop,
            ..Default::default()
        }
    }

    /// One 3072x1920 screen at 200%, which is the machine this runs on: a press at
    /// DIP `(100, 50)` is physical `(200, 100)`, so every expectation below states
    /// the scale conversion once instead of trusting a helper to hide it.
    fn one_screen() -> (Vec<Slot>, PhysRect) {
        let bounds = PhysRect::new(0, 0, 3072, 1920);
        (vec![slot(0, bounds, 2.0)], bounds)
    }

    /// §5.3.1 steps 2-3: hold, move, release, and the rectangle is there - in
    /// physical pixels, which is the space the crop is taken in afterwards.
    #[test]
    fn a_drag_from_nothing_draws_a_selection() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert_eq!(m.press(0, 100.0, 50.0), None);
        assert_eq!(m.grip_code(), 1, "the grip is a new rectangle");
        assert_eq!(m.hole, PhysRect::new(200, 100, 0, 0));
        m.drag(0, 300.0, 200.0);
        assert_eq!(m.hole, PhysRect::new(200, 100, 400, 300));
        m.release();
        assert!(m.has_selection());
        assert_eq!(m.grip_code(), 0, "nothing is held after the release");
    }

    /// The same drag in the other direction, because "from the anchor" is only
    /// honest if up-and-left works without producing a negative size.
    #[test]
    fn a_drag_up_and_left_normalises_instead_of_going_negative() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        m.press(0, 300.0, 200.0);
        m.drag(0, 100.0, 50.0);
        assert_eq!(m.hole, PhysRect::new(200, 100, 400, 300));
    }

    /// §5.3.1 step 4: a corner drags the two edges it owns and leaves the opposite
    /// corner exactly where it was.
    #[test]
    fn a_corner_drag_resizes_and_keeps_the_opposite_corner() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        // The bottom-right corner is physical (1200, 1000) = DIP (600, 500).
        assert!(matches!(m.press(0, 600.0, 500.0), Some(Handle::Se)));
        m.drag(0, 700.0, 600.0);
        assert_eq!(m.hole, PhysRect::new(400, 400, 1000, 800));
        assert_eq!(m.hole.top_left(), PhysPoint::new(400, 400));
    }

    /// The same corner pushed past the edge of the screen. §5.3.9's rule is that a
    /// selection may not leave the valid area, and the crop would otherwise ask the
    /// frozen frame for pixels that do not exist.
    #[test]
    fn a_drag_off_the_screen_stops_at_its_edge() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        m.press(0, 600.0, 500.0);
        m.drag(0, 2000.0, 1200.0);
        assert_eq!(m.hole, PhysRect::new(400, 400, 2672, 1520));
        assert_eq!(m.hole.right(), 3072);
        assert_eq!(m.hole.bottom(), 1920);
    }

    /// Grabbing the middle moves the selection by the pointer's travel. It must not
    /// put the rectangle's origin under the cursor: that jump is what `resize`'s own
    /// `Body` branch does, and it is right for a tool you place with a click and
    /// wrong for something you already have hold of.
    #[test]
    fn a_drag_inside_moves_without_jumping() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        assert!(matches!(m.press(0, 300.0, 300.0), Some(Handle::Body)));
        m.drag(0, 350.0, 320.0);
        assert_eq!(m.hole, PhysRect::new(500, 440, 800, 600));
    }

    /// The grab radius is a distance a finger travels, so it is stated in DIP and
    /// this screen's scale turns it into pixels: at 200% a press 8 physical pixels
    /// off the edge is 4 DIP off it, inside `DEFAULT_HIT_SLOP`, and grabs the edge
    /// instead of starting a new rectangle on top of the one already there.
    #[test]
    fn the_hit_test_reaches_five_dip_in_whatever_units_the_screen_uses() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(200, 100, 800, 600), desk);
        assert!(matches!(m.press(0, 96.0, 100.0), Some(Handle::W)));
    }

    /// §5.3.10: a drag that leaves its own monitor keeps extending the selection,
    /// so the clamp is against the desktop rather than the window. Two screens at
    /// 100%, so the arithmetic is the seam itself and not a scale factor.
    #[test]
    fn a_drag_can_cross_the_seam_between_two_screens() {
        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 1.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let mut m = selectable(
            vec![left, right],
            PhysRect::default(),
            PhysRect::new(0, 0, 3840, 1080),
        );
        m.press(0, 1900.0, 100.0);
        m.drag(0, 2100.0, 300.0);
        assert_eq!(m.hole, PhysRect::new(1900, 100, 200, 200));
        // And each window still sees only its own half of it.
        assert_eq!(m.view_data(0).hole, PhysRect::new(1900, 100, 20, 200));
        assert_eq!(m.view_data(1).hole, PhysRect::new(0, 100, 180, 200));
    }

    /// The Esc ladder, as a return value rather than a property: two mask windows
    /// can both receive one keystroke, and if each read "is there a selection"
    /// instead of being told, the first would clear it and the second would cancel
    /// the capture in the same press.
    #[test]
    fn escape_clears_the_selection_before_it_asks_to_cancel() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(200, 100, 800, 600), desk);
        assert!(m.escape());
        assert!(!m.has_selection());
        assert!(!m.escape(), "the second Esc is the one that ends the flow");
    }

    /// §5.7.11's box as two windows see it. The judgement is `typing_live` and not
    /// `typing`: a clipped rect on both screens is two text fields, and two fields
    /// competing for the keyboard is a caret that moves when the other one is typed
    /// into. The selection already splits at the seam; the field is the one thing in
    /// this picture that must not.
    #[test]
    fn a_text_box_belongs_to_the_window_that_was_clicked() {
        use falcon_core::annotation::model::Kind;

        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 1.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let mut m = selectable(
            vec![left, right],
            PhysRect::default(),
            PhysRect::new(0, 0, 3840, 1080),
        );
        // Through `set_hole`, not the struct literal: the layer keeps its own copy of
        // the selection to clamp gestures into, and this harness has to keep them
        // together the way `open` does.
        m.set_hole(PhysRect::new(1900, 10, 40, 100));
        m.ink.select_tool(annotate::code_of(Some(Kind::Text)));
        assert!(
            m.press(0, 1910.0, 20.0).is_none(),
            "the pen did not take the press"
        );
        m.release();
        m.ink.type_text("a string long enough to run over the seam");

        let desk = m.ink.typing_rect().expect("the click opened no box");
        assert!(desk.right() > 1920, "the box does not straddle: {desk:?}");
        let on_left = m.view_data(0);
        let on_right = m.view_data(1);
        assert!(on_left.typing_live, "the clicked window has no field");
        assert!(!on_right.typing_live, "the other window was handed one too");
        // The clicked window still shows only its own piece of the box, because the
        // field is a window's child and cannot reach past its edge.
        assert_eq!((on_left.typing.x, on_left.typing.y), (1910, 20));
        assert_eq!(
            on_left.typing.w, 10,
            "the box was not clipped to the screen"
        );
        assert_eq!(
            on_right.typing,
            PhysRect::default(),
            "the other window drew a box"
        );
    }

    /// The same box on the 200% screen this runs on: the field QML places lives in
    /// device-independent pixels, and so does the 字号 it has to match. `--mask`
    /// measures the conversion for the selection; this is the same three steps for the
    /// one item that arrives at a *click* rather than from a drag.
    #[test]
    fn a_text_box_is_placed_and_sized_in_the_windows_own_units() {
        use falcon_core::annotation::model::Kind;

        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        m.set_hole(PhysRect::new(200, 100, 800, 600));
        m.ink.select_tool(annotate::code_of(Some(Kind::Text)));
        // DIP (150,70) is physical (300,140) on this screen.
        m.press(0, 150.0, 70.0);
        m.release();
        m.ink.type_text("hi");
        let box_ = m.ink.typing_rect().unwrap();
        assert_eq!(
            (box_.x, box_.y),
            (300, 140),
            "the box is not in device pixels"
        );

        let v = m.view_data(0);
        assert!(v.typing_live);
        assert_eq!(
            (v.typing.x, v.typing.y),
            (150, 70),
            "the field is not at the click"
        );
        assert_eq!(v.font_dip, 9, "the 字号 did not divide by the scale");
        // `phys_to_dip_i` rounds, so the doubled number can be one off the measure.
        assert!(
            (v.typing.w as i32 * 2 - box_.w as i32).abs() <= 1,
            "the field is {} DIP wide for a {} px box",
            v.typing.w,
            box_.w
        );

        // And the box is the ladder's first rung: the typed string is in no history, so
        // a first Esc that un-selected instead would take the letters with it and leave
        // the selection standing.
        assert!(m.escape(), "the first Esc did not close the box");
        assert!(!m.view_data(0).typing_live, "the box survived Esc");
        assert_eq!(
            m.hole,
            PhysRect::new(200, 100, 800, 600),
            "the first Esc moved the selection"
        );
        assert!(m.escape(), "the second Esc did not clear the selection");
        assert!(!m.has_selection());
        assert!(!m.escape(), "the third Esc is the one that ends the flow");
    }

    /// Enter with nothing drawn is not a crop the user never drew.
    #[test]
    fn commit_only_returns_a_rect_that_exists() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert_eq!(m.commit(), None);
        m.set_hole(PhysRect::new(10, 20, 30, 40));
        assert_eq!(m.commit(), Some(PhysRect::new(10, 20, 30, 40)));
    }

    /// §5.5.3 的那一格开关在**完成**这道门上的三格读数：关着一个字节也不写，开着写的
    /// 是 §5.5.4 那套命名规则给的名字，而被拒的一次不吃掉这一次确认。
    ///
    /// 第一格判的是"和这一格存在之前一模一样"，而不是"少写了一个文件"： shipped 的
    /// `auto_save` 是 `false`，所以任何已有的 `Enter`、双击、和拿它们当步骤的量具腿，
    /// 在这一格看到的还是它们一直看到的字节。第二格才是新功能。第三格是 §5.5.3 规则
    /// 那句"保存失败时应提示原因"——原因由 [`Self::quick_out`] 那一处说，这一处只保证
    /// 它没有被"确认失败"顶掉，也没有花掉一个序号。
    #[test]
    fn auto_save_writes_at_the_confirmation_only_when_the_setting_says_so() {
        // 一、开关关着：门还是那扇门，沙盒里还是空的。
        let dir = sandbox("auto-off");
        let mut off = savable(PhysRect::new(10, 20, 100, 80));
        off.naming = crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "auto".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        assert!(!off.auto_save, "默认的门不该自己开着");
        assert_eq!(off.commit(), Some(PhysRect::new(10, 20, 100, 80)));
        assert_eq!(off.quick_saves, 0, "一次关着的确认却落了一份文件");
        assert!(off.landed.is_none(), "关着的门留下了一张已经落盘的图的路径");
        assert_eq!(off.auto_copies, 0, "auto_copy 关着时默认复制也数了一次");
        assert!(
            off.auto_copy_last.is_empty(),
            "两格开关都没开的确认，替默认复制说了一句话: {:?}",
            off.auto_copy_last
        );
        assert_eq!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .filter_map(Result::ok)
                .count(),
            0,
            "auto_save 关着时，确认选区往用户的目录里写了东西"
        );

        // 二、开关开着：同一道门、同一个所有者，写的是那一份 §5.5.4 会写的文件。
        let on_dir = sandbox("auto-on");
        let mut on = savable(PhysRect::new(10, 20, 100, 80));
        on.auto_save = true;
        on.naming = crate::settings::Naming {
            dir: on_dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "auto".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        assert_eq!(on.commit(), Some(PhysRect::new(10, 20, 100, 80)));
        let landed = on_dir.join("auto-001.png");
        assert!(landed.exists(), "{:?}", on.message);
        assert!(
            on.message.contains("auto-001.png"),
            "自动保存的那句话没有说出它写的文件: {:?}",
            on.message
        );
        assert_eq!(on.quick_saves, 1, "落了一份文件却没数到");
        assert_eq!(
            on.landed.as_deref(),
            Some(landed.as_path()),
            "通知手里拿的不是这一个文件"
        );
        assert_eq!(
            crate::save::written_as(&landed),
            Some(encode::Format::Png),
            "自动保存写出来的不是 `[output] format`"
        );

        // 三、一次被拒的写：选区照旧确认了，原因照旧由写的那一处理赔，序号一个没花。
        std::fs::write(on_dir.join("auto-002.png"), b"not a picture").expect("plant");
        on.naming.collision = falcon_core::naming::Collision::KeepBoth;
        assert_eq!(on.commit(), Some(PhysRect::new(10, 20, 100, 80)));
        assert!(on.message.contains("keep_both"), "{:?}", on.message);
        assert_eq!(on.quick_saves, 1, "一次没有落盘的写花掉了一个序号");
        assert_eq!(
            on.auto_copies, 0,
            "只开着保存的那道确认，替默认复制数了一次"
        );
        assert!(
            on.auto_copy_last.is_empty(),
            "auto_copy 关着时它自己开口了: {:?}",
            on.auto_copy_last
        );
    }

    /// §5.5.1 step 3 的默认行为在**完成**这道门上的 headless 那一半：开关开着而画面拼不出来
    /// 时，答的话出自缝合那一梯，剪贴板一次也没被打开，计数一次也没动。
    ///
    /// 这是 §9.1 第十条为 `copy_out` 立过的那格规矩的第二次：
    /// `copying_answers_with_a_sentence_before_it_opens_the_clipboard` 那句"为了证明自己能
    /// 拒绝而去写用户的剪贴板，是一笔坏买卖"在这里一字不改地成立——`savable` 才拼得出图，
    /// 而一张拼得出图的图一旦被递进去，就跑到了运行单测的那台机器的剪贴板上。成功那一梯的
    /// 读数归 `--autosave` 那一档：它在真剪贴板前读回来，并且先把用户原来的内容放回去。
    #[test]
    fn auto_copy_at_the_confirmation_answers_before_it_opens_the_clipboard() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(!m.auto_copy, "默认的门不该自己开着");
        m.set_hole(PhysRect::new(10, 20, 30, 40));
        m.auto_copy = true;
        assert_eq!(
            m.commit(),
            Some(PhysRect::new(10, 20, 30, 40)),
            "一次默认复制的失败顶掉了这一次确认"
        );
        assert!(
            m.auto_copy_last.contains("拼不出来"),
            "默认复制答的不是缝合那一梯: {:?}",
            m.auto_copy_last
        );
        assert_eq!(m.auto_copies, 0, "一次没递进去的默认复制数到了自己");
        assert!(
            m.message.is_empty(),
            "自动那一格占了有人按的那句话的槽位: {:?}",
            m.message
        );
        assert!(m.landed.is_none() && m.quick_saves == 0, "只开复制却落了盘");
    }

    /// §5.14.1 那一行的 `monitor_id` 从哪儿来：一屏装得下、跨两屏取宽的那一枚、并列取
    /// 列表在前、交不到任何一屏、和一屏也没给。
    ///
    /// 这是 P93 那道门上唯一的**纯函数**证人（P67 `tray_from` / P68 `history_open_in`
    /// 同一族）：`record_history` 本体要 SQLite、要 disk、要一张真拼得出的图，§9.1 第十条
    /// 不许它进单测；而"这一张图算哪一屏"这个决定整个在那一步里。
    #[test]
    fn capture_meta_for_names_the_screen_the_hole_lands_on() {
        let one = vec![("m0".to_string(), PhysRect::new(0, 0, 1920, 1080))];
        let two = vec![
            ("m0".to_string(), PhysRect::new(0, 0, 1920, 1080)),
            ("m1".to_string(), PhysRect::new(1920, 0, 1920, 1080)),
        ];

        // 一、框整个在 m1 上：名字是 m1，不是列表里第一枚。
        let whole = PhysRect::new(2000, 100, 300, 200);
        assert_eq!(monitor_slot_for(&two, whole), Some(1));
        let meta = capture_meta_for(&two, whole, 1_700_000_000_000, -480, false);
        assert_eq!(meta.monitor_id, "m1");
        assert_eq!(meta.rect, whole, "那一行的选区不是被确认的那一枚");
        assert_eq!(meta.at_ms, 1_700_000_000_000);
        assert_eq!(meta.offset_min, -480);
        // 这道门只接 `Enter`/双击确认的那一条：拖出来的框就是 §5.2.1 的区域截图。
        assert_eq!(meta.source, falcon_core::history::Source::Region);
        // 自 P95 起 `cancelled` 是调用方交进来的，不再由这一位代答：传 `false` 是确认那
        // 条腿。Esc 那一族传的必须是 `true`，见下面第五段。
        assert!(!meta.cancelled, "确认的那一次被记成了取消");

        // 二、跨边界：m0 那边 120×600、m1 那边 480×600 ⇒ 取重叠最宽的那一枚。
        let straddle = PhysRect::new(1800, 100, 600, 600);
        assert_eq!(monitor_slot_for(&two, straddle), Some(1));
        assert_eq!(
            capture_meta_for(&two, straddle, 0, 0, false).monitor_id,
            "m1"
        );

        // 三、并列取列表在前那一枚：`slots` 是 Qt 的屏幕序号，所以这是确定的而不是碰巧。
        let tie = vec![
            ("a".to_string(), PhysRect::new(0, 0, 100, 100)),
            ("b".to_string(), PhysRect::new(100, 0, 100, 100)),
        ];
        assert_eq!(
            monitor_slot_for(&tie, PhysRect::new(50, 0, 100, 100)),
            Some(0),
            "两边各占一半时那一行换了屏幕"
        );

        // 四、交不到任何一屏 / 一枚屏幕也没给 ⇒ 说不出。而"说不出"是一个**合法的读数**
        // （§6.2 的 `monitor_id` 是 `TEXT NOT NULL DEFAULT ''`），不是一条不该发的行。
        let elsewhere = PhysRect::new(5000, 5000, 10, 10);
        assert_eq!(monitor_slot_for(&one, elsewhere), None);
        assert_eq!(
            capture_meta_for(&one, elsewhere, 0, 0, false).monitor_id,
            ""
        );
        assert_eq!(monitor_slot_for(&[], PhysRect::new(0, 0, 10, 10)), None);
        assert_eq!(
            capture_meta_for(&[], PhysRect::new(0, 0, 10, 10), 7, 8, false).monitor_id,
            "",
            "一屏也没给的时候那一行凭空发明了一个名字"
        );

        // 五、P95 起 `cancelled` 是这一位唯一**不**照抄实参的东西：同一枚屏、同一枚框、
        // 同一个时刻传两遍，除了那一格必须全等 —— §5.14.3 要的是"同一次截图的另一份读数"，
        // 不是第二条截图。屏幕名也不许因为"这是取消的"就换一位主人。
        let cancelled = capture_meta_for(&two, whole, 1_700_000_000_000, -480, true);
        assert!(cancelled.cancelled, "取消的那一次被记成了没取消");
        assert_eq!(cancelled.monitor_id, meta.monitor_id);
        assert_eq!(cancelled.rect, meta.rect);
        assert_eq!(cancelled.at_ms, meta.at_ms);
        assert_eq!(cancelled.offset_min, meta.offset_min);
        assert_eq!(cancelled.source, meta.source);
    }

    /// §5.14.1 第二条那一格分岔的本体：一次被 Esc 清掉的选区到底欠不欠一行历史。
    ///
    /// 这里**故意不判 `enabled`**：那一格自 P68 起只有 [`crate::settings::history_open_in`]
    /// 一个主人，而 `record_history` 走它 —— 在这里再判一遍就是养第二个主人，两边一旦分岔
    /// 就没有可读的数了。所以这一位的实参只有两枚事实：开关说没说要留、场上有没有一块框。
    #[test]
    fn esc_owes_a_row_only_when_the_switch_says_so_and_a_hole_existed() {
        // 一、开了、且有框：欠一行（§5.14.3 那条"找到标记为已取消的记录"的全部依据）。
        assert!(
            esc_owes_row(true, true),
            "开关说要留、场上有框，那一行却没欠"
        );
        // 二、没开：一条也不留。§5.14.1 的措辞是"用户可开启"，默认不留是用户没要的那一份。
        assert!(
            !esc_owes_row(false, true),
            "开关没说留，Esc 却还是往历史里写了一张没人要的图"
        );
        // 三、开了、但场上没框：那一下 Esc 只是收起这一场（`escape()` 答 false 的那条腿），
        // 没有选区、没有图、也没有一行可记 —— 记了就是一行空读数。
        assert!(!esc_owes_row(true, false), "没有框的时候那一行凭空欠了出来");
        // 四、两枚都不成立。
        assert!(!esc_owes_row(false, false));
    }

    /// §9.1 第十条在 P93 这道门上的形状：一次**真拼得出图**的确认，不往任何地方写历史。
    ///
    /// `record_history` 头上那句 `history = None` 的检查看着像防御式的冗余（真机上
    /// `open()` 当场把它填成 `Some`），但它挡的是**跑单测的那台机器**：`commit()` 已经被
    /// 三格已有单测直接调用，而 `savable` 那一格交回的确是一张真图 —— 门上一旦改成"读
    /// 进程全局配置决定要不要开 SQLite"，`cargo test -p qt_bridge` 就会往用户的
    /// `%APPDATA%\ai-falconshot\history\` 里长出一份 `history.db`、一行记录和一张原图。
    ///
    /// 这一格因此是那道新代码的反证，而不只是它的镜子：开着 `auto_save` 的一次确认照旧
    /// 落了盘、照旧数到 `quick_saves`（历史这一格没把它顶掉），而历史那一族三格全空。
    #[test]
    fn a_headless_confirmation_writes_no_history_row() {
        let dir = sandbox("history-off");
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        m.auto_save = true;
        m.naming = crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "nohist".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        assert!(m.history.is_none(), "单测里的门自己把历史打开了");
        assert_eq!(
            m.history_dir,
            std::path::PathBuf::new(),
            "落点说不出，却还带着一枚默认目录"
        );

        assert_eq!(m.commit(), Some(PhysRect::new(10, 20, 100, 80)));
        assert!(
            dir.join("nohist-001.png").exists(),
            "自动保存那一半被历史这一格改掉了: {:?}",
            m.message
        );
        assert_eq!(m.quick_saves, 1, "同一道门多了一样之后落盘没数到");
        assert_eq!(m.history_records, 0, "一场没开历史门的确认记了一行");
        assert_eq!(m.history_last_id, None, "没记成却有行号");
        assert!(
            m.history_last.is_empty(),
            "关着的门替历史说了一句话: {:?}",
            m.history_last
        );
    }

    /// §5.14.1 的第二条（"用户可开启『保存所有截图记录』，包括被取消的截图"）与 §5.14.3
    /// 的第一格（历史里那条要"标记为已取消"）：P95 起 Esc 那一格也走 [`Self::record_history`]，
    /// 而开关键是新造的 `[history] save_all`。
    ///
    /// 两臂各判三件事，少一件都不算数：**门开没开**（`history_records`）、**盘上此刻真有
    /// 几行**（自己开一次 `History::count` 读回，不信产品自己数的那个数 —— 与 P94 量具那一族
    /// 同一个口径：一个数由产品说、另一个数由 SQLite 说，两者不等就是缺陷而不是风格）、
    /// **那一行到底标没标成已取消**（`Record.cancelled` 与 [`MaskState::history_last`] 那句
    /// 话各读一次，它们必须是同一个数）。`enabled` 在两臂都开着，所以关着那一臂输掉的
    /// 只能是 `save_all` 这一格。
    ///
    /// 头里那扇 `history-off` 的档与这一档各自一枚沙盒：数行数的那把尺子数的是**行**，
    /// 混用一份目录会把"这一臂没记"读成"上一臂也没记"。
    #[test]
    fn an_escaped_selection_lands_in_history_only_when_save_all_is_on() {
        let base = falcon_core::config::History {
            enabled: true,
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        };
        let hole = PhysRect::new(10, 20, 100, 80);

        let off = sandbox("esc-history-off");
        let mut m = savable(hole);
        m.history = Some(falcon_core::config::History {
            save_all: false,
            ..base.clone()
        });
        m.history_dir = off.clone();
        assert!(m.escape(), "有选区的一场 Esc 没吃掉自己");
        assert!(m.hole.is_empty(), "历史这一格把选区留在了场上");
        assert_eq!(m.history_records, 0, "save_all 关着却记了一行");
        assert!(
            m.history_last.is_empty(),
            "关着的门替历史说了一句话: {:?}",
            m.history_last
        );
        assert_eq!(
            falcon_core::history::History::open(&off)
                .unwrap()
                .count()
                .unwrap(),
            0,
            "产品说没记，盘上却不是这个数"
        );

        let on = sandbox("esc-history-on");
        let mut m = savable(hole);
        m.history = Some(falcon_core::config::History {
            save_all: true,
            ..base.clone()
        });
        m.history_dir = on.clone();
        assert!(m.escape(), "开了开关的 Esc 反倒没吃掉自己");
        assert_eq!(m.history_records, 1, "save_all 开着却没记成");
        let id = m.history_last_id.expect("记成了一行却没有行号");
        assert_eq!(
            falcon_core::history::History::open(&on)
                .unwrap()
                .count()
                .unwrap(),
            1,
            "产品数了一行，盘上不是这个数"
        );
        let rec = falcon_core::history::History::open(&on)
            .unwrap()
            .get(id)
            .unwrap()
            .expect("读回找不到产品报的那个行号");
        assert!(rec.cancelled, "落库那一格没标成已取消");
        assert!(
            m.history_last.contains("已取消"),
            "盘上标了、屏幕上那句话没标: {:?}",
            m.history_last
        );
        assert_eq!(rec.rect, hole, "记下的选区不是被 Esc 清掉的那一枚");

        // 第三条腿：这把开关只管 Esc。确认那道门在开关**关着**时照样记一行，而且落库的是
        // `cancelled = 0` —— 两腿共用同一个写者，一旦 `commit()` 被它带着一起变成"已取消"，
        // 用户就再也找不回一条正常的记录。
        let mut m = savable(hole);
        m.history = Some(base);
        m.history_dir = on.clone();
        assert_eq!(m.commit(), Some(hole));
        assert_eq!(m.history_records, 1, "开关关着却把确认那道门也一起关掉了");
        assert_eq!(
            falcon_core::history::History::open(&on)
                .unwrap()
                .count()
                .unwrap(),
            2,
            "两腿各记一行，盘上只剩一行"
        );
        let id = m.history_last_id.expect("确认那一格没记成");
        let rec = falcon_core::history::History::open(&on)
            .unwrap()
            .get(id)
            .unwrap()
            .expect("确认的那一行读不回来");
        assert!(!rec.cancelled, "确认的门把这一行标成了已取消");
        assert!(
            !m.history_last.contains("已取消"),
            "盘上没标、屏幕上却标了: {:?}",
            m.history_last
        );
    }

    /// §5.14.2「从历史记录中……保存」的第一条**产品**腿（P97 的 [`MaskState::replay_history`]）。
    ///
    /// 门那一档只能读"这台机器此刻有屏、有剪贴板、跑得动事件循环"的几条腿，而这一条的正反
    /// 两面都不需要它们：先让产品自己记一行（`commit`），再把那一行的行号**交回**产品，问它
    /// 能不能把那张图从盘上读回来再落一次。正面四件一起判才算成：落点目录里多且只多一份文件、
    /// 那份文件解码出来就是当初那一枚选区的尺寸（同一张图，不是一张空白也不是别的图）、
    /// `history_replays` 加一、而 `quick_saves` 与 `landed` 一动不动（回放不许去改 §5.5.4 那格
    /// 的账）。反面要的是"什么都不多"：一个不在库里的行号如果能把它推进去，正面那句"我保存了"
    /// 就等于谁也没数过。
    ///
    /// 自动保存在这一档里是**关着**的：不然落点目录里那一份文件就说不清是回放的还是确认的。
    #[test]
    fn a_history_row_can_be_read_back_and_written_out_again() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let hdir = sandbox("replay-history");
        let out = sandbox("replay-out");
        let files = |d: &std::path::Path| {
            std::fs::read_dir(d)
                .map(|rd| rd.filter_map(Result::ok).count())
                .unwrap_or(usize::MAX)
        };

        let mut m = savable(hole);
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        });
        m.history_dir = hdir.clone();
        m.naming = crate::settings::Naming {
            dir: out.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "replay".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        assert_eq!(m.commit(), Some(hole), "确认没交出选区，历史也就没有那一行");
        assert_eq!(m.quick_saves, 0, "关着的自动保存反倒落了盘");
        let id = m
            .history_last_id
            .expect("§5.14.1 那一行没记成，回放无从跑起");
        assert_eq!(m.history_replays, 0, "还没回放就数到了一次");

        let landed = m
            .replay_history(id)
            .expect("那一行的图读回来再落一次盘没成");
        assert_eq!(
            landed.parent(),
            Some(out.as_path()),
            "回放没写到用户那个目录"
        );
        assert_eq!(files(&out), 1, "回放不止多了一份文件");
        let frame = falcon_core::encode::decode_file(&landed).expect("回放落的那份读不回来");
        assert_eq!(
            (frame.width, frame.height),
            (100, 80),
            "回放写出去的不是那一行记的那张图"
        );
        assert_eq!(m.history_replays, 1, "回放成了却没数到");
        assert_eq!(m.quick_saves, 0, "回放挪了 §5.5.4 那格的计数");
        assert!(m.landed.is_none(), "回放把那张落盘的通知占成了自己的");
        assert!(
            m.history_replay_last.contains(&format!("第 {id} 行")),
            "回放成了却没说它是哪一行: {:?}",
            m.history_replay_last
        );

        let missing = id + 1_000;
        assert!(
            m.replay_history(missing).is_none(),
            "库里没有的那一行反倒落了一份"
        );
        assert_eq!(files(&out), 1, "一次被拒的回放在盘上留了东西");
        assert_eq!(m.history_replays, 1, "被拒的那次吃了计数");
        assert!(
            m.history_replay_last.contains("没有第")
                && m.history_replay_last.contains(&missing.to_string()),
            "拒了却没说拒在哪一行: {:?}",
            m.history_replay_last
        );
    }

    /// §5.14.2 step 1「打开历史记录」那张清单的五种答法，P98 起 —— [`MaskState::list_history`]
    /// 是 [`falcon_core::history::History::newest`] 在产品里的第一位读者（P95 普查点名的那 11
    /// 枚仍为零的函数之一），而这一档是它唯一不需要屏幕、不需要剪贴板、也不需要 Qt 的证人
    /// （§9.1 第十条的 headless 半边）。
    ///
    /// 五种答法**必须各说各的话**，因为对用户它们是五件不同的事：这一场没有历史可读、档里
    /// `[history] enabled` 是关的、开了而库里还没有一行、库里确实有几行（以及用户只要最前
    /// n 行）、库根本打不开。把第三种说成第四种的"0 行"、或把第三与第五种都塌成一个空串，
    /// 等于让一枚坏掉的 `history.db` 读成一次正常的清空 —— [`MaskState::replay_history`]
    /// 头上立的是同一条口径。
    ///
    /// 读侧还要证明自己是**纯读**：两次确认之后清单数到 2 行，再整个读一次、按 `limit` 读
    /// 一次，`history_records` 与 `history_last_id` 都一动不动（清单不许往库里塞行，也不许
    /// 改这一场最后记成的那一行的名字），而 `quick_saves`/`landed`/`history_replays` 也不许
    /// 被一条只读的路径挪走。
    #[test]
    fn listing_history_answers_separately_for_none_off_empty_rows_and_a_closed_db() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let hdir = sandbox("list-history");
        let mut m = savable(hole);

        // ① 这一场没有历史：`MaskState` 自己的 `Default` 就是这一格，开关和落点都没给。
        assert_eq!(m.list_history(10), "", "没有历史的一场反倒读出了行");
        assert!(m.history_list.is_empty(), "① 没读却留了清单在场");
        assert!(
            m.history_list_last.contains("没有历史"),
            "① 说成了别的: {:?}",
            m.history_list_last
        );

        // ② 档里 `[history] enabled` 是关的：说的是「没开」，而不是「空」，也不许开库。
        m.history = Some(falcon_core::config::History {
            enabled: false,
            ..Default::default()
        });
        m.history_dir = hdir.clone();
        assert_eq!(m.list_history(10), "", "关着的那枚开关反倒交回了行");
        assert!(
            m.history_list_last.contains("没开"),
            "② 说成了别的: {:?}",
            m.history_list_last
        );
        assert!(
            !hdir.join("history.db").exists(),
            "关着的开关把库建出来了: {:?}",
            hdir
        );

        // ③ 开了、而库里还没有一行 —— 一次**正常的空**，那句要说成「0 行」。
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        });
        assert_eq!(m.list_history(10), "", "空库读出了行");
        assert!(
            m.history_list_last.contains("0 行") && !m.history_list_last.contains("没开"),
            "③ 要说成一次正常的空，而不是②那句没开: {:?}",
            m.history_list_last
        );

        // ④ 产品自己记了两次（两次不同的选区，好在清单里分辨得开）：清单就该数到 2 行，
        // 而**新的一行在前**是 §5.14.2「最近的一次在最上面」那句话的读侧版本。
        assert_eq!(m.commit(), Some(hole), "第一次确认没交出选区");
        let older_id = m
            .history_last_id
            .expect("④ 里先记的那一行没记成，清单无从数起");
        let second = PhysRect::new(5, 6, 30, 40);
        m.set_hole(second);
        assert_eq!(m.commit(), Some(second), "第二次确认没交出选区");
        let newest_id = m.history_last_id.expect("④ 里后记的那一行没记成");
        assert_ne!(
            older_id, newest_id,
            "两次确认记成了同一行，清单的先后无从分辨"
        );

        let rows = m.list_history(10);
        let seg: Vec<&str> = rows.split('|').collect();
        assert_eq!(
            seg.len(),
            2,
            "两次确认之后清单该数到 2 段，读回 {} 段",
            seg.len()
        );
        assert!(
            seg[0].starts_with(&format!("{newest_id} · ")) && seg[0].contains("30×40"),
            "清单第一段不是刚记的那一行: {seg:?}"
        );
        assert!(
            seg[1].starts_with(&format!("{older_id} · ")) && seg[1].contains("100×80"),
            "清单第二段不是先记的那一行: {seg:?}"
        );
        assert_eq!(
            m.history_list, rows,
            "清单没留在字段里，界面就读不到这一次的答案"
        );
        assert!(
            m.history_list_last.contains("2 行"),
            "数到 2 行却说成了别的: {:?}",
            m.history_list_last
        );

        // `limit` 到得了 SQL：只要 1 行就只剩最前那一段。这一格证明它不是"把整份读回来
        // 再在 Rust 这边切一刀" —— 那在库里有一万行时是完全不同的代价。
        let one = m.list_history(1);
        assert_eq!(one.split('|').count(), 1, "limit=1 却读回多段: {one:?}");
        assert!(
            one.starts_with(&format!("{newest_id} · ")),
            "limit 切错了头: {one:?}"
        );

        // 纯读的五件：清单不记行、不改名、不回放、不落盘、不占通知。
        assert_eq!(m.history_records, 2, "读清单往库里塞了行");
        assert_eq!(
            m.history_last_id,
            Some(newest_id),
            "读清单改了这一场最后记成的那一行"
        );
        assert_eq!(m.history_replays, 0, "读清单跑到了回放那条腿上");
        assert_eq!(m.quick_saves, 0, "读清单挪了 §5.5.4 那格的计数");
        assert!(m.landed.is_none(), "读清单把落盘通知占成了自己的");

        // ⑤ 库根本打不开：把落点换到"一份普通文件底下"，`History::open` 里那次
        // `create_dir_all` 当场被拒。这句话必须说成**打不开**而不是没开，也不许把 ④ 读到
        // 的那两行留在场上——否则一次失败的读会拿上一次的清单冒充这一次的答案。
        let wall = sandbox("list-history-wall");
        std::fs::write(wall.join("plain.txt"), b"not a directory").expect("⑤ 的前置摆不出普通文件");
        m.history_dir = wall.join("plain.txt").join("history");
        assert_eq!(m.list_history(10), "", "打不开的库却交回了一份清单");
        assert!(m.history_list.is_empty(), "⑤ 把 ④ 读到的清单留在了场上");
        assert!(
            m.history_list_last.contains("打不开"),
            "⑤ 要说清是打不开而不是没开: {:?}",
            m.history_list_last
        );
    }

    /// §5.14.4「选择单条、多条 → 点击删除 → 系统释放相应磁盘空间」的第一条**产品**腿，P99 起
    /// （[`MaskState::delete_history`]，也是 [`falcon_core::history::History::delete`] 在产品里的
    /// 第一位读者）。这一档判的是**盘上**而不是库里那一行：它指的那份 PNG 删前还在、删后不在，
    /// 而交回的字节数就是现场数出来的那一份 —— §5.14.4 step 4 那句"释放相应磁盘空间"的对面是
    /// 文件系统，一条只会在 SQLite 里打勾的路径证不了它。
    ///
    /// 三格各说各的话，少一格就等于把两件事讲成一件：**点名的锁定项照删**（`History::delete`
    /// 不看锁 —— `history.rs` 头上第二条口径说的是"手动删除是用户自己点的名"，而 [`MaskState::clear_history`]
    /// 恰好相反，两格并成一个数就分不出这两条相反的家规）；**空点名不开库**（UI 一行也没选中时
    /// 递来的 `[]` 与"选中的都删成了"在盘上长得一模一样）；**不存在的行号答"没删成"而不是
    /// "删成了 0 行"**。
    ///
    /// 最后一档判的仍是 P96/P97/P98 三轮立过的那条口径：新腿不许改老账。删行不许挪
    /// `history_records`、`history_last_id`、`quick_saves`、`landed`，也不许把
    /// [`MaskState::history_list`] 里那一份**上一次读回来的快照**修剪成看着与库同步的样子 ——
    /// 界面要新清单就得再走一次 [`MaskState::list_history`]，本条不许替它作弊。
    #[test]
    fn deleting_a_named_history_row_gives_its_picture_and_its_bytes_back() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let hdir = sandbox("delete-history");
        let mut m = savable(hole);
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        });
        m.history_dir = hdir.clone();
        let rows = || {
            falcon_core::history::History::open(&hdir)
                .and_then(|h| h.count())
                .unwrap_or(u64::MAX)
        };

        assert_eq!(m.commit(), Some(hole), "确认没交出选区，库里也就没有那一行");
        let id = m.history_last_id.expect("§5.14.1 那一行没记成");
        let picture = {
            let store = falcon_core::history::History::open(&hdir).expect("前置打不开库");
            let rec = store
                .get(id)
                .expect("前置读不回那一行")
                .expect("前置：库里没有那一行");
            store
                .path_of(&rec)
                .expect("前置：那一行没有原图，§5.14.4 那格量不到文件")
        };
        let bytes = std::fs::metadata(&picture)
            .expect("前置读不到那份文件")
            .len();
        assert!(bytes > 0, "前置：那份文件是空的，字节那一问量不到东西");

        // ① 空点名先挡在开库之前：换一枚还不存在的落点，于是"有没有去开库"是当场数得出来的。
        let fresh = sandbox("delete-empty").join("brand-new");
        m.history_dir = fresh.clone();
        assert!(
            m.delete_history(&[]).is_none(),
            "① 一行也没点名，反倒交回了一次删除结果"
        );
        assert!(
            !fresh.join("history.db").exists(),
            "① 没点名的那一次把库建了"
        );
        assert!(
            m.history_delete_last.contains("没有点名"),
            "① 拒了却没说是空点名: {:?}",
            m.history_delete_last
        );
        assert_eq!(m.history_deletes, 0, "① 吃了删除计数");
        m.history_dir = hdir.clone();

        // ② 读侧先走一次，把清单摆在场上，好让③那句"快照不许修剪"有东西可对。
        let listed = m.list_history(10);
        assert!(
            listed.contains(&id.to_string()),
            "② 清单里就该有那一行: {listed:?}"
        );

        // ③ 正面：删掉点名的那一行。四件一起判才算成。
        let freed = m.delete_history(&[id]).expect("③ 点名的那一行没删成");
        assert_eq!(freed.ids, vec![id], "③ 交回的行号不是点名的那一个");
        assert!(
            freed.bytes >= bytes,
            "③ 交回的字节 {} 比那份文件的 {bytes} 还少",
            freed.bytes
        );
        assert!(!picture.exists(), "③ 库里少了那一行，盘上那份图却还在");
        assert_eq!(rows(), 0, "③ 删完库里还剩行");
        assert_eq!(m.history_deletes, 1, "③ 删成了却没数到");
        assert!(
            m.history_delete_last.contains(&format!("第 {id} 行"))
                && m.history_delete_last.contains(&freed.bytes.to_string()),
            "③ 成了却没点名行号或那句字节数与交回的不是一个数: {:?}",
            m.history_delete_last
        );
        assert_eq!(
            m.history_list, listed,
            "③ 把上一次读回来的清单修剪成了看着同步的样子"
        );

        // ④ 反面：一个不在库里的行号。§6.2 的库删过一行之后 `id` 不会复用，但这一格要的
        // 不是"它肯定不在"，而是"万一它在，这一格就该红"——所以判据先把库里那一问答成
        // `Ok(None)`，与门那一档同一个形状。
        let missing = id + 1_000;
        assert!(
            falcon_core::history::History::open(&hdir)
                .and_then(|h| h.get(missing))
                .expect("④ 的前置读不出那一行在不在了")
                .is_none(),
            "④ 挑的行号其实存在，这一格作废"
        );
        let freed = m
            .delete_history(&[missing])
            .expect("④ 库里没有那一行却整条路没跑成");
        assert!(freed.ids.is_empty(), "④ 没在库里的行号被算进删掉的行");
        assert_eq!(rows(), 0, "④ 一次空删往库里删掉了东西");
        assert_eq!(m.history_deletes, 1, "④ 删掉 0 行却吃了计数");
        assert!(
            m.history_delete_last.contains("一个也没有"),
            "④ 把空手说成了一次成功: {:?}",
            m.history_delete_last
        );

        // ⑤ 三条相反的口径之一：**点名的锁定项照删**。`History::delete` 不看锁是 §6.2 那
        // 篇口径说的（手动删除是用户自己点的名），而 ⑥ 那次清空恰好相反。行由产品自己记，
        // 锁由前置打上 —— 产品里没有"锁定"那一条腿（`History::set_locked` 至今零产品读者，
        // 本轮结转），本档不假装它有。
        let second = PhysRect::new(5, 6, 30, 40);
        m.set_hole(second);
        assert_eq!(m.commit(), Some(second), "⑤ 那一行没记成");
        let locked = m.history_last_id.expect("⑤ 没报出行号");
        {
            let store = falcon_core::history::History::open(&hdir).expect("⑤ 前置打不开库");
            store.set_locked(locked, true).expect("⑤ 前置锁不上那一行");
        }
        let locked_picture = {
            let store = falcon_core::history::History::open(&hdir).expect("⑤ 前置打不开库");
            let rec = store
                .get(locked)
                .expect("⑤ 前置读不回那一行")
                .expect("⑤ 前置：库里没有那一行");
            store
                .path_of(&rec)
                .expect("⑤ 那一行没有原图，锁定那一问量不到文件")
        };
        assert!(
            locked_picture.exists(),
            "⑤ 前置：锁定那一行的图不在盘上，删除那问无从判"
        );
        let freed = m.delete_history(&[locked]).expect("⑤ 点名的锁定项没被删");
        assert_eq!(freed.ids, vec![locked], "⑤ 删掉的不是点名的那一枚锁定项");
        assert!(!locked_picture.exists(), "⑤ 库里删了，盘上那份图却留着");
        assert_eq!(rows(), 0, "⑤ 删完库里还剩行");
        assert_eq!(m.history_deletes, 2, "⑤ 点名删掉的第二行没数到");

        // ⑥ 老账一位不动：这场记了两行、最后记成的是⑤那一行、一次也没回放、也没落盘。
        assert_eq!(m.history_records, 2, "删除那条腿改了 §5.14.1 那格的计数");
        assert_eq!(
            m.history_last_id,
            Some(locked),
            "删除那条腿改了这一场最后记成的那一行"
        );
        assert_eq!(m.history_replays, 0, "删除那条腿跑到了回放上");
        assert_eq!(m.history_clears, 0, "删除那条腿吃了清空的计数");
        assert_eq!(m.quick_saves, 0, "删除那条腿挪了 §5.5.4 那格的计数");
        assert!(m.landed.is_none(), "删除那条腿把落盘通知占成了自己的");
        assert!(m.message.is_empty(), "删除那条腿把那句话占成了自己的");

        // ⑦ 库打不开：把落点换到一份普通文件底下。这句话要说成**打不开**，而且不许把 ⑤
        // 那次成功的"删了第 N 行"留在场上冒充这一次的答案 —— 一次失败的删除如果留下一句
        // 成功的话，判据读到的就是产品从没说过的话。
        let wall = sandbox("delete-history-wall");
        std::fs::write(wall.join("plain.txt"), b"not a directory").expect("⑦ 的前置摆不出普通文件");
        m.history_dir = wall.join("plain.txt").join("history");
        assert!(
            m.delete_history(&[locked]).is_none(),
            "⑦ 打不开的库反倒交回了一次删除结果"
        );
        assert!(
            m.history_delete_last.contains("打不开"),
            "⑦ 要说清是打不开而不是没开: {:?}",
            m.history_delete_last
        );
        assert_eq!(m.history_deletes, 2, "⑦ 一次开库失败吃了计数");
    }

    /// §5.14.4 step 2 的"全部"那一档，加规则 1 的**二次确认**与规则 2 的锁定项，P99 起
    /// （[`MaskState::clear_history`]，也是 [`falcon_core::history::History::clear_all`] 在产品
    /// 里的第一位读者）。
    ///
    /// 反面那一臂要的判据比"库里没少一行"更强：**没点头的那一次连库都不打开**。
    /// `History::open` 会 `create_dir_all` 并建出 `history.db`，所以一次"什么都没删"的清空如果
    /// 在盘上留下新文件，判据就只剩"数行数的差"可说；这里先拿一枚还不存在的落点证
    /// `history.db` 为 false，把"根本没开库"做成当场数得出的一格。
    ///
    /// 正面那一臂判的是规则 2 在产品里的形状：`include_locked` 钉死 `false` ⇒ 两行里锁定那一行
    /// **留下**、它的文件也**留下**，而 [`MaskState::history_deletes`] 一动不动（清空数在
    /// [`MaskState::history_clears`] 那枚上 —— [`MaskState::delete_history`] 头上写的是相反的
    /// 家规，两格并成一个数就分不出谁是谁）。
    ///
    /// 最后两臂各答一种"空手"：库里只剩锁定项 ⇒ 跑成而一行也没删；库根本打不开 ⇒ 说成打不开
    /// 而不许留着上一次那句成功的话。
    #[test]
    fn clearing_history_waits_for_confirmation_and_spares_locked_rows() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let hdir = sandbox("clear-history");
        let mut m = savable(hole);
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        });
        m.history_dir = hdir.clone();
        let rows = || {
            falcon_core::history::History::open(&hdir)
                .and_then(|h| h.count())
                .unwrap_or(u64::MAX)
        };
        let picture_of = |id: i64| -> Option<std::path::PathBuf> {
            let store = falcon_core::history::History::open(&hdir).ok()?;
            let rec = store.get(id).ok().flatten()?;
            store.path_of(&rec)
        };

        // ① 没点头：换一枚不存在的落点，"开没开库"当场可数。
        let fresh = sandbox("clear-unconfirmed").join("brand-new");
        m.history_dir = fresh.clone();
        assert!(
            m.clear_history(false).is_none(),
            "① 二次确认没点头反倒跑了清空"
        );
        assert!(
            !fresh.join("history.db").exists(),
            "① 没点头的那一次把库建了"
        );
        assert_eq!(m.history_clears, 0, "① 被拒的那一次吃了计数");
        assert!(
            m.history_clear_last.contains("二次确认"),
            "① 拒了却没说是没确认: {:?}",
            m.history_clear_last
        );
        // ①b 点头了而这一场压根没有历史：话说成"没有历史可清"，而不是"没确认"。
        let none = sandbox("clear-no-history");
        m.history = None;
        m.history_dir = none.clone();
        assert!(
            m.clear_history(true).is_none(),
            "①b 没有历史的一场反倒清了库"
        );
        assert!(
            m.history_clear_last.contains("没有历史"),
            "①b 说成了别的: {:?}",
            m.history_clear_last
        );
        assert!(
            !none.join("history.db").exists(),
            "①b 没有历史却把库建了出来"
        );
        // ①c `[history] enabled` 关着 ⇒ 说的是"没开"，也不许开库（P68 那枚分岔只有一个主人）。
        let off = sandbox("clear-disabled");
        m.history = Some(falcon_core::config::History {
            enabled: false,
            ..Default::default()
        });
        m.history_dir = off.clone();
        assert!(m.clear_history(true).is_none(), "①c 关着的开关反倒清了库");
        assert!(
            m.history_clear_last.contains("enabled"),
            "①c 要说清是没开: {:?}",
            m.history_clear_last
        );
        assert!(
            !off.join("history.db").exists(),
            "①c `enabled` 关着却把库打开了"
        );
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        });
        m.history_dir = hdir.clone();

        // ② 两行：先前那一枚锁起来，后面那一枚留着。
        assert_eq!(m.commit(), Some(hole), "② 第一行没记成");
        let locked = m.history_last_id.expect("② 第一行没报出行号");
        let second = PhysRect::new(5, 6, 30, 40);
        m.set_hole(second);
        assert_eq!(m.commit(), Some(second), "② 第二行没记成");
        let free = m.history_last_id.expect("② 第二行没报出行号");
        assert_ne!(locked, free, "② 两行记成了同一行，清空那问无从分辨");
        {
            let store = falcon_core::history::History::open(&hdir).expect("② 前置打不开库");
            store.set_locked(locked, true).expect("② 前置锁不上那一行");
        }
        let locked_file = picture_of(locked).expect("② 锁定那一行没有原图");
        let free_file = picture_of(free).expect("② 另一行没有原图");
        assert!(
            locked_file.exists() && free_file.exists(),
            "② 前置：两份图不在盘上"
        );
        assert_eq!(rows(), 2, "② 前置的库里不是两行");

        // ③ 正面：点头清空 ⇒ 未锁的那一行连它的文件一起走，锁定那一行两件都留下。
        let freed = m.clear_history(true).expect("③ 点头的清空没跑成");
        assert_eq!(freed.ids, vec![free], "③ 清掉的不是那一枚未锁的行");
        assert!(!free_file.exists(), "③ 库里删了那一行，盘上那份图却留着");
        assert_eq!(rows(), 1, "③ 清完库里不是只剩锁定那一行");
        assert!(
            locked_file.exists(),
            "③ 把用户锁定的那一行的图也删了 —— §5.14.4 规则 2 不许"
        );
        assert_eq!(m.history_clears, 1, "③ 清成了却没数到");
        assert_eq!(m.history_deletes, 0, "③ 清空吃了删除那格的计数");
        assert!(
            m.history_clear_last.contains("锁定"),
            "③ 没说清锁定项留下: {:?}",
            m.history_clear_last
        );
        assert_eq!(m.history_records, 2, "③ 清空改了 §5.14.1 那格的计数");

        // ④ 空手之一：库里只剩锁定项 ⇒ 跑成了而一行也没删。这一句不许写成"清空了 0 行"那种
        // 听着像成功的样子，也不许把 ③ 那句成功的话留在场上。
        let freed = m.clear_history(true).expect("④ 第二次清空没跑成");
        assert!(freed.ids.is_empty(), "④ 只剩锁定项却删掉了东西");
        assert_eq!(rows(), 1, "④ 一次空手把库里那一行也带走了");
        assert!(locked_file.exists(), "④ 一次空手删了锁定项的图");
        assert_eq!(m.history_clears, 1, "④ 删掉 0 行却吃了计数");
        assert!(
            m.history_clear_last.contains("一行也没删"),
            "④ 把空手说成了别的: {:?}",
            m.history_clear_last
        );

        // ⑤ 库打不开 ⇒ 说成打不开，并且不许留着 ④ 那句话冒充这次的答案。
        let wall = sandbox("clear-history-wall");
        std::fs::write(wall.join("plain.txt"), b"not a directory").expect("⑤ 的前置摆不出普通文件");
        m.history_dir = wall.join("plain.txt").join("history");
        assert!(
            m.clear_history(true).is_none(),
            "⑤ 打不开的库反倒交回了一次清空结果"
        );
        assert!(
            m.history_clear_last.contains("打不开"),
            "⑤ 要说清是打不开: {:?}",
            m.history_clear_last
        );
        assert_eq!(m.history_clears, 1, "⑤ 一次开库失败吃了计数");

        // ⑥ 没点头而库里确实有未锁的行：话要说成没确认，库里那位数字也不许动。这条与 ① 的
        // 区别是 ① 量"开没开库"，这一条量"有东西可删时它删不删" —— 少这一臂，①那句"没点头"
        // 就可能只是空库里的一句废话。
        m.history_dir = hdir.clone();
        assert!(
            m.clear_history(false).is_none(),
            "⑥ 没点头而库里有可删的行，反倒跑了"
        );
        assert_eq!(rows(), 1, "⑥ 没点头却删了东西");
        assert_eq!(m.history_clears, 1, "⑥ 被拒的那一次吃了计数");
        assert!(
            m.history_clear_last.contains("二次确认"),
            "⑥ 有东西可删时把话说成了别的: {:?}",
            m.history_clear_last
        );
    }

    /// §5.14.4 规则 2 里**打上那一枚锁**这半，P100 起（[`MaskState::lock_history`]，也是
    /// [`falcon_core::history::History::set_locked`] 在产品里的第一位读者）。
    ///
    /// 这一格要的判据是"只写一枚布尔"，所以它对面摆的是**四件不该动的东西**：库里的行数、那一行
    /// 指的原图、另两格计数、以及那两条腿各自说过的话 —— 一把锁如果顺手把清单快照或"上一次删了
    /// 几行"改了，§5.14.4 那三条相反的口径就在产品里并成了一条。
    ///
    /// 反向那一臂判的是"库里没有第 N 行"：`History::set_locked` 对不存在的行答
    /// `Missing`，本条不许把它转述成一次写成了的锁（与 [`MaskState::delete_history`] 对
    /// "删成了 0 行"那档同一条家规）。解开那一臂也当场跑一遍，因为规则 2 那句"用户明确"必然
    /// 包含用户改主意。
    ///
    /// 最后一条臂量的是两条腿**对上**的那件：锁上之后 [`MaskState::clear_history`] 带不走那一行，
    /// 而解开之后同一句 `clear_history(true)` 就带走了 —— 规则 2 从此在产品里是一整条，不再
    /// 是一半有读者、另一半由 core 的单测自己守着。
    #[test]
    fn a_cancelled_history_row_can_take_its_selection_back() {
        // §5.14.3 step 4「恢复该截图内容或重新进入原选区」的第一条产品腿（P102）。四格各问一件
        // 事：回去的框**是不是**那一行记的那一个、库里与盘上有没有被这条"读"的腿动过、那一行
        // 标着已取消时这句话有没有点名它、以及三种"回不去"是否一行也没动。
        let hole_a = PhysRect::new(10, 20, 100, 80);
        let hole_b = PhysRect::new(7, 11, 41, 23);
        let hdir = sandbox("restore-history");
        let mut m = savable(hole_a);
        // `save_all` 开着：③ 里 Esc 那一格欠一行，而那一条正是 §5.14.3 的开头。
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            save_all: true,
            ..Default::default()
        });
        m.history_dir = hdir.clone();
        let rows = || {
            falcon_core::history::History::open(&hdir)
                .and_then(|h| h.count())
                .unwrap_or(u64::MAX)
        };

        // ① 这一场没有历史 ⇒ 说不出往哪儿回，而且不许把库建出来。
        let fresh = sandbox("restore-no-history").join("brand-new");
        m.history = None;
        m.history_dir = fresh.clone();
        assert!(
            m.restore_history_selection(1).is_none(),
            "① 没有历史的一场反倒交回了一个选区"
        );
        assert!(
            !fresh.join("history.db").exists(),
            "① 没有历史却把库建了出来"
        );
        assert!(
            m.history_restore_last.contains("没有历史"),
            "① 要说清是这一场没有历史: {:?}",
            m.history_restore_last
        );
        assert_eq!(m.history_restores, 0, "① 一次没跑成的恢复吃了计数");

        // ①b `[history] enabled` 关着 ⇒ 说的是"没开"，那枚分岔只有一个主人（P68）。
        let off = sandbox("restore-disabled");
        m.history = Some(falcon_core::config::History {
            enabled: false,
            ..Default::default()
        });
        m.history_dir = off.clone();
        assert!(
            m.restore_history_selection(1).is_none(),
            "①b 关着的开关反倒交回了选区"
        );
        assert!(
            m.history_restore_last.contains("enabled"),
            "①b 要说清是没开: {:?}",
            m.history_restore_last
        );
        assert!(
            !off.join("history.db").exists(),
            "①b `enabled` 关着却把库打开了"
        );
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            save_all: true,
            ..Default::default()
        });
        m.history_dir = hdir.clone();

        // ② 正面一臂：**没取消**的那一行也回得去。`cancelled` 不是门槛（§5.14.3 问的是取消
        // 的那一条，但"把这个框交回遮罩"这个动作对两行同样成立），所以这句话只如实说它是什么。
        assert_eq!(m.commit(), Some(hole_a), "② 那一行没记成，恢复无从可恢复");
        let id_a = m.history_last_id.expect("② §5.14.1 没报出行号");
        m.set_hole(PhysRect::default());
        let (recs_b, replays_b, dels_b, clears_b, locks_b, said_b) = (
            m.history_records,
            m.history_replays,
            m.history_deletes,
            m.history_clears,
            m.history_locks,
            m.message.clone(),
        );
        let files_b = m.quick_saves;
        assert_eq!(
            m.restore_history_selection(id_a),
            Some(hole_a),
            "② 库里明明有那一行，交回来的却不是它记的那个框"
        );
        assert_eq!(m.hole, hole_a, "② 交回来了 rect 而遮罩的选区没跟着回去");
        assert_eq!(m.history_restores, 1, "② 回成了却没数到");
        assert_eq!(rows(), 1, "② 一次恢复带走了库里那一行");
        assert!(
            !m.history_restore_last.contains("已取消"),
            "② 那一行没取消，这句话却替它标了取消: {:?}",
            m.history_restore_last
        );
        assert!(
            m.history_restore_last.contains(&format!("第 {id_a} 行")),
            "② 成了却没点名是哪一行: {:?}",
            m.history_restore_last
        );
        assert_eq!(
            (
                m.history_records,
                m.history_replays,
                m.history_deletes,
                m.history_clears,
                m.history_locks,
                m.quick_saves
            ),
            (recs_b, replays_b, dels_b, clears_b, locks_b, files_b),
            "② 一条只读的腿挪动了记录数、回放数、删除/清空/锁、或落盘那几格计数"
        );
        assert_eq!(m.message, said_b, "② 恢复把落盘那句话覆盖成了自己的");

        // ③ §5.14.3 的那一臂：误按 Esc 欠下的那一行（`cancelled = true`），找回来的框必须
        // 就是它当初记的那一个，而且这句话要点名"已取消"——step 3 说的就是这一枚标记。
        m.set_hole(hole_b);
        assert!(m.escape(), "③ 前置：有框的一场按 Esc 却没接住");
        let id_b = m.history_last_id.expect("③ Esc 那一格没报出行号");
        assert_ne!(id_a, id_b, "③ 前置：取消的那一行没记成新的行号");
        m.set_hole(PhysRect::default());
        assert_eq!(
            m.restore_history_selection(id_b),
            Some(hole_b),
            "③ 取消那一行记的原选区没被交回来"
        );
        assert_eq!(m.hole, hole_b, "③ 取消那一行回成了别的框");
        assert_eq!(m.history_restores, 2, "③ 回成了却没数到第二次");
        assert!(
            m.history_restore_last.contains("已取消")
                && m.history_restore_last.contains(&format!("第 {id_b} 行")),
            "③ 这句话要同时点名行号与「已取消」: {:?}",
            m.history_restore_last
        );
        assert_eq!(rows(), 2, "③ 两行都在库里，数出来却不是二");

        // ④ 库里没有那一行 ⇒ 拒绝，且**当前的框一动不动**：把"没有这一行"讲成"回到了一个空框"
        // 就等于当着用户把选区清掉。
        let ghost = 90_000_000_i64;
        let keep = m.hole;
        assert!(
            m.restore_history_selection(ghost).is_none(),
            "④ 库里没有那一行，却交回了一次恢复"
        );
        assert_eq!(m.hole, keep, "④ 一次拒绝挪动了用户当前的选区");
        assert_eq!(m.history_restores, 2, "④ 一次没跑成的恢复吃了计数");
        assert!(
            m.history_restore_last.contains("没有第") && m.history_restore_last.contains("行"),
            "④ 要说清是库里没有那一行: {:?}",
            m.history_restore_last
        );

        // ⑤ 那一行记的**本来就是空 rect** ⇒ 同样拒绝。这一格问的是本条唯一那道边界：交回空框
        // 与清掉选区在 `set_hole` 那一头是同一个动作，所以门槛必须立在读回来之后、写进去之前。
        let empty_id = {
            let mut store = falcon_core::history::History::open(&hdir).unwrap();
            let cfg = falcon_core::config::History {
                enabled: true,
                keep_originals: true,
                ..Default::default()
            };
            let frame = falcon_core::frame::Frame::filled(3, 3, [9, 9, 9, 255]).unwrap();
            let meta = falcon_core::history::CaptureMeta {
                at_ms: 1_700_000_000_000,
                offset_min: 0,
                monitor_id: "单测种的那一块",
                rect: PhysRect::default(),
                source: falcon_core::history::Source::Manual,
                cancelled: false,
            };
            store.record(&meta, &frame, &cfg).unwrap()
        };
        let keep = m.hole;
        assert!(
            m.restore_history_selection(empty_id).is_none(),
            "⑤ 空的那一行反倒被当成一次恢复交回了"
        );
        assert_eq!(m.hole, keep, "⑤ 交回一个空框就是清掉了当前选区");
        assert_eq!(m.history_restores, 2, "⑤ 一次被拒的恢复吃了计数");
        assert!(
            m.history_restore_last.contains("空"),
            "⑤ 要说清那一行的原选区是空的: {:?}",
            m.history_restore_last
        );
        assert_eq!(rows(), 3, "⑤ 前置那三行该都在库里");
    }

    /// §5.14.2 翻页那一半的**锚点**（P113）：五格各问一件事 —— 说不出锚点时说的是"没有"、
    /// 刚记完一行时停在哪儿、记第二行会不会把界面也带走、翻回旧行时谁说了算、以及一次被拒的
    /// 翻页会不会伪造锚点。第三格与第四格是对着 [`Self::history_last_id`] 那枚证人问的：
    /// 一条只翻页的腿不许动"行数加一"那格的读数，否则量具上那一格就成了两条腿的合计。
    #[test]
    fn the_history_anchor_names_the_row_the_panel_is_sitting_on() {
        let hole_a = PhysRect::new(10, 20, 100, 80);
        let hole_b = PhysRect::new(7, 11, 41, 23);
        let hdir = sandbox("anchor-history");
        let mut m = savable(hole_a);
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            save_all: true,
            ..Default::default()
        });
        m.history_dir = hdir.clone();

        // ① 既没记成过、也没翻到过 ⇒ 说的是"没有锚点可问"，而不是第 0 行。`-1` 而不是 0，
        //    因为 0 会被 [`crate::settings::neighbor_id_for`] 那一头读成"库里第 0 行"。
        assert_eq!(m.history_anchor(), -1, "① 一场里什么都没有，锚点却报了行号");
        assert_eq!(m.history_viewed, None, "① 前置：这一场一开头就有锚点");

        // ② 记成第一行 ⇒ 锚点退到 [`Self::history_last_id`]：截图刚落地时界面就在那一行上。
        assert_eq!(m.commit(), Some(hole_a), "② 前置：第一行没记成");
        let id_a = m.history_last_id.expect("② §5.14.1 没报出行号");
        assert_eq!(m.history_viewed, None, "② 记一行不是翻一页，它不该产生锚点");
        assert_eq!(
            m.history_anchor(),
            id_a,
            "② 界面正停在刚记的那一行，锚点却不在它身上"
        );

        // ③ 记成第二行 ⇒ 跟着最新那一行走，仍然是回退那条腿在说话。
        m.set_hole(hole_b);
        assert!(m.escape(), "③ 前置：有框的一场按 Esc 却没接住");
        let id_b = m.history_last_id.expect("③ Esc 那一格没报出行号");
        assert_ne!(id_a, id_b, "③ 前置：两行记成了同一个行号");
        assert_eq!(m.history_viewed, None, "③ 又一次记录被当成了一次翻页");
        assert_eq!(
            m.history_anchor(),
            id_b,
            "③ 界面停在最近记成的那一行，锚点却还在旧行"
        );

        // ④ 翻回上一行 ⇒ 从这一刻起 [`Self::history_viewed`] 说了算，而 `history_last_id`
        //    一动不动。这一格才是 P113 不复用那一枚的理由被读出来的地方。
        assert_eq!(
            m.restore_history_selection(id_a),
            Some(hole_a),
            "④ 翻回上一行没把它的框交回来"
        );
        assert_eq!(m.history_viewed, Some(id_a), "④ 翻成了却没留下锚点");
        assert_eq!(
            m.history_anchor(),
            id_a,
            "④ 界面已经翻回上一行，锚点却还停在最新那一行"
        );
        assert_eq!(
            m.history_last_id,
            Some(id_b),
            "④ 一次翻页改写了「最近记成第几行」——那一枚是行数证人"
        );
        assert_eq!(m.history_records, 2, "④ 一次翻页凭空多记了一行");

        // ⑤ 一次被拒的翻页（库里没有那一行）⇒ 锚点不动。留着它跟着失败走，下一次翻页就会
        //    问到一个不存在的行的左右，而 §5.14.2 那句"到头了就不动"说的正是这一格。
        assert!(
            m.restore_history_selection(90_000_000).is_none(),
            "⑤ 库里没有那一行，却交回了一次恢复"
        );
        assert_eq!(m.history_anchor(), id_a, "⑤ 一次被拒的翻页挪动了锚点");
    }

    /// P113：翻页那一问**该开哪座库**。四格各问一件事：`[history]` 缺一半时说的是"没有库"而不是
    /// 拿空路径去开、两半齐了交回的是这一场自己那一对、以及**换一座库**之后同一枚行号就翻不动。
    ///
    /// 第四格不是假想的形状：`--autosave` 那档的 `retarget()` 只重钉 `MaskState` 这一对，而
    /// `Session` 的翻页腿自 P112 起读的是 `settings::history_data_dir()`（全局 config 的答案）。
    /// 门第一次跑第 31 格时行号是从 `%APPDATA%` 那座空库里问的、选区要交回的是沙盒那座 ⇒ 屏幕上
    /// 那句是「翻页没跑：上一条没有可去的行（… 库里没有第 3 行）」（`p113_auto_A.log`）。真实用户
    /// 那一头两份永远相等（`open()` 就是从同一处取的快照），所以这一族只有门能读出来。
    #[test]
    fn the_history_store_a_flip_asks_is_the_one_the_restore_reads() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let here = sandbox("store-here");
        let there = sandbox("store-there");
        let mut m = savable(hole);
        let cfg = falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            save_all: true,
            ..Default::default()
        };
        m.history = Some(cfg.clone());
        m.history_dir = here.clone();

        // ① / ② 两枚前置各缺一半 ⇒ 「没有可开的库」，不许把空路径拼成一座库去开
        m.history = None;
        assert!(
            m.history_store().is_none(),
            "① `[history]` 缺位却报出了一座库"
        );
        m.history = Some(cfg.clone());
        m.history_dir = std::path::PathBuf::new();
        assert!(
            m.history_store().is_none(),
            "② 落点是空串却报出了一座库——那一问会去开进程当前的工作目录"
        );

        // ③ 两枚都在 ⇒ 交回的就是这一场钉的那一对，一个字节也不换
        m.history_dir = here.clone();
        let (dir, got) = m.history_store().expect("③ 两枚前置都齐了却说没有库");
        assert_eq!(dir, here, "③ 交回的库不是这一场那一座");
        assert!(
            got.save_all,
            "③ 交回的 cfg 不是这一场那一份（出厂 `save_all` 为关）"
        );
        assert_eq!(got.max_items, 0, "③ 交回的 cfg 把这一场的上限换回了出厂值");

        // ④ 同一枚行号、两座库：先在自己那座记成一行、翻得动；把落点换成另一座（它有自己一座
        //    空库）⇒ 同一枚号立刻翻不动。判据对面是 SQLite，不是那句返回的话。
        assert_eq!(m.commit(), Some(hole), "④ 前置：第一行没记成");
        let id = m.history_last_id.expect("④ §5.14.1 没报出行号");
        assert_eq!(
            m.restore_history_selection(id),
            Some(hole),
            "④ 就在刚才记成的那座库里，自己的行翻不回来"
        );
        m.history_dir = there.clone();
        let (dir2, _) = m.history_store().expect("④ 换了落点却说没有库");
        assert_eq!(
            dir2, there,
            "④ `history_store` 还指着旧那一座——它不是这一场的快照"
        );
        assert!(
            m.restore_history_selection(id).is_none(),
            "④ 另一座库里没有那一行，却交回了一次恢复"
        );
        assert!(
            m.history_restore_last
                .contains(&format!("库里没有第 {id} 行")),
            "④ 换库之后那句该点名「库里没有第 {id} 行」，实际是 {:?}",
            m.history_restore_last
        );
    }

    /// P113：一次翻页同时吃两枚计数，而一次由代码直接按的恢复只吃一枚 —— 这两半合在
    /// [`MaskState::history_restores`] 一个数里就读不出是谁递的。本枚问的四件事都把对面放在
    /// **计数**上而不是那句返回的话上：话由 [`MaskState::restore_history_selection`] 一处说了算，
    /// 早已由 P102 那枚单测钉过，这里只钉"谁被数进去了、谁没有"。
    #[test]
    fn a_flip_counts_itself_apart_from_the_restore_it_reused() {
        let hole_a = PhysRect::new(10, 20, 100, 80);
        let hole_b = PhysRect::new(7, 11, 41, 23);
        let hdir = sandbox("flip-count");
        let mut m = savable(hole_a);
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            save_all: true,
            ..Default::default()
        });
        m.history_dir = hdir.clone();

        // ① 一次也没翻过 ⇒ 两枚都是 0。这一格是下面所有加数的零位，也是 `open()` 里那枚归零
        //    在本档唯一读得到的地方（单测不走 `open()`，所以它只能证明"新建的状态本来就带 0"）。
        assert_eq!(m.history_restores, 0, "① 新状态里恢复计数不是零");
        assert_eq!(m.history_flips, 0, "① 新状态里翻页计数不是零");

        // ② 被拒的翻页（库里没有那一行）⇒ 两枚都不加。「按了一下但什么都没换成」不算翻过一页，
        //    也不算回进过一次原选区 —— 这一格防的是把计数挂在调用上而不是挂在回成上。
        assert!(
            m.flip_history_selection(90_000_000).is_none(),
            "② 库里没有那一行，翻页却交回了一次成功"
        );
        assert_eq!(m.history_restores, 0, "② 一次被拒的翻页吃了恢复计数");
        assert_eq!(m.history_flips, 0, "② 一次被拒的翻页吃了翻页计数");
        assert!(
            m.history_restore_last.contains("没有第 90000000 行"),
            "② 本条不另编话，交回的应是 `restore_history_selection` 那一句: {:?}",
            m.history_restore_last
        );

        // ③ 记两行，然后**直接**恢复一次 ⇒ 只有 `history_restores` 加。这一格是那枚拆分的另一半：
        //    没有它，"翻页数 = 恢复数"这一句在这里与④之后都成立，拆开的理由就没了证人。
        assert_eq!(m.commit(), Some(hole_a), "③ 前置：第一行没记成");
        let id_a = m.history_last_id.expect("③ §5.14.1 没报出行号");
        m.set_hole(hole_b);
        assert!(m.escape(), "③ 前置：有框的一场按 Esc 却没接住");
        let id_b = m.history_last_id.expect("③ Esc 那一格没报出行号");
        assert_eq!(
            m.restore_history_selection(id_a),
            Some(hole_a),
            "③ 前置：库里刚记的那一行恢复不回来"
        );
        assert_eq!(m.history_restores, 1, "③ 一次直接恢复没被数进恢复计数");
        assert_eq!(m.history_flips, 0, "③ 一次直接恢复被当成了翻页");

        // ④ 翻成一页 ⇒ 两枚各加一，而 `restores - flips` 恰好还是"直接按的那几次"。锚点跟着走
        //    是 §5.14.2 那一格的本分（P113 的锚点单测已钉），这里补一句：加两枚不等于记两行。
        assert_eq!(
            m.flip_history_selection(id_b),
            Some(hole_b),
            "④ 库里明明有那一行，翻页却没把框交回来"
        );
        assert_eq!(
            m.history_restores, 2,
            "④ 一次翻页没走 `restore_history_selection` 那条通路"
        );
        assert_eq!(m.history_flips, 1, "④ 翻成了却没数到");
        assert_eq!(
            m.history_restores - m.history_flips,
            1,
            "④ 拆出来的这一半应当正好是直接按的那一次"
        );
        assert_eq!(m.hole, hole_b, "④ 翻回来了而遮罩的选区没跟着过去");
        assert_eq!(m.history_viewed, Some(id_b), "④ 一次成功的翻页没留下锚点");
        assert_eq!(m.history_records, 2, "④ 一次恢复与一次翻页凭空多记了一行");
    }

    #[test]
    fn locking_a_named_history_row_writes_the_lock_and_nothing_else() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let hdir = sandbox("lock-history");
        let mut m = savable(hole);
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        });
        m.history_dir = hdir.clone();
        let rows = || {
            falcon_core::history::History::open(&hdir)
                .and_then(|h| h.count())
                .unwrap_or(u64::MAX)
        };
        let locked_of = |id: i64| -> Option<bool> {
            let store = falcon_core::history::History::open(&hdir).ok()?;
            store.get(id).ok()?.map(|rec| rec.locked)
        };
        let picture_of = |id: i64| -> Option<std::path::PathBuf> {
            let store = falcon_core::history::History::open(&hdir).ok()?;
            let rec = store.get(id).ok().flatten()?;
            store.path_of(&rec)
        };

        // ① 这一场没有历史：换一枚不存在的落点，"开没开库"当场数得出来。
        let fresh = sandbox("lock-no-history").join("brand-new");
        m.history = None;
        m.history_dir = fresh.clone();
        assert!(
            m.lock_history(1, true).is_none(),
            "① 没有历史的一场反倒去锁了库里的行"
        );
        assert!(
            !fresh.join("history.db").exists(),
            "① 没有历史却把库建了出来"
        );
        assert!(
            m.history_lock_last.contains("没有历史"),
            "① 要说清是这一场没有历史: {:?}",
            m.history_lock_last
        );
        assert_eq!(m.history_locks, 0, "① 一次没跑成的锁吃了计数");

        // ①b `[history] enabled` 关着 ⇒ 说的是"没开"，也不许开库（P68 那枚分岔只有一个主人）。
        let off = sandbox("lock-disabled");
        m.history = Some(falcon_core::config::History {
            enabled: false,
            ..Default::default()
        });
        m.history_dir = off.clone();
        assert!(
            m.lock_history(1, true).is_none(),
            "①b 关着的开关反倒去锁了库"
        );
        assert!(
            m.history_lock_last.contains("enabled"),
            "①b 要说清是没开: {:?}",
            m.history_lock_last
        );
        assert!(
            !off.join("history.db").exists(),
            "①b `enabled` 关着却把库打开了"
        );
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        });
        m.history_dir = hdir.clone();

        // ② 正面：记成一行，锁上它。写成的那一次只许动那一枚布尔，四件都不许跟着动。
        assert_eq!(m.commit(), Some(hole), "② 那一行没记成，锁无从可锁");
        let id = m.history_last_id.expect("② §5.14.1 没报出行号");
        let picture = picture_of(id).expect("② 前置：那一行没有原图");
        assert!(picture.exists(), "② 前置：那份原图不在盘上");
        let (recs_b, last_b, dels_b, clears_b) = (
            m.history_records,
            m.history_last_id,
            m.history_deletes,
            m.history_clears,
        );
        m.history_delete_last = "上一次删除说过的话".to_string();
        m.history_clear_last = "上一次清空说过的话".to_string();
        let del_said_b = m.history_delete_last.clone();
        let clear_said_b = m.history_clear_last.clone();
        assert!(
            m.lock_history(id, true).is_some(),
            "② 库里明明有那一行，锁却没写成"
        );
        assert_eq!(locked_of(id), Some(true), "② 锁上了盘上却读不到");
        assert_eq!(m.history_locks, 1, "② 锁成了却没数到");
        assert!(
            m.history_lock_last.contains(&format!("第 {id} 行"))
                && m.history_lock_last.contains("锁上"),
            "② 成了却没点名行号、或没说清是锁上: {:?}",
            m.history_lock_last
        );
        assert_eq!(rows(), 1, "② 一把锁带走了库里那一行");
        assert!(picture.exists(), "② 一把锁带走了那份原图");
        assert_eq!(
            (
                m.history_records,
                m.history_last_id,
                m.history_deletes,
                m.history_clears
            ),
            (recs_b, last_b, dels_b, clears_b),
            "② 一把锁挪动了记录数、最后行号、或删除/清空那两格计数"
        );
        assert_eq!(
            (
                m.history_delete_last.as_str(),
                m.history_clear_last.as_str()
            ),
            (del_said_b.as_str(), clear_said_b.as_str()),
            "② 一把锁把另两条腿说过的话覆盖成了自己的"
        );

        // ③ 库里没有的那一行 ⇒ `Missing`，不许转述成一次写成了的锁、也不许吃计数。
        let ghost = 90_000_000_i64;
        assert_ne!(
            locked_of(ghost),
            Some(true),
            "③ 前置：不存在的那一行反倒带着锁"
        );
        assert!(
            m.lock_history(ghost, true).is_none(),
            "③ 库里没有那一行，却交回了一次写成的锁"
        );
        assert_eq!(m.history_locks, 1, "③ 一次被拒的锁吃了计数");
        assert!(
            m.history_lock_last.contains("改不动")
                && m.history_lock_last.contains(&ghost.to_string()),
            "③ 要说清是第 {ghost} 行改不动: {:?}",
            m.history_lock_last
        );
        assert_eq!(rows(), 1, "③ 一次空手把库里那一行也带走了");
        assert_eq!(locked_of(id), Some(true), "③ 一次空手把②那枚锁也改了");

        // ④ 解开那一臂：规则 2 那句"用户明确"包含用户改主意，所以 `false` 也得当场跑一遍。
        assert!(m.lock_history(id, false).is_some(), "④ 解开那一臂没写成");
        assert_eq!(locked_of(id), Some(false), "④ 解开了盘上却还带着锁");
        assert_eq!(m.history_locks, 2, "④ 解开那一臂没数到");
        assert!(
            m.history_lock_last.contains("解开"),
            "④ 解开却说成了锁上: {:?}",
            m.history_lock_last
        );

        // ⑤ 两条腿对上规则 2：锁上 ⇒ `clear_history(true)` 带不走它；解开 ⇒ 同一句话就带走了。
        assert!(m.lock_history(id, true).is_some(), "⑤ 前置：那一行没锁上");
        let cleared_locked = m.clear_history(true).expect("⑤ 清空没跑成");
        assert_eq!(cleared_locked.ids, vec![], "⑤ 锁着的那一行被清空带走了");
        assert_eq!(rows(), 1, "⑤ 清空锁着的那行之后库里不该剩一行");
        assert!(picture.exists(), "⑤ 清空带走了锁着那行的原图");
        assert_eq!(m.history_clears, clears_b, "⑤ 删了 0 行却吃了清空计数");
        assert_eq!(m.history_locks, 3, "⑤ 前置那次锁没数到");
        assert!(m.lock_history(id, false).is_some(), "⑤ 收尾前的解开没写成");
        let cleared_free = m.clear_history(true).expect("⑤ 第二次清空没跑成");
        assert_eq!(cleared_free.ids, vec![id], "⑤ 解开之后清空该带走那一行");
        assert_eq!(rows(), 0, "⑤ 解开并被清空之后库里还剩行");
        assert!(!picture.exists(), "⑤ 那一行走空了、原图却还在盘上");

        // ⑥ 库打不开 ⇒ 说成打不开，并且不许留着 ⑤ 那句成功的答复冒充这次的答案。
        let wall = sandbox("lock-history-wall");
        std::fs::write(wall.join("plain.txt"), b"not a directory").expect("⑥ 的前置摆不出普通文件");
        m.history_dir = wall.join("plain.txt").join("history");
        assert!(
            m.lock_history(id, true).is_none(),
            "⑥ 打不开的库反倒交回了一次写成的锁"
        );
        assert!(
            m.history_lock_last.contains("打不开"),
            "⑥ 要说清是打不开: {:?}",
            m.history_lock_last
        );
        assert_eq!(m.history_locks, 4, "⑥ 一次开库失败吃了计数");
    }

    /// The two rungs of 复制 that answer *before* the clipboard is touched, and the
    /// sentences that say which one answered.
    ///
    /// This is the headless half of §9.1 第十条: the gauge's own copy row walks the
    /// real clipboard and then puts the user's old contents back, so the refusals
    /// underneath the write - no selection, no picture - are branches it can only
    /// reach by accident. Here they are the whole test, and neither of them may open
    /// the clipboard: a test that wrote to the user's clipboard to prove it can
    /// decline would be a bad trade.
    #[test]
    fn copying_answers_with_a_sentence_before_it_opens_the_clipboard() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(!m.copy_out());
        assert!(
            m.message.contains("还没有选区"),
            "an empty hole said {:?}",
            m.message
        );

        // A hole the user drew, on a layer that was never given frames: the answer is
        // a different sentence, which is what tells the gauge's row - and a user -
        // that this is the stitching, not the selection, that came up empty.
        m.set_hole(PhysRect::new(10, 20, 30, 40));
        assert!(!m.copy_out());
        assert!(
            m.message.contains("拼不出来"),
            "a hole with no frames said {:?}",
            m.message
        );
    }

    /// A directory under the OS temp dir, emptied first: a file the previous run of
    /// this test left behind would grade this one's write without being written by it.
    fn sandbox(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-save-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    /// A mask state whose layer can really stitch a frame. [`annotate::Layer::begin`]
    /// is the only way a canvas exists without Qt, and nothing in it reaches the image
    /// provider. The freeze is `Frame::new` - all zero, so every pixel of the selection
    /// is alpha 0, which is the one condition the transparency prompt is about and does
    /// not need an eraser, a pen or a screen to arrange.
    fn savable(hole: PhysRect) -> MaskState {
        let bounds = PhysRect::new(0, 0, 640, 480);
        let snap = snapshot(vec![monitor("m0", 0, 0, 640, 480, 1.0, true)]);
        let mut m = state_with(vec![slot(0, bounds, 1.0)], PhysRect::default());
        m.ink.begin(&snap, &m.slots);
        m.virtual_bounds = bounds;
        m.set_hole(hole);
        m
    }

    /// The rungs of 另存为 that answer before a byte is written, and the refusal that
    /// does not leave a file behind.
    ///
    /// The headless half of §9.1 第十条 again: the gauge's save legs walk a real file on
    /// a real desktop, so "nothing selected", "nothing stitched" and "a name this app
    /// cannot write" are branches it reaches only by accident. Here they are the whole
    /// test. The `!exists()` assertions are what makes it worth having - a refusal that
    /// wrote an empty or half file would still return `false` and still read as a
    /// refusal, while leaving the user a picture that is not a picture.
    #[test]
    fn saving_refuses_without_writing_anything() {
        let dir = sandbox("refuse");
        let png = dir.join("selection.png");

        let mut m = state_with(
            vec![slot(0, PhysRect::new(0, 0, 640, 480), 1.0)],
            PhysRect::default(),
        );
        assert!(!m.save_out(&png.to_string_lossy(), ""));
        assert!(
            m.message.contains("还没有选区"),
            "an empty hole said {:?}",
            m.message
        );
        assert!(!png.exists(), "a refusal wrote a file");

        // A hole the user drew, on a layer that was never given frames.
        m.hole = PhysRect::new(10, 20, 100, 80);
        assert!(!m.save_out(&png.to_string_lossy(), ""));
        assert!(
            m.message.contains("拼不出来"),
            "a hole with no frames said {:?}",
            m.message
        );
        assert!(!png.exists(), "the second refusal wrote a file");

        // A name that is not a format, on a state that could have written it: refused
        // after the frame exists, which is the order that lets the sentence name the
        // formats rather than the state.
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        let odd = dir.join("selection.qqq");
        assert!(!m.save_out(&odd.to_string_lossy(), "*.png"));
        assert!(
            m.message.contains("不是本程序会写的格式"),
            "an unknown extension said {:?}",
            m.message
        );
        assert!(!odd.exists(), "the refused name appeared on disk");
        assert!(
            !dir.join("selection.qqq.tmp").exists(),
            "a tmp file was left"
        );
    }

    /// PRD 506 ("包含透明背景时，应推荐 PNG") and 447 ("不支持透明度的输出格式应提醒用户
    /// 背景将被填充"), read against the same selection: PNG keeps what the document has,
    /// JPG does not - and says so, with the number of pixels it threw away.
    #[test]
    fn saving_says_what_a_format_without_alpha_threw_away() {
        let dir = sandbox("transparency");
        let hole = PhysRect::new(10, 20, 100, 80);
        let mut m = savable(hole);
        let shipped = m
            .ink
            .deliver()
            .expect("a hole inside one canvas has a picture");
        let total = (hole.w * hole.h) as usize;

        let png = dir.join("selection.png");
        assert!(m.save_out(&png.to_string_lossy(), ""), "{:?}", m.message);
        assert!(m.message.contains("已保存到"), "{:?}", m.message);
        assert!(
            !m.message.contains("透明"),
            "PNG was told it lost transparency: {:?}",
            m.message
        );
        let back = encode::decode(&std::fs::read(&png).expect("the png is there")).expect("png");
        assert_eq!(back.width, shipped.width);
        assert_eq!(back.height, shipped.height);
        assert_eq!(
            back.pixels, shipped.pixels,
            "PNG did not keep the alpha the document had"
        );

        let jpg = dir.join("selection.jpg");
        assert!(m.save_out(&jpg.to_string_lossy(), ""), "{:?}", m.message);
        assert!(
            m.message.contains(&format!("{total} px 透明")),
            "the prompt did not name the size of what it filled: {:?}",
            m.message
        );
        assert!(m.message.contains("填成白色"), "{:?}", m.message);
        assert!(m.message.contains("PNG"), "{:?}", m.message);
        let back = encode::decode(&std::fs::read(&jpg).expect("the jpg is there")).expect("jpg");
        assert!(
            back.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            "JPG kept an alpha channel, so the sentence lied"
        );
    }

    /// §10 第 46 项 ①：PRD 447 说的是"**应提醒**用户背景**将**被填充"，而不是"存完之后告
    /// 诉用户填了"。所以这一条测的是**顺序**：提醒说出口的那一刻，文件还不许在盘上；它自己也
    /// 不许写。第二半是两句话只差一个时态——同一位用户在一扇门前不能听到两种说法。
    #[test]
    fn the_notice_is_said_before_the_file_exists() {
        let dir = sandbox("pre-notice");
        let hole = PhysRect::new(10, 20, 100, 80);
        let mut m = savable(hole);
        let total = (hole.w * hole.h) as usize;

        let png = dir.join("selection.png");
        let jpg = dir.join("selection.jpg");

        // PNG 档无话：空串是"这一档不丢透明"，不是"没提醒"。
        assert!(m.remind_before_save(&png.to_string_lossy(), "").is_empty());
        assert_eq!(m.pre_notice, "");
        assert!(!png.exists(), "a reminder wrote a file");

        // JPG 档有话，而且说的时候那份文件还不存在。
        let pre = m.remind_before_save(&jpg.to_string_lossy(), "");
        assert!(pre.contains(&format!("{total} px 透明")), "{pre}");
        assert!(pre.contains("将被填成白色"), "{pre}");
        assert!(pre.contains("存成 PNG"), "{pre}");
        assert!(!jpg.exists(), "the reminder itself wrote {jpg:?}");
        assert_eq!(m.pre_notice, pre, "窗口那一格与返回值不是同一句");

        assert!(m.save_out(&jpg.to_string_lossy(), ""), "{:?}", m.message);
        assert!(jpg.exists(), "the write did not follow the notice");
        let post = pre.replace("将被填成白色", "已填成白色");
        assert!(
            m.message.ends_with(&post),
            "两句话差的不止时态：前 {pre:?} 后 {:?}",
            m.message
        );

        // 一扇会拒的门也不留下提醒：名字定不下格式，就没有"将要丢掉的像素"可说。
        let odd = dir.join("selection.qqq");
        assert!(m.remind_before_save(&odd.to_string_lossy(), "").is_empty());
        assert_eq!(
            m.pre_notice, "",
            "a refused name kept a stale notice on the window"
        );
        assert!(!odd.exists());
    }

    /// §5.5.5's dialog is showing a format while the user types a bare name: when the
    /// filter names nothing the configured format wins, and the file that appears is the
    /// one the sentence names.
    ///
    /// BMP is the format under test, not JPG, because the prompt must key off "this
    /// format has no alpha" rather than off the one format the PRD happens to name.
    #[test]
    fn a_bare_name_becomes_the_configured_format() {
        let dir = sandbox("bare");
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        m.export.format = encode::Format::Bmp;
        let bare = dir.join("selection");
        assert!(m.save_out(&bare.to_string_lossy(), ""), "{:?}", m.message);
        let written = dir.join("selection.bmp");
        assert!(written.exists(), "the bare name did not become {written:?}");
        assert!(
            !bare.exists(),
            "the name without an extension was written too"
        );
        assert!(
            m.message.contains("selection.bmp"),
            "the sentence named {:?}",
            m.message
        );
        assert!(m.message.contains("透明"), "{:?}", m.message);
    }

    /// The same door one rung further along (§10 第 40 项 ②): the mask's `另存为` now hands
    /// `save::write` the extension text of the filter the dialog was showing, so a bare
    /// name takes *that* format and `[output] format` speaks only when the filter does not.
    ///
    /// The configured format is BMP here while the filter says PNG, and the second leg says
    /// JPG - two formats the setting would never have chosen - so a door that drops `chosen`
    /// on the way to the owner, which is exactly the argument shape this round changed,
    /// cannot pass. The sentence is read as well as the file: a mask that wrote
    /// `selection.jpg` and named `selection.png` in `message` would still be a mask that
    /// lied to the user.
    #[test]
    fn the_dialogs_filter_gives_a_bare_name_its_format() {
        for (filter, want) in [("*.png", "png"), ("*.jpg", "jpg")] {
            let dir = sandbox("filter-door");
            let mut m = savable(PhysRect::new(10, 20, 100, 80));
            m.export.format = encode::Format::Bmp;
            let bare = dir.join("selection");
            assert!(
                m.save_out(&bare.to_string_lossy(), filter),
                "{:?}",
                m.message
            );
            let written = dir.join(format!("selection.{want}"));
            assert!(
                written.exists(),
                "{filter}: the bare name did not become {written:?}; it said {:?}",
                m.message
            );
            assert_eq!(
                crate::save::written_as(&written),
                crate::save::filter_format(filter),
                "{filter}: the name is right but a different encoder ran"
            );
            assert!(
                m.message.contains(&format!("selection.{want}")),
                "{filter}: the sentence named {:?}",
                m.message
            );
            assert!(
                !dir.join("selection.bmp").exists(),
                "{filter}: the configured format was written as well"
            );
        }
    }

    /// §5.3.9: arrows, with the step and the modifier this project chose (the PRD
    /// fixes only "finer or larger steps" and "never leave the valid area").
    #[test]
    fn arrows_move_and_a_modifier_resizes() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        m.nudge_by(1, 0, false);
        assert_eq!(m.hole, PhysRect::new(401, 400, 800, 600));
        m.nudge_by(0, -10, true);
        assert_eq!(m.hole, PhysRect::new(401, 400, 800, 590));
        // Pinned to the right edge, a further push right cannot take it out.
        m.set_hole(PhysRect::new(2272, 100, 800, 600));
        m.nudge_by(50, 0, false);
        assert_eq!(m.hole.right(), 3072);
    }

    /// A grip code is what QML turns into a cursor, so the numbering is an
    /// interface: eight edges and corners, the body, a new rectangle, nothing.
    #[test]
    fn the_grip_codes_name_every_handle_once() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        assert_eq!(m.grip_code(), 0);
        // The selection is physical `(400,400,800,600)`; at 200% the pointer lives
        // in `(200,200)-(600,500)`, so every coordinate below is the rect's edge
        // divided by the scale, not the rect's edge.
        let mut codes = Vec::new();
        for (x, y) in [
            (200.0, 200.0), // Nw
            (400.0, 200.0), // N
            (600.0, 200.0), // Ne
            (600.0, 350.0), // E
            (600.0, 500.0), // Se
            (400.0, 500.0), // S
            (200.0, 500.0), // Sw
            (200.0, 350.0), // W
        ] {
            m.press(0, x, y);
            codes.push(m.grip_code());
            m.release();
        }
        assert_eq!(codes, vec![2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(!codes.contains(&0) && !codes.contains(&1));
        m.press(0, 400.0, 350.0);
        assert_eq!(m.grip_code(), 10, "the body");
        m.release();
        m.press(0, 1400.0, 900.0);
        assert_eq!(m.grip_code(), 1, "a new rectangle");
    }

    /// The cursor is drawn from a hover, so the hover answer has to be the press
    /// answer - and it must reach that conclusion without moving anything.
    #[test]
    fn the_pointer_is_asked_before_it_presses() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        let before = m.hole;
        assert_eq!(m.hit_test(0, 200.0, 200.0), 2, "the top-left corner");
        assert_eq!(m.hit_test(0, 400.0, 350.0), 10, "the body");
        assert_eq!(m.hit_test(0, 1400.0, 900.0), 1, "a fresh rectangle");
        assert_eq!(
            m.hit_test(0, 200.0, 205.0),
            2,
            "still the corner, 5 DIP down"
        );
        assert_eq!(m.hole, before, "and nothing moved");

        // With the grip named, the hover and the press must agree everywhere.
        for (x, y) in [
            (200.0, 200.0),
            (400.0, 200.0),
            (600.0, 200.0),
            (600.0, 350.0),
            (600.0, 500.0),
            (400.0, 500.0),
            (200.0, 500.0),
            (200.0, 350.0),
            (1400.0, 900.0),
        ] {
            let hover = m.hit_test(0, x, y);
            m.press(0, x, y);
            assert_eq!(m.grip_code(), hover, "hover and press at ({x}, {y})");
            m.release();
        }

        // No selection yet: every point of the screen is "start drawing", which is
        // what keeps the cursor a crosshair instead of an arrow on a fresh mask.
        m.set_hole(PhysRect::default());
        assert_eq!(m.hit_test(0, 700.0, 400.0), 1);
    }

    /// Two captures in one process must not share a frame key.
    ///
    /// The failure this guards is invisible on screen: `Image.cache: false` only
    /// re-requests when the URL changes, so a repeated key means the second freeze
    /// is stored under a name Qt has already cached a picture for, and the overlay
    /// reports a new frame while showing the old one.
    #[test]
    fn a_second_capture_asks_for_a_key_nobody_has_used() {
        let mut m = MaskState::default();
        let first = m.take_revision();
        let second = m.take_revision();
        assert_eq!(first, 1, "revisions are one-based, 0 is no frame at all");
        assert_ne!(first, second, "and never repeated");
        assert_ne!(
            MaskState::frame_key("m0-display1", first),
            MaskState::frame_key("m0-display1", second),
            "so the provider URL changes with the capture"
        );
    }

    /// §5.5.4's 快速保存, from the door a click and `Ctrl+Shift+S` both reach.
    ///
    /// The template is `%p-%i` rather than the shipped one because this leg has to be
    /// about the *door*: a name built from the wall clock would make two presses in one
    /// test run collide on the second and turn the assertion into a race with the clock,
    /// while `%i` is the setting's own way of saying "the second press is the second
    /// file". `%p` is asserted too, since a prefix that never reached the name is the
    /// silent half of this feature - the file would appear, and nobody would be able to
    /// tell that `[output] naming_prefix` had anything to do with it.
    #[test]
    fn a_quick_save_names_its_own_file_and_counts_its_own_series() {
        let dir = sandbox("quick");
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        m.naming = crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "quick".into(),
            collision: falcon_core::naming::Collision::Increment,
        };

        // Nothing selected: the refusal is the user's own, and it writes nothing.
        let mut empty = savable(PhysRect::default());
        empty.naming = m.naming.clone();
        assert!(!empty.quick_out());
        assert!(empty.message.contains("还没有选区"), "{:?}", empty.message);

        assert!(m.quick_out(), "{:?}", m.message);
        let first = dir.join("quick-001.png");
        assert!(first.exists(), "{:?}", m.message);
        assert!(
            m.message.contains("quick-001.png"),
            "the sentence did not name the file it wrote: {:?}",
            m.message
        );
        assert!(m.message.contains("已保存到"), "{:?}", m.message);
        assert_eq!(
            crate::save::written_as(&first),
            Some(encode::Format::Png),
            "a quick save wrote something other than `[output] format`"
        );

        // The second press is the second file, and the number moved with it.
        assert!(m.quick_out(), "{:?}", m.message);
        assert!(dir.join("quick-002.png").exists(), "{:?}", m.message);
        assert!(m.message.contains("quick-002.png"), "{:?}", m.message);
        assert_eq!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .filter_map(Result::ok)
                .count(),
            2,
            "the two presses wrote a different number of files than the sentence counted"
        );

        // A refusal does not spend a number. `keep_both` turns the third press into a
        // sentence about the file that was already there and touches nothing; the same
        // press under `increment` still asks for `003` - the refused name, not the next
        // one - and makes room beside it.
        let planted = dir.join("quick-003.png");
        std::fs::write(&planted, b"not a picture this program wrote").expect("plant");
        let before = std::fs::read_dir(&dir)
            .expect("dir")
            .filter_map(Result::ok)
            .count();
        m.naming.collision = falcon_core::naming::Collision::KeepBoth;
        assert!(!m.quick_out(), "{:?}", m.message);
        assert!(m.message.contains("keep_both"), "{:?}", m.message);
        assert!(m.message.contains("quick-003.png"), "{:?}", m.message);
        assert!(
            std::fs::read(&planted)
                .expect("still there")
                .starts_with(b"not a picture"),
            "the refusal overwrote the file it refused about"
        );
        assert_eq!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .filter_map(Result::ok)
                .count(),
            before,
            "a refusal wrote a file"
        );

        m.naming.collision = falcon_core::naming::Collision::Increment;
        assert!(m.quick_out(), "{:?}", m.message);
        let beside = dir.join("quick-003_1.png");
        assert!(
            crate::save::written_as(&beside) == Some(encode::Format::Png),
            "the retry did not land beside the name the refusal quoted: {:?}",
            m.message
        );
        assert!(m.message.contains("quick-003_1.png"), "{:?}", m.message);

        // And the number the refusal declined to use is still unused: the next press is
        // the third file of the series, not the fourth.
        assert!(m.quick_out(), "{:?}", m.message);
        assert!(m.message.contains("quick-004.png"), "{:?}", m.message);
        assert!(dir.join("quick-004.png").exists(), "{:?}", m.message);
    }

    /// §5.5.8 "覆盖原图" —— 同一次 mask 会话里连按两次快速保存：
    /// 开关关着时（默认）第二份是 `-002`；开关开着时第二次覆写第一次那一份、
    /// 目录里始终只有一枚文件，`quick_saves` 仍加一而 `landed` 保持不动。
    #[test]
    fn reuse_last_file_overwrites_the_same_path_only_when_the_switch_says_so() {
        let dir = sandbox("reuse");
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        m.naming = crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "reuse".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        assert!(
            !m.reuse_last_file,
            "默认的 reuse_last_file 就该是关着（config `Default` = false）"
        );

        // 关着 —— 第一次落 `reuse-001`，第二次落 `reuse-002`，目录里两枚文件。
        assert!(m.quick_out(), "{:?}", m.message);
        assert!(dir.join("reuse-001.png").exists(), "{:?}", m.message);
        assert_eq!(
            m.landed.as_deref(),
            Some(dir.join("reuse-001.png").as_path())
        );
        assert!(m.quick_out(), "{:?}", m.message);
        assert!(dir.join("reuse-002.png").exists(), "{:?}", m.message);
        assert_eq!(m.quick_saves, 2, "关着时两次快速保存应各自加一次数");
        let files_off = std::fs::read_dir(&dir)
            .expect("dir")
            .filter_map(Result::ok)
            .count();
        assert_eq!(files_off, 2, "开关关着时那两枚文件各在自己的位置上");

        // 开着 —— 目录换新，落一枚，再落覆写同一枚；landed 不变，quick_saves 仍递增。
        let dir2 = sandbox("reuse_on");
        let mut n = savable(PhysRect::new(10, 20, 100, 80));
        n.naming = crate::settings::Naming {
            dir: dir2.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "reuse".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        n.reuse_last_file = true;
        assert!(n.quick_out(), "{:?}", n.message);
        let only = dir2.join("reuse-001.png");
        assert!(only.exists(), "{:?}", n.message);
        assert_eq!(n.landed.as_deref(), Some(only.as_path()));
        let first_bytes = std::fs::read(&only).expect("first write");
        std::thread::sleep(std::time::Duration::from_millis(30));

        assert!(n.quick_out(), "{:?}", n.message);
        assert!(
            !dir2.join("reuse-002.png").exists(),
            "开着时第二次快速保存不该另开一枚文件，而是覆写第一次那一份"
        );
        assert_eq!(n.landed.as_deref(), Some(only.as_path()));
        assert_eq!(n.quick_saves, 2, "开着时 quick_saves 仍要按次数加");
        let second_bytes = std::fs::read(&only).expect("second write");
        assert!(!second_bytes.is_empty(), "覆写之后那一份文件不能是空的");
        let files_on = std::fs::read_dir(&dir2)
            .expect("dir2")
            .filter_map(Result::ok)
            .count();
        assert_eq!(
            files_on,
            1,
            "开着时两次快速保存应在同一枚文件上，而不是各留一份；first_len={} second_len={}",
            first_bytes.len(),
            second_bytes.len()
        );
    }

    /// §5.5.8 **跨 mask 会话** reuse 那半（第 63 项 ⑦）：`seed_landed_from` 的三格判定 ——
    /// 关着时不抄、`prev=None` 时不抄、路径不存在时也不抄；三格都过了才把上一次会话
    /// 留下的那一份抄进本会话的 `landed`，于是本会话第一次 `quick_out` 覆写的是同一枚
    /// 文件而不是新起一份 `quick-00N`。
    ///
    /// 这里传的是 `Option<PathBuf>` 参数而不是走 `crate::settings::last_landed()`：
    /// 后者是进程全局（OnceLock + Mutex），并行跑测里改一次会砸到其他测试的读数上，
    /// 而这一句的语义本身与全局无关 —— 全局那一头由 `open()` 里那一次调用点负责。
    #[test]
    fn seed_landed_from_only_delivers_when_the_switch_is_on_and_the_file_is_there() {
        let dir = sandbox("seed");
        let prev = dir.join("reuse-001.png");
        std::fs::write(&prev, b"seed-prev").expect("seed prev file");

        // 格 1 —— reuse_last_file=false，prev 存在也不抄。
        let mut off = savable(PhysRect::new(10, 20, 100, 80));
        off.reuse_last_file = false;
        off.seed_landed_from(Some(prev.clone()));
        assert!(
            off.landed.is_none(),
            "开关关着时跨会话那半整段不该生效，landed 保持 None（第 63 项 ⑦ 三格判定 1）"
        );

        // 格 2 —— reuse_last_file=true，prev=None。
        let mut none = savable(PhysRect::new(10, 20, 100, 80));
        none.reuse_last_file = true;
        none.seed_landed_from(None);
        assert!(
            none.landed.is_none(),
            "上一次会话没落过任何东西时没有可抄的（第 63 项 ⑦ 三格判定 2）"
        );

        // 格 3 —— reuse_last_file=true，prev 指向一枚已不存在的文件。
        let mut ghost = savable(PhysRect::new(10, 20, 100, 80));
        ghost.reuse_last_file = true;
        ghost.seed_landed_from(Some(dir.join("gone.png")));
        assert!(
            ghost.landed.is_none(),
            "路径已经不在了就不能把覆写往不存在的文件上引 —— 走命名分支更诚实（第 63 项 ⑦ 三格判定 3）"
        );

        // 三格都过 —— reuse 开、prev 存在 → landed 抄到手，且第一次 quick_out 覆写它。
        let mut on = savable(PhysRect::new(10, 20, 100, 80));
        on.reuse_last_file = true;
        on.naming = crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "reuse".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        on.seed_landed_from(Some(prev.clone()));
        assert_eq!(
            on.landed.as_deref(),
            Some(prev.as_path()),
            "三格都过了就该抄到手，本会话第一次 quick_out 前 landed 就已经带着那一份"
        );
        let bytes_before = std::fs::read(&prev).expect("read prev");
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(on.quick_out(), "{:?}", on.message);
        assert!(
            !dir.join("reuse-002.png").exists(),
            "开着且 prev 抄到手时，第一次 quick_out 就该覆写那一份而不是新起一枚"
        );
        let bytes_after = std::fs::read(&prev).expect("re-read prev");
        assert_ne!(
            bytes_before, bytes_after,
            "覆写要把那一份真的换掉，不是留个同名空壳"
        );
        assert_eq!(on.quick_saves, 1, "跨会话那半仍然算一次快速保存");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// §5.3.5 —— `show_dimensions` 从 config 到 QML 的那一条路径上的两格：
    /// 一、`MaskState::show_dimensions` 从 `crate::settings::show_dimensions()` 取一次；
    /// 二、`MaskViewData::show_dimensions` 由 `view_data()` 从 `MaskState` 抄过来。
    ///
    /// 出厂值（`Capture::default()` 里 `show_dimensions = true`）不落在 `MaskState::default()` 上 ——
    /// `#[derive(Default)]` 给 bool 的是 `false`，这是给单测留的那一格，同 [`MaskState::auto_copy`]
    /// 那一格"单测里 false、真机上从 config 里取"的规矩；这里只测两格联动，不改那条约定。
    #[test]
    fn show_dimensions_reaches_the_view_only_by_the_path_open_walks() {
        // 一：`MaskState::default()` 的这一枚必须是 `false`（derive），出厂 config 是 `true`。
        //   两者不一致是 §10 第 62 项 ⑤ 里 `auto_copy` 那一族第一次落地时立的口径。
        assert!(
            !MaskState::default().show_dimensions,
            "derive(Default) 给 bool 的是 false，而 config 的默认是 true —— 两者必须分开"
        );

        // 二：view_data 的字段，是从 MaskState 抄过来的而不是自己算的。
        let bounds = PhysRect::new(0, 0, 640, 480);
        let mut m = state_with(vec![slot(0, bounds, 1.0)], PhysRect::new(10, 20, 100, 80));
        assert!(!m.view_data(0).show_dimensions, "state 关着 ⇒ view 也关着");
        m.show_dimensions = true;
        assert!(
            m.view_data(0).show_dimensions,
            "state 开着 ⇒ view 才开着：view 上的这一格除了从 state 抄没有第二条来路"
        );

        // 三：`view_data` 的 index 越界那一路（返回 default）也不该被这枚位污染 —— 它是
        // 一张**根本没开**的窗的证据，而不是"这一格关着"的证据。
        m.show_dimensions = true;
        assert!(
            !m.view_data(99).show_dimensions,
            "越界槽位的 view_data 走 default，不该跟 state 的这一枚同值"
        );
    }

    /// §5.5.4 step 4 的那一次点击，问三种**在开口之前就该被拦下**的状态：什么都还没
    /// 落盘、落过盘但那个文件不在了、以及默认设置下一次也没有自动揭示。
    ///
    /// 三种都不调用 [`platform_windows::reveal::select`]，所以这一支不会在用户桌面上
    /// 开出一个本进程关不掉的窗——会开的那一种在门里是一格发出来的 Blocked，而不是
    /// 一格被删掉。计数在动手之前加，所以"点了两次、一次也没到 shell"这句话问得出来。
    #[test]
    fn a_click_is_counted_before_it_acts_and_refuses_before_it_opens() {
        // 一、还没有任何东西落盘。
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        assert!(m.landed.is_none(), "一张新遮罩不该带着上一条图的路径");
        assert!(
            !MaskState::default().reveal_after_save,
            "默认的开关就该是关着"
        );
        assert!(!m.reveal_landed(), "没有路径可点却答了 true");
        assert_eq!(m.reveal_clicks, 1, "这一下没记上");
        assert_eq!(m.reveal_acted, 0, "一次没有路径的点击递到了 shell 那里");
        assert!(
            m.reveal_last.contains("还没有落到任何地方"),
            "{:?}",
            m.reveal_last
        );

        // 二、落过一次盘：通知手里那个 `PathBuf`，和门从那句话里抠出来的路径，是同一个人。
        let dir = sandbox("reveal");
        m.naming = crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "quick".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        assert!(m.quick_out(), "{:?}", m.message);
        let landed = m.landed.clone().expect("落盘之后通知该拿着路径");
        assert_eq!(landed, dir.join("quick-001.png"), "{:?}", m.message);
        assert_eq!(
            crate::annotate_check::landed_path(&m.message).as_ref(),
            Some(&landed),
            "通知手里的路径与那句话里的路径不是同一个"
        );
        assert_eq!(m.reveal_auto, 0, "开关关着，落盘自己去了 shell 一次");
        let said_save = m.message.clone();

        // 三、把那个文件删掉再点：拒绝发生在 shell 之前，点名的是那个已经不在的文件。
        std::fs::remove_file(&landed).expect("remove");
        assert!(!m.reveal_landed(), "删掉的文件还能开窗");
        assert_eq!(m.reveal_clicks, 2, "两下点成了一下");
        assert_eq!(m.reveal_acted, 0, "一次盘上没有的揭示递到了 shell");
        assert!(
            m.reveal_last.contains("不在盘上")
                && m.reveal_last.contains(&landed.display().to_string()),
            "{:?}",
            m.reveal_last
        );
        assert_eq!(m.message, said_save, "揭示的那句话把保存的那句证据挤掉了");
    }

    /// §5.4.3 的算术本体（[`MaskState::stamp_cursor`]）：不开窗、不读屏、不要真光标。
    ///
    /// 分成这几臂是因为它们在字节上各是一种错法：贴错原点、把差值当成绝对坐标、
    /// 手写第二遍边界、把覆盖层的透明外圈当成不透明。每一臂只破其中一种。
    #[test]
    fn the_cursor_is_placed_by_the_selection_not_the_desktop() {
        let base = [10u8, 20, 30, 255];
        let red = [255u8, 0, 0, 255];
        let img = Frame::filled(8, 6, red).expect("cursor");
        let untouched = Frame::filled(100, 80, base).expect("raw");
        let hole = PhysRect::new(768, 480, 100, 80);

        // 一、`at` 是**屏幕**坐标，目的坐标是 `at - hole.origin`；右下角那一枚也要贴上。
        let out = MaskState::stamp_cursor(
            Frame::filled(100, 80, base).expect("raw"),
            &img,
            hole,
            PhysPoint::new(788, 510),
            1.0,
        );
        assert_eq!(out.width, 100, "贴光标把画面贴大了");
        assert_eq!(out.height, 80, "贴光标把画面贴大了");
        assert_eq!(out.get(20, 30), red, "光标没有落在 at - hole.origin");
        assert_eq!(out.get(27, 35), red, "覆盖层右下角那一枚没贴上");
        assert_eq!(out.get(19, 30), base, "落在自己范围之外还改了底色");
        assert_eq!(out.get(28, 36), base, "落在自己范围之外还改了底色");

        // 二、副屏在主屏**左边**时选区原点可以是负的：要的是差值，不是绝对坐标。
        let left = PhysRect::new(-1920, 0, 100, 80);
        let neg = MaskState::stamp_cursor(
            Frame::filled(100, 80, base).expect("raw"),
            &img,
            left,
            PhysPoint::new(-1917, 4),
            1.0,
        );
        assert_eq!(neg.get(3, 4), red, "选区原点为负时把绝对坐标当成了目的坐标");
        assert_eq!(neg.get(2, 3), base, "选区原点为负时改了光标之外的底色");

        // 三、`opacity` 由调用方交进来：0.0 必须是恒等，而不是"半透明的红"。
        let zero = MaskState::stamp_cursor(
            Frame::filled(100, 80, base).expect("raw"),
            &img,
            hole,
            PhysPoint::new(788, 510),
            0.0,
        );
        assert_eq!(zero.pixels, untouched.pixels, "opacity 0 不是恒等");

        // 四、越界交给 [`falcon_core::frame::Frame::blend_over`] 自己裁：整枚在外一格不改，
        // 半枚在外则 inside 那半贴上、outside 那半不留痕。
        let half = MaskState::stamp_cursor(
            Frame::filled(100, 80, base).expect("raw"),
            &img,
            hole,
            PhysPoint::new(864, 557),
            1.0,
        );
        assert_eq!(half.get(99, 79), red, "被裁掉的那半不是丢掉，而是没贴");
        assert_eq!(half.get(95, 76), base, "半枚在外时改了覆盖层之外的底色");
        let beyond = MaskState::stamp_cursor(
            Frame::filled(100, 80, base).expect("raw"),
            &img,
            hole,
            PhysPoint::new(968, 480),
            1.0,
        );
        assert_eq!(
            beyond.pixels, untouched.pixels,
            "整枚在选区之外的光标在选区里留了痕"
        );

        // 五、真光标的外圈是透明的：alpha 0 必须跳过，否则一枚箭头会带出一个不透明的方块。
        let ring = Frame::new(8, 6).expect("transparent");
        let holed = MaskState::stamp_cursor(
            Frame::filled(100, 80, base).expect("raw"),
            &ring,
            hole,
            PhysPoint::new(788, 510),
            1.0,
        );
        assert_eq!(
            holed.pixels, untouched.pixels,
            "光标图的透明外圈被当成了不透明"
        );
    }

    /// §5.4.3 接进那唯一一道缝合（[`MaskState::delivered`]）的那一格：`None` 那一侧
    /// 必须**还是本轮之前那批字节** —— 门上的判据、P58/P59/P93 接过的每一格都靠这一条
    /// 不破。这一臂只能在这里成立，因为门（`--autosave`）默认把键关着，它跑到的永远是
    /// `None`，观察不到"开了会怎样"。
    #[test]
    fn delivered_hands_the_cursor_to_every_door_and_to_none_of_them_when_closed() {
        let red = [255u8, 0, 0, 255];
        let mut m = savable(PhysRect::new(100, 60, 100, 80));

        let plain = m.delivered().expect("no cursor");
        assert_eq!(
            plain.get(20, 30),
            [0, 0, 0, 0],
            "键没开的时候缝合就已经画了光标"
        );

        m.cursor = Some((
            Frame::filled(8, 6, red).expect("cursor"),
            PhysPoint::new(120, 90),
        ));
        let stamped = m.delivered().expect("cursor");
        assert_eq!(stamped.width, plain.width, "交出去的宽度跟着光标变了");
        assert_eq!(stamped.height, plain.height, "交出去的高度跟着光标变了");
        assert_eq!(stamped.get(20, 30), red, "缝合那一格没有把光标交出去");
        assert_eq!(
            stamped.get(19, 30),
            plain.get(19, 30),
            "光标那一格改了光标之外的像素"
        );

        // 六、`None` 是同一位读者、同一批字节：关门那侧与本轮之前不可分辨。
        m.cursor = None;
        let again = m.delivered().expect("cursor off");
        assert_eq!(
            again.pixels, plain.pixels,
            "把光标收回去之后缝合不再是同一批字节"
        );
    }
}
