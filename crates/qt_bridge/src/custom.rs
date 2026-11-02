//! §5.3.5 的那一扇「自定义截图」对话框（PRD 第 294–309 行）。
//!
//! **这一档存在的原因**：PRD 第 298 行写「用户打开『自定义截图』对话框」、第 299 行「输入 X、Y
//! 坐标」、第 300 行「输入宽度和高度」、第 301 行「可选输入延时时间」、第 302 行「点击开始截图」、
//! 第 303 行「系统按输入参数创建选区」。P166 之前这一节三条校验规则里只有前两行有落地之处 —— 命令行
//! 的 `--region x,y,w,h`（它要的正是「按填好的坐标与尺寸截一块区域，不经过遮罩」，
//! `falcon_core::config::ACTION_TEXTS` 里 `capture_custom` 那句说明就是这么写的）。对话框本身没有：
//! `hotkey.rs` 头顶那封信明写着 "the coordinates dialog it names (§5.3.5) does not exist yet"，
//! `Work::of("capture_custom")` 因此一直是 `None`，设置页与托盘把它报成「未接线」。这一档把那句话
//! 变成历史：五格填字的是页上，判形状与抓图的是下面这一只手，而第 309 行「应支持键盘方向键微调数值」
//! 的那一枚算术在 [`nudge`] —— 页上那两枚键只调用它、不自己算。
//!
//! **它不重新发明任何一只手**：
//! * 「宽度和高度必须大于 0」（第 307 行）那句话的作者仍是 [`crate::capture::size_guard`]，对话框
//!   只把**自己的名字**递进去当 `what` —— 页上那句拒绝与终端那句拒绝是同一句话，不是它的译文（#281）；
//! * 「坐标超出屏幕时应提示或自动修正」（第 308 行）仍是 [`crate::capture::region_frame`] 那一只手：
//!   裁是 core 裁的（`Captured::clipped`），说是 `clip_note` 说的，页上画的是 [`crate::capture::pin_rect`]
//!   交回的那一行原文；
//! * 「按输入参数创建选区」（第 303 行）交回的是一张**真的贴图**，与命令行同一只 `pin_rect`，那句话的
//!   作者是 [`crate::capture::pinned_line`]；
//! * 「延时时间」（第 301 行）用的是 [`crate::delay`] 里那同一副倒计时算术（`Countdown` 的
//!   `held/planned`、同枚 `STEP_MS`），只有**实例**另起一场：全局那枚槽数完开的是遮罩，这一场数完截
//!   的是填好的那一块。界也读的是同一枚常数 [`falcon_core::config::DELAY_SECONDS_MIN`] /
//!   [`falcon_core::config::DELAY_SECONDS_MAX`]，所以「页上拒的那个数」与「档里钳的那个数」不可能不同。
//!
//! **它不写用户的档**（§9.1 第十五条）：五格里没有任何一枚是持久键，按下「开始截图」只冻一次画面、
//! 只添一枚贴图。`[capture] delay_seconds` 在这里是被**读出来当起点**（P164 给单位那一格立的形状：
//! 开在一枚持久键上而不是每场归零）；要改它得走 §5.20 那一页。

use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use falcon_core::config::{DELAY_SECONDS_MAX, DELAY_SECONDS_MIN};
use falcon_core::geometry::PhysRect;

use crate::capture;
use crate::delay::{self, Countdown};
use crate::state::Check;

/// 五格的名字，次序就是 PRD 第 299 / 300 / 301 行说话的样子：X、Y 先，宽高次之，延时最后。
///
/// 名单住在这一档而不是长在 `.qml` 里，与 `SettingsWindow.qml` 头顶那封信同一个理由：页上抄一份
/// 名单，改一处就会留下「框上写着宽、递进 Rust 的是高」那种坏，而它只能靠**数**出来 —— 门里
/// `custom window` 那一格判的正是「页上画的格数与本名单一样长」。
pub const CELLS: [&str; 5] = ["X", "Y", "宽度", "高度", "延时"];

/// 每一格旁边那句话，与 [`CELLS`] 同序同长。它是**提示**不是判据：判据的措辞只有三处作者
/// （[`int_note`] / [`delay_note`] / [`crate::capture::size_guard`]），这一处只说「这一格填什么」。
const HINTS: [&str; CELLS.len()] = [
    "桌面 X（物理像素，可以为负）",
    "桌面 Y（物理像素，可以为负）",
    "宽度（物理像素，要大于 0）",
    "高度（物理像素，要大于 0）",
    "秒；留空或 0 就是不延时",
];

/// 那一枚钮上写的两个字（PRD 第 302 行「点击开始截图」）。页上不许多写一份：与 `guide::ACK_WORD`
/// 同一枚道理 —— 句子里说的字与钮上写的字分家，只在屏幕上露出来。
pub const START_WORD: &str = "开始截图";

/// 窗出生之后到它开始跑腿之间那一枚等待 —— 它是计划那十五段里的第 0 段，页上
/// `gaugeLeg { interval: Number(win.plan[0]) }` 读的就是这一枚。与 `settings_view::LEG_MS` 同族而
/// **不共用**：那一枚按「写一枚档里的值、再问一次页」排，所以它是**腿与腿之间**的间隔；这一档的六条腿
/// 在一次 `runGauge()` 里连着跑（填五格、按一次钮、数一枚延时都是同步的桥调用），所以这一枚数是
/// 「等窗把自己画完」而不是「等下一腿」。**这一处措辞先前写错了**：它被写成「每一枚腿之间隔多久」，
/// 而 A/B/C 三代的戳显示真正长的那一段是腿**内部**的抓图（`offscreen @1247 → clip @4754`），不是腿之间。
pub const LEG_MS: u64 = 300;

/// 延时那一腿打的就是这一枚秒数。1 是能让一条腿在闹钟内跑完的最短**非零**延时：0 秒走的是立刻那一支，
/// 上面几条腿已经在判它了。计划里这一段与 `measure` 里那句「计划该等多久」取的是同一枚常数。
const DELAY_LEG_SECONDS: i32 = 1;

/// 这一档的闹钟地板。它**不是**照着「腿等待 + 延时腿」算出来的，而是照着三代实测的戳钉的：
///   `p166_window_A.txt` 闹钟 5,200 ⇒ `tick @684 → offscreen @1247 → clip @4754 → delay-armed @4756`
///   `p166_window_B.txt` 闹钟 9,000 ⇒ `tick @1022 → offscreen @2461 → delay-armed @9283`
///   `p166_windowonly_C.txt` 闹钟 20,000 ⇒ `delay-fired|1025|1000|贴图 1 已钉 @6120`，八格全绿
///
/// 三行读下来，⑦ 那一格在 A 与 B 里红的是**这一档的闹钟**，不是页上那枚钟：延时腿必须排在 ⑤ 之后
/// （⑤ 那格的判据是「贴图因此多一枚」，读的是它自己那一行那一刻的旁证，延时腿若先钉一枚就把那一句
/// 的分母变成 +2），而 ⑤ 自己去抓一整块桌面（99999 被裁成 2972×1820）再钉一枚真贴图 —— 同一份二进制，
/// 这一腿在 A 里花 3.5 秒、在 B 里花到 6.8 秒，B 里连「装填」那一行都落在 9,000 ms 闹钟**之后**才递回来。
/// C 把它跑完了，用的仍是那两个没有放宽的判据（`held >= planned` 与 `planned == 延时秒数 × STEP_MS`），
/// 递回的是实测 1,025 ms 对计划 1,000 ms。
///
/// 地板因此钉在最坏一次实测之上而不是之中：9,283 ms（B 代装填完）+ 1,000 ms（数满）+ 抓图与递行的余量。
/// 门里真跑的那一枚数（`p166_door.cmd` 的 `MS`）比地板更大，地板只在有人给了一枚太短的闹钟时把它抬起来
/// —— 它不会把一次已经够长的等待改短，所以这一处改动的对面只有「红在闹钟上」那一种坏。
pub const MIN_WINDOW_MS: i32 = 12000;

/// 一扇对话框此刻的样子。它是进程级的一格 —— 与 `MaskState`、`settings_view::WINDOW` 同一族：
/// 「有没有一扇窗、窗里填的是什么」只有**一处**能答，两个实例各填一份就会有一份是过期的。
#[derive(Debug, Default)]
struct Dialog {
    /// 五格的**现文本**。空串是「还没填」，不是 0 —— 把没填读成 0 会让一格空白变成一次合法的
    /// `x=0` 请求，而第 299 行要的是「输入」，不是「替用户输入」。
    text: [String; CELLS.len()],
    /// 页上那一句结果 / 拒绝。最近一次按下说的原话，只有这一位作者写过它。
    note: String,
    /// 对话框自己那一场倒计时。`None` = 此刻没在数。它与 `delay::RUNNING` 那枚全局槽不是同一场。
    countdown: Option<Countdown>,
    /// 倒计时数完要截的那一块。与 `countdown` 同生同死：挂上必记一块，数完必清两者。
    pending: Option<PhysRect>,
    /// 按过几次「开始截图」、其中几次被形状判据拒掉。门读的是这两枚数，不读「页上有没有红字」。
    starts: u64,
    refused: u64,
    /// 数满并真的抓成几场。
    fired: u64,
    /// 这一趟窗被要开几回（真人每点一次托盘里那一项加一回，门每开一扇加一回）。
    opened: u64,

    // ── 门那一侧（`--custom-window`）
    armed: bool,
    reason: String,
    plan: String,
    lines: Vec<String>,
    /// 与 `lines` 同序：每一行落在 `arm()` 之后第几毫秒。不进任何一格的判定，只为让「窗生得太晚」
    /// 与「Timer 没触发」两种坏在报告里长得不一样（`settings_view::seen` 立的形）。
    stamps: Vec<u64>,
    /// 与 `lines` 同序：每一行递回来**那一刻**的旁证（在册几枚贴图、冻过几次面、记忆里是哪一块）。
    /// 行说的话要与同一时刻的账对得上，而对账必须在它说话的那一刻 —— 下一枚行会把记忆改掉
    /// （§9.1：判据的分母钉在「被量的那一只手正在量的那一刻」）。
    mates: Vec<Mate>,

    // ── 下面三枚都是**门与页共用的读数**，不是第三种措辞。
    /// 最近一场数满时 `Tick::Go` 交回的那两枚毫秒（实测 | 计划）。页递回 `delay-fired` 那一行时
    /// 读的就是这里 —— 数满那一刻的两枚数若不留下来，问「等了多久」就只能重新拿表算，而重算的
    /// 那一刻已经不是数满的那一刻。
    last_wait: (u64, u64),
    /// `arm()` 那一刻的在册贴图数与冻面次数。⑤⑥⑦ 三格判的是「比开工时多一枚 / 多几次」，
    /// 而同一进程里前一趟留下的贴图不会自己消失，所以分母只能是开工时的读数。
    pins_at_arm: usize,
    freezes_at_arm: u64,
}

/// 一行递回来那一刻，Rust 这一侧同时读到的三样旁证。
#[derive(Clone, Debug, PartialEq)]
struct Mate {
    pins: usize,
    freezes: u64,
    remembered: Option<PhysRect>,
}

fn slot() -> &'static Mutex<Dialog> {
    static D: OnceLock<Mutex<Dialog>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(Dialog::default()))
}

fn with<R>(f: impl FnOnce(&mut Dialog) -> R) -> R {
    let mut guard = slot().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// `arm()` 的那一刻，给每一行盖戳用。
fn arm_at() -> &'static Instant {
    static A: OnceLock<Instant> = OnceLock::new();
    A.get_or_init(Instant::now)
}

/// 页上五格的名字，`|` 连接。
pub fn labels() -> String {
    CELLS.join("|")
}

/// 每一格旁边那句话，`|` 连接，与 [`labels`] **同序同长**。
pub fn hints() -> String {
    HINTS.join("|")
}

/// 这一扇窗的标题，也就是 PRD 第 298 行那五个字。它从那张唯一的名册 [`falcon_core::config`] 的
/// `ACTION_TEXTS` 读，与托盘里那一项、设置页里那一行**同一个名**：窗开着而托盘那一项写着别的字，
/// 是抬眼就看得出的坏，而它只能靠「只有一本名册」不出现。
pub fn title() -> String {
    crate::hotkey::name_of("capture_custom").to_string()
}

/// 页上那一句结果的原文。
pub fn note() -> String {
    with(|d| d.note.clone())
}

/// 第 `i` 格的现文本。越界交回空串而不是 panic：页上的下标来自 `model`，这里错了要说成「那一格没有
/// 内容」，不该把整个进程带走。
pub fn text(i: usize) -> String {
    with(|d| d.text.get(i).cloned().unwrap_or_default())
}

/// 还剩几枚数没数（`Session.custom_left` 绑的就是它，绑**值**不绑调用 —— `main.qml` 在 `warmRounds`
/// 那一段钉下的口径：一枚 invokable 的绑定只求值一次）。
pub fn left() -> u32 {
    with(|d| d.countdown.as_ref().map(Countdown::left).unwrap_or(0))
}

/// 每枚数之间隔多久，直接交给 `Timer.interval`。它是 [`crate::delay::STEP_MS`] 本身，不随这一场变，
/// 所以页上可以在出生时读一次。
pub fn step_ms() -> u64 {
    delay::STEP_MS
}

/// 「这一格还空着」那一句的作者。
fn empty_note(label: &str) -> String {
    format!("「{label}」这一格还空着：五格都填了才截得出区域（延时那一格填 0 就是不延时）")
}

/// 「这一格写的不是整数」那一句的作者。它点名**哪一格**与**页上此刻写的字**：一句「输入有误」在五格
/// 面前等于没说。
fn int_note(label: &str, raw: &str) -> String {
    format!("「{label}」这一格要的是整数，现在写的是「{raw}」")
}

/// 延时越界 / 不是整数那一句。界读的是档里那两枚常数，所以页上拒的数与 `write_back` 时钳的数是同一枚
/// 数 —— 写成两份字面量，改天一处改成 120 就留下「页上放过去、档里被悄悄改掉」那种坏。
fn delay_note(raw: &str) -> String {
    format!(
        "延时要的是 {} 到 {} 之间的整数秒，现在写的是「{raw}」（0 是不延时）",
        DELAY_SECONDS_MIN, DELAY_SECONDS_MAX
    )
}

/// 把此刻的五格读成「一块区域 + 几枚秒」。交回第一格该说的那句拒绝。
///
/// 次序是有意的：四格形状 → 宽高那一条规则 → 延时。宽高那一句为什么不在 [`set`] 里单格说？因为它说的
/// 是**一对**数（`w=` 与 `h=`），只有一格的数字时那句拒绝会指着半句话。
fn read(d: &Dialog) -> Result<(PhysRect, u32), String> {
    let mut n = [0i32; 4];
    for (i, cell) in n.iter_mut().enumerate() {
        let raw = d.text[i].trim();
        if raw.is_empty() {
            return Err(empty_note(CELLS[i]));
        }
        *cell = raw.parse::<i32>().map_err(|_| int_note(CELLS[i], raw))?;
    }
    capture::size_guard(&title(), n[2], n[3])?;
    let raw = d.text[4].trim();
    let seconds = if raw.is_empty() {
        0
    } else {
        let v = raw.parse::<i32>().map_err(|_| delay_note(raw))?;
        // 界外的那个数**拒**而不是钳。档那一头钳是有道理的：`write_back` 改的是 `[capture]
        // delay_seconds`，而写进去的必须是一枚档里合法的数；这一头改的是用户刚刚打进对话框的一个数，
        // 悄悄当成 300 去截说的是「你要的 301 秒我听成了 300 秒」，而第 301 行「可选输入延时时间」的
        // 对面是他填的那一枚。界还是那两枚常数，所以这里拒的数与档里钳的数同一枚（[`delay_note`] 说的
        // 也是它们）—— 页上不许有第二个界。
        if !(DELAY_SECONDS_MIN as i32..=DELAY_SECONDS_MAX as i32).contains(&v) {
            return Err(delay_note(raw));
        }
        v as u32
    };
    Ok((PhysRect::new(n[0], n[1], n[2] as u32, n[3] as u32), seconds))
}

/// 填一格。**只记账，不抓图**：页上每次 `onEditingFinished` 都叫这里，而一边打字一边冻画面不是谁要的
/// 行为。它顺手把这一格的毛病说出来，为的是「这一格待会儿会被拒」在打字那一刻就露出来。
pub fn set(i: usize, raw: &str) -> String {
    let label = CELLS.get(i).copied().unwrap_or("");
    let trimmed = raw.trim();
    with(|d| {
        if let Some(cell) = d.text.get_mut(i) {
            *cell = raw.to_string();
        }
        if trimmed.is_empty() {
            // 空格不是毛病，是「还没填」。清掉上一句，页上那格就此回到中性。
            d.note.clear();
            return String::new();
        }
        let why = if i < 4 {
            trimmed
                .parse::<i32>()
                .err()
                .map(|_| int_note(label, trimmed))
                .unwrap_or_default()
        } else {
            match trimmed.parse::<i32>() {
                Ok(v) if (DELAY_SECONDS_MIN as i32..=DELAY_SECONDS_MAX as i32).contains(&v) => None,
                _ => Some(delay_note(trimmed)),
            }
            .unwrap_or_default()
        };
        d.note.clone_from(&why);
        why
    })
}

/// 第 309 行「应支持键盘方向键微调数值」的那一枚**算术**：把第 `i` 格往上（`delta = 1`）或往下
/// （`delta = -1`）挪一枚。
///
/// 它住在这一档而不是长在 `.qml` 里，为的是让「挪一枚」这件事有单测、有门可问：页上那一行
/// `Keys.onUpPressed` 只做一次调用与一次回填，算术若留在页里，这一条规则在门与单测两头都是空的
/// （本仓没有输入注入器 —— 量具递不进真按键，但量具能叫同一只手，§10 第 150 项③那一条口径）。
///
/// 三格形状各是一个读数：空的一格读成 0 再挪（`+1` 交回 `"1"`，那是「从空白起步」而不是「替用户
/// 填一个起点」——起点是用户按下的那一枚键）；挪过的格仍走 [`set`]，所以「挪出来的数」与「打出来的
/// 数」在这一格里没有两种写法；而一格写着 `abc` 时**原样交回**：把坏格悄悄改成一枚数，等于替用户
/// 抹掉他还没改对的那一格。
pub fn nudge(i: usize, delta: i32) -> String {
    let raw = with(|d| d.text.get(i).cloned().unwrap_or_default());
    let trimmed = raw.trim();
    let moved = match trimmed.parse::<i32>() {
        Ok(v) => v.saturating_add(delta).to_string(),
        // 空的一格从 0 起步（`+1` 交回 `"1"`）：起点是用户按下的那一枚键，不是替用户填的数。
        Err(_) if trimmed.is_empty() => delta.to_string(),
        Err(_) => raw,
    };
    set(i, &moved);
    moved
}

/// 窗出生时该起手的五格。
///
/// 前四格**空着**：档里没有任何位置键（`[capture]` 那一段压根没有 x / y / 宽 / 高），把 0 或屏幕中点
/// 当起点都是替用户填表。第五格不同 —— `[capture] delay_seconds` 是一枚真的持久键（§5.2.4），这一扇窗
/// 正是它在 PRD 里点名的第二个读者，所以它开在档里那枚值上而不是每场归零。
pub fn seed() {
    with(|d| {
        d.opened += 1;
        d.countdown = None;
        d.pending = None;
        d.note.clear();
        for cell in d.text[..4].iter_mut() {
            cell.clear();
        }
        d.text[4] = crate::settings::capture_cfg().delay_seconds.to_string();
    });
}

/// 按下「开始截图」（第 302 行）。交回这一刻说给用户的那句话，并把它留在 [`note`] 里。
///
/// 两支：**延时为 0** 立刻抓；**延时 > 0** 挂上倒计时、把这块区域记成「数完要截的那一块」，交回的是
/// 「还剩几枚数」。第二支**不冻画面** —— 冻只在 [`tick`] 数满那一刻发生，这正是 PRD 第 420 行
/// （§5.2.4 第五步「倒计时结束后……按配置直接截取指定区域」）要的那一瞬。
pub fn start() -> String {
    let ready = with(|d| read(d));
    let (rect, seconds) = match ready {
        Ok(v) => v,
        Err(why) => {
            with(|d| {
                d.starts += 1;
                d.refused += 1;
                d.note.clone_from(&why);
            });
            return why;
        }
    };
    if seconds == 0 {
        with(|d| d.starts += 1);
        return fire(&rect);
    }
    let ticks = delay::countdown(seconds);
    let left = ticks.len() as u32;
    with(|d| {
        d.countdown = Some(Countdown::new(ticks, delay::STEP_MS, delay::clock_ms()));
        d.pending = Some(rect);
        d.starts += 1;
        d.note = format!("延时 {seconds} 秒开始截图：还剩 {left} 枚数");
    });
    note()
}

/// 页上那枚 Timer 每走一步叫一次。交回**这一刻**要说的一句话：还在数就什么也不说（页上那行倒计时
/// 自己跟着 [`left`] 走），数满就说抓出来的那一块。
///
/// 一场上限一句话：`Countdown` 数满即交场，槽在这一刻清空，之后 [`tick`] 读到的是空槽 —— 绝不会有
/// 第二次冻结，与 [`crate::delay::tick`] 为全局那枚槽立的同一规矩。
pub fn tick() -> String {
    // 把倒计时从槽里**取出来**再数：`Tick::Go` 意味着这一场已经结束，留着它下一次调用就会拿到第二个
    // `Go`（结构体那一侧答 `Spent`，但产品路径不该依赖有人记得它）。取出来也解开了借用 —— 数完要在
    // 同一把锁里清槽、记毫秒、拿走那块待截的区域。
    let gone: Option<(u64, u64, Option<PhysRect>)> = with(|d| match d.countdown.take() {
        None => None,
        Some(mut c) => match c.tick(delay::clock_ms()) {
            delay::Tick::Waiting { .. } => {
                d.countdown = Some(c);
                None
            }
            delay::Tick::Go {
                held_ms,
                planned_ms,
            } => {
                d.last_wait = (held_ms, planned_ms);
                Some((held_ms, planned_ms, d.pending.take()))
            }
            delay::Tick::Spent => None,
        },
    });
    match gone {
        Some((_, _, Some(rect))) => fire(&rect),
        // 数满了而手里没有区域 ⇒ 没有东西可截，也就没有话可说。上面那一段把两者同生同死钉死了，
        // 走到这里是这一档自己的坏，而它的表现是「一句也没说」而不是「说了一句假的成功」。
        _ => String::new(),
    }
}

/// 数满那一刻的两枚毫秒，`|` 连接，页递回 `delay-fired` 那一行时接在后面。
///
/// 它为什么不在页上算：`Countdown::held_ms` 归 [`tick`] 那一次调用所有，而 `Tick::Go` 已经把两枚数
/// 交回来了 —— 重新拿表算的那一刻已经不是数满的那一刻。
pub fn wait_line() -> String {
    let (held, planned) = with(|d| d.last_wait);
    format!("{held}|{planned}")
}

/// 真的抓一张：[`crate::capture::pin_rect`] —— 冻面、裁到桌面、添一枚贴图、把「裁没裁」写进那一行，
/// 全在那一只手里。这一档只负责把它的原话说给用户听，所以页上那句话里没有一处措辞是这里新写的。
fn fire(rect: &PhysRect) -> String {
    let said = match capture::pin_rect(rect) {
        Ok((id, line)) => capture::pinned_line(id, &line),
        Err(e) => e,
    };
    with(|d| {
        if said.starts_with("贴图 ") {
            d.fired += 1;
        }
        d.note.clone_from(&said);
    });
    said
}

/// 这一刻的三枚旁证。与 [`report`] 同一把锁里读，保证「行说话的那一刻」与「账上的那一刻」同一刻。
fn mate() -> Mate {
    Mate {
        pins: crate::state::with(|s| s.ids().len()),
        freezes: capture::freeze_count(),
        remembered: capture::last_region().map(|l| l.rect),
    }
}

// ── 门那一侧：`--custom-window` 的骨架，与 `--settings-window` 同一副形状（前置写在
// `engine.load()` 之前：`CustomCaptureWindow.qml` 只在 `Component.onCompleted` 读一次计划）。

/// 计划那十五段：`腿等待 ms | 出界那块的 x y w h | 零宽高那对的 w h | 不是整数那一格的下标与
/// 写的字 | 越界那一枚延时 | 在界内那块的 x y w h | 延时腿的秒数`。
///
/// 缺一窗里那条腿就不跑（同 `settingsGaugePlan` 那一族：空串 = 产品会话与另外七道门）。坐标与尺寸
/// 由门钉死、`measure` 也从同一串里读回来判 —— 页上打的数与 Rust 对的数因此是同一枚数，而「出界」
/// 那一块用的是 `99999` 这种在任何桌面上都出界的数，「界内」那一块用的是 `10,10,64,48` 这种在任何
/// 桌面上都在桌上的数，所以这一档不需要知道本机分辨率（P165 门里第 5 腿同一个理由）。
pub fn arm() -> String {
    let plan = format!(
        "{LEG_MS}|100|100|99999|99999|0|-40|0|abc|{}|10|10|64|48|{DELAY_LEG_SECONDS}",
        DELAY_SECONDS_MAX + 1
    );
    let pins = crate::state::with(|s| s.ids().len());
    let freezes = capture::freeze_count();
    with(|d| {
        d.armed = true;
        d.reason = String::new();
        d.plan = plan.clone();
        d.lines.clear();
        d.stamps.clear();
        d.mates.clear();
        d.text = Default::default();
        d.note.clear();
        d.countdown = None;
        d.pending = None;
        d.starts = 0;
        d.refused = 0;
        d.fired = 0;
        d.opened = 0;
        d.last_wait = (0, 0);
        d.pins_at_arm = pins;
        d.freezes_at_arm = freezes;
    });
    format!(
        "计划 {plan}；腿等待 {LEG_MS} ms；开工时在册贴图 {pins} 枚、已冻面 {freezes} 次；闹钟地板 {MIN_WINDOW_MS} ms"
    )
}

/// 计划那一串，原文。窗读它自己那一格用。
pub fn plan_line() -> String {
    with(|d| d.plan.clone())
}

/// 窗读它自己那一格的入口：这一趟该不该有一扇对话框窗。
pub fn window_wanted() -> bool {
    with(|d| d.armed)
}

/// 窗递回来的一行，连同**递回来那一刻**的三枚旁证。前缀是那一腿的名字，其余原样存着。
pub fn report(line: &str) {
    let m = mate();
    with(|d| {
        d.lines.push(line.to_string());
        d.stamps.push(arm_at().elapsed().as_millis() as u64);
        d.mates.push(m);
    });
}

/// 窗一共递回来了哪些行，每行末尾挂一枚「落在 `arm()` 之后第几毫秒」的戳，再缀上那一刻的三枚旁证。
/// 打印与「窗到底有没有说话」那一格用。
pub fn seen() -> Vec<String> {
    with(|d| {
        d.lines
            .iter()
            .enumerate()
            .map(|(i, l)| match (d.stamps.get(i), d.mates.get(i)) {
                (Some(ms), Some(m)) => format!(
                    "{l} @{ms}ms（贴图 {} 枚、冻面 {} 次、记忆 {:?}）",
                    m.pins, m.freezes, m.remembered
                ),
                _ => l.clone(),
            })
            .collect()
    })
}

/// 某一腿递回来的那些行的尾巴（第一段 `|` 之前是腿名，其余**整段留着** —— 句子里的 `|` 不归这里切）。
fn rows_of(prefix: &str) -> Vec<String> {
    with(|d| {
        d.lines
            .iter()
            .filter(|l| l.split('|').next() == Some(prefix))
            .filter_map(|l| l.split_once('|').map(|(_, tail)| tail.to_string()))
            .collect()
    })
}

/// 某一腿**头一行**递回来那一刻的旁证。
fn mate_of(prefix: &str) -> Option<Mate> {
    with(|d| {
        d.lines
            .iter()
            .position(|l| l.split('|').next() == Some(prefix))
            .and_then(|i| d.mates.get(i).cloned())
    })
}

/// 计划里那一段读成一个数。缺一窗里那条腿就不跑，但 `measure` 走到这里时窗已经跑过，所以段数不够
/// 是**这一档自己的坏**，要说成一句判不过而不是 panic。
fn seg(plan: &[&str], i: usize) -> i64 {
    plan.get(i)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(i64::MIN)
}

/// 计划里那一段读成一枚宽高（⑤ 与 ⑦ 那两块区域用）。负数与读不出来的段都落成 0：一块 0 宽的区域
/// 在这一档里只会被 [`crate::capture::size_guard`] 拒掉，而那一格（②）判的是窗**递回来的那句拒绝**、
/// 不是这里现算的矩形，所以一串坏掉的段只会让格子变红，不会把判据蒙过去。
fn seg_u32(plan: &[&str], i: usize) -> u32 {
    seg(plan, i).max(0) as u32
}

/// 这一档的报告：八格判定 + 窗递回的每一行原样附在后面。
///
/// 判的全是**窗那一侧**递回来的话；本档手里那份模型只用来对、不用来说（与 `--settings-window` 同一
/// 分工）。窗一行也没递回来 ⇒ BLOCKED 而不是 FAIL：那一格问的是「这一台机器上有没有那扇窗」，而
/// QML 文档编译不过与页上少一格是两种坏。
pub fn measure(want_ms: i32) -> (Check, String) {
    use crate::r13::Report;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (armed, reason, plan) = with(|d| (d.armed, d.reason.clone(), d.plan.clone()));
    if !armed {
        rep.row(
            "custom window",
            Check::Blocked,
            format!("这一趟没有 {want_ms} ms 的计划：{reason}"),
        );
        return rep.finish("custom-window door");
    }
    let p: Vec<&str> = plan.split('|').collect();
    let (clip, badwh, badidx, badword, over, inb, dsec) = (
        PhysRect::new(
            seg(&p, 1) as i32,
            seg(&p, 2) as i32,
            seg_u32(&p, 3),
            seg_u32(&p, 4),
        ),
        (seg(&p, 5) as i32, seg(&p, 6) as i32),
        seg(&p, 7) as usize,
        p.get(8).copied().unwrap_or_default().to_string(),
        seg(&p, 9) as i32,
        PhysRect::new(
            seg(&p, 10) as i32,
            seg(&p, 11) as i32,
            seg_u32(&p, 12),
            seg_u32(&p, 13),
        ),
        seg(&p, 14),
    );
    let (pins_at_arm, freezes_at_arm) = with(|d| (d.pins_at_arm, d.freezes_at_arm));

    let seen_lines = seen();
    if seen_lines.is_empty() {
        rep.row(
            "custom window",
            Check::Blocked,
            // 这里没有第二枚读数可印：`qml_loaded` 是 `--probe` 那一档的证人，本档手里只有「窗一共
            // 递回几行」这一枚数，而它此刻是 0。分界写在页上 —— `born` 那一行由
            // `CustomCaptureWindow.qml` 的 `Component.onCompleted` 递回，有 `born` 而无 `open` 才是
            // 「画出来了而腿没跑」。
            "窗没有递回任何一行 —— 「窗压根没被造出来」与「造出来了而腿没跑」在这一趟还没分开过；\
             分界是页上那行 `born`（见 `CustomCaptureWindow.qml`）"
                .to_string(),
        );
        return rep.finish("custom-window door");
    }

    // ① 窗在不在、页上几格。`visible` 是窗自己问 Qt 的那一枚数，不是「我们叫过 show()」。
    let open = rows_of("open");
    let tail = open.first().map(String::as_str).unwrap_or_default();
    let f: Vec<&str> = tail.split('|').collect();
    let ok = f.len() == 3
        && f[0] == "1"
        && f[1].parse::<usize>() == Ok(CELLS.len())
        && f[2].parse::<usize>() == Ok(CELLS.len());
    rep.row(
        "custom window",
        Check::from(ok),
        format!(
            "窗自报 可见={} 页上格数={} 名单长度={}（名单 {:?}）；窗一共递回 {} 行",
            f.first().copied().unwrap_or("?"),
            f.get(1).copied().unwrap_or("?"),
            f.get(2).copied().unwrap_or("?"),
            CELLS.join("|"),
            seen_lines.len()
        ),
    );

    // ② 第 307 行在这一扇窗上的那一句，必须**与本档现算的那一句一字不差** —— 而本档现算走的是与
    //    命令行同一个作者。差一个标点都是红。
    let expect = capture::size_guard(&title(), badwh.0, badwh.1)
        .err()
        .unwrap_or_default();
    let said = rows_of("badsize").first().cloned().unwrap_or_default();
    let (starts, refused) = with(|d| (d.starts, d.refused));
    rep.row(
        "custom 宽高必须大于 0",
        Check::from(said == expect),
        format!(
            "窗递回 {said:?}；同一趟现算 {expect:?}；按下 {starts} 回、被拒 {refused} 回（第 307 行的\
             措辞作者因此是同一位）"
        ),
    );

    // ③ 一格写的不是数：点名哪一格、页上此刻写的字。
    let label = CELLS.get(badidx).copied().unwrap_or_default();
    let expect = int_note(label, &badword);
    let said = rows_of("badint").first().cloned().unwrap_or_default();
    rep.row(
        "custom 一格不是整数",
        Check::from(said == expect),
        format!("窗递回 {said:?}；现算 {expect:?}（点的是第 {badidx} 格 {label:?}）"),
    );

    // ④ 延时的界与档里的钳位同源。
    let expect = delay_note(&over.to_string());
    let said = rows_of("baddelay").first().cloned().unwrap_or_default();
    rep.row(
        "custom 延时的界",
        Check::from(said == expect),
        format!(
            "窗递回 {said:?}；现算 {expect:?}；界 = {}..={}",
            DELAY_SECONDS_MIN, DELAY_SECONDS_MAX
        ),
    );

    // ④½ 第 309 行「应支持键盘方向键微调数值」：页上那两枚方向键叫的是 [`nudge`]，而挪出来的那枚数
    //    必须真的是「挪之前那一个 + 1」。这一格读的仍是窗递回来的那一对读数 —— 本档手里那份模型
    //    只用来对（与上面三格同一分工），所以窗跳过这一腿时它会因为等不到行而红。
    let nudged = rows_of("nudge").first().cloned().unwrap_or_default();
    let parts: Vec<&str> = nudged.splitn(2, '|').collect();
    let (before, after) = (
        parts.first().and_then(|v| v.trim().parse::<i32>().ok()),
        parts.get(1).and_then(|v| v.trim().parse::<i32>().ok()),
    );
    let typed = seg(&p, 11);
    rep.row(
        "custom 方向键微调",
        Check::from(before == Some(typed as i32) && after == Some(typed as i32 + 1)),
        format!(
            "窗递回 {nudged:?}；计划里那一格写的是 {typed}（页上方向键往上一枚，交回的要的是 {}）",
            typed + 1
        ),
    );

    // ⑤ 第 308 行「坐标超出屏幕时应提示或自动修正」：裁掉的是桌面那一截，说的是 `clip_note` 的原话。
    //    三样现读同场对账：窗递回那行、那一刻的记忆是哪一块、那一刻在册几枚贴图。
    let want_note = capture::clip_note(&clip);
    let said = rows_of("clip").first().cloned().unwrap_or_default();
    let m = mate_of("clip");
    let remembered = m.as_ref().and_then(|x| x.remembered);
    let pins = m.map(|x| x.pins).unwrap_or(usize::MAX);
    let corrected = remembered.map(|r| r != clip && r.w < clip.w && r.h < clip.h);
    rep.row(
        "custom 出界已裁并提示",
        Check::from(
            said.starts_with("贴图 ") && said.contains(want_note.as_str())
                && corrected.unwrap_or(false)
                && pins == pins_at_arm + 1,
        ),
        format!(
            "窗递回 {said:?}；那一刻记忆 = {remembered:?}（要的是 {clip:?}）；那一刻在册贴图 {pins} 枚、\
             开工时 {pins_at_arm} 枚；判据 = 句子里含 `clip_note` 原文 **且** 记下来的是更小的那块 \
             **且** 贴图因此多一枚"
        ),
    );

    // ⑥ 完全出桌面：不许假装成功。真出界时 `region_frame` 交回的是 core 的拒绝，页上那句因此不能是
    //    「已钉」，而贴图一枚也不能多。
    let said = rows_of("offscreen").first().cloned().unwrap_or_default();
    let grew = mate_of("offscreen").map(|x| x.pins).unwrap_or(usize::MAX);
    rep.row(
        "custom 完全出桌面不写图",
        Check::from(!said.is_empty() && !said.starts_with("贴图 ") && grew == pins_at_arm),
        format!("窗递回 {said:?}；那一刻在册贴图 {grew} 枚（开工时 {pins_at_arm} 枚）"),
    );

    // ⑦ 第 301 行 + 第 302 行 + §5.2.4 第五步那一支：**延时数满才冻面**。四枚现读同场对账：实测毫秒、
    //    计划毫秒、那一刻冻面次数比开工时多、记忆是那块在界内的区域。
    let armed_said = rows_of("delay-armed").first().cloned().unwrap_or_default();
    let fired_tail = rows_of("delay-fired").first().cloned().unwrap_or_default();
    // 三刀切定：实测毫秒 | 计划毫秒 | 那一句话。句子留成**整段**，因为 `pinned_line` 那句话里带着
    // 原文（`region_line` 的括号里还有 `clip_note`），多切一刀就会把一句话切成两半去判。
    let parts: Vec<&str> = fired_tail.splitn(3, '|').collect();
    let (held, planned) = (
        parts
            .first()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0),
        parts
            .get(1)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0),
    );
    let line = parts.get(2).copied().unwrap_or_default();
    let m = mate_of("delay-fired");
    let (froze, remembered) = (
        m.as_ref().map(|x| x.freezes).unwrap_or(0),
        m.as_ref().and_then(|x| x.remembered),
    );
    let ok = held >= planned
        && planned == (dsec.max(0) as u64) * delay::STEP_MS
        && line.starts_with("贴图 ")
        && remembered == Some(inb)
        && froze > freezes_at_arm
        && armed_said.contains(&format!("还剩 {dsec} 枚数"));
    rep.row(
        "custom 延时数满才截",
        Check::from(ok),
        format!(
            "按下时窗说 {armed_said:?}；数满那行 = 实测 {held} ms | 计划 {planned} ms | {line:?}；\
             那一刻记忆 = {remembered:?}（要求 {inb:?}）、冻面 {froze} 次（开工时 {freezes_at_arm} 次）"
        ),
    );

    // ⑧ 这一趟一共说了几句拒绝：被拒的次数必须与三条形状腿的数对得上。它不问「页上红不红」，问的是
    //    **这一档的账**，为的是形状三腿里任何一条被窗跳过时，②③④ 那三格会因为「等不到行」而红，
    //    而这里给出的是同一件事的第二个读数。
    let close = rows_of("close");
    rep.note(
        "窗自己收窗那一行",
        format!(
            "{:?}（按下 {starts} 回、被拒 {refused} 回、数满抓成 {} 场、窗被要开 {} 回）",
            close.first(),
            with(|d| d.fired),
            with(|d| d.opened)
        ),
    );
    for l in &seen_lines {
        rep.note("窗递回", l.clone());
    }
    rep.finish("custom-window door")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Dialog` 是进程级的一格，本档这些测试各填各的五格 ⇒ 必须排队。与 `delay::global_gate()`
    /// 同一条教训。
    fn gate() -> std::sync::MutexGuard<'static, ()> {
        static G: std::sync::Mutex<()> = std::sync::Mutex::new(());
        G.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn typed(vals: [&str; CELLS.len()]) -> Dialog {
        Dialog {
            text: vals.map(|v| v.to_string()),
            ..Dialog::default()
        }
    }

    /// 名单与提示同长同序，且名字就是 PRD 那五格。
    #[test]
    fn the_name_list_is_five_with_one_hint_each() {
        assert_eq!(CELLS, ["X", "Y", "宽度", "高度", "延时"]);
        assert_eq!(labels().split('|').count(), CELLS.len());
        assert_eq!(hints().split('|').count(), CELLS.len());
        assert_eq!(START_WORD, "开始截图");
        assert_eq!(title(), "自定义截图");
    }

    /// 第 307 行在这一扇窗上的那句，必须与命令行那句**只差那个名字**。
    #[test]
    fn the_size_rule_is_one_sentence_with_two_readers() {
        let cli = capture::size_guard("--region", 0, -40).err().unwrap();
        let win = capture::size_guard(&title(), 0, -40).err().unwrap();
        let tail = "（给的是 w=0, h=-40）：0 或负数不是一块可截取的区域";
        assert!(cli.starts_with("--region 的宽度和高度必须大于 0"));
        assert!(win.starts_with(&format!("{} 的宽度和高度必须大于 0", title())));
        assert!(cli.ends_with(tail) && win.ends_with(tail), "{cli}\n{win}");
        assert!(capture::size_guard(&title(), 1, 1).is_ok());
    }

    /// 空的一格、不是数的一格、越界的延时：三句都点名到**那一格**，而 `read` 一次也不返回区域。
    #[test]
    fn each_bad_cell_is_named_and_gives_no_rectangle() {
        for (vals, needle) in [
            (["", "0", "10", "10", "0"], "「X」这一格还空着"),
            (["abc", "0", "10", "10", "0"], "「X」这一格要的是整数"),
            (["1", "2", "10", "0", "0"], "宽度和高度必须大于 0"),
            (["1", "2", "10", "10", "301"], "延时要的是 0 到 300"),
            (["1", "2", "10", "10", "x"], "延时要的是 0 到 300"),
        ] {
            let got = read(&typed(vals)).err().unwrap_or_default();
            assert!(got.contains(needle), "{vals:?} 说的是 {got:?}");
            assert!(read(&typed(vals)).is_err(), "{vals:?} 竟然过了");
        }
    }

    /// 界内那一枚延时**放得过去**：页上拒的数与档里钳的数同源，所以 300 不能拒、301 必须拒。
    /// 这一格是 [`delay_note`] 那句话与 `Config::normalize` 那次 `clamp` 共用两枚常数的证人。
    #[test]
    fn the_delay_bound_is_the_same_number_the_config_clamps_to() {
        assert_eq!(
            falcon_core::config::Config::default().capture.delay_seconds,
            DELAY_SECONDS_MIN
        );
        assert!(read(&typed(["1", "2", "10", "10", "300"])).is_ok());
        assert!(read(&typed(["1", "2", "10", "10", "301"])).is_err());
        assert_eq!(read(&typed(["-7", "2", "10", "10", ""])).unwrap().0.x, -7);
        assert_eq!(read(&typed(["1", "2", "10", "10", ""])).unwrap().1, 0);
    }

    /// 第 309 行那一枚「微调」的算术：往上一枚就是 +1、往下一枚就是 -1，空的那一格从 0 起步，
    /// 而写着不是数的字**原样交回**（悄悄把坏格改成一枚数，等于替用户抹掉他还没改对的那一格）。
    #[test]
    fn the_arrow_steps_one_and_leaves_a_broken_cell_alone() {
        let _g = gate();
        set(0, "12");
        assert_eq!(nudge(0, 1), "13");
        assert_eq!(nudge(0, -1), "12");
        set(1, "");
        assert_eq!(nudge(1, 1), "1");
        assert_eq!(nudge(1, -1), "0");
        set(2, "abc");
        assert_eq!(nudge(2, 1), "abc");
        assert_eq!(text(2), "abc", "坏格被挪了一枚就变成另一枚数了");
        set(3, "-5");
        assert_eq!(nudge(3, 1), "-4", "负坐标往上是往 0 靠");
    }

    /// 打字那一刻就说这一格的形状坏了，而空格不算坏（还没填而已）。
    #[test]
    fn typing_reports_the_shape_while_an_empty_cell_stays_neutral() {
        let _g = gate();
        assert_eq!(set(2, ""), "");
        assert_eq!(set(2, "xyz"), int_note(CELLS[2], "xyz"));
        assert_eq!(set(2, "-5"), "");
        assert_eq!(set(4, "301"), delay_note("301"));
        assert_eq!(set(4, "0"), "");
    }

    /// 延时那一支**当场不冻面**：挂上倒计时的那一刻，冻面次数一次也不能多。这一条是 PRD 第 420 行
    /// 「倒计时结束那一刻」在这一扇窗上唯一能不靠真时间问出的那一半（另一半「等满了才冻」由门的
    /// 第七格读实测毫秒判）。
    #[test]
    fn arming_a_delay_freezes_nothing() {
        let _g = gate();
        let before = capture::freeze_count();
        for (i, v) in ["100", "100", "64", "48", "2"].iter().enumerate() {
            set(i, v);
        }
        let said = start();
        assert!(said.contains("还剩 2 枚数"), "{said}");
        assert_eq!(left(), 2);
        assert_eq!(
            capture::freeze_count(),
            before,
            "挂上倒计时的那一刻就冻了面"
        );
        assert_eq!(tick(), "", "还在数就什么都不该说");
        assert_eq!(capture::freeze_count(), before);
        assert_eq!(note(), said, "页上那格读的就该是按下那一刻说的话");
    }

    /// 一场上限一句话：数满之后槽即清空，再叫 [`tick`] 读到的是空槽 ⇒ 既没有第二句话，也没有第二次
    /// 冻结。这里把 `pending` 故意留空，为的是**不**去动真画面 —— 真抓一张是门第七格的事。
    #[test]
    fn one_countdown_speaks_once() {
        let _g = gate();
        let fired_before = with(|d| d.fired);
        with(|d| {
            d.countdown = Some(Countdown::new(
                vec![1],
                delay::STEP_MS,
                delay::clock_ms().saturating_sub(5_000),
            ));
            d.pending = None;
        });
        assert_eq!(tick(), "", "手里没有区域就不该说任何一句成功");
        assert_eq!(left(), 0, "数满的那一次必须把槽清空");
        assert_eq!(tick(), "", "第二次调用不能拿到第二个 Go");
        assert_eq!(with(|d| d.fired), fired_before, "一场倒计时只能抓一张");
        assert_ne!(wait_line(), "0|0", "数满那一刻的两枚毫秒要留下来给页递回去");
    }

    /// 计划那十五段与 `measure` 读回来的那几枚数同源：段数、延时腿秒数、出界那块的尺寸。
    #[test]
    fn the_plan_is_fifteen_segments_and_measure_reads_it_back() {
        let s = arm();
        assert!(s.contains("腿等待 300 ms"), "{s}");
        let plan = plan_line();
        let p: Vec<&str> = plan.split('|').collect();
        assert_eq!(p.len(), 15, "{plan}");
        assert_eq!(seg(&p, 3), 99999);
        assert_eq!(
            seg(&p, 14),
            delay::countdown(DELAY_LEG_SECONDS as u32).len() as i64
        );
        assert!(window_wanted());
        report("open|1|5|5");
        assert_eq!(rows_of("open"), vec!["1|5|5".to_string()]);
        assert!(seen()[0].contains("贴图"));
        with(|d| {
            d.armed = false;
            d.lines.clear();
            d.stamps.clear();
            d.mates.clear();
            d.plan.clear();
        });
    }
}
