//! §5.15 热区 Hot Corner: the bridge's half of the corner machine.
//!
//! [`falcon_core::hotcorner`] has answered "has this pointer sat in a corner long
//! enough" since it was written, and until this round nothing in the product called it.
//! The gap was never the arithmetic — a corner is not a thing a library can observe.
//! Somebody has to ask Windows where the pointer is, how long it has been there, and
//! whether the screen currently belongs to a fullscreen app.
//!
//! So this module splits those questions the way the rest of the bridge splits them:
//! [`step`], [`should_pause`] and [`covers_a_monitor_exactly`] are pure and graded by
//! [`check`] without a screen, while [`Runner`] is the only place that reads the OS. The
//! door (`--hotcorner`) grades the pure half and *observes* the live half — it never
//! dispatches, because a corner that fired during a measurement run would change the
//! desktop the run came to read.

use falcon_core::capture::MonitorInfo;
use falcon_core::config::{HotCorner, HOT_CORNER_ACTIONS};
use falcon_core::geometry::{PhysPoint, PhysRect, Scale};
use falcon_core::hotcorner::{Corner, Firing, HotCorners};
use falcon_core::hotkeys::Modifiers;

/// How often the resident pump wakes to look at the pointer when it has nothing else to
/// do. The default dwell is 300 ms, so four samples inside one dwell time it well without
/// turning the idle loop into a syscall storm.
pub const POLL_MS: u64 = 80;

/// How often [`Runner`] re-reads the config and the display layout. Corner squares are
/// geometry that moves when a monitor is plugged in, not every 80 ms.
pub const SNAPSHOT_MS: u64 = 1_000;

/// §5.15 规则: a fullscreen app or game may pause the corners — *by setting*.
///
/// `Unknown` is not `No`. A session with no foreground window (locked, or a desktop the
/// shell has not given focus) cannot answer the question, and guessing `Yes` would
/// silently disable a feature the user switched on, so an unanswered question leaves the
/// corners live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fullscreen {
    /// Some window's own bounds are exactly one monitor's bounds.
    Yes,
    /// Nothing on the desktop covers a monitor completely.
    No,
    /// The machine did not answer: no foreground window, or unreadable bounds.
    Unknown,
}

/// The whole content of §5.15's second rule, spelled out because both halves of it are
/// graded by name: with the switch off a corner keeps working even while a game holds the
/// screen, and with it on only a *confirmed* fullscreen app pauses anything.
pub fn should_pause(pause_on_fullscreen: bool, screen: Fullscreen) -> bool {
    pause_on_fullscreen && screen == Fullscreen::Yes
}

/// Does `rect` coincide exactly with one of `monitors`?
///
/// Equality, not containment: a maximised ordinary window is drawn *larger* than the
/// monitor it covers (the invisible resize border), so "at least as big as the screen"
/// would read every maximised window as a fullscreen game and pause the corners forever.
pub fn covers_a_monitor_exactly(rect: &PhysRect, monitors: &[MonitorInfo]) -> bool {
    !rect.is_empty() && monitors.iter().any(|m| &m.bounds == rect)
}

/// Fold the two OS answers into the three-way [`Fullscreen`], so a door can print which
/// answer it got: "there was a foreground window but no readable bounds" and "its bounds
/// are not the whole screen" are different facts about the machine, and only the first is
/// a reason not to trust the reading.
pub fn fullscreen_of(
    foreground: Option<u64>,
    rect: Option<PhysRect>,
    monitors: &[MonitorInfo],
) -> Fullscreen {
    match (foreground, rect) {
        (Some(_), Some(r)) if covers_a_monitor_exactly(&r, monitors) => Fullscreen::Yes,
        (Some(_), Some(_)) => Fullscreen::No,
        _ => Fullscreen::Unknown,
    }
}

/// One poll of the corner machine.
///
/// `None` for `sample` means the pointer could not be read at all. That ends every visit
/// rather than extending a dwell on stale coordinates: a corner has to be *sat in*, and an
/// unreadable pointer is no evidence anybody sat anywhere.
pub fn step(
    corners: &mut HotCorners,
    sample: Option<(PhysPoint, Modifiers)>,
    now_ms: u64,
) -> Option<Firing> {
    match sample {
        None => {
            corners.reset();
            None
        }
        Some((p, mods)) => corners.pointer(p, mods, now_ms),
    }
}

/// The live loop's state: the corner machine plus the snapshot it was built from, so a
/// change in either is a fact the run can print rather than an assumption.
pub struct Runner {
    cfg: HotCorner,
    monitors: Vec<MonitorInfo>,
    corners: HotCorners,
    /// How many times the snapshot moved. A run that never moved it reads `0`, and that is
    /// a reading rather than an absence (#453).
    pub rebuilds: u32,
}

impl Runner {
    pub fn new(cfg: HotCorner, monitors: Vec<MonitorInfo>) -> Self {
        let corners = HotCorners::new(&cfg, &monitors);
        Runner {
            cfg,
            monitors,
            corners,
            rebuilds: 0,
        }
    }

    /// Take a new snapshot only if something the corners depend on actually changed.
    ///
    /// Rebuilding on every poll would be a bug dressed up as a config refresh:
    /// [`HotCorners::new`] starts every visit over, so a periodic rebuild would clear the
    /// dwell the loop is trying to time *and* reset the repeat guard with it. The guard has
    /// to survive a thousand polls that change nothing — which is what the two rows
    /// `refresh 没换快照不清停留` and `refresh 换了快照才重开一次` grade.
    pub fn refresh(&mut self, cfg: HotCorner, monitors: Vec<MonitorInfo>) -> bool {
        if cfg == self.cfg && monitors == self.monitors {
            return false;
        }
        let was_paused = self.corners.paused();
        self.cfg = cfg;
        self.monitors = monitors;
        self.corners = HotCorners::new(&self.cfg, &self.monitors);
        self.corners.set_paused(was_paused);
        self.rebuilds += 1;
        true
    }

    pub fn corners(&self) -> &HotCorners {
        &self.corners
    }

    pub fn config(&self) -> &HotCorner {
        &self.cfg
    }

    pub fn layout(&self) -> &[MonitorInfo] {
        &self.monitors
    }

    /// Feed one sample, applying §5.15's pause rule first so a fullscreen app cannot be
    /// interrupted by a corner the user never meant to cross.
    pub fn poll(
        &mut self,
        screen: Fullscreen,
        sample: Option<(PhysPoint, Modifiers)>,
        now_ms: u64,
    ) -> Option<Firing> {
        self.corners
            .set_paused(should_pause(self.cfg.pause_on_fullscreen, screen));
        step(&mut self.corners, sample, now_ms)
    }
}

/// One 1,000x800 screen at 100%, so every coordinate the table below uses is arithmetic a
/// reader can check on the page. The default radius is 8 DIP, which at 100% is 8 physical
/// pixels: the bottom-right square is therefore x 992..=999, y 792..=799.
fn one_monitor() -> Vec<MonitorInfo> {
    vec![MonitorInfo {
        id: "0,0".to_string(),
        name: "Gauge".to_string(),
        friendly: "Gauge".to_string(),
        bounds: PhysRect::new(0, 0, 1_000, 800),
        scale: Scale::of(1.0),
        primary: true,
        builtin: false,
    }]
}

/// A `[hot_corner]` section with all four corners named, one per interesting case:
/// bottom-right fires `capture`, top-left fires `solo`, top-right is switched off, and
/// bottom-left carries `capture_repeat` — a legal *hot key* action that §5.15 deliberately
/// does not let a corner run.
fn gauge_cfg() -> HotCorner {
    let mut cfg = HotCorner {
        enabled: true,
        ..HotCorner::default()
    };
    cfg.corners
        .insert("bottom_right".to_string(), "capture".to_string());
    cfg.corners
        .insert("top_left".to_string(), "solo".to_string());
    cfg.corners.insert("top_right".to_string(), String::new());
    cfg.corners
        .insert("bottom_left".to_string(), "capture_repeat".to_string());
    cfg
}

const BR: (i32, i32) = (999, 799);
const BL: (i32, i32) = (0, 799);
const TL: (i32, i32) = (0, 0);
const TR: (i32, i32) = (999, 0);
const MIDDLE: (i32, i32) = (500, 400);
/// One pixel outside the eight-pixel square: near the corner, and never a trigger.
const NEAR_NOT_IN: (i32, i32) = (991, 799);

/// One row of a script: where the pointer was (`None` = the platform could not read it),
/// whether Ctrl was held, and the monotonic clock at that sample.
type Sample = (Option<(i32, i32)>, bool, u64);

/// A firing named the way the ledger quotes it: `bottom_right=capture@300`. The clock is
/// part of the name on purpose — two scripts that both fire exactly once are the same
/// answer under a count-only reading and a different product under this one, which is what
/// lets the row about an unreadable pointer say anything at all.
fn said(f: &Firing, now_ms: u64) -> String {
    format!("{}={}@{}", f.corner.key(), f.action, now_ms)
}

fn sample_at(point: Option<(i32, i32)>, ctrl: bool) -> Option<(PhysPoint, Modifiers)> {
    point.map(|(x, y)| {
        (
            PhysPoint::new(x, y),
            Modifiers {
                ctrl,
                ..Modifiers::default()
            },
        )
    })
}

/// Run one script on its own corner machine and collect what it fired.
///
/// One machine per case because a fired visit would otherwise leak into the next row's
/// expectations, and a ruler that grades two cases with one state answers two questions
/// with one reading (#281).
fn run_script(cfg: &HotCorner, monitors: &[MonitorInfo], script: &[Sample]) -> Vec<String> {
    let mut corners = HotCorners::new(cfg, monitors);
    script
        .iter()
        .filter_map(|(point, ctrl, now)| {
            step(&mut corners, sample_at(*point, *ctrl), *now).map(|f| said(&f, *now))
        })
        .collect()
}

/// The headless half of §5.15: real [`HotCorners`] machines, synthetic pointers, synthetic
/// modifier sets and a synthetic clock. Nothing here opens a window, reads a screen or
/// sleeps, which is why a locked session can still grade every row.
pub fn check() -> (crate::state::Check, String) {
    let mut bad: Vec<String> = Vec::new();
    let mut graded = 0usize;
    let monitors = one_monitor();
    let base = gauge_cfg();

    // (case, cfg override, script, expected firings) — one machine per row, so a fired
    // visit cannot leak into the next row's expectation.
    let scripts: Vec<(&str, Vec<Sample>, Vec<&str>)> = vec![
        (
            "停留不足不触发",
            vec![
                (Some(BR), false, 0),
                (Some(BR), false, 100),
                (Some(BR), false, 299),
            ],
            vec![],
        ),
        (
            "停留够了一次就翻不第二次",
            vec![
                (Some(BR), false, 0),
                (Some(BR), false, 300),
                (Some(BR), false, 900),
                (Some(BR), false, 2_000),
            ],
            vec!["bottom_right=capture@300"],
        ),
        (
            "扫过屏幕顶角只算一次动作",
            vec![
                (Some(BR), false, 0),
                (Some(BR), false, 300),
                (Some(MIDDLE), false, 400),
                (Some(BR), false, 400),
                (Some(BR), false, 700),
            ],
            vec!["bottom_right=capture@300"],
        ),
        (
            "隔够了一秒的新访问再触发",
            vec![
                (Some(BR), false, 0),
                (Some(BR), false, 300),
                (Some(MIDDLE), false, 400),
                (Some(BR), false, 400),
                (Some(BR), false, 700),
                (Some(MIDDLE), false, 1_300),
                (Some(BR), false, 1_300),
                (Some(BR), false, 1_700),
            ],
            vec!["bottom_right=capture@300", "bottom_right=capture@1700"],
        ),
        (
            "没绑动作的角落什么都不发",
            vec![
                (Some(TR), false, 0),
                (Some(TR), false, 300),
                (Some(TR), false, 3_000),
            ],
            vec![],
        ),
        (
            "五枚之外的动作名不认",
            vec![
                (Some(BL), false, 0),
                (Some(BL), false, 300),
                (Some(BL), false, 3_000),
            ],
            vec![],
        ),
        (
            "solo 那枚动作认",
            vec![(Some(TL), false, 0), (Some(TL), false, 300)],
            vec!["top_left=solo@300"],
        ),
        (
            "差一格就站在角落外",
            vec![
                (Some(NEAR_NOT_IN), false, 0),
                (Some(NEAR_NOT_IN), false, 300),
                (Some(NEAR_NOT_IN), false, 3_000),
            ],
            vec![],
        ),
        (
            "整段关掉什么都不发",
            vec![(Some(BR), false, 0), (Some(BR), false, 10_000)],
            vec![],
        ),
        (
            "要求 Ctrl 时裸指针不算进角落",
            vec![
                (Some(BR), false, 0),
                (Some(BR), false, 300),
                (Some(BR), true, 400),
                (Some(BR), true, 700),
            ],
            vec!["bottom_right=capture@700"],
        ),
        (
            "读不到指针就把这次访问作废",
            vec![
                (Some(BR), false, 0),
                (Some(BR), false, 200),
                (None, false, 250),
                (Some(BR), false, 300),
                (Some(BR), false, 600),
            ],
            vec!["bottom_right=capture@600"],
        ),
        (
            "没有显示器布局不是 panic",
            vec![(Some(BR), false, 0), (Some(BR), false, 10_000)],
            vec![],
        ),
    ];
    for (name, script, expect) in &scripts {
        let cfg = if *name == "整段关掉什么都不发" {
            let mut off = base.clone();
            off.enabled = false;
            off
        } else if *name == "要求 Ctrl 时裸指针不算进角落" {
            let mut need = base.clone();
            need.require_ctrl = true;
            need
        } else {
            base.clone()
        };
        let monitors_for = if *name == "没有显示器布局不是 panic" {
            vec![]
        } else {
            monitors.clone()
        };
        let got = run_script(&cfg, &monitors_for, script);
        graded += 1;
        if got != *expect {
            bad.push(format!("{name}: 期望 {expect:?} 收到 {got:?}"));
        }
    }

    // §5.15's own geometry: which corner a point belongs to. A corner square is a
    // rectangle over the virtual desktop, so the four vertices are graded as one row —
    // it is the reading `--hotcorner` prints, not a private fact of this file.
    {
        let corners = HotCorners::new(&base, &monitors);
        let at = [
            (BR, Some("bottom_right")),
            (BL, Some("bottom_left")),
            (TL, Some("top_left")),
            (TR, Some("top_right")),
            (NEAR_NOT_IN, None),
            (MIDDLE, None),
        ];
        let mut wrong: Vec<String> = Vec::new();
        for ((x, y), want) in at {
            let got = corners.corner_at(PhysPoint::new(x, y)).map(|c| c.key());
            if got != want {
                wrong.push(format!("({x},{y}) 期望 {want:?} 收到 {got:?}"));
            }
        }
        graded += 1;
        if !wrong.is_empty() {
            bad.push(format!("角落归属: {}", wrong.join(" ")));
        }
    }

    // §5.15 规则 2: the pause is a setting, so both switch positions are graded, through
    // the same Runner the resident pump drives.
    {
        let mut keep = Runner::new(base.clone(), monitors.clone());
        let seq: Vec<(Fullscreen, Sample)> = vec![
            (Fullscreen::No, (Some(BR), false, 0)),
            (Fullscreen::Yes, (Some(BR), false, 100)),
            (Fullscreen::Yes, (Some(BR), false, 600)),
            (Fullscreen::No, (Some(BR), false, 700)),
            (Fullscreen::No, (Some(BR), false, 1_000)),
        ];
        let mut got: Vec<String> = Vec::new();
        for (screen, (point, ctrl, now)) in &seq {
            if let Some(f) = keep.poll(*screen, sample_at(*point, *ctrl), *now) {
                got.push(said(&f, *now));
            }
        }
        graded += 1;
        // The pause clears the visit, so the dwell restarts when the screen is handed
        // back: the only acceptable firing is the one at 1,000 ms.
        if got != vec!["bottom_right=capture@1000".to_string()] {
            bad.push(format!(
                "全屏暂停时角落等着: 期望 [\"bottom_right=capture@1000\"] 收到 {got:?}"
            ));
        }

        let mut loose = {
            let mut cfg = base.clone();
            cfg.pause_on_fullscreen = false;
            Runner::new(cfg, monitors.clone())
        };
        graded += 1;
        let mut got = Vec::new();
        for (point, ctrl, now) in [(Some(BR), false, 0u64), (Some(BR), false, 300)] {
            if let Some(f) = loose.poll(Fullscreen::Yes, sample_at(point, ctrl), now) {
                got.push(said(&f, now));
            }
        }
        if got != vec!["bottom_right=capture@300".to_string()] {
            bad.push(format!(
                "开关关掉时全屏也照触发: 期望 [\"bottom_right=capture@300\"] 收到 {got:?}"
            ));
        }
    }

    // The three-way answer, and the maximised-window trap it exists to avoid.
    {
        let fs_rect = PhysRect::new(0, 0, 1_000, 800);
        let maximised = PhysRect::new(-8, -8, 1_016, 816);
        let other = PhysRect::new(0, 0, 900, 700);
        let empty = PhysRect::default();
        let cases: Vec<(&str, bool, bool)> = vec![
            (
                "恰好盖住一枚显示器算全屏",
                covers_a_monitor_exactly(&fs_rect, &monitors),
                true,
            ),
            (
                "最大化那一圈看不见的边不算全屏",
                covers_a_monitor_exactly(&maximised, &monitors),
                false,
            ),
            (
                "小一圈不算全屏",
                covers_a_monitor_exactly(&other, &monitors),
                false,
            ),
            (
                "空矩形不算全屏",
                covers_a_monitor_exactly(&empty, &monitors),
                false,
            ),
            (
                "一枚显示器也没有就不算全屏",
                covers_a_monitor_exactly(&fs_rect, &[]),
                false,
            ),
        ];
        for (name, got, want) in cases {
            graded += 1;
            if got != want {
                bad.push(format!("{name}: 期望 {want} 收到 {got}"));
            }
        }

        let reads: Vec<(&str, Fullscreen, Fullscreen)> = vec![
            (
                "没有前台窗是不确定",
                fullscreen_of(None, None, &monitors),
                Fullscreen::Unknown,
            ),
            (
                "读不到矩形也是不确定",
                fullscreen_of(Some(1), None, &monitors),
                Fullscreen::Unknown,
            ),
            (
                "前台是普通窗才算不全屏",
                fullscreen_of(Some(1), Some(other), &monitors),
                Fullscreen::No,
            ),
            (
                "前台正好盖住一整屏才算全屏",
                fullscreen_of(Some(1), Some(fs_rect), &monitors),
                Fullscreen::Yes,
            ),
        ];
        for (name, got, want) in reads {
            graded += 1;
            if got != want {
                bad.push(format!("{name}: 期望 {want:?} 收到 {got:?}"));
            }
        }
        graded += 1;
        if should_pause(false, Fullscreen::Yes) {
            bad.push("开关关掉时全屏仍被暂停（用户那句被吞了）".to_string());
        }
        graded += 1;
        if should_pause(true, Fullscreen::Unknown) {
            bad.push("机器没答就被当成全屏".to_string());
        }
    }

    // The five §5.15 actions each need a hand that can actually run them. Two of them
    // (`show_all_pins` / `hide_all_pins`) had no `Work` arm before this round, which is
    // how a corner could be *configured* to do something and still do nothing.
    {
        for name in HOT_CORNER_ACTIONS {
            graded += 1;
            if crate::hotkey::Work::of(name).is_none() {
                bad.push(format!("动作 {name} 配得上却没有归宿"));
            }
        }
    }

    // A snapshot refresh must not silently restart the dwell it is timing.
    {
        let mut keep = Runner::new(base.clone(), monitors.clone());
        graded += 1;
        let first = keep.poll(Fullscreen::No, sample_at(Some(BR), false), 0);
        if first.is_some() {
            bad.push("刚进角落就发了（停留还没数完）".to_string());
        }
        if keep.refresh(base.clone(), monitors.clone()) {
            bad.push("同一份快照被判成变了".to_string());
        }
        let same = keep.poll(Fullscreen::No, sample_at(Some(BR), false), 300);
        if same.map(|f| said(&f, 300)) != Some("bottom_right=capture@300".to_string()) {
            bad.push("没换快照却清了停留".to_string());
        }
        graded += 2;
        if keep.rebuilds != 0 {
            bad.push(format!("没换快照却重开了 {} 次", keep.rebuilds));
        }
        let mut moved = base.clone();
        moved.dwell_ms = 600;
        if !keep.refresh(moved.clone(), monitors.clone()) {
            bad.push("换了配置却没重开".to_string());
        }
        if keep.rebuilds != 1 {
            bad.push(format!("换配置应重开一次，实为 {}", keep.rebuilds));
        }
        // The visit restarts with the new squares, so 300 ms of a 600 ms dwell is nothing.
        if keep
            .poll(Fullscreen::No, sample_at(Some(BR), false), 700)
            .is_some()
        {
            bad.push("重开后仍按旧停留计时".to_string());
        }
        graded += 4;
    }

    let verdict = bad.is_empty();
    (
        crate::state::Check::from(verdict),
        format!(
            "{graded} 例合成指针与全屏读法 + 五枚角落动作路由；{}（真指针与真角落那一腿在 `--hotcorner` 门上观测，不是本档的 headless 半边）",
            if verdict {
                "全部对上".to_string()
            } else {
                format!("对不上：{}", bad.join(" "))
            }
        ),
    )
}

/// 这一档自己读真指针至少要给的毫秒数：短于一枚 dwell 就连一次停留都量不出来，而那正是
/// §5.15 第 4 步要量的那一件东西。
pub const MIN_LIVE_MS: i32 = 1_200;

/// 再多也不读了。live 那一格读的是「这台机器答不答得出指针与全屏」，不是「这个人肯不肯把
/// 鼠标停在角上」—— 后一件是真人 A/B 的账，门不许替它读数。
pub const MAX_LIVE_MS: i64 = 3_000;

/// `--hotcorner <ms>` 的那一扇门：§5.15 的两半都印出来，而**一次也不派发**。
///
/// 派发那一条腿在 [`crate::hotkey`] 的 `CornerPoll` 里，只有产品自己（无 argv 那一趟）走得到。
/// 这一趟来读桌面，而角落的动作改的就是桌面上的贴图：一次测量跑不该动它，也不该因为量它而
/// 多收起或放开一次。于是这一档四格：
///
/// * `corner headless` —— [`check`]，合成指针、合成时钟，锁屏会话也判得出。
/// * `corner geometry` —— **这台机器**的真显示器与由它算出的四枚角落方块。
/// * `corner sweep` —— 真几何、真配置，合成指针与合成时钟：把每一枚绑了动作的角落停够 dwell，
///   看它交不交出一个动作。时钟合成是因为真等四枚 dwell 只是把 live 那一格再等一遍，读到的
///   还是同一个数。
/// * `corner live` —— **这台机器**的真指针、真修饰键、真时钟。没有人的手把指针停在角上时
///   触发次数为 0 是这一趟的实话，不是失败；它也不许因此变绿，所以它判的是「问不问得出」。
pub fn measure(live_want: i32) -> (crate::state::Check, String) {
    use crate::r13::Report;
    use crate::state::Check;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (headless, detail) = check();
    rep.row("corner headless", headless, detail);

    let cfg = crate::settings::hot_corner_cfg();
    let bound: Vec<(Corner, String)> = Corner::ALL
        .iter()
        .filter_map(|c| {
            cfg.corners
                .get(c.key())
                .cloned()
                .filter(|a| !a.is_empty())
                .map(|a| (*c, a))
        })
        .collect();
    rep.note(
        "corner config",
        format!(
            "{}；enabled={} 停留={} ms 护栏={} ms 全屏暂停={} 需Ctrl={} 需Shift={} 半径={} DIP；\
             绑了 {} 角：{}",
            crate::settings::with(|s| s.line()),
            cfg.enabled,
            cfg.dwell_ms,
            cfg.repeat_guard_ms,
            cfg.pause_on_fullscreen,
            cfg.require_ctrl,
            cfg.require_shift,
            cfg.trigger_dip,
            bound.len(),
            if bound.is_empty() {
                "(四角都没绑)".to_string()
            } else {
                bound
                    .iter()
                    .map(|(c, a)| format!("{}={}", c.key(), a))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        ),
    );
    let live_ms = i64::from(live_want).clamp(i64::from(MIN_LIVE_MS), MAX_LIVE_MS);

    match crate::capture::monitor_table() {
        Err(e) => rep.row(
            "corner geometry",
            Check::Blocked,
            format!("这台机器答不出显示器（{e}）：角落方块没有几何可算，后面两格没有起点"),
        ),
        Ok(monitors) => {
            let corners = HotCorners::new(&cfg, &monitors);
            let rects: Vec<String> = Corner::ALL
                .iter()
                .map(|c| {
                    let r = corners.rect(*c);
                    format!("{}=[{},{},{}x{}]", c.key(), r.x, r.y, r.w, r.h)
                })
                .collect();
            if monitors.is_empty() {
                rep.row(
                    "corner geometry",
                    Check::Blocked,
                    "显示器清单是空的（这一台没答），角落方块算不出来".to_string(),
                );
            } else {
                // 判的不是「方块非空」那一句同义反复：每一枚方块的**中心**必须落在这一台真
                // 答出来的某一块显示器里。几何算歪了、半径被夹成了整桌面、清单里混进一块
                // 零尺寸的虚拟屏，都会在这一格红。
                let on_screen = |c: Corner| {
                    let r = corners.rect(c);
                    !r.is_empty() && monitors.iter().any(|m| m.bounds.contains(r.center()))
                };
                let ok = Corner::ALL.iter().all(|c| on_screen(*c));
                rep.row(
                    "corner geometry",
                    Check::from(ok),
                    format!("{} 屏；{}", monitors.len(), rects.join(" ")),
                );
            }

            if monitors.is_empty() {
                // 这一格先问几何有没有，再问开关：清单是空的时候「躲开角落」那个点也算不出来，
                // 而 `unwrap_or(&monitors[0])` 会当场把这一趟变成 panic 而不是读数。
                rep.row(
                    "corner sweep",
                    Check::Blocked,
                    "显示器清单是空的（这一台没答）：没有一角可停，也没有一点可躲".to_string(),
                );
            } else if !cfg.enabled {
                rep.row(
                    "corner sweep",
                    Check::Blocked,
                    "[hot_corner] enabled=false：这一场四角都是关的，谁也不该动".to_string(),
                );
            } else if bound.is_empty() {
                rep.row(
                    "corner sweep",
                    Check::Blocked,
                    "四角都没绑动作：没有可触发的东西，这一格不给自己判绿".to_string(),
                );
            } else {
                let mods = Modifiers {
                    ctrl: cfg.require_ctrl,
                    shift: cfg.require_shift,
                    ..Modifiers::default()
                };
                let mut machine = HotCorners::new(&cfg, &monitors);
                let middle = monitors
                    .iter()
                    .find(|m| m.primary)
                    .unwrap_or(&monitors[0])
                    .bounds;
                let away = middle.center();
                let mut fired: Vec<String> = Vec::new();
                let mut unrouted: Vec<String> = Vec::new();
                let mut t: u64 = 0;
                for (corner, _) in &bound {
                    let r = machine.rect(*corner);
                    let p = r.center();
                    machine.pointer(p, mods, t);
                    t += u64::from(cfg.dwell_ms) + 1;
                    if let Some(f) = machine.pointer(p, mods, t) {
                        if crate::hotkey::Work::of(&f.action).is_none() {
                            unrouted.push(said(&f, t));
                        }
                        fired.push(said(&f, t));
                    }
                    // 离开这一角，再把时钟推过护栏：下一枚角落的读数不该由这一枚的护栏解释。
                    t += 1;
                    machine.pointer(away, mods, t);
                    t += u64::from(cfg.repeat_guard_ms) + 1;
                }
                let ok = fired.len() == bound.len() && unrouted.is_empty();
                rep.row(
                    "corner sweep",
                    Check::from(ok),
                    format!(
                        "{} 枚绑了动作，触发 {} 次：{}；配得上却没人接得住 {:?}",
                        bound.len(),
                        fired.len(),
                        fired.join(" "),
                        unrouted
                    ),
                );
            }

            let mut runner = Runner::new(cfg.clone(), monitors.clone());
            let mut samples: usize = 0;
            let mut unreadable: usize = 0;
            let mut visits: Vec<String> = Vec::new();
            let mut live_fired: Vec<String> = Vec::new();
            let mut seen: Option<String> = None;
            let until = crate::delay::clock_ms() + live_ms as u64;
            while crate::delay::clock_ms() < until {
                let now = crate::delay::clock_ms();
                let point = platform_windows::proc::cursor_pos();
                match point {
                    None => unreadable += 1,
                    Some(p) => match runner.corners().corner_at(p) {
                        // 只在换了角落的时候落一行：这一格要的是「这一趟进过哪几个角」，
                        // 不是每 80 ms 一行的转速表。
                        Some(c) => {
                            let key = c.key().to_string();
                            if seen.as_deref() != Some(key.as_str()) {
                                seen = Some(key.clone());
                                visits.push(format!("{key}@{now}"));
                            }
                        }
                        None => seen = None,
                    },
                }
                let foreground = platform_windows::proc::foreground_window();
                let screen = fullscreen_of(
                    foreground,
                    foreground.and_then(platform_windows::proc::window_rect),
                    runner.layout(),
                );
                if let Some(f) = runner.poll(
                    screen,
                    point.map(|p| (p, platform_windows::proc::modifiers_down())),
                    now,
                ) {
                    live_fired.push(said(&f, now));
                }
                samples += 1;
                std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
            }
            // 三岔：一问不答是这台机器的账（Blocked，和遮罩读不到桌面同一族规矩），全答是
            // Pass，而**答着答着不答了**才是这一趟的读数真断了 —— 只有那一格该红。
            let verdict = if samples == 0 || unreadable == samples {
                Check::Blocked
            } else if unreadable > 0 {
                Check::Fail
            } else {
                Check::Pass
            };
            rep.row(
                "corner live",
                verdict,
                format!(
                    "{samples} 格采样（{live_ms} ms，每格 {POLL_MS} ms），读不到指针 {unreadable} 格；\
                     进过 {} 处：{}；这一趟真触发 {} 次 {:?}（本档只观测，派发在产品的采样腿上）",
                    visits.len(),
                    if visits.is_empty() {
                        "(一次也没进角落)".to_string()
                    } else {
                        visits.join(" ")
                    },
                    live_fired.len(),
                    live_fired
                ),
            );
        }
    }
    rep.finish("hotcorner door")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole table, graded headless. `check()` is the same function `--selftest` runs,
    /// so this test and that row read one list rather than two.
    #[test]
    fn the_corner_table_holds_headless() {
        let (verdict, said) = check();
        assert_eq!(verdict, crate::state::Check::Pass, "{said}");
    }

    /// §5.15 lists five actions. A corner that is *configured* to do one of them and then
    /// does nothing is the bug this round exists to close, so the routing is graded on its
    /// own rather than only inside the table.
    #[test]
    fn all_five_corner_actions_have_a_hand() {
        for name in HOT_CORNER_ACTIONS {
            let work = crate::hotkey::Work::of(name);
            assert!(work.is_some(), "角落配得上 {name}，却没有一只 Work 接得住");
        }
        assert_eq!(HOT_CORNER_ACTIONS.len(), 5);
    }

    /// The one reading that separates a fullscreen game from a maximised window. Getting
    /// this backwards would pause the corners for the entire working life of the app.
    #[test]
    fn maximised_is_not_fullscreen() {
        let monitors = one_monitor();
        assert!(covers_a_monitor_exactly(
            &PhysRect::new(0, 0, 1_000, 800),
            &monitors
        ));
        assert!(!covers_a_monitor_exactly(
            &PhysRect::new(-8, -8, 1_016, 816),
            &monitors
        ));
        assert_eq!(
            fullscreen_of(Some(7), None, &monitors),
            Fullscreen::Unknown,
            "读不到矩形不是「没在 fullscreen」"
        );
    }

    /// The user's switch outranks the machine's answer, in both directions.
    #[test]
    fn the_pause_is_a_setting_not_a_guess() {
        assert!(!should_pause(false, Fullscreen::Yes));
        assert!(should_pause(true, Fullscreen::Yes));
        assert!(!should_pause(true, Fullscreen::No));
        assert!(!should_pause(true, Fullscreen::Unknown));
    }

    /// An unreadable pointer must not leave a dwell running on stale coordinates, and the
    /// clock is what proves it: the same firing at 300 ms would mean the stale reading won.
    #[test]
    fn an_unreadable_pointer_restarts_the_dwell() {
        let monitors = one_monitor();
        let mut corners = HotCorners::new(&gauge_cfg(), &monitors);
        assert!(step(&mut corners, sample_at(Some(BR), false), 0).is_none());
        assert!(step(&mut corners, None, 250).is_none());
        assert!(
            step(&mut corners, sample_at(Some(BR), false), 300).is_none(),
            "读不到指针之后，300 ms 那一刻不该按旧坐标算够停留"
        );
        let f = step(&mut corners, sample_at(Some(BR), false), 600);
        assert_eq!(
            f.map(|f| said(&f, 600)),
            Some("bottom_right=capture@600".to_string())
        );
    }

    /// Defaults: the feature is switched off until the user switches it on, and the
    /// fullscreen pause is on by default because §5.15 names误操作 as the thing to avoid.
    #[test]
    fn the_defaults_are_off_but_guarded() {
        let d = HotCorner::default();
        assert!(!d.enabled, "热区不该默认开着");
        assert!(d.pause_on_fullscreen);
        assert_eq!(d.dwell_ms, 300);
        assert_eq!(d.trigger_dip, 8);
        assert_eq!(
            d.corners.get("bottom_right").map(String::as_str),
            Some("capture")
        );
    }
}
