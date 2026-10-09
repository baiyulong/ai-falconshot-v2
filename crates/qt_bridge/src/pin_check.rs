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
//! lines [`report`] receives. Nothing here pretends to click the mouse: the gauge
//! calls the same two statements `PinWindow.qml`'s own handlers call, which is what
//! makes the sentences the user would see rather than a paraphrase of them.
//!
//! ## What it does not claim
//!
//! `saveAs.accept()` is asked whether a dialog that was never `open()`ed can still
//! fire its own `accepted` signal. That is a question about Qt, not a promise the
//! product makes to a user, so its row answers `SKIP` when the dialog stays quiet -
//! the two file writes below then carry the measurement, and the reading of
//! `currentFile` sits in the log either way. A witness that needs a human is not a
//! defect, and P36's whole point was to stop grading the difference as one.

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

/// The shortest `--pin` run that can have measured anything: the leg's delay, then
/// the same again for the window to be exposed, two files written and the lines to
/// arrive. A run the user asked for shorter than this is raised rather than refused -
/// it asked for the mode, and the mode simply needs that much time.
pub const MIN_MS: i32 = 2 * LEG_DELAY_MS;

/// The five names this run owns inside [`DIR`], in the order the plan keeps them.
/// `never.png` is written by the leg that must *not* write it.
const NAMES: [&str; 5] = ["qml.png", "qml.jpg", "rust.png", "rust.jpg", "never.png"];

/// The formats §5.9.17's dialog offers, in the order `PinWindow.qml` lists them.
/// The extension is what picks the encoder, so this list is also the list of things
/// the door can promise to write.
const FILTERS: [&str; 3] = ["*.png", "*.jpg", "*.bmp"];

/// How far a window's device rect may sit from the pin's own rect and still be it.
/// The shim reads the frame Win32 has; the pin's rect is what the state machine
/// asked Qt for, in device pixels. Rounding on the logical-pixel way back
/// (`width: view.size_w / dpr`) is what this slack is for - the same 2 px the mask's
/// rect row allows.
const RECT_SLACK: i32 = 2;

/// The plan, the paths, and whatever QML has reported into it so far.
struct Plan {
    dir: PathBuf,
    qml_png: PathBuf,
    qml_jpg: PathBuf,
    rust_png: PathBuf,
    rust_jpg: PathBuf,
    never: PathBuf,
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
/// here: `目录 | qml.png | qml.jpg | never.png | 贴图 id | 不存在的 id | 延迟 ms`. The
/// id that has no pin travels with the plan rather than being recomputed in QML,
/// because `NEVER_OFFSET` is this file's business and a second copy of a number in
/// the other language is a pair that drifts without either side looking wrong.
pub fn plan_line() -> String {
    with(|p| {
        if p.dir.as_os_str().is_empty() {
            return String::new();
        }
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            p.dir.display(),
            p.qml_png.display(),
            p.qml_jpg.display(),
            p.never.display(),
            p.id,
            p.bad,
            LEG_DELAY_MS
        )
    })
}

/// One line QML read off the door. `key|value`, graded by [`measure`].
pub fn report(line: &str) {
    with(|p| p.lines.push(line.to_string()));
}

/// Plant the pin and the plan. Must run before the QML engine: the window's
/// `Component.onCompleted` reads the plan, and a delegate with no plan has nothing
/// to measure.
pub fn plant() -> Result<String, String> {
    let mut dir = std::env::temp_dir();
    dir.push(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let [qml_png, qml_jpg, rust_png, rust_jpg, never] = NAMES.map(|n| dir.join(n));
    for p in [&qml_png, &qml_jpg, &rust_png, &rust_jpg, &never] {
        let _ = std::fs::remove_file(p);
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
        p.id = id;
        p.bad = bad;
        p.rect = Some(rect);
        p.lines.clear();
    });
    Ok(format!(
        "贴图 {id} 的窗在 {rect}，写进 {}，另有一个不存在的 id {bad}",
        dir.display()
    ))
}

/// The value QML reported under one key, or `None` when that leg never spoke.
///
/// The key is matched as a whole field rather than as a prefix, so a future `png2`
/// cannot be read as `png`.
fn said<'a>(lines: &'a [String], key: &str) -> Option<&'a str> {
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
    let rust_said = state::with(|s| s.save_image(id, &rust_path.to_string_lossy()));
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
    let mut rep = Report::new();
    let (id, bad, qml_png, qml_jpg, rust_png, rust_jpg, never, rect, lines) = with(|p| {
        (
            p.id,
            p.bad,
            p.qml_png.clone(),
            p.qml_jpg.clone(),
            p.rust_png.clone(),
            p.rust_jpg.clone(),
            p.never.clone(),
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
    rep.row(
        "pin leg ran",
        Check::from(keys.contains(&"done")),
        format!("{} 行：{}", lines.len(), keys.join(" ")),
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

    let (was, now) = (
        said(&lines, "accept-was").unwrap_or("<没有读数>"),
        said(&lines, "accept").unwrap_or("<没有读数>"),
    );
    let fired = now != was && (now.starts_with("已保存到") || now.starts_with("保存失败"));
    rep.row(
        "pin dialog accept",
        if fired { Check::Pass } else { Check::Blocked },
        format!(
            "未 open() 的窗 accept() 后 status {was:?} -> {now:?}（{}）",
            if fired {
                "onAccepted 跑了"
            } else {
                "onAccepted 没跑，下面两格改走同一句直接调用"
            }
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
    use super::said;

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
}
