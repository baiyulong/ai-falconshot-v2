//! The pin window's own output doors, walked from QML.
//!
//! `--pin <ms>` plants a real pin, lets the event loop build its window, and then
//! has *that window's* QML drive the two things its menu offers - 复制图片 and
//! 另存为… - and report back what each one answered. §10 第 37 项 ① is the reason
//! this mode exists rather than another row in `--ink`: until P37 the only
//! readings of the pin's save path came from Rust calling
//! [`crate::state::PinState::save_image`] directly, so the QML half - the menu item,
//! the dialog, the filter list, the string that reaches the user - had never been
//! through a machine at all. That is how an English sentence survived inside a
//! Chinese prefix (`保存失败：pin 3 has never been drawn`) for as long as it did:
//! nothing that was graded had ever run it.
//!
//! The leg is therefore *QML's*, and this file only holds the plan and grades the
//! lines [`report`] receives. Since P38 it no longer stops at calling the door's own
//! statements: one leg opens the real 另存为 dialog, has that native window's default
//! button pressed with `BM_CLICK`, and reads what `onAccepted` left on disk. That is
//! still not a synthetic click - it is the same message the shell's own mouse handling
//! sends, to a dialog this process is showing, found by pid so that no other program's
//! window is ever a target.
//!
//! ## What it does not claim
//!
//! The press is a message to a button, not a mouse event at a coordinate: where the
//! dialog sat on the screen, and whether a human could have reached it there, are not
//! what §10 第 38 项 ① asks. And the question that turned out to be about Qt rather
//! than about the product - can a dialog that was never `open()`ed still fire its own
//! `accepted`? - is reported as a note now, because whatever it answers says something
//! about this Qt and nothing about a promise to a user.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use falcon_core::geometry::{PhysPoint, PhysRect};
use falcon_core::pin::PinId;

use crate::mask_check::TITLE;
use crate::r13::{parse_levels, Report};
use crate::session::shim;
use crate::state;
use crate::state::Check;

/// The directory the run writes into, under the OS temp dir. A name this process
/// chose, so a leftover from the previous run is recognisable - and deleted, because
/// a file that was already there is not this run's answer.
pub const DIR: &str = "falconshot-pin-gauge";

/// The pin the gauge owns. The size is `--demo`'s default so the two modes measure
/// the same picture, and a checkerboard's 2 px transparent margin is what gives the
/// JPG leg a loss to report about ([`crate::state::demo_frame`]).
pub const DEMO_W: u32 = 320;
pub const DEMO_H: u32 = 200;

/// An id far past anything the set can reach, for the two legs that have to fail.
///
/// `+ 999_999` rather than a fixed number because the id the pin got is whatever
/// `PinSet` handed out: the offset stays un-taken on the first pin and on the
/// thousandth.
pub const NEVER_OFFSET: PinId = 999_999;

/// How long after its window is built the leg waits before it reads anything.
///
/// Not immediately: the QQuickWindow's native side is made on first exposure, which
/// is an event-loop step after `Component.onCompleted`, and `pin window` asks Win32
/// for the frame. A leg that ran at once would report a missing window as a missing
/// feature. The interval lives here and travels to QML inside [`plan_line`], because
/// a second copy of the number in the other language is a pair that can drift apart
/// without either file looking wrong.
pub const LEG_DELAY_MS: i32 = 400;

/// When the *clip's* own delegate reads its GIF state: one eighth of [`LEG_DELAY_MS`].
///
/// The fraction is the schedule, not a speed-up. Every other leg of this run waits a full
/// `LEG_DELAY_MS` after its window was built, and two timers expiring inside the same
/// event-loop step are dispatched in an order this file cannot read from the source - so a
/// clip leg armed at the same delay would arrive at an undetermined place in
/// [`PIN_SCHEDULE`], and the graded row would then be grading Qt's timer list. Armed at
/// an eighth it is certain to speak before the subject's first line, and its one tail wait
/// still lands before the dialog's first press, which is where the schedule puts it.
///
/// It reads nothing Win32 owns, so it does not need the exposure the full delay is for:
/// frames, play state and the menu's own visibility are Qt-side values the state machine
/// filled when the pin was assigned.
///
/// The number travels to QML inside [`plan_line`] for the usual reason: `/ 8` written in
/// two languages is a pair that drifts.
pub const GIF_ARM_MS: i32 = LEG_DELAY_MS / 8;

/// How many items §5.9.14's 播放控制 section is, as read from this round's
/// `PinWindow.qml`. A `#[cfg(test)]` test counts the occurrences of the section's own
/// visibility condition in the file, so this number cannot outlive the menu it describes;
/// the graded row compares the *live* window's count against it, on a clip (all of them)
/// and on a still (none).
pub const GIF_MENU_ITEMS: usize = 7;

/// How many items §5.9.15's 背景模式 section is. Counted the same way as the number above —
/// a `#[cfg(test)]` test reads the section's own click handler out of `PinWindow.qml` — for
/// the same reason: four is also [`falcon_core::config::AlphaBg::index`]'s domain, and a menu
/// that grew a fifth row while this constant stayed at four would have the *state machine*
/// refuse the row (`AlphaBg::from_index(4)` is `None`) and this row still print PASS.
pub const ALPHA_MENU_ITEMS: usize = 4;

/// How the *subject's* menu leg waits: this interval twice, once to arm the leg and once for
/// its tail, so its shut count is read at a hundred milliseconds and its open count at two
/// hundred.
///
/// P128 C 档的真机读数把这件事钉住了：同一枚 `gifMenuItemCount()` 在两扇窗上都答 0，因为那一节
/// 的七枚 `MenuItem` 在菜单没打开时不交出可见性——所以两枚计数都要在**菜单开着**的时候读，而
/// 「静置窗读回 0」这一枚反例必须由同一个读法供，否则它只是一个没被实例化的形状。
///
/// 所以计数必须落在 `open()` 之后的**另一个**事件循环轮次里，而那一轮不能排在那扇对话框后面：
/// `saveAs.open()` 交出去的是 shell 自己的模态循环，本文件对它没有任何承诺。臂也不取
/// [`GIF_ARM_MS`]：那一毫秒是**会动的那一扇**读它自己那枚关着的计数的时刻，两扇窗的定时器在同一个
/// 事件循环步里到期时谁先跑，本文件从源码读不出来（[`GIF_ARM_MS`] 存在的同一个理由）。一枚本常数
/// 隔开两扇的臂，两条反证之间就有了一个由算术决定的先后；静置窗那一读落在第二百毫秒，在它自己那枚
/// `menu`（第四百）之前，也在会动的那一扇的尾读（第四百五十）之外整整二百五十。
///
/// A quarter of the delay, and it is spent inside the subject's own pre-dialog phase: the
/// first thing that can disturb a timer here is the shell's modal loop, and that is two full
/// waits away.
pub const GIF_STILL_MS: i32 = LEG_DELAY_MS / 4;

/// How many times the leg may ask the shell for its button before it stops asking.
///
/// The wait is not a courtesy: Qt returns from `open()` with the dialog marked visible
/// while Win32 has not made the `#32770` yet, and the press has to land on that window.
/// Three runs of P38 say when it appears - one had it at the third wait, one at the
/// fourth, and one never had it inside four, which is the same measurement read as a
/// passing run and a failing one by nothing but the cap. Ten waits is two and a half
/// times the slowest arrival seen, and the cap still has to exist: a shell that never
/// shows its dialog must end the run with a `pushed=no`, not with an endless poll.
///
/// The number travels to QML inside [`plan_line`] rather than being written there too,
/// and [`LEG_WAITS`] is derived from it, because these three files otherwise hold three
/// copies of one schedule.
pub const PUSH_TRIES_MAX: i32 = 10;

/// The waits the leg actually spends, once its window exists: one for the exposure and
/// the leg's first half, up to [`PUSH_TRIES_MAX`] for the native dialog's window, one for
/// the tail that reads what `onAccepted` left behind, one for the tail of the close -
/// §10 第 55 项's census taken a loop turn after `closePin`, because a window Qt has not
/// retired yet is not a window the screen lost - and one more after the leg says it
/// is finished, which is the grace [`report`] arms so the run ends when the measurement
/// does rather than when a guessed budget runs out. They are phases rather than one
/// straight run because a door that is opened by a real click cannot be driven inside a
/// single event-loop turn: the click has to land while the dialog's own loop is running.
///
/// §5.9.14's clip legs are deliberately *not* counted here: they run beside the subject's
/// chain on their own window (`GIF_ARM_MS` plus one wait, both of which fit inside the
/// subject's second wait), so the longest path of this run is unchanged and a budget grown
/// for them would be a number this schedule does not owe.
pub const LEG_WAITS: i32 = PUSH_TRIES_MAX + 4;

/// What the leg is allowed to spend *besides* its own schedule.
///
/// The floor's clock does not start where the plan is armed: `main` arms it just before
/// `app.exec()`, which is after the QML document has loaded and reached
/// `Component.onCompleted` (stamped `loaded`). So this grace is slack for whatever else
/// the event loop has to do before the leg's first wait fires - not for start-up, and
/// certainly not for `--ink`'s script, which runs before the engine exists at all and
/// took 15.6 s of the 19.7 s the P38 ink leg's `起步` span shows.
///
/// Three runs of P38 bracket the arm→leg gap itself: the one whose tail went missing
/// placed it between 800 and 1200 ms, and the two that printed it measured **2917 ms** and
/// **3787 ms** for the same distance - so that gap is neither small nor stable, and the
/// report prints what the current run measured rather than what this paragraph claims.
pub const STARTUP_GRACE_MS: i32 = 5000;

/// The shortest `--pin` run that can have measured anything - a *backstop*, not the
/// schedule: the leg ends itself one wait after it prints `done` (see [`report`]), so a
/// run that gets there early does not sit for the rest of this. What the number is for is
/// the run that never gets there, because a truncated leg reports its own absence as
/// several failing rows and that is the most expensive mistake this gauge can make.
///
/// A run the user asked for shorter than this is raised rather than refused - it asked
/// for the mode, and the mode simply needs that much time. `main` prints both the
/// requested and the effective number (§10 第 38 项 ②), and the report prints how long the
/// run really was.
pub const MIN_MS: i32 = STARTUP_GRACE_MS + LEG_WAITS * LEG_DELAY_MS;

/// The four intervals one leg's timing is made of, and who ended the run.
///
/// Both doors read the same four stamps and the same schedule, so the arithmetic lives
/// here once: `--pin`'s leg and `--ink`'s mask leg differ only in which names were stamped.
///
/// The question this answers is *"which of these numbers was too small?"* - and the first
/// P38 answer to it was wrong. `全程` measured from the plan's arming against the floor
/// said `是地板收尾的` for an ink leg that had in fact said `done` 3.5 s earlier; the floor
/// never competes with start-up (see [`STARTUP_GRACE_MS`]) and the run had closed on the
/// leg's own word. So the attribution is read off `done`'s own two intervals rather than
/// off a total: the wait between `done` and the read-back is the leg's tail, and anything
/// longer than that is the backstop having arrived first.
pub(crate) struct LegTiming {
    /// 种旗标 → 腿开口.
    pub start: Option<u64>,
    /// 腿开口 → 那一句 `done`.
    pub own: Option<u64>,
    /// `done` → 读表.
    pub tail: Option<u64>,
    /// 种旗标 → 读表.
    pub total: Option<u64>,
    /// The one wait the leg arms its own end with.
    pub delay: u64,
    /// The waits the schedule spends.
    pub waits: u64,
    pub grace: u64,
    pub min: u64,
}

/// The four stamps one leg speaks, by name, and nothing else. This is the only thing the
/// two doors differ in - which is why it is one argument and the schedule it is measured
/// against is four: each door hands its own `delay`/`waits`/`grace`/`min`, so the line
/// quotes the wait *that* door asked for rather than a number this module guessed.
pub(crate) struct LegStamps {
    pub arm: &'static str,
    pub leg: &'static str,
    pub done: &'static str,
    pub end: &'static str,
}

impl LegTiming {
    /// Read the four intervals out of `state`, by stamp name.
    pub(crate) fn read(stamps: &LegStamps, delay: i32, waits: i32, grace: i32, min: i32) -> Self {
        Self {
            start: state::between(stamps.arm, stamps.leg),
            own: state::between(stamps.leg, stamps.done),
            tail: state::between(stamps.done, stamps.end),
            total: state::between(stamps.arm, stamps.end),
            delay: delay as u64,
            waits: waits as u64,
            grace: grace as u64,
            min: min as u64,
        }
    }

    pub(crate) fn line(&self) -> String {
        let Some(start) = self.start else {
            return "没有读数：那条腿一次也没说话".to_string();
        };
        let head = format!(
            "起步 {start} ms（种旗标→腿开口；地板的钟从 loaded 之后才走，另留 {} ms 松弛），",
            self.grace
        );
        let sched = format!("{} 次 × {} ms 的日程", self.waits, self.delay);
        match (self.own, self.tail, self.total) {
            (Some(own), Some(tail), Some(total)) => {
                let who = if tail <= 2 * self.delay {
                    format!(
                        "是腿自己那一句 done 之后 {tail} ms 收的尾，地板（{} ms）没插手",
                        self.min
                    )
                } else {
                    format!(
                        "done 之后过了 {tail} ms 才收，超过 {} ms 的两等，是地板先到",
                        self.delay
                    )
                };
                format!(
                    "{head}腿自己 {own} ms（{sched}，说完了 done），收尾 {tail} ms —— {who}；全程 {total} ms"
                )
            }
            (own, _, Some(total)) => format!(
                "{head}腿没有说 done（{sched}，说到 {own:?} ms 为止），全程 {total} ms —— \
                 是地板（{} ms）收尾的，尾下那几行是没读到而不是读错",
                self.min
            ),
            (own, _, None) => format!("{head}读表那一步没有 stamp（腿说到 {own:?} ms 为止）"),
        }
    }
}

/// The eight names this run owns inside [`DIR`], in the order the plan keeps them.
/// `never.png` is written by the leg that must *not* write it; `dialog.png` is written
/// by the door a real click closed, and is the only file here whose name the gauge did
/// not choose - the dialog picked the extension off its own filter list. `from-file.png`
/// is written by this file's own plant (§10 第 43 项 ①), and is the only one here that is
/// *read* back in: the pin on the other side of it is the thing under test.
/// `from-gif.gif` is §5.9.14's: two frames this file writes through the same fixture the
/// selftest uses, pinned through the same door a dropped file goes through, so the window
/// on the other side of it is a clip rather than a picture.
const NAMES: [&str; 8] = [
    "qml.png",
    "qml.jpg",
    "rust.png",
    "rust.jpg",
    "never.png",
    "dialog.png",
    "from-file.png",
    "from-gif.gif",
];

/// The name the real-click leg hands the dialog, as a `currentFile` without an
/// extension. Whether the dialog then appends the selected filter's extension is
/// measured, not assumed: it is the `pin save via real click` row that says whether
/// `dialog.png` is what arrived. The stem travels in the plan rather than being written
/// in both languages, because a pair of copies of one string is a pair that drifts - and
/// because there is no `fileName` to write it into, which P38's probe F found by
/// assignment (`Cannot assign to non-existent property "fileName"`).
const DIALOG_STEM: &str = "dialog";

/// The formats §5.9.17's dialog offers, in the order `PinWindow.qml` lists them.
/// The extension is what picks the encoder, so this list is also the list of things
/// the door can promise to write.
pub(crate) const FILTERS: [&str; 3] = ["*.png", "*.jpg", "*.bmp"];

/// How far a window's device rect may sit from the pin's own rect and still be it.
/// The shim reads the frame Win32 has; the pin's rect is what the state machine
/// asked Qt for, in device pixels. Rounding on the logical-pixel way back
/// (`width: view.size_w / dpr`) is what this slack is for - the same 2 px the mask's
/// rect row allows.
const RECT_SLACK: i32 = 2;

/// The order this door's legs are meant to run in, as one list.
///
/// §10 第 41 项 ② is the reason a schedule is graded rather than merely counted: P40 added
/// a leg that writes `session.status` in front of the click's before-reading, and the row
/// count went up while the meaning of one of them went down - `pin dialog accept` kept
/// printing PASS because it still saw *two* different sentences, just no longer the pair it
/// was written to compare. Which leg runs when is therefore part of what a row means, and a
/// row is the only place here that can say so.
///
/// Two entries are allowed to be absent, because they are confessions rather than phases:
/// `dlg-dismiss` is the leg admitting it closed a door the click should have closed, and
/// `dlg-rejected` is that close's own signal. [`leg_order`] grades what arrived, so a leg
/// that did not run costs nothing - while a leg that runs and is *not* in this list costs
/// the row. That is the point of keeping the list in Rust instead of in a comment: adding a
/// leg means saying where it runs.
///
/// The ten `gif-*` keys are §5.9.14's, and six of them sit at the very front: the clip's
/// delegate speaks the first four, armed at [`GIF_ARM_MS`], and the subject's own menu leg
/// speaks the next two, armed at [`GIF_STILL_MS`] - places decided by arithmetic rather than
/// by timer dispatch order, which is the whole reason those two constants exist.
///
/// C 档那一趟真机把这五枚的位置全部改过一次，而判据一枚没动（#302/#353）：`MenuItem` 的可见性在
/// 菜单没打开时读不回那七项，两扇窗都答 0，于是「开着的读法」成了唯一能供出 7 与 0 那一读的读法。
/// 两枚开着的计数各按自己的算术落位——静置窗那一枚在第二百毫秒（两枚 [`GIF_STILL_MS`]，赶在它自己
/// 那枚 `menu` 之前），会动的那一枚在第四百五十（[`GIF_ARM_MS`] 加一整枚 [`LEG_DELAY_MS`]）——两枚
/// 关着的计数各留在自己那一腿的最前当反证，而静置窗那一枚的臂取本常数而非 [`GIF_ARM_MS`]，正是为了
/// 让两条反证之间也有一个算术决定的先后；`gif-menu-shape` 则是那一读的机制证人。
///
/// 三枚 `alpha-*` 不在 §5.9.14 的算术里，也不在自己新立的定时器里：它们挨着静置窗那一双腿跑（同
/// 一腿、同一事件循环步、代码顺序在前一枚之后），因为 §5.9.15 的读数要的是「菜单开着的那一瞬」，
/// 而全门只有静置那两枚定时器开这扇窗的菜单。为定时器再添一枚常数会造出两条反证之间新的先后问题，
/// 而这一轮的判据不需要新的先后 —— 它需要的是同一枚 `pinMenu.open()` 之内的一次读。
///
/// `copy-drawn` 是 §5.9.16 那两条门的第一次真机对面：它不另写一遍 `copyPin`，而是 `trigger()`
/// 菜单与快捷键共用的那枚 `Action`，所以它落在 `runGauge()` 里 `jpg` 之后、两枚反证之前——和
/// `never-copy` 用的是同一条腿的同一事件循环步，两句不同的话因此由同一趟供出，无需新的定时器。
/// 它排在 `pre-save` 之前是必须的：`pre-save` 读的是「点击前最后一句」，而那一句自 P41 起就是
/// `never-save` 的拒绝；把成功的一句插在更后面会让那枚已有的行比较两句不相干的话（P40 的 `f-jpg`
/// 就是这么被这枚日程抓住的）。
/// 十三枚 `sel-*` 排在 `menu` 之前、`alpha-turn` 之后，位置由它们长在哪儿决定：这一组是
/// `runGauge()` 的**头一段**，所以实测里它们必然与 `menu` 同一腿、同一事件循环步，只是走在它前面。
/// 为什么必须在同一段的最前：`sel-before-*` 与三次手势之间不许插进任何别的 `reload` —— 一旦有别的
/// 腿在中间叫过一次 `view.reload()`，"边亮了"就既可以由本轮那一声 `selection_epoch` 扇出来、也可以
/// 由那一叫来，这一枚读数就分不开两件事（#281 那一族）。而排在 `dlg-*` 与 `close-*` 之前，是给
/// 后面每一条腿留原状：这一段跑完名单是空的，`close-before` 读的 `covering=` / `pins=` 那几格
/// 因此一个字节也没被本轮动过。
///
/// 两枚 `sel-observe-*` 夹在中间，位置不是排出来的而是**叫出来的**：`sel-observe-on` 说在
/// `selectTarget(other, 1)` 那句的里面——notify 在 invokable 返回之前就扇完了，所以它必然晚于
/// `sel-add-target`、早于 `sel-add`；`sel-observe-off` 同理说在 `clearSelectionAction.trigger()`
/// 里面。谁要是把手势挪到别的步上、或把 notify 改成排队发，这两枚键的先后就会当场和日程不符，
/// 由 [`sel_rows`] 按段报出来（而不是由本段的措辞自证）。
///
/// 它们是**可重入的证人而不是腿**（[`REENTRANT_WITNESS`]）：P134 的 `des-*` 那一族要的三种名单
/// 形状（{a,b}、{本窗,a}、{a,b}）借不来多选那两手的任何一种，只能自己点名，于是同一趟里
/// `sel-observe-on` 必然被扇三遍。旧文写的是「不许多叫一次」，P134 把它订正成「多叫由段来分」：
/// 判据一枚没放宽——那一段里照样要恰好一句、且必须是**被点名那一枚**说的（放宽的读法会连
/// 「别的窗没亮」一起读不出来，那是 #433 的另一面）。
///
/// 十五枚 `mov-*`（§5.10.2）不另起一段，而是夹在 `sel-*` 中间：它们要的**名单长度**恰好就是多选
/// 那两手跑出来的状态——`sel-plain` 之后场上只本窗一枚（拖它该交回 1），`sel-add` 之后有本窗与
/// 被点名那一枚两枚（拖本窗该交回 2）。另起一段就得自己再 `selectPin` 一遍，而 `selectTarget`
/// 多叫一次会把 `sel-observe-on` 那一声多扇一遍——那一声是证人不是腿，多出来的那几句由
/// [`sel_rows`] 按段分开，见上面那封（P134 订正）。`mov-batch-*` 三枚在 `other` 为 -1 时一句也不
/// 说，由 `mov_rows` 读成「这一格没有问」而不是读成 1（#453）。
///
/// 四枚反手读数（`mov-undo-self` / `mov-undo-other` / `mov-epoch-undo` / `mov-pos-after`）排在
/// `sel-cleared-self` **之后**，因为反手必须走在名单已经
/// 空了的那一刻：本窗被两拖搬过 (40,20) 而另一枚只被搬过 (23,11)，两者差一枚「单枚那一拖」，而
/// 名单还两枚时的任何反手都是按名单搬两枚，抹平一枚就多出另一枚。空名单那一路一次只搬被拖的那
/// 一扇，才谈得上按每一枚各自的位移抹回去。它们不是装饰：上一趟真机没有这四拖，`pin window` 被
/// 自己这一族的拖动挪到了 648,400（要的是 608,380），`pin pile *` 两枚跟着读「没有窗」——本族
/// 的腿把别家的行弄红了，盘证 `.qoder\scratch\p132_door_A.txt`。
///
/// 两枚 `mov-rev-*` 一前一后夹住 `mov-lone`，读的是本窗自己那枚纹理号：搬动只改位置，而
/// `content_key` 不含位置，所以两枚必须相等。它不等就是每一次鼠标移动事件都在重画一张 PNG，
/// 那一问与「搬几枚」和「发不发号」都不是同一件事，因此单独占一枚判据（[`mov_rows`]）。
///
/// 二十四枚 `op-*`（§5.10.3）与 `mov-*` 同族，也夹在 `sel-*` 中间，因为它要的名单长度就是那两手
/// 跑出来的状态：七枚 `op-lone-*` 长在 `mov-rev-lone` **之后**（那时名单里只有本窗一枚，绝对值那一手
/// 该交回 1），三枚 `op-*-ghost` 长在 `mov-epoch-ghost` 之后（同一枚还没被发出去的 id，透明度那一手
/// 也必须交 0 且**不发号**），十四枚批量那几枚长在 `mov-batch-epoch` 之后（名单两枚时两只手都该交回
/// 2）。它们不另起一段的理由与上面那封信同一枚：`selectTarget` 多叫一次会把 `sel-observe-on` 那一声
/// 多扇一遍，而那一声明白着是属于哪一问的由段界决定（见上面那封 P134 的订正）。
///
/// 归位是这一族自己欠的一条：`op-lone-restore` 与 `op-set-back` 把本窗还回它**自己现读到的**那一级
/// （不是写死 100 —— 默认不透明度是配置里的一格，写死就是拿本档的假设去改用户的环境，#261）。
/// 名单里**另一枚**的原来那一级本窗读不到（QML 里没有别的窗的 opacity，Session 也没有按 id 问 opacity
/// 的那只手），所以段末那手 `op-set-back` 把两枚一起钉在本窗的原级上 —— 那一半的得失逐名记在门档，
/// 不拿一枚弱尺冒充（#261/#440）。
///
/// 三十四枚 `solo-*`（§5.11 独显）排在 `des-undo-again` **之后**、`menu` 之前，理由与上面那几封同一枚
/// 而方向相反：`des-*` 那一族的最后一手把名单收回**空**、把场上收回**满**，而独显四臂要的正是这个起点
/// —— 第一臂要的「名单空着点一枚窗的菜单」（`hand_targets` 因此交回单枚）与第三臂要的「名单空着按热键」
/// （core 直接拒绝）都只能从空名单起步，另起一段就得自己再点一遍名，而那一点会把 `sel-observe-on`
/// 那一声多扇一遍（它是证人不是腿，见上面那封 P134 的订正）。第二臂与第四臂要的「别的那些枚」用的就是
/// `des-*` 已经点好的那两枚证人，本族不另数（#281）。四臂各走一遍又把名单与独显都收回进门时的样子，
/// 后面 `menu` 往上那几枚读对话框的行因此一个字节也没被本族动过。
///
/// 四十枚 `xf-*`（§5.9.5–§5.9.8 的六只手）排在 `solo-ghost-kept` **之后**、`menu` 之前，理由与
/// 上面那封同一枚而方向也同一枚：独显那四臂收回来的起点——名单空、场上满、本窗没被压着、自己的
/// 透明度还是进门那一格——正是显示状态那六只手要的起点，另起一段就得自己再把名单清一遍，而清
/// 名单那一声 `sel-observe-off` 是证人不是腿。这一族自己同样收梢：旋转按满一圈回到进门那个角、
/// 四枚开关各按两下回到进门那一格，所以它把窗留在别的样子上的可能性由第一格自己判住
/// （`xf-rot-geo-start` 对 `xf-rot-geo-end`），后面 `menu` 往上那几枚读对话框的行也一个字节
/// 没被本族动过。本族不点别的窗：六只手全按在本窗自己身上，场上有几枚贴图对它没有要求，所以
/// 单窗那一趟也问得动（`des-*` 与 `solo-*` 那种「场上得真有别的两枚」的前提在这里不存在）。
pub(crate) const PIN_SCHEDULE: &[&str] = &[
    "gif-frames",
    "gif-frame",
    "gif-playing",
    "gif-items-closed",
    "gif-items-still-closed",
    "alpha-closed",
    "gif-items-still",
    "alpha-open",
    "alpha-turn",
    "mov-pos-before",
    "sel-before-count",
    "sel-before-self",
    "sel-plain",
    "sel-plain-count",
    "sel-plain-self",
    "mov-epoch-before",
    "mov-rev-before",
    "mov-lone",
    "mov-epoch-lone",
    "mov-rev-lone",
    "op-lone-start",
    "op-lone-epoch-before",
    "op-lone-set",
    "op-lone-opacity",
    "op-lone-restore",
    "op-lone-restored",
    "op-lone-epoch-end",
    "rst-lone-base",
    "rst-lone-turned",
    "rst-lone-epoch-before",
    "rst-lone-rev-before",
    "rst-lone",
    "rst-lone-zoom",
    "rst-lone-rev",
    "rst-lone-epoch-end",
    "mov-ghost",
    "mov-epoch-ghost",
    "op-epoch-ghost-before",
    "op-ghost",
    "op-epoch-ghost",
    "rst-epoch-ghost-before",
    "rst-ghost",
    "rst-epoch-ghost",
    "sel-add-target",
    "sel-observe-on",
    "sel-add",
    "sel-add-count",
    "sel-add-self",
    "mov-batch-target",
    "mov-batch",
    "mov-batch-epoch",
    "op-start",
    "op-epoch-before",
    "op-rev-before",
    "op-set-batch",
    "op-opacity-set",
    "op-epoch-set",
    "op-rev-set",
    "op-step-down",
    "op-opacity-down",
    "op-step-up",
    "op-opacity-up",
    "op-set-back",
    "op-opacity-end",
    "op-epoch-end",
    "rst-batch-target",
    "rst-batch-epoch-before",
    "rst-batch",
    "rst-batch-epoch",
    "sel-observe-off",
    "sel-cleared-count",
    "sel-cleared-self",
    "mov-undo-self",
    "mov-undo-other",
    "mov-epoch-undo",
    "mov-pos-after",
    "des-target-a",
    "des-target-b",
    "des-pile-before",
    "des-reach-self",
    "des-reach-batch",
    "des-menu-shape",
    "des-menu-label",
    "des-undo-absent",
    "des-batch",
    "des-pile-gone",
    "des-trashed",
    "des-undo-shown",
    "des-ghost",
    "des-trashed-kept",
    "des-undo",
    "des-pile-back",
    "des-trashed-after",
    "des-undo-again",
    "solo-dims",
    "solo-pile",
    "solo-reach-self",
    "solo-epoch-before",
    "solo-enter",
    "solo-epoch-enter",
    "solo-kept",
    "solo-hidden-self",
    "solo-live-self",
    "solo-said",
    "solo-menu-shape",
    "solo-menu-label",
    "solo-exit",
    "solo-epoch-exit",
    "solo-kept-after",
    "solo-target-other",
    "solo-op-start",
    "solo-enter-other",
    "solo-hidden",
    "solo-live",
    "solo-opacity",
    "solo-epoch-other",
    "solo-exit-other",
    "solo-op-end",
    "solo-hidden-end",
    "solo-live-end",
    "solo-refuse-before",
    "solo-refuse",
    "solo-refuse-epoch",
    "solo-refuse-kept",
    "solo-refuse-said",
    "solo-ghost",
    "solo-epoch-ghost",
    "solo-ghost-kept",
    "xf-rot-start",
    "xf-rot-geo-start",
    "xf-rot-rev-before",
    "xf-rot-cw",
    "xf-rot-rev-cw",
    "xf-rot-ccw",
    "xf-rot-rev-ccw",
    "xf-rot-half",
    "xf-rot-full",
    "xf-rot-rev-full",
    "xf-rot-geo-end",
    "xf-fh-start",
    "xf-fh-on",
    "xf-fh-rev-on",
    "xf-fh-off",
    "xf-fh-rev-off",
    "xf-fv-start",
    "xf-fv-on",
    "xf-fv-rev-on",
    "xf-fv-off",
    "xf-fv-rev-off",
    "xf-gray-start",
    "xf-gray-on",
    "xf-gray-rev-on",
    "xf-gray-off",
    "xf-gray-rev-off",
    "xf-inv-start",
    "xf-inv-on",
    "xf-inv-rev-on",
    "xf-inv-off",
    "xf-inv-rev-off",
    "xf-label-left",
    "xf-label-right",
    "xf-label-h",
    "xf-label-v",
    "xf-label-gray",
    "xf-label-inv",
    "xf-key-left",
    "xf-key-right",
    "xf-key-gray",
    "menu",
    "filters",
    "filter",
    "before",
    "current",
    "call-accept",
    "call-after",
    "png-url",
    "png",
    "jpg",
    "copy-drawn",
    "never-copy",
    "never-save",
    "pre-save",
    "dlg-folder",
    "dlg-name",
    "dlg-open",
    "gif-items",
    "gif-menu-shape",
    "gif-turns",
    "gif-distinct",
    "dlg-shown",
    "dlg-shown-filter",
    "accept-was",
    "dlg-push",
    "dlg-push-tries",
    "dlg-push-log",
    "dlg-probe",
    "dlg-live-open",
    "dlg-epoch-open",
    "dlg-accepted",
    "accept",
    "dlg-filter",
    "dlg-after",
    "dlg-dismiss",
    "dlg-rejected",
    "dlg-probe-gone",
    "dlg-live-gone",
    "dlg-epoch-gone",
    "f-jpg",
    "close-before",
    "close",
    "close-after",
    "done",
];

/// 两枚 `sel-observe-*` 不是腿，是**别的窗**在自己那圈边亮/灭时说的话：值里带着说话那一枚的 id，
/// 一趟里可以说好几次。P134 起这是必然的而不是失误——`des-*` 那一族要的三种名单形状
/// （{a,b}、{本窗,a}、{a,b}）借不来多选那两手的任何一种，只能自己点，而每点一次就扇一遍。
///
/// 所以这一枚键照旧要在日程里出现（多出来一枚就叫「日程里没有这条腿」），但不进那条单调链：
/// 「这一声有没有在被点名那一声返回之前扇完、且场上只有被点名那一枚开口」由 [`sel_rows`] 按段
/// 判——一枚判据只写一次（#281），而按段那一读比全程数次数更贴它要问的事。
pub(crate) const REENTRANT_WITNESS: [&str; 2] = ["sel-observe-on", "sel-observe-off"];

/// What the observed key order says about its schedule: `None` when every leg that ran is
/// in the schedule and ran in its relative order, otherwise the problems, joined.
///
/// The comparison is *relative*, the way the finding is: two legs changing position inside
/// one phase is the same defect as two phases swapping, and a fixed index per line would
/// call a run that skipped its optional legs out of order. Equal indices are allowed,
/// because a key a door writes twice is the same leg answering twice, not a schedule change.
/// [`REENTRANT_WITNESS`] is the one case where a key is not a leg at all; it still has to
/// be in the schedule, it just cannot be placed at one spot in it.
pub(crate) fn leg_order(keys: &[&str], schedule: &[&str]) -> Option<String> {
    let mut problems: Vec<String> = Vec::new();
    let mut prev: Option<(&str, usize)> = None;
    for key in keys {
        let Some(at) = schedule.iter().position(|s| s == key) else {
            if !problems.iter().any(|p| p.ends_with(key)) {
                problems.push(format!("日程里没有这条腿：{key}"));
            }
            continue;
        };
        if REENTRANT_WITNESS.contains(key) {
            continue;
        }
        if let Some((before, before_at)) = prev {
            if at < before_at {
                problems.push(format!(
                    "{before} 与 {key} 的相对顺序和日程不符（实测 {before} 先、日程 {key} 先）"
                ));
            }
        }
        prev = Some((key, at));
    }
    if problems.is_empty() {
        None
    } else {
        Some(problems.join("；"))
    }
}

/// Every leg key a door's QML writes. The list comes out grouped by emitter form, not in
/// the order the file reads, which is the first reason nothing below claims to be a schedule.
///
/// Two forms emit a key: `gaugeSay("<key>", …)` hands over a value the leg computed, and
/// `readFilter("<key>")` asks the door for its selected filter, which is why that one is a
/// call of its own. Scanning both is what lets a schedule be compared against the source as
/// a *set*.
///
/// Order is deliberately not one of the claims this can make. A leg emitted inside a
/// function is found in the text where the function is defined, and P41's probe put
/// `jpgArmLeg()` back above the click's pre-state read without moving that definition, so
/// the text still said "arm last" while the door ran it first - a source scan that stays
/// green over the exact defect it is supposed to catch. The run reads the keys in the order
/// they arrive, so the run is the order authority and [`leg_order`] is the row that reddens.
#[cfg(test)]
pub(crate) fn qml_leg_keys(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for emitter in ["gaugeSay(\"", "readFilter(\""] {
        let mut from = 0usize;
        while let Some(hit) = src[from..].find(emitter) {
            let start = from + hit + emitter.len();
            let Some(end) = src[start..].find('"') else {
                break;
            };
            let key = &src[start..start + end];
            if !key.is_empty() && !out.iter().any(|k| k == key) {
                out.push(key.to_string());
            }
            from = start + end;
        }
    }
    out
}

/// The plan, the paths, and whatever QML has reported into it so far.
struct Plan {
    dir: PathBuf,
    qml_png: PathBuf,
    qml_jpg: PathBuf,
    rust_png: PathBuf,
    rust_jpg: PathBuf,
    never: PathBuf,
    dialog: PathBuf,
    /// §10 第 43 项 ①'s artifact: a file this run wrote through [`crate::save`] and then
    /// pinned through [`crate::state::PinState::add_from_file`]. Not in [`plan_line`]:
    /// no QML leg reads it, the pin it names is a second window that stays out of the
    /// leg because `PinWindow.qml` gates on `gauge[4]`, and a field that crosses the
    /// boundary needs a field index in the other language to break.
    from_file: PathBuf,
    /// The pin made from [`Self::from_file`], and the rect it asked Qt for.
    fid: PinId,
    frect: Option<PhysRect>,
    /// §5.9.14's artifact: the two-frame clip this run writes through the *same fixture the
    /// selftest uses*, then pins through the same [`crate::state::PinState::add_from_file`]
    /// door. Unlike [`Self::from_file`] this one *is* in [`plan_line`]: the reading of a clip
    /// is taken by the window it belongs to, and a delegate cannot know which pin it is
    /// without the id crossing the boundary.
    from_gif: PathBuf,
    gid: PinId,
    grect: Option<PhysRect>,
    /// Why the run has no clip pin: the owner's own refusal, kept for the same reason as
    /// [`Self::file_said`].
    gif_said: String,
    /// Why the run has no file-pin: the owner's own refusal, kept so the row quotes the
    /// sentence the product said rather than this file's guess about it.
    file_said: String,
    id: PinId,
    bad: PinId,
    /// What the pin asked Qt to show, in device pixels, as the state machine saw it.
    rect: Option<PhysRect>,
    /// §10 第 55 项: the pin this run *closes*, and the rect it held while it lived.
    ///
    /// It is planted between the two graded pins, so it is neither the bottom of the pile
    /// nor the top, and closing it is the only shape of close that can disagree with the
    /// QML slot bookkeeping: `PinSet` is bottom-first, so a close shifts every later pin
    /// down one index, while an `Instantiator` over an `int` model can only retire its
    /// *last* object. Closing the topmost pin moves nothing and therefore proves nothing.
    ///
    /// No sentinel and no "not armed" value: a gauge run always plants it (the plant
    /// fails the run when it cannot) and the plan always carries its id, because id 0 is a
    /// real pin - the first one the set hands out.
    xid: PinId,
    xrect: Option<PhysRect>,
    lines: Vec<String>,
}

impl Plan {
    /// Nothing planted. `id: 0` and an empty dir are the "not a gauge run" state,
    /// which is what keeps every product session untouched by this file.
    const fn none() -> Plan {
        Plan {
            dir: PathBuf::new(),
            qml_png: PathBuf::new(),
            qml_jpg: PathBuf::new(),
            rust_png: PathBuf::new(),
            rust_jpg: PathBuf::new(),
            never: PathBuf::new(),
            dialog: PathBuf::new(),
            from_file: PathBuf::new(),
            fid: 0,
            frect: None,
            file_said: String::new(),
            from_gif: PathBuf::new(),
            gid: 0,
            grect: None,
            gif_said: String::new(),
            id: 0,
            bad: 0,
            rect: None,
            xid: 0,
            xrect: None,
            lines: Vec::new(),
        }
    }
}

static PLAN: Mutex<Plan> = Mutex::new(Plan::none());

/// The same unlocked-panic rule as [`state::with`]: a gauge must not take the app
/// down because one of its own rows panicked on the way in.
fn with<R>(f: impl FnOnce(&mut Plan) -> R) -> R {
    let mut plan = PLAN.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut plan)
}

/// One window's worth of plan as one `|`-separated line, for QML to split.
///
/// A string rather than four invokables or a `QStringList`: cxx-qt's QML face gives
/// this bridge `QString` for sure, and `|` cannot appear in a Windows path, so the
/// encoding cannot disagree with itself. Empty means no plan, which is how
/// `PinWindow.qml` knows to stay out of this.
///
/// The fields are in one order and QML reads them by index, so both are written out
/// here: `目录 | qml.png | qml.jpg | never.png | 贴图 id | 不存在的 id | 延迟 ms |
/// 对话框默认名 | 推开尝试上限 | 要关掉的那枚 id | 会动的那枚贴图 id | 会动那一腿的起步 ms |
/// 静置那一腿的间隔 ms`. The id that has no pin travels with the plan rather than
/// being recomputed in QML, because `NEVER_OFFSET` is this file's business and a second
/// copy of a number in the other language is a pair that drifts without either side
/// looking wrong. The last three fields are the same argument for a string and for a
/// schedule: QML types the name into the dialog and this file reads the file it produces,
/// and QML counts the waits while this file budgets them - all three only agree if the
/// values were handed over rather than written twice.
///
/// Field 10 is the clip's id, and the gate every delegate reads (`win.gauge.length`) is
/// thirteen rather than ten *because* it is a field: a window that cannot tell whether this
/// run planted a clip has no way to stay out of a leg that belongs to one. Field 11 is the
/// clip leg's own start delay, for the same reason field 6 is the subject's. Field 12 is the
/// subject's menu leg's interval, which QML spends twice - once to arm the leg, once for its
/// tail - and it exists so that no two of the four menu readings fall in the same event-loop
/// step: which of two timers expiring together Qt runs first is not a thing this file can read
/// from the source, and [`PIN_SCHEDULE`] claims `gif-items-still` before `gif-items`.
pub fn plan_line() -> String {
    with(|p| {
        if p.dir.as_os_str().is_empty() {
            return String::new();
        }
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            p.dir.display(),
            p.qml_png.display(),
            p.qml_jpg.display(),
            p.never.display(),
            p.id,
            p.bad,
            LEG_DELAY_MS,
            DIALOG_STEM,
            PUSH_TRIES_MAX,
            p.xid,
            p.gid,
            GIF_ARM_MS,
            GIF_STILL_MS
        )
    })
}

/// One line QML read off the door. `key|value`, graded by [`measure`].
///
/// The first one is stamped, because it is the leg's start and the interval from the plan's
/// arming to it is what the run's `起步` reading prints. The `done` line is stamped for the
/// same reason in the other direction: it is the leg's last word, and the interval from it
/// to the read-back is the only reading that says which of the two closers fired - the leg's
/// own 400 ms grace armed here, or `main`'s floor armed just before `app.exec()`.
///
/// The `done` line arms the run's own end, one wait later: the leg has then said
/// everything it has to say, and the remaining wait is only for the last readings to
/// reach the plan. `main`'s floor stays underneath as the backstop that makes a leg which
/// never arrives a finite run - P38's first passing-looking run is the counter-example,
/// where a dialog's own modal loop outlived the floor and the tail survived on luck
/// rather than on arithmetic.
///
/// The two census keys are this file's answer rather than QML's: the leg names the moment to
/// look (§10 第 55 项), and the list of windows belongs to [`shim::top_levels`], which is a
/// Rust-side call - a QML delegate has no door to it, and opening one for a reading that has
/// no business leaving this file would be a second way to ask the same question. So what QML
/// sent is kept as a prefix and the dump [`pin_census`] reads is appended after a `#`, in the
/// same event-loop turn the leg wrote in. Which turn that is is the leg's claim; what is in it
/// is this file's. The prefix is the delegate's own `pinId`: a leg that closes a pin retires a
/// window, and the first run of this leg could not say afterwards *which* window had said the
/// line before it, which is the difference between a defect in the door and one in the leg.
///
/// Each line is printed the moment it arrives, not only in the report at the end. The reason is
/// the run this round had first: a leg that dies midway leaves no `done`, and the floor that is
/// supposed to make such a run finite did not fire at all - P38's counter-example, quoted above,
/// was exactly a native dialog's own loop outliving it. A run that cannot be trusted to reach its
/// own read-back therefore has to speak while it goes, and the transcript lands in the same
/// archive as the rows.
pub fn report(line: &str) {
    let key = line.split_once('|').map(|(k, _)| k).unwrap_or(line);
    let line = match key {
        "close-before" | "close-after" => {
            let tail = line.split_once('|').map(|(_, v)| v).unwrap_or("");
            format!("{key}|{tail}#{}", shim::top_levels())
        }
        _ => line.to_string(),
    };
    let done = key == "done";
    println!("[pin leg] {line}");
    with(|p| {
        if p.lines.is_empty() {
            state::stamp("pin leg");
        }
        p.lines.push(line);
    });
    if done {
        state::stamp("pin done");
        state::quit_after(LEG_DELAY_MS);
    }
}

/// Plant the pin and the plan. Must run before the QML engine: the window's
/// `Component.onCompleted` reads the plan, and a delegate with no plan has nothing
/// to measure.
pub fn plant() -> Result<String, String> {
    state::stamp("pin arm");
    let mut dir = std::env::temp_dir();
    dir.push(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let [qml_png, qml_jpg, rust_png, rust_jpg, never, dialog, from_file, from_gif] =
        NAMES.map(|n| dir.join(n));
    for p in [
        &qml_png, &qml_jpg, &rust_png, &rust_jpg, &never, &dialog, &from_file, &from_gif,
    ] {
        let _ = std::fs::remove_file(p);
    }
    // The filter leg's name, which the door finishes: QML hands the door `<stem>-f` with no
    // extension and the format the menu was showing decides what follows the dot, so this
    // file cannot name the file it will read. Every spelling the stem could have become is
    // cleared, or a file the last run left behind answers for a leg that never ran.
    let swept = format!("{DIALOG_STEM}-f");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_stem().and_then(|s| s.to_str()) == Some(swept.as_str()) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    let id = state::add_demo(DEMO_W, DEMO_H)?;
    let view = state::with(|s| s.view_data(id));
    let rect = PhysRect::new(
        view.pos_x,
        view.pos_y,
        view.size_w.max(1) as u32,
        view.size_h.max(1) as u32,
    );

    // §10 第 55 项: the doomed pin, planted *between* the two the rows above already grade.
    //
    // It goes through `PinState::add` with its own point rather than through
    // [`state::add_demo`], because `add_demo` parks its pin at the middle of the primary
    // screen - which is exactly where the subject already sits, and [`pin_window`] chooses
    // by overlap, so two windows sharing one rect would make every reading below
    // ambiguous. The spot below the subject is clear of both neighbours on this desktop
    // and of the file pin's offset to the right and down; a desktop too small for it says
    // so through the two rects the rows print, the same way `plant_file_pin` lets a clamp
    // speak.
    //
    // Being second of three is the whole point: the pin set is bottom-first, so closing it
    // is the only shape of close that moves the *slot* of a pin that survives - and a slot
    // is what [`qml/main.qml`]'s window bookkeeping used to be keyed by. The topmost pin
    // would move nothing and prove nothing.
    //
    // The subject stays at index 0 through it, which is what its own window's `gaugeLeg`
    // test reads. That was not enough: the first run of this leg had the windows made by an
    // `Instantiator` over `model: app.session.count`, and P55 measured that a change to that
    // `int` rebuilds *every* window, so the subject kept its index and lost its window
    // mid-sentence (`.qoder/scratch/p55_pin_diag4.log`). The rows below are the census that
    // says whether the two survivors are the same two windows they were before the close.
    let xat = PhysPoint::new(rect.x, rect.y + rect.h as i32 + 16);
    let xid = state::with(|s| s.add(state::demo_frame(DEMO_W, DEMO_H), xat))?;
    let xview = state::with(|s| s.view_data(xid));
    let xrect = PhysRect::new(
        xview.pos_x,
        xview.pos_y,
        xview.size_w.max(1) as u32,
        xview.size_h.max(1) as u32,
    );

    // §10 第 43 项 ①: 存完再钉图, planted the way a user's flow reaches it - the
    // product's own write owner, then the product's own file-to-pin door, in that order,
    // with nothing of this file's in between. Both steps run *here* rather than in
    // [`measure`] because the window is the subject: a pin born after `app.exec()`
    // returns never gets one (that is `annotate_check::pin_save_rows`'s limitation), and
    // this mode's whole claim is about a window the OS can be asked about.
    let (fid, frect, file_said) = plant_file_pin(id, &from_file, rect);

    // §5.9.14: 会动的贴图, planted the way a user's flow reaches it - the same fixture bytes
    // the selftest writes, then the same `add_from_file` door. It goes *above* the file pin
    // and to its side (`rect`'s right, `rect`'s own top) so that no two of the four windows
    // share a pixel: [`pin_window`] selects by largest overlap and [`close_rows`] names every
    // rect it expects to find, so an overlap would be one reading answering for two pins.
    let (gid, grect, gif_said) = plant_gif_pin(&from_gif, rect);

    let bad = id + NEVER_OFFSET;
    with(|p| {
        p.dir = dir.clone();
        p.qml_png = qml_png;
        p.qml_jpg = qml_jpg;
        p.rust_png = rust_png;
        p.rust_jpg = rust_jpg;
        p.never = never;
        p.dialog = dialog;
        p.from_file = from_file;
        p.fid = fid;
        p.frect = frect;
        p.file_said = file_said;
        p.from_gif = from_gif;
        p.gid = gid;
        p.grect = grect;
        p.gif_said = gif_said;
        p.id = id;
        p.bad = bad;
        p.rect = Some(rect);
        p.xid = xid;
        p.xrect = Some(xrect);
        p.lines.clear();
    });
    Ok(format!(
        "贴图 {id} 的窗在 {rect}，写进 {}，另有一个不存在的 id {bad}，真点击那一格交给 {} 的对话框自己取名，{} 先由 `save::write_to` 写成再交 `add_from_file` 钉成贴图 {fid}，{} 由同一份两帧夹具写成再交同一扇门钉成会动的贴图 {gid}（窗在 {}），贴图 {xid} 在 {xrect} 是腿最后要关掉的那一枚",
        dir.display(),
        DIALOG_STEM,
        NAMES[6],
        NAMES[7],
        grect.unwrap_or(PhysRect::new(0, 0, 0, 0))
    ))
}

/// 存完再钉图's two steps, run as a user's flow runs them: [`crate::save::write_to`] on
/// the picture the first pin is already showing, then
/// [`crate::state::PinState::add_from_file`] on the name that write returned - never on
/// the name this file asked for, because an extension can be appended on the way
/// (§10 第 43 项 ①).
///
/// Answers the new pin's id, the rect it asked Qt for, and the sentence belonging to
/// whichever step spoke last: the success clause when both ran, the refusal when either
/// did not. `fid: 0` is therefore not a pin - the set's first id is the demo's - and the
/// rows read `frect` rather than trusting it.
///
/// The file-pin goes beside the first one rather than on top of it: [`pin_window`] picks
/// the window whose overlap is *largest*, so two windows sharing a rectangle would make
/// every reading below ambiguous, and a desktop too small for the offset says so through
/// the two rects the rows print instead of through a clamp this file invented.
fn plant_file_pin(
    first: PinId,
    from_file: &Path,
    rect: PhysRect,
) -> (PinId, Option<PhysRect>, String) {
    let export = crate::settings::export();
    // The first pin's rendered frame, so the file and the first window are the same image
    // and anything the rows find is the round trip's doing, not two subjects'.
    let Some(written) = state::with(|s| {
        s.with_rendered(first, |frame| {
            crate::save::write_to(frame, &from_file.to_string_lossy(), &export, "贴图", "")
        })
    }) else {
        return (0, None, "贴图还没有画过，没有可写的像素".to_string());
    };
    let (landed, said) = match written {
        Ok(landed) => landed,
        Err(crate::save::Refusal::BadName(e)) => return (0, None, e),
        Err(crate::save::Refusal::Write(e)) => return (0, None, e),
    };
    let at = PhysPoint::new(rect.x + rect.w as i32 + 16, rect.y + rect.h as i32 + 16);
    match state::with(|s| s.add_from_file(&landed.to_string_lossy(), at)) {
        Ok(id) => {
            let view = state::with(|s| s.view_data(id));
            (
                id,
                Some(PhysRect::new(
                    view.pos_x,
                    view.pos_y,
                    view.size_w.max(1) as u32,
                    view.size_h.max(1) as u32,
                )),
                said,
            )
        }
        // No pin, so no window: the row says this in the product's own words and the two
        // under it read as absent rather than as wrong.
        Err(e) => (0, None, e),
    }
}

/// §5.9.14's clip, planted the way a file the user dropped reaches the screen: the *same
/// fixture* the selftest writes ([`state::write_two_frame_gif`]), then the *same door*
/// ([`crate::state::PinState::add_from_file`]), with nothing of this file's in between. A
/// gauge that built a clip in memory would be grading a pin the product never made.
///
/// Answers the new pin's id, the rect it asked Qt for, and the refusal when there is no
/// pin - and an empty sentence when there is one, because unlike [`plant_file_pin`] there
/// is no write of ours whose `已保存到` line could be quoted: the frames are read back from
/// the state machine by the rows below, not promised here.
///
/// The point is to the subject's right and level with its top edge, which is the one spot
/// on this desktop that is clear of all three neighbours: four windows each owning pixels no
/// other window owns is what lets [`pin_window`] and [`close_rows`] name one pin per rect.
fn plant_gif_pin(from_gif: &Path, rect: PhysRect) -> (PinId, Option<PhysRect>, String) {
    if let Err(e) = state::write_two_frame_gif(from_gif) {
        return (0, None, format!("{} 写不出来：{e}", from_gif.display()));
    }
    let at = PhysPoint::new(rect.x + rect.w as i32 + 16, rect.y);
    match state::with(|s| s.add_from_file(&from_gif.to_string_lossy(), at)) {
        Ok(id) => {
            let view = state::with(|s| s.view_data(id));
            (
                id,
                Some(PhysRect::new(
                    view.pos_x,
                    view.pos_y,
                    view.size_w.max(1) as u32,
                    view.size_h.max(1) as u32,
                )),
                String::new(),
            )
        }
        Err(e) => (0, None, e),
    }
}

/// What [`crate::session`]'s `pushDialogButton` answers: the C++ shim's one-line
/// report of which window it found and what it pressed.
///
/// Only ever called by the gauge leg, and only while that leg has its own dialog up -
/// the shim filters to this process's pid, so a `#32770` belonging to another program
/// is not a target. There is no `push a specific hwnd` form of this call: the run has
/// one dialog open and pressing a button the report does not name would be a click
/// this file cannot account for.
pub fn push_dialog_button() -> String {
    shim::push_dialog_button()
}

/// 一枚 [`crate::state::PinState::dialog_probe`] 行里的 `key=value` 字段。行是空格分隔的，
/// 所以"这一枚键压根没来"读 `None`，与产品自己写的"没有一枚盖得住"（`-`）是两种答案——
/// 把它们并成一格，门就没法说清是哪一种缺席（#238 那一族：空值与被判的缺席不同种）。
pub(crate) fn probe_field(line: Option<&str>, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    line?
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix(&prefix))
        .map(str::to_owned)
}

/// 行里那枚矩形，读成 `(x, y, w, h)`。`None` 有三种：这一格压根没有 `rect=`、产品写的是
/// `-`、或那四枚数解析不出来。**零面积也算 `None`**，与 [`crate::session`] 的 `dialog_rect`
/// 同一读法：门不该判一枚产品自己拒绝看见的窗。
pub(crate) fn probe_rect(line: Option<&str>) -> Option<(i32, i32, u32, u32)> {
    let said = probe_field(line, "rect")?;
    let mut it = said.split(',');
    let x = it.next()?.parse::<i32>().ok()?;
    let y = it.next()?.parse::<i32>().ok()?;
    let w = it.next()?.parse::<u32>().ok()?;
    let h = it.next()?.parse::<u32>().ok()?;
    if it.next().is_some() || w == 0 || h == 0 {
        return None;
    }
    Some((x, y, w, h))
}

/// 一枚逗号名单读成 id 集合：`-`、空串、`None` 都是"一枚也没有"，而不是"有一枚叫 `-`"。
pub(crate) fn probe_ids(said: Option<&str>) -> Vec<String> {
    match said {
        None | Some("") | Some("-") => Vec::new(),
        Some(list) => list
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
    }
}

/// `dlg-live-*` 那两枚读数读成 `(替哪一枚说话, 屏幕上在不在)`。`None` 有三样：这一格压根没有
/// 这句、那句里没有 `=`（键改了名而值没跟上）、`=` 后面不是 `true`/`false`。三样在这里是同一个
/// 问题——**这一格问得出问不出**。
pub(crate) fn live_said(said: Option<&str>) -> Option<(String, bool)> {
    let (id, v) = said?.split_once('=')?;
    if id.is_empty() {
        return None;
    }
    match v {
        "true" => Some((id.to_owned(), true)),
        "false" => Some((id.to_owned(), false)),
        _ => None,
    }
}

/// §5.5.11 步骤 3 与步骤 4 的第一次**按枚**读数：`hidden=` 是桥那一侧的账，`dlg-live-open` 是
/// 说话那一枚贴图在自己窗里读到的 `view.live`，两者必须对上同一枚 id 才算同一件事。
///
/// 三枚各判一步：开着那一下**不在屏上**（3）、关完之后**回到屏上**（4）、而这两者之间
/// `dialog_epoch` 确实动过——最后这一枚把「一块屏正好自己空了」从结论里挑出去（#316）。
/// 两半说的不是同一枚贴图也算 Fail：一趟里换了一枚说话，那条腿量的就不是同一次隐藏。
///
/// 那枚键关着时答**问不了**：出厂 `false` 那一臂是步骤 1 的用户决定，不是这一格的成就，与
/// 「藏与放」那一格同一个方向。
pub(crate) fn dialog_live(
    lines: &[String],
    enabled: Option<&str>,
    hidden: &[String],
) -> (Check, String) {
    let open = live_said(said(lines, "dlg-live-open"));
    let gone = live_said(said(lines, "dlg-live-gone"));
    let epoch_open = said(lines, "dlg-epoch-open").and_then(|s| s.parse::<i32>().ok());
    let epoch_gone = said(lines, "dlg-epoch-gone").and_then(|s| s.parse::<i32>().ok());
    if enabled != Some("1") {
        return (
            Check::Blocked,
            format!("那枚键关着（enabled={enabled:?}）⇒ 这一格答\"问不了\"而不是答通过"),
        );
    }
    let (Some((id, live_before)), Some((id_after, live_back)), Some(eo), Some(eg)) =
        (open.clone(), gone.clone(), epoch_open, epoch_gone)
    else {
        return (
            Check::Blocked,
            format!(
                "四枚读数缺任一就问不出：open={:?} gone={:?} epoch {:?} -> {:?}",
                said(lines, "dlg-live-open"),
                said(lines, "dlg-live-gone"),
                epoch_open,
                epoch_gone
            ),
        );
    };
    let mine = hidden.iter().any(|h| h == &id);
    let same_pin = id == id_after;
    let hid = live_before == !mine;
    let back = live_back;
    let moved = eg > eo;
    let check = if same_pin && hid && back && moved {
        Check::Pass
    } else {
        Check::Fail
    };
    (
        check,
        format!(
            "替 {id} 说话（关后那半是 {id_after}，{}）；账上 hidden={hidden:?} 里有它 {}，\
             于是屏上要求 live={} 而现读 {live_before}；关后要求 live=true 而现读 {live_back}；\
             epoch {eo} -> {eg}（要求真动过，否则这两半只是屏上恰好自己空了）",
            if same_pin {
                "同一枚"
            } else {
                "换了一枚 ⇒ Fail"
            },
            if mine { "在" } else { "不在" },
            if mine { "false" } else { "true" },
        ),
    )
}

/// A push report with its `desc=<n>` field removed, for the text of a judgement row.
///
/// §10 第 39 项 ⑤ / 第 40 项 ⑤. `desc` counts the dialog's descendant windows, and the
/// number is a fact about the shell's moment, not about this product: two gauges of one
/// binary disagreed in the same round (`p40_brokenpin1.log` read `desc=50`,
/// `p40_brokenink1.log` read `desc=48`), and P38 read 49 and 50 inside one round. Nothing
/// grades it - the row's three claims are `pushed=yes`, `class=[#32770]` and `bid=1` - so
/// leaving it in the row's *text* made that text uncomparable across runs, which is the
/// only thing a ladder archive is for. The full line stays reachable: it goes into a
/// `note` row beside this one, because a note is not a judgement and the raw reading is
/// still the honest witness if the count is ever asked about.
pub(crate) fn push_row_text(report: &str) -> String {
    report
        .split(' ')
        .filter(|field| !field.starts_with("desc="))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether one push report describes the click this row is about.
///
/// Spelled out rather than reduced to `starts_with("pushed=yes")`, because `yes` is
/// what the shim says whenever it found *a* button: the row is about whether a real
/// native save dialog's default button was pressed, and that is three separate claims -
/// that the window was a `#32770`, that the control pressed carried `IDOK`, and that
/// the report names the dialog rather than some other window. A `#32770` with no
/// `IDOK` button answers `pushed=no` and fails here rather than passing on one word.
///
/// The message's return value is printed in the report and deliberately not graded
/// here: `BM_CLICK`'s return is undefined by the API, so a row that read it as success
/// would be grading a number that means nothing. Whether the click *worked* is the
/// `pin save via real click` row's business, which reads the file the dialog wrote.
pub(crate) fn real_click(report: &str) -> bool {
    report.starts_with("pushed=yes ")
        && report.contains("class=[#32770]")
        && report.contains("bid=1 ")
}

/// Which of the three questions a press leg actually answered (§10 第 47 项 ④).
///
/// The column has three verdicts and this has four shapes, so two of them share
/// [`Check::Blocked`] - what they may never share again is the sentence. Until P47 a leg
/// that never spoke and a desktop that would not make a `#32770` both arrived as
/// [`real_click`]'s `false`, i.e. as *the product answering wrong*, which is how one refused
/// press printed four FAILs in a row (第 46 项 ⑥'s pass-c cascade) while that same report's
/// own census said `dialogs=0` - the count that says nobody had a window to press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ask {
    /// The leg never spoke. This run did not ask the question.
    Unasked,
    /// Asked, and this process had no pressable save dialog - the desktop's answer.
    Refused,
    /// A press went out, but not at this process's save dialog's `IDOK`: answered wrong.
    Mispressed,
    /// `pushed=yes` on a `#32770` whose default button carried `IDOK`.
    Pressed,
}

/// [`Ask`] read off one report, where `None` means the leg is absent rather than silent.
pub(crate) fn ask(push: Option<&str>) -> Ask {
    match push {
        None => Ask::Unasked,
        Some(r) if r.starts_with("pushed=yes ") => {
            if real_click(r) {
                Ask::Pressed
            } else {
                Ask::Mispressed
            }
        }
        Some(_) => Ask::Refused,
    }
}

/// The verdict a shape is allowed to reach, said once so the two doors' press rows cannot
/// drift into two readings of one census.
pub(crate) fn ask_check(a: Ask) -> Check {
    match a {
        Ask::Pressed => Check::Pass,
        Ask::Mispressed => Check::Fail,
        Ask::Unasked | Ask::Refused => Check::Blocked,
    }
}

/// Which of the three the column is saying, for the row's own text.
pub(crate) fn ask_said(a: Ask) -> &'static str {
    match a {
        Ask::Unasked => "未问：那一腿本轮没有发声",
        Ask::Refused => "问了，桌面不肯答：本进程名下没有可按的 #32770",
        Ask::Mispressed => "按了，但按的不是本进程那扇保存对话框的默认按钮 —— 这一格是产品答错",
        Ask::Pressed => "按到了本进程那扇 #32770 的默认按钮",
    }
}

/// Whether the dialog phase left its own door standing, graded from the three readings its
/// legs already take (第 47 项 ④'s hygiene half).
///
/// The claim is narrow on purpose: `dlg-after` is read *before* the phase's own `reject()`
/// and `dlg-rejected` is that reject's confession, so this says "the sweep was asked and the
/// door answered it", not "the native window is gone". The native witness for that would be a
/// census that counts without pressing, and this run does not have one (结转第 48 项).
pub(crate) fn dialog_hygiene(
    shown: Option<&str>,
    after: Option<&str>,
    dismissed: bool,
    rejected: bool,
) -> (Check, String) {
    match (shown, after) {
        (None, _) => (
            Check::Blocked,
            "未问：这一档本轮没有把对话框开到屏幕上过（dlg-shown 没有读数）".to_string(),
        ),
        (_, None) => (
            Check::Blocked,
            "未问：对话框开了，但关上的那一格没读（dlg-after 没有读数）".to_string(),
        ),
        (_, Some("false")) if !dismissed => (
            Check::Pass,
            "点击自己就把门关了，本段没有窗要撤（dlg-after=false）".to_string(),
        ),
        (_, Some("false")) => (
            Check::Fail,
            "点完之后 visible=false，本段却又 reject() 了一次 —— 撤的是已经不在的那扇，这两句读数自相矛盾".to_string(),
        ),
        (_, Some("true")) if dismissed && rejected => (
            Check::Pass,
            "点击没关成，本段自己 reject() 且门应了（dlg-rejected 有读数）".to_string(),
        ),
        (_, Some("true")) if dismissed => (
            Check::Fail,
            "本段 reject() 了而门没应（没有 dlg-rejected 读数）—— 这一档留下的窗要由下一段代为认领".to_string(),
        ),
        (_, Some("true")) => (
            Check::Fail,
            "点完之后 visible=true 而本段没有 reject() —— 自家的窗留给下一段了".to_string(),
        ),
        // The reading exists and is neither of the two this predicate knows. Saying that is
        // the honest answer; folding it into the arm above would report a window this run
        // never measured as one it did.
        (_, Some(other)) => (
            Check::Fail,
            format!("dlg-after 读回 {other:?}，既不是 \"true\" 也不是 \"false\" —— 本判据不认识这一格读数"),
        ),
    }
}

/// Whether a census taken *after* the dialog phase can blame what it counted on that phase.
///
/// This is the half of 第 46 项 ⑥ a verdict column can carry without a new instrument: a
/// quick door's claim is "I open no dialog", and when its census answers `dialogs=1` the row
/// has to say whose hygiene it is grading. If the dialog phase left a door standing, the
/// quiet door is not the one that opened it.
pub(crate) fn census_blames_earlier_phase(
    census: &str,
    hygiene: Check,
    dialog_phase_ran: bool,
) -> bool {
    dialog_phase_ran && hygiene != Check::Pass && census.contains("dialogs=1")
}

/// The value QML reported under one key, or `None` when that leg never spoke.
///
/// The key is matched as a whole field rather than as a prefix, so a future `png2`
/// cannot be read as `png`. Shared with [`crate::annotate_check`], whose mask leg reports
/// in the same shape and has the same reason not to read a prefix as a key.
pub(crate) fn said<'a>(lines: &'a [String], key: &str) -> Option<&'a str> {
    lines.iter().find_map(|l| {
        let (k, v) = l.split_once('|')?;
        (k == key).then_some(v)
    })
}

/// [`said`] 认键的那一枚尺，单独写一份好让它的两种问法都从这一处走：整字段相等，不读前缀。
fn line_speaks(l: &str, key: &str) -> bool {
    l.split_once('|').is_some_and(|(k, _)| k == key)
}

/// 第一句说出 `key` 的行号（不是值）：`sel_rows` 的证人那一段由两枚这样的行号圈出来。
fn said_at(lines: &[String], key: &str) -> Option<usize> {
    lines.iter().position(|l| line_speaks(l, key))
}

/// `key` 在这几行里说过几句：一段里说过一次、全程说过几句是两个问法，读法必须同一个。
fn spoke_times(lines: &[String], key: &str) -> usize {
    lines.iter().filter(|l| line_speaks(l, key)).count()
}

fn read(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn bytes(v: &Option<Vec<u8>>) -> String {
    v.as_ref()
        .map(|b| format!("{} B", b.len()))
        .unwrap_or_else(|| "没有文件".to_string())
}

/// The same file, allowing for the spelling the shell chose. The name the dialog hands
/// back is its own writing - a drive letter it may case differently from `temp_dir()` -
/// and `C:\` against `c:\` says nothing about which picture got saved.
///
/// Only this one comparison is folded. Every other path row here reads a name the gauge
/// itself produced, where matching it exactly is the point of the row.
fn same_file(a: &Path, b: &Path) -> bool {
    fn key(p: &Path) -> String {
        p.to_string_lossy().replace('\\', "/").to_ascii_lowercase()
    }
    !a.as_os_str().is_empty() && key(a) == key(b)
}

/// The window the state machine asked for, out of the top-levels Qt really made.
///
/// Chosen by overlap rather than by title, because a pin sets no title - and
/// anything that *does* have a mask title is not this mode's subject, so it is
/// filtered out rather than merely unlikely.
fn pin_window(want: PhysRect) -> Option<crate::r13::Top> {
    let mut levels = parse_levels(&shim::top_levels());
    levels.retain(|t| t.visible && !t.title.starts_with(TITLE));
    levels
        .into_iter()
        .map(|t| (t.phys.intersection(&want).map(|i| i.area()).unwrap_or(0), t))
        .max_by_key(|(area, _)| *area)
        .filter(|(area, _)| *area > 0)
        .map(|(_, t)| t)
}

/// Whether a window's device rect is the one the state machine asked for, allowing for
/// the rounding [`RECT_SLACK`] exists for. One predicate rather than two copies, because
/// this run reads two windows the same way and a slack written twice is a pair that drifts.
fn within_slack(got: &PhysRect, want: &PhysRect) -> bool {
    (got.x - want.x).abs() <= RECT_SLACK
        && (got.y - want.y).abs() <= RECT_SLACK
        && (got.w as i32 - want.w as i32).abs() <= RECT_SLACK
        && (got.h as i32 - want.h as i32).abs() <= RECT_SLACK
}

/// Every pin window this process was showing at one moment, read from the dump the leg
/// reported rather than from the one taken after `app.exec()`.
///
/// The same two filters [`pin_window`] applies - visible, and not a mask's title - applied
/// to a whole list instead of reduced to the best overlap. That reduction is what a
/// single-window row needs and what a census must not do: §10 第 55 项's claim is about *how
/// many* windows survived a close and *which* rects they sit on, and one rect cannot be
/// within [`RECT_SLACK`] of two of the wants below (the pile is planted at least 16 px
/// apart), so matching each want independently is the same as matching them distinctly.
///
/// The `#` is [`report`]'s doing and the part before it is the delegate's own `pinId`, so this
/// reads the census out of the value rather than the whole line: the prefix carries who spoke,
/// not what was on the screen.
fn pin_census(raw: Option<&str>) -> Vec<crate::r13::Top> {
    let mut levels = raw
        .map(|s| s.split_once('#').map(|(_, dump)| dump).unwrap_or(s))
        .map(parse_levels)
        .unwrap_or_default();
    levels.retain(|t| t.visible && !t.title.starts_with(TITLE));
    levels
}

/// The window sitting on `want`, if the census has one.
fn window_at<'a>(census: &'a [crate::r13::Top], want: &PhysRect) -> Option<&'a crate::r13::Top> {
    census.iter().find(|t| within_slack(&t.phys, want))
}

/// Which delegate said a census line, in its own words. The prefix is whatever QML sent, so an
/// older shape (no prefix at all) reads as unsigned rather than as a wrong id.
fn close_speaker(raw: Option<&str>) -> &str {
    raw.and_then(|s| s.split_once('#').map(|(sp, _)| sp))
        .filter(|sp| !sp.is_empty())
        .unwrap_or("未署名")
}

/// The five rows the close leg registers (§10 第 55 项 ①), in the order the leg reaches them.
const CLOSE_ROWS: [&str; 5] = [
    "pin pile before close",
    "pin close",
    "pin pile after close",
    "pin closed pin has no window",
    "pin survivors keep their windows",
];

/// 关掉非最上层的一枚, as five rows: what the pile looked like, what the close answered,
/// what the pile looks like now, whether the closed pin's own rect is empty, and whether the
/// pictures still on the screen are the same windows they were.
///
/// The pair that carries the claim is row 3 and row 4: closing a pin in the middle moves
/// the *slot* of every pin after it while leaving those pins on the screen, and a bookkeeping
/// keyed by the slot gets that pair wrong twice over - the window that was showing the
/// topmost pin is retired (row 3 finds no window on its rect) and the window that was
/// showing the closed pin survives, parked where the dead pin was (row 4). Closing the
/// topmost pin instead would move nothing and pass both rows for free, which is why
/// [`plant`] puts the doomed pin in the middle.
///
/// Since §5.9.14 the pile is **four**, not three, and the counts in rows 1 and 3 are the
/// run's own claim about it: the clip's window is a top-level like any other, so a census of
/// four that came back with three after the close would be one pin short of the four this
/// file planted. That is the reason [`close_rows`] takes the clip's rect rather than ignoring
/// the window it belongs to - an unlisted fourth window is exactly what a *count* would have
/// reddened without saying which pin was missing.
///
/// The two rows are also what the shape before them failed on, and not the way this
/// paragraph first predicted: with the windows made by an `Instantiator` over
/// `model: app.session.count`, the run never *reached* row 3 to find anything out. The
/// delegate that said `close-before` was destroyed inside the `closePin` call it had just
/// made - this Qt rebuilds every object of that `Instantiator` when the `int` changes - and
/// the fresh window at index 0, reading `pinId 0` like the leg's own subject, started the
/// whole leg again from `menu` (P55, `.qoder/scratch/p55_pin_diag4.log`). `main.qml` keys
/// its windows by pin id now, which is the difference rows 1 and 3 read as two identical
/// hwnds rather than as two new ones.
///
/// Row 1 is the witness that the pile was as full as this run planted it, so that row 3's
/// count is read as a close rather than as a leg that never planted. Row 2 is the product's
/// own answer, kept out of the others' way: a close that said `0` would otherwise show up
/// only as a pile that did not shrink, which is this file's reading and not the door's.
///
/// `Blocked` for a row whose census never arrived, and the rows below a missing one say so
/// under their own names - the rule 第 42 项 ② set for the file-pin rows, reused here
/// because a QML leg that stops mid-function stops silently.
fn close_rows(
    lines: &[String],
    rect: Option<PhysRect>,
    xrect: Option<PhysRect>,
    frect: Option<PhysRect>,
    grect: Option<PhysRect>,
) -> Vec<(Check, &'static str, String)> {
    let (Some(rect), Some(xrect), Some(frect), Some(grect)) = (rect, xrect, frect, grect) else {
        return CLOSE_ROWS
            .iter()
            .map(|name| {
                (
                    Check::Blocked,
                    *name,
                    "这一格没有问：计划里没有那四枚贴图的位置。".to_string(),
                )
            })
            .collect();
    };
    let before = pin_census(said(lines, "close-before"));
    let mut out: Vec<(Check, &'static str, String)> = Vec::new();

    // Each of the four wants read by name, so a pile that came back short says *which*
    // rect was missing rather than only that the count was.
    let pile = |census: &[crate::r13::Top]| -> Vec<String> {
        [rect, xrect, frect, grect]
            .iter()
            .map(|want| {
                window_at(census, want)
                    .map(|t| format!("{want}=0x{:x}", t.hwnd))
                    .unwrap_or_else(|| format!("{want}=没有窗"))
            })
            .collect()
    };

    out.push(match said(lines, "close-before") {
        None => (
            Check::Blocked,
            CLOSE_ROWS[0],
            "这一格没有问：腿没有报上关窗前的点名。".to_string(),
        ),
        Some(_) => {
            let complete = [rect, xrect, frect, grect]
                .iter()
                .all(|w| window_at(&before, w).is_some());
            (
                Check::from(before.len() == 4 && complete),
                CLOSE_ROWS[0],
                format!(
                    "{} 扇可见顶层，那句由贴图 {} 报上；{}",
                    before.len(),
                    close_speaker(said(lines, "close-before")),
                    pile(&before).join(" ")
                ),
            )
        }
    });

    out.push(match said(lines, "close") {
        Some(said) => (
            Check::from(said == "1"),
            CLOSE_ROWS[1],
            format!("closePin 答 {said:?}（要求 1 枚）"),
        ),
        None => (
            Check::Blocked,
            CLOSE_ROWS[1],
            "这一格没有问：腿没有报上 closePin 的回答。".to_string(),
        ),
    });

    let Some(after_raw) = said(lines, "close-after") else {
        let why = said(lines, "close")
            .map(|s| format!("腿在报完 close({s:?}) 之后就停了"))
            .unwrap_or_else(|| "腿没有报上关窗的那一句".to_string());
        for name in &CLOSE_ROWS[2..] {
            out.push((
                Check::Blocked,
                *name,
                format!("这一格没有问：{why}，关窗后的点名没有读到。"),
            ));
        }
        return out;
    };

    let after = pin_census(Some(after_raw));
    out.push((
        Check::from(
            after.len() == 3
                && [rect, frect, grect]
                    .iter()
                    .all(|w| window_at(&after, w).is_some()),
        ),
        CLOSE_ROWS[2],
        format!(
            "{} 扇可见顶层；还活着的三格 {}",
            after.len(),
            pile(&after).join(" ")
        ),
    ));
    out.push((
        Check::from(window_at(&after, &xrect).is_none()),
        CLOSE_ROWS[3],
        format!(
            "关掉那枚的 {xrect} 上还有窗={}",
            window_at(&after, &xrect).is_some()
        ),
    ));

    // The survivors by *handle*. Rows 3 and 4 ask which rects have a window, and a
    // pile whose windows were all rebuilt underneath the close answers them exactly like a
    // pile that kept them - the right count, the right rects - so the half of 第 55 项 that
    // reaches a user ("the pictures you did not close are still the ones you were looking
    // at, not a redraw of them") is only sayable of the hwnds. P55 measured the rebuild this
    // row is written against: `model: app.session.count` answered the close by destroying
    // every window and making two new ones, and the leg died with the one that was speaking
    // (`.qoder/scratch/p55_pin_diag4.log`). `main.qml` keys its windows by pin id now, which is what this row reads as three
    // unchanged handles rather than as three new ones.
    let hwnd_of = |census: &[crate::r13::Top], want: &PhysRect| {
        window_at(census, want)
            .map(|t| format!("0x{:x}", t.hwnd))
            .unwrap_or_else(|| "没有窗".to_string())
    };
    let kept: Vec<(bool, String)> = [rect, frect, grect]
        .iter()
        .map(|want| {
            let (then, now) = (hwnd_of(&before, want), hwnd_of(&after, want));
            let same = then == now;
            (same, format!("{want} {then} -> {now} 同一扇={same}"))
        })
        .collect();
    out.push((
        Check::from(kept.iter().all(|(same, _)| *same)),
        CLOSE_ROWS[4],
        kept.iter()
            .map(|(_, line)| line.clone())
            .collect::<Vec<_>>()
            .join("；"),
    ));
    out
}

/// Per-channel distance still counted as the same colour, and the share of a window's
/// opaque pixels that has to answer to it for the row to pass. Both numbers are R13's,
/// whose matrix of four capture paths settled the tolerance at one rounding per channel
/// and grades "did this path see the window" as a ratio for the same reason: a capture
/// that came back one pixel off-position still reports most of the colour, and the ratio
/// is what says how off.
const PIXEL_TOL: u8 = 8;
const PIXEL_SHARE: f64 = 0.95;

/// `PrintWindow` 的那一枚 flag，§10 第 46 项 ④ 把第三格换到这条腿上来采集时用的数。
///
/// 不是随手挑的：R13 的四路矩阵在这台机器上对同一扇贴图窗（240x160）三种 flag 各问过一次，
/// 归档在 `.qoder/scratch/p45_r13_*.log` —— `2` 答 `ctrl=0.959`，`0` 与 `4` 都答
/// `ctrl=0.000 flat=1.000`（画出来一张全黑）。于是这一格只问 `2`，问另外两种是把已知会
/// 全黑的答案当成采集结果。
///
/// 而"只问一次"正是换腿的理由：`crates/core/src/capture.rs` 那条产品腿会连着试
/// `2`/`0`/`4` 再退回桌面 `BitBlt`，所以它的一次成功可能是"读到了窗后面那张桌面"；
/// 这台机器上它对贴图窗的实际回答是 `no window`（`p45_pin_*.log` 里第三格常年 Blocked），
/// 于是"窗自己画不画得出来"这句话从来没被这一格问过。
const PRINT_FLAG: u32 = 2;

/// The three rows [`file_pin_rows`] owes, in the order it reaches them. The list is a
/// constant rather than a memory because the reason below is a count: a leg that stops at
/// row 1 and says nothing about rows 2 and 3 leaves the door three lines short, and the
/// reader cannot tell a stopped leg from a different run of the gauge.
const FILE_PIN_ROWS: [&str; 3] = ["pin from file", "pin from file window", "pin window pixels"];

/// Every row *below* index `after` - the one that stopped this leg - as its own graded
/// line: named, `Blocked`, and saying what made it unaskable.
///
/// `Blocked` and not `Fail`, because the claim the row makes was never put: the file was
/// not readable, so the window it would have been pinned from does not exist to be looked
/// for. Item 42 sub-item 2's rule is that a row which asked and a row the desktop refused
/// are two rows; the same rule read the other way is that a row this run never asked is
/// still a row the report carries - and P43's 行数不是护栏 is why the name has to be
/// printed rather than inferred from a length.
fn unasked_rows(after: usize, why: &str) -> Vec<(Check, &'static str, String)> {
    FILE_PIN_ROWS[(after + 1).min(FILE_PIN_ROWS.len())..]
        .iter()
        .map(|name| (Check::Blocked, *name, format!("这一格没有问：{why}")))
        .collect()
}

/// The four rows §5.9.14's clip registers, in the order the flow reaches them: the pin the
/// file became, the window that pin got, the frame the *screen* saw, the menu the clip puts
/// up.
const GIF_PIN_ROWS: [&str; 4] = [
    "pin gif planted",
    "pin gif window",
    "pin gif clock",
    "pin gif menu",
];

/// §5.9.14 graded. The rows the clip leg never asked are printed as `Blocked` under their
/// own names, the rule 第 42 项 ② set for the file-pin rows and 第 55 项 reused here: a
/// delegate that did not arm, a plan that did not carry the id, and a clip that does not
/// animate are three different answers, and only the last of them is the product's.
///
/// Row 3 is the one this round was waiting for. The CLI selftest has moved a frame since
/// P128 by calling `tick_gif` from Rust, which proves the arithmetic and proves nothing
/// about the window: the claim §5.9.14 makes to a user is that the picture *on the screen*
/// changes by itself, and that is only sayable of what the delegate's own
/// `gif_frameChanged` handler counted while the event loop was running. Two readings, not
/// one, and each closes a hole the other leaves open:
///
/// - `gif-turns` counts signal emissions. If cxx-qt's setter emits whether or not the
///   value moved, a count alone would also be produced by a clip whose index never left 0.
/// - `gif-distinct` counts the *different frame indices* the window was showing during the
///   wait. That is the half a signal count cannot fake, and it is deliberately not "the
///   index read at the end": the fixture's cycle is 180 ms against a 400 ms wait, so an
///   end-sample can land back on frame 0 for a clip that ran perfectly (450 ms of virtual
///   time is frame 0 again) - a row that asked for the last index would have reddened on a
///   working product.
///
/// Row 3's other half is the guard against a leg that ran on the wrong window: `frames` is
/// compared against the frame count the *state machine* reports for `gid`, not only against
/// the number the delegate printed. A delegate bound to some other pin would answer its own
/// clip's numbers, and on this run the neighbours are PNGs whose frame count is 0.
fn gif_rows(
    lines: &[String],
    grect: Option<PhysRect>,
    gid: PinId,
    gif_said: &str,
    from_gif: &Path,
) -> Vec<(Check, &'static str, String)> {
    let named = from_gif.display().to_string();
    let mut out: Vec<(Check, &'static str, String)> = Vec::new();

    // Rows 2..4 read the same leg, so a clip that never became a pin leaves all three
    // unasked - and says so one row at a time rather than three rows of the same silence.
    let Some(rect) = grect else {
        out.push((
            Check::Fail,
            GIF_PIN_ROWS[0],
            format!("{named} 没有钉成会动的贴图：{gif_said}"),
        ));
        out.push((
            Check::Blocked,
            GIF_PIN_ROWS[1],
            format!("贴图 {gid} 没有 rect，那扇窗无从问起"),
        ));
        let why = format!("贴图 {gid} 没有钉成，那一腿压根没有窗可报");
        out.extend(
            GIF_PIN_ROWS[2..]
                .iter()
                .map(|name| (Check::Blocked, *name, format!("这一格没有问：{why}"))),
        );
        return out;
    };

    out.push((
        Check::Pass,
        GIF_PIN_ROWS[0],
        format!(
            "{named} 由 {} 帧的两色夹具写成、交 `add_from_file` 钉成贴图 {gid}",
            state::GIF_TEST_FRAMES.len()
        ),
    ));

    match pin_window(rect) {
        None => out.push((
            Check::Fail,
            GIF_PIN_ROWS[1],
            format!("没有一个可见顶层和会动的那枚贴图的 {rect} 重叠"),
        )),
        Some(t) => {
            let close = within_slack(&t.phys, &rect);
            out.push((
                Check::from(close),
                GIF_PIN_ROWS[1],
                format!(
                    "顶层 {}x{}@{},{} 和要求的 {rect} 相差在 {RECT_SLACK} px 内={close}（class={}）",
                    t.phys.w, t.phys.h, t.phys.x, t.phys.y, t.class
                ),
            ));
        }
    }

    let num = |key: &str| said(lines, key).and_then(|v| v.parse::<i64>().ok());
    let want_frames = state::GIF_TEST_FRAMES.len() as i64;
    let rust_frames = state::with(|s| s.view_data(gid).gif_frames) as i64;
    let asked = said(lines, "gif-turns").is_some();
    let detail = format!(
        "qml 报 frames={:?} frame={:?} playing={:?}；本趟信号 {} 次、看到过的帧号 {} 枚；状态机那一边给贴图 {gid} 报 {rust_frames} 帧（要求 {want_frames}）",
        said(lines, "gif-frames"),
        said(lines, "gif-frame"),
        said(lines, "gif-playing"),
        said(lines, "gif-turns").unwrap_or("<没有读数>"),
        said(lines, "gif-distinct").unwrap_or("<没有读数>"),
    );
    out.push(if !asked {
        (
            Check::Blocked,
            GIF_PIN_ROWS[2],
            format!("这一格没有问：会动的那一枚自己那一腿没有报上读数。{detail}"),
        )
    } else {
        let ok = num("gif-frames") == Some(want_frames)
            && said(lines, "gif-playing") == Some("true")
            && num("gif-turns").is_some_and(|n| n >= 2)
            && num("gif-distinct") == Some(want_frames)
            && rust_frames == num("gif-frames").unwrap_or(-1);
        (
            Check::from(ok),
            GIF_PIN_ROWS[2],
            format!(
                "{detail}；判据＝帧数与夹具一致、播放中、一等待里信号至少两枚、看到过全部 {want_frames} 枚帧号、且 qml 报的帧数与状态机给 {gid} 的一致"
            ),
        )
    });

    let (clip_items, still_items) = (num("gif-items"), num("gif-items-still"));
    let text = format!(
        "菜单开着时：会动的那扇窗上 {} 项可见、静止的那扇上 {} 项可见；菜单关着时同一枚计数读回 {} 与 {}；机制证人={}（要求 {want} 与 0；§5.9.14 那一节的条件是 `gif_frames > 1`）",
        clip_items.map(|n| n.to_string()).unwrap_or("<没有读数>".into()),
        still_items.map(|n| n.to_string()).unwrap_or("<没有读数>".into()),
        said(lines, "gif-items-closed").unwrap_or("<没有读数>"),
        said(lines, "gif-items-still-closed").unwrap_or("<没有读数>"),
        said(lines, "gif-menu-shape").unwrap_or("<没有读数>"),
        want = GIF_MENU_ITEMS,
    );
    out.push(if clip_items.is_none() || still_items.is_none() {
        (
            Check::Blocked,
            GIF_PIN_ROWS[3],
            format!("这一格没有问：两扇窗里有一扇没报上自己看见几项。{text}"),
        )
    } else {
        (
            Check::from(clip_items == Some(GIF_MENU_ITEMS as i64) && still_items == Some(0)),
            GIF_PIN_ROWS[3],
            text,
        )
    });
    out
}

/// The two rows §5.9.15 registers, in the order the flow reaches them: the section the
/// menu puts up on a picture with a hole in it, and the writes that change the mode.
const ALPHA_ROWS: [&str; 2] = ["pin alpha section", "pin alpha turn"];

/// One `alphaReport()`, cut into its `k=v` pairs.
type AlphaReading = Vec<(String, String)>;

/// The three writes' own answers, then the three readings taken around them: 棋盘格, 透明, 改回来.
type AlphaTurn = (bool, bool, bool, AlphaReading, AlphaReading, AlphaReading);

/// One `alphaReport()` reading: `k=v;k=v;…`, the seven keys `PinWindow.qml` spells.
///
/// A key the report does not carry answers `None` rather than a plausible default, because
/// "那一腿没说话" and "它说了 0" are two different answers and only the second is the
/// product's (第 42 项 ②).
fn alpha_pairs(report: Option<&str>) -> AlphaReading {
    report
        .unwrap_or_default()
        .split(';')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn alpha_field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn alpha_num(pairs: &[(String, String)], name: &str) -> Option<i64> {
    alpha_field(pairs, name).and_then(|v| v.parse().ok())
}

fn alpha_yes(pairs: &[(String, String)], name: &str) -> Option<bool> {
    alpha_field(pairs, name).map(|v| v == "true")
}

/// `alpha-turn` is `found=…;took=…;board=<report>;cleared=…;transparent=<report>;restored=…
/// ;after=<report>`, so the two nested reports are cut on their own labels rather than by
/// the `;` inside them. The three answers that are not reports are returned with them:
/// splitting once per label is also what makes a report that grew a key named `board=` a
/// broken reading rather than a silent one.
fn alpha_turn(raw: Option<&str>) -> Option<AlphaTurn> {
    let t = raw?;
    let rest = t.strip_prefix("found=")?.split_once(";took=")?.1;
    let (took, rest) = rest.split_once(";board=")?;
    let (board, rest) = rest.split_once(";cleared=")?;
    let (cleared, rest) = rest.split_once(";transparent=")?;
    let (transparent, rest) = rest.split_once(";restored=")?;
    let (restored, after) = rest.split_once(";after=")?;
    Some((
        took == "true",
        cleared == "true",
        restored == "true",
        alpha_pairs(Some(board)),
        alpha_pairs(Some(transparent)),
        alpha_pairs(Some(after)),
    ))
}

/// §5.9.15 graded, from the readings the subject window takes on one open menu: shut,
/// open, and open again around three writes that pick a background mode.
///
/// Row 1 is the menu's half: four rows on a window whose picture has a hole in it, exactly
/// one of them marked, and the count compared against what the *state machine* answers for
/// that id (`has_transparent_pixels`) rather than against this file's memory of the demo
/// picture. `alpha-closed` is printed here rather than judged: whether an unopened `Menu`
/// realizes its items is a fact about this Qt, like the `dlg-open` note, and P128's C 档 had
/// to learn it twice - which is a thing worth seeing change under one Qt and not a promise
/// the product makes.
///
/// Row 2 is the half no other row in this door can make: 显示 and 点击区域 move on the same
/// call. It is graded as absolute answers on modes the leg *set* - the board takes the
/// point in the margin, 透明 refuses it again - and not as the difference between the board
/// and whatever `pin.alpha_bg` this user's config says, because a run that reddened over a
/// preference would be grading a person. The revision moving once per write is 显示那一半:
/// a mode that repaints nothing would answer every click question correctly.
fn alpha_rows(lines: &[String], id: PinId) -> Vec<(Check, &'static str, String)> {
    let name = |pairs: &[(String, String)], key: &str| -> String {
        alpha_field(pairs, key)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<没有读数>".into())
    };

    let Some(raw_open) = said(lines, "alpha-open") else {
        // Neither reading came, so neither question was put: the open and the turned
        // readings are spoken by one function on one turn of one timer, and a run in which
        // it did not fire asks nothing below it (第 42 项 ②). Nothing of the state machine
        // is read on this path, which is also what lets the shape be graded in a test that
        // has no window.
        let why = "静置那一腿的尾没有报上开着的读数，四行那一问与三次写那一问都没被问过";
        return ALPHA_ROWS
            .iter()
            .map(|n| (Check::Blocked, *n, format!("这一格没有问：{why}")))
            .collect();
    };

    let open = alpha_pairs(Some(raw_open));
    let closed = alpha_pairs(said(lines, "alpha-closed"));
    let rust = state::with(|s| s.view_data(id));

    let text = format!(
        "菜单开着：洞={}、{} 项可见、{} 项打勾、模式 {}、纹理第 {} 次、边界点吃点击={}、画面内点={}；菜单关着时同一份读法报 {} 项；状态机给贴图 {id} 答 has_transparent_pixels={}",
        name(&open, "holes"),
        name(&open, "items"),
        name(&open, "marked"),
        name(&open, "mode"),
        name(&open, "rev"),
        name(&open, "hole"),
        name(&open, "body"),
        name(&closed, "items"),
        rust.has_transparent_pixels,
    );
    let mut out: Vec<(Check, &'static str, String)> = Vec::new();
    let ok = alpha_yes(&open, "holes") == Some(true)
        && rust.has_transparent_pixels
        && alpha_num(&open, "items") == Some(ALPHA_MENU_ITEMS as i64)
        && alpha_num(&open, "marked") == Some(1)
        && alpha_num(&open, "mode").is_some_and(|m| (0..ALPHA_MENU_ITEMS as i64).contains(&m))
        && alpha_yes(&open, "body") == Some(true);
    out.push((
        Check::from(ok),
        ALPHA_ROWS[0],
        format!(
            "{text}；判据＝这一枚图真有洞（qml 与状态机同答）、四行都在（`ALPHA_MENU_ITEMS`）、只一行打勾、模式落在四枚之内、画面内那一点吃点击"
        ),
    ));

    let raw = said(lines, "alpha-turn");
    let Some((took, cleared, restored, board, transparent, after)) = alpha_turn(raw) else {
        out.push(if raw.is_none() {
            (
                Check::Blocked,
                ALPHA_ROWS[1],
                format!(
                    "这一格没有问：静置那一腿的尾没有报上改过模式的读数。开着的读法={}",
                    name(&open, "mode")
                ),
            )
        } else {
            (
                Check::Fail,
                ALPHA_ROWS[1],
                format!(
                    "那一腿报了 `alpha-turn`，但不是这个形状：{}",
                    raw.unwrap_or_default()
                ),
            )
        });
        return out;
    };
    let rev = |pairs: &[(String, String)]| alpha_num(pairs, "rev");
    let (r_open, r_board, r_clear, r_after) =
        (rev(&open), rev(&board), rev(&transparent), rev(&after));
    let moved = |to: Option<i64>, from: Option<i64>| to.is_some() && from.is_some() && to > from;
    let detail = format!(
        "三次写各答 took/cleared/restored={took}/{cleared}/{restored}；深色棋盘格：模式 {}、{} 项打勾、边界点={}、纹理第 {} 次；透明：模式 {}、边界点={}、纹理第 {} 次；改回来：模式 {}（和开着的读数 {} 一致={}）、纹理第 {} 次；画面内那一点三次各答 {}、{}、{}、{}",
        name(&board, "mode"),
        name(&board, "marked"),
        name(&board, "hole"),
        name(&board, "rev"),
        name(&transparent, "mode"),
        name(&transparent, "hole"),
        name(&transparent, "rev"),
        name(&after, "mode"),
        name(&open, "mode"),
        alpha_num(&after, "mode") == alpha_num(&open, "mode"),
        name(&after, "rev"),
        name(&board, "body"),
        name(&transparent, "body"),
        name(&after, "body"),
        name(&open, "body"),
    );
    let ok = took
        && cleared
        && restored
        && alpha_num(&board, "mode") == Some(2)
        && alpha_num(&board, "marked") == Some(1)
        && alpha_yes(&board, "hole") == Some(true)
        && alpha_num(&transparent, "mode") == Some(0)
        && alpha_yes(&transparent, "hole") == Some(false)
        && alpha_yes(&board, "body") == Some(true)
        && alpha_yes(&transparent, "body") == Some(true)
        && alpha_yes(&after, "body") == Some(true)
        && alpha_yes(&after, "holes") == Some(true)
        && alpha_num(&after, "mode") == alpha_num(&open, "mode")
        && alpha_num(&after, "mode") == Some(rust.alpha_bg as i64)
        && moved(r_board, r_open)
        && moved(r_clear, r_board)
        && r_after.is_some()
        && r_after >= r_clear;
    out.push((
        Check::from(ok),
        ALPHA_ROWS[1],
        format!(
            "{detail}；判据＝三次写都被接受、棋盘格下边界点吃点击而透明下不吃（§5.9.15 的点击区域）、两次写各推出一张新纹理、最后一枚模式与状态机给 {id} 的一致"
        ),
    ));
    out
}

/// §5.10.1 的三步各占一枚名字：选一枚、扇到另一扇窗、退出多选。
const SEL_ROWS: [&str; 3] = ["pin select one", "pin select fanout", "pin select exit"];

/// `sel-observe-on` / `sel-observe-off` 的那一句：`<贴图 id>=<true|false>`。
///
/// 报的是**哪一扇窗**自己说亮了，所以 id 和布尔是一枚读数而不是两枚：只印布尔的话，档里就分不出
/// 「亮的是被点名的那一扇」和「亮的是随便哪一扇」——而 §5.10.1 第 3 步要的正是前者。
fn sel_observer(raw: Option<&str>) -> Option<(i64, bool)> {
    let (id, seen) = raw?.split_once('=')?;
    Some((id.parse().ok()?, seen == "true"))
}

/// §5.10.1「多选贴图」四步里能被真机问出来的那三问，从量规的十三枚 `sel-*` 行里读。
///
/// 三枚判据问的是三件不同的事，一枚也不并：本窗选得到（第 1、2 步的落点）、**别的窗**跟着亮
/// （第 3 步的「统一」）、两只门背后那只手把名单清空且两边都灭（第 4 步）。合起来写就成了一枚
/// 「多选好像能用」，那一枚读不出只坏掉其中一件的那趟。
///
/// 中间那一枚只由另一扇窗自己说的话构成，本档不替它读别人：`view_data(other).selected` 是 Rust
/// 那份权威值，扇出没跑到那扇窗它照样是 true，所以这一问的证人必须是那扇窗的 `selRing`
/// （见 `PinWindow.qml` 里那枚 handler）。
///
/// 缺席一律读成「这一格没有问」而不是读成 0（#453）：`sel-add-target` 交 -1 时场上只有一枚贴图，
/// 后面三枚键根本没说过话——那一枚读数是环境的答案，不是产品的红。
fn sel_rows(lines: &[String], id: PinId) -> Vec<(Check, &'static str, String)> {
    let num = |k: &str| said(lines, k).and_then(|v| v.parse::<i64>().ok());
    let yes = |k: &str| match said(lines, k) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    // **证人那一段**：从 `sel-add-target` 那句之后起、到 `sel-cleared-count` 那句之前止（那一声明
    // 在 Esc 那扇门背后，`sel-observe-off` 就说在它里面）。P134 起 `des-*` 那一族要的三种名单形状
    // 借不来多选那两手的任何一种，只能自己点名，每点一次别的窗就再说一遍自己亮/灭——全程数次数
    // 量的已经不是 §5.10.1 这一问了（证人 `p125_pin_p134doorA.log`：本窗之外那五句「自己说亮了」里
    // 只有两句属于这一问）。改的是尺读哪几行，不是判据的宽严：这一段里照样要**恰好一句**、
    // 而且必须是**被点名那一枚**说的。
    let from = said_at(lines, "sel-add-target").map_or(0, |i| i + 1);
    let to = said_at(lines, "sel-cleared-count").unwrap_or(lines.len());
    let slice = if to > from {
        &lines[from..to]
    } else {
        &lines[0..0]
    };
    let spoke = |k: &str| spoke_times(slice, k);
    let said_in = |k: &str| said(slice, k);
    let read = |k: &str| -> String {
        said(lines, k)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<没有读数>".into())
    };

    if said(lines, "sel-before-count").is_none() {
        let why = "runGauge 那一腿没报上 `sel-before-count`，§5.10.1 的三步都没被问过";
        return SEL_ROWS
            .iter()
            .map(|n| (Check::Blocked, *n, format!("这一格没有问：{why}")))
            .collect();
    }

    let mut out: Vec<(Check, &'static str, String)> = Vec::new();
    let ok = num("sel-before-count") == Some(0)
        && yes("sel-before-self") == Some(false)
        && num("sel-plain") == Some(1)
        && num("sel-plain-count") == Some(1)
        && yes("sel-plain-self") == Some(true);
    out.push((
        Check::from(ok),
        SEL_ROWS[0],
        format!(
            "点本窗之前：名单 {} 枚、本窗边可见={}；点下去之后：`selectPin` 自报 {} 枚、名单 {} 枚、本窗边可见={}；判据＝空场上单击一枚把名单填到 1 且本窗那圈边亮（§5.10.1 第 2 步的落点，Ctrl 那一路见第二格）",
            read("sel-before-count"),
            read("sel-before-self"),
            read("sel-plain"),
            read("sel-plain-count"),
            read("sel-plain-self"),
        ),
    ));

    let target = num("sel-add-target");
    let on = sel_observer(said_in("sel-observe-on"));
    let off = sel_observer(said_in("sel-observe-off"));
    let spoke_all = |k: &str| spoke_times(lines, k);
    // 第二格要的四枚数全在 `sel-add-target` 之后说；它们不齐就是这一问没被问过，不是产品答错。
    // `asked` 同时是第三格的前置：一枚贴图的场上没有别的窗跟着灭，逼它报 `sel-observe-off`
    // 就是尺读空而不是产品读红。
    let asked = match (target, num("sel-add")) {
        (Some(other), Some(said_count)) => {
            let ok = other >= 0
                && other != id as i64
                && on == Some((other, true))
                && spoke("sel-observe-on") == 1
                && said_count == 2
                && num("sel-add-count") == Some(2)
                && yes("sel-add-self") == Some(true);
            out.push((
                Check::from(ok),
                SEL_ROWS[1],
                format!(
                    "本窗点名了 {}（本窗是 {id}）；这一问那一段里有 {} 扇窗自己说亮了（整档该键说过 {} 句），那一句读回={:?}；`selectPin` 自报 {} 枚、名单 {} 枚、本窗边可见={}；判据＝**另一扇窗**的描边由那一声扇出来（它一次也没被点过），且这一段里只有被点名的那一扇亮、且只说了一次",
                    read("sel-add-target"),
                    spoke("sel-observe-on"),
                    spoke_all("sel-observe-on"),
                    on,
                    read("sel-add"),
                    read("sel-add-count"),
                    read("sel-add-self"),
                ),
            ));
            other >= 0
        }
        _ => {
            out.push((
                Check::Blocked,
                SEL_ROWS[1],
                format!(
                    "这一格没有问：场上除本窗以外没有别的贴图（`sel-add-target`={})",
                    read("sel-add-target")
                ),
            ));
            false
        }
    };

    if num("sel-cleared-count").is_none() {
        out.push((
            Check::Blocked,
            SEL_ROWS[2],
            "这一格没有问：`clearSelectionAction.trigger()` 那一句没报上收尾的名单".to_string(),
        ));
    } else {
        // 只有第二问真被问过（场上有另一扇窗被点名），第三问才要求**别人**也灭。
        let ok = num("sel-cleared-count") == Some(0)
            && yes("sel-cleared-self") == Some(false)
            && yes("sel-plain-self") == Some(true)
            && (!asked
                || (off.map(|(_, seen)| !seen) == Some(true)
                    && spoke("sel-observe-off") == 1
                    && off.map(|(who, _)| who) == target));
        out.push((
            Check::from(ok),
            SEL_ROWS[2],
            format!(
                "退出之后：名单 {} 枚、本窗边可见={}、这一问那一段里有 {} 扇窗自己说灭了（整档该键说过 {} 句），那一句读回={:?}；判据＝两只门（点空白 / Esc）共用的那一只手把名单清空，且**两边**的描边都跟着灭——本窗这一格亮过={}（没亮过就灭不成，那一读数是装饰）",
                read("sel-cleared-count"),
                read("sel-cleared-self"),
                spoke("sel-observe-off"),
                spoke_all("sel-observe-off"),
                off,
                yes("sel-plain-self") == Some(true),
            ),
        ));
    }
    out
}

/// §5.10.2 的五问各占一枚名字：拖名单里唯一的那一枚、拖两枚里的一枚、拖一枚场上没有的、
/// 搬动有没有顺手重画一张纹理，以及这一族的腿跑完之后本窗有没有回到它开头站的那个位置。
const MOV_ROWS: [&str; 5] = [
    "pin drag one",
    "pin drag batch",
    "pin drag ghost",
    "pin drag repaint",
    "pin drag undo",
];

/// §5.10.2「批量移动」里能被真机问出来的那五问，从量规的十五枚 `mov-*` 行里读。
///
/// 五枚判据问的是五件不同的事，一枚也不并：**报几枚**由名单决定（第 2 问的红就是那只手偷偷把
/// 批量降成了单枚）、那一枚号只在真搬动时走（第 3 问的红就是空手也发号，全场为一次什么也没
/// 发生的手势重读一遍像素）、一枚不在集里的贴图交回的是 0 而不是把 0 扮成一次成功（#455）、
/// 以及搬动**不许**重画纹理（第 4 问的红就是位置被算进了 `content_key`，于是每一次鼠标移动
/// 事件都编一张 PNG）。
///
/// 第 5 问是本轮自己给自己立的一条规矩：**量规的腿不许把被量的东西弄走**。上一趟真机没有这一问，
/// 本族的四拖把 `pin window` 从 608,380 拖到 648,400、把 `pin pile *` 两枚拖成「没有窗」，而门
/// 只是把这三枚红的名字印出来、一枚也没有判红（`.qoder\scratch\p132_door_A.txt`）。所以这一问读
/// 的是段首与段末那两枚落点是否**同一串字**：不相等就是这一拖没有还回去，相等也只说明**本窗**还
/// 回去了 —— 另一枚贴图的落点本窗读不到（QML 里没有别的窗的 pos），那一半由 `pin window` /
/// `pin pile *` 三枚行对面，两枚证人各答各的（#281/#271）。
///
/// 为什么这里判的是**枚数与号**而不是**位置**：本窗的 `x` 是 `view.pos_x / win.dpr` 那枚绑定的
/// 结果，它在这扇窗被分配给另一枚贴图时也会动 —— 一枚在别的因由下也会动的读数供不出「这一手
/// 扇到了」那一句（#271）。而「另一扇窗自己说它被搬走了」在本轮没有只在批量移动时开口的证人
/// （位置那一路没有 `selRing` 那种「亮/灭」两态可跟），所以那一问逐名记在门档的 blocked 里，
/// 不拿一枚弱尺冒充（#261/#440：证人不在就不猜，替代尺也得能红）。
///
/// 位移算得对不对（含「撞墙那一枚不停下其余」）由 `state.rs` 的 selftest 四臂与 `pin.rs` 的
/// clamp 单测判：那里能现读 `desk.keep_visible` 算每一枚该落在哪儿，桌面的界没有送到 QML 一侧。
fn mov_rows(lines: &[String], id: PinId) -> Vec<(Check, &'static str, String)> {
    let num = |k: &str| said(lines, k).and_then(|v| v.parse::<i64>().ok());
    let text = |k: &str| said(lines, k).map(|v| v.to_string());
    let read = |k: &str| -> String {
        said(lines, k)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<没有读数>".into())
    };

    let mut out: Vec<(Check, &'static str, String)> = Vec::new();
    // `mov-epoch-before` 是这一组的锚：它是段里第一句，没有它，后面每一枚差值都是在拿一枚
    // 没读过的数当起点（#271 不许向读数要它没供的东西，反过来也不许把没供的读成 0）。
    let Some(e0) = num("mov-epoch-before") else {
        let why = "runGauge 那一腿没报上 `mov-epoch-before`，§5.10.2 的五问都没被问过";
        return MOV_ROWS
            .iter()
            .map(|n| (Check::Blocked, *n, format!("这一格没有问：{why}")))
            .collect();
    };

    let el = num("mov-epoch-lone");
    out.push((
        Check::from(num("mov-lone") == Some(1) && el == Some(e0 + 1)),
        MOV_ROWS[0],
        format!(
            "场上只有本窗一枚在名单里时拖它：`dragPin` 自报 {} 枚、号从 {e0} 走到 {}；判据＝§5.9.1 那一手在名单只有一枚时仍是搬一枚，而真搬动必须发号——不发，本窗之外的那些扇窗就没人叫它们重读自己该画在哪儿（`main.qml` 的 `moveTick` 借的就是这声 notify）",
            read("mov-lone"),
            read("mov-epoch-lone"),
        ),
    ));

    let target = num("mov-batch-target");
    let eb = num("mov-batch-epoch");
    // 第二问要的两枚数全在 `mov-batch-target` 之后说；它们不齐就是这一问没被问过，不是产品答错
    // （#453）。场上只有一枚贴图时 `sel-add-target` 交 -1，本窗也就没有第二枚可以一起点名。
    match target {
        Some(other) if other >= 0 && other != id as i64 => out.push((
            Check::from(num("mov-batch") == Some(2) && eb == Some(e0 + 2)),
            MOV_ROWS[1],
            format!(
                "本窗点名了 {other}（本窗是 {id}），名单两枚时拖**本窗**：`dragPin` 自报 {} 枚、号从 {e0} 走到 {}；判据＝搬几枚由**名单**决定而不是由被按住的那扇窗决定（这一拖交回 1 就是那只手把批量降成了单枚，交回 0 就是它压根没搬），且第二手真搬动也发一次号",
                read("mov-batch"),
                read("mov-batch-epoch"),
            ),
        )),
        _ => out.push((
            Check::Blocked,
            MOV_ROWS[1],
            format!(
                "这一格没有问：场上除本窗以外没有别的贴图可以一起点名（`mov-batch-target`={}）",
                read("mov-batch-target")
            ),
        )),
    }

    let eg = num("mov-epoch-ghost");
    out.push((
        Check::from(num("mov-ghost") == Some(0) && eg == el),
        MOV_ROWS[2],
        format!(
            "拖一枚场上没有的 id（场上最大那枚 id 加一）：`dragPin` 自报 {} 枚、号={} 而上一手之后是 {}；判据＝0 只有一枚贴图能给出（那一枚不在集里，是一扇已关掉的窗剩下的鼠标事件），把它扮成一次悄悄的成功就会让用户以为自己的拖动搬走了并不存在的一扇窗，而**空手不许发号**——发了就是全场为一件没发生的事重读一遍像素",
            read("mov-ghost"),
            read("mov-epoch-ghost"),
            read("mov-epoch-lone"),
        ),
    ));

    // 第四问读的是本窗自己那枚纹理号，与前三问同一趟、夹在 `mov-lone` 两侧。两枚读数缺一只就是
    // 「这一问没被问过」而不是「纹理没重画」（#453）：判据要的是**相等**，一枚都不在时它相等
    // 于两枚 `<没有读数>`，那就是一枚读空的尺（#440）。
    let (rev0, rev1) = (num("mov-rev-before"), num("mov-rev-lone"));
    out.push((
        match (rev0, rev1) {
            (Some(before), Some(after)) => Check::from(before == after),
            _ => Check::Blocked,
        },
        MOV_ROWS[3],
        format!(
            "搬动之前本窗自报纹理号 {}、之后 {}；判据＝一次纯搬动**不许**换走一张纹理——位置不在 `content_key` 里，动了就是把位置算进了内容的指纹，于是拖动一路上每一次鼠标移动都编一张 PNG（八枚贴图一起拖时这就是那一句「一起移动」的代价）。这一格只读**本窗**自己那枚号，别的窗的号没有在这里被问（#271）。{}",
            read("mov-rev-before"),
            read("mov-rev-lone"),
            if rev0.is_some() && rev1.is_some() {
                "两枚都在场，判的是相等".to_string()
            } else {
                "两枚不齐，这一格没有问".to_string()
            },
        ),
    ));

    // 第 5 问要的四枚读数全在 `sel-cleared-self` 之后说；不齐就是这一问没被问过，不是产品答错
    // （#453）。场上只有一枚贴图时反手没有第二枚要抹，那四枚键一句也不说。
    let miss: Vec<&str> = [
        "mov-undo-self",
        "mov-undo-other",
        "mov-epoch-undo",
        "mov-pos-after",
    ]
    .iter()
    .filter(|k| said(lines, k).is_none())
    .copied()
    .collect();
    let asked = match target {
        Some(other) if other >= 0 && other != id as i64 => miss.is_empty(),
        _ => false,
    };
    let (p0, p1) = (text("mov-pos-before"), text("mov-pos-after"));
    // 落点两枚不齐同样是「没有问」而不是「没还回去」：这一问比的是**两串字**相等，只剩一串时
    // 它既不能说还了也不能说没还（#444/#453 缺席读成没问，不读成红，也不读成 0）。
    let gradable = asked && p0.is_some() && p1.is_some();
    out.push((
        if !gradable {
            Check::Blocked
        } else {
            Check::from(
                num("mov-undo-self") == Some(1)
                    && num("mov-undo-other") == Some(1)
                    && num("mov-epoch-undo") == Some(e0 + 4)
                    && p0 == p1,
            )
        },
        MOV_ROWS[4],
        format!(
            "反手两拖各交回 {} / {} 枚、号从 {e0} 走到 {}；本窗在这一段开头自报落点 {}、结尾 {}；判据＝量规的腿跑完要把自己挪走的落点**一枚一枚**还回去，还回去靠的是名单已空时那一拖一次只搬被拖的那一扇（交回 1 而不是 2），而两拖之和必须正好抹平前面那两拖；上一趟真机没有这一问，本族的拖动把 `pin window` 从 608,380 拖到 648,400、把 `pin pile *` 两枚拖成「没有窗」。**另一枚**贴图的落点本窗读不到（QML 里没有别的窗的 pos），那一半的证人是 `pin window` 与 `pin pile *` 三枚行，两枚证人各答各的（#271/#281）。{}",
            read("mov-undo-self"),
            read("mov-undo-other"),
            read("mov-epoch-undo"),
            read("mov-pos-before"),
            read("mov-pos-after"),
            if asked && p0.is_some() && p1.is_some() {
                "反手四枚与落点两枚齐，判的是开头与结尾同一串字".to_string()
            } else if !miss.is_empty() {
                format!("反手读数不齐（缺 {miss:?}），这一格没有问")
            } else if !asked {
                "场上除本窗以外没有别的贴图可以反手，这一格没有问".to_string()
            } else {
                "落点两枚不齐，这一格没有问".to_string()
            },
        ),
    ));
    out
}

/// §5.10.3 的六问各占一枚名字：名单里只有本窗一枚时那一手改**一枚**、名单两枚时那一手改**两枚**、
/// 阶梯的两步各改两枚且反手回到同一级、一枚场上没有的 id 交回 0 且不發号、调透明度有没有顺手
/// 换走一张纹理，以及这一族的腿跑完之后本窗有没有回到它开头站的那一级。
const OP_ROWS: [&str; 6] = [
    "pin opacity one",
    "pin opacity batch",
    "pin opacity step",
    "pin opacity ghost",
    "pin opacity repaint",
    "pin opacity undo",
];

/// §5.10.3「批量透明度」里能被真机问出来的那六问，从量规的二十四枚 `op-*` 行里读。
///
/// 六枚判据问的是六件不同的事，一枚也不并（承 [`mov_rows`] 那一族）：**交回几枚**由名单决定
/// （第 2 问的红就是那只手偷偷把批量降成了单枚）、那一枚号只在真改动时走（第 3 问的红就是空手
/// 也发号）、一枚不在集里的贴图交回的是 0 而不是把 0 扮成一次成功（#455）、调透明度**不许**重画
/// 纹理（`opacity` 不在 `content_key` 里，动了就是每按一次「更透明」编一张 PNG）、以及本窗自己
/// 有没有在**没有 reload** 的前提下重读到那一级——最后这一枚才是 `main.qml` 那一声明
/// （`onOpacityTickChanged: app.redrawPins()`）的证人。
///
/// 为什么这里判的是**枚数、号与本窗自己那一级**而不是**另一枚那一级**：Session 没有按 id 问
/// opacity 的那只手，QML 里也没有别的窗的 opacity，所以「另一扇窗自己说它被调了」在本轮是一问
/// **没有证人**的，逐名记在门档的 blocked；同理，段末那一手把**名单**一起钉回本窗的原级，另一枚
/// 原本那一级本窗读不到、还得回去的那一半也由门档逐名（#261/#440：证人不在就不猜）。
///
/// 两枚起点各迈一步（30% 与 80% 一起「更透明」得 20% 与 70%）那一半由 `pin.rs` 的
/// `a_batch_opacity_step_keeps_each_pin_on_its_own_rung` 与 `state.rs` selftest 那一只手的四臂判：
/// 本窗只能报自己那一级，场上另一枚的起点它摆不开，也读不回。
fn op_rows(lines: &[String], id: PinId) -> Vec<(Check, &'static str, String)> {
    let num = |k: &str| said(lines, k).and_then(|v| v.parse::<i64>().ok());
    let read = |k: &str| -> String {
        said(lines, k)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<没有读数>".into())
    };
    let miss = |keys: &[&'static str]| -> Vec<&'static str> {
        keys.iter()
            .filter(|k| said(lines, k).is_none())
            .copied()
            .collect()
    };

    let mut out: Vec<(Check, &'static str, String)> = Vec::new();
    // `op-lone-epoch-before` 是这一组的锚：它是本族第一句，没有它，后面每一枚差值都是在拿一枚
    // 没读过的数当起点（#271 不许向读数要它没供的东西，反过来也不许把没供的读成 0）。
    let Some(l0) = num("op-lone-epoch-before") else {
        let why = "runGauge 那一腿没报上 `op-lone-epoch-before`，§5.10.3 的六问都没被问过";
        return OP_ROWS
            .iter()
            .map(|n| (Check::Blocked, *n, format!("这一格没有问：{why}")))
            .collect();
    };

    out.push((
        Check::from(
            num("op-lone-set") == Some(1)
                && num("op-lone-restore") == Some(1)
                && num("op-lone-epoch-end") == Some(l0 + 2),
        ),
        OP_ROWS[0],
        format!(
            "名单里只有本窗一枚时调它：`setPinOpacity` 自报 {} 枚、归位那一手自报 {} 枚、号从 {l0} 走到 {}；判据＝§5.9.4 那一手在名单只有一枚时仍是改一枚（Ctrl+滚轮 与菜单那三枚入口在用户只选了一枚时走的就是这一路），而两次真改动各发一次号——不发，本窗之外的那些扇窗就没人叫它们重读自己该多透（`main.qml` 的 `opacityTick` 借的就是这声 notify）",
            read("op-lone-set"),
            read("op-lone-restore"),
            read("op-lone-epoch-end"),
        ),
    ));

    // 批量那几问的读数全在 `other >= 0` 那一格里说；不齐就是这一问没被问过，不是产品答错
    // （#453）。场上只有一枚贴图时 `mov-batch-target` 交 -1，本族那十四枚也就一句也不说。
    let batch = [
        "op-start",
        "op-epoch-before",
        "op-set-batch",
        "op-opacity-set",
        "op-epoch-set",
    ];
    let lost = miss(&batch);
    let e0 = num("op-epoch-before");
    out.push((
        if !lost.is_empty() {
            Check::Blocked
        } else {
            // 本窗原级恰好就是这一腿要钉的那一级时，「重读到」天然成立，那一格量不出扇出（#440
            // 一枚读空的尺不许冒充绿）：交 Blocked 并说明环境没给它拉开的余地。
            if num("op-start") == Some(50) {
                Check::Blocked
            } else {
                Check::from(
                    num("op-set-batch") == Some(2)
                        && num("op-epoch-set") == e0.map(|v| v + 1)
                        && num("op-opacity-set") != num("op-start"),
                )
            }
        },
        OP_ROWS[1],
        format!(
            "本窗点名了 {}（本窗是 {id}），名单两枚时调**本窗**：`setPinOpacity` 自报 {} 枚、号从 {} 走到 {}；本窗在这一手之前自报 {}、之后 {}（它自己没有 reload 过）；判据＝改几枚由**名单**决定而不是由按住滚轮的那扇窗决定（这一手交回 1 就是那只手把批量降成了单枚），且本窗重读到的那一级**不是**它原来那一级才说明那一声扇到了、`redrawPins()` 替它问回来了。{}",
            read("mov-batch-target"),
            read("op-set-batch"),
            read("op-epoch-before"),
            read("op-epoch-set"),
            read("op-start"),
            read("op-opacity-set"),
            if !lost.is_empty() {
                format!("批量那几枚不齐（缺 {lost:?}），这一格没有问")
            } else if num("op-start") == Some(50) {
                "本窗原本就在这一腿要钉的那一级上，重读那一半天然相等 ⇒ 这一格没有问".to_string()
            } else {
                "五枚齐，判的是枚数、号与重读到".to_string()
            },
        ),
    ));

    let step_keys = [
        "op-opacity-set",
        "op-step-down",
        "op-opacity-down",
        "op-step-up",
        "op-opacity-up",
        "op-epoch-end",
    ];
    let lost_step = miss(&step_keys);
    let e1 = num("op-epoch-before");
    out.push((
        if !lost_step.is_empty() || e1.is_none() {
            Check::Blocked
        } else {
            Check::from(
                num("op-step-down") == Some(2)
                    && num("op-step-up") == Some(2)
                    && num("op-opacity-down") != num("op-opacity-set")
                    && num("op-opacity-up") == num("op-opacity-set")
                    && num("op-epoch-end") == e1.map(|v| v + 4),
            )
        },
        OP_ROWS[2],
        format!(
            "名单两枚时阶梯两步各自报 {} / {} 枚、号从 {} 走到 {}；本窗自报钉好的那一级 {}、下一步 {}、反手 {}；判据＝§5.10.3 那一句「所有选中贴图同步更新透明度」落在阶梯上是**每一枚各自迈一级**（本窗这一枚下一步必须换一个读数、反手必须回到同一级；两枚里起点不同各自迈步那一半本窗报不出，由 `pin.rs` 的单测与 selftest 四臂判），而四手各发一次号。{}",
            read("op-step-down"),
            read("op-step-up"),
            read("op-epoch-before"),
            read("op-epoch-end"),
            read("op-opacity-set"),
            read("op-opacity-down"),
            read("op-opacity-up"),
            if !lost_step.is_empty() {
                format!("阶梯那几枚不齐（缺 {lost_step:?}），这一格没有问")
            } else {
                "六枚齐，判的是两步各两枚、反手同级与四手四次号".to_string()
            },
        ),
    ));

    let lost_ghost = miss(&["op-epoch-ghost-before", "op-ghost", "op-epoch-ghost"]);
    let eg = num("op-epoch-ghost-before");
    out.push((
        if !lost_ghost.is_empty() || eg.is_none() {
            Check::Blocked
        } else {
            Check::from(num("op-ghost") == Some(0) && num("op-epoch-ghost") == eg)
        },
        OP_ROWS[3],
        format!(
            "调一枚场上没有的 id（场上最大那枚 id 加一）：`stepPinOpacity` 自报 {} 枚、号={} 而这一手之前是 {}；判据＝0 只有一枚贴图能给出（那一枚不在集里，是一扇已关掉的窗剩下的那一次滚轮），把它扮成一次悄悄的成功就会让用户以为自己的滚轮调亮了并不存在的一扇窗，而**空手不许发号**——发了就是全场为一件没发生的事重读一遍。{}",
            read("op-ghost"),
            read("op-epoch-ghost"),
            read("op-epoch-ghost-before"),
            if lost_ghost.is_empty() {
                "三枚齐，判的是交回 0 且号不动".to_string()
            } else {
                format!("ghost 那三枚不齐（缺 {lost_ghost:?}），这一格没有问")
            },
        ),
    ));

    let lost_rev = miss(&["op-rev-before", "op-rev-set"]);
    let (rv0, rv1) = (num("op-rev-before"), num("op-rev-set"));
    out.push((
        match (rv0, rv1) {
            (Some(before), Some(after)) => Check::from(before == after),
            _ => Check::Blocked,
        },
        OP_ROWS[4],
        format!(
            "调透明度之前本窗自报纹理号 {}、之后 {}；判据＝一次纯透明度调整**不许**换走一张纹理——`opacity` 不在 `content_key` 里，动了就是把它算了进去，于是「更透明」按八次就编八张 PNG（八枚贴图一起调时这就是那一句「同步更新」的代价）。这一格只读**本窗**自己那枚号，别的窗的号没有在这里被问（#271）。{}",
            read("op-rev-before"),
            read("op-rev-set"),
            if rv0.is_some() && rv1.is_some() {
                "两枚都在场，判的是相等".to_string()
            } else {
                format!("纹理号那两枚不齐（缺 {lost_rev:?}），这一格没有问")
            },
        ),
    ));

    let lost_back = miss(&["op-lone-start", "op-lone-restored", "op-opacity-end"]);
    out.push((
        if !lost_back.is_empty() || !lost.is_empty() {
            Check::Blocked
        } else {
            Check::from(
                num("op-lone-restored") == num("op-lone-start")
                    && num("op-opacity-end") == num("op-start")
                    && num("op-set-back") == Some(2),
            )
        },
        OP_ROWS[5],
        format!(
            "本族那几手跑完之后：单枚那一格自报 {} 而开头是 {}，批量那一格自报 {} 而开头是 {}、归位那一手交回 {} 枚；判据＝量规的腿不许把被量的东西弄走——本窗必须回到它开头那一级，靠的是归位读的是**现读到**的那一级而不是写死 100（默认不透明度是配置里的一格，写死就是拿本档的假设去改用户的环境，#261）。**名单里另一枚**原本那一级本窗读不到（Session 没有按 id 问 opacity 的那只手），段末那一手把它一起钉在本窗的原级上，那一半的得失由门档逐名（#271/#281）。{}",
            read("op-lone-restored"),
            read("op-lone-start"),
            read("op-opacity-end"),
            read("op-start"),
            read("op-set-back"),
            if !lost_back.is_empty() {
                format!("归位那几枚不齐（缺 {lost_back:?}），这一格没有问")
            } else if !lost.is_empty() {
                "场上除本窗以外没有别的贴图可以一起点名，这一格没有问".to_string()
            } else {
                "五枚齐，判的是两格各回到自己开头那一级".to_string()
            },
        ),
    ));
    out
}

/// §5.10.4「重置」的四问各占一枚名字：名单里只有本窗一枚时那一手改**一枚**并把手势改过的
/// 那一档还回去、名单两枚时那一手改**两枚**且一次批量只发一声、一枚场上没有的 id 交回 0 且
/// 不发号，以及重置有没有顺手换走一张纹理。
const RST_ROWS: [&str; 4] = [
    "pin reset one",
    "pin reset batch",
    "pin reset ghost",
    "pin reset repaint",
];

/// §5.10.4「重置」里能被真机问出来的那四问，从量规的十五枚 `rst-*` 行里读。
///
/// 这一族与 [`op_rows`] 同一形状，因为它是同一只手族（§5.10.2 搬动 / §5.10.3 透明度 / §5.10.4
/// 重置）里最后一只有读者的手：判据 `state::batch_hand` 全仓只写那一次，所以这里判的是**那一次
/// 判据在第三只手上还成不成立**，而不是再数一遍名单。
///
/// 几何那一只证人为什么是 `zoom` 而不是 `rotation`：`rotation` 自 P145 起确实送到了 QML 那一侧
/// （[`crate::pin_view`] 的 `rotation` 格），但它**进** `content_key`，而 `zoom` 与 `pos` 都不进
/// —— 于是「重置把视图还回默认」与「重置不许重编像素」这两问才各自量得动，不互相冒充（#281）。
///
/// 第一问读的是 `rst-lone-zoom == rst-lone-base`，成立的前提是**这一格之前场上没有任何一只手
/// 改过缩放**：本档在 QML 里逐名核过，`runGauge` 这一段只有 `rst-lone-turned` 那一行叫
/// `zoomStep`，菜单里那三项本腿不点。这个前提写在这里而不是埋在代码里，是因为将来谁在这一格
/// 上面添一枚缩放腿，这一枚就会红成一句真话（默认档 ≠ 进门那一级），而红的是尺的前提，不是产品。
fn rst_rows(lines: &[String]) -> Vec<(Check, &'static str, String)> {
    let num = |k: &str| said(lines, k).and_then(|v| v.parse::<i64>().ok());
    let read = |k: &str| -> String {
        said(lines, k)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<没有读数>".into())
    };
    let miss = |keys: &[&'static str]| -> Vec<&'static str> {
        keys.iter()
            .filter(|k| said(lines, k).is_none())
            .copied()
            .collect()
    };

    let mut out: Vec<(Check, &'static str, String)> = Vec::new();

    let lost_lone = miss(&[
        "rst-lone-base",
        "rst-lone-turned",
        "rst-lone-epoch-before",
        "rst-lone",
        "rst-lone-zoom",
        "rst-lone-epoch-end",
    ]);
    // 迈不动就是环境没给这一格拉开的余地（缩放已经贴在下限），不是产品答错：拿一个没动过的
    // 缩放判「还回来了」，那一枚尺读的是空气（#433/#440）。
    let turned = num("rst-lone-turned") != num("rst-lone-base");
    out.push((
        if !lost_lone.is_empty() || !turned {
            Check::Blocked
        } else {
            Check::from(
                num("rst-lone") == Some(1)
                    && num("rst-lone-zoom") == num("rst-lone-base")
                    && num("rst-lone-epoch-end") == num("rst-lone-epoch-before").map(|v| v + 1),
            )
        },
        RST_ROWS[0],
        format!(
            "名单里只有本窗一枚时：本窗进门那一级 {}、往下迈一档之后 {}、那一手自报 {} 枚、还回来是 {}、号从 {} 走到 {}；判据＝§5.10.4 那一枚「重置视图」在用户只选了一枚时改的就是那一枚（交 2 就是它偷偷把单枚升成了一批），迈得动的那一档必须被还回进门那一级，而一次真改动发一次号——不发，`main.qml` 里 `resetTick` 那一声明就没有读者，别的窗不会重读自己该被转成什么样子（#316：绑的是**值**，号是**读者**在数的那件事）。{}",
            read("rst-lone-base"),
            read("rst-lone-turned"),
            read("rst-lone"),
            read("rst-lone-zoom"),
            read("rst-lone-epoch-before"),
            read("rst-lone-epoch-end"),
            if !lost_lone.is_empty() {
                format!("单枚那六枚不齐（缺 {lost_lone:?}），这一格没有问")
            } else if !turned {
                "本窗进门时缩放就贴着阶梯的端，这一档迈不动，这一格没有问".to_string()
            } else {
                "六枚齐，判的是枚数、归位与那一号各一步".to_string()
            },
        ),
    ));

    let lost_batch = miss(&[
        "rst-batch-target",
        "rst-batch-epoch-before",
        "rst-batch",
        "rst-batch-epoch",
    ]);
    out.push((
        if !lost_batch.is_empty() {
            Check::Blocked
        } else {
            Check::from(
                num("rst-batch") == Some(2) && num("rst-batch-epoch") == num("rst-batch-epoch-before").map(|v| v + 1),
            )
        },
        RST_ROWS[1],
        format!(
            "本窗点名了 {}（本窗之外的第一枚），名单两枚时重置**本窗**：`resetPin` 自报 {} 枚、号从 {} 走到 {}；判据＝带走几枚由**名单**决定而不是由被点的那扇窗决定（这一手交回 1 就是那只手偷偷把批量降成了单枚，而 P134 之前菜单那一枚叫的是窗自己那一格手势，交回的永远是 1），且两枚各 sync 一遍而号只走一步——一手一声，不是枚枚一声（与 `op-epoch-set` 同一形状）。{}",
            read("rst-batch-target"),
            read("rst-batch"),
            read("rst-batch-epoch-before"),
            read("rst-batch-epoch"),
            if lost_batch.is_empty() {
                "四枚齐，判的是枚数二与号一步".to_string()
            } else {
                format!("场上除本窗以外没有别的贴图可以一起点名（缺 {lost_batch:?}），这一格没有问")
            },
        ),
    ));

    let lost_ghost = miss(&["rst-epoch-ghost-before", "rst-ghost", "rst-epoch-ghost"]);
    out.push((
        if !lost_ghost.is_empty() {
            Check::Blocked
        } else {
            Check::from(num("rst-ghost") == Some(0) && num("rst-epoch-ghost") == num("rst-epoch-ghost-before"))
        },
        RST_ROWS[2],
        format!(
            "重置一枚场上没有的 id（场上最大那枚 id 加一）：`resetPin` 自报 {} 枚、号={} 而这一手之前是 {}；判据＝0 只有一枚贴图能给出（那一枚不在集里，是一扇已经关掉的窗剩下的那一次菜单点击），把它扮成一次悄悄的成功就会让用户以为自己的「重置」转亮了并不存在的一扇窗，而**空手不许发号**——发了就是全场为一件没发生的事重读一遍（承 [`op_rows`] 的第四问同一形状，换一只手再问一遍不是重复：判据在 `state::reset_pin` 那一格是新写的）。{}",
            read("rst-ghost"),
            read("rst-epoch-ghost"),
            read("rst-epoch-ghost-before"),
            if lost_ghost.is_empty() {
                "三枚齐，判的是交回 0 且号不动".to_string()
            } else {
                format!("ghost 那三枚不齐（缺 {lost_ghost:?}），这一格没有问")
            },
        ),
    ));

    let lost_rev = miss(&["rst-lone-rev-before", "rst-lone-rev"]);
    let (rv0, rv1) = (num("rst-lone-rev-before"), num("rst-lone-rev"));
    out.push((
        match (rv0, rv1) {
            (Some(before), Some(after)) => Check::from(before == after),
            _ => Check::Blocked,
        },
        RST_ROWS[3],
        format!(
            "重置之前本窗自报纹理号 {}、之后 {}；判据＝一次纯视图重置**不许**换走一张纹理——`zoom` 与 `pos` 都不在 `content_key` 里，把它们算进去就是每按一次「重置视图」重编一张 PNG（八枚贴图一起重置时这就是那一句「批量」的代价）。两枚读数之间隔着的那一档缩放是**故意**的：`rst-lone-turned` 那一手先把缩放挪开，这一格才判得动（读在挪开之后、重置之前）。这一格只读**本窗**自己那枚号，别的窗的号没有在这里被问（#271）。{}",
            read("rst-lone-rev-before"),
            read("rst-lone-rev"),
            if rv0.is_some() && rv1.is_some() {
                "两枚都在场，判的是相等".to_string()
            } else {
                format!("纹理号那两枚不齐（缺 {lost_rev:?}），这一格没有问")
            },
        ),
    ));
    out
}

/// §5.9.18 / §5.10.4「销毁」那一族的五问各占一枚名字：影响数量那张表的两臂、菜单字面上印没印、
/// 按下去真带走几枚、一枚场上没有的点击不许清掉撤销的账，以及回头路。
const DES_ROWS: [&str; 5] = [
    "pin destroy reach",
    "pin destroy label",
    "pin destroy take",
    "pin destroy ghost",
    "pin destroy undo",
];

/// §5.9.18 的「销毁 / 撤销 / 影响数量」三格与 §5.10.4 那两只手，从量规的十八枚 `des-*` 行里读。
///
/// 这五问一枚也不并，因为 PRD 第 1148/1149 行的两句话（「应显示确认或支持撤销」「批量销毁时
/// 应提示影响数量」）与「按下去带走几枚」是三件不同的事：第一格里那一句撤销由菜单那一行**出现
/// 与否**答（`des-undo-shown` / `des-undo-absent`），第二格由**字面上那个数**答
/// （`des-menu-label`），第三格由那只手自己的答复答（`des-batch`）。三处读的都是同一格
/// `state::destroy_answer` → 同一张 `hand_targets`，所以它们不可能各说一套（#281：两处各数一遍
/// 就是那一族，而那一处的错用户看得见——印着的数与真被带走的那些不一致）。
///
/// 为什么 `des-menu-label` 那一问判得动而不是一枚天然的绿：菜单那一格 `destroyReach` 的进门值是
/// 1，而读回那一行时名单是 {本窗, 另一枚} 两枚 —— 字面要印 2 只有 `onAboutToShow` 真跑过那一行
/// 才做得到。写在这一格的最初值就是 #433 那一族（一枚读不出差别的尺不许冒充绿）。
///
/// 本窗不能被带走，也不能是名单里唯一的那一枚：说这十八行的正是它（第 55 项那一封同一枚理由）。
/// 所以这一腿点的是**别的**两枚，而 `victim` 的取法里排除的那一枚 `gauge[10]` 是量规自己的排期
/// 约束（那扇会动的贴图长着一段腿），不是产品的规则——逐名记在门档。
fn des_rows(lines: &[String], id: PinId) -> Vec<(Check, &'static str, String)> {
    let num = |k: &str| said(lines, k).and_then(|v| v.parse::<i64>().ok());
    let read = |k: &str| -> String {
        said(lines, k)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<没有读数>".into())
    };
    let miss = |keys: &[&'static str]| -> Vec<&'static str> {
        keys.iter()
            .filter(|k| said(lines, k).is_none())
            .copied()
            .collect()
    };

    let mut out: Vec<(Check, &'static str, String)> = Vec::new();

    let lost_reach = miss(&[
        "des-target-a",
        "des-target-b",
        "des-pile-before",
        "des-reach-self",
    ]);
    // 证人必须是**别的那些枚**：点中本窗自己就是让说话的那一扇窗把自己带走，那一趟之后没有腿了。
    let victims_ok = match (num("des-target-a"), num("des-target-b")) {
        (Some(a), Some(b)) => a != b && a != id as i64 && b != id as i64,
        _ => false,
    };
    out.push((
        if !lost_reach.is_empty() || !victims_ok {
            Check::Blocked
        } else {
            Check::from(num("des-reach-self") == Some(1))
        },
        DES_ROWS[0],
        format!(
            "场上 {} 枚，点中的是 {} 与 {}（本窗是 {id}），名单两枚而本窗不在名单里时「销毁本窗这一枚」答 {}：判据＝影响数量按 id 问而不是全局报一个数——同一张表在两臂上给两个答（另一臂那一个读在 `pin destroy label` 那一格），而窗自己一个也不数（那是 `batch_hand` 的第二份）。{}",
            read("des-pile-before"),
            read("des-target-a"),
            read("des-target-b"),
            read("des-reach-self"),
            if !lost_reach.is_empty() {
                format!("场上凑不出两枚非本窗的证人（缺 {lost_reach:?}），这一格没有问")
            } else if !victims_ok {
                "两枚证人不齐（重复或点中了本窗自己），这一格没有问".to_string()
            } else {
                "四枚齐，判的是本窗那一臂答 1".to_string()
            },
        ),
    ));

    let lost_label = miss(&["des-reach-batch", "des-menu-label", "des-menu-shape"]);
    let want = num("des-reach-batch").map(|n| format!("销毁（影响 {n} 枚）"));
    out.push((
        if !lost_label.is_empty() || want.is_none() {
            Check::Blocked
        } else {
            Check::from(said(lines, "des-menu-label") == want.as_deref())
        },
        DES_ROWS[1],
        format!(
            "名单换成 {{本窗, {}}}(两枚)之后开菜单：桥侧那一格答 {}，菜单那一行字面读回的是「{}」，而这一格进门时缓存里存的是 1；判据＝PRD 第 1149 行要的那个数**印在用户点下去之前看得见的那一行上**，且它与那只手的答复同源——字面从 1 走到 2 只有 `onAboutToShow` 真叫过一次 `destroyCountFor` 才做得到（一枚读不出差别的初值量不到扇出，#433）。机制那三枚是 {}。{}",
            read("des-target-a"),
            read("des-reach-batch"),
            read("des-menu-label"),
            read("des-menu-shape"),
            if lost_label.is_empty() {
                "三枚齐，判的是字面与那张表给同一个数".to_string()
            } else {
                format!("字面那三枚不齐（缺 {lost_label:?}），这一格没有问")
            },
        ),
    ));

    let lost_take = miss(&[
        "des-pile-before",
        "des-batch",
        "des-pile-gone",
        "des-trashed",
        "des-undo-shown",
    ]);
    let gone_n = num("des-reach-batch");
    out.push((
        if !lost_take.is_empty() || gone_n.is_none() {
            Check::Blocked
        } else {
            Check::from(
                num("des-batch") == gone_n
                    && num("des-pile-gone") == num("des-pile-before").zip(gone_n).map(|(v, n)| v - n)
                    && num("des-trashed") == gone_n
                    && said(lines, "des-undo-shown") == Some("true"),
            )
        },
        DES_ROWS[2],
        format!(
            "名单换回 {{ {}, {} }}（本窗不在里面）再按「销毁」那一级：那只手自报带走 {} 枚、场上从 {} 枚变成 {} 枚、账上欠 {} 枚、菜单里「撤销刚才销毁的」那一行出现={}；判据＝带走几枚由名单决定（交 1 就是那只手把批量降成单枚），堆少的那些枚必须**正好**是那几只手答应的枚数（`count` 与 `trashed` 是两格账，各错一半就看不出来），而 PRD 第 1148 行那句「显示确认或支持撤销」在这里走的是撤销那一行：它只在账上欠着东西时出现，一枚也不欠时那一枚死行不出现（#433）。**这些枚的窗真被退役了吗**——本窗读不到别的窗，那一半的证人是 `pin pile before close` / `pin pile after close` 两枚行与 `state.rs` selftest（#271/#453）。",
            read("des-target-a"),
            read("des-target-b"),
            read("des-batch"),
            read("des-pile-before"),
            read("des-pile-gone"),
            read("des-trashed"),
            read("des-undo-shown"),
        ),
    ));

    let lost_ghost = miss(&["des-ghost", "des-trashed", "des-trashed-kept"]);
    out.push((
        if !lost_ghost.is_empty() {
            Check::Blocked
        } else {
            Check::from(num("des-ghost") == Some(0) && num("des-trashed-kept") == num("des-trashed"))
        },
        DES_ROWS[3],
        format!(
            "账上欠 {} 枚时销毁一枚场上没有的 id（计划的第五格）：那只手自报 {} 枚，账上还剩 {} 枚；判据＝0 只有一枚贴图能给出（不在集里），而**一次落空的点击不许把撤销的账清掉**——清了就是对用户说「你刚才那批没了」，那一手是点击不存在的一枚贴图点出来的，不是用户撤销了自己的回头路（#455 那一族：0 不许扮成一次悄悄的成功，也不许顺手扮成一次失败）。{}",
            read("des-trashed"),
            read("des-ghost"),
            read("des-trashed-kept"),
            if lost_ghost.is_empty() {
                "三枚齐，判的是交回 0 且账不动".to_string()
            } else {
                format!("ghost 那三枚不齐（缺 {lost_ghost:?}），这一格没有问")
            },
        ),
    ));

    let lost_undo = miss(&[
        "des-pile-before",
        "des-undo-absent",
        "des-undo",
        "des-pile-back",
        "des-trashed-after",
        "des-undo-again",
    ]);
    out.push((
        if !lost_undo.is_empty() || gone_n.is_none() {
            Check::Blocked
        } else {
            Check::from(
                said(lines, "des-undo-absent") == Some("false")
                    && num("des-undo") == gone_n
                    && num("des-pile-back") == num("des-pile-before")
                    && num("des-trashed-after") == Some(0)
                    && num("des-undo-again") == Some(0),
            )
        },
        DES_ROWS[4],
        format!(
            "销毁之前菜单里那一行出现={}(账是空的)、之后按一次撤销：那只手自报还回 {} 枚、场上从 {} 枚回到 {} 枚、账归零={}、再按一次交回 {} 枚；判据＝第 1148 行那句「支持撤销」在这一格里是真的——还回来的是**成员**所以 `count` 跟着走、core 那一格 `PinSlot.index` 记的是它们各自离开的那一格（撤销回来还是原来那堆而不是全摞在最上面），账只认**最近一手**所以第二按交回 0 而不是把上一批还第二次。{} 后面这两枚读数是这一趟的堆没被本族弄走的证人：`pin window` 与 `pin pile *` 那几枚在本腿之后还要按每一枚各自的落点数一遍。{}",
            read("des-undo-absent"),
            read("des-undo"),
            read("des-pile-gone"),
            read("des-pile-back"),
            read("des-trashed-after"),
            read("des-undo-again"),
            read("des-pile-before"),
            if lost_undo.is_empty() {
                "六枚齐，判的是还回的枚数、堆、账归零与空账上那一次".to_string()
            } else {
                format!("撤销那六枚不齐（缺 {lost_undo:?}），这一格没有问")
            },
        ),
    ));
    out
}

/// §5.11 独显的七问各占一枚名字：带走的那一把与它的账、屏幕上那一句话、菜单上那一行字面、同一枚
/// 手势的第二下、本窗自己被带走时的一进一出、空名单上的一记热键，以及一枚场上没有的 id。
const SOLO_ROWS: [&str; 7] = [
    "pin solo enter",
    "pin solo hint",
    "pin solo label",
    "pin solo exit",
    "pin solo restore",
    "pin solo refuse",
    "pin solo ghost",
];

/// §5.11 里能被真机问出来的那七问，从量规的三十四枚 `solo-*` 行里读。
///
/// `dim` 是从 [`crate::settings::pin_cfg`] 现读的那一枚 `solo_dim_opacity`。「压暗还是藏起来」这一问
/// 的源档就是那个键，本档不许假设它等于默认值 35（#261）；也不许只拿 QML 递上来的 `solo-dims` 当尺——
/// 那一枚是**被问的人自己**的答案，用它当尺就是让它替尺作答（#274 的反面），所以两枚在此各读一处、
/// 又在同一格里对得上（对不上就是桥侧那一只读错了配置）。
///
/// 七问一枚也不并，因为它们各是 §5.11 自己的一句话：步骤 3「隐藏未选中的贴图」（第一问）、
/// 规则③「屏幕提示中显示当前处于 Solo 状态」（第二问）、那一行菜单的字面（第三问）、步骤 4
/// 「再次执行 Solo 或退出命令」（第四问）、规则① 与 ②「记录进入前的显示状态 / 恢复原状态而不是
/// 无条件显示全部」（第五问，本窗的一进一出）、步骤 1「必须先选中至少一枚贴图」（第六问），以及
/// 「0 不许扮成一次悄悄的成功」那一族的第五格（第七问）。
///
/// 别的窗各自被压暗 / 藏成什么样，本窗读不到（QML 里没有第二扇窗的 `view`），那一半的证人是
/// `state.rs` 的 selftest 与 `pin pile *` 那几枚从产品外侧数桌面上还剩几扇窗的行（#271）——所以
/// 第五问只判**说话的那一扇窗自己**被带走的那一场往返，不冒充判过了全体。
fn solo_rows(lines: &[String], id: PinId, dim: u32) -> Vec<(Check, &'static str, String)> {
    let num = |k: &str| said(lines, k).and_then(|v| v.parse::<i64>().ok());
    let read = |k: &str| -> String {
        said(lines, k)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<没有读数>".into())
    };
    let miss = |keys: &[&'static str]| -> Vec<&'static str> {
        keys.iter()
            .filter(|k| said(lines, k).is_none())
            .copied()
            .collect()
    };
    // `solo-menu-shape` 那一句是窗把三格缓存一次交回来的（`open=` / `reach=` / `way=`），逐名拆读
    // 而不是整串相等：那一串里「way」的取值由配置决定，写死任何一枚都是本档替环境作答（#261）。
    let shape = |field: &str| -> Option<String> {
        said(lines, "solo-menu-shape").and_then(|v| {
            v.split(';')
                .find_map(|part| part.split_once('=').filter(|(name, _)| *name == field))
                .map(|(_, val)| val.to_string())
        })
    };
    let dims = dim > 0;
    let verb = if dims { "压暗" } else { "隐藏" };

    let mut out: Vec<(Check, &'static str, String)> = Vec::new();

    let lost_enter = miss(&[
        "solo-pile",
        "solo-reach-self",
        "solo-epoch-before",
        "solo-enter",
        "solo-epoch-enter",
        "solo-kept",
        "solo-hidden-self",
        "solo-live-self",
    ]);
    // 场上得真的有别枚可以被带走：只剩本窗一枚时那一手交 0 是**这件事本身**，不是产品答错
    // （那一格的判据会读成「带走的加留下的 = 场上的」，而 1 = 1 + 0 也成——一枚天然绿的尺，#433）。
    let field_ok = lost_enter.is_empty() && num("solo-pile").is_some_and(|n| n >= 2);

    out.push((
        if !field_ok {
            Check::Blocked
        } else {
            Check::from(
                num("solo-enter")
                    == num("solo-pile").zip(num("solo-kept")).map(|(p, k)| p - k)
                    && num("solo-kept").is_some_and(|k| k >= 1)
                    && num("solo-reach-self") == num("solo-enter")
                    && num("solo-epoch-enter") == num("solo-epoch-before").map(|v| v + 1)
                    && said(lines, "solo-hidden-self") == Some("false")
                    && said(lines, "solo-live-self") == Some("true"),
            )
        },
        SOLO_ROWS[0],
        format!(
            "名单空着、点的是本窗（{id}）这一枚的菜单：场上 {} 枚，那一手自报带走 {} 枚、留着 {} 枚，开菜单之前桥侧对「这一手会带走几枚」答 {}，号从 {} 走到 {}，本窗自己 solo_hidden={} live={}；判据＝§5.11 步骤 3「系统隐藏未选中的贴图」在这里是把**名单以外**的那些枚带走，于是带走的加留下的必须正好等于场上的（少了就是有一枚没人管，多了就是本窗自己也被带走，而它恰恰是用户点中的那一枚），菜单上印着的那个数与按下去真带走的那些枚必须同源——两处读的都是同一张 `solo_kept_table`，窗自己一枚也不数（#281）；一次真改动发一次号，不发则 `main.qml` 里 `soloTick` 那一声明没有读者、别的窗不会重读自己该被压到多暗（#316）；留着的那一枚不许不知道自己被留着（`solo_hidden` 为假、`live` 为真，两档配置下都该如此）。{}",
            read("solo-pile"),
            read("solo-enter"),
            read("solo-kept"),
            read("solo-reach-self"),
            read("solo-epoch-before"),
            read("solo-epoch-enter"),
            read("solo-hidden-self"),
            read("solo-live-self"),
            if !lost_enter.is_empty() {
                format!("八枚不齐（缺 {lost_enter:?}），这一格没有问")
            } else if !field_ok {
                "场上只有本窗一枚，没有别枚可被带走，这一格没有问".to_string()
            } else {
                "八枚齐，判的是那本账平不平、字面与手同不同源、号走没走、本窗在不在留着那一侧".to_string()
            },
        ),
    ));

    let lost_said = miss(&["solo-said", "solo-enter", "solo-dims", "solo-pile"]);
    let said_ok = said(lines, "solo-said").is_some_and(|s| {
        let n = num("solo-enter").unwrap_or_default();
        s.contains("独显") && s.contains(verb) && s.contains(&format!("{n} 枚"))
    });
    out.push((
        if !lost_said.is_empty() || !num("solo-pile").is_some_and(|n| n >= 2) {
            Check::Blocked
        } else {
            Check::from(said_ok && said(lines, "solo-dims") == Some(if dims { "true" } else { "false" }))
        },
        SOLO_ROWS[1],
        format!(
            "进入之后屏幕上那一格活话（`session.status`，`HintWindow.qml` 绑的就是这一格）读回 {:?}，同一次那一只手自报带走 {} 枚，窗自己那一侧读到的机制是 {:?} 而配置键 `pin.solo_dim_opacity`={}；判据＝§5.11 规则③「应在屏幕提示中显示当前处于 Solo 状态」由**产品自己写的那一句**答，本腿不替它编一句——所以那一句话里必须有「独显」两个字、必须有它自己报出的那个枚数（印着的数与真的数各走各的，就是菜单那一行同一枚病，#281），而句里的动词必须跟着配置键：>0 是「压暗」、=0 是「隐藏」。托盘那一臂本轮没做，逐名记在台账的欠账里。{}",
            read("solo-said"),
            read("solo-enter"),
            read("solo-dims"),
            dim,
            if !lost_said.is_empty() {
                format!("提示那四枚不齐（缺 {lost_said:?}），这一格没有问")
            } else if !num("solo-pile").is_some_and(|n| n >= 2) {
                "场上只有本窗一枚，这一格没有问".to_string()
            } else {
                "四枚齐，判的是那句话里有没有独显、有没有它自己报的数、动词对不对得上配置".to_string()
            },
        ),
    ));

    let lost_label = miss(&["solo-menu-shape", "solo-menu-label", "solo-dims"]);
    let (open, reach) = (shape("open"), shape("reach"));
    let way = shape("way");
    out.push((
        if !lost_label.is_empty() || open.is_none() || reach.is_none() || way.is_none() {
            Check::Blocked
        } else {
            Check::from(
                open.as_deref() == Some("true")
                    && reach.as_deref() == Some("0")
                    && said(lines, "solo-menu-label") == Some("退出独显")
                    && way.as_deref() == Some(verb),
            )
        },
        SOLO_ROWS[2],
        format!(
            "独显开着时把本窗的菜单打开，三格缓存一次读回：{}（门出现={}、这一手会带走几枚={}、机制={:?}），菜单那一行的字面是「{}」；判据＝退出那一支**不许**印一个数——独显开着时「这一手会带走几枚」问的其实是「会放回去几枚」，而那个数只活在 core 的 `Solo::saved` 那张表里，桥侧不许留第二份副本，于是 `solo_answer` 在场外答数、在场内答 0，菜单那一行随之改口成「退出独显」：这一格判的就是它真的改口了（没改口就是给用户印一枚它自己答不出的数，而那一句「影响 N 枚」按下去带走的却是另一枚数——#281 说的那件看得见的事）。字面里那个动词也得与配置键对得上（{:?} 而 `solo_dim_opacity`={}），否则「隐藏」与「压暗」这两档在菜单上是同一句话。{}",
            read("solo-menu-shape"),
            open.clone().unwrap_or_default(),
            reach.clone().unwrap_or_default(),
            way.clone().unwrap_or_default(),
            read("solo-menu-label"),
            read("solo-dims"),
            dim,
            if !lost_label.is_empty() {
                format!("字面那三枚不齐（缺 {lost_label:?}），这一格没有问")
            } else if open.is_none() || reach.is_none() || way.is_none() {
                "那一格交回来的串拆不出 open/reach/way 三名，这一格没有问".to_string()
            } else {
                "三枚齐，判的是门真开了、那一格真的答 0、字面真的改了口".to_string()
            },
        ),
    ));

    let lost_exit = miss(&[
        "solo-pile",
        "solo-enter",
        "solo-exit",
        "solo-epoch-enter",
        "solo-epoch-exit",
        "solo-kept-after",
    ]);
    out.push((
        if !lost_exit.is_empty() || !num("solo-pile").is_some_and(|n| n >= 2) {
            Check::Blocked
        } else {
            Check::from(
                num("solo-exit") == num("solo-enter")
                    && num("solo-exit") == num("solo-pile").zip(num("solo-kept")).map(|(p, k)| p - k)
                    && num("solo-kept-after") == Some(0)
                    && num("solo-epoch-exit") == num("solo-epoch-enter").map(|v| v + 1),
            )
        },
        SOLO_ROWS[3],
        format!(
            "同一枚入口的第二下（§5.11 步骤 4「用户再次执行 Solo 或退出命令」）：那一手自报放回 {} 枚、第一下带走的是 {} 枚、场上 {} 枚、`solo_kept` 归零={}、号从 {} 走到 {}；判据＝同一枚手势两走，而第二下交回的必须是**被带走的那些枚**而不是场上的全部——这一格就是规则②那句「而不是无条件显示全部」落在数上的形状（{} 枚而场上有 {} 枚，差的那一枚是本来就没被带走的本窗自己，`exit_solo` 不该去摸它一下），账上也不留副本所以 `solo_kept` 必须归零（不归零就是退出之后屏幕上那一行还写着「退出独显」，用户再按一下就走进第二次）。{}",
            read("solo-exit"),
            read("solo-enter"),
            read("solo-pile"),
            read("solo-kept-after"),
            read("solo-epoch-enter"),
            read("solo-epoch-exit"),
            read("solo-exit"),
            read("solo-pile"),
            if !lost_exit.is_empty() {
                format!("第二下那六枚不齐（缺 {lost_exit:?}），这一格没有问")
            } else if !num("solo-pile").is_some_and(|n| n >= 2) {
                "场上只有本窗一枚，这一格没有问".to_string()
            } else {
                "六枚齐，判的是放回的枚数、它与第一下相不相等、账归没归零、号再走一步".to_string()
            },
        ),
    ));

    let lost_restore = miss(&[
        "solo-target-other",
        "solo-op-start",
        "solo-enter-other",
        "solo-hidden",
        "solo-live",
        "solo-opacity",
        "solo-epoch-exit",
        "solo-epoch-other",
        "solo-exit-other",
        "solo-op-end",
        "solo-hidden-end",
        "solo-live-end",
    ]);
    let want = dim.min(100) as i64;
    let (op0, op1) = (num("solo-op-start"), num("solo-opacity"));
    let other_ok = num("solo-target-other").is_some_and(|v| v != id as i64);
    // 压暗那一级与这一枚进门那一级**相同**时，这一格读不出「被压暗了」与「什么都没动」的差别：
    // 交回 Blocked 而不是拿一个没动过的读数冒充绿（#433/#440，与 `rst_rows` 那一档缩放同一枚形状）。
    // 只在这一格相等时才放弃——读数一动不动而压暗那一级是**另一个数**，那是没压，不是问不出。
    let flat = dims && op0 == Some(want) && op1 == op0;
    let crossed = if dims {
        said(lines, "solo-hidden") == Some("false")
            && said(lines, "solo-live") == Some("true")
            && op1 == Some(want)
    } else {
        said(lines, "solo-hidden") == Some("true")
            && said(lines, "solo-live") == Some("false")
            && op1 == op0
    };
    out.push((
        if !lost_restore.is_empty() || !other_ok || flat {
            Check::Blocked
        } else {
            Check::from(
                crossed
                    && num("solo-epoch-other") == num("solo-epoch-exit").map(|v| v + 1)
                    && num("solo-exit-other") == num("solo-enter-other")
                    && num("solo-op-end") == op0
                    && said(lines, "solo-hidden-end") == Some("false")
                    && said(lines, "solo-live-end") == Some("true"),
            )
        },
        SOLO_ROWS[4],
        format!(
            "留着的是**别的**那一枚 {}（本窗是 {id}），于是被带走的是说话的这一扇窗自己：进门时自己的不透明度 {}，那一手自报带走 {} 枚、本窗 solo_hidden={} live={} 不透明度={}、号从 {} 走到 {}；再按第二下自报放回 {} 枚、自己的不透明度回到 {}、solo_hidden={} live={}；判据＝规则①「应记录进入 Solo 前各贴图的显示状态」与规则②「退出后应恢复原状态」在这一格里是同一枚窗的一进一出——这一场是压暗（配置键 {}>0）时本窗必须被压到那一格上（读到 {}，要求 {}）而不是被藏掉（`live` 仍为真），这一场是隐藏时反过来（`solo_excluded` 为真、`live` 为假、自己的不透明度一动不动：两档各判各的，判错档就是桥侧读错了那枚键）；两场的**出**都要等于**进**（不透明度回到进门那一级而不是 100——无条件显示全部就是拿默认值盖掉用户自己调过的那一级）。本窗一次也没 `reload` 过自己，这几格只可能是扇出替它问的，那一条腿的证人是 `main.qml` 里 `soloTick` 那一声明（#316）。{}",
            read("solo-target-other"),
            read("solo-op-start"),
            read("solo-enter-other"),
            read("solo-hidden"),
            read("solo-live"),
            read("solo-opacity"),
            read("solo-epoch-exit"),
            read("solo-epoch-other"),
            read("solo-exit-other"),
            read("solo-op-end"),
            read("solo-hidden-end"),
            read("solo-live-end"),
            dim,
            read("solo-opacity"),
            want,
            if !lost_restore.is_empty() {
                format!("往返那十二枚不齐（缺 {lost_restore:?}），这一格没有问")
            } else if !other_ok {
                "点名的证人就是本窗自己，这一格没有问".to_string()
            } else if flat {
                "这一枚进门那一级已经就等于压暗那一级，压没压读不出差别，这一格没有问".to_string()
            } else {
                "十二枚齐，判的是这一档该动的那几格真动了、该不动的那一格没动、出等于进".to_string()
            },
        ),
    ));

    let lost_refuse = miss(&[
        "solo-refuse-before",
        "solo-refuse",
        "solo-refuse-epoch",
        "solo-refuse-kept",
        "solo-refuse-said",
    ]);
    out.push((
        if !lost_refuse.is_empty() {
            Check::Blocked
        } else {
            Check::from(
                num("solo-refuse") == Some(0)
                    && num("solo-refuse-epoch") == num("solo-refuse-before")
                    && num("solo-refuse-kept") == Some(0)
                    && said(lines, "solo-refuse-said")
                        .is_some_and(|s| s.contains("独显") && s.contains("没做成") && s.contains("选中")),
            )
        },
        SOLO_ROWS[5],
        format!(
            "名单空着按 `ctrl+shift+s` 那一路（`toggleSoloSelection`，本轮第一次有读者）：那一手自报 {} 枚、号从 {} 到 {}、`solo_kept`={}、屏幕上那句读回 {:?}；判据＝§5.11 步骤 1「必须先选中至少一枚贴图」在被满足之前不许动手——交 0 且**不发号**（发了就是全场为一件没发生的事重读一遍，#455），屏幕上出现的那一句必须是拒绝而不是「已进入独显」。这一格挡的正是 core 那一枚 `toggle_solo` 的 `bool` 装不下的那一件事：`false` 同时容纳「刚退出 Solo」与「没选中所以被拒绝」，只看它就无法知道有没有暗掉一片屏（[bool-loss] 那一格记的就是这条缝），所以桥侧那一手数的是枚数、拒绝的那一支另写一句话。{}",
            read("solo-refuse"),
            read("solo-refuse-before"),
            read("solo-refuse-epoch"),
            read("solo-refuse-kept"),
            read("solo-refuse-said"),
            if lost_refuse.is_empty() {
                "五枚齐，判的是交 0、号不动、账为空、那句是拒绝".to_string()
            } else {
                format!("拒绝那五枚不齐（缺 {lost_refuse:?}），这一格没有问")
            },
        ),
    ));

    let lost_ghost = miss(&[
        "solo-ghost",
        "solo-epoch-ghost",
        "solo-ghost-kept",
        "solo-refuse-epoch",
    ]);
    out.push((
        if !lost_ghost.is_empty() {
            Check::Blocked
        } else {
            Check::from(
                num("solo-ghost") == Some(0)
                    && num("solo-epoch-ghost") == num("solo-refuse-epoch")
                    && num("solo-ghost-kept") == Some(0),
            )
        },
        SOLO_ROWS[6],
        format!(
            "独显那一只手上按一枚还没被发出去的 id（`win.gauge[5]`，与 `des-ghost` 同一枚证人）：那一手自报 {} 枚、号={} 而上一手之后是 {}、`solo_kept`={}；判据＝0 只有一枚贴图能给出——`hand_targets` 对不在集里的那一枚交回空表，于是桥侧在 core 之前就把这一手挡下（不是 core 拒绝的：那一格拒绝的是空名单，两件事各有其名，见上一格）；空手不许发号，也不许把任何一枚记成「留着的一枚」（记了就成「独显开着却无人知晓」，下一次按下去交回的是一批没人同意过的窗）。**别人的窗没被碰**这一半本窗读不到，证人是 `pin pile *` 那几枚与 `state.rs` selftest（#271）。{}",
            read("solo-ghost"),
            read("solo-epoch-ghost"),
            read("solo-refuse-epoch"),
            read("solo-ghost-kept"),
            if lost_ghost.is_empty() {
                "四枚齐，判的是交 0、号不动、没人被记成留着".to_string()
            } else {
                format!("ghost 那四枚不齐（缺 {lost_ghost:?}），这一格没有问")
            },
        ),
    ));
    out
}

/// §5.9.5–§5.9.8 那六只手的七枚判据，一枚也不并：旋转那一格、两枚翻转各一格（§5.9.6 那句
/// 「选择水平翻转或垂直翻转」是两个落点，不是一个）、灰度、颜色反转、六枚字面、数字键那一问自己。
const XF_ROWS: [&str; 7] = [
    "pin rotate step",
    "pin flip horizontal",
    "pin flip vertical",
    "pin grayscale toggle",
    "pin invert toggle",
    "pin transform labels",
    "pin transform keys",
];

/// 菜单上那六行自己写的那六个词，逐字取自 PRD 的 §5.9.5 / §5.9.6 / §5.9.7 / §5.9.8 那四节
/// （「向左、向右旋转」「水平翻转」「垂直翻转」「灰度」「颜色反转」）。放在这里而不是散进
/// 判据：字面对不上是一格，六个字面里错哪一个是那一格的 detail 要报的。
const XF_LABELS: [&str; 6] = [
    "向左旋转",
    "向右旋转",
    "水平翻转",
    "垂直翻转",
    "灰度",
    "颜色反转",
];

/// 本档给那三枚数字键钉的键帽（PRD 第 1010 行只说「数字键」、第 1026 行只说「对应快捷键」，
/// 第 7 行明写「快捷键使用常见默认值举例，实际以用户设置和运行平台为准」）。键帽这一半由
/// [`the_transform_keys_share_the_menu_route`] 那枚源码扫描在文件原文上逐字判；第七格那一枚
/// live 读数判的是「注册上了一枚键序、且此刻开着」——两半合起来才是「数字键能用」这句话。
const XF_KEYS: [(&str, &str); 3] = [
    ("xf-key-left", "1"),
    ("xf-key-right", "2"),
    ("xf-key-gray", "3"),
];

/// §5.9.5–§5.9.8 里能被真机问出来的那七问，从量规的四十枚 `xf-*` 行里读。
///
/// 每一枚读数读的都是 `view` 上那一格**本体**（`rotation` / `flipped_h` / `flipped_v` /
/// `grayscale` / `inverted`，P145 才送到窗上）。为什么非要那一格：§5.9.6 步骤 3 写的是「系统
/// **更新显示状态**」，§5.9.7 与 §5.9.8 步骤 4 写的是「**再次执行**恢复」，两句话问的都是
/// 「按下之前是什么、按下之后是什么」，而窗上读不到那一格之前，本腿能问的只有「按下发生过」
/// 这一件（#271：不许向一枚从没供过这格的读数要这一句）。
///
/// 纹理号 `revision` 每按一次必须 +1。这五格全在 `state.rs` 的 `content_key` 里，按一次不涨
/// 就是像素没到屏幕上；一次按两格涨就是同一张图编了两遍。两问都不并，因为它们是两只手的事。
///
/// **旋转的窗口几何只判收梢**（`xf-rot-geo-start` 与 `xf-rot-geo-end` 必须相等），不判「转 90 度
/// 窗跟着换横竖」：那一问在一张方图上是天然绿的（交换前后同一枚数，#433），而本档的证人正是
/// 那几枚方图。换横竖那一半的证人是 core 的 `sync_size` 单测与 `pin window` 那一行。
///
/// 第七格判的是**注册**：三枚 `Shortcut` 自己报的「键序枚数」与 `enabled`。键帽本体不在这一格
/// 判（Qt 的 QKeySequence 字符串不是产品约定），它在 `PinWindow.qml` 的原文上由源码扫描逐字读。
/// 这一格也不判「按下」——全仓一枚注入器也没有（`SendInput` 零命中），所以「键按下去那只手有没有
/// 动」在本档只有 [`the_transform_keys_share_the_menu_route`] 那一枚源码扫描能证到「门与键共指一枚
/// Action」，逐名记在台账的欠账里，不拿注册冒充。
fn xf_rows(lines: &[String]) -> Vec<(Check, &'static str, String)> {
    let num = |k: &str| said(lines, k).and_then(|v| v.parse::<i64>().ok());
    let read = |k: &str| -> String {
        said(lines, k)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "<没有读数>".into())
    };
    let miss = |keys: &[&'static str]| -> Vec<&'static str> {
        keys.iter()
            .filter(|k| said(lines, k).is_none())
            .copied()
            .collect()
    };
    let mut out: Vec<(Check, &'static str, String)> = Vec::new();

    // 第一格：§5.9.5 步骤 2「使用数字键或菜单执行向左、向右旋转」+ 步骤 3「以 90 度为基本单位」。
    // 六次 `trigger()` 走的是菜单那一行背后那枚 Action，角度这一格必须走出
    // start → +90 → start → +180 → start，一步也不多、一步也不少；一圈之内不许出现非 90 的角。
    let lost_rot = miss(&[
        "xf-rot-start",
        "xf-rot-geo-start",
        "xf-rot-rev-before",
        "xf-rot-cw",
        "xf-rot-rev-cw",
        "xf-rot-ccw",
        "xf-rot-rev-ccw",
        "xf-rot-half",
        "xf-rot-full",
        "xf-rot-rev-full",
        "xf-rot-geo-end",
    ]);
    let r0 = num("xf-rot-start");
    let unit = r0.is_some_and(|v| v.rem_euclid(90) == 0 && (0..=270).contains(&v));
    let want = |turns: i64| r0.map(|v| (v + turns * 90).rem_euclid(360));
    out.push((
        if !lost_rot.is_empty() {
            Check::Blocked
        } else {
            Check::from(
                unit
                    && num("xf-rot-cw") == want(1)
                    && num("xf-rot-ccw") == r0
                    && num("xf-rot-half") == want(2)
                    && num("xf-rot-full") == r0
                    && num("xf-rot-rev-cw") == num("xf-rot-rev-before").map(|v| v + 1)
                    && num("xf-rot-rev-ccw") == num("xf-rot-rev-before").map(|v| v + 2)
                    && num("xf-rot-rev-full") == num("xf-rot-rev-before").map(|v| v + 6)
                    && said(lines, "xf-rot-geo-end") == said(lines, "xf-rot-geo-start"),
            )
        },
        XF_ROWS[0],
        format!(
            "本窗进门角度 {}（90 的整数倍且在 0..=270={}），向右一次读回 {}、向左一次读回 {}、再向右两次读回 {}、再向右两次读回 {}；纹理号从 {} 走到 {}（向右一次）再 {}（向左一次）最后 {}（共六次按下）；收梢尺寸 {} 对进门 {}；判据＝§5.9.5 步骤 3 那句「以 90 度为基本单位」在这一格里是**每次按下恰好一步**：+90、一步回到原角、+180、满一圈回到原角，四读同一枚起点算出来的数不相等就是那只手一次迈了两档或压根没迈；六次按下六次涨号，一次不涨就是转过的像素没重编（`rotation` 在 `content_key` 里，不涨意味着窗上还是旧图），两次一起涨就是同一张编了两遍；一圈之后尺寸必须与进门同一枚数——这一格是后面每一格与 `pin window` 那一行的前置：本族不许把窗留在别的尺寸上。「转 90 度窗跟着换横竖」那一半这里**不判**，方图上交换前后同一枚数、天然绿（#433），它的证人是 core 的 `sync_size` 单测。{}",
            read("xf-rot-start"),
            unit,
            read("xf-rot-cw"),
            read("xf-rot-ccw"),
            read("xf-rot-half"),
            read("xf-rot-full"),
            read("xf-rot-rev-before"),
            read("xf-rot-rev-cw"),
            read("xf-rot-rev-ccw"),
            read("xf-rot-rev-full"),
            read("xf-rot-geo-end"),
            read("xf-rot-geo-start"),
            if lost_rot.is_empty() {
                "十一枚齐，判的是那四个角、六次涨号、收梢等于进门".to_string()
            } else {
                format!("旋转那十一枚不齐（缺 {lost_rot:?}），这一格没有问")
            },
        ),
    ));

    // 第二至第五格：两只翻转、灰度、颜色反转。四格同一形状，所以判据写一次（#281）在
    // [`xf_toggle`] 那一处，但四枚名字各占一格——§5.9.6 那句是两个落点，§5.9.7 与 §5.9.8
    // 是两节不同的话。
    out.push(xf_toggle(
        lines,
        "水平翻转",
        [
            "xf-fh-start",
            "xf-fh-on",
            "xf-fh-rev-on",
            "xf-fh-off",
            "xf-fh-rev-off",
        ],
        XF_ROWS[1],
        "§5.9.6 步骤 2「选择水平翻转」与步骤 3「系统更新显示状态」在这一格里是同一枚窗的一进一出",
        "flipped_h",
    ));
    out.push(xf_toggle(
        lines,
        "垂直翻转",
        [
            "xf-fv-start",
            "xf-fv-on",
            "xf-fv-rev-on",
            "xf-fv-off",
            "xf-fv-rev-off",
        ],
        XF_ROWS[2],
        "同一句话的另一只手——并成一格就会读不出「只有水平那一只在动」",
        "flipped_v",
    ));
    out.push(xf_toggle(
        lines,
        "灰度",
        [
            "xf-gray-start",
            "xf-gray-on",
            "xf-gray-rev-on",
            "xf-gray-off",
            "xf-gray-rev-off",
        ],
        XF_ROWS[3],
        "§5.9.7 步骤 3「显示为灰度」加步骤 4「再次执行可恢复原始颜色」——「恢复」在这一格是回到进门那一格而不是回到某个默认值（#261：进门那格本来就可以是灰的）",
        "grayscale",
    ));
    out.push(xf_toggle(
        lines,
        "颜色反转",
        [
            "xf-inv-start",
            "xf-inv-on",
            "xf-inv-rev-on",
            "xf-inv-off",
            "xf-inv-rev-off",
        ],
        XF_ROWS[4],
        "§5.9.8 步骤 3「RGB 反相显示」加步骤 4「再次执行恢复原图」，与灰度同一形状而那只手是另一枚",
        "inverted",
    ));

    // 第六格：六枚字面。读的是 Action 自己那一格（菜单行的字面从它抄），判的是 PRD 那四节里的词。
    let label_keys = [
        "xf-label-left",
        "xf-label-right",
        "xf-label-h",
        "xf-label-v",
        "xf-label-gray",
        "xf-label-inv",
    ];
    let lost_label = miss(&label_keys);
    let wrong: Vec<String> = label_keys
        .iter()
        .zip(XF_LABELS)
        .filter(|(k, want)| said(lines, k) != Some(*want))
        .map(|(k, want)| format!("{k}={:?} 要 {want:?}", said(lines, k).unwrap_or("<没读>")))
        .collect();
    out.push((
        if lost_label.is_empty() {
            Check::from(wrong.is_empty())
        } else {
            Check::Blocked
        },
        XF_ROWS[5],
        format!(
            "窗上那六枚 Action 自己报的字面：{}；判据＝§5.9.5 写的是「向左、向右旋转」而不是「顺时针/逆时针」、§5.9.8 写的是「颜色反转」而不是「反色」，六个词逐字比（菜单那一行的字面是从这一格抄的，读两处只会数出两枚读者而它们本来就是同一枚，#281）；方向那一半不在这格里——`向左旋转` 点的必须是 core 的 -90 那一支，那是第一格的读数管的。对不住的是「哪一行挂在哪个落点上」：那一问 QML 里没有活的可读的格，由源码扫描那枚单测逐枚点名（每枚 Action 一处落点、每枚 `MenuItem {{ action: … }}` 指向同一枚 id）。{}",
            label_keys
                .iter()
                .zip(XF_LABELS)
                .map(|(k, w)| match said(lines, k) {
                    Some(v) => format!("{k}={v:?}/{w:?}"),
                    None => format!("{k}=<没读>/{w:?}"),
                })
                .collect::<Vec<String>>()
                .join("，"),
            if wrong.is_empty() && lost_label.is_empty() {
                "六枚齐且六个词全对".to_string()
            } else if !wrong.is_empty() {
                format!("对不上的：{}", wrong.join("；"))
            } else {
                format!("六枚不齐（缺 {lost_label:?}），这一格没有问")
            },
        ),
    ));

    // 第七格：三枚数字键的**注册**。`sequences` 与 `enabled` 是窗上读得到的两格真值；「按下」
    // 那一跳没有注入器，本档不判也不冒充判（见上面那封信）。
    let key_read = |k: &str| -> Option<(String, String)> {
        said(lines, k).and_then(|v| {
            v.split_once('|')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        })
    };
    let lost_key: Vec<&str> = XF_KEYS
        .iter()
        .filter(|(k, _)| key_read(k).is_none())
        .map(|(k, _)| *k)
        .collect();
    let bad_key: Vec<String> = XF_KEYS
        .iter()
        .filter(|(k, _)| match key_read(k) {
            Some((count, enabled)) => count != "1" || enabled != "true",
            None => false,
        })
        .map(|(k, want)| match key_read(k) {
            Some((count, enabled)) => {
                format!("{k} 报 {count}枚键序/{enabled}，要 1枚/true（这一枚钉的键帽是 {want}）")
            }
            None => String::new(),
        })
        .collect();
    out.push((
        if lost_key.is_empty() {
            Check::from(bad_key.is_empty())
        } else {
            Check::Blocked
        },
        XF_ROWS[6],
        format!(
            "三枚数字键自己报的「注册了几枚键序 | 开开关关」：{}；判据＝每一枚都必须**恰好注册上一枚键序**且 `enabled` 为真——报 0 枚就是那行 `sequences:` 没接上（键压根不在窗上），报 2 枚以上说明有人给同一只手加了指法而本档只钉了一枚，`false` 那一支本腿不可能出现（分组名框从没被开过，§5.12.1 那扇格栅窗不在这条路上，退让那一支不该关掉键）。**键帽「1/2/3」钉在哪一枚键上不在这一格判**：live 这半读到的若是 `String(shortcut.sequences)`，出来的是 Qt 自己那套 QKeySequence 调试串，格式不是产品约定，拿它当判据等于把绿的红的交给 Qt 的版本（#261）；键帽那一半由源码扫描那枚单测在 `PinWindow.qml` 的原文上逐字读，那是文件的字面而不是运行时的字符串。**「按下」这一跳本档没判也不装判**：全仓零枚 `SendInput`，键帽送到哪扇窗这件事在无人值守的机器上做不到；那只手的落点由第一格与第四格按过（`trigger()` 的就是这枚 Shortcut 的 `onActivated` 要叫的同一枚 Action），而「门与菜单确实共指同一枚 Action」由 [`the_transform_keys_share_the_menu_route`] 那枚源码扫描点名。「§5.16.1 用户可在设置中修改快捷键」目前只覆盖全局表 `hotkey.actions`，这三枚窗口内的键还没进那张表——逐名记在台账。{}",
            XF_KEYS
                .iter()
                .map(|(k, _)| {
                    format!("{k}={:?}", said(lines, k).unwrap_or("<没读>").to_string())
                })
                .collect::<Vec<String>>()
                .join("，"),
            if !lost_key.is_empty() {
                format!("三枚不齐（缺 {lost_key:?}），这一格没有问")
            } else if !bad_key.is_empty() {
                bad_key.join("；")
            } else {
                "三枚齐，判的是各注册上一枚键序、开关为真、以及那一处退让没把键关掉".to_string()
            },
        ),
    ));
    out
}

/// 四格同一形状的开关，判据只写这一次（#281）：进门那一格、按一次、再按一次，中间夹两次按下
/// 各自的纹理号。四枚名字仍各占一格，见 [`xf_rows`] 上面那封信。
///
/// 「进门那一格本来就可以已经是 `true`」是被允许的：恢复的是**进门那一格**而不是某个默认值
/// （#261——拿 100 当不透明度的归位、拿 `false` 当灰度的起点，都是本档替用户的环境作答）。
/// 于是第一次按下判的是「读回的要与进门的不同」而不是「读回的必须是 `true`」。
fn xf_toggle(
    lines: &[String],
    name: &str,
    keys: [&'static str; 5],
    row: &'static str,
    clause: &str,
    prop: &str,
) -> (Check, &'static str, String) {
    let [start, on, rev_on, off, rev_off] = keys;
    let num = |k: &str| said(lines, k).and_then(|v| v.parse::<i64>().ok());
    let read = |k: &str| -> String {
        said(lines, k)
            .map(str::to_string)
            .unwrap_or_else(|| "<没有读数>".into())
    };
    let lost: Vec<&'static str> = keys
        .iter()
        .filter(|k| said(lines, k).is_none())
        .copied()
        .collect();
    let turned = matches!((said(lines, start), said(lines, on)), (Some(a), Some(b)) if a != b);
    let back = said(lines, off).is_some() && said(lines, off) == said(lines, start);
    let one_bake = num(rev_off) == num(rev_on).map(|v| v + 1);
    (
        if lost.is_empty() {
            Check::from(turned && back && one_bake)
        } else {
            Check::Blocked
        },
        row,
        format!(
            "{name}（窗上那一格 `view.{prop}`）：进门 {}，按一次读回 {}（纹理号 {}），再按一次读回 {}（纹理号 {}）；判据＝{clause}——第一下必须**翻**（读回的不许是进门那一格；进门本来就已经是灰的，第一下就该翻成不是灰的，这一格不替 PRD 猜起点），第二下必须**还回去**（与进门那一格同一枚值：「再次执行恢复」是回到自己那一格，不是再翻一次别的、也不是回到某个默认值，#261），两下之间纹理号恰好 +1（`{prop}` 在 `content_key` 的那一列里，不涨就是屏幕上的图没跟着这次开关重编；一次翻两格涨就是同一张编了两遍）。{}",
            read(start),
            read(on),
            read(rev_on),
            read(off),
            read(rev_off),
            if lost.is_empty() {
                "五枚齐，判的是翻、还、号恰好加一".to_string()
            } else {
                format!("五枚不齐（缺 {lost:?}），这一格没有问")
            },
        ),
    )
}

/// §5.9.16 的复制本身、§5.9.17 的那一路文件，各占一枚名字。
const COPY_ROWS: [&str; 2] = ["pin copy", "pin copy file"];
/// The one sentence `copy-drawn` handed back, graded twice: whether the copy of a real,
/// drawn pin was accepted at all, and whether the file leg came along *exactly* when the
/// switch said it should.
///
/// `on` 是这一趟产品自己加载的那枚 `output.copy_as_file`——调用点读它用的就是 `copy_pin` 走的
/// 同一条路（[`crate::settings::output_cfg`]），所以判的是「写这句话时生效的那枚开关」而不是本
/// 档对配置的记忆；两种答法因此都能在一只没有窗、没有配置的测试里被驱动（§10 第 69 项 ⑫ 那一族）。
///
/// The path is read out of the product's own sentence rather than recomputed here, and the
/// two claims made about it are the two §5.9.17 spells out: it sits under the directory the
/// product names as its own ([`state::PinState::clip_dir`]), and it is on disk *now* — the
/// promise of this copy is a file the paste target can open, so a sentence naming a path
/// that is not there is this row's own red, not a note.
///
/// 反证是关掉那一侧（#274/#461）：开关关着时这句里**不许**出现「文件引用」，于是这一格有两种答法。
/// 命名规则只读不判（文件名原样印在 detail 里）：§5.9.17 那句「应遵守预设命名规则」的判据属于
/// `naming` 那一族已经拿起的尺，这一轮不另立一枚会读空的尺（#274 的「尺要展示会读空的一面」）。
fn copy_rows(lines: &[String], on: bool) -> Vec<(Check, &'static str, String)> {
    let mut out: Vec<(Check, &'static str, String)> = Vec::new();
    let Some(said) = said(lines, "copy-drawn") else {
        let why =
            "runGauge 那一腿没报上 `copy-drawn`，§5.9.16 的复制与 §5.9.17 的那一路文件都没被问过";
        return COPY_ROWS
            .iter()
            .map(|n| (Check::Blocked, *n, format!("这一格没有问：{why}")))
            .collect();
    };

    let copied = said.starts_with("已复制贴图");
    out.push((
        Check::from(copied),
        COPY_ROWS[0],
        format!(
            "{:?}；判据＝这一枚真贴图（`copyAction.trigger()` 点的那枚 `win.pinId`）的复制被接受，话的开头是那一句用户会看见的确认",
            said
        ),
    ));

    let named: Option<std::path::PathBuf> = said
        .split_once("文件引用 ")
        .map(|(_, rest)| std::path::PathBuf::from(rest.trim()));
    let ours = named
        .as_ref()
        .is_some_and(|p| p.starts_with(state::PinState::clip_dir()));
    let there = named.as_ref().is_some_and(|p| p.is_file());
    let (grade, ask) = if on {
        (
            copied && named.is_some() && ours && there,
            "开着那一侧要：这句点名一路文件、文件在产品自己那枚临时目录之下、此刻真在盘上",
        )
    } else {
        (
            copied && named.is_none(),
            "关着那一侧要：这句里不出现「文件引用」——开关是这条路唯一的门，反证就是它关上的那一面",
        )
    };
    out.push((
        Check::from(grade),
        COPY_ROWS[1],
        format!(
            "开关={}；点名文件={}；在那枚目录下={ours}；此刻在盘上={there}；判据＝{ask}（§5.9.17 step 3，文件名只读不判）",
            on,
            named
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<这句没点名文件>".to_string())
        ),
    ));
    out
}

/// The three rows 存完再钉图 registers (§10 第 43 项 ①), in the order the flow reaches
/// them: the file, the pin made from it, the window that pin got.
///
/// **The registered pair is row 1's bytes and row 2's hwnd.** The item's wording asks for
/// "保存产物那枚 sha ↔ 钉出来的那扇窗首帧的 sha 同值"; this compares the two frames
/// channel by channel instead of hashing them, which says the same thing with more
/// detail - and hashing it would need a crate the lock file does not have, while the
/// pixels are already in hand.
///
/// Neither side of row 1 is a copy of the frame the write went out from: the bytes come
/// from `decode_file` of the artifact on disk, the other half from the state machine's
/// first frame for the id the file door handed back. So a `save` that wrote something
/// other than what it was given, or an `add_from_file` that pinned something other than
/// what it read, is a count of differing channels here rather than a passing grade.
///
/// Row 2's other side is the operating system, twice over and by two roads that have to
/// agree: [`crate::capture::desktop_scene`]'s own `window_at_point` - the point-in-z-order
/// road, at a point inside the rect the pin asked for - and [`pin_window`], which reads
/// Qt's top-level list. One number for both is what says the window the product believes
/// it made is the window the shell says is there. `Blocked` when the point answers
/// nothing: that is the desktop being unreadable, not the pin missing.
///
/// `fid: 0` means [`plant_file_pin`] refused. Rows 2 and 3 are then not gradable - there
/// is no window to read and none was asked about - so [`unasked_rows`] prints both as
/// their own `Blocked` line instead of grading a window that was never made, which would
/// be grading this file.
fn file_pin_rows(
    frect: Option<PhysRect>,
    fid: PinId,
    from_file: &Path,
    file_said: &str,
) -> Vec<(Check, &'static str, String)> {
    let named = from_file.display().to_string();
    let Some(rect) = frect else {
        let mut rows = vec![(
            Check::Fail,
            "pin from file",
            format!("{named} 没有钉成贴图：{file_said}"),
        )];
        rows.extend(unasked_rows(
            0,
            &format!("贴图 {fid} 没有 rect，窗与像素两格无从问起"),
        ));
        return rows;
    };
    let from_disk = match falcon_core::encode::decode_file(from_file) {
        Ok(f) => f,
        Err(e) => {
            let mut rows = vec![(
                Check::Fail,
                "pin from file",
                format!("{named} 在盘上读不回来：{e}"),
            )];
            rows.extend(unasked_rows(
                0,
                &format!("{} 读不回来，没有可比的首帧", from_file.display()),
            ));
            return rows;
        }
    };
    let mut rows: Vec<(Check, &'static str, String)> = Vec::new();

    // 1. The artifact, read back off the disk, against the frame the window is built from.
    //
    // §10 第 45 项 ④：这一格不再把"没有可比的分母"写成一个整数。它以前是 `usize::MAX`，
    // 于是句子印成 `不同的通道 18446744073709551615 个（要求 0）`——一个整数上限冒充了通道数，
    // 而读者要的那句人话在括号里。`None` 就是没有分母，它自己带为什么；判定照旧 Fail，一格没松。
    let shown = state::with(|s| s.with_rendered(fid, |f| f.clone()));
    let (diff, sizes) = match &shown {
        None => (None, "没有首帧可读".to_string()),
        Some(frame) => {
            let shape = frame.width == from_disk.width
                && frame.height == from_disk.height
                && frame.pixels.len() == from_disk.pixels.len();
            (
                if shape {
                    Some(
                        frame
                            .pixels
                            .iter()
                            .zip(&from_disk.pixels)
                            .filter(|(a, b)| a != b)
                            .count(),
                    )
                } else {
                    None
                },
                if shape {
                    format!(
                        "文件 {}x{} ↔ 贴图 {fid} 首帧 {}x{}",
                        from_disk.width, from_disk.height, frame.width, frame.height
                    )
                } else {
                    format!(
                        "文件 {}x{} 与贴图 {fid} 首帧 {}x{} 形状不同，逐通道没有对齐的两串像素可比",
                        from_disk.width, from_disk.height, frame.width, frame.height
                    )
                },
            )
        }
    };
    let counted = match diff {
        Some(n) => format!("不同的通道 {n} 个（要求 0）"),
        None => "这一格没有可比的分母，所以它是 FAIL 而不是 0 个不同".to_string(),
    };
    rows.push((
        Check::from(diff == Some(0)),
        "pin from file",
        format!("{named}（{sizes}）{counted}；钉它的那句 {file_said:?}"),
    ));

    // 2. That window's hwnd, by the two roads, in one number.
    let centre = PhysPoint::new(rect.x + rect.w as i32 / 2, rect.y + rect.h as i32 / 2);
    let at_point = platform_windows::proc::window_at_point(centre);
    let Some(t) = pin_window(rect) else {
        rows.push((
            Check::Fail,
            "pin from file window",
            format!(
                "没有一个可见顶层和贴图 {fid} 的 {rect} 重叠（中心 ({},{}) 上是 {at_point:?}）",
                centre.x, centre.y
            ),
        ));
        // The row below reads this window's pixels, and there is no handle to hand it.
        // Index 1 is the row just pushed, so exactly the one underneath it is owed.
        rows.extend(unasked_rows(
            1,
            &format!("贴图 {fid} 的窗找不着，没有 hwnd 可截"),
        ));
        return rows;
    };
    let close = within_slack(&t.phys, &rect);
    rows.push(match at_point {
        None => (
            Check::Blocked,
            "pin from file window",
            format!(
                "({},{}) 处桌面 shell 不认任何窗口，这一格问不了；Qt 那份是 0x{:x}",
                centre.x, centre.y, t.hwnd
            ),
        ),
        Some(there) => {
            // Who the shell puts there, named, because the two numbers disagreeing is
            // the question and *which* window answered it is what tells a defect in this
            // product's stacking apart from a session no click can reach. `--r13` has
            // spelled the reading this way since P39 and [`crate::capture::is_lock`] is
            // the one rule for it (§10 第 41 项 ④); a locked workstation covers the whole
            // desktop with `LockApp`, so 这一格在本机本次问不了 - and per §10 第 41 项 ⑤
            // that is this row's input, not a verdict over the round: the same run reads
            // the pixels of the same window two rows below and passes.
            let app = platform_windows::proc::pid_of_window(there)
                .and_then(platform_windows::proc::process_name)
                .unwrap_or_else(|| "未知进程".to_string());
            let title = platform_windows::proc::window_title(there).unwrap_or_default();
            let same = there == t.hwnd;
            let check = match (same, crate::capture::is_lock(&app)) {
                (true, _) => Check::from(close),
                (false, true) => Check::Blocked,
                (false, false) => Check::Fail,
            };
            (
                check,
                "pin from file window",
                format!(
                    "同一位置两条路读回 0x{there:x}（{app}「{title}」）vs Qt 0x{:x}，一致={}；顶层 {}x{}@{},{} 和要求的 {rect} 相差在 {RECT_SLACK} px 内={}（class={}）{}",
                    t.hwnd,
                    same,
                    t.phys.w,
                    t.phys.h,
                    t.phys.x,
                    t.phys.y,
                    close,
                    t.class,
                    if check == Check::Blocked {
                        "；会话已锁屏，桌面那一条路在本机本次问不了"
                    } else {
                        ""
                    },
                ),
            )
        }
    });

    // 3. The same window's pixels as *the window itself* paints them, against the file -
    // graded on a *share*, because this is the composited side of the claim: what the
    // desktop shows of a pin with alpha in it is the picture plus whatever was behind it.
    // The file's opaque pixels have to be there; its transparent ones are counted out, not
    // excused.
    //
    // P46 ④ moved the collection off the product's xcap leg and onto `PrintWindow`, for
    // the reason [`PRINT_FLAG`]'s doc gives: a success there can be a `BitBlt` of the
    // desktop behind the window, and on this machine it was not a success at all, so this
    // row had been `Blocked` in every archived run since it was written - the claim was
    // registered but never put. The old leg's answer is still printed next to the new one,
    // because a reader has to be able to see which leg moved rather than guess it from a
    // sentence that changed wording. The thresholds are the same two numbers.
    let old_leg = match crate::capture::service().capture_window(t.hwnd as u32) {
        Err(e) => e.to_string(),
        Ok(cap) => format!("{}x{} rect={}", cap.frame.width, cap.frame.height, cap.rect),
    };
    rows.push(
        match platform_windows::print::print_window(t.hwnd, t.phys.w, t.phys.h, PRINT_FLAG) {
            None => (
                Check::Blocked,
                "pin window pixels",
                format!(
                    "0x{:x} 这一腿（PrintWindow flag {PRINT_FLAG}）交回没有像素；产品那条腿（xcap）\
                     本轮答 {old_leg}",
                    t.hwnd
                ),
            ),
            Some(cap) => {
                let mut seen = 0usize;
                let mut matched = 0usize;
                if cap.width == from_disk.width && cap.height == from_disk.height {
                    for (px, file) in
                        cap.pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .zip(from_disk.pixels.as_chunks::<4>().0)
                    {
                        if file[3] == 255 {
                            seen += 1;
                            if px[..3]
                                .iter()
                                .zip(&file[..3])
                                .all(|(a, b)| a.abs_diff(*b) <= PIXEL_TOL)
                            {
                                matched += 1;
                            }
                        }
                    }
                }
                let share = if seen == 0 {
                    0.0
                } else {
                    matched as f64 / seen as f64
                };
                (
                    Check::from(seen > 0 && share >= PIXEL_SHARE),
                    "pin window pixels",
                    format!(
                        "窗 {}x{} 取自 {} 的 PrintWindow flag {PRINT_FLAG}（这一腿自己填的 alpha \
                         恒为 255，所以比的是文件那侧的不透明），文件里不透明的 {seen} px 中对上 \
                         {matched} px = {share:.3}（要求 ≥ {PIXEL_SHARE}，单通道容差 {PIXEL_TOL}）；\
                         产品那条腿（xcap）本轮答 {old_leg}",
                        cap.width, cap.height, t.class
                    ),
                )
            }
        },
    );
    rows
}

/// One file both doors were asked to write: QML's and the state machine's own,
/// compared byte for byte.
///
/// The Rust half runs here, after the event loop, on the same pin and with the same
/// `[output]` settings - so the two files can only differ if the doors differ, which
/// is the thing a QML-only reading cannot tell you.
fn both_doors(
    rep: &mut Report,
    label: &str,
    id: PinId,
    rust_path: &Path,
    qml_path: &Path,
    qml_said: Option<&str>,
    want: &str,
) {
    let rust_said = state::with(|s| s.save_image(id, &rust_path.to_string_lossy(), ""));
    let (qml_bytes, rust_bytes) = (read(qml_path), read(rust_path));
    let same = qml_bytes.is_some() && qml_bytes == rust_bytes;
    let verdict =
        Check::from(same && qml_said.is_some_and(|s| s.contains(want)) && rust_said.is_ok());
    rep.row(
        label,
        verdict,
        format!(
            "qml={} rust={} 字节一致={same}；qml 那句 {:?}；rust 那句 {:?}",
            bytes(&qml_bytes),
            bytes(&rust_bytes),
            qml_said,
            rust_said
        ),
    );
}

/// The two failure sentences, graded as the user sees them: this file's prefix and
/// [`crate::session`]'s together have to read as one Chinese line.
///
/// The English this row replaces was not in the prefix - `复制失败：` and `保存失败：`
/// were always Chinese - it was the clause the state machine answered with, which is
/// why the row checks both halves and why it is graded on the *composed* string.
fn never_drawn(rep: &mut Report, key: &str, label: &str, prefix: &str, lines: &[String]) {
    let said = said(lines, key);
    let ok = match said {
        // No Latin letter at all, rather than the absence of the one English clause
        // this row was written for: a sentence that grew a *different* English word
        // would have passed a `contains` test on the old one, and this row is graded on
        // what the user would have read.
        Some(s) => {
            s.starts_with(prefix)
                && s.contains("贴图")
                && !s.chars().any(|c| c.is_ascii_alphabetic())
        }
        None => false,
    };
    rep.row(
        label,
        Check::from(ok),
        format!("{said:?}（要求前缀 {prefix:?}、含「贴图」、不含拉丁字母）"),
    );
}

/// The check itself, after the event loop has run and QML has reported.
pub fn measure() -> (Check, String) {
    state::stamp("pin end");
    let mut rep = Report::new();
    // §10 第 41 项 ④: `--pin` never looks at the desktop (第 41 项 ⑧ is why its rows survive
    // a change of wallpaper), but a run of it alone is still a run inside some session, and
    // "which session" was the question this round could not answer from an archived log.
    crate::r13::scene_row(&mut rep);
    let (
        id,
        bad,
        qml_png,
        qml_jpg,
        rust_png,
        rust_jpg,
        never,
        dialog,
        dir,
        rect,
        lines,
        fid,
        frect,
        from_file,
        file_said,
    ) = with(|p| {
        (
            p.id,
            p.bad,
            p.qml_png.clone(),
            p.qml_jpg.clone(),
            p.rust_png.clone(),
            p.rust_jpg.clone(),
            p.never.clone(),
            p.dialog.clone(),
            p.dir.clone(),
            p.rect,
            p.lines.clone(),
            p.fid,
            p.frect,
            p.from_file.clone(),
            p.file_said.clone(),
        )
    });
    // Read in its own call rather than lengthening the one above: that one is already at
    // the arity a Rust tuple can be.
    let (xid, xrect) = with(|p| (p.xid, p.xrect));
    let (gid, grect, from_gif, gif_said) =
        with(|p| (p.gid, p.grect, p.from_gif.clone(), p.gif_said.clone()));

    // Every other row reads a value, so this one reads whether there is a value to
    // read: a document that failed to build, a delegate that never matched the plan,
    // or a timer that never armed all leave the same silence, and the rows below
    // would each report it as their own failure.
    let keys: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.split_once('|').map(|(k, _)| k))
        .collect();
    let order = leg_order(&keys, PIN_SCHEDULE);
    rep.row(
        "pin leg ran",
        Check::from(keys.contains(&"done") && order.is_none()),
        format!(
            "{} 行：{}{}",
            lines.len(),
            keys.join(" "),
            order
                .as_deref()
                .map(|p| format!("；{p}"))
                .unwrap_or_default()
        ),
    );
    // The run's own arithmetic, printed rather than trusted. Which of the two closers
    // ended the run is a fact read off `done`'s own interval and not off a total - see
    // [`LegTiming`], whose first reading of the ink leg got this wrong in this direction.
    rep.note(
        "量具计时",
        LegTiming::read(
            &LegStamps {
                arm: "pin arm",
                leg: "pin leg",
                done: "pin done",
                end: "pin end",
            },
            LEG_DELAY_MS,
            LEG_WAITS,
            STARTUP_GRACE_MS,
            MIN_MS,
        )
        .line(),
    );

    let want = rect.unwrap_or(PhysRect::new(0, 0, 1, 1));
    match pin_window(want) {
        None => rep.row(
            "pin window",
            Check::Fail,
            format!("没有一个可见顶层和贴图的 {want} 重叠"),
        ),
        Some(t) => {
            let close = within_slack(&t.phys, &want);
            rep.row(
                "pin window",
                Check::from(close),
                format!(
                    "顶层 {}x{}@{},{} 和要求的 {want} 相差在 {RECT_SLACK} px 内={close}（class={}）",
                    t.phys.w, t.phys.h, t.phys.x, t.phys.y, t.class
                ),
            );
        }
    }
    for (verdict, label, detail) in file_pin_rows(frect, fid, &from_file, &file_said) {
        rep.row(label, verdict, detail);
    }
    for (verdict, label, detail) in gif_rows(&lines, grect, gid, &gif_said, &from_gif) {
        rep.row(label, verdict, detail);
    }
    for (verdict, label, detail) in alpha_rows(&lines, id) {
        rep.row(label, verdict, detail);
    }
    // §5.10.1 的三问，读的是同一条腿的十三枚 `sel-*`；三枚判据各答各的，一枚结论也不并。
    for (verdict, label, detail) in sel_rows(&lines, id) {
        rep.row(label, verdict, detail);
    }
    // §5.10.2 的五问读的是**同一趟**的十五枚 `mov-*`：前十一枚长在多选那三腿中间（落点那一枚在
    // 整段最前），因为问的名单长度就是上面那两手跑出来的状态；后四枚走在 `sel-cleared-self` 之后，
    // 是反手把这一族自己挪走的落点一枚一枚还回去。五枚判据同样一枚也不并。
    for (verdict, label, detail) in mov_rows(&lines, id) {
        rep.row(label, verdict, detail);
    }
    // §5.10.3 的六问读的是**同一趟**的二十四枚 `op-*`：七枚单枚那格长在 `mov-rev-lone` 之后（那时
    // 名单里只有本窗一枚），三枚 ghost 长在 `mov-epoch-ghost` 之后，十四枚批量那格长在
    // `mov-batch-epoch` 之后（那时名单两枚）。六枚判据一枚也不并，理由与上面那封信同一枚。
    for (verdict, label, detail) in op_rows(&lines, id) {
        rep.row(label, verdict, detail);
    }
    // §5.10.4「重置」的四问读的是**同一趟**的十五枚 `rst-*`：八枚单枚那格长在 `op-lone-epoch-end`
    // 之后（那时名单里只有本窗一枚），三枚 ghost 长在 `op-epoch-ghost` 之后（同一枚还没被发出去
    // 的 id），四枚批量那格长在 `op-epoch-end` 之后（那时名单两枚）。四枚判据一枚也不并，理由与
    // 上面那两封信同一枚。
    for (verdict, label, detail) in rst_rows(&lines) {
        rep.row(label, verdict, detail);
    }
    // §5.9.18 / §5.10.4「销毁 + 撤销 + 影响数量」的五问读的是长在 `mov-pos-after` 之后、`menu`
    // 之前的十八枚 `des-*`：这一腿点的是**别的那些枚**，所以本窗能一路说到 `done`（第 55 项那一封
    // 同一枚理由：说话者与受害者必须是两扇窗）。五枚判据同样一枚也不并。
    for (verdict, label, detail) in des_rows(&lines, id) {
        rep.row(label, verdict, detail);
    }
    // §5.11 独显的七问读的是长在 `des-undo-again` 之后、`menu` 之前的三十四枚 `solo-*`：四臂各走
    // 一遍（留着本窗、留着别人、空名单上的一记热键、一枚场上没有的 id），起点正是销毁那一族收回来
    // 的那个状态——名单空、场上满。七枚判据一枚也不并，因为它们各是 §5.11 自己的一句话；`dim` 从
    // 配置现读而不是从 QML 那枚 `solo-dims` 取，理由写在 [`solo_rows`] 上面那封信里。
    for (verdict, label, detail) in
        solo_rows(&lines, id, crate::settings::pin_cfg().solo_dim_opacity)
    {
        rep.row(label, verdict, detail);
    }
    // §5.9.5–§5.9.8 的七问读的是长在 `solo-ghost-kept` 之后、`menu` 之前的四十枚 `xf-*`：
    // 那一族按的是**本窗自己**的六只手，所以要的起点正是独显收回来的那一个（名单空、场上满、
    // 本窗没被压着），而它自己也收梢——四枚开关各两下、旋转满一圈，后面读对话框的那几枚
    // 行因此一个字节也没被本族动过。七枚判据一枚也不并，理由写在 [`xf_rows`] 上面那封信里。
    for (verdict, label, detail) in xf_rows(&lines) {
        rep.row(label, verdict, detail);
    }
    // 这一格的开关读的是产品这一趟加载的那一枚，和 `copy_pin` 走的是同一条路。
    for (verdict, label, detail) in copy_rows(&lines, crate::settings::output_cfg().copy_as_file) {
        rep.row(label, verdict, detail);
    }

    rep.row(
        "pin menu item",
        Check::from(said(&lines, "menu") == Some("另存为…")),
        format!("{:?}（§5.9.17 的名字）", said(&lines, "menu")),
    );

    let filters: Vec<&str> = said(&lines, "filters")
        .map(|s| s.split(";;").collect())
        .unwrap_or_default();
    let offered = filters
        .iter()
        .zip(FILTERS)
        .all(|(got, want)| got.contains(want));
    rep.row(
        "pin name filters",
        Check::from(filters.len() == FILTERS.len() && offered),
        format!("{} 项 {filters:?}", filters.len()),
    );

    // The dialog's own state, read for the first time in P37. `selectedNameFilter`
    // is the one §10 第 37 项 ④ lists as having no reader in the product, so there is
    // no promise here to grade yet - only the number, which the first run showed is not
    // a string but a `QQuickFileNameFilter` (`name|extensions|index`).
    rep.note("filter selected", format!("{:?}", said(&lines, "filter")));
    rep.note(
        "currentFile 回读",
        format!(
            "赋值前 {:?} 赋值后 {:?}",
            said(&lines, "before"),
            said(&lines, "current")
        ),
    );

    // Whether the native dialog was actually on the screen when something pressed its
    // button. `dlg-open` is the same member read in the turn that called `open()` and
    // is kept as a note: whether the shell comes up inside the call or one loop turn
    // later is a fact about this Qt, not a promise the product makes.
    rep.note(
        "dlg visible 于 open()",
        format!("{:?}", said(&lines, "dlg-open")),
    );
    rep.row(
        "pin dialog opened",
        Check::from(said(&lines, "dlg-shown") == Some("true")),
        format!(
            "按之前 visible={:?}；currentFolder {:?} currentFile {:?}",
            said(&lines, "dlg-shown"),
            said(&lines, "dlg-folder"),
            said(&lines, "dlg-name")
        ),
    );

    let push = said(&lines, "dlg-push").unwrap_or("");
    let shown = push_row_text(push);
    // How many waits the press needed, and what each of the earlier ones saw. The count
    // is not a grade - a door that took three waits still opened - but it is the
    // difference between a press that was earned and one that was lucky, and the reasons
    // are what say whether the cap was reached rather than the window found.
    let tries = said(&lines, "dlg-push-tries").unwrap_or("<没有读数>");
    let asked = ask(said(&lines, "dlg-push"));
    rep.row(
        "pin dialog pushed",
        ask_check(asked),
        format!(
            "{shown}（{tries} 次尝试；要求 pushed=yes、class=[#32770]、bid=1）；三态读作：{}",
            ask_said(asked)
        ),
    );
    rep.note(
        "push 每次尝试",
        format!("原始 {push:?}；每次 {:?}", said(&lines, "dlg-push-log")),
    );

    // §5.5.11 步骤 3 在这一格里第一次对面是**一扇真窗**：`dlg-probe` 取在按下之前那一手的
    // 开头，而同一手随后就答出 `pushed=yes` —— 那一句要求本进程确有一扇 #32770、且它的
    // IDOK 真被点了。所以这枚矩形不是从"等了三次终于有了"推出来的：它读的就是按下那一手。
    //
    // 两格各问一件事，不并成一格。第一格只问"读到了没有"（#176 自 P123 起欠的就是这一枚证人）；
    // 第二格问"藏与放有没有在这扇窗上真走过"。开关关着时第二格答**问不了**而不是答通过：
    // 出厂 `false` 那一臂是 §5.5.11 步骤 1 的用户决定，不是这一格的成就。
    // 名字不叫 `rect`：这一层早已有一枚 `rect`（本趟那枚贴图自己的矩形，来自 `Plan`），而下面
    // `close_rows` 读的就是它。同名 shadow 会把那三格的几何判据换成一枚对话框的矩形，类型不同
    // 才被编译挡住 —— 挡住之前它是一枚会自己改判据的名字，所以这里从源头避开。
    let probe = said(&lines, "dlg-probe");
    let gone = said(&lines, "dlg-probe-gone");
    let dlg_rect = probe_rect(probe);
    let covers = probe_ids(probe_field(probe, "covering").as_deref());
    let hidden = probe_ids(probe_field(probe, "hidden").as_deref());
    let enabled = probe_field(probe, "enabled");
    let layers = probe_field(probe, "layers");
    rep.row(
        "pin dialog rect read",
        if asked != Ask::Pressed {
            Check::Blocked
        } else if dlg_rect.is_some() {
            Check::Pass
        } else {
            Check::Fail
        },
        format!(
            "按下之前现读 rect={:?}（要求四枚数、且面积非零）；probe 行 {probe:?}；press 答 {}；几何挑出 {:?} 而账上藏着 {:?}",
            dlg_rect,
            ask_said(asked),
            covers,
            hidden
        ),
    );
    let gone_layers = probe_field(gone, "layers");
    let gone_hidden = probe_ids(probe_field(gone, "hidden").as_deref());
    let hid_one =
        enabled.as_deref() == Some("1") && layers.as_deref() == Some("1") && !hidden.is_empty();
    let gave_back = gone_layers.as_deref() == Some("0") && gone_hidden.is_empty();
    rep.row(
        "pin dialog hide and restore",
        if enabled.as_deref() != Some("1") || probe.is_none() || gone.is_none() {
            Check::Blocked
        } else if hid_one && gave_back {
            Check::Pass
        } else {
            Check::Fail
        },
        format!(
            "开时 enabled={enabled:?} layers={layers:?} hidden={hidden:?}（要求 1、1、非空）；关后 layers={gone_layers:?} hidden={gone_hidden:?}（要求 0、空）；两行任一没读到、或那枚键关着 ⇒ 本格答\"问不了\"而不是答通过"
        ),
    );
    // 上面那一格读的是账，这一格读的是屏：同一枚贴图在自己窗里读到的 `view.live`，开时该是
    // `false`、关完该是 `true`，而这两者之间 `dialog_epoch` 得真动过。P123 起 #176 欠的正是
    // 这一枚证人——账能对平而屏幕上什么都没藏起来，是那一趟唯一没有被问到的那半。
    let (live_check, live_said_row) = dialog_live(&lines, enabled.as_deref(), &hidden);
    rep.row("pin dialog live per pin", live_check, live_said_row);

    // §10 第 41 项 ②: this door now reads its before-state the way the mask door has since
    // P39, because comparing "some sentence" against "another sentence" was never the claim.
    // `pre-save` is the product's own refusal (`never-save` above answers `保存失败：贴图 999999
    // 还没有画过…`), and the pair only counts while the two are still the same string - so a
    // leg that writes `session.status` between the refusal and the click reddens this row
    // instead of quietly changing what it compares. That is P40's defect, made mechanical.
    let was = said(&lines, "accept-was");
    let now = said(&lines, "accept").unwrap_or("<没有读数>");
    let pre = said(&lines, "pre-save");
    let shown = was.unwrap_or("<没有读数>");
    let fired = crate::annotate_check::before_is_products_own(was, pre)
        && now.starts_with("已保存到")
        && now != shown;
    rep.row(
        "pin dialog accept",
        if fired {
            Check::Pass
        } else if asked == Ask::Pressed {
            // The click happened and the sentence did not change: that is the door
            // failing to open, not a question this run is unable to ask.
            Check::Fail
        } else {
            Check::Blocked
        },
        format!(
            "真点击前后 status {shown:?} -> {now:?}；前态那句读自被拒的那次 savePin 之后的 {:?}（{}；{}）",
            pre.unwrap_or("<没有读数>"),
            if fired {
                "onAccepted 是那一下点击跑起来的"
            } else if asked == Ask::Pressed {
                "点了，但 onAccepted 没跑或前态不是产品那句"
            } else {
                "这一格问不了"
            },
            ask_said(asked)
        ),
    );
    // The other half of §10 第 39 项 ①'s asymmetry: the mask door has had this grade since
    // P38, while on this one the reading (`dlg-after`) travelled in the transcript as a note
    // only - so "the click closed the door" and "the leg closed it for her" were both
    // recorded and neither was judged. `dlg-dismiss` is the leg admitting it did the job the
    // click was supposed to have done.
    //
    // §10 第 47 项 ④ gives this row its third reading: a press the desktop never answered
    // cannot leave a door standing, and grading that as the gauge closing a door was one of
    // the four FAILs in 第 46 项 ⑥'s cascade.
    let (hy_check, hy_said) = dialog_hygiene(
        said(&lines, "dlg-shown"),
        said(&lines, "dlg-after"),
        keys.contains(&"dlg-dismiss"),
        said(&lines, "dlg-rejected").is_some(),
    );
    rep.row(
        "pin dialog closed",
        if hy_check == Check::Blocked {
            hy_check
        } else if said(&lines, "dlg-after") == Some("false") && !keys.contains(&"dlg-dismiss") {
            Check::Pass
        } else {
            Check::Fail
        },
        format!(
            "点完之后 visible {:?}，量具代关={}（未问 / 桌面不肯答 / 产品答错 三态读作：{}）",
            said(&lines, "dlg-after"),
            keys.contains(&"dlg-dismiss"),
            if hy_check == Check::Blocked {
                hy_said.as_str()
            } else {
                ask_said(asked)
            }
        ),
    );
    // The hygiene claim of its own row: whose window this phase left, said before the next
    // phase's census has to answer for it.
    rep.row(
        "pin dialog hygiene",
        hy_check,
        format!(
            "{hy_said}；本段的三次读数 dlg-shown {:?} / dlg-after {:?} / dlg-rejected {:?}",
            said(&lines, "dlg-shown"),
            said(&lines, "dlg-after"),
            said(&lines, "dlg-rejected")
        ),
    );
    // The pair that P37 measured, kept as a reading rather than a grade: what a call
    // to `accept()` on a dialog that was never shown does. §10 第 38 项 ①'s point is
    // that this is not the same question as the row above, and the two now have two
    // pairs of keys.
    rep.note(
        "未 open() 就 accept()",
        format!(
            "{:?} -> {:?}",
            said(&lines, "call-accept"),
            said(&lines, "call-after")
        ),
    );

    // The door's own spelling. `onAccepted` hands over URL text, so this is the row
    // that says whether that text becomes the file the plan computed - or a *relative*
    // path whose first component is `file:`, which is what P37 found: both product
    // doors were calling a `toLocalFile()` this Qt does not give JS, so a dialog
    // accept threw before it wrote anything at all.
    let url = said(&lines, "png-url").unwrap_or("");
    let landed = PathBuf::from(crate::save::local_path(url).into_owned());
    let sentence = said(&lines, "png").unwrap_or("");
    rep.row(
        "pin url is a path",
        Check::from(
            url.starts_with("file:///")
                && landed == qml_png
                && sentence.contains(&qml_png.to_string_lossy().to_string()),
        ),
        format!("{url:?} 读成 {landed:?}，要求 {qml_png:?}；那句 {sentence:?}"),
    );

    // The two writes, through the door's own call. `want` is the clause every
    // sentence in `save` is built with: the subject word the caller handed it, so a
    // pin that says 选区 here would be the second door quoting the first one.
    both_doors(
        &mut rep,
        "pin save via qml",
        id,
        &rust_png,
        &qml_png,
        said(&lines, "png"),
        "已保存到",
    );
    both_doors(
        &mut rep,
        "pin save jpg via qml",
        id,
        &rust_jpg,
        &qml_jpg,
        said(&lines, "jpg"),
        "贴图 ",
    );
    rep.note(
        "jpg 透明提示",
        format!("{:?}", said(&lines, "jpg").unwrap_or("")),
    );

    // Which format a name *without* an extension takes, at this door (§10 第 40 项 ②).
    // Every arm of the menu gets a leg against the same bare stem, because the config these
    // runs is pinned to says `format = "png"`: one PNG leg would grade the setting and call
    // it the filter, and a door that dropped the filter on the way to `save::write` would
    // pass it. The configured format is printed beside the legs so the reading says which
    // of the three were redundant on this machine.
    //
    // Two independent readings, not one: the file's first four bytes name the encoder that
    // actually ran (a PNG wearing a `.jpg` name is the failure this row is out to catch, and
    // the name alone cannot see it), and the count says nothing else appeared - a door that
    // wrote every candidate would satisfy a row that only looked for the one it expected.
    let configured = crate::settings::export().format;
    let candidates: Vec<PathBuf> = FILTERS
        .iter()
        .copied()
        .filter_map(crate::save::filter_format)
        .map(|f| dir.join(format!("filtered.{}", f.ext())))
        .collect();
    let mut legs = Vec::new();
    let mut agreed = candidates.len() == FILTERS.len();
    for &filter in &FILTERS {
        for stale in &candidates {
            let _ = std::fs::remove_file(stale);
        }
        let bare = dir.join("filtered");
        let said = state::with(|s| s.save_image(id, &bare.to_string_lossy(), filter));
        let Some(want) = crate::save::filter_format(filter) else {
            agreed = false;
            legs.push(format!("{filter} 认不出格式"));
            continue;
        };
        let target = dir.join(format!("filtered.{}", want.ext()));
        let wrote = crate::save::written_as(&target);
        let only = candidates.iter().filter(|p| p.exists()).count();
        let ok = wrote == Some(want)
            && only == 1
            && said
                .as_ref()
                .is_ok_and(|s| s.contains(&format!("filtered.{}", want.ext())));
        agreed &= ok;
        legs.push(format!(
            "{filter}→{} 头字节={wrote:?} 只这一份={}{}",
            want.ext(),
            only == 1,
            if ok {
                String::new()
            } else {
                format!("；那句 {said:?}")
            }
        ));
    }
    rep.row(
        "pin filter decides format",
        Check::from(agreed),
        format!(
            "配置 {:?}；{}",
            configured.ext(),
            if legs.is_empty() {
                "没有一腿跑".to_string()
            } else {
                legs.join(" ; ")
            }
        ),
    );

    // The filter text has to *cross* the boundary, which nothing above can see: the Rust
    // legs call the state machine directly, and the real-click row is handed a name the
    // shell already suffixed, so a `savePin` whose third argument QML never passes would
    // leave every row in this report green. This leg calls the door exactly the way
    // `onAccepted` does - a bare URL and one of the dialog's own filter texts - and reads
    // the encoder off the file that appears.
    let filtered = dir.join(format!("{DIALOG_STEM}-f.jpg"));
    let leg = said(&lines, "f-jpg").unwrap_or("");
    let wrote = crate::save::written_as(&filtered);
    let arm = FILTERS[1];
    rep.row(
        "pin filter via qml",
        Check::from(
            leg.starts_with(arm)
                && leg.contains("已保存到")
                && leg.contains("-f.jpg")
                && wrote == Some(falcon_core::encode::Format::Jpg),
        ),
        format!(
            "QML 那一条 {leg:?}（要求以 {arm:?} 开头）；{} 的头四字节读成 {wrote:?}",
            filtered.display()
        ),
    );

    // The one file this run did not name. The dialog was handed a folder and a stem and
    // chose the extension off its own filter list - measured in P38, so this row is the
    // grade of that measurement rather than an assumption of it - and it is the only
    // write in this report whose every step came from the door: the signal fired by a
    // `BM_CLICK`, `onAccepted` read `selectedFile`, and the product's own `savePin` put
    // the bytes there.
    let dlg_bytes = read(&dialog);
    let same_as_rust = dlg_bytes.is_some() && dlg_bytes == read(&rust_png);
    let dlg_url = said(&lines, "dlg-accepted").unwrap_or("");
    let dlg_landed = PathBuf::from(crate::save::local_path(dlg_url).into_owned());
    // The name the dialog wrote, printed separately from the folder: the stem was the
    // gauge's and the extension was the shell's choice, so the last component is the
    // only part of this path that was not decided before the click.
    let dlg_name = dlg_landed
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    rep.row(
        "pin save via real click",
        Check::from(
            same_as_rust && same_file(&dlg_landed, &dialog) && now.starts_with("已保存到"),
        ),
        format!(
            "对话框写 {dlg_name:?} 与 rust 那份字节一致={same_as_rust}；{dlg_url:?} 读成 {dlg_landed:?}，要求 {dialog:?}；那句 {now:?}"
        ),
    );
    // The filter has a reader now. §10 第 37 项 ④ listed `selectedNameFilter` as a value
    // with nothing that consumed it; the extension on the file above is that consumer,
    // so the three readings of the member are printed together - before any dialog was
    // shown, while one was up, and after the click closed it.
    rep.note(
        "filter 三次读数",
        format!(
            "未开 {:?} 开着 {:?} 关上 {:?}",
            said(&lines, "filter"),
            said(&lines, "dlg-shown-filter"),
            said(&lines, "dlg-filter")
        ),
    );
    rep.note(
        "dlg visible 于尾",
        format!("{:?}", said(&lines, "dlg-after")),
    );
    rep.note("dlg 信号那句", format!("{dlg_url:?}"));

    never_drawn(
        &mut rep,
        "never-copy",
        "pin copy undrawn",
        "复制失败：",
        &lines,
    );
    never_drawn(
        &mut rep,
        "never-save",
        "pin save undrawn",
        "保存失败：",
        &lines,
    );
    // The refusal is supposed to cost nothing: `save_image` answers before the
    // encoder is reached, so a file at that name means the order is wrong.
    rep.row(
        "undrawn wrote nothing",
        Check::from(!never.exists()),
        format!("{} 存在={}", never.display(), never.exists()),
    );

    for (verdict, label, detail) in close_rows(&lines, rect, xrect, frect, grect) {
        rep.row(label, verdict, detail);
    }

    // QML's voice, and the state machine's own list of complaints - both shared with
    // the mask gauge, because the shape of the silence is the same.
    crate::mask_check::qml_row(&mut rep);
    let problems = state::with(|s| s.problems.clone());
    rep.row(
        "no problems",
        Check::from(problems.is_empty()),
        problems.join(" | "),
    );

    let (verdict, text) = rep.finish("pin check");
    // The id that was never planted, printed with the report rather than inside a
    // row: a run whose leg stopped early is read from which keys arrived, and the
    // plan that should have produced them is the rest of the context.
    (
        verdict,
        format!("{text}\npin plan: id={id} bad={bad} rect={want} close={xid} gif={gid}"),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        alpha_num, alpha_rows, alpha_turn, alpha_yes, ask, ask_check, ask_said,
        census_blames_earlier_phase, close_rows, copy_rows, des_rows, dialog_hygiene, dialog_live,
        file_pin_rows, gif_rows, leg_order, live_said, mov_rows, op_rows, probe_field, probe_ids,
        probe_rect, qml_leg_keys, real_click, rst_rows, said, same_file, sel_rows, solo_rows,
        state, unasked_rows, xf_rows, Ask, Check, PhysRect, PinId, ALPHA_MENU_ITEMS, ALPHA_ROWS,
        CLOSE_ROWS, COPY_ROWS, DES_ROWS, FILE_PIN_ROWS, GIF_ARM_MS, GIF_MENU_ITEMS, GIF_PIN_ROWS,
        GIF_STILL_MS, LEG_DELAY_MS, MOV_ROWS, OP_ROWS, PIN_SCHEDULE, RST_ROWS, SEL_ROWS, SOLO_ROWS,
        XF_KEYS, XF_LABELS, XF_ROWS,
    };
    use std::path::Path;

    /// The keys are the entire protocol between QML and this file, so one key must not
    /// be read as another that merely starts the same way: a row grading the wrong
    /// line is a number with someone else's name on it, which is the shape of finding
    /// this mode is here to close out.
    #[test]
    fn a_reported_key_matches_whole_not_as_a_prefix() {
        let lines = vec![
            "没有竖线".to_string(),
            "png2|甲".to_string(),
            "jpg|乙".to_string(),
        ];
        assert_eq!(said(&lines, "png"), None);
        assert_eq!(said(&lines, "png2"), Some("甲"));
        assert_eq!(said(&lines, "jpg"), Some("乙"));
    }

    /// §10 第 38 项 ① grades one string: what the shim said it pressed. `pushed=yes`
    /// alone would let this pass on *any* button of *any* window the walk happened to
    /// like, so the three claims the row is about - a real `#32770`, the control id the
    /// shell's own default button carries, and the report naming the dialog - each have
    /// to be in the line.
    ///
    /// The controls are the two ways that shape fails for real, not strawmen: another
    /// program's dialog, and a dialog whose default button is not the one asked for.
    #[test]
    fn only_a_default_button_of_a_real_save_dialog_counts_as_a_click() {
        let measured = "pushed=yes dialog=0x1a2c class=[#32770] title=[另存为] dialogs=1 \
                        desc=50 buttons=2 button=0x3c4d btitle=[&Save] bid=1 ret=0 visible_after=1";
        assert!(real_click(measured), "{measured}");

        // Nothing was pressed, because nothing of this process was on the screen.
        let none = "pushed=no reason=no-visible-#32770-of-this-pid dialogs=0";
        assert!(!real_click(none), "{none}");

        // A button found in a window that is not a file dialog - the walk could report
        // `pushed=yes` here and the run would have clicked something else entirely.
        let wrong_window = "pushed=yes dialog=0x9 class=[Qt652QWindowIcon] title=[] \
                            dialogs=1 desc=3 buttons=1 button=0xa btitle=[OK] bid=1 ret=0 visible_after=1";
        assert!(!real_click(wrong_window), "{wrong_window}");

        // The right class with the wrong control: `bid=1` is what makes it the default
        // button, and a dialog can hold several `Button`s.
        let wrong_button = "pushed=yes dialog=0x1 class=[#32770] title=[另存为] \
                            dialogs=1 desc=50 buttons=2 button=0x2 btitle=[取消] bid=2 ret=0 visible_after=1";
        assert!(!real_click(wrong_button), "{wrong_button}");

        // The failure sentence the shim writes when the dialog has no enabled `IDOK`.
        let no_button =
            "pushed=no reason=no-enabled-idok-button dialogs=1 desc=50 buttons=2 title=[另存为]";
        assert!(!real_click(no_button), "{no_button}");
    }

    /// §10 第 40 项 ⑤'s other half. A judgement row's text is the thing two ladder
    /// archives are `diff`ed, so a field the shell changes between runs makes the archive
    /// unreadable - and one round of `--pin` / `--ink` produced `desc=50` and `desc=48`
    /// from the *same* binary. Two things are checked, in this order: the count is gone
    /// from the row text, and the grade is bit-for-bit the grade the raw line got, because
    /// a normalisation that could flip a verdict would be a new judgement wearing the old
    /// row's name.
    #[test]
    fn a_push_row_says_nothing_the_shell_decides_on_its_own() {
        use super::push_row_text;
        let raw = "pushed=yes dialog=0x4c134e class=[#32770] title=[另存为] dialogs=1 \
                   desc=50 buttons=4 button=0x3c4d btitle=[&Save] bid=1 ret=0 visible_after=1";
        let row = push_row_text(raw);
        assert!(!row.contains("desc="), "{row}");
        assert!(
            row.contains("pushed=yes") && row.contains("class=[#32770]") && row.contains("bid=1"),
            "{row}"
        );
        assert_eq!(real_click(raw), real_click(&row));
        assert!(real_click(&row), "{row}");

        // The field is a token of its own, so removing it cannot eat a neighbour: a
        // report that failed stays failed with the count taken out.
        let failed = "pushed=no reason=no-enabled-idok-button dialogs=1 desc=50 buttons=2";
        assert_eq!(
            push_row_text(failed),
            "pushed=no reason=no-enabled-idok-button dialogs=1 buttons=2"
        );
        assert!(!real_click(&push_row_text(failed)));

        // Nothing to remove, nothing changed - the shape a `pushed=no` first-try line has.
        assert_eq!(push_row_text("dialogs=0"), "dialogs=0");
    }

    /// The name the dialog hands back is its own writing, so this one comparison folds
    /// case and separators. It must still be able to say no: the control below is the
    /// difference between the PNG the plan asked for and a JPG with somebody else's
    /// pixels in it.
    #[test]
    fn a_dialog_can_spell_the_name_it_was_given_differently_but_not_differently_enough() {
        let want = Path::new("C:\\Users\\a\\Temp\\falconshot-pin-gauge\\dialog.png");
        assert!(same_file(
            Path::new("c:/users/a/temp/falconshot-pin-gauge/dialog.png"),
            want
        ));
        assert!(!same_file(
            Path::new("C:\\Users\\a\\Temp\\falconshot-pin-gauge\\dialog.jpg"),
            want
        ));
        // An empty reading is a leg that never spoke, not a file with a short name.
        assert!(!same_file(Path::new(""), want));
    }

    /// §10 第 41 项 ②, as a name a test can hit. The sequence below is copied out of
    /// `p40_pin1500.log`'s own `pin leg ran` row - not invented - because the defect that
    /// round registered is exactly a schedule change that no row could see: `f-jpg` ran
    /// between the refusal and the click's before-reading, so `pin dialog accept` stayed
    /// green while comparing two file names instead of two sentences. Counting rows cannot
    /// say that; the relative order can.
    #[test]
    fn last_rounds_pin_order_is_out_of_order_by_this_rounds_schedule() {
        let p40 = [
            "menu",
            "filters",
            "filter",
            "before",
            "current",
            "call-accept",
            "call-after",
            "png-url",
            "png",
            "jpg",
            "never-copy",
            "never-save",
            "f-jpg",
            "dlg-folder",
            "dlg-name",
            "dlg-open",
            "dlg-shown",
            "dlg-shown-filter",
            "accept-was",
            "dlg-push",
            "dlg-push-tries",
            "dlg-push-log",
            "dlg-accepted",
            "accept",
            "dlg-filter",
            "dlg-after",
            "done",
        ];
        let problems = leg_order(&p40, PIN_SCHEDULE).unwrap_or_default();
        assert!(problems.contains("f-jpg"), "{problems}");
        assert!(problems.contains("日程不符"), "{problems}");
    }

    /// The order this round's QML writes, which has to be the schedule's own order: the
    /// row that grades the run is only worth having if the plan it grades against is the
    /// plan the door follows.
    #[test]
    fn the_pin_schedule_orders_its_own_legs() {
        let p41: Vec<&str> = PIN_SCHEDULE
            .iter()
            .filter(|k| **k != "dlg-dismiss" && **k != "dlg-rejected")
            .copied()
            .collect();
        assert_eq!(leg_order(&p41, PIN_SCHEDULE), None, "{p41:?}");
    }

    /// A key given two places in the schedule is invisible to every reader of it, and this
    /// round met one: `"gif-items-still"` had been left next to the entry it was renamed from,
    /// so the array held it twice. Nothing reddened. `leg_order` asks for a *non-decreasing*
    /// index, so a repeat can only ever be satisfied; the two set-comparisons that check the
    /// schedule against the QML are membership tests, so both halves stay empty; and
    /// `position()`, which is what the ordering assertions below use to name a leg's place,
    /// answers the first index and keeps quiet about the second. The claim a duplicate breaks
    /// is not "did this run follow a plan" - it is *which* plan, and the row that grades the
    /// run reads its schedule from here.
    #[test]
    fn no_leg_key_is_given_two_places_in_the_schedule() {
        let dups_of = |keys: &[&str]| -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for (i, key) in keys.iter().enumerate() {
                if keys[..i].contains(key) {
                    out.push((*key).to_string());
                }
            }
            out
        };
        // The counter-example, in the same breath as the claim: a yard that can only answer
        // empty is not grading anything, so the shape it is meant to catch is run through it.
        assert_eq!(
            dups_of(&["a", "b", "a"]).join(","),
            "a",
            "尺抓不住一枚重名，就谈不上放行"
        );
        let found = dups_of(PIN_SCHEDULE);
        assert!(
            found.is_empty(),
            "PIN_SCHEDULE 里 {found:?} 被给了两枚落点：`leg_order` 只认非降、`position()` 只报第一枚，所以两枚落点没有一位读者会发现"
        );
    }

    /// C 档那两枚开着的计数只在其毫秒数成立时才是日程：同一事件循环步里到期的两枚定时器，
    /// 谁先跑没人能从源码读出来，所以 [`PIN_SCHEDULE`] 声称的每一个先后都要由算术供。下面这三
    /// 条不等式就是那个「由」，读的是常数而不是一趟运行——运行只能说出发生了什么，说不出保证。
    #[test]
    fn the_two_menu_legs_never_share_an_event_loop_step() {
        // 三枚先后一起交回，而不是三枚 `assert!`：clippy 认得常数的比较
        // （`assertions_on_constants`，p128c 那一代第 2 格就是红在这一枚上），而这一格要的读数
        // 是「三条各成不成立」，数组自己把那三枚都印得出来。
        let holds = [
            GIF_ARM_MS < GIF_STILL_MS,
            GIF_STILL_MS + GIF_STILL_MS < LEG_DELAY_MS,
            GIF_ARM_MS + LEG_DELAY_MS > 2 * GIF_STILL_MS,
        ];
        assert!(
            holds.iter().all(|h| *h),
            "臂 {} / 静置 {} / 一整 delay {}：三条先后 = {:?}（臂要早于静置、静置那两枚要早于它自己的 `menu`、会动那一扇的尾读要晚于静置那两枚）",
            GIF_ARM_MS,
            GIF_STILL_MS,
            LEG_DELAY_MS,
            holds
        );
        let at = |k: &str| PIN_SCHEDULE.iter().position(|s| *s == k).unwrap();
        assert!(at("gif-items-closed") < at("gif-items-still-closed"));
        assert!(at("gif-items-still-closed") < at("gif-items-still"));
        assert!(at("gif-items-still") < at("menu"));
        assert!(at("gif-items-still") < at("gif-items"));
    }

    /// Two things the grade must not do: fail a run for the two legs that are confessions
    /// rather than phases, and let a leg through that nobody gave a place to.
    #[test]
    fn an_unregistered_leg_costs_the_row_and_an_absent_optional_one_costs_nothing() {
        let mut legs: Vec<&str> = PIN_SCHEDULE.to_vec();
        assert_eq!(leg_order(&legs, PIN_SCHEDULE), None);
        // The click closed the door, so the leg that would have admitted closing it stayed
        // silent, and the one that would have reported its signal did too.
        legs.retain(|k| *k != "dlg-dismiss" && *k != "dlg-rejected");
        assert_eq!(leg_order(&legs, PIN_SCHEDULE), None);

        legs.push("bmp-arm");
        let problems = leg_order(&legs, PIN_SCHEDULE).unwrap_or_default();
        assert!(problems.contains("日程里没有这条腿：bmp-arm"), "{problems}");

        // A key the same leg writes twice is one leg answering twice, not a reorder.
        let twice = ["accept-was", "accept", "accept"];
        assert_eq!(leg_order(&twice, PIN_SCHEDULE), None);

        // P134：两枚 `sel-observe-*` 是**别的窗**说的话，同一趟里销毁那一族换三次名单就说三遍，
        // 而它们在日程里只占一格——按那一格的先后要求它们必然假红（门档 A 那 5 句「自己说亮了」
        // 而场上只有 4 枚就是这么来的）。豁免只给这两枚名字，日程里照旧得有它们。
        let mut echo = PIN_SCHEDULE.to_vec();
        echo.push("sel-observe-on");
        echo.push("sel-observe-off");
        echo.push("sel-observe-on");
        assert_eq!(leg_order(&echo, PIN_SCHEDULE), None, "{echo:?}");

        // 反证（#274）：真腿前后挪一格照样红；证人换了个名字照样「日程里没有这条腿」。
        let mut moved = PIN_SCHEDULE.to_vec();
        let at = moved.iter().position(|k| *k == "des-trashed-kept").unwrap();
        let key = moved.remove(at);
        moved.insert(at - 3, key);
        let problems = leg_order(&moved, PIN_SCHEDULE).unwrap_or_default();
        assert!(problems.contains("相对顺序和日程不符"), "{problems}");
        let problems = leg_order(&["des-batch", "sel-observed"], PIN_SCHEDULE).unwrap_or_default();
        assert!(
            problems.contains("日程里没有这条腿：sel-observed"),
            "{problems}"
        );
    }

    /// The other half of ②(b), and the half that reaches the file that actually runs the
    /// legs: the schedule above is only this round's reading if `PinWindow.qml` still has
    /// the legs it names. Not their order - P41's probe is the reason that claim is gone, and
    /// [`qml_leg_keys`] says why a scan of the source cannot make it. What a scan can settle
    /// is the set, which is what a rename or a deleted leg breaks, and it breaks quietly: the
    /// door stops writing the key, the row reports fewer lines, and only someone comparing
    /// this round's count with last round's notices. So both directions are graded here.
    #[test]
    fn the_pin_qml_still_emits_exactly_the_scheduled_legs() {
        let keys = qml_leg_keys(include_str!("../qml/PinWindow.qml"));
        let missing: Vec<&str> = PIN_SCHEDULE
            .iter()
            .filter(|k| !keys.contains(&k.to_string()))
            .copied()
            .collect();
        let extra: Vec<&str> = keys
            .iter()
            .filter(|k| !PIN_SCHEDULE.contains(&k.as_str()))
            .map(|k| k.as_str())
            .collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "日程与 PinWindow.qml 的腿对不上：日程有而 QML 不写 {missing:?}；QML 写而日程没有 {extra:?}"
        );
    }

    /// §5.9.14's number of menu items, checked against the file that draws them rather than
    /// against this round's memory of them. The graded row compares a *live window's* count
    /// against [`GIF_MENU_ITEMS`], so the constant needs a reader that notices when the menu
    /// changes: without this one, adding an eighth item reddens a passing run from the
    /// desktop, and deleting one turns a red row into a green one - both are the row grading
    /// a number that no longer describes anything.
    ///
    /// The count is taken off the section's own visibility condition, which is the one thing
    /// every item in it shares, so this test cannot pass by counting something else.
    #[test]
    fn the_clip_section_of_the_menu_is_the_size_the_row_expects() {
        let src = include_str!("../qml/PinWindow.qml");
        let seen = src.matches("visible: win.view.gif_frames > 1").count();
        assert_eq!(
            seen, GIF_MENU_ITEMS,
            "PinWindow.qml 里 §5.9.14 那一节有 {seen} 项，而 GIF_MENU_ITEMS 写的是 {GIF_MENU_ITEMS}"
        );
    }

    /// The same guard for §5.9.15's four rows, taken off the handler every one of them
    /// shares rather than off the section's `visible:` condition — because that condition is
    /// the *separator's*, and a count that swept the separator in would say five. A gauge leg
    /// calls `win.view.setAlphaBg(` too, and does not `onTriggered:` it, so this pattern is
    /// the menu and nothing else.
    #[test]
    fn the_background_section_of_the_menu_is_the_size_the_row_expects() {
        let src = include_str!("../qml/PinWindow.qml");
        let seen = src.matches("onTriggered: win.view.setAlphaBg(").count();
        assert_eq!(
            seen, ALPHA_MENU_ITEMS,
            "PinWindow.qml 里 §5.9.15 那一节有 {seen} 项，而 ALPHA_MENU_ITEMS 写的是 {ALPHA_MENU_ITEMS}"
        );
    }

    /// §5.9.16 step 2 names two doors to one thing, and the sentence is only worth
    /// printing because the file says it in exactly one place each: one `copyPin` call
    /// site (inside the `Action`) and one `Shortcut` bound to that same action. A
    /// duplicated `onTriggered` would make "两条腿走的是同一条路" false while every other
    /// row still passed, so this row reads the QML rather than this round's memory of it.
    ///
    /// 反证同一把尺的另一面（#274/#461）：needle 是从本轮真的写进 QML 的那三行取的，若其中
    /// 任一行被改名或删掉，这一格读回的是 0 而不是"没这一问"。
    /// 四枚 needle 各数一枚不同的东西，而且每一枚都是**整行**而不是前缀：`never-copy` 那一腿
    /// 早就写着 `win.session.copyPin(Number(win.gauge[5]))`，只数 `win.session.copyPin(` 会把它
    /// 也算成一条路，于是 p130a 那一代读回 2 并报红——红得对，但红的是尺而不是门（#274 的
    /// 另一面：一把会读多的尺和一把会读空的尺都得先证明自己能读准）。
    #[test]
    fn the_copy_has_one_route_and_two_doors_in_the_qml() {
        let src = include_str!("../qml/PinWindow.qml");
        let routes = src.matches("win.session.copyPin(win.pinId)").count();
        let shortcuts = src.matches("sequences: [StandardKey.Copy]").count();
        let fired = src.matches("onActivated: copyAction.trigger()").count();
        let bound = src.matches("MenuItem { action: copyAction }").count();
        assert_eq!(
            routes, 1,
            "复制的落点在 PinWindow.qml 里只该有一处（菜单与 Ctrl+C 共走一条路），读到 {routes} 处"
        );
        assert_eq!(
            shortcuts, 1,
            "Ctrl+C 那一支路只该出现一次，读到 {shortcuts} 处"
        );
        assert_eq!(
            fired, 1,
            "快捷键点的必须就是菜单那枚 Action，读到 {fired} 处"
        );
        assert_eq!(
            bound, 1,
            "菜单那一项必须 `action:` 到同一枚 Action（自己再写一遍 onTriggered 就成了第二条路），读到 {bound} 处"
        );
    }

    /// 同一把尺的四面（#274/#461）：开关开着那一侧「点了文件、文件在自家目录下、此刻在盘上」
    /// 才绿，关着那一侧「不许点文件」才绿——**同一句话在两侧得到两个不同的答案**，这就是这枚行
    /// 真的在读那枚开关而不是读它自己。三枚红各自只有一个理由（目录外、不在盘上、开了却没点），
    /// 所以红了以后能一眼看出是哪一枚谓词没成立。
    ///
    /// 两枚 witness 文件写进 `PinState::clip_dir()` 与系统临时目录根，各自 `remove_file` 之后
    /// 才断言——判完就收，绝不让测试在产品的目录下留一枚没有读者的文件（那一枚目录归本轮门档
    /// `[clip]` 那一腿数，测试不该替它添数）。
    #[test]
    fn the_copy_row_answers_differently_for_each_side_of_the_switch() {
        let legs = |s: &str| vec![format!("copy-drawn|{s}")];
        let at = |rows: &[(Check, &'static str, String)], i: usize| rows[i].0;

        let dir = state::PinState::clip_dir();
        let _ = std::fs::create_dir_all(&dir);
        let ours = dir.join("p130-copy-row-witness.png");
        let outside = std::env::temp_dir().join("p130-copy-row-outside.png");
        let wrote = std::fs::write(&ours, b"witness").is_ok()
            && std::fs::write(&outside, b"witness").is_ok();
        let named = format!("已复制贴图（16 像素），文件引用 {}", ours.display());
        let elsewhere = format!("已复制贴图（16 像素），文件引用 {}", outside.display());
        let gone = format!(
            "已复制贴图（16 像素），文件引用 {}",
            dir.join("p130-copy-row-never-written.png").display()
        );
        let plain = "已复制贴图（16 像素）".to_string();
        let refused = "复制失败：贴图 4294967295 还没有画过".to_string();

        let on = copy_rows(&legs(&named), true);
        let off = copy_rows(&legs(&plain), false);
        let on_named_elsewhere = copy_rows(&legs(&elsewhere), true);
        let on_gone = copy_rows(&legs(&gone), true);
        let on_without_file = copy_rows(&legs(&plain), true);
        let off_with_file = copy_rows(&legs(&named), false);
        let refused_on = copy_rows(&legs(&refused), true);
        let silent = copy_rows(&[], true);

        let _ = std::fs::remove_file(&ours);
        let _ = std::fs::remove_file(&outside);

        assert!(wrote, "两枚 witness 写不下去，这一格的四面都无从判起");
        for rows in [&on, &off, &on_named_elsewhere, &silent] {
            let names: Vec<&str> = rows.iter().map(|(_, n, _)| *n).collect();
            assert_eq!(names, COPY_ROWS.to_vec(), "{names:?}");
        }
        assert_eq!(at(&on, 0), Check::Pass, "确认句那一行");
        assert_eq!(
            at(&on, 1),
            Check::Pass,
            "开着且点了自家的文件：{:#?}",
            on[1].2
        );
        assert_eq!(at(&off, 0), Check::Pass, "关掉那一侧的确认句");
        assert_eq!(
            at(&off, 1),
            Check::Pass,
            "开着没点文件是红，关着没点文件才是绿：{:#?}",
            off[1].2
        );
        assert_eq!(
            at(&on_named_elsewhere, 1),
            Check::Fail,
            "目录外那枚必须红，且只因为目录"
        );
        assert!(on_named_elsewhere[1].2.contains("在那枚目录下=false"));
        assert!(on_named_elsewhere[1].2.contains("此刻在盘上=true"));
        assert_eq!(at(&on_gone, 1), Check::Fail, "点了却没在盘上必须红");
        assert_eq!(
            at(&on_without_file, 1),
            Check::Fail,
            "开关开着而这句话没点文件：红"
        );
        assert_eq!(
            at(&off_with_file, 1),
            Check::Fail,
            "开关关着却点了文件：红，同一句话在两侧的答案相反"
        );
        assert_eq!(at(&refused_on, 0), Check::Fail, "拒绝句不是确认句");
        assert_eq!(
            at(&refused_on, 1),
            Check::Fail,
            "复制没成功时那一枚文件行也不许绿"
        );
        for (grade, name, detail) in &silent {
            assert_eq!(*grade, Check::Blocked, "{name} 没被问过，不该算红");
            assert!(!detail.is_empty(), "{name} 那一格连为什么都没说");
        }
    }

    /// 第 42 项 ② for §5.9.15's two rows: the tail leg that never fired asks *both*
    /// questions, and both are printed as their own `Blocked` line. The two readings are one
    /// function on one turn, so a run missing one is a run missing the other, and the row
    /// that says which leg stayed shut is the row that keeps a stopped run from being read as
    /// a product that has no background modes.
    #[test]
    fn a_run_that_never_opened_the_menu_still_names_both_alpha_rows() {
        let rows = alpha_rows(&[], 0);
        let names: Vec<&str> = rows.iter().map(|(_, name, _)| *name).collect();
        assert_eq!(names, ALPHA_ROWS.to_vec(), "{names:?}");
        for (grade, name, detail) in &rows {
            assert_eq!(*grade, Check::Blocked, "{name} 没被问过，不该算红");
            assert!(!detail.is_empty(), "{name} 那一格连为什么都没说");
        }
    }

    /// `alpha-turn` is three readings inside one line, cut on their own labels, so the
    /// parser is the one place in this row that can mis-read a leg that ran. A line missing a
    /// segment, or with the segments in another order, answers `None` — which the row prints
    /// as its own broken-instrument red rather than as a pin whose mode did not move.
    #[test]
    fn the_turned_reading_is_cut_by_its_own_labels() {
        let good = "found=0;took=true;\
            board=holes=true;items=4;marked=1;mode=2;rev=7;hole=true;body=true;\
            cleared=true;\
            transparent=holes=true;items=4;marked=1;mode=0;rev=8;hole=false;body=true;\
            restored=true;after=holes=true;items=4;marked=1;mode=0;rev=8;hole=false;body=true";
        let (took, cleared, restored, board, transparent, after) =
            alpha_turn(Some(good)).expect("这一行是那一腿现下写的形状");
        assert!(took && cleared && restored);
        assert_eq!(alpha_num(&board, "mode"), Some(2), "{board:?}");
        assert_eq!(alpha_yes(&board, "hole"), Some(true));
        assert_eq!(alpha_num(&board, "rev"), Some(7));
        assert_eq!(alpha_yes(&transparent, "hole"), Some(false));
        assert_eq!(alpha_num(&transparent, "rev"), Some(8));
        assert_eq!(alpha_num(&after, "mode"), Some(0));
        assert_eq!(alpha_yes(&after, "holes"), Some(true));

        assert_eq!(alpha_turn(None), None);
        // 少一枚 `cleared=`：后面的读法全都串了位，而不是各自读成零。
        assert_eq!(
            alpha_turn(Some("took=true;board=holes=true;mode=2;after=holes=true")),
            None
        );
        // 段落顺序被改，同样读不出。
        assert_eq!(
            alpha_turn(Some(
                "found=0;took=true;cleared=true;board=holes=true;transparent=holes=true;restored=true;after=holes=true"
            )),
            None
        );
    }

    /// The three §5.9.15 readings run where [`PIN_SCHEDULE`] says they run: the shut reading
    /// before the still window's open count, the open reading before the turn, and the whole
    /// set before the subject's own `menu`. A swap of the pair the row grades *relative* to
    /// is caught here rather than read off a desktop.
    #[test]
    fn the_alpha_legs_are_ordered_by_the_schedule_that_orders_the_menu() {
        assert_eq!(
            leg_order(
                &[
                    "alpha-closed",
                    "gif-items-still",
                    "alpha-open",
                    "alpha-turn",
                    "menu"
                ],
                PIN_SCHEDULE
            ),
            None
        );
        let swap = leg_order(&["alpha-open", "alpha-closed"], PIN_SCHEDULE).unwrap_or_default();
        assert!(swap.contains("日程不符"), "{swap}");
        let late = leg_order(&["menu", "alpha-open"], PIN_SCHEDULE).unwrap_or_default();
        assert!(late.contains("日程不符"), "{late}");
    }

    /// 第 42 项 ②'s rule again, for §5.9.14's four rows: a run with no clip pin must still
    /// print all four names, the first one red and the other three asking nothing. The shape
    /// is not hypothetical - a fixture that cannot be written (`{}` on a full temp dir) or a
    /// clip the decoder refuses leaves the delegate with nothing to arm, and P43's
    /// 行数不是护栏 is why the three below say *why* they were not asked rather than sitting
    /// out the run.
    #[test]
    fn a_run_with_no_clip_still_names_the_three_rows_below_it() {
        let nowhere = Path::new("falconshot-gauge-this-file-was-never-written.gif");
        let lines: Vec<String> = Vec::new();
        let rows = gif_rows(&lines, None, 0, "这不是一个能读的图", nowhere);
        let names: Vec<&str> = rows.iter().map(|(_, name, _)| *name).collect();
        assert_eq!(names, GIF_PIN_ROWS.to_vec(), "{names:?}");
        assert_eq!(
            rows[0].0,
            Check::Fail,
            "停住的那一格本身要红：{:?}",
            rows[0]
        );
        for (grade, name, detail) in &rows[1..] {
            assert_eq!(*grade, Check::Blocked, "{name} 没被问过，不该算红");
            assert!(!detail.is_empty(), "{name} 那一格连为什么都没说");
        }
    }

    /// §10 第 42 项 ② read against this file rather than against the desktop: a leg that
    /// stops early used to stop *writing*, so the door came back three lines short and the
    /// only evidence that anything had gone wrong was a row count - which P43 established is
    /// not a guardrail. Both shapes below are the real stops (no rect, and a file the disk
    /// will not hand back), and neither is allowed to swallow the rows beneath it.
    ///
    /// `Blocked`, not `Fail`, for the rows never asked: their claim was not put to anything,
    /// and a red row here would say "the pin is wrong" when the run only ever learned that
    /// the file is unreadable.
    #[test]
    fn a_leg_that_stops_early_still_names_the_rows_below_it() {
        let nowhere = Path::new("falconshot-gauge-this-file-was-never-written.png");
        let no_rect = file_pin_rows(None, 0, nowhere, "还没有选区：先把要钉的范围拖出来。");
        let unreadable = file_pin_rows(
            Some(PhysRect::new(10, 20, 30, 40)),
            7,
            nowhere,
            "已保存到 x.png（30×40）。",
        );
        for rows in [&no_rect, &unreadable] {
            let names: Vec<&str> = rows.iter().map(|(_, name, _)| *name).collect();
            assert_eq!(names, FILE_PIN_ROWS.to_vec(), "{names:?}");
            assert_eq!(
                rows[0].0,
                Check::Fail,
                "停住的那一格本身要红：{:?}",
                rows[0]
            );
            for (grade, name, detail) in &rows[1..] {
                assert_eq!(*grade, Check::Blocked, "{name} 没被问过，不该算红");
                assert!(!detail.is_empty(), "{name} 那一格连为什么都没说");
            }
        }

        // The index is the whole contract between the three stops and this list, so the
        // boundary is graded: the row under row 2 is exactly the pixels row, and there is
        // nothing at all under the last one - a slipped index would otherwise reprint a row
        // the leg did produce, next to the real one and with the same name.
        let below_two = unasked_rows(1, "没有 hwnd 可截");
        assert_eq!(below_two.len(), 1, "{below_two:?}");
        assert_eq!(below_two[0].1, "pin window pixels");
        assert!(unasked_rows(2, "下面没有格了").is_empty());
    }

    /// §10 第 55 项 ①'s grading half, headless: the shapes the same binary can come back
    /// in, told apart by *which* rects survive and by *which windows* they are.
    ///
    /// The dumps are written in `pinTopLevels`'s own field spelling because that string is all
    /// the leg hands over - `close_rows` reads a census, not a fact about Qt. Runs of this
    /// mode on this desktop put the pile at these rects (and since §5.9.14 the clip's own
    /// 40x30 window is the fourth of them), and both shapes below leave exactly three windows,
    /// which is the reason a count is not the row: the fixed one has the live pins' rects, the
    /// broken one has one live rect and the dead pin's. The third has the right rects too and
    /// answers for them with different handles, which is the fifth row's question rather than
    /// the third's.
    #[test]
    fn a_middle_close_is_graded_by_which_rects_survive_not_how_many() {
        let rect = PhysRect::new(608, 380, 320, 200);
        let xrect = PhysRect::new(608, 596, 320, 200);
        let frect = PhysRect::new(944, 596, 320, 200);
        let grect = PhysRect::new(944, 380, 40, 30);
        let win = |hwnd: u64, r: PhysRect| {
            format!(
                "hwnd=0x{hwnd:x} class=[PinWindow_QMLTYPE_17] phys={},{},{},{} visible=1 title=[]",
                r.x, r.y, r.w, r.h
            )
        };
        let before = [win(1, rect), win(2, xrect), win(3, frect), win(4, grect)].join("\n");
        let alive = [win(1, rect), win(3, frect), win(4, grect)].join("\n");
        let legs = |after: &str| {
            vec![
                format!("close-before|{before}"),
                "close|1".to_string(),
                format!("close-after|{after}"),
            ]
        };

        let fixed = close_rows(
            &legs(&alive),
            Some(rect),
            Some(xrect),
            Some(frect),
            Some(grect),
        );
        let names: Vec<&str> = fixed.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, CLOSE_ROWS.to_vec(), "{names:?}");
        for (grade, name, detail) in &fixed {
            assert_eq!(*grade, Check::Pass, "{name} 修好之后该绿：{detail}");
        }

        // The close took the clip's window instead of the doomed one: the count is still the
        // three this row expects, which is why row 3 reads *which* rects are covered and not
        // how many windows came back.
        let broken = close_rows(
            &legs(&[win(1, rect), win(2, xrect), win(3, frect)].join("\n")),
            Some(rect),
            Some(xrect),
            Some(frect),
            Some(grect),
        );
        assert_eq!(broken[0].0, Check::Pass, "点名那格两种跑法都该绿");
        assert_eq!(broken[1].0, Check::Pass, "关掉的枚数两种跑法都一样");
        assert_eq!(broken[2].0, Check::Fail, "会动的那枚没有窗了，该红");
        assert_eq!(broken[3].0, Check::Fail, "关掉那枚还留着窗，该红");

        // The third shape the same binary can come back in, and the one the two rect rows are
        // blind to: the close leaves the right windows on the right rects, and they are not the
        // windows that were there - which is what an `int` model did to this pile, rebuilding
        // every object under a close that removed one (§10 第 55 项, P55). Row 4 is the only one
        // that reads handles, so row 4 is the only one that goes red.
        let rebuilt = close_rows(
            &legs(&[win(7, rect), win(8, frect), win(9, grect)].join("\n")),
            Some(rect),
            Some(xrect),
            Some(frect),
            Some(grect),
        );
        assert_eq!(rebuilt[2].0, Check::Pass, "三格都还有窗，点名那格看不出差");
        assert_eq!(
            rebuilt[3].0,
            Check::Pass,
            "关掉那枚的位置也空着，那一格同样看不出差"
        );
        assert_eq!(
            rebuilt[4].0,
            Check::Fail,
            "换过窗该由第五格报红：{:?}",
            rebuilt[4]
        );
        assert!(rebuilt[4].2.contains("同一扇=false"), "{:?}", rebuilt[4]);

        // The leg stopped after the close: the rows under it keep their names and say
        // they were never asked, which is 第 42 项 ②'s rule read against this leg too.
        let stopped = close_rows(
            &["close|1".to_string()],
            Some(rect),
            Some(xrect),
            Some(frect),
            Some(grect),
        );
        assert_eq!(stopped[0].0, Check::Blocked, "{:?}", stopped[0]);
        assert_eq!(stopped[2].0, Check::Blocked, "{:?}", stopped[2]);
        assert_eq!(stopped[3].0, Check::Blocked, "{:?}", stopped[3]);
        assert_eq!(stopped[4].0, Check::Blocked, "{:?}", stopped[4]);
        assert!(stopped[2].2.contains("close(\"1\")"), "{:?}", stopped[2]);
        // …and a census the leg did report but that was empty is a *reading*, not a stop:
        // the three windows the close should have left are absent, so both rows go red.
        let none_left = close_rows(&legs(""), Some(rect), Some(xrect), Some(frect), Some(grect));
        assert_eq!(none_left[2].0, Check::Fail, "{:?}", none_left[2]);
        assert_eq!(none_left[3].0, Check::Pass, "{:?}", none_left[3]);
        // The signed shape is what `report` now writes: the delegate's own id sits before the
        // `#`, so the census reads the same and the row says who said it. An unsigned line is
        // read as unsigned rather than as an id of 0 - 0 is a pin id the set really hands out.
        assert!(fixed[0].2.contains("未署名"), "{:?}", fixed[0]);
        let signed = close_rows(
            &[
                format!("close-before|1#{before}"),
                "close|1".to_string(),
                format!("close-after|1#{alive}"),
            ],
            Some(rect),
            Some(xrect),
            Some(frect),
            Some(grect),
        );
        for (grade, name, detail) in &signed {
            assert_eq!(*grade, Check::Pass, "{name} 署了名不该改判：{detail}");
        }
        assert!(signed[0].2.contains("贴图 1"), "{:?}", signed[0]);
    }

    /// §10 第 47 项 ④'s grading half, headless. Every string below is shaped by one of
    /// `pinPushDialogButton()`'s four returns, because the verdict column hands its three
    /// answers out by reading those strings and nothing else - so a shim that reworded itself
    /// past this test would be silently re-choosing who gets blamed.
    ///
    /// The split is 第 46 项 ⑥'s own shape: one refused press printed four FAILs in a row,
    /// which reads as "the pin is broken", while that same report's `dialogs=0` says nobody had
    /// a window to press. So a refusal may never reach `Fail`, and the two shapes that share
    /// `Blocked` must still be told apart by their sentence - `Check` has three variants, and
    /// this has four shapes, which is exactly why the sentence carries the difference.
    #[test]
    fn a_press_leg_says_which_of_the_three_it_asked() {
        let no_dialog = "pushed=no reason=no-visible-#32770-of-this-pid dialogs=0 \
                         mine_tops=3 mine_visible=2 foreign_dialogs=0 mine=[-]";
        let no_ok_button = "pushed=no reason=no-enabled-idok-button dialogs=1 desc=4 buttons=3 \
                            title=[另存为]";
        let wrong_window = "pushed=yes dialog=0x1234 class=[Button] title=[OK] dialogs=1 desc=4 \
                            buttons=3 button=0x5678 btitle=[OK] bid=1 ret=0 visible_after=0";
        let wrong_button = "pushed=yes dialog=0x1234 class=[#32770] title=[另存为] dialogs=1 \
                            desc=4 buttons=3 button=0x5678 btitle=[取消] bid=2 ret=0 \
                            visible_after=1";
        let pressed = "pushed=yes dialog=0x1234 class=[#32770] title=[另存为] dialogs=1 desc=4 \
                       buttons=3 button=0x5678 btitle=[保存] bid=1 ret=0 visible_after=0";

        assert_eq!(ask(None), Ask::Unasked);
        assert_eq!(ask(Some(no_dialog)), Ask::Refused, "{no_dialog}");
        assert_eq!(ask(Some(no_ok_button)), Ask::Refused, "{no_ok_button}");
        assert_eq!(ask(Some(wrong_window)), Ask::Mispressed, "{wrong_window}");
        assert_eq!(ask(Some(wrong_button)), Ask::Mispressed, "{wrong_button}");
        assert_eq!(ask(Some(pressed)), Ask::Pressed, "{pressed}");

        // The verdict each shape is allowed to reach - the two blocked ones are the whole
        // finding, and the mis-press is the only shape that may call the product wrong.
        assert_eq!(ask_check(Ask::Unasked), Check::Blocked);
        assert_eq!(ask_check(Ask::Refused), Check::Blocked);
        assert_eq!(ask_check(Ask::Mispressed), Check::Fail);
        assert_eq!(ask_check(Ask::Pressed), Check::Pass);

        // 第 46 项 ⑥ pinned as a one-liner: the report that said `dialogs=0` used to be a red
        // row, and it must not become one again by way of any helper this file grows.
        assert!(!real_click(no_dialog));
        assert_ne!(ask_check(ask(Some(no_dialog))), Check::Fail);
        assert!(no_dialog.contains("dialogs=0"));

        // Two shapes, one verdict, and they may not have one sentence either.
        let unasked_said = ask_said(Ask::Unasked);
        let refused_said = ask_said(Ask::Refused);
        assert!(unasked_said.contains("未问") && refused_said.contains("桌面不肯答"));
        assert_ne!(unasked_said, refused_said);
        assert!(ask_said(Ask::Mispressed).contains("产品答错"));
        assert!(ask_said(Ask::Pressed).contains("按到了"));

        // `ask` reads the same prefix `real_click` does, so the two can never disagree about
        // whether a leg claimed to have pressed at all: bare `pushed=yes` is not a claim.
        assert_eq!(ask(Some("pushed=yes")), ask(Some("pushed=no reason=x")));
    }

    /// The other half of 第 47 项 ④: whether the dialog phase took its own door back in.
    ///
    /// All eight readings the legs can hand over are graded here, including the one this
    /// predicate does not know - an unknown `dlg-after` failing loudly is the alternative to
    /// reporting a window this run never measured as one it did.
    #[test]
    fn the_dialog_phase_answers_for_its_own_door() {
        let grade = |shown: Option<&str>, after: Option<&str>, dismissed, rejected| {
            let (check, said) = dialog_hygiene(shown, after, dismissed, rejected);
            (check, said)
        };

        let (check, said) = grade(None, Some("false"), false, false);
        assert_eq!(check, Check::Blocked, "{said}");
        assert!(said.contains("未问"), "{said}");

        let (check, said) = grade(Some("true"), None, false, false);
        assert_eq!(check, Check::Blocked, "{said}");
        assert!(said.contains("未问"), "{said}");

        // The click closed it: nothing to sweep, and nothing to blame the next phase for.
        let (check, said) = grade(Some("true"), Some("false"), false, false);
        assert_eq!(check, Check::Pass, "{said}");

        // Closed by the click *and* rejected afterwards - the two readings cannot both hold.
        let (check, said) = grade(Some("true"), Some("false"), true, true);
        assert_eq!(check, Check::Fail, "{said}");
        assert!(said.contains("自相矛盾"), "{said}");

        let (check, said) = grade(Some("true"), Some("true"), true, true);
        assert_eq!(check, Check::Pass, "{said}");

        // Rejected but the door did not confess: the window is still out there and now has a
        // name, which is the sentence the next phase's census needs to attribute itself.
        let (check, said) = grade(Some("true"), Some("true"), true, false);
        assert_eq!(check, Check::Fail, "{said}");
        assert!(said.contains("下一段代为认领"), "{said}");

        let (check, said) = grade(Some("true"), Some("true"), false, false);
        assert_eq!(check, Check::Fail, "{said}");
        assert!(said.contains("自家的窗"), "{said}");

        let (check, said) = grade(Some("true"), Some("yes"), false, false);
        assert_eq!(check, Check::Fail, "{said}");
        assert!(said.contains("不认识"), "{said}");

        // The quick door's "I open no dialog" claim, blamed on whoever left the window.
        let left = "pushed=no reason=no-visible-#32770-of-this-pid dialogs=1 mine_tops=3";
        let whole = "pushed=no reason=no-visible-#32770-of-this-pid dialogs=0 mine_tops=3";
        let (opening, _) = grade(Some("true"), Some("true"), false, false);
        assert!(census_blames_earlier_phase(left, opening, true));
        assert!(!census_blames_earlier_phase(left, Check::Pass, true));
        assert!(!census_blames_earlier_phase(whole, opening, true));
        // The dialog phase never ran, so there is no earlier phase to point at - the row that
        // counts has to keep the blame itself rather than hand it to a phase that did not exist.
        assert!(!census_blames_earlier_phase(left, opening, false));
    }

    /// 两枚新行读的是同一枚字符串，所以**读法**先于判据：`None`（这一格压根没来）与 `-`
    /// （产品说它没有）必须分成两种答案，把它们并成一格，门就在替产品说它没说过的话。
    ///
    /// 三格反例都是真会来的形状，而不是稻草人：腿被别的腿提前中止（`None`）、联动关着的
    /// 那一趟（`-`）、以及键名相近——日程里 `dlg-push` 正是 `dlg-push-tries` 的前缀，
    /// 前缀匹配会把一枚键的读数记到另一枚名下。
    #[test]
    fn a_probe_field_separates_the_two_ways_a_key_can_be_missing() {
        let line = "rect=45,35,300,200 covering=7 pins=2 enabled=1 layers=1 hidden=7";
        assert_eq!(probe_field(Some(line), "covering").as_deref(), Some("7"));
        assert_eq!(probe_field(Some(line), "layers").as_deref(), Some("1"));
        assert_eq!(probe_field(Some(line), "hidden").as_deref(), Some("7"));

        // 产品没有窗可读时自己写 `-`，这一格是来了并答"没有"。
        let silent = "rect=- covering=- pins=2 enabled=0 layers=0 hidden=-";
        assert_eq!(probe_field(Some(silent), "enabled").as_deref(), Some("0"));
        assert_eq!(probe_field(Some(silent), "covering").as_deref(), Some("-"));

        // 这一格压根没来：判据读 `None`，读不出 `-`。
        assert_eq!(probe_field(None, "covering"), None);
        // 整行不是它的那一行，同样 `None`——两件事都发生在"腿提前中止"那一趟里。
        assert_eq!(probe_field(Some(""), "covering"), None);
        assert_eq!(probe_field(Some("pushed=no dialogs=0"), "covering"), None);

        // 前缀不算命中：`pin` 与 `pins`、`layer` 与 `layers` 各问各的。
        assert_eq!(probe_field(Some(line), "pin"), None);
        assert_eq!(probe_field(Some(line), "layer"), None);
    }

    /// 那枚矩形只有四枚数都成其为一次测量才算读到。**零面积也算没读到**，与
    /// [`crate::session`] 的 `dialog_rect` 同一读法：那扇窗存在过而量不出尺寸时，门要说
    /// "这趟没量到"，不能说"量到一枚 0×0 的窗"——同一事实两处口径，正是 #281 那一族。
    #[test]
    fn a_probe_rect_refuses_the_window_the_product_refused_to_see() {
        let real = "rect=45,35,300,200 covering=- pins=2 enabled=0 layers=0 hidden=-";
        assert_eq!(probe_rect(Some(real)), Some((45, 35, 300, 200)));
        // 负坐标是真的：多屏桌面左上角那块就在原点之外。
        let offscreen = "rect=-1280,-200,400,300 covering=3 pins=2 enabled=1 layers=1 hidden=3";
        assert_eq!(probe_rect(Some(offscreen)), Some((-1280, -200, 400, 300)));

        assert_eq!(probe_rect(None), None);
        assert_eq!(
            probe_rect(Some("rect=- covering=- pins=1 enabled=0 layers=0 hidden=-")),
            None
        );
        assert_eq!(probe_rect(Some("covering=- pins=1")), None);

        // 三枚与五枚都不是这一格的形状；把它们补成四枚，是把排版当成了测量。
        assert_eq!(probe_rect(Some("rect=45,35,300 covering=-")), None);
        assert_eq!(probe_rect(Some("rect=45,35,300,200,1 covering=-")), None);
        // 不是数：宽为负、高为字母、段为空。
        assert_eq!(probe_rect(Some("rect=45,35,-300,200 covering=-")), None);
        assert_eq!(probe_rect(Some("rect=45,35,300,x covering=-")), None);
        assert_eq!(probe_rect(Some("rect=45,,300,200 covering=-")), None);
        // 零面积：产品自己拒绝交出这枚窗，门不许替它交。
        assert_eq!(probe_rect(Some("rect=45,35,0,0 covering=7")), None);
        assert_eq!(probe_rect(Some("rect=45,35,300,0 covering=7")), None);
    }

    /// 名单这一格与上面两格相反：`None`、空串与 `-` 都读成"一枚也没有"，因为这里没有两种
    /// 缺席可分——门判的是**集合**，而集合为空只有一种说法。把 `-` 当成一枚 id 才会出的那
    /// 种红（"隐藏了名为 `-` 的贴图"）比它的绿更贵。
    #[test]
    fn a_probe_list_reads_no_id_from_the_word_that_means_none() {
        assert!(probe_ids(None).is_empty());
        assert!(probe_ids(Some("")).is_empty());
        assert!(probe_ids(Some("-")).is_empty());

        assert_eq!(probe_ids(Some("7")), vec!["7".to_string()]);
        assert_eq!(
            probe_ids(Some("7,9")),
            vec!["7".to_string(), "9".to_string()]
        );
        // 产品 `join(",")` 的名单里若混进空段，它不算一枚 id；一枚也没有也不算空。
        assert_eq!(probe_ids(Some("7,")), vec!["7".to_string()]);
        assert!(probe_ids(Some(",")).is_empty());
    }

    /// `dlg-live-*` 的三样缺席读成同一个答案，与上面两格**相反**：那里 `covering` 与 `pin` 是
    /// 两种问法（"这一格没有这个字段" 与 "这个字段是空的"），而这里没有——屏上读不到屏上，
    /// 就是问不出。
    #[test]
    fn a_live_reading_carries_the_pin_it_speaks_for() {
        assert_eq!(live_said(Some("7=false")), Some(("7".to_string(), false)));
        assert_eq!(live_said(Some("7=true")), Some(("7".to_string(), true)));

        assert_eq!(live_said(None), None);
        // 键改了名而值没跟上：一枚没有 id 的 live，门无从知道它替谁说话。
        assert_eq!(live_said(Some("false")), None);
        assert_eq!(live_said(Some("=false")), None);
        // 不是 `true`/`false`：`"1"` 是桥那一侧的写法（`enabled=1`），借到这一格就是两份事实
        // 各用一套记号，而这一格读的是 QML 那句 `String(view.live)`。
        assert_eq!(live_said(Some("7=1")), None);
        assert_eq!(live_said(Some("7")), None);
    }

    /// 这一格的整条判据是「账说藏了，屏上就得真没了；账说收回，屏上就得真回来」，而**中间那一步
    /// 是 epoch 动过**。三臂各挡住一种会绿的读法：
    /// ① 账平而屏没动（P123 起 #176 欠的那一枚证人）；② 屏上恰好自己空了（#316）；
    /// ③ 两半说的不是同一枚贴图（一趟里换了一枚说话，量的就不是同一次隐藏）。
    /// 第四臂是诚实的另一半：账上没藏的那一枚，屏上**留着**才是对的。
    #[test]
    fn dialog_live_asks_the_screen_and_not_the_account() {
        let ls = |pairs: &[(&str, &str)]| -> Vec<String> {
            pairs.iter().map(|(k, v)| format!("{k}|{v}")).collect()
        };
        let grade = |pairs: &[(&str, &str)], hidden: &[&str]| {
            let list: Vec<String> = hidden.iter().map(|s| s.to_string()).collect();
            dialog_live(&ls(pairs), Some("1"), &list).0
        };
        let hidden = ["7"];
        let good = [
            ("dlg-live-open", "7=false"),
            ("dlg-live-gone", "7=true"),
            ("dlg-epoch-open", "3"),
            ("dlg-epoch-gone", "4"),
        ];
        assert_eq!(grade(&good, &hidden), Check::Pass, "藏了、回来了、中间动过");

        // ① 账上藏着它，屏上却还站着。
        assert_eq!(
            grade(
                &[
                    ("dlg-live-open", "7=true"),
                    ("dlg-live-gone", "7=true"),
                    ("dlg-epoch-open", "3"),
                    ("dlg-epoch-gone", "4"),
                ],
                &hidden
            ),
            Check::Fail,
            "layers=1、hidden 非空而屏上没动 ⇒ 这一格必须红"
        );
        // 收回那一半没回来。
        assert_eq!(
            grade(
                &[
                    ("dlg-live-open", "7=false"),
                    ("dlg-live-gone", "7=false"),
                    ("dlg-epoch-open", "3"),
                    ("dlg-epoch-gone", "4"),
                ],
                &hidden
            ),
            Check::Fail,
            "藏了却没放回来 ⇒ 步骤 4 没走"
        );
        // ② epoch 一格没动：两半看着像结果，其实是巧合。
        assert_eq!(
            grade(
                &[
                    ("dlg-live-open", "7=false"),
                    ("dlg-live-gone", "7=true"),
                    ("dlg-epoch-open", "3"),
                    ("dlg-epoch-gone", "3"),
                ],
                &hidden
            ),
            Check::Fail,
            "§5.5.11 那一声没扇出来，这两半就不是它的结果"
        );
        // ③ 两半换了枚贴图。
        assert_eq!(
            grade(
                &[
                    ("dlg-live-open", "7=false"),
                    ("dlg-live-gone", "8=true"),
                    ("dlg-epoch-open", "3"),
                    ("dlg-epoch-gone", "4"),
                ],
                &hidden
            ),
            Check::Fail,
            "开时替 7 说话、关完替 8 说话 ⇒ 量的不是同一次隐藏"
        );
        // ④ 账上没藏 8，屏上留着才是对的（读得到矩形、而这一枚不压在那扇窗上那一臂）。
        assert_eq!(
            grade(
                &[
                    ("dlg-live-open", "8=true"),
                    ("dlg-live-gone", "8=true"),
                    ("dlg-epoch-open", "3"),
                    ("dlg-epoch-gone", "4"),
                ],
                &hidden
            ),
            Check::Pass,
            "没藏着却不许屏上消失，是把保守读法当成了判据"
        );
        assert_eq!(
            grade(
                &[
                    ("dlg-live-open", "8=false"),
                    ("dlg-live-gone", "8=true"),
                    ("dlg-epoch-open", "3"),
                    ("dlg-epoch-gone", "4"),
                ],
                &hidden
            ),
            Check::Fail,
            "账上没藏它，屏上却少了 ⇒ 藏着与放着的名单不一致"
        );

        // 读不到就是问不出，四种缺法同一种答。
        for miss in [
            &[
                ("dlg-live-gone", "7=true"),
                ("dlg-epoch-open", "3"),
                ("dlg-epoch-gone", "4"),
            ][..],
            &[
                ("dlg-live-open", "7=false"),
                ("dlg-epoch-open", "3"),
                ("dlg-epoch-gone", "4"),
            ][..],
            &[
                ("dlg-live-open", "7=false"),
                ("dlg-live-gone", "7=true"),
                ("dlg-epoch-gone", "4"),
            ][..],
            &[
                ("dlg-live-open", "7=false"),
                ("dlg-live-gone", "7=true"),
                ("dlg-epoch-open", "3"),
            ][..],
        ] {
            assert_eq!(
                dialog_live(
                    &ls(miss),
                    Some("1"),
                    &hidden.iter().map(|s| s.to_string()).collect::<Vec<_>>()
                )
                .0,
                Check::Blocked,
                "缺一枚读数 ⇒ 问不了，不是红"
            );
        }
        // 那枚键关着：步骤 1 是用户的决定，这一格不替它记功。
        let list7: Vec<String> = hidden.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            dialog_live(&ls(&good), Some("0"), &list7).0,
            Check::Blocked,
            "出厂 false 那一臂不许写成通过"
        );
        assert_eq!(
            dialog_live(&ls(&good), None, &list7).0,
            Check::Blocked,
            "连键都没读到也是问不了"
        );
    }

    /// §5.10.1 的那一份名单是**全局**的（core 里只有一枚 `BTreeSet<PinId>`），而 cargo 把同档
    /// 的 test 分到多线程跑。两枚 test 同时改它就会读到对方的手势 —— 那不是判据，是竞态。
    /// 这里不引外部设备，一枚 `Mutex` 就够；取不到锁（别的 test  panic 过）也照走，因为
    /// 这一族的失败信息要由断言自己说，不由锁说。
    static SELECTION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// §5.10.1 步骤 1/2/3/4 一次跑完，判的是**每一手各自的行为**：单击只留一枚、Ctrl+单击
    /// 加、再按同一枚 Ctrl+单击是**减**、点不存在的贴图交 Err 且不动名单、清场之后一枚也不留。
    /// 而每一手之后，窗侧那一格 `view_data(id).selected` 必须与 core 的名单同集合 ——
    /// 「这格没崩」不算答案（#274 那一族）。
    #[test]
    fn a_selection_is_one_list_the_bridge_can_read() {
        let _guard = SELECTION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let flags = |id: u64| state::with(|s| s.view_data(id).selected);
        let drawn = || {
            state::with(|s| {
                let mut v: Vec<u64> = s
                    .ids()
                    .into_iter()
                    .filter(|id| s.view_data(*id).selected)
                    .collect();
                v.sort_unstable();
                v
            })
        };
        let a = state::add_demo(8, 8).expect("第一枚贴图开不出来");
        let b = state::add_demo(8, 8).expect("第二枚贴图开不出来");
        let c = state::add_demo(8, 8).expect("第三枚贴图开不出来");
        let _ = state::with(|s| s.clear_selection());
        assert_eq!(state::with(|s| s.selection_count()), 0, "起手就得是空名单");

        // 步骤 2：单击只留这一枚，前一枚自动落选。
        assert_eq!(state::with(|s| s.select_pin(a, false)), Ok(1));
        assert!(flags(a) && !flags(b) && !flags(c));
        // 步骤 1：按住 Ctrl 再单击第二枚 —— 加，而不是把第一枚顶掉。
        assert_eq!(state::with(|s| s.select_pin(b, true)), Ok(2));
        assert!(flags(a) && flags(b) && !flags(c));
        // 同一手势按在同一枚上是**减**：这一手最容易写成"永远加"，那样 Ctrl+单击就
        // 只能往上叠、取不下来，步骤 4 之外的第二条退出路就没有了。
        assert_eq!(state::with(|s| s.select_pin(b, true)), Ok(1));
        assert!(flags(a) && !flags(b));
        // 点一枚不在集里的贴图（菜单陈旧、窗已关）：交 Err，且名单一枚也不动。
        // 悄悄清场或悄悄失败都是把调用方的 bug 扮成用户的成功。
        assert!(
            state::with(|s| s.select_pin(4_000_003, false)).is_err(),
            "不存在的贴图必须交 Err"
        );
        assert_eq!(
            state::with(|s| s.selection_count()),
            1,
            "失败的那一手不许动名单"
        );
        // 步骤 3：统一选中状态只有一份数据 —— 窗侧读到的那些枚，必须与名单同集合。
        let mut ids = state::with(|s| s.selected_ids());
        ids.sort_unstable();
        assert_eq!(ids, vec![a], "名单本身就是 a；读到 {ids:?}");
        assert_eq!(drawn(), vec![a], "窗侧那一格与名单读成同一批");
        // 步骤 4：清场。
        assert_eq!(state::with(|s| s.clear_selection()), 1);
        assert_eq!(state::with(|s| s.selection_count()), 0);
        assert!(
            !flags(a) && !flags(b) && !flags(c),
            "清场之后不许留下一枚带边的窗"
        );
        let closed = state::with(|s| s.close(&[a, b, c]));
        assert_eq!(closed, 3, "三枚都得关回去，别把桌面留给下一枚 test");
    }

    /// 「退出多选」那句要能报数量。所以清场那一手交的是**清掉了多少枚**，不是清完再数：
    /// 后者永远是 0，拿它当答案就是把一句实话换成一枚恒真的读数（#455 那一族）。
    /// `0` 与「没有这一手」在这一格也是两枚不同的读数（#453）——这里只能交数，不交 Option。
    #[test]
    fn the_clear_hand_reports_how_many_it_took_out() {
        let _guard = SELECTION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let a = state::add_demo(8, 8).expect("贴图开不出来");
        let b = state::add_demo(8, 8).expect("贴图开不出来");
        let _ = state::with(|s| s.clear_selection());
        assert_eq!(state::with(|s| s.clear_selection()), 0, "本来就没枚，交 0");
        let _ = state::with(|s| s.select_pin(a, false));
        let _ = state::with(|s| s.select_pin(b, true));
        assert_eq!(
            state::with(|s| s.clear_selection()),
            2,
            "交的是「清掉两枚」"
        );
        assert_eq!(
            state::with(|s| s.selection_count()),
            0,
            "清完再数才是这一问"
        );
        // 关掉一枚被选中的贴图：名单里那一枚跟着走，不许留下指名不存在的窗。
        let _ = state::with(|s| s.select_pin(a, false));
        let _ = state::with(|s| s.select_pin(b, true));
        let _ = state::with(|s| s.close(&[a]));
        assert_eq!(
            state::with(|s| s.selected_ids()),
            vec![b],
            "关掉 a 之后名单该只剩 b"
        );
        let _ = state::with(|s| s.clear_selection());
        let closed = state::with(|s| s.close(&[b]));
        assert_eq!(closed, 1);
    }

    /// §5.10.1 那三枚行各自的四面（#274/#461）。三枚判据各答各的事，所以「扇出没跑」那一趟
    /// 只能红中间那一枚：第一枚读的是本窗自己的名单与本窗自己的边，第三枚读的是清完以后的
    /// 两边——它们都对，中间那一枚单独把这一趟和绿的那一趟分开。
    ///
    /// 五面：全好、别人一次也没开口、亮的是没被点名的那一扇、场上只有一枚贴图、腿根本没跑。
    /// 前两面在真的窗口里才会碰上，本档由 `--pin` 的量规读；这里三面都是**造出来的行**——
    /// 判据的形状不能只在它成立的那一趟里被看见。
    #[test]
    fn the_three_selection_rows_answer_apart_for_each_way_the_fan_out_can_fail() {
        let subject: PinId = 7;
        let legs = |pairs: &[(&str, &str)]| -> Vec<String> {
            pairs.iter().map(|(k, v)| format!("{k}|{v}")).collect()
        };
        let at = |rows: &[(Check, &'static str, String)], i: usize| rows[i].0;

        let good: [(&str, &str); 13] = [
            ("sel-before-count", "0"),
            ("sel-before-self", "false"),
            ("sel-plain", "1"),
            ("sel-plain-count", "1"),
            ("sel-plain-self", "true"),
            ("sel-add-target", "9"),
            ("sel-observe-on", "9=true"),
            ("sel-add", "2"),
            ("sel-add-count", "2"),
            ("sel-add-self", "true"),
            ("sel-observe-off", "9=false"),
            ("sel-cleared-count", "0"),
            ("sel-cleared-self", "false"),
        ];
        let quiet: Vec<(&str, &str)> = good
            .iter()
            .filter(|(k, _)| *k != "sel-observe-on" && *k != "sel-observe-off")
            .copied()
            .collect();
        let mut wrong: Vec<(&str, &str)> = good.to_vec();
        wrong[6] = ("sel-observe-on", "11=true");
        wrong[10] = ("sel-observe-off", "11=false");
        let mut doubled: Vec<(&str, &str)> = good.to_vec();
        doubled.insert(7, ("sel-observe-on", "11=true"));
        // 同一枚键在**这一段之后**又说了一次：P134 的 `des-*` 那一族按名单换三次点名，每次都扇
        // 一遍别的窗。这一枚是正控制（不是反证）——它证明段界不是白画的：那一趟属于销毁那一问，
        // 不该挪动 §5.10.1 这两枚的颜色。
        let mut later: Vec<(&str, &str)> = good.to_vec();
        later.push(("sel-observe-on", "12=true"));
        later.push(("sel-observe-off", "12=false"));
        let only_one: Vec<(&str, &str)> = good
            .iter()
            .filter(|(k, _)| {
                !matches!(
                    *k,
                    "sel-add-target"
                        | "sel-observe-on"
                        | "sel-add"
                        | "sel-add-count"
                        | "sel-add-self"
                        | "sel-observe-off"
                )
            })
            .copied()
            .collect();

        let all = sel_rows(&legs(&good), subject);
        let no_fanout = sel_rows(&legs(&quiet), subject);
        let other_window = sel_rows(&legs(&wrong), subject);
        let two_lit = sel_rows(&legs(&doubled), subject);
        let out_of_slice = sel_rows(&legs(&later), subject);
        let names: Vec<&str> = all.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, SEL_ROWS.to_vec(), "{names:?}");
        for row in &all {
            assert_eq!(row.0, Check::Pass, "好的一趟该三枚全绿：{:#?}", row.2);
        }

        // 扇出没跑：另一扇窗一次也没说自己亮了。第一枚照旧绿——它读不到别人，本就该读不到。
        assert_eq!(at(&no_fanout, 0), Check::Pass, "第一枚管的是本窗");
        assert_eq!(
            at(&no_fanout, 1),
            Check::Fail,
            "点名了别人而别人没开口，这一枚必须红：{:#?}",
            no_fanout[1].2
        );
        assert_eq!(
            at(&no_fanout, 2),
            Check::Fail,
            "退出也没扇到别人：{:#?}",
            no_fanout[2].2
        );

        // 亮的是没被点名的那一扇：名单数字全对，这一枚照样红——§5.10.1 第 3 步要的是「被选中的」亮。
        assert_eq!(at(&other_window, 0), Check::Pass, "本窗那圈边确实亮了");
        assert_eq!(
            at(&other_window, 1),
            Check::Fail,
            "{:#?}",
            other_window[1].2
        );
        assert_eq!(
            at(&other_window, 2),
            Check::Fail,
            "{:#?}",
            other_window[2].2
        );

        // 两扇窗同时说自己亮了：场上只点名了两枚，第三枚亮就是名单外的窗跟着动了。
        assert_eq!(at(&two_lit, 1), Check::Fail, "{:#?}", two_lit[1].2);

        // 段界正控制：那一段里还是恰好一句，段外那两句是销毁那一族点名单时扇出来的，
        // 不该挪动 §5.10.1 这两枚的颜色（P134 门档那 12 枚红就是这么来的）。
        for row in &out_of_slice {
            assert_eq!(row.0, Check::Pass, "{:#?}", row.2);
        }

        // 场上只有一枚贴图：第二枚是「没有问」，第三枚不因问不到的那一半而红。
        let one_pin = {
            let mut pairs = only_one.clone();
            pairs.insert(5, ("sel-add-target", "-1"));
            sel_rows(&legs(&pairs), subject)
        };
        assert_eq!(at(&one_pin, 0), Check::Pass, "第一枚与场上几枚贴图无关");
        assert_eq!(
            at(&one_pin, 1),
            Check::Blocked,
            "没有别的窗可点名，这一格没被问过：{:#?}",
            one_pin[1].2
        );
        assert_eq!(at(&one_pin, 2), Check::Pass, "{:#?}", one_pin[2].2);

        // 腿没跑：三枚全是「没有问」，一枚也不红（红是给产品的话，不是给门的）。
        let silent = sel_rows(&[], subject);
        for row in &silent {
            assert_eq!(row.0, Check::Blocked, "{:#?}", row.2);
        }
    }

    /// 扇出的证人必须来自**别的窗**自己那张嘴，而且只有别的窗说话：主体那一腿已经在读
    /// `sel-*-self` 两格，再让它的 ring 报一次就是同一件事两枚数（#281）。所以这一枚判三件事：
    /// 两枚键各一处、gate 认的是「不是主体」那一面。
    #[test]
    fn the_other_window_reports_its_own_ring_and_the_subject_does_not() {
        let src = include_str!("../qml/PinWindow.qml");
        let on = src.matches("gaugeSay(\"sel-observe-on\"").count();
        let off = src.matches("gaugeSay(\"sel-observe-off\"").count();
        let gated = src
            .matches("String(win.gauge[4]) !== String(win.pinId)")
            .count();
        let ring = src.matches("visible: win.view.selected").count();
        assert_eq!(on, 1, "亮那一句只该有一处，读到 {on} 处");
        assert_eq!(off, 1, "灭那一句只该有一处，读到 {off} 处");
        assert_eq!(
            gated, 1,
            "这一族只许有一枚 gate，且必须是「不是主体」那一面，读到 {gated} 处"
        );
        assert_eq!(ring, 1, "选中状态只该有一处绑定，读到 {ring} 处");
        // 反证（#274）：裸 `sel-observe` 在本档有 4 枚——两枚键各一枚 emitter、外加日程表里
        // 两枚名字，而 gate 那一句一次也没提到它们。若判据收的是裸串，handler 被删空它照样绿。
        assert!(
            src.matches("sel-observe").count() > on + off,
            "裸 `sel-observe` 的枚数本就该大于两枚 emitter，这把尺才分得开键名与代码：{} vs {}",
            src.matches("sel-observe").count(),
            on + off
        );
    }

    /// §5.10.1 落进 QML 的那几只手势与落点，逐串现读 `PinWindow.qml`（引语要回盘上档案对
    /// #271/#341）。四枚判据各有各的失败形状：
    /// ① `selectPin` 只许一处落点（两处就是两套路，迟早一路是装饰）—— 量规那一段因此也走这
    /// 同一处（`win.selectSelf`），量的与按的是同一条语句；② Ctrl+单击这一支
    /// 必须读 `event.modifiers & Qt.ControlModifier`，滚轮那一路的 `wheel.modifiers` 不算证人
    /// （本轮之前 `Ctrl` 只在滚轮上被读过，那正是开工戳记下的那一枚陷阱）；③「点空白」与
    /// 「按 Esc」两扇门共用一只手，所以 `session.clearSelection(` 只出现一次；
    /// `clearSelectionAction.trigger()` 在场上有**六枚读者** —— 两扇门各一枚，量规那四腿各一枚
    /// （`sel` 腿收名单为 P128 那一族服务，`des` 腿收名单为 P134 那一族服务，`solo` 段前那一次为
    /// §5.11 的「空名单上按热键」那一臂把名单清空、段末那一次把本族点过的名收回 —— 四枚量规读者
    /// 都是「同一只手多出来的一个测量读者」，不是第三扇门）。第七枚才是多出来的一套路，而 Esc
    /// 那一门的 handler 单独点名（一枚 Shortcut 一枚 handler）；
    /// ④ 统一选中状态绑的是
    /// `win.view.selected` 那一枚值，窗侧不另存名单。
    #[test]
    fn the_qml_gives_selection_one_landing_and_two_doors_out() {
        let src = include_str!("../qml/PinWindow.qml");
        let routes = src.matches("win.session.selectPin(").count();
        let ctrl = src.matches("event.modifiers & Qt.ControlModifier").count();
        let wheel_ctrl = src.matches("wheel.modifiers & Qt.ControlModifier").count();
        let clear = src.matches("win.session.clearSelection(").count();
        let doors = src.matches("clearSelectionAction.trigger()").count();
        let esc_door = src
            .matches("onActivated: clearSelectionAction.trigger()")
            .count();
        let bound = src.matches("visible: win.view.selected").count();
        let esc = src.matches("StandardKey.Cancel").count();
        // 本轮踩过一次：`des` 那一腿的**注释**里抄了一遍那枚字面串，于是 `doors` 从四枚读成五枚。
        // 判据数的是读者，注释不是读者，所以这枚尺必须分得开两件事——加一枚反证，把「注释里被抄
        // 出来」这一种坏法钉死（#274/#433：只加一面，不放宽任何一枚既有数字）。
        let echoed = src
            .lines()
            .filter(|l| l.contains("clearSelectionAction.trigger()"))
            .filter(|l| l.trim_start().starts_with("//"))
            .count();
        assert_eq!(
            routes, 1,
            "选中的落点在 PinWindow.qml 里只该有一处，读到 {routes} 处"
        );
        assert_eq!(
            ctrl, 1,
            "Ctrl+单击这一支只该读一次 event.modifiers，读到 {ctrl} 处"
        );
        assert_eq!(
            wheel_ctrl, 1,
            "滚轮那一路是既有码，本轮不许动它，读到 {wheel_ctrl} 处"
        );
        assert_eq!(clear, 1, "退出多选只该有一只手，读到 {clear} 处");
        assert_eq!(
            doors, 6,
            "那一只手在场上的读者该是枚数分得清的六枚（空白门 / Esc 门 / sel 量规腿 / des 量规腿 / solo 段前那一次清名 / solo 段末那一次收回），读到 {doors} 处"
        );
        assert_eq!(
            echoed, 0,
            "那枚字面串在注释里每抄一次，这枚尺就多读出一枚假读者，读到 {echoed} 行"
        );
        assert_eq!(
            esc_door, 1,
            "Esc 那一门的 handler 只该有一枚，读到 {esc_door} 处"
        );
        assert_eq!(bound, 1, "选中状态只该有一处绑定，读到 {bound} 处");
        assert_eq!(esc, 1, "Esc 那一支只该写一遍，读到 {esc} 处");
        // 反证（#274）：这一族的尺必须读得出"没有"。裸 `selected` 在本档有 14 枚，全部属于
        // 截图遮罩的选区 —— 若判据收的是裸串，它会在桥上一枚新码都没加的时候先绿。
        assert!(
            src.matches("selected").count() > bound,
            "裸 `selected` 的枚数本就该大于一枚绑定，这把尺才分得开两族：{} vs {bound}",
            src.matches("selected").count()
        );
    }

    /// 选中变了要让**所有**贴图窗重读，这一手借的是既有那条 `redrawPins`，形状照
    /// `dialogTick` 那一族（main.qml:28-35 记下过：cxx-qt 不给 qproperty 驼峰化，
    /// `onSelectionEpochChanged` 场上没有那一枚信号，名字写错是**静默**的 #316）。
    /// 所以本枚判三件事：别名一处、handler 一处、而那个不存在的驼峰名一枚也不许在场。
    #[test]
    fn the_selection_epoch_fans_out_through_a_binding_not_a_misnamed_signal() {
        let main = include_str!("../qml/main.qml");
        let session = include_str!("session.rs");
        let alias = main
            .matches("readonly property int selectionTick: app.session.selection_epoch")
            .count();
        let handler = main
            .matches("onSelectionTickChanged: app.redrawPins()")
            .count();
        let wrong = main.matches("onSelectionEpochChanged").count();
        let prop = session
            .matches("#[qproperty(i32, selection_epoch)]")
            .count();
        assert_eq!(alias, 1, "扇出的别名只该有一处，读到 {alias} 处");
        assert_eq!(
            handler, 1,
            "handler 只该长在声明它的那个对象根上，读到 {handler} 处"
        );
        assert_eq!(
            wrong, 0,
            "那枚驼峰 notify 场上不存在，写进产品码就是静默失灵，读到 {wrong} 处"
        );
        assert_eq!(prop, 1, "qproperty 声明只该有一处，读到 {prop} 处");
        // 反证：绑的是**值**而不是一次调用 —— 写成 invokable 就是一次读死（P119 那两封信）。
        assert!(
            !main.contains("selectionTick()"),
            "别名若被当成函数调用，绑定就不会随 notify 重新求值"
        );
    }

    /// §5.10.2 那五枚行各自的十三面（#274/#440/#461）：判据一枚也不并，所以坏法一枚也只红一枚。
    ///
    /// 十三面：全好、批量被那只手偷偷降成单枚、空手也发号、搬动顺手换走一张纹理、反手四枚各自
    /// 坏一次（多搬一枚 ×2、落点没还、号少发一次）、反手四枚根本没跑、场上只有一枚贴图、纹理那
    /// 两枚不齐、段首落点缺席，以及腿根本没跑。前几面在真的产品里才会碰上，本档由 `--pin` 的量规
    /// 读；后几面是**造出来的行** —— 判据的形状不能只在它成立的那一趟里被看见，而「没有问」与
    /// 「答错」必须由两枚不同的结论分开（#453/#444）。
    ///
    /// 「降成单枚」那一面只改 `mov-batch` 一枚读数、号照旧走：这一格量的就是**报几枚**这一件事，
    /// 若把号也一起改，五枚会一起红，那一趟就分不出坏掉的是哪一件（#224 逐名不并成「若干」）。
    #[test]
    fn the_five_drag_rows_answer_apart_for_each_way_the_batch_can_fail() {
        let subject: PinId = 7;
        let legs = |pairs: &[(&str, &str)]| -> Vec<String> {
            pairs.iter().map(|(k, v)| format!("{k}|{v}")).collect()
        };
        let at = |rows: &[(Check, &'static str, String)], i: usize| rows[i].0;
        // 换一枚读数而不背它的下标：这一族自 P132 起有十五枚键，写死下标的反证会在别人插腿时
        // 悄悄改量另一枚行（#316 名字写错是静默的，下标写错也是）。
        let flip = |pairs: &[(&'static str, &'static str)],
                    key: &'static str,
                    value: &'static str|
         -> Vec<(&'static str, &'static str)> {
            pairs
                .iter()
                .map(|(k, v)| if *k == key { (*k, value) } else { (*k, *v) })
                .collect()
        };

        let good: [(&str, &str); 15] = [
            ("mov-pos-before", "100,80"),
            ("mov-epoch-before", "5"),
            ("mov-rev-before", "1"),
            ("mov-lone", "1"),
            ("mov-epoch-lone", "6"),
            ("mov-rev-lone", "1"),
            ("mov-ghost", "0"),
            ("mov-epoch-ghost", "6"),
            ("mov-batch-target", "9"),
            ("mov-batch", "2"),
            ("mov-batch-epoch", "7"),
            ("mov-undo-self", "1"),
            ("mov-undo-other", "1"),
            ("mov-epoch-undo", "9"),
            ("mov-pos-after", "100,80"),
        ];
        let downgraded = flip(&good, "mov-batch", "1");
        let ghost_spoke = flip(&good, "mov-epoch-ghost", "7");
        let repainted = flip(&good, "mov-rev-lone", "2");

        let only_one: Vec<(&str, &str)> = good
            .iter()
            .filter(|(k, _)| {
                !matches!(
                    *k,
                    "mov-batch-target"
                        | "mov-batch"
                        | "mov-batch-epoch"
                        | "mov-undo-self"
                        | "mov-undo-other"
                        | "mov-epoch-undo"
                        | "mov-pos-after"
                )
            })
            .copied()
            .collect();
        let mut no_batch = only_one.clone();
        no_batch.push(("mov-batch-target", "-1"));

        let all = mov_rows(&legs(&good), subject);
        let names: Vec<&str> = all.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, MOV_ROWS.to_vec(), "{names:?}");
        for row in &all {
            assert_eq!(row.0, Check::Pass, "好的一趟该五枚全绿：{:#?}", row.2);
        }

        // 名单两枚、拖其中一枚却只搬走一枚：§5.10.2 那一句「所有选中的贴图一起移动」就是这一枚红。
        let one = mov_rows(&legs(&downgraded), subject);
        assert_eq!(at(&one, 0), Check::Pass, "第一枚读的是自己那一拖");
        assert_eq!(at(&one, 1), Check::Fail, "{:#?}", one[1].2);
        assert_eq!(at(&one, 2), Check::Pass, "号那一路没坏，就不该跟着红");
        assert_eq!(at(&one, 3), Check::Pass, "纹理那一路也没坏");
        assert_eq!(at(&one, 4), Check::Pass, "反手那一问读的是自己那两拖");

        // 一次什么也没搬的手势也发号：全场为它重读一遍像素，这一枚红，其余照旧绿。
        let echo = mov_rows(&legs(&ghost_spoke), subject);
        assert_eq!(at(&echo, 0), Check::Pass);
        assert_eq!(at(&echo, 1), Check::Pass);
        assert_eq!(at(&echo, 2), Check::Fail, "{:#?}", echo[2].2);
        assert_eq!(at(&echo, 3), Check::Pass);
        assert_eq!(at(&echo, 4), Check::Pass);

        // 搬动顺手换走了一张纹理：位置被算进了内容的指纹，代价那一枚红，枚数与号都不跟着红。
        let redraw = mov_rows(&legs(&repainted), subject);
        assert_eq!(at(&redraw, 0), Check::Pass, "枚数与号都还对");
        assert_eq!(at(&redraw, 3), Check::Fail, "{:#?}", redraw[3].2);
        assert_eq!(at(&redraw, 4), Check::Pass, "落点那一问不读纹理号");

        // 反手多搬了一枚 / 落点没还回去 / 号少发一次：三面各红在第 5 问，前四问一枚也不跟着红。
        // 反手走在名单空了之后靠的就是「一次只搬被拖的那一扇」，交回 2 就是那一拖又按名单搬了两枚。
        for (key, value, face_name) in [
            ("mov-undo-self", "2", "本窗那一反手按名单搬了两枚"),
            ("mov-undo-other", "2", "另一枚那一反手也按名单搬了两枚"),
            ("mov-pos-after", "140,100", "落点没还回去"),
            ("mov-epoch-undo", "8", "反手少发一次号"),
        ] {
            let face = flip(&good, key, value);
            let rows = mov_rows(&legs(&face), subject);
            assert_eq!(at(&rows, 4), Check::Fail, "{face_name}：{:#?}", rows[4].2);
            for i in 0..4 {
                assert_eq!(
                    at(&rows, i),
                    Check::Pass,
                    "{face_name}：第 {i} 问不该跟着红"
                );
            }
        }

        // 反手四枚没跑：是「没有问」而不是「落点相等」——第 5 问的锚不全，读空的尺不许冒充绿。
        let mut no_undo: Vec<(&str, &str)> = good.to_vec();
        no_undo.retain(|(k, _)| {
            !matches!(
                *k,
                "mov-undo-self" | "mov-undo-other" | "mov-epoch-undo" | "mov-pos-after"
            )
        });
        let unasked_undo = mov_rows(&legs(&no_undo), subject);
        assert_eq!(
            at(&unasked_undo, 4),
            Check::Blocked,
            "反手不齐，这一格没被问过：{:#?}",
            unasked_undo[4].2
        );
        for i in 0..4 {
            assert_eq!(at(&unasked_undo, i), Check::Pass, "前四问读数齐，该照旧绿");
        }

        // 场上只有一枚贴图：第 2 与第 5 枚是「没有问」，不是红（#453）。
        let lone_only = mov_rows(&legs(&no_batch), subject);
        assert_eq!(at(&lone_only, 0), Check::Pass, "第一枚与场上几枚贴图无关");
        assert_eq!(
            at(&lone_only, 1),
            Check::Blocked,
            "没有别的窗可一起点名，这一格没被问过：{:#?}",
            lone_only[1].2
        );
        assert_eq!(at(&lone_only, 2), Check::Pass, "{:#?}", lone_only[2].2);
        assert_eq!(
            at(&lone_only, 3),
            Check::Pass,
            "纹理那一问不依赖场上几枚贴图"
        );
        assert_eq!(
            at(&lone_only, 4),
            Check::Blocked,
            "没有第二枚要抹，反手这一格没被问过：{:#?}",
            lone_only[4].2
        );

        // 纹理那两枚不齐：是「没有问」而不是「相等」（#440 一枚读空的尺不许冒充绿）。
        let mut half: Vec<(&str, &str)> = good.to_vec();
        half.retain(|(k, _)| *k != "mov-rev-before");
        let unasked = mov_rows(&legs(&half), subject);
        assert_eq!(
            at(&unasked, 3),
            Check::Blocked,
            "两枚号不齐，这一格没被问过：{:#?}",
            unasked[3].2
        );

        // 段首那枚落点缺席：这一问比的是「开头与结尾同一串字」，只剩一串时既不能说还了、也不能
        // 说没还，所以它是「没有问」而不是红（#444/#453）。
        let mut no_p0: Vec<(&str, &str)> = good.to_vec();
        no_p0.retain(|(k, _)| *k != "mov-pos-before");
        let unasked_pos = mov_rows(&legs(&no_p0), subject);
        assert_eq!(
            at(&unasked_pos, 4),
            Check::Blocked,
            "落点只剩结尾一串，这一格没被问过：{:#?}",
            unasked_pos[4].2
        );
        for i in 0..4 {
            assert_eq!(
                at(&unasked_pos, i),
                Check::Pass,
                "第 {i} 问不读落点，该照旧绿"
            );
        }

        // 腿没跑：五枚全是「没有问」，一枚也不红（红是给产品的话，不是给门的）。
        let silent = mov_rows(&[], subject);
        assert_eq!(silent.len(), 5, "{silent:?}");
        for row in &silent {
            assert_eq!(row.0, Check::Blocked, "{:#?}", row.2);
        }
    }

    /// §5.10.2 落进 QML 的那一手，逐串现读 `PinWindow.qml`（引语要回盘上档案对 #271/#341）。
    /// 四枚判据各有各的失败形状：① `dragPin` 只许一处落点（两处就是两套路，迟早一路是装饰）；
    /// ② 那一处落点的**六个读者**逐名点得出 —— 鼠标那一腿、量规那三腿，以及反手抹回落点那两腿，
    /// 各是一条具体的语句，而不是「大约几处」；③ 旧那一路 `view.dragMove` 必须一枚也不在场上
    /// （留着它就是第二套路，而第二套路不会有人读它）；④ 窗侧不许自己分「拖一枚」和「拖一批」。
    #[test]
    fn the_qml_gives_the_drag_one_landing_and_six_named_readers() {
        let src = include_str!("../qml/PinWindow.qml");
        let view = include_str!("pin_view.rs");
        let routes = src.matches("win.session.dragPin(").count();
        let def = src.matches("function dragTarget(").count();
        let mouse = src.matches("win.dragTarget(win.pinId, dx, dy)").count();
        let lone = src.matches("win.dragTarget(win.pinId, 17, 9)").count();
        let ghost = src.matches("win.dragTarget(top + 1, 5, 5)").count();
        let batch = src.matches("win.dragTarget(win.pinId, 23, 11)").count();
        let undo_self = src.matches("win.dragTarget(win.pinId, -40, -20)").count();
        let undo_other = src.matches("win.dragTarget(other, -23, -11)").count();
        let gone = src.matches("dragMove").count() + view.matches("dragMove").count();
        let second = src.matches("if (win.view.selected)").count();
        assert_eq!(
            routes, 1,
            "拖动的落点在 PinWindow.qml 里只该有一处，读到 {routes} 处"
        );
        assert_eq!(def, 1, "那一处落点只该被声明一次，读到 {def} 处");
        assert_eq!(
            (mouse, lone, ghost, batch, undo_self, undo_other),
            (1, 1, 1, 1, 1, 1),
            "六个读者各自都该点得出名字：鼠标 {mouse} / 一枚 {lone} / 没有的 {ghost} / 两枚 {batch} / 反手本窗 {undo_self} / 反手另一枚 {undo_other}"
        );
        assert_eq!(
            gone, 0,
            "旧那一路 `dragMove`（桥上的 invokable 与 QML 的调用）该一枚也不在，读到 {gone} 处"
        );
        assert_eq!(
            second, 0,
            "窗侧不许自己分批量与单枚——那一问只有 `state::drag_pin` 一处答案，读到 {second} 处"
        );
        // 反证（#274）：`dragPin` 这个词在本档比一枚落点多——注释里也叫它。若判据收的是裸串，
        // 落点被复制成两处它照样绿。
        assert!(
            src.matches("dragPin").count() > routes,
            "裸 `dragPin` 的枚数本就该大于一处落点，这把尺才分得开词名与代码：{} vs {routes}",
            src.matches("dragPin").count()
        );
    }

    /// 搬动了要让**所有**贴图窗重读自己该画在哪儿，借的还是 `dialogTick` / `selectionTick` 那条
    /// 已经跑通的腿（cxx-qt 不给 qproperty 驼峰化，`onMoveEpochChanged` 场上没有那一枚信号，
    /// 名字写错是**静默**的 #316）。三枚判据之外还有一枚尺读的是"只有一处做批量算术"：
    /// core 那手 `translate_selection` 在桥上的读者必须恰好一枚，且在 `state.rs` —— 它若被抬到
    /// `pin_view.rs`，就又是一扇窗自己决定搬几枚（而窗的章程说它不决定这件事）。
    #[test]
    fn the_drag_fans_out_through_a_binding_and_the_batch_is_counted_once() {
        let main = include_str!("../qml/main.qml");
        let session = include_str!("session.rs");
        let state = include_str!("state.rs");
        let view = include_str!("pin_view.rs");
        let alias = main
            .matches("readonly property int moveTick: app.session.move_epoch")
            .count();
        let handler = main.matches("onMoveTickChanged: app.redrawPins()").count();
        let wrong = main.matches("onMoveEpochChanged").count();
        let prop = session.matches("#[qproperty(i32, move_epoch)]").count();
        let hands = state.matches("self.set.translate_selection").count();
        let bare = state.matches("translate_selection").count();
        let orphan = view.matches("translate_selection").count()
            + view.matches(".move_by(").count()
            + view.matches("move_by").count();
        assert_eq!(alias, 1, "扇出的别名只该有一处，读到 {alias} 处");
        assert_eq!(
            handler, 1,
            "handler 只该长在声明它的那个对象根上，读到 {handler} 处"
        );
        assert_eq!(
            wrong, 0,
            "那枚驼峰 notify 场上不存在，写进产品码就是静默失灵，读到 {wrong} 处"
        );
        assert_eq!(prop, 1, "qproperty 声明只该有一处，读到 {prop} 处");
        assert_eq!(
            hands, 1,
            "core 那手 `translate_selection` 在桥上的产品读者只该有一处（`drag_pin`），读到 {hands} 处"
        );
        assert_eq!(
            orphan, 0,
            "`pin_view.rs` 不该再自己做批量算术，也不该留着没人读的 `move_by`，读到 {orphan} 处"
        );
        // 反证（#274）：裸词 `translate_selection` 在 `state.rs` 里比一枚调用多——注释也引它。
        // 若判据收的是裸串，第二处产品读者被加进来时这一枚照样绿。
        assert!(
            bare > hands,
            "裸词的枚数本就该大于一处调用，这把尺才分得开注释与代码：{bare} vs {hands}"
        );
        // 反证：绑的是**值**而不是一次调用 —— 写成 invokable 就是一次读死。
        assert!(
            !main.contains("moveTick()"),
            "别名若被当成函数调用，绑定就不会随 notify 重新求值"
        );
    }

    /// §5.10.3 的六枚判据各有各的失败形状：把批量降成单枚、空手也发号、阶梯那一步压根没动、
    /// 顺手重画一张纹理、归位没还回去、单枚那一格少发一次号——每一面只红自己那一枚，其余照旧绿。
    /// 好的一趟该六枚全绿。这是 `the_five_drag_rows_answer_apart_for_each_way_the_batch_can_fail`
    /// 那一族的另一员，一枚也不并（#281）。
    #[test]
    fn the_six_opacity_rows_answer_apart_for_each_way_the_batch_can_fail() {
        let subject: PinId = 7;
        let legs = |pairs: &[(&str, &str)]| -> Vec<String> {
            pairs.iter().map(|(k, v)| format!("{k}|{v}")).collect()
        };
        let at = |rows: &[(Check, &'static str, String)], i: usize| rows[i].0;
        let flip = |pairs: &[(&'static str, &'static str)],
                    key: &'static str,
                    value: &'static str|
         -> Vec<(&'static str, &'static str)> {
            pairs
                .iter()
                .map(|(k, v)| if *k == key { (*k, value) } else { (*k, *v) })
                .collect()
        };

        // 一趟「产品全对」的读数：单枚那一格从 100 钉到 50 再还回 100（号 3→5），ghost 交 0 且
        // 号不动，批量那一格从 100 钉到 50（号 5→6）、下一步 40、反手 50、归位 100（号 →9）。
        let good: [(&str, &str); 25] = [
            ("op-lone-start", "100"),
            ("op-lone-epoch-before", "3"),
            ("op-lone-set", "1"),
            ("op-lone-opacity", "50"),
            ("op-lone-restore", "1"),
            ("op-lone-restored", "100"),
            ("op-lone-epoch-end", "5"),
            ("op-epoch-ghost-before", "5"),
            ("op-ghost", "0"),
            ("op-epoch-ghost", "5"),
            ("mov-batch-target", "9"),
            ("op-start", "100"),
            ("op-epoch-before", "5"),
            ("op-rev-before", "2"),
            ("op-set-batch", "2"),
            ("op-opacity-set", "50"),
            ("op-epoch-set", "6"),
            ("op-rev-set", "2"),
            ("op-step-down", "2"),
            ("op-opacity-down", "40"),
            ("op-step-up", "2"),
            ("op-opacity-up", "50"),
            ("op-set-back", "2"),
            ("op-opacity-end", "100"),
            ("op-epoch-end", "9"),
        ];
        let downgraded = flip(&good, "op-set-batch", "1");
        let echo = flip(&good, "op-epoch-ghost", "6");
        let repainted = flip(&good, "op-rev-set", "3");
        let flat = flip(&good, "op-opacity-down", "50");
        let unreturned = flip(&good, "op-opacity-end", "50");
        let lone_echo = flip(&good, "op-lone-epoch-end", "4");

        let all = op_rows(&legs(&good), subject);
        let names: Vec<&str> = all.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, OP_ROWS.to_vec(), "{names:?}");
        for row in &all {
            assert_eq!(row.0, Check::Pass, "好的一趟该六枚全绿：{:#?}", row.2);
        }

        // 名单两枚、调其中一枚却只改一枚：§5.10.3 那一句「所有选中贴图同步更新透明度」就是这一枚红。
        let one = op_rows(&legs(&downgraded), subject);
        assert_eq!(at(&one, 0), Check::Pass, "第一枚读的是自己那一手");
        assert_eq!(at(&one, 1), Check::Fail, "{:#?}", one[1].2);
        for i in [2, 3, 4, 5] {
            assert_eq!(
                at(&one, i),
                Check::Pass,
                "第 {i} 问不该跟着红：{:#?}",
                one[i].2
            );
        }

        // 一次什么也没改的手势也发号：全场为它重读一遍，这一枚红，其余照旧绿。
        let noise = op_rows(&legs(&echo), subject);
        assert_eq!(at(&noise, 3), Check::Fail, "{:#?}", noise[3].2);
        for i in [0, 1, 2, 4, 5] {
            assert_eq!(at(&noise, i), Check::Pass, "第 {i} 问不读 ghost 那枚号");
        }

        // 调透明度顺手换走一张纹理：`opacity` 被算进了内容的指纹，代价那一枚红，枚数与号都不跟着红。
        let redraw = op_rows(&legs(&repainted), subject);
        assert_eq!(at(&redraw, 4), Check::Fail, "{:#?}", redraw[4].2);
        for i in [0, 1, 2, 3, 5] {
            assert_eq!(at(&redraw, i), Check::Pass, "第 {i} 问不读纹理号");
        }

        // 阶梯那一步没动：本窗重读到的还是同一级——那只手要么没迈步、要么那一声扇出没跑。
        let stuck = op_rows(&legs(&flat), subject);
        assert_eq!(at(&stuck, 2), Check::Fail, "{:#?}", stuck[2].2);
        for i in [0, 1, 3, 4, 5] {
            assert_eq!(at(&stuck, i), Check::Pass, "第 {i} 问不读阶梯那两枚");
        }

        // 归位没还回去：量规的腿把自己量的东西弄走了，第 6 问红而前五问照旧绿。
        let left = op_rows(&legs(&unreturned), subject);
        assert_eq!(at(&left, 5), Check::Fail, "{:#?}", left[5].2);
        for i in [0, 1, 2, 3, 4] {
            assert_eq!(at(&left, i), Check::Pass, "第 {i} 问不读段末那枚归位");
        }

        // 单枚那一格少发一次号：那一手改了本窗却没叫别的窗重读。
        let short = op_rows(&legs(&lone_echo), subject);
        assert_eq!(at(&short, 0), Check::Fail, "{:#?}", short[0].2);
        for i in [1, 2, 3, 4, 5] {
            assert_eq!(at(&short, i), Check::Pass, "第 {i} 问读的是批量那一族的号");
        }

        // 本窗原本就在这一腿要钉的那一级上：重读那一半天然相等，第 2 问是「没有问」而不是绿
        // （#440 一枚读空的尺不许冒充绿）。第 6 问读的是归位前后两串，仍判得。
        let pinned = flip(&good, "op-start", "50");
        let pinned = flip(&pinned, "op-opacity-end", "50");
        let vacuous = op_rows(&legs(&pinned), subject);
        assert_eq!(
            at(&vacuous, 1),
            Check::Blocked,
            "环境没给这一格拉开的余地：{:#?}",
            vacuous[1].2
        );
        assert_eq!(at(&vacuous, 5), Check::Pass, "归位那一问读的是自己那两串");

        // 场上只有一枚贴图：批量那十四枚一句也不说，四枚是「没有问」而不是红（#453）。
        let no_batch: Vec<(&str, &str)> = good
            .iter()
            .filter(|(k, _)| {
                !matches!(
                    *k,
                    "mov-batch-target"
                        | "op-start"
                        | "op-epoch-before"
                        | "op-rev-before"
                        | "op-set-batch"
                        | "op-opacity-set"
                        | "op-epoch-set"
                        | "op-rev-set"
                        | "op-step-down"
                        | "op-opacity-down"
                        | "op-step-up"
                        | "op-opacity-up"
                        | "op-set-back"
                        | "op-opacity-end"
                        | "op-epoch-end"
                )
            })
            .copied()
            .collect();
        let lone_only = op_rows(&legs(&no_batch), subject);
        assert_eq!(at(&lone_only, 0), Check::Pass, "第一问与场上几枚贴图无关");
        assert_eq!(at(&lone_only, 3), Check::Pass, "ghost 那一问也不依赖第二枚");
        for i in [1, 2, 4, 5] {
            assert_eq!(
                at(&lone_only, i),
                Check::Blocked,
                "第 {i} 问的读数没被说出来：{:#?}",
                lone_only[i].2
            );
        }

        // 锚没读出来：六枚全是「没有问」，一枚也不红（红是给产品的话，不是给门的）。
        let mut no_anchor: Vec<(&str, &str)> = good.to_vec();
        no_anchor.retain(|(k, _)| *k != "op-lone-epoch-before");
        let blind = op_rows(&legs(&no_anchor), subject);
        assert_eq!(blind.len(), 6, "{blind:?}");
        for row in &blind {
            assert_eq!(row.0, Check::Blocked, "{:#?}", row.2);
        }

        let silent = op_rows(&[], subject);
        assert_eq!(silent.len(), 6, "{silent:?}");
        for row in &silent {
            assert_eq!(row.0, Check::Blocked, "{:#?}", row.2);
        }
    }

    /// §5.10.3 落进 QML 的那两只手，逐串现读 `PinWindow.qml`（引语要回盘上档案对 #271/#341）。
    /// 四件事各判各的：① 两只落点各只一句（多一处就是两套路）；② 那两处落点的**十一枚读者**
    /// 逐名点得出——滚轮一枚、菜单三枚、量规七枚；③ 旧那一路 `view.opacityStep` / `view.applyOpacity`
    /// 与桥上的 `PinView` 那两手必须一枚也不在场上（留着就是第二条透明度路）；④ 窗侧不许自己
    /// 分「调一枚」与「调一批」。
    #[test]
    fn the_qml_gives_the_opacity_two_landings_and_eleven_named_readers() {
        let src = include_str!("../qml/PinWindow.qml");
        let view = include_str!("pin_view.rs");
        let set_landing = src.matches("win.session.setPinOpacity(").count();
        let step_landing = src.matches("win.session.stepPinOpacity(").count();
        let set_def = src.matches("function opacitySetTarget(").count();
        let step_def = src.matches("function opacityStepTarget(").count();
        let wheel = src
            .matches("win.opacityStepTarget(win.pinId, wheel.angleDelta.y > 0)")
            .count();
        let menu_down = src
            .matches("onTriggered: win.opacityStepTarget(win.pinId, false) }")
            .count();
        let menu_up = src
            .matches("onTriggered: win.opacityStepTarget(win.pinId, true) }")
            .count();
        let menu_reset = src
            .matches("onTriggered: win.opacitySetTarget(win.pinId, 100) }")
            .count();
        let g_set = src
            .matches("gaugeSay(\"op-lone-set\", String(win.opacitySetTarget(win.pinId, 50)))")
            .count();
        let g_restore = src
            .matches(
                "gaugeSay(\"op-lone-restore\", String(win.opacitySetTarget(win.pinId, opLoneStart)))",
            )
            .count();
        let g_ghost = src
            .matches("gaugeSay(\"op-ghost\", String(win.opacityStepTarget(top + 1, false)))")
            .count();
        let g_batch = src
            .matches("gaugeSay(\"op-set-batch\", String(win.opacitySetTarget(win.pinId, 50)))")
            .count();
        let g_down = src
            .matches("gaugeSay(\"op-step-down\", String(win.opacityStepTarget(win.pinId, false)))")
            .count();
        let g_up = src
            .matches("gaugeSay(\"op-step-up\", String(win.opacityStepTarget(win.pinId, true)))")
            .count();
        let g_back = src
            .matches("gaugeSay(\"op-set-back\", String(win.opacitySetTarget(win.pinId, opStart)))")
            .count();
        let gone = src.matches("view.opacityStep").count()
            + src.matches("view.applyOpacity").count()
            + view.matches("applyOpacity").count()
            + view.matches("apply_opacity").count()
            + view.matches("opacityStep").count()
            + view.matches("opacity_step").count();
        let second = src.matches("selection_count > 1").count();
        assert_eq!(
            set_landing, 1,
            "绝对值那一手在 PinWindow.qml 里只该有一处落点，读到 {set_landing} 处"
        );
        assert_eq!(
            step_landing, 1,
            "阶梯那一手只该有一处落点，读到 {step_landing} 处"
        );
        assert_eq!(
            (set_def, step_def),
            (1, 1),
            "两处落点各只该被声明一次：绝对值 {set_def} / 阶梯 {step_def}"
        );
        assert_eq!(
            (
                wheel, menu_down, menu_up, menu_reset, g_set, g_restore, g_ghost, g_batch, g_down,
                g_up, g_back
            ),
            (1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1),
            "十一枚读者各该点得出名字：滚轮 {wheel} / 菜单更透明 {menu_down} / 菜单更实 {menu_up} / 菜单 100% {menu_reset} / 量规单枚 {g_set} / 单枚归位 {g_restore} / ghost {g_ghost} / 批量 {g_batch} / 阶梯下 {g_down} / 阶梯上 {g_up} / 批量归位 {g_back}"
        );
        assert_eq!(
            gone, 0,
            "旧那一路（QML 的 `view.opacityStep` / `view.applyOpacity` 与 `PinView` 那两手）该一枚也不在，读到 {gone} 处"
        );
        assert_eq!(
            second, 0,
            "窗侧不许自己分批量与单枚——那一问只有 `state::batch_hand` 一处答案，读到 {second} 处"
        );
        // 反证（#274）：`opacity` 这个词在本档比两枚落点多得多——注释、绑定、读数键都叫它。
        // 若判据收的是裸串，落点被复制成两处它照样绿。
        assert!(
            src.matches("opacity").count() > set_landing + step_landing,
            "裸 `opacity` 的枚数本就该大于两枚落点，这把尺才分得开词名与代码：{} vs {}",
            src.matches("opacity").count(),
            set_landing + step_landing
        );
    }

    /// 调透明度要让**所有**贴图窗重读自己该多透，借的还是 `moveTick` / `selectionTick` 那条已经
    /// 跑通的腿（cxx-qt 不给 qproperty 驼峰化，`onOpacityEpochChanged` 场上没有那一枚信号，名字
    /// 写错是**静默**的 #316）。三枚扇出的判据之外还有两枚尺读的是「只有一处做批量算术」：
    /// `state.rs` 里那一句判据只许长在 [`crate::state::PinState::batch_hand`] 一格，而它在桥上的
    /// 读者枚数分得清——P128 那一族是三枚（拖 / 绝对值 / 阶梯），P134 把 §5.10.4 剩下那几只手抬过
    /// 桥之后是六枚（外加 重置 / `hand_targets` 那张名单表 / `destroy_answer` 那一格「影响数量」）。
    /// 读者变多是**这一族该多的事**，判据本身仍只有一处：它们若被抬到 `pin_view.rs`，就又是一扇
    /// 窗自己决定改几枚。
    #[test]
    fn the_opacity_change_fans_out_and_the_batch_is_counted_once() {
        let main = include_str!("../qml/main.qml");
        let session = include_str!("session.rs");
        let state = include_str!("state.rs");
        let view = include_str!("pin_view.rs");
        let alias = main
            .matches("readonly property int opacityTick: app.session.opacity_epoch")
            .count();
        let handler = main
            .matches("onOpacityTickChanged: app.redrawPins()")
            .count();
        let wrong = main.matches("onOpacityEpochChanged").count();
        let prop = session.matches("#[qproperty(i32, opacity_epoch)]").count();
        let bumps = session.matches("set_opacity_epoch(next)").count();
        let asked = state.matches("self.batch_hand(").count();
        let judge = state
            .matches("self.set.is_selected(id) && self.set.selection_count() > 1")
            .count();
        let abs_readers = state.matches("self.set.set_selection_opacity(").count();
        let step_readers = state.matches("self.set.step_selection_opacity(").count();
        let orphan = view.matches("selection_opacity").count();
        let bare = session.matches("opacity_epoch").count();
        assert_eq!(alias, 1, "扇出的别名只该有一处，读到 {alias} 处");
        assert_eq!(
            handler, 1,
            "handler 只该长在声明它的那个对象根上，读到 {handler} 处"
        );
        assert_eq!(
            wrong, 0,
            "那枚驼峰 notify 场上不存在，写进产品码就是静默失灵，读到 {wrong} 处"
        );
        assert_eq!(prop, 1, "qproperty 声明只该有一处，读到 {prop} 处");
        assert_eq!(
            bumps, 2,
            "两只手各发一次号（改了几枚 >0 才发），读到 {bumps} 处"
        );
        assert_eq!(
            asked, 6,
            "§5.10.1/2 那三只手（拖 / 绝对值 / 阶梯）加上 §5.10.4 这一族抬过桥的三只（重置 / \
             `hand_targets` / `destroy_answer`）共问同一枚判据，读到 {asked} 处"
        );
        assert_eq!(
            judge, 1,
            "「这一手算几枚」全仓只许写一次（#281），读到 {judge} 处"
        );
        assert_eq!(
            (abs_readers, step_readers),
            (1, 1),
            "core 那两只批量手在桥上的产品读者各一枚：绝对值 {abs_readers} / 阶梯 {step_readers}"
        );
        assert_eq!(
            orphan, 0,
            "`pin_view.rs` 不该留着批量那两手的读者，读到 {orphan} 处"
        );
        // 反证（#274）：`batch_hand` 这个**词名**在 `state.rs` 里比六枚读者多（定义一枚，散文若干），
        // 所以这枚尺收的是调用形状而不是词名——读者被复制成第七枚时它读得出。
        let bare_hand = state.matches("batch_hand").count();
        assert!(
            bare_hand > asked,
            "裸词 `batch_hand` 的枚数本就该大于读者枚数，这把尺才分得开词名与代码：{bare_hand} vs {asked}"
        );
        // 反证（#274）：裸词 `opacity_epoch` 在 `session.rs` 里比一枚声明多——注释与 impl 都引它。
        assert!(
            bare > prop,
            "裸词的枚数本就该大于一处声明，这把尺才分得开注释与代码：{bare} vs {prop}"
        );
        // 反证：绑的是**值**而不是一次调用 —— 写成 invokable 就是一次读死。
        assert!(
            !main.contains("opacityTick()"),
            "别名若被当成函数调用，绑定就不会随 notify 重新求值"
        );
    }

    /// §5.10.4「重置」的四问逐枚拆开判（#274）：一趟产品全对的读数为底，每一枚红都只由**它自己
    /// 那一问**的读数翻转，别的答案不动。八枚 `rst-lone-*`、三枚 ghost、四枚批量。
    #[test]
    fn the_four_reset_rows_answer_apart_for_each_way_the_hand_can_fail() {
        let legs = |pairs: &[(&str, &str)]| -> Vec<String> {
            pairs.iter().map(|(k, v)| format!("{k}|{v}")).collect()
        };
        let at = |rows: &[(Check, &'static str, String)], i: usize| rows[i].0;
        let flip = |pairs: &[(&'static str, &'static str)],
                    key: &'static str,
                    value: &'static str|
         -> Vec<(&'static str, &'static str)> {
            pairs
                .iter()
                .map(|(k, v)| if *k == key { (*k, value) } else { (*k, *v) })
                .collect()
        };

        // 一趟全对：单枚那一格从 100 往下迈到 90（号 3→4）、ghost 交 0 且号不动、批量那一格交 2
        // 且号只走一步（4→5）。
        let good: [(&str, &str); 15] = [
            ("rst-lone-base", "100"),
            ("rst-lone-turned", "90"),
            ("rst-lone-epoch-before", "3"),
            ("rst-lone-rev-before", "2"),
            ("rst-lone", "1"),
            ("rst-lone-zoom", "100"),
            ("rst-lone-rev", "2"),
            ("rst-lone-epoch-end", "4"),
            ("rst-epoch-ghost-before", "4"),
            ("rst-ghost", "0"),
            ("rst-epoch-ghost", "4"),
            ("rst-batch-target", "9"),
            ("rst-batch-epoch-before", "4"),
            ("rst-batch", "2"),
            ("rst-batch-epoch", "5"),
        ];

        let all = rst_rows(&legs(&good));
        let names: Vec<&str> = all.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, RST_ROWS.to_vec(), "{names:?}");
        for row in &all {
            assert_eq!(row.0, Check::Pass, "好的一趟该四枚全绿：{:#?}", row.2);
        }

        // 名单两枚、重置本窗却只改一枚：§5.10.4 那一句「批量」就是这一枚红，单枚那一格不动。
        let downgraded = flip(&good, "rst-batch", "1");
        let one = rst_rows(&legs(&downgraded));
        assert_eq!(at(&one, 0), Check::Pass, "第一问读的是自己那一手");
        assert_eq!(at(&one, 1), Check::Fail, "{:#?}", one[1].2);
        for i in [2, 3] {
            assert_eq!(
                at(&one, i),
                Check::Pass,
                "第 {i} 问不该跟着红：{:#?}",
                one[i].2
            );
        }

        // 一次批量发了两声（枚枚一声）：号那一问红，枚数那一问不动。
        let twice = flip(&good, "rst-batch-epoch", "6");
        let echo = rst_rows(&legs(&twice));
        assert_eq!(at(&echo, 1), Check::Fail, "{:#?}", echo[1].2);
        assert_eq!(at(&echo, 2), Check::Pass, "ghost 那一格的号不读批量段");

        // 那一手没把手势改过的那一档还回来：第一枚红，纹理号那一枚照旧绿（两问是两件事）。
        let kept = flip(&good, "rst-lone-zoom", "90");
        let stuck = rst_rows(&legs(&kept));
        assert_eq!(at(&stuck, 0), Check::Fail, "{:#?}", stuck[0].2);
        assert_eq!(at(&stuck, 3), Check::Pass, "归位与否不读纹理号那一问");

        // 真改动没发号：`main.qml` 的 `resetTick` 就没读者，别的窗不会重读自己。
        let silent_epoch = flip(&good, "rst-lone-epoch-end", "3");
        let nofan = rst_rows(&legs(&silent_epoch));
        assert_eq!(at(&nofan, 0), Check::Fail, "{:#?}", nofan[0].2);

        // 空手也发号（ghost 那一格）：第三枚红，其余不动。
        let ghost_spoke = flip(&good, "rst-epoch-ghost", "5");
        let noise = rst_rows(&legs(&ghost_spoke));
        assert_eq!(at(&noise, 2), Check::Fail, "{:#?}", noise[2].2);
        for i in [0, 1, 3] {
            assert_eq!(at(&noise, i), Check::Pass, "第 {i} 问不读 ghost 那枚号");
        }

        // 重置顺手换走一张纹理：`zoom` 被算进了内容的指纹，代价那一枚红，枚数与号都不跟着红。
        let repainted = flip(&good, "rst-lone-rev", "3");
        let cost = rst_rows(&legs(&repainted));
        assert_eq!(at(&cost, 3), Check::Fail, "{:#?}", cost[3].2);
        for i in [0, 1, 2] {
            assert_eq!(at(&cost, i), Check::Pass, "第 {i} 问不读纹理号那一问");
        }

        // 缩放贴在下限、那一档迈不动：环境没给拉开的余地，第一枚读 Blocked 而不是红（#433）。
        let flat = flip(&good, "rst-lone-turned", "100");
        let floor = rst_rows(&legs(&flat));
        assert_eq!(at(&floor, 0), Check::Blocked, "{:#?}", floor[0].2);
        for i in [1, 2, 3] {
            assert_eq!(
                at(&floor, i),
                Check::Pass,
                "第 {i} 问不受那一档迈不迈得动影响"
            );
        }

        // 场上只有一枚贴图：批量那四枚一句也不说，第二枚读 Blocked，另外三枚照旧绿（#453）。
        let no_batch: Vec<(&str, &str)> = good
            .iter()
            .filter(|(k, _)| !k.starts_with("rst-batch"))
            .copied()
            .collect();
        let lone_only = rst_rows(&legs(&no_batch));
        assert_eq!(at(&lone_only, 1), Check::Blocked, "{:#?}", lone_only[1].2);
        for i in [0, 2, 3] {
            assert_eq!(at(&lone_only, i), Check::Pass, "第 {i} 问读的是自己那几枚");
        }

        // 一趟谁也没问：四枚各占一行 Blocked，且每一行都说得清为什么。
        let silent = rst_rows(&[]);
        let names: Vec<&str> = silent.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, RST_ROWS.to_vec(), "{names:?}");
        for (grade, name, detail) in &silent {
            assert_eq!(*grade, Check::Blocked, "{name} 没被问过，不该算红");
            assert!(!detail.is_empty(), "{name} 那一格连为什么都没说");
        }
    }

    /// §5.9.18「销毁 / 撤销 / 影响数量」的五问逐枚拆开判（#274）：好读数为底，每一枚红只由自己
    /// 那一问翻转。这五问分别是**表**、**字面**、**按下去**、**空账不许清**、**回头路**。
    #[test]
    fn the_five_destroy_rows_answer_apart_for_each_way_the_ledger_can_fail() {
        let subject: PinId = 7;
        let legs = |pairs: &[(&str, &str)]| -> Vec<String> {
            pairs.iter().map(|(k, v)| format!("{k}|{v}")).collect()
        };
        let at = |rows: &[(Check, &'static str, String)], i: usize| rows[i].0;
        let flip = |pairs: &[(&'static str, &'static str)],
                    key: &'static str,
                    value: &'static str|
         -> Vec<(&'static str, &'static str)> {
            pairs
                .iter()
                .map(|(k, v)| if *k == key { (*k, value) } else { (*k, *v) })
                .collect()
        };

        let good: [(&str, &str); 18] = [
            ("des-target-a", "8"),
            ("des-target-b", "9"),
            ("des-pile-before", "4"),
            ("des-reach-self", "1"),
            ("des-reach-batch", "2"),
            ("des-menu-shape", "open=true;reach=2;undo=false"),
            ("des-menu-label", "销毁（影响 2 枚）"),
            ("des-undo-absent", "false"),
            ("des-batch", "2"),
            ("des-pile-gone", "2"),
            ("des-trashed", "2"),
            ("des-undo-shown", "true"),
            ("des-ghost", "0"),
            ("des-trashed-kept", "2"),
            ("des-undo", "2"),
            ("des-pile-back", "4"),
            ("des-trashed-after", "0"),
            ("des-undo-again", "0"),
        ];

        let all = des_rows(&legs(&good), subject);
        let names: Vec<&str> = all.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, DES_ROWS.to_vec(), "{names:?}");
        for row in &all {
            assert_eq!(row.0, Check::Pass, "好的一趟该五枚全绿：{:#?}", row.2);
        }

        // 字面停在进门那个初值上：`onAboutToShow` 没跑过，那一枚数没印出来。表与手都照旧绿——
        // 这一枚红说的是菜单那一行，不是那只手。
        let unprinted = flip(&good, "des-menu-label", "销毁（影响 1 枚）");
        let stale = des_rows(&legs(&unprinted), subject);
        assert_eq!(at(&stale, 0), Check::Pass, "表那一问不读字面");
        assert_eq!(at(&stale, 1), Check::Fail, "{:#?}", stale[1].2);
        for i in [2, 3, 4] {
            assert_eq!(at(&stale, i), Check::Pass, "第 {i} 问不读字面那一问");
        }

        // 名单两枚、按下去只带走一枚：第三枚红。
        let downgraded = flip(&good, "des-batch", "1");
        let one = des_rows(&legs(&downgraded), subject);
        assert_eq!(at(&one, 2), Check::Fail, "{:#?}", one[2].2);
        for i in [0, 1, 3, 4] {
            assert_eq!(at(&one, i), Check::Pass, "第 {i} 问不读带走的枚数");
        }

        // 堆没跟着少：那只手报的数与场上真的少了的那些不一致，第三枚红。
        let notgone = flip(&good, "des-pile-gone", "4");
        let lying = des_rows(&legs(&notgone), subject);
        assert_eq!(at(&lying, 2), Check::Fail, "{:#?}", lying[2].2);

        // 一次落空的点击把撤销的账清掉了：用户点了一枚不存在的贴图，回头路没了。第四枚红，
        // 第五枚跟着红是**应该**的——它的判据读的就是账上欠着的那些枚。
        let wiped = flip(&good, "des-trashed-kept", "0");
        let lost = des_rows(&legs(&wiped), subject);
        assert_eq!(at(&lost, 3), Check::Fail, "{:#?}", lost[3].2);

        // 撤销回来少了一枚：第五枚红，第四枚不动。
        let partial = flip(&good, "des-undo", "1");
        let short = des_rows(&legs(&partial), subject);
        assert_eq!(at(&short, 4), Check::Fail, "{:#?}", short[4].2);
        assert_eq!(at(&short, 3), Check::Pass, "第三问与第五问读的是不同的手");

        // 账空了还能再撤销一次（把同一批还第二遍）：第五枚红。
        let twice = flip(&good, "des-undo-again", "2");
        let greedy = des_rows(&legs(&twice), subject);
        assert_eq!(at(&greedy, 4), Check::Fail, "{:#?}", greedy[4].2);

        // 账上一枚也不欠时那一行还是出现了：那是菜单里的一枚死行（#433 那一族）。
        let shown = flip(&good, "des-undo-absent", "true");
        let dead = des_rows(&legs(&shown), subject);
        assert_eq!(at(&dead, 4), Check::Fail, "{:#?}", dead[4].2);
        assert_eq!(
            at(&dead, 3),
            Check::Pass,
            "那一行出不出与 ghost 那一手是两件事"
        );

        // 证人点中了本窗自己：说这些话的窗不能正是被带走的那一扇，第一枚读 Blocked 而不是红。
        let speaker = flip(&good, "des-target-a", "7");
        let self_taken = des_rows(&legs(&speaker), subject);
        assert_eq!(at(&self_taken, 0), Check::Blocked, "{:#?}", self_taken[0].2);

        // 场上凑不出两枚非本窗的证人：整段一句也没说，五枚各占一行 Blocked（#453）。
        let silent = des_rows(&[], subject);
        let names: Vec<&str> = silent.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, DES_ROWS.to_vec(), "{names:?}");
        for (grade, name, detail) in &silent {
            assert_eq!(*grade, Check::Blocked, "{name} 没被问过，不该算红");
            assert!(!detail.is_empty(), "{name} 那一格连为什么都没说");
        }
    }

    /// §5.11 独显的七问逐枚拆开判（#274）：好读数为底，每一枚的红只由自己那一问翻转。这七问分别是
    /// **那本账**、**屏幕上那一句话**、**菜单上那一行字面**、**同一枚手势的第二下**、**本窗的一进
    /// 一出**、**空名单上的一记热键**、**一枚场上没有的 id**。
    ///
    /// 同一批读数配两档 `dim` 各判一遍，因为「压暗还是隐藏」是配置键 `pin.solo_dim_opacity` 答的：
    /// 尺若只读窗递上来的 `solo-dims`，产品读错配置就没人说（#274 的反面）。两档的好趟各绿七枚，
    /// 而把隐藏那一档的读数交给压暗那一把尺，红的必须正是读配置的那三枚。
    #[test]
    fn the_seven_solo_rows_answer_apart_for_each_way_the_ledger_can_fail() {
        let subject: PinId = 7;
        let legs = |pairs: &[(&str, &str)]| -> Vec<String> {
            pairs.iter().map(|(k, v)| format!("{k}|{v}")).collect()
        };
        let at = |rows: &[(Check, &'static str, String)], i: usize| rows[i].0;
        let flip = |pairs: &[(&'static str, &'static str)],
                    key: &'static str,
                    value: &'static str|
         -> Vec<(&'static str, &'static str)> {
            pairs
                .iter()
                .map(|(k, v)| if *k == key { (*k, value) } else { (*k, *v) })
                .collect()
        };

        // 压暗那一档：`pin.solo_dim_opacity` = 35，场上四枚、名单空着点本窗。
        let dimmed: [(&str, &str); 34] = [
            ("solo-dims", "true"),
            ("solo-pile", "4"),
            ("solo-reach-self", "3"),
            ("solo-epoch-before", "2"),
            ("solo-enter", "3"),
            ("solo-epoch-enter", "3"),
            ("solo-kept", "1"),
            ("solo-hidden-self", "false"),
            ("solo-live-self", "true"),
            ("solo-said", "已进入独显：其余 3 枚压暗"),
            ("solo-menu-shape", "open=true;reach=0;way=压暗"),
            ("solo-menu-label", "退出独显"),
            ("solo-exit", "3"),
            ("solo-epoch-exit", "4"),
            ("solo-kept-after", "0"),
            ("solo-target-other", "8"),
            ("solo-op-start", "100"),
            ("solo-enter-other", "3"),
            ("solo-hidden", "false"),
            ("solo-live", "true"),
            ("solo-opacity", "35"),
            ("solo-epoch-other", "5"),
            ("solo-exit-other", "3"),
            ("solo-op-end", "100"),
            ("solo-hidden-end", "false"),
            ("solo-live-end", "true"),
            ("solo-refuse-before", "6"),
            ("solo-refuse", "0"),
            ("solo-refuse-epoch", "6"),
            ("solo-refuse-kept", "0"),
            ("solo-refuse-said", "独显没做成：先选中至少一枚贴图"),
            ("solo-ghost", "0"),
            ("solo-epoch-ghost", "6"),
            ("solo-ghost-kept", "0"),
        ];

        let all = solo_rows(&legs(&dimmed), subject, 35);
        let names: Vec<&str> = all.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, SOLO_ROWS.to_vec(), "{names:?}");
        for row in &all {
            assert_eq!(
                row.0,
                Check::Pass,
                "压暗那一档好的一趟该七枚全绿：{:#?}",
                row.2
            );
        }

        // 同一批读数交给隐藏那一把尺：只有不读配置的四枚还绿，读配置的三枚必须红。
        let wrong_mode = solo_rows(&legs(&dimmed), subject, 0);
        for i in [0, 3, 5, 6] {
            assert_eq!(at(&wrong_mode, i), Check::Pass, "第 {i} 问不读配置键");
        }
        for i in [1, 2, 4] {
            assert_eq!(at(&wrong_mode, i), Check::Fail, "{:#?}", wrong_mode[i].2);
        }

        // 账不平：留着两枚而带走的还是三枚，场上就多出来一枚没人管的窗。第一枚红，第二枚不读账。
        let unbalanced = flip(&dimmed, "solo-kept", "2");
        let off = solo_rows(&legs(&unbalanced), subject, 35);
        assert_eq!(at(&off, 0), Check::Fail, "{:#?}", off[0].2);
        assert_eq!(at(&off, 1), Check::Pass, "屏幕上那句话不读留着的枚数");

        // 菜单上印的数与那只手答的数各走一枚：窗自己数了一遍（#281 那一族）。第一枚红。
        let stale_reach = flip(&dimmed, "solo-reach-self", "1");
        let apart = solo_rows(&legs(&stale_reach), subject, 35);
        assert_eq!(at(&apart, 0), Check::Fail, "{:#?}", apart[0].2);
        for i in [1, 2, 3, 4, 5, 6] {
            assert_eq!(at(&apart, i), Check::Pass, "第 {i} 问不读那一臂的答案");
        }

        // 留着的那一枚被告知自己被带走了：桥侧扇出的那一把伸进了 `kept` 里。第一枚红。
        let self_taken = flip(&dimmed, "solo-hidden-self", "true");
        let lied = solo_rows(&legs(&self_taken), subject, 35);
        assert_eq!(at(&lied, 0), Check::Fail, "{:#?}", lied[0].2);

        // 屏幕上那句话里没有它自己报出的那个数：托盘与提示那一臂与那只手脱了钩。第二枚红。
        let mute = flip(&dimmed, "solo-said", "已进入独显");
        let unsaid = solo_rows(&legs(&mute), subject, 35);
        assert_eq!(at(&unsaid, 1), Check::Fail, "{:#?}", unsaid[1].2);
        for i in [0, 2, 3, 4, 5, 6] {
            assert_eq!(at(&unsaid, i), Check::Pass, "第 {i} 问不读屏幕上那句话");
        }

        // 独显开着而菜单那一行还写着「独显（压暗 3 枚）」：退出那一支印了一枚桥侧答不出的数。
        let not_renamed = flip(&dimmed, "solo-menu-label", "独显（压暗 3 枚）");
        let still = solo_rows(&legs(&not_renamed), subject, 35);
        assert_eq!(at(&still, 2), Check::Fail, "{:#?}", still[2].2);

        // 菜单缓存里那一格在独显开着时没答 0：与上面那一枚是同一句病，长在桥侧而不是窗侧。
        let counted = flip(&dimmed, "solo-menu-shape", "open=true;reach=3;way=压暗");
        let doubled = solo_rows(&legs(&counted), subject, 35);
        assert_eq!(at(&doubled, 2), Check::Fail, "{:#?}", doubled[2].2);

        // 第二下放回四枚而不是三枚：`exit_solo` 去摸了本来就没被带走的那一扇（规则②那句
        // 「而不是无条件显示全部」在这一格里第一次有了数上的形状）。第四枚红。
        let showed_all = flip(&dimmed, "solo-exit", "4");
        let allback = solo_rows(&legs(&showed_all), subject, 35);
        assert_eq!(at(&allback, 3), Check::Fail, "{:#?}", allback[3].2);
        assert_eq!(at(&allback, 0), Check::Pass, "第一问读的是第一下");

        // 退出之后账上还写着留着一枚：屏幕上那一行会接着说「退出独显」，用户再按一次就走进
        // 第二次没被同意的独显。第四枚红。
        let leftover = flip(&dimmed, "solo-kept-after", "1");
        let uncleared = solo_rows(&legs(&leftover), subject, 35);
        assert_eq!(at(&uncleared, 3), Check::Fail, "{:#?}", uncleared[3].2);

        // 本窗被压到 60 而不是配置里那一格：那一档压暗读的是空气。第五枚红。
        let off_key = flip(&dimmed, "solo-opacity", "60");
        let misdimmed = solo_rows(&legs(&off_key), subject, 35);
        assert_eq!(at(&misdimmed, 4), Check::Fail, "{:#?}", misdimmed[4].2);

        // 出等于进那一格被打破：退出之后本窗停在 80，用户自己调过的那一级被默认那一档盖掉了。
        let not_back = flip(&dimmed, "solo-op-end", "80");
        let uncovered = solo_rows(&legs(&not_back), subject, 35);
        assert_eq!(at(&uncovered, 4), Check::Fail, "{:#?}", uncovered[4].2);
        assert_eq!(
            at(&uncovered, 3),
            Check::Pass,
            "第四问读的是枚数与账，不读这一级"
        );

        // 证人就是本窗自己：说这些话的窗不能正是被带走的那一扇，第五枚读 Blocked 而不是红。
        let speaker = flip(&dimmed, "solo-target-other", "7");
        let self_witness = solo_rows(&legs(&speaker), subject, 35);
        assert_eq!(
            at(&self_witness, 4),
            Check::Blocked,
            "{:#?}",
            self_witness[4].2
        );

        // 进门那一级已经就等于压暗那一格：压没压读不出差别，第五枚 Blocked（拿没动过的读数
        // 冒充绿是 #433 那一族）。
        let flat_start = flip(&dimmed, "solo-op-start", "35");
        let flat = flip(&flat_start, "solo-op-end", "35");
        let indistinguishable = solo_rows(&legs(&flat), subject, 35);
        assert_eq!(
            at(&indistinguishable, 4),
            Check::Blocked,
            "{:#?}",
            indistinguishable[4].2
        );

        // 空名单上那记热键带走三枚：步骤 1 那句「必须先选中至少一枚贴图」没被守住。第六枚红。
        let acted = flip(&dimmed, "solo-refuse", "3");
        let refused = solo_rows(&legs(&acted), subject, 35);
        assert_eq!(at(&refused, 5), Check::Fail, "{:#?}", refused[5].2);

        // 拒绝的那一次屏幕上说的却是成功：一枚 bool 装不下的那一件事在这一格里可见。
        let wrong_said = flip(&dimmed, "solo-refuse-said", "已进入独显：其余 3 枚压暗");
        let lying_refusal = solo_rows(&legs(&wrong_said), subject, 35);
        assert_eq!(
            at(&lying_refusal, 5),
            Check::Fail,
            "{:#?}",
            lying_refusal[5].2
        );

        // 场上没有的那一枚被答成带走一枚：`hand_targets` 那面挡空的墙塌了。第七枚红。
        let ghost_took = flip(&dimmed, "solo-ghost", "1");
        let invented = solo_rows(&legs(&ghost_took), subject, 35);
        assert_eq!(at(&invented, 6), Check::Fail, "{:#?}", invented[6].2);

        // 落空的那一次把某枚记成了「留着的一枚」：独显开着却无人知晓。第七枚红。
        let ghost_kept = flip(&dimmed, "solo-ghost-kept", "1");
        let remembered = solo_rows(&legs(&ghost_kept), subject, 35);
        assert_eq!(at(&remembered, 6), Check::Fail, "{:#?}", remembered[6].2);

        // 场上只剩本窗一枚：没有别枚可被带走，读账、读那句话、读第二下的三枚 Blocked 而不是红
        // （#453——没问出口的行不算赢，也不算输）。
        let alone = flip(&dimmed, "solo-pile", "1");
        let lone = solo_rows(&legs(&alone), subject, 35);
        for i in [0, 1, 3] {
            assert_eq!(
                at(&lone, i),
                Check::Blocked,
                "{i}：场上只一枚时这一格没有问"
            );
            assert!(!lone[i].2.is_empty(), "{i} 那一格连为什么都没说");
        }

        // 隐藏那一档的好趟：`solo_dim_opacity` = 0，被带走的窗是藏起来而不是压暗，而它自己的
        // 不透明度一动不动。七枚同样全绿。
        let hid_dims = flip(&dimmed, "solo-dims", "false");
        let hid_said = flip(&hid_dims, "solo-said", "已进入独显：其余 3 枚隐藏");
        let hid_shape = flip(&hid_said, "solo-menu-shape", "open=true;reach=0;way=隐藏");
        let hid_hidden = flip(&hid_shape, "solo-hidden", "true");
        let hid_live = flip(&hid_hidden, "solo-live", "false");
        let hid_opacity = flip(&hid_live, "solo-opacity", "100");
        let hidden = solo_rows(&legs(&hid_opacity), subject, 0);
        let hid_names: Vec<&str> = hidden.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(hid_names, SOLO_ROWS.to_vec(), "{hid_names:?}");
        for row in &hidden {
            assert_eq!(
                row.0,
                Check::Pass,
                "隐藏那一档好的一趟该七枚全绿：{:#?}",
                row.2
            );
        }
        // 反证（#274）：这一批读数的不透明度一动不动，于是把尺换成压暗那一档就必须红第五枚——
        // 「没动」在隐藏那一档是对的答案，在压暗那一档不是。
        let hidden_on_dim = solo_rows(&legs(&hid_opacity), subject, 35);
        assert_eq!(
            at(&hidden_on_dim, 4),
            Check::Fail,
            "{:#?}",
            hidden_on_dim[4].2
        );

        // 四臂一句也没说（`victim` 凑不齐两枚 ⇒ 整段不跑）：七枚各占一行 Blocked。
        let silent = solo_rows(&[], subject, 35);
        let silent_names: Vec<&str> = silent.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(silent_names, SOLO_ROWS.to_vec(), "{silent_names:?}");
        for (grade, name, detail) in &silent {
            assert_eq!(*grade, Check::Blocked, "{name} 没被问过，不该算红");
            assert!(!detail.is_empty(), "{name} 那一格连为什么都没说");
        }
    }

    /// §5.10.4 那四只手在源码里的形状：一只落点、若干枚具名读者、一枚判据、零第二份。
    ///
    /// 这一格量的是**结构**而不是行为（行为由上面两枚真机判据与 `state.rs` selftest 量），它挡的
    /// 是这一族最容易复发的那一件：窗自己再数一遍名单（`selection_count > 1` 的第二处）、或那只
    /// 视图列里遗留的第二只「重置」手。P134 之前「重置视图」叫的正是后者
    /// （`win.view.resetView()` —— 名单对它没有分量），所以 `pin_view.rs` 里那两枚必须读 0：
    /// 删干净了才算抬过桥，留一枚「以后可能用得上」的 invokable 就是留第二份答案。
    #[test]
    fn the_four_view_hands_have_one_landing_each_and_no_second_judgement() {
        let src = include_str!("../qml/PinWindow.qml");
        let main = include_str!("../qml/main.qml");
        let view = include_str!("pin_view.rs");
        let state = include_str!("state.rs");
        let session = include_str!("session.rs");

        // 四只手的落点：每只在 QML 里只许出现一次（多一处就是第二条路绕过了那张表）。
        let reset_landing = src.matches("win.session.resetPin(").count();
        let destroy_landing = src.matches("win.session.destroyPin(").count();
        let undo_landing = src.matches("win.session.undoDestroy(").count();
        let close_landing = src.matches("win.session.closePin(").count();
        // 菜单那一枚入口（产品对用户的那扇门）与量规的读者要分得开：门各一枚，读者各若干。
        let reset_door = src
            .matches("onTriggered: win.resetTarget(win.pinId) }")
            .count();
        let destroy_door = src
            .matches("onTriggered: win.destroyTarget(win.pinId) }")
            .count();
        let undo_door = src
            .matches("onTriggered: win.undoDestroyTarget() }")
            .count();
        let close_door = src
            .matches("onTriggered: win.session.closePin(win.pinId) }")
            .count();
        let reset_readers = src.matches("win.resetTarget(").count();
        let destroy_readers = src.matches("win.destroyTarget(").count();
        let undo_readers = src.matches("win.undoDestroyTarget(").count();
        // 「影响数量」那一问：门是 `onAboutToShow` 那一行，量规另问两枚。
        let reach_asks = src.matches("win.session.destroyCountFor(").count();

        assert_eq!(
            reset_landing, 1,
            "重置那一手只该有一处落点，读到 {reset_landing} 处"
        );
        assert_eq!(
            destroy_landing, 1,
            "销毁那一手只该有一处落点，读到 {destroy_landing} 处"
        );
        assert_eq!(
            undo_landing, 1,
            "撤销那一手只该有一处落点，读到 {undo_landing} 处"
        );
        assert_eq!(
            close_landing, 2,
            "关闭那一手是一枚门加一段腿，读到 {close_landing} 处"
        );
        for (name, door) in [
            ("重置视图", reset_door),
            ("销毁", destroy_door),
            ("撤销", undo_door),
            ("关闭", close_door),
        ] {
            assert_eq!(door, 1, "菜单里「{name}」那一项只该点一次，读到 {door} 次");
        }
        // 反证（#274）：读者必须**多过**门，否则这一格只是在重复「门存在」那一枚尺的答案；
        // 而门也必须在 —— 少了门就是量规自己发明了那一只手。
        assert!(
            reset_readers > reset_door,
            "量规那一族没替重置多问一遍：{reset_readers} vs {reset_door}"
        );
        assert!(
            destroy_readers > destroy_door,
            "量规那一族没替销毁多问一遍：{destroy_readers} vs {destroy_door}"
        );
        assert!(
            undo_readers > undo_door,
            "量规那一族没替撤销多问一遍：{undo_readers} vs {undo_door}"
        );
        assert!(
            reach_asks > close_door,
            "字面上那个数只被门问过、没被量规问过，那一枚就是自证：{reach_asks}"
        );

        // 视图那一列把「重置」交回去了：桥侧那一手是唯一的重置，`pin_view.rs` 里一个也不留。
        let orphan = view.matches("resetView").count() + view.matches("reset_view").count();
        assert_eq!(
            orphan, 0,
            "重置视图已抬到 `state::reset_pin`，视图列里不该留第二只手：{orphan}"
        );
        // 窗不数名单：第二份判据一旦出现，`batch_hand` 那句话就有了两个答法。
        let second = src.matches("selection_count > 1").count();
        assert_eq!(second, 0, "窗里不许有第二份「算不算批量」：{second}");

        // 判据与那张表各写一次：`batch_hand` 一处声明六处调用（搬动、迈一档、绝对值、重置、
        // `hand_targets`、影响数量），`hand_targets` 一处声明三处调用（关闭、影响数量、销毁）。
        assert_eq!(
            state.matches("batch_hand(").count(),
            7,
            "判据那一枚在 `state.rs` 里是「一处声明 + 六处读者」，读到不同"
        );
        assert_eq!(
            state.matches("hand_targets(").count(),
            5,
            "「带走哪几枚」那张表在 `state.rs` 里是「一处声明 + 四处读者」（关闭、影响数量、销毁、独显），读到不同"
        );
        // 号各归各的手：`reset_epoch` 只有重置那一格发，`trashed` 只有 `refresh` 那一格写。
        assert_eq!(
            session.matches("set_reset_epoch").count(),
            1,
            "销毁与撤销不发重置的号（成员走的是 `count`）"
        );
        assert_eq!(
            session.matches("set_trashed").count(),
            1,
            "欠着几枚撤销只有一处写：`refresh` 从状态里读回来"
        );
        // #316：cxx-qt 不给驼峰 qproperty 的 notify，绑的是**值**，一次也不许当函数调用。
        assert_eq!(
            main.matches("readonly property int resetTick: app.session.reset_epoch")
                .count(),
            1,
            "重置那一号在 `main.qml` 里是一枚绑值的别名"
        );
        assert_eq!(
            main.matches("onResetTickChanged: app.redrawPins()").count(),
            1,
            "那一声明的读者只有 `redrawPins()` 一处"
        );
        assert!(
            !main.contains("resetTick()"),
            "别名若被当成函数调用，绑定就不会随 notify 重新求值"
        );
    }

    /// §5.9.5–§5.9.8 的七枚判据各有各的失败形状：一次迈两档、按了不涨号、满一圈没回到进门的
    /// 尺寸、翻转没翻、翻转没还回去、两下涨了不止一格、菜单词被改写、键帽注册了不止一枚——
    /// 每一面只红自己那一枚，其余照旧绿。好的一趟该七枚全绿。缺读数的那几格是「没有问」而不是
    /// 红（#453），这一族里唯一一处天然绿的尺（转 90 度换横竖）压根不设（#433）。
    #[test]
    fn the_seven_transform_rows_answer_apart_for_each_way_the_display_state_can_fail() {
        let legs = |pairs: &[(&str, &str)]| -> Vec<String> {
            pairs.iter().map(|(k, v)| format!("{k}|{v}")).collect()
        };
        let at = |rows: &[(Check, &'static str, String)], i: usize| rows[i].0;
        let flip = |pairs: &[(&'static str, &'static str)],
                    key: &'static str,
                    value: &'static str|
         -> Vec<(&'static str, &'static str)> {
            pairs
                .iter()
                .map(|(k, v)| if *k == key { (*k, value) } else { (*k, *v) })
                .collect()
        };

        // 一趟「产品全对」的四十枚读数：进门 0 度、窗 120x80、纹理号 10，六次按下走到 16，
        // 四枚开关各两下走到 24。这几枚数是本档替量规写的**形状**，不是产品的默认值：进门那格
        // 换成 90 度或换成「已经是灰的」，七枚行照样该全绿（下面 `pre_gray` 那一臂就验这一句）。
        let good: [(&str, &str); 40] = [
            ("xf-rot-start", "0"),
            ("xf-rot-geo-start", "120x80"),
            ("xf-rot-rev-before", "10"),
            ("xf-rot-cw", "90"),
            ("xf-rot-rev-cw", "11"),
            ("xf-rot-ccw", "0"),
            ("xf-rot-rev-ccw", "12"),
            ("xf-rot-half", "180"),
            ("xf-rot-full", "0"),
            ("xf-rot-rev-full", "16"),
            ("xf-rot-geo-end", "120x80"),
            ("xf-fh-start", "false"),
            ("xf-fh-on", "true"),
            ("xf-fh-rev-on", "17"),
            ("xf-fh-off", "false"),
            ("xf-fh-rev-off", "18"),
            ("xf-fv-start", "false"),
            ("xf-fv-on", "true"),
            ("xf-fv-rev-on", "19"),
            ("xf-fv-off", "false"),
            ("xf-fv-rev-off", "20"),
            ("xf-gray-start", "false"),
            ("xf-gray-on", "true"),
            ("xf-gray-rev-on", "21"),
            ("xf-gray-off", "false"),
            ("xf-gray-rev-off", "22"),
            ("xf-inv-start", "false"),
            ("xf-inv-on", "true"),
            ("xf-inv-rev-on", "23"),
            ("xf-inv-off", "false"),
            ("xf-inv-rev-off", "24"),
            ("xf-label-left", "向左旋转"),
            ("xf-label-right", "向右旋转"),
            ("xf-label-h", "水平翻转"),
            ("xf-label-v", "垂直翻转"),
            ("xf-label-gray", "灰度"),
            ("xf-label-inv", "颜色反转"),
            ("xf-key-left", "1|true"),
            ("xf-key-right", "1|true"),
            ("xf-key-gray", "1|true"),
        ];
        let all = xf_rows(&legs(&good));
        let names: Vec<&str> = all.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, XF_ROWS.to_vec(), "{names:?}");
        for row in &all {
            assert_eq!(row.0, Check::Pass, "好的一趟该七枚全绿：{:#?}", row.2);
        }

        // 进门就已经是灰的、已经是翻的：第一下该翻成**不是**灰的，第二下该还回「是灰的」。
        // 这一臂是 #261 的证人——判据若写死「按一次读回 true」，产品在这里就红了而它没错。
        let pre_gray = flip(&good, "xf-gray-start", "true");
        let pre_gray = flip(&pre_gray, "xf-gray-on", "false");
        let pre_gray = flip(&pre_gray, "xf-gray-off", "true");
        let entered = xf_rows(&legs(&pre_gray));
        assert_eq!(
            at(&entered, 3),
            Check::Pass,
            "灰度那一格读的是自己那两枚：{:#?}",
            entered[3].2
        );
        for i in 0..7 {
            assert_eq!(
                at(&entered, i),
                Check::Pass,
                "第 {i} 问不替用户作答：{:#?}",
                entered[i].2
            );
        }

        // 一次迈两档：向右那一下读回 180。角度那一枚红，字面与键照旧绿——它们是三只不同的手。
        let over = flip(&good, "xf-rot-cw", "180");
        let over = xf_rows(&legs(&over));
        assert_eq!(at(&over, 0), Check::Fail, "{:#?}", over[0].2);
        for i in 1..7 {
            assert_eq!(at(&over, i), Check::Pass, "第 {i} 问不读角度");
        }

        // 按了不涨号：转过了却没重编纹理，屏幕上的图还是进门那一张。
        let no_bake = flip(&good, "xf-rot-rev-cw", "10");
        let no_bake = xf_rows(&legs(&no_bake));
        assert_eq!(at(&no_bake, 0), Check::Fail, "{:#?}", no_bake[0].2);
        for i in 1..7 {
            assert_eq!(at(&no_bake, i), Check::Pass, "第 {i} 问不读旋转那一族的号");
        }

        // 满一圈没把窗收回进门的尺寸：后续每一格与 `pin window` 那一行读的就不是同一枚窗了。
        let grown = flip(&good, "xf-rot-geo-end", "80x120");
        let grown = xf_rows(&legs(&grown));
        assert_eq!(at(&grown, 0), Check::Fail, "{:#?}", grown[0].2);
        for i in 1..7 {
            assert_eq!(at(&grown, i), Check::Pass, "第 {i} 问不读几何");
        }

        // 进门角度不是 90 的整数倍：本族没把窗留在这种角度上，出现了就是有人在 core 外面转它。
        let skewed = flip(&good, "xf-rot-start", "45");
        let skewed = flip(&skewed, "xf-rot-cw", "135");
        let skewed = flip(&skewed, "xf-rot-half", "225");
        let skewed = xf_rows(&legs(&skewed));
        assert_eq!(
            at(&skewed, 0),
            Check::Fail,
            "步长那一问读的是整圈：{:#?}",
            skewed[0].2
        );

        // 翻转按下去没动：窗上那一格还是进门那一格。
        let stuck = flip(&good, "xf-fh-on", "false");
        let stuck = xf_rows(&legs(&stuck));
        assert_eq!(at(&stuck, 1), Check::Fail, "{:#?}", stuck[1].2);
        assert_eq!(at(&stuck, 2), Check::Pass, "垂直那一只没被这一枚反证碰到");
        for i in [0, 3, 4, 5, 6] {
            assert_eq!(at(&stuck, i), Check::Pass, "第 {i} 问不读水平那两枚");
        }

        // 第二下没还回去（还翻着）：§5.9.6 步骤 3 与 §5.9.7 步骤 4 那两句「恢复」红了。
        let unturned = flip(&good, "xf-fv-off", "true");
        let unturned = xf_rows(&legs(&unturned));
        assert_eq!(at(&unturned, 2), Check::Fail, "{:#?}", unturned[2].2);
        assert_eq!(
            at(&unturned, 1),
            Check::Pass,
            "水平那一只没被这一枚反证碰到"
        );

        // 两下涨了不止一格：同一张图编了两遍，代价那一枚红而翻与还那两问照旧。
        let double = flip(&good, "xf-inv-rev-off", "26");
        let double = xf_rows(&legs(&double));
        assert_eq!(at(&double, 4), Check::Fail, "{:#?}", double[4].2);
        for i in [0, 1, 2, 3, 5, 6] {
            assert_eq!(at(&double, i), Check::Pass, "第 {i} 问不读反转那一族的号");
        }

        // 菜单词被改写成「反色」：PRD §5.9.8 步骤 2 写的就是「颜色反转」四个字。
        let stale = flip(&good, "xf-label-inv", "反色");
        let stale = xf_rows(&legs(&stale));
        assert_eq!(at(&stale, 5), Check::Fail, "{:#?}", stale[5].2);
        for i in [0, 1, 2, 3, 4, 6] {
            assert_eq!(at(&stale, i), Check::Pass, "第 {i} 问不读字面");
        }

        // 键被关掉（分组名框那处退让在没开过格栅窗的一趟里不该发生），以及同一只手注册了两枚键帽。
        let muted = flip(&good, "xf-key-gray", "1|false");
        let muted = xf_rows(&legs(&muted));
        assert_eq!(at(&muted, 6), Check::Fail, "{:#?}", muted[6].2);
        for i in 0..6 {
            assert_eq!(at(&muted, i), Check::Pass, "第 {i} 问不读键");
        }
        let doubled = flip(&good, "xf-key-left", "2|true");
        let doubled = xf_rows(&legs(&doubled));
        assert_eq!(
            at(&doubled, 6),
            Check::Fail,
            "一枚键上两把键帽：{:#?}",
            doubled[6].2
        );

        // 缺读数：那一格是「没有问」，其余六格照旧绿（红是给产品的话，不是给门的）。
        let mut no_rot: Vec<(&str, &str)> = good.to_vec();
        no_rot.retain(|(k, _)| *k != "xf-rot-rev-full");
        let blind = xf_rows(&legs(&no_rot));
        assert_eq!(at(&blind, 0), Check::Blocked, "{:#?}", blind[0].2);
        for i in 1..7 {
            assert_eq!(at(&blind, i), Check::Pass, "第 {i} 问不读旋转那十二枚");
        }

        let mut no_switch: Vec<(&str, &str)> = good.to_vec();
        no_switch.retain(|(k, _)| *k != "xf-fh-rev-off");
        let blind = xf_rows(&legs(&no_switch));
        assert_eq!(at(&blind, 1), Check::Blocked, "{:#?}", blind[1].2);
        assert_eq!(at(&blind, 2), Check::Pass, "垂直那一格与水平那枚缺读数无关");

        let mut no_label: Vec<(&str, &str)> = good.to_vec();
        no_label.retain(|(k, _)| *k != "xf-label-left");
        let blind = xf_rows(&legs(&no_label));
        assert_eq!(at(&blind, 5), Check::Blocked, "{:#?}", blind[5].2);

        let mut no_key: Vec<(&str, &str)> = good.to_vec();
        no_key.retain(|(k, _)| *k != "xf-key-right");
        let blind = xf_rows(&legs(&no_key));
        assert_eq!(at(&blind, 6), Check::Blocked, "{:#?}", blind[6].2);

        // 一句也不说：七枚全是 Blocked，一枚也不红。
        let silent = xf_rows(&[]);
        assert_eq!(silent.len(), 7, "{silent:?}");
        for row in &silent {
            assert_eq!(row.0, Check::Blocked, "{:#?}", row.2);
        }
    }

    /// 那三枚数字键与那六行菜单共指同一批落点，逐串现读 `PinWindow.qml`（引语要回盘上档案对
    /// #271/#341）。四件事各判各的：① 每枚 Action 的 `id` 与落点各一处（多一处就是第二条路绕过
    /// 了菜单那一行）；② 每行菜单只写 `action:`，字面不重述；③ 三枚 `Shortcut` 块内**同一块里**
    /// 键帽、上下文、退让、`trigger()` 四行齐——分开数枚数就证不出「这一枚键叫的是这一只手」；
    /// ④ 四十枚 `gaugeSay("xf-` 与 `PIN_SCHEDULE` 里那四十枚同名键一枚不多一枚不少。
    #[test]
    fn the_transform_keys_share_the_menu_route() {
        let src = include_str!("../qml/PinWindow.qml");

        // ① 六枚落点：`id` 一次、handler 一次。handler 住的是 core 那六只手的名字。
        let hands = [
            ("rotateLeftAction", "win.view.rotateCcw()"),
            ("rotateRightAction", "win.view.rotateCw()"),
            ("flipHAction", "win.view.flipH()"),
            ("flipVAction", "win.view.flipV()"),
            ("grayscaleAction", "win.view.toggleGrayscale()"),
            ("invertAction", "win.view.toggleInvert()"),
        ];
        for (id, landing) in hands {
            let decl = src.matches(&format!("id: {id}")).count();
            let handler = src.matches(&format!("onTriggered: {landing}")).count();
            let menu = src.matches(&format!("MenuItem {{ action: {id} }}")).count();
            assert_eq!(decl, 1, "`{id}` 的声明该只有一处，读到 {decl}");
            assert_eq!(
                handler, 1,
                "`{id}` 的落点该只有一处（{landing}），读到 {handler}——两处就是两条路"
            );
            assert_eq!(menu, 1, "菜单里 `{id}` 那一行该只挂一处，读到 {menu}");
        }

        // ② 三枚键：块内四行齐，且 `trigger()` 叫的正是上面那枚 Action。
        let keys = [
            ("rotateLeftKey", "1", "rotateLeftAction"),
            ("rotateRightKey", "2", "rotateRightAction"),
            ("grayscaleKey", "3", "grayscaleAction"),
        ];
        let block = |from: &str| -> String {
            let start = src.find(from).unwrap_or_else(|| panic!("找不到 {from}"));
            let rest = &src[start..];
            let end = rest.find("\n    }").unwrap_or(rest.len());
            rest[..end].to_string()
        };
        for (id, cap, action) in keys {
            let text = block(&format!("id: {id}"));
            assert!(
                text.contains(&format!("sequences: [\"{cap}\"]")),
                "`{id}` 这一块里没读到键帽 {cap}：{text}"
            );
            assert!(
                text.contains("context: Qt.WindowShortcut"),
                "`{id}` 不许是 ApplicationShortcut：那会把 {cap} 从别的窗带走：{text}"
            );
            assert!(
                text.contains("enabled: !groupField.activeFocus"),
                "`{id}` 少了格栅窗那处退让：用户打「{cap}」就成了一次手势：{text}"
            );
            assert!(
                text.contains(&format!("onActivated: {action}.trigger()")),
                "`{id}` 叫的不是 `{action}`：门与键就不是同一只手了：{text}"
            );
        }
        // 退让那一枚数数的是**代码行**而不是裸词：上面那封信里就引用了同一串字面（带反引号），
        // 拿裸词数会数出四枚而产品只有三枚——这一族在 `translate_selection` 那一枚反证里记过同一课。
        assert_eq!(
            src.matches("\n        enabled: !groupField.activeFocus")
                .count(),
            3,
            "这处退让只该长在那三枚键上，读到不同"
        );

        // ③ 反证两面：PRD 没写过的词不许进窗；PRD 没给键的那三只手不许自己长出一枚键来
        // （§5.9.6 只说「选择水平翻转或垂直翻转」、§5.9.8 只说「选择"颜色反转"」，两句都没有键）。
        for banned in ["text: \"顺时针\"", "text: \"逆时针\"", "text: \"反色\""] {
            assert_eq!(
                src.matches(banned).count(),
                0,
                "菜单里不许出现 PRD 之外的说法：{banned}"
            );
        }
        for orphan in ["flipHAction", "flipVAction", "invertAction"] {
            assert_eq!(
                src.matches(&format!("onActivated: {orphan}.trigger()"))
                    .count(),
                0,
                "`{orphan}` 不该被一枚 `Shortcut` 叫：PRD 那两节没给键，本档也不替它补一枚"
            );
        }

        // ④ 四十枚腿：量规说的枚数与 `PIN_SCHEDULE` 里 `xf-` 那一族的枚数同一枚数。
        let legs = src.matches("gaugeSay(\"xf-").count();
        let scheduled = PIN_SCHEDULE.iter().filter(|k| k.starts_with("xf-")).count();
        assert_eq!(legs, 40, "`runGauge` 里 `xf-` 那一族说了 {legs} 枚");
        assert_eq!(scheduled, 40, "排程里 `xf-` 那一族收了 {scheduled} 枚");
        assert_eq!(legs, scheduled, "说的与排程的必须同一批名字（两侧都数）");
    }

    /// 六个词在本档只写一次：`XF_LABELS` 是 PRD 那四句的**唯一**一处誊写处（#281），窗上每枚
    /// Action 的 `text:` 各一处，菜单那一行没有第二处。四句原文抄在下面，改动任何一枚都得
    /// 对着这四句改——这一枚单测就是那四句的第二枚读者。
    #[test]
    fn the_six_transform_labels_are_written_exactly_once_each() {
        // PRD §5.9.5 步骤 2「使用数字键或菜单执行向左、向右旋转」
        // PRD §5.9.6 步骤 2「选择水平翻转或垂直翻转」
        // PRD §5.9.7 步骤 2「使用对应快捷键或菜单选择“灰度”」
        // PRD §5.9.8 步骤 2「选择“颜色反转”」
        assert_eq!(
            XF_LABELS,
            [
                "向左旋转",
                "向右旋转",
                "水平翻转",
                "垂直翻转",
                "灰度",
                "颜色反转",
            ],
            "六枚词是上面那四句的誊写处，改动要对那四句改"
        );
        let src = include_str!("../qml/PinWindow.qml");
        for word in XF_LABELS {
            let on_action = src.matches(&format!("text: \"{word}\"")).count();
            let on_menu = src
                .matches(&format!("MenuItem {{ text: \"{word}\""))
                .count();
            assert_eq!(on_action, 1, "窗上「{word}」该只写一次，读到 {on_action}");
            assert_eq!(
                on_menu, 0,
                "菜单不许重述「{word}」：字面只有 Action 那一枚读者"
            );
        }
        assert_eq!(XF_LABELS.len(), 6, "六枚词与六枚 Action 同一枚数");
        assert_eq!(
            XF_KEYS.len(),
            3,
            "三枚键帽：§5.9.5 两枚、§5.9.7 一枚，§5.9.6 与 §5.9.8 原文没给键"
        );
    }
}
