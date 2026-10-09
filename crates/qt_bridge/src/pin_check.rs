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

use falcon_core::geometry::PhysRect;
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
/// the tail that reads what `onAccepted` left behind, and one more after the leg says it
/// is finished, which is the grace [`report`] arms so the run ends when the measurement
/// does rather than when a guessed budget runs out. They are phases rather than one
/// straight run because a door that is opened by a real click cannot be driven inside a
/// single event-loop turn: the click has to land while the dialog's own loop is running.
pub const LEG_WAITS: i32 = PUSH_TRIES_MAX + 3;

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

/// The six names this run owns inside [`DIR`], in the order the plan keeps them.
/// `never.png` is written by the leg that must *not* write it; `dialog.png` is written
/// by the door a real click closed, and is the only file here whose name the gauge did
/// not choose - the dialog picked the extension off its own filter list.
const NAMES: [&str; 6] = [
    "qml.png",
    "qml.jpg",
    "rust.png",
    "rust.jpg",
    "never.png",
    "dialog.png",
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
pub(crate) const PIN_SCHEDULE: &[&str] = &[
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
    "pre-save",
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
    "dlg-dismiss",
    "dlg-rejected",
    "f-jpg",
    "done",
];

/// What the observed key order says about its schedule: `None` when every leg that ran is
/// in the schedule and ran in its relative order, otherwise the problems, joined.
///
/// The comparison is *relative*, the way the finding is: two legs changing position inside
/// one phase is the same defect as two phases swapping, and a fixed index per line would
/// call a run that skipped its optional legs out of order. Equal indices are allowed,
/// because a key a door writes twice is the same leg answering twice, not a schedule change.
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
    id: PinId,
    bad: PinId,
    /// What the pin asked Qt to show, in device pixels, as the state machine saw it.
    rect: Option<PhysRect>,
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
            id: 0,
            bad: 0,
            rect: None,
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
/// 对话框默认名 | 推开尝试上限`. The id that has no pin travels with the plan rather than
/// being recomputed in QML, because `NEVER_OFFSET` is this file's business and a second
/// copy of a number in the other language is a pair that drifts without either side
/// looking wrong. The last two fields are the same argument for a string and for a
/// schedule: QML types the name into the dialog and this file reads the file it produces,
/// and QML counts the waits while this file budgets them - all three only agree if the
/// values were handed over rather than written twice.
pub fn plan_line() -> String {
    with(|p| {
        if p.dir.as_os_str().is_empty() {
            return String::new();
        }
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            p.dir.display(),
            p.qml_png.display(),
            p.qml_jpg.display(),
            p.never.display(),
            p.id,
            p.bad,
            LEG_DELAY_MS,
            DIALOG_STEM,
            PUSH_TRIES_MAX
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
pub fn report(line: &str) {
    let done = matches!(line.split_once('|'), Some((key, _)) if key == "done");
    with(|p| {
        if p.lines.is_empty() {
            state::stamp("pin leg");
        }
        p.lines.push(line.to_string());
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
    let [qml_png, qml_jpg, rust_png, rust_jpg, never, dialog] = NAMES.map(|n| dir.join(n));
    for p in [&qml_png, &qml_jpg, &rust_png, &rust_jpg, &never, &dialog] {
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
    let bad = id + NEVER_OFFSET;
    with(|p| {
        p.dir = dir.clone();
        p.qml_png = qml_png;
        p.qml_jpg = qml_jpg;
        p.rust_png = rust_png;
        p.rust_jpg = rust_jpg;
        p.never = never;
        p.dialog = dialog;
        p.id = id;
        p.bad = bad;
        p.rect = Some(rect);
        p.lines.clear();
    });
    Ok(format!(
        "贴图 {id} 的窗在 {rect}，写进 {}，另有一个不存在的 id {bad}，真点击那一格交给 {} 的对话框自己取名",
        dir.display(),
        DIALOG_STEM
    ))
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
    let (id, bad, qml_png, qml_jpg, rust_png, rust_jpg, never, dialog, dir, rect, lines) =
        with(|p| {
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
            )
        });

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
            let close = (t.phys.x - want.x).abs() <= RECT_SLACK
                && (t.phys.y - want.y).abs() <= RECT_SLACK
                && (t.phys.w as i32 - want.w as i32).abs() <= RECT_SLACK
                && (t.phys.h as i32 - want.h as i32).abs() <= RECT_SLACK;
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
    rep.row(
        "pin dialog pushed",
        Check::from(real_click(push)),
        format!("{shown}（{tries} 次尝试；要求 pushed=yes、class=[#32770]、bid=1）"),
    );
    rep.note(
        "push 每次尝试",
        format!("原始 {push:?}；每次 {:?}", said(&lines, "dlg-push-log")),
    );

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
        } else if real_click(push) {
            // The click happened and the sentence did not change: that is the door
            // failing to open, not a question this run is unable to ask.
            Check::Fail
        } else {
            Check::Blocked
        },
        format!(
            "真点击前后 status {shown:?} -> {now:?}；前态那句读自被拒的那次 savePin 之后的 {:?}（{}）",
            pre.unwrap_or("<没有读数>"),
            if fired {
                "onAccepted 是那一下点击跑起来的"
            } else if real_click(push) {
                "点了，但 onAccepted 没跑或前态不是产品那句"
            } else {
                "没点成，这一格问不了"
            }
        ),
    );
    // The other half of §10 第 39 项 ①'s asymmetry: the mask door has had this grade since
    // P38, while on this one the reading (`dlg-after`) travelled in the transcript as a note
    // only - so "the click closed the door" and "the leg closed it for her" were both
    // recorded and neither was judged. `dlg-dismiss` is the leg admitting it did the job the
    // click was supposed to have done.
    rep.row(
        "pin dialog closed",
        Check::from(said(&lines, "dlg-after") == Some("false") && !keys.contains(&"dlg-dismiss")),
        format!(
            "点完之后 visible {:?}，量具代关={}",
            said(&lines, "dlg-after"),
            keys.contains(&"dlg-dismiss")
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
        format!("{text}\npin plan: id={id} bad={bad} rect={want}"),
    )
}

#[cfg(test)]
mod tests {
    use super::{leg_order, qml_leg_keys, real_click, said, same_file, PIN_SCHEDULE};
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
}
