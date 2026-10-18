//! The process's copy of the settings file (§6.1) - who reads it, who gets asked
//! for a value, and who writes it back.
//!
//! Before this module the file had no owner at all: `Config::load` and `Config::save`
//! existed and were tested against strings, and nothing in the product ever called
//! them. That left §9.2's "restart" promise unkept in the only place it can be kept -
//! the process that starts up after the one that remembered.
//!
//! The three touch points are the three that cost something:
//! * **start-up** reads the file (PRD §5.1.1: a damaged config never blocks startup);
//! * **opening the mask** hands the remembered pens to the layer about to draw
//!   (§5.7.1), which is on the hot path §9.2 caps at 400 ms;
//! * **ending a flow** writes what the layer learned back out.
//!
//! Saving happens at the end of a flow rather than at process exit on purpose: an
//! exit handler does not run when the user signs out or the process is killed, and
//! "the pen I just set" is exactly the thing that has to survive that.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use falcon_core::config::{config_path, Config, ImportChange, ImportScope, LoadSource, Output};
use falcon_core::encode::EncodeOptions;
use falcon_core::imageops::Decoration;
use falcon_core::naming::Collision;

use crate::annotate::Layer;

/// Where the gauges put the file they are measured against.
///
/// This is a measurement hook, not a setting the product promises: reading the real
/// path would mean the first `--mask` run of the day touches
/// `%APPDATA%\ai-falconshot\config.toml`, and a gauge that writes at flow end would
/// then edit it. Nothing reads this unless it was set.
pub const ENV_OVERRIDE: &str = "FALCONSHOT_CONFIG";

/// The file to read, and the pin on where writes may go.
fn boot() -> (PathBuf, Option<PathBuf>) {
    let pinned = std::env::var_os(ENV_OVERRIDE)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let read = pinned
        .clone()
        .unwrap_or_else(|| config_path(&Config::default()));
    (read, pinned)
}

/// Everything the process knows about its settings, and what each touch point cost.
#[derive(Default)]
pub struct Settings {
    pub config: Config,
    /// Where this copy was read from.
    pub read_from: PathBuf,
    /// Where writes go. Not always `read_from`: the file itself can say where its
    /// data lives (§6.1's `advanced.data_dir`).
    pub write_to: PathBuf,
    pub source: LoadSource,
    pub warnings: Vec<String>,
    pub backup: Option<PathBuf>,
    /// Writes the machine refused. The values stay in memory either way, which is
    /// §9.4's rule for a failed save read for the settings file.
    pub problems: Vec<String>,
    pub load_ms: u64,
    pub applies: u32,
    pub apply_ms: u64,
    pub apply_max_ms: u64,
    pub saves: u32,
    pub save_ms: u64,
    pub save_max_ms: u64,
    /// §5.5.8 那句"用户可选择覆盖原图或生成新图"里 `reuse_last_file` **跨 mask 会话**那半的载体
    /// （第 63 项 ⑦ / PRD §5.5.8）。上一次 mask 会话里 [`crate::mask::MaskState::quick_out`]
    /// 落盘成功的路径存在这里，下一次 mask 会话的 `open()` 在 `reuse_last_file` 关着时
    /// 忽略它、开着时把它抄进 `MaskState::landed` —— 于是同一次进程里第二次按 quick-save
    /// 覆写的是**上一段流程**留下的那一份文件，而不是新起一份 `quick-001`。
    /// `Default` 是 `None`：跨进程重启不留（PRD 只说"覆盖原图"，没说重启后仍覆盖，本轮
    /// 就照最近的这一格），也不写进 `config.toml`（内部状态不占用户档里的行）。
    pub last_landed: Option<PathBuf>,
}

impl Settings {
    /// Read `path`, and write to `pinned` when the caller pinned one - the gauges do -
    /// or to wherever the file's own `advanced.data_dir` points otherwise.
    pub fn open(path: &Path, pinned: Option<&Path>) -> Settings {
        let started = Instant::now();
        let (config, report) = Config::load(path);
        let load_ms = started.elapsed().as_millis() as u64;
        let write_to = match pinned {
            Some(p) => p.to_path_buf(),
            None => config_path(&config),
        };
        Settings {
            config,
            read_from: path.to_path_buf(),
            write_to,
            source: report.source,
            warnings: report.warnings,
            backup: report.backup,
            load_ms,
            ..Settings::default()
        }
    }

    fn tag(&self) -> &'static str {
        match self.source {
            LoadSource::Defaults => "defaults",
            LoadSource::File => "file",
            LoadSource::Recovered => "recovered",
        }
    }

    /// The pens the file remembered, into the layer about to draw with them. Returns
    /// how many tools the file spoke about: `0` is a first run, and a restart test
    /// that reads back `0` proved nothing.
    ///
    /// `[annotation]` goes in first and `[annotation.tool_style]` after it, which is the
    /// order §5.7.1's fallback is written in: the scalars are the pen a tool starts with
    /// and a taught tool overrides them. Reversed, a per-tool entry would be overwritten
    /// by the default row on every start and the memory would not survive one restart.
    pub fn apply_to(&mut self, layer: &mut Layer) -> usize {
        let started = Instant::now();
        let pens = self.config.annotation.tool_style.clone();
        layer.apply_annotation_defaults(&self.config.annotation);
        layer.apply_tool_styles(&pens);
        let ms = started.elapsed().as_millis() as u64;
        self.applies += 1;
        self.apply_ms = ms;
        self.apply_max_ms = self.apply_max_ms.max(ms);
        pens.len()
    }

    /// §5.5.5's two export knobs, in the shape the encoder takes them.
    ///
    /// Read at the mask's `open`, next to the pens, for the reason the pens are read
    /// there: the file is touched before the user has a picture to save, so 另存为 is
    /// never the first thing to cost a load. `quality` is `u32` in the file and `u8`
    /// to the encoder, and the file's own sanitising already keeps it inside 1..=100 -
    /// clamped again here because a `Config` built in memory is not sanitised, and a
    /// silently truncated `200` would be a JPG written at quality 200's low cousin.
    pub fn export_opts(&self) -> EncodeOptions {
        EncodeOptions {
            format: self.config.output.format,
            quality: self.config.output.jpeg_quality.clamp(1, 100) as u8,
            ..Default::default()
        }
    }

    /// What the layer learned, back into the config. `None` when nothing moved, and
    /// nothing is written then: a screenshot that never touched a pen must not
    /// rewrite the file.
    pub fn write_back(&mut self, layer: &Layer) -> Option<u64> {
        let learned = layer.tool_styles();
        if learned == self.config.annotation.tool_style {
            return None;
        }
        self.config.annotation.tool_style = learned;
        Some(self.save())
    }

    /// One atomic write, timed. A refusal is recorded and the values are kept.
    pub fn save(&mut self) -> u64 {
        let started = Instant::now();
        match self.config.save(&self.write_to) {
            Ok(()) => self.saves += 1,
            Err(e) => self
                .problems
                .push(format!("{}: {e}", self.write_to.display())),
        }
        let ms = started.elapsed().as_millis() as u64;
        self.save_ms = ms;
        self.save_max_ms = self.save_max_ms.max(ms);
        ms
    }

    /// One line, because every other gauge line is one line.
    pub fn line(&self) -> String {
        format!(
            "read={} load={}ms applies={} apply={}/{}ms saves={} save={}/{}ms from={} write_to={}",
            self.tag(),
            self.load_ms,
            self.applies,
            self.apply_ms,
            self.apply_max_ms,
            self.saves,
            self.save_ms,
            self.save_max_ms,
            self.read_from.display(),
            self.write_to.display(),
        )
    }

    /// Send this process's later writes somewhere else, and say where.
    ///
    /// The read is not touched: `read_from` is still the file this copy came from, and
    /// `write_back`'s "did anything move?" is still decided against what that file
    /// says. Only the destination changes, which is what makes `saves=` a count of
    /// writes the product would really have made.
    pub fn pin_writes(&mut self, path: &Path) -> PathBuf {
        self.write_to = path.to_path_buf();
        self.write_to.clone()
    }

    /// What had to be corrected and what the machine refused - empty on a clean first
    /// run, which is why the caller prints it only when there is something to say.
    pub fn notes(&self) -> Vec<String> {
        let mut out = self.warnings.clone();
        out.extend(self.problems.iter().cloned());
        if let Some(b) = &self.backup {
            out.push(format!("unreadable config moved aside to {}", b.display()));
        }
        out
    }
}

static SETTINGS: OnceLock<Mutex<Settings>> = OnceLock::new();

/// The one copy the process runs on. Asking for it is what loads the file, so no
/// caller can get an empty `Settings` by asking before `start()` ran - the
/// alternative was a load in the middle of opening the mask, which is the hot path.
fn cell() -> &'static Mutex<Settings> {
    SETTINGS.get_or_init(|| {
        let (read, pinned) = boot();
        Mutex::new(Settings::open(&read, pinned.as_deref()))
    })
}

pub fn with<R>(f: impl FnOnce(&mut Settings) -> R) -> R {
    let mut guard = cell().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// Take the file at start-up and return the line the gauges print.
pub fn start() -> String {
    with(|s| s.line())
}

/// §5.7.1's memory, handed over as the mask opens.
pub fn apply_to(layer: &mut Layer) -> usize {
    with(|s| s.apply_to(layer))
}

/// §5.5.5's format and JPG quality, handed over as the mask opens.
pub fn export() -> EncodeOptions {
    with(|s| s.export_opts())
}

/// The `[output]` fields that decide a *name* rather than an encoder (§5.5.3's 命名规则,
/// §5.5.4's 快速保存目录).
///
/// A view rather than the whole `Output` because the mask snapshots this at `open` next
/// to its [`EncodeOptions`] and the two have to come from one instant: a file named by
/// the template as of *now* and an extension as of when the mask opened would be one name
/// built out of two settings states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Naming {
    /// `[output] save_dir`; empty means the pictures folder, see [`Naming::directory`].
    pub dir: String,
    pub template: String,
    /// `[output] naming_prefix`, the text `%p` stands for.
    pub prefix: String,
    pub collision: Collision,
}

impl Default for Naming {
    /// The shipped settings rather than empty strings: a mask that never reached `open`
    /// then names its files the way a first run with no `config.toml` does, which is the
    /// same reason `MaskState::export` defaults to `{Png, 92, true}` instead of zeros.
    fn default() -> Self {
        Self::from(&Output::default())
    }
}

impl From<&Output> for Naming {
    fn from(o: &Output) -> Self {
        Self {
            dir: o.save_dir.clone(),
            template: o.naming_template.clone(),
            prefix: o.naming_prefix.clone(),
            collision: o.collision.clone(),
        }
    }
}

impl Naming {
    /// Where a save that opens no dialog writes: `[output] save_dir`, or §6.2's pictures
    /// folder while that setting is empty.
    ///
    /// `None` is the machine declining to say where its pictures are, and it is carried
    /// as a refusal rather than resolved here: inventing a directory - temp, home, the
    /// current working directory - would put the user's screenshots somewhere nobody
    /// chose, which is the one thing §5.5.4 step 1's "预先配置" exists to prevent.
    pub fn directory(&self) -> Option<PathBuf> {
        if self.dir.is_empty() {
            falcon_core::config::pictures_dir()
        } else {
            Some(PathBuf::from(&self.dir))
        }
    }
}

/// §5.5.3's naming rule and §5.5.4's directory, handed over as the mask opens.
pub fn naming() -> Naming {
    with(|s| Naming::from(&s.config.output))
}

/// §5.5.6's rounded corners and §5.5.7's shadow and border, taken at the same `open` as
/// the [`EncodeOptions`] they are applied in front of.
///
/// A snapshot for the reason [`Naming`] is one: these decide pixels in a file the user is
/// waiting for, and reading the settings file in front of that write would both cost the
/// time and let an edit made mid-flow land on half a picture. The view is *identity* at
/// the shipped values, which is what lets every door that already exists keep its exact
/// bytes - the keys were in the file with no reader, and now they have one.
pub fn decoration() -> Decoration {
    with(|s| Decoration::from(&s.config.output))
}

/// §5.5.4 step 4's 揭示开关, taken at the same `open` as the name it reveals.
/// A one-line read rather than a field of [`Naming`], because `Naming` is `Eq` and
/// compares as a name: a bool that is not part of the name would make two masks that
/// name files identically look different to every leg that compares them.
pub fn reveal_after_save() -> bool {
    with(|s| s.config.output.reveal_after_save)
}

/// §5.5.3 step 2 的"截图后自动保存", the other one-line read beside [`Self::reveal_after_save`].
///
/// `Default` is `false`, which is the whole reason this round can claim every existing door
/// still answers with the same bytes: a key that was in the file unread becomes a door that
/// is not taken unless the user opened it in 首选项 > 输出.
pub fn auto_save() -> bool {
    with(|s| s.config.output.auto_save)
}

/// §5.5.1 step 3 的那一格"按默认行为复制"，与 [`Self::auto_save`] 在同一刻取。
///
/// 这一枚的 `Default` 是 `true`，所以下一行代码落下去的那一刻，一次 `Enter` 的字节就变了：
/// 这不是"新加一扇没人按的门"，是把 PRD 写着的默认行为接上 —— 而它和 `auto_save` 共用同
/// 一道 [`crate::mask::MaskState::commit`]，所以一次按下最多复制一次、最多落盘一次。
/// 单测里它是 `false`（`MaskState` 的 `Default`），那是"测试不许往用户剪贴板里写"那一格
/// （§9.1 第十条的 headless 半边）唯一的挡法；真机上这一格由 `open` 从配置文件里取。
pub fn auto_copy() -> bool {
    with(|s| s.config.output.auto_copy)
}

/// §5.5.8 那句"用户可选择覆盖原图或生成新图"里的**覆盖原图**开关（`[output] reuse_last_file`，
/// `Default` = `false`）。取的一刻与 [`Self::auto_copy`]、[`Self::auto_save`] 同一场 `open`，
/// 理由是同一句：它属于那一次流程，不属于手指按下去的那一秒。
///
/// 这一枚从"键在档里没人读"变到有读者的那一刻起，同一次 mask 会话里的第二次快速保存就不再
/// 递增序号，而是覆写第一次落下的那份文件。跨会话的"上一份"自 P63（第 63 项 ⑦）起由
/// [`last_landed`] / [`record_landed`] 那一对承担 —— 上一次会话落盘的路径记进 `Settings::last_landed`
/// 内存里一格，下一次会话的 `open()` 通过 [`crate::mask::MaskState::seed_landed_from`] 抄进本会话
/// 的 `landed`。跨进程重启仍不留（`config.toml` 里没有这一格）；那一半不在 §5.5.8 的原句要求内。
pub fn reuse_last_file() -> bool {
    with(|s| s.config.output.reuse_last_file)
}

// §5.5.11 `hide_pins_on_dialog` 这枚键本轮**没有**单独的一行读者（上面那四枚各有一枚的族），
// 理由是这一枚的调用点尚不存在：读它的那一手是"对话框出现 ⇒ 藏贴图"，而那一路要等
// `falcon_core::dialog_link::Hidden` 接到真的对话框事件上（第 176 项剩下的那一半）。
// 一枚没有调用点的一行读者在 bin crate 里就是 dead_code warning，而本档不留 `#[allow]`
// （P75-A 撤掉最后那一枚）。键本身自此有读者：[`output_cfg`] 一次 clone 整段、
// [`format_output_banner`] 把它逐字印进 `[output cfg]` 那一行。

/// §5.5.8 **跨 mask 会话**那半的落点（第 63 项 ⑦）：上一次会话 `quick_out` 落盘成功之后，
/// 那一份的路径由 [`record_landed`] 存在这一格里；下一次会话的 `open()` 通过 [`last_landed`]
/// 把它抄进 [`crate::mask::MaskState::landed`]。
///
/// 只在内存里存 —— 跨进程重启不留，因为 PRD §5.5.8 只说"用户可选择覆盖原图或生成新图"，
/// 没说重启之后仍要覆盖上一份；而把内部状态写进 `config.toml` 会占用户档里一行、
/// 让 `p58_deadkeys` 那一族的"键在档里没人读"这问反过来变成"没人写的键出现在档里"。
/// `Default` 是 `None`，因此出厂与未落盘时都答"没有可复用的上一张"，quick_out 的新写
/// 分支照走命名引擎。
pub fn last_landed() -> Option<std::path::PathBuf> {
    with(|s| s.last_landed.clone())
}

/// 记下这份刚刚落盘的路径，供下一次会话的 [`last_landed`] 抄走。
///
/// [`crate::mask::MaskState::quick_out`] 只在写成功之后调用 —— 写失败留下的路径不能进入
/// 这一格，否则下一次会话打开时拿到一张"上次我以为我落过、其实磁盘上没有"的文件，
/// reuse 那一支会覆写一份不存在的路径、或者覆写一份和用户上一次决定无关的文件。
pub fn record_landed(path: &std::path::Path) {
    with(|s| s.last_landed = Some(path.to_path_buf()));
}

/// §5.3.5 那格"实时显示选区尺寸"（`[capture] show_dimensions`，`Default` = `true`）。
///
/// 与 [`Self::auto_copy`] / [`Self::auto_save`] / [`Self::reuse_last_file`] 同一场 `open` 取值 —— 它是
/// 那一次流程决定的画面，而不是手指按下去的那一秒。
///
/// 出厂 `true`，所以键从"档里没人读"变到有读者那一刻起，默认路径的遮罩画面就多了一行 W×H 文字。
/// 与 auto_copy 那一格同形：这不是"新加一扇没人按的门"，是把 PRD 写着的默认行为接上；未接之前
/// PRD §5.3.5 那一栏只有 config 里的一枚 bool 而没有读者（§10 第 62 项 ⑤ 的第三枚）。
pub fn show_dimensions() -> bool {
    with(|s| s.config.capture.show_dimensions)
}

/// §5.5.4–14 那一整段 `[pin]` —— `default_topmost` / `default_opacity` / `default_zoom` /
/// `default_click_through` / `alpha_bg` / `wheel_resizes` / `cycle_on_click` /
/// `state_save_seconds` / `solo_dim_opacity`。P64 之前这一族在 `crates/core/src/config.rs` 里
/// 每枚都有键、每枚都有 `Default`，但 [`crate::state::PinState::new`] 用 `..Default::default()`
/// 把整段 `[pin]` 挡在运行期之外 —— 也就是**每一枚都没有读者**。
///
/// 这一枚读者一次性把九枚键接进 [`crate::state::PinState::cfg`]：`PinState::new()` 从这里取一次
/// 快照，之后 `spawn(... &self.cfg)` 用它建新 pin（`crates/core/src/pin.rs:459`
/// `topmost: cfg.default_topmost` 那一行自此真的会读到用户写的值而不是硬编码 `true`）。
///
/// 只取一次快照而不是每枚键单独走一次 `with`：`PinState::new()` 只在 STATE OnceLock 初始化时
/// 跑一次，取整段 clone 比取九次单字段更贴合"这一场进程看到的 [pin] 段"这个语义 —— 若中途
/// 有人 `write_back` 改了 [pin] 段，进程已经在跑的那些 pin 也不会跟着抖，和 PRD §5.5 里
/// "默认"两字的直觉一致（默认是**新**贴图的默认，不是**所有**贴图的当前）。
pub fn pin_cfg() -> falcon_core::config::Pin {
    with(|s| s.config.pin.clone())
}

/// §5.2–5.4：整段 `[capture]` 由这一枚 reader 一次性送进运行期 —— P64 之前 12 枚键里
/// 只有 `show_dimensions` 一枚有独立 reader，其余 11 枚（`default_mode` / `include_cursor`
/// / `freeze_frame` / `highlight_element` / `magnifier_radius` / `magnifier_zoom`
/// / `magnifier_grid` / `magnifier_coordinates` / `magnifier_color_format`
/// / `delay_seconds` / `snap_to_window`）都停在"键在档里没人读"。与 [`pin_cfg`] 同一形状：
/// **一次 open 只取一次快照**，中途 `write_back` 不改本会话已经开着的遮罩。
///
/// 快照而不是逐字段 reader，是因为 `[capture]` 那 12 枚键共同决定**同一场遮罩**：
/// `freeze_frame` 决定 open 抓哪一张、`magnifier_*` 决定 hover 时放大镜长什么样。分开取十几次
/// lock 与整段 clone 一次在这几个语义上是同一件事，但整段 clone 保证"同一场 open 看到的是同一份
/// config"，与 §5.2.5 里"按下快门那一刻"的直觉一致。
///
/// `delay_seconds` 自 P108 起**不在这一枚快照里被消费**：它决定的是 open **之前**等多久，而
/// "之前"意味着遮罩还不存在、本枚快照也还没取。它的行为读者是 [`crate::delay::arm_from_config`]，
/// 每次按下开始重新问一遍 —— 于是上面那句"11 枚停在键在档里没人读"到今天只剩 10 枚成立。
pub fn capture_cfg() -> falcon_core::config::Capture {
    with(|s| s.config.capture.clone())
}

/// §5.1：整段 `[general]` 由这一枚 reader 一次性取快照 —— P65 之前 6 枚键（`launch_at_startup`
/// / `start_minimized` / `show_tray` / `close_to_tray` / `confirm_exit` / `language`）全部停在
/// "键在档里没人读"：`crates/qt_bridge/src/hotkey.rs:352` 那一句 `Some(TraySpec { ... })` 是
/// P50 落 tray 那轮的硬编码，用户把 `[general] show_tray = false` 写进档里也不会让托盘消失。
/// P66 把 `show_tray` 接进 `Pump::start` 的 tray 参数（`Some` ↔ `None` 由这一枚 clone 出来
/// 的 `general.show_tray` 决定），其余 5 枚键（`launch_at_startup` 需要注册表 / 任务计划、
/// `start_minimized` 需要 main window 初始化时机、`close_to_tray` 需要窗口关闭拦截、
/// `confirm_exit` 需要对话框、`language` 需要 i18n 装填）本轮**由快照字段而不是由独立调用点**
/// 建了读者：`hotkey::install` 里 clone 一次整段 `[general]`，`show_tray` 那一支本轮真分岔，
/// 其余五支**待改码接进各自的调用点**（§5.15 那一族）。与 [`pin_cfg`] / [`capture_cfg`]
/// 同形：一次 clone 整段快照、进程启动那一次读，`write_back` 不改本会话已经开着的 tray。
pub fn general_cfg() -> falcon_core::config::General {
    with(|s| s.config.general.clone())
}

/// §5.14 与 §6.2：整段 `[history]` 由这一枚 reader 一次性取快照 —— P68 之前 7 枚键
/// （`enabled` / `max_items` / `max_age_days` / `max_size_mb` / `keep_originals` /
/// `thumb_px` / `exempt_locked`）全部停在"键在档里没人读"：`crates/core/src/history.rs`
/// 的 `History::open` / `record` / `delete` 那几枚 API 都在，`#[cfg(test)]` 里的 14 枚
/// 单测也都跑得过，但整个 crate 之外**没有一处调用点**。也就是说：截图软件把每一张图
/// 存进磁盘之后，历史库从来没被建过、行从来没被写过、清理逻辑从来没被跑过 —— 用户
/// 在档里写的 `[history] enabled = false` 与 `= true` 是同一件事。
///
/// 与 [`pin_cfg`] / [`capture_cfg`] / [`general_cfg`] 同一形状：一次 clone 整段快照、
/// 进程启动那一次读、`write_back` 不改本会话已经开着的 History 句柄。
///
/// **本轮不是"读者-only"**：紧接其下的 [`history_open_in`] 是这一枚快照的第一位调用点 ——
/// 把 `[history].enabled` 那一支变成一次可测的分岔（Enabled 走 `History::open(dir)` 建出
/// `dir/history.db`，Disabled 直接 `Ok(None)` 而 disk 上一件物事也不动）。于是 §9.1
/// 第十条的 headless 半边覆盖到 `[history]` 这一段 —— 与 P67 给 `show_tray` 那一格
/// 补 `tray_from()` 是同一条路。
///
/// **P72 起本函数有非 test 调用点**：[`startup_banner`] 在 `main.rs` 里 `settings::start()`
/// 之后立刻读一次，把 `[history].enabled` 打进 `[startup cfg]` 那一行。这一族自此与
/// P64 / P65 / P66 三轮的 `pin_cfg` / `capture_cfg` / `general_cfg` 同形、函数头上的
/// `#[allow(dead_code)]` 撤。
pub fn history_cfg() -> falcon_core::config::History {
    with(|s| s.config.history.clone())
}

/// §5.14 与 §6.2 的**分岔本体**：把 `[history] enabled` 与一份数据目录合成"要不要真开
/// SQLite 句柄"这个决定 —— 真则 `Ok(Some(History))` 且 `dir/history.db` 被建出来（或
/// 打开已有的那一份）、假则 `Ok(None)` 且 disk 上一件物事也不动。
///
/// 抽出来是因为 P66 那句"分岔代码自己作证"本轮不满足（§9.1 第十条 P67 改口的口径：
/// 分岔本体是纯函数时不再撤单测）。这一枚是纯函数：输入是 cfg 与 dir、输出是
/// `Result<Option<History>, HistoryError>`、没有消息泵、没有窗柄、没有会话 —— `cargo
/// test -p qt_bridge` 直接调这个函数、看两侧各自的磁盘副作用对不对，就替 `[history]`
/// 那一族补上第一位 §9.1 单测证人。
///
/// Enabled 一侧调用 [`falcon_core::history::History::open`] —— 那是 P68 之前整个
/// `falcon_core::history` 模块的第一次非 test 调用点。open 的失败（`Schema` 版本
/// 太新 / IO 拒绝 / SQLite 拒绝）原样返回、不吞：调用点决定是 log 还是降级为无历史，
/// 而不是在这里假装成功。
///
/// **P68 起本函数建分岔本体但**暂时**没有非 test 调用点（`#[allow(dead_code)]` 挂着）；
/// **P75 起本函数有非 test 调用点**：[`startup_history_probe`] 在 `main.rs` 的
/// `--selftest --history` 档里被调、由 [`state::history_selftest`] 组一行 verdict ⇒
/// `#[allow(dead_code)]` 撤。§5 config 的 dispatch 层自此每一位读者都跨过产品路径门槛
/// （P64 pin / P65 capture / P66 general / P72 history + advanced / P73 appearance /
/// P74 about / **P75 history_open_in**）；本轮的**分岔本体是纯函数**（`cfg.enabled` 一侧
/// 真开 SQLite、另一侧 `Ok(None)` 不动 disk） ⇒ 证人 dichotomy 的第 69 项 ⑫ 那一族本轮
/// **从 P73/P74 的"读者-only ⇒ 撤单测"一侧再换回 P67/P68/P72 那一侧**（**分岔本体是纯函数
/// ⇒ 不撤单测** 那一族本轮回归：P67 / P68 / P72 / **P75** 四轮；本轮的证人是 §9.1 那一枚
/// 新增 cargo 单测 `refusals_reject_history_without_selftest`，见 `main.rs` 测试模块）。
pub fn history_open_in(
    dir: &std::path::Path,
    cfg: &falcon_core::config::History,
) -> Result<Option<falcon_core::history::History>, falcon_core::history::HistoryError> {
    if cfg.enabled {
        falcon_core::history::History::open(dir).map(Some)
    } else {
        Ok(None)
    }
}

/// §5.15 那 8 枚 `[appearance]` 键（`theme` / `accent` / `toolbar_placement` /
/// `ui_font_size` / `magnifier_border` / `magnifier_corner_radius` /
/// `magnifier_foreground` / `magnifier_background`）的首位读者。
///
/// 与 [`pin_cfg`] / [`capture_cfg`] / [`general_cfg`] / [`history_cfg`] 同一族：一次 clone
/// 整段快照、进程启动那一次读、`write_back` 不改本会话已经开着的 Appearance 影子。
///
/// **P69 起本函数建首位读者但**暂时**没有非 test 调用点（`#[allow(dead_code)]` 挂着）；
/// **P73 起本函数有非 test 调用点**：[`startup_appearance_banner`] 在 `main.rs` 里
/// `settings::start()` 之后紧跟 [`startup_banner`] 之后调一次、打印 `[appearance cfg]`
/// 一行 ⇒ `#[allow(dead_code)]` 撤。这一族自此与 P64 pin_cfg / P65 capture_cfg / P66 general_cfg /
/// P72 history_cfg + advanced_cfg 五轮"读者 + 真调用点"同形。
///
/// **本轮没有分岔本体**：8 枚键逐字回读即可（`theme` 是 enum 走 `{:?}`、`accent` /
/// `toolbar_placement` / `magnifier_*` 是 String 直读、`ui_font_size` / `magnifier_border` /
/// `magnifier_corner_radius` 是 u32 直读）⇒ 证人 dichotomy 的第 69 项 ⑫ 那一族本轮**回到
/// "读者-only ⇒ 撤单测"一侧**（P64 / P65 / P66 / P69 / P70 / P71 六轮同族、P67 / P68 / P72 三轮
/// 是"分岔本体是纯函数 ⇒ 不撤单测"另一侧、本轮再回来 ⇒ **本轮不加 cargo 单测**、
/// qt 计数仍 207、三枚之和仍 499）。
pub fn appearance_cfg() -> falcon_core::config::Appearance {
    with(|s| s.config.appearance.clone())
}

/// §5.16 那 6 枚 `[advanced]` 键（`cli_enabled` / `log_level` / `data_dir` / `recognize_text`
/// / `crash_recovery` / `privacy_no_temp_files`）的首位读者。
///
/// 与 [`pin_cfg`] / [`capture_cfg`] / [`general_cfg`] / [`history_cfg`] / [`appearance_cfg`]
/// 同一族：一次 clone 整段快照、进程启动那一次读、`write_back` 不改本会话已经开着的 Advanced
/// 影子。本轮是"读者-only 无分岔"那一族（证人 dichotomy 见 第 69 项 ⑫）：不加 cargo 单测，
/// 本轮的证人是梯 rc 0 + exe sha 换，而不是 §9.1 第十条 headless。
///
/// `#[allow(dead_code)]` 挂在函数头上、P72 起由 [`startup_banner`] 一次读 `log_level` 与
/// `data_dir` 两格 ⇒ `#[allow(dead_code)]` 撤。剩下四格（`cli_enabled` / `recognize_text`
/// / `crash_recovery` / `privacy_no_temp_files`）仍待 P73+ 接进各自的调用点（`cli_enabled`
/// 那一格要 UI 面：Settings 里的 CLI 注册按钮；`recognize_text` 要 §5.6 OCR dispatch；
/// `crash_recovery` 要 §7.2 recovery snapshot；`privacy_no_temp_files` 要 temp-file policy）。
/// 但**整段读者**自此有非 test 调用点、不再算死码。
pub fn advanced_cfg() -> falcon_core::config::Advanced {
    with(|s| s.config.advanced.clone())
}

/// §5.17 那 4 枚 `[about]` 键（`auto_check_updates` / `check_every_days` / `channel`
/// / `send_diagnostics`）的首位读者。
///
/// 与 [`pin_cfg`] / [`capture_cfg`] / [`general_cfg`] / [`history_cfg`] / [`appearance_cfg`]
/// / [`advanced_cfg`] 同一族：一次 clone 整段快照、进程启动那一次读、`write_back` 不改本会话
/// 已经开着的 About 影子。本轮是"读者-only 无分岔"那一族（证人 dichotomy 见 第 69 项 ⑫）：
/// 不加 cargo 单测，本轮的证人是梯 rc 0 + exe sha 换，而不是 §9.1 第十条 headless。
///
/// **P71 起本函数建首位读者但**暂时**没有非 test 调用点（`#[allow(dead_code)]` 挂着）；
/// **P74 起本函数有非 test 调用点**：[`startup_about_banner`] 在 `main.rs` 里
/// `settings::start()` 之后紧跟 [`startup_appearance_banner`] 之后调一次、打印 `[about cfg]`
/// 一行 ⇒ `#[allow(dead_code)]` 撤。§5 那 7 段 config 至此**全部**建完首位读者**且**全部
/// 有非 test 调用点（P64 pin / P65 capture / P66 general / P72 history + advanced /
/// P73 appearance / **P74 about**）。整段读者链自此收齐；剩下的 `history_open_in` 一枚仍挂
/// `#[allow]`（它不是读者而是 dispatch，要等后续轮次接进 `Settings::open` 之后那一道门）。
pub fn about_cfg() -> falcon_core::config::About {
    with(|s| s.config.about.clone())
}

/// §5.14 / §5.16 的关键值合成一行 String；空 `data_dir` 打印 `(platform default)` 而不是空。
///
/// **P72 的分岔本体**：`a.data_dir.is_empty()` 一侧显示 `(platform default)`（§5.16 那一格写着
/// "Empty = the platform default under `%APPDATA%`"，把空字符串原样印出来会被读成"这一档没值"），
/// 另一侧显示用户自己填的路径。纯函数：输入是 cfg 的两个借用、输出是 String、没有 disk / 没有
/// 窗柄 / 没有会话 ⇒ **分岔是纯函数就不撤单测**（证人 dichotomy 的第 69 项 ⑫ 那一族、与 P67
/// `tray_from` / P68 `history_open_in` 同形）。
pub fn format_startup_banner(
    h: &falcon_core::config::History,
    a: &falcon_core::config::Advanced,
) -> String {
    let dir_field = if a.data_dir.is_empty() {
        "(platform default)"
    } else {
        &a.data_dir
    };
    format!(
        "history.enabled={} history.max_items={} advanced.log_level={} advanced.data_dir={}",
        h.enabled, h.max_items, a.log_level, dir_field
    )
}

/// 一次读 `history_cfg` + `advanced_cfg`、把 [`format_startup_banner`] 的返回值交给 `main.rs`。
///
/// **P72 起两枚读者自此有非 test 调用点**：main.rs 在 `settings::start()` 之后立刻调这一枚、
/// 打印 `[startup cfg]` 那一行。这一族自此与 P64 pin_cfg / P65 capture_cfg / P66 general_cfg
/// 三轮"读者 + 真调用点"同形、两枚函数头上的 `#[allow(dead_code)]` 撤。
///
/// **本轮不动 disk**：`history_cfg.enabled` 只被读进 String、[`history_open_in`] 不被调；
/// 真开 SQLite 句柄留给 P73+ 接进 `Settings::open` 之后那一道门（那时 disk 副作用会跟着
/// `history.db` 长出来一份、[history_open_in] 头上的 `#[allow]` 才撤）。`appearance_cfg`
/// 与 `about_cfg` 那两枚仍挂 `#[allow]`，各自待后续轮次接进 QML `#[qproperty]` 与 update-checker。
pub fn startup_banner() -> String {
    format_startup_banner(&history_cfg(), &advanced_cfg())
}

/// P73：把整段 `[appearance]` 的 8 枚键逐字回读成一行。
///
/// **本轮没有分岔本体**（P64 / P65 / P66 / P69 / P70 / P71 那一族"读者-only 无分岔 ⇒ 撤单测"
/// 本轮沿用）：`theme` 是 enum 走 `{:?}`（`System` / `Light` / `Dark` 三臂、本轮不加"System
/// 展开成实际生效的一臂"那种分岔，那需要 `QGuiApplication::styleHints()->colorScheme()` 的
/// 运行期读数、本轮 `settings::start()` 早于 Qt 存在 ⇒ 无法读）、其余 7 枚是 String / u32 直读。
///
/// **证人 dichotomy 的第 69 项 ⑫ 那一族本轮回到"读者-only ⇒ 撤单测"一侧**（P67 / P68 / P72
/// 三轮分岔+单测之后、本轮同 P64 / P65 / P66 / P69 / P70 / P71 六轮那一族 ⇒ **本轮不加 cargo
/// 单测**、qt 计数仍 207、三枚之和仍 499，本轮的证人是梯 rc 0 + exe sha 换）。
pub fn format_appearance_banner(a: &falcon_core::config::Appearance) -> String {
    format!(
        "appearance.theme={:?} appearance.accent={} appearance.toolbar_placement={} appearance.ui_font_size={} appearance.magnifier_border={} appearance.magnifier_corner_radius={} appearance.magnifier_foreground={} appearance.magnifier_background={}",
        a.theme,
        a.accent,
        a.toolbar_placement,
        a.ui_font_size,
        a.magnifier_border,
        a.magnifier_corner_radius,
        a.magnifier_foreground,
        a.magnifier_background
    )
}

/// 一次读 `appearance_cfg`、把 [`format_appearance_banner`] 的返回值交给 `main.rs`。
///
/// **P73 起 appearance_cfg 自此有非 test 调用点**：main.rs 在 [`startup_banner`] 之后
/// 立刻调这一枚、打印 `[appearance cfg]` 那一行。
pub fn startup_appearance_banner() -> String {
    format_appearance_banner(&appearance_cfg())
}

/// P118：§5.19.1 第 1 步「读取系统主题（Windows 注册表 `AppsUseLightTheme`）」的那一行读数。
///
/// **本轮的分岔本体是两格**：`system_light` 三臂（`Some(true)` / `Some(false)` / `None`）
/// 与 `resolve_theme(requested, system_light)` 的返回值。前者是机器的 answer、后者是「用户
/// 的请求 + 机器的 answer」合成出的实际生效一臂。纯函数：两枚输入都是借用、输出是 String、
/// 不读 disk、不碰窗柄、不碰注册表（注册表那一步在 [`startup_theme_banner`] 里）⇒
/// **分岔是纯函数就不撤单测**（证人 dichotomy 的第 69 项 ⑫ 那一族、与 P67 `tray_from` /
/// P68 `history_open_in` / P72 `format_startup_banner` / P76 `format_output_banner` 同形）。
///
/// 三格各写各的、不合并成一格：`appearance.theme=System` 是用户钉的、`system.theme=light`
/// 是机器答的、`resolved.theme=Light` 是本程序生效的。合并会丢掉「机器压根没答」这一臂
/// （`(unreadable)`）与手动主题「机器答什么都不影响生效值」这一事实。
pub fn format_theme_banner(
    requested: falcon_core::config::Theme,
    system_light: Option<bool>,
) -> String {
    let resolved = falcon_core::config::resolve_theme(requested, system_light);
    let machine = match system_light {
        Some(true) => "light",
        Some(false) => "dark",
        None => "(unreadable)",
    };
    format!(
        "appearance.theme={:?} system.theme={} resolved.theme={:?}",
        requested, machine, resolved
    )
}

/// 一次读 `appearance_cfg` 的 `theme` + 一次读注册表，把 [`format_theme_banner`] 交给 `main.rs`。
///
/// **P118 起 `platform_windows::theme::apps_use_light_theme` 自此有非 test 调用点**：main.rs
/// 在 [`startup_appearance_banner`] 之后立刻调这一枚、打印 `[theme cfg]` 一行。运行期读数走
/// 的是 HKCU 那枚 `AppsUseLightTheme`，而不是 P73 注释里写明的
/// `QGuiApplication::styleHints()->colorScheme()` —— 后者要求 Qt 已存在，而本行的读数刻意
/// 留在 `settings::start()` 那一段（Qt 之前），前者只需要 kernel32/advapi32。
///
/// **§5.19.1 第 3 步「系统主题切换时同步更新」自 P120 起有监听腿**：`platform_windows::hotkey`
/// 的 pump 把 `WM_SETTINGCHANGE` 交成 `Event::SettingChange`，GUI 线程那侧的 `theme_refresh` 由此
/// 叫 `apps_use_light_theme_invalidate()`（那一枚缓存唯一的失效点）重新问一次机器 ⇒ 本函数读到的
/// 不再是「只有启动那一刻有效」的一枚值，而是下一次读到时机器的当前值。P118/P119 注释里「仍不做、
/// 没有监听」那句自此被 P120 的读数作废（#332）。第 2 步（调色板切换）自 P119 起由 [`theme_face`] 交货。
pub fn startup_theme_banner() -> String {
    format_theme_banner(
        appearance_cfg().theme,
        platform_windows::theme::apps_use_light_theme(),
    )
}

/// P119：§5.19.1 第 2 步「按主题切换 QML 调色板」要的那一枚词。
///
/// 界面侧只认 `"light"` / `"dark"` 两个串，因为它写不出 `Option<bool>` 这种形状 —— 而
/// 「机器压根没答」那一臂在这里必须有归宿：[`falcon_core::config::resolve_theme`] 对 `None`
/// 交回 `Light`（Windows 自己的兜底方向，也是 [`format_theme_banner`] 的
/// `system.theme=(unreadable)` 那一臂），所以形状丢了、口径没丢。
///
/// **分岔本体是 `resolve_theme` 的输出而不是机器的 answer**：`Theme::System` 吃三臂、
/// `Theme::Light` / `Theme::Dark` 两枚手动臂不吃。纯函数（借用进、`&'static str` 出、不读
/// disk、不碰注册表）⇒ 分岔是纯函数就不撤单测（P67 `tray_from` / P68 `history_open_in` /
/// P118 `format_theme_banner` 同族）；注册表那一格长在 [`startup_theme_face`] 里。
pub const fn theme_face(
    requested: falcon_core::config::Theme,
    system_light: Option<bool>,
) -> &'static str {
    match falcon_core::config::resolve_theme(requested, system_light) {
        falcon_core::config::Theme::Dark => "dark",
        _ => "light",
    }
}

/// 一次读 `[appearance] theme`、一次读注册表，交出 [`theme_face`] 那一枚词。
///
/// `Session.theme` 这枚 qproperty 的唯一来源（P119）：界面拿到的与 `[theme cfg]` 那行拿到的
/// 是同一次 `apps_use_light_theme()` 的读数走出来的两格，而不是两次各读各的注册表。
/// 「同一次」在 P119 之前只是这句注释，之后是 [`platform_windows::theme::apps_use_light_theme`]
/// 里那一枚 `OnceLock`：这一枚函数被调三次（banner、`refresh_theme`、`hint palette` 的期待值）
/// 而 HKCU 只开一趟。
pub fn startup_theme_face() -> &'static str {
    theme_face(
        appearance_cfg().theme,
        platform_windows::theme::apps_use_light_theme(),
    )
}

/// P74：把整段 `[about]` 的 4 枚键逐字回读成一行。
///
/// **本轮没有分岔本体**（P64 / P65 / P66 / P69 / P70 / P71 / P73 那一族"读者-only 无分岔
/// ⇒ 撤单测"本轮沿用）：4 枚键是 2 枚 bool + 1 枚 u32 + 1 枚 String，全部直读。`channel`
/// 三臂（`stable` / `beta` / `nightly`）的 clamp 已经在
/// [`falcon_core::config::Config::normalize`] 里做过 ⇒ 本轮不需要"未知 channel 显示 stable"
/// 那种分岔，读到这一格就已经是三条中的一条（未知 → `stable` 已经在 cfg.normalize 那一步完成）。
///
/// **证人 dichotomy 的第 69 项 ⑫ 那一族本轮回到"读者-only ⇒ 撤单测"一侧**（P72 是分岔本体、
/// P73 / P74 是读者-only ⇒ **本轮不加 cargo 单测**、qt 计数仍 207、三枚之和仍 499，本轮的
/// 证人是梯 rc 0 + exe sha 换）。
pub fn format_about_banner(a: &falcon_core::config::About) -> String {
    format!(
        "about.auto_check_updates={} about.check_every_days={} about.channel={} about.send_diagnostics={}",
        a.auto_check_updates, a.check_every_days, a.channel, a.send_diagnostics
    )
}

/// 一次读 `about_cfg`、把 [`format_about_banner`] 的返回值交给 `main.rs`。
///
/// **P74 起 about_cfg 自此有非 test 调用点**：main.rs 在 [`startup_appearance_banner`] 之后
/// 立刻调这一枚、打印 `[about cfg]` 那一行。至此 §5 那 7 段 config 全部跨过产品路径门槛。
pub fn startup_about_banner() -> String {
    format_about_banner(&about_cfg())
}

/// §5.5 `[output]` 段那 19 枚键（含 §5.5.11 `share_targets` / §5.5.12 `send_to_apps`
/// 这两枚"第 63 项 ⑥ 起整批结转"的死键）的首位读者。
///
/// 枚数由现读而来，不由手数：`.qoder/scratch/p123_outputkeys_run_b.txt` 那一趟读
/// `[output fields] n=19` 且 `[missing in Default] []`。同一趟也当场把这行旧账作废 —— 这里
/// 曾经写"17 枚键"（`main.rs` 那行也写着同一枚过期数），而本轮建键之前逐名点到的是 18 枚。
/// ⇒ 第 332 项那一族（注释被自己的读数作废而未改码）本轮再履行一次。
///
/// 与 [`pin_cfg`] / [`capture_cfg`] / [`general_cfg`] / [`history_cfg`] / [`appearance_cfg`]
/// / [`advanced_cfg`] / [`about_cfg`] 同一族：一次 clone 整段快照、进程启动那一次读。
/// P58 / P59 / P60-③ 曾建过 `Output` 里几枚键的**独立**读者（auto_save / auto_copy /
/// reuse_last_file），本函数是**整段**的第一位读者，也是这两枚余额键的首位读者。
///
/// §5.5.11 的 `hide_pins_on_dialog`（第 176 项、P123 新建）自此与这两枚同一族：整段读者 +
/// [`format_output_banner`] 的逐字回读，**没有**独立的一行读者 —— 理由写在 [`reuse_last_file`]
/// 下面那段注释里（它的调用点还没到）。
pub fn output_cfg() -> falcon_core::config::Output {
    with(|s| s.config.output.clone())
}

/// P76：把整段 `[output]` 的 `share_targets` + `send_to_apps` 两枚 Vec<String> 回读成一行。
///
/// **本轮的分岔本体**：`o.share_targets.is_empty()` 一侧显示 `(none)`、另一侧显示 `a,b,c`
/// joined；`o.send_to_apps.is_empty()` 同一族。纯函数：输入是 cfg 的一份借用、输出是 String、
/// 没有 disk / 没有窗柄 / 没有会话 ⇒ **分岔是纯函数就不撤单测**（证人 dichotomy 的第 69 项 ⑫
/// 那一族、与 P67 `tray_from` / P68 `history_open_in` / P72 `format_startup_banner` /
/// P75 refuse 规则同形，本轮是**第五次**走这一族）。
pub fn format_output_banner(o: &falcon_core::config::Output) -> String {
    let shares = if o.share_targets.is_empty() {
        "(none)".to_string()
    } else {
        o.share_targets.join(",")
    };
    let apps = if o.send_to_apps.is_empty() {
        "(none)".to_string()
    } else {
        o.send_to_apps.join(",")
    };
    format!(
        "output.share_targets={} output.send_to_apps={} output.reuse_last_file={} output.hide_pins_on_dialog={}",
        shares, apps, o.reuse_last_file, o.hide_pins_on_dialog
    )
}

/// 一次读 `output_cfg`、把 [`format_output_banner`] 的返回值交给 `main.rs`。
///
/// **P76 起 output_cfg 自此有非 test 调用点**：main.rs 在 [`startup_about_banner`] 之后
/// 立刻调这一枚、打印 `[output cfg]` 那一行。§5.5.11 / §5.5.12 那两枚键自此**第一次**跨过
/// 产品路径门槛 ⇒ 第 63 项 ⑥ → 第 76 项 ⑥ 整批结清。
pub fn startup_output_banner() -> String {
    format_output_banner(&output_cfg())
}

/// §5.5.12 `send_to_apps` 那一批 `name|executable` 条目的**解析本体**。
///
/// P76 把这两枚死键的**读侧**结清之后、本轮 P77 补的是 dispatch 侧的**第一步**：
/// 只做"能不能拆、拆完谁留下"，不 fork 外部程序、不 touch 用户会话（会话锁状态未知 ⇒
/// 真 spawn 留给下一轮）。返回 `(accepted, refused_count)`；accepted 是
/// `(trim(name), trim(exe))` 的 Vec。
///
/// **分岔本体**（第 69 项 ⑫ 那一族、**第六次**走"分岔是纯函数 ⇒ 加独立 cargo 单测证人"
/// 一侧 —— 前五轮：P67 [`tray_from`] / P68 [`history_open_in`] / P72 [`format_startup_banner`]
/// / P75 refuse 规则 / P76 [`format_output_banner`]）：
/// - 无 `|` → 拒；
/// - 有 `|` 但首段 `.trim()` 为空 → 拒；
/// - 有 `|` 但尾段 `.trim()` 为空 → 拒；
/// - 其余一律按**首个** `|` 拆，右侧原样保留（`a|b|c` → name=`a`、exe=`b|c`）。
///
/// 纯函数：输入 `&[String]`、输出 `(Vec<(String, String)>, usize)`，不落盘 / 不开窗 /
/// 不 spawn ⇒ 分岔本体是纯函数就不撤单测。
pub fn parse_send_to(entries: &[String]) -> (Vec<(String, String)>, usize) {
    let mut ok: Vec<(String, String)> = Vec::new();
    let mut refused = 0usize;
    for e in entries {
        match e.split_once('|') {
            Some((name, exe)) => {
                let n = name.trim();
                let x = exe.trim();
                if n.is_empty() || x.is_empty() {
                    refused += 1;
                } else {
                    ok.push((n.to_string(), x.to_string()));
                }
            }
            None => refused += 1,
        }
    }
    (ok, refused)
}

/// P77：把 [`parse_send_to`] 的结果回读成一行、走产品路径。
///
/// `main.rs` 在 `[output cfg]` 之后立刻调这一枚、打印 `[send_to cfg] parsed=N refused=M`
/// 一行。至此 §5.5.12 那条 `name|executable` 键自此**跨过解析侧门槛**：读侧（P76）
/// + 解析侧（P77）都有非 test 调用点。UI 面与真 spawn 留下一轮。
pub fn startup_send_to_banner() -> String {
    let o = output_cfg();
    let (ok, refused) = parse_send_to(&o.send_to_apps);
    format!("send_to parsed={} refused={}", ok.len(), refused)
}

/// §5.5.11 `share_targets` 那一批条目的解析本体（P78）。
///
/// PRD §5.5.11 未定 `name|exe` 那一族格式（PRD 原句只写 "share targets shown in the
/// toolbar"）⇒ 本轮走**最保守解析**：每条 `.trim()` 之后**非空**即收、空即拒；输出
/// 侧保留 trim 之后的顺序、不去重、不校验目标名是否落在已知 share-provider 白名单
/// 上（白名单那一层是下一轮的活）。返回 `(accepted, refused_count)`。
///
/// **分岔本体**（第 69 项 ⑫ 那一族、**第七次**走"分岔是纯函数 ⇒ 加独立 cargo 单测证人"
/// 一侧 —— 前六轮：P67 tray_from / P68 history_open_in / P72 format_startup_banner /
/// P75 refuse 规则 / P76 format_output_banner / P77 parse_send_to）：
/// - `.trim()` 空 → 拒；
/// - 其余一律收，输出侧带 trim 后的字符串。
///
/// 纯函数：输入 `&[String]`、输出 `(Vec<String>, usize)`，不落盘 / 不开窗 / 不 spawn。
pub fn parse_share_targets(entries: &[String]) -> (Vec<String>, usize) {
    let mut ok: Vec<String> = Vec::new();
    let mut refused = 0usize;
    for e in entries {
        let t = e.trim();
        if t.is_empty() {
            refused += 1;
        } else {
            ok.push(t.to_string());
        }
    }
    (ok, refused)
}

/// P78：把 [`parse_share_targets`] 的结果回读成一行、走产品路径。
///
/// `main.rs` 在 `[send_to cfg]` 之后立刻调这一枚、打印 `[share cfg] parsed=N refused=M`
/// 一行。至此 §5.5.11 那一批条目自此**跨过解析侧门槛**：读侧（P76）+ 解析侧（P78）。
/// UI 面与真 share-provider 白名单校验留下一轮。
pub fn startup_share_banner() -> String {
    let o = output_cfg();
    let (ok, refused) = parse_share_targets(&o.share_targets);
    format!("share parsed={} refused={}", ok.len(), refused)
}

/// P79：把 [`parse_share_targets`] 输出的条目对照一枚**内置白名单**分派 —— 命中 ⇒ known、
/// 未命中 ⇒ unknown。大小写不敏感（PRD §5.5.11 的 share 目标名与 UI 显示同源；测试里
/// `Teams` 也能命中 `"teams"`）。
///
/// 纯函数：输入 `(parsed, allowed)`、输出 `(known, unknown)`，不落盘 / 不开窗 / 不 spawn。
pub fn filter_share_targets(parsed: &[String], allowed: &[&str]) -> (Vec<String>, Vec<String>) {
    let mut known: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for p in parsed {
        if allowed.iter().any(|a| a.eq_ignore_ascii_case(p.as_str())) {
            known.push(p.clone());
        } else {
            unknown.push(p.clone());
        }
    }
    (known, unknown)
}

/// P79：[`filter_share_targets`] 挂上产品路径 —— 从 [`output_cfg`] 读 share_targets、
/// 走 [`parse_share_targets`] 再对照内置 5 枚白名单（`slack / teams / wecom / dingtalk /
/// feishu`）、把 known/unknown 计数回读成一行由 `main.rs` 打印。至此 §5.5.11 那一批
/// 条目跨过 **读侧（P76）+ 解析侧（P78）+ 分派侧（P79）** 三道门槛；UI 面与真
/// share-provider 集成留下一轮。
pub fn startup_share_filter_banner() -> String {
    const ALLOWED: &[&str] = &["slack", "teams", "wecom", "dingtalk", "feishu"];
    let o = output_cfg();
    let (parsed, _refused) = parse_share_targets(&o.share_targets);
    let (known, unknown) = filter_share_targets(&parsed, ALLOWED);
    format!("share known={} unknown={}", known.len(), unknown.len())
}

/// P80：把 [`parse_send_to`] 输出的 `(name, exe)` 对按 **Windows 可执行扩展名**
/// （`.exe` / `.cmd` / `.bat`，大小写不敏感）分派 —— 命中 ⇒ 收进 ok、未命中 ⇒ 计入
/// refused。PRD §5.5.12 的"发送到应用"要求条目**真的可执行**；本轮只做**扩展名形状**
/// 这一格，不 `Path::exists()` 也不 spawn（会话锁状态未知 ⇒ 留下一轮）。
///
/// 纯函数：输入 `&[(String, String)]`、输出 `(Vec<(String, String)>, usize)`，
/// 不落盘 / 不开窗 / 不 spawn。
pub fn validate_exe_paths(parsed: &[(String, String)]) -> (Vec<(String, String)>, usize) {
    const OK_EXTS: &[&str] = &[".exe", ".cmd", ".bat"];
    let mut ok: Vec<(String, String)> = Vec::new();
    let mut refused = 0usize;
    for (name, exe) in parsed {
        let lower = exe.to_lowercase();
        if OK_EXTS.iter().any(|e| lower.ends_with(e)) {
            ok.push((name.clone(), exe.clone()));
        } else {
            refused += 1;
        }
    }
    (ok, refused)
}

/// P80：[`validate_exe_paths`] 挂上产品路径 —— 从 [`output_cfg`] 读 send_to_apps、
/// 走 [`parse_send_to`] 再按扩展名校验、把 validated/refused 计数回读成一行由 `main.rs`
/// 打印。至此 §5.5.12 那批条目跨过 **读侧（P76）+ 解析侧（P77）+ 校验侧（P80）** 三道门槛；
/// UI 面与真 `Command::spawn()` 留下一轮。
pub fn startup_send_to_validate_banner() -> String {
    let o = output_cfg();
    let (parsed, _refused) = parse_send_to(&o.send_to_apps);
    let (ok, refused) = validate_exe_paths(&parsed);
    format!("send_to validated={} refused={}", ok.len(), refused)
}

/// P81：把 [`parse_send_to`]（P77）与 [`validate_exe_paths`]（P80）**复合成一趟**：
/// 输入原始 `&[String]` 条目，一趟返回 `(final_ok, parse_refused, validate_refused)`
/// 三枚计数 —— 前者是按 `|` 拆过 + 按 Windows 可执行扩展名校验过的 (name, exe) 对，
/// 中者是拆管道被拒（无 `|` / 首尾段空）的条目数，后者是扩展名不在
/// `.exe` / `.cmd` / `.bat` 的条目数。
///
/// 纯函数：不落盘 / 不开窗 / 不 spawn。
pub fn pipeline_send_to(entries: &[String]) -> (Vec<(String, String)>, usize, usize) {
    let (parsed, parse_refused) = parse_send_to(entries);
    let (final_ok, validate_refused) = validate_exe_paths(&parsed);
    (final_ok, parse_refused, validate_refused)
}

/// P81：[`pipeline_send_to`] 挂上产品路径 —— 从 [`output_cfg`] 一次读整段 send_to_apps、
/// 走一趟 parse+validate、把三枚计数回读成一行由 `main.rs` 打印。至此 §5.5.12 那批条目
/// 跨过 **读侧（P76）+ 解析侧（P77）+ 校验侧（P80）+ 复合侧（P81）** 四道门槛；
/// UI 面与真 `Command::spawn()` 留下一轮。
pub fn startup_send_to_pipeline_banner() -> String {
    let o = output_cfg();
    let (final_ok, p_refused, v_refused) = pipeline_send_to(&o.send_to_apps);
    format!(
        "pipeline final={} parse_refused={} validate_refused={}",
        final_ok.len(),
        p_refused,
        v_refused
    )
}

/// P82：§5.5.11 `share_targets` 的**复合** dispatch —— 把 P78 `parse_share_targets`
/// 与 P79 `filter_share_targets` 一次跑完，返回 `(known, parse_refused, unknown)`：
/// parse_refused 是空条目被 trim 掉的数目，unknown 是过 parse 但不在白名单的条目。
///
/// 纯函数：不落盘 / 不开窗 / 不 spawn / 不联网。
pub fn pipeline_share_targets(entries: &[String]) -> (Vec<String>, usize, usize) {
    const ALLOWED: &[&str] = &["slack", "teams", "wecom", "dingtalk", "feishu"];
    let (parsed, parse_refused) = parse_share_targets(entries);
    let (known, unknown) = filter_share_targets(&parsed, ALLOWED);
    (known, parse_refused, unknown.len())
}

/// P82：[`pipeline_share_targets`] 挂上产品路径 —— 从 [`output_cfg`] 一次读整段
/// share_targets、走一趟 parse+filter、把三枚计数回读成一行由 `main.rs` 打印。
/// 至此 §5.5.11 那批条目跨过 **读侧（P76）+ 解析侧（P78）+ 白名单侧（P79）+
/// 复合侧（P82）** 四道门槛；UI 面（工具条分享按钮 + provider 派发）留下一轮。
pub fn startup_share_pipeline_banner() -> String {
    let o = output_cfg();
    let (known, p_refused, u_count) = pipeline_share_targets(&o.share_targets);
    format!(
        "pipeline final={} parse_refused={} unknown={}",
        known.len(),
        p_refused,
        u_count
    )
}

/// P83：§5.5.12 `send_to_apps` 通过校验的条目自此可读成一串 UI 显示 ——
/// 把 `(name, exe)` 对映射成 `"{name} ({exe_stem})"`：exe 剥掉最后一段
/// `.exe` / `.cmd` / `.bat` 扩展名（大小写不敏感匹配、原大小写保留），
/// name 原样保留。分派侧（真 `Command::new(exe).arg(path).spawn()`）仍未建。
///
/// 纯函数：不落盘 / 不开窗 / 不 dispatch。
pub fn send_to_menu_entries(parsed_ok: &[(String, String)]) -> Vec<String> {
    parsed_ok
        .iter()
        .map(|(name, exe)| {
            let lower = exe.to_lowercase();
            let stem = [".exe", ".cmd", ".bat"]
                .iter()
                .find(|e| lower.ends_with(**e))
                .map(|e| &exe[..exe.len() - e.len()])
                .unwrap_or(exe.as_str());
            format!("{} ({})", name, stem)
        })
        .collect()
}

/// P83：[`send_to_menu_entries`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍
/// [`pipeline_send_to`]、把通过校验的条目映射成 UI 显示串、条数回读成一行由
/// `main.rs` 打印。至此 §5.5.12 那批条目跨过 **读侧（P76）+ 解析侧（P77）+
/// 校验侧（P80）+ 复合侧（P81）+ 显示侧（P83）** 五道门槛；真 spawn 留下一轮。
pub fn startup_send_to_menu_banner() -> String {
    let o = output_cfg();
    let (final_ok, _, _) = pipeline_send_to(&o.send_to_apps);
    let entries = send_to_menu_entries(&final_ok);
    format!("menu entries={}", entries.len())
}

/// P84：§5.5.11 `share_targets` 过白名单那批条目自此可读成一串 UI 显示 ——
/// 每条首字母大写、其余原样保留（"slack" → "Slack"、"TEAMS" → "TEAMS"）。
/// 分派侧（真 provider 派发）仍未建。
///
/// 纯函数：不落盘 / 不开窗 / 不 dispatch。
pub fn share_menu_entries(known: &[String]) -> Vec<String> {
    known
        .iter()
        .map(|s| match s.chars().next() {
            Some(first) => {
                let up: String = first.to_uppercase().collect();
                up + &s[first.len_utf8()..]
            }
            None => String::new(),
        })
        .collect()
}

/// P84：[`share_menu_entries`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍
/// [`pipeline_share_targets`]、把 known 列表映射成 UI 显示串、条数回读成一行由
/// `main.rs` 打印。至此 §5.5.11 那批条目跨过 **读侧（P76）+ 解析侧（P78）+
/// 白名单侧（P79）+ 复合侧（P82）+ 显示侧（P84）** 五道门槛；真 provider 派发
/// 与工具条按钮留下一轮。
pub fn startup_share_menu_banner() -> String {
    let o = output_cfg();
    let (known, _, _) = pipeline_share_targets(&o.share_targets);
    let entries = share_menu_entries(&known);
    format!("menu entries={}", entries.len())
}

/// P85：§5.5.12 `send_to_apps` 那批条目自此可按显示名查回 exe —— 走一遍
/// [`pipeline_send_to`]、按大小写不敏感匹配找 name、返回 exe（原大小写保留）。
/// 未命中 ⇒ `None`；被 parse 拒 / 被 validate 拒的条目也 ⇒ `None`。
///
/// 纯函数：不落盘 / 不开窗 / 不 `Command::spawn()`。
pub fn resolve_send_to(entries: &[String], name: &str) -> Option<String> {
    let (final_ok, _, _) = pipeline_send_to(entries);
    final_ok
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, exe)| exe)
}

/// P85：[`resolve_send_to`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍流水线，
/// 拿首条通过的 name 反查 exe、比对是否同一枚 ⇒ 一行 selftest 冒烟。空配置
/// 一侧走 `resolve (empty)`。至此 §5.5.12 那批条目跨过 **读侧（P76）+ 解析侧
/// （P77）+ 校验侧（P80）+ 复合侧（P81）+ 显示侧（P83）+ 反查侧（P85）** 六
/// 道门槛；真 spawn 与工具条留下一轮。
pub fn startup_resolve_send_to_banner() -> String {
    let o = output_cfg();
    let (final_ok, _, _) = pipeline_send_to(&o.send_to_apps);
    if let Some((first_name, first_exe)) = final_ok.first() {
        let resolved = resolve_send_to(&o.send_to_apps, first_name);
        match resolved {
            Some(exe) if exe == *first_exe => format!("resolve ok first={}", first_name),
            _ => format!("resolve MISS first={}", first_name),
        }
    } else {
        "resolve (empty)".to_string()
    }
}

/// P86：§5.5.11 provider 派发前的 URL scheme 拼装。纯函数、无 side effect。
///
/// 走一遍 P82 `pipeline_share_targets` 输出的 `known` 一侧（大小写不敏感命中内置 5 枚
/// slack / teams / wecom / dingtalk / feishu），按 target 名字映射到一条 URL scheme 串。
/// 未在白名单或 `path` 为空 ⇒ `None`。
///
/// **本轮只到"拼串"这一步** —— 不 ShellExecute、不打开浏览器、不 `Command::spawn()`、
/// 不 touch 用户会话；会话锁状态未知 ⇒ provider 真派发留下一轮。
pub fn share_scheme_for(target: &str, path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let t = target.to_lowercase();
    Some(match t.as_str() {
        "slack" => format!("slack://share?file={}", path),
        "teams" => format!("msteams://share?path={}", path),
        "wecom" => format!("wecom://send?file={}", path),
        "dingtalk" => format!("dingtalk://share?path={}", path),
        "feishu" => format!("feishu://send?file={}", path),
        _ => return None,
    })
}

/// P86：[`share_scheme_for`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍 P82 流水线、
/// 拿首条 known target + 哨兵 path 冒烟，回读成一行 banner。
pub fn startup_share_scheme_banner() -> String {
    let o = output_cfg();
    let (known, _, _) = pipeline_share_targets(&o.share_targets);
    match known.first() {
        Some(t) => {
            let sentinel = "C:\\placeholder.png";
            match share_scheme_for(t, sentinel) {
                Some(url) => format!("scheme {}={}", t, url),
                None => format!("scheme NO-TEMPLATE target={}", t),
            }
        }
        None => "scheme (empty)".to_string(),
    }
}

/// P87：§5.5.12 send-to-app 前的 argv 拼装。纯函数、无 side effect。
///
/// 走一遍 P85 [`resolve_send_to`] 拿到 exe，返回 `vec![exe, path]` argv 数组（模拟
/// `Command::new(exe).arg(path)` 的参数、**不 spawn**）。name 未命中 / 被 parse 拒 /
/// 被 validate 拒 ⇒ `None`；`path` 为空 ⇒ `None`。
///
/// **本轮只到"拼 argv"这一步** —— 不 `Command::spawn()`、不 touch 用户会话；会话锁状态
/// 未知 ⇒ 真 spawn 留下一轮。
pub fn send_to_exec_argv(entries: &[String], name: &str, path: &str) -> Option<Vec<String>> {
    if path.is_empty() {
        return None;
    }
    resolve_send_to(entries, name).map(|exe| vec![exe, path.to_string()])
}

/// P87：[`send_to_exec_argv`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍 P81 流水线拿首条
/// 通过项 name + 哨兵 path，回读成一行 banner。
pub fn startup_send_to_exec_argv_banner() -> String {
    let o = output_cfg();
    let (final_ok, _, _) = pipeline_send_to(&o.send_to_apps);
    match final_ok.first() {
        Some((name, _)) => {
            let sentinel = "C:\\snapshot.png";
            match send_to_exec_argv(&o.send_to_apps, name, sentinel) {
                Some(argv) => format!("argv {}={} slots", name, argv.len()),
                None => format!("argv MISS name={}", name),
            }
        }
        None => "argv (empty)".to_string(),
    }
}

/// P88：§5.5.11 known 一侧**全体** scheme 拼装（P86 的复数版）。纯函数、无 side effect。
///
/// 把 P82 pipeline 输出的 `known` 一侧逐条走 P86 [`share_scheme_for`]，返回 `Vec<(target, url)>`
/// 列表（跳过 None），保持顺序、保留 target 原大小写。空 `path`  ⇒ 全体 None ⇒ 空列表。
pub fn share_scheme_for_all(known: &[String], path: &str) -> Vec<(String, String)> {
    known
        .iter()
        .filter_map(|t| share_scheme_for(t, path).map(|url| (t.clone(), url)))
        .collect()
}

/// P88：[`share_scheme_for_all`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍 P82 流水线拿 known
/// 一侧 + 哨兵 path，回读成一行 banner。
pub fn startup_share_scheme_all_banner() -> String {
    let o = output_cfg();
    let (known, _, _) = pipeline_share_targets(&o.share_targets);
    let sentinel = "C:\\shot.png";
    let entries = share_scheme_for_all(&known, sentinel);
    format!("scheme_all {} entries", entries.len())
}

/// P89：§5.5.12 send-to-app 前的**自动首条** argv 拼装。纯函数、无 side effect。
///
/// 走一遍 P81 [`pipeline_send_to`] 拿 final_ok、取首条通过项的 exe、返回 `vec![exe, path]`
/// argv 数组（模拟 `Command::new(exe).arg(path)` 的参数、**不 spawn**）。与 P87
/// [`send_to_exec_argv`] 的差别：**name 参数消失** ⇒ 由流水线自动决定首条 ⇒ 用户点击
/// "Send to ▸" 时不必展开子菜单即可一步走。final_ok 为空 / `path` 为空 ⇒ `None`。
///
/// **本轮只到"自动取首条 + 拼 argv"这一步** —— 不 `Command::spawn()`、不 touch 用户会话；
/// 会话锁状态未知 ⇒ 真 spawn 留下一轮。
pub fn send_to_exec_argv_first(entries: &[String], path: &str) -> Option<Vec<String>> {
    if path.is_empty() {
        return None;
    }
    let (final_ok, _, _) = pipeline_send_to(entries);
    final_ok
        .first()
        .map(|(_name, exe)| vec![exe.clone(), path.to_string()])
}

/// P89：[`send_to_exec_argv_first`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍、
/// 哨兵 path `C:\\auto.png`、回读 `argv_first N slots` / `argv_first (empty)` 两态。
pub fn startup_send_to_exec_argv_first_banner() -> String {
    let o = output_cfg();
    let sentinel = "C:\\auto.png";
    match send_to_exec_argv_first(&o.send_to_apps, sentinel) {
        Some(argv) => format!("argv_first {} slots", argv.len()),
        None => "argv_first (empty)".to_string(),
    }
}

/// P90：§5.5.11 known 一侧的**自动首条** provider scheme（P86 的 name-less 版、与 P89 对称）。
///
/// 逐条走 P86 [`share_scheme_for`]，返回第一条命中模板的 `(target, url)`；全体未注册或
/// 空 `path` ⇒ `None`。target 原大小写保留（与 P88 [`share_scheme_for_all`] 同一族）。
///
/// **本轮 dispatch 只到"挑首条"这一步** —— 不 ShellExecute、不打开浏览器、不 touch
/// 用户会话；真派发留下一轮。
pub fn share_scheme_first(known: &[String], path: &str) -> Option<(String, String)> {
    known
        .iter()
        .find_map(|t| share_scheme_for(t, path).map(|url| (t.clone(), url)))
}

/// P90：[`share_scheme_first`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍 P82 流水线拿 known
/// 一侧 + 哨兵 path `C:\\first.png`，回读 `scheme_first NAME=url` / `scheme_first (empty)` 两态。
pub fn startup_share_scheme_first_banner() -> String {
    let o = output_cfg();
    let (known, _, _) = pipeline_share_targets(&o.share_targets);
    let sentinel = "C:\\first.png";
    match share_scheme_first(&known, sentinel) {
        Some((name, url)) => format!("scheme_first {}={}", name, url),
        None => "scheme_first (empty)".to_string(),
    }
}

/// P91：§5.5.12 send-to 显示条目跨过 Rust→QML 边界的那道 join ——
/// 把 [`send_to_menu_entries`] 输出的 `Vec<String>` 用 `|` 联成一枚纯 `String`，
/// 与 [`crate::annotate::tool_names`] 同规（QML 侧 `.split("|")` 即可喂 Repeater）。
/// 空集合 ⇒ 空串。
///
/// 分岔本体是纯函数：输入是 P81 [`pipeline_send_to`] 已经校验过的那一段
/// `&[(String, String)]`；不读 config、不落盘、不开窗、不 spawn。调用点在
/// [`crate::mask_view`] 的 `#[qinvokable] fn send_to_menu_names` —— QML 层第一次读到
/// 这条清单。
pub fn send_to_menu_names_joined(parsed_ok: &[(String, String)]) -> String {
    send_to_menu_entries(parsed_ok).join("|")
}

/// P91：[`send_to_menu_names_joined`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍 P81
/// [`pipeline_send_to`] 拿到校验通过的条目，join 出串、回读条数与前 40 字节预览，
/// 形如 `names 2 [Notepad (notepad)|Runner (Run)]`。空集合 ⇒ `names 0 []`。
pub fn startup_send_to_menu_names_banner() -> String {
    let o = output_cfg();
    let (final_ok, _, _) = pipeline_send_to(&o.send_to_apps);
    let joined = send_to_menu_names_joined(&final_ok);
    let count = if joined.is_empty() {
        0
    } else {
        joined.split('|').count()
    };
    let preview: String = joined.chars().take(40).collect();
    format!("names {} [{}]", count, preview)
}

/// P92：§5.5.11 share 显示条目跨过 Rust→QML 边界的那道 join —— 把
/// [`share_menu_entries`] 输出的 `Vec<String>` 用 `|` 联成一枚纯 `String`，与 P91
/// [`send_to_menu_names_joined`]、[`crate::annotate::tool_names`] 同规（QML 侧
/// `.split("|")` 即可喂 Repeater）。空集合 ⇒ 空串。
///
/// 分岔本体是纯函数：输入是 P82 [`pipeline_share_targets`] 已经过白名单的那一段
/// `&[String]`；不读 config、不落盘、不开窗、不 ShellExecute。调用点在
/// [`crate::mask_view`] 的 `#[qinvokable] fn share_menu_names` —— share 一侧自此与
/// P91 的 send-to 一侧对称。
pub fn share_menu_names_joined(known: &[String]) -> String {
    share_menu_entries(known).join("|")
}

/// P92：[`share_menu_names_joined`] 挂上产品路径 —— 从 [`output_cfg`] 走一遍 P82
/// [`pipeline_share_targets`] 拿到白名单内的 known 列表，join 出串、回读条数与前
/// 40 字节预览，形如 `names 2 [Slack|TEAMS]`。空集合 ⇒ `names 0 []`。
pub fn startup_share_menu_names_banner() -> String {
    let o = output_cfg();
    let (known, _, _) = pipeline_share_targets(&o.share_targets);
    let joined = share_menu_names_joined(&known);
    let count = if joined.is_empty() {
        0
    } else {
        joined.split('|').count()
    };
    let preview: String = joined.chars().take(40).collect();
    format!("names {} [{}]", count, preview)
}

/// P75：`history_open_in` 的第一位非 test 调用点。
///
/// 从 `--selftest --history` 那一档进来时调一次；dir 从整段 config 里的 `advanced.data_dir`
/// 派生（空 ⇒ `%APPDATA%\ai-falconshot\`、非空 ⇒ 用户填的绝对路径，与 [`falcon_core::config::
/// config_path`] 同一族）；[`history_cfg`] 决定走哪一臂：Enabled ⇒ 真调 `History::open(dir)`
/// 让 disk 长出 `history.db`、Disabled ⇒ `Ok(None)` 不动 disk。
///
/// **本轮的分岔本体在 `history_open_in` 里已经建好**（P68 那一族）：`cfg.enabled` 一侧开
/// SQLite、另一侧 `Ok(None)`；本轮的 cargo 单测证人不在这一族而在 `main.rs` 的 refuse 规则
/// （`--history` 单独出现 ⇒ 必须 refuse 且带 `--selftest` 字样），是 P72 `banner_names_platform_default_only_when_data_dir_is_empty`
/// 那一族的第四次；证人 dichotomy 的第 69 项 ⑫ 本轮**换回 P67 / P68 / P72 那一侧**（读者-only
/// 无分岔一侧自 P75 起再次不适用，本轮是 P73/P74 两轮"读者-only"之后再回来）。
pub fn startup_history_probe(
) -> Result<Option<falcon_core::history::History>, falcon_core::history::HistoryError> {
    let dir = with(|s| falcon_core::config::data_dir(&s.config));
    history_open_in(&dir, &history_cfg())
}

/// The directory [`startup_history_probe`] resolves to without opening it, so the
/// `--history` verdict line can name where a `history.db` would land.
pub fn history_data_dir() -> std::path::PathBuf {
    with(|s| falcon_core::config::data_dir(&s.config))
}

/// P98：§5.14.2 step 1「打开历史记录」那张清单跨过 Rust→QML 边界的那道 join —— 把
/// [`falcon_core::history::Record`] 逐行写成界面能直接喂 `Repeater` 的形状，再用 `|`
/// 联成一枚纯 `String`，与 [`send_to_menu_names_joined`]、[`crate::annotate::tool_names`]
/// 同规（QML 侧 `.split("|")` 即可）。空清单 ⇒ 空串。
///
/// `|` 是**行与行之间**唯一的主人，所以一行之内不许再出现一枚：`monitor_id` 是从库里读回
/// 来的自由文本（§6.2 那一列），它自己带不带 `|` 本程序说不上，故这一族里它是唯一被洗一遍
/// 的字段。行内字段用 ` · ` 分节，第一段是**裸行号** —— 界面那一头要点某一行重放时，它得
/// 从这一串里拿回 SQLite 的那个 id，而不是从一句中文里猜数字。
///
/// 分岔本体是纯函数：输入是已经取回的那些行加一枚分钟偏移，不读 config、不开 SQLite、
/// 不落盘、不开窗。时间偏移是**调用方交进来的**而不是这里读的：`captured_at` 是 epoch 秒，
/// 而"这一行写于几点"要的是那一台机器当时的偏移，一处读时钟就多了第二个主人。
pub fn history_rows_joined(records: &[falcon_core::history::Record], offset_min: i32) -> String {
    records
        .iter()
        .map(|rec| {
            let stamp =
                falcon_core::naming::LocalStamp::from_epoch(rec.captured_at / 1_000, offset_min);
            let mut label = format!(
                "{} · {:04}-{:02}-{:02} {:02}:{:02} · {}×{} · {} · {}",
                rec.id,
                stamp.year,
                stamp.month,
                stamp.day,
                stamp.hour,
                stamp.min,
                rec.rect.w,
                rec.rect.h,
                rec.monitor_id.replace('|', "/"),
                rec.source.as_str(),
            );
            // 「已取消」与「已锁定」各说一件事，而 §5.14.3 的「找到标记为已取消的记录」要的
            // 就是这一枚字样在清单里读得到；两枚都不带时那一行什么都不多写。
            if rec.cancelled {
                label.push_str(" · 已取消");
            }
            if rec.locked {
                label.push_str(" · 已锁定");
            }
            label
        })
        .collect::<Vec<String>>()
        .join("|")
}

/// [`history_rows_joined`] 挂上产品路径的那一臂：[`history_open_in`] 的分岔开出去 ⇒
/// [`falcon_core::history::History::newest`] 取最新 `limit` 行 ⇒ join。
/// **P98 起本函数是 `newest` 的第一位非 test 调用点**（P95 普查点名的那 11 枚 still-zero
/// 里的一枚，§10 第 98 项）。
///
/// 答法与 [`crate::mask::MaskState::replay_history`] 同一口径：`Ok(空串)` 是「库里没有行」
/// 这一次正常的空，`Err(那一句)` 是「历史没开」或「库打不开」或「那些行读不回」——把三种
/// 都说成空，等于让一枚坏掉的 `history.db` 读成一次正常清空。行数的**次序**也不在这里重定：
/// `newest` 自己按 `(captured_at, id)` 倒序，join 不排序、不去重，所以界面看到的就是库里
/// 排出来的那一份。
pub fn history_rows_at(
    dir: &std::path::Path,
    cfg: &falcon_core::config::History,
    limit: usize,
    offset_min: i32,
) -> Result<String, String> {
    match history_open_in(dir, cfg) {
        Ok(Some(store)) => store
            .newest(limit)
            .map(|records| history_rows_joined(&records, offset_min))
            .map_err(|e| format!("{} 里那些行读不回（{e}）", dir.display())),
        Ok(None) => Err(format!(
            "{} 里的历史没开（`[history] enabled` 是关的）",
            dir.display()
        )),
        Err(e) => Err(format!("打不开 {} 里的 history.db（{e}）", dir.display())),
    }
}

/// P110：§6.2「显示历史记录占用多少空间」那四枚数的**分岔本体** —— 记录数、原图字节、
/// 缩略图字节、库本身字节进、一行 `String` 出。纯函数：不读 config、不开 SQLite、不落盘、
/// 不开窗。合计与 `falcon_core::history::Usage::total_bytes()` 同一道算术（原图 + 缩略图 +
/// 库），本函数不叫 `.total_bytes()`，因为它的输入是四枚裸 `u64`、而不是那一枚 struct。
///
/// 本轮是「读者-only 无分岔」与「分岔本体是纯函数」两族里后那一族（P67 `tray_from` /
/// P68 `history_open_in` / P72 `format_startup_banner` / P76 `format_output_banner` /
/// P98 `history_rows_joined` 那一族） ⇒ 独立 cargo 单测两枚证人（见 tests 模块）。
pub fn history_usage_line(
    records: u64,
    original_bytes: u64,
    thumb_bytes: u64,
    db_bytes: u64,
) -> String {
    let total = original_bytes + thumb_bytes + db_bytes;
    format!(
        "history records={} original={}B thumb={}B db={}B total={}B",
        records, original_bytes, thumb_bytes, db_bytes, total
    )
}

/// P110：[`history_usage_line`] 挂上产品路径 —— `History::usage()` 的第一位非 test 调用点。
///
/// 与 [`startup_history_probe`]（P75）同一族：dir 从整段 config 里的 `advanced.data_dir`
/// 派生、[`history_cfg`] 决定走哪一臂。三臂各有一行返回：开着一臂 ⇒ 真问 SQLite、
/// 关着一臂 ⇒ 说「开关关着」、开不上（IO / Schema / SQLite 拒）⇒ 说「打不开」。**降级
/// 为一行说明而不是让启动 banner 整段丢一行**，因为 §6.2 那句"show what the history is
/// using"要的是这一格在**每一条启动**上都印得出东西，而不是在开关关着时静默。
///
/// `#[allow(dead_code)]` 挂着与否自本轮起都不适用：本函数有非 test 调用点（`main.rs` 的
/// `[history usage]` 那一行 println），P64 pin / P65 capture / P66 general / P72 history +
/// advanced / P73 appearance / P74 about / P75 history_open_in / P76 output / P92 share
/// names / **P110 usage** 这一族自此第一位读者与第一位**产品路径**读者同时到齐。
pub fn startup_history_usage_banner() -> String {
    let dir = history_data_dir();
    let cfg = history_cfg();
    match history_open_in(&dir, &cfg) {
        Ok(Some(store)) => match store.usage() {
            Ok(u) => history_usage_line(u.records, u.original_bytes, u.thumb_bytes, u.db_bytes),
            Err(e) => {
                format!("history records=? original=?B thumb=?B db=?B total=?B (usage 读不回: {e})")
            }
        },
        Ok(None) => "history records=0 original=0B thumb=0B db=0B total=0B (开关关着)".to_string(),
        Err(e) => format!(
            "history records=? original=?B thumb=?B db=?B total=?B (打不开 history.db: {e})"
        ),
    }
}

/// P111：§5.14.2「按上下方向键在历史里前后翻」那一句里 prev/next 两条腿的**分岔本体**
/// —— 纯函数，把三枚可选行号拼成一行短串。`None` 一律印 `-`，让读的人一眼看出是「没有邻居」
/// 而不是「读不到」。三臂都印得出东西：中间行 ⇒ 两边都有号；最旧一行 ⇒ 只 next；新一行 ⇒ 只 prev；
/// 库里就一行 ⇒ 两边都 `-`。
///
/// 与 P110 的 [`history_usage_line`] 同一族：分岔本体是纯函数 ⇒ 独立 cargo 单测；三臂各有一行
/// 返回：开着 / 关着 / 开不上，都在启动 banner 上印得出东西。
pub fn history_neighbors_line(anchor: i64, prev: Option<i64>, next: Option<i64>) -> String {
    let dash = |id: Option<i64>| match id {
        Some(v) => v.to_string(),
        None => "-".to_string(),
    };
    format!(
        "history anchor={} prev={} next={}",
        anchor,
        dash(prev),
        dash(next)
    )
}

pub fn startup_history_neighbors_banner() -> String {
    let dir = history_data_dir();
    let cfg = history_cfg();
    neighbors_at_banner(&dir, &cfg)
}

/// P112：§5.14.2 那两条 prev/next 腿的**界面读法**（Session 的 `historyNeighborsAt`
/// qinvokable 自此有了产品侧调用者；P111 那一条 startup banner 走的是「最新那一行」这一枚
/// anchor，界面上当用户把选区挪到别的行时要问这一枚行自己的邻居）。
///
/// 与 [`startup_history_neighbors_banner`] 共用同一个 body，只是把「拿库里最新那一行做
/// anchor」换成「caller 递进来的 dir/cfg 里的最新那一行」—— 两枚 caller 走的**是同一条
/// 判据**（`newest(1)` + `prev(id)` + `next(id)`），差别只在 dir/cfg 从哪来。分岔本体
/// [`history_neighbors_line`] 因此继续是**纯函数**，被两侧同用。
pub fn neighbors_at_banner(dir: &std::path::Path, cfg: &falcon_core::config::History) -> String {
    match history_open_in(dir, cfg) {
        Ok(Some(store)) => match store.newest(1) {
            Ok(rows) => match rows.first() {
                Some(top) => {
                    let prev = match store.prev(top.id) {
                        Ok(r) => r.map(|rec| rec.id),
                        Err(e) => {
                            return format!(
                                "history anchor={} prev=? next=? (上一条读不回: {e})",
                                top.id
                            );
                        }
                    };
                    let next = match store.next(top.id) {
                        Ok(r) => r.map(|rec| rec.id),
                        Err(e) => {
                            return format!(
                                "history anchor={} prev=? next=? (下一条读不回: {e})",
                                top.id
                            );
                        }
                    };
                    history_neighbors_line(top.id, prev, next)
                }
                None => "history anchor=- prev=- next=- (库里一行也没有)".to_string(),
            },
            Err(e) => format!("history anchor=? prev=? next=? (newest 读不回: {e})"),
        },
        Ok(None) => "history anchor=- prev=- next=- (开关关着)".to_string(),
        Err(e) => format!("history anchor=? prev=? next=? (打不开 history.db: {e})"),
    }
}

/// P112：§5.14.2 那两条 prev/next 腿的**界面读法**里「anchor 由 caller 指定」那一支 ——
/// Session 的 `#[qinvokable] history_neighbors_at(id)` 递进来的 `id` 走的就是这一条。与
/// [`neighbors_at_banner`] 的差别只在 anchor 来源：那一枚读库里最新那一行、这一枚按 id
/// 定位。**三臂**照旧：开着 ⇒ 按 id 查邻居；库里没这一行 ⇒ 直说没有；关着 / 打不开 ⇒
/// 与 [`neighbors_at_banner`] 一字同。
pub fn neighbors_of_banner_for(
    dir: &std::path::Path,
    cfg: &falcon_core::config::History,
    anchor: i64,
) -> String {
    match history_open_in(dir, cfg) {
        Ok(Some(store)) => {
            let exists = match store.get(anchor) {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(e) => {
                    return format!(
                        "history anchor={anchor} prev=? next=? (第 {anchor} 行读不回来: {e})"
                    );
                }
            };
            if !exists {
                return format!("history anchor={anchor} prev=- next=- (库里没有第 {anchor} 行)");
            }
            let prev = match store.prev(anchor) {
                Ok(r) => r.map(|rec| rec.id),
                Err(e) => {
                    return format!("history anchor={anchor} prev=? next=? (上一条读不回: {e})");
                }
            };
            let next = match store.next(anchor) {
                Ok(r) => r.map(|rec| rec.id),
                Err(e) => {
                    return format!("history anchor={anchor} prev=? next=? (下一条读不回: {e})");
                }
            };
            history_neighbors_line(anchor, prev, next)
        }
        Ok(None) => "history anchor=- prev=- next=- (开关关着)".to_string(),
        Err(e) => format!("history anchor=? prev=? next=? (打不开 history.db: {e})"),
    }
}

/// §5.14.2 step 2「使用上一条、下一条快捷键浏览记录」里**只回答一个号**的那一支：给定锚与
/// 方向，交回该翻去的那一行的 `id`，翻不动就交回 `None`。
///
/// 它与 [`neighbors_of_banner_for`] 同库同锚而**不共用返回形状**，理由是两问不同：那一枚问
/// 「这一行的前后各是谁」，答的是一句给人看的话（三臂各有措辞，坏库与没这一行分得开）；这一枚
/// 问「按下方向键该去哪一行」，答的是一个可以直接交给
/// [`crate::mask::MaskState::restore_history_selection`] 的号。把话拿来解析成号，等于让界面
/// 依赖措辞；把三臂压成 `None`，等于让「库坏了」和「到顶了」说同一句话 —— 所以两条并存，而
/// **开库那一步仍然只有一处**（[`history_open_in`]），这一枚不另立库的口径。
///
/// `None` 的四格合一（关着 / 打不开 / 库里没这一锚 / 那一侧到头了）是**有意的**：调用方拿到的
/// 都是「这一趟没得翻」，而那四格的分别是 [`neighbors_of_banner_for`] 那一枚负责说的，界面上
/// 那句话由 Session 从它取，不由这一枚编。
pub fn neighbor_id_for(
    dir: &std::path::Path,
    cfg: &falcon_core::config::History,
    anchor: i64,
    back: bool,
) -> Option<i64> {
    let store = match history_open_in(dir, cfg) {
        Ok(Some(store)) => store,
        Ok(None) => return None,
        Err(_) => return None,
    };
    match store.get(anchor) {
        Ok(Some(_)) => {}
        _ => return None,
    }
    let side = if back {
        store.prev(anchor)
    } else {
        store.next(anchor)
    };
    match side {
        Ok(Some(rec)) => Some(rec.id),
        _ => None,
    }
}

/// The flow is over: keep what the layer learned, and write if that moved anything.
pub fn write_back(layer: &Layer) -> Option<u64> {
    with(|s| s.write_back(layer))
}

/// Send this process's later writes somewhere else, and say where.
///
/// The `--ink` leg needs it. That gauge teaches the pens it is about to measure -
/// `select_tool`, `set_color`, the widths - and then ends the flow the way the product
/// does, which reaches [`Settings::write_back`]. Left alone, the run that checks the
/// green 矩形 leaves the user's own `config.toml` holding a green 矩形: §5.7.1 promises
/// that file the *user's* last pen, and §9.1 第十五条 is exactly this debt - a gauge
/// that walks the product's door takes the product's side effect with it.
///
/// See [`Settings::pin_writes`] for what does and does not move.
pub fn pin_writes(path: &Path) -> PathBuf {
    with(|s| s.pin_writes(path))
}

// --------------------------------------------------------------------- §5.20.3

/// §5.20.3 步骤 4 的那一句:一份档会盖掉哪些项,一项一行,带上 §5.20.2 要的「需重启」标记,
/// 再带上 [`falcon_core::config::Config::parse_import`] 不得不做的校正。
///
/// 两半都写得出「零」这一行。空手回来和没去看,在日志里必须长得不一样:导入一份与本机
/// 完全相同的档,`--confirm` 加与不加退出码都是 0,可那一行得说清是没得改。
pub fn render_import_preview(
    changes: &[ImportChange],
    warnings: &[String],
    scope: ImportScope,
) -> String {
    let scope_name = match scope {
        ImportScope::All => "全部",
        ImportScope::Suggested => "建议范围",
    };
    let mut out = format!(
        "[import] 范围={scope_name}；将被覆盖 {} 项；被校正 {} 项\n",
        changes.len(),
        warnings.len()
    );
    for c in changes {
        // 注释挂在行尾而不是单起一行:一行一项,读的人才能拿行数对上上面那个数。
        let restart = if c.restart_required {
            "（需重启）"
        } else {
            ""
        };
        out.push_str(&format!(
            "  覆盖 {} : {} -> {}{restart}\n",
            c.key, c.from, c.to
        ));
    }
    for w in warnings {
        out.push_str(&format!("  校正 {w}\n"));
    }
    if changes.is_empty() {
        out.push_str("  （没有将被覆盖的项）\n");
    }
    if warnings.is_empty() {
        out.push_str("  （没有被校正的项）\n");
    }
    out
}

/// §5.20.3 步骤 2:把这一台机器正在跑的 settings 写成一份可带走的档,返回它写了几枚键。
///
/// 走 [`falcon_core::config::Config::export_text`] 而不是 [`Settings::save`]:save 会把
/// 目标文件已有的注释与排版留住,那是「改自己这一份」的行为;导出给另一台机器的档不该
/// 继承这一台的注释。键数取 [`falcon_core::config::Config::flat`] 的长度,于是那句
/// 「写出 N 枚键」是盘上那份文件的读数,不是设置页自报的枚数。
pub fn config_export(path: &Path) -> Result<String, String> {
    let (text, keys) = with(|s| {
        let cfg = &s.config;
        cfg.export_text()
            .and_then(|t| cfg.flat().map(|f| (t, f.len())))
            .map_err(|e| e.to_string())
    })?;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    std::fs::write(path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("[export] 写出 {keys} 枚键到 {}\n", path.display()))
}

/// §5.20.3 步骤 4 与步骤 5:读一份档、校验它、报出会盖掉哪些项;只有 `confirm` 才真写。
///
/// `confirm` 就是那一节里的「用户确认后应用」。没确认时除了预览那句,还专门补一行说
/// 未应用 —— 退出码 0 在这一支是「档读得通」的意思,不是「已经改了」。
pub fn config_import(path: &Path, scope: ImportScope, confirm: bool) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (incoming, warnings) = Config::parse_import(&text).map_err(|e| e.to_string())?;
    let changes = with(|s| s.config.preview_import(&incoming, scope));
    let mut out = render_import_preview(&changes, &warnings, scope);
    if !confirm {
        out.push_str("[import] 未应用：加 --confirm 才会写入（§5.20.3 步骤 5）\n");
        return Ok(out);
    }
    let applied = changes.len();
    // [`Settings::save`] 把写盘失败记进 `problems` 而自己只返回耗时:内存里的值照改。
    // 那一手对设置页是对的(§9.4),对 CLI 不行 —— PRD §5.17 要的是「文件写失败不得报告
    // 成功」。所以这里看的是 save 前后 `problems` 长没长,长了就当 Err 报出去。
    let outcome = with(|s| -> Result<u64, String> {
        s.config
            .apply_import(&incoming, scope)
            .map_err(|e| e.to_string())?;
        let before = s.problems.len();
        let ms = s.save();
        if s.problems.len() > before {
            Err(s.problems[before].clone())
        } else {
            Ok(ms)
        }
    })?;
    out.push_str(&format!(
        "[import] 已应用 {applied} 项，写盘耗时 {outcome} ms（§5.20.3 步骤 5）\n"
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use falcon_core::annotation::model::Kind;
    use falcon_core::config::FILE_NAME;
    use falcon_core::encode::Format;

    /// A directory under the OS temp dir, one per test. `%APPDATA%` is not a test
    /// fixture, and none of these reach it: every `Settings` here is opened against
    /// an explicit path rather than through the process global.
    fn sandbox(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-settings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    fn file_in(dir: &Path) -> PathBuf {
        dir.join(FILE_NAME)
    }

    /// The only way a test here may open settings that write.
    ///
    /// `None` for the pin is not "stay in this directory" - it is "ask the file where
    /// its data lives", and an empty `advanced.data_dir` answers with the real
    /// `%APPDATA%\ai-falconshot\config.toml`. The first draft of these tests made that
    /// mistake three times, and the writes landed there: a sandbox directory in the
    /// assertion text is not a sandbox.
    fn pinned(dir: &Path) -> Settings {
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "settings tests may only write under the temp dir, got {dir:?}"
        );
        let path = file_in(dir);
        Settings::open(&path, Some(&path))
    }

    fn code_of(kind: Kind) -> i32 {
        crate::annotate::TOOLS
            .iter()
            .position(|t| *t == Some(kind))
            .expect("tool has a button") as i32
    }

    /// Teach one tool a pen the way the toolbar does: pick the button, then set the
    /// knobs. A pen set with no tool selected is forgotten by the layer (§5.7.1).
    fn teach(layer: &mut Layer, kind: Kind, rgba: [u8; 4], width: u32) {
        layer.select_tool(code_of(kind));
        layer.set_color(rgba);
        layer.set_width(width);
    }

    /// §5.5.5's two export knobs, out of the file rather than out of a default.
    ///
    /// A save that ignored `format` would write PNG under a user who set BMP, and one
    /// that ignored `jpeg_quality` would write quality 92 under a user who set 40; both
    /// are silent, which is why they are asserted here rather than left to a gauge that
    /// reads the file back and finds a picture either way. The two clamps are the other
    /// half: a `Config` built in memory has not been sanitised, and the encoder's `u8`
    /// would wrap 1000 into 232 without this line.
    #[test]
    fn export_knobs_come_from_the_file() {
        let dir = sandbox("export");
        let path = file_in(&dir);
        let mut cfg = Config::default();
        cfg.output.format = Format::Bmp;
        cfg.output.jpeg_quality = 40;
        std::fs::write(&path, cfg.export_text().unwrap()).expect("seed");
        let s = Settings::open(&path, Some(&path));
        let opt = s.export_opts();
        assert_eq!(opt.format, Format::Bmp);
        assert_eq!(opt.quality, 40);

        let mut raw = pinned(&dir);
        raw.config.output.jpeg_quality = 1_000;
        assert_eq!(raw.export_opts().quality, 100);
        raw.config.output.jpeg_quality = 0;
        assert_eq!(raw.export_opts().quality, 1);
    }

    /// §5.5.4 step 1's 预先配置, read out of a real file rather than out of a default - the
    /// same claim [`export_knobs_come_from_the_file`] makes about the two encoder knobs.
    ///
    /// A door that ignored `save_dir` would write into the pictures folder while the user's
    /// settings said somewhere else, and one that ignored `naming_template` or
    /// `naming_prefix` would answer a named rule with a timestamped one. All three are
    /// silent, and all three are why they are asserted against the bytes on disk here: the
    /// gauge's version of this row can only read a name that already landed, so a door that
    /// invented the name it wrote would satisfy it.
    ///
    /// The empty `save_dir` leg is the other half of §6.2: empty means the pictures folder,
    /// and a door that read it as a literal directory would write into whatever folder the
    /// process started in.
    #[test]
    fn naming_rows_come_from_the_file() {
        let dir = sandbox("naming");
        let path = file_in(&dir);
        let mut cfg = Config::default();
        cfg.output.save_dir = dir.to_string_lossy().into_owned();
        cfg.output.naming_template = "%p-#%i-%t".into();
        cfg.output.naming_prefix = "snap".into();
        cfg.output.collision = Collision::KeepBoth;
        std::fs::write(&path, cfg.export_text().unwrap()).expect("seed");
        let got = Naming::from(&Settings::open(&path, Some(&path)).config.output);
        assert_eq!(got.dir, cfg.output.save_dir, "{got:?}");
        assert_eq!(got.template, "%p-#%i-%t", "{got:?}");
        assert_eq!(got.prefix, "snap", "{got:?}");
        assert_eq!(got.collision, Collision::KeepBoth, "{got:?}");
        assert_eq!(got.directory().as_deref(), Some(dir.as_path()));

        // The shipped default is the one leg no file can be blamed for.
        let blank = Naming::default();
        assert!(blank.dir.is_empty(), "{blank:?} 的默认值挑了一个目录");
        if let Some(p) = blank.directory() {
            assert!(p.is_absolute(), "{} 不是用户选过的目录", p.display());
        }
    }

    #[test]
    fn a_first_run_reads_defaults_and_leaves_no_file_behind() {
        let dir = sandbox("first-run");
        let path = file_in(&dir);
        let s = pinned(&dir);
        assert!(matches!(s.source, LoadSource::Defaults));
        assert_eq!(s.config, Config::default());
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        // Reading the settings must not create them: the file appears when there is
        // something of the user's to keep, not when the product first wakes up.
        assert!(!path.exists(), "startup wrote a config file");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_pen_taught_in_one_process_is_drawn_with_in_the_next() {
        // §9.2's sentence end to end, through a file rather than through a test's
        // memory - the half P17 left as two seams.
        let dir = sandbox("restart");
        let path = file_in(&dir);

        let mut s = pinned(&dir);
        let mut layer = Layer::default();
        assert_eq!(
            s.apply_to(&mut layer),
            0,
            "first run has nothing to remember"
        );
        teach(&mut layer, Kind::Rect, [1, 2, 3, 240], 7);
        let ms = s
            .write_back(&layer)
            .expect("a pen moved, so a write happens");
        assert!(path.exists(), "the write did not land");
        assert_eq!(s.saves, 1);
        assert!(ms < 5_000, "a save that costs {ms} ms is not a save");

        // The next process: the same path, nothing in memory.
        let mut s2 = pinned(&dir);
        assert!(matches!(s2.source, LoadSource::File));
        assert_eq!(s2.config.annotation.tool_style.len(), 1);
        let mut after = Layer::default();
        assert_eq!(s2.apply_to(&mut after), 1, "the file handed over one pen");
        // And the toolbar still has to see it: selecting the tool is what shows the
        // pen, so this is the readback a user gets rather than a field of the table.
        after.select_tool(code_of(Kind::Rect));
        assert_eq!(after.color(), [1, 2, 3, 240], "the pen did not come back");
        assert_eq!(after.width(), 7);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_row_written_by_hand_reaches_the_pen_across_a_real_file() {
        // The `[annotation]` scalars, through bytes on a disk rather than through a
        // `Config` built in a test - which is the only form of this row a user can
        // actually produce. Until this round the file accepted all four keys, kept them
        // through a save, and handed `apply_to` a table that read none of them, so every
        // test in the tree that touched a pen went through `tool_style` instead and the
        // row stayed unmeasured.
        let dir = sandbox("annotation-row");
        let path = file_in(&dir);
        std::fs::write(
            &path,
            "[annotation]\nstroke_color = \"#00FF00FF\"\nstroke_width = 5\n\
             font_size = 32\nfont_family = \"Consolas\"\n",
        )
        .expect("write the hand-made file");

        let mut s = pinned(&dir);
        assert!(matches!(s.source, LoadSource::File));
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        let mut layer = Layer::default();
        assert_eq!(
            s.apply_to(&mut layer),
            0,
            "the row is not a tool_style table"
        );
        assert_eq!(layer.color(), [0, 255, 0, 255], "the file's colour lost");
        assert_eq!(layer.width(), 5);
        assert_eq!(layer.font_px(), 32);
        assert_eq!(layer.font_family(), "Consolas");
        assert!(layer.problems.is_empty(), "{:?}", layer.problems);
        // And the row that says nothing about a tool still must not invent one: a save
        // here would rewrite the user's file with a pen they never set.
        assert_eq!(layer.tool_styles().len(), 0);
        assert_eq!(s.write_back(&layer), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_flow_that_never_touched_a_pen_writes_nothing() {
        let dir = sandbox("no-move");
        let path = file_in(&dir);
        let mut s = pinned(&dir);
        assert_eq!(s.write_back(&Layer::default()), None);
        assert_eq!(s.saves, 0);
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_second_flow_learns_what_the_first_one_wrote() {
        // Two rounds against the *same* Settings, because the product keeps one
        // process up: the file must not grow a tool per screenshot, and a pen that
        // is already stored is not a change.
        let dir = sandbox("two-flows");
        let mut s = pinned(&dir);

        let mut first = Layer::default();
        teach(&mut first, Kind::Marker, [9, 8, 7, 255], 40);
        s.write_back(&first);

        let mut second = Layer::default();
        assert_eq!(s.apply_to(&mut second), 1);
        teach(&mut second, Kind::Marker, [9, 8, 7, 255], 40);
        assert_eq!(
            s.write_back(&second),
            None,
            "the same pen set twice is not a change"
        );
        assert_eq!(s.saves, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_damaged_file_is_kept_and_the_app_still_opens() {
        let dir = sandbox("damaged");
        let path = file_in(&dir);
        std::fs::write(&path, b"[[[ not toml at all\n\t\n[").expect("write garbage");

        let mut s = Settings::open(&path, Some(&path));
        assert!(matches!(s.source, LoadSource::Recovered));
        assert_eq!(s.config, Config::default(), "defaults must win");
        let backup = s.backup.clone().expect("the bad file was kept");
        assert!(backup.exists(), "{backup:?}");
        assert!(!s.warnings.is_empty());
        assert_eq!(s.notes().len(), 2, "{:?}", s.notes());

        // "Recovered" is not "read-only": the next pen still has somewhere to go.
        let mut layer = Layer::default();
        teach(&mut layer, Kind::Rect, [4, 5, 6, 255], 3);
        s.write_back(&layer);
        let again = Settings::open(&path, Some(&path));
        assert!(matches!(again.source, LoadSource::File));
        assert_eq!(again.config.annotation.tool_style.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_file_decides_where_the_next_write_goes() {
        // `advanced.data_dir` (§6.1) is a redirect set *inside* the file, so reading
        // one place and writing another is the configured behaviour, not a bug.
        let read_dir = sandbox("redirect-read");
        let data_dir = sandbox("redirect-data");
        let path = file_in(&read_dir);
        let mut cfg = Config::default();
        cfg.advanced.data_dir = data_dir.display().to_string();
        std::fs::write(&path, cfg.export_text().unwrap()).expect("seed");

        let s = Settings::open(&path, None);
        assert_eq!(s.read_from, path);
        assert_eq!(s.write_to, file_in(&data_dir));

        // A gauge that pinned its own file keeps its writes there instead.
        let pinned = Settings::open(&path, Some(&path));
        assert_eq!(pinned.write_to, path);
        std::fs::remove_dir_all(&read_dir).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn a_write_pinned_for_a_gauge_leaves_the_file_it_read_alone() {
        // §10 第 33 项 ①, in the shape the `--ink` leg leaves it in: read the user's
        // file, teach it a pen through the product's own `write_back`, and the user's
        // file must still be the bytes it was.
        //
        // Opened with a pin first (`Some(&read)`) rather than without one, because
        // `pinned=None` means "ask the file where its data lives" and an empty
        // `advanced.data_dir` answers with the real `%APPDATA%` - see `pinned` above.
        // The pin then moves to the gauge's path, which is the whole of what the gauge
        // does.
        let dir = sandbox("pinned-write");
        let read = file_in(&dir);
        std::fs::write(&read, "[annotation]\nstroke_width = 5\n").expect("seed");
        let before = std::fs::read(&read).expect("seeded bytes");

        let mut s = Settings::open(&read, Some(&read));
        let pin = dir.join("gauge").join(FILE_NAME);
        assert_eq!(s.pin_writes(&pin), pin);
        assert_eq!(s.read_from, read, "the read followed the write");

        let mut layer = Layer::default();
        teach(&mut layer, Kind::Marker, [1, 1, 1, 255], 3);
        s.write_back(&layer)
            .expect("a pen moved, so a write happens");

        assert!(pin.exists(), "the write never reached the pin");
        assert_eq!(s.saves, 1);
        assert_eq!(
            std::fs::read(&read).expect("the file it read"),
            before,
            "the gauge rewrote the file it was only supposed to read"
        );
        // And the redirect is the *only* thing the pin changed: the line still names
        // the user's file as where this copy came from.
        assert!(s.line().contains(&format!("from={}", read.display())));
        assert!(s.line().contains(&format!("write_to={}", pin.display())));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_write_refused_keeps_the_values_and_says_so() {
        let dir = sandbox("refused");
        // A directory sitting where the file should be: the write cannot land.
        let blocked = file_in(&dir);
        std::fs::create_dir_all(&blocked).expect("block the path with a directory");
        let mut s = Settings::open(&blocked, Some(&blocked));
        let mut layer = Layer::default();
        teach(&mut layer, Kind::Rect, [7, 7, 7, 255], 5);
        s.write_back(&layer);
        assert_eq!(s.saves, 0);
        assert_eq!(s.problems.len(), 1, "{:?}", s.problems);
        assert_eq!(s.config.annotation.tool_style.len(), 1, "values kept");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_line_names_every_cost_the_plan_asks_for() {
        // The report is the measurement: a number that is not printed is a number
        // nobody can check against the 400 ms line.
        let dir = sandbox("line");
        let path = file_in(&dir);
        let s = Settings::open(&path, Some(&path));
        let line = s.line();
        for part in [
            "read=defaults",
            "load=",
            "applies=0",
            "saves=0",
            "write_to=",
        ] {
            assert!(line.contains(part), "{line} lacks {part}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// §5.14 与 §6.2 里 `[history] enabled = false` 那一支的第一位 §9.1 单测证人：
    /// [`history_open_in`] 必须**不动 disk** —— 目录里一件物事也不长。P68 之前整个
    /// `falcon_core::history` 模块在 `crates/` 之外没有一位调用点，用户在档里写
    /// `enabled = false` 与写 `enabled = true` 是同一件事（因为两边都不开 SQLite）。
    /// 本轮把这一格抽成纯函数、加两单测：enabled 一侧真建出 `history.db`、
    /// disabled 一侧什么都不建。这一枚证的是 disabled 半边。
    #[test]
    fn a_disabled_history_never_touches_the_disk() {
        let dir = sandbox("hist-off");
        let cfg = falcon_core::config::History {
            enabled: false,
            ..Default::default()
        };
        let out = history_open_in(&dir, &cfg).expect("disabled must not error");
        assert!(
            out.is_none(),
            "enabled = false must hand the caller a None so no SQLite handle is built"
        );
        assert!(
            !dir.join("history.db").exists(),
            "the disabled branch must not create history.db"
        );
        assert!(
            !dir.join("history.db-wal").exists() && !dir.join("history.db-shm").exists(),
            "the disabled branch must not create WAL/SHM side-cars either"
        );
        // The dir itself does not need to pre-exist. `History::open` would create it;
        // the disabled branch must not.
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "disabled branch left {:?} in the sandbox",
            entries.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// §5.14 与 §6.2 里 `[history] enabled = true` 那一支的第一位 §9.1 单测证人：
    /// [`history_open_in`] 必须真调 [`falcon_core::history::History::open`] —— 于是
    /// 目录里长出 `history.db`（外加 WAL 与 SHM 两枚 side-car）、且 `History::dir()`
    /// 回过来的正是我们送进去的那一枚。这一枚证的是 enabled 半边，也是整个
    /// `falcon_core::history` 模块自落地以来**第一位非 test 调用点**（对照：
    /// `crates/core/src/history.rs` 里那 14 枚 `#[cfg(test)]` 单测直接调
    /// `History::open`，而 `crates/qt_bridge/` 之外没人调）。
    #[test]
    fn an_enabled_history_opens_its_db_under_the_data_dir() {
        let dir = sandbox("hist-on");
        let cfg = falcon_core::config::History {
            enabled: true,
            ..Default::default()
        };
        let out = history_open_in(&dir, &cfg)
            .expect("enabled opens")
            .expect("Some when enabled");
        assert!(
            dir.join("history.db").exists(),
            "enabled branch must create history.db under the given dir"
        );
        assert_eq!(
            out.dir(),
            dir.as_path(),
            "the handle reports the dir it was handed"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// §5.16 那句"Empty = the platform default under `%APPDATA%`"的两侧各问一次：空的一格
    /// 打印 `(platform default)` 而不是空字符串（否则读者会把它当成"这一档没值"），非空的
    /// 一侧打印用户填的原字符串。分岔本体是纯函数 ⇒ 单测证人（P67 `tray_from` / P68
    /// `history_open_in` 那一族的第三次走法，与 P72 `startup_banner` 的 `#[allow]` 撤同一轮）。
    #[test]
    fn banner_names_platform_default_only_when_data_dir_is_empty() {
        let mut h = falcon_core::config::History::default();
        let mut a = falcon_core::config::Advanced::default();
        h.enabled = true;
        h.max_items = 500;
        a.log_level = "info".into();
        a.data_dir = String::new();
        let empty = format_startup_banner(&h, &a);
        assert!(
            empty.contains("advanced.data_dir=(platform default)"),
            "空 data_dir 那侧必须显示 (platform default)：{empty}"
        );
        assert!(
            empty.contains("history.enabled=true"),
            "enabled=true 那一格要出现在同一行：{empty}"
        );

        a.data_dir = "D:\\flacon".into();
        h.enabled = false;
        let set = format_startup_banner(&h, &a);
        assert!(
            set.contains("advanced.data_dir=D:\\flacon"),
            "非空 data_dir 那侧必须原样显示：{set}"
        );
        assert!(
            set.contains("history.enabled=false"),
            "enabled=false 那一格要出现在同一行：{set}"
        );
    }

    /// P76：`format_output_banner` 的两枚 `is_empty()` 分岔各走一臂 ⇒ 空 Vec 一侧印 `(none)`、
    /// 非空一侧原样 joined。分岔本体是纯函数 ⇒ 单测证人（P67 `tray_from` / P68 `history_open_in`
    /// / P72 `banner_names_platform_default_only_when_data_dir_is_empty` / P75 refuse 规则
    /// 那一族的第五次走法）。
    #[test]
    fn banner_names_none_only_when_output_list_is_empty() {
        let o = falcon_core::config::Output::default();
        let empty = format_output_banner(&o);
        assert!(
            empty.contains("output.share_targets=(none)"),
            "空 share_targets 那侧必须显示 (none)：{empty}"
        );
        assert!(
            empty.contains("output.send_to_apps=(none)"),
            "空 send_to_apps 那侧必须显示 (none)：{empty}"
        );

        let filled = falcon_core::config::Output {
            share_targets: vec!["slack".into(), "teams".into()],
            send_to_apps: vec!["Notepad|notepad.exe".into()],
            ..Default::default()
        };
        let set = format_output_banner(&filled);
        assert!(
            set.contains("output.share_targets=slack,teams"),
            "非空 share_targets 那侧必须原样 joined：{set}"
        );
        assert!(
            set.contains("output.send_to_apps=Notepad|notepad.exe"),
            "非空 send_to_apps 那侧必须原样 joined：{set}"
        );
    }

    /// P123：`format_output_banner` 自此逐字回读 §5.5.11 那枚开关键，两臂各走一遍 ——
    /// `Default` 那一侧印 `false`（步骤 1 是用户自己那一格，出厂不替他开启），翻成 `true`
    /// 的那一侧原样印 `true`。这一行是这枚键目前**唯一**的运行期出口 ⇒ 分岔本体是纯函数
    /// ⇒ 单测证人（第 69 项 ⑫ 那一族：P67 tray_from / P68 history_open_in / P72 banner /
    /// P75 refuse / P76 上面那枚 / P77–P81 那四枚 parse·validate 之后的又一次走法）。
    #[test]
    fn banner_prints_the_file_dialog_linkage_switch_both_ways() {
        let off = format_output_banner(&falcon_core::config::Output::default());
        assert!(
            off.contains("output.hide_pins_on_dialog=false"),
            "出厂默认就该显 false，联动整段不该发生：{off}"
        );
        let on = falcon_core::config::Output {
            hide_pins_on_dialog: true,
            ..Default::default()
        };
        let shown = format_output_banner(&on);
        assert!(
            shown.contains("output.hide_pins_on_dialog=true"),
            "开着那一侧要原样印 true，不能被上面两枚 (none) 分岔顶掉：{shown}"
        );
    }

    /// P77：`parse_send_to` 的四臂 —— 无 `|` / 空 name / 空 exe / 三段首拆 / 前后空格 ——
    /// 各走一遍。分岔本体是纯函数 ⇒ 单测证人（第 69 项 ⑫ 那一族的**第六次**走法：P67
    /// tray_from / P68 history_open_in / P72 banner_names_platform_default_only_when_
    /// data_dir_is_empty / P75 a_mis_tuned_invocation_is_refused_before_the_first_window
    /// / P76 banner_names_none_only_when_output_list_is_empty 之后本轮）。
    #[test]
    fn parse_send_to_splits_on_first_pipe_and_refuses_the_flawed() {
        let entries = vec![
            "Notepad|notepad.exe".to_string(),
            "  Spaced  |  mspaint.exe  ".to_string(),
            "Multi|a|b".to_string(),
            "|only-exe".to_string(),
            "only-name|".to_string(),
            "no-pipe-here".to_string(),
        ];
        let (ok, refused) = parse_send_to(&entries);
        assert_eq!(
            refused, 3,
            "三条应被拒（空 name / 空 exe / 无 |）：ok={ok:?} refused={refused}"
        );
        assert_eq!(ok.len(), 3, "三条应通过：{ok:?}");
        assert_eq!(
            ok[0],
            ("Notepad".to_string(), "notepad.exe".to_string()),
            "正常一条原样：{ok:?}"
        );
        assert_eq!(
            ok[1],
            ("Spaced".to_string(), "mspaint.exe".to_string()),
            "两侧 trim 生效：{ok:?}"
        );
        assert_eq!(
            ok[2],
            ("Multi".to_string(), "a|b".to_string()),
            "首个 `|` 之后原样保留：{ok:?}"
        );
    }

    /// P78：`parse_share_targets` 的两臂 —— trim 空 → 拒、其余一律收并保留 trim 后的字符串。
    /// 分岔本体是纯函数 ⇒ 单测证人（第 69 项 ⑫ 那一族的**第七次**走法：P67 tray_from /
    /// P68 history_open_in / P72 banner_names_platform_default_only_when_data_dir_is_empty /
    /// P75 a_mis_tuned_invocation_is_refused_before_the_first_window / P76
    /// banner_names_none_only_when_output_list_is_empty / P77
    /// parse_send_to_splits_on_first_pipe_and_refuses_the_flawed 之后本轮）。
    #[test]
    fn parse_share_targets_trims_and_refuses_blank_only() {
        let entries = vec![
            "slack".to_string(),
            "  teams  ".to_string(),
            "".to_string(),
            "   ".to_string(),
            "wecom".to_string(),
        ];
        let (ok, refused) = parse_share_targets(&entries);
        assert_eq!(
            refused, 2,
            "两条应被拒（空串 + 纯空白）：ok={ok:?} refused={refused}"
        );
        assert_eq!(ok.len(), 3, "三条应通过：{ok:?}");
        assert_eq!(ok[0], "slack", "正常一条：{ok:?}");
        assert_eq!(ok[1], "teams", "两侧 trim 生效：{ok:?}");
        assert_eq!(ok[2], "wecom", "顺序保留：{ok:?}");
    }

    /// P79：`filter_share_targets` 的两臂 —— 命中白名单 → known、未命中 → unknown；
    /// 大小写不敏感（`"Teams"` 也命中 `"teams"`）。分岔本体是纯函数 ⇒ 单测证人
    /// （第 69 项 ⑫ 那一族的**第八次**走法：P67 tray_from / P68 history_open_in /
    /// P72 banner_names_platform_default_only_when_data_dir_is_empty / P75
    /// a_mis_tuned_invocation_is_refused_before_the_first_window / P76
    /// banner_names_none_only_when_output_list_is_empty / P77
    /// parse_send_to_splits_on_first_pipe_and_refuses_the_flawed / P78
    /// parse_share_targets_trims_and_refuses_blank_only 之后本轮）。
    #[test]
    fn filter_share_targets_splits_by_lowercase_against_allow_list() {
        let parsed = vec![
            "slack".to_string(),
            "Teams".to_string(),
            "github".to_string(),
            "wecom".to_string(),
            "MyCloud".to_string(),
        ];
        let allowed: &[&str] = &["slack", "teams", "wecom", "dingtalk", "feishu"];
        let (known, unknown) = filter_share_targets(&parsed, allowed);
        assert_eq!(known.len(), 3, "三条命中白名单：{known:?}");
        assert_eq!(known[0], "slack", "顺序保留：{known:?}");
        assert_eq!(
            known[1], "Teams",
            "原大小写保留、判定按 lowercase：{known:?}"
        );
        assert_eq!(known[2], "wecom", "顺序保留：{known:?}");
        assert_eq!(unknown.len(), 2, "两条未命中：{unknown:?}");
        assert_eq!(unknown[0], "github", "顺序保留：{unknown:?}");
        assert_eq!(unknown[1], "MyCloud", "顺序保留：{unknown:?}");
    }

    /// P80：`validate_exe_paths` 的两臂 —— 命中 `.exe` / `.cmd` / `.bat`（大小写不敏感）
    /// → 收进 ok、未命中 → 计入 refused。分岔本体是纯函数 ⇒ 单测证人
    /// （第 69 项 ⑫ 那一族的**第九次**走法：P67 tray_from / P68 history_open_in /
    /// P72 banner_names_platform_default_only_when_data_dir_is_empty / P75
    /// a_mis_tuned_invocation_is_refused_before_the_first_window / P76
    /// banner_names_none_only_when_output_list_is_empty / P77
    /// parse_send_to_splits_on_first_pipe_and_refuses_the_flawed / P78
    /// parse_share_targets_trims_and_refuses_blank_only / P79
    /// filter_share_targets_splits_by_lowercase_against_allow_list 之后本轮）。
    #[test]
    fn validate_exe_paths_accepts_exe_cmd_bat_and_refuses_others() {
        let parsed = vec![
            ("Notepad".to_string(), "notepad.exe".to_string()),
            ("Script".to_string(), "run.CMD".to_string()),
            ("Batch".to_string(), "install.bat".to_string()),
            ("Text".to_string(), "readme.txt".to_string()),
            ("NoExt".to_string(), "weird".to_string()),
        ];
        let (ok, refused) = validate_exe_paths(&parsed);
        assert_eq!(ok.len(), 3, "三条命中 Windows 扩展名：{ok:?}");
        assert_eq!(ok[0].1, "notepad.exe", "顺序保留：{ok:?}");
        assert_eq!(ok[1].1, "run.CMD", "扩展名大小写不敏感、原样保留：{ok:?}");
        assert_eq!(ok[2].1, "install.bat", "顺序保留：{ok:?}");
        assert_eq!(refused, 2, "两条被拒（.txt + 无扩展名）：refused={refused}");
    }

    /// P81：`pipeline_send_to` 的复合 —— parse_refused 与 validate_refused 各自分账、
    /// final 只剩"按 `|` 拆过 + 扩展名是 `.exe` / `.cmd` / `.bat`"的条目。分岔本体是
    /// 纯函数 ⇒ 单测证人（第 69 项 ⑫ 那一族的**第十次**走法：P67 tray_from /
    /// P68 history_open_in / P72 banner_names_platform_default_only_when_data_dir_is_empty /
    /// P75 a_mis_tuned_invocation_is_refused_before_the_first_window / P76
    /// banner_names_none_only_when_output_list_is_empty / P77 parse_send_to_... / P78
    /// parse_share_targets_... / P79 filter_share_targets_... / P80 validate_exe_paths_...
    /// 之后本轮）。
    #[test]
    fn pipeline_send_to_composes_parse_then_validate() {
        let entries = vec![
            "Notepad|notepad.exe".to_string(),
            "Text|readme.txt".to_string(),
            "|only-exe".to_string(),
            "Script|run.bat".to_string(),
            "no-pipe".to_string(),
        ];
        let (final_ok, parse_refused, validate_refused) = pipeline_send_to(&entries);
        assert_eq!(
            parse_refused, 2,
            "|only-exe 与 no-pipe 被 parse 拒：{parse_refused}"
        );
        assert_eq!(
            validate_refused, 1,
            "readme.txt 被 validate 拒：{validate_refused}"
        );
        assert_eq!(
            final_ok.len(),
            2,
            "notepad.exe + run.bat 通过：{final_ok:?}"
        );
        assert_eq!(final_ok[0].0, "Notepad", "顺序保留：{final_ok:?}");
        assert_eq!(final_ok[1].0, "Script", "顺序保留：{final_ok:?}");
    }

    /// P82：`pipeline_share_targets` 的复合 —— parse_refused 与 unknown 各自分账、
    /// final 只剩"过 trim + 大小写不敏感命中 ALLOWED"的条目（canonical 由 parse 侧
    /// 保留原样）。分岔本体是纯函数 ⇒ 单测证人（第 69 项 ⑫ 那一族的**第十一次**走法：
    /// P67 / P68 / P72 / P75 / P76 / P77 / P78 / P79 / P80 / P81 之后本轮）。
    #[test]
    fn pipeline_share_targets_composes_parse_then_filter() {
        let entries = vec![
            "slack".to_string(),
            "".to_string(),
            "TEAMS".to_string(),
            "myspace".to_string(),
            "   ".to_string(),
            "wecom".to_string(),
        ];
        let (known, parse_refused, unknown_count) = pipeline_share_targets(&entries);
        assert_eq!(
            parse_refused, 2,
            "空串 + 三空格被 parse 拒（trim 后空）：{parse_refused}"
        );
        assert_eq!(unknown_count, 1, "myspace 不在白名单：{unknown_count}");
        assert_eq!(known.len(), 3, "slack + TEAMS + wecom 通过：{known:?}");
        assert_eq!(known[0], "slack", "顺序保留：{known:?}");
        assert_eq!(known[1], "TEAMS", "原样保留大小写：{known:?}");
        assert_eq!(known[2], "wecom", "顺序保留：{known:?}");
    }

    /// P83：`send_to_menu_entries` 剥扩展名、保大小写、保顺序。分岔本体是纯函数 ⇒
    /// 单测证人（第 69 项 ⑫ 那一族的**第十二次**走法：P67 / P68 / P72 / P75 / P76 /
    /// P77 / P78 / P79 / P80 / P81 / P82 之后本轮）。
    #[test]
    fn send_to_menu_entries_strips_extension_and_preserves_case() {
        let ok = vec![
            ("Notepad".to_string(), "notepad.exe".to_string()),
            ("Runner".to_string(), "Run.CMD".to_string()),
            ("Installer".to_string(), "install.bat".to_string()),
        ];
        let entries = send_to_menu_entries(&ok);
        assert_eq!(entries.len(), 3, "三条全出：{entries:?}");
        assert_eq!(entries[0], "Notepad (notepad)", "{entries:?}");
        assert_eq!(
            entries[1], "Runner (Run)",
            "扩展名剥、大小写保：{entries:?}"
        );
        assert_eq!(entries[2], "Installer (install)", "{entries:?}");
    }

    /// P84：`share_menu_entries` 首字母大写、其余原样。分岔本体是纯函数 ⇒
    /// 单测证人（第 69 项 ⑫ 那一族的**第十三次**走法：P67 / P68 / P72 / P75 / P76 /
    /// P77 / P78 / P79 / P80 / P81 / P82 / P83 之后本轮）。
    #[test]
    fn share_menu_entries_capitalizes_first_letter_preserves_rest() {
        let known = vec![
            "slack".to_string(),
            "TEAMS".to_string(),
            "wecom".to_string(),
        ];
        let entries = share_menu_entries(&known);
        assert_eq!(entries.len(), 3, "三条全出：{entries:?}");
        assert_eq!(entries[0], "Slack", "{entries:?}");
        assert_eq!(entries[1], "TEAMS", "首字母已大写则不动：{entries:?}");
        assert_eq!(entries[2], "Wecom", "{entries:?}");
    }

    /// P85：`resolve_send_to` 大小写不敏感命中、exe 原样保留、未注册 / 被拒条目
    /// ⇒ None。分岔本体是纯函数 ⇒ 单测证人（第 69 项 ⑫ 那一族的**第十四次**走法：
    /// P67 / P68 / P72 / P75 / P76 / P77 / P78 / P79 / P80 / P81 / P82 / P83 / P84
    /// 之后本轮）。
    #[test]
    fn resolve_send_to_finds_exe_by_name_case_insensitive() {
        let entries = vec![
            "Notepad|notepad.exe".to_string(),
            "Runner|Run.CMD".to_string(),
            "bad".to_string(),
        ];
        assert_eq!(
            resolve_send_to(&entries, "notepad"),
            Some("notepad.exe".to_string()),
            "大小写不敏感命中"
        );
        assert_eq!(
            resolve_send_to(&entries, "RUNNER"),
            Some("Run.CMD".to_string()),
            "exe 原样保留大小写"
        );
        assert_eq!(resolve_send_to(&entries, "Missing"), None, "未注册 ⇒ None");
        assert_eq!(
            resolve_send_to(&entries, "bad"),
            None,
            "被 parse 拒的条目 ⇒ None"
        );
    }

    /// P86：`share_scheme_for` 五枚内置 target 全部映射、大小写不敏感、未注册 / 空 path ⇒ None。
    /// 分岔本体是纯函数 ⇒ 单测证人（证人 dichotomy 第十五次：P67 / P68 / P72 / P75 / P76 /
    /// P77 / P78 / P79 / P80 / P81 / P82 / P83 / P84 / P85 / **P86**）。
    #[test]
    fn share_scheme_for_maps_all_five_targets_case_insensitive() {
        let p = "/tmp/x.png";
        assert_eq!(
            share_scheme_for("slack", p),
            Some("slack://share?file=/tmp/x.png".to_string()),
            "slack 小写命中"
        );
        assert_eq!(
            share_scheme_for("SLACK", p),
            Some("slack://share?file=/tmp/x.png".to_string()),
            "大小写不敏感"
        );
        assert_eq!(
            share_scheme_for("Teams", p),
            Some("msteams://share?path=/tmp/x.png".to_string()),
            "teams 混合大小写命中"
        );
        assert_eq!(
            share_scheme_for("wecom", p),
            Some("wecom://send?file=/tmp/x.png".to_string())
        );
        assert_eq!(
            share_scheme_for("DingTalk", p),
            Some("dingtalk://share?path=/tmp/x.png".to_string())
        );
        assert_eq!(
            share_scheme_for("feishu", p),
            Some("feishu://send?file=/tmp/x.png".to_string())
        );
        assert_eq!(share_scheme_for("myspace", p), None, "不在白名单 ⇒ None");
        assert_eq!(share_scheme_for("slack", ""), None, "空 path ⇒ None");
    }

    /// P87：`send_to_exec_argv` 返回 [exe, path]、大小写不敏感命中、未注册 / 空 path /
    /// 被 parse 拒条目 ⇒ None。分岔本体是纯函数 ⇒ 单测证人（证人 dichotomy 第十六次：
    /// P67 / P68 / P72 / P75 / P76 / P77 / P78 / P79 / P80 / P81 / P82 / P83 / P84 /
    /// P85 / P86 / **P87**）。
    #[test]
    fn send_to_exec_argv_returns_exe_then_path_or_none() {
        let entries = vec![
            "Notepad|notepad.exe".to_string(),
            "Runner|Run.CMD".to_string(),
            "bad".to_string(),
        ];
        assert_eq!(
            send_to_exec_argv(&entries, "notepad", "/tmp/a.png"),
            Some(vec!["notepad.exe".to_string(), "/tmp/a.png".to_string()]),
            "命中 ⇒ [exe, path]"
        );
        assert_eq!(
            send_to_exec_argv(&entries, "RUNNER", "/tmp/b.png"),
            Some(vec!["Run.CMD".to_string(), "/tmp/b.png".to_string()]),
            "大小写不敏感、exe 原样保留"
        );
        assert_eq!(
            send_to_exec_argv(&entries, "Missing", "/tmp/c.png"),
            None,
            "未命中 ⇒ None"
        );
        assert_eq!(
            send_to_exec_argv(&entries, "Notepad", ""),
            None,
            "空 path ⇒ None"
        );
        assert_eq!(
            send_to_exec_argv(&entries, "bad", "/tmp/d.png"),
            None,
            "被 parse 拒的条目 ⇒ None"
        );
    }

    /// P88：`share_scheme_for_all` 顺序保留、target 原大小写保留、未在白名单跳过；
    /// 空 known / 空 path ⇒ 空列表。分岔本体是纯函数 ⇒ 单测证人（证人 dichotomy
    /// 第十七次：P67 / P68 / P72 / P75 / P76 / P77 / P78 / P79 / P80 / P81 / P82 /
    /// P83 / P84 / P85 / P86 / P87 / **P88**）。
    #[test]
    fn share_scheme_for_all_maps_in_order_and_skips_unknown() {
        let known = vec![
            "slack".to_string(),
            "myspace".to_string(),
            "TEAMS".to_string(),
            "wecom".to_string(),
        ];
        let p = "/tmp/y.png";
        let got = share_scheme_for_all(&known, p);
        assert_eq!(got.len(), 3, "myspace 不在白名单 ⇒ 跳过：{got:?}");
        assert_eq!(
            got[0],
            (
                "slack".to_string(),
                "slack://share?file=/tmp/y.png".to_string()
            ),
            "顺序保留：{got:?}"
        );
        assert_eq!(
            got[1],
            (
                "TEAMS".to_string(),
                "msteams://share?path=/tmp/y.png".to_string()
            ),
            "原大小写保留：{got:?}"
        );
        assert_eq!(
            got[2],
            (
                "wecom".to_string(),
                "wecom://send?file=/tmp/y.png".to_string()
            ),
            "顺序保留：{got:?}"
        );
        let empty = share_scheme_for_all(&[], p);
        assert!(empty.is_empty(), "空 known ⇒ 空列表");
        let no_path = share_scheme_for_all(&known, "");
        assert!(no_path.is_empty(), "空 path ⇒ 全体 None ⇒ 空列表");
    }

    /// P89：`send_to_exec_argv_first` 走 P81 pipeline 拿首条 final_ok、返回 `[exe, path]`；
    /// 首条被 P77 parse 拒 ⇒ 落到下一条；空 entries / 空 path ⇒ `None`。
    /// 分岔本体是纯函数 ⇒ 单测证人（证人 dichotomy 第十八次：P67 / P68 / P72 / P75 /
    /// P76 / P77 / P78 / P79 / P80 / P81 / P82 / P83 / P84 / P85 / P86 / P87 / P88 / **P89**）。
    #[test]
    fn send_to_exec_argv_first_picks_pipeline_head_or_none() {
        let entries = vec![
            "Notepad|notepad.exe".to_string(),
            "Runner|Run.CMD".to_string(),
            "bad".to_string(),
        ];
        assert_eq!(
            send_to_exec_argv_first(&entries, "/tmp/a.png"),
            Some(vec!["notepad.exe".to_string(), "/tmp/a.png".to_string()]),
            "首条 notepad 通过 parse+validate ⇒ argv = [notepad.exe, path]"
        );
        // 首条被 P77 parse 拒（缺 `|`）⇒ pipeline 落第二条 mspaint。
        let skipped = vec![
            "broken-entry-without-pipe".to_string(),
            "Paint|mspaint.exe".to_string(),
        ];
        assert_eq!(
            send_to_exec_argv_first(&skipped, "/tmp/b.png"),
            Some(vec!["mspaint.exe".to_string(), "/tmp/b.png".to_string()]),
            "首条 parse 拒 ⇒ 落第二条"
        );
        assert_eq!(
            send_to_exec_argv_first(&[], "/tmp/c.png"),
            None,
            "空 entries ⇒ None"
        );
        assert_eq!(
            send_to_exec_argv_first(&entries, ""),
            None,
            "空 path ⇒ None（即便有可用条目）"
        );
    }

    /// P90：`share_scheme_first` 逐条走 P86 [`share_scheme_for`]、返回第一条 Some；
    /// 首条未注册 ⇒ 落第二条；全体未注册 / 空 known / 空 path ⇒ None。
    /// 分岔本体是纯函数 ⇒ 单测证人（证人 dichotomy 第十九次：P67 / P68 / P72 / P75 /
    /// P76 / P77 / P78 / P79 / P80 / P81 / P82 / P83 / P84 / P85 / P86 / P87 / P88 / P89 / **P90**）。
    #[test]
    fn share_scheme_first_returns_head_or_none() {
        // 首条 slack 命中 ⇒ Some((slack, slack://...))。
        let known = vec!["slack".to_string(), "teams".to_string()];
        assert_eq!(
            share_scheme_first(&known, "/tmp/x.png"),
            Some((
                "slack".to_string(),
                "slack://share?file=/tmp/x.png".to_string()
            )),
            "首条 slack ⇒ Some"
        );
        // 首条 myspace 未注册 ⇒ 落第二条 TEAMS（原大小写保留）。
        let skipped = vec!["myspace".to_string(), "TEAMS".to_string()];
        assert_eq!(
            share_scheme_first(&skipped, "/tmp/y.png"),
            Some((
                "TEAMS".to_string(),
                "msteams://share?path=/tmp/y.png".to_string()
            )),
            "首条未注册 ⇒ 落第二条、原大小写保留"
        );
        // 全体未注册 ⇒ None。
        let all_unknown = vec!["myspace".to_string(), "friendster".to_string()];
        assert_eq!(
            share_scheme_first(&all_unknown, "/tmp/z.png"),
            None,
            "全体未注册 ⇒ None"
        );
        // 空 known ⇒ None。
        assert_eq!(
            share_scheme_first(&[], "/tmp/a.png"),
            None,
            "空 known ⇒ None"
        );
        // 空 path ⇒ None（即便有 slack）。
        assert_eq!(share_scheme_first(&known, ""), None, "空 path ⇒ None");
    }

    /// P91：`send_to_menu_names_joined` 把 [`send_to_menu_entries`] 输出的 UI 显示串
    /// 用 `|` 联成一枚纯 `String`；空集合 ⇒ 空串；一条 ⇒ 无 `|`；两条 ⇒ 恰好一个 `|`
    /// 分节；三条 ⇒ 恰好两个。这里传的 parsed_ok 是 P81 [`pipeline_send_to`] **已经**
    /// 校验过之后剩下的 `(name, exe)` 二元组，所以不会被 parse/validate 拒。
    ///
    /// 分岔本体是纯函数 ⇒ 单测证人（证人 dichotomy 第二十次：P67 / P68 / P72 / P75 /
    /// P76 / P77 / P78 / P79 / P80 / P81 / P82 / P83 / P84 / P85 / P86 / P87 / P88 /
    /// P89 / P90 / **P91**）。
    #[test]
    fn send_to_menu_names_joined_uses_pipe_separator() {
        // 空 ⇒ 空串。
        assert_eq!(send_to_menu_names_joined(&[]), "", "空集合 ⇒ 空串");

        // 一条 ⇒ 无 `|`。
        let one = vec![("Notepad".to_string(), "notepad.exe".to_string())];
        assert_eq!(
            send_to_menu_names_joined(&one),
            "Notepad (notepad)",
            "一条 ⇒ 无分隔符"
        );

        // 两条 ⇒ 一个 `|`；扩展名截掉那一支走 `.CMD` 大小写不敏感。
        let two = vec![
            ("Notepad".to_string(), "notepad.exe".to_string()),
            ("Runner".to_string(), "Run.CMD".to_string()),
        ];
        assert_eq!(
            send_to_menu_names_joined(&two),
            "Notepad (notepad)|Runner (Run)",
            "两条 ⇒ 恰好一个 `|`；.CMD 大小写不敏感截掉"
        );

        // 三条 ⇒ 恰好两个 `|`；无扩展名的 exe 保留整串（P77 的 `stem` 兜底分支）。
        let three = vec![
            ("A".to_string(), "a.exe".to_string()),
            ("B".to_string(), "b.bat".to_string()),
            ("C".to_string(), "noext".to_string()),
        ];
        let joined = send_to_menu_names_joined(&three);
        assert_eq!(
            joined, "A (a)|B (b)|C (noext)",
            "三条 ⇒ 恰好两个 `|`；无扩展名保留整串"
        );
        assert_eq!(joined.split('|').count(), 3, "三条 split ⇒ 3 段");
    }

    /// P92：`share_menu_names_joined` 把 [`share_menu_entries`] 输出的显示串用 `|`
    /// 联成一枚纯 `String`；空集合 ⇒ 空串；一条 ⇒ 无 `|`；两条 ⇒ 恰好一个；三条 ⇒
    /// 恰好两个。P84 的显示侧口径在这一族里保持不动：只大写首字符、其余原样
    /// （`"slack"` ⇒ `"Slack"`、`"TEAMS"` ⇒ `"TEAMS"`），join 也不排序、不去重。
    ///
    /// 分岔本体是纯函数 ⇒ 单测证人（证人 dichotomy 第二十一次：P67 / … / P91 /
    /// **P92**）。
    #[test]
    fn share_menu_names_joined_uses_pipe_separator() {
        // 空 ⇒ 空串。JS 侧 `"".split("|")` 会得到一枚空串段而非 0 段，所以 P93 的
        // Repeater 接线在把模型交给列表之前必须自己判空；这一格本轮只钉事实、不接线。
        assert_eq!(share_menu_names_joined(&[]), "", "空集合 ⇒ 空串");

        // 一条 ⇒ 无 `|`。
        let one: Vec<String> = vec!["slack".to_string()];
        assert_eq!(
            share_menu_names_joined(&one),
            "Slack",
            "一条 ⇒ 无分隔符；slack ⇒ Slack"
        );

        // 两条 ⇒ 恰好一个 `|`；整串大写的条目不被改写。
        let two: Vec<String> = vec!["slack".to_string(), "TEAMS".to_string()];
        let joined = share_menu_names_joined(&two);
        assert_eq!(joined, "Slack|TEAMS", "两条 ⇒ 恰好一个 `|`；TEAMS 原样");
        assert_eq!(joined.split('|').count(), 2, "两条 split ⇒ 2 段");

        // 三条 ⇒ 恰好两个 `|`，且顺序就是 known 的顺序。
        let three: Vec<String> = vec!["slack".to_string(), "TEAMS".to_string(), "zoom".to_string()];
        let joined3 = share_menu_names_joined(&three);
        assert_eq!(
            joined3, "Slack|TEAMS|Zoom",
            "三条 ⇒ 恰好两个 `|`；按 known 原序"
        );
        assert_eq!(joined3.split('|').count(), 3, "三条 split ⇒ 3 段");
    }

    /// P98：[`history_rows_joined`] 的那一串里 `|` **只**分隔行。界面那一头是
    /// `.split("|")`，所以「一段 = 一行」是这一族唯一能被界面验证的形状：条数不等、
    /// 或者某一行自己带了一枚 `|`，面板就会把一行历史渲染成两格。
    ///
    /// 行号在第一段（裸的），因为点某一行重放时界面要的是 SQLite 的那个 id 而不是一句中文；
    /// 「已取消」「已锁定」是 §5.14.3 要找的那两个字样，不带时那一行什么都不多写。
    ///
    /// 分岔本体是纯函数 ⇒ 单测证人（证人 dichotomy 第二十二次：P67 / … / P92 / **P98**）。
    #[test]
    fn history_rows_joined_gives_one_segment_per_row() {
        // 2023-11-14T22:13:20Z 那一秒，往西 480 分钟 ⇒ 本地 14:13。偏移是调用方交的，
        // 这一格因此量的不是本机的钟。
        let at_ms = 1_700_000_000_000;
        let rec =
            |id: i64, monitor: &str, cancelled: bool, locked: bool| falcon_core::history::Record {
                id,
                captured_at: at_ms,
                monitor_id: monitor.to_string(),
                rect: falcon_core::PhysRect::new(10, 20, 64, 32),
                source: falcon_core::history::Source::Region,
                path: None,
                thumb: None,
                cancelled,
                locked,
                ocr_text: None,
                note: String::new(),
            };

        // 空 ⇒ 空串（对着 P92 那条「JS 侧 `"".split("|")` 得一枚空段」的口径写：调用方
        // 判空要看这一串本身空不空，不能看段数）。
        assert_eq!(history_rows_joined(&[], -480), "", "空清单 ⇒ 空串");

        let one = vec![rec(7, "主屏", false, false)];
        assert_eq!(
            history_rows_joined(&one, -480),
            "7 · 2023-11-14 14:13 · 64×32 · 主屏 · region",
            "一条 ⇒ 无 `|`；行号在第一段，字段以 ` · ` 分节"
        );

        // 这一枚 `|` 是这一格的全部理由：库里那一列是自由文本，带进来就把一行切成两格。
        let pipe = vec![rec(8, "屏|二|号", false, false)];
        let joined = history_rows_joined(&pipe, 0);
        assert_eq!(
            joined.split('|').count(),
            1,
            "monitor_id 里的 `|` 被洗成 `/`，行段数仍然等于条数"
        );
        assert!(
            joined.contains("屏/二/号"),
            "洗的是分隔符而不是整列：{joined}"
        );

        // 两枚字样各说一件事，同带时都在。
        let flags = vec![rec(9, "主屏", true, false), rec(10, "主屏", false, true)];
        let joined = history_rows_joined(&flags, -480);
        let rows: Vec<&str> = joined.split('|').collect();
        assert_eq!(rows.len(), 2, "两条 ⇒ 恰好一个 `|`");
        assert!(
            rows[0].ends_with(" · 已取消"),
            "cancelled 的字样要在：{}",
            rows[0]
        );
        assert!(
            rows[1].ends_with(" · 已锁定"),
            "locked 的字样要在：{}",
            rows[1]
        );
    }

    /// P98：[`history_rows_at`] 的三种答法是三件事，而这一族把它们**分开**答：
    /// `[history] enabled` 关着 ⇒ `Err` 点「没开」；开着而库里没行 ⇒ `Ok(空串)`（一次
    /// 正常的空）；库里真有两行 ⇒ `Ok` 里倒序点名那两个行号，而 `limit` 是 `newest` 里
    /// 那一枚 LIMIT，不是这里事后截的。
    ///
    /// 这一格同时把 `History::newest` 在单测里读通（产品那一头的首位读者是
    /// [`crate::mask::MaskState::list_history`]，同轮）。
    #[test]
    fn history_rows_at_answers_separately_for_off_empty_and_two_rows() {
        let dir = sandbox("history-listing");
        // 写成 `fn` 而不是闭包：`CaptureMeta` 带着 `monitor_id` 的那个借用，而闭包的返回类型
        // 只能有一个匿名生命周期，编译器当场不肯让它在两次调用之间各借一次。
        fn meta<'a>(at_ms: i64, monitor: &'a str) -> falcon_core::history::CaptureMeta<'a> {
            falcon_core::history::CaptureMeta {
                at_ms,
                offset_min: -480,
                monitor_id: monitor,
                rect: falcon_core::PhysRect::new(0, 0, 4, 4),
                source: falcon_core::history::Source::Region,
                cancelled: false,
            }
        }
        // 缩略图与原图都关掉：这一格问的是清单读不读得回，不是盘上多不多一张图。
        let cfg = falcon_core::config::History {
            enabled: true,
            keep_originals: false,
            thumb_px: 0,
            ..Default::default()
        };
        let frame = falcon_core::frame::Frame::filled(4, 4, [0, 0, 0, 255]).unwrap();
        let mut store = falcon_core::history::History::open(&dir).unwrap();
        let older = store
            .record(&meta(1_700_000_000_000, "先那一块"), &frame, &cfg)
            .unwrap();
        let newer = store
            .record(&meta(1_700_003_600_000, "后那一块"), &frame, &cfg)
            .unwrap();
        drop(store);

        let joined = history_rows_at(&dir, &cfg, 50, -480).expect("开着就该读得回");
        let rows: Vec<&str> = joined.split('|').collect();
        assert_eq!(rows.len(), 2, "两行 ⇒ 两段：{joined}");
        assert!(
            rows[0].starts_with(&format!("{newer} · ")),
            "倒序的第一段是后那一行：{}",
            rows[0]
        );
        assert!(
            rows[1].starts_with(&format!("{older} · ")),
            "第二段是先那一行：{}",
            rows[1]
        );
        assert!(
            rows[0].contains("后那一块"),
            "那一行的 `monitor_id` 要跟着行号一起回：{}",
            rows[0]
        );
        // 后那一行是 2023-11-15 的（比先那一行晚一小时，跨到同一天的另一格钟点）。
        assert!(
            rows[0].starts_with(&format!("{newer} · 2023-11-14 15:13")),
            "时间由调用方那枚偏移算出，不是本机钟：{}",
            rows[0]
        );

        // `limit` 真到了 SQL：只要一行时读回的就是倒序的那一枚，而不是两枚再截。
        let one = history_rows_at(&dir, &cfg, 1, -480).expect("limit 1 也该读得回");
        assert_eq!(one.split('|').count(), 1, "limit 1 ⇒ 一段");
        assert!(
            one.starts_with(&format!("{newer} · ")),
            "留下的该是新的那一枚：{one}"
        );

        // 开着而库里没有行 = 一次正常的空，不是错。
        let empty = sandbox("history-listing-empty");
        assert_eq!(
            history_rows_at(&empty, &cfg, 50, -480).as_deref(),
            Ok(""),
            "空库 ⇒ Ok(空串)"
        );

        // 开关关着 ⇒ Err 点「没开」。
        let off = falcon_core::config::History {
            enabled: false,
            ..cfg.clone()
        };
        let said = history_rows_at(&dir, &off, 50, -480).unwrap_err();
        assert!(said.contains("没开"), "关着的答法要点名开关：{said}");
    }

    /// P110：[`history_usage_line`] 的分岔本体是纯函数 ⇒ 独立 cargo 单测证人
    /// （P67 `tray_from` / P68 `history_open_in` / P72 `format_startup_banner` /
    /// P76 `format_output_banner` / P98 `history_rows_joined` 那一族的第十次实例）。
    /// 这一格测**三枚字节列相加得合计**：1000 + 40 + 60 = 1100，与 `Usage::total_bytes()`
    /// 的算术同一道；`records` 独立打印、不进加法。
    #[test]
    fn history_usage_line_adds_the_three_byte_columns() {
        let l = history_usage_line(3, 1000, 40, 60);
        assert!(l.contains("records=3"), "{l}");
        assert!(l.contains("original=1000B"), "{l}");
        assert!(l.contains("thumb=40B"), "{l}");
        assert!(l.contains("db=60B"), "{l}");
        assert!(l.contains("total=1100B"), "{l}");
    }

    /// P110：`records=0 / 三枚字节 0 / 合计 0` ⇒ 一行照印，不返回空串、不 `unwrap`、
    /// 不 `Err`。这条判据对面是 §6.2 那句 "show what the history is using" 的**空态**
    /// 一侧 —— 关掉历史或库里没有行时，那一行仍然要读得出「零」，而不是静默丢格。
    #[test]
    fn history_usage_line_reports_zero_when_everything_is_empty() {
        let l = history_usage_line(0, 0, 0, 0);
        assert!(l.contains("records=0"), "{l}");
        assert!(l.contains("total=0B"), "{l}");
    }

    /// P111：中间行 ⇒ prev 与 next 两边都有号，`anchor=<id>` 那一节直接接住 §5.14.2
    /// 「按上下方向键在历史里前后翻」的**读数本体**。这条判据对面是纯函数返回值，
    /// 不含时间也不含剪贴板字节 ⇒ 与 P108 / P109 那两格「不含时间」同族。
    #[test]
    fn history_neighbors_line_shows_both_sides_for_a_middle_row() {
        let l = history_neighbors_line(20, Some(19), Some(21));
        assert!(l.contains("anchor=20"), "{l}");
        assert!(l.contains("prev=19"), "{l}");
        assert!(l.contains("next=21"), "{l}");
    }

    /// P111：到顶那一行（ newest ） ⇒ next 侧要印 `-`，不是 `next=None`、不是空、不是 0。
    /// 这条判据对面是 §5.14.2 「按到最新一条就停下」那一句 —— 到顶的读数是 "-"，
    /// 让界面读得出「再按也没用」，而不是把"没有下一条"读成"读失败"。
    #[test]
    fn history_neighbors_line_dashes_a_missing_side() {
        let l = history_neighbors_line(21, Some(20), None);
        assert!(l.contains("anchor=21"), "{l}");
        assert!(l.contains("prev=20"), "{l}");
        assert!(l.contains("next=-"), "{l}");
    }

    /// P111：库里就一行 ⇒ 两边都 `-`。这一格对面是 §5.14.2 里"只有一行历史时上下键都停"
    /// 那一句的**读数一侧**：分岔两边都说得清「没有邻居」，而不是把「一行也没有」的
    /// 空库与「一行但两端」的窄库混在同一句返回里。
    #[test]
    fn history_neighbors_line_dashes_both_sides_of_a_single_row() {
        let l = history_neighbors_line(7, None, None);
        assert!(l.contains("anchor=7"), "{l}");
        assert!(l.contains("prev=-"), "{l}");
        assert!(l.contains("next=-"), "{l}");
    }

    /// P112：[`neighbors_of_banner_for`] 按 caller 递进来的 id 定位 —— 三行库里的**中间那一行**
    /// 前后都有邻居。这条判据对面是 §5.14.2「按上下方向键在历史里前后翻」那两条腿的
    /// **界面读法**（P111 banner 拿的是 newest，界面把选区挪到中间行时问的是那一行自己）；
    /// 分岔本体仍走 [`history_neighbors_line`] 那一枚纯函数。
    #[test]
    fn neighbors_of_banner_for_resolves_an_explicit_middle_anchor() {
        let dir = sandbox("neighbors-explicit-middle");
        let cfg = falcon_core::config::History {
            enabled: true,
            keep_originals: false,
            thumb_px: 0,
            ..Default::default()
        };
        let frame = falcon_core::frame::Frame::filled(4, 4, [0, 0, 0, 255]).unwrap();
        let meta = |at_ms: i64| falcon_core::history::CaptureMeta {
            at_ms,
            offset_min: -480,
            monitor_id: "m",
            rect: falcon_core::PhysRect::new(0, 0, 4, 4),
            source: falcon_core::history::Source::Region,
            cancelled: false,
        };
        let mut store = falcon_core::history::History::open(&dir).unwrap();
        let a = store
            .record(&meta(1_700_000_000_000), &frame, &cfg)
            .unwrap();
        let b = store
            .record(&meta(1_700_000_001_000), &frame, &cfg)
            .unwrap();
        let c = store
            .record(&meta(1_700_000_002_000), &frame, &cfg)
            .unwrap();
        drop(store);

        let mid = neighbors_of_banner_for(&dir, &cfg, b);
        assert!(mid.contains(&format!("anchor={b}")), "{mid}");
        assert!(mid.contains(&format!("prev={a}")), "{mid}");
        assert!(mid.contains(&format!("next={c}")), "{mid}");

        // 最新那一行：prev 有、next `-`（到顶 ⇒ 界面读得出「再按也没用」）。
        let top = neighbors_of_banner_for(&dir, &cfg, c);
        assert!(top.contains(&format!("anchor={c}")), "{top}");
        assert!(top.contains(&format!("prev={b}")), "{top}");
        assert!(top.contains("next=-"), "{top}");
    }

    /// P112：caller 递进来一个库里没有的 id ⇒ 说「库里没有第 N 行」，而不是拿 `prev=?` 冒充
    /// 读失败、也不 `unwrap`、也不 `Err`。这条对面是 §5.14.2「界面把选区指向了一行已经
    /// 被 §5.14.4 删掉的记录」那一臂 —— 空态要读得出**为什么是空**，而不是把它吞成 0。
    #[test]
    fn neighbors_of_banner_for_names_a_missing_anchor_row() {
        let dir = sandbox("neighbors-explicit-missing");
        let cfg = falcon_core::config::History {
            enabled: true,
            keep_originals: false,
            thumb_px: 0,
            ..Default::default()
        };
        let frame = falcon_core::frame::Frame::filled(4, 4, [0, 0, 0, 255]).unwrap();
        let mut store = falcon_core::history::History::open(&dir).unwrap();
        store
            .record(
                &falcon_core::history::CaptureMeta {
                    at_ms: 1_700_000_000_000,
                    offset_min: -480,
                    monitor_id: "m",
                    rect: falcon_core::PhysRect::new(0, 0, 4, 4),
                    source: falcon_core::history::Source::Region,
                    cancelled: false,
                },
                &frame,
                &cfg,
            )
            .unwrap();
        drop(store);

        // 库里只有 id=1；问 id=99 ⇒ 直说没有这一行。
        let said = neighbors_of_banner_for(&dir, &cfg, 99);
        assert!(said.contains("anchor=99"), "{said}");
        assert!(said.contains("库里没有第 99 行"), "{said}");
        assert!(said.contains("prev=-"), "{said}");
        assert!(said.contains("next=-"), "{said}");
    }

    /// P113：§5.14.2 快捷键那一臂**只回答一个号**的那一支。三行库里从中间那行往两侧各翻一次，
    /// 交回的必须是**那一行的 id**而不是别的东西 —— 界面上一次按键能走到的只有这一个数，
    /// 它交给 `restore_history_selection` 之后动的是屏幕上那只框。
    #[test]
    fn neighbor_id_for_answers_the_row_to_land_on() {
        let dir = sandbox("neighbor-id-middle");
        let cfg = falcon_core::config::History {
            enabled: true,
            keep_originals: false,
            thumb_px: 0,
            ..Default::default()
        };
        let frame = falcon_core::frame::Frame::filled(4, 4, [0, 0, 0, 255]).unwrap();
        let meta = |at_ms: i64| falcon_core::history::CaptureMeta {
            at_ms,
            offset_min: -480,
            monitor_id: "m",
            rect: falcon_core::PhysRect::new(0, 0, 4, 4),
            source: falcon_core::history::Source::Region,
            cancelled: false,
        };
        let mut store = falcon_core::history::History::open(&dir).unwrap();
        let a = store
            .record(&meta(1_700_000_000_000), &frame, &cfg)
            .unwrap();
        let b = store
            .record(&meta(1_700_000_001_000), &frame, &cfg)
            .unwrap();
        let c = store
            .record(&meta(1_700_000_002_000), &frame, &cfg)
            .unwrap();
        drop(store);

        assert_eq!(neighbor_id_for(&dir, &cfg, b, true), Some(a));
        assert_eq!(neighbor_id_for(&dir, &cfg, b, false), Some(c));
        // 到顶那一格交回的是 `None` 而不是 `Some(自己)`：快捷键按到没有的那一侧，
        // 屏幕上那只框必须一动不动，而不是"翻回原处"假装翻过一次。
        assert_eq!(neighbor_id_for(&dir, &cfg, c, false), None);
        assert_eq!(neighbor_id_for(&dir, &cfg, a, true), None);
    }

    /// P113：`None` 那四格合一，而**其中一格是「库里没有这一锚」** —— §5.14.4 删掉了正看着的
    /// 那一行、用户再按一次方向键，走的正是这一格。它不许被读成"到顶了"，但那一格分别是
    /// [`neighbors_of_banner_for`] 的职责，本枚只保证一件事：**不动那只框**。
    #[test]
    fn neighbor_id_for_gives_up_on_an_anchor_that_is_not_there_or_a_switch_that_is_off() {
        let dir = sandbox("neighbor-id-gives-up");
        let cfg = falcon_core::config::History {
            enabled: true,
            keep_originals: false,
            thumb_px: 0,
            ..Default::default()
        };
        let frame = falcon_core::frame::Frame::filled(4, 4, [0, 0, 0, 255]).unwrap();
        let mut store = falcon_core::history::History::open(&dir).unwrap();
        let only = store
            .record(
                &falcon_core::history::CaptureMeta {
                    at_ms: 1_700_000_000_000,
                    offset_min: -480,
                    monitor_id: "m",
                    rect: falcon_core::PhysRect::new(0, 0, 4, 4),
                    source: falcon_core::history::Source::Region,
                    cancelled: false,
                },
                &frame,
                &cfg,
            )
            .unwrap();
        let later = store
            .record(
                &falcon_core::history::CaptureMeta {
                    at_ms: 1_700_000_001_000,
                    offset_min: -480,
                    monitor_id: "m",
                    rect: falcon_core::PhysRect::new(0, 0, 4, 4),
                    source: falcon_core::history::Source::Region,
                    cancelled: false,
                },
                &frame,
                &cfg,
            )
            .unwrap();
        drop(store);

        // 库里有两行，问一锚不存在的 ⇒ None。
        assert_eq!(neighbor_id_for(&dir, &cfg, 99, true), None);
        // 开关关着 ⇒ 连库都不打开，也是 None（与 `history_open_in` 的 Ok(None) 同一格）。
        let off = falcon_core::config::History {
            enabled: false,
            ..cfg.clone()
        };
        assert_eq!(neighbor_id_for(&dir, &off, later, true), None);
        // 对照：同一枚锚、开关开着、往**有**的那一侧问 ⇒ 答得出的就是那一行的 id。
        // 少了这一格，上面那三枚 `None` 可以来自任何一处而没人能排除。
        assert_eq!(neighbor_id_for(&dir, &cfg, later, true), Some(only));
    }

    /// §5.20.3 步骤 4 的那一句是纯函数,所以它自己得有证人:两项覆盖、其中一项要重启,
    /// 行里就得点名那一枚键、给出 from -> to,并且只有那一项带「需重启」。
    #[test]
    fn import_preview_names_each_key_and_marks_only_the_restarting_one() {
        let changes = vec![
            ImportChange {
                key: "hotkey.actions.capture".to_string(),
                from: "f13".to_string(),
                to: "ctrl+shift+a".to_string(),
                restart_required: true,
            },
            ImportChange {
                key: "appearance.theme".to_string(),
                from: "dark".to_string(),
                to: "light".to_string(),
                restart_required: false,
            },
        ];
        let s = render_import_preview(&changes, &[], ImportScope::Suggested);
        let lines: Vec<&str> = s.trim_end().lines().collect();
        // 头一行报数、两项各一行、外加一枚「这一侧没有」的声明 —— 四行,而空侧那半
        // 必须自己出声,不能靠行数被人脑补。
        assert_eq!(lines.len(), 4, "{s:?}");
        assert!(lines[0].contains("范围=建议范围"), "{:?}", lines[0]);
        assert!(lines[0].contains("将被覆盖 2 项"), "{:?}", lines[0]);
        assert!(lines[0].contains("被校正 0 项"), "{:?}", lines[0]);
        // 那句「将被覆盖 2 项」不是写给人看的形容词:数一覆盖的行,必须是同一个 2。
        let rows = lines.iter().filter(|l| l.starts_with("  覆盖 ")).count();
        assert_eq!(rows, 2, "{s:?}");
        assert_eq!(lines[3], "  （没有被校正的项）", "{:?}", lines[3]);
        assert!(
            lines[1].contains("hotkey.actions.capture"),
            "{:?}",
            lines[1]
        );
        assert!(lines[1].contains("f13 -> ctrl+shift+a"), "{:?}", lines[1]);
        assert!(lines[1].contains("需重启"), "{:?}", lines[1]);
        assert!(!lines[2].contains("需重启"), "{:?}", lines[2]);
        assert!(lines[2].contains("appearance.theme"), "{:?}", lines[2]);
    }

    /// 一份与本机相同的档:什么也不会被盖掉。这一支必须自己说「没有将被覆盖的项」,
    /// 而不是留下一截空白让人猜是没得改,还是这一路压根没跑。
    #[test]
    fn import_preview_says_so_when_nothing_would_be_overwritten() {
        let s = render_import_preview(&[], &[], ImportScope::All);
        assert!(s.contains("范围=全部"), "{s:?}");
        assert!(s.contains("将被覆盖 0 项"), "{s:?}");
        assert!(s.contains("（没有将被覆盖的项）"), "{s:?}");
        assert!(s.contains("（没有被校正的项）"), "{s:?}");
        // 「没有覆盖行」问的是行,不是整段:头那一行本来就写着「将被覆盖 0 项」。
        assert!(!s.lines().any(|l| l.starts_with("  覆盖 ")), "{s:?}");
        assert_eq!(s.trim_end().lines().count(), 3, "{s:?}");
    }

    /// 校正与覆盖是两笔账:`parse_import` 把越界的值夹回来的那些,得跟着预览一起报,
    /// 否则用户确认的是一份已经被改过的档而不自知(§5.20.3 步骤 4 的「显示将被覆盖的
    /// 项目」与 §8.3 的「不当声闷」是同一笔债)。
    #[test]
    fn import_preview_reports_corrections_beside_overwrites() {
        let changes = vec![ImportChange {
            key: "output.format".to_string(),
            from: "png".to_string(),
            to: "jpg".to_string(),
            restart_required: false,
        }];
        let warnings = vec!["capture.magnifier_zoom 越界,已夹回 12".to_string()];
        let s = render_import_preview(&changes, &warnings, ImportScope::Suggested);
        assert!(s.contains("将被覆盖 1 项"), "{s:?}");
        assert!(s.contains("被校正 1 项"), "{s:?}");
        assert!(
            s.contains("校正 capture.magnifier_zoom 越界,已夹回 12"),
            "{s:?}"
        );
        assert!(!s.contains("（没有被校正的项）"), "{s:?}");
    }

    /// §5.19.1 第 1 步那三分岔本体逐臂各问一次:跟随系统时机器答 light / dark / 不答,
    /// 手动钉的那一臂必须不被机器的答案挪走。机器不答那一臂单独问,因为「读不到」在
    /// 台账里既不是 light 也不是 dark,而 `(unreadable)` 这一格印错方向就等于把失败读成
    /// 成功(§7.2「不当声闷」的同一族)。
    #[test]
    fn theme_banner_expands_system_but_manual_theme_ignores_the_machine() {
        use falcon_core::config::Theme;

        let system_light = format_theme_banner(Theme::System, Some(true));
        assert!(
            system_light.contains("appearance.theme=System"),
            "{system_light:?}"
        );
        assert!(
            system_light.contains("system.theme=light"),
            "{system_light:?}"
        );
        assert!(
            system_light.contains("resolved.theme=Light"),
            "{system_light:?}"
        );

        let system_dark = format_theme_banner(Theme::System, Some(false));
        assert!(system_dark.contains("system.theme=dark"), "{system_dark:?}");
        assert!(
            system_dark.contains("resolved.theme=Dark"),
            "{system_dark:?}"
        );

        // 机器没答:读数那一格写 `(unreadable)`,生效那一格落在 Light —— 与 core 里
        // `resolve_theme` 的 `None` 臂同一口径,也是 Windows 自己的兜底方向。
        let unreadable = format_theme_banner(Theme::System, None);
        assert!(
            unreadable.contains("system.theme=(unreadable)"),
            "{unreadable:?}"
        );
        assert!(
            unreadable.contains("resolved.theme=Light"),
            "{unreadable:?}"
        );

        // 手动钉的那两臂:机器答什么都不得改动生效值,而且「用户的请求」那一格必须
        // 照原样印出来,不能被 resolved 覆盖成同一枚。
        let pinned_dark = format_theme_banner(Theme::Dark, Some(true));
        assert!(
            pinned_dark.contains("appearance.theme=Dark"),
            "{pinned_dark:?}"
        );
        assert!(
            pinned_dark.contains("system.theme=light"),
            "{pinned_dark:?}"
        );
        assert!(
            pinned_dark.contains("resolved.theme=Dark"),
            "{pinned_dark:?}"
        );

        let pinned_light = format_theme_banner(Theme::Light, Some(false));
        assert!(
            pinned_light.contains("resolved.theme=Light"),
            "{pinned_light:?}"
        );
    }

    /// 反向对照:任何一臂都不得把 `System` 交回给界面 —— 那是一枚「还没展开的问题」,
    /// 而不是一种画法。这条腿问的是整行的形状而不是某一格,所以它跟着上一枚的六臂
    /// 一起跑,免得日后有人往 `resolve_theme` 里添第四条臂。
    #[test]
    fn theme_banner_never_hands_system_back_as_the_resolved_one() {
        use falcon_core::config::Theme;

        for requested in [Theme::System, Theme::Light, Theme::Dark] {
            for system_light in [Some(true), Some(false), None] {
                let line = format_theme_banner(requested, system_light);
                assert_eq!(line.lines().count(), 1, "{line:?}");
                assert!(
                    !line.contains("resolved.theme=System"),
                    "the resolved field is still a question: {line:?}"
                );
                assert!(
                    line.contains("resolved.theme=Light") || line.contains("resolved.theme=Dark"),
                    "the resolved field is neither look: {line:?}"
                );
            }
        }
    }

    /// P119：§5.19.1 第 2 步的分岔本体，九臂逐一点名（三枚 `requested` × 三枚机器的 answer）。
    ///
    /// 这一格不许由「反正 `resolve_theme` 在 core 里测过」代劳：`theme_face` 是界面唯一读得到
    /// 的那一步，而它把 `Dark` 之外的两臂合成 `"light"` 这件事自己就可以写反 —— 写反了 core
    /// 的九格照样全绿。臂不合并、不省略：`(System, None)` 与 `(Dark, None)` 走的是同一个
    /// `None`，落到两枚不同的词上。
    #[test]
    fn theme_face_answers_nine_arms_and_never_a_third_word() {
        use falcon_core::config::Theme;

        assert_eq!(theme_face(Theme::System, Some(true)), "light");
        assert_eq!(theme_face(Theme::System, Some(false)), "dark");
        assert_eq!(theme_face(Theme::System, None), "light");
        // 手动两臂不吃机器的 answer：这三行与上面三行同输入不同答，正是「钉了就照钉的走」。
        assert_eq!(theme_face(Theme::Light, Some(true)), "light");
        assert_eq!(theme_face(Theme::Light, Some(false)), "light");
        assert_eq!(theme_face(Theme::Light, None), "light");
        assert_eq!(theme_face(Theme::Dark, Some(true)), "dark");
        assert_eq!(theme_face(Theme::Dark, Some(false)), "dark");
        assert_eq!(theme_face(Theme::Dark, None), "dark");
    }

    /// #281 那一族（「显示的那份」与「应用的那份」是两次调用）在这一腿上反着判：banner 里
    /// `resolved.theme=` 那一臂与界面绑上去的 `theme_face` 必须由**同一对输入**走出来、且同指
    /// 一臂。可红的两条路各点名一次 —— 把 `theme_face` 的 `Dark => "dark"` 写反，九臂那格先红、
    /// 这一格跟着红；把 banner 的 `resolved` 换成 `requested`（`System` 也被当成一臂印出去），
    /// 只有这一格红。
    #[test]
    fn theme_face_and_banner_point_at_the_same_arming() {
        use falcon_core::config::Theme;

        for requested in [Theme::System, Theme::Light, Theme::Dark] {
            for system_light in [Some(true), Some(false), None] {
                let face = theme_face(requested, system_light);
                let line = format_theme_banner(requested, system_light);
                let want = if face == "dark" {
                    "resolved.theme=Dark"
                } else {
                    "resolved.theme=Light"
                };
                assert!(
                    line.contains(want),
                    "the banner arms {} while the face arms {:?}",
                    line,
                    face
                );
                assert!(
                    face == "light" || face == "dark",
                    "a third word reached the interface: {face:?}"
                );
            }
        }
    }
}
