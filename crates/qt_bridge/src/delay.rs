//! §5.2.4 延时截图：按下开始之后**先数完秒数**，再进入截图状态。
//!
//! 这枚模块在 P108 之前不存在，而它要补的不是「一个还没写的小功能」，是一枚**已经在档里、
//! 也被整段快照读者 clone 走、却没有任何行为读者**的键：`capture.delay_seconds`
//! （`crates/core/src/config.rs:178` 字段、`:196` 默认 `0`、`:829-830` 解析表）。P65 给整段
//! `[capture]` 建了快照读者 [`crate::settings::capture_cfg`]，于是这枚键第一次被**读出来**了；
//! 但读出来之后没人拿它决定任何事，所以用户把它写成 5 和写成 0 是同一个程序。这一格把那半条
//! 链闭上：[`arm_from_config`] 是它的**第一位行为读者**，它的返回值直接决定冻结发生在哪一刻。
//! 芯子那一侧早就在等这枚键了 —— `crates/core/src/capture.rs:336` 给 `capture_rect` 写的
//! 用途清单里就列着 “delayed capture”，而那条用途到今天为止没有调用者。
//!
//! ## 为什么这一臂必须长在 `capture::freeze()` 之前
//!
//! PRD 第 420 行「延时截图应捕获**倒计时结束时**的指针状态」是一句时刻的断言，而 P107 刚把
//! 指针那一问挪到冻结那一格（[`crate::capture::freeze`] 里 `cursor_overlay()` 那一行）。两条
//! 合起来的本轮结论是：**延时不需要为指针写任何新代码** —— 只要 `freeze()` 发生在倒计时结束
//! 之后，指针读数就自动是结束那一刻的。反过来，任何「先冻好画面、再摆倒计时」的实现都会当场
//! 违第 420 行：画面和指针都停在按下开始那一刻，用户按 §5.2.4「适用场景」里那三条（下拉菜单、
//! 悬停状态、需要提前展开的界面）就没法做。
//!
//! 于是 [`Countdown::tick`] 的返回值形态就是那条不变量本身：**`Go` 只在第 N 枚 tick 上交回**，
//! 前 N-1 枚一律 `Waiting`。单测证的正是这个，而 [`Countdown::held_ms`] 把「倒计时真的数完了」
//! 做成一个可数的毫秒数，好让门在运行期读到数而不是读到承诺。
//!
//! ## P108 那一轮没做的半，P109 补上了
//!
//! §5.2.4 步骤 4「系统显示倒计时」要的是一块看得见的表面。P108 只把倒计时做到了 `Session` 的
//! 两枚 qproperty 上（`delay_left` / `delay_step`，由 `main.qml` 那枚 `Timer` 消耗）—— 于是
//! 「还剩几秒」自此是**程序知道、并且正在据此行动**的数，但屏幕上还没有那一枚数字。P109 补的
//! 就是那一半：[`hint_text`] 与 [`hint_sequence`] 两枚纯函数把「left ⇒ 屏幕上写不写话」这枚分岔
//! 挪出了 QML，`Session::refresh_delay` 与 `Session::delay_round` 各在自己的那一臂上写 status；
//! `HintWindow.qml` 里那条归零收窗的 handler 长在 `delay_left <= 0` 那一支上，把已经
//! 写出去的那句收回 —— 而这一支必须**赶在 `capture::freeze()` 之前**走完，否则屏幕上那一枚数字
//! 自己会进图：[`crate::capture`] 头顶那条「画面到手之前不存在任何我们的窗」的规则就是同一条禁令
//! 的另一种写法。cxx-qt 的 qproperty setter 是同线程 direct connection，所以「Rust 里前一行叫
//! `set_delay_left(0)`、下一行才叫 `open_mask`」这两行之间就是这条同步语义；P109 的运行期证人
//! 在门的第 30 格，判的是「三句话按 3→2→1 依次出、第四枚 `tick` 归零之后交回 `None`」这条完整
//! 显示链 —— 也就是屏幕上真的会出现那三句话，并且**归零之后不再写**。
//!
//! ## P119：这条 handler 的名字，P109 起写错到 P118 为止
//!
//! 上面那句「`HintWindow.qml` 里那条 handler」在 P109〜P118 十轮里指的是
//! `function onDelayLeftChanged()`，而 cxx-qt 为 `#[qproperty(i32, delay_left)]` 生成的信号名是
//! **`delay_leftChanged`**（不做 camelCase 转换）。QML 的 `Connections` 只把 `on` 之后首字母归一
//! 化，于是 `onDelayLeftChanged` 剥掉 `on` 得到 `delayLeftChanged` —— 场上没有那一枚信号，这条
//! handler 十轮里一次都没跑过，运行时只在 stderr 留一行 `warn:`，不报错、不红。P119 把它改成绑
//! 一枚 QML 自己声明的值属性（`readonly property bool counting: hint.session.delay_left > 0`，
//! notify 由 Qt 生成，名字与 `onCountingChanged` 对得上），语义等价而机制是本项目有运行期证人的
//! 那一种。守这一格的源码扫描在 [`crate::hint_check`] 的
//! `a_camel_cased_session_notify_never_hangs_on_a_window`。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

/// 一秒一枚数，是 §5.2.4 步骤 4「显示倒计时」里那个数的单位。写死而不是做成配置：PRD 里没有任何
/// 一句支持「每半秒报一个数」，而把步长做成第二枚键，会在显示面还没有落地的这一轮先造出一枚死键。
pub const STEP_MS: u64 = 1000;

/// 一枚倒计时：还剩哪几个数没数、每个数之间隔多久、以及它是从哪一刻开始等的。
///
/// `now_ms` 由调用方注入而不是自己读表：这条链上唯一要证的判断是**次序**，而把表交出去之后，
/// 「第 3 枚 tick 才交回 `Go`」和「等满了 3000 ms」能在同一枚单测里当场算出来 —— 不等真实
/// 时间、不开窗。生产路径传的锚是 [`clock_ms`]，即进程自身的单调毫秒数。
#[derive(Clone, Debug)]
pub struct Countdown {
    ticks: Vec<u32>,
    step_ms: u64,
    fired: usize,
    armed_ms: u64,
}

impl Countdown {
    fn new(ticks: Vec<u32>, step_ms: u64, armed_ms: u64) -> Self {
        Countdown {
            ticks,
            step_ms,
            fired: 0,
            armed_ms,
        }
    }

    /// 还剩几枚数没数。0 就是数完了 —— `main.qml` 那枚 `Timer` 的 `running` 绑的正是这枚数，
    /// 所以它归零的同一刻定时器自己停，不需要谁去 `stop()`。
    pub fn left(&self) -> u32 {
        self.ticks.len().saturating_sub(self.fired) as u32
    }

    /// 每个数之间隔多久，直接交给 `Timer` 的 `interval`。
    pub fn step_ms(&self) -> u64 {
        self.step_ms
    }

    /// 数一枚。**这是整枚模块唯一决定「冻结发生在哪一刻」的地方。**
    ///
    /// 前 N-1 枚交回 [`Tick::Waiting`]，第 N 枚交回 [`Tick::Go`]；一枚已经走满的倒计时再被叫到，
    /// 交回 [`Tick::Spent`] 而不是第二个 `Go` —— 一次按下只能冻一张画面。
    pub fn tick(&mut self, now_ms: u64) -> Tick {
        if self.fired >= self.ticks.len() {
            return Tick::Spent;
        }
        self.fired += 1;
        let left = self.left();
        if left > 0 {
            Tick::Waiting { left }
        } else {
            Tick::Go {
                held_ms: self.held_ms(now_ms),
                planned_ms: self.planned_ms(),
            }
        }
    }

    /// 从「按下开始」到「倒计时结束」实际等了多久。门读的就是这一枚数：它不小于 [`Countdown::planned_ms`]，
    /// 才谈得上「画面冻在倒计时结束那一刻」，也就是 PRD 第 420 行那条时刻的实测部分。
    ///
    /// `saturating_sub` 不是装饰：表是调用方给的，一枚倒退的表读成 0，不会读成一枚巨大的数去把
    /// 判据蒙过去。
    pub fn held_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.armed_ms)
    }

    /// 请求的等待时长：数了几枚就是几个 `step_ms`。与 [`Countdown::held_ms`] 是一对 —— 一枚是
    /// 计划、一枚是实测，两者在同一枚 `Tick::Go` 里一起交回，这条链才是量到的而不是写对的。
    pub fn planned_ms(&self) -> u64 {
        (self.ticks.len() as u64).saturating_mul(self.step_ms)
    }
}

/// [`Countdown::tick`] 的答案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// 还在数：还剩 `left` 枚。**这一格绝不能冻结。**
    Waiting { left: u32 },
    /// 数完了：这一刻就是 §5.2.4 步骤 5 的「进入截图状态」，也是 §5.4.3 第 420 行的「倒计时结束」。
    /// 两枚毫秒一起交回，好让记账不需要再回头看别的状态。
    Go { held_ms: u64, planned_ms: u64 },
    /// 一枚已经走满的倒计时又被叫了一次。没有第二次冻结。
    Spent,
}

/// [`arm_from_config`] 的答案：这一次按下是立刻冻，还是先数秒。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    Immediate,
    Waiting { left: u32, step_ms: u64 },
}

/// 秒数换算成倒计时的那几枚数。`0` 交回空表 —— 默认那一侧（`crates/core/src/config.rs:196`）
/// 就是空表，所以没写过这枚键的用户走的仍是立刻冻结那条老路。
///
/// 倒序不是排版口味：屏幕上要显示的是「还剩几秒」，而 [`Countdown::left`] 从长度往下减，
/// 正着排会让第一枚数报成 1。
pub fn countdown(seconds: u32) -> Vec<u32> {
    (1..=seconds).rev().collect()
}

/// 进程自己的单调毫秒锚。`Countdown` 不等它：它等调用方把表递给它的这一刻。
fn clock() -> &'static std::time::Instant {
    static CLOCK: OnceLock<std::time::Instant> = OnceLock::new();
    CLOCK.get_or_init(std::time::Instant::now)
}

pub fn clock_ms() -> u64 {
    clock().elapsed().as_millis() as u64
}

static RUNNING: OnceLock<Mutex<Option<Countdown>>> = OnceLock::new();

/// 数过的 tick 总数、走满的倒计时总数、最近一场的「实测等了 | 计划要等」。
///
/// 三枚都是进程级 `AtomicU64`，和 [`crate::capture`] 里那两枚计数器（`FREEZE_COUNT` /
/// `CURSOR_READS`）同一族：门只能读到数、读不到像素，所以这条链的运行期证人只能是次序与时长，
/// 而这三枚恰好就是那条链的次序与时长。
static TICKS: AtomicU64 = AtomicU64::new(0);
static HOLED: AtomicU64 = AtomicU64::new(0);
static LAST_HELD_MS: AtomicU64 = AtomicU64::new(0);
static LAST_PLANNED_MS: AtomicU64 = AtomicU64::new(0);

fn slot() -> &'static Mutex<Option<Countdown>> {
    RUNNING.get_or_init(|| Mutex::new(None))
}

fn with<R>(f: impl FnOnce(&mut Option<Countdown>) -> R) -> R {
    let mut guard = slot().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// §5.2.4 步骤 3「用户点击开始」的产品入口：**`capture.delay_seconds` 的第一位行为读者**。
///
/// 它从 [`crate::settings::capture_cfg`] 那枚整段快照里取这一个字段，然后把结果交成一次分岔：
/// `0` ⇒ [`Arm::Immediate`]（与 P108 之前的行为逐字节相同）；`>0` ⇒ [`Arm::Waiting`]，并把
/// 倒计时挂上槽。调用方 —— [`crate::hotkey`] 的 `Work::Capture` 那一格 —— 据此决定这一刻冻不冻
/// 画面。这就是「行为读者」与「只是 clone 走」的分别：键的值第一次能让程序走两条不同的路了。
pub fn arm_from_config() -> Arm {
    arm_at(crate::settings::capture_cfg().delay_seconds, clock_ms())
}

/// [`arm_from_config`] 去掉表的那一半：单测从这里进，`--autosave` 那档的第 29 格也从这里进
/// （那一格要量的是「三秒的倒计时真的等满了三秒」，而它不能靠改用户档里的键来起手）。
pub(crate) fn arm_at(seconds: u32, now_ms: u64) -> Arm {
    let ticks = countdown(seconds);
    if ticks.is_empty() {
        // 一枚挂着的倒计时也要顺手作废：用户能在两场之间把这枚键改回 0（`write_back`），而
        // 「立刻拍」若排在前一场还没数完的时候，留着它就是留着一个还会自己开遮罩的后台。
        with(|r| *r = None);
        return Arm::Immediate;
    }
    let left = ticks.len() as u32;
    with(|r| *r = Some(Countdown::new(ticks, STEP_MS, now_ms)));
    Arm::Waiting {
        left,
        step_ms: STEP_MS,
    }
}

/// 数一枚，并把走满的那一场记到账上。`None` = 这一刻槽里没东西，也就是没有任何倒计时在等。
///
/// `Go` 之后立刻清空槽：这枚倒计时已经交回了它唯一要交的答案，留着它只会让下一枚 `tick()`
/// 交出第二次冻结（结构体层面的那种回答是 [`Tick::Spent`]，但产品路径不该依赖有人记得它）。
pub fn tick() -> Option<Tick> {
    let out = with(|r| {
        let t = match r.as_mut() {
            Some(c) => c.tick(clock_ms()),
            None => return None,
        };
        if matches!(t, Tick::Go { .. }) {
            *r = None;
        }
        Some(t)
    });
    match out {
        // `Spent` 也记进 ticks：三者的共同点是「这一刻确实有一枚挂着的倒计时被叫到了」，
        // 而门要的正是「叫了几回」这一枚数。
        Some(Tick::Waiting { .. } | Tick::Spent) => {
            TICKS.fetch_add(1, Ordering::Relaxed);
        }
        Some(Tick::Go {
            held_ms,
            planned_ms,
        }) => {
            TICKS.fetch_add(1, Ordering::Relaxed);
            HOLED.fetch_add(1, Ordering::Relaxed);
            LAST_HELD_MS.store(held_ms, Ordering::Relaxed);
            LAST_PLANNED_MS.store(planned_ms, Ordering::Relaxed);
        }
        None => {}
    }
    out
}

/// 还剩几枚数（没挂着倒计时就是 0）。`Session.delay_left` 这枚 qproperty 读的就是这里，而它绑的是
/// **值**不是调用，所以 `Timer` 的 `running` 会在归零那一刻重新求值 —— 这条口径是 P37 那枚
/// 「invokable 只被绑定调用一次」的教训抄来的，写在 `main.qml` 的 `warmRounds` 头上。
pub fn left() -> u32 {
    with(|r| r.as_ref().map_or(0, |c| c.left()))
}

/// 下一枚数多久之后来。`Session.delay_step` 读这里，`Timer.interval` 绑它。
pub fn step_ms() -> u64 {
    with(|r| r.as_ref().map_or(STEP_MS, |c| c.step_ms()))
}

/// 还挂着的这一场计划等多久（没挂着就是 0）。`--autosave` 第 29 格在数第一枚之前读的就是这一枚，
/// 它和 [`last_hold`] 是一对：一枚是挂上时说好的，一枚是数满时量到的。
pub fn planned_ms() -> u64 {
    with(|r| r.as_ref().map_or(0, |c| c.planned_ms()))
}

/// 数过的 tick 总数与走满的倒计时总数。第 29 格判的是**差**（本格前后各读一次），不是绝对值 ——
/// 门可能在一场里被别的格叫到，只有差是这一格自己的读数。
pub fn counts() -> (u64, u64) {
    (TICKS.load(Ordering::Relaxed), HOLED.load(Ordering::Relaxed))
}

/// 最近一场走满的倒计时：`实测 | 计划`。门判的是 `实测 >= 计划`，也就是 PRD 第 420 行那句
/// 「倒计时结束时」里「结束」二字的实测部分。
pub fn last_hold() -> (u64, u64) {
    (
        LAST_HELD_MS.load(Ordering::Relaxed),
        LAST_PLANNED_MS.load(Ordering::Relaxed),
    )
}

/// §5.2.4 步骤 4「系统显示倒计时」的那一枚屏幕上要出现的字。P108 把倒计时接成了行为读者
/// （Session 两枚 qproperty + `main.qml` 那枚 `Timer`），但屏幕上还没有那一枚数字 —— P109 补的就是
/// 这一格。这枚函数是那一格里**唯一决定"写不写 status"的分岔**：`left > 0` ⇒ `Some("还剩 N 秒")`
/// 交回给 [`crate::session::SessionRust::delay_round`] 的 `Waiting` 那一臂去 `set_status`；
/// `left == 0` ⇒ `None`，也就是「这一刻不许写话、把已经写出去的那句收回来」—— 而收回的动作
/// 是 QML 那一侧绑在 `delayLeftChanged` 上的一条 handler 干的（`hint.shown = false` ⇒ `hide()`）。
///
/// 之所以返回 `Option<String>` 而不是 `String`：**空字符串也是一次写、也是一次 notify**，QML
/// 那一侧的 `onStatusChanged` 会当场把 `hint.shown` 又设回 true。所以「收回去」这件事**不能**
/// 走 status 那条腿；只能走 `delay_left == 0` 那一条。这一枚 `None` 就是那条腿在 Rust 侧的对应物。
pub fn hint_text(left: u32) -> Option<String> {
    if left == 0 {
        None
    } else {
        Some(format!("还剩 {left} 秒"))
    }
}

/// 一枚倒计时从挂上到走满，屏幕上应该依次出现的那几句话。空表 ⇒ 空列表（立刻拍那一条老路
/// 一格话也不写）；`seconds = 3` ⇒ `["还剩 3 秒", "还剩 2 秒", "还剩 1 秒"]`，然后归零 ⇒ 收回。
///
/// **这枚函数不改动任何状态**：[`arm_at`] 与 [`tick`] 才是状态机，这里只是把它们的读数换算成
/// 屏幕上的话。门的第 30 格读的就是这条换算，因为「倒计时有没有真的数出那三句话」在 P109 里
/// 和「倒计时有没有真的等满」是同一条链的两半，而前者是 P108 没做的那半。
pub fn hint_sequence(seconds: u32) -> Vec<String> {
    countdown(seconds)
        .into_iter()
        .filter_map(hint_text)
        .collect()
}

/// 碰槽、碰账的测试共用这一枚锁。
///
/// `cargo test` 在同一枚进程里多线程跑测试，而这个模块的状态是**进程级**的 —— 不是产品的问题
/// （产品只有一场倒计时在跑），是测试的：两枚各挂各倒计数的测试会读到对方挂上的那一枚，
/// 于是一枚「永远绿」的测试其实是在赌调度。这里不让那种赌发生。
#[cfg(test)]
fn global_gate() -> std::sync::MutexGuard<'static, ()> {
    static GATE: Mutex<()> = Mutex::new(());
    GATE.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认那一侧：`delay_seconds = 0` ⇒ 一枚数也不数，当场就拍。P108 之前的行为必须在这一枚
    /// 测试里活下来，否则「建了延时那一臂」就等于改了所有没写过这枚键的用户的热键路径。
    #[test]
    fn zero_seconds_shoots_immediately() {
        let _g = global_gate();
        assert!(countdown(0).is_empty());
        assert_eq!(arm_at(0, 5_000), Arm::Immediate);
        assert_eq!(left(), 0);
        assert_eq!(tick(), None);
    }

    /// §5.2.4 步骤 4：三秒就是三枚数，从 3 数到 1。
    #[test]
    fn three_seconds_is_three_ticks_counting_down() {
        let _g = global_gate();
        assert_eq!(countdown(3), vec![3, 2, 1]);
        assert_eq!(
            arm_at(3, 0),
            Arm::Waiting {
                left: 3,
                step_ms: STEP_MS
            }
        );
        assert_eq!(left(), 3);
        assert_eq!(step_ms(), STEP_MS);
        assert_eq!(planned_ms(), 3 * STEP_MS);
        // 收尾走产品那条路：把这枚键改回 0 就是作废（§5.2.4 没有「取消」这一步，所以也不为该
        // 它写一枚没人调的 API）。这一枚测试不判它，判它的是 `arming_zero_clears_a_pending_countdown`。
        assert_eq!(arm_at(0, 0), Arm::Immediate);
        assert_eq!(left(), 0);
    }

    /// 整枚模块的那条不变量，也是 PRD 第 420 行的全部可证内容：**第 N 枚之前交不回 `Go`**。
    /// 前 N-1 枚一旦能交回 `Go`，画面就冻在倒计时中途，而指针读数自 P107 起长在冻结那一格，
    /// 第 420 行当场落空 —— 所以这一枚测试数的不是文案，是时刻。
    #[test]
    fn freeze_is_released_only_by_the_last_tick() {
        let mut c = Countdown::new(countdown(3), STEP_MS, 0);
        assert_eq!(
            c.tick(1_000),
            Tick::Waiting { left: 2 },
            "第一枚数完之后还剩两枚，此刻绝不能冻"
        );
        assert_eq!(c.tick(2_000), Tick::Waiting { left: 1 });
        assert_eq!(
            c.tick(3_000),
            Tick::Go {
                held_ms: 3_000,
                planned_ms: 3_000
            },
            "第三枚才是倒计时结束，实测与计划都是 3000 ms"
        );
        assert_eq!(c.left(), 0);
    }

    /// 一枚走满的倒计时不该交出第二次 `Go`：一次按下只能冻一张画面。这条不是防御性的 ——
    /// `Timer` 的 `running` 与 `delayRound()` 之间隔着一帧，多一次触发是一种真实的可能。
    #[test]
    fn a_finished_countdown_never_fires_twice() {
        let mut c = Countdown::new(countdown(1), STEP_MS, 100);
        assert_eq!(
            c.tick(1_100),
            Tick::Go {
                held_ms: 1_000,
                planned_ms: 1_000
            }
        );
        assert_eq!(c.tick(2_100), Tick::Spent);
        assert_eq!(c.tick(3_100), Tick::Spent);
    }

    /// 实测与计划在同一枚 `Go` 里一起交回；等少了当场读出来，而不是「大概三秒」。
    #[test]
    fn held_ms_is_measured_from_the_armed_anchor() {
        let mut c = Countdown::new(countdown(2), STEP_MS, 4_000);
        assert_eq!(c.planned_ms(), 2_000);
        assert_eq!(c.tick(4_500), Tick::Waiting { left: 1 });
        assert_eq!(
            c.tick(5_000),
            Tick::Go {
                held_ms: 1_000,
                planned_ms: 2_000
            },
            "按下开始到倒计时结束实测 1000 ms，而计划要 2000 ms —— 门读到的必须是这对差而不是平均值"
        );
        let early = Countdown::new(countdown(2), STEP_MS, 9_000);
        assert_eq!(
            early.held_ms(8_000),
            0,
            "时钟倒退读成 0，不读成一枚巨大的数"
        );
    }

    /// `Go` 之后槽里必须不留东西：下一枚 `tick()` 交回 `None`，而不是第二次冻结。
    #[test]
    fn a_completed_hold_leaves_nothing_armed() {
        let _g = global_gate();
        assert_eq!(
            arm_at(1, clock_ms()),
            Arm::Waiting {
                left: 1,
                step_ms: STEP_MS
            }
        );
        assert!(matches!(tick(), Some(Tick::Go { .. })));
        assert_eq!(left(), 0);
        assert_eq!(planned_ms(), 0);
        assert_eq!(tick(), None);
    }

    /// 记账：一枚还没数完的 tick 只加 ticks，一枚走满的才加 holed，并把这一场的两枚毫秒留下。
    /// 门读的就是这三件事，所以它们的语义必须是「数过几回 / 走满几场 / 最近那场等了多久」，
    /// 而不是「这轮测试跑了几次」。
    #[test]
    fn only_a_completed_hold_is_accounted() {
        let _g = global_gate();
        assert_eq!(arm_at(0, 0), Arm::Immediate);
        let (t0, h0) = counts();
        assert_eq!(
            arm_at(2, clock_ms()),
            Arm::Waiting {
                left: 2,
                step_ms: STEP_MS
            }
        );
        assert_eq!(tick(), Some(Tick::Waiting { left: 1 }));
        let (t1, h1) = counts();
        assert_eq!((t1, h1), (t0 + 1, h0), "没数完的一枚不该算作走满");
        let out = tick().expect("第二枚 tick 该数完这场");
        let held = match out {
            Tick::Go { held_ms, .. } => held_ms,
            other => panic!("预期 Go，交回 {other:?}"),
        };
        let (t2, h2) = counts();
        assert_eq!((t2, h2), (t1 + 1, h1 + 1));
        let (last_held, last_planned) = last_hold();
        assert_eq!(
            (last_held, last_planned),
            (held, 2 * STEP_MS),
            "账上那两枚必须是这一场的实测与计划"
        );
        assert!(
            last_held < last_planned,
            "单测不真的等：两枚 tick 背靠背跑完，实测必须明显小于计划。这一枚断言防的是\
             「把计划抄成实测」那种假记账：{last_held} vs {last_planned}"
        );
    }

    /// 挂着的倒计时被作废之后：不能再数、计划时长归零。作废走的就是产品那条路（把这枚键改回 0），
    /// 而不是另一枚只给测试用的 API —— §5.2.4 没有一步写「按下开始之后可以取消」，为它另开一枚
    /// 入口就是造一枚死键的姊妹：一枚死函数。
    #[test]
    fn voiding_a_pending_countdown_leaves_nothing_to_fire() {
        let _g = global_gate();
        assert_eq!(
            arm_at(4, 0),
            Arm::Waiting {
                left: 4,
                step_ms: STEP_MS
            }
        );
        assert_eq!(tick(), Some(Tick::Waiting { left: 3 }));
        assert_eq!(planned_ms(), 4 * STEP_MS, "作废之前，计划时长挂得实实的");
        assert_eq!(arm_at(0, 0), Arm::Immediate);
        assert_eq!(left(), 0);
        assert_eq!(planned_ms(), 0);
        assert_eq!(tick(), None, "作废之后再数，什么都不该发生");
    }

    /// 把键改回 0 必须顺手作废上一场没数完的倒计时，否则留着它就是一个还会自己开遮罩的后台。
    #[test]
    fn arming_zero_clears_a_pending_countdown() {
        let _g = global_gate();
        assert_eq!(
            arm_at(3, 0),
            Arm::Waiting {
                left: 3,
                step_ms: STEP_MS
            }
        );
        assert_eq!(arm_at(0, 1_000), Arm::Immediate);
        assert_eq!(left(), 0);
        assert_eq!(tick(), None);
    }

    /// §5.2.4 步骤 4 的显示分岔：一枚数字要不要上屏，由这一枚纯函数决定。判据只有一件：
    /// `left > 0` 交回 `Some` 且串里带的正是「还剩 N 秒」；`left == 0` 交回 `None`。
    ///
    /// 为什么 `None` 而不是空串：空串是一次 `set_status` 写入，QML 那侧的 `onStatusChanged`
    /// 会当场把 `hint.shown = true` 再设回来 ⇒ 屏幕上一格也没少。`None` 才是那一格"不写"的
    /// 唯一表示，而收回已经在屏上的那一句走的是另一条腿（`delay_left == 0` ⇒ QML handler ⇒
    /// `hint.shown = false` ⇒ `hide()`）。
    #[test]
    fn hint_text_shows_nonzero_and_goes_silent_at_zero() {
        assert_eq!(hint_text(0), None);
        assert_eq!(hint_text(1), Some("还剩 1 秒".to_string()));
        assert_eq!(hint_text(2), Some("还剩 2 秒".to_string()));
        assert_eq!(hint_text(3), Some("还剩 3 秒".to_string()));
        assert_eq!(hint_text(10), Some("还剩 10 秒".to_string()));
        assert_eq!(hint_text(99), Some("还剩 99 秒".to_string()));
    }

    /// 三秒倒计时的完整显示链：屏幕依次拿到「还剩 3 秒 / 还剩 2 秒 / 还剩 1 秒」三句话，
    /// 第四枚 tick 是 `Go` ⇒ 归零 ⇒ 收回。零秒那一条老路（P108 之前唯一的用户路径）交回空表 ⇒
    /// 屏幕上一格话也没写过。
    ///
    /// 这条链的**顺序**不是口味：`countdown(3) = [3, 2, 1]`（倒序）与 `Countdown::left()` 从长度
    /// 往下减，两条一起决定"第一枚数报 3、最后一枚报 1"；改成 `[1, 2, 3]` 会让屏幕上出现「还剩
    /// 1 秒 / 还剩 2 秒 / 还剩 3 秒」，那就是往回数、PRD 里没有任何一句支持这么读。
    #[test]
    fn three_second_display_counts_down_from_three_to_one() {
        assert_eq!(
            hint_sequence(3),
            vec![
                "还剩 3 秒".to_string(),
                "还剩 2 秒".to_string(),
                "还剩 1 秒".to_string(),
            ]
        );
        assert_eq!(
            hint_sequence(1),
            vec!["还剩 1 秒".to_string()],
            "一秒倒计时只报一句『还剩 1 秒』，然后 Go 归零 ⇒ 收回"
        );
        assert!(
            hint_sequence(0).is_empty(),
            "`delay_seconds`=0 是 P108 之前唯一的用户路径；那一臂屏幕上从来就没写过话"
        );
    }
}
