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

// §5.5.11 `hide_pins_on_dialog` 这枚键**没有**单独的一行读者，而这一次的理由与建它那一轮不同：
// 调用点自 P123/P124 起是真的了（[`crate::session`] 里 `dialog_opened` 每一扇对话框现问一次），
// 而它问的是 `output_cfg()` 那一次整段 clone —— 与启动横幅 `[output cfg]` 同一枚读者。为这一枚
// 键再补一行 `fn hide_pins_on_dialog() -> bool` 就是把同一件事交给两处（#281）：屏幕上藏没藏
// 由桥那一侧读的那一枚说了算，界面不再递一枚布尔进来。
// 一枚没有调用点的一行读者在 bin crate 里就是 dead_code warning，而本档不留 `#[allow]`
// （P75-A 撤掉最后那一枚）—— 这一条自此用不上，留着是因为它就是上面那句的同一枚理由。

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

/// §5.5.4–14 那一整段 `[pin]` —— [`falcon_core::config::Pin`] 上的**每一枚**键。P64 之前这一族在
/// `crates/core/src/config.rs` 里每枚都有键、每枚都有 `Default`，但 [`crate::state::PinState::new`]
/// 用 `..Default::default()` 把整段 `[pin]` 挡在运行期之外 —— 也就是**每一枚都没有读者**。
///
/// 这一枚读者一次性把整段键接进 [`crate::state::PinState::cfg`]：`PinState::new()` 从这里取一次
/// 快照，之后 `spawn(... &self.cfg)` 用它建新 pin（`crates/core/src/pin.rs:459`
/// `topmost: cfg.default_topmost` 那一行自此真的会读到用户写的值而不是硬编码 `true`）。
///
/// 那一串枚数不在这里点名：名单的唯一作者是 [`falcon_core::config::Pin`] 那个 struct 自己，而这一
/// 层每加一枚键都要来改写一句散文，就是 #455 拦的那件事（那份清单只会比真相旧一天）。
///
/// 只取一次快照而不是每枚键单独走一次 `with`：`PinState::new()` 只在 STATE OnceLock 初始化时
/// 跑一次，取整段 clone 比逐枚取更贴合"这一场进程看到的 [pin] 段"这个语义 —— 若中途
/// 有人 `write_back` 改了 [pin] 段，进程已经在跑的那些 pin 也不会跟着抖，和 PRD §5.5 里
/// "默认"两字的直觉一致（默认是**新**贴图的默认，不是**所有**贴图的当前）。
pub fn pin_cfg() -> falcon_core::config::Pin {
    with(|s| s.config.pin.clone())
}

/// §5.8.4 方式二第 3 步那枚「具体行为由设置决定」的**现读**：`[pin] drop_replaces_current`。
///
/// 为什么这一枚不像 [`pin_cfg`] 那样吃启动快照：那一枚快照说的是「新贴图的默认长相」——它属于
/// **一枚贴图**，贴图一旦建出来，那几格就归它自己了，中途改设置不该让桌上已有的窗抖一下。这一枚
/// 说的却是「**这一趟**落下往哪儿装」，它属于一个正要发生的事件：用户在设置页把那一格扳过去，
/// 下一次拖进来就该按新的那一格走（§5.20.2「修改后即时生效」在那一节里的形状）。让新拖进来的
/// 那一批还读三分钟前那次启动的快照，就是让用户反复试那一枚看着坏掉的开关。
///
/// 落点本身怎么算不在这里判：那一张表只有一个作者（[`falcon_core::drop::Landing::plan`]，由
/// [`crate::state::Decision::at`] 问一次），这一手只交回那一枚开关的读数。
pub fn drop_replaces() -> bool {
    with(|s| s.config.pin.drop_replaces_current)
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
/// 每次按下开始重新问一遍 —— 于是上面那句"11 枚停在键在档里没人读"到今天只剩一枚成立：
/// `highlight_element`。其余各枚都问得出读者 —— `include_cursor` 在
/// [`crate::capture::cursor_overlay`]（每次贴指针现问）、`magnifier_*` 五枚在
/// [`falcon_core::capture`] 拼放大镜参数那一手、`snap_to_window` 在 [`crate::mask::MaskState::release`]、
/// `delay_seconds` 与 `size_unit` 各在 [`crate::delay::arm_from_config`] 与本文件的
/// [`crate::mask::MaskState::open`] 那一行。
///
/// `default_mode` 与 `freeze_frame` 自本轮起也有行为读者了：[`crate::mask::MaskState::apply_opening`]
/// 在**每场 `open` 现读一次**这枚快照（前者定遮罩进场那一刻选区停在哪一块，判的本体是
/// [`crate::mask::MaskState::opening_hole`]；后者答的是「这一版产品没有实时取景那一路」，本体是
/// [`crate::mask::MaskState::freeze_notice`]）。与 `include_cursor` / `size_unit` 同一族口径：改完档，
/// 下一次按下热键就用新的，而已经开着的这一场不动（§5.20.2「设置应即时生效」在遮罩这一路的形状）。
pub fn capture_cfg() -> falcon_core::config::Capture {
    with(|s| s.config.capture.clone())
}

/// §5.1：整段 `[general]` 由这一枚 reader 一次性取快照 —— P65 之前 6 枚键（`launch_at_startup`
/// / `start_minimized` / `show_tray` / `close_to_tray` / `confirm_exit` / `language`）全部停在
/// "键在档里没人读"：`crates/qt_bridge/src/hotkey.rs:352` 那一句 `Some(TraySpec { ... })` 是
/// P50 落 tray 那轮的硬编码，用户把 `[general] show_tray = false` 写进档里也不会让托盘消失。
/// P66 把 `show_tray` 接进 `Pump::start` 的 tray 参数（`Some` ↔ `None` 由这一枚 clone 出来
/// 的 `general.show_tray` 决定）。
///
/// `launch_at_startup` 自本轮起**不再只是快照里的一个字段**：它有两只手，都不走这一枚快照 ——
/// * [`crate::settings_view::write`] 在 [`falcon_core::config::AUTOSTART_KEY`] 那一枚键上拧
///   `HKCU\...\Run` 里那条启动项（`platform_windows::autostart::sync`），系统拒了的那一趟
///   档一个字也不写、页上那句说的是原因；
/// * [`startup_autostart_banner`] 在启动时读一次机器（`autostart::inspect`，只报告不改动），
///   以机器为准回填这一枚键。
/// 于是「档里写着开而注册表里没有」这一族在这一档里没有可达的写法，而这一枚键仍是
/// `RESTART_KEYS` 里的那一枚：启动项是立刻写进机器的，**用它登录一次**才是它生效的时候。
///
/// 其余 4 枚键（`start_minimized` 需要 main window 初始化时机、`close_to_tray` 需要窗口关闭
/// 拦截、`confirm_exit` 需要对话框、`language` 需要 i18n 装填）本轮**由快照字段而不是由独立
/// 调用点**建了读者：`hotkey::install` 里 clone 一次整段 `[general]`，`show_tray` 那一支真
/// 分岔，其余四支**待改码接进各自的调用点**（§5.15 那一族）。与 [`pin_cfg`] / [`capture_cfg`]
/// 同形：一次 clone 整段快照，而 `launch_at_startup` 那一支自本轮起每次问的是机器。
pub fn general_cfg() -> falcon_core::config::General {
    with(|s| s.config.general.clone())
}

/// §5.15：整段 `[hot_corner]` 由这一枚 reader 取快照 —— P140 之前这一族十枚键（`enabled` /
/// `corners.` 四角 / `trigger_dip` / `dwell_ms` / `require_ctrl` / `require_shift` /
/// `repeat_guard_ms` / `pause_on_fullscreen`）在 `crates/core/src/config.rs` 里每枚都有键、
/// 每枚都有 `Default`、`validate()` 也夹了那三枚数，可整个 crate 之外**一处调用点都没有** ——
/// `falcon_core::hotcorner` 那台纯状态机写完之后从没被人喂过一个指针，用户写
/// `[hot_corner] enabled = true` 与不写是同一件事。
///
/// 与 [`pin_cfg`] / [`capture_cfg`] / [`general_cfg`] 同一形状：一次 clone 整段快照。但这一枚
/// **不是「进程启动那一次读」**，而是每次采样读一次 —— 角落几何要跟着显示器热插拔走，而
/// [`crate::hot_corner::Runner::refresh`] 只在快照真的变了才重建那台机器，所以「每枚一秒问一次」
/// 的成本是一次 clone，收益是 `--hotcorner` 门上「改了档、角落跟着改」这一腿能被观测。
pub fn hot_corner_cfg() -> falcon_core::config::HotCorner {
    with(|s| s.config.hot_corner.clone())
}

/// §5.18.2：整段 `[tray]` 由这一枚 reader 取快照，四位读者 = 托盘那四枚键。
///
/// **每一次点击现问一次**，而不是进程启动那一次读：四枚键全不在 [`RESTART_KEYS`] 里，
/// 「改了档、下一次点击就跟着变」是 §5.18.2 第四步的字面要求，而这一族的成本与
/// [`hot_corner_cfg`] 同形 —— 一次 clone，点击不是每秒一百次。
pub fn tray_cfg() -> falcon_core::config::Tray {
    with(|s| s.config.tray.clone())
}

/// §5.16.1 第十三名那一枚命令的存储键住在 `[hotkey] command`，整段 `[hotkey]` 因此有了这一枚
/// reader。**每一次按下现问一次**，与 [`tray_cfg`] 同一理由：`command` 不在 [`RESTART_KEYS`] 里，
/// 而「改了档、下一次按下去跑的就是新那一串」是这一格唯一能被观测的样子 —— 启动那一次读会把
/// 改档与按下之间的那一段时间做成一句假话。
pub fn hotkey_cfg() -> falcon_core::config::Hotkey {
    with(|s| s.config.hotkey.clone())
}

/// §5.4.4 / §5.4.5：整段 `[whiteboard]` 由这一枚 reader 取快照，唯一的读者 = 开白板那一步。
///
/// **每一趟现问一次**，与 [`tray_cfg`] 同一理由：两枚键都不在 [`RESTART_KEYS`] 里，而白板是
/// 一次"点一下才有的东西"，不是每秒一百次的采样 —— 改了底色，下一次点「白板」就该是新那一身。
pub fn whiteboard_cfg() -> falcon_core::config::Whiteboard {
    with(|s| s.config.whiteboard.clone())
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
/// `data_dir` 两格 ⇒ `#[allow(dead_code)]` 撤。剩下三格自此各有自己的行为读者，而**没有一格
/// 从这里读**（这一枚函数只交回快照，判据长在别处）：
/// * `cli_enabled` —— [`crate::cli_gate`]（`main.rs`，一趟 argv 被 [`crate::cli::classify`] 判成
///   `Run` 之后现读一次；关着 ⇒ 终端上读到一句点名这枚键的拒绝并退 1，而不是静默退 0。闸的只有
///   §5.17 那一臂：`NotCli` / `Help` / `Error` 三臂原样放行，出厂 `false` 才不会把十九扇量具门、
///   四档文件命令与常驻应用一起关掉）。
/// * `privacy_no_temp_files` —— [`crate::mask::MaskState::sweep_temp_at_open`]（每场遮罩进场前
///   现读一次，收回本程序自己那枚带标签的 `%TEMP%\falconshot-sendto` 里属于过去流程的旧图；
///   分岔本体是 [`crate::mask::MaskState::sweep_stale_temp`]。另一枚临时目录 `falconshot-clip`
///   不归它 —— 那一枚的死活由「剪贴板还引用着就别删」那条规矩问剪贴板，读者是
///   [`crate::state::PinState::sweep_clip_files`]，两处各按各自那一问）。
/// * `crash_recovery` —— 仍待 §7.2 的 recovery snapshot 那一路。
///
/// `recognize_text` 已由 P148 接进 `MaskState::record_history` 的确认臂。
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

/// P73：把整段 `[appearance]` 的键逐字回读成一行。
///
/// **本轮没有分岔本体**（P64 / P65 / P66 / P69 / P70 / P71 那一族"读者-only 无分岔 ⇒ 撤单测"
/// 本轮沿用）：`theme` 是 enum 走 `{:?}`（`System` / `Light` / `Dark` 三臂、本轮不加"System
/// 展开成实际生效的一臂"那种分岔，那需要 `QGuiApplication::styleHints()->colorScheme()` 的
/// 运行期读数、本轮 `settings::start()` 早于 Qt 存在 ⇒ 无法读）、其余各枚是 String / u32 直读。
///
/// **P149 起本行是 9 枚而不是 8 枚**：`appearance.ui_font_family`（§5.19.4「调整界面字体」里
/// **字体**那一半，字号是 `ui_font_size`）在 core 侧建键时一并进了 [`falcon_core::config::Appearance`]，
/// 而这一行是「整段回读」的证人——漏一枚就是「导出/导入之外还有一份没人念过的档」。名单与
/// `core::config::tests::the_appearance_section_is_nine_keys_all_reachable` 数的是同一批。
///
/// **证人 dichotomy 的第 69 项 ⑫ 那一族本轮回到"读者-only ⇒ 撤单测"一侧**（P67 / P68 / P72
/// 三轮分岔+单测之后、本轮同 P64 / P65 / P66 / P69 / P70 / P71 六轮那一族 ⇒ **本轮不加 cargo
/// 单测**、qt 计数仍 207、三枚之和仍 499，本轮的证人是梯 rc 0 + exe sha 换）。
pub fn format_appearance_banner(a: &falcon_core::config::Appearance) -> String {
    format!(
        "appearance.theme={:?} appearance.accent={} appearance.toolbar_placement={} appearance.ui_font_size={} appearance.ui_font_family={} appearance.magnifier_border={} appearance.magnifier_corner_radius={} appearance.magnifier_foreground={} appearance.magnifier_background={}",
        a.theme,
        a.accent,
        a.toolbar_placement,
        a.ui_font_size,
        a.ui_font_family,
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

/// §5.19.2「主题色」交到界面那一侧的**那一枚色**：用户钉的 `accent` 先过对比度闸
/// [`falcon_core::config::accent_on`]、再按**当前生效的那套餐具**夹到底色上。
///
/// **分岔本体是两格，都是纯的**：`face` 的两臂（`"dark"` 吃 [`falcon_core::config::SURFACE_DARK`]、
/// 其余一律吃 `SURFACE_LIGHT`，与 `theme_face` 丢掉 `Option<bool>` 那一形状同一个「没有第三臂」
/// 的口径）× `accent` 那枚字面在**这一面底**上够不够 [`falcon_core::config::ACCENT_MIN_CONTRAST`]
/// （够 ⇒ 原样；不够 ⇒ core 只往能看见的那个方向夹明度）。界面因此永远拿到一枚看得见、
/// 且仍然是用户挑的那枚色相的色——§7.4「所有主题下均应满足基本可读性和对比度要求」在
/// **读**的那一刻落地，而不是等画师逐窗核对。
///
/// 分岔是纯函数 ⇒ 不撤单测（证人 dichotomy 的第 69 项 ⑫ 那一族、P118 `format_theme_banner` /
/// P119 `theme_face` 同形）。**这一枚不裹一层「现读档」的 `startup_*`**：界面那一侧四枚外观键
/// 由 session.rs 的 `appearance_words` **一次锁、一份快照**地读，`theme_face` 由调用方递进来——
/// 因为底名必须吃**当前生效的那套餐具**，而不是各走各的再去问一次注册表。自己现推导底名，
/// 用户钉了 dark、机器是浅色，页上就会「深色餐具配浅色底夹出来的色」。
pub fn accent_face(a: &falcon_core::config::Appearance, face: &str) -> String {
    let surface = if face == "dark" {
        falcon_core::config::SURFACE_DARK
    } else {
        falcon_core::config::SURFACE_LIGHT
    };
    falcon_core::config::accent_on(&a.accent, surface)
}

/// §5.19.4「支持调整界面字体和字号」交给界面的两枚字：族（family）与**磅**（不是像素）。
///
/// **磅这一枚没有分岔**——档里存的就是磅（[`falcon_core::config::Appearance::ui_font_size`]），
/// QML 绑 `font.pointSize` 就是它，DPI 由 Qt 按那一面屏自己乘；`ui_font_px` 那一枚换算只服务
/// 「要说出一枚像素数」的读者（core 头顶那封信）。族同样直读：空族在**读档那一步**就被
/// `validate` 归成出厂族（`ui_font_family_has_a_reader_side_floor_and_survives_a_round_trip`
/// 在 core 侧盯着），所以界面这一侧永远不会拿到一枚空串把系统字体挤掉——**这里不再兜第二次底**，
/// 兜底只有 validate 那一处（#281：同一句判断只许一处回答）。
///
/// 分岔本体是 `family.trim().is_empty()` 的**反面**：本轮没有——所以这两枚是纯读者、
/// 没有新的单测要加在这一格上（core 那侧的往返单测就是它们的对面）。
pub fn ui_font_face(a: &falcon_core::config::Appearance) -> (i32, String) {
    (
        a.ui_font_size as i32,
        a.ui_font_family.clone(),
    )
}

/// 界面字体的**出厂那一枚磅**，也就是页上那套字号层次所围绕的基准。
///
/// 为什么要单独递出去（而不是让 QML 自己写 `ui_font / 9`）：页上画的是有层次的三十多枚字号
/// （正文 13、说明 12、标题 14/18），把整页压成一枚磅数会把那层结构抹平，所以界面乘的是
/// 「此刻的磅 ÷ 出厂的磅」那一枚倍数 —— 而分母正是 core 那一枚出厂值，页上不许抄第二份数字
/// （#281：默认值只有一个作者）。现问 [`falcon_core::config::Config::default`]，所以 core 那侧
/// 改了出厂磅，页上的基准跟着改，不需要谁记得去改 QML。
pub fn ui_font_baseline() -> i32 {
    falcon_core::config::Config::default().appearance.ui_font_size as i32
}

/// §5.19.4「调整工具栏方向和位置」里**落点**那一枚交给界面的词。
///
/// 闭集在 core（[`falcon_core::config::TOOLBAR_PLACEMENTS`]，五臂 `auto`/`top`/`bottom`/`left`/
/// `right`）：页上那一格打不进名单外的字（`set_str` 拒）、档里读来的垃圾由 `validate` 归一
/// 并说出来，所以**界面这一侧拿到的永远是名单里的那五枚之一**，这里不需要、也不许再兜一次
/// 「认不出就算 auto」——那是第二份判断（#281）。`auto` 的展开不在这里做：那一枚要吃**选区**
/// 的长短边，而选区是遮罩那边的事实（[`falcon_core::config::toolbar_edge`] 由 `mask` 那一侧
/// 带着宽高现问）。
///
/// 与上面两枚同一口径：**不裹 `startup_*`**——页上四枚外观值一次读一份快照，这里只是那一份里
/// 的一格。
pub fn toolbar_placement_face(a: &falcon_core::config::Appearance) -> String {
    a.toolbar_placement.clone()
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

/// §5.1.2 启动回填的三臂判据。**机器是那一枚开关的真相**，所以不一致时以机器为准改档，
/// 而不是反过来拿档去拧机器 —— 后者会让一次 `cargo run`（`target\debug` 那条路径）替安装好的
/// 那一台重写启动项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutostartRead {
    /// 机器没答上来（那一格读不动、或这台不是 Windows）：**档里那一格不动**，页上照档里说。
    /// 一条读不到的启动项既证明不了「没开」，也就没资格把用户那一枚改成 `false`。
    Silent(String),
    /// 机器与档说的是同一件事：一个字节也不动。
    Agrees {
        file: bool,
        held: platform_windows::autostart::Held,
    },
    /// 机器与档不一样：以机器为准，档里那一枚该改成 `machine`。
    Drifts {
        file: bool,
        machine: bool,
        held: platform_windows::autostart::Held,
    },
}

/// 判据本体，纯的：`read` 是**机器那一侧**交回来的位置或一句原因。单测对着这一枚问
/// 「档与机器不一致时该改哪一头」，一台有注册表的机器都不是这一问的前提。
pub fn autostart_backfill(
    file_says: bool,
    read: Result<platform_windows::autostart::Held, String>,
) -> AutostartRead {
    match read {
        Err(why) => AutostartRead::Silent(why),
        Ok(held) => {
            let machine = held.switch_on();
            if machine == file_says {
                AutostartRead::Agrees {
                    file: file_says,
                    held,
                }
            } else {
                AutostartRead::Drifts {
                    file: file_says,
                    machine,
                    held,
                }
            }
        }
    }
}

/// 一行说完这一趟启动回填，给 `main.rs` 的 `[autostart cfg]` 那一行。
///
/// `saved_ms` 是**真落了盘**的那一次的毫秒数，`None` = 这一趟没写文件。那一格不许由调用方
/// 猜：「一致所以没写」与「改了档而没写成功」是两件要分开说的事（§9.4 对失败保存定的规矩）。
pub fn format_autostart_line(read: &AutostartRead, saved_ms: Option<u64>) -> String {
    use platform_windows::autostart::held_line;
    let wrote = match saved_ms {
        Some(ms) => format!("（以机器为准改档，落盘 {ms} ms）"),
        None => String::new(),
    };
    match read {
        AutostartRead::Silent(why) => {
            format!("读不到启动项：{why}（档里那一格未动，页上那一枚照档里说）")
        }
        AutostartRead::Agrees { file, held } => {
            format!("档里={file} 机器={} ⇒ 一致{wrote}", held_line(held))
        }
        AutostartRead::Drifts {
            file,
            machine,
            held,
        } => format!(
            "档里={file} 机器={} ⇒ 档里改成 {machine}{wrote}",
            held_line(held)
        ),
    }
}

/// §5.1.2 的启动腿：读一次机器里的启动项、用它回填 `[general] launch_at_startup`，交回一行
/// 给 `main.rs` 打印。
///
/// 这一格存在的对面是「拨上去没反应」那一族欠账的另一半：页上写的是档里那一枚，而档里那一枚
/// 可以由别人改（手改文件、导入、上一场安装在另一条路径上留下的启动项）。启动时以机器为准对
/// 一次账，页上那一枚开关从此说的是「下一次登录会不会启动」，而不是「文件里上次写了什么」。
///
/// 只有真不一致才写盘：默认那一趟（档里 `false`、机器里没有启动项）一个字节也不动。
pub fn startup_autostart_banner() -> String {
    let read = autostart_backfill(
        general_cfg().launch_at_startup,
        platform_windows::autostart::inspect().map_err(|e| e.to_string()),
    );
    let saved = match &read {
        AutostartRead::Drifts { machine, .. } => Some(with(|s| {
            // 直接改这一枚 bool 字段，而不是绕 [`falcon_core::config::AUTOSTART_KEY`] 那一条
            // `settings_view::write`：回填的读数出自机器而不是用户的输入，没有语法要过，而那条
            // 路会**再拧一次启动项** —— 拿回填去改机器是本节最不该做的事。
            s.config.general.launch_at_startup = *machine;
            s.save()
        })),
        _ => None,
    };
    format_autostart_line(&read, saved)
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
/// 下面那段注释里，而那一封自 P155 起说的是「调用点到了，所以更要只留一处读者」而不是「调用点
/// 还没到」：`crate::session::dialog_opened` 每一扇对话框现问的就是这一枚函数交出去的那一份。
pub fn output_cfg() -> falcon_core::config::Output {
    with(|s| s.config.output.clone())
}

/// §5.8.5 / §5.8.6：整段 `[card]` 由这一枚 reader 送进运行期 —— 那四枚键（`width` /
/// `font_size` / `font_family` / `background`）在 P126 之前只有 `config.rs` 里的键与
/// `Default`，一个读者也没有，于是 `crates/core/src/card.rs` 造出来也没人能给它一份设置。
///
/// 与 [`pin_cfg`] / [`output_cfg`] 同一形状（一次 clone 整段快照，调用点拿到的是一份借用
/// 生命周期内不会变的设置），差别在**问的时机**：`pin_cfg` 在 `PinState::new()` 里问一次、
/// 也就是进程一次，而这一枚在**每一张贴图成型的那一刻**问一次。理由是 §5.8.5 的四枚键决定的
/// 是「这一张卡片长什么样」而不是「这个会话的窗口长什么样」—— 用户改完档再贴一张，贴出来的
/// 就该是新档里那张；把快照缓存进 STATE 反而是这里不采用的做法。
pub fn card_cfg() -> falcon_core::config::Card {
    with(|s| s.config.card.clone())
}

/// §5.7.21 / §5.7.22 的第一位读者：整段 `[annotation]` 由这一枚 reader 送进运行期。
///
/// 形状与 [`output_cfg`] / [`card_cfg`] 同族（一次 clone 整段快照），而**问的时机**与
/// [`card_cfg`] 同族而不是与 [`pin_cfg`] 同族：色带在每一扇遮罩窗创建的那一刻读一次，
/// 用户改完档再截一张，屏幕上就该是新档里那一排。
///
/// `Annotation::palette` 自 §5.20.3 起就能被档带着走（`Config::flat` 里有它自己那一行、
/// `ImportScope::Suggested` 覆盖它、`validate` 丢读不出来的并回填 `default_palette`），
/// 而本轮之前产品路径上没有一个人读它 —— 第 35 项 ⑨ 那一族的形状。工具栏那六枚色是
/// `AnnotationToolbar.qml` 自己写的字面量，与档里那十枚之间没有一条通路。
pub fn annotation_cfg() -> falcon_core::config::Annotation {
    with(|s| s.config.annotation.clone())
}

/// 色带走桥的那一份：档里的每一枚颜色经 [`falcon_core::colors::parse`] 归一成
/// `RRGGBB`（大写、无 `#`），`|` 联起来。**枚数与顺序由档决定**，这一枚函数只换写法。
///
/// 为什么不让页上自己解析：`colors::parse` 认 `#RGB` / `#RGBA` / `#RRGGBB` / `#RRGGBBAA` /
/// `rgb()` / `hsl()` 六族，而 QML 那一侧只认 `#` 那一族 —— 在页上再写一台解析器就是让
/// 同一件事有两个主人（P180 那一路由的口径）。alpha 在这一格落地为丢弃，因为递下去的门把手
/// 本来就是 `setColor(r, g, b)`（`mask_view.rs` 里那枚硬编码的 255），§5.7.21 第三步的
/// 「在主颜色按钮上滚动调整透明度」是那一枚门把手之后的账。
///
/// 读不出来的一枚不留，与 [`crate::annotate::Layer::apply_annotation_defaults`] 同一条理由：
/// [`crate::config`] 的 `validate` 载入时用的就是这台解析器，读不出来的已经丢档并回填了
/// `default_palette`，所以走到这一枚函数跟手里的颜色值颜色读得出来。
///
/// 空表 ⇒ 空串，而 `"".split("|")` 在 JS 里数出**一段空串**而不是零段（P92 / P93 钉过的
/// 那一格），所以页上先滤零长段。纯函数：输入是一份借用、输出是一枚串，没有档、没有窗柄、
/// 没有会话 ⇒ 分岔不走产品路径也要走单测。
pub fn band_colors_joined(colors: &[String]) -> String {
    colors
        .iter()
        .filter_map(|c| falcon_core::colors::parse(c))
        .map(|[r, g, b, _]| format!("{r:02X}{g:02X}{b:02X}"))
        .collect::<Vec<String>>()
        .join("|")
}

/// P76：把整段 `[output]` 的 `share_targets` + `send_to_apps` 两枚 Vec<String> 回读成一行。
///
/// P130 起这一行多出第五格 `output.copy_as_file`：§5.9.17 步骤 1 的那枚开关在设置面板里
/// 还没有对应的控件（面板是 §5.20 的那一批），而 `[output]` 里一枚没人读的键正是第 35 项 ⑨
/// 那一族的形状，所以启动横幅先把它逐字印出来——真正的读者是 `crate::session` 里的
/// `copy_pin`，这一格只是让"出厂到底是哪一支"在启动那一行上可读。
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
        "output.share_targets={} output.send_to_apps={} output.reuse_last_file={} output.hide_pins_on_dialog={} output.copy_as_file={}",
        shares, apps, o.reuse_last_file, o.hide_pins_on_dialog, o.copy_as_file
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
///
/// 「位置」那一节答的是 PRD §5.14.1 第三句「历史记录应包含时间、屏幕、位置、尺寸和截图内容」
/// 里那**一样**此前只在库里、没上过屏的：`x`/`y` 自 `history.rs:199-200` 建表就在写，而这一串
/// 只取了 `rect.w` 与 `rect.h` —— 于是清单把「在哪一块」说成了「有多大」，五样里少一样。那两个
/// 数**只从 `rec.rect` 来**：界面上、量具里都不许拿尺寸或屏名再推一次起点（#281），推出来的那份
/// 与 SQLite 里那一列没有任何东西保证相等。摆在尺寸**之前**是有意的两件事：位置与尺寸同属
/// 「那一块地方」，读的人先看在哪、再看多大。
pub fn history_rows_joined(records: &[falcon_core::history::Record], offset_min: i32) -> String {
    records
        .iter()
        .map(|rec| {
            let stamp =
                falcon_core::naming::LocalStamp::from_epoch(rec.captured_at / 1_000, offset_min);
            let mut label = format!(
                "{} · {:04}-{:02}-{:02} {:02}:{:02} · 位置 {},{} · {}×{} · {} · {}",
                rec.id,
                stamp.year,
                stamp.month,
                stamp.day,
                stamp.hour,
                stamp.min,
                rec.rect.x,
                rec.rect.y,
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
            // §5.16 那枚 `[advanced] recognize_text` 落进 `ocr_text` 的那一格的**第一位读者**
            // （P148）：库里那三种形状在清单里说三句话，一句也不并成一句 ——
            // 「没识别过」是 NULL、「识别过而一个字也没有」是空串、「有字」才数长度。
            // 数长度而不是判 `is_some()`，因为「那一行有文字」与「那一行跑过识别」是两件事，
            // 前者才是用户在清单里要挑出第 N 行时所指的「有字」。
            match rec.ocr_text.as_deref() {
                None => {}
                Some("") => label.push_str(" · 本机识别过，没有字"),
                Some(text) => {
                    // 字数按字符（`chars`）而不是按字节：中文一帧识别回来的字节数是字数的三倍。
                    label.push_str(&format!(" · 有识别文字 {} 字", text.chars().count()));
                }
            }
            label
        })
        .collect::<Vec<String>>()
        .join("|")
}

/// [`history_rows_joined`] 挂上产品路径的那一臂：[`history_open_in`] 的分岔开出去 ⇒
/// [`falcon_core::history::History::newest`] 取最新 `limit` 行（筛开着时是
/// [`falcon_core::history::History::cancelled_page`]）⇒ join。
/// **P98 起本函数是 `newest` 的第一位非 test 调用点**（P95 普查点名的那 11 枚 still-zero
/// 里的一枚，§10 第 98 项）。
///
/// 答法与 [`crate::mask::MaskState::replay_history`] 同一口径：`Ok(空串)` 是「库里没有行」
/// 这一次正常的空，`Err(那一句)` 是「历史没开」或「库打不开」或「那些行读不回」——把三种
/// 都说成空，等于让一枚坏掉的 `history.db` 读成一次正常清空。行数的**次序**也不在这里重定：
/// `newest` 自己按 `(captured_at, id)` 倒序，join 不排序、不去重，所以界面看到的就是库里
/// 排出来的那一份。
///
/// `cancelled_only` 是 PRD §5.14.3 步 3「找到标记为“已取消”的记录」里**筛**那一半，它落到
/// 的是**查询**（core `History::cancelled_page`，多一枚 `WHERE cancelled = 1`）而不是读完之后
/// 自己截掉：事后筛的那一页永远不知道「这一页短是因为到底了，还是因为中间那些没取消的行被
/// 我扔了」，屏上那句「清单到底了」就会说反话（§7.1 分页那一支的判据，见
/// [`crate::mask::MaskState::list_history`] 头上那三句答法）。空清单在这一读里还是**两种**
/// 不同的事：不筛时是「库里没有行」，筛过时是「库里没有已取消的行」——那一句由
/// [`crate::mask::MaskState`] 分开说，不在这里合并。
pub fn history_rows_at(
    dir: &std::path::Path,
    cfg: &falcon_core::config::History,
    limit: usize,
    offset_min: i32,
    cancelled_only: bool,
) -> Result<String, String> {
    match history_open_in(dir, cfg) {
        Ok(Some(store)) => {
            let rows = if cancelled_only {
                store.cancelled_page(None, limit)
            } else {
                store.newest(limit)
            };
            rows.map(|records| history_rows_joined(&records, offset_min))
                .map_err(|e| format!("{} 里那些行读不回（{e}）", dir.display()))
        }
        Ok(None) => Err(format!(
            "{} 里的历史没开（`[history] enabled` 是关的）",
            dir.display()
        )),
        Err(e) => Err(format!("打不开 {} 里的 history.db（{e}）", dir.display())),
    }
}

/// §7.1「历史记录加载应支持分页或虚拟列表」里**更旧那一页**的分岔本体：先按游标行号把那一行
/// 问回来，再按 `(captured_at, id)` 取比它更旧的 `limit` 行 ——
/// [`falcon_core::history::History::older_than`] 自此有了产品读者（此前它的调用点只有单测）。
///
/// 游标用的是**行号**而不是整行：界面那一头手里只有清单最后一行第一段那枚裸数字
/// （[`history_rows_joined`] 头顶钉下的口径），而 core 那一枚要的是一对 `(captured_at, id)`。
/// 「行号 → 那一行」这一步就住在本函数里，只此一份（#281）—— 页上不许替它猜一个时间戳，
/// `newest` 那一头也不许第二次把两列排出来。
///
/// 三种答法各说各的话（#453）：库里再没有更旧的行 ⇒ `Ok(空串)`，那是「读到底了」；游标那一枚
/// 已经不在库里 ⇒ `Err` 点它的名 —— 把「你手里最后一行刚被删掉」说成「到底了」，用户读到的
/// 就是一句反话；没开 / 打不开 / 那些行读不回 ⇒ 与 [`history_rows_at`] 同一句拒绝。
///
/// 「到底了」那一臂在筛开着时说的是**这张筛过的清单**到底了（库里再没有更旧的已取消行），
/// 而不是库里再没有行 —— 说这句话的 [`crate::mask::MaskState::list_history_older`] 手里有
/// `cancelled_only` 这一枚，它因此能分开那句中文（#453）。游标那一枚行号**不**跟着筛走：
/// `store.get(cursor)` 问的是屏上最后一行那一枚行号，那一行本来就是筛过之后留下来的，
/// 按 `cancelled` 再筛一次就把「这一行还在不在库里」问成了「这一行算不算数」。
pub fn history_rows_older_at(
    dir: &std::path::Path,
    cfg: &falcon_core::config::History,
    cursor: i64,
    limit: usize,
    offset_min: i32,
    cancelled_only: bool,
) -> Result<String, String> {
    match history_open_in(dir, cfg) {
        Ok(Some(store)) => {
            let was = store
                .get(cursor)
                .map_err(|e| format!("{} 里那一行读不回（{e}）", dir.display()))?;
            let Some(was) = was else {
                return Err(format!(
                    "{} 里没有第 {cursor} 行（它已被删除或清理，清单上那一行是旧的）",
                    dir.display()
                ));
            };
            let rows = if cancelled_only {
                store.cancelled_page(Some(&was), limit)
            } else {
                store.older_than(&was, limit)
            };
            rows.map(|records| history_rows_joined(&records, offset_min))
                .map_err(|e| format!("{} 里那些行读不回（{e}）", dir.display()))
        }
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

/// §8.7「提示用户清理历史或调整存储上限」那一句话本体：占用与上限进、一句中文出。纯函数，
/// 与 [`history_usage_line`] 同一族。
///
/// **判「顶到没顶到」不在这里**：那一问读的是与自动清理同一份上限语义（`0` = 这一枚没设），
/// 所以它住在 core 的 `falcon_core::history::Retention::at_cap` 里，本函数只把已经判出来的那一趟
/// 的数字说出来（#281）。这里也**不做单位换算**：`B` 与上面那行 §6.2 的占用同一枚单位，把字节
/// 除以 1048576 去凑档上那枚 `max_size_mb` 就是给同一道算术第二个作者，四舍五入还会替用户编出
/// 「0 MB 已到上限」那种话。
///
/// `max_items` 为 `0` 时说得出「此刻 N 条」而说不出条数上限——那一枚上限压根没设，此时顶到的
/// 是字节那一枚。两形都得说人话，不许退成一句「已到上限」了事。
pub fn history_pressure_line(records: u64, max_items: usize) -> String {
    let counted = if max_items > 0 {
        format!("此刻 {records} 条、上限 {max_items} 条")
    } else {
        format!("此刻 {records} 条（条数没设上限，顶到的是字节那一枚）")
    };
    format!(
        "历史已到存储上限（{counted}，占用见上一行）：要么在「首选项 > 历史记录」把上限调大，\
         要么删掉不要的那几行——自动清理只动未锁定的最旧记录，锁着的行不会被动。"
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

/// 一项一行的那一行,由 [`ImportChange`] 做成页上/档上的样子。
///
/// 它单独成手是因为 §5.20.3 步骤 4 现在有两个读者:CLI 那份文本与设置页那一列行。措辞写两遍
/// 就会分岔(「需重启」那三个字在哪儿出现),所以两边都叫这一枚 —— 容器各拼各的分隔符:
/// 档面用换行,桥那侧用 [`crate::settings_view::RECORD`]。
pub fn import_row_text(c: &ImportChange) -> String {
    let restart = if c.restart_required {
        "（需重启）"
    } else {
        ""
    };
    format!("覆盖 {} : {} -> {}{}", c.key, c.from, c.to, restart)
}

/// 那一列行上面的一句总数。与 [`import_row_text`] 同一理由:页与档都得说同一句「将被覆盖几项」。
pub fn import_note(changes: usize, warnings: usize, scope: ImportScope) -> String {
    let scope_name = match scope {
        ImportScope::All => "全部",
        ImportScope::Suggested => "建议范围",
    };
    format!("[import] 范围={scope_name}；将被覆盖 {changes} 项；被校正 {warnings} 项")
}

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
    let mut out = String::new();
    out.push_str(&import_note(changes.len(), warnings.len(), scope));
    out.push('\n');
    for c in changes {
        // 注释挂在行尾而不是单起一行:一行一项,读的人才能拿行数对上上面那个数。
        out.push_str(&format!("  {}\n", import_row_text(c)));
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

/// 一份档读进来、校验、按范围算出会盖掉哪些项 —— §5.20.3 步骤 4 的全部算术,只在这里做一次。
///
/// 它存在的原因是这一节现在有两个入口:命令行(`--import-config <档>`)与设置页上那两枚钮
/// (`config_import` 走前一个,[`crate::settings_view`] 走第二个)。预览那一半要是各数一遍,
/// 「将被覆盖几项」就会有两种答案,而页上那一列行恰恰是要给用户看的数(同一个理由见
/// [`import_row_text`])。
///
/// [`ImportRead::incoming`] 是校正过的那一份:步骤 5 写进本机的必须是同一份,而不是重新读一次原文。
pub struct ImportRead {
    pub incoming: Config,
    pub changes: Vec<ImportChange>,
    pub warnings: Vec<String>,
}

pub fn import_read(path: &Path, scope: ImportScope) -> Result<ImportRead, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (incoming, warnings) = Config::parse_import(&text).map_err(|e| e.to_string())?;
    let changes = with(|s| s.config.preview_import(&incoming, scope));
    Ok(ImportRead {
        incoming,
        changes,
        warnings,
    })
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

/// §5.20.3 步骤 5 的那一句:盘写完之后报一次。
///
/// 这里的「应用后又校正」与 [`render_import_preview`] 那句「被校正」不是同一份读数:
/// 那一份是 parse 时对**来的那份档**做的,这一份是逐项写进**这一台机器**之后才发现的
/// —— 两份档各自都合法、合起来撞一枚热键,只有这一份会说。写它成独立一手是为了能让测试
/// 读到:步骤 5 本体走的是进程级 Settings,测试碰不到。
///
/// 与预览同一道理,有校正与没校正两半都写得出行 —— 「零枚」和「没去看」在日志里得长得不一样。
pub fn render_import_applied(applied: usize, ms: u64, corrections: &[String]) -> String {
    let mut out = format!("[import] 已应用 {applied} 项，写盘耗时 {ms} ms（§5.20.3 步骤 5）\n");
    out.push_str(&format!(
        "[import] 应用后又校正 {} 项（写进本机之后门才发现的）\n",
        corrections.len()
    ));
    for c in corrections {
        // 一项一行:读的人拿行数才能对上上面那个数。
        out.push_str(&format!("  应用时校正 {c}\n"));
    }
    if corrections.is_empty() {
        out.push_str("  （应用时没有再校正任何项）\n");
    }
    out
}

/// §5.20.3 步骤 4 与步骤 5:读一份档、校验它、报出会盖掉哪些项;只有 `confirm` 才真写。
///
/// `confirm` 就是那一节里的「用户确认后应用」。没确认时除了预览那句,还专门补一行说
/// 未应用 —— 退出码 0 在这一支是「档读得通」的意思,不是「已经改了」。
pub fn config_import(path: &Path, scope: ImportScope, confirm: bool) -> Result<String, String> {
    let read = import_read(path, scope)?;
    let mut out = render_import_preview(&read.changes, &read.warnings, scope);
    if !confirm {
        out.push_str("[import] 未应用：加 --confirm 才会写入（§5.20.3 步骤 5）\n");
        return Ok(out);
    }
    let applied = read.changes.len();
    // [`Settings::save`] 把写盘失败记进 `problems` 而自己只返回耗时:内存里的值照改。
    // 那一手对设置页是对的(§9.4),对 CLI 不行 —— PRD §5.17 要的是「文件写失败不得报告
    // 成功」。所以这里看的是 save 前后 `problems` 长没长,长了就当 Err 报出去。
    let (outcome_ms, corrections) = with(|s| -> Result<(u64, Vec<String>), String> {
        let corrections = s
            .config
            .apply_import(&read.incoming, scope)
            .map_err(|e| e.to_string())?;
        let before = s.problems.len();
        let ms = s.save();
        if s.problems.len() > before {
            Err(s.problems[before].clone())
        } else {
            Ok((ms, corrections))
        }
    })?;
    out.push_str(&render_import_applied(applied, outcome_ms, &corrections));
    Ok(out)
}

/// §5.7.22 步骤 4 与步骤 5 之间的那一份读数：模型留下的那些枚，加上**这份文件自己**的账面。
///
/// 「谁是合法的一行、重复的留哪一枚」全部由 [`falcon_core::colors::Palette::from_text`] 判,
/// 这里不另写一套规矩;记下来的是它没说的三笔账 —— 几行、几行是空行、几行读不出来。一份档
/// 被丢下两行而报告里一个字也没有,与安静地不同意是同一种坏(§8.3)。
pub struct PaletteRead {
    pub palette: falcon_core::colors::Palette,
    pub lines: usize,
    pub blank: usize,
    pub dropped: Vec<String>,
}

impl PaletteRead {
    /// 读得出、却没留下一枚的那些行 —— [`falcon_core::colors::Palette::add`] 那条
    /// 「已经在就不再加」(§5.7.22 步骤 2「调色板不该每点一次就长一枚」)在这一份档上的账面。
    ///
    /// 每一枚加数都是量出来的:一行必定属于空行 / 读不出 / 读得出三者之一,而读得出的那些行
    /// 恰由「留下的」与「重复的」两堆组成。所以这一式不会算出负数,除非有人往这个结构里
    /// 塞了一份不是 `palette_read_text` 读出来的读数。
    pub fn repeats(&self) -> usize {
        self.lines - self.blank - self.dropped.len() - self.palette.colors.len()
    }
}

/// 拿产品那一只手读一份调色板档的文本。纯函数:进一份借用、出一份读数,没有档、没有窗柄。
pub fn palette_read_text(text: &str) -> PaletteRead {
    PaletteRead {
        palette: falcon_core::colors::Palette::from_text(text),
        lines: text.lines().count(),
        blank: text.lines().filter(|l| l.trim().is_empty()).count(),
        dropped: text
            .lines()
            .filter(|l| !l.trim().is_empty() && falcon_core::colors::parse(l).is_none())
            .map(|l| l.to_string())
            .collect(),
    }
}

/// 档里那一排原文走成一份 `Palette`:读不出的那一枚不占位、重复的只留头一枚。
///
/// 载入时 [`crate::config`] 那道闸已经丢过一遍读不出的,所以这一手通常只是「原样 + 去重」;
/// 留在桥这一侧而不是省掉,是因为**写出的那一份文件必须是模型自己的拼法**(`#RRGGBB` 一行一枚)
/// —— 直接落 `annotation.palette` 的原文会把 `rgb(232, 17, 35)` 那种拼法带进一份「一枚一行」的档。
pub(crate) fn palette_from_stored(colors: &[String]) -> falcon_core::colors::Palette {
    let mut p = falcon_core::colors::Palette { colors: Vec::new() };
    for c in colors.iter().filter_map(|c| falcon_core::colors::parse(c)) {
        p.add(c);
    }
    p
}

/// 名单那一行的写法:一枚一行是**档**的形状,一行报完是**报告**的形状。
fn palette_roster(palette: &falcon_core::colors::Palette) -> String {
    palette.to_text().lines().collect::<Vec<_>>().join(", ")
}

/// §5.7.22 步骤 5 的前半:读到什么、本机是什么、两份一样不一样。
///
/// 三份数各自独立成行,是因为「零枚」与「没去看」在日志里得长得不一样 —— 与
/// [`render_import_applied`] 同一枚理由。丢掉的那些行逐行印,一行一枚,读的人拿行数才能对上
/// 头部那句里的 `读不出 K 行`。
pub fn render_palette_preview(file: &Path, read: &PaletteRead, current: &[String]) -> String {
    let mut out = format!(
        "[palette] 读到 {}：{} 行、留下 {} 枚、空行 {} 行、读不出 {} 行、重复 {} 枚（§5.7.22 步骤 5）\n",
        file.display(),
        read.lines,
        read.palette.colors.len(),
        read.blank,
        read.dropped.len(),
        read.repeats(),
    );
    for d in &read.dropped {
        out.push_str(&format!("  读不出 {d}\n"));
    }
    if read.dropped.is_empty() {
        out.push_str("  （没有读不出的行）\n");
    }
    let mine = palette_from_stored(current);
    out.push_str(&format!(
        "[palette] 本机 {} 枚：{}\n",
        mine.colors.len(),
        palette_roster(&mine)
    ));
    out.push_str(&format!(
        "[palette] 这一份 {} 枚：{}\n",
        read.palette.colors.len(),
        palette_roster(&read.palette)
    ));
    out.push_str(if mine.to_text() == read.palette.to_text() {
        "[palette] 两份名单相同\n"
    } else {
        "[palette] 两份名单不同\n"
    });
    out
}

/// §5.7.22 步骤 5 的后半:落盘之后报一次。与 [`render_import_applied`] 同一副骨架,
/// 因为「已应用」这句话在两族里问的是同一件事 —— 写没写成、耗时多少、门有没有再校正。
pub fn render_palette_applied(kept: usize, ms: u64, corrections: &[String]) -> String {
    let mut out = format!("[palette] 已应用 {kept} 枚，写盘耗时 {ms} ms（§5.7.22 步骤 5）\n");
    out.push_str(&format!(
        "[palette] 应用后又校正 {} 项（写进本机之后闸才发现的）\n",
        corrections.len()
    ));
    for c in corrections {
        out.push_str(&format!("  应用时校正 {c}\n"));
    }
    if corrections.is_empty() {
        out.push_str("  （应用时没有再校正任何项）\n");
    }
    out
}

/// §5.7.22 步骤 4:把这一台机器档里那一排色写成一份可以带给别人的调色板档,返回写出的枚数。
///
/// 走 [`falcon_core::colors::Palette::to_text`] 而不是 [`Config::export_text`]:后者写出的是
/// **一整份 settings**(`annotation.palette` 只是其中一行),而 §5.7.22 步骤 4 要递给人的是
/// 「一组色」—— 一枚一行、`#RRGGBB`、谁都能拿记事本改。枚数取自写出去的那一份文本自己的
/// 行数,不是设置页自报的枚数。
///
/// alpha 在这一手同样是丢弃:`to_text` 用的是 `ColorFormat::HexUpper`,写出 `#RRGGBB`。
/// 那是 §5.7.21 第 3 步那只手欠着的同一件事(V1.181 块 ⑨),本档不替它编一个已落地的说法。
pub fn palette_export(path: &Path) -> Result<String, String> {
    let (stored, text) = with(|s| {
        let cfg = s.config.annotation.palette.clone();
        (cfg.len(), palette_from_stored(&cfg).to_text())
    });
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    // 末尾那一枚换行是给记事本看的:一份没有换行结尾的档,编辑器会把它画成缺了最后一行。
    // `Palette::from_text` 走 `str::lines()`,那一枚换行因此不会多数出一行。
    std::fs::write(path, format!("{text}\n").as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "[palette] 本机 {stored} 枚 → 写出 {} 枚到 {}（§5.7.22 步骤 4）\n",
        text.lines().count(),
        path.display()
    ))
}

/// 没有 confirm 时补的那一句，命令行那一副：它的门是 `--confirm`，所以只能说 `--confirm`。
pub const PALETTE_UNAPPLIED_CLI: &str = "[palette] 未应用：加 --confirm 才会写入（§5.7.22 步骤 5）";
/// 同一件事的页面那一副：设置页上没有 `--confirm` 可加，它有的是一枚「确认应用」钮。说一句
/// 「加 --confirm」的页是在教人敲命令行。
pub const PALETTE_UNAPPLIED_PAGE: &str =
    "[palette] 未应用：按「确认应用」才会写入（§5.7.22 步骤 5）";

/// §5.7.22 步骤 5：命令行那一副。
pub fn palette_import(path: &Path, confirm: bool) -> Result<String, String> {
    palette_import_at(path, confirm, PALETTE_UNAPPLIED_CLI)
}

/// §5.7.22 步骤 5：设置页那一副。与上面**同一只手**，只有「没写」那一句的措辞按门换 ——
/// 页与命令行之间因此不存在两套落盘码。
pub fn palette_import_page(path: &Path, confirm: bool) -> Result<String, String> {
    palette_import_at(path, confirm, PALETTE_UNAPPLIED_PAGE)
}

/// §5.7.22 步骤 5 的两扇门共用的一只手：读一份调色板档、报出会盖掉本机哪一排色;只有 `confirm`
/// 才真写。`unapplied` 是没写那一句按门换的措辞（[`PALETTE_UNAPPLIED_CLI`] /
/// [`PALETTE_UNAPPLIED_PAGE`]）。
///
/// 两件事这一手必须拦住:
///
/// 1. **一份读不出任何颜色的档不许把出厂那十枚带回来**。[`Config::validate`]
///    对空名单的做法是回填 `default_palette()` 而**不记一句警告**,所以「这档是空的」与
///    「这档写了十枚出厂色」在落盘之后完全同一副样子。空档在这里当 Err 退回,本机一枚不动。
/// 2. **写失败不许报成功**。[`Settings::save`] 把失败记进 `problems` 而自己只返回耗时,所以
///    这一手看的是 save 前后 `problems` 长没长 —— 与 [`config_import`] 同一条 §5.17 的规矩。
fn palette_import_at(path: &Path, confirm: bool, unapplied: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let read = palette_read_text(&text);
    // 这一手排在读本机之前:拦下一份空档不需要知道本机有什么,而**退了这一趟就一定一枚没写**。
    if read.palette.colors.is_empty() {
        return Err(format!(
            "{} 读不出任何颜色（{} 行里 {} 行读不出、{} 行是空行）：不落盘，本机那一排原样不动",
            path.display(),
            read.lines,
            read.dropped.len(),
            read.blank
        ));
    }
    let current = with(|s| s.config.annotation.palette.clone());
    let mut out = render_palette_preview(path, &read, &current);
    if !confirm {
        out.push_str(&format!("{unapplied}\n"));
        return Ok(out);
    }
    let kept = read.palette.colors.len();
    let roster = read
        .palette
        .to_text()
        .lines()
        .collect::<Vec<_>>()
        .join(", ");
    let (outcome_ms, corrections) = with(|s| -> Result<(u64, Vec<String>), String> {
        let corrections = s
            .config
            .set_str("annotation.palette", &roster)
            .map_err(|e| e.to_string())?;
        let before = s.problems.len();
        let ms = s.save();
        if s.problems.len() > before {
            Err(s.problems[before].clone())
        } else {
            Ok((ms, corrections))
        }
    })?;
    out.push_str(&render_palette_applied(kept, outcome_ms, &corrections));
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

    /// P130：同一族写法给 §5.9.17 步骤 1 那枚开关走两臂。`Default` 侧显 `false` ——
    /// 「同时提供文件」要在剪贴板旁边落一份盘，出厂不替用户开这一格；翻成 `true` 的那一侧
    /// 原样印 `true`，不能被 `hide_pins_on_dialog` 那一臂顶掉。
    #[test]
    fn banner_prints_the_copy_as_file_switch_both_ways() {
        let off = format_output_banner(&falcon_core::config::Output::default());
        assert!(
            off.contains("output.copy_as_file=false"),
            "出厂默认就该显 false，复制不该顺手落盘：{off}"
        );
        let on = falcon_core::config::Output {
            copy_as_file: true,
            ..Default::default()
        };
        let shown = format_output_banner(&on);
        assert!(
            shown.contains("output.copy_as_file=true"),
            "开着那一侧要原样印 true，且与 hide_pins_on_dialog 同屏共存：{shown}"
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
            "7 · 2023-11-14 14:13 · 位置 10,20 · 64×32 · 主屏 · region",
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

    /// §5.16 那枚 `[advanced] recognize_text` 落到库里那一格之后，清单这一格自此是它的**第一位
    /// 读者**（P148）：库里那三种形状说三句话，一句也不并成一句。
    ///
    /// 数的是**字符**而不是字节 —— 同一句中文在 UTF-8 里的字节数是它的三倍，拿字节数当「几个字」
    /// 报给用户就是把尺用错。而「有字」判的是长度而不是 `is_some()`：跑过而一个字也没有，
    /// 对用户不是「这一行有文字」，那是第三件事。
    #[test]
    fn the_history_list_says_what_the_recognition_left_in_a_row() {
        let at_ms = 1_700_000_000_000;
        let rec = |ocr: Option<&str>| falcon_core::history::Record {
            id: 7,
            captured_at: at_ms,
            monitor_id: "主屏".to_string(),
            rect: falcon_core::PhysRect::new(10, 20, 64, 32),
            source: falcon_core::history::Source::Region,
            path: None,
            thumb: None,
            cancelled: false,
            locked: false,
            ocr_text: ocr.map(str::to_string),
            note: String::new(),
        };
        let one = |r: falcon_core::history::Record| history_rows_joined(&[r], -480);

        // ① NULL：这一行从没识别过 ⇒ 与本轮之前一字不差。
        assert_eq!(
            one(rec(None)),
            "7 · 2023-11-14 14:13 · 位置 10,20 · 64×32 · 主屏 · region",
            "没识别过的那一行多了话"
        );

        // ② 空串：识别跑成了而一个字也没有 —— 第三件事，不能说成①，也不能说成③。
        let blank = one(rec(Some("")));
        assert!(
            blank.ends_with(" · 本机识别过，没有字"),
            "跑过而没字的那一行没说它跑过：{blank}"
        );
        assert!(!blank.contains("有识别文字"), "空串被说成了有字：{blank}");

        // ③ 有字：数到 12 个字符（含那个换行），而不是 22 个字节。
        let text = one(rec(Some("总价 12.00\n第二行")));
        assert_eq!(
            "总价 12.00\n第二行".len(),
            22,
            "这一格的对照要指着字节数（`len()`）与字符数（`chars().count()`）不等的那一串"
        );
        assert!(
            text.ends_with(" · 有识别文字 12 字"),
            "有字那一行报的不是字符数：{text}"
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

        let joined = history_rows_at(&dir, &cfg, 50, -480, false).expect("开着就该读得回");
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
        let one = history_rows_at(&dir, &cfg, 1, -480, false).expect("limit 1 也该读得回");
        assert_eq!(one.split('|').count(), 1, "limit 1 ⇒ 一段");
        assert!(
            one.starts_with(&format!("{newer} · ")),
            "留下的该是新的那一枚：{one}"
        );

        // 开着而库里没有行 = 一次正常的空，不是错。
        let empty = sandbox("history-listing-empty");
        assert_eq!(
            history_rows_at(&empty, &cfg, 50, -480, false).as_deref(),
            Ok(""),
            "空库 ⇒ Ok(空串)"
        );

        // 开关关着 ⇒ Err 点「没开」。
        let off = falcon_core::config::History {
            enabled: false,
            ..cfg.clone()
        };
        let said = history_rows_at(&dir, &off, 50, -480, false).unwrap_err();
        assert!(said.contains("没开"), "关着的答法要点名开关：{said}");
    }

    /// §7.1「历史记录加载应支持分页或虚拟列表」里分页那一支的本体：[`history_rows_older_at`]
    /// 的三种答法分开，而游标用的是 `(captured_at, id)` **那一对**而不是单看时间。
    ///
    /// 前两枚种的是同一个 `at_ms`：拿时间当游标的人会在这一格红 —— 同一毫秒的两行之间，
    /// `< ?` 谁也不满足谁，于是第 2 行读不回第 1 行，屏上那条清单停在第 2 行就说「到底了」。
    /// 那是把一句反话显示给用户，所以这一格钉的是**次序**，不是"有没有读回一行"。
    #[test]
    fn the_older_page_walks_the_cursor_pair_not_the_clock() {
        let dir = sandbox("history-paging");
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
        let cfg = falcon_core::config::History {
            enabled: true,
            keep_originals: false,
            thumb_px: 0,
            ..Default::default()
        };
        let frame = falcon_core::frame::Frame::filled(4, 4, [0, 0, 0, 255]).unwrap();
        let mut store = falcon_core::history::History::open(&dir).unwrap();
        // 同一毫秒的两行（甲、乙）+ 晚一小时的第三行（丙）。清单倒序是 丙 · 乙 · 甲。
        let jia = store
            .record(&meta(1_700_000_000_000, "甲"), &frame, &cfg)
            .unwrap();
        let yi = store
            .record(&meta(1_700_000_000_000, "乙"), &frame, &cfg)
            .unwrap();
        let bing = store
            .record(&meta(1_700_003_600_000, "丙"), &frame, &cfg)
            .unwrap();
        assert!(jia < yi, "行号是自增的，这一格的对照指着它");
        drop(store);

        // 丙 那一页往旧：只读一行的话，读回的必须是**同一毫秒里行号较小的那一枚**（乙）。
        let one =
            history_rows_older_at(&dir, &cfg, bing, 1, -480, false).expect("更旧的那一页该读得回");
        assert_eq!(one.split('|').count(), 1, "limit 1 ⇒ 一段：{one}");
        assert!(
            one.starts_with(&format!("{yi} · ")) && one.contains("乙"),
            "游标是 `(captured_at, id)` 那一对：{one}"
        );

        // 再走一步：乙 往旧一枚就是甲 —— 毫秒相同也照样分得出先后，这一格量的就是那把刀。
        let next = history_rows_older_at(&dir, &cfg, yi, 1, -480, false).expect("第二页也该读得回");
        assert!(
            next.starts_with(&format!("{jia} · ")) && next.contains("甲"),
            "同一毫秒的更旧那一枚没被当成\"到底了\"：{next}"
        );

        // 甲 已是最后一行：Ok(空串) = 读到底了，不是错。
        assert_eq!(
            history_rows_older_at(&dir, &cfg, jia, 50, -480, false).as_deref(),
            Ok(""),
            "手里已是最后一行 ⇒ 一次正常的空"
        );

        // 游标那一枚已经不在了 ⇒ Err 点它的名。把「你手里那一行刚被删掉」说成「到底了」
        // 是一句反话（#453），屏上那一行确实还是旧的。
        let gone = history_rows_older_at(&dir, &cfg, 987_654, 50, -480, false).unwrap_err();
        assert!(
            gone.contains("987654") && gone.contains("没有第"),
            "游标不在库里的那句答复要点名它：{gone}"
        );

        // 开关关着：与 [`history_rows_at`] 同一句拒绝，本函数不另编一种"没开"。
        let off = falcon_core::config::History {
            enabled: false,
            ..cfg.clone()
        };
        let said = history_rows_older_at(&dir, &off, bing, 50, -480, false).unwrap_err();
        assert!(said.contains("没开"), "关着的答法要点名开关：{said}");
    }

    /// §5.14.1 规则第三句「历史记录应包含时间、屏幕、**位置**、尺寸和截图内容」里那一枚**位置**：
    /// 这一格钉的不是"行上有没有两个数"，而是**那两个数是 `rec.rect` 的 x 与 y**——尺寸那一节
    /// （`64×32`）本轮之前就在，把行换成 `10,20 · 64×32` 之后，只有位置写得对、写得是那一节的位置
    /// 才过得了这一格（#281：`history_rows_joined` 是行文的唯一作者，别处不该有第二把对照）。
    ///
    /// 三臂各挡一种偷懒：① 负坐标夹成 0（扩展桌面里左边那块屏的原点本就是负的，抄成 0 是把
    /// 「在左边那块屏的最左边」说成「在主屏的左上角」）；② `0,0` 那一行**不许**因为两个数都是 0
    /// 就少写一节 —— 少写就是 #433 那句静默；③ 同一枚尺寸、不同的 x/y，两行的位置节必须不同。
    #[test]
    fn the_history_list_names_where_the_selection_was() {
        fn rec(id: i64, x: i32, y: i32) -> falcon_core::history::Record {
            falcon_core::history::Record {
                id,
                captured_at: 1_700_000_000_000,
                monitor_id: "主屏".to_string(),
                rect: falcon_core::PhysRect::new(x, y, 64, 32),
                source: falcon_core::history::Source::Region,
                path: None,
                thumb: None,
                cancelled: false,
                locked: false,
                ocr_text: None,
                note: String::new(),
            }
        }
        let one = |r: falcon_core::history::Record| history_rows_joined(&[r], -480);

        assert_eq!(
            one(rec(7, 10, 20)),
            "7 · 2023-11-14 14:13 · 位置 10,20 · 64×32 · 主屏 · region",
            "位置那一节写在哪一节之后、以什么字形写，都由这一格钉死"
        );

        let left = one(rec(8, -1920, 0));
        assert!(
            left.contains(" · 位置 -1920,0 · "),
            "负坐标被夹成了 0，屏上那行就把另一块屏说成了主屏：{left}"
        );

        let origin = one(rec(9, 0, 0));
        assert!(
            origin.contains(" · 位置 0,0 · "),
            "选区从原点开始时不许少写一节（#433）：{origin}"
        );

        let pair = history_rows_joined(&[rec(10, 1, 2), rec(11, 3, 4)], -480);
        let rows: Vec<&str> = pair.split('|').collect();
        assert_eq!(rows.len(), 2, "两条 ⇒ 恰好一个 `|`");
        assert!(
            rows[0].contains("位置 1,2") && rows[1].contains("位置 3,4"),
            "那两个数不是从尺寸或屏名推出来的：{rows:?}"
        );
    }

    /// 清单里**行身份**的唯一读法：一行是 `id · 时间 · 位置 x,y · 宽×高 · 屏幕 · 来源[ · 字样]`，
    /// 所以那一枚行号是第一个 ` · ` 之前的整段。
    ///
    /// 不用 `page.contains(&format!("{id} · "))` 问「这一行在不在」：那一串里的分隔符是复用字面，
    /// `17:13 · 位置` 里躺着「3 · 」、`4×4 · 主屏` 里躺着「4 · 」—— 数字小的行号会被别人的时间或
    /// 尺寸命中，一次**对的**筛选因此被报成漏了行（本档第一版就是这么红的）。空串是一串没有行，
    /// 它 split 出来的一段 `""` 不算一行。
    fn row_ids(page: &str) -> Vec<String> {
        page.split('|')
            .filter(|l| !l.is_empty())
            .map(|l| l.split(" · ").next().unwrap_or_default().to_string())
            .collect()
    }

    /// §5.14.3 第 3 步「找到标记为“已取消”的记录」里**筛**那一半在产品这一读上的三臂：筛上而有
    /// 那些行 / 筛上而一行也没有 / 没筛。
    ///
    /// 关键是**没有一臂是"读全量再在 Rust 里挑"**：筛住在 SQL 里（
    /// [`falcon_core::history::History::cancelled_page`] 那一记 `WHERE cancelled = 1`），于是
    /// 「这一页少于页宽」从此真的等于「问到底了」。读完再挑的那一版会有一页筛得空、而库里还有
    /// 更旧的已取消行 —— 那句「清单到底了」就成了猜，页上要么停在半路，要么一轮一轮问空页（#453）。
    ///
    /// 第二臂那句 `Ok("")` 与「库里根本没有行」是同一串空、**两句不同的话**：分辨它的是
    /// [`crate::mask::MaskState::list_history`]（那儿才有 `cancelled_only` 这枚语境），这一格只管
    /// 交回的东西对不对。
    #[test]
    fn the_cancelled_filter_has_rows_has_none_and_is_off_by_default() {
        let dir = sandbox("history-filter");
        let empty = sandbox("history-filter-none-cancelled");
        fn meta<'a>(at_ms: i64, cancelled: bool) -> falcon_core::history::CaptureMeta<'a> {
            falcon_core::history::CaptureMeta {
                at_ms,
                offset_min: -480,
                monitor_id: "主屏",
                rect: falcon_core::PhysRect::new(0, 0, 4, 4),
                source: falcon_core::history::Source::Region,
                cancelled,
            }
        }
        let cfg = falcon_core::config::History {
            enabled: true,
            keep_originals: false,
            thumb_px: 0,
            ..Default::default()
        };
        let frame = falcon_core::frame::Frame::filled(4, 4, [0, 0, 0, 255]).unwrap();

        // 四行：两行按 Esc 取消过（乙、丁），两行没有（甲、丙）。行号随插入自增，甲<乙<丙<丁。
        let mut store = falcon_core::history::History::open(&dir).unwrap();
        let jia = store
            .record(&meta(1_700_000_000_000, false), &frame, &cfg)
            .unwrap();
        let yi = store
            .record(&meta(1_700_003_600_000, true), &frame, &cfg)
            .unwrap();
        let bing = store
            .record(&meta(1_700_007_200_000, false), &frame, &cfg)
            .unwrap();
        let ding = store
            .record(&meta(1_700_010_800_000, true), &frame, &cfg)
            .unwrap();
        drop(store);

        // ① 筛上而有那些行：只有两行，而且行末都带着那个字样；没取消过的两枚一个字也不提。
        let filtered = history_rows_at(&dir, &cfg, 50, -480, true).expect("筛这一问该读得回");
        let rows: Vec<&str> = filtered.split('|').collect();
        assert_eq!(rows.len(), 2, "筛上 ⇒ 只回那两行：{filtered}");
        // 行身份按**整段**比，不按 `contains("{id} · ")`：那一串里「3 · 」本来就躺在
        // `17:13 · 位置` 的边界上、「4 · 」躺在 `4×4 · 主屏` 上，拿子串问「这一行没进来」
        // 会把一次正确的筛选读成漏了。
        assert_eq!(
            row_ids(&filtered),
            vec![ding.to_string(), yi.to_string()],
            "筛过的清单应是倒序的那两行，别的一行也不许在：{filtered}"
        );
        for r in &rows {
            assert!(r.ends_with(" · 已取消"), "筛进来的那行没带字样：{r}");
        }

        // ② 没筛：四行全在，其中两行带字样。这一臂挡住"筛写在了 join 之后"——那样全量那一问
        // 也会只剩两行，而用户没说要看筛过的。
        let all = history_rows_at(&dir, &cfg, 50, -480, false).expect("全量那一问该读得回");
        let all_rows: Vec<&str> = all.split('|').collect();
        assert_eq!(all_rows.len(), 4, "没筛 ⇒ 四行：{all}");
        assert_eq!(
            row_ids(&all),
            vec![
                ding.to_string(),
                bing.to_string(),
                yi.to_string(),
                jia.to_string()
            ],
            "全量那一问应是四行倒序，一位也不许缺：{all}"
        );
        assert!(
            all_rows.iter().filter(|l| l.ends_with(" · 已取消")).count() == 2,
            "全量里那两行的字样数不对：{all:?}"
        );

        // ③ 筛上而一行也没有：库里三行都没取消过 ⇒ `Ok("")`，不是 `Err`。把一次正常的空答成
        // "打不开 / 没开"就是无病呻吟（#453）。
        let mut bare = falcon_core::history::History::open(&empty).unwrap();
        for i in 0..3 {
            bare.record(&meta(1_700_000_000_000 + i * 1000, false), &frame, &cfg)
                .unwrap();
        }
        drop(bare);
        assert_eq!(
            history_rows_at(&empty, &cfg, 50, -480, true).as_deref(),
            Ok(""),
            "没有已取消的行 ⇒ 一次正常的空"
        );
        assert_eq!(
            history_rows_at(&empty, &cfg, 50, -480, false).as_deref().map(|s| s.split('|').count()),
            Ok(3),
            "同一座库没筛时是三行：这一对照挡住「筛被写死成了 1」"
        );
    }

    /// 同一枚筛**翻页**那一臂：拿已取消那一页的最后一行当游标继续问，读回的必须是更旧的已取消行；
    /// 问到底时交回 `Ok("")` —— 那是**筛过的**清单到底了，不是"库里再没有更旧的行"。
    ///
    /// 中间那一档（`limit 2` 而只回一行）是本格的全部理由：筛住在 SQL 里时，短页就等于到底；
    /// 若是读完再筛，短页既可能是到底也可能只是这一页没赶上，而页上那句「到底了」只能二选一，
    /// 选错就说成反话。游标那一枚仍走 [`falcon_core::history::History::get`] 的**不筛**那一问：
    /// 它问的是「那一行还在库里吗」，拿 `cancelled` 去问它就把一句拒绝改写成另一种。
    #[test]
    fn the_older_page_stays_filtered_and_says_when_it_runs_out() {
        let dir = sandbox("history-filter-paging");
        fn meta<'a>(at_ms: i64, cancelled: bool) -> falcon_core::history::CaptureMeta<'a> {
            falcon_core::history::CaptureMeta {
                at_ms,
                offset_min: -480,
                monitor_id: "主屏",
                rect: falcon_core::PhysRect::new(0, 0, 4, 4),
                source: falcon_core::history::Source::Region,
                cancelled,
            }
        }
        let cfg = falcon_core::config::History {
            enabled: true,
            keep_originals: false,
            thumb_px: 0,
            ..Default::default()
        };
        let frame = falcon_core::frame::Frame::filled(4, 4, [0, 0, 0, 255]).unwrap();
        // 五行按时间递增、取消与否交替：筛过的清单是 戊 · 丙 · 甲，全量是 戊丁丙乙甲。
        let mut store = falcon_core::history::History::open(&dir).unwrap();
        for i in 0..5 {
            store
                .record(
                    &meta(1_700_000_000_000 + i * 1000, i % 2 == 0),
                    &frame,
                    &cfg,
                )
                .unwrap();
        }
        let ids: Vec<i64> = store
            .newest(50)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .rev()
            .collect();
        drop(store);
        let (jia, yi, bing, ding, wu) = (ids[0], ids[1], ids[2], ids[3], ids[4]);

        // 第一页（页宽 2、筛上）：戊、丙 —— 页上此刻的最后一行是丙，下一页的游标就是它。
        let first = history_rows_at(&dir, &cfg, 2, -480, true).expect("筛上的第一页该读得回");
        let rows: Vec<&str> = first.split('|').collect();
        assert_eq!(rows.len(), 2, "筛上的第一页该有两行：{first}");
        assert!(
            rows[0].starts_with(&format!("{wu} · ")) && rows[1].starts_with(&format!("{bing} · ")),
            "筛过的那一页里混进了没取消的行，或是次序反了：{rows:?}"
        );

        // 同一枚游标（戊）不筛那一问回的是 丁、丙 —— 两问不一样，这一格才量得出筛住在查询里。
        let unfiltered = history_rows_older_at(&dir, &cfg, wu, 2, -480, false).expect("全量那一页");
        let urows: Vec<&str> = unfiltered.split('|').collect();
        assert!(
            urows[0].starts_with(&format!("{ding} · "))
                && urows[1].starts_with(&format!("{bing} · ")),
            "没筛的那一问被筛污染了：{urows:?}"
        );
        let same_cursor =
            history_rows_older_at(&dir, &cfg, wu, 2, -480, true).expect("筛过的那一页");
        assert_eq!(
            same_cursor.split('|').count(),
            2,
            "同一枚游标筛上时也该有两行（丙、甲）：{same_cursor}"
        );

        // 游标那一行自己**不复查** `cancelled`：丁 没被取消过，而它是"从哪儿继续找"的坐标，
        // 不是候选。拿筛条件去问它，这一问就成了 `Err`「没有第 丁 行」。
        let odd_cursor = history_rows_older_at(&dir, &cfg, ding, 2, -480, true)
            .expect("没取消过的那一枚当游标，也只是一个坐标");
        let orows: Vec<&str> = odd_cursor.split('|').collect();
        assert!(
            orows[0].starts_with(&format!("{bing} · "))
                && orows[1].starts_with(&format!("{jia} · ")),
            "游标那一行被复查了：{orows:?}"
        );
        // 没取消过的那两枚（乙、丁）在筛过的**任何**一页里一个字也不提 —— 丁 出现过的位置只有
        // 游标那枚坐标，不是清单上的一行。行身份按整段比（见 [`row_ids`] 头上那段）。
        for page in [&first, &same_cursor, &odd_cursor] {
            let leaked: Vec<String> = row_ids(page)
                .into_iter()
                .filter(|id| *id == yi.to_string() || *id == ding.to_string())
                .collect();
            assert!(leaked.is_empty(), "未取消的行被递进了筛过的那一问：{page}");
        }

        // 页上那一页的最后一行（丙）当游标：更旧的已取消行只剩甲 ⇒ **短页**，而它不是"没赶上"。
        let tail = history_rows_older_at(&dir, &cfg, bing, 2, -480, true).expect("更旧的那一页");
        let tail_rows: Vec<&str> = tail.split('|').collect();
        assert_eq!(tail_rows.len(), 1, "筛过的清单只剩一行：{tail}");
        assert!(
            tail_rows[0].starts_with(&format!("{jia} · ")) && tail_rows[0].ends_with(" · 已取消"),
            "短页里那一行不是最旧的那一枚已取消行：{tail_rows:?}"
        );

        // 再走一步：筛过的清单到底 ⇒ `Ok("")`，页上就此不再问（`tailed`）。同一枚游标不筛时
        // 也到底 —— 两问在这一档相等，差异全在前面那一页，这一格因此量的不是"空不空"，而是
        // **短页与到底之间没有第三种答**。
        assert_eq!(
            history_rows_older_at(&dir, &cfg, jia, 2, -480, true).as_deref(),
            Ok(""),
            "筛过的清单到底 ⇒ 一次正常的空"
        );
        assert_eq!(
            history_rows_older_at(&dir, &cfg, jia, 2, -480, false).as_deref(),
            Ok(""),
            "同一枚游标不筛时也该到底（甲是最旧的一行）"
        );

        // 游标那一枚根本不在库里 ⇒ 与不筛那一问同一句拒绝，本函数不另编一种"没有"。
        let gone = history_rows_older_at(&dir, &cfg, 987_654, 2, -480, true).unwrap_err();
        assert!(
            gone.contains("987654") && gone.contains("没有第"),
            "筛上的那一问点不齐游标时仍要点它的名：{gone}"
        );
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

    /// §8.7「提示用户清理历史或调整存储上限」那一句话**本体**。这一格只问「说了什么」——
    /// 「顶到没顶到」那一判在 core 的 `Retention::at_cap`（`0` = 那一枚上限没设，两边同一枚读法），
    /// 这里第二次判就成了同一件事实的两个作者（#281）。
    #[test]
    fn the_pressure_line_offers_both_exits_and_spares_locked_rows() {
        let said = history_pressure_line(120, 100);
        // PRD 那一句给的是两条路（清理 / 调上限），一句「已到上限」把两条路都省了就是没说；
        // §8.7 第二、三条（先清未锁定的最旧、不静默删锁着的）也在同一句里，页上不许再补一句。
        for want in ["清理", "调大", "上限", "未锁定", "最旧"] {
            assert!(said.contains(want), "那一句少了「{want}」：{said}");
        }
        assert!(said.contains("此刻 120 条、上限 100 条"), "{said}");
        // 条数那一枚没设上限时不许说「上限 0 条」——设 `0` 的意思就是不设，那样一句话是反的；
        // 此时顶到的只能是字节，而那两枚数本函数拿不到，所以指上面那一行 §6.2 的占用。
        let bytes_only = history_pressure_line(45, 0);
        assert!(!bytes_only.contains("上限 0 条"), "{bytes_only}");
        assert!(bytes_only.contains("顶到的是字节那一枚"), "{bytes_only}");
        assert!(bytes_only.contains("此刻 45 条"), "{bytes_only}");
        // 单位换算一处也不做：把字节除以 1048576 去凑档上那枚 `max_size_mb` 就是给同一道算术
        // 第二个作者，四舍五入还会替用户编出「0 MB 已到上限」那种话。
        assert!(!said.contains("MB"), "{said}");
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

    /// §5.20.3 步骤 5 写完盘那一报的两半:「门改了点什么」与「门没改任何东西」都得有自己
    /// 那一行,而每一枚校正各占一行 —— 读的人拿行数才能对上上面那个数。
    ///
    /// 这一手与上面那份预览分开,是因为它答的是另一个问题:那份是**来的档**被 parse 校正
    /// 了几枚,这一份是写进**这一台机器**之后门又发现了几枚(§10 第 170 项)。
    #[test]
    fn import_applied_report_prints_each_correction_it_had_to_make() {
        let clash = "hotkey.actions.capture, toggle_pins: f13 is bound twice".to_string();
        let s = render_import_applied(2, 7, std::slice::from_ref(&clash));
        assert!(s.contains("[import] 已应用 2 项，写盘耗时 7 ms"), "{s:?}");
        assert!(s.contains("[import] 应用后又校正 1 项"), "{s:?}");
        assert_eq!(
            s.lines().filter(|l| l.starts_with("  应用时校正 ")).count(),
            1,
            "{s:?}"
        );
        assert!(s.contains(&format!("  应用时校正 {clash}")), "{s:?}");
        assert!(!s.contains("（应用时没有再校正任何项）"), "{s:?}");

        let none = render_import_applied(0, 1, &[]);
        assert!(none.contains("[import] 已应用 0 项"), "{none:?}");
        assert!(none.contains("应用后又校正 0 项"), "{none:?}");
        assert!(none.contains("（应用时没有再校正任何项）"), "{none:?}");
        assert_eq!(
            none.lines()
                .filter(|l| l.starts_with("  应用时校正 "))
                .count(),
            0,
            "{none:?}"
        );

        // 枚数与行数一道涨:三枚校正就是三行,不多不少。
        let three = render_import_applied(3, 4, &["a".into(), "b".into(), "c".into()]);
        assert_eq!(
            three
                .lines()
                .filter(|l| l.starts_with("  应用时校正 "))
                .count(),
            3,
            "{three:?}"
        );
        assert!(three.contains("应用后又校正 3 项"), "{three:?}");
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

    /// P149：§5.19.2 的主题色走到界面那一侧的分岔本体，四臂逐一点名：`face` 两臂（浅底/深底）×
    /// 「够 3:1 就原样」与「不够就往那一面夹」两臂。**夹的算术本体在 core `accent_on`**（那边
    /// 有 hue/方向那格），这里判的是**界面这一侧的接线**：底选错了臂，浅底上的暗夹会把已经看得见
    /// 的色往更暗里推、深底反之 —— 两臂都写对才画得出「同一枚主题色在两套餐具上都能看见」。
    #[test]
    fn accent_face_arms_the_surface_by_the_theme_and_keeps_a_visible_color() {
        use falcon_core::config::{Appearance, ACCENT_MIN_CONTRAST};

        let mut a = Appearance::default();
        // 出厂那一枚在两面底上本来就够：够 ⇒ **原样**，界面不许自作主张地改颜色。
        a.accent = "#0078D7".into();
        assert_eq!(accent_face(&a, "light"), "#0078D7");
        assert_eq!(accent_face(&a, "dark"), "#0078D7");
        // 第三词不是第三臂：除 `"dark"` 一律走浅色那一面（与 `theme_face` 只有两词同一口径）。
        assert_eq!(accent_face(&a, ""), accent_face(&a, "light"));
        assert_eq!(accent_face(&a, "LIGHT"), accent_face(&a, "light"));

        // 不够的那枚（浅灰）在浅底上必须被夹成看得见的，且**夹出来的东西**过 3:1 那条线。
        a.accent = "#DADCE0".into();
        let sunk = accent_face(&a, "light");
        assert_ne!(sunk, "#DADCE0", "浅底上不够的主题色没被夹");
        let ratio = falcon_core::config::contrast_ratio(&sunk, falcon_core::config::SURFACE_LIGHT);
        assert!(
            ratio.is_some_and(|c| c >= ACCENT_MIN_CONTRAST),
            "夹完仍然不够 3:1：{sunk} -> {ratio:?}"
        );
        let lifted = accent_face(&a, "dark");
        assert!(
            falcon_core::config::contrast_ratio(&lifted, falcon_core::config::SURFACE_DARK)
                .is_some_and(|c| c >= ACCENT_MIN_CONTRAST),
            "深底上这一枚不够 3:1：{lifted}"
        );

        // 读不出来的字面**不许把空串递给界面**（那一枚画出来是「没有主题色」，PRD 说的不是这个）。
        a.accent = "not-a-color".into();
        let fallback = accent_face(&a, "light");
        assert!(!fallback.is_empty(), "非法主题色递给了界面一个空串");
        assert!(
            falcon_core::config::contrast_ratio(&fallback, falcon_core::config::SURFACE_LIGHT)
                .is_some_and(|c| c >= ACCENT_MIN_CONTRAST),
            "非法主题色的归宿这一枚在浅底上看不见：{fallback}"
        );
    }

    /// P149：§5.19.4 那两枚字（族 + 磅）与落点那一枚词的读者本体。这一格的对面不是「算错了」，
    /// 而是「递给界面的那一枚**不是档里的那一枚**」：磅不许换算成像素再递（QML 绑 `font.pointSize`，
    /// 乘两遍就是高分屏上大一号的那一种坏），族不许空（空串会把系统字体挤进来），落点不许是
    /// 名单外的字（闭集在 core 的 `set_str` / `validate`，这里只验「递给界面的那一枚仍在名单里」）。
    #[test]
    fn the_ui_font_pair_and_the_placement_word_reach_the_page_as_the_file_has_them() {
        use falcon_core::config::{Appearance, TOOLBAR_PLACEMENTS, UI_FONT_FAMILY_DEFAULT};

        let mut a = Appearance::default();
        let (points, family) = ui_font_face(&a);
        assert_eq!(points, a.ui_font_size as i32, "磅被换过一手才递给界面");
        assert_eq!(family, UI_FONT_FAMILY_DEFAULT);
        assert!(!family.trim().is_empty(), "出厂那一枚族是空串");

        a.ui_font_size = 11;
        a.ui_font_family = "Arial".into();
        assert_eq!(ui_font_face(&a), (11, "Arial".into()));

        a.ui_font_family = "   ".into();
        let (points, family) = ui_font_face(&a);
        assert_eq!(points, 11);
        // **这一族不在这里兜底**：validate 在读档那一步就把它归成出厂族（core 那侧的往返单测
        // 盯着那一格），所以内存里是空串 ⇒ 递出去的也是空串。写这一行是为了**反着钉**：谁要把
        // 「空 ⇒ 出厂」抄进这一格，就有第二份归一、两份会先旧（#281）。
        assert_eq!(family, "   ");
        // 整段读回走的是**过闸之后**的那一份：一次 `validate` 之后，界面拿到的就不是空串了。
        let mut cfg = falcon_core::config::Config::default();
        cfg.set_str("appearance.ui_font_family", "   ").unwrap();
        assert_eq!(ui_font_face(&cfg.appearance).1, UI_FONT_FAMILY_DEFAULT);

        assert!(TOOLBAR_PLACEMENTS.contains(&toolbar_placement_face(&a).as_str()));
        for edge in TOOLBAR_PLACEMENTS {
            a.toolbar_placement = (*edge).into();
            assert_eq!(toolbar_placement_face(&a), *edge);
            assert!(TOOLBAR_PLACEMENTS.contains(&toolbar_placement_face(&a).as_str()));
        }
    }

    /// §5.19.2 / §5.19.4 那四枚外观键在**页上**的对面：每一枚都得有真读者，而「怎么把它变成
    /// 像素」这一句判断只许有一位作者。
    ///
    /// 这一格管的是上一轮那两种坏同时存在的那一形：**逻辑齐备、生产路径零读者**（`accent` 递到
    /// `Session` 了，可四扇窗各写各的 `dark?"#4C8DF6":"#2563B8"`，改档不动一个像素），以及
    /// **#281**（倍数的算术在四栋里各抄一遍，四份会先旧）。运行期的门只看得见「这一趟屏幕是什么
    /// 颜色」，看不见「读数是不是只长在一处」——所以这一格判**源码**，与
    /// [`crate::annotate_check`] 里 `the_bands_colors_have_exactly_one_owner_and_it_is_the_file`
    /// 同一族，尺子自己也要被反证（摘掉共享对象里那一句，作者计数就得掉一枚）。
    ///
    /// 注释不参与：本档那几段说明把 `session.accent`、`ui_font ÷ 出厂磅` 都当**引文**写过，
    /// 算进去就是把「解释绑定」读成「绑定」（#313 / #253 那一族）。
    #[test]
    fn the_appearance_keys_reach_every_window_and_the_font_ratio_has_one_author() {
        let code = |l: &str| -> String {
            let t = l.trim_start();
            if t.starts_with("//") {
                return String::new();
            }
            t.to_string()
        };
        let reads = |src: &str, needle: &str| {
            src.lines()
                .map(code)
                .filter(|l| l.contains(needle))
                .count()
        };

        const SHARED: &str = include_str!("../qml/UiFont.qml");
        const WINDOWS: [(&str, &str); 4] = [
            ("SettingsWindow.qml", include_str!("../qml/SettingsWindow.qml")),
            ("HistoryWindow.qml", include_str!("../qml/HistoryWindow.qml")),
            ("GroupWindow.qml", include_str!("../qml/GroupWindow.qml")),
            (
                "CustomCaptureWindow.qml",
                include_str!("../qml/CustomCaptureWindow.qml"),
            ),
        ];

        // ① 主题色：四扇窗都绑 `Session.accent` 那枚**值**。少一扇，那一扇就还停在画师写死的
        //    两枚字面上；而字面本身不许出现在别处（`accent_fill` 之外的第三份色 = 第三个主人）。
        for (name, src) in WINDOWS {
            assert!(
                reads(src, "session.accent") >= 1,
                "{name} 不读 Session.accent，那一页上的强调色与档无关"
            );
            assert_eq!(
                reads(src, "property color accent_fill"),
                1,
                "{name} 的强调色有了第二处声明"
            );
        }

        // ② 字号与字族：**倍数的算术只长在共享对象那一枚文件里**，页上一处也没有。
        assert_eq!(
            reads(SHARED, "session.ui_font /"),
            1,
            "ui_font ÷ 出厂磅这一句有了第二个主人"
        );
        for (name, src) in WINDOWS {
            assert!(
                reads(src, "UiFont {") == 1,
                "{name} 没有把那枚共享读者接进来（或接了两枚）"
            );
            assert!(
                reads(src, "uiFont.px(") >= 1,
                "{name} 画着字却不乘倍数：改 appearance.ui_font_size 不动这一扇窗的像素"
            );
            assert!(
                reads(src, "font.family: uiFont.family") >= 1,
                "{name} 不读 appearance.ui_font_family"
            );
            assert_eq!(
                reads(src, "session.ui_font "),
                0,
                "{name} 自己算起了倍数：#281 说的就是这一句"
            );
        }

        // ③ 落点：`[appearance] toolbar_placement` 自此有了一枚界面读者，而 `auto` 的展开不在
        //    页上判（页上问 `toolbarEdge`，长边优先那一句的本体在 core `config::toolbar_edge`）。
        const BAR: &str = include_str!("../qml/AnnotationToolbar.qml");
        assert_eq!(
            reads(BAR, "session.toolbar_placement"),
            1,
            "落点那一枚词要么没读者，要么在栏里被读了两遍"
        );
        assert_eq!(
            reads(BAR, "toolbarEdge(view.hole_w, view.hole_h)"),
            1,
            "auto 的展开没现问桥，或在页上判了第二遍"
        );

        // 尺子自己也要被量：摘掉共享对象里那一句比值，作者计数就得掉一枚（#269 同一族）。
        let dropped = SHARED.replace("ui.session.ui_font / ui.baseline", "1");
        assert_eq!(
            reads(&dropped, "session.ui_font /"),
            0,
            "这一格的尺子抓不到「倍数被写死」那一形"
        );
    }

    /// §5.1.2 的启动回填判据。**机器是那一枚开关的真相**，六臂都在这里：两枚一致、四种漂移、
    /// 与「机器没答」那一臂。最后一臂是最容易写反的一格 —— 把读不到折成 `Agrees { file: false }`
    /// 或折成 `Drifts { machine: false }`，页上那枚开关就会在一次读不动注册表的启动里被悄悄关掉。
    #[test]
    fn the_machine_is_the_truth_the_file_follows_and_a_silent_machine_moves_nothing() {
        use platform_windows::autostart::Held;

        let absent = Ok(Held::Absent);
        let here = Ok(Held::Here);
        // 一条指向别处的启动项：机器确实会在登录时启动一个 falconshot ⇒ 开关是开着的。
        let elsewhere = Ok(Held::Elsewhere(std::ffi::OsString::from(
            r"C:\Old\Path\falconshot.exe",
        )));

        assert_eq!(
            autostart_backfill(false, absent.clone()),
            AutostartRead::Agrees {
                file: false,
                held: Held::Absent
            }
        );
        assert_eq!(
            autostart_backfill(true, here.clone()),
            AutostartRead::Agrees {
                file: true,
                held: Held::Here
            }
        );
        // 档里写着开而注册表里没有 —— 这一族欠账的形状，回填交回的是「改成 false」。
        let drift = autostart_backfill(true, absent.clone());
        assert!(
            matches!(
                drift,
                AutostartRead::Drifts {
                    file: true,
                    machine: false,
                    ..
                }
            ),
            "{drift:?}"
        );
        assert!(
            matches!(
                autostart_backfill(false, here.clone()),
                AutostartRead::Drifts {
                    file: false,
                    machine: true,
                    ..
                }
            ),
            "机器里明明有一条指向本程序的启动项，回填却说没开"
        );
        assert!(
            matches!(
                autostart_backfill(false, elsewhere.clone()),
                AutostartRead::Drifts { machine: true, .. }
            ),
            "「指向别处」被折成了没开"
        );
        assert!(
            matches!(
                autostart_backfill(true, elsewhere),
                AutostartRead::Agrees { file: true, .. }
            ),
            "回填拿档去改机器了：那条别人的启动项不该被这一趟动过"
        );

        let silent = autostart_backfill(true, Err("权限被拒".to_string()));
        assert_eq!(
            silent,
            AutostartRead::Silent("权限被拒".to_string()),
            "读不到被折成了一枚答案"
        );
        assert_eq!(
            format_autostart_line(&silent, None),
            "读不到启动项：权限被拒（档里那一格未动，页上那一枚照档里说）"
        );
    }

    /// 那一行里「有没有写文件」必须是量得出来的：`saved_ms` 为 `None` 时不许出现落盘那几个字，
    /// 为 `Some` 时不许丢掉原因。默认那一趟（档里 false、机器里没有）一个字节也不动 —— 每次启动
    /// 都重写一遍用户的设置档是这一格最容易长成的坏形状。
    #[test]
    fn the_autostart_line_names_the_write_only_when_there_was_one() {
        use platform_windows::autostart::Held;

        let agrees = AutostartRead::Agrees {
            file: false,
            held: Held::Absent,
        };
        let line = format_autostart_line(&agrees, None);
        assert!(line.contains("一致"), "{line}");
        assert!(line.contains("不在"), "{line}");
        assert!(!line.contains("落盘"), "{line}");

        let drift = AutostartRead::Drifts {
            file: true,
            machine: false,
            held: Held::Absent,
        };
        let wrote = format_autostart_line(&drift, Some(7));
        assert!(wrote.contains("改成 false"), "{wrote}");
        assert!(wrote.contains("落盘 7 ms"), "{wrote}");
        // 改了档而没报毫秒数：那一趟不许看起来像没发生过。
        assert!(!format_autostart_line(&drift, None).contains("落盘"));
    }

    /// §5.7.22 的第一位读者：色带走桥的那一份，枚数与顺序都由档决定。
    ///
    /// 反证钉在这里 —— 本文件之外那一页曾经写着六枚自己挑的字面量，而档里是十枚。一轮只
    /// 量「递出六枚」的跑级会把它自己写的常量读成绿色，所以这里量的不是某枚数，而是
    /// **输出的段数跟着输入的段数走**：1 / 3 / 6 / 10 / 20 各跑一遍，六那一遍也在内。
    #[test]
    fn band_colors_joined_hands_back_as_many_colors_as_the_file_named() {
        for n in [1usize, 3, 6, 10, 20] {
            let colors: Vec<String> = (0..n).map(|i| format!("#{:02X}0000", i + 1)).collect();
            let joined = band_colors_joined(&colors);
            let cells: Vec<&str> = if joined.is_empty() {
                vec![]
            } else {
                joined.split('|').collect()
            };
            assert_eq!(cells.len(), n, "{n} 枚颜色走到页上成了 {} 段", cells.len());
            for (i, cell) in cells.iter().enumerate() {
                assert_eq!(
                    *cell,
                    format!("{:02X}0000", i + 1),
                    "{n} 枚里的第 {i} 段读成 {cell:?}，档里写的是 {:?}",
                    colors[i]
                );
            }
        }
    }

    /// 同一枚颜色的两种拼法必须落到同一段上：归一那一步只有一台解析器（`colors::parse`），
    /// 而 `#E81123` 走十六进制那一支、`rgb(232, 17, 35)` 走函数那一支 —— 两支给出同一个答案
    /// 才是「页上没有第二台解析器」这句断言的机械形状。顺带钉三件：大写、六位、无 `#`；
    /// `#RGB` 短式展开成 `RRGGBB`；以及档里重复的一枚**照旧重复**（不替用户去重）。
    #[test]
    fn band_colors_joined_normalizes_spellings_without_editing_the_list() {
        let colors = [
            "#E81123",
            "rgb(232, 17, 35)",
            "#fff",
            "rgba(0, 120, 215, 1)",
            "#E81123",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<String>>();
        assert_eq!(
            band_colors_joined(&colors),
            "E81123|E81123|FFFFFF|0078D7|E81123"
        );

        // alpha 在这一格落地为丢弃，而这一条是本文档里写着的账（§5.7.21 第三步的门把手还不交
        // alpha），不是悄悄丢：带 `AA` 的八位式与前六位读成同一段。顺带钉住"谁在前"——
        // 档里的拼法是 CSS 的（alpha 在末），不是 QML 那枚整数读的 `0xAARRGGBB`，同一串十六
        // 进制数字两种读法给出两枚不同的颜色，所以这一格不许留给读者猜。
        assert_eq!(
            band_colors_joined(&["#E8112380".to_string()]),
            "E81123",
            "rgba 的一枚落成了别的段"
        );
        assert_eq!(
            band_colors_joined(&["#80E81123".to_string()]),
            "80E811",
            "八位式被当成 `#AARRGGBB` 读了：档里的 alpha 在末"
        );
        // 四位短式 `#RGBA` 同样展开到 RGB 三段（alpha 落回丢弃那一条）。
        assert_eq!(band_colors_joined(&["#fff8".to_string()]), "FFFFFF");
    }

    /// 读不出来的一枚不上带，与空表 ⇒ 空串 —— 后一半是页上那一次 `.split("|")` 的陷阱：
    /// `"".split("|")` 在 JS 里数出**一段空串**而不是零段（P92 / P93 钉过的那一格），所以
    /// `AnnotationToolbar.qml` 必须先滤零长段。这里把这枚形状留在档上，是因为一旦有人把
    /// 那一次 filter 删掉，档里一枚也没有时屏幕上会多出一枚点不动的空芯片。
    #[test]
    fn band_colors_joined_leaves_no_room_for_a_color_it_cannot_read() {
        let colors = ["#E81123", "not-a-colour", "255", "hsl(0, 100%, 49%)"]
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<String>>();
        let joined = band_colors_joined(&colors);
        let cells: Vec<&str> = joined.split('|').collect();
        assert_eq!(cells.len(), 2, "{joined:?} 不是两枚");
        assert_eq!(cells[0], "E81123");
        assert_eq!(
            cells[1],
            format_hsl_cell(),
            "hsl 那一支没有走到与 hex 同一台解析器上"
        );
        assert_eq!(band_colors_joined(&[]), "", "空表该给出空串");
    }

    /// `hsl(0, 100%, 49%)` 归一成什么，由 `colors::parse` 自己算——这一枚辅助函数存在的唯一
    /// 理由是把「同一台解析器」这句断言写成读数而不是写成一枚手抄的常量。
    fn format_hsl_cell() -> String {
        let [r, g, b, _] = falcon_core::colors::parse("hsl(0, 100%, 49%)")
            .expect("hsl parses with the file's own parser");
        format!("{r:02X}{g:02X}{b:02X}")
    }

    /// 出厂那一排确实是十枚而不是六枚：`Annotation::default()` 的主人（`config.rs`）与桥
    /// 递出的段数在这里对一次。写死 10 是一枚会被 `default_palette` 的改动当场抓到的断言，
    /// 而不是对旧页上那六个字面量的引用。
    #[test]
    fn the_shipped_palette_hands_the_band_more_than_the_old_page_drew() {
        let ann = falcon_core::config::Annotation::default();
        let joined = band_colors_joined(&ann.palette);
        let cells: Vec<&str> = joined.split('|').filter(|c| !c.is_empty()).collect();
        assert_eq!(cells.len(), ann.palette.len(), "{joined:?} 段数没跟着档走");
        assert_eq!(
            cells.len(),
            10,
            "config.rs 的 default_palette 改了枚数，这一格与它上面那两格一起重读"
        );
        assert_ne!(cells.len(), 6, "六枚是旧页上那一份常量，不是档里的答案");
    }

    /// §5.7.22 步骤 4 与步骤 5 是同一只手写、同一只手读：导出的那一份必须原样读得回来，
    /// 而它写的拼法只能是模型自己的那一拼（一枚一行、`#RRGGBB`）。拿「每行再走一次导出」
    /// 来断言拼法，是为了不在这里再抄一份正则——规则一旦有两套，漂的那一套永远是没人测的。
    #[test]
    fn a_palette_file_this_round_writes_reads_back_as_the_same_row() {
        let ann = falcon_core::config::Annotation::default();
        let text = palette_from_stored(&ann.palette).to_text();
        let read = palette_read_text(&text);
        assert_eq!(read.palette.colors.len(), 10, "出厂那一排是十枚，不是别的");
        assert_eq!((read.lines, read.blank, read.dropped.len()), (10, 0, 0));
        assert_eq!(read.repeats(), 0);
        for line in text.lines() {
            assert_eq!(
                palette_from_stored(&[line.to_string()]).to_text(),
                line,
                "{line:?} 不是模型自己会写出的那一拼"
            );
        }
        // 落盘的那一份多末尾一枚换行（给记事本看的），而 `str::lines()` 不因此多数一行。
        assert_eq!(
            palette_read_text(&format!("{text}\n")).palette.colors.len(),
            10
        );
    }

    /// 空行、读不出的行、重复的行——三笔账各数各的，加起来必须等于文件的行数。这一式是
    /// 那句「留下 M 枚」能不能被读的人信的证据；它要是算不平，报告里那三个数就有一个是编的。
    #[test]
    fn palette_read_text_keeps_the_bookkeeping_of_the_file_it_read() {
        let read = palette_read_text("#E81123\n#E81123\n#F38B00\n\nnope\n#0078D7\n");
        assert_eq!(
            (read.lines, read.blank, read.dropped.len()),
            (6, 1, 1),
            "{:?}",
            read.dropped
        );
        assert_eq!(read.dropped, vec!["nope".to_string()]);
        assert_eq!(read.palette.colors.len(), 3, "重复的那一枚不许占两个位置");
        assert_eq!(read.repeats(), 1);
        assert_eq!(
            read.lines,
            read.blank + read.dropped.len() + read.palette.colors.len() + read.repeats()
        );
        // 记事本那种 CRLF 结尾：同一排色，不该多数一行，也不该把 `\r` 当成颜色的一部分。
        let win = palette_read_text("#E81123\r\n#F38B00\r\n");
        assert_eq!(
            (win.lines, win.blank, win.dropped.len(), win.repeats()),
            (2, 0, 0, 0)
        );
        assert_eq!(win.palette.colors.len(), 2);
    }

    /// 一份读不出任何颜色的档**不许**变成出厂那十枚。[`Config::validate`] 对空名单的做法是
    /// 回填 `default_palette()` 而一个字也不警告，所以走到那一步之后，「这档是空的」与
    /// 「这档写了十枚出厂色」在盘上完全同一副样子——用户那一排色就此没有作者地没了。
    #[test]
    fn a_palette_file_with_nothing_readable_is_refused_before_it_touches_the_machine() {
        let dir = sandbox("empty-palette");
        let path = dir.join("empty.txt");
        std::fs::write(&path, "not a colour\n\n").unwrap();
        let e = palette_import(&path, true).expect_err("空档必须被拒，不能被回填成出厂那十枚");
        assert!(e.contains("读不出任何颜色"), "{e}");
        assert!(e.contains(&path.display().to_string()), "{e}");
        assert_eq!(
            palette_read_text(&std::fs::read_to_string(&path).unwrap())
                .palette
                .colors
                .len(),
            0,
            "这一句拒的就是这一手读出来的那个 0，不是别的读数"
        );
        // 加不加 `--confirm` 改的是「写不写」，不是「拦不拦」：预览那一趟也说同一句话。
        assert!(palette_import(&path, false).is_err());
    }

    /// 头部那句报「读不出 K 行」，下面就得有 K 行「读不出 …」——「零枚」与「没去看」在日志里
    /// 得长得不一样，与 [`render_import_applied`] 同一枚理由。「相同」那一支也得有它的对面，
    /// 否则这一格只测了走得到的一半。
    #[test]
    fn the_palette_preview_prints_one_row_for_every_row_it_dropped() {
        let path = std::path::Path::new("shared.txt");
        let current: Vec<String> = falcon_core::config::Annotation::default().palette;
        let read = palette_read_text("#123456\nnope\n#ABCDEF\nstill-not\n");
        let out = render_palette_preview(path, &read, &current);
        let rows: Vec<&str> = out.lines().filter(|l| l.starts_with("  读不出 ")).collect();
        assert_eq!(rows, vec!["  读不出 nope", "  读不出 still-not"], "{out}");
        assert_eq!(rows.len(), read.dropped.len());
        assert!(out.contains("[palette] 本机 10 枚："), "{out}");
        assert!(
            out.contains("[palette] 这一份 2 枚：#123456, #ABCDEF"),
            "{out}"
        );
        assert!(out.contains("两份名单不同"), "{out}");

        let mine = palette_from_stored(&current);
        let same = render_palette_preview(path, &palette_read_text(&mine.to_text()), &current);
        assert!(same.contains("两份名单相同"), "{same}");
        assert!(same.contains("（没有读不出的行）"), "{same}");
        assert!(
            same.contains("[palette] 读到 shared.txt：10 行、留下 10 枚"),
            "{same}"
        );
        // 「未应用」那一句是 `--confirm` 那一道闸的话，不属于这份读数。
        assert!(!same.contains("未应用"), "{same}");
        assert!(!same.contains("已应用"), "{same}");
    }

    /// 落盘之后那一行报的枚数，报的是**模型留下的那些枚**，不是文件行数、也不是本机枚数；
    /// 校正那一句两半都印得出，「零项」与「没去看」得长得不一样。
    #[test]
    fn render_palette_applied_counts_the_row_it_written_and_the_corrections_it_found() {
        let one = render_palette_applied(3, 7, &["annotation.palette: dropped x".to_string()]);
        assert!(
            one.contains("[palette] 已应用 3 枚，写盘耗时 7 ms"),
            "{one}"
        );
        assert!(one.contains("应用后又校正 1 项"), "{one}");
        assert_eq!(
            one.lines()
                .filter(|l| l.starts_with("  应用时校正 "))
                .count(),
            1,
            "{one}"
        );
        let none = render_palette_applied(10, 1, &[]);
        assert!(none.contains("应用后又校正 0 项"), "{none}");
        assert!(none.contains("（应用时没有再校正任何项）"), "{none}");
        assert_eq!(
            none.lines()
                .filter(|l| l.starts_with("  应用时校正 "))
                .count(),
            0,
            "{none}"
        );
        assert_eq!(none.lines().count(), 3, "{none}");
    }

    /// alpha 在这一手是**丢弃**而不是支持：一份写着 `#E8112380` 的档，模型留下的是 `#E81123`。
    /// 这与 V1.181 块 ⑨ 那句「递给 QML 的门把手只带 RGB」是同一件事的两面，钉在这里是为了让
    /// 「共享的那份文件」与「屏幕上那一排」说的是同一枚颜色——而不是档里写着八位、屏上画着六位。
    #[test]
    fn a_shared_palette_carries_no_alpha_and_the_band_reads_the_same_row() {
        let read = palette_read_text("#E8112380\n#80E81123\n");
        assert_eq!(read.palette.to_text(), "#E81123\n#80E811");
        let stored: Vec<String> = read
            .palette
            .to_text()
            .lines()
            .map(|l| l.to_string())
            .collect();
        assert_eq!(band_colors_joined(&stored), "E81123|80E811");
    }
}
