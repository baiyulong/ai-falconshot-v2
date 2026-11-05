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
    anchor_of_corner, anchor_of_drag, constrain_aspect, handle_at, nudge, nudge_edge, nudge_sample,
    parse_ratio, resize, set_size_within, Anchor, Handle, PhysPoint, PhysRect, Scale, Unit,
    DEFAULT_HIT_SLOP,
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

/// §5.3.8 第一步与第二步的清单：`(芯片标签, 可被 `parse_ratio` 解的比例串, 悬停句)`。
///
/// 一份清单只长一处，与 `annotate::TOOL_TEXTS` 同一条口径 —— 标签、比例与句子必须同枚同序，
/// 而把它们抄进 QML 之后，第 `i` 枚钮说的就不再是第 `i` 个比例。
///
/// 第 0 枚是「无」且比例串为空：§5.3.8 的规则只说"用户应可以临时解除比例约束"，而把解除做成
/// 一格能按回去的芯片，比只有一条"按住 Alt"的手势多一条**看得见**的路。`ratio_index` 落在这一枚
/// 时锁是彻底关着的（不是 1:1 那种"看着像没锁"）。
pub const RATIO_CELLS: [(&str, &str, &str); 4] = [
    ("无", "", "无：选区不锁宽高比，拖到哪算哪"),
    ("1:1", "1:1", "1:1：拖角点时保持正方形"),
    ("4:3", "4:3", "4:3：拖角点时保持 4 比 3"),
    ("16:9", "16:9", "16:9：拖角点时保持 16 比 9"),
];

/// §5.3.8 第二句规则「用户应可以临时解除比例约束」里**那句话**的唯一主人。
///
/// 这一枚常量的理由不是"文案要集中"，而是漂移：按 Alt 这件事写在 [`MaskState::drag`] 的
/// `unlocked` 参数上，而用户能看见它的唯一一处是芯片的悬停句。句子抄在 QML 里，QML 改了而
/// Rust 没改（或反过来）时，屏幕上教的手势就不是手指真正要做的那个 —— 而没有任何一格会因此变红。
/// 于是句子从这里拼过去，且只拼给**真的锁着**的那几枚（见 [`ratio_hints`]）。
pub const RATIO_RELEASE_HINT: &str = "按住 Alt 可临时解除";

/// 标签，按 [`RATIO_CELLS`] 序、`|` 分隔 —— 与 [`ratio_hints`] 同枚数同序。
pub fn ratio_names() -> String {
    RATIO_CELLS
        .iter()
        .map(|(name, _, _)| *name)
        .collect::<Vec<_>>()
        .join("|")
}

/// 同一排的悬停句，按 [`RATIO_CELLS`] 序、`|` 分隔（§5.21.2 第一枚的又一处落点）。
///
/// 每一句后面跟着 [`RATIO_RELEASE_HINT`] —— 但只跟在那一枚比例串**真的解得开**的芯片后面。
/// 「无」那一格没有可解除的东西，教它一句"按住 Alt"就是把一条不存在的手势写到屏幕上。
/// 判据因此不是芯片标签、也不是 index，而是 `parse_ratio(spec)`：与 [`MaskState::ratio_lock`]
/// 问的是同一枚函数，句子和锁没法各说各话。
pub fn ratio_hints() -> String {
    RATIO_CELLS
        .iter()
        .map(|(_, spec, hint)| {
            if parse_ratio(spec).is_some() {
                format!("{hint}，{RATIO_RELEASE_HINT}")
            } else {
                (*hint).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// 锚点在桥上的序号：与 [`RATIO_CELLS`] 同一条口径，**名字与序号都住在 Rust**，QML 只把第
/// `i` 个名字贴在第 `i` 个序号上。抄一份到 QML 之后，"屏幕上写着左上角而 Rust 用的那枚是
/// BottomRight"就成了第二条可以各自漂移的路，而没有一格会因此变红。
pub const SIZE_ANCHOR_NAMES: [&str; 5] = ["左上角", "右上角", "左下角", "右下角", "中心"];

/// [`Anchor`] → 序号。手写五支而不是 `as i32`：枚举的**声明序**不是契约，而这一枚序号是
/// [`SIZE_ANCHOR_NAMES`] 唯一的下标来历。`the_anchor_code_table_covers_every_anchor` 那一格
/// 钉的就是这两者不能各长各的。
pub fn anchor_code(anchor: Anchor) -> i32 {
    match anchor {
        Anchor::TopLeft => 0,
        Anchor::TopRight => 1,
        Anchor::BottomLeft => 2,
        Anchor::BottomRight => 3,
        Anchor::Center => 4,
    }
}

/// §5.3.6 第一条规则「应明确选区从哪个方向或锚点扩展」里那枚锚点的名字表，`|` 分隔、按
/// [`anchor_code`] 序 —— 与 [`ratio_names`] 同一形状。
pub fn size_anchor_names() -> String {
    SIZE_ANCHOR_NAMES.join("|")
}

/// §5.3.7 第二步的那两格单位，**清单只长这一处**：桥上的序号、页上的字样、以及"点一下切到
/// 哪一格"全都从这一枚数组读出来。把 `["px","dip"]` 抄进 QML 之后，第 `i` 枚字样说的就不再
/// 是第 `i` 个单位 —— 而单位这件事说错的代价是数字本身：屏幕上写着 683 而裁下去是 1024，
/// 用户看到的是一句换算错的谎，不是一格样式不对的钮。
///
/// P164 起这枚数组是 [`Unit::ALL`] 的别名而不是第二份清单：设置页那一格（`[capture] size_unit`）
/// 的闸也要数这两枚单位，两处各抄一遍就会有一处漂 —— 芯片的序、字样的序、页上收下的拼法自此
/// 是同一枚序。
///
/// 顺序即序号，[`unit_code`] 手写两支而不是 `as i32`：同 [`anchor_code`] 那条理由，枚举的
/// **声明序**不是契约。
pub const SIZE_UNITS: [Unit; 2] = Unit::ALL;

/// [`Unit`] → 序号（[`SIZE_UNITS`] 的下标）。
pub fn unit_code(unit: Unit) -> i32 {
    match unit {
        Unit::Px => 0,
        Unit::Dip => 1,
    }
}

/// 序号 → [`Unit`]。过界的序号不被当成"用户挑了 0 格"：`0` 是 px，而一次页面上的错序点击
/// 什么也没挑，所以它答出厂的那一格（[`Unit::default`] = px）。
pub fn unit_at_code(code: i32) -> Unit {
    usize::try_from(code)
        .ok()
        .and_then(|i| SIZE_UNITS.get(i).copied())
        .unwrap_or_default()
}

/// 单位字样表，`|` 分隔、按 [`unit_code`] 序 —— 与 [`size_anchor_names`] 同一形状：
/// **序号过桥、句子留在 Rust**（句子本身是 [`Unit::label`]，所以这张表没有第二个主人）。
pub fn unit_names() -> String {
    SIZE_UNITS
        .iter()
        .map(|u| u.label())
        .collect::<Vec<_>>()
        .join("|")
}

/// 那两枚芯片各自的悬停句，序与 [`SIZE_UNITS`] 一一对上（`the_unit_tables_cover_every_unit_and_agree_on_their_own_length`
/// 钉住这两枚不许各长各的 —— 页拿第 `i` 句配第 `i` 枚字样，错一格就是一句贴在错单位上的话）。
///
/// 两句都说的是**这一格会改什么东西**，而不是把单位名再念一遍：点下去变的不只是读数末尾那两个
/// 字母，还有"打字进去的那对数被怎么解释"。这一句不说清楚，用户就会以为切了单位只是换个写法。
pub const UNIT_HINTS: [&str; 2] = [
    "按实际像素说：这对数就是结果详情里那句「… 像素」的那对数",
    "按设备无关像素说：这对数是按当前显示器缩放除过的，缩放不是 100% 时它与实际像素不成一比一",
];

/// 悬停句表，`|` 分隔 —— 与 [`unit_names`] 同一形状。
pub fn unit_hints() -> String {
    UNIT_HINTS.join("|")
}

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

/// `[advanced] recognize_text` 那枚键的分岔本体：新记成的这一行值不值得为它跑一次引擎。
///
/// 两个条件各有对面。键关着 ⇒ 一次也不该跑，而出厂默认就是关着（`config.rs` 的
/// `Advanced::default`），所以「默认什么都不做」这一条自此有可测的形状而不是话术。
/// 行是**取消**的那一行 ⇒ 也不该跑：§5.14.1 要「包括被取消的截图」进的是那一页清单，
/// 而一张用户已经 Esc 掉的图没有第二位读者会去要它的文字，跑一次只是白占几百毫秒 CPU。
///
/// 纯函数，与上面 [`esc_owes_row`] 同一条理由：这一格判的是「要不要花一次引擎」，
/// `cargo test` 问它四种形状既不用冻结桌面，也不用本机 OCR 引擎在场。
pub fn auto_ocr_wanted(recognize_text: bool, cancelled: bool) -> bool {
    recognize_text && !cancelled
}

/// 引擎那句话翻成 SQLite 那一格的值，两件事必须保持两个形状：`None` 是**没跑成**（引擎缺、
/// 帧读不动、这一行本就不该跑），那一格留着 NULL；`Some(_)` 是跑成了。
///
/// 跑成之后还折一次：一帧全是空白（识别回来的只有空格与换行）与识别出字是两件事 —— 那一格
/// 若写进一串空白，界面读回来会说「这一行有文字」而用户看见的是空的。故空白折成空串，
/// 非空原样交回（**不** trim 掉行内换行，§5.6.1「结果应尽量保留自然段和换行」在那儿钉着）。
pub fn auto_ocr_slot(outcome: Option<String>) -> Option<String> {
    outcome.map(|text| {
        if text.trim().is_empty() {
            String::new()
        } else {
            text
        }
    })
}

/// worker 落笔那一步，与 `session.rs` 里那两条 `*_landed` 回调同一形：先过
/// [`auto_ocr_slot`]，再交给 [`falcon_core::history::History::set_ocr`]。抽成一枚函数是为了
/// 让「字到底进没进那一格」这一问有一位**不必真跑引擎**的证人：单测把 `Some("…")` 直接递
/// 进来，指着的就是 SQLite 那一格；线程上跑的是同一只手，不是它的副本。
///
/// 写失败（行不在、库被占）只把 `Err` 交回而不响：调用方丢弃它，因为 §5.14.1 那条
/// 「历史记录失败不许吃掉截图」已经由 [`MaskState::record_history`] 定了口径。
pub fn write_auto_ocr(
    history: &falcon_core::history::History,
    id: i64,
    outcome: Option<String>,
) -> Result<(), falcon_core::history::HistoryError> {
    match auto_ocr_slot(outcome) {
        Some(text) => history.set_ocr(id, Some(text.as_str())),
        None => Ok(()),
    }
}

/// 把「跑引擎 + 落那一格」整条腿搬到一枚命名线程上。理由是 P146 给 OCR 立的那一条：本机引擎
/// 是阻塞的（`RecognizeAsync` 由 5 ms 一轮的轮询取回），让 GUI 线程等它，僵住的是桌上每一扇窗。
///
/// 线程名字独立（`falconshot-ocr-auto`）而不是复用 `falconshot-ocr`：那一枚是用户点「文字提取」
/// 起的，这一枚是 `[advanced]` 那枚键起的，两趟挤在一枚线程上就分不出是谁跑的。
///
/// SQLite 的连接**在这条线程上开**：把 GUI 线程那枚 `History` 递过线程边界，等于让两个线程握
/// 同一枚 `rusqlite::Connection`（它 `!Send`，编译也过不去）⇒ 重开一次，而不是共享。
/// 起不来（线程数耗尽）也不响：那一行留着 NULL，与「没跑成」同一格。
fn spawn_auto_ocr(
    frame: falcon_core::frame::Frame,
    dir: std::path::PathBuf,
    cfg: falcon_core::config::History,
    id: i64,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("falconshot-ocr-auto".to_string())
        .spawn(move || {
            let outcome = platform_windows::ocr::extract(&frame).ok();
            let Ok(Some(history)) = crate::settings::history_open_in(&dir, &cfg) else {
                return;
            };
            let _ = write_auto_ocr(&history, id, outcome);
        })
        .ok()
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
    /// §5.4.1：这一扇窗**此刻**发给 Qt 的那块放大纹理的键，空串 = 场上还没这一窗的放大镜。
    /// 每一窗一份是因为放大镜只有一个 —— 光标在哪一扇窗就只哪一扇窗的 `MouseArea` 会收到
    /// hover —— 但**纹理生命周期**仍按窗分开记：一次 hover 换键时只 drop 掉本窗上一张，
    /// 别窗即使残留一张也藏在那一窗的 `containsMouse = false` 之后，屏幕上看不到。全局一枚
    /// 计数器 [`MaskState::mag_seq`] 只保证 URL 每次变、Qt 才肯重取；键的**归属**在这一格。
    pub mag_key: String,
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
    /// The 字号 the open box is drawn in, in DIP, by the same division. Not the pen's own
    /// number while a retype (§5.7.16 步 3's 文字内容) is in hand: the field sits inside a
    /// box measured with the object's style, and letters at anybody else's size overflow it.
    ///
    /// 这一格与页上 `text_seed` 那一格读的是同一只框，但走的是两条腿：`MaskViewData` 是
    /// `Copy` 的一份几何样本，装不动一串字，所以那句由 [`crate::mask_view`] 在同一次锁里
    /// 直接向 [`crate::annotate::Layer::typing_text`] 问 —— 同一把锁就是同一个时刻，与
    /// `pen_family` 那一格同一待遇。
    pub font_dip: u32,
    /// §5.7.3 step 4's 圆角 handle on this window, in the same window-local DIP as
    /// [`Self::typing`]: the point itself is core's answer ([`crate::annotate::Layer::radius_handle`]
    /// asks [`falcon_core::annotation::grip_point`]) and this layer only moves it into the
    /// window's own units, through `MaskState::local_dip` like the field above. `radius_live` is
    /// "there is one to draw"; the rect is 1×1 because that is the shape the divider takes,
    /// and QML reads `x`/`y` out of it and draws its own dot.
    pub radius: PhysRect,
    pub radius_live: bool,
    pub revision: u32,
    pub swaps: u32,
    pub first_swap_ms: Option<u64>,
    pub shader_status: Option<i32>,
    /// §5.3.5 —— 这一格是"这一张遮罩要不要显示选区尺寸"，从 [`MaskState::show_dimensions`] 抄过来。
    /// 单独放在这里而不复用 [`MaskState::show_dimensions`]，是因为 QML 只拿 `MaskViewData` 而不
    /// 直接读 `MaskState`；跨这一层加一枚 bool 而不是新增一次 lock，成本与 [`Self::shown`] 同量级。
    pub show_dimensions: bool,
    /// §5.3.8 —— 哪一格比例芯片亮着（抄自 [`MaskState::ratio_index`]，`-1` = 自定义那一格）。
    /// 走 [`MaskViewData`] 而不是让 QML 直读 `MaskState`，与 [`Self::show_dimensions`] 同一条
    /// 理由；多带这一枚而不是让 QML 自己记住按了哪格，是因为 `--mask` 要问的就是"锁到底是
    /// 哪一格"，而一把只存在于 QML 记忆里的芯片没有证人。
    pub ratio_index: i32,
    /// 这一刻锁**真的**在不在（[`MaskState::ratio_lock`] 答 `Some` 才为 `true`）。自定义那一格
    /// 正打到一半（`"1"` 还没冒号）时 [`Self::ratio_index`] 仍是 -1 而这一枚是 `false`：芯片亮着
    /// 而画不出锁，QML 要把那一格点灰就只有这一枚读数可问。
    pub ratio_live: bool,
    /// §5.3.6 —— 打进来的那对宽高会从哪一角扩展，[`anchor_code`] 的序号（抄自
    /// [`MaskState::size_anchor`]）。名字表不在这里：与 [`Self::ratio_index`] 那一格同理，
    /// 序号过桥、句子留在 Rust（[`SIZE_ANCHOR_NAMES`]），QML 只做一次下标。
    ///
    /// 没有槽位时 [`MaskViewData::default`] 给 `0` = 左上角，与 [`MaskState::default`] 的
    /// 那一枚是同一个答案，所以"窗还没分到屏"与"用户还没碰过任何一角"在屏幕上读起来一样。
    pub size_anchor: i32,
    /// §5.3.7 —— 选区尺寸**按当前单位**的那对读数（[`MaskState::size_shown`] 的答案）。
    ///
    /// 不复用 [`Self::hole`] 的 w/h：那两格是"这一扇窗怎么画自己那块暗化与八枚把手"的
    /// 窗内 DIP 几何（[`MaskState::local_dip`] 还把跨屏的选区**裁**进这一扇窗），而这一对
    /// 是"你圈的这块有多大"的一句读数。一句问几何、一句问尺寸，把它们塞进同一对字段就是
    /// 让两个问题共用一个答案 —— P162 记过的那一族"绑过一次就再也更不动的数字"正是同一族。
    ///
    /// 出厂 `0`：还没有选区就没有尺寸可说，与 [`Self::hole`] 的默认一致。
    pub size_w: u32,
    pub size_h: u32,
    /// 这一刻用的单位，[`unit_code`] 的序号（名字表在 [`SIZE_UNITS`] / [`unit_names`]）。
    /// 页上那行字与那枚芯片都从这一格取下标，所以"显示的是 dip"与"打字进去的数按 dip 解释"
    /// 是同一枚状态说的同一件事，不会各说各的。
    pub size_unit: i32,
    /// §5.3.2 第三步「系统高亮窗口边界」那块框，**这一扇窗自己的**窗内 DIP。
    ///
    /// 与 [`Self::hole`] 完全对称：`hole` 是选区在这一扇窗上怎么看，`hover` 是光标此刻
    /// 最直接指向的那一扇**桌面上的窗**怎么看，两格都出自 [`MaskState::local_dip`] 那一位
    /// 作者，所以在 200% 屏上高亮框与选区不会差出一像素。不相交时为默认值（全 0）而
    /// [`Self::hover_live`] 是 `false` —— 画不画只看那一枚 bool，不看尺寸，因为 0×0 在
    /// QML 里本来就画不出东西，把它当"没亮"读是巧合不是判据。
    pub hover: PhysRect,
    /// 这一刻这一扇窗上有没有该亮的边界（[`MaskState::hover_bounds`] 答得出才 `true`）。
    pub hover_live: bool,
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

/// §5.19.3「放大镜外观」那四枚 `[appearance] magnifier_*` 键在桥里的读数。这四枚键此前的
/// 唯一读者是 [`crate::settings::format_appearance_banner`] 那一行启动横幅 —— 键在档里、
/// 横幅里念过、一个像素也没穿过；`magPanel` 画的是页上写死的四枚字面。这一枚结构就是那条
/// 一直没接上的通路本身：`CaptureMask.qml` 现在绑的每一个值都从它来。
///
/// 为什么嵌成一枚而不是四枚平铺进 [`MagnifierData`]：[`MaskState::magnifier_at`] 有四条早退
/// （指针不在槽位上 / 没有快照 / 配置关了放大镜 / core 没交出帧），每一条都得把这份外观**一起**
/// 带走 —— 面板的颜色与它的 `visible` 是同一趟交出去的，早退时留下四枚空串就等于让页上在
/// 「这一趟不画」那一刻绑到一枚读不出来的颜色。平铺要写四遍，嵌一枚只需一句。
///
/// 两枚颜色是**已经归一过**的串（`#RRGGBB`；alpha 不是 255 时 `#AARRGGBB`），页上直接
/// `color:` 绑它、不再自己解析第二台（#281，与 [`crate::settings::band_colors_joined`] 同一只
/// 口径）。那一枚 drop alpha 是因为它递下去的门把手是 `setColor(r, g, b)`；这一枚留着，因为
/// QML 本来就认 `#AARRGGBB`，丢掉就是丢掉用户写下的那两位。
#[derive(Clone, Debug, PartialEq)]
pub struct MagnifierLook {
    /// `magnifier_background` 归一后的那一枚：面板的底。
    pub background: String,
    /// `magnifier_foreground` 归一后的那一枚：坐标那一行、颜色那一行，**加上描边那一笔**。
    /// 档里没有第五枚颜色键 —— `magnifier_border` 在 core 是一枚 u32 宽度、上下限住在
    /// `Config::validate` 的 `clamp` 里 —— 而一块面板总要有笔画边，所以这一笔吃的是同一枚
    /// 前景色。桥不替它挑一枚新色。
    pub foreground: String,
    /// `magnifier_border`，只做 `u32 → i32` 的落地。上下限只有一个主人（上面那两行），
    /// 这里夹第二次就是给同一件事找第二个作者。
    pub border: i32,
    /// `magnifier_corner_radius`，同上一枚的规矩。
    pub radius: i32,
}

impl MagnifierLook {
    /// 档里那四枚键 → 面板画得出来的四个读数。**纯**：输入一份借用的 `Appearance`、输出四个
    /// 值，不碰档、不碰锁、不碰 Qt —— 所以这一族的分岔能在没有窗、没有事件循环的地方判。
    ///
    /// **读不出来的那一枚颜色归宿是出厂的那一枚，不是「页上自己挑一个」**：`Config::validate`
    /// 在载入时就用同一台 [`falcon_core::colors::parse`] 丢过坏串并回填默认，但运行期被写回
    /// 路径塞进来的坏串不过那道门，所以这一层是第二道门 —— 落回
    /// [`falcon_core::config::Appearance::default`] 里那两枚字面（由本函数的 `shipped` 参数交出，
    /// 默认值本身仍只有 core 一个作者，#281），两枚都读不出来时把原串递出去，让页上
    /// `!== ""` 那一臂兜住。发明一枚新色是这一族最不该发生的事。
    pub fn from_appearance(a: &falcon_core::config::Appearance) -> Self {
        let shipped = falcon_core::config::Appearance::default();
        Self {
            background: qml_colour(&a.magnifier_background, &shipped.magnifier_background),
            foreground: qml_colour(&a.magnifier_foreground, &shipped.magnifier_foreground),
            border: a.magnifier_border.min(i32::MAX as u32) as i32,
            radius: a.magnifier_corner_radius.min(i32::MAX as u32) as i32,
        }
    }
}

/// 一枚档里的颜色串 → QML 认的那一种写法。走的是档自己那台解析器（[`falcon_core::colors::parse`]，
/// 认 `#RGB` / `#RGBA` / `#RRGGBB` / `#RRGGBBAA` / `rgb()` / `hsl()` 六族），读不出来就退到
/// `shipped` 再走同一台一次 —— 见 [`MagnifierLook::from_appearance`] 头上那段归宿的口径。
fn qml_colour(text: &str, shipped: &str) -> String {
    match falcon_core::colors::parse(text).or_else(|| falcon_core::colors::parse(shipped)) {
        Some([r, g, b, 255]) => format!("#{r:02X}{g:02X}{b:02X}"),
        Some([r, g, b, a]) => format!("#{a:02X}{r:02X}{g:02X}{b:02X}"),
        // 出厂那两枚是 core 写死的字面量，读不出来只能是 core 自己的事故。这一臂不兜一枚
        // 新色，把原串原样递出去：页上那一臂（`!== ""`）本来就为「还没落地的第一帧」而活。
        None => shipped.to_string(),
    }
}

/// `Default` 不是四枚零值，而是**出厂那一份**走同一台映射：这样任何一手还没把档里的外观送进
/// 桥时（单测里手搭的 `MaskState`、第一趟 hover 之前），页上拿到的是出厂那块面板，而不是派生
/// `Default` 会给的空串 + 0 半径。出厂值本身只有一个作者：[`falcon_core::config::Appearance`]。
impl Default for MagnifierLook {
    fn default() -> Self {
        Self::from_appearance(&falcon_core::config::Appearance::default())
    }
}

/// §5.4.1 —— 放大镜这一块一次 hover 的全部读数，[`MaskState::magnifier_at`] 的答案。
///
/// 纯数据、`Default` 是"什么都不显示"（`show=false` + 空 key + 零尺寸 + 空文本），所以
/// 桥层任何一手走到"这一趟没生成"时可以直接回 `Default::default()`，页上的绑定就退化成
/// 一块不画的矩形 —— 不需要额外一根 bool 说"这次是干净的失败"。唯一的例外是 [`Self::look`]
/// 那一格：它的 `Default` 是**出厂的外观**而不是四枚空值，理由写在 [`MagnifierLook`] 头上。
///
/// 为什么字段这么薄：像素不进来，进来的是**已经存进 Qt 纹理仓库**的那张图的**键**（同
/// [`Slot::key`] 那一族的理由，§3.6 约束 8）。字符串也不进来 —— 坐标与颜色句子都在
/// core 里 [`falcon_core::capture::ScreenSnapshot::magnifier_view`] 一次装配好，桥只做搬运。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MagnifierData {
    /// 这一趟到底画不画。`false` = 配置关了放大镜 / 指针跑到快照外面 / 这一场还没快照。
    pub show: bool,
    /// 放大纹理的键，`""` = 场上没有本窗的放大镜（页上 `source` 应绑空串）。
    pub key: String,
    /// 放大纹理的**物理**像素尺寸（`w == h` 恒成立，源图是正方形），页上要除以 `dpr` 才
    /// 摆到窗内 DIP。
    pub w: i32,
    pub h: i32,
    /// 坐标那一行的原文，`""` = 配置里 `magnifier_coordinates` 关了。
    pub coord: String,
    /// 颜色那一行的原文，按配置的 `magnifier_color_format` 拼（HEX/RGB/HSL 三族都走
    /// [`falcon_core::colors::format`]）。`""` = 指针落在屏幕外，没有颜色可读。
    pub color: String,
    /// 颜色的 RGB 三字节打包（`0xRRGGBB`），给色板那一小块 `Rectangle.color` 用。
    pub rgb: i32,
    /// 上面那一格**有没有**颜色 —— 屏幕外时 `rgb` 是 0，那本身也是一个合法的 RGB，
    /// 所以"这一格该不该画色板"要单独一格 bool 说清楚。
    pub has_color: bool,
    /// §5.4.1 第 3 步的像素网格：overlay 上要不要按 `zoom` 划格子。配置里的
    /// `magnifier_grid` 直接抄过来。
    pub grid: bool,
    /// 每边**源**像素数（`radius * 2 + 1`），页上算格子间距用；`zoom` 是同一次装配里
    /// 那两枚夹过上下限的数字，桥不做第二次夹。
    pub pixel_w: i32,
    pub zoom: i32,
    /// §5.3.9 第 4 步 —— **这一扇窗**摆这一块面板该摆在哪一格（窗内 DIP）。
    ///
    /// 它是这一趟真正去采样的那一点（[`Self::cursor_phys`]）在本窗的读法，走的仍是
    /// [`Self::local_dip`] 那一位作者 —— 与选区、高亮框、文本框、圆角把手同一把尺子。
    ///
    /// 为什么它要由桥交出来、而不是页上自己记一份 `mouse.x`：那两个答案早晚会漂开。鼠标那条
    /// 腿下它们本来就相等（同一个点，两次读法），而键盘微调那一趟屏幕上**没有**新的鼠标事件 ——
    /// 页上那份就会留在原地，于是「面板摆在指针那儿、里面画的是另一条边的像素」。让面板跟着
    /// 被采样的那一点走，屏幕上那一块面板说的才是它自己里面那件事。
    ///
    /// `show=false` 的几条早退同样带着它（与 [`Self::look`] 同一条口径：不画 ≠ 没有值），所以
    /// 面板永远不会因为「这一趟没像素」而跳到 `(0, 0)` 那一角。
    pub anchor_x: f64,
    pub anchor_y: f64,
    /// §5.19.3 那四枚 `[appearance] magnifier_*` 键的读数：面板的底、字与描边的一笔、
    /// 边框宽度、圆角。`show=false` 的那四条早退也带着同一份，理由在 [`MagnifierLook`] 头上。
    pub look: MagnifierLook,
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
    /// §5.4.4 / §5.4.5：这一次流程背后的板子。`None` = 抓来的画面（截图那一路），
    /// `Some` = 做出来的画面（白板那一路）。它决定的是**遮罩怎么画这块板**：透明的板
    /// 要让人看穿，所以暗化必须关掉、窗底色不能是黑 —— 不记这一格，透明白板开出来的
    /// 就是一张黑屏，而 §5.4.5 第二步要的正好是反面。
    ///
    /// 它是 [`Self::open`] 之后由 [`open_board`] 点的一格，而且 [`Self::open`] 每趟先清：
    /// 上一张板的可见性不能跟着下一张活（与 `landed` / `quick_saves` 同一族）。
    pub board: Option<falcon_core::whiteboard::Surface>,
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
    /// §5.4.1 / §5.4.2：这一场遮罩**自己留一份**冻结快照，放大镜与取色器都读它。
    ///
    /// 为什么不是每次 hover 现问 [`crate::capture::current`]：那一手回的是整段快照的
    /// `clone`，鼠标每动一次就复制一屏像素，是这一路负担不起的；而 [`crate::capture`] 的
    /// 全局槽本就不是为"每一帧都问一次"设计的。这一格只在 [`Self::open`] 取一次（一次按
    /// 下热键一次拷贝，与 `store_raw` 铺纹理同一量级），之后 [`Self::magnifier_at`] /
    /// [`Self::pick_color`] 就地读，颜色与放大像素始终来自**同一帧、同一份遮罩前就已冻住**
    /// 的画面 —— 这正是 §5.4.2「颜色值应基于原始屏幕像素，不应受截图遮罩影响」要求的形状。
    /// `None` = 这一场没开过（单测里手搭的 `MaskState` 就是这一格为空）。
    pub snapshot: Option<ScreenSnapshot>,
    /// 放大镜**此刻**在采的那一颗，桌面物理点：由 [`Self::magnifier_at_point`] 记下，而它是
    /// 两条腿共用的那一手 —— 鼠标那三处 hover/drag/release 经 [`Self::magnifier_at`] 进来，
    /// §5.3.9 第 4 步键盘那一趟经 [`Self::nudge_magnified`] 进来（那里先由
    /// [`falcon_core::geometry::nudge_sample`] 算出"刚被挪掉的那条边"再递给它）。取色器（按
    /// `C`）读它而不是让 QML 再报一遍坐标：放大镜已经在跟着这枚点走，两件事共用同一个
    /// "此刻在看哪一颗"，免得两本账漂开。`None` = 这一场指针还没在遮罩上动过、也没键过方向键。
    pub cursor_phys: Option<PhysPoint>,
    /// 放大纹理的**代数**，只增不减：URL 变了 Qt 才会重新取（同 §3.6 约束 8、与
    /// `overlay_key` 每画一笔就换键同一族），而键里不带这枚数，回到同一个像素点上一次就
    /// 不会重取。上一次发给 Qt 的键记在**这一扇窗自己的** [`Slot::mag_key`] 上（下一次 hover
    /// 先按窗丢掉自己那张，鼠标走一万下也不会攒一万张 136×136 的图），`open` 每场把这枚代数
    /// 与新建槽位里那三格一起从 0 起。
    pub mag_seq: u32,
    /// §5.3.2 —— 这一场遮罩的**候选窗口表**，`open` 时取一次。
    ///
    /// 为什么不是每次 hover 现问 [`crate::capture::window_at`]：那一手每问一次就把桌面重新
    /// 枚举一遍（外加每扇窗一次 DWM 属性、一次进程名查询），而 hover 是鼠标每动一次问一句 ——
    /// 与 [`Self::snapshot`] 头上记的是同一族教训、同一族做法：一次按下热键一次拷贝，之后
    /// 就地读。缓存的另一半好处是**这一场的高亮不会漂**：桌面上的窗在遮罩开着的时候是会同
    /// 在关的，而底图早在 `open` 之前就冻住了，跟着冻住的那一张画面走才是诚实的。
    ///
    /// 空表是合法状态而不是失败（枚举失败 [`crate::capture::window_candidates`] 就交空表）：
    /// §5.3.2 整节是一条可选的快捷路径，量不到候选时自由选区那一腿必须照常能走。
    pub windows: Vec<falcon_core::capture::WindowInfo>,
    /// §5.3.2 第三、四步的读数：光标此刻最直接指向的那一扇窗，`hwnd` 与它**可截图**的那块
    /// 边界（桌面物理像素）。由 [`Self::hover_window`] 写，`open` / [`Self::close`] /
    /// [`Self::escape`] 清。
    ///
    /// 存的是**桌面物理矩形**而不是窗内 DIP：这一格说的「那一扇窗在屏幕上占哪一块」是一
    /// 份全局几何，而窗内 DIP 是每一扇遮罩窗自己的画法 —— 画法在 [`Self::local_dip`] 那一
    /// 一位作者手里算，这里再存一份就是第二个答案（跨屏的窗在两扇窗上是两块不同的矩形，
    /// 更不可能用一格存下）。
    ///
    /// `None` = 光标此刻不在任何一扇**可截**的窗上（桌面、全透的窗、或候选表是空的）。
    pub hover_win: Option<(u32, PhysRect)>,
    /// 这一场用户**已经按下去过一次**了 —— 于是 §5.3.2 第三步的高亮不再报。
    ///
    /// 为什么需要这一格，而不是让 QML 用「有没有选区」来判：[`Self::open`] 是带着一枚
    /// 默认选区（[`Self::default_hole`]，主屏中间那半块）开门的，`selected` 从第一场按下
    /// 热键起就是 `true`，拿它当闸门就等于把这一路整个关掉。这一格说的才是真正那件事：
    /// 「用户还在挑吗」—— 挑的阶段亮，已经开始画/已经选中了就不亮，而 Esc 第一档
    /// ([`Self::escape`]) 把选区清掉之后又回到「还在挑」，高亮跟着回来。
    ///
    /// 写在 [`Self::press`] 而不是写在页上的 `pressed` 上：按下这一格**同时**是
    /// [`Self::release`] 那一路整窗选中的起点，两件事得有同一个主人。
    pub hover_taken: bool,
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
    /// §5.3.8 的比例锁，**第几格**：`0..[`RATIO_CELLS`]` 是表里那四格（0 = 「无」），`-1` 是
    /// "自定义那一格"，其内容在 [`Self::ratio_spec`]。
    ///
    /// 不直接存那枚 `f64` 而存"第几格 + 那句话"，是因为芯片亮哪格与锁是什么不能有两个主人：
    /// 存 `f64` 就多出一格可以与 index 漂移的状态，而那正是"按钮上写着 1:1、拖出来是 2.0"
    /// 这种画。比例由 [`Self::ratio_lock`] 现场从这两格算。
    ///
    /// `#[derive(Default)]` 给 i32 的 0 就是「无」，所以 `MaskState::default()` 与已有的每一张
    /// 单测在没有挑过芯片之前锁是关着的 —— 与 P161 之前的字节零改变是同一条。
    pub ratio_index: i32,
    /// 自定义那一格的原文（`"2.35:1"` 这样）。选表里那四格时被清空，所以"此刻的比例"永远只有
    /// 一条来历：index 为负看这句话，index 非负看 [`RATIO_CELLS`]。
    pub ratio_spec: String,
    /// §5.3.6 第一条规则：打进来的那对宽高从**哪一角**扩展。
    ///
    /// 它是"上一次真正定住过一枚角的动作"的答案：拖出新选区时是指针落点那一角
    /// （[`anchor_of_drag`]），按住角点调整时是那枚角点的**对角**（[`anchor_of_corner`]）。
    /// 拖边（N/S/E/W）与整块移动**不改**这一枚，因为它们没有回答"哪一角"这个问题：一条边的
    /// 对面那条边上，两枚角都站着不动，选哪一枚都是编的。
    ///
    /// 存这一格而不是到打尺寸那一刻现算，是因为 `grab` 在松手时就没了（[`Self::release`]），
    /// 而"我刚刚按住的是哪一角"正是松手之后要打字那一刻才用的信息。出厂 [`Anchor::TopLeft`]
    /// （`#[derive(Default)]` 给的那一枚）：一张还没被碰过的选区没有别的来历。
    pub size_anchor: Anchor,
    /// §5.3.7 第二步的那枚开关：**屏幕上那对数字按什么单位说，打字进去的那对数字就按什么
    /// 单位解释**。两个问题共用这一枚状态，因为分开就是两只主人：读数说 dip 而输入说 px 的
    /// 那一格，是本轮在这一路**现场量到的既有不一致**（`hole_w`/`hole_h` 走
    /// [`MaskState::local_dip`] 除过 `slot.scale`，而 [`Self::type_size`] 把递来的数当作物理
    /// 像素直接喂给 `set_size_within`）。
    ///
    /// 出厂 [`Unit::default`] = px（`#[derive(Default)]` 给 `#[default] Px` 的那一枚），所以
    /// 这格落地的那一刻起，**已存在每一次按下的答案字节零改变**：px 那两支换算都是恒等
    /// （[`Unit::show`] / [`Unit::to_phys`] 的 `Unit::Px` 分支原样返回），P162 钉下的每一张
    /// `type_size` 单测与 `--mask` 门里那 13 行都不必跟着改口。
    ///
    /// 它**不再**随会话归零（P164）：[`Self::open`] 在取整段 `[capture]` 快照的那一刻把
    /// [`Self::capture`]`.size_unit` 交给它，所以这一场的起点是**用户在截图设置里挑过的那一格**，
    /// 而页上那枚芯片当场切的只是这一场 —— 切完不写档，下一场仍回到档里那一格。这与
    /// [`Self::size_anchor`]、[`Self::ratio_lock`] 分家是有理由的：那两枚说的是"这一张怎么画"，
    /// 留着上一张的决定会画出用户没挑过的形状；这一枚说的"这一张按什么单位看"在 PRD §5.3.7
    /// 第一步里被明写成一格**设置**，设置就该跨过按下的那一次活下来。
    pub unit: Unit,
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
    /// §5.5.10 步骤 2 那一枚芯片被**点**了几次：一次点击恰好一次，与 shell 收没收下无关。
    /// 计数在动手**之前**加，与 [`Self::reveal_clicks`] 同一条口径（第 46 项 ③）—— 一个失败了
    /// 却不计数的点击会把两次真点击读成一次。
    pub send_to_clicks: u32,
    /// shell **收下**了几次递图请求。`0` 不等于没点过：它是"这一程真往别人家的进程里交过
    /// 一次东西"的唯一证据，而 §5.5.10 第三步在这台机器上问得出的全部就是这一格。
    pub send_to_acted: u32,
    /// 落到盘上、递出去过几张（不是点了几次）：那枚临时文件的名字要按这一格数，撞了名也按
    /// `[output] collision` 那一条既有规则走。写失败的那一次不计数 —— 这数的含义是"盘上有几张"。
    pub send_to_writes: u32,
    /// 最近一次发送到应用的话，成功失败都写这里而**不**写 [`Self::message`]：那句话要带出
    /// 交出的是哪一枚程序、哪一枚文件、三臂里的哪一臂，而 `message` 那一行留着的是"已保存到 …"
    /// 那句证据（[`Self::reveal_last`] / [`Self::auto_copy_last`] 为同一条理由各占过一格）。
    pub send_to_last: String,
    /// 这一次递给目标的那枚临时文件。留着有两个用处：门要读回"我们递出去的那一枚确实在盘上，
    /// 且在**我们自己那一枚**目录下"；而下一次派发要先把上一枚收回（§6.2 第 1600-1601 行
    /// 「临时文件应按策略自动清理」在派发这一路上的那一次轮次）。
    pub send_to_landed: Option<std::path::PathBuf>,
    /// §5.7.21 第二步（把工具栏那一排色块拖着换顺序）的那一句话，改成了与被拒绝都写这里而**不**写
    /// [`Self::message`]：「排头那一枚没有上一格」与「已保存到某个路径」要在屏幕上同时在场，而
    /// [`Self::reveal_last`] / [`Self::send_to_last`] 为同一条理由各占过一格。被拒绝那一句尤其要
    /// 留得下来 —— 它是「拖了这一枚却没动」与「压根没拖」之间唯一的分别。
    pub band_last: String,
    /// 那一排色带被改过几次：页上任何一只手（拖、设置页那两只钮、应用方案）都算一次。
    ///
    /// 它只有一个用处：递给 QML 那枚 `band_said` 绑定。`bandColors()` 是一枚**函数**而不是属性，
    /// 绑定不会为一次函数调用重发，所以「档里那一排换了顺序」要走到屏幕上，就得有一枚真的会变的数。
    ///
    /// 与 [`Self::next_revision`] 同一族理由**不清零**：一枚会在两场流程之间回到同一个数的计数，
    /// 等于一次没有重读。
    pub band_revisions: u32,
    /// Bumped by every `open`, so a second capture in the same process gets keys
    /// nobody has asked for yet. A constant revision would reuse the previous
    /// frame's key, and `Image.cache: false` only re-requests when the URL changes:
    /// a "re-capture" would then show the old freeze, which is exactly the bug
    /// §5.2.4 cannot survive.
    next_revision: u32,
    pub problems: Vec<String>,
    /// §5.2–5.4：整段 `[capture]` 自 P65（第 65 项 ② 那一族的结法与 P64 的 `[pin]` 同形）
    /// 起由 [`crate::settings::capture_cfg`] 送进运行期 —— `open()` 里取一次快照，之后 13
    /// 枚键（`default_mode` / `include_cursor` / `freeze_frame` / `highlight_element`
    /// / `magnifier_radius` / `magnifier_zoom` / `magnifier_grid` / `magnifier_coordinates`
    /// / `magnifier_color_format` / `show_dimensions` / `size_unit` / `delay_seconds`
    /// / `snap_to_window`）都在 `self.capture.<键>` 那格。P65 之前只有 `show_dimensions`
    /// 单独走 [`crate::settings::show_dimensions`]，其余停在"键在档里没人读"。
    ///
    /// 「等着被接进 QML」这一句自 P164 起对 `size_unit` 不再成立：`open()` 在取完这枚快照的下一行
    /// 就读它（[`Self::unit`]），与 [`Self::show_dimensions`]、[`crate::delay::arm_from_config`]
    /// 那两枚同族 —— 都是**有行为读者**的那几枚。写在这里而不是只写在那一行注释里，是因为本段的
    /// 口径就是"哪几枚还停在没人读"。
    ///
    /// 除 `show_dimensions` 之外的 12 枚自 P108 起少了一枚（`delay_seconds` 有了行为读者
    /// [`crate::delay::arm_from_config`]，而它**不从这里读** —— 延时决定的是 `open()` 之前等多久，
    /// 那一刻这枚快照还没取），自 P164 起再少一枚（`size_unit`），自本轮起再少两枚：
    /// `default_mode` 与 `freeze_frame` 的行为读者是 [`Self::apply_opening`]（`open()` 每场现读，
    /// 判的本文件 [`Self::opening_hole`] / [`Self::freeze_notice`] 那两枚纯本体）。留在本格的那一份
    /// 仍然是「这一场遮罩开着的时候配置长什么样」，各枚读者各说各的时刻，没有谁覆盖谁。
    ///
    /// 快照而不是逐字段 reader：`open()` 只跑一次，中途 `write_back` 不改本会话已经开着的
    /// 遮罩，和 P64 的 `PinState::new()` 同一形状。**运行期读数仍欠**（第 65 项 ②）——
    /// `--mask` 门本轮 0 趟，cargo 单测证的是"拨一下读得回两样答案"（`default_mode` 那一臂的
    /// 两块选区、`freeze_frame` 那一臂的那句话），而**证不了**「用户把 `freeze_frame` 拨到关之后
    /// 遮罩背景会跟着桌面刷新」—— 那一问在这一版产品没有对应的通路，说的就是
    /// [`Self::freeze_notice`] 交回来的那一句，不是漏了一句待办。
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
    /// 整段 `[advanced]` 的快照，与 [`Self::history`]、[`Self::capture`] 同一场 `open` 取。
    /// 本档在这一层只有一枚键有读者（`recognize_text` ⇒ [`auto_ocr_wanted`]），其余五枚停在
    /// 别处已有的读者手里（`log_level` / `data_dir` 由 `settings::startup_banner` 念，
    /// `cli_enabled` 等 Settings 的 CLI 注册按钮）—— 取整段而不是取一枚，是为了不再多养一枚
    /// 逐字段的 reader（P65 给 `[capture]` 立的同一族处置）。
    pub advanced: falcon_core::config::Advanced,
    /// 这一场把「跑一次本机识别、把字落进那一行」交出去几次。数的是**交出去**而不是**落成了**：
    /// 引擎在另一枚线程上跑，落没落成这一刻在这里说不出（[`Self::auto_ocr_join`] 那枚把手
    /// 就是为了让单测能等到它）。关着的键一次也不加，与出厂默认同形。
    pub auto_ocr_asked: u32,
    /// 最近一次交出去的那条线程的把手。产品路径上一位也不读它 —— 图的是 `cargo test` 能
    /// `join` 一下，让那条线程不会活过它所测的那个临时目录。
    pub auto_ocr_join: Option<std::thread::JoinHandle<()>>,
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
    /// [`Self::pin_history_row`] 那一条腿说的话（§5.14.2 step 4「贴图」）。它**包含**落盘那一句
    /// （[`Self::history_replay_last`] 的原文）再接上钉图的结果：文件写成了而贴图被拒是用户必须
    /// 知道的一格 —— 图在盘上、只是没飘起来，把这一句压成「没钉出来」会让人把整件事重做一遍，
    /// 与 [`Self::save_then_pin`] 头上那段同一个形状。
    pub history_pin_last: String,
    /// [`Self::annotate_history_row`] 那一条腿说的话（§5.14.2 step 4「标注」，也是 §5.14.3
    /// step 4「恢复该截图**内容**」唯一把内容放回屏幕上那一腿）。六格失败各说各话：「这一场没有
    /// 历史」「`enabled` 关着」「库打不开」「没有第 N 行」「那一行的图取不回来」「画面铺不上去」
    /// 「遮罩开不起来」是七件不同的事，与 [`Self::history_replay_last`] 那五格同一口径。
    /// 不占 [`Self::message`]：那一格留给落盘。
    pub history_annotate_last: String,
}

/// [`MaskState::opening_hole`] 要的**事实**：遮罩进场那一刻能问出来的几块矩形，全部由调用方
/// 递进来，分岔本体自己一问桌面也不问。
///
/// 拆成单独一型的理由与 [`MagnifierLook`] 同一枚：§9.1 第十条要「拨一下读得回两样答案」在一台
/// 不开窗的机器上判得出，而「光标下是哪一扇窗」「上一次截图是哪一块」这两问都要碰桌面。问桌面
/// 那一半因此留在 [`MaskState::apply_opening`]，这一型只装已经答出来的那半。
///
/// `window` / `last` 是 `Option` 而不是空矩形：「这一刻问不出」在那两臂都是**合法答案**而不是
/// 失败，把它写成 `PhysRect::new(0,0,0,0)` 就会与「光标下真有一扇零大小的窗」混成同一格（#453）。
pub struct OpeningFacts {
    /// `[capture] default_mode = region`（出厂那一侧）用的：[`MaskState::default_hole`] 算出来
    /// 的那一块，一字不改。
    pub free: PhysRect,
    /// `fullscreen` 用的：整块桌面（所有显示器的并集），而不是主屏 —— 两块屏的机器上「全屏」
    /// 问的是用户看得见的全部。
    pub desktop: PhysRect,
    /// `window` 用的：光标下那一扇窗**可截图**的边界。`None` = 这一刻问不出（枚举交回空表、
    /// 或指针落在桌面上）。
    pub window: Option<PhysRect>,
    /// `repeat` 用的：本进程上一次截成过的那一块（§5.2.5）。`None` = 这一趟进程还没有过一次
    /// 选区。
    pub last: Option<PhysRect>,
}

/// [`MaskState::opening_hole`] 的答案：这一场按哪一块进场，以及需不需要开口说一句。
pub struct Opening {
    pub hole: PhysRect,
    pub said: Option<String>,
}

impl Opening {
    /// 照办的那一臂：不开口。用户挑的就是这块区域，没有需要解释的第二句。
    fn at(hole: PhysRect) -> Self {
        Opening { hole, said: None }
    }

    /// 没能照办的那一臂：按自由区域进场，并把**为什么**当场说出来（#433 不许静默）。
    fn falling_back(hole: PhysRect, said: impl Into<String>) -> Self {
        Opening {
            hole,
            said: Some(said.into()),
        }
    }
}

/// [`MaskState::sweep_stale_temp`] 的答案：收回了几枚，和该说的那几句。
///
/// `deleted` 与 `said` 分着放，因为「删掉了」是**盘上读得出**的一件事（那一枚文件没了），而
/// 「删不掉」与「目录打不开」是只有说出来才不会静默的两格。
#[derive(Default)]
pub struct TempSweep {
    pub deleted: usize,
    pub said: Vec<String>,
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
        self.board = None;

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
                mag_key: String::new(),
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
        // §5.3.8 的锁跟着这一场归零，同 [`Self::quick_saves`] 那一族的理由：锁说的是"这一张要
        // 怎么截"。留着上一张的 16:9，下一次按下热键后的第一次拖拽就会画出一个用户这一场没有
        // 挑过的形状 —— 而那一刻芯片是亮着的，没有人在拖之前先去看它。
        self.reset_ratio();
        self.reset_size_anchor();
        // 单位自此不在这三枚里（P164）：PRD §5.3.7 第一步把它写在**截图设置**里，所以它的起点是
        // 档里那一格而不是出厂那一格 —— 落在下面 `self.capture` 快照之后的那一行。
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
        // §5.5.10 那三枚计数与那枚临时文件走的是同一条理由：它们说的是**这一次**流程递出去过
        // 什么。留着上一张的路径，下一次派发就会去收回一张与用户此刻的决定无关的文件 ——
        // 而收回这件事本来就不保证成功（对面的进程也许正开着它），更不该去动别人的。
        self.send_to_clicks = 0;
        self.send_to_acted = 0;
        self.send_to_writes = 0;
        self.send_to_last.clear();
        self.send_to_landed = None;
        // 那一排色带的顺序**不**跟着一场流程走（它是档里的一行，跨窗、跨流程都在），所以这里清的
        // 只有那句话；`band_revisions` 反而一枚也不清 —— 见它自己那一格的口径。
        self.band_last.clear();
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
        // §5.3.7 第一步「用户在截图设置……选择单位」的那一条腿：这一场的单位从档里那一格起，
        // 而不是从 [`Unit::default`] 起。写在快照之后而不是之前，是因为它读的就是这枚快照 ——
        // 与 [`Self::show_dimensions`] 同一刻取值，所以设置页改完单位，下一次按下热键就用新的，
        // 而**已经开着的这一场不动**（一页读数在自己脚下被人换掉，正是 P161 那轮写过的不该有的事）。
        self.unit = self.capture.size_unit;
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
        // §5.16 的 `[advanced]` 整段同场取快照：`recognize_text` 那一枚键自本轮起有了一位产品
        // 读者（[`auto_ocr_wanted`]），而它判的是「刚写下的这一行要不要再花一次引擎」——
        // 与上面那枚 `[history]` 快照不同场取，就会出现「行按旧配置写、识别按新配置跑」的一帧
        // 两份状态。计数与把手同样随会话归零：上一场交出去几次，与这一场跑不跑是两件事。
        self.advanced = crate::settings::advanced_cfg();
        self.auto_ocr_asked = 0;
        self.auto_ocr_join = None;
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
        // §5.14.2 step 4 那两条新腿同理：上一场贴出/标过第几行，与这一场点没点是两件事。
        // `annotate` 那句由 [`Self::annotate_history_row`] 在 `open` **之后**写，清在这里
        // 正好，不会被自己抹掉。
        self.history_pin_last.clear();
        self.history_annotate_last.clear();
        // 锚点同理随会话归零（P113）：上一场界面停在第几行，与这一场从哪一页翻起是两件事，
        // 留着它会让新会话第一次 Ctrl+Down 落在上一场那一行的下一行上。
        self.history_viewed = None;
        // 翻页那枚计数同理（P113）：它拆的是「这一场被翻过几页」，上一场的页数不是这一场的加数。
        self.history_flips = 0;
        self.ink.set_hole(self.hole);
        // §5.4.1 / §5.4.2：这一场留一份冻结快照给放大镜与取色器。一次按下热键一次 `clone`
        // （一帧的量级，与 `store_raw` 铺满每块屏同一刻、同一族代价），之后 `magnifier_at` /
        // `pick_color` 就地读它，绝不再问桌面。放大纹理代数与光标读数同样随会话归零：新的槽
        // 位是刚建的，每格自己的 `Slot::mag_key` 就是空串，不需要在这一趟再 drop 什么。
        self.mag_seq = 0;
        self.cursor_phys = None;
        self.snapshot = Some(snap.clone());
        // §5.3.2：候选窗口表与这一场同生死，与上面那枚快照**同一族理由**（一次按下热键一次
        // 拷贝，之后每次 hover 就地读，绝不再问桌面），也**同一个时刻**取：这一张表说的必须是
        // 屏幕上这一张冻住的画面里有什么，否则高亮框会框住一块用户此刻看不见的地方。
        // 高亮读数一并归零 —— 新一场的光标还没在屏上走过，留着上一场那一扇窗的框就是在屏幕
        // 上画一块与此刻无关的矩形（`landed` / `ratio` / `size_anchor` 同一族）。
        self.windows = capture::window_candidates();
        self.hover_win = None;
        self.hover_taken = false;
        self.virtual_bounds = snap.virtual_bounds;
        // `[capture] default_mode`、`[capture] freeze_frame`、`[advanced] privacy_no_temp_files`
        // 三枚键在产品里的**唯一读者**就在这一行：一场遮罩进场的那一刻。选这一格而不是选在
        // [`crate::hotkey`] 那一趟里，是因为这里已经有快照、有候选窗口表、有那一位算默认选区的
        // 作者（[`Self::default_hole`]），另起一处就会把这些事实各问第二遍（#281）；选在**每场
        // `open` 现读档**而不是启动时缓存，是因为 §5.20.2「设置应即时生效」—— 与上面那几枚
        // `crate::settings::*_cfg()` 同一时刻取值，所以设置页改完模式，下一次按下热键就用新的，
        // 而已经开着的这一场不动。
        self.apply_opening(snap);
        self.sweep_temp_at_open();
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

    /// `[capture] default_mode` 的**分岔本体**：一次没有点名模式的截图（§5.2.1 的快捷键、
    /// §5.2.2 的托盘项）按下之后，遮罩进场时选区停在哪一块，以及需不需要开口。
    ///
    /// 写成递事实的一枚纯函数（`free` / `desktop` / `window` / `last` 全部从外面进来），
    /// 与 [`crate::capture::window_frame`] 的 `decorate_window` 同一枚理由：本函数不开窗、
    /// 不抓屏、不问桌面，所以 §9.1 第十条那一半（拨一下读得回两样答案）能在 `cargo test` 里判，
    /// 而不是只在真机上被看见。[`MaskState::open`] 是唯一递真事实进来的地方。
    ///
    /// 四臂各自对面，而**出厂那一臂（`Region`）一字不改** —— 这一枚键从「档里没人读」变成
    /// 有人读的那一刻，默认路径的进场选区必须还是 [`Self::default_hole`] 算出来的那一块：
    /// * `Region` ⇒ 就是 `free`，不开口（§5.3.1 自由区域，一直是那样）。
    /// * `Fullscreen` ⇒ `desktop` = 整桌面那一块（遮罩本来就铺满每一块屏，所以这里取快照的
    ///   `virtual_bounds`：两块屏的机器上「全屏」= 用户看得见的全部，而不是只有主屏）。
    ///   选区仍可被拖、被改，进场那一块只是起点。
    /// * `Window` ⇒ `window` = 光标下那一扇窗，问的正是 §5.3.2 第四步用的同一枚闸
    ///   （[`crate::capture::window_under`] 对着**这一场**那张候选表，不另开一条挑窗的路）。
    ///   答不出时**不说成「这一枚模式没做」**：那是另一件事（#453），所以那一臂单独一句。
    /// * `Repeat` ⇒ `last` = §5.2.5 记住的那一块；这一趟进程没截成过东西就说那一句。
    /// * `Element` ⇒ **这一版产品做不到**：界面元素检测的那一只手（UIA `ElementFromPoint`）
    ///   在本仓库是 [`falcon_core::capture::WindowExtras::element_chain`] 那一枚交空表的占位，
    ///   没有第二条路能答出「按钮/输入框的边界」。不替它编一个读者，也不静默：话当场说出口，
    ///   遮罩按 `free` 进场。
    pub fn opening_hole(
        mode: falcon_core::config::CaptureMode,
        facts: &OpeningFacts,
    ) -> Opening {
        use falcon_core::config::CaptureMode as M;
        match mode {
            M::Region => Opening::at(facts.free),
            M::Fullscreen => Opening::at(facts.desktop),
            M::Window => match facts.window {
                Some(rect) => Opening::at(rect),
                None => Opening::falling_back(
                    facts.free,
                    "[capture] default_mode = window：这一刻问不出光标下是哪一扇窗\
                     （窗口枚举交回空表，或指针不在任何窗上），这一场按自由区域进场。"
                        .to_string(),
                ),
            },
            M::Repeat => match facts.last {
                Some(rect) => Opening::at(rect),
                None => Opening::falling_back(
                    facts.free,
                    "[capture] default_mode = repeat：这一趟进程还没有过一次选区，可重复的\
                     「上一次截图」是 §5.2.5 那一枚快捷键的事；这一场按自由区域进场。"
                        .to_string(),
                ),
            },
            M::Element => Opening::falling_back(
                facts.free,
                "[capture] default_mode = element：界面元素检测这一版产品还没有那一只手\
                 （core 的 element_chain 只会交回空表），这一枚键做不到；这一场按自由区域进场。"
                    .to_string(),
            ),
        }
    }

    /// §5.2.1 第二步「应用**冻结**或捕获当前屏幕画面」里 `[capture] freeze_frame` 的那一判。
    ///
    /// **这一枚键产品做不到**，而做不到的那一面不许静默（#433）：本产品的遮罩背景**天生就是**
    /// 按下那一刻冻住的那一帧，且这是全模块的头一条规矩（[`crate::capture`] 头顶：屏幕在任何
    /// 一扇遮罩窗存在之前只读一次，之后每一问都从那一帧答 —— 遮罩出现之后再 `BitBlt` 一次，
    /// 拍进图里的就是截图工具自己）。「实时取景」要的是一条会随桌面刷新的背景纹理通路，本仓库
    /// 一处也没有：槽位按 `revision` 存**一帧**、`Image` 的 URL 换了才重画，没有第二条刷新路。
    ///
    /// 所以关着那一臂交回的就是这句话本身，而不是一个假读者（把画面改成"看起来像活的"既做不
    /// 到、也不该做）。开着那一臂交回 `None`：默认那一侧屏幕上不许多出一行与用户决定无关的话。
    pub fn freeze_notice(freeze_frame: bool) -> Option<String> {
        if freeze_frame {
            return None;
        }
        Some(
            "[capture] freeze_frame 关着，可这一版遮罩的背景永远是按下那一刻冻住的那一帧：\
             产品没有实时取景那一路（遮罩出现之后再抓一次屏幕，图里就会带上遮罩自己），\
             这一枚键做不到，画面仍是冻的。"
                .to_string(),
        )
    }

    /// §6.2「临时文件应按策略自动清理」里 `[advanced] privacy_no_temp_files` 的**分岔本体**：
    /// 本程序自己那枚**带标签**的临时目录里、不属于当前这一场的旧图，收不收。
    ///
    /// 只动那一枚目录（[`Self::sendto_dir`] 的 `%TEMP%\falconshot-sendto`），口径与
    /// [`crate::state::PinState::clip_dir`] 头顶那句同一枚：标签是本程序自己打的，那里面只可能
    /// 是本程序写过的东西；temp 里别人家的文件一枚也不碰。
    ///
    /// 三臂三样答案（#453「键说不要」「这台机器做不到」「读不出」各说各的话）：
    /// * 关着（出厂默认 `true` 的反面，用户自己关的）⇒ **一枚也不删**、一句也不说：不清理不是
    ///   一次失败，是用户许过的状态。
    /// * 开着 ⇒ 除 `keep` 之外每一枚试删一次；删掉的进 `deleted`，**删不掉的**（对面的程序还
    ///   开着它）单独说一句并原样留着 —— 不假装收回了。
    /// * 目录打不开 ⇒ 另起一句，与"删不掉"不混成一句；其中**目录不存在**那一格不算打不开：
    ///   没有旧图就是没有旧图，那枚目录是 [`Self::sendto_dir`] 自己造的，还没人发送过东西时它
    ///   本就该不在，此时屏幕上多出一句抱怨反而是假话。
    ///
    /// 说的话交回给调用者写进 [`MaskState::problems`]：这一枚键的效果本来就在盘上读得出
    /// （旧图还在不在），话是证人而不是界面。
    pub fn sweep_stale_temp(
        dir: &std::path::Path,
        keep: &[std::path::PathBuf],
        privacy: bool,
    ) -> TempSweep {
        if !privacy {
            return TempSweep::default();
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(it) => it,
            // 目录压根不存在 = 这一趟程序没写过、也没有欠着的旧图，那是**干净**而不是一次没办成
            // 的事，所以一句也不说：每一场遮罩都要说一遍「临时目录打不开」，用户读到的是抱怨而不
            // 是事实。除此之外的失败（权限、盘没了）才单独说一句，且与「删不掉」不混成一句（#453）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TempSweep::default(),
            Err(e) => {
                return TempSweep {
                    said: vec![format!(
                        "[advanced] privacy_no_temp_files 开着，可临时目录 {} 打不开：{e}",
                        dir.display()
                    )],
                    ..Default::default()
                };
            }
        };
        let mut swept = TempSweep::default();
        for entry in entries.flatten() {
            let path = entry.path();
            if keep.iter().any(|held| held == &path) {
                continue;
            }
            let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
            if !is_file {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => swept.deleted += 1,
                Err(e) => swept.said.push(format!(
                    "{} 还收不掉（{e}），原样留着",
                    path.display()
                )),
            }
        }
        if swept.deleted > 0 {
            swept.said.insert(
                0,
                format!(
                    "[advanced] privacy_no_temp_files 开着：收回了 {} 枚旧图。",
                    swept.deleted
                ),
            );
        }
        swept
    }

    /// `[capture] default_mode` 与 `[capture] freeze_frame` 这两枚键**在产品里的读处**：一场遮
    /// 罩进场的那一刻，档现读、现判、现落地。
    ///
    /// 两枚键各只有一次 `self.capture` 的读数（[`MaskState::capture`] 是 `open` 开头取的那枚
    /// 快照，与 `include_cursor` / `size_unit` 同一场、同一理由），而**问桌面那一半只问开着
    /// 的那一枚需要问的**：`region` / `fullscreen` 两臂一次桌面也不碰，`window` 那一臂问的正是
    /// §5.3.3 那位既有的读者 [`crate::capture::window_under_cursor`]（本模块不第二次算「光标落在
    /// 哪一扇窗、该窗可截图的边界是哪一块」，#281），`repeat` 那一臂问的是 §5.2.5 记住的那一块
    /// （[`crate::capture::last_region`]，与自定义截图对话框、`--last` 同一枚来源）。
    ///
    /// 落地走 [`Self::set_hole`] 而不是直接改 `self.hole`：那是选区唯一那一位作者，标注层的裁
    /// 剪跟着它走 —— §5.3.5 的自定义截图与 §5.14.3 的「回到原选区」都是从这一手进去的。
    ///
    /// 说出来的话进 [`Self::say`]：进场那几格说的是「这一场为什么不是你点的那一枚钮」，必须在
    /// 屏幕上看得见（#433），而它**不进** [`MaskState::problems`] —— 那一格在 `--mask` 那扇门上
    /// 是「一场流程该不该红」的判据，把一枚用户自己拨过的开关算成一次故障，会让那扇门红在用户
    /// 的决定上而不是红在产品出错上（那是两件事，#453）。
    fn apply_opening(&mut self, snap: &ScreenSnapshot) {
        use falcon_core::config::CaptureMode as M;
        let mode = self.capture.default_mode;
        let mut facts = OpeningFacts {
            free: Self::default_hole(snap),
            desktop: snap.virtual_bounds,
            window: None,
            last: None,
        };
        match mode {
            M::Window => {
                facts.window = capture::window_under_cursor().map(|w| w.visible_bounds());
            }
            M::Repeat => {
                facts.last = capture::last_region().map(|l| l.rect);
            }
            _ => {}
        }
        let opening = Self::opening_hole(mode, &facts);
        self.set_hole(opening.hole);
        if let Some(said) = opening.said {
            self.say(&said);
        }
        // `freeze_frame` 关着时交回的就是「这一枚键这一版产品做不到」那句话本身：本模块没有
        // 一条能让背景随桌面刷新的路（见 [`Self::freeze_notice`] 头顶），所以这里能做的诚实事情
        // 只有把它说出来，而不是把画面改成"看起来像活的"。
        if let Some(said) = Self::freeze_notice(self.capture.freeze_frame) {
            self.say(&said);
        }
    }

    /// `[advanced] privacy_no_temp_files` 在产品里的**读处**：新的一场开始之前，把上一场（或
    /// 上一趟进程）留在本程序自己那枚带标签的临时目录里的旧图收回来。
    ///
    /// 时刻选在 `open` 而不是选在流程收梢，有一条硬理由与一条软理由。软的：开新图时旧图已经
    /// 没有用了，而「剪贴板还引用着就别删」那一条管的是**另一枚目录**（`falconshot-clip`，由
    /// [`crate::state::PinState::sweep_clip_files`] 问剪贴板，两处各按各自那一问执行，见
    /// [`Self::sendto_dir`] 头顶）。硬的：`--sendto` 那扇门在开完遮罩之后才对那枚目录做普查
    /// （`start_left`），收回发生在普查之前才谈得上"门数到的是收完之后的盘"；写在
    /// [`Self::send_to_target`] 里就会把门刚递出去的那一张算成它自己留下的垃圾。
    ///
    /// `keep` 递的是**这一场**还欠着的那一枚（[`MaskState::send_to_landed`]，`open` 开头已清成
    /// `None`，所以这里是空表）：清空之后本程序在这一趟里没有任何还欠着的文件，目录里的每一张
    /// 都属于过去的流程。话进 [`Self::say`] 而不进 `problems`，理由与上面那一格同一枚。
    fn sweep_temp_at_open(&mut self) {
        let keep: Vec<std::path::PathBuf> = self
            .send_to_landed
            .clone()
            .into_iter()
            .collect::<Vec<_>>();
        let swept = Self::sweep_stale_temp(
            &Self::sendto_dir(),
            &keep,
            crate::settings::advanced_cfg().privacy_no_temp_files,
        );
        for said in swept.said {
            self.say(&said);
        }
    }

    /// 把一句**这一场自己该说的话**接在 [`Self::message`] 后面。
    ///
    /// 接而不是换：一次进场可能有两句话（模式问不出 + 实时取景做不到），把它们各占一格就得新
    /// 建一枚 qproperty 并改 QML 绑定，而这一轮的 QML 归另一位（#281 也把"屏幕上那一行该说什么"
    /// 这一件事留给已经有的那一位作者）。空表就是空串，所以 `--mask` 门上「话由落盘那一手写」
    /// 那几格的开局没有被抢先。
    fn say(&mut self, said: &str) {
        if self.message.is_empty() {
            self.message = said.to_string();
        } else {
            self.message.push(' ');
            self.message.push_str(said);
        }
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
            // §5.4.1：这一扇窗**最后**一次 hover 留下的那张放大纹理也一起带走。
            // [`Self::magnifier_at`] 已经在每次换键时 drop 上一张，所以场上最多只留本窗
            // 当前那一张；一次 `close` 之后 Qt 的纹理仓库里不该有 `mag-*` 前缀的东西。
            // 空串就是"这一场这个窗一次 hover 都没走过"，`drop_frame` 对它是 no-op。
            if !slot.mag_key.is_empty() {
                shim::drop_frame(&slot.mag_key);
            }
        }
        self.slots.clear();
        self.grab = None;
        // §5.3.2：候选表与高亮读数一起带走。这一张表是**一扇窗一份快照**的量级（每格四根
        // 字符串 + 两枚矩形），一场流程攒着它没有意义，而留着它会让下一场第一次 hover 读到
        // 上一场那扇还开着的窗 —— 与 [`Self::slots`] 被清空同一条理由。
        self.windows.clear();
        self.hover_win = None;
        self.hover_taken = false;
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

    /// 这一块板子要暗化多少。截图那一路（`board = None`）要的是 [`DIM`] —— 桌面是被藏起来
    /// 的那一层；透明白板要的是 **0**，因为 §5.4.5 第二步「以透明画布进入标注状态」问的就是
    /// 用户能不能看穿自己那块画布，压暗 60% 就是把那一句反着做。
    ///
    /// 实心板仍旧暗化：它的"未选中区"是这块板的边框，而 #433 那一族要的是每一枚读数都有
    /// 一个对面 —— `DIM` 在门上的对面是 `1 - DIM`，把它替 0 之后那一格就再没有证人。
    pub fn board_dim(&self) -> f64 {
        match self.board {
            Some(falcon_core::whiteboard::Surface::Transparent) => 0.0,
            _ => DIM,
        }
    }

    /// 窗底色该不该跟着透明。QML 那侧的 `color: "black"` 是遮罩窗自己的底，暗化层画在它
    /// 上面：暗化归零而底还是黑，屏幕上出现的仍是一张黑幕 —— 这两格必须一起点。
    pub fn board_clear(&self) -> bool {
        matches!(
            self.board,
            Some(falcon_core::whiteboard::Surface::Transparent)
        )
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
        // §5.3.2：按下这一点也算「光标走到了这一点」，所以候选窗在这里也重算一次，而不是
        // 指望 QML 先调过 [`Self::hover_window`]。不这样写的话，「一次直接按下（中间没有
        // 任何 move 事件）」就会让 [`Self::release`] 读到上一一次 hover 留下的那一扇窗 —— 高亮
        // 说 A、单击选中 B，正是 `hit_test` 头上那条「指针不该谎报自己即将抓住什么」。
        self.hover_win = capture::window_under(&self.windows, p);
        // 按下就是「不挑了，开始要一块了」：第三步的高亮自此不再报，直到 Esc 第一档把选区
        // 清掉（[`Self::escape`]）。写在选区那两臂之前而不是之后，是因为 `hover_taken` 一
        // 旦点起来，[`Self::hover_bounds`] 就答 `None`，而上面那一格 `hover_win` 得留着 ——
        // 它是 [`Self::release`] 整窗选中要读的那一块，不能被"不亮了"顺手清掉。
        self.hover_taken = true;
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

    /// §5.3.8 的锁此刻是什么（宽 / 高），`None` = 不锁。从 [`Self::ratio_index`] 与
    /// [`Self::ratio_spec`] **算**出来的，不是第三格存着的 —— 见那两格的注释。
    ///
    /// 自定义那一格解不出来时答 `None` 而不是留着上一次的比例：`ratio_spec` 是用户**正在打**
    /// 的那句话，打到 `"1"` 还没打冒号的那一刻，屏幕上亮着的"自定义"芯片不该假装锁着 1:1。
    pub fn ratio_lock(&self) -> Option<f64> {
        if self.ratio_index < 0 {
            parse_ratio(&self.ratio_spec)
        } else {
            RATIO_CELLS
                .get(self.ratio_index as usize)
                .and_then(|(_, spec, _)| parse_ratio(spec))
        }
    }

    /// §5.3.8 第一步与第二步的前半：按下第 `index` 格芯片。越界**不动**并返回 `false` ——
    /// 一把门把手不该因为 QML 递来一个越界数字就把锁清了。
    pub fn pick_ratio(&mut self, index: i32) -> bool {
        let Some((_, _, _)) = RATIO_CELLS.get(index as usize) else {
            return false;
        };
        self.ratio_index = index;
        self.ratio_spec.clear();
        true
    }

    /// §5.3.8 第二步的后半：「或输入自定义比例」。
    ///
    /// 那句话**总是**被记下（屏幕上显示的就是用户打的字，主人只能有一个），返回值说的是
    /// **锁上没上**：解得出来 `true`，解不出来 `false` 而 [`Self::ratio_live`] 跟着变 `false`，
    /// QML 拿这一枚把文本框点灰。"打错的那句话把已经锁好的 1:1 悄悄松开"与"打错的那句话在
    /// 屏幕上被改掉"是两件事，前者由 [`Self::ratio_lock`] 现场算出来，后者永远不发生。
    ///
    /// 空串是例外 —— 那不是"打错了"而是"清掉了"，与按「无」同一意思，于是 index 回到 0。
    pub fn set_ratio_spec(&mut self, spec: &str) -> bool {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            return self.pick_ratio(0);
        }
        self.ratio_index = -1;
        self.ratio_spec = trimmed.to_string();
        parse_ratio(trimmed).is_some()
    }

    /// §5.3.8：这一场的锁归零。抽成一枚函数是因为 `open()` 要抓屏、单测不能抓屏（§9.1 第十条
    /// 的 headless 半边，同 [`Self::seed_landed_from`] 那一条理由），而"下一场不该继承上一场的
    /// 比例"这一句得有证人。
    fn reset_ratio(&mut self) {
        self.ratio_index = 0;
        self.ratio_spec.clear();
    }

    /// §5.3.6 那一枚锚点跟着这一场归零，与 [`Self::reset_ratio`] 同一条理由：它说的是"这一张
    /// 刚刚被按住过哪一角"。留着上一张的 `BottomRight`，下一场第一次打尺寸就会从一张用户这一场
    /// 没拖过的角上长出去。抽成一枚函数同样是因为 `open()` 要抓屏、单测不能抓屏（§9.1 第十条）。
    fn reset_size_anchor(&mut self) {
        self.size_anchor = Anchor::default();
    }

    /// §5.3.6 第三步与第四步：把打进来的这对着实尺寸换成选区的尺寸，从 [`Self::size_anchor`]
    /// 那一角扩展，超出桌面的那一头让路（[`set_size_within`]）。
    ///
    /// `false` 的三种来历**都不动选区**：第一步还没走（没有选区可调整）、数字不是正数
    /// （§5.3.5 那条「宽度和高度必须大于 0」在这一路同用，而 0 与负数打的是同一格键盘）、
    /// 以及递来的数装不进 `u32`。
    ///
    /// `true` 说的只是"这一对字换掉了尺寸"，**不是**"这就是你打的数"：夹过的结果可能比打的
    /// 小，那正是第二条规则要的让路，而让了多少由 `reload` 之后那行 W×H 读数自己说。
    ///
    /// 与 §5.3.8 那把锁撞车时，本轮的答案是**打的数字赢、芯片跟着改口**：一次显式的输入比一把
    /// 还亮着的锁新。留着亮着的 1:1 而矩形已经不是 1:1，屏幕上就是一枚在说谎的芯片。判据不新造
    /// 容差 —— 把打出来的矩形再喂回锁自己用的那把 [`constrain_aspect`]，它答"不用改"才算还锁着。
    pub fn type_size(&mut self, w: i32, h: i32) -> bool {
        if !self.has_selection() {
            return false;
        }
        let (Ok(w), Ok(h)) = (u32::try_from(w), u32::try_from(h)) else {
            return false;
        };
        if w < 1 || h < 1 {
            return false;
        }
        // §5.3.7 第三步：这对数**先按当前单位换成物理像素**，再交给桌面边界去夹。顺序不能
        // 反过来 —— `set_size_within` 夹的是 [`Self::desktop`]，而桌面是物理像素做的：先夹后换
        // 就把"200% 屏上的 1024 dip"当成 1024 物理像素夹掉一半，那正是本轮要防的那类错。
        // [`Unit::Px`] 这一支 `to_phys` 是恒等，所以出厂状态下的字节零改变不是承诺，是算出来的。
        let scale = self.size_scale();
        let (w, h) = (self.unit.to_phys(w, scale), self.unit.to_phys(h, scale));
        let anchor = self.size_anchor;
        let next = set_size_within(&self.hole, w, h, anchor, &self.desktop());
        let broke_the_lock = self
            .ratio_lock()
            .and_then(|r| constrain_aspect(&next, r, anchor))
            .is_some_and(|kept| (kept.w, kept.h) != (next.w, next.h));
        self.set_hole(next);
        if broke_the_lock {
            // 走 `pick_ratio(0)` 而不是只把 index 挪一格：自定义那一格的原文也得跟着清，否则
            // 文本框里还写着「16:10」而锁已经不在了（第 162 项 ⑤ 那一族"亮着却画不出锁"）。
            self.pick_ratio(0);
        }
        true
    }

    /// §5.3.7 第三步里那句「**当前显示器**缩放比例」的所指：选区**自己那张屏**的缩放，由
    /// [`monitor_slot_for`] 从槽位里问出来（与选区重叠面积最宽的那一扇，打平取更早的那一扇）。
    ///
    /// 不用"这一扇窗自己的缩放"（[`Slot::scale`]）：那样同一张跨屏选区在两扇窗上会读出两个
    /// 不同的数字，而单位切换问的是"这一块有多大"，不是"你在哪一扇窗上看它"。两张屏、一个
    /// 状态 —— 与 §5.3.6 的锚点、§5.3.8 的锁走的是同一条口径。
    ///
    /// `monitor_slot_for` 自 P67 写下之后一直没有生产调用者，它自己的文档就写着这枚缩放是它
    /// 的用途（"used to pick which scale a mixed-DIP readout shows"）；本轮是它第一次被真的读。
    ///
    /// 一扇槽位都没有（`open` 之前、或槽位全被配不上对丢掉）时答 [`Scale::ONE`]：px 那一支
    /// 根本不看它，而 dip 那一支在"没有当前显示器可说"的时候，恒等至少不会冒充另一张屏的缩放。
    pub fn size_scale(&self) -> Scale {
        let screens: Vec<(String, PhysRect)> = self
            .slots
            .iter()
            .map(|s| (s.name.clone(), s.bounds))
            .collect();
        monitor_slot_for(&screens, self.hole)
            .and_then(|i| self.slots.get(i))
            .map(|s| s.scale)
            .unwrap_or(Scale::ONE)
    }

    /// 屏幕上那对数字：选区自己的物理尺寸先经 [`Self::size_scale`]、再按 [`Self::unit`] 换算过
    /// 后的样子。**这是一句读数，不是一份几何** —— 窗怎么画暗化与八枚把手仍然走
    /// [`Self::hole_for`]，见 [`MaskViewData::size_w`] 那一格的理由。
    pub fn size_shown(&self) -> (u32, u32) {
        let scale = self.size_scale();
        (
            self.unit.show(self.hole.w, scale),
            self.unit.show(self.hole.h, scale),
        )
    }

    /// §5.3.7 第一步「用户在…**尺寸输入区域**选择单位」：把单位定成 [`SIZE_UNITS`] 的第 `code`
    /// 格。过界的序号答出厂那一格（[`unit_at_code`]）而不是最近的一格 —— 页上的下标错了是
    /// 一次编程事故，它不该被读成"用户挑了 px"。
    ///
    /// 返回**真正生效**的那一格而不是 `bool`：这一格没有"没做成"的臂（任何下标都有答案），
    /// 而页要点亮的芯片必须跟着 Rust 的答案走，不能跟着自己递出去的那个数字走。
    pub fn set_unit(&mut self, code: i32) -> Unit {
        self.unit = unit_at_code(code);
        self.unit
    }

    /// The pointer moved while held. A new rectangle is normalised from the anchor,
    /// so dragging up-and-left works as well as down-and-right.
    ///
    /// A stroke mid-drag is not a selection drag: the drawing tool already took the
    /// press, so the point goes to the ink and the hole is left alone.
    ///
    /// `unlocked` is §5.3.8 的那句"用户应可以临时解除比例约束"：按住修饰键的那一段拖拽
    /// 走自由比例，松手即回锁（这一格不进 [`Self::ratio_index`]，所以没有一把"临时按着却
    /// 永久留在解除态"的门）。
    pub fn drag(&mut self, index: usize, x: f64, y: f64, unlocked: bool) {
        let Some(p) = self.desk_point(index, x, y) else {
            return;
        };
        if self.ink.dragging() {
            self.ink.drag(p);
            return;
        }
        let Some(g) = self.grab else { return };
        let bounds = self.desktop();
        let free = match g.handle {
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
        };
        // §5.3.8「调整角点时保持比例」。`anchor_of_corner` 只对四枚角点答话，所以边把手与
        // 拖动整块（`Body`）保持原样 —— PRD 把规则钉在角点上，不是漏了边：一条边只能改一个
        // 维度，把它锁进比例就等于把它变成第二条对角线。
        //
        // 桌面边界与这一枚锁相撞时谁付账，本轮量过（`the_border_shortens_the_drag_and_pays_
        // neither_the_ratio_nor_the_anchor` 与 `the_desktop_never_touches_a_lock_selected_rect`
        // 的四枚比例串 × 四枚角点共 16 臂）：**边界先赢，但它赢在 `desk_point` 那一次指针夹取
        // 里**，等轮到比例时它看到的自由矩形已经贴在桌面上。而 `constrain_aspect` 取面积更近的
        // 那一支，就意味着它只会把两条边收短（要收长另一支才轮得到它，这一句在 16 臂里逐臂反证
        // 过：选区自己的 w/h 与 r 差不到一枚像素），于是锁选出的矩形 ⊆ 自由矩形 ⊆ 桌面，下面那
        // 一次 `fitted_into` 在角点锁路上取不到任何动作。所以付账的是**拖拽行程**：抓住的那枚角
        // 停在桌面上，而不是停在指针上；§5.3.1 那枚对角与比例两条都原样。`fitted_into` 的滑动
        // 分支不是永远不动 —— Body 那一支每一轮都在用它，16 臂之外还有一臂就是量这个的；尺寸
        // 改写分支只在锁选出的矩形比整张桌面还大时才走到。
        // `mask_check` 那一扇门问不出上面任何一句：它没有指针注入器（第 46 项 ②）。
        let locked = match (self.ratio_lock(), unlocked) {
            (Some(r), false) => {
                let anchor = match g.handle {
                    None => Some(anchor_of_drag(g.anchor, p)),
                    Some(h) => anchor_of_corner(h),
                };
                match anchor.and_then(|a| constrain_aspect(&free, r, a)) {
                    Some(c) => c.fitted_into(&bounds),
                    None => free,
                }
            }
            _ => free,
        };
        // §5.3.6 第一条规则的来历：这一趟拖拽**定住了哪一角**。它写在锁的外面，因为不锁的时候
        // 用户同样是在按住一角调整，而松手之后打进来的那对宽高要用的就是这一枚 —— 锁开没开
        // 不该改变"我刚按住的是哪一角"这个答案。
        //
        // `unwrap_or(自己)` 那一支是边把手与整块移动：它们没有回答"哪一角"（[`anchor_of_corner`]
        // 只对四枚角点答话），所以留着上一次的答案而不是猜一枚新的。
        self.size_anchor = match g.handle {
            None => anchor_of_drag(g.anchor, p),
            Some(h) => anchor_of_corner(h).unwrap_or(self.size_anchor),
        };
        self.set_hole(locked);
    }

    /// The pointer let go: the selection's grab is dropped, and a half-drawn stroke
    /// becomes an object (or is discarded) in the layer.
    pub fn release(&mut self) {
        // §5.3.2 第四步「用户单击选中整个窗口」落在这一手，而不是落在 [`Self::press`]：
        // 第三步的高亮是 hover 的读数，而**按下不等于要这一扇窗** —— §5.3.1 第二步要的正是
        // 「按住鼠标左键并拖动」，一次拖选如果先被整窗选中带跑，第一条腿就没了。
        //
        // 判据是「这一趟按住过、而选区还停在 [`Self::press`] 当场立的那一格 0×0@anchor」，
        // 不是「选区是空的」：后者会把**纯水平**那一拖（`from_points` 交出 h=0，按
        // [`PhysRect::is_empty`] 就是空）读成一次单击，于是用户拖着一条 0 高的带子松手，
        // 屏幕上跳出来的是一整扇窗。逐格比那一张矩形，任何一点移动都会改掉 w、h 或原点。
        let clicked_not_dragged = self.grab.as_ref().is_some_and(|g| {
            g.handle.is_none() && self.hole == PhysRect::new(g.anchor.x, g.anchor.y, 0, 0)
        });
        self.grab = None;
        if self.ink.dragging() {
            self.ink.release();
            return;
        }
        if clicked_not_dragged && self.capture.snap_to_window {
            // 抽成 [`Self::select_hovered_window`] 是因为这一臂要能在没有窗、没有 Qt 的地方被判：
            // 同 [`Self::reset_ratio`] 那一条理由（§9.1 第十条的 headless 半边）。
            self.select_hovered_window();
        }
    }

    /// §5.3.2 第四步的落点：把手里那一扇**光标最直接指向**的窗（[`Self::hover_win`]，第三
    /// 步的高亮就是它）整窗交出去成为选区。
    ///
    /// 交的是这块可截图边界而不是 `GetWindowRect` 那一条带阴影的框 —— 一位作者
    /// ([`falcon_core::capture::WindowInfo::visible_bounds`]，经
    /// [`crate::capture::window_under`])，与窗口截图、与屏幕上那圈高亮框同一条矩形。选区、
    /// 高亮、最后截出来的图三处因此不可能各说一遍。
    ///
    /// 让路给桌面边界（[`PhysRect::clamp_to`] 的**裁剪**而不是
    /// [`PhysRect::fitted_into`] 的滑动）：一扇被拖出屏幕外的窗、一扇 DWM 说比它所在显示器
    /// 更宽的最大化窗，屏幕上真的有内容的只有相交那一块，选区多出来的那一圈底下是别的窗。
    /// 相交为空（这扇窗已经整扇离开桌面）时**不动选区**：把用户刚点出来的空选区再交回一次
    /// 空，等于让他以为这一枪打空了；留着 0×0 让下一次 hover 重新说话更诚实。
    ///
    /// `false` = 此刻没有可整窗选中的窗（候选表空、光标在桌面上、或那一扇已经量不出可见
    /// 内容）。`true` 说的只是「选区换成了这一扇窗的边界」，不是「这就是全屏幕」。
    pub fn select_hovered_window(&mut self) -> bool {
        let Some((_, rect)) = self.hover_win else {
            return false;
        };
        let desk = self.desktop();
        let Some((clipped, _)) = rect.clamp_to(&desk) else {
            return false;
        };
        if clipped.is_empty() {
            return false;
        }
        self.set_hole(clipped);
        true
    }

    /// §5.3.2 第二、三步：光标走到了这一点，把「最直接指向的那一扇窗」记进
    /// [`Self::hover_win`]，并答这一趟有没有选中一扇（QML 拿这一枚决定高亮框画不画）。
    ///
    /// 读数**从这一点现算**而不是留着上一次的答案：`hit_test` 头上那句「一把会谎报自己
    /// 即将抓住什么的指针」在这里同样成立 —— 高亮框说这一扇窗、而按下之后选中的是另一扇，
    /// 用户只会以为是吸附在乱来。 [`Self::press`] 因此也走这一手，不依赖 QML 先调 hover。
    ///
    /// 判据（哪一扇、可截边界）一概不在这里写第二遍：[`crate::capture::window_under`] 与
    /// 它背后的 core 那两只手是唯一作者。这一手只做**搬运**和一次坐标换算。
    ///
    /// 指针越出这一扇窗（`desk_point` 取不到槽位）时**清掉**高亮而不是留着上一扇：一扇
    /// 遮罩窗收不到 hover 之后屏幕上也就不该再画着它上一次说的那一块。
    pub fn hover_window(&mut self, index: usize, x: f64, y: f64) -> bool {
        let Some(p) = self.desk_point(index, x, y) else {
            self.hover_win = None;
            return false;
        };
        self.hover_win = capture::window_under(&self.windows, p);
        // 答的是**页上会画出来的那一格**（[`Self::hover_bounds`]），不是「命中了一扇窗」：
        // 两件事必须有同一个主人，否则桥上就会出现「`live` 说亮、五格读数是空的」或者反过来
        // —— 上面那段「高亮说 A、单击选中 B」防的是同一族错，只是这一回的错法是画不画。
        self.hover_bounds(index).is_some()
    }

    /// [`Self::hover_win`] 在**这一扇遮罩窗**上怎么看：裁进这扇屏、移到这扇屏的原点、除过
    /// 这扇屏的缩放。走 [`Self::local_dip`] 那一位作者，与选区 `hole` 画在哪一格用的是同一
    /// 把尺子 —— 高亮框与选区若各算一遍，200% 屏上就会差出一像素，而那正是「看着贴住了、
    /// 单击之后跳一格」的来历。
    ///
    /// `None` = 此刻没有可高亮的窗，或那一扇与这扇屏不相交（跨两扇屏的窗在另一扇屏上画的
    /// 是另一块，`local_dip` 自己会裁）。
    pub fn hover_bounds(&self, index: usize) -> Option<PhysRect> {
        // 「用户还在挑」才亮 —— 见 [`MaskState::hover_taken`] 头上那一段：拿 `selected`
        // 当闸门在这一场是永远不亮的，因为遮罩是带着默认选区开门的。
        if self.hover_taken {
            return None;
        }
        let slot = self.slot(index)?;
        let (_, rect) = self.hover_win?;
        let local = self.local_dip(slot, &rect);
        (!local.is_empty()).then_some(local)
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

    /// §5.4.1：指针落在**这一扇窗**的 `(x, y)`（窗内 DIP）时，放大镜这一块的完整读数。
    ///
    /// 这一手只做一次换算 —— 把窗内 DIP 折成桌面物理点（[`Self::desk_point`]，那一位作者），
    /// 再交给真正的装配 [`Self::magnifier_at_point`]。页上 `surface` 的按下/移动/松开三处走的
    /// 是这一手；§5.3.9 第 4 步键盘那一腿走的是下面那一手。**两条腿共用同一份装配**，所以
    /// 「面板里的像素、坐标那一句、按 `C` 复制的那一串」在任何一条腿上都只可能来自同一个点 ——
    /// 给键盘另写一遍采样就是给同一件事找第二个作者（#281）。
    ///
    /// 这一点折不出本槽位（没有这一扇窗）时 [`Self::cursor_phys`] 被清掉：这一场此刻连"指针
    /// 在哪"都还没有答案，取色器该说的是"先把指针放到遮罩上"，而不是上一次的那一颗。
    pub fn magnifier_at(
        &mut self,
        index: usize,
        x: f64,
        y: f64,
        look: &MagnifierLook,
    ) -> MagnifierData {
        match self.desk_point(index, x, y) {
            Some(p) => self.magnifier_at_point(index, p, look),
            None => {
                self.cursor_phys = None;
                MagnifierData {
                    look: look.clone(),
                    ..Default::default()
                }
            }
        }
    }

    /// §5.4.1 / §5.3.9 第 4 步 —— 放大镜读数的**唯一装配**：一颗桌面物理点 → 面板的全部读数。
    ///
    /// 每一步都在 core 里做，这一手只做**搬运**：像素由 [`ScreenSnapshot::magnifier`] 放大、
    /// 坐标与颜色字符串由 [`ScreenSnapshot::magnifier_view`] 装配；这一手只把 `frame` 送进
    /// Qt 的纹理仓库（键 = `mag-<index>-<seq>`，`seq` 每 hover 一次加一，同 §3.6 约束 8 与
    /// [`Slot::mag_key`] 上那三行理由），把剩下的字段原样交给桥。
    ///
    /// 递进来的是**桌面物理点**而不是窗内 DIP，为的是让键盘那一条腿也能叫同一次装配：方向键
    /// 那一趟屏幕上没有新的鼠标事件，能拿得出来的只有"刚被挪掉的那条边在桌面上是哪一颗"
    /// （[`falcon_core::geometry::nudge_sample`]，[`Self::nudge_magnified`] 递进来）。换算成
    /// 桌面点这件事仍然只有 [`Self::desk_point`] 一个作者，这一手不再倒着折一遍。
    ///
    /// **先按窗 drop 上一张**：光标在同一扇窗里走一万下，就会有一万张 136×136 的 RGBA 躺在
    /// 仓库里 —— [`MaskState::mag_seq`] 只保证 URL 每次不同、Qt 才肯重取，不保证旧的被清；
    /// 这一格的 `Drop` 语义由本函数负责。别的窗此刻若有残图，藏在那一窗自己的
    /// `containsMouse = false` 之后，屏幕上看不见，等它自己被下一次 hover 换掉。
    ///
    /// 这一点先夹进**本窗那一扇屏**：跨屏选区被键盘挪掉的那条边可能整个在另一扇屏上，而一块
    /// 面板只能画它脚下这一扇屏的像素。夹住之后本窗报的是「这一扇屏上离那条边最近的一格」，
    /// 坐标那一句、色板、按 `C` 复制的、还有面板摆在哪一格（[`MagnifierData::anchor_x`]）全是
    /// 同一颗 —— 报一颗本窗屏幕上根本没有的像素，页上就会长出一块指不到任何东西的面板。
    ///
    /// 无槽 / 无快照 / 放大镜关 / core 没交出帧 ⇒ [`MagnifierData::show`] 为 `false`，且这一趟
    /// 不留新键；关掉了配置就把这一窗当前那张也顺手 drop，屏幕上放大镜消失得干净。这四条早退
    /// 仍然交出递进来的那一份 `look` 与刚记下那一点的 `anchor`（面板不画 ≠ 面板的颜色变成空串、
    /// ≠ 面板跳回左上角，见 [`MagnifierLook`] 头上那段与 [`MagnifierData::anchor_x`] 头上那段）。
    ///
    /// §5.19.3 的外观为什么是**参数**而不是这一手自己去问 [`crate::settings::appearance_cfg`]：
    /// 「每 hover 问一次」这件时机的事归 [`crate::mask_view::MaskViewRust::updateMagnifier`]
    /// （放大镜那一族的唯一作者），这一手只管搬运、不碰第二把锁，于是单测里手搭的 `MaskState`
    /// 不会撞到 `settings` 那枚 OnceLock 去读盘。`self.capture` 那一格是 `open()` 里取的快照
    /// （一场遮罩共用一份），而外观那四枚要跟着档即时走，两者时刻不同、不能合成一格。
    pub fn magnifier_at_point(
        &mut self,
        index: usize,
        p: PhysPoint,
        look: &MagnifierLook,
    ) -> MagnifierData {
        // 先把上一次发给 Qt 的那张按窗 drop 掉，再决定要不要发新的：即使这一趟什么都没
        // 生成（配置刚关、或指针离开了快照），本窗旧的那张也不该继续留着。
        let prior = self
            .slots
            .iter_mut()
            .find(|s| s.index == index)
            .map(|s| std::mem::take(&mut s.mag_key))
            .unwrap_or_default();
        if !prior.is_empty() {
            shim::drop_frame(&prior);
        }

        // `slot` 是对整个 `self` 的借用，所以下面凡是写 `self` 的三行都排在它用完之后。
        let Some(slot) = self.slot(index) else {
            return MagnifierData {
                look: look.clone(),
                ..Default::default()
            };
        };
        // 夹进本窗那一扇屏。`hi` 取 `right() - 1`：那是那一扇屏**真的有**的最右一列。
        let (l, t, r, b) = slot.bounds.ltrb();
        let keep = |v: i32, lo: i32, hi: i32| v.max(lo).min(hi.max(lo));
        let p = PhysPoint::new(keep(p.x, l, r - 1), keep(p.y, t, b - 1));
        // 面板摆在哪一格 = 这同一点在本窗的读法，走的仍是 [`Self::local_dip`] 那一位作者
        // （与选区、高亮框、文本框、圆角把手同一把尺子；1×1 正是它接的形状）。
        let anchor = self.local_dip(slot, &PhysRect::new(p.x, p.y, 1, 1));
        // 早退之前就先记下：§5.4.2 按 `C` 读的是 [`Self::cursor_phys`]，屏幕上放大镜正指着
        // 这一颗，配置刚关掉的那一趟不许让它复制上一次 hover 的那一颗。
        self.cursor_phys = Some(p);
        let base = MagnifierData {
            anchor_x: anchor.x as f64,
            anchor_y: anchor.y as f64,
            look: look.clone(),
            ..Default::default()
        };
        let Some(snap) = self.snapshot.as_ref() else {
            return base;
        };
        let cfg = &self.capture;
        if !cfg.magnifier_enabled {
            return base;
        }
        let view = snap.magnifier_view(p, cfg);
        let Some(frame) = view.frame.as_ref() else {
            return base;
        };
        // 每次 hover 一个新键：URL 变了 Qt 才会重新拉像素（§3.6 约束 8、`overlay_key`
        // 每画一笔就换键同一族）。`wrapping_add` 而非 `+1`：一场遮罩不会跑到 40 亿次
        // hover，但溢出也不该 panic —— 归零继续加就行，反正键唯一性只要**接下来**的够用。
        self.mag_seq = self.mag_seq.wrapping_add(1);
        let key = format!("mag-{index}-{}", self.mag_seq);
        shim::store_raw(&key, &frame.pixels, frame.width, frame.height);
        if let Some(slot) = self.slots.iter_mut().find(|s| s.index == index) {
            slot.mag_key = key.clone();
        }
        // [`Self::color`] 走 RGB 三字节打包：QML 只要给色板一块 `#AARRGGBB` 就能填，
        // `alpha` 现在永远 255（源像素），单独一格不划算。`None` = 屏幕外（跨屏缝），
        // 桥把这一格留 0、文本留 `""`，页上一格不留旧值（同 §5.3.8 那一族的口径）。
        let rgb = view.color.map(|c| {
            (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2])
        });
        MagnifierData {
            show: true,
            key,
            w: frame.width as i32,
            h: frame.height as i32,
            coord: view.coord_text,
            color: view.color_text,
            rgb: rgb.unwrap_or(0) as i32,
            has_color: rgb.is_some(),
            grid: view.grid,
            pixel_w: cfg.magnifier_radius.max(1) as i32 * 2 + 1,
            zoom: cfg.magnifier_zoom.clamp(1, 64) as i32,
            ..base
        }
    }

    /// §5.4.2：按 `C` 把光标所在那一点的颜色按配置格式复制进剪贴板。
    ///
    /// 读的是 [`Self::cursor_phys`] —— 也就是 [`Self::magnifier_at`] 刚记下的那一点，与
    /// 屏幕上放大镜正在展示的那一颗**同源**。这是 §5.4.2 第 4 步"取当前放大镜位置颜色"的
    /// 形状：放大镜跟着指针走、指针在屏幕上只有一处、取色器读那同一处；两件事共用一本账，
    /// 免得出现"放大镜写着 #FF0000、按 C 复制出 #00FF00"。
    ///
    /// 快照读的是**冻结那一帧**的原始像素（[`ScreenSnapshot::color_at`]），不是屏幕合成结果，
    /// 所以「颜色值应基于原始屏幕像素、不应受截图遮罩影响」结构性成立。字符串由
    /// [`ScreenSnapshot::color_label`] 拼，格式名走 [`Self::capture`] 的 `magnifier_color_format`
    /// —— 与放大镜显示那一行**同一格配置**，§5.19.3 里 HEX/RGB/HSL 三族本来就只有一枚键，
    /// 这里不再新造同义键。
    ///
    /// 返回 `false` 的情形有三种，各自在 [`Self::message`] 上留一句原样：
    /// ① 指针还没在遮罩上动过（`cursor_phys` 是 `None`）；
    /// ② 那一点没落在任何一块屏上（`color_label` 回 `None`，跨屏缝外）；
    /// ③ 剪贴板占用/写入失败（[`platform_windows::clip::usable`] /
    ///   [`platform_windows::clip::write_text`] 报错，把占用者名字递回来）。
    pub fn pick_color(&mut self) -> bool {
        let Some(p) = self.cursor_phys else {
            self.message = "还没有光标：先把指针放到遮罩上，放大镜跟上之后再按 C。".to_string();
            return false;
        };
        let Some(snap) = self.snapshot.as_ref() else {
            self.message = "快照不在了，取不了颜色。".to_string();
            return false;
        };
        let Some(label) = snap.color_label(p, &self.capture.magnifier_color_format) else {
            self.message = format!("({}, {}) 不在任何一块屏上，没颜色可取。", p.x, p.y);
            return false;
        };
        if let Err(e) = platform_windows::clip::usable() {
            self.message = format!("{e}——颜色没复制出去，剪贴板上此刻那份东西没动。");
            return false;
        }
        match platform_windows::clip::write_text(&label) {
            Ok(()) => {
                self.message = format!("已复制颜色 {label}（来自({}, {}) 的原始像素）。", p.x, p.y);
                true
            }
            Err(e) => {
                self.message = format!("{e}——颜色 {label} 没复制出去。");
                false
            }
        }
    }

    /// Arrow keys (§5.3.9). The step is this project's: one physical pixel, ten with
    /// a modifier, and `resize_edge` moves the bottom-right edge instead of the
    /// whole rect. Both helpers clamp to the desktop, which is the rule the PRD
    /// does fix - "不允许选区超出有效截图区域".
    ///
    /// 交回的是**这一趟挪完之后放大镜该采的那一颗**（桌面物理点），而挪本身与它是同一次算的：
    /// `handle` 在这一手里只写一次，既递给 [`nudge_edge`] 说"Alt 那一档挪的是右下这条边"，也
    /// 递给 [`falcon_core::geometry::nudge_sample`] 说"放大镜就去那条边上"。抄第二遍几何就
    /// 是第二个作者，而那两个答案漂移的形状是"框在 A、放大镜在 B"（#281）。
    ///
    /// `None` = 没有选区可挪（`hole` 是空的）。调用方据此**什么也不动**：放大镜留在它此刻那
    /// 一颗上，而不是被藏起来 —— 一次谁也没挪动的按键不该让屏幕上那块面板闪一下。
    pub fn nudge_by(&mut self, dx: i32, dy: i32, resize_edge: bool) -> Option<PhysPoint> {
        if self.hole.is_empty() {
            return None;
        }
        let handle = if resize_edge { Handle::Se } else { Handle::Body };
        let bounds = self.desktop();
        self.set_hole(if handle == Handle::Body {
            nudge(&self.hole, dx, dy, &bounds)
        } else {
            nudge_edge(&self.hole, handle, dx, dy, &bounds)
        });
        // 还没有过一次 hover 时（键盘比指针先动），`cursor` 那一臂取不到东西，`hole.center()`
        // 是本窗唯一一处不会说谎的落点；而 `dx == dy == 0` 那一臂原样交回 `cursor`，也就是
        // §5.4.1「放大镜跟指针走」那件旧事 —— 鼠标那条腿在这格上没有被改动过一个字。
        let cursor = self.cursor_phys.unwrap_or_else(|| self.hole.center());
        Some(nudge_sample(&self.hole, handle, dx, dy, cursor))
    }

    /// §5.3.9 第 4 步「放大镜同步显示边缘像素」在桥里的**那一只手**：一次键盘微调之后，紧接着
    /// 把放大镜重采样到刚被挪掉的那条边，并交出**这一扇窗**的面板全部读数。
    ///
    /// 它是键盘那条腿唯一的产品入口，也是唯一一次把 [`Self::nudge_by`] 的答案递给
    /// [`Self::magnifier_at_point`] 的地方 —— 与鼠标那三处 hover 递的是同一次装配，所以像素、
    /// 坐标那一句、色板、按 `C` 复制的那一串、面板摆在哪一格，五条永远来自同一颗点。
    ///
    /// `None` = 这一趟没有选区可挪，调用方**不动**放大镜那一族（理由在 [`Self::nudge_by`] 头上）。
    ///
    /// 为什么返回值而不是让页上再叫一次 `updateMagnifier`：页上算不出那一条边 —— 它手上只有
    /// `dx`/`dy` 与一枚 handle 序号，而"夹回桌面之后那条边现在在哪一列"是 core 的几何
    /// （[`falcon_core::geometry::nudge_sample`]）。让页上再报一遍指针，正是这一轮要修的缺陷。
    pub fn nudge_magnified(
        &mut self,
        index: usize,
        dx: i32,
        dy: i32,
        resize_edge: bool,
        look: &MagnifierLook,
    ) -> Option<MagnifierData> {
        let p = self.nudge_by(dx, dy, resize_edge)?;
        Some(self.magnifier_at_point(index, p, look))
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
        // §5.3.2：选区清掉了 = 用户又回到「还在挑」，于是第三步的高亮跟着回来（下一次
        // hover 就亮）。不留这一行，Esc 之后整扇窗检测这一路就在这一场里再也亮不起来了，
        // 而 Esc 第一档要的正是"回到没选的状态"。
        self.hover_taken = false;
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
    /// 永远是最后一条被考虑淘汰的。清理失败不许吃掉已经记成的那一行，所以它只改那句话；清理
    /// 真的交走过行时，那本 `history.db` 也跟着缩一次（core 的 `History::compact` 住在
    /// `cleanup` 里面），说的话依旧只添在这一句后面，作者见 [`Self::compaction_said`]。
    /// 清理真的交走过行时，那本 `history.db` 也在这一步跟着缩（core 的 `compact` 住在
    /// `cleanup` 里面，读者就是这一句），说的话依旧只添在这一句后面 —— 三臂怎么说归
    /// [`Self::compaction_said`] 一个人说。
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
                match history.cleanup(&falcon_core::history::Retention::from(&cfg)) {
                    Ok(freed) => {
                        // §6.2 那句「按策略自动清理」交走的行同样把那本文件腾空，所以这一腿也
                        // 说同一句压实的话（作者仍是 [`Self::compaction_said`]）。没顶到任何一枚
                        // 上限时 `Freed` 是空的、`Freed::compacted` 是 `None`，这一句因此平时一个
                        // 字也不添 —— 每次截图都念一遍「库没压实」是吵，不是诚实。
                        self.history_last = format!(
                            "{}{}",
                            self.history_last,
                            Self::compaction_said(&freed)
                        );
                    }
                    Err(e) => {
                        self.history_last =
                            format!("{}，但 §5.14.4 的自动清理没跑成：{e}", self.history_last);
                    }
                }
                // §5.16 那枚 `[advanced] recognize_text` 自本轮起有第一位产品读者：这一行刚写进
                // 库，键开着就多交一条腿出去，把识别回来的文字补进**同一行**的 `ocr_text`。
                // 三条口径与这一族已有的决定同形：① 取消的那一行不交（[`auto_ocr_wanted`] 判）；
                // ② 识别结果面板**不开** —— 这一条腿落的是库而不是屏幕，用户点「文字提取」那条
                // 手动腿（P146）依旧是唯一会开面板的；③ 引擎在另一枚线程上跑，GUI 线程一次也
                // 不等它（P146 给 OCR 立的那一条）。那句话只说「起了一次」而不说「识别出了什么」，
                // 这一格在这条腿上还没有读数可说。
                if auto_ocr_wanted(self.advanced.recognize_text, cancelled) {
                    self.auto_ocr_asked += 1;
                    self.auto_ocr_join = spawn_auto_ocr(frame.clone(), dir, cfg.clone(), id);
                    self.history_last = format!(
                        "{}并按 `[advanced] recognize_text` 起了一次本机识别（不联网，结果落进那一行）。",
                        self.history_last
                    );
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
    ///
    /// `cancelled_only` 是 §5.14.3 步 3「找到标记为“已取消”的记录」里**筛**那一半，它由界面
    /// 递进来而不是这一场自己记着：[`Self::open`] 每开一扇遮罩就把会话那些格子清一遍，把
    /// 「用户勾了什么」寄在那样的格子里，就会有一趟「屏上那枚勾还亮着、清单却回到全量」——
    /// 从历史窗按「标注」或「重进选区」正是那样一趟。那一格产品里没人能核对（它与
    /// [`Self::flip_history_selection`] 拒绝让界面推行号是同一条理由的**反面**：那枚锚点只有
    /// 一个权威，这一枚勾只有界面知道）。两条查询都住在
    /// [`crate::settings::history_rows_at`]，这一手只决定那句话怎么说。
    ///
    /// 筛过之后一行也没有是**第三句**，不并进「库里没有行」那一句：库里可能有五十行，只是没有
    /// 一行被取消过，而这一问答的是「有哪几行被标了“已取消”」。「库里一共几行」它不替谁猜 ——
    /// 那是另一枚数，本函数没问过，编出来就是替用户数了一遍他没法核对的东西。
    pub fn list_history(&mut self, limit: usize, cancelled_only: bool) -> String {
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_list.clear();
            self.history_list_last = "清单没读：这一场没有历史可读。".to_string();
            return String::new();
        };
        let (_epoch, offset_min) = platform_windows::clock::wall_clock();
        match crate::settings::history_rows_at(&dir, &cfg, limit, offset_min, cancelled_only) {
            Ok(rows) => {
                let n = if rows.is_empty() {
                    0
                } else {
                    rows.split('|').count()
                };
                self.history_list = rows.clone();
                // 「只看已取消」那三个字的名不是这里编的（#281）：页上那枚勾选框写的、库里那一
                // 行末尾挂的、与这一句说的是同一串字，作者都在 [`crate::history_window`]。
                let filter = crate::history_window::CANCELLED_FILTER_WORD;
                let word = crate::history_window::CANCELLED_WORD;
                self.history_list_last = match (cancelled_only, n) {
                    (true, 0) => format!(
                        "读了 {dir:?} 里的历史清单（{filter}）：0 行 —— 这一问只找行末写着「{word}」\
                         的那些行，没被筛进来的行它不说，也没数过库里一共几行。"
                    ),
                    (true, _) => format!("读了 {dir:?} 里的历史清单（{filter}）：{n} 行。"),
                    (false, _) => format!("读了 {dir:?} 里的历史清单：{n} 行。"),
                };
                rows
            }
            Err(why) => {
                self.history_list.clear();
                self.history_list_last = format!("清单没读：{why}。");
                String::new()
            }
        }
    }

    /// §7.1「历史记录加载应支持分页或虚拟列表」里**更旧那一页**的产品腿：清单已经显示了
    /// 第一页（[`Self::list_history`]），用户滚到尾巴上就要问「比最后一行更旧的那些呢」。
    ///
    /// 与 `list_history` 的分别只在两处：**累计**而不覆盖 [`Self::history_list`]（那一格记的
    /// 是「上一次读回来的快照」，删与清那几条腿都按它对齐屏上，这一页既然接在屏上那条清单
    /// 尾巴上，它就得跟着长 —— 另起一格就是给「屏上此刻有哪些行」第二个作者 #281）；游标不
    /// 在库里时那句拒绝**原样**递进 [`Self::history_list_last`]，因为「到底了」与「你手里那
    /// 一行已经不在了」是两件事（#453），而屏上那一行确实还是旧的。
    ///
    /// 落点、`[history]` 快照与时间偏移的取法与 [`Self::list_history`] 同一份：每次读都问一次
    /// [`platform_windows::clock::wall_clock`]，不在别处养第二个主人。
    ///
    /// `cancelled_only` 必须与**第一页那一次**同值 —— 界面在两记调用里递的是同一枚勾
    /// （`HistoryWindow.qml` 的 `win.cancelledOnly`）。筛着开第一页、不筛着走尾巴，屏上那条
    /// 清单就一半是筛过的、一半是全量，而下面那句「到底了」随之成了一句猜。
    ///
    /// **筛开着时的「到底」说的是这张筛过的清单到底了**（§5.14.3 步 3 与 §7.1 的交点）：core
    /// 那一枚 `cancelled_page` 的 `LIMIT` 数的是**筛过之后**的行，所以「比第 N 行更旧的已取消
    /// 记录没有了」是 SQLite 答出来的，不是「这一页恰好全被我筛掉了」。页上因此可以在筛着的
    /// 时候停下不再问，不必一记一记翻下去找那些永远数不满一页的空档 —— 那既是一台轮询机，
    /// 又会把「到底了」报在还有更旧的已取消行时。
    pub fn list_history_older(&mut self, cursor: i64, limit: usize, cancelled_only: bool) -> String {
        let (Some(cfg), Some(dir)) = (
            self.history.clone(),
            (!self.history_dir.as_os_str().is_empty()).then(|| self.history_dir.clone()),
        ) else {
            self.history_list_last = "清单没读：这一场没有历史可读。".to_string();
            return String::new();
        };
        let (_epoch, offset_min) = platform_windows::clock::wall_clock();
        match crate::settings::history_rows_older_at(&dir, &cfg, cursor, limit, offset_min, cancelled_only)
        {
            Ok(rows) => {
                let n = if rows.is_empty() {
                    0
                } else {
                    rows.split('|').count()
                };
                if !rows.is_empty() {
                    if self.history_list.is_empty() {
                        self.history_list = rows.clone();
                    } else {
                        self.history_list.push('|');
                        self.history_list.push_str(&rows);
                    }
                }
                let total = if self.history_list.is_empty() {
                    0
                } else {
                    self.history_list.split('|').count()
                };
                let filter = crate::history_window::CANCELLED_FILTER_WORD;
                let word = crate::history_window::CANCELLED_WORD;
                self.history_list_last = if n == 0 {
                    if cancelled_only {
                        format!(
                            "读了 {dir:?}：比第 {cursor} 行更旧的「{word}」记录没有了，按「{filter}」\
                             这张清单到底了（累计 {total} 行）—— 没被筛进来的那些行不在这一问里。"
                        )
                    } else {
                        format!("读了 {dir:?}：比第 {cursor} 行更旧的一页是空的，清单到底了（累计 {total} 行）。")
                    }
                } else if cancelled_only {
                    format!("读了 {dir:?} 里更旧的一页（{filter}）：{n} 行（累计 {total} 行）。")
                } else {
                    format!("读了 {dir:?} 里更旧的一页：{n} 行（累计 {total} 行）。")
                };
                rows
            }
            Err(why) => {
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

    /// §6.2 存储规则里 core `History::compact` 那一步（`crates/core/src/history.rs` 里那扇门）
    /// 在三条删除腿上**怎么说**的唯一作者：删行、清空、自动清理各说各的句子，而压实这一格
    /// 的话在三条里是同一句 —— 所以这里排一次，三处各 append 一次。
    ///
    /// `Skipped` 是**沉默**：那一次删除本来就不该重写整本文件，无话可说才是实话，说一句
    /// 「这次没给你压实」反倒让用户以为还有别的一件事要办。`Done` 报的是那本文件自己省下的
    /// 字节，与 [`falcon_core::history::Freed::bytes`]（删掉的那些**文件**与缩略图 BLOB）
    /// 两笔独立的账 —— §5.14.4 步 4「系统释放相应磁盘空间」说的正是这两笔合起来。
    ///
    /// `Refused` 只添话、**不动返回值**：core 把压实失败包在 `Freed::compacted` 里交回来，
    /// 就是因为删除已经成了；把这一臂读成一次失败的删除，等于替用户把已经没了的那一行说回来。
    fn compaction_said(freed: &falcon_core::history::Freed) -> String {
        use falcon_core::history::Compacted;
        match freed.compacted {
            Some(Compacted::Done { reclaimed, .. }) => {
                format!("并按 §6.2 压实了 history.db，那本文件省下 {reclaimed} 字节。")
            }
            Some(Compacted::Refused { ref reason }) => {
                format!("但 §6.2 那一步 history.db 没压实：{reason}")
            }
            Some(Compacted::Skipped { .. }) | None => String::new(),
        }
    }

    /// §5.14.4「选择单条、多条记录 → 点击删除 → 系统释放相应磁盘空间」的第一条**产品**腿，
    /// P99 起。[`falcon_core::history::History::delete`] 自 §6.2 建库那天起只有单测读者：
    /// 写侧四轮（P93/P95/P96）与读侧两轮（P97/P98）之外，库里那一行的**去**一次也没发生过。
    ///
    /// 这一格要的记账面比"库里少一行"硬：§5.14.4 step 4 那句"释放相应磁盘空间"对面是文件系统，
    /// 所以 [`falcon_core::history::Freed`] 交回来的那些字段（哪些行、多少字节、以及那本
    /// `history.db` 自己这一格）原样交回，本条一处也不替它估 —— 字节是 `bytes_of` 现场问那份
    /// 文件与那一行的缩略图 BLOB 加起来得来的，而 `History::delete` 顺手跑的 `prune_days` 会把
    /// 删空的日期文件夹也收走（那是 §5.14.4 自己说的话，不是本条加的）。压实那一格的读者是
    /// [`Self::compaction_said`]：删完行之后 `history.db` **这枚文件**也跟着下来，才是
    /// §6.2 存储规则那句话的完整版（`Freed::bytes` 一个人说不到那本库身上）。
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
                // §6.2 那一步压实的话跟在删除那一句后面：`Skipped` 一个字也不添，`Refused`
                // 也只添话 —— 上面那个 `Some(freed)` 交回的还是那次成功的删除。
                self.history_delete_last
                    .push_str(&Self::compaction_said(&freed));
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
                // 同一句作者，同一格理由：`clear_all` 走的正是 `History::delete`，那本文件
                // 跟着下来的那一格因此也在这里说话；一行也没删成（剩下全是锁定项）时
                // `Freed::compacted` 是 `None`，这里自然也无话可说。
                self.history_clear_last
                    .push_str(&Self::compaction_said(&freed));
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
    /// 归 [`Self::clear_history`]，而**打上那一枚锁**这半在 P99 之前没有产品腿 ——
    /// [`falcon_core::history::History::set_locked`] 从 P89 建库起到那时只有 core 自己的单测和量具
    /// 的**前置**在调（P99 第 22 格为此当场开了一次库），界面递不出「收藏这一行」这个动作。
    /// 本条是那半第一次有个能被界面按的地方，P100 起它的读者就是 `qml/HistoryWindow.qml`
    /// 每一行右边那枚「锁上 / 解开」。
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

    /// 把 `[history]` 的会话快照在**没有遮罩**的时候也立起来。
    ///
    /// 为什么需要这一格：窗上那些腿（[`Self::list_history`] / [`Self::replay_history`] /
    /// [`Self::copy_history_content`] …）问的都是 [`Self::history_store`] 这一对，而它自
    /// [`Self::open`] 起才有值。于是一次截图也没跑过的新进程里，用户从托盘点开「历史记录管理」
    /// 再按下那四枚钮，全体答的是「这一场没有历史可读」——那句话说的是"进程刚起"，不是用户
    /// 问的那件事。
    ///
    /// 取的是与 [`Self::open`] 那两行**同一份**读法（`settings::history_cfg()` +
    /// `history_data_dir()`），本条不另定一套口径；已经立着就一动不动，否则一次窗上的刷新会把
    /// 量具钉进沙盒的那一对（`history_window::retarget`）踩回全局，删掉的就是用户真的历史。
    pub fn arm_history_store(&mut self) {
        if self.history_store().is_none() {
            self.history = Some(crate::settings::history_cfg());
            self.history_dir = crate::settings::history_data_dir();
        }
        // 命名那一半同场补上：§5.14.2 step 4 的「贴图」要先从那一行落一份文件到盘上
        // （[`Self::replay_history`]），而它写的目录取的就是这一格。空目录说的是"这一场没有
        // 落点"，用户读到的却是"那一行没处可写"——那是同一句谎的两种说法。已经立着同样不动。
        if self.naming.dir.is_empty() {
            self.naming = crate::settings::naming();
        }
    }

    /// 只读地问一句「历史第 `id` 行当初框住的是哪一块」。
    ///
    /// 本函数**不**说话：它没有自己那句话可说。「库打不开」「没有第 N 行」「那一行的选区是空的」
    /// 在调用者嘴里是三件事还是同一件，只有调用者知道自己是谁 —— [`Self::restore_history_selection`]
    /// 有它自己的五格，[`Self::pin_history_row`] 只在一位已经读过同一行之后才问它。把话写在这里
    /// 就会多出第六只说错的那一张嘴。
    fn row_rect(&self, id: i64) -> Option<falcon_core::PhysRect> {
        let (dir, cfg) = self.history_store()?;
        let history = crate::settings::history_open_in(&dir, &cfg).ok()??;
        let rec = history.get(id).ok()?;
        let rect = rec?.rect;
        (!rect.is_empty()).then_some(rect)
    }

    /// §5.14.2 step 4「贴图」那一半的产品腿。走的是 [`Self::save_then_pin`] 立的口径：图先落到
    /// 盘上（[`Self::replay_history`]，命名规则用的是用户自己那一份），贴图**从那个文件**钉起来
    /// （[`crate::state::PinState::add_from_file`]），落点是那一行当初框住的那块地方、整块搬回
    /// 今天的桌面内（[`falcon_core::geometry::PhysRect::fitted_into`]）—— 一块已经不在今天布局上
    /// 的原选区，也要钉成伸手够得到的一枚。
    ///
    /// 不复用 [`Self::save_then_pin`] 本体：那一扇要的是**当前选区**的 `hole` 与
    /// [`Self::delivered`]，而这一行要的是库里那张图，两件事。也不碰 [`Self::history_viewed`]、
    /// 不加 [`Self::history_restores`]：一次贴图不该改界面停在第几行（与
    /// [`Self::restore_history_selection`] 不复用 [`Self::history_last_id`] 同一条理由）。
    pub fn pin_history_row(&mut self, id: i64) -> bool {
        let Some(path) = self.replay_history(id) else {
            self.history_pin_last = self.history_replay_last.clone();
            return false;
        };
        // 落盘那一句原样带过来：它说的是"这一行已经在了"，而这一格要多说的只是"它现在也飘着"。
        let said = self.history_replay_last.clone();
        let Some(rect) = self.row_rect(id) else {
            self.history_pin_last = format!(
                "{said}但贴图没有钉出来：第 {id} 行当初框住的那一块读不回来（库里没有那一行，或那一行记下的是空的）。"
            );
            return false;
        };
        let at = rect.fitted_into(&self.virtual_bounds);
        match crate::state::with(|s| s.add_from_file(&path.to_string_lossy(), PhysPoint::new(at.x, at.y))) {
            Ok(pin) => {
                self.history_pin_last = format!("{said}也钉成了贴图 {pin}。");
                true
            }
            // 文件那半句留着：写成了是真的，把这一格压成一句"没钉出来"会让人重做整件事。
            Err(e) => {
                self.history_pin_last = format!("{said}但贴图没有钉出来：{e}");
                false
            }
        }
    }

    /// §5.14.2 step 4「标注」的输入腿，也是 §5.14.3 step 4「恢复该截图**内容**」唯一把内容放回
    /// 屏幕上的那一腿。四步的**次序就是这一格的全部风险**：
    ///
    /// 冻一帧（[`crate::capture::freeze`]，几何那把尺子只能从这儿来，理由写在 [`open_board`]
    /// 头上）→ 把那一行的图像进它原选区的位置（[`falcon_core::history::stamp_row`]，纯函数，
    /// core 单测钉住落点/跨屏/尺寸三件事）→ 开遮罩（[`Self::open`]，与热键那一路同一扇门，纹理
    /// 就是刚铺好的那份）→ 把洞交回那一行的原选区（[`Self::restore_history_selection`]）。
    ///
    /// 铺像素必须在 `open` **之前**、交回选区必须在它**之后**：`open` 既重铺纹理、又把 `hole`
    /// reset 成整屏默认。而 `open` 还会从全局 `[history]` 重取快照，所以它在读完那一行之后被
    /// 按回**读取时用的那一对** —— 行号只在一座库里有意义，留着 `open` 那份，下面那只手就会去
    /// 另一座库里找同一个号（量具那一档把这一对钉进进程私有的沙盒，正是为这一趟）。
    ///
    /// 交回选区失败时遮罩**已经开着**（整屏默认那一框），本条不偷偷关掉它：关掉要走
    /// [`Self::escape`]，而那会往历史里再记一行「已取消」—— 一次失败的标注就这样长出了两条历史。
    pub fn annotate_history_row(&mut self, id: i64, shader: bool) -> bool {
        let Some((dir, cfg)) = self.history_store() else {
            self.history_annotate_last =
                format!("标注没跑：这一场没有历史可读，第 {id} 行的画面铺不出来。");
            return false;
        };
        let history = match crate::settings::history_open_in(&dir, &cfg) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.history_annotate_last = format!(
                    "标注没跑：`[history] enabled` 是关的，第 {id} 行没有可读的图。"
                );
                return false;
            }
            Err(e) => {
                self.history_annotate_last =
                    format!("标注没跑：打不开 {dir:?} 里的 history.db（{e}）。");
                return false;
            }
        };
        let rec = match history.get(id) {
            Ok(Some(rec)) => rec,
            Ok(None) => {
                self.history_annotate_last =
                    format!("标注没跑：{dir:?} 的库里没有第 {id} 行。");
                return false;
            }
            Err(e) => {
                self.history_annotate_last =
                    format!("标注没跑：第 {id} 行读不回来（{e}）。");
                return false;
            }
        };
        if rec.rect.is_empty() {
            self.history_annotate_last = format!(
                "标注没跑：第 {id} 行记的原选区是空的，铺过去不知道要铺在哪一块。"
            );
            return false;
        }
        let frame = match history.frame(&rec) {
            Ok(frame) => frame,
            Err(e) => {
                self.history_annotate_last = format!(
                    "标注没跑：第 {id} 行的图取不回来（{e}），库里那一行还在。"
                );
                return false;
            }
        };
        let frozen = match capture::freeze() {
            Ok(frozen) => frozen,
            Err(e) => {
                self.history_annotate_last =
                    format!("标注没跑：画面冻不下来（{e}），遮罩没开。");
                return false;
            }
        };
        let mut snap = frozen.snap;
        if let Err(e) = falcon_core::history::stamp_row(&mut snap, &rec.rect, &frame) {
            self.history_annotate_last = format!(
                "标注没跑：第 {id} 行的画面铺不到那块桌面上（{e}），遮罩没开。"
            );
            return false;
        }
        let slots = match self.open(&snap, shader) {
            Ok(slots) => slots,
            Err(e) => {
                self.history_annotate_last = format!("标注没跑：遮罩开不起来（{e}）。");
                return false;
            }
        };
        self.history = Some(cfg);
        self.history_dir = dir;
        self.history_annotate_last = match self.restore_history_selection(id) {
            Some(rect) => format!(
                "标注从历史第 {id} 行起：{}×{} 的画面铺回 {rect}，遮罩开了 {slots} 屏。",
                frame.width, frame.height
            ),
            None => format!(
                "标注从历史第 {id} 行起：画面铺好了、遮罩开了 {slots} 屏，但那一行的原选区没交回来（{}）——屏幕上现在是整屏默认那一框，Esc 出得去。",
                self.history_restore_last
            ),
        };
        true
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
    ///
    /// 第四道梯之后才有分岔：[`falcon_core::config::Output::copy_as_file`]（§5.9.17 步 1 那一枚
    /// 开关，「复制图片时同时提供文件」）管的是**所有**复制图片的手，不只贴图菜单那一扇 —— 这一扇
    /// 是 §5.5.2 步 3 那一按，图上飘的是选区而不是贴图。开着 ⇒ 同一张图（[`Self::delivered`]
    /// 那一枚，与关着时**同一手**拼的）进
    /// [`crate::state::PinState::copy_frame_with_files`] 那架梯子，句子里多点名一次落盘的临时
    /// 文件；关着 ⇒ 与 §5.9.16 一直的读法逐字同形。开关每按一次读一次，与
    /// [`crate::session::Session::copy_pin`] 同一族：两处各读各的当下，不在页面里另立第二把尺。
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
        if crate::settings::output_cfg().copy_as_file {
            return match crate::state::with(|s| {
                s.copy_frame_with_files(&frame, crate::state::ClipDoor::Selection)
            }) {
                Ok((_, path)) => {
                    self.message = format!(
                        "已复制 {}×{} 像素，文件引用 {}",
                        frame.width,
                        frame.height,
                        path.display()
                    );
                    true
                }
                // 梯子交回来的已经是一整句（它知道临时文件有没有被收回），这里不再套一层
                // 「{e}。截图还在」—— 那一层会把同一枚规则说两遍。
                Err(e) => {
                    self.message = e;
                    false
                }
            };
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
    ///
    /// 开关那一岔也与 `copy_out` 同一处读、同一架梯子（§5.9.17 步 1 说的是「复制图片」，而这一扇
    /// 正在复制图片），差别只在递进去的那扇门是 [`crate::state::ClipDoor::AutoSelection`]：
    /// 梯子那句「写不出去时什么还在」因此不说「可以再复制一次」—— 这一趟没人按过复制，替用户
    /// 安排一次他要做的手指，是 §5.5.3 那句"两份证据同时在场"里多出来的一句假话。
    fn auto_copy_out(&mut self) -> bool {
        let Some(frame) = self.delivered() else {
            self.auto_copy_last = "选区的画面拼不出来：默认行为没递任何东西。".to_string();
            return false;
        };
        if let Err(e) = platform_windows::clip::usable() {
            self.auto_copy_last = format!("{e}——默认行为没递东西，截图照旧在。");
            return false;
        }
        if crate::settings::output_cfg().copy_as_file {
            let done = crate::state::with(|s| {
                s.copy_frame_with_files(&frame, crate::state::ClipDoor::AutoSelection)
            });
            match done {
                Ok((_, path)) => {
                    self.auto_copies += 1;
                    self.auto_copy_last = format!(
                        "已按默认行为复制 {}×{} 像素，文件引用 {}",
                        frame.width,
                        frame.height,
                        path.display()
                    );
                    return true;
                }
                Err(e) => {
                    self.auto_copy_last = e;
                    return false;
                }
            }
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
                self.message = format!("{e}。截图还在，{}", crate::save::SAVE_EXITS);
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
                        self.message = format!("{e}。截图还在，{}", crate::save::SAVE_EXITS);
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
                self.message = format!("{e}。截图还在，{}", crate::save::SAVE_EXITS);
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

    /// §5.5.9 的替代分支：本产品**没有**原生分享面板，分享因此由「用系统默认程序打开」承担。
    ///
    /// 那一臂按 2026-11-02 的划界不做（Windows 的原生分享是 WinRT 的 `DataTransferManager`，
    /// 它要应用以包身份运行，而本程序是一枚未打包 exe）。PRD 那一节允许的两条分支里，
    /// 「隐藏入口」会把 `[output] share_targets` 那批条目也一起藏掉，走的是另一条：
    /// **先把这一张写成一份文件，再把那枚路径交给 shell** —— 那只手早在
    /// [`platform_windows::open::file`] 上长好了，本轮之前它是零 caller 的一只手。
    ///
    /// 三道梯，每一道只答自己那一问：
    ///  1. 落盘走 [`Self::quick_out`] 那一道唯一缝合：`[output] naming_template` / `collision`
    ///     / `format` 三条规则不在这里第二次拼（P35 那一族「两条路对一张图说两句话」）。它自己
    ///     那三道拒绝（没有选区 / 拼不出图 / 没有目录可写）原样递回用户，这一格不替它补第二句。
    ///  2. **每一次按都新写一份**，不复用 [`Self::landed`] 里那一份。理由是像素而不是磁盘：
    ///     `landed` 可能是用户改标注**之前**的画面，递一张旧图给对面程序比多留一枚文件坏得多
    ///     （与 [`Self::send_to_target`] 每次派发都现写一张同一读法）。
    ///  3. 话是**接**在 `quick_out` 那句之后的，不是替换掉它：那句「已保存到 …（选区 800×600
    ///     像素）」是这份文件存在的证据，替换掉就等于把路径从用户眼前收走 —— 而 shell 收下请求
    ///     之后本进程举不到对面那一扇窗，能交给用户的只剩这一行。
    ///
    /// `Ok` 只到 shell 收下这条请求为止（与 [`platform_windows::open::browser`] 同一条限界：
    /// 对面那个进程归它自己管，我们没资格说页面已经打开）。
    ///
    /// `[output] reveal_after_save` 开着时，这一次按会**同时**亮出目录与打开图片：那句「每次
    /// 保存后揭示文件」是用户对每一次保存立的规矩，分享这一步走的正是保存那一腿，本轮不给它
    /// 开例外 —— 一柄设置被两枚钮各自解释，就是那枚键再也不说话的那天。
    pub fn share_out(&mut self) -> bool {
        if !self.quick_out() {
            return false;
        }
        let Some(path) = self.landed.clone() else {
            // `quick_out` 交回 `true` 就一定先把路径写进了 `landed`（它成功那两臂都是这个次序）。
            // 真到了这一格是它食言，而那句话该由它自己说：这里不替它编第二句，也不动它那句。
            return false;
        };
        match platform_windows::open::file(&path.to_string_lossy()) {
            Ok(()) => {
                self.message.push_str(
                    "已把这份文件交给系统的默认程序：它开在哪一扇、什么时候开好，本进程举不到。",
                );
                true
            }
            Err(e) => {
                self.message
                    .push_str(&format!("{e}。那份文件还在盘上，可以再分享一次。"));
                false
            }
        }
    }

    /// PRD 491 step 4 的那一次点击：通知上按下去，把刚存的那张图在它自己的目录里亮出来。
    ///
    /// 计数在**动手之前**加，不在成功之后加：这一格要判的是"点了几下"与"递出去几次"
    /// 是不是同一个数，而一个失败了却不计数的点击会把两次真点击读成一次。
    pub fn reveal_landed(&mut self) -> bool {
        self.reveal_clicks += 1;
        self.reveal_now()
    }

    /// §5.5.10 的临时目录：OS temp 下我们自己的那一枚标签。标签是"收回"这件事安全的唯一理由 ——
    /// [`Self::send_to_landed`] 删得着的只可能是这个目录里的文件。
    ///
    /// 为什么不复用 [`crate::state::PinState::clip_dir`]：那一枚目录里每张文件的死活是**问剪贴板**
    /// 问出来的（`sweep_clip_files`），而派发出去的这一张没有任何一枚剪贴板引用会指向它 ——
    /// 放进去等于把它交给一条不属于它的清理规则。§6.2 第 1600-1601 行那两句（「临时文件应按策略
    /// 自动清理」与「应避免在退出时删除仍被剪贴板引用的文件」）在两处各按各自那一问执行。
    ///
    /// `pub(crate)` 是给门的，与 `clip_dir` 当初从 `pub(super)` 放开到 `pub(crate)` 同一条理由：
    /// `sendto_check` 要判「递出去的那一枚在不在我们自己这一枚目录下」，用的是这一枚函数而不是
    /// 在档头再抄一遍 `%TEMP%\falconshot-sendto`。
    pub(crate) fn sendto_dir() -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push("falconshot-sendto");
        dir
    }

    /// §5.5.10 步骤 3：把这一次的图片作为**一条文件参数**交给第 `index` 枚目标应用。
    ///
    /// 六道梯，每一道只答自己那一问，而**判**那一道不写在本案：
    ///   1. 计数先加 —— 第 46 项 ③ 那条口径（一次点击恰好一次，与成没成无关）；
    ///   2. 手里有没有东西：[`Self::has_selection`] 与 [`Self::delivered`]，后者仍是全仓库
    ///      唯一一处缝合，所以递出去的那张与 复制/保存 交的是同一张图；
    ///   3. 清单上还有没有第 `index` 枚；
    ///   4. 落盘：文件名与字节都交给 [`crate::save::quick_to`]，也就是 `[output]
    ///      naming_template` / `collision` / `format` 那三条既有规则的唯一读者。本模块不自己
    ///      拼名字 —— 拆成两只手就是 P35 那一族"两条路对一张图说两句话"的反面；
    ///   5. 判：三臂择一、目标在不在这台机器上、参数加不加引号，全在
    ///      [`falcon_core::sendto::plan`]，连拒绝的那一句也是它拼的（`Repath::prompt`）；
    ///   6. 递：[`platform_windows::open::dispatch`] 那只手只搬运，不第二次判。
    ///
    /// 两处次序是**故意**的，写在这里而不是藏起来：
    ///   * 先落盘后问对面在不在 —— `quick_to` 是"决定文件名"与"写字节"的同一只手，想先问目标
    ///     就得把命名规则再算一遍。代价是 `Repath` 那一臂会白写一张图，于是那一臂顺手把它收回
    ///     并把收回说进提示里（与 §5.9.17「剪贴板没接住就收回临时文件」同一枚形状）。
    ///   * 上一枚派发的文件在**这一次真递成功之后**才收回 —— 一次失败的发送没有资格顺手销毁
    ///     上一次发送的证据（对面的进程也许正开着它）。
    pub fn send_to_target(&mut self, index: usize) -> bool {
        self.send_to_clicks += 1;
        if !self.has_selection() {
            self.send_to_last = "还没有选区：先把要发送的范围拖出来。".to_string();
            return false;
        }
        let Some(frame) = self.delivered() else {
            self.send_to_last = "选区的画面拼不出来：截图还在，可以再发送一次。".to_string();
            return false;
        };
        let (final_ok, _, _) =
            crate::settings::pipeline_send_to(&crate::settings::output_cfg().send_to_apps);
        let Some((name, exe)) = final_ok.get(index).cloned() else {
            // 序号越界只在"清单在两次读之间变了"时到得了：芯片那串名字是工具条绑定时读的那一份，
            // 而这一枚数是从此刻这份清单里数的。报出**这次**读到的枚数，是因为用户要知道自己
            // 按到的是第几枚、而清单现在还剩几枚 —— 至于"重新截一次"那句，它不给任何一条本轮
            // 还没建的控件（工具条不会自己重读那份清单），所以话里只能说清此刻还剩几枚。
            self.send_to_last = format!(
                "这一枚目标已经不在这份清单上了：此刻清单里只剩 {} 枚，请重新截一次图再选。",
                final_ok.len()
            );
            return false;
        };
        let dir = Self::sendto_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.send_to_last = format!("发送用的临时目录打不开：{e}");
            return false;
        }
        let (epoch, offset_min) = platform_windows::clock::wall_clock();
        let facts = naming::NameFacts {
            width: frame.width,
            height: frame.height,
            monitor: "",
            tool: self.ink.tool().map(|kind| kind.key()).unwrap_or(""),
        };
        let quick = crate::save::Quick {
            dir: &dir,
            template: &self.naming.template,
            prefix: &self.naming.prefix,
            seq: self.send_to_writes + 1,
            collision: &self.naming.collision,
        };
        let path = match crate::save::quick_to(
            &frame,
            naming::LocalStamp::from_epoch(epoch, offset_min),
            facts,
            &quick,
            &self.export,
            "选区",
        ) {
            Ok((landed, _said)) => landed,
            Err(crate::save::Refusal::BadName(e)) => {
                self.send_to_last = e;
                return false;
            }
            Err(crate::save::Refusal::Write(e)) => {
                self.send_to_last = format!("{e}。截图还在，可以再发送一次。");
                return false;
            }
        };
        let spelled = path.to_string_lossy().to_string();
        match falcon_core::sendto::plan(&name, &exe, &spelled) {
            falcon_core::sendto::Delivery::Dispatch {
                arm,
                exe: used,
                parameters,
                path: handed,
            } => match platform_windows::open::dispatch(&used, &parameters) {
                Ok(()) => {
                    self.send_to_acted += 1;
                    self.send_to_writes += 1;
                    if let Some(old) = self.send_to_landed.as_ref() {
                        if old != &path {
                            let _ = std::fs::remove_file(old);
                        }
                    }
                    self.send_to_landed = Some(path);
                    self.send_to_last = format!(
                        "已按{}把 {}×{} 那张交给 {}（{used}）：{handed}。",
                        arm.label(),
                        frame.width,
                        frame.height,
                        name,
                    );
                    true
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&path);
                    self.send_to_last = format!(
                        "{e}。刚写的那张临时文件（{}）已收回，截图还在，可以再发送一次。",
                        path.display()
                    );
                    false
                }
            },
            falcon_core::sendto::Delivery::Repath { prompt, .. } => {
                let _ = std::fs::remove_file(&path);
                self.send_to_last = format!("{prompt}刚写的那张临时文件已收回。");
                false
            }
            falcon_core::sendto::Delivery::Refused { why, .. } => {
                let _ = std::fs::remove_file(&path);
                self.send_to_last = format!("{why}刚写的那张临时文件已收回。");
                false
            }
        }
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
                self.message = format!("{e}。截图还在，{}", crate::save::SAVE_EXITS);
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

    /// §5.8.1 步 2 的那一枚「贴图」钮：选区直接成为一扇浮窗，全程不碰盘。
    ///
    /// 它是 [`Self::save_then_pin`] 减掉落盘那一步，而那一步正是这一节存在的理由：PRD 433 的
    /// 「完成后可复制、保存**或**贴图」是三扇出口，第三扇被要的是一枚贴图 —— 先要用户交出一个
    /// 目录、一个名字和一份他没打算要的文件，等于把第二扇出口挂着第三扇的牌子再让人走一遍
    /// （§10 第 43 项 ① 说的是那一件，它有自己的那枚钮，本档不动它）。
    ///
    /// 用的是 [`crate::state::PinState::add`] 而不是 [`crate::state::PinState::add_captured`]：
    /// 后者是 [`falcon_core::pin::PinItem::captured_at`] 唯一的写入者，记的是「这些像素来自屏幕上
    /// 这一块」，而这一格的像素**不是**从 `at` 那块读回来的 —— 它是标注拼出来的那一张。认下那块
    /// 原坐标，§5.5.8 的「刷新」就会拿当下的活屏幕盖掉用户画的箭头与文字，那是这一扇门唯一能
    /// 悄悄毁掉自己刚交出去的那张图的方式。留 `None` 时同一枚键回答的是
    /// [`falcon_core::refresh::plan`] 那句「不是从屏幕上截下来的：请重新截一次」，是真话，而且
    /// 照样给用户一条路（重截）。
    ///
    /// 落点算法与 `save_then_pin` 同一份，理由写在那一格头顶：§5.5.7 的阴影把画布往**外**长，
    /// 所以图片自己的那一角要先往外挪 [`falcon_core::imageops::Decoration::extra`]，画面才停在
    /// 用户拖出来的那块矩形上，并且卡在桌面的左上角为止。
    ///
    /// 和 复制/保存 一样不收这一层：话只活在 [`Self::message`] 这一格里，两扇窗动手之后各重读一次。
    pub fn pin_selection(&mut self) -> bool {
        if !self.has_selection() {
            self.message = "还没有选区：先把要贴的范围拖出来。".to_string();
            return false;
        }
        let Some(frame) = self.delivered() else {
            self.message = "选区的画面拼不出来：截图还在，可以再钉一次。".to_string();
            return false;
        };
        let grow = self.decor.extra() as i32;
        let at = PhysPoint::new(
            (self.hole.x - grow).max(self.virtual_bounds.x),
            (self.hole.y - grow).max(self.virtual_bounds.y),
        );
        match crate::state::with(|s| s.add(frame, at)) {
            Ok(id) => {
                self.message = format!("贴成了贴图 {id}：这一张没有写成文件，只在屏幕上。");
                true
            }
            Err(e) => {
                self.message = format!("贴图没有钉出来：{e}");
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

    /// §5.7.16 步 3 的「文字内容」的入口：把页上那一枚 `TextEdit` 就地摊到选中的那一枚
    /// 文本 上（F2 那一枚键，与 [`Self::finish_ink`] 收尾的是同一只框）。这一层只递问题：
    /// 哪一枚改得、框开着不许叠第二只、手里是笔还是箭头，都在
    /// [`crate::annotate::Layer::edit_selected_text`] 那一位作者那儿 —— 在页上写第二套判据
    /// 就是两条会各说一半的梯子。
    ///
    /// 门没开时屏幕上要说一句话（#433）：这一枚键是用户按的，而按下去没有反应的话，「为什么
    /// 没反应」有四种答案，页上一格也说不出哪一种是这一次的 —— 那一句由这里给，四条都归它，
    /// 因为这一层分不清是哪一条拦的，硬要分就得在这里重问一遍上面那一位作者的问题。
    ///
    /// `true` = 框开了；调用者要把这一趟递到别的窗上去（`win.grabbed()`），因为那一枚字段
    /// 该在哪一扇亮、里面预先放着哪一串，都是这同一份样本里的两格。
    pub fn edit_text(&mut self) -> bool {
        if self.ink.edit_selected_text() {
            return true;
        }
        self.message = "改字没开门：先用「选择」工具点中一枚文字标注，再按 F2（锁着的、多选与编号都不在这一格里改）。".to_string();
        false
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
        // 圆角那一枚把手与上面那个文本框走同一条腿：位置是 core 算的，这里只做本窗那一次除法，
        // 而「不在这扇窗里」的那一支答案是 `None` —— 一台双屏的遮罩上，把手画在握不到的那一只
        // 窗口上，比不画更难解释。
        let radius = self
            .ink
            .radius_handle()
            .filter(|p| slot.bounds.contains(*p))
            .map(|p| self.local_dip(slot, &PhysRect::new(p.x, p.y, 1, 1)));
        // §5.3.7 那对读数在这一趟算，与下面 `hole`／`scale` 是同一份样本：`size_shown()` 读的
        // 是 `self.hole` 与 `self.unit`，分两次问就会出现"数字是这一趟的、单位是上一趟的"。
        let shown = self.size_shown();
        // §5.3.2 第三步的高亮框与上面 `hole`／`scale` 是**同一份样本**：它读的是
        // [`Self::hover_win`] 与这一格槽位的缩放，分两次问就会出现"框是这一趟的、缩放是
        // 上一趟的"那一族（P162 记过的"绑过一次就再也更不动的数字"同族）。
        let hover = self.hover_bounds(index);
        MaskViewData {
            live: true,
            shown: self.shown,
            geom: slot.geom,
            hole: self.hole_for(slot),
            hover: hover.unwrap_or_default(),
            hover_live: hover.is_some(),
            scale: slot.scale,
            typing: typing.unwrap_or_default(),
            typing_live,
            font_dip: slot
                .scale
                .phys_to_dip_i(self.ink.typing_font_px() as i32)
                .max(1) as u32,
            radius: radius.unwrap_or_default(),
            radius_live: radius.is_some(),
            revision: slot.revision,
            swaps: slot.swaps,
            first_swap_ms: slot.first_swap_ms,
            shader_status: slot.shader_status,
            show_dimensions: self.show_dimensions,
            ratio_index: self.ratio_index,
            ratio_live: self.ratio_lock().is_some(),
            size_anchor: anchor_code(self.size_anchor),
            size_w: shown.0,
            size_h: shown.1,
            size_unit: unit_code(self.unit),
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

/// §5.4.4 / §5.4.5 的那一路：同一套遮罩、同一套标注，只有背后的像素是做的而不是抓的。
///
/// 为什么还要冻一次画面：[`MaskState::open`] 是按**物理尺寸**把快照里的显示器与 Qt 报的屏幕
/// 配对的（[`MaskState::match_monitor`]，两边不同名也不同序，只有尺寸可配），所以一块白板要的
/// 几何信息只能来自这条唯一认识这台显示器的路。这里把抓到的像素整批换掉、只留那把尺子，代价
/// 是一次 freeze（`ms=` 照原样印），换来的是"白板开不起来"这一族里最贵的那种失败——屏幕配不上
/// 对——在这条路上结构上不可能发生。
///
/// `size = WxH` 改的是**选区**而不是那块画布：遮罩是一屏一窗、图按窗拉伸，一块比屏幕小的图
/// 只会得到一张变形的板子；而选区正是这一块流程里"多大一块"的那一格 —— 标注的裁剪跟着它走
/// （[`MaskState::set_hole`] → `ink.set_hole`），交付的也是它。于是 §5.4.4 第三步那句「与屏幕
/// 或指定区域匹配」落成的形状是：板子铺满屏，区域居中。
pub fn open_board(shader: bool) -> Result<Opened, String> {
    let board = crate::settings::whiteboard_cfg();
    let surface = board.surface().map_err(|e| e.to_string())?;
    let size = board.size().map_err(|e| e.to_string())?;
    let frozen = capture::freeze()?;
    let mut snap = frozen.snap;
    for m in snap.monitors.iter_mut() {
        m.frame = falcon_core::whiteboard::canvas(surface, m.info.bounds.w, m.info.bounds.h)
            .map_err(|e| e.to_string())?;
    }
    let slots = with(|state| {
        let n = state.open(&snap, shader)?;
        state.board = Some(surface);
        if let falcon_core::whiteboard::Size::Canvas { width, height } = size {
            let b = snap
                .monitors
                .iter()
                .find(|m| m.info.primary)
                .map(|m| m.info.bounds)
                .unwrap_or(snap.virtual_bounds);
            let want = PhysRect::new(
                b.x + (b.w.saturating_sub(width) / 2) as i32,
                b.y + (b.h.saturating_sub(height) / 2) as i32,
                width,
                height,
            );
            if let Some((hole, _)) = want.clamp_to(&b) {
                state.set_hole(hole);
            }
        }
        Ok::<_, String>(n)
    })?;
    Ok(Opened {
        snap,
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
            // §5.4.1：单测里手搭的槽位，放大镜这一格从空串起 —— 空串意味着"这一窗
            // 此刻没有发给 Qt 的放大纹理"，[MaskState::magnifier_at] 第一次换键时 drop
            // 掉的是这个空串，`shim::drop_frame` 对它是 no-op。
            mag_key: String::new(),
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
        m.drag(0, 300.0, 200.0, false);
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
        m.drag(0, 100.0, 50.0, false);
        assert_eq!(m.hole, PhysRect::new(200, 100, 400, 300));
    }

    // ------------------------------------------------------------ §5.3.8 固定比例
    //
    // 这一族量的是 `MaskState::drag` 这条**手指走的那条路**，而不是 `constrain_aspect` 自己：
    // 后者在 `crates/core/src/geometry.rs` 里有它的单测与 proptest。分开的理由是这一条规矩的
    // 难点不在算，而在"哪一次拖拽该被锁、以哪一枚角为钉"。`--mask` 那扇门没有指针注入器，
    // 它问不出这一段（第 46 项 ② 同一族），所以这三十七条读数之外的那一句只能在这里说。

    /// §5.3.8 步骤 1-4 的一条完整腿：挑 1:1，拖出一张本该是 400×300 的矩形，落下来是正方形。
    ///
    /// 期望值不是"随便一枚正方形"：`constrain_aspect` 在两种取整里选**面积更接近**的那一个
    /// （400×400 差 40,000、300×300 差 30,000 ⇒ 300×300），钉住的是按下那一点 (200,100)。
    /// 这一格把这枚选择写死在测试里，是为了让它下次被改时在这里发红，而不是在某张屏幕上。
    #[test]
    fn a_picked_chip_locks_a_new_drag() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(m.pick_ratio(1), "第 1 枚是 1:1");
        assert_eq!(m.ratio_lock(), Some(1.0));
        m.press(0, 100.0, 50.0);
        m.drag(0, 300.0, 200.0, false);
        assert_eq!(m.hole, PhysRect::new(200, 100, 300, 300));
    }

    /// 锁钉住的是**按下那一点**，而按下那一点跟手指走：往左上拖时它是右下角。
    /// 这条腿单独有，是因为 `anchor_of_drag` 是新画这一支唯一的方向判断，而它错了的话
    /// 正方形会"跟着起点跑"——用户看到的是一张选区在手指的反方向上滑走。
    #[test]
    fn the_lock_anchors_the_press_point_of_a_new_drag() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(m.pick_ratio(1));
        m.press(0, 300.0, 200.0);
        m.drag(0, 100.0, 50.0, false);
        // 自由矩形是 (200,100,400,300)，钉住右下角 (600,400) ⇒ 300×300 落在 (300,100)。
        assert_eq!(m.hole, PhysRect::new(300, 100, 300, 300));
        assert_eq!(m.hole.right(), 600, "按下那一点没动");
        assert_eq!(m.hole.bottom(), 400);
    }

    /// §5.3.8 的规则原文是「调整角点时保持比例」，这是那一句的正面：角点在锁上时既保持比例，
    /// 也保持**对角**那枚角点（§5.3.1 的那一条不能被比例偷偷改掉）。
    #[test]
    fn a_corner_drag_under_the_lock_keeps_the_ratio_and_the_corner() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        assert!(m.pick_ratio(1));
        assert!(matches!(m.press(0, 600.0, 500.0), Some(Handle::Se)));
        m.drag(0, 700.0, 600.0, false);
        // 自由的 1000×800：面积 800,000，by_w 差 200,000、by_h 差 160,000 ⇒ 800×800。
        assert_eq!(m.hole, PhysRect::new(400, 400, 800, 800));
        assert_eq!(m.hole.top_left(), PhysPoint::new(400, 400));
    }

    /// 同一枚锁下的**边**把手：一条边只改一个维度，所以它不保持比例 —— 这不是漏，是 PRD 把
    /// 规则钉在角点上。这一格存在的理由是反面：如果哪天有人给边也上了锁，宽度 1000 会变 600
    /// 或 800，而这条断言就是那一次的证词。
    #[test]
    fn an_edge_handle_keeps_one_dimension_free_under_the_lock() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        assert!(m.pick_ratio(1));
        assert!(matches!(m.press(0, 600.0, 350.0), Some(Handle::E)));
        m.drag(0, 700.0, 350.0, false);
        assert_eq!(m.hole, PhysRect::new(400, 400, 1000, 600));
    }

    /// 「用户应可以临时解除比例约束」的那只手：按住修饰键的这一段拖拽画自由矩形，而**芯片没被
    /// 改动**（`ratio_index` 仍是 1:1），所以下一次拖拽回到锁上。临时如果改的是芯片，松手就
    /// 永久留在解除态 —— 那不是"临时"。
    #[test]
    fn holding_the_release_key_drops_the_lock_for_that_drag_only() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(m.pick_ratio(1));
        m.press(0, 100.0, 50.0);
        m.drag(0, 300.0, 200.0, true);
        assert_eq!(m.hole, PhysRect::new(200, 100, 400, 300), "这一拖是自由的");
        m.drag(0, 300.0, 200.0, false);
        assert_eq!(m.hole, PhysRect::new(200, 100, 300, 300), "松手就回锁");
        assert_eq!(m.ratio_index, 1, "临时解除没有动芯片");
    }

    /// 第 0 枚「无」是一把真的关：挑过 16:9 再挑它，锁必须彻底没有，而不是留着上一格的余量。
    #[test]
    fn the_no_chip_is_a_real_off_switch() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(m.pick_ratio(3), "第 3 枚是 16:9");
        assert_eq!(m.ratio_lock(), Some(16.0 / 9.0));
        assert!(m.pick_ratio(0));
        assert_eq!(m.ratio_index, 0);
        assert_eq!(m.ratio_lock(), None);
        m.press(0, 100.0, 50.0);
        m.drag(0, 300.0, 200.0, false);
        assert_eq!(
            m.hole,
            PhysRect::new(200, 100, 400, 300),
            "关着 ⇒ 与本轮之前字节零改变"
        );
    }

    /// 「或输入自定义比例」解得出来那一支：`16:10` 是表里没有的，芯片因此落到 -1，
    /// 而之后按回表里任一格时那句话被清掉 —— 一句话不能在被换掉之后还留在档上。
    #[test]
    fn a_custom_line_becomes_the_ratio_and_a_chip_clears_it() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(m.set_ratio_spec("16:10"));
        assert_eq!(m.ratio_index, -1);
        assert_eq!(m.ratio_spec, "16:10");
        assert_eq!(m.ratio_lock(), Some(1.6));
        assert!(m.pick_ratio(2));
        assert_eq!(m.ratio_spec, "", "表里那一格自此是唯一主人");
        assert_eq!(m.ratio_lock(), Some(4.0 / 3.0));
    }

    /// 解不出来那一支：话**留着**（屏幕上显示的就是用户打的字），锁不成立，返回值 `false`。
    /// 三样必须同时说真话，否则"打错的句子被偷偷改掉"与"看着像锁着其实没锁"两种都会发生。
    #[test]
    fn an_unparsable_custom_line_greys_the_lock_without_erasing_the_words() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(!m.set_ratio_spec("16:"), "半个比例不该算锁上");
        assert_eq!(m.ratio_spec, "16:", "但那句话是用户写的，不清");
        assert_eq!(m.ratio_index, -1);
        assert_eq!(m.ratio_lock(), None);
        let v = m.view_data(0);
        assert_eq!(v.ratio_index, -1);
        assert!(!v.ratio_live, "芯片亮着而锁不在 —— QML 点灰靠这一枚");
        // 空串不是"打错了"，是"清掉了"：与按「无」同一意思。
        assert!(m.set_ratio_spec("   "));
        assert_eq!(m.ratio_index, 0);
        assert_eq!(m.ratio_spec, "");
    }

    /// 越界的那一枚 index 不改任何东西。这一格管的不是"QML 会不会递错"，而是**递错了以后
    /// 屏幕上写着什么**：`false` 之后芯片仍亮在原来那格、锁仍是原来那个，而不是被一次失败的
    /// 调用清成「无」。
    #[test]
    fn a_chip_index_that_is_not_in_the_table_changes_nothing() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(m.pick_ratio(3));
        for bad in [4, 99, -1, i32::MIN] {
            assert!(!m.pick_ratio(bad), "{bad} 不在表里");
            assert_eq!(m.ratio_index, 3, "{bad} 那次没改动芯片");
            assert_eq!(m.ratio_lock(), Some(16.0 / 9.0));
        }
    }

    /// §5.3.8 的锁不跨场：`open()` 走 `reset_ratio`，而那一行在单测里够不着（`open` 要抓屏，
    /// §9.1 第十条的 headless 半边）。这一格至少钉住那枚函数自己做什么。
    #[test]
    fn reset_ratio_leaves_nothing_of_the_last_flow() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        assert!(m.set_ratio_spec("2.35:1"));
        m.reset_ratio();
        assert_eq!(m.ratio_index, 0);
        assert!(m.ratio_spec.is_empty());
        assert_eq!(m.ratio_lock(), None);
        assert_eq!(MaskState::default().ratio_index, 0, "出厂就是「无」");
    }

    /// §5.3.6 那几格用的 100% 单屏台子：算式就是算术本身，不让缩放系数掺进来 —— 与
    /// `a_drag_can_cross_the_seam_between_two_screens` 同一条理由。
    fn one_screen_100(hole: PhysRect) -> MaskState {
        let desk = PhysRect::new(0, 0, 1920, 1080);
        selectable(vec![slot(0, desk, 1.0)], hole, desk)
    }

    /// §5.3.6 第三、四步：拖出来的一张选区，定住的是指针落点那一角，所以打进来的宽高只动
    /// 右下那两条边 —— 左上那一点一步不走。
    #[test]
    fn a_typed_size_grows_from_the_corner_the_drag_held() {
        let mut m = one_screen_100(PhysRect::default());
        m.press(0, 300.0, 200.0);
        m.drag(0, 700.0, 400.0, false);
        m.release();
        assert_eq!(m.hole, PhysRect::new(300, 200, 400, 200));
        assert_eq!(
            m.size_anchor,
            Anchor::TopLeft,
            "往右下拖的那一趟定住的是左上角"
        );
        assert!(m.type_size(640, 120));
        assert_eq!(m.hole, PhysRect::new(300, 200, 640, 120));
    }

    /// 按住**角点**调整之后，锚点是那一角的对角（§5.3.1 与 §5.3.8 共用的那枚
    /// [`anchor_of_corner`]），所以松手之后打尺寸从对角长出去。这一格是 §5.3.6 第一条规则与
    /// 拖拽那条路的接缝：答案不是"永远左上角"，那会把用户刚刚按住的东西忘掉。
    #[test]
    fn holding_a_corner_moves_the_anchor_to_the_opposite_corner() {
        let mut m = one_screen_100(PhysRect::new(300, 200, 400, 200));
        assert!(matches!(m.press(0, 700.0, 400.0), Some(Handle::Se)));
        m.drag(0, 600.0, 350.0, false);
        m.release();
        assert_eq!(m.size_anchor, Anchor::TopLeft);
        assert!(m.type_size(200, 500));
        assert_eq!(m.hole, PhysRect::new(300, 200, 200, 500));
    }

    /// 拖**边**不写锚点：一条边的对面那条边上两枚角都站着不动，选哪一枚都是编的。整块移动
    /// 同理（它没定过任何一角）。所以这两类动作留下上一次的答案。
    #[test]
    fn an_edge_or_body_drag_invents_no_anchor() {
        let mut m = one_screen_100(PhysRect::default());
        m.press(0, 300.0, 200.0);
        m.drag(0, 700.0, 400.0, false);
        m.release();
        assert_eq!(m.size_anchor, Anchor::TopLeft);
        // 上边（N）：往下拖一条，选区长一截，而锚点还是左上角。
        assert!(matches!(m.press(0, 500.0, 200.0), Some(Handle::N)));
        m.drag(0, 500.0, 150.0, false);
        m.release();
        assert_eq!(m.size_anchor, Anchor::TopLeft, "拖边没有答案，只有上一次的");
        // 整块拖动（Body）：走的是 `fitted_into` 那一条滑动路，两枚角都跟着走。
        assert!(matches!(m.press(0, 500.0, 300.0), Some(Handle::Body)));
        m.drag(0, 900.0, 500.0, false);
        m.release();
        assert_eq!(m.size_anchor, Anchor::TopLeft);
        // 而从右下往左上拖出来的那一张，定住的确实是右下角。
        let mut back = one_screen_100(PhysRect::default());
        back.press(0, 700.0, 400.0);
        back.drag(0, 300.0, 200.0, false);
        back.release();
        assert_eq!(back.size_anchor, Anchor::BottomRight);
        assert!(back.type_size(100, 50));
        // 右下角站在 (700,400) 上不动，长出去的是左边与上边。
        assert_eq!(back.hole, PhysRect::new(600, 350, 100, 50));
    }

    /// §5.3.6 第二条规则在遮罩这一层的版本：打到桌面外时对面让路，而锚住的那一角不搬。
    /// `geometry.rs` 那几格钉的是同一件事的算术，这一格钉的是"这一趟真从 `MaskState` 走"。
    #[test]
    fn a_typed_size_too_big_for_the_desktop_gives_way_at_the_far_edge() {
        let mut m = one_screen_100(PhysRect::new(1800, 980, 100, 100));
        assert_eq!(m.size_anchor, Anchor::TopLeft, "没拖过 = 出厂那一枚");
        assert!(m.type_size(2000, 2000));
        assert_eq!(m.hole, PhysRect::new(1800, 980, 120, 100));
        assert_eq!(m.hole.right(), 1920, "右边贴到桌面上沿");
        assert_eq!(m.hole.bottom(), 1080, "下边贴到桌面下沿");
    }

    /// `false` 的三种来历都不动选区、也不动芯片：一把门把手不该因为递来 0 就顺手清掉别的格子
    /// （与 [`MaskState::pick_ratio`] 的越界那一支同一条口径）。
    #[test]
    fn a_typed_size_that_is_not_a_positive_number_changes_nothing() {
        let mut m = one_screen_100(PhysRect::new(300, 200, 400, 200));
        assert!(m.pick_ratio(2));
        for bad in [
            (0, 100),
            (100, 0),
            (0, 0),
            (-5, 100),
            (100, -1),
            (i32::MIN, 100),
        ] {
            assert!(!m.type_size(bad.0, bad.1), "{bad:?} 不该被接受");
            assert_eq!(
                m.hole,
                PhysRect::new(300, 200, 400, 200),
                "{bad:?} 那次动了选区"
            );
            assert_eq!(m.ratio_index, 2, "{bad:?} 那次动了芯片");
        }
        // 没有选区时同样拒绝：第一步还没走，"以当前锚点调整尺寸"没有对象。
        let mut none = one_screen_100(PhysRect::default());
        assert!(!none.type_size(100, 100));
        assert!(!none.has_selection());
    }

    /// 打的数字与亮着的锁撞车时，本轮的答案是**数字赢、芯片跟着改口**：一次显式的输入比一把
    /// 还亮着的锁新，而留着亮着的 1:1 配一张 640×120 的矩形，屏幕上就是一枚在说谎的芯片。
    #[test]
    fn a_typed_size_that_breaks_the_lock_takes_the_chip_down_with_it() {
        let mut m = one_screen_100(PhysRect::new(300, 200, 400, 400));
        assert!(m.pick_ratio(1), "1:1");
        assert!(m.type_size(640, 120));
        assert_eq!(
            m.hole,
            PhysRect::new(300, 200, 640, 120),
            "锁没有把打的数字改回去"
        );
        assert_eq!(m.ratio_index, 0, "芯片改口到「无」");
        assert_eq!(m.ratio_lock(), None);
    }

    /// 另一臂：打的数字本来就合于锁（1:1 打 500×500），芯片一个字都不动 —— 上一格说的是
    /// "撞车时谁说谎"，这一格说的是"没撞车时不许无动改口"。
    #[test]
    fn a_typed_size_that_agrees_with_the_lock_leaves_the_chip_alone() {
        let mut m = one_screen_100(PhysRect::new(300, 200, 400, 400));
        assert!(m.pick_ratio(1));
        assert!(m.type_size(500, 500));
        assert_eq!(m.hole, PhysRect::new(300, 200, 500, 500));
        assert_eq!(m.ratio_index, 1, "合于锁的那一次不该改口");
        assert_eq!(m.ratio_lock(), Some(1.0));
    }

    /// 序号表与名字表不能各长各的：QML 只做一次下标，而"屏幕上写着左上角、Rust 用的那枚是
    /// BottomRight"这一族漂移没有别的证人。
    #[test]
    fn the_anchor_code_table_covers_every_anchor() {
        let all = [
            Anchor::TopLeft,
            Anchor::TopRight,
            Anchor::BottomLeft,
            Anchor::BottomRight,
            Anchor::Center,
        ];
        let codes: Vec<i32> = all.iter().map(|a| anchor_code(*a)).collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4], "五枚锚点各占一格、不漏不重");
        assert_eq!(SIZE_ANCHOR_NAMES.len(), all.len());
        assert_eq!(size_anchor_names(), "左上角|右上角|左下角|右下角|中心");
        assert_eq!(Anchor::default(), all[0], "出厂那一枚就是序号 0 那一枚");
    }

    /// 锚点得过桥才有证人：QML 那一行字绑的是 qproperty，而 qproperty 的来历是
    /// [`MaskViewData::size_anchor`] 这一格。与上面那枚芯片的走法同一条路。
    #[test]
    fn the_view_carries_the_anchor_a_typed_size_will_use() {
        let mut m = one_screen_100(PhysRect::default());
        assert_eq!(m.view_data(0).size_anchor, 0, "没拖过 = 左上角");
        m.press(0, 700.0, 400.0);
        m.drag(0, 300.0, 200.0, false);
        m.release();
        assert_eq!(m.view_data(0).size_anchor, 3, "从右下拖出的那一张");
        // 槽位都不存在时读到的还是出厂那一枚，而不是越界下标。
        assert_eq!(m.view_data(9).size_anchor, 0);
        assert_eq!(MaskViewData::default().size_anchor, 0);
    }

    /// 锁不跨场那一格的锚点版：`open()` 里那一行在单测够不着（要抓屏），这一格钉住那枚函数
    /// 自己做什么，以及出厂值。
    #[test]
    fn reset_size_anchor_leaves_nothing_of_the_last_flow() {
        let mut m = one_screen_100(PhysRect::new(300, 200, 400, 200));
        m.size_anchor = Anchor::BottomRight;
        m.reset_size_anchor();
        assert_eq!(m.size_anchor, Anchor::TopLeft);
        assert_eq!(
            MaskState::default().size_anchor,
            Anchor::TopLeft,
            "出厂就是左上角"
        );
    }

    // ---- §5.3.7 尺寸单位切换 ----------------------------------------------------------

    /// 字样表、句子表与序号表三枚不许各长各的：页拿第 `i` 句配第 `i` 枚字样、再按
    /// [`unit_code`] 的下标点亮第 `i` 枚芯片，而这里没有第二个人会发红 —— 错一格就是一句
    /// 贴在错单位上的话。`|` 是分隔符，所以句子里出现 `|` 也是同一族漂移。
    #[test]
    fn the_unit_tables_cover_every_unit_and_agree_on_their_own_length() {
        let codes: Vec<i32> = [Unit::Px, Unit::Dip]
            .iter()
            .map(|u| unit_code(*u))
            .collect();
        assert_eq!(codes, vec![0, 1], "两枚单位各占一格、不漏不重");
        assert_eq!(SIZE_UNITS.len(), UNIT_HINTS.len(), "字样与句子必须同枚同序");
        assert_eq!(unit_names(), "px|dip");
        assert_eq!(
            unit_hints().split('|').count(),
            SIZE_UNITS.len(),
            "句子里不许出现分隔符"
        );
        assert_eq!(Unit::default(), SIZE_UNITS[0], "出厂那一格就是序号 0");
        // 过界的下标不被读成"用户挑了某一格"：它答出厂那一格，并交回真正生效的那枚。
        assert_eq!(unit_at_code(7), Unit::Px);
        assert_eq!(unit_at_code(-1), Unit::Px);
        assert_eq!(unit_at_code(1), Unit::Dip);
    }

    /// 预期结果第一句「单位切换后，界面显示正确的换算值」在 Rust 侧的那对数字：px 那一格读
    /// 的是**选区自己的物理尺寸**，与快速保存那句「已保存到 …（选区 800×600 像素）」同一个数
    /// 字；而窗内几何 `hole` 仍然是除过这一扇窗缩放的画法。两者分开是本轮的量点 —— 换绑之前
    /// 屏幕上那两行 W×H 读的就是后者，却不带任何单位字样。
    #[test]
    fn the_px_readout_is_the_number_the_result_detail_names() {
        let desk = PhysRect::new(0, 0, 1920, 1080);
        let mut m = selectable(
            vec![slot(0, desk, 2.0)],
            PhysRect::new(300, 200, 800, 600),
            desk,
        );
        assert_eq!(m.unit, Unit::Px, "出厂是 px");
        assert_eq!(m.size_shown(), (800, 600));
        let v = m.view_data(0);
        assert_eq!((v.size_w, v.size_h), (800, 600));
        assert_eq!(v.size_unit, 0);
        // 画法归画法：这一扇窗的洞仍然是 200% 除过的数。
        assert_eq!((v.hole.w, v.hole.h), (400, 300));
        m.set_unit(1);
        assert_eq!(m.size_shown(), (400, 300), "200% 屏上 dip 就是那个除过的数");
        assert_eq!(m.view_data(0).size_unit, 1);
    }

    /// 单位是一枚状态，不是一扇窗的看法：两扇窗上那行 W×H 必须同字，用的是**选区自己那张屏**
    /// 的缩放（[`monitor_slot_for`]，它自 P67 写下之后一直没有生产调用者，本轮第一次被读）。
    #[test]
    fn one_unit_reads_the_same_size_on_both_windows() {
        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 2.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let desk = PhysRect::new(0, 0, 3840, 1080);
        let mut m = selectable(vec![left, right], PhysRect::new(2100, 100, 400, 300), desk);
        assert_eq!(m.size_scale(), Scale::of(1.0), "选区整个站在右屏");
        m.set_unit(1);
        assert_eq!(m.size_shown(), (400, 300), "100% 那张屏：dip 与 px 同数");
        assert_eq!(
            m.view_data(0).size_w,
            m.view_data(1).size_w,
            "两张屏、一个状态"
        );
        assert_eq!(m.view_data(0).size_unit, 1);
        assert_eq!(m.view_data(1).size_unit, 1);
    }

    /// 跨屏那一条对角线：两扇缩放不同的屏都沾到选区时，说了算的是**重叠更宽**的那一扇
    /// （打平取更早的一扇，[`monitor_slot_for`] 自己的规则，这里只钉它真的被这样用了）。
    #[test]
    fn a_straddle_pays_the_wider_overlap_for_its_scale() {
        let left = slot(0, PhysRect::new(0, 0, 1920, 1080), 2.0);
        let right = slot(1, PhysRect::new(1920, 0, 1920, 1080), 1.0);
        let desk = PhysRect::new(0, 0, 3840, 1080);
        // 1000..2000：左屏占 920、右屏只占 80 ⇒ 用左屏的 2.0。
        let mut m = selectable(vec![left, right], PhysRect::new(1000, 100, 1000, 300), desk);
        assert_eq!(m.size_scale(), Scale::of(2.0));
        assert_eq!(m.size_shown(), (1000, 300), "px 那一格不看任何缩放");
        m.set_unit(1);
        assert_eq!(m.size_shown(), (500, 150), "dip 那一格按赢的那一扇除");
    }

    /// 操作步骤第三句「系统根据当前显示器缩放比例换算尺寸」倒过来那一路：**打字进去的数**按
    /// 当前单位解释。150% 屏（`slot(..., 1.5)` 就是这一枚测试的那台）上打 200×100 dip，落地是
    /// 300×150 物理像素，而读回来的还是用户打的那对数 —— 屏幕上没有变形，只是它说的是 dip。
    #[test]
    fn typing_in_dip_lands_on_the_desktop_as_physical_pixels() {
        let desk = PhysRect::new(0, 0, 1920, 1080);
        let mut m = selectable(
            vec![slot(0, desk, 1.5)],
            PhysRect::new(100, 100, 300, 300),
            desk,
        );
        m.set_unit(1);
        assert!(m.type_size(200, 100));
        assert_eq!(m.hole, PhysRect::new(100, 100, 300, 150));
        assert_eq!(m.size_shown(), (200, 100));
    }

    /// 出厂那一格必须**什么都不换算**：`Unit::Px` 那两支是恒等，所以 P162 钉下的每一张
    /// `type_size` 单测与 `--mask` 门里那 13 行在 200% 屏上也给出与本轮之前一样的答案。
    #[test]
    fn the_px_unit_converts_nothing_even_on_a_200_percent_screen() {
        let desk = PhysRect::new(0, 0, 1920, 1080);
        let mut m = selectable(
            vec![slot(0, desk, 2.0)],
            PhysRect::new(100, 100, 300, 300),
            desk,
        );
        assert!(m.type_size(500, 400));
        assert_eq!(m.hole, PhysRect::new(100, 100, 500, 400), "px 就是像素");
        assert_eq!(m.size_shown(), (500, 400));
    }

    /// 换算与夹的顺序：先按单位换成物理像素、再让桌面边界夹（§5.3.6 第二条规则）。反过来就
    /// 会把"200% 屏上的 1000 dip"当成 1000 物理像素夹掉一半。
    #[test]
    fn a_typed_dip_number_is_still_clamped_by_the_desktop() {
        let desk = PhysRect::new(0, 0, 1920, 1080);
        let mut m = selectable(
            vec![slot(0, desk, 2.0)],
            PhysRect::new(0, 0, 100, 100),
            desk,
        );
        m.set_unit(1);
        assert!(m.type_size(1000, 100));
        assert_eq!(m.hole.w, 1920, "桌面赢，不是单位赢");
        assert_eq!(
            m.size_shown(),
            (960, 100),
            "读回来的是夹过之后按 dip 说的数"
        );
    }

    /// 还没有选区时单位什么也换不了：`type_size` 那一族的三条拒绝一条不因为本轮多出一格而变，
    /// 而读数在 dip 那一格也不能凭空造出一对数字。
    #[test]
    fn the_unit_cannot_convert_a_selection_that_does_not_exist() {
        let mut m = one_screen_100(PhysRect::default());
        m.set_unit(1);
        assert!(!m.type_size(100, 100), "第一步还没走：什么单位都救不了");
        assert_eq!(m.hole, PhysRect::default());
        assert_eq!(m.size_shown(), (0, 0));
    }

    /// 桥上交回的是**真正生效**的那一格而不是"做没做成"：页上的芯片只有跟着这个答案亮，
    /// 才不会在递错下标的时候自己骗自己。
    #[test]
    fn set_unit_answers_with_the_unit_that_actually_landed() {
        let mut m = one_screen_100(PhysRect::new(0, 0, 100, 100));
        assert_eq!(m.set_unit(1), Unit::Dip);
        assert_eq!(m.unit, Unit::Dip);
        assert_eq!(m.set_unit(9), Unit::Px);
        assert_eq!(m.view_data(0).size_unit, 0);
    }

    /// 切单位是一句读数的事：选区一步不走，锚点与比例锁也不改口 —— 它们各有各的主人，本轮
    /// 不许顺手把别人那格也动一下（`applySize` 撞锁改口是**那一次输入**的后果，不是这一枚）。
    #[test]
    fn switching_the_unit_moves_no_selection_and_touches_no_other_state() {
        let mut m = one_screen_100(PhysRect::new(300, 200, 400, 200));
        m.size_anchor = Anchor::BottomRight;
        assert!(m.pick_ratio(1));
        let before = m.hole;
        assert_eq!(m.set_unit(1), Unit::Dip);
        assert_eq!(m.hole, before, "切单位不动选区");
        assert_eq!(m.size_anchor, Anchor::BottomRight, "锚点没有第二个主人");
        assert_eq!(m.ratio_index, 1, "锁不因切单位而改口");
    }

    /// 三格一起过桥，且没有槽位时不报越界下标：`view_data(9)` 走 [`MaskViewData::default`]，
    /// 那对读数是 0×0、单位是出厂那一格 —— 与"窗还没分到屏"在屏幕上读起来一样。
    #[test]
    fn the_view_carries_the_unit_and_its_own_pair_of_numbers() {
        let mut m = one_screen_100(PhysRect::default());
        assert_eq!(m.view_data(0).size_unit, 0, "没挑过 = px");
        assert_eq!((m.view_data(0).size_w, m.view_data(0).size_h), (0, 0));
        assert_eq!(MaskViewData::default().size_unit, 0);
        m.set_unit(1);
        assert_eq!(m.view_data(0).size_unit, 1);
        assert_eq!(m.view_data(9).size_unit, 0, "没有槽位就回答出厂那一格");
        assert_eq!((m.view_data(9).size_w, m.view_data(9).size_h), (0, 0));
    }

    /// §5.3.7 第一步那条腿的证人。`open()` 里那一行单测够不着（要抓屏），所以这里钉的是它**做的
    /// 那一手**：单位的起点是 `[capture] size_unit` 那一格，而不是枚举的出厂格；以及这一手会一路
    /// 走到页上那个序号。两半各钉一件事 —— 前半是"档说了算"，后半是"**芯片不改档**"：页上那一枚
    /// 点下去只改这一场，下一场仍回到档里那一格。把这两半混成一格，就分不清"用户记住了 dip"与
    /// "用户上一张临时看了 dip"。
    #[test]
    fn the_flow_opens_on_the_unit_the_file_picks() {
        let mut m = one_screen_100(PhysRect::new(0, 0, 100, 100));
        assert_eq!(
            MaskState::default().unit,
            Unit::Px,
            "没碰过档的状态仍是出厂那一格"
        );
        assert_eq!(
            falcon_core::config::Capture::default().size_unit,
            Unit::Px,
            "档的出厂与枚举的出厂是同一格，所以已存在每一次按下零改变"
        );
        m.unit = Unit::Dip;
        m.capture.size_unit = Unit::Dip;
        m.unit = m.capture.size_unit; // `open()` 走的那一手
        assert_eq!(m.unit, Unit::Dip);
        assert_eq!(m.view_data(0).size_unit, 1, "页上亮的是第二枚芯片");

        let mut n = one_screen_100(PhysRect::new(0, 0, 100, 100));
        n.unit = Unit::Dip; // 这一场在芯片上点的
        n.capture = falcon_core::config::Capture::default(); // 下一场开档：档里写的是 px
        n.unit = n.capture.size_unit;
        assert_eq!(n.unit, Unit::Px, "芯片不写档，所以下一场回到档里那一格");
    }

    /// 芯片亮哪格与锁在不在，是 QML 绑的读数而不是它自己记的账 —— 与 `show_dimensions`
    /// 同一条路径：`MaskState` → `MaskViewData` → qproperty。
    #[test]
    fn the_view_carries_which_chip_is_lit_and_whether_the_lock_holds() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        let v = m.view_data(0);
        assert_eq!((v.ratio_index, v.ratio_live), (0, false), "没挑过 = 无");
        for (index, want) in [(1, true), (2, true), (3, true)] {
            assert!(m.pick_ratio(index));
            let v = m.view_data(0);
            assert_eq!(v.ratio_index, index);
            assert_eq!(v.ratio_live, want, "{index} 那格该是锁着的");
        }
        // 一张不存在的屏：`view_data` 走 `MaskViewData::default()`，两枚跟着回到「无」。
        let v = m.view_data(99);
        assert_eq!((v.ratio_index, v.ratio_live), (0, false));
    }

    /// 那张表自己的体检，同 `annotate::TOOL_TEXTS` 的那一枚：清单只长一处，而每一格的比例串
    /// 必须**解得开**（除了「无」那一格的空串，它解不开正是它的意思）。没有这一格，"芯片写着
    /// 4:3 而拖出来不锁"是一种能在表里安静长大的缺陷。
    #[test]
    fn every_ratio_chip_names_a_spec_that_parses() {
        assert_eq!(RATIO_CELLS.len(), 4);
        assert_eq!(ratio_names().split('|').count(), RATIO_CELLS.len());
        assert_eq!(ratio_hints().split('|').count(), RATIO_CELLS.len());
        for (i, (name, spec, hint)) in RATIO_CELLS.iter().enumerate() {
            if i == 0 {
                assert_eq!(*spec, "", "「无」那一格必须真的没有比例串");
                assert!(parse_ratio(spec).is_none());
            } else {
                let r = parse_ratio(spec);
                assert!(r.is_some(), "{name} 的比例串 {spec:?} 解不开");
                assert!(r.unwrap().is_finite() && r.unwrap() > 0.0);
            }
            assert!(
                hint.starts_with(name),
                "{name} 那句提示开头说的不是它自己：{hint}"
            );
        }
    }

    /// §5.3.8 第二句规则「用户应可以临时解除比例约束」在屏幕上**唯一的入口**是芯片的悬停句，
    /// 所以那半句必须说的是真话：只跟在锁真的成立的那几枚后面。判据不是清单第三格的形状，
    /// 而是 [`MaskState::ratio_lock`] 与它绑出去的 `ratio_live` —— 也就是 `drag` 那一支真正读的
    /// 同一枚函数，于是句子与锁没法各说各话。
    ///
    /// 没有这一格会怎样：`ratio_hints()` 把句子拼给全部四枚，或谁都不拼，而 `drag` 照旧按
    /// `unlocked` 分支 —— 屏幕上多教或少教的那半句不会让任何一行变红。「无」那一格带上这句，
    /// 教的就是一条按下去什么都不会发生的手势。
    #[test]
    fn the_release_sentence_rides_only_the_chips_that_lock() {
        let all = ratio_hints();
        let hints: Vec<&str> = all.split('|').collect();
        assert_eq!(hints.len(), RATIO_CELLS.len(), "悬停句与芯片不同数");
        for (index, (name, spec, base)) in RATIO_CELLS.iter().enumerate() {
            let (slots, desk) = one_screen();
            let mut m = selectable(slots, PhysRect::default(), desk);
            assert!(m.pick_ratio(index as i32), "{name} 那格按不动");
            let locked = m.ratio_lock().is_some();
            assert_eq!(
                locked,
                parse_ratio(spec).is_some(),
                "{name}：锁在不在与比例串解不解得开各说了一套"
            );
            assert_eq!(
                m.view_data(0).ratio_live,
                locked,
                "{name}：绑出去的与锁不在"
            );
            assert!(
                !base.contains(RATIO_RELEASE_HINT),
                "{name} 的句子本来就带了一遍，拼出来会是两遍"
            );
            let want = if locked {
                format!("{base}，{RATIO_RELEASE_HINT}")
            } else {
                (*base).to_string()
            };
            assert_eq!(hints[index], want.as_str(), "{name} 那句的拼法");
        }
        assert_eq!(hints[0], RATIO_CELLS[0].2, "「无」那一格必须原样");
    }

    /// 桌面上那一枚边界与这一枚锁相撞时**谁付账**。量下来的答案是"锁与对角都不付，付的是拖拽
    /// 行程"：指针在 `desk_point` 里先被夹进桌面（DIP −1000 → 物理 0），`resize` 给出的自由矩形
    /// 是 (0,0,1200,1000)，1:1 那一枚锁取面积更近的一支 → 1000×1000，而 §5.3.1 那枚钉住的右下角
    /// 原样停在 (1200,1000)。抓住的那枚左上角停在 (200,0) —— 指针到得了，选区到不了。
    ///
    /// 记下来的理由是这一格补的是什么：`drag` 那段注释曾经写反过（"被裁掉之后比例可能不再精确"），
    /// 而那句话任何一扇门都读不出来 —— `mask_check` 没有指针注入器（第 46 项 ②），它只能量静态
    /// 颜色，量不到一次拖拽。注释与实现的差别不会让任何一行变红，这一格会。
    #[test]
    fn the_border_shortens_the_drag_and_pays_neither_the_ratio_nor_the_anchor() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        assert!(m.pick_ratio(1));
        assert!(matches!(m.press(0, 200.0, 200.0), Some(Handle::Nw)));
        m.drag(0, -1000.0, -1000.0, false);
        assert_eq!(m.hole, PhysRect::new(200, 0, 1000, 1000));
        assert_eq!(m.hole.w, m.hole.h, "比例在边界上不该付账");
        assert_eq!(
            (m.hole.right(), m.hole.bottom()),
            (1200, 1000),
            "§5.3.1 那枚对角逐像素没动"
        );
        assert_eq!(m.hole.y, 0, "贴到桌面上沿那一条");
        assert_eq!(
            m.hole.top_left(),
            PhysPoint::new(200, 0),
            "付账的是拖拽行程：抓住的那枚角停在 (200,0)"
        );
        assert_eq!(
            m.desk_point(0, -1000.0, -1000.0),
            Some(PhysPoint::new(0, 0)),
            "边界动手的地方在指针上：夹到桌面角，比例看到的已经是夹过的那一点"
        );
    }

    /// 上面那一格是一臂；这一格把**四枚比例串 × 四枚角点**共 16 臂逐臂问同一句话：`drag` 末尾
    /// 那一次 `fitted_into` 在锁着的角点路上动过手没有。问法是把测试自己算出的自由矩形喂回
    /// `constrain_aspect`，拿返回值与屏幕上的选区比 —— 相等就是那一步没动手。另有四句是同一臂
    /// 必须同时说真话的地方：选区在桌面内、钉住的那枚角没动、锁只收短边不拉长边（那正是
    /// `fitted_into` 唯一可能动手的前提）、比例在选区自己的 w/h 上复现得到（差不到一枚像素）。
    /// 最后一臂是反面：同一条 `fitted_into` 在 `Body` 那一支每一轮都在滑动整块选区，所以那 16
    /// 臂的"没动"不是因为这条代码路永远不动 —— 是锁自己先把它排除了。
    #[test]
    fn the_desktop_never_touches_a_lock_selected_rect() {
        let start = PhysRect::new(400, 400, 800, 600);
        // (把手, 按下的 DIP, 拖到的 DIP) —— 拖的那一点每一臂都远在桌面外，于是指针必定被夹住。
        let corners = [
            (Handle::Nw, 200.0, 200.0, -4000.0, -4000.0),
            (Handle::Ne, 600.0, 200.0, 4000.0, -4000.0),
            (Handle::Sw, 200.0, 500.0, -4000.0, 4000.0),
            (Handle::Se, 600.0, 500.0, 4000.0, 4000.0),
        ];
        let mut arms = 0;
        for spec in ["1:1", "4:3", "16:9", "3:4"] {
            let r = parse_ratio(spec).unwrap();
            for (handle, px, py, dx, dy) in corners {
                let (slots, desk) = one_screen();
                let mut m = selectable(slots, start, desk);
                assert!(m.set_ratio_spec(spec));
                assert!(
                    matches!(m.press(0, px, py), Some(h) if h == handle),
                    "{spec} × {handle:?}：抓错了把手，这一臂问的就不是它"
                );
                m.drag(0, dx, dy, false);
                let p = m.desk_point(0, dx, dy).unwrap();
                let free = resize(&start, handle, p, &desk);
                let anchor = anchor_of_corner(handle).unwrap();
                let want = constrain_aspect(&free, r, anchor).unwrap();
                assert_eq!(
                    m.hole, want,
                    "{spec} × {handle:?}：边界动了一次锁选出的矩形"
                );
                assert!(
                    m.hole.x >= desk.x
                        && m.hole.y >= desk.y
                        && m.hole.right() <= desk.right()
                        && m.hole.bottom() <= desk.bottom(),
                    "{spec} × {handle:?} 跑到了桌面上没有的位置：{:?}",
                    m.hole
                );
                assert_eq!(
                    anchor.point(&m.hole),
                    anchor.point(&free),
                    "{spec} × {handle:?}：钉住的那枚角动了"
                );
                assert!(
                    (m.hole.w == free.w && m.hole.h <= free.h)
                        || (m.hole.h == free.h && m.hole.w <= free.w),
                    "{spec} × {handle:?}：锁把一条边拉长了（自由 {free:?} → 锁后 {:?}），\
                     那正是 `fitted_into` 唯一可能动手的前提",
                    m.hole
                );
                let err = (m.hole.w as f64 / m.hole.h as f64 - r).abs() * m.hole.h as f64;
                assert!(err <= 1.0, "{spec} × {handle:?}：比例差了 {err:.3} 枚像素");
                arms += 1;
            }
        }
        assert_eq!(arms, 16, "四枚比例串 × 四枚角点");

        // 反面那一臂：整块拖动（不是角点）走的就是 `fitted_into`，它把选区整块滑回桌面内。
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, start, desk);
        assert!(m.pick_ratio(1));
        assert!(matches!(m.press(0, 400.0, 350.0), Some(Handle::Body)));
        m.drag(0, 4000.0, 4000.0, false);
        assert_eq!(m.hole, PhysRect::new(2272, 1320, 800, 600));
        assert_eq!(
            m.hole.w, 800,
            "Body 那一路不锁 —— 尺寸是整块滑过来的，不是裁过来的"
        );
    }

    /// §5.3.1 step 4: a corner drags the two edges it owns and leaves the opposite
    /// corner exactly where it was.
    #[test]
    fn a_corner_drag_resizes_and_keeps_the_opposite_corner() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        // The bottom-right corner is physical (1200, 1000) = DIP (600, 500).
        assert!(matches!(m.press(0, 600.0, 500.0), Some(Handle::Se)));
        m.drag(0, 700.0, 600.0, false);
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
        m.drag(0, 2000.0, 1200.0, false);
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
        m.drag(0, 350.0, 320.0, false);
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
        m.drag(0, 2100.0, 300.0, false);
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
        assert_eq!(m.list_history(10, false), "", "没有历史的一场反倒读出了行");
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
        assert_eq!(m.list_history(10, false), "", "关着的那枚开关反倒交回了行");
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
        assert_eq!(m.list_history(10, false), "", "空库读出了行");
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

        let rows = m.list_history(10, false);
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
        let one = m.list_history(1, false);
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
        assert_eq!(m.list_history(10, false), "", "打不开的库却交回了一份清单");
        assert!(m.history_list.is_empty(), "⑤ 把 ④ 读到的清单留在了场上");
        assert!(
            m.history_list_last.contains("打不开"),
            "⑤ 要说清是打不开而不是没开: {:?}",
            m.history_list_last
        );
    }

    /// §5.14.3 步 3「找到标记为“已取消”的记录」在产品这一头上说的**几句不同的话**：没筛、筛着
    /// 而有行、筛着而一行也没有，再加上翻页那一臂的筛着到底 / 不筛还有 —— 五档五句，一句也不并
    /// （#453）。第三档不许说成「库里没有行」：库里可能有五十行，只是没一行被取消过，而这一问
    /// 答的是"有哪几行被标了那一字"，「库里一共几行」它没问过、也不替谁猜。
    ///
    /// 已取消的那几行走的是**产品的 Esc 那一只手**（[`MaskState::escape`] + `[history] save_all`）
    /// 而不是测试自己往库里种行：这样"行末写着那一字的那一行"与"筛这一问读回的那一行"之间没有
    /// 替身。筛的字样也不是这里编的：句里的「只看已取消」与「已取消」都引
    /// [`crate::history_window`] 那两枚常量（#281）。
    #[test]
    fn the_cancelled_filter_says_its_own_sentences_and_keeps_the_page_honest() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let cfg = falcon_core::config::History {
            enabled: true,
            save_all: true,
            keep_originals: false,
            thumb_px: 0,
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        };
        let filter = crate::history_window::CANCELLED_FILTER_WORD;
        let word = crate::history_window::CANCELLED_WORD;

        // 一座库、三行：甲是确认的（没取消），乙与丙是 Esc 掉的（已取消）。
        let dir = sandbox("filter-sentences");
        let mut m = savable(hole);
        m.history = Some(cfg.clone());
        m.history_dir = dir.clone();
        assert_eq!(m.commit(), Some(hole), "第一格没确认成");
        let jia = m.history_last_id.expect("确认的那一行没记成");
        m.set_hole(PhysRect::new(1, 2, 30, 40));
        assert!(m.escape(), "有选区的一场 Esc 没吃掉自己");
        let yi = m.history_last_id.expect("Esc 那一格没记成行");
        m.set_hole(PhysRect::new(3, 4, 50, 60));
        assert!(m.escape(), "第二次 Esc 没吃掉自己");
        let bing = m.history_last_id.expect("第二次 Esc 没记成行");
        assert_ne!(yi, bing, "两次 Esc 记成了同一行，后面的先后无从分辨");
        let (rows_before, last_before) = (m.history_records, m.history_last_id);

        // ① 没筛：三行全在，而那句里不许出现筛的字样 —— 写了，用户就会以为这三行是筛剩下的。
        let all = m.list_history(10, false);
        assert_eq!(all.split('|').count(), 3, "没筛时该数到三行：{all}");
        let said_all = m.history_list_last.clone();
        assert!(said_all.contains("3 行"), "① 没数到三行：{said_all}");
        assert!(
            !said_all.contains(filter),
            "全量那一问里出现了筛的字样：{said_all}"
        );

        // ② 筛着而有行：只剩 Esc 掉的那两行，行末都带着那一字。
        let some = m.list_history(10, true);
        let picked: Vec<&str> = some.split('|').collect();
        assert_eq!(picked.len(), 2, "筛着时该只剩两行：{some}");
        assert!(
            picked[0].starts_with(&format!("{bing} · "))
                && picked[1].starts_with(&format!("{yi} · ")),
            "筛过的那一问递回了没取消的那一行，或是把次序弄反了：{picked:?}"
        );
        for r in &picked {
            assert!(r.ends_with(word), "筛进来的那行没带字样：{r}");
        }
        let said_some = m.history_list_last.clone();
        assert!(
            said_some.contains(filter) && said_some.contains("2 行"),
            "② 要说清筛着、以及筛剩下几行：{said_some}"
        );
        assert_ne!(said_some, said_all, "筛与不筛说了同一句话");

        // ③ 筛着而一行也没有：另一座只有确认行的库。它不是「没开」、不是「打不开」、也不是
        //    ① 那一句「3 行」。
        let bare_dir = sandbox("filter-sentences-bare");
        let mut q = savable(hole);
        q.history = Some(cfg);
        q.history_dir = bare_dir.clone();
        assert_eq!(q.commit(), Some(hole), "对照那一座库没记成行");
        assert_eq!(
            q.list_history(10, true),
            "",
            "库里没有那种行时，筛这一问一行也不该递回来"
        );
        let said_none = q.history_list_last.clone();
        assert!(
            said_none.contains(filter) && said_none.contains("0 行"),
            "③ 要说清筛着、而答案是零行：{said_none}"
        );
        assert!(
            !said_none.contains("没开") && !said_none.contains("打不开"),
            "一次正常的空被说成了开关或库坏了：{said_none}"
        );
        assert_ne!(said_none, said_all, "「库里没有那种行」被并进了全量那一句");

        // ④ 翻页那一臂：拿筛着这一页的最后一行（乙）当游标 —— 更旧的已取消行没有了，而库里
        //    还有更旧的甲。那句「到底了」说的是**筛过的这张清单**到底了。
        let tail = m.list_history_older(yi, 10, true);
        assert_eq!(tail, "", "筛着到底的那一问不该递回行：{tail}");
        let said_tail = m.history_list_last.clone();
        assert!(
            said_tail.contains(filter) && said_tail.contains("到底了"),
            "④ 那句要说这是筛过的清单到底了：{said_tail}"
        );
        // ⑤ 同一枚游标、不筛：还有甲。两问在这一格里各说各的，谁也不替谁圆。
        let older = m.list_history_older(yi, 10, false);
        assert!(
            older.starts_with(&format!("{jia} · ")),
            "不筛时更旧那一页该还有甲：{older}"
        );
        let said_older = m.history_list_last.clone();
        assert!(
            !said_older.contains("到底了") && !said_older.contains(filter),
            "全量那一页被筛过那句污染了：{said_older}"
        );

        // 五档走完，账没动：清单是纯读，它不许记行、也不许改这一场最后记成的那一行。
        assert_eq!((m.history_records, m.history_last_id), (rows_before, last_before), "读清单改了库里的账");
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
        let listed = m.list_history(10, false);
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
        // 这一格的锁由前置经 core 打上 —— 不是因为产品里没有"锁定"那一条腿（那句话是本档
        // P99 写的，那时确实零读者；P100 起它有了：`crates/qt_bridge/src/mask.rs` 的
        // `lock_history` ⇒ `crates/qt_bridge/src/session.rs` 的 `historyLock` ⇒
        // `qml/HistoryWindow.qml` 每一行右边那枚「锁上 / 解开」），而是因为本格要量的只是
        // "删除这一条腿不看锁"，锁从哪一只手来与那一问不相干。
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

    /// §5.8.1 步 2 与步 3 的那一枚「贴图」：一份文件也不写，屏幕上飘的就是 `delivered()`
    /// 交出的那一张。
    ///
    /// 四格各判一件事，不并成一句"按下去有反应"（#453）：① 没有选区时拒得干净 —— 名册的读数
    /// 一字不动；② 成功那一臂真的一枚新贴图进了名册、落点正是用户拖出来的那块矩形（带着
    /// §5.5.7 阴影往**外**长那一段偏移，并且卡在桌面左上角为止），而贴上屏幕的那一张与
    /// [`MaskState::delivered`] 逐像素同；③ 这一枚贴图**不记得**自己的原截图坐标，所以
    /// §5.5.8 的「刷新」对它只能交回 [`falcon_core::refresh::plan`] 那句"请重新截一次"；④ 页与桥
    /// 按的是同一个名字（`bar.view.pinSelection()` 对上 `#[cxx_name = "pinSelection"]`），因为门把手
    /// 的名字写错既不发红也不报错，只静默读回 `undefined`。
    /// 第三格是 `add` 与 `add_captured` 唯一的分野，也是这一档为什么不肯用后者：认下那块原坐标
    /// 就等于同意让活屏幕盖掉用户刚画的箭头。
    ///
    /// 不写盘那一格读的是 `[output]` 那一枚目录：门本身一步也不碰它，两趟按完那里还是空的。
    ///
    /// 名册是进程内那一份，同一趟里别的测试也会往里加贴图，所以判的是「那句话点名的**那一枚**
    /// id 是这趟新长的、且在名册里」，不是「这趟只多了一枚」——与 `annotate_check` 里
    /// `save_then_pin_rows` 那两行同一把尺。
    #[test]
    fn the_toolbar_pin_cell_floats_the_selection_and_writes_no_file() {
        let dir = sandbox("pinonly");
        let named = |said: &str| -> Option<u64> {
            let tail = said.split_once("贴成了贴图 ")?.1;
            tail.split(['：', '。', ' ']).next()?.parse().ok()
        };
        let naming = || crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "pinonly".into(),
            collision: falcon_core::naming::Collision::Increment,
        };
        let mut m = savable(PhysRect::new(30, 40, 120, 90));
        m.naming = naming();
        // 阴影把画布每一边都往**外**长 12 px，于是图片自己那一角要往外挪 12 px，画面才不漂。
        m.decor.plate.shadow_margin = 12;

        // ① 空选区：拒绝，而且一枚贴图都不许多出来。
        let mut none = savable(PhysRect::default());
        none.naming = naming();
        let before = crate::state::with(|s| s.ids());
        assert!(!none.pin_selection(), "{:?}", none.message);
        assert!(none.message.contains("还没有选区"), "{:?}", none.message);
        assert_eq!(
            crate::state::with(|s| s.ids()),
            before,
            "一句拒绝往名册里递了贴图：{:?}",
            none.message
        );

        // ② 按下去：那一张浮在用户拖出来的那块矩形上，画面与 `delivered()` 那一格同。
        let before = crate::state::with(|s| s.ids());
        assert!(m.pin_selection(), "{:?}", m.message);
        let id = named(&m.message).unwrap_or_else(|| panic!("那句话没点名贴图 id：{:?}", m.message));
        assert!(
            !before.contains(&id) && crate::state::with(|s| s.ids()).contains(&id),
            "{:?} 点名的 {id} 不是这一趟新长出来的那一枚",
            m.message
        );
        assert!(m.message.contains("没有写成文件"), "{:?}", m.message);
        let want = m.delivered().expect("the stitched frame");
        assert_eq!(
            crate::state::with(|s| s.with_rendered(id, |f| f.clone())),
            Some(want.clone()),
            "贴上屏幕的那一张与 delivered() 交出的不是同一份像素"
        );
        assert_eq!(
            crate::state::with(|s| s
                .pin_rects()
                .into_iter()
                .find(|(pid, _)| *pid == id)
                .map(|(_, r)| r)),
            // 30-12, 40-12；120+24, 90+24
            Some(PhysRect::new(18, 28, 144, 114)),
            "贴图没有落在用户拖出来的那块矩形上"
        );

        // ③ 同一枚键对这一张贴图说什么：它没有原截图坐标可重读。
        assert!(
            crate::state::with(|s| s.captured_of(id)).is_none(),
            "贴图记下了原截图坐标：下一次「刷新」会把用户画的标注盖掉"
        );

        // 卡在桌面左上角的那一臂：选区贴边时，往外的 12 px 被桌面截住，落点就是 (0,0)。
        let mut edge = savable(PhysRect::new(3, 5, 40, 30));
        edge.naming = naming();
        edge.decor.plate.shadow_margin = 12;
        assert!(edge.pin_selection(), "{:?}", edge.message);
        let clipped = named(&edge.message).expect("the edge sentence");
        assert_eq!(
            crate::state::with(|s| s
                .pin_rects()
                .into_iter()
                .find(|(pid, _)| *pid == clipped)
                .map(|(_, r)| r)),
            Some(PhysRect::new(0, 0, 64, 54)),
            "贴边的选区把贴图推到了桌面之外"
        );

        // 页上那一腿按的是桥交出的**那一个名字**：`#[qinvokable]` 的门把手对页上露的是驼峰，
        // 写成下划线既不报错也不在构造期发红，只是静默绑不上、读回 `undefined`（同一族在
        // `annotate_check::the_band_clock_and_its_sentence_are_read_by_their_real_names` 记过
        // 一次账）。本档没有一枚输入注入器能按屏幕上那枚钮，所以这一格能被机械判的就是这一条
        // 页 ↔ 桥的名字配对，加上 `annotate_check` 那一道 hint 分母。
        const QML: &str = include_str!("../qml/AnnotationToolbar.qml");
        const LEGS: &str = include_str!("mask_view.rs");
        assert!(
            QML.contains("bar.delivered = bar.view.pinSelection()")
                && QML.contains("onTapped: bar.pin()")
                && LEGS.contains("#[cxx_name = \"pinSelection\"]"),
            "工具栏那枚「贴图」没有按桥交出的名字走到档里那一格：页与桥那一腿断了"
        );

        // 一句也不落盘：两趟按完，`[output]` 那一枚目录里一个文件也没有。
        assert_eq!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .filter_map(Result::ok)
                .count(),
            0,
            "这一枚钮写了一份文件，而它承诺的是不写"
        );
        crate::state::with(|s| s.close(&[id, clipped]));
    }

    /// §8.3 第三句：写不进去的那一句要把两条退路递回用户手里。
    ///
    /// 这一格的对面不是「话说得好听」，是失败之后**还剩什么可办**：第二句（不得直接关闭截图
    /// 界面）由 `save_out` 返回 `false` 而 `message` 有字来保证，第三句由这一句的两个名字保证。
    /// 名字必须是按得动的东西，所以后半格读的是工具栏那份源码里那两枚 `Cell` 真在不在 ——
    /// 一句「可以复制到剪贴板」如果那枚钮不在了，就是 #433 说的指着一处没人能打开的地方。
    /// 落点选「往一枚文件里写」：这台机器上它当场回 `183`，而 `encode::save` 会替不存在的路径
    /// 造目录，所以造出来的路径那一腿根本走不到写失败（工具栏那档量的正是这一点）。
    #[test]
    fn a_failed_write_hands_back_both_exits_that_are_reachable() {
        let dir = sandbox("exits");
        let blocker = dir.join("occupied.png");
        std::fs::write(&blocker, b"not a directory").expect("blocker");
        let mut m = savable(PhysRect::new(10, 20, 100, 80));
        let inner = blocker.join("selection.png");
        assert!(!m.save_out(&inner.to_string_lossy(), ""), "{:?}", m.message);
        let said = m.message.clone();
        assert!(said.contains("路径无效"), "{said} 没说它是哪一类失败");
        assert!(said.contains("截图还在"), "{said} 没接住第二句那枚「还在」");
        assert!(said.contains("换一处目录"), "{said} 没递出第一条退路");
        assert!(said.contains("剪贴板"), "{said} 没递出第二条退路");
        assert!(!inner.exists(), "一句拒绝写出了一个文件");
        assert!(
            !blocker.join("selection.png.tmp").exists(),
            "一句拒绝留下了一半的文件"
        );

        let drawn = include_str!("../qml/AnnotationToolbar.qml");
        let cells: Vec<&str> = drawn
            .lines()
            .map(str::trim)
            .filter(|t| t.starts_with("label:"))
            .collect();
        assert!(
            cells.iter().any(|t| *t == r#"label: "保存""#)
                && cells.iter().any(|t| *t == r#"label: "复制""#),
            "两句退路指的是工具栏上没有的两枚钮：{cells:?}"
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
    ///
    /// 第 4 步之后 `nudge_by` 交的不只是"挪没挪"，还有**放大镜这一趟该采哪一颗**：下面三枚
    /// `Some` 与它上面那一枚矩形是同一趟算出来的，挪对了而那颗点没跟着走，屏幕上就是"框在 A、
    /// 放大镜在 B"。这一格里 `cursor_phys` 一直是 `None`（没人 hover 过），所以没挪动竖边的
    /// 那一趟取的是那一头的中点、而不是指针 —— 指针那一路由上面 `the_pointer_leg_still_samples_the_pointer` 判。
    #[test]
    fn arrows_move_and_a_modifier_resizes() {
        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::new(400, 400, 800, 600), desk);
        let one = m.nudge_by(1, 0, false);
        assert_eq!(m.hole, PhysRect::new(401, 400, 800, 600));
        assert_eq!(
            one,
            Some(PhysPoint::new(1200, 700)),
            "§5.3.9 第 4 步：往右挪的那一趟，放大镜该跟到刚被挪掉的那条竖边上"
        );
        let shrunk = m.nudge_by(0, -10, true);
        assert_eq!(m.hole, PhysRect::new(401, 400, 800, 590));
        assert_eq!(
            shrunk,
            Some(PhysPoint::new(801, 989)),
            "Alt 那一档收的是下边，放大镜跟的是那一条边（401+590-1），不是没动的上边"
        );
        // Pinned to the right edge, a further push right cannot take it out.
        m.set_hole(PhysRect::new(2272, 100, 800, 600));
        let pinned = m.nudge_by(50, 0, false);
        assert_eq!(m.hole.right(), 3072);
        assert_eq!(
            pinned,
            Some(PhysPoint::new(3071, 400)),
            "夹在桌面右沿上的那一趟，报的是屏幕上真的那一列（3071），不是要去的那一列（3121）"
        );
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

    /// §5.5.9 的替代分支：[`MaskState::share_out`] 那三道梯里**手还没碰到 shell** 的那几级。
    ///
    /// 正面那一臂不在单测里，守的是 [`platform_windows::open::browser`] 头顶写死的同一条规矩：
    /// 一次真的 `open::file` 会在用户桌面上开出一枚**别的进程**的窗，而单测跑完关不掉它。这一格
    /// 因此只量落盘那一级之上和之中的三种拒绝 —— 它们全在 shell 之前，所以既不开窗、也留不下文件。
    ///
    /// 另有一件要判的事：`quick_out` 那句拒绝被**原样**递回，这一格不替它补第二句（P35 那一族
    /// 「两条路对一张图说两句话」）。所以每一臂都断言「交给系统」那半句**不在**话里 —— 一句写了
    /// 出去的谎，比一枚按下去不动的钮坏得多（#494 管的是前者，#261 管的是这里不能没有证）。
    #[test]
    fn a_share_refuses_on_every_rung_before_it_touches_the_shell() {
        let dir = sandbox("share");
        let naming = crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".into(),
            prefix: "share".into(),
            collision: falcon_core::naming::Collision::Increment,
        };

        // 一、还没有选区。
        let mut m = savable(PhysRect::default());
        m.naming = naming.clone();
        assert!(!m.share_out(), "{:?}", m.message);
        assert!(m.message.contains("还没有选区"), "{:?}", m.message);
        assert!(m.landed.is_none(), "一次没选区的分享落了一份文件");
        assert_eq!(m.quick_saves, 0, "一次没选区的分享花掉了编号");

        // 二、洞画了、那一层没有画面可拼。这一臂的话**不带**退路（§8.3 那一格立的界线），
        // `share_out` 也就一处不补：它递回的是 `quick_out` 那一句原话。
        let mut bare = state_with(
            vec![slot(0, PhysRect::new(0, 0, 640, 480), 1.0)],
            PhysRect::default(),
        );
        bare.naming = naming.clone();
        bare.set_hole(PhysRect::new(10, 20, 100, 80));
        assert!(!bare.share_out(), "{:?}", bare.message);
        assert!(bare.message.contains("拼不出来"), "{:?}", bare.message);
        assert!(!bare.message.contains("换一处目录"), "{:?}", bare.message);
        assert!(bare.landed.is_none(), "一次拼不出图的分享落了一份文件");

        // 三、图拼得出来、目录写不进去：把 `[output] save_dir` 指到一枚**文件**上，
        // `encode::save` 那一级 `create_dir_all` 当场回 183。这一臂是 §8.3 那三类的第三类，
        // 话该说全（原因 + 截图还在 + 两条退路），而**没有**「交给系统」那半句。
        let blocker = dir.join("occupied.png");
        std::fs::write(&blocker, b"not a directory").expect("blocker");
        let mut stuck = savable(PhysRect::new(10, 20, 100, 80));
        stuck.naming = naming.clone();
        stuck.naming.dir = blocker.to_string_lossy().into_owned();
        assert!(!stuck.share_out(), "{:?}", stuck.message);
        let said = stuck.message.clone();
        assert!(said.contains("路径无效"), "{said} 没说它是哪一类失败");
        assert!(
            said.contains("那个目录的位置上站着的是别的东西"),
            "{said}：这一臂是 `create_dir_all` 撞在一枚文件上（本机回 183 / `AlreadyExists`），\
             话该指到那一格，不是泛指找不到"
        );
        assert!(said.contains("截图还在"), "{said} 没接住「还在」那半句");
        assert!(said.contains("换一处目录"), "{said} 没递出第一条退路");
        assert!(
            !said.contains("交给系统"),
            "{said}：文件一份没落成，却已经对用户说递出去了"
        );
        assert_eq!(stuck.quick_saves, 0, "一次写不进去的分享花掉了编号");

        // 三道梯都在 shell 之前，所以这一档从头到尾没有在这台机器上多留一枚文件。
        assert_eq!(
            std::fs::read_dir(&dir)
                .expect("dir")
                .filter_map(Result::ok)
                .count(),
            1,
            "除了那枚挡路的文件，这一档一份都不该写"
        );
    }

    /// §5.5.9 的替代分支在页上的那一枚入口：接线在，而**话不在**。
    ///
    /// 读的是 `AnnotationToolbar.qml` 那份源码，理由与工具栏每一档一样：这一族的三道梯（落盘、
    /// 命名、话）全在 [`MaskState::share_out`] 那一格，QML 只需要把手搭过去。`hint` 那句是这一枚
    /// 钮唯一的解释，它说的是「交给系统默认的看图程序」而不是任何**具体目标**的名字 —— 下面那一排
    /// `share_targets` 的名单仍是 `Text`，页上要是冒出一句「已分享给 slack」，那就是屏幕上有一句假话。
    #[test]
    fn the_toolbar_wires_the_share_exit_without_retelling_it() {
        let drawn = include_str!("../qml/AnnotationToolbar.qml");
        let lines: Vec<&str> = drawn.lines().map(str::trim).collect();
        assert!(
            lines.contains(&"function shareOut() {") && lines.contains(&"bar.view.shareOut()"),
            "「打开方式」那枚入口没有接到 mask 门上"
        );
        let at = lines
            .iter()
            .position(|t| *t == r#"label: "打开方式""#)
            .expect("入口不在");
        let cell = &lines[at..at + 4];
        assert!(
            cell.contains(&"onTapped: bar.shareOut()")
                && cell.contains(&"usable: bar.view.selected"),
            "入口在而不按，或者没有选区时不按：{cell:?}"
        );

        for shell in ["交给系统的默认程序", "那份文件还在盘上"] {
            assert!(
                !drawn.contains(shell),
                "Rust 那句「{shell}」在 QML 里被重打了一遍"
            );
        }
    }

    /// §5.5.10 步骤 2/3 那道梯在**动手之前**的那一格：没有选区的一次点击要说"还没有选区"，
    /// 而那一句要落在 `send_to_last` 上而不是 `message` 上 —— 后者是"已保存到 …"那句证据的
    /// 槽位（[`MaskState::reveal_last`]、[`MaskState::auto_copy_last`] 为同一条理由各占过一格）。
    #[test]
    fn a_send_to_click_without_a_selection_says_so_without_hiding_the_save() {
        let mut m = MaskState::default();
        assert!(!m.send_to_target(0));
        assert_eq!(m.send_to_clicks, 1, "失败的一次点击不该不计数");
        assert_eq!(m.send_to_acted, 0, "没有选区的一次点击递到了 shell");
        assert!(
            m.send_to_last.contains("还没有选区"),
            "话没说出来：{:?}",
            m.send_to_last
        );
        assert_eq!(m.message, "", "派发的那句话挤掉了保存的那句证据");
    }

    /// 序号越界那一臂答在**写文件之前**：清单上没有第 N 枚时，一次点击既不该往 temp 里落一张
    /// 没人接的图，也不该走到 shell。索引取 999 而不是 0，是因为这一格不能依赖这台机器的
    /// `[output] send_to_apps` 此刻配了几枚 —— 那句话里报出的枚数由实现自己数，测试只判臂。
    #[test]
    fn an_out_of_range_target_is_refused_before_a_byte_is_written() {
        let mut m = savable(PhysRect::new(10, 20, 30, 40));
        assert!(
            !m.send_to_target(999),
            "清单上没有的第 999 枚被当成递出去了"
        );
        assert_eq!(m.send_to_clicks, 1);
        assert_eq!(m.send_to_acted, 0);
        assert_eq!(m.send_to_writes, 0, "一次越界的点击落了一张盘上的文件");
        assert!(
            m.send_to_last.contains("不在这份清单"),
            "话没说出来：{:?}",
            m.send_to_last
        );
        assert!(m.send_to_landed.is_none(), "越界那一臂留下了临时文件");
    }

    /// 那枚目录是本程序自己打了标签的一处，而"收回"删得着的只可能是它里面的东西 —— 与
    /// [`crate::state::PinState::clip_dir`] 同一枚安全口径：不替用户动他自己在 temp 里的文件。
    #[test]
    fn the_send_to_directory_is_ours_and_tagged() {
        let dir = MaskState::sendto_dir();
        assert!(dir.starts_with(std::env::temp_dir()), "{dir:?}");
        assert_eq!(
            dir.file_name().map(|s| s.to_string_lossy().into_owned()),
            Some("falconshot-sendto".to_string())
        );
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

    /// §5.16 那枚 `[advanced] recognize_text` 的第一分岔（要不要花一次引擎），P148 起。
    /// 纯函数 ⇒ 四种形状都不开库、不起线程、也不需要本机 OCR 引擎在场（§9.1 第十条的
    /// headless 半边）。
    #[test]
    fn the_auto_ocr_switch_spends_an_engine_run_only_when_it_should() {
        assert!(!auto_ocr_wanted(false, false), "关着的键反倒要跑");
        assert!(!auto_ocr_wanted(false, true), "关着又是取消的一行, 更要跑?");
        assert!(
            auto_ocr_wanted(true, false),
            "开着、这一行是确认的, 反倒不跑"
        );
        assert!(
            !auto_ocr_wanted(true, true),
            "Esc 掉的那一行没有第二位读者, 不该花一次引擎"
        );
    }

    /// 第二分岔：引擎那句话翻成那一格的值。三种形状必须分得开 —— **没跑成**（NULL）、
    /// **跑成而全空白**（空串）、**跑成而有字**（原样, 换行留着）。把第二种塌成第一种,
    /// §5.6 那一列自此说不出「跑过没有」；把第二种留成空白串, 界面读回来会说「这一行有
    /// 文字」而用户看见的是空的。
    #[test]
    fn a_blank_recognition_is_not_the_same_shape_as_no_recognition() {
        assert_eq!(auto_ocr_slot(None), None, "没跑成却交回了一格值");
        assert_eq!(
            auto_ocr_slot(Some(String::new())),
            Some(String::new()),
            "跑成而没字, 不该被说成没跑成"
        );
        assert_eq!(
            auto_ocr_slot(Some("  \n\t ".to_string())),
            Some(String::new()),
            "一串空白折不成空串"
        );
        assert_eq!(
            auto_ocr_slot(Some("第一行\n第二行".to_string())),
            Some("第一行\n第二行".to_string()),
            "§5.6.1 要保留自然段和换行, 这里不许把它们抹平"
        );
    }

    /// 落笔那一步对面是**真的那一格 SQLite**：[`write_auto_ocr`] 就是线程上跑的同一只手,
    /// 这里把 `Some(…)` 直接递进来, 于是「字进没进它点名的那一行」有一位不必真跑引擎的证人。
    /// 四件事各判一次：新行是 NULL、有字写进去、没跑成**不清**已有的字、空白写成空串;
    /// 外加库里没有的那一行只回 `Err` 而不响。
    #[test]
    fn the_auto_ocr_write_lands_on_the_row_it_names_and_leaves_the_one_it_does_not() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let hdir = sandbox("auto-ocr-write");
        let mut m = savable(hole);
        let cfg = falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        };
        m.history = Some(cfg.clone());
        m.history_dir = hdir.clone();
        // 键在这一格**关着**：这一趟要的是「一行带着 NULL 进来」, 而不是顺带起一条线程。
        assert!(
            !m.advanced.recognize_text,
            "出厂默认就该是关着的, 那句『默认什么都不做』指着这一格"
        );
        assert_eq!(m.commit(), Some(hole), "确认没交出选区, 历史也就没有那一行");
        let id = m.history_last_id.expect("§5.14.1 那一行没记成");
        assert_eq!(m.auto_ocr_asked, 0, "关着的键在确认那一趟里交出去过一次");
        assert!(
            !m.history_last.contains("识别"),
            "关着的键那句话里说了识别: {:?}",
            m.history_last
        );

        let h = crate::settings::history_open_in(&hdir, &cfg)
            .expect("库打不开")
            .expect("开着却答 None");
        assert_eq!(
            h.get(id).unwrap().unwrap().ocr_text,
            None,
            "新记成的那一格该是 NULL"
        );

        write_auto_ocr(&h, id, Some("总价 12.00\n第二行".to_string())).expect("有字那趟没写成");
        assert_eq!(
            h.get(id).unwrap().unwrap().ocr_text.as_deref(),
            Some("总价 12.00\n第二行"),
            "写进了那一行却没读回来"
        );

        write_auto_ocr(&h, id, None).expect("没跑成那一趟不该响");
        assert_eq!(
            h.get(id).unwrap().unwrap().ocr_text.as_deref(),
            Some("总价 12.00\n第二行"),
            "引擎失败反倒把上一趟跑出来的字清了"
        );

        write_auto_ocr(&h, id, Some("   \n  ".to_string())).expect("空白那趟没写成");
        assert_eq!(
            h.get(id).unwrap().unwrap().ocr_text.as_deref(),
            Some(""),
            "空白没落成空串"
        );

        assert!(
            write_auto_ocr(&h, id + 9_999, Some("x".to_string())).is_err(),
            "库里没有的那一行反倒写成了"
        );
    }

    /// 键开着那一条腿：确认一趟交出去**一次**, 那句话只说「起了一次本机识别」而不说
    /// 「识别出了什么」（这一格在这条腿上还没有读数可说）, 而那条线程在本趟 **join 掉** ——
    /// 它写的正是这一趟那个临时目录, 让它活过测试就成了这位没人等的证人。
    ///
    /// 引擎在这台机器上到底跑不跑得得出字, 本项**不判**：判的是这条腿不占 GUI 线程、
    /// 只在开键时起、以及线程自己不 panic。字落没落进那一格由上一项作证。
    #[test]
    fn an_open_switch_hands_the_recognition_off_without_making_the_gui_wait() {
        let hole = PhysRect::new(10, 20, 100, 80);
        let hdir = sandbox("auto-ocr-handoff");
        let mut m = savable(hole);
        m.history = Some(falcon_core::config::History {
            max_items: 0,
            max_age_days: 0,
            max_size_mb: 0,
            ..Default::default()
        });
        m.history_dir = hdir.clone();
        m.advanced.recognize_text = true;

        assert_eq!(m.commit(), Some(hole), "确认没交出选区");
        assert_eq!(m.auto_ocr_asked, 1, "开着只该交出去一次");
        assert!(
            m.history_last.contains("recognize_text") && m.history_last.contains("不联网"),
            "那句话没说它是按哪枚键起的、也没说本机: {:?}",
            m.history_last
        );
        let handle = m.auto_ocr_join.take().expect("交出去了却没有把手");
        assert!(handle.join().is_ok(), "那条线程 panic 了");
        assert_eq!(m.auto_ocr_asked, 1, "join 挪了交出去的次数");
    }

    // §5.4.1 / §5.4.2 —— 桥层的三条不碰 Qt 就能判的：
    //   ① `pick_color` 在 `cursor_phys` 没落地前不能"什么也不说地返回 false"，它得把
    //      为什么取不了颜色那句话留在 `message` 上（页上的 `message` 栏是唯一读者）；
    //   ② `magnifier_at` 在**没有快照**时不能假装放大镜开着；
    //   ③ §5.19.3 的"是否显示放大镜"这一格关掉之后，`magnifier_at` 要回答 `show=false`
    //      并且**不能**留下一张新纹理 —— 这一条是这一族配置里唯一原本没有读者的那一格。
    //
    // 这三条都停在桥自己的搬运算法上，core 那一族的像素/字符串装配在
    // `falcon_core::capture::tests` 里已经判过了（`magnifier_view_names_the_pixel_the_coordinate_and_the_colour`
    // / `colour_label_is_the_string_the_picker_copies` / `colour_label_and_magnifier_view_agree_at_the_seam`）。
    // 不测到 `shim::store_raw` 那一步：单元测试里没有 Qt 事件循环，那一手会走进 C++ 侧，
    // 桥里"能不能真的把纹理送到 Qt 屏幕上"这件事仍然只能靠真机 `--mask` 跑一次。

    #[test]
    fn pick_color_without_cursor_says_why() {
        let mut m = MaskState::default();
        // 没快照、没指针 —— 第一件事就撞上"指针还没落到遮罩上"。
        assert!(!m.pick_color(), "光标都没落地，取色器不能假装成功");
        assert!(
            m.message.contains("光标"),
            "答案要说清为什么取不了，实际说的是：{}",
            m.message
        );
    }

    #[test]
    fn pick_color_without_snapshot_says_why() {
        // 光标在、快照不在了（单测里手搭的 `MaskState` 就是这个形状，`open` 从未跑过）：
        // 这一档要说"快照没了"，不能只说"取不了"——两种失败在门后各自一句话。
        let mut m = MaskState::default();
        m.cursor_phys = Some(PhysPoint::new(10, 10));
        assert!(!m.pick_color());
        assert!(
            m.message.contains("快照"),
            "没有快照时要说清是没快照，实际说的是：{}",
            m.message
        );
    }

    #[test]
    fn pick_color_off_any_screen_says_where() {
        // 快照在、光标"在桌面里" —— 但落在这两块屏之外（跨屏缝），
        // [`ScreenSnapshot::color_label`] 回 `None`，桥要把那一点报出来。
        let mut m = MaskState::default();
        m.snapshot = Some(snapshot(vec![monitor("a", 0, 0, 100, 100, 1.0, true)]));
        m.cursor_phys = Some(PhysPoint::new(500, 500));
        assert!(!m.pick_color());
        assert!(
            m.message.contains("500, 500"),
            "跨屏缝外要说清是哪一点，实际说的是：{}",
            m.message
        );
    }

    #[test]
    fn magnifier_off_the_config_leaves_no_new_key() {
        // §5.19.3 那一格"是否显示放大镜"关掉之后：`magnifier_at` 说 `show=false`、
        // 而且桥**不给**这一窗留新键。这是这一格配置原本没有读者、这一轮给它找到读者的
        // 那一条判据。
        let mut m = state_with(
            vec![slot(0, PhysRect::new(0, 0, 100, 100), 1.0)],
            PhysRect::default(),
        );
        m.snapshot = Some(snapshot(vec![monitor("a", 0, 0, 100, 100, 1.0, true)]));
        // `desk_point` 把点夹进 `desktop()`，而 `desktop()` 读的是 `virtual_bounds` ——
        // 真实那一趟在 `show()` 里从快照取（`self.virtual_bounds = snap.virtual_bounds`），
        // 这一格的夹具也要从同一处取，否则默认那枚空矩形会把 (50,50) 夹成 (0,0)，
        // 判出来的就不是「放大镜关了以后指针还在哪」而是夹具自己造的洞。
        m.virtual_bounds = m.snapshot.as_ref().unwrap().virtual_bounds;
        m.capture.magnifier_enabled = false;
        let d = m.magnifier_at(0, 50.0, 50.0, &MagnifierLook::default());
        assert!(!d.show, "关掉放大镜之后 `show` 不能再真");
        assert!(d.key.is_empty(), "关掉放大镜之后不能留下新纹理键");
        assert!(
            m.slots.iter().all(|s| s.mag_key.is_empty()),
            "关着放大镜时，槽位里的 `mag_key` 也该是空"
        );
        // 指针仍然被记下 —— 这是 §5.4.2「取色器读放大镜那一颗」的**同一枚**坐标；
        // 关掉放大镜不代表关掉"光标此刻在哪"这本账，按 C 还是要能拿到颜色。
        assert_eq!(
            m.cursor_phys,
            Some(PhysPoint::new(50, 50)),
            "配置关了放大镜也不该关掉取色器要用的那一点"
        );
    }

    #[test]
    fn magnifier_at_a_missing_slot_returns_default() {
        // `desk_point` 找不到槽位 → 放大镜这一趟**没**生成任何东西，返回 default。
        let mut m = MaskState::default();
        let d = m.magnifier_at(0, 50.0, 50.0, &MagnifierLook::default());
        assert_eq!(d, MagnifierData::default());
    }

    // §5.3.9 第 4 步「键盘微调选区时，放大镜应同步显示边缘像素」—— 桥里那两条腿的判据。
    //
    // 「刚被挪掉的那条边在哪一列」那一颗点本身住在 `falcon_core::geometry::nudge_sample`，
    // 四枚箭头 / 夹在桌面边上 / Alt 只挪一条边 / 零位移交回指针，都在那一档以纯函数判过。
    // 这一组判的是**剩下那一段**：桥交出的一份读数（面板摆在哪一格 = `anchor`）与
    // [`MaskState::cursor_phys`] 记下的那一点，在键盘那一趟之后必须正好是**那条边**，而不再
    // 是指针停留的那一颗 —— 缺陷的形状是"框挪走了、放大镜没同步"，判据就得量那两格。
    //
    // 夹具一律 `magnifier_enabled = false` 且**不放快照**：[`MaskState::magnifier_at_point`]
    // 那四条早退在把 `anchor` 与那一点记下**之后**才发生，于是 `shim::store_raw` 一次也不会
    // 被叫到（同上面 `magnifier_off_the_config_leaves_no_new_key` 的理由，§9.1 第十条的
    // headless 半边）。放大出来的那些像素与坐标字符串是 core 的 `magnifier_view` 的活。

    /// 一扇 `zoom` 倍的屏 + 一块选区，放大镜那一族**关着**的夹具：这一组判的是**哪一颗点**，
    /// 不是画不画。桌面取满屏，免得夹桌面把夹具自己先夹一遍。
    fn keyboard_leg(zoom: f64, hole: PhysRect) -> MaskState {
        let desk = PhysRect::new(0, 0, 3072, 1920);
        let mut m = selectable(vec![slot(0, desk, zoom)], hole, desk);
        m.capture.magnifier_enabled = false;
        m
    }

    /// 200% 屏（本机那一扇）上按一次 →：选区右移一格，放大镜那一点与面板的落点**都**跟着
    /// 走到刚被挪掉的那条竖边上。指针停在选区外那一颗就是缺陷发生时的那一颗，它不许留在答案里。
    #[test]
    fn a_keyboard_nudge_re_anchors_the_magnifier_to_the_moved_edge() {
        let mut m = keyboard_leg(2.0, PhysRect::new(400, 400, 800, 600));
        m.cursor_phys = Some(PhysPoint::new(40, 40));
        let d = m
            .nudge_magnified(0, 1, 0, false, &MagnifierLook::default())
            .expect("有选区就该交出一份读数");
        assert_eq!(m.hole, PhysRect::new(401, 400, 800, 600), "选区本身先要挪对");
        // 右边 = 401 + 800 - 1（选区**自己的**最后一列），另一根轴 = 这一条边的中点 400+300。
        assert_eq!(
            m.cursor_phys,
            Some(PhysPoint::new(1200, 700)),
            "键盘把选区挪走了、放大镜还停在指针那一颗 —— 这一格量的就是 §5.3.9 第 4 步那一句"
        );
        // 面板摆在哪一格走的是 [`MaskState::local_dip`] 那一位作者：200% 屏上物理 1200 = DIP 600。
        assert_eq!(
            (d.anchor_x, d.anchor_y),
            (600.0, 350.0),
            "读数的落点与那一点不同源，就是同一件事的两个答案"
        );
        assert_ne!(d.anchor_x, 20.0, "指针那一颗(40, 40)不许冒充答案");
        assert!(!d.show, "夹具里放大镜是关着的");
        assert!(d.key.is_empty(), "关着放大镜不该留下新纹理键");
    }

    /// 四枚方向键各把面板搬到**自己那一条**边上。100% 屏那一点与面板落点是同一个数，
    /// 这一格因此读起来不用先做一遍除法；与上面那一格合起来，"哪条腿、哪个 scale"都不偏心。
    #[test]
    fn every_keyboard_arrow_moves_the_panel_onto_the_edge_that_moved() {
        for (dx, dy, want) in [
            (1, 0, PhysPoint::new(300, 150)),
            (-1, 0, PhysPoint::new(99, 150)),
            (0, -1, PhysPoint::new(200, 99)),
            (0, 1, PhysPoint::new(200, 200)),
        ] {
            let mut m = keyboard_leg(1.0, PhysRect::new(100, 100, 200, 100));
            m.cursor_phys = Some(PhysPoint::new(3, 7));
            let d = m
                .nudge_magnified(0, dx, dy, false, &MagnifierLook::default())
                .expect("有选区");
            assert_eq!(m.cursor_phys, Some(want), "{dx},{dy} 之后该看刚挪掉的那条边");
            assert_eq!(
                (d.anchor_x, d.anchor_y),
                (want.x as f64, want.y as f64),
                "{dx},{dy}：面板落点与那一点分家了"
            );
            assert_ne!(
                (d.anchor_x, d.anchor_y),
                (3.0, 7.0),
                "{dx},{dy} 之后还报指针，就是「键盘挪了、放大镜没同步」"
            );
        }
    }

    /// Alt 那一档挪的是右下两条边：`nudge_by` 里那一枚 `Handle::Se` 必须**同一次**递给
    /// 挪与递给放大镜（分两处写就是两个作者，而它们漂移的形状正是"框在 A、放大镜在 B"）。
    #[test]
    fn the_alt_arm_grows_one_edge_and_the_panel_follows_that_edge_only() {
        let mut m = keyboard_leg(1.0, PhysRect::new(100, 100, 200, 100));
        m.cursor_phys = Some(PhysPoint::new(3, 7));
        let d = m
            .nudge_magnified(0, 0, 1, true, &MagnifierLook::default())
            .expect("有选区");
        assert_eq!(m.hole, PhysRect::new(100, 100, 200, 101), "Alt 那一档长的是下边");
        assert_eq!(m.cursor_phys, Some(PhysPoint::new(200, 200)));
        assert_eq!((d.anchor_x, d.anchor_y), (200.0, 200.0));
        assert_ne!(d.anchor_y, 100.0, "一动未动的上边不许冒充「刚挪过的那条边」");
    }

    /// 夹在桌面右沿上的那一趟：报的是**夹住之后**那条真的边（999），不是按键要求去的那一列
    /// （1049）。这一格同时钉住"桥把**挪完之后**的那一块交进去"—— 交挪之前那一块就答不出这句。
    #[test]
    fn a_keyboard_nudge_pinned_at_the_desktop_edge_names_the_clamped_edge() {
        let desk = PhysRect::new(0, 0, 1000, 800);
        let mut m = selectable(vec![slot(0, desk, 1.0)], PhysRect::new(990, 790, 10, 10), desk);
        m.capture.magnifier_enabled = false;
        m.cursor_phys = Some(PhysPoint::new(10, 10));
        let d = m
            .nudge_magnified(0, 50, 0, false, &MagnifierLook::default())
            .expect("有选区");
        assert_eq!(
            m.hole,
            PhysRect::new(990, 790, 10, 10),
            "夹具要先证明这一趟被桌面夹住了：一格也没挪"
        );
        assert_eq!(m.cursor_phys, Some(PhysPoint::new(999, 795)));
        assert_eq!((d.anchor_x, d.anchor_y), (999.0, 795.0));
        assert_ne!(d.anchor_x, 1049.0, "报的是要求去的那一列，屏幕上根本没有它");
    }

    /// §5.4.1「放大镜跟指针走」在这一轮之后仍然是那件旧事：鼠标那条腿叫的仍是
    /// [`MaskState::magnifier_at`]，而键盘这条腿收到一枚**没有位移**的按键（`dx == dy == 0`，
    /// 产品里发不出来，但那一臂兜的就是"这不是按键、是指针"）时交回的必须还是指针那一颗 ——
    /// 一次没挪动任何东西的按键不许把面板从指针上掀走。
    #[test]
    fn the_pointer_leg_still_samples_the_pointer() {
        let mut m = keyboard_leg(2.0, PhysRect::new(400, 400, 800, 600));
        // 窗内 DIP (150, 75) 在 200% 屏上是桌面物理 (300, 150)：`desk_point` 那一位作者一字未改。
        let d = m.magnifier_at(0, 150.0, 75.0, &MagnifierLook::default());
        assert_eq!(m.cursor_phys, Some(PhysPoint::new(300, 150)), "鼠标那一颗还是指针");
        assert_eq!((d.anchor_x, d.anchor_y), (150.0, 75.0));
        let k = m
            .nudge_magnified(0, 0, 0, false, &MagnifierLook::default())
            .expect("有选区");
        assert_eq!(
            m.cursor_phys,
            Some(PhysPoint::new(300, 150)),
            "零位移那一趟把面板搬去了别处，鼠标那条腿就被改了口"
        );
        assert_eq!((k.anchor_x, k.anchor_y), (150.0, 75.0));
    }

    /// 没有选区可挪的那一趟交 `None`：调用方（[`crate::mask_view::MaskViewRust::nudge`]）据此
    /// **什么也不动**，放大镜留在它此刻那一颗上，而不是闪一下变成一块空面板。
    #[test]
    fn a_nudge_with_no_selection_leaves_the_panel_alone() {
        let mut m = keyboard_leg(2.0, PhysRect::default());
        m.cursor_phys = Some(PhysPoint::new(300, 150));
        assert_eq!(
            m.nudge_magnified(0, 1, 0, false, &MagnifierLook::default()),
            None
        );
        assert_eq!(
            m.cursor_phys,
            Some(PhysPoint::new(300, 150)),
            "一次没挪成任何东西的按键不该改那一点"
        );
        assert_eq!(m.hole, PhysRect::default());
    }

    /// 跨屏那一趟：键盘挪掉的边整个在**另一扇屏**上时，本窗报的是"这一扇屏上离那条边最近的
    /// 一格"，而不是面板摆在折出来的 1×1 之外（`local_dip` 与槽位之外那一臂会把面板送到看不见的
    /// 地方去）。两扇屏各 1536 宽，选区贴在第二扇屏的左沿上。
    #[test]
    fn a_nudge_onto_the_other_screen_anchors_on_this_screens_nearest_column() {
        let left = PhysRect::new(0, 0, 1536, 1920);
        let right = PhysRect::new(1536, 0, 1536, 1920);
        let mut m = selectable(
            vec![slot(0, left, 1.0), slot(1, right, 1.0)],
            PhysRect::new(1536, 100, 200, 100),
            PhysRect::new(0, 0, 3072, 1920),
        );
        m.capture.magnifier_enabled = false;
        m.cursor_phys = Some(PhysPoint::new(10, 10));
        // ← 一像素：那条边现在是 1535，仍在左扇屏里；下一趟再往左就整条跑到别的地方去了。
        let d = m
            .nudge_magnified(0, -1, 0, false, &MagnifierLook::default())
            .expect("有选区");
        assert_eq!(m.cursor_phys, Some(PhysPoint::new(1535, 150)));
        assert_eq!((d.anchor_x, d.anchor_y), (1535.0, 150.0));
        // → 十像素（Shift 那一档）把边推回第二扇屏：本窗（index 0）报的是自己那一扇的最右一列。
        let back = m
            .nudge_magnified(0, 10, 0, false, &MagnifierLook::default())
            .expect("有选区");
        assert_eq!(back.anchor_x, 1535.0, "面板该夹在本窗那扇屏上，别跑到屏外");
        assert_eq!(m.cursor_phys, Some(PhysPoint::new(1535, 150)));
    }

    // §5.19.3「放大镜外观」那四枚键 —— `magnifier_border` / `magnifier_corner_radius` /
    // `magnifier_foreground` / `magnifier_background` 在 core 里建了键、在启动横幅里被念过，
    // 但页上 `magPanel` 画的是四枚写死的字面，所以「档里改了、屏幕上什么都不动」。下面这一组
    // 判的是桥把这四枚键翻译成面板读数的那只手：[`MagnifierLook::from_appearance`] 是纯的
    // （不碰档、不碰锁、不碰 Qt），[`MaskState::magnifier_at`] 只把它的答案原样带给自己那四条
    // 早退。四枚键**落到屏幕上**那一腿仍然只有真机 `--mask` 能判（第 46 项 ② 那一族）。

    /// 一份只动了外观那四枚键的 `Appearance`。默认值由
    /// [`falcon_core::config::Appearance::default`] 交出 —— 测试不抄第二份默认值（#281）。
    fn appearance(
        foreground: &str,
        background: &str,
        border: u32,
        radius: u32,
    ) -> falcon_core::config::Appearance {
        let mut a = falcon_core::config::Appearance::default();
        a.magnifier_foreground = foreground.into();
        a.magnifier_background = background.into();
        a.magnifier_border = border;
        a.magnifier_corner_radius = radius;
        a
    }

    #[test]
    fn magnifier_look_reads_the_four_appearance_keys_into_the_panel() {
        let look = MagnifierLook::from_appearance(&appearance("#FF0000", "#00FF00", 3, 12));
        assert_eq!(look.foreground, "#FF0000", "前景色没走到面板那一枚读数上");
        assert_eq!(look.background, "#00FF00", "背景色没走到面板那一枚读数上");
        assert_eq!(look.border, 3, "边框宽度没走到面板那一枚读数上");
        assert_eq!(look.radius, 12, "圆角没走到面板那一枚读数上");
    }

    #[test]
    fn magnifier_look_lands_on_the_shipped_defaults_when_nothing_is_written() {
        // 没动过的档（= 用户从来没配过放大镜长相）与**空串**（= 档里那一行被写成
        // `magnifier_background = `）都要落在出厂那一份上，而且那一份的来源是
        // [`falcon_core::config::Appearance::default`]、不是本文件里再抄一遍的四枚字面。
        let shipped = falcon_core::config::Appearance::default();
        let unset = MagnifierLook::from_appearance(&shipped);
        let blank = MagnifierLook::from_appearance(&appearance("", "", 1, 6));
        assert_eq!(
            unset, blank,
            "空串没退到出厂那一枚，实际拿到的是：{blank:?}"
        );
        assert_eq!(
            unset,
            MagnifierLook::default(),
            "`MagnifierLook::default()` 与出厂那一份不是同一块面板"
        );
        assert_eq!(
            unset.background, "#1F1F1F",
            "出厂背景读出来不是档里写的那一枚"
        );
        assert_eq!(
            unset.foreground, "#FFFFFF",
            "出厂前景读出来不是档里写的那一枚"
        );
        assert_eq!(unset.border, 1, "出厂边框宽度读出来不是一");
        assert_eq!(unset.radius, 6, "出厂圆角读出来不是六");
    }

    #[test]
    fn magnifier_look_falls_back_on_a_colour_it_cannot_parse_instead_of_blanking_the_panel() {
        // 手改坏的档（`magnifier_foreground = notacolor`）不能让面板变成一块没有颜色
        // 的矩形：页上那两行字与描边落回出厂那一枚，边框宽度与圆角照旧跟着档走。
        let a = appearance("notacolor", "rgb(31, 31, 31)", 4, 20);
        let look = MagnifierLook::from_appearance(&a);
        let shipped = falcon_core::config::Appearance::default();
        assert_eq!(
            look.foreground, shipped.magnifier_foreground,
            "读不出来的前景色没有落回出厂那一枚，实际是：{}",
            look.foreground
        );
        assert_eq!(
            look.background, "#1F1F1F",
            "rgb() 那一族没被档的解析器归一成 #RRGGBB"
        );
        assert!(
            !look.foreground.is_empty() && !look.background.is_empty(),
            "面板不能因为一档坏串就拿到空颜色：{look:?}"
        );
        assert_eq!(look.border, 4, "一枚颜色坏了不该顺手动掉边框宽度");
        assert_eq!(look.radius, 20, "一枚颜色坏了不该顺手动掉圆角");
    }

    #[test]
    fn magnifier_look_normalizes_every_form_the_config_parser_understands() {
        // 归一这一步只有一个主人：[`falcon_core::colors::parse`]。页上拿到的是同一种写法，
        // 所以 `#abc` / `rgb()` / `hsl()` 三族与 `#RRGGBB` 在屏幕上长得一样（#281）。
        assert_eq!(
            MagnifierLook::from_appearance(&appearance("#abc", "#1F1F1F", 1, 6)).foreground,
            "#AABBCC",
            "三位短十六没被展开成六位"
        );
        assert_eq!(
            MagnifierLook::from_appearance(&appearance("#FFFFFF", "hsl(0, 0%, 12%)", 1, 6))
                .background,
            "#1F1F1F",
            "hsl() 与 #RRGGBB 这两种写法画出了两块不同的底"
        );
        // 进出两种写法：档里那枚是 CSS 的 `#RRGGBBAA`，页上那一臂是 Qt 的 `#AARRGGBB`。
        // alpha 只在不是 255 时才写出来 —— 丢掉就是丢掉用户写下的那两位。
        assert_eq!(
            MagnifierLook::from_appearance(&appearance("#FFFFFF", "#FF000080", 1, 6)).background,
            "#80FF0000",
            "半透明的底被当成不透明画了，或两位 alpha 换错了边"
        );
        assert_eq!(
            MagnifierLook::from_appearance(&appearance(
                "rgba(255, 255, 255, 0.5)",
                "#000000",
                1,
                6
            ))
            .foreground,
            "#80FFFFFF",
            "rgba() 的 alpha 没有走到页上认的那一种写法"
        );
    }

    #[test]
    fn magnifier_at_carries_the_look_into_the_closed_panel() {
        // §5.19.3 的"是否显示放大镜"关掉之后面板不画，但它**的颜色**不能因此变成空串：
        // `show=false` 那四条早退也带着递进来的那一份，页上的绑定在任何一趟都拿到一块
        // 画得出来的面板（同 `#1F1F1F` 那一族「不画 ≠ 没有值」的口径）。
        let look = MagnifierLook::from_appearance(&appearance("#00FF00", "#FF0000", 5, 30));
        let mut m = MaskState::default();
        let d = m.magnifier_at(0, 50.0, 50.0, &look);
        assert!(!d.show, "没有槽位时不能假装放大镜开着");
        assert_eq!(
            d.look, look,
            "早退把递进来的外观丢了，面板会在第一帧拿到空颜色：{d:?}"
        );
    }

    // ------------------------------------------------------- §5.3.2 遮罩这一腿
    //
    // 两条排除规则与 z-order 的次序**不在这里判**（它们在 `falcon_core::capture`，各自的
    // 测试也在那里）。这一组判的是遮罩这一腿接没接上：hover 报不报那块边界、单击选不选
    // 整窗、拖选那条腿有没有被顶掉、以及 `[capture] snap_to_window` 是不是真开关。
    // 用的都是**手搭的候选表** —— `hover_window` 只读 `self.windows`，一次也不问桌面，
    // 所以这一族能在没有窗、没有 Qt 的地方判（§9.1 第十条的 headless 半边）。

    /// 一扇候选窗。默认全是「可截」的形状，每一枚测试只改它要判的那一格。
    fn win(hwnd: u32, z_order: usize, bounds: PhysRect) -> falcon_core::capture::WindowInfo {
        falcon_core::capture::WindowInfo {
            hwnd,
            title: format!("w{hwnd}"),
            bounds,
            dwm_bounds: None,
            app_name: format!("app{hwnd}"),
            process_path: String::new(),
            pid: 1_000 + hwnd,
            minimized: false,
            focused: false,
            z_order,
            own_process: false,
            opacity: None,
        }
    }

    /// 一扇 100% 的单屏遮罩 + 两扇叠在一起的窗（上面那扇小、只盖住左上角一块）。
    fn hovering() -> MaskState {
        let desk = PhysRect::new(0, 0, 1_000, 800);
        let mut m = MaskState {
            slots: vec![slot(0, desk, 1.0)],
            // 遮罩开门时带着默认选区，这一格要的是「还没有任何读数」那一种状态。
            hole: PhysRect::default(),
            virtual_bounds: desk,
            ..Default::default()
        };
        m.windows = vec![
            win(7, 0, PhysRect::new(20, 20, 200, 150)),
            win(8, 1, PhysRect::new(0, 0, 500, 400)),
        ];
        m
    }

    /// §5.3.2 第二步与第三步：光标走到一扇窗上，桥报出**那一扇**的可截图边界，单位是
    /// 这一扇遮罩窗自己的窗内 DIP。
    #[test]
    fn hovering_a_window_reports_its_bounds_in_this_windows_dip() {
        let mut m = hovering();
        assert!(m.hover_window(0, 30.0, 30.0), "这一点上那两扇都盖得住");
        // 命中 z-order 最靠前的 7 号（判据在 core，这里只判「报没报」）。
        assert_eq!(m.hover_win, Some((7, PhysRect::new(20, 20, 200, 150))));
        assert_eq!(
            m.hover_bounds(0),
            Some(PhysRect::new(20, 20, 200, 150)),
            "100% 屏上窗内 DIP 与桌面物理同数"
        );

        // 走出 7 号、还在 8 号里 ⇒ 换成下面那一扇，不是留着上一扇的框。
        assert!(m.hover_window(0, 400.0, 300.0));
        assert_eq!(m.hover_win.map(|(h, _)| h), Some(8));
        // 两扇都不在了 ⇒ 没有可亮的，且**旧的那一格不能留在场上**。
        assert!(!m.hover_window(0, 900.0, 700.0));
        assert_eq!(m.hover_win, None);
        assert_eq!(m.hover_bounds(0), None);
    }

    /// 混合缩放：同一块桌面几何在 200% 那扇窗上是一半大的窗内 DIP。高亮框与选区共用
    /// [`MaskState::local_dip`] 那一位作者，所以这里算出来的数与洞画在哪一格不可能差一像素。
    #[test]
    fn the_highlight_and_the_hole_use_the_same_ruler_at_mixed_scale() {
        let desk = PhysRect::new(0, 0, 2_000, 1_600);
        let mut m = MaskState {
            slots: vec![slot(0, desk, 2.0)],
            hole: PhysRect::default(),
            virtual_bounds: desk,
            ..Default::default()
        };
        m.windows = vec![win(9, 0, PhysRect::new(200, 100, 400, 300))];
        // 窗内 (110, 60) × 2 = 桌面 (220, 120)，在那扇窗里。
        assert!(m.hover_window(0, 110.0, 60.0));
        assert_eq!(
            m.hover_bounds(0),
            Some(PhysRect::new(100, 50, 200, 150)),
            "桌面 200,100 400x300 ÷ 2 = 窗内 100,50 200x150"
        );
        m.set_hole(PhysRect::new(200, 100, 400, 300));
        assert_eq!(
            m.view_data(0).hole,
            m.view_data(0).hover,
            "同一块几何，两格读数必须同数"
        );
    }

    /// §5.3.2 第四步：单击（按住而什么都没拖）选中**整窗**。
    #[test]
    fn a_click_selects_the_whole_window() {
        let mut m = hovering();
        assert!(m.hover_window(0, 30.0, 30.0));
        m.press(0, 30.0, 30.0);
        m.release();
        assert_eq!(m.hole, PhysRect::new(20, 20, 200, 150), "选区就是那一扇窗的边界");
        assert!(m.has_selection());
        // 标注层的 clip 跟着选区走是 [`Self::set_hole`] 一位作者的规矩，这里只确认走到了它。
        assert_eq!(m.hole, PhysRect::new(20, 20, 200, 150));
    }

    /// 单击选中之后**仍然能像今天一样调整**：八枚把手的命中走的是同一把 `hit_slop` 尺子，
    /// 而选好的那一块矩形就是它们的 `start`。这一条是任务里「用户随后仍可调整选区」那一问。
    #[test]
    fn the_selected_window_can_still_be_adjusted() {
        let mut m = hovering();
        m.press(0, 30.0, 30.0);
        m.release();
        let hole = m.hole;
        assert_eq!(hole, PhysRect::new(20, 20, 200, 150));
        // 右下角那枚把手。这一扇槽位是 100% 的单屏（[`hovering`]），所以窗内 DIP 与桌面
        // 物理同数，按住点就是 `hole` 右下角往里一格 —— 仍在 `hit_slop` 那 5 px 之内。
        let grip = m.press(0, f64::from(hole.right() - 1), f64::from(hole.bottom() - 1));
        assert_eq!(grip, Some(Handle::Se), "整窗选完之后角点把手还在原处");
        m.drag(0, 400.0, 400.0, false);
        m.release();
        assert_eq!(m.hole, PhysRect::new(20, 20, 380, 380), "拖角点把选区改大了");
        assert_ne!(m.hole, hole, "而且没被整窗选中顶回去");
    }

    /// 反例（§5.3.1 那条腿没被顶掉）：按住并拖动**画矩形**，松手后留下的必须是拖出来的
    /// 那一块，而不是光标底下那扇窗。这一条不成立的话，整个 §5.3.1 就没了。
    #[test]
    fn dragging_still_draws_a_rectangle_and_does_not_snap() {
        let mut m = hovering();
        m.press(0, 30.0, 30.0);
        m.drag(0, 130.0, 130.0, false);
        m.release();
        assert_eq!(m.hole, PhysRect::new(30, 30, 100, 100));
        assert_ne!(m.hole, PhysRect::new(20, 20, 200, 150), "没有偷偷换成整扇窗");
    }

    /// 边界那一臂（这一轮真踩到过的形状）：**纯水平**拖一条 0 高的带子，松手时按
    /// [`PhysRect::is_empty`] 是「空选区」，拿它当「什么都没拖」的判据就会把这一趟读成
    /// 单击、跳出一整扇窗。判据逐格比那一张 0×0@anchor，所以任何一点移动都算拖过。
    #[test]
    fn a_flat_horizontal_drag_is_not_read_as_a_click() {
        let mut m = hovering();
        m.press(0, 30.0, 30.0);
        m.drag(0, 130.0, 30.0, false);
        assert!(m.hole.is_empty(), "夹具：这一趟拖出来的确实是 0 高的一条");
        m.release();
        assert_eq!(m.hole, PhysRect::new(30, 30, 100, 0));
        assert_ne!(m.hole, PhysRect::new(20, 20, 200, 150));
    }

    /// 按在既有选区的把手上**不动**、松手：那是「抓住边角但没改」，不是「挑了一扇新窗」，
    /// 绝不能把用户调过的选区顶回成一整扇窗。
    #[test]
    fn holding_a_handle_is_never_a_window_pick() {
        let mut m = hovering();
        m.set_hole(PhysRect::new(300, 300, 100, 100));
        // 右下角把手上按住，原地松手。
        m.press(0, 399.0, 399.0);
        m.release();
        assert_eq!(m.hole, PhysRect::new(300, 300, 100, 100), "选区原样留着");
    }

    /// §5.3.2 第三步的「亮 / 不亮」闸门：按过一次就不再报高亮，Esc 第一档清掉选区之后
    /// 又回到「还在挑」。这一格判的是为什么不能用 `selected` 当闸门（遮罩是带着默认选区
    /// 开门的）—— 换成 `selected`，第一枚断言就会永远成立在「不亮」上，整路静默失效。
    #[test]
    fn the_highlight_stops_once_the_user_starts_choosing_and_returns_on_escape() {
        let mut m = hovering();
        m.hole = PhysRect::new(0, 0, 500, 400);
        assert!(m.hover_window(0, 30.0, 30.0));
        assert!(m.hover_bounds(0).is_some(), "没按之前是亮的");
        m.press(0, 30.0, 30.0);
        assert!(m.hover_taken, "按下就是「开始要一块了」");
        assert_eq!(m.hover_bounds(0), None, "按过之后不再报高亮");
        // 读数本身留着：`release` 整窗选中要读的正是这一格。
        assert_eq!(m.hover_win.map(|(h, _)| h), Some(7));
        assert!(m.escape(), "夹具：这一格有选区可清");
        assert!(!m.hover_taken, "Esc 第一档回到「还在挑」");
        assert!(m.hover_window(0, 30.0, 30.0));
        assert!(m.hover_bounds(0).is_some());
    }

    /// `[capture] snap_to_window` 是**真开关**（任务第 ④ 项里 §5.3.2 的那一枚键）。
    /// 关掉之后单击就是单击：选区停在 0×0，整窗那一路不参与，而高亮读数仍然算得出来
    /// （关掉的是「自动吸到窗」，不是「窗检测本身」）。
    #[test]
    fn snap_to_window_off_leaves_a_click_a_click() {
        let mut m = hovering();
        m.capture.snap_to_window = false;
        m.press(0, 30.0, 30.0);
        m.release();
        assert!(m.hole.is_empty(), "键关着 ⇒ 单击不整窗选中");
        assert_eq!(m.hover_win.map(|(h, _)| h), Some(7), "判据仍在算，只是不自动落");
        // 而**点名**要这一扇窗的那条路还能走（`select_hovered_window` 是那一臂的作者）。
        assert!(m.select_hovered_window());
        assert_eq!(m.hole, PhysRect::new(20, 20, 200, 150));
    }

    /// 一扇被拖出屏幕的窗：交出去的是**在屏幕上那一块**（相交、裁剪，不是滑动），因为
    /// 多出去的那一圈底下是别的窗，截进去就是把别人的内容混进这张图（§5.2.5 / §8.5 同族）。
    #[test]
    fn a_window_running_off_the_desktop_selects_only_the_part_on_screen() {
        let mut m = hovering();
        m.hover_win = Some((11, PhysRect::new(900, 700, 400, 300)));
        assert!(m.select_hovered_window());
        assert_eq!(m.hole, PhysRect::new(900, 700, 100, 100));
        // 整扇离开桌面的窗 ⇒ 不动选区，也不谎报成功。
        m.set_hole(PhysRect::new(50, 50, 10, 10));
        m.hover_win = Some((12, PhysRect::new(5_000, 5_000, 40, 40)));
        assert!(!m.select_hovered_window());
        assert_eq!(m.hole, PhysRect::new(50, 50, 10, 10));
    }

    /// 候选表是空的（枚举失败 [`crate::capture::window_candidates`] 就交空表）⇒ 这一路整个
    /// 安静下来，而**自由选区那条腿照常能走**。§5.3.2 是一条可选的快捷路径，不是前提。
    #[test]
    fn no_candidates_degrades_to_a_plain_free_selection() {
        let desk = PhysRect::new(0, 0, 1_000, 800);
        let mut m = MaskState {
            slots: vec![slot(0, desk, 1.0)],
            hole: PhysRect::default(),
            virtual_bounds: desk,
            ..Default::default()
        };
        m.windows.clear();
        assert!(!m.hover_window(0, 30.0, 30.0));
        m.press(0, 30.0, 30.0);
        m.drag(0, 130.0, 130.0, false);
        m.release();
        assert_eq!(m.hole, PhysRect::new(30, 30, 100, 100), "没有候选也要拖得动矩形");
        assert!(!m.select_hovered_window(), "没命中任何窗就不谎报选中");
    }

    /// `open` / `close` 之间不串：上一场按过一次的闸门与那一张候选表都不能跟着下一场活。
    ///
    /// 这一句**没有**单测，而且是明知故欠：[`MaskState::open`] 要冻结桌面、[`MaskState::close`]
    /// 要动 Qt 的纹理仓库并回写设置档，两趟都不是一枚 `#[test]` 能开的（§9.1 第十条的 headless
    /// 半边，与本文件 [`Self::reset_ratio`]、[`Self::seed_landed_from`] 头上记的是同一条理由）。
    /// 两行赋值就写在它们各自的函数里，读得到、跑不到 —— 判它得靠 `--mask` 那一扇门，而那一扇
    /// 门现在还不问这一格。写在这一行而不是留在这个注释外面，是为了让下一位知道这一路欠第几步。
    #[test]
    fn hover_taken_starts_down_and_only_a_press_points_it_up() {
        // `open` 里那一格归零 ⇒ 一场新遮罩的第一次 hover 就该亮。这一枚是那一行的可测替身。
        let mut m = hovering();
        assert!(!m.hover_taken, "出厂与 `open` 都是「还在挑」");
        assert!(m.hover_window(0, 30.0, 30.0));
        m.press(0, 30.0, 30.0);
        assert!(m.hover_taken);
        // 而 `hover_win` 这一格**留着**：不留着，[`Self::release`] 整窗选中就没得读。
        assert_eq!(m.hover_win.map(|(h, _)| h), Some(7));
    }

    /// 一组进场事实：主屏 640×480 上「自由区域」是中间那半块（[`MaskState::default_hole`] 自己
    /// 算的，本函数只是把它的读数抄下来），整桌面是**两块**屏的并集，所以 `desktop` 与 `free`
    /// 天生不同一块 —— 这正是 `fullscreen` 那一臂要被判的地方。
    fn opening_facts() -> OpeningFacts {
        let snap = snapshot(vec![
            monitor("m0", 0, 0, 640, 480, 1.0, true),
            monitor("m1", 640, 0, 640, 480, 1.0, false),
        ]);
        let desktop = snap.virtual_bounds;
        OpeningFacts {
            free: MaskState::default_hole(&snap),
            desktop,
            window: Some(PhysRect::new(12, 34, 200, 100)),
            last: Some(PhysRect::new(300, 200, 120, 90)),
        }
    }

    /// `[capture] default_mode` 是这轮要判的那枚键，而它的读者只有一处
    /// （[`MaskState::opening_hole`]）。判法就是把**同一组事实**递给两拨法，问两次答案：
    /// 拨一下读不回两样，这一枚键就还停在「档里没人读」。
    ///
    /// 四格各有对面，而**出厂那一臂（`region`）与本轮之前字节相同**：`free` 就是
    /// [`MaskState::default_hole`] 的那一块、也不开口，所以第一次有人读这枚键的那一刻，默认
    /// 路径上没有任何一格变了行为 —— 这是「新读者不许顺手改旧答案」的唯一证人。
    #[test]
    fn the_default_mode_key_decides_where_the_mask_enters() {
        use falcon_core::config::CaptureMode as M;
        let facts = opening_facts();
        let region = MaskState::opening_hole(M::Region, &facts);
        let full = MaskState::opening_hole(M::Fullscreen, &facts);
        // ① 两拨法 ⇒ 两块选区。这一格就是「键被读了」本身。
        assert_eq!(region.hole, facts.free, "出厂那一臂必须是今天那块默认选区");
        assert_eq!(full.hole, facts.desktop, "全屏那一臂要的是整块桌面");
        assert_ne!(region.hole, full.hole, "拨了 `default_mode` 而选区没动 = 这枚键没人读");
        assert!(
            region.said.is_none() && full.said.is_none(),
            "照办的两臂不该多说一句：{:?} / {:?}",
            region.said,
            full.said
        );
        // ② `window` / `repeat`：问得出就照办，问不出就退回自由区域**并说出来**。同一枚模式、
        // 两样事实 ⇒ 两样答案，这是这两臂各自的可证形状。
        let win_yes = MaskState::opening_hole(M::Window, &facts);
        let mut no_window = opening_facts();
        no_window.window = None;
        let win_no = MaskState::opening_hole(M::Window, &no_window);
        assert_eq!(win_yes.hole, facts.window.unwrap(), "光标下那扇窗就是进场选区");
        assert!(win_yes.said.is_none());
        assert_eq!(win_no.hole, facts.free, "问不出窗也要进得了场（§5.3.1 那条腿不能断）");
        let said_win = win_no.said.expect("问不出窗必须开口");
        assert!(
            said_win.contains("[capture] default_mode") && said_win.contains("window"),
            "那句退路话没点名这枚键：{said_win:?}"
        );
        assert!(!said_win.contains("做不到"), "「这一刻问不出」不等于「这枚键做不到」（#453）");

        let rep_yes = MaskState::opening_hole(M::Repeat, &facts);
        let mut no_last = opening_facts();
        no_last.last = None;
        let rep_no = MaskState::opening_hole(M::Repeat, &no_last);
        assert_eq!(rep_yes.hole, facts.last.unwrap());
        let said_rep = rep_no.said.expect("没有上一次也要开口");
        assert!(said_rep.contains("[capture] default_mode") && said_rep.contains("repeat"));
        assert_ne!(said_rep, said_win, "两臂问不出的是两件事，话不能是同一句（#453）");

        // ③ `element`：这一版产品**做不到**（UIA 那一只手在本仓库只会交空表），所以这一臂的
        // 答案不是"另一种选区"而是「照自由区域进场 + 把那句话说出口」（#433 不许静默）。
        let elem = MaskState::opening_hole(M::Element, &facts);
        assert_eq!(elem.hole, facts.free, "做不到那一臂按自由区域进场");
        let said_elem = elem.said.expect("做不到也要开口，不许静默");
        assert!(
            said_elem.contains("[capture] default_mode")
                && said_elem.contains("element")
                && said_elem.contains("做不到"),
            "做不到那一臂的话要点名键、要点名哪一臂：{said_elem:?}"
        );
        assert_ne!(said_elem, said_win, "「这枚键做不到」与「这一刻问不出」是两句话（#453）");
    }

    /// `[capture] freeze_frame` 分岔的**唯一读者**是 [`MaskState::freeze_notice`]，而它交回的
    /// 是「这一枚键这一版产品做不到」那句话本身 —— 见它头顶那段：本产品没有让遮罩背景随桌面
    /// 刷新的通路，`capture` 那一模块的头一条规矩就是屏幕在任何窗存在之前只读一次。
    ///
    /// 两拨法交回两样答案：开着 ⇒ `None`（默认那一侧屏幕上不该多出一行），关着 ⇒ 那句话，且
    /// 句里点名这枚键、点名「做不到」、也点名画面仍然是冻的（用户有权知道自己拨的东西没生效）。
    /// 它与 `default_mode = element` 那句也不是一句：一枚键做不到与另一枚键做不到，各有各的
    /// 来处（#453）。
    #[test]
    fn the_freeze_frame_key_says_what_it_cannot_do() {
        assert!(
            MaskState::freeze_notice(true).is_none(),
            "开着（出厂默认）那一臂不许在屏幕上多留一行与用户决定无关的话"
        );
        let off = MaskState::freeze_notice(false).expect("关着那一臂必须开口");
        assert!(
            off.contains("[capture] freeze_frame") && off.contains("做不到"),
            "那句话没点名这枚键：{off:?}"
        );
        assert!(off.contains("实时取景"), "得说的是**为什么**做不到：{off:?}");
        assert_ne!(
            off,
            MaskState::opening_hole(
                falcon_core::config::CaptureMode::Element,
                &opening_facts()
            )
            .said
            .expect("element 那臂也该开口")
            .as_str(),
            "两枚不同的键做不到是两件不同的事，话不能共用一句（#453）"
        );
    }

    /// `[advanced] privacy_no_temp_files` 分岔的**唯一读者**是 [`MaskState::sweep_stale_temp`]。
    /// 判法与本文件其他几枚开关同一形状：同一枚目录、同一份 `keep`、两拨法，然后**看盘**。
    ///
    /// 开着 ⇒ 除 `keep` 那枚之外的一张被收回、`keep` 那张原地不动（"还欠着的不许动"），并且
    /// 说出一句点名这枚键的话；关着 ⇒ 一枚也不少删、一句也不说（不清理不是失败，是用户许过的
    /// 状态）。目录根本不存在 ⇒ 第三臂：不开口 —— 那枚目录是产品自己造的，还没人发送过东西时
    /// 它本就该不在，此时每一场遮罩抱怨一次「打不开」是假话而不是证据。
    #[test]
    fn the_no_temp_files_key_sweeps_our_own_tagged_dir() {
        let dir = sandbox("privacy-sweep");
        let owed = dir.join("quick-001.png");
        let stale = dir.join("quick-002.png");
        std::fs::write(&owed, b"still owed").expect("write owed");
        std::fs::write(&stale, b"last flow's handover").expect("write stale");

        let off = MaskState::sweep_stale_temp(&dir, &[owed.clone()], false);
        assert_eq!(off.deleted, 0, "键关着时一枚也不该删");
        assert!(off.said.is_empty(), "关着不是失败，没什么可说：{:?}", off.said);
        assert!(stale.exists(), "关着那一臂把盘留给用户：{stale:?} 没了");

        let on = MaskState::sweep_stale_temp(&dir, &[owed.clone()], true);
        assert_eq!(on.deleted, 1, "开着那一臂收回的正是还欠着之外的那一张");
        assert!(!stale.exists(), "计数器说删掉了一张，盘上就得没有那张");
        assert!(owed.exists(), "本程序还欠着的那一张不许动（§5.5.10 的派发证据）");
        assert!(
            on.said.iter().any(|s| s.contains("[advanced] privacy_no_temp_files")),
            "收回这件事要点名是哪枚键促成的：{:?}",
            on.said
        );
        assert_ne!(
            (off.deleted, off.said.len()),
            (on.deleted, on.said.len()),
            "两拨法交回了同一个答案 = 这枚键没人读"
        );

        // 第三臂：目录不在。既没活可干，也就没话可说。
        let missing = dir.join("no-such-dir");
        let none = MaskState::sweep_stale_temp(&missing, &[], true);
        assert_eq!(none.deleted, 0);
        assert!(
            none.said.is_empty(),
            "「还没有过临时文件」不该被说成一次失败：{:?}",
            none.said
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn f2_at_the_mask_reopens_the_field_on_the_object_and_divides_its_own_font() {
        use falcon_core::annotation::model::Kind;

        let (slots, desk) = one_screen();
        let mut m = selectable(slots, PhysRect::default(), desk);
        m.set_hole(PhysRect::new(200, 100, 800, 600));

        // 拒门先走一遍：此刻串里没有东西，而按下去没反应的那四种可能里，屏幕上要有话
        // （#433）。页上那一格亮着的还是 `text_editing = false`。
        assert!(!m.edit_text(), "什么都没选，这一枚键该拒");
        assert!(m.message.contains("F2"), "拒门没有话要说：{}", m.message);
        assert!(!m.view_data(0).typing_live, "拒门把框亮给了用户");

        m.ink.select_tool(annotate::code_of(Some(Kind::Text)));
        // DIP (150,70) 是物理 (300,140)：这一枚对象的角。
        m.press(0, 150.0, 70.0);
        m.release();
        m.ink.type_text("hi");
        assert!(m.ink.finish_typing(), "文本的回车没交出去");
        assert_eq!(m.ink.selected_count(), 1, "提交的那一枚不是当前对象");

        // 手里还是笔，那一枚键不开第二次改字 —— 笔底下点哪都先落新的一枚。
        assert!(!m.edit_text(), "一支笔开不了改字的框");

        // 拿起矩形是把选中交还画笔的那一支手（§5.7.16 步 4 的出路），此刻串里空的，
        // 所以这一格字号只动画笔 —— 下面要的就是「笔与对象不是同一个数」那一分岔。
        m.ink.select_tool(annotate::code_of(Some(Kind::Rect)));
        assert_eq!(m.ink.selected_count(), 0);
        m.ink.set_font_size(36);

        // 放下笔、点中那一枚对象，再按这一枚键。递进 [`crate::annotate::Layer::press`] 的
        // 是**桌面**坐标，与 [`MaskState::press`] 自己那条腿同一个口径。
        m.ink.select_tool(annotate::code_of(None));
        assert!(m.ink.press(PhysPoint::new(304, 144)));
        assert_eq!(m.ink.selected_count(), 1, "单击落在那几个像素上没选中它");
        assert!(m.edit_text(), "选中的文字标注上，F2 没开门");

        let v = m.view_data(0);
        assert!(v.typing_live, "页上那一格没亮");
        assert_eq!((v.typing.x, v.typing.y), (150, 70), "框没开在对象自己的角上");
        // 字段除的是**对象**那一格字号（18 → 9 DIP），而工具栏亮着的是画笔的 36（18 DIP）：
        // 两位作者各报各的，#281。
        assert_eq!(m.ink.font_px(), 36, "画笔那一格被对象带了走");
        assert_eq!(v.font_dip, 9, "字段没有按对象自己的字号除：{}", v.font_dip);

        // 收尾走的是同一只梯子（页上那一枚 `finishInk`），而改字没有变成第二枚对象。
        m.ink.type_text("hi there");
        assert!(m.ink.finish_typing());
        assert!(!m.view_data(0).typing_live, "提交之后字段还亮着");
        assert_eq!(m.ink.objects(), 1, "改字又落了一枚新对象");
        assert!(m.ink.undo_step(), "这一次改字退不回来");
        assert_eq!(m.ink.objects(), 1, "撤销把那一枚对象一起退掉了");
    }

    /// 层画布上这一点的 alpha。这一场的画布铺在桌面 (0,0)，所以**画布局部就是桌面**，下面
    /// 那些坐标不用换算。遮罩这一层的手势证人过去只数计数（`objects()`、`selected_count()`），
    /// 而「指针在 DIP 上走了 10」与「对象在桌面上走了 10」在两枚计数上是同一个数 —— 这一枚
    /// 是本文件第一次真去数字节。
    fn ink_alpha(l: &annotate::Layer, x: i32, y: i32) -> u8 {
        let f = l.layer_of("m0").expect("这一场有画布");
        let i = ((y as u64 * f.width as u64 + x as u64) * 4) as usize;
        f.pixels[i + 3]
    }

    /// §5.7.18 步 2「按住 Ctrl 拖动」在遮罩这一层的两条腿。层里那五枚证人量的是一串**桌面**
    /// 坐标上的手势，而生产路径上的那一只手是页在**自己这一窗的 DIP** 里按下去的：
    /// [`MaskState::press`] 先乘回桌面才递给 [`annotate::Layer::press`]，而 [`MaskState::drag`]
    /// 与 [`MaskState::release`] 都先问 [`annotate::Layer::dragging`] 才转交。这一枚量的就是
    /// 那一乘，加上"这一趟按住遮罩自己不接手"（`grab` 留空、选区一动不动）。
    #[test]
    fn a_ctrl_drag_at_the_mask_lands_a_copy_twice_as_far_as_the_pointer_walked() {
        use falcon_core::annotation::model::Kind;

        // 200% 的一张窗（这台机器此刻的数）：DIP (140,70) 就是桌面 (280,140)。画布只能由
        // [`annotate::Layer::begin`] 造出来（本文件 `savable` 头上那一句），所以这一场自己搭
        // 一份冻结帧 —— 那张帧是 `Frame::new`，全零，于是画布上任何一点非零的 alpha 都是层
        // 自己描的，不是背景。
        let bounds = PhysRect::new(0, 0, 800, 600);
        let snap = snapshot(vec![monitor("m0", 0, 0, 800, 600, 2.0, true)]);
        let mut m = selectable(vec![slot(0, bounds, 2.0)], PhysRect::default(), bounds);
        m.ink.begin(&snap, &m.slots);
        m.set_hole(PhysRect::new(200, 100, 400, 300));

        // 第一枚对象走的也是页真正会走的那三件套，而不是层的桌面 API：DIP 起笔 (120,70) =
        // 桌面 (240,140)，拖到 (180,120) = 桌面 (360,240)。
        m.ink.select_tool(annotate::code_of(Some(Kind::Rect)));
        m.press(0, 120.0, 70.0);
        m.drag(0, 180.0, 120.0, false);
        m.release();
        assert_eq!(m.ink.objects(), 1, "矩形没在遮罩上落下来");
        m.ink.select_tool(annotate::code_of(None));

        // 那一枚修饰键写在按下之前（页上就是这个次序），按的那一点落在**画出来的那一条边**上：
        // 没灌满的矩形中间那片不是它的目标，而 (280,140) 离那八枚把手都远超 GRIP_SLOP 那一圈。
        m.ink.hold_clone(true);
        assert!(
            m.press(0, 140.0, 70.0).is_none(),
            "这一枚按下被遮罩自己接走了，层没握住"
        );
        assert_eq!(m.ink.objects(), 2, "副本没在按下这一刻摆进文档");
        assert_eq!(m.ink.selected_count(), 1, "这一串说的该是原物，不是副本");
        assert!(m.ink.dragging(), "层没握住这一趟，下面两枚事件谁也到不了它");
        assert_eq!(m.hole, PhysRect::new(200, 100, 400, 300), "选区被这趟拖拽改写了");

        // 指针在 DIP 上走了 (10,10)，乘回桌面是 (20,20)，于是副本的左上角在 (260,160)。
        // (250,150) 那一格是"DIP 被当成桌面"的那一副脸：它离两枚框的每一条描边都还有十枚像素。
        m.drag(0, 150.0, 80.0, false);
        assert_ne!(
            ink_alpha(&m.ink, 260, 160),
            0,
            "副本的预览没描在桌面走过 (20,20) 的那一格"
        );
        assert_eq!(
            ink_alpha(&m.ink, 250, 150),
            0,
            "指针在 DIP 上走的 10 被当成桌面的 10 用了"
        );
        assert_ne!(ink_alpha(&m.ink, 280, 140), 0, "原物被这一趟拖拽搬走了");

        m.release();
        assert_eq!(m.ink.objects(), 2, "松手没有把副本交出去");
        assert_ne!(
            ink_alpha(&m.ink, 260, 160),
            0,
            "预览被抹了而副本没落下"
        );
        assert!(m.ink.undo_step(), "一趟按住 Ctrl 的拖该是一条撤销，不是两条");
        assert_eq!(m.ink.objects(), 1);
        assert_eq!(ink_alpha(&m.ink, 260, 160), 0, "撤销之后画布上还留着副本那一笔");
        assert_ne!(ink_alpha(&m.ink, 280, 140), 0, "撤销把原物一起退掉了");
    }
}
