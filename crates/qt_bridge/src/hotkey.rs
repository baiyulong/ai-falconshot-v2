//! §5.2's front door: the global hot key and the tray icon, driven into the product.
//!
//! The Win32 half ([`platform_windows::hotkey`]) knows about `RegisterHotKey`, a
//! message-only window and a notification-area icon. It deliberately knows nothing
//! about a screenshot: it answers with "id 3 arrived". This module is where that
//! number becomes the mask, and the reason it cannot happen on the pump thread is
//! that [`mask::open_from_freeze`] freezes the desktop, publishes textures and shows
//! `Window`s - Qt work, which belongs to the thread that owns the `QGuiApplication`.
//!
//! So the chain is three threads long, and each link is a different library's
//! primitive:
//!
//! ```text
//! shell ─WM_HOTKEY─▶ pump thread ─mpsc::Receiver─▶ listener ─CxxQtThread::queue─▶ GUI thread
//! ```
//!
//! The pump hands over rather than acting (spike P3, `spike/hello-cxxqt/src/pump.rs:178`),
//! the listener decides *whether* anything should happen at all - §5.16.2's
//! per-application exclusions, plus the one rule below that a second freeze cannot
//! photograph a mask that already covers the screen - and the GUI thread does the
//! work inside a closure that carries nothing from the other two.
//!
//! ## What gets registered
//!
//! Only an action this build can actually perform. §5.16.1's table has nine rows and
//! this round wires four of them, so the other five are *not* registered: a
//! combination the shell holds for us while nothing answers it does not merely sit
//! idle, it takes the key away from whatever the user expected it to do.
//! `capture_custom` is the clearest case - the coordinates dialog it names (§5.3.5)
//! does not exist yet, so registering `Ctrl+Shift+F` would leave that chord with no
//! answer at all. Every unregistered row still gets its own sentence in the report,
//! so the census is the nine the plan counts rather than the four that happen to work.

use std::pin::Pin;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use cxx_qt::CxxQtThread;
use cxx_qt_lib::QString;
use falcon_core::frame::Frame;
use falcon_core::hotkeys;
use platform_windows::hotkey::{bind, Event, Pump, Request, Started, TraySpec};

use crate::capture;
use crate::delay;
use crate::mask;
use crate::r13::Report;
use crate::session;
use crate::state;

/// The shortest `--hotkeys` run that can answer: the press is posted before the
/// event loop turns, and the leg it triggers is a freeze plus N window presentations.
/// A run shorter than this is raised rather than refused, and prints both numbers -
/// the rule §10 第 38 项 ② wrote for `--pin` and `--ink`.
pub const MIN_MS: i32 = 4_000;

/// The product's mask path. `--mask-soft` is a gauge's alternate drawing route,
/// chosen by a flag no user presses; the hot key takes the shader, which is what
/// ships.
const SHADER: bool = true;

/// The Qt thread, as the type another thread is allowed to hold on to. `Session`
/// opted into `Threading` (`session.rs`), which is what makes this name writable
/// from outside the bridge module.
type Gui = CxxQtThread<session::qobject::Session>;

/// A hot key or menu command, and the work it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// §5.2.1: freeze and cover the screens.
    Capture,
    /// §5.2.6: the window the shell is giving keystrokes to, as its own picture.
    ActiveWindow,
    /// §5.2.5: the region the last capture used, on a frame taken now.
    Repeat,
    /// §5.8.2: turn the clipboard into a pin.
    PastePin,
    /// §5.1.1's tray menu entry. Tray-only: no key binding means "quit", because a
    /// binding that could end the resident process from the keyboard is not
    /// something to hand out before a settings page has asked about it.
    Quit,
}

impl Work {
    /// The config action this work answers, or `None` for the tray-only one. This
    /// match arm is the whole wiring table: an action with no arm here is reported
    /// as 未接线 and its key is left to the system.
    fn action(self) -> Option<&'static str> {
        match self {
            Work::Capture => Some("capture"),
            Work::ActiveWindow => Some("capture_active_window"),
            Work::Repeat => Some("capture_repeat"),
            Work::PastePin => Some("paste_pin"),
            Work::Quit => None,
        }
    }

    /// What the tray menu calls it, and what the history calls the work.
    fn label(self) -> &'static str {
        match self {
            Work::Capture => "截图",
            Work::ActiveWindow => "活动窗口截图",
            Work::Repeat => "重复上次截图",
            Work::PastePin => "贴出剪贴板",
            Work::Quit => "退出",
        }
    }

    /// The work an action id names, if this build performs it.
    fn of(action: &str) -> Option<Work> {
        [
            Work::Capture,
            Work::ActiveWindow,
            Work::Repeat,
            Work::PastePin,
            Work::Quit,
        ]
        .into_iter()
        .find(|work| work.action() == Some(action))
    }

    /// How one line of the history names this work: the config action when there is
    /// one, since the door reads back by action.
    fn tag(self) -> String {
        self.action().unwrap_or(self.label()).to_string()
    }
}

/// One id the pump may deliver, and where it goes. Menu-only ids join the hot key
/// ids in this one table, so a tray click and a keystroke are routed by the same
/// lookup rather than by two copies of the same decision.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Route {
    id: u32,
    action: String,
    work: Work,
    /// The key's own label, kept so the history can name the binding that fired.
    display: String,
}

/// What phase 1 decided, before the shell was asked anything. Separate from
/// [`install`] because a sentence about a config file is a pure function of that
/// config file, and only a pure function can be tested without a desktop.
struct Plan {
    /// One row per configured action, in the config's own order.
    rows: Vec<String>,
    requests: Vec<Request>,
    routes: Vec<Route>,
    /// The next free id, so a tray entry that has no key still gets one.
    next: u32,
}

/// What one run installed. The report reads this alongside the history: a count on
/// one side of the chain is not evidence until the other side says the same number.
pub struct Launch {
    pub started: Started,
    pub rows: Vec<String>,
    /// Actions the config names, wired or not - the denominator every count is read
    /// against.
    pub actions: usize,
    /// Registrations this build asked the shell for.
    pub attempts: usize,
    pub menu: Vec<(u32, String)>,
    pub tip: String,
    /// Whether the tooltip rewrite reached the pump thread.
    pub tip_set: bool,
}

static GUI: Mutex<Option<Gui>> = Mutex::new(None);
static PUMP: Mutex<Option<Pump>> = Mutex::new(None);
static ROUTES: Mutex<Vec<Route>> = Mutex::new(Vec::new());
static LISTENER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
/// Every decision the listener and the GUI closure made, in order. The door reads
/// this back because "the post succeeded" and "the product moved" are two facts.
static HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn held<R>(cell: &Mutex<R>) -> MutexGuard<'_, R> {
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One line of the chain. Kept short: it is read as evidence, and the sentence is
/// what says which link of the chain it came from.
fn note(line: String) {
    held(&HISTORY).push(line);
}

pub fn history() -> Vec<String> {
    held(&HISTORY).clone()
}

/// The handle QML hands over in `Component.onCompleted`. Everything else this module
/// does needs a thread it cannot reach on its own, because the `Session` object
/// belongs to the QML document rather than to Rust.
pub fn hand(thread: Gui) {
    let mut slot = held(&GUI);
    if slot.is_some() {
        note("GUI 线程句柄：第二次递过来，留着第一个".to_string());
        return;
    }
    *slot = Some(thread);
    note("GUI 线程句柄已到手".to_string());
}

fn gui() -> Option<Gui> {
    held(&GUI).clone()
}

/// Sort the config into "this build will ask the shell for it" and every other
/// answer, and give each action exactly one row. The branches are ordered so that
/// the reason a key is *not* taken is the strongest one that applies: a reserved
/// combination is reported as reserved even when its action is unwired, because the
/// user reading "未接线" would conclude the key was still theirs.
fn plan(actions: &std::collections::BTreeMap<String, String>) -> Plan {
    let mut out = Plan {
        rows: Vec::new(),
        requests: Vec::new(),
        routes: Vec::new(),
        next: 1,
    };
    for (action, spec) in actions {
        let text = spec.trim();
        if text.is_empty() {
            out.rows
                .push(format!("{action} 未绑定（配置里这格是空的）"));
            continue;
        }
        let parsed = match hotkeys::parse_bindable(text) {
            Ok(spec) => spec,
            Err(e) => {
                out.rows.push(format!("{action} {text:?} 注册不了：{e}"));
                continue;
            }
        };
        let canonical = parsed.canonical();
        let display = hotkeys::display(parsed);
        if let Some(why) = parsed.reserved_reason() {
            out.rows.push(format!(
                "{action} {display} 是系统保留组合：{why}（不注册）"
            ));
            continue;
        }
        let Some(work) = Work::of(action) else {
            out.rows
                .push(format!("{action} {display} 未接线（不注册，键留给系统）"));
            continue;
        };
        match bind(parsed, &canonical) {
            Ok(binding) => {
                out.routes.push(Route {
                    id: out.next,
                    action: action.clone(),
                    work,
                    display: display.clone(),
                });
                out.requests.push(Request {
                    id: out.next,
                    action: action.clone(),
                    canonical,
                    binding,
                });
                out.next += 1;
            }
            Err(e) => out.rows.push(format!("{action} {display} 递不出注册：{e}")),
        }
    }
    out
}

/// The tray menu: the two works a key can start, then 退出. An entry whose key
/// never got an id (unbound, reserved, unwired) is given a menu-only id, because the
/// icon is the other way to reach the same feature - and the only way to reach 退出.
fn tray_menu(plan: &mut Plan) -> Vec<(u32, String)> {
    let mut menu = Vec::new();
    for work in [Work::Capture, Work::PastePin] {
        let id = match plan.routes.iter().find(|route| route.work == work) {
            Some(route) => route.id,
            None => {
                let id = plan.next;
                plan.next += 1;
                plan.routes.push(Route {
                    id,
                    action: work.action().unwrap_or_default().to_string(),
                    work,
                    display: String::new(),
                });
                id
            }
        };
        let display = plan
            .routes
            .iter()
            .find(|route| route.id == id)
            .map(|route| route.display.clone())
            .unwrap_or_default();
        menu.push((
            id,
            if display.is_empty() {
                work.label().to_string()
            } else {
                format!("{}（{display}）", work.label())
            },
        ));
    }
    let id = plan.next;
    plan.next += 1;
    plan.routes.push(Route {
        id,
        action: String::new(),
        work: Work::Quit,
        display: String::new(),
    });
    menu.push((id, Work::Quit.label().to_string()));
    menu
}

/// §5.1.1 那一格 `show_tray` 的分岔本体：把 `[general] show_tray` 与托盘的两样材料
/// (tip 文案 + menu 项) 合成 `Pump::start` 的第二参数 —— 真则 `Some(TraySpec)`、
/// 假则 `None`。抽出来是因为 `install()` 需要真 Win32 消息泵、单测跑不动 (§9.1 第十条
/// 的 headless 半边)，而这一支的分支语义本身不需要消息泵就能证：`cargo test -p
/// qt_bridge` 直接调这个纯函数、看两边各自的 `Option` 形状对不对，就替 P66 那句
/// "分岔代码自己作证"补上一枚 §9.1 单测证人。
fn tray_from(show_tray: bool, tip: String, items: Vec<(u32, String)>) -> Option<TraySpec> {
    if show_tray {
        Some(TraySpec { tip, items })
    } else {
        None
    }
}

/// Register what this build can do, install the icon, and start the listener.
///
/// Returns the census rather than printing it, so the caller decides what a census
/// is for: a door grades the rows, the app just tells the user what it took over. An
/// `Err` is a run that installed nothing.
pub fn install() -> Result<Launch, String> {
    if held(&PUMP).is_some() {
        return Err("这个进程已经有一个热键泵了：一条线程一个窗口，不注册两次".to_string());
    }
    let Some(handle) = gui() else {
        return Err("QML 还没交出它的 GUI 线程（handThread 没跑），热键一个也不注册".to_string());
    };
    let actions = crate::settings::with(|s| s.config.hotkey.actions.clone());

    let mut planned = plan(&actions);
    let menu = tray_menu(&mut planned);
    let routes = std::mem::take(&mut planned.routes);

    // Two bindings that mean one thing is a fact about the config, reported whether
    // or not the shell then accepted both. Core owns that rule, so this is the same
    // sentence the settings page will show (§5.16.1).
    let mut rows = planned.rows;
    for clash in hotkeys::conflicts(&actions) {
        if clash.actions.len() > 1 {
            rows.push(format!(
                "冲突 {}：{} 都在用这一串键",
                clash.display,
                clash.actions.join(" + ")
            ));
        }
    }

    let attempts = planned.requests.len();
    let initial = "Falconshot 正在运行".to_string();
    // §5.1.1 / P66：托盘的"上或不上"由 `[general] show_tray` 决定，而不是由 P50 那一句
    // `Some(TraySpec { ... })` 硬编码。整段 `[general]` 由 `crate::settings::general_cfg()`
    // 一次 clone（与 P64 `pin_cfg()` / P65 `capture_cfg()` 同形），`show_tray = false` 时
    // `Pump::start` 的第二个参数换 `None` —— Win32 `Shell_NotifyIcon` 那一头不会挂上，
    // 托盘左键 (`Event::TrayLeft`) 也永远不来。热键那一支不受影响：`planned.requests`
    // 与 menu 已经建了、`Pump::start` 里的热键注册照旧跑，只是 tray 那一格空着。
    // P67：分岔本身由 `tray_from()` 承担，那是一枚纯函数、`cargo test -p qt_bridge`
    // 能直接跑，本轮的证人不只是"分岔代码自己"（P66 的形状），而是 `install` 之外
    // 那一支 `tray_honours_the_general_switch` 单测（§9.1 第十条的 headless 半边）。
    let general = crate::settings::general_cfg();
    let tray = tray_from(general.show_tray, initial, menu.clone());
    let (mut pump, started) = match Pump::start(planned.requests, tray) {
        Ok(pair) => pair,
        Err(e) => return Err(e.to_string()),
    };

    // The shell's own answer, one row per registration. Reading `started` instead of
    // assuming the call worked is what turns a taken key into a sentence with a
    // Win32 number in it rather than a half-working window.
    for refusal in &started.refused {
        let display = routes
            .iter()
            .find(|route| route.id == refusal.id)
            .map(|route| route.display.clone())
            .unwrap_or_else(|| refusal.canonical.clone());
        rows.push(format!(
            "{} {display} 注册失败：{}（Win32 {}）",
            refusal.action, refusal.why, refusal.win32_error
        ));
    }
    for (id, action) in &started.registered {
        let display = routes
            .iter()
            .find(|route| route.id == *id)
            .map(|route| route.display.clone())
            .unwrap_or_default();
        rows.push(format!("{action} {display} 已注册"));
    }

    // §5.1.1's "正确反映应用运行状态", as far as a tooltip can: the count the shell
    // actually took, which is not the count this process asked for until `started`
    // says so.
    let tip = format!(
        "Falconshot 正在运行（{} 个全局热键待命，菜单里有截图）",
        started.registered.len()
    );
    let tip_set = pump.set_tip(&tip);
    let Some(events) = pump.take_events() else {
        pump.stop();
        return Err("事件流被取走了两次，这一趟没人读它".to_string());
    };
    let listener = std::thread::Builder::new()
        .name("falconshot-hotkey-listen".into())
        .spawn(move || listen(events, handle));
    let listener = match listener {
        Ok(handle) => handle,
        Err(err) => {
            // The keys are the user's already, so a listener that cannot start has to
            // give them back before the error is returned.
            pump.stop();
            return Err(format!("热键监听线程起不来：{err}"));
        }
    };

    *held(&ROUTES) = routes;
    *held(&PUMP) = Some(pump);
    *held(&LISTENER) = Some(listener);
    note(format!(
        "注册 {} 个、菜单 {} 项，hwnd={}",
        started.registered.len(),
        menu.len(),
        started.hwnd
    ));

    Ok(Launch {
        started,
        rows,
        actions: actions.len(),
        attempts,
        menu,
        tip,
        tip_set,
    })
}

/// The listener: pump messages in, GUI-thread work out. It never blocks on the
/// queue, so a busy event loop delays a screenshot rather than stalling the pump -
/// and a stalled pump would stop the tray icon answering its own clicks.
fn listen(events: Receiver<Event>, handle: Gui) {
    while let Ok(event) = events.recv() {
        let through = matches!(event, Event::Menu(_));
        let (what, work) = match &event {
            Event::TrayLeft => ("托盘左键".to_string(), Some(Work::Capture)),
            Event::Hotkey(id) | Event::Menu(id) => {
                let route = held(&ROUTES).iter().find(|route| route.id == *id).cloned();
                let name = format!(
                    "{} id={} {}",
                    if through { "菜单" } else { "热键" },
                    id,
                    route.as_ref().map_or(String::new(), |r| r.action.clone())
                );
                (name, route.map(|route| route.work))
            }
            // §5.19.1 第 3 步：机器自己播了一次设置变更。这一枚不进下面那张 `Work` 表，因为它
            // 没有「哪个动作」的归宿，也不该被遮罩挡住（遮罩开着也不会多冻一次画面）—— 它唯一的
            // 活是让界面那一枚词重新问一次机器。名字原样印出来当证人，本层不拿它当判据。
            Event::SettingChange(name) => {
                let shown = if name.is_empty() {
                    "(无名)".to_string()
                } else {
                    name.clone()
                };
                note(format!("设置变更 {shown}：排队到 GUI 线程重问主题"));
                // 名字要跟着闭包走，所以给 GUI 线程一份自己的副本；`shown` 留在这一条线程上，
                // 为的是「GUI 线程没接住」那一格也要能把名字说出来。
                let tagged = shown.clone();
                if let Err(e) = handle.queue(move |session| theme_refresh(&tagged, session)) {
                    note(format!("设置变更 {shown}：GUI 线程没接住（{e}）"));
                }
                continue;
            }
        };
        let Some(work) = work else {
            note(format!("{what}：没有归宿，不动手"));
            continue;
        };
        if let Some(why) = silenced(work) {
            note(format!("{what} {}：被静音（{why}）", work.tag()));
            continue;
        }
        if work != Work::Quit && mask::with(|m| m.shown) {
            // A second freeze would photograph the mask it is about to cover, which
            // is the one thing §5.3's whole design avoids. Escape still closes it.
            note(format!("{what} {}：遮罩已经开着，这一次不动手", work.tag()));
            continue;
        }
        note(format!(
            "{what} {}：排队到 GUI 线程（{}）",
            work.tag(),
            work.label()
        ));
        // `queue` neither blocks this thread nor panics on a destroyed object when
        // its result is read rather than unwrapped - cxx-qt's own guidance.
        if let Err(e) = handle.queue(move |session| run(work, session)) {
            note(format!("{what} {}：GUI 线程没接住（{e}）", work.tag()));
        }
    }
    note("事件流结束：泵停了，监听线程收工".to_string());
}

/// 让界面那一枚词重新问一次机器，并把前后两枚词打出来。
///
/// 四步是同一条腿、同一枚闭包、同一个 GUI 线程，为的是 #281 那一问在这里也有答案：
/// `before` 与 `after` 读的都是 [`crate::settings::startup_theme_face`]，也就是
/// [`session::SessionRust::refresh_theme`] 往 qproperty 里写的那**同一枚来源** —— 显示的那份与
/// 判的那份从来不是两次读数，所以 `moved=` 说的就是屏幕上那一格动了没有。中间只有两件事：先让
/// 缓存失效（那是它唯一的失效点），再重新合成。
///
/// 机器的答案也一并印出来（`None` = 这一台没答），因为「词没动」有两种来路：机器改了而合成后
/// 恰好同词，以及机器压根没答 —— 只有前者才叫 moved=false 而值得追。
fn theme_refresh(shown: &str, mut session: Pin<&mut session::qobject::Session>) {
    let before = crate::settings::startup_theme_face();
    let machine = platform_windows::theme::apps_use_light_theme_invalidate();
    session.as_mut().refresh_theme();
    let after = crate::settings::startup_theme_face();
    note(format!(
        "GUI 线程 设置变更 {shown}：机器答 {machine:?}，主题 {before}→{after}，moved={}",
        before != after
    ));
}

/// Freeze and open the mask, and print whichever half of that actually happened.
///
/// Shared by two callers on purpose: the immediate arm below, and [`crate::delay`]'s last
/// tick. They are the same product action (§5.2's 截图) reached at two different moments,
/// and a second copy of it would be a second sentence for the door to read back — which is
/// how `--hotkeys` and `--mask` stopped measuring one hot path.
pub(crate) fn open_mask(tag: &str, mut session: Pin<&mut session::qobject::Session>) {
    match mask::open_from_freeze(SHADER) {
        Ok(opened) => {
            session.as_mut().refresh_masks();
            note(format!(
                "GUI 线程 {tag}：遮罩开了 {} 屏，freeze {} ms",
                opened.slots, opened.ms
            ));
        }
        Err(e) => note(format!("GUI 线程 {tag}：遮罩开不起来：{e}")),
    }
}

/// The work itself, on the thread that owns Qt. Every line names the action it
/// answers, so the door's read-back can be tied to the press that caused it.
fn run(work: Work, mut session: Pin<&mut session::qobject::Session>) {
    let tag = work.tag();
    match work {
        Work::Capture => {
            // The same stamp the gauge doors write, so `--hotkeys` and `--mask`
            // measure one hot path rather than two that happen to look alike.
            state::stamp("asked");
            // §5.2.4：`capture.delay_seconds` 自此决定这一刻冻不冻画面 —— 这一枚 match 就是
            // 那唯一一位行为读者。0（默认那一侧）走的还是下面那条与 P108 之前逐字相同的路；
            // >0 把遮罩交给 `main.qml` 里那枚绑在 `delay_left` 上的定时器，由它在数完的那一刻
            // 回调 [`session::SessionRust::delay_round`]，于是画面和指针都冻在倒计时结束那一刻
            // （PRD 第 420 行要的正是这个时刻，而 P107 已把指针那一问安在冻结那一格里）。
            match delay::arm_from_config() {
                delay::Arm::Immediate => open_mask(&tag, session.as_mut()),
                delay::Arm::Waiting { left, step_ms } => {
                    session.as_mut().refresh_delay();
                    note(format!(
                        "GUI 线程 {tag}：延时截图，还剩 {left} 枚数才冻画面（每枚 {step_ms} ms）"
                    ));
                }
            }
        }
        Work::PastePin => {
            // `pin_clipboard` owns the failure sentence (it writes `status`); the
            // id it returns is the only thing this layer can check for itself.
            let id = session.as_mut().pin_clipboard();
            if id >= 0 {
                session.as_mut().set_status(QString::from("已从剪贴板贴图"));
            }
            note(format!("GUI 线程 {tag}：剪贴板贴图 id={id}"));
        }
        // Both legs end in a picture of something the desktop is showing right now,
        // so both answer with an id and the sentence the door prints beside it. A
        // refusal is `id=-1` for the same reason `pin_clipboard`'s is: the door grades
        // on whether the product answered, and a prose-only refusal would make that
        // grade a reading of this file's wording rather than of the run.
        Work::ActiveWindow | Work::Repeat => {
            let (leg, outcome) = if work == Work::ActiveWindow {
                ("活动窗口", capture::pin_active_window())
            } else {
                ("上一次区域", capture::pin_last_region())
            };
            match outcome {
                Ok((id, line)) => {
                    let said = format!("{leg}截图：{line}");
                    session.as_mut().set_status(QString::from(&*said));
                    note(format!("GUI 线程 {tag}：{leg}贴图 id={id}，{line}"));
                }
                Err(e) => {
                    let said = format!("{leg}没截成：{e}");
                    session.as_mut().set_status(QString::from(&*said));
                    note(format!("GUI 线程 {tag}：{leg}贴图 id=-1，{e}"));
                }
            }
        }
        Work::Quit => {
            // The mask's frames and windows go first: leaving them in a process on
            // its way out leaks nothing, but they would be the last thing the user
            // saw of a quit that was meant to be quiet.
            session.as_mut().end_mask();
            note(format!("GUI 线程 {tag}：退出请求已递交"));
            state::quit_after(0);
        }
    }
}

/// §5.16.2: does the application in front swallow this action?
///
/// Failing to name the foreground process lets the action through. That is the right
/// way round for a tool whose job is to look at the screen: a refusal to answer "who
/// is in front" - another user's elevated process, a protected one - is not evidence
/// the user asked for the key to stop working, and §5.16.2's exclusions are opt-in
/// silence rather than opt-in listening.
fn silenced(work: Work) -> Option<String> {
    let action = work.action()?;
    let (name, path) = platform_windows::proc::foreground_app()?;
    let exclusions = crate::settings::with(|s| s.config.hotkey.exclusions.clone());
    if hotkeys::is_suppressed(&exclusions, &path, action) {
        Some(format!("前台是 {name}"))
    } else {
        None
    }
}

/// The door's own clipboard leg, and the debt it takes on for using it.
///
/// `paste_pin` reads whatever the user last copied, so grading that work's *effect*
/// needs something on the clipboard to read. Two gauges already owe the machine for
/// taking one (`state::put_back`'s doc names them); this is the third, and it pays
/// into that same function rather than a fourth copy of its branches. The write is the
/// very call a pin's 复制 uses and the picture is two by two, so no one can mistake it
/// for what they copied - and it happens **only under `--hotkeys`**, never on the app's
/// own path, because a resident tool that took the clipboard over while nobody asked
/// would be a defect, not a feature. Sizes and answers reach the log, never pixels
/// (§9.1).
pub struct Clip {
    planted: (state::Check, String),
    /// What was on the clipboard before this leg wrote, and `None` whenever the write
    /// was refused or never attempted - so a leg that took nothing never claims to
    /// have given something back.
    before: Option<platform_windows::clip::Contents>,
}

impl Clip {
    /// The rungs are the product's own, in the product's order (see
    /// `annotate_check::clipboard_rows`): the first three are the shapes in which
    /// taking the clipboard would be taking it *irreversibly*, so nothing is written
    /// in them.
    pub fn plant() -> Self {
        let before = match platform_windows::clip::read() {
            Ok(contents) => contents,
            Err(e) => {
                return Self {
                    planted: (
                        state::Check::Blocked,
                        format!(
                            "剪贴板读不开（{e}），这一档不写自己放不回去的东西——一个字也没动它"
                        ),
                    ),
                    before: None,
                }
            }
        };
        let formats = platform_windows::clip::formats().unwrap_or_default();
        if !before.is_empty() && before.image_bytes.is_none() && before.text.is_none() {
            return Self {
                planted: (
                    state::Check::Blocked,
                    format!(
                        "剪贴板上是 {}，本应用不认这些格式，写下去就还不回来——一个字也没动它",
                        formats.join(",")
                    ),
                ),
                before: None,
            };
        }
        // Ask before writing, the way `--clipboard` does: the holder can be another
        // process, and then the honest reading is who holds it rather than a row of
        // failures about this program's clipboard code.
        if let Some(who) = platform_windows::clip::usable()
            .err()
            .map(|e| e.to_string())
        {
            return Self {
                planted: (
                    state::Check::Blocked,
                    format!("剪贴板此刻归别人，递不进去：{who}——一个字也没动它"),
                ),
                before: None,
            };
        }
        let frame = match Frame::from_rgba(
            2,
            2,
            vec![
                12, 34, 56, 255, 200, 100, 50, 255, 0, 255, 0, 128, 30, 40, 50, 255,
            ],
        ) {
            Ok(frame) => frame,
            Err(e) => {
                return Self {
                    planted: (
                        state::Check::Fail,
                        format!("这档自己连一张测试图都造不出来：{e}"),
                    ),
                    before: None,
                }
            }
        };
        // From here the clipboard has been emptied: `write_formats` opens with
        // `EmptyClipboard`, so the debt exists whether this write succeeds or not.
        let planted = match platform_windows::clip::write_image(&frame) {
            Ok(()) => (
                state::Check::Pass,
                format!(
                    "{}×{} 的一张测试图已按 pin 复制那一条腿（clip::write_image）递进剪贴板，\
                     原本上面是 {}；下面 press paste_pin 贴出来的就是这张两像素的图，\
                     不是用户之前拷的东西",
                    frame.width,
                    frame.height,
                    if before.is_empty() {
                        "空的"
                    } else {
                        "一份本应用能还回去的东西"
                    }
                ),
            ),
            Err(e) => (
                state::Check::Fail,
                format!(
                    "write_image 答 {e}（问过 usable 是通的，写这一笔仍可能被系统拒）——\
                     但 EmptyClipboard 已经在它前面执行了，所以这一格不能报成\"没动过\""
                ),
            ),
        };
        Self {
            planted,
            before: Some(before),
        }
    }

    /// Give the user's clipboard back, as far as this app can represent it. Runs after
    /// the presses, because the paste leg's own reading has to see the planted picture
    /// first - the same read-before-restore order `annotate_check` learned the hard way.
    ///
    /// A failed restore is `Fail`, not `Blocked`: the contents were taken by this leg
    /// and they are gone whatever caused the refusal, and a row that read "the machine
    /// declined to be measured" would hide that behind an excuse.
    fn settle(&self) -> (state::Check, String) {
        match &self.before {
            None => (
                state::Check::Pass,
                "这一档没写剪贴板（上面那一行说了为什么），所以没有要还的东西".to_string(),
            ),
            Some(_) => match state::put_back(&self.before) {
                Ok(what) => (state::Check::Pass, format!("还回去的是：{what}")),
                Err(e) => (state::Check::Fail, format!("还不回去：{e}")),
            },
        }
    }

    /// The app's own path: no measurement, no write, no debt.
    pub fn untouched() -> Self {
        Self {
            planted: (
                state::Check::Blocked,
                "这一档没跑：只有 `--hotkeys` 才动剪贴板，产品自己那条路读的是用户拷的东西"
                    .to_string(),
            ),
            before: None,
        }
    }
}

/// What the door did so §5.2.5 could be pressed at all.
///
/// `capture_repeat` repeats the region the *last* capture used, and a door that has
/// not captured anything has no last - so the run plants one the same way
/// [`Clip::plant`] plants a picture: through the product's own leg
/// ([`capture::pin_rect`], which is what writes the memory), at the centre of the
/// primary monitor, before any press. The row's grading then has the same two
/// answers as the paste leg's: a repeat that comes back `id=-1` after the door
/// itself made a region exist is the product refusing to repeat what it just
/// captured, while a plant that could not freeze anything is the machine declining
/// to be measured.
pub struct Region {
    pub planted: (state::Check, String),
}

impl Region {
    /// Freeze, take 320×200 out of the middle of the primary monitor and pin it -
    /// which is also the act that gives this process a 上一次区域.
    ///
    /// The memory is wiped first. A run that also asked for `--rect` has a region
    /// already, and `press capture_repeat` would then be measuring the CLI leg's
    /// rectangle while the row beside it claims the door planted one.
    pub fn plant() -> Self {
        capture::forget_region();
        let said = |check, line: String| Self {
            planted: (check, line),
        };
        let rect = match capture::centre_rect(320, 200) {
            Ok(rect) => rect,
            Err(e) => {
                return said(
                    state::Check::Blocked,
                    format!("连整屏都冻不出来，也就没有一个可重复的区域：{e}——一张贴图也没造"),
                )
            }
        };
        match capture::pin_rect(&rect) {
            Ok(id) => said(
                state::Check::Pass,
                format!(
                    "{rect:?} 已按产品自己那条腿（capture::pin_rect）贴成 id={id}，\
                     下面 press capture_repeat 重复的就是这一块"
                ),
            ),
            Err(e) => said(
                state::Check::Fail,
                format!(
                    "冻住了、区域也算出来了，可 {rect:?} 仍没贴成：{e}——这一格不能报成\"没动过\""
                ),
            ),
        }
    }

    /// The app's own path: no measurement, no extra picture. The resident process
    /// gets its 上一次区域 from the captures the user actually made.
    pub fn untouched() -> Self {
        Self {
            planted: (
                state::Check::Blocked,
                "这一档没跑：只有 `--hotkeys` 才替重复造一个区域，产品自己那条路重复的是用户上一次真的截过的地方"
                    .to_string(),
            ),
        }
    }
}

/// Stand in for one keystroke: post the delivery the shell would post for a
/// registered key. The registration is the shell's own; only the finger is missing,
/// so a run that presses this has proved the chain from a `WM_HOTKEY` onward and
/// nothing about the combination - see [`platform_windows::Pump::press`].
pub fn press(action: &str) -> bool {
    let id = held(&ROUTES)
        .iter()
        .find(|route| route.action == action)
        .map(|route| route.id);
    let pump_cell = held(&PUMP);
    let Some(pump) = pump_cell.as_ref() else {
        return false;
    };
    id.is_some_and(|id| pump.press(id))
}

/// §5.19.1 第 3 步的那一发广播，由门自己替系统播一次：把这枚进程里那两条线程之间的链
/// 走满一遍，而缺的是「另一个进程真的播了」。泵不在（`install` 没成）就是 `false`，
/// 与那枚顶层窗口没建起来（`settings_hwnd == 0`）在读回里分得开。
pub fn notify_setting_change(name: &str) -> bool {
    let pump_cell = held(&PUMP);
    match pump_cell.as_ref() {
        Some(pump) => pump.notify_setting_change(name),
        None => false,
    }
}

/// Unregister, remove the icon, stop both threads. The order matters twice over:
/// the pump's own `stop` unregisters before its window dies, so nothing is left
/// holding a key; and dropping the sender inside it is what lets the listener's
/// `recv()` return - so the pump goes first and the join second.
///
/// Never called from the listener: 退出 asks the event loop to end, and the process's
/// own `main` tears this down on the way out.
pub fn shutdown() {
    if let Some(mut pump) = held(&PUMP).take() {
        pump.stop();
    }
    if let Some(handle) = held(&LISTENER).take() {
        let _ = handle.join();
    }
    held(&ROUTES).clear();
    held(&GUI).take();
}

pub fn is_running() -> bool {
    held(&PUMP).is_some()
}

/// One injected press, graded. The three answers are not two: a `posted=false` is
/// this process having no jurisdiction over that id (it was never registered, so
/// there is no keystroke to imitate), while `posted=true` with no line after it is
/// a chain that swallowed a message - a defect.
///
/// `setup_ok` names the door's own prior link. With a clipboard this door wrote
/// itself, an `id=-1` is the product failing to read what is demonstrably there -
/// `Fail`. On a clipboard this process does not control, the same answer is the
/// machine declining to be measured - `Blocked`. The repeat leg is graded the same
/// way against [`Region::plant`], and the plain capture leg has no setup to speak of:
/// a freeze this process cannot take is always the environment's answer.
fn press_verdict(
    action: &str,
    posted: bool,
    history: &[String],
    setup_ok: bool,
) -> (state::Check, Vec<String>) {
    // The action plus the colon every note line writes after it. Matching the bare
    // name would let `capture` read `capture_repeat`'s and `capture_active_window`'s
    // chains as its own - three actions share that prefix, and a row that passes on
    // another leg's pixels is the kind of reading this door exists to refuse.
    let needle = format!("{action}：");
    let lines: Vec<String> = history
        .iter()
        .filter(|line| line.contains(&needle))
        .cloned()
        .collect();
    let queued = lines.iter().any(|line| line.contains("排队到"));
    let gui = lines.iter().any(|line| line.starts_with("GUI 线程 "));
    let refused = lines
        .iter()
        .any(|line| line.contains("遮罩开不起来") || line.contains("id=-1"));
    let verdict = if !posted {
        state::Check::Blocked
    } else if !queued || !gui {
        state::Check::Fail
    } else if refused {
        if setup_ok {
            state::Check::Fail
        } else {
            // A freeze the machine would not hand over, and a clipboard this process
            // does not hold: both are the environment declining to be measured.
            state::Check::Blocked
        }
    } else {
        state::Check::Pass
    };
    (verdict, lines)
}

/// The door's read-back: what the shell took, what the tray says, and what the
/// injected presses did to the product.
///
/// `clip` is the door's own clipboard leg, planted by `main` before the presses: the
/// paste leg's grading depends on whether there was anything to paste, and this is the
/// one place that says which of the two it was. It is also what pays the clipboard back
/// - on the way out of this function, after every chain reading has been taken.
///
/// `region` is the same argument for §5.2.5: the repeat leg can only be graded
/// against a region this run made exist.
///
/// `setting` is the §5.19.1 leg: the name the door broadcast on the system's behalf,
/// and whether the send came back. A run that never broadcast anything passes
/// `("", false)`, which the row reads as its own answer rather than as a zero.
pub fn report(
    launch: &Launch,
    asked_ms: i32,
    ran_ms: i32,
    injected: &[(String, bool)],
    clip: &Clip,
    region: &Region,
    setting: (&str, bool),
) -> (state::Check, String) {
    let mut rep = Report::new();
    // The read-back's own place on the clock. `main` stamped `hotkeys loop` just before
    // it handed the thread to `exec()`, so the difference is how long the event loop
    // actually ran rather than how long this door asked it to - which is the only
    // reading that can tell "the timer ended the run" from "something ended it early".
    state::stamp("hotkeys back");
    crate::r13::scene_row(&mut rep);

    let started = &launch.started;
    let history = history();
    let handed = history
        .iter()
        .any(|line| line.contains("GUI 线程句柄已到手"));
    rep.row(
        "qml handed the thread",
        state::Check::from(handed && state::qml_loaded()),
        format!(
            "handThread 的读数={handed}（历史第一行 {:?}），qml_loaded={}",
            history.first().map(String::as_str).unwrap_or(""),
            state::qml_loaded()
        ),
    );

    // The census, numerator *and* denominator: one row per configured action, and
    // the shell's own answer for every registration this build asked for.
    let named = rows_of_actions(&launch.rows);
    let counted = started.registered.len() + started.refused.len();
    rep.row(
        "census adds up",
        state::Check::from(counted == launch.attempts && named == launch.actions),
        format!(
            "配置里 {} 个动作，逐个有一行（读到 {named} 行，冲突 {} 行另计）；递出去注册 {} 个，\
             shell 答 registered={} + refused={}（合计 {counted}）；没要那把键的动作 {} 个\
             （未绑定/保留/未接线，它们的键留在系统那边）",
            launch.actions,
            launch.rows.len() - named,
            launch.attempts,
            started.registered.len(),
            started.refused.len(),
            launch.actions.saturating_sub(launch.attempts)
        ),
    );
    for row in &launch.rows {
        rep.note("install", row.clone());
    }

    rep.row(
        "tray icon is shown",
        if started.tray {
            state::Check::Pass
        } else {
            state::Check::Blocked
        },
        format!(
            "NIM_ADD 的答复={}，菜单 {} 项 {:?}；tooltip={:?}（改写送达={}）。托盘不判成败：\
             通知区域可以拒一个图标，那两个键照样能用",
            started.tray,
            launch.menu.len(),
            launch.menu.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            launch.tip,
            launch.tip_set
        ),
    );

    // The menu's routes, read as a table rather than as a click: `TrackPopupMenu` is
    // modal to the pump thread, so a door that opened it would be waiting on a
    // human. Every id it could return names a work here, which is what the routing
    // table says without the menu.
    rep.note(
        "menu routes",
        format!(
            "{} 项的归宿 {}",
            launch.menu.len(),
            launch
                .menu
                .iter()
                .map(|(id, label)| format!("{id}={label}→{}", label_of(*id)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    );
    rep.note(
        "menu not clicked",
        "这一档不点菜单：TrackPopupMenu 对泵线程是模态的，点开它就是等真人。左键那一路\
         （Event::TrayLeft → 截图）同样要真人——PRD §5.2.2 的那一次点击不在能注入的范围里"
            .to_string(),
    );

    // The paste leg's precondition, said before its verdict: whether there was
    // anything on the clipboard to paste, and who put it there. This is the row that
    // lets `press paste_pin` answer `Fail` instead of `SKIP` on a door that planted a
    // bitmap the product then could not read.
    let planted = clip.planted.0 == state::Check::Pass;
    rep.row(
        "clipboard planted by the door",
        clip.planted.0,
        clip.planted.1.clone(),
    );
    rep.row(
        "region planted by the door",
        region.planted.0,
        region.planted.1.clone(),
    );
    let region_planted = region.planted.0 == state::Check::Pass;

    for (action, posted) in injected {
        let setup_ok =
            (action == "paste_pin" && planted) || (action == "capture_repeat" && region_planted);
        let (verdict, lines) = press_verdict(action, *posted, &history, setup_ok);
        rep.row(
            &format!("press {action}"),
            verdict,
            format!(
                "PostMessageW 的答复={posted}（WM_HOTKEY，只对本进程注册过的 id 有效）；链路读数 {:?}。\
                 真按键要键盘驱动经 shell 投给这个窗口，这一档缺的就是那一步：注册是真的，手指是替的",
                if lines.is_empty() {
                    vec!["一行都没有".to_string()]
                } else {
                    lines
                }
            ),
        );
    }

    // §5.19.1 第 3 步：那一发广播本来由**别的进程**播给系统的顶层窗口，门替不了它，只能替
    // 系统播一次。所以三格读数各自说自己那一头：那枚顶层窗口在不在、监听线程有没有接住、
    // GUI 线程有没有真的重问注册表。名字是证人不是判据 —— 下面判的三问里没有一问读它。
    let (setting_name, setting_sent) = setting;
    let lines: Vec<String> = history
        .iter()
        .filter(|line| line.contains("设置变更"))
        .cloned()
        .collect();
    let queued = lines
        .iter()
        .any(|line| line.starts_with("设置变更") && line.contains("排队到 GUI 线程"));
    let reasked = lines
        .iter()
        .find(|line| line.starts_with("GUI 线程 设置变更"))
        .is_some_and(|line| line.contains("机器答"));
    let moved = lines
        .iter()
        .find(|line| line.starts_with("GUI 线程 设置变更"))
        .map(|line| line.contains("moved=true"))
        .unwrap_or(false);
    rep.row(
        "wm_settingchange reaches the theme",
        if started.settings_hwnd == 0 || !setting_sent || !queued || !reasked {
            state::Check::Fail
        } else {
            state::Check::Pass
        },
        format!(
            "顶层窗口 hwnd={}（`0` = 没建起来；`HWND_BROADCAST` 那条链不含消息窗口，所以这一枚\
             必须是顶层的），替系统播一次 name={setting_name:?} 的答复={setting_sent}\
             （`SendMessageW` 返回即接收方 wndproc 已读完那串指针）；监听线程接住={queued}，\
             GUI 线程重问注册表={reasked}，主题变了={moved}（这一档不动用户的选择，重问答的\
             就是同一份，所以 `false` 是这一趟应有的读数）；链路读数 {:?}\
             ——缺的那一半是别的进程真播了这一发，而那一半不在本进程能替的范围内",
            started.settings_hwnd,
            if lines.is_empty() {
                vec!["一行都没有".to_string()]
            } else {
                lines
            }
        ),
    );

    // The debt, paid after every chain reading above was taken. This row is part of the
    // verdict, not a footnote: a door that took the user's clipboard and could not give
    // it back is a door that did damage, whatever the two keys otherwise proved.
    let back = clip.settle();
    rep.row("clipboard given back", back.0, back.1);

    // What the product ended up looking like, read out of its own state rather than
    // out of the sentences above.
    let (shown, slots) = mask::with(|m| (m.shown, m.slots.len()));
    let pins = state::with(|s| s.ids().len());
    let ran = state::between("hotkeys loop", "hotkeys back");
    rep.row(
        "product state moved",
        state::Check::from(shown && slots > 0),
        format!(
            "遮罩 shown={shown} slots={slots}（hwnd={} 的泵驱动的那一次冻结），贴图 {pins} 张；\
             事件环实际跑了 {}（闹钟定在 {ran_ms} ms，命令行要的是 {asked_ms} ms）——短于闹钟就是\
             有人提前带走了进程，而这一档里唯一能提前带走它的东西是\"最后一个窗口关了\"",
            started.hwnd,
            ran.map(|ms| format!("{ms} ms"))
                .unwrap_or_else(|| "没打点，量不出来".to_string())
        ),
    );
    rep.note(
        "chain",
        format!(
            "整条链的读数 {}",
            if history.is_empty() {
                "一行也没有".to_string()
            } else {
                history.join(" ┃ ")
            }
        ),
    );
    rep.finish("hotkeys door")
}

/// The per-action rows, which is the census: conflict rows are about a pair of
/// actions rather than one, so they are counted apart.
fn rows_of_actions(rows: &[String]) -> usize {
    rows.iter().filter(|row| !row.starts_with("冲突")).count()
}

/// What the routing table says a menu id leads to, for the report's route line.
fn label_of(id: u32) -> String {
    held(&ROUTES)
        .iter()
        .find(|route| route.id == id)
        .map(|route| route.work.label().to_string())
        .unwrap_or_else(|| "没有归宿".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use falcon_core::config::{AppExclusion, Config};
    use platform_windows::hotkey::{virtual_key, MOD_CONTROL, MOD_SHIFT};

    /// §5.1.1 的 `show_tray` 分岔，两边各一次：真给 `Some(TraySpec)` 且把两样材料
    /// 原样送到 `Pump::start`，假给 `None` 且 `Shell_NotifyIcon` 挂不上。P66 落这
    /// 支分岔时它只有 install 现场作证人（那一头需要真消息泵、headless 跑不动）；
    /// 本轮把分岔本体抽成 `tray_from` 这一枚纯函数，于是 §9.1 第十条的"由 cargo
    /// 单测而不是由门作证"这一族自此也覆盖到 `[general] show_tray` 那一格。
    #[test]
    fn tray_honours_the_general_switch() {
        let tip = "Falconshot 正在运行".to_string();
        let items: Vec<(u32, String)> =
            vec![(1, "截图".into()), (2, "贴图".into()), (3, "退出".into())];

        let on = tray_from(true, tip.clone(), items.clone());
        let on = on.expect("show_tray = true must hand Pump::start a Some(TraySpec)");
        assert_eq!(on.tip, tip);
        assert_eq!(
            on.items, items,
            "the three menu rows must reach the pump verbatim"
        );

        let off = tray_from(false, tip, items);
        assert!(
            off.is_none(),
            "show_tray = false must hand Pump::start a None so Shell_NotifyIcon does not mount"
        );
    }

    /// The wiring table, and the promise it keeps: an action this build cannot
    /// perform is left alone rather than taken over. `capture_custom` is the row that
    /// makes the difference visible - a shipped default whose coordinates dialog
    /// (§5.3.5) does not exist, so its key stays the user's.
    #[test]
    fn only_wired_actions_are_asked_for() {
        let actions = Config::default().hotkey.actions;
        assert_eq!(actions.len(), 9, "§5.16.1 的表不再是九个动作");
        let planned = plan(&actions);
        let asked: Vec<&str> = planned
            .requests
            .iter()
            .map(|request| request.action.as_str())
            .collect();
        assert_eq!(
            asked,
            vec![
                "capture",
                "capture_active_window",
                "capture_repeat",
                "paste_pin"
            ],
            "接线表变了"
        );
        assert_eq!(planned.routes.len(), 4, "路由表该和注册表一样长");
        assert_eq!(
            rows_of_actions(&planned.rows) + planned.requests.len(),
            actions.len()
        );
        for row in &planned.rows {
            assert!(row.contains("未接线"), "{row}");
        }
        assert_eq!(planned.next, 5, "id 从 1 连续发到注册的下一个");
        // The five that stay the user's, by name: this is the census the report
        // prints, and a row that dropped out of it would take a key with it.
        let unwired: Vec<&str> = planned
            .rows
            .iter()
            .map(|row| row.split(' ').next().unwrap_or_default())
            .collect();
        assert_eq!(
            unwired,
            vec![
                "capture_custom",
                "pin_edit",
                "solo",
                "switch_group",
                "toggle_pins"
            ]
        );
    }

    /// Census rule the report asserts: every configured action gets exactly one row,
    /// whatever it says. A row count that drifts is how a refused key disappears
    /// from a report rather than appearing in it.
    #[test]
    fn every_action_gets_exactly_one_row() {
        let mut actions = Config::default().hotkey.actions;
        actions.insert("solo".to_string(), "".to_string());
        actions.insert("switch_group".to_string(), "ctrl+shift+z".to_string());
        actions.insert("pin_edit".to_string(), "win+l".to_string());
        actions.insert("toggle_pins".to_string(), "nonsense".to_string());
        let planned = plan(&actions);
        // One layer at a time: `plan` writes the row for every action it has *finished*
        // talking about, and hands the ones it wants registered to `install`, which
        // writes their row only once the shell has answered. So the census of this
        // layer is rows + requests, and the census of the report is rows alone.
        assert_eq!(
            rows_of_actions(&planned.rows) + planned.requests.len(),
            actions.len()
        );
        // The four shapes that stop a registration are said in four voices, and each
        // one names the action it refused.
        let text = planned.rows.join("\n");
        for (needle, action) in [
            ("未绑定", "solo"),
            ("系统保留组合", "pin_edit"),
            ("注册不了", "toggle_pins"),
            ("未接线", "capture_custom"),
        ] {
            assert!(
                planned
                    .rows
                    .iter()
                    .any(|row| row.contains(needle) && row.contains(action)),
                "{needle} 的那行没说到 {action}"
            );
            assert!(text.contains(needle), "{needle} 一行都没有");
        }
    }

    /// The four defaults that *are* registered must survive the whole trip: core's
    /// grammar, core's bindability rule, and this module's virtual-key table. A
    /// default that cannot register is a shipped key that does nothing.
    #[test]
    fn the_four_defaults_reach_a_binding() {
        for (text, action, work, chords) in [
            ("ctrl+shift+a", "capture", Work::Capture, true),
            ("print", "capture_active_window", Work::ActiveWindow, false),
            ("ctrl+shift+x", "capture_repeat", Work::Repeat, true),
            ("ctrl+shift+v", "paste_pin", Work::PastePin, true),
        ] {
            let spec = hotkeys::parse(text).unwrap();
            assert!(spec.is_bindable(), "{text} 不该被 is_bindable 拦下");
            assert!(spec.reserved_reason().is_none(), "{text} 不是保留组合");
            let canonical = spec.canonical();
            let vk = virtual_key(spec.key).expect("{text} 的键在这张表里没有码");
            let binding = bind(spec, &canonical).unwrap();
            assert_eq!(Work::of(action), Some(work));
            assert_eq!(work.action(), Some(action));
            assert_eq!(binding.vk, vk, "{text} 落到别的键上了");
            // No modifier at all means no global delivery: `RegisterHotKey` takes the
            // combination, not the key, and three of the four shipped defaults are
            // chords. `bind` adds `MOD_NOREPEAT` on top, so this names the two keys a
            // user presses. `print` is the exception core allows exactly - a
            // modifierless key that is still delivered globally.
            assert_eq!(
                binding.modifiers & (MOD_CONTROL | MOD_SHIFT),
                if chords { MOD_CONTROL | MOD_SHIFT } else { 0 },
                "{text} 的修饰键位不是 Ctrl+Shift"
            );
        }
        let print = hotkeys::parse("print").unwrap();
        assert!(print.is_bindable(), "`print` 是 Win32 认得的特例");
        assert_eq!(Work::of("capture_custom"), None);
    }

    /// The tray menu always offers three entries and every one of them routes: an
    /// entry with no destination would be a menu item that does nothing when
    /// clicked, which is the failure this table exists to avoid.
    #[test]
    fn every_menu_entry_has_a_destination() {
        let mut planned = plan(&Config::default().hotkey.actions);
        let menu = tray_menu(&mut planned);
        assert_eq!(menu.len(), 3);
        assert_eq!(
            menu.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(),
            vec!["截图（Ctrl+Shift+A）", "贴出剪贴板（Ctrl+Shift+V）", "退出"]
        );
        for (id, _) in &menu {
            let route = planned
                .routes
                .iter()
                .find(|route| route.id == *id)
                .unwrap_or_else(|| panic!("菜单 id={id} 没有归宿"));
            assert!(!route.work.label().is_empty());
            assert!(!route.work.tag().is_empty());
        }
        // 退出 is the one entry no key answers, and its id is not a hot key id.
        let quit = planned
            .routes
            .iter()
            .find(|route| route.work == Work::Quit)
            .unwrap();
        assert_eq!(quit.action, "");
        assert!(quit.display.is_empty());
        assert!(!planned.requests.iter().any(|request| request.id == quit.id));

        // An unbound `capture` still gets a menu entry - the icon is the other half
        // of §5.2.2 - and that entry takes an id the shell was never asked about, so
        // `press` has nothing to imitate for it. The label drops the key name,
        // because there is no key to name.
        let mut unbound = Config::default().hotkey.actions;
        unbound.insert("capture".to_string(), "".to_string());
        let mut planned = plan(&unbound);
        let menu = tray_menu(&mut planned);
        assert_eq!(menu[0].1, "截图");
        let entry = planned
            .routes
            .iter()
            .find(|route| route.work == Work::Capture)
            .unwrap();
        assert!(
            !planned
                .requests
                .iter()
                .any(|request| request.id == entry.id),
            "未绑定的一档不该出现在注册里"
        );
    }

    /// §5.16.2's exclusion, as the listener applies it - including the two shapes
    /// that are easy to get backwards: an empty action list silences *everything*,
    /// and a named action silences only itself.
    #[test]
    fn exclusions_silence_the_action_they_name() {
        let every = [AppExclusion {
            app: "game.exe".to_string(),
            actions: vec![],
        }];
        let only_capture = [AppExclusion {
            app: "game.exe".to_string(),
            actions: vec!["capture".to_string()],
        }];
        assert!(hotkeys::is_suppressed(
            &every,
            "C:\\Games\\game.exe",
            "capture"
        ));
        assert!(hotkeys::is_suppressed(
            &every,
            "C:\\Games\\game.exe",
            "paste_pin"
        ));
        assert!(hotkeys::is_suppressed(&only_capture, "game.exe", "capture"));
        assert!(!hotkeys::is_suppressed(
            &only_capture,
            "game.exe",
            "paste_pin"
        ));
        assert!(!hotkeys::is_suppressed(
            &every,
            "C:\\Windows\\explorer.exe",
            "capture"
        ));
        // The one thing this module adds to core's rule: 退出 has no action, so no
        // exclusion can reach it - the menu stays usable in front of a game.
        assert_eq!(Work::Quit.action(), None);
    }

    /// The three answers of an injected press are not two, and the door's grading is
    /// the only place that decision lives. A chain that posted but produced no
    /// product line is a defect; an id that was never registered is not. The fourth
    /// answer comes from `setup_ok`: the same `id=-1` is the machine's answer on a
    /// clipboard nobody planted, and the product's answer on one this door wrote.
    #[test]
    fn an_injected_press_is_graded_by_what_came_back() {
        let history = vec![
            "热键 id=1 capture：排队到 GUI 线程（截图）".to_string(),
            "GUI 线程 capture：遮罩开了 2 屏，freeze 31 ms".to_string(),
            "热键 id=2 paste_pin：被静音（前台是 game.exe）".to_string(),
        ];
        assert_eq!(
            press_verdict("capture", true, &history, false).0,
            state::Check::Pass
        );
        assert_eq!(
            press_verdict("paste_pin", true, &history, false).0,
            state::Check::Fail,
            "被静音不是链路通了"
        );
        assert_eq!(
            press_verdict("solo", false, &history, false).0,
            state::Check::Blocked,
            "没注册的 id 不是产品答错了"
        );
        // A machine that will not hand over a freeze, and a clipboard with no image.
        let refused = vec![
            "热键 id=1 capture：排队到 GUI 线程（截图）".to_string(),
            "GUI 线程 capture：遮罩开不起来：拿不到桌面快照".to_string(),
            "热键 id=2 paste_pin：排队到 GUI 线程（贴出剪贴板）".to_string(),
            "GUI 线程 paste_pin：剪贴板贴图 id=-1".to_string(),
        ];
        assert_eq!(
            press_verdict("capture", true, &refused, false).0,
            state::Check::Blocked
        );
        assert_eq!(
            press_verdict("paste_pin", true, &refused, false).0,
            state::Check::Blocked
        );
        assert_eq!(
            press_verdict("paste_pin", true, &refused, true).0,
            state::Check::Fail,
            "这一档自己写进去的图，产品读不出来就不是环境的账"
        );
        // A press that reached the GUI thread and produced a real id is the pass this
        // door exists to certify, and the plant does not soften it.
        let pasted = vec![
            "热键 id=2 paste_pin：排队到 GUI 线程（贴出剪贴板）".to_string(),
            "GUI 线程 paste_pin：剪贴板贴图 id=7".to_string(),
        ];
        assert_eq!(
            press_verdict("paste_pin", true, &pasted, true).0,
            state::Check::Pass
        );
        // Three of the nine actions begin with the same word, so a row for `capture`
        // must not be satisfied by a leg that repeated a region instead. This is the
        // reading the door would otherwise get wrong on a machine where the mask never
        // opened but the repeat worked.
        let three = vec![
            "热键 id=1 capture：排队到 GUI 线程（截图）".to_string(),
            "热键 id=2 capture_active_window：排队到 GUI 线程（活动窗口截图）".to_string(),
            "热键 id=3 capture_repeat：排队到 GUI 线程（重复上次截图）".to_string(),
            "GUI 线程 capture_active_window：活动窗口贴图 id=3，notepad".to_string(),
            "GUI 线程 capture_repeat：上一次区域贴图 id=4，120 ms".to_string(),
            "GUI 线程 capture：遮罩开了 1 屏，freeze 151 ms".to_string(),
        ];
        let (verdict, lines) = press_verdict("capture", true, &three, false);
        assert_eq!(verdict, state::Check::Pass);
        assert_eq!(lines.len(), 2, "两个兄弟动作的链路不是这一行的读数");
        assert_eq!(
            press_verdict("capture_active_window", true, &three, false).0,
            state::Check::Pass
        );
        // A GUI line with no listener line in front of it is a chain that skipped a
        // link: the product answered, but this process never queued the press. That is
        // the reading `queued && gui` is there to keep apart from a pass.
        let orphan = vec!["GUI 线程 capture_repeat：上一次区域贴图 id=4".to_string()];
        assert_eq!(
            press_verdict("capture_repeat", true, &orphan, true).0,
            state::Check::Fail
        );
        // A refusal on the new legs says `id=-1`, so the door sees a chain that ran and
        // answered nothing. Whether that is the product's fault or the machine's is the
        // plant's answer, exactly as it is for the paste leg.
        let refused_repeat = vec![
            "热键 id=3 capture_repeat：排队到 GUI 线程（重复上次截图）".to_string(),
            "GUI 线程 capture_repeat：上一次区域贴图 id=-1，还没有截图过".to_string(),
        ];
        assert_eq!(
            press_verdict("capture_repeat", true, &refused_repeat, false).0,
            state::Check::Blocked
        );
        assert_eq!(
            press_verdict("capture_repeat", true, &refused_repeat, true).0,
            state::Check::Fail,
            "这一档自己种下的区域，产品不肯重复就不是环境的账"
        );
    }

    /// The census helper cannot be wrong about its own denominator: with a config
    /// whose every action is refused for a different reason, the row count still has
    /// to be the action count, or a key has vanished from the report.
    #[test]
    fn the_census_counts_rows_not_verdicts() {
        let mut actions = Config::default().hotkey.actions;
        actions.insert("capture".to_string(), "win+d".to_string());
        actions.insert("paste_pin".to_string(), "ctrl+shift+,.".to_string());
        // The two the previous round left unwired are wired now, so a config that wants
        // *no* registrations has to refuse those as well - the denominator this test
        // counts is the config's nine, not the four this build can perform.
        actions.insert("capture_active_window".to_string(), "".to_string());
        actions.insert("capture_repeat".to_string(), "".to_string());
        let planned = plan(&actions);
        assert_eq!(rows_of_actions(&planned.rows), actions.len());
        assert_eq!(planned.requests.len(), 0, "这趟一个都注册不了");
        let rows = vec![
            "冲突 Ctrl+Shift+A：capture + solo 都在用这一串键".to_string(),
            "capture Ctrl+Shift+A 已注册".to_string(),
        ];
        assert_eq!(rows_of_actions(&rows), 1);
    }
}
