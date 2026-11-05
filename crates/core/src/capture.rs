//! Screen/window capture and the frozen-frame model.
//!
//! The mask never enters a screenshot because the whole screen is captured
//! **before** any mask window exists, and every later crop reads from that
//! snapshot (plan §4-M1). Colour picking and the magnifier read the same
//! snapshot, which is why they are correct under mixed DPI and under the
//! dimmed overlay.

use crate::frame::{Frame, FrameError};
use crate::geometry::{PhysPoint, PhysRect, Scale};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture backend failed: {0}")]
    Backend(String),
    #[error("no monitor matches {0:?}")]
    UnknownMonitor(String),
    #[error("region {0:?} is outside every monitor")]
    OutsideAllMonitors(PhysRect),
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("nothing to capture")]
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub id: String,
    pub name: String,
    pub friendly: String,
    /// Physical pixels, in virtual-desktop coordinates.
    pub bounds: PhysRect,
    pub scale: Scale,
    pub primary: bool,
    pub builtin: bool,
}

impl MonitorInfo {
    pub fn contains(&self, p: PhysPoint) -> bool {
        self.bounds.contains(p)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub hwnd: u32,
    pub title: String,
    pub bounds: PhysRect,
    /// `DWMWA_EXTENDED_FRAME_BOUNDS`, when the platform could read it. This is
    /// the visually correct crop for a window shot (it drops the invisible
    ///收缩边 that `GetWindowRect` includes).
    pub dwm_bounds: Option<PhysRect>,
    pub app_name: String,
    /// Full executable path, filled by the platform layer when the backend
    /// cannot name it. Hotkey exclusion rules match on this (PRD §5.16.2).
    pub process_path: String,
    pub pid: u32,
    pub minimized: bool,
    pub focused: bool,
    pub z_order: usize,
    pub own_process: bool,
    /// §5.3.2 规则第一句里「完全透明窗口」那一格：这扇窗**自己的整体不透明度**，
    /// 0（全透）到 255（完全不透），由 [`WindowExtras::window_opacity`] 问出来。
    ///
    /// `None` **不是**「全透」也不是「不透」，是「平台没说」—— 与
    /// [`WindowExtras::window_rect`] 头上那句同一口径：不拿读不到的东西去冒充一个答案。
    /// 所以 [`window_is_shootable`] 只把 `Some(0)` 判掉，`None` 与 `Some(1..=255)` 都放行：
    /// 半透的一扇窗（毛玻璃、贴图的 60% 不透明度）屏幕上看得见，就该能截。
    ///
    /// 出厂 `None`：`xcap` 0.9.8 的 `Window` 没有回答这一枚的手，`XcapCapture::windows`
    /// 也不猜。这一格是给平台层的**槽**，填了它，排除规则当场就生效，不需要第二处改。
    pub opacity: Option<u8>,
}

impl WindowInfo {
    /// What a window screenshot should use.
    pub fn visible_bounds(&self) -> PhysRect {
        self.dwm_bounds.unwrap_or(self.bounds)
    }
}

/// PRD §5.3.2 规则第一句「应排除不可见窗口和完全透明窗口」，落成**一处**可判的判断。
///
/// 说的是「这扇窗在屏幕上有没有一块可截的内容」，而不是「它此刻在不在最上面」—— 后者是
/// [`window_at_in`] 的次序问题。三条都在 `WindowInfo` 已有的数据里，所以这一手不用问桌面，
/// 单测递一组候选就能判：
///
/// - `minimized`：最小化的窗在屏幕上没有自己那块位置（§5.2.6 规则第二句同一条口径）。
/// - `bounds` / [`WindowInfo::visible_bounds`] 为空：量不出矩形就说不出截哪一块。
///   两条都查而不是只查一条，因为 DWM 那一张可能比 `GetWindowRect` 更窄，反之也可能。
/// - `opacity == Some(0)`：平台明确说了这扇窗整体全透，屏幕上看得见的是它**后面**那一扇。
///
/// `opacity = None`（平台没说）放行而不是拦下：拦下就等于把「量不到」写成「看不见」，
/// 而 §5.3.2 要排除的是**看得见没有内容的窗**，不是**平台说不清的窗**。
///
/// 「不可见」这一半在本仓的出厂路径上主要由上游兜着 —— `xcap::Window::all()` 的
/// `is_valid_window` 已经筛掉 `IsWindowVisible` 为假、被 DWM cloak 掉、以及类名为
/// `Progman`/`Button` 的那几类，所以 `XcapCapture` 交回来的候选本来就都是可见者。这一手
/// 管的是**换了一个后端**（`FrameSource` 是可插拔的）时规矩仍然由一处说了算。
pub fn window_is_shootable(w: &WindowInfo) -> bool {
    !w.minimized && !w.bounds.is_empty() && !w.visible_bounds().is_empty() && w.opacity != Some(0)
}

/// PRD §5.3.2 规则第二句「多个窗口重叠时，优先选择光标最直接指向的可见窗口」里，
/// 「最直接」落成的那条**可判**次序：在通过 [`window_is_shootable`] 的候选里，取可见边界
/// 覆盖住 `p` 的那些当中 `z_order` 最小（最靠前）的一扇。
///
/// 为什么 z-order 就是「最直接指向」：`p` 那一点同时落在好几扇窗的矩形里时，用户屏幕上
/// 看见的只有最上面那一扇，其余几扇在那一点被完全盖住 —— 所以「光标指向的内容」与
/// 「z-order 最靠前的可见者」是同一件事的两种说法，而后者可判、前者要读心。
///
/// 次序**在这一手内部**按 `z_order` 重排，不靠调用方交来的顺序：`Vec<WindowInfo>` 从
/// `xcap` 出来是 z-order 降下的，但那是一个后端的事实，不是这一手的假设；把它写成假设，
/// 换一次枚举顺序就会悄悄选中底下那扇窗。`sort_by_key` 是稳定排序，`z_order` 相同（同一扇
/// 窗被列了两次）时保持交来的顺序，不引入第三种随机。
///
/// 没有任何一扇通过 ⇒ `None`，这是答案而不是失败：光标此刻落在桌面／被排除的窗上，
/// §5.3.2 第四那一步就没有可选中的整窗，界面那一腿要退回自由选区。
pub fn window_at_in<'a>(candidates: &'a [WindowInfo], p: PhysPoint) -> Option<&'a WindowInfo> {
    let mut hits: Vec<&WindowInfo> = candidates
        .iter()
        .filter(|w| window_is_shootable(w))
        .filter(|w| w.visible_bounds().contains(p))
        .collect();
    hits.sort_by_key(|w| w.z_order);
    hits.into_iter().next()
}

/// §5.3.2 规则第一句里「完全透明」那一问：把平台层问到的 `GetLayeredWindowAttributes` 三格
/// 读数收成 [`WindowInfo::opacity`] 那一格。**读法在这里，问数在平台层**，因为这一格不需要
/// 桌面就能判错，而 `crates/platform-windows/src/win.rs` 里那一手需要。
///
/// 三格读数的意思（Win32 那一侧交回来的原话）：
///
/// - `answered == false` —— 那一手失败。两种窗口都会失败：根本没置 `WS_EX_LAYERED` 的（绝大多数），
///   和用 `UpdateLayeredWindow` 逐像素给 alpha 的（这一路问不出整体值）。两种都答 `None` =
///   「系统没说这扇窗整体多透」，于是 [`window_is_shootable`] 放行。**这里不许把「问不出」写成
///   「全透」**：那样一次就筛掉整批正常窗口，比漏掉一扇透明窗严重得多。
/// - `answered && alpha_flag` —— `LWA_ALPHA` 置着，`alpha` 就是这扇窗的整体不透明度，原样交回。
///   其中 `0` 正是 §5.3.2 要排除的那一格（屏幕上看得见的是它**后面**那一扇）。
/// - `answered && !alpha_flag` —— 只有 `LWA_COLORKEY`：这扇窗靠某个颜色镂空，整体 alpha 从来没被
///   设过，`alpha` 交回的是无意义的数。答 `None`，与「问不出」同一格。已知欠账（§10）：整窗都
///   被镂成 key 色的窗在这一路上仍然算可截，那一格要的是逐像素的读法。
pub fn layered_window_opacity(answered: bool, alpha_flag: bool, alpha: u8) -> Option<u8> {
    (answered && alpha_flag).then_some(alpha)
}

/// Implemented by the platform layer for things xcap does not cover: the DWM
/// bounds of one window and the UI-element hit test (PRD §5.3.3/§5.3.4).
pub trait WindowExtras: Send + Sync {
    fn dwm_bounds(&self, hwnd: u32) -> Option<PhysRect>;
    /// UIA element bounds under the cursor, innermost first (wheel cycles).
    fn element_chain(&self, p: PhysPoint) -> Vec<PhysRect>;
    fn cursor_image(&self, p: PhysPoint) -> Option<(Frame, PhysPoint)>;
    /// (executable name, full path) of the process owning `hwnd`.
    fn process_path(&self, hwnd: u32) -> Option<(String, String)>;
    /// `GetWindowRect`: the window's *own* rectangle, which on Windows 10/11 is
    /// wider than [`WindowExtras::dwm_bounds`] by the invisible resize band where
    /// the DWM draws its drop shadow. Read by [`window_shadow_margin`] when
    /// §5.2.6's 「包含窗口阴影」 is on and the user gave no explicit width.
    ///
    /// `None` is not an error: it means this platform could not name the band, and
    /// the shadow then falls back to [`WINDOW_SHADOW_MARGIN`] rather than to
    /// "no shadow" — the switch was asked for, the picture must get bigger.
    fn window_rect(&self, hwnd: u32) -> Option<PhysRect>;
    /// The radius the *system* rounds this window's own corners with, in physical
    /// pixels (`DWMWA_WINDOW_CORNER_PREFERENCE` + that window's DPI). This is
    /// §5.5.6 步骤 3 的「使用系统窗口原始圆角」: 0 means the window really is
    /// square-topped, which is an answer, while `None` means the platform will not
    /// say and [`resolved_corner_radius`] must therefore not invent one.
    fn window_corner_radius(&self, hwnd: u32) -> Option<u32>;
    /// PRD §5.3.2 规则第一句里「完全透明」那一问：这扇窗**自己的**整体不透明度。
    ///
    /// 带**默认实现**（`None` = 这一层的平台没这把手），理由是这一格的存在方式而不是省事：
    /// §5.3.2 的排除规矩只有一个作者（[`window_is_shootable`]），而它读的必须是
    /// [`WindowInfo::opacity`] 这一格数据；数据由谁填是各平台层自己的事，答不出就让它
    /// 停在 `None`，不许在这一处替平台编一个「不透」—— [`WindowExtras::window_rect`]
    /// 头上那条「`None` is not an error」在这里同样成立。
    ///
    /// Windows 那一侧的答案是 `GetLayeredWindowAttributes`（`WS_EX_LAYERED` 且用整体 alpha
    /// 的那一类），`Some(0)` 才是「完全透明」。用 `UpdateLayeredWindow` 逐像素透的窗那一手
    /// 问不出来，会回到 `None`，与答不出同一格 —— 已知欠账，见 §10。
    ///
    /// 那一手交回的三格读数怎么收成这一枚 `Option<u8>`，作者不在这一行：
    /// [`layered_window_opacity`]。实现者只问数、不第二次算。
    fn window_opacity(&self, _: u32) -> Option<u8> {
        None
    }
}

pub struct NoopExtras;

impl WindowExtras for NoopExtras {
    fn dwm_bounds(&self, _: u32) -> Option<PhysRect> {
        None
    }
    fn element_chain(&self, _: PhysPoint) -> Vec<PhysRect> {
        Vec::new()
    }
    fn cursor_image(&self, _: PhysPoint) -> Option<(Frame, PhysPoint)> {
        None
    }
    fn process_path(&self, _: u32) -> Option<(String, String)> {
        None
    }
    fn window_rect(&self, _: u32) -> Option<PhysRect> {
        None
    }
    fn window_corner_radius(&self, _: u32) -> Option<u32> {
        None
    }
}

/// §5.2.6 规则第一句「可按设置决定是否包含窗口阴影」里，阴影落在几何上的那一格。
///
/// 窗口截图的像素来自 `PrintWindow`（[`FrameSource::capture_window`]），而 DWM 画在窗口
/// 外头的那圈投影**不在那张位图里** —— 真实窗口框（[`WindowExtras::window_rect`]）比可见边
/// 宽出来的那一条带，位图里是应用自己从没画过的地方，多裁一块只会得到一条透明的带，不是
/// 阴影。所以「开」在这一路上的做法是**补一圈**并把阴影画进那一圈，用的就是 §5.5.7 已经有
/// 的那只手（[`crate::imageops::plate`]），而不是在这里写第二种阴影算法。
///
/// 这一枚常数管的是「两样都量不到」时的那一圈有多宽：Windows 10/11 那条不可见收缩边实测
/// 8 px 上下，量不到窗口自己的带、用户也没在 `[output] shadow_size` 里写过宽度时，按它算
/// —— 因为开关开着而框不变大，等于这一格没有读者。
pub const WINDOW_SHADOW_MARGIN: u32 = 8;

/// §5.2.6 的「阴影」那一格，两条路。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowShadow {
    /// 关：截图就是 DWM 可见边，不给阴影留位置。出厂那一侧，今天的字节形状。
    Off,
    /// 开：每边外扩 [`window_shadow_margin`] 算出的那一圈，把 `color` 的阴影画进去。
    /// `configured` 是用户为阴影配过的宽度（`[output] shadow_size`，0 = 没配过）；
    /// `color` 是同一节给的阴影颜色与强度（`[output] shadow_strength` 换算的那一枚）。
    On { configured: u32, color: [u8; 4] },
}

/// §5.2.6 的「圆角」那一格，加上 §5.5.6 步骤 3「设置圆角半径，或使用系统窗口原始圆角」
/// 在两读法之间的分岔。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowCorner {
    /// 关：方角，一格不切（出厂那一侧）。
    Off,
    /// 开：`requested` > 0 就用它；== 0 就用 `system`（平台量的那枚系统原始圆角）；
    /// 系统也答不出（`None`）就不切 —— 替用户猜一个半径不是 §5.5.6 问过的事。
    /// `outside` 是切下来的四格里外头变成什么：透明，或 `[output] corner_fill` 那一枚色。
    On {
        requested: u32,
        system: Option<u32>,
        outside: crate::imageops::Outside,
    },
}

/// 阴影那一圈有多宽：**窗口自己的带、用户配的宽度、本模块的常数，三者取最宽**。
///
/// 取最宽而不是先到先得，是因为这三格各自是一种实话：量出来的那条带说的是这台机器上
/// DWM 把阴影画在多远（§5.2.6 的「窗口阴影」），配出来的那一枚说的是用户在 §5.5.7 里
/// 为阴影留过多少边距，而常数说的是「开关开着总得有个最小值」。谁也不许被静默顶掉：
/// 用户写了 24 而窗口只有 8 的带，拿 24；窗口有 20 的带而用户没配，拿 20。
///
/// `real` 比 `visible` 窄（DWM 有时对某些窗口交回相反的包含关系）时那一条带读成 0，
/// 不读成负数，因此这一枚函数在任何输入下都至少交回 [`WINDOW_SHADOW_MARGIN`] ——
/// 「开」一定让框变大，这一句就是被这条下界钉住的。
pub fn window_shadow_margin(visible: &PhysRect, real: Option<&PhysRect>, configured: u32) -> u32 {
    let band = match real {
        Some(r) => {
            let inset = [
                visible.x - r.x,
                visible.y - r.y,
                r.right() - visible.right(),
                r.bottom() - visible.bottom(),
            ];
            inset
                .into_iter()
                .map(|v| v.max(0) as u32)
                .max()
                .unwrap_or(0)
        }
        None => 0,
    };
    band.max(configured).max(WINDOW_SHADOW_MARGIN)
}

/// §5.5.6 步骤 3 的两读法落成一枚半径：配过的半径优先，其次是系统给这扇窗切的圆角，
/// 都没有就是 0（不切）。
///
/// `system == Some(0)` 是答案不是缺答：那扇窗本来就是方角（`DWMWCP_DONOTROUND`），
/// 照 0 切等于不切，而这正是用户看得见的那扇窗的形状。
pub fn resolved_corner_radius(requested: u32, system: Option<u32>) -> u32 {
    if requested > 0 {
        requested
    } else {
        system.unwrap_or(0)
    }
}

/// 一扇窗的截图，按 §5.2.6 那三项设置走完最后一程：像素进、像素出，外加它现在盖住的
/// 那块屏幕矩形（贴图落位与 §5.2.5 的「上一次」都要它）。
///
/// 三项的顺序是有对的，不是随手排的：**指针先落进画面，再切圆角，再长阴影圈**。
/// 这与 [`crate::imageops::Decoration::apply`] 头顶那一句同源（圆角先于铭牌，否则铭牌会在
/// 即将变透明的像素底下画阴影），而指针排在最前是 §5.4.3 第 419 行的读法：一枚停在窗口角上
/// 的指针若先被圆角裁掉半只，图上就没有「用户此刻看见的那根箭头」了。
///
/// 全部关掉时交回的就是进来的那一张：`Off`/`Off`/`None` 一个像素也不动、矩形一字不改 ——
/// 这一句是 §10 那一族「默认那一侧不许挪字节」的判据在这条路上的写法，单测钉着它。
pub fn window_picture(
    src: &Frame,
    visible: &PhysRect,
    real: Option<&PhysRect>,
    shadow: WindowShadow,
    corner: WindowCorner,
    cursor: Option<(&Frame, PhysPoint)>,
) -> Result<(Frame, PhysRect), CaptureError> {
    let mut out = src.clone();
    let mut rect = *visible;
    // §5.4.3 第 419 行「指针热点位置应与实际屏幕位置一致」：`cursor` 交回来的那个点是
    // **指针图的左上角**在屏幕上的位置（平台层已经减过热点，见 `platform_windows::cursor`），
    // 而这一格要的偏移是相对这张图的左上角，所以减的是 `rect`，不是屏幕原点。裁剪由
    // [`Frame::blend_over`] 自己做，这里不手写第二遍边界。
    if let Some((img, at)) = cursor {
        out.blend_over(img, PhysPoint::new(at.x - rect.x, at.y - rect.y), 1.0);
    }
    let (radius, outside) = match corner {
        WindowCorner::Off => (0, None),
        WindowCorner::On {
            requested,
            system,
            outside,
        } => (resolved_corner_radius(requested, system), Some(outside)),
    };
    if radius > 0 {
        out = crate::imageops::rounded(
            &out,
            radius,
            outside.unwrap_or(crate::imageops::Outside::Transparent),
        )?;
    }
    if let WindowShadow::On { configured, color } = shadow {
        let margin = window_shadow_margin(&rect, real, configured);
        out = crate::imageops::plate(
            &out,
            &crate::imageops::Plate {
                shadow_margin: margin,
                // 与 §5.5.7 同一读法：强度说多黑、边距说多远，fade 花掉它被给的整圈。
                shadow_radius: margin,
                shadow_color: color,
                ..Default::default()
            },
        )?;
        rect = rect.inflate(margin);
    }
    Ok((out, rect))
}

/// §5.2.6 规则第二句「最小化窗口不可作为活动窗口截图对象」的判据本体。
///
/// 一格一种拒绝，措辞留给调用方（那句中文由 `qt_bridge::capture` 印，它的门钉的是字，
/// 这一格钉的是**次序与分支**）。「查不到主人」排在「最小化」之前，是可信度的次序而不只是
/// 成本的次序：一个 shell 不肯解释的把手，后面那些读数值不值得信本身就是问题。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveWindowBlock {
    /// 前台把手问不出它是哪个进程：受保护的进程或别的会话。
    UnknownOwner,
    /// 前台是本程序自己：贴图截贴图只会得到一张图里的图。
    SelfForeground,
    /// 最小化：桌面上没有它的画面可截。这一格就是规则第二句。
    Minimized,
    /// 会话锁屏，前台是锁屏那扇窗。
    LockedSession,
}

impl ActiveWindowBlock {
    /// 规则第二句那一格：只有 `Minimized` 是 PRD 第 239 行点名的那一种。
    pub fn is_minimised(&self) -> bool {
        *self == ActiveWindowBlock::Minimized
    }
}

/// [`ActiveWindowBlock`] 的判定表。四个输入都是平台各自答过一次的问题，
/// 所以这一格不需要桌面就能测：把表递进来，看它先撞上哪一条。
pub fn active_window_block(
    owner: Option<u32>,
    self_pid: u32,
    minimized: bool,
    locked: bool,
) -> Option<ActiveWindowBlock> {
    let Some(pid) = owner else {
        return Some(ActiveWindowBlock::UnknownOwner);
    };
    if pid == self_pid {
        return Some(ActiveWindowBlock::SelfForeground);
    }
    if minimized {
        return Some(ActiveWindowBlock::Minimized);
    }
    if locked {
        return Some(ActiveWindowBlock::LockedSession);
    }
    None
}

/// Where pixels come from. `xcap`'s GDI path is the default; plan P5 measured
/// WGC as slower on every case that matters here.
pub trait FrameSource: Send + Sync {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;
    fn capture_monitor(&self, id: &str) -> Result<Frame, CaptureError>;
    fn capture_monitor_region(&self, id: &str, rel: &PhysRect) -> Result<Frame, CaptureError> {
        let _ = (id, rel);
        Err(CaptureError::Backend("region capture unsupported".into()))
    }
    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError>;
    fn capture_window(&self, hwnd: u32) -> Result<Frame, CaptureError>;
    fn name(&self) -> &'static str {
        "xcap-gdi"
    }
}

/// A monitor whose content is already decoded, so repeated selections never
/// re-read the screen.
#[derive(Clone, Debug)]
pub struct FrozenMonitor {
    pub info: MonitorInfo,
    pub frame: Frame,
}

#[derive(Clone, Debug, Default)]
pub struct ScreenSnapshot {
    pub monitors: Vec<FrozenMonitor>,
    pub virtual_bounds: PhysRect,
    /// Wall-clock ms since epoch, for history rows.
    pub taken_at_ms: i64,
    pub backend: &'static str,
}

#[derive(Clone, Debug)]
pub struct Captured {
    pub frame: Frame,
    /// Physical rect the result really covers after clipping to the desktop.
    pub rect: PhysRect,
    pub monitors: Vec<String>,
    /// Set when the requested rect did not fit and was clipped (PRD §5.2.5/§8.5).
    pub clipped: bool,
}

/// §5.4.1 第 3 步要说的三件事一起装配好：光标附近那一块的**放大像素**、**坐标**、
/// **颜色值**。它是这一族里唯一的一位作者 —— 桥与 QML 只做搬运，不再各写一遍像素选择、
/// 坐标字符串和颜色字符串，而 §5.4.2 第 4 步「系统复制当前颜色值」用的正是这里的
/// [`ScreenSnapshot::color_label`]，两处读的是同一句格式化。
///
/// 三条数字都从**冻住的那一帧**答（[`ScreenSnapshot::color_at`] /
/// [`ScreenSnapshot::magnifier`]），所以「颜色值应基于原始屏幕像素，不应受截图遮罩影响」
/// 这一条在这一层是结构性成立的：这里从不问桌面，只问那一份快照。
///
/// [`Self::show`] 抄的是 [`crate::config::Capture::magnifier_enabled`] —— §5.19.3
/// 最后一枚「是否显示放大镜」与 §5.4.1 第 4 步「用户可在设置中……隐藏放大镜」的那一枚读者。
/// `false` 时其余读数照旧组装，只是遮罩不再画这层，这样关掉放大镜**不需要**关掉其它逻辑。
#[derive(Clone, Debug, PartialEq)]
pub struct MagnifierView {
    /// 光标附近的放大像素。`None` 是"这一场连一块源像素都取不到"，不是"应当画黑色"——
    /// 遮罩那一侧要能辨出这一格是没内容而不是空色板，与 [`ScreenSnapshot::magnifier`] 的
    /// `Err` 同一族。
    pub frame: Option<Frame>,
    /// 放大镜所对的那一枚**桌面物理点**（调用方已经把它夹进桌面范围）。
    pub point: PhysPoint,
    /// `point` 那一枚原始屏幕像素。`None` = 快照里没有哪块屏覆盖它（跨屏缝外、或
    /// 尚未冻帧）。
    pub color: Option<[u8; 4]>,
    /// §5.19.3「是否显示放大镜」。`false` 时遮罩不画这层，其余读数仍算得出来。
    pub show: bool,
    /// §5.19.3「网格显示」—— 只有**这一枚**决定要不要在放大块上叠一层网格。像素与
    /// 字符串都不受影响，画不画由遮罩按这一格决定。
    pub grid: bool,
    /// §5.4.1「坐标」那一句。`magnifier_coordinates = false` 时是空串（不是 0）。
    pub coord_text: String,
    /// §5.4.1「颜色值」那一句，按 `magnifier_color_format` 走
    /// [`crate::colors::format`] 装配。读不到源像素时是空串（不是"未知"，也不是
    /// "#000000"——那是伪造的一个读数）。
    pub color_text: String,
}

/// `magnifier_color_format` / 取色器默认复制格式里那一枚名字 → [`crate::colors::ColorFormat`]。
///
/// [`crate::config::Config::validate`] 已经在开档时把这一枚夹回了已知名单，这里再兜一次
/// 只为让 `Capture::default()` 之外**手写**一份 `Capture` 的读者（单测、别的路径）不会拿到
/// 一句关于整份配置的抱怨、而是回到 PRD 明示的第一格（HEX 大写）。
fn pick_format(name: &str) -> crate::colors::ColorFormat {
    crate::colors::ColorFormat::from_name(name).unwrap_or(crate::colors::ColorFormat::HexUpper)
}

impl ScreenSnapshot {
    pub fn monitor_at(&self, p: PhysPoint) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|m| m.info.contains(p))
            .map(|m| &m.info)
    }

    /// Primary wins ties; used to pick which scale a mixed-DIP readout shows.
    pub fn monitor_for_rect(&self, r: &PhysRect) -> Option<&MonitorInfo> {
        let mut best: Option<(u64, &MonitorInfo)> = None;
        for m in &self.monitors {
            let Some(hit) = m.info.bounds.intersection(r) else {
                continue;
            };
            let a = hit.area();
            let better = match best {
                None => true,
                Some((ba, bi)) => a > ba || (a == ba && m.info.primary && !bi.primary),
            };
            if better {
                best = Some((a, &m.info));
            }
        }
        best.map(|(_, m)| m)
    }

    pub fn scale_at(&self, p: PhysPoint) -> Scale {
        self.monitor_at(p).map(|m| m.scale).unwrap_or(Scale::ONE)
    }

    /// Colour of the raw screen pixel (PRD §5.4.2: unaffected by the overlay).
    pub fn color_at(&self, p: PhysPoint) -> Option<[u8; 4]> {
        let m = self.monitors.iter().find(|m| m.info.contains(p))?;
        let x = (p.x - m.info.bounds.x) as u32;
        let y = (p.y - m.info.bounds.y) as u32;
        if x >= m.frame.width || y >= m.frame.height {
            return None;
        }
        Some(m.frame.get(x, y))
    }

    /// Magnifier content: a square neighbourhood around `p` scaled up with hard
    /// edges, in destination coordinates relative to `p`.
    /// `radius` is in *screen* pixels around `p`; `zoom` is the magnification
    /// factor. The output is therefore `(2*radius+1) * zoom` a side, so a hard
    /// edge landing between two source pixels stays a hard edge in the output
    /// (PRD §5.4.1 — the magnifier is used to align a selection to a pixel).
    pub fn magnifier(&self, p: PhysPoint, radius: u32, zoom: u32) -> Result<Frame, CaptureError> {
        let radius = radius.max(1);
        let zoom = zoom.clamp(1, 64);
        let src_side = radius * 2 + 1;
        let mut out = Frame::filled(src_side * zoom, src_side * zoom, [0, 0, 0, 255])?;
        for dy in 0..src_side {
            for dx in 0..src_side {
                let sp = PhysPoint::new(
                    p.x - radius as i32 + dx as i32,
                    p.y - radius as i32 + dy as i32,
                );
                if let Some(c) = self.color_at(sp) {
                    out.fill_rect(
                        &PhysRect::new((dx * zoom) as i32, (dy * zoom) as i32, zoom, zoom),
                        c,
                    );
                }
            }
        }
        Ok(out)
    }

    /// §5.4.1 第 3 步：放大镜这一块的完整读数一次装配 —— 像素走 [`Self::magnifier`]，
    /// 颜色走 [`Self::color_at`]，坐标与颜色字符串在这里拼。它是这一族唯一的作者：桥只把
    /// [`Self::frame`] 送进纹理、QML 只绑字符串，两处都不再各写一份放大/坐标/格式化。
    ///
    /// `cfg` 只取放大镜那一族（`magnifier_enabled` / `_radius` / `_zoom` / `_grid` /
    /// `_coordinates` / `_color_format`），所以运行期改这几格、下一趟 hover 就用新的；其余
    /// `[capture]` 键这一枚函数一概不看。
    pub fn magnifier_view(&self, p: PhysPoint, cfg: &crate::config::Capture) -> MagnifierView {
        let radius = cfg.magnifier_radius.max(1);
        let zoom = cfg.magnifier_zoom.clamp(1, 64);
        let frame = self.magnifier(p, radius, zoom).ok();
        let color = self.color_at(p);
        let coord_text = if cfg.magnifier_coordinates {
            format!("{}, {}", p.x, p.y)
        } else {
            String::new()
        };
        let color_text = match color {
            Some(c) => crate::colors::format(c, pick_format(&cfg.magnifier_color_format)),
            None => String::new(),
        };
        MagnifierView {
            frame,
            point: p,
            color,
            show: cfg.magnifier_enabled,
            grid: cfg.magnifier_grid,
            coord_text,
            color_text,
        }
    }

    /// §5.4.2 第 3-4 步：按 `C` 要复制进剪贴板的那一串颜色值。读的是**冻结那一帧**的原始
    /// 屏幕像素（[`Self::color_at`]），不是屏幕合成结果，所以「颜色值应基于原始屏幕像素、
    /// 不应受截图遮罩影响」这一条在这里结构性成立。`format_name` 是用户配置的默认复制格式
    /// （[`crate::colors::ColorFormat`] 的名字）；不认识的落回 HEX 大写，与
    /// [`crate::config::Config::validate`] 对 `magnifier_color_format` 的兜底同一格。
    ///
    /// 返回 `None` = 没有任何一块屏覆盖 `p`（跨屏缝外、或还没冻帧），调用方据此什么也不复制，
    /// 而不是复制一个伪造的颜色。
    pub fn color_label(&self, p: PhysPoint, format_name: &str) -> Option<String> {
        let c = self.color_at(p)?;
        Some(crate::colors::format(c, pick_format(format_name)))
    }

    /// Cross-monitor capture of an arbitrary rect: one output image, stitched
    /// in physical space (PRD §5.3.10 / §4.3).
    pub fn capture(&self, want: &PhysRect) -> Result<Captured, CaptureError> {
        if want.is_empty() {
            return Err(CaptureError::Empty);
        }
        let (rect, clipped) = match want.clamp_to(&self.virtual_bounds) {
            Some(v) => v,
            None => return Err(CaptureError::OutsideAllMonitors(*want)),
        };
        let mut out = Frame::new(rect.w, rect.h)?;
        // Areas covered by no monitor at all stay transparent; PRD §5.3.10
        // forbids stretches, not gaps, but we report them.
        let mut used = Vec::new();
        for m in &self.monitors {
            let Some(hit) = rect.intersection(&m.info.bounds) else {
                continue;
            };
            let src = PhysRect::new(
                hit.x - m.info.bounds.x,
                hit.y - m.info.bounds.y,
                hit.w,
                hit.h,
            );
            let piece = m.frame.crop(&src)?;
            out.paste(&piece, PhysPoint::new(hit.x - rect.x, hit.y - rect.y));
            used.push(m.info.id.clone());
        }
        if used.is_empty() {
            return Err(CaptureError::OutsideAllMonitors(*want));
        }
        Ok(Captured {
            frame: out,
            rect,
            monitors: used,
            clipped,
        })
    }
}

pub struct CaptureService<E: WindowExtras + ?Sized> {
    source: Box<dyn FrameSource>,
    pub extras: Box<E>,
    pub self_pid: u32,
}

impl CaptureService<NoopExtras> {
    pub fn new(source: Box<dyn FrameSource>) -> Self {
        Self {
            source,
            extras: Box::new(NoopExtras),
            self_pid: std::process::id(),
        }
    }
}

impl<E: WindowExtras + ?Sized> CaptureService<E> {
    pub fn with_extras(source: Box<dyn FrameSource>, extras: Box<E>) -> Self {
        Self {
            source,
            extras,
            self_pid: std::process::id(),
        }
    }

    pub fn source(&self) -> &dyn FrameSource {
        &*self.source
    }

    pub fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let mut v = self.source.monitors()?;
        v.sort_by(|a, b| {
            (!a.primary)
                .cmp(&!b.primary)
                .then(a.bounds.x.cmp(&b.bounds.x))
                .then(a.bounds.y.cmp(&b.bounds.y))
        });
        Ok(v)
    }

    pub fn virtual_bounds(&self) -> Result<PhysRect, CaptureError> {
        let ms = self.monitors()?;
        ms.iter()
            .map(|m| m.bounds)
            .reduce(|a, b| a.union(&b))
            .ok_or(CaptureError::Empty)
    }

    /// Step 1 of every capture flow, run before the mask window is created.
    pub fn freeze(&self) -> Result<ScreenSnapshot, CaptureError> {
        let monitors = self.monitors()?;
        let mut frozen = Vec::with_capacity(monitors.len());
        let mut virtual_bounds = PhysRect::default();
        let mut backend = "xcap-gdi";
        for info in monitors {
            backend = self.source.name();
            let frame = match self.source.capture_monitor(&info.id) {
                Ok(f) => f,
                // A monitor that refuses to be captured must not blank the
                // whole snapshot (PRD §7.2).
                Err(e) => {
                    tracing::warn!(monitor = %info.id, error = %e, "monitor capture failed");
                    Frame::new(info.bounds.w, info.bounds.h)?
                }
            };
            virtual_bounds = if virtual_bounds.is_empty() {
                info.bounds
            } else {
                virtual_bounds.union(&info.bounds)
            };
            frozen.push(FrozenMonitor { info, frame });
        }
        if frozen.is_empty() {
            return Err(CaptureError::Empty);
        }
        Ok(ScreenSnapshot {
            monitors: frozen,
            virtual_bounds,
            taken_at_ms: now_ms(),
            backend,
        })
    }

    /// Freeze + crop in one call: used by repeat-last-region, delayed capture,
    /// the CLI and any path with no interactive selection.
    pub fn capture_rect(&self, want: &PhysRect) -> Result<Captured, CaptureError> {
        self.freeze()?.capture(want)
    }

    pub fn capture_window(&self, hwnd: u32) -> Result<Captured, CaptureError> {
        let frame = self.source.capture_window(hwnd)?;
        let info = self.windows(true)?.into_iter().find(|w| w.hwnd == hwnd);
        let want = info
            .as_ref()
            .map(|w| w.visible_bounds())
            .unwrap_or_else(|| frame.bounds());
        let monitors = self
            .monitors()?
            .iter()
            .filter(|m| m.bounds.intersection(&want).is_some())
            .map(|m| m.id.clone())
            .collect();
        // P5: `PrintWindow`-backed shots drift in size, so never assume the
        // decoded frame matches the geometry we asked for.
        let cropped = if frame.width == want.w && frame.height == want.h {
            frame
        } else {
            let keep = PhysRect::new(0, 0, want.w.min(frame.width), want.h.min(frame.height));
            frame.crop(&keep)?
        };
        Ok(Captured {
            frame: cropped,
            rect: want,
            monitors,
            clipped: false,
        })
    }

    /// z-order top-down, with invisible / minimised / (optionally) our own
    /// windows removed, so hovering picks what the user sees (PRD §5.3.2).
    pub fn windows(&self, include_own_process: bool) -> Result<Vec<WindowInfo>, CaptureError> {
        let mut ws = self.source.windows()?;
        if !include_own_process {
            ws.retain(|w| !w.own_process);
        }
        ws.sort_by_key(|w| w.z_order);
        for w in &mut ws {
            if w.dwm_bounds.is_none() {
                w.dwm_bounds = self.extras.dwm_bounds(w.hwnd);
            }
            if w.process_path.is_empty() {
                if let Some((_, path)) = self.extras.process_path(w.hwnd) {
                    w.process_path = path;
                }
            }
            if w.opacity.is_none() {
                w.opacity = self.extras.window_opacity(w.hwnd);
            }
        }
        // §5.3.2 规则第一句在这一手生效，而**判据本身不在这里**（[`window_is_shootable`]）：
        // 原先这一行 `retain` 只管 minimized 与空矩形，遮罩那一路要的「完全透明也排除」没有
        // 落脚处，于是同一句规矩就会在两个调用方各长出一半。
        //
        // 排在上面那个 fill **之后**，不是之前：`dwm_bounds` 与 `opacity` 都是这一趟才问出来
        // 的，而 [`WindowInfo::visible_bounds`] 会换一张矩形 —— 排在之前就等于拿半份数据去判
        // 「这扇窗有没有可见内容」，那一格就会漏掉「后端不报、只有 DWM 说得出空」的那一类。
        ws.retain(window_is_shootable);
        Ok(ws)
    }

    /// The window the cursor points at most directly: topmost in z-order whose
    /// visible bounds contain the point.
    ///
    /// 两条规则的**判据**不在这一手里，在 [`window_is_shootable`] 与 [`window_at_in`]，因为
    /// 遮罩每次 hover 问的是同一句话而它手里只有一份缓存的候选表、不能再枚举一次桌面
    /// （`crates/qt_bridge/src/mask.rs`）。这一手因此只是「先取表、再问表」。
    pub fn window_at(&self, p: PhysPoint) -> Option<WindowInfo> {
        let ws = self.windows(false).ok()?;
        window_at_in(&ws, p).cloned()
    }

    /// PRD §5.3.4: the wheel cycles through the element chain under the cursor.
    pub fn element_chain(&self, p: PhysPoint) -> Vec<PhysRect> {
        self.extras.element_chain(p)
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------- xcap backend

pub struct XcapCapture;

fn monitor_id(index: usize, name: &str) -> String {
    let slug: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    format!("m{index}-{slug}")
}

impl FrameSource for XcapCapture {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let ms = xcap::Monitor::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(ms
            .iter()
            .enumerate()
            .map(|(i, m)| MonitorInfo {
                id: monitor_id(i, &m.name().unwrap_or_default()),
                name: m.name().unwrap_or_default(),
                friendly: m.friendly_name().unwrap_or_default(),
                bounds: PhysRect::new(
                    m.x().unwrap_or(0),
                    m.y().unwrap_or(0),
                    m.width().unwrap_or(0),
                    m.height().unwrap_or(0),
                ),
                scale: Scale::of(f64::from(m.scale_factor().unwrap_or(1.0))),
                primary: m.is_primary().unwrap_or(false),
                builtin: m.is_builtin().unwrap_or(false),
            })
            .collect())
    }

    fn capture_monitor(&self, id: &str) -> Result<Frame, CaptureError> {
        let m = self.monitor(id)?;
        let img = m
            .capture_image()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(Frame::from_image(img))
    }

    fn capture_monitor_region(&self, id: &str, rel: &PhysRect) -> Result<Frame, CaptureError> {
        let m = self.monitor(id)?;
        let img = m
            .capture_region(rel.x.max(0) as u32, rel.y.max(0) as u32, rel.w, rel.h)
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(Frame::from_image(img))
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        let ws = xcap::Window::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        let me = std::process::id();
        Ok(ws
            .iter()
            .enumerate()
            .map(|(z, w)| {
                let pid = w.pid().unwrap_or(0);
                WindowInfo {
                    hwnd: w.id().unwrap_or(0),
                    title: w.title().unwrap_or_default(),
                    bounds: PhysRect::new(
                        w.x().unwrap_or(0),
                        w.y().unwrap_or(0),
                        w.width().unwrap_or(0),
                        w.height().unwrap_or(0),
                    ),
                    dwm_bounds: None,
                    app_name: w.app_name().unwrap_or_default(),
                    process_path: String::new(),
                    pid,
                    minimized: w.is_minimized().unwrap_or(false),
                    focused: w.is_focused().unwrap_or(false),
                    z_order: z,
                    own_process: pid == me,
                    // `xcap` 0.9.8 的 `Window` 没有回答整体不透明度这把手，而**不知道**不等于
                    // 「全透」也不等于「不透」—— 交给 [`window_is_shootable`] 按 `None` 放行。
                    // 这一格等着 `WindowExtras::window_opacity` 在 Windows 那侧接上
                    // `GetLayeredWindowAttributes`：接上之后 [`CaptureService::windows`] 的
                    // fill 那一步会自己去问，这里不必改第二处。
                    opacity: None,
                }
            })
            .collect())
    }

    fn capture_window(&self, hwnd: u32) -> Result<Frame, CaptureError> {
        let ws = xcap::Window::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        let w = ws
            .iter()
            .find(|w| w.id().unwrap_or(0) == hwnd)
            .ok_or(CaptureError::Backend(format!("no window {hwnd:#08x}")))?;
        let img = w
            .capture_image()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(Frame::from_image(img))
    }

    fn name(&self) -> &'static str {
        if cfg!(feature = "wgc") {
            "xcap-wgc"
        } else {
            "xcap-gdi"
        }
    }
}

impl XcapCapture {
    fn monitor(&self, id: &str) -> Result<xcap::Monitor, CaptureError> {
        let ms = xcap::Monitor::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        ms.into_iter()
            .enumerate()
            .find(|(i, m)| monitor_id(*i, &m.name().unwrap_or_default()) == id)
            .map(|(_, m)| m)
            .ok_or_else(|| CaptureError::UnknownMonitor(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use crate::geometry::Scale;

    fn mon(id: &str, x: i32, w: u32, scale: f64) -> MonitorInfo {
        MonitorInfo {
            id: id.into(),
            name: id.into(),
            friendly: id.into(),
            bounds: PhysRect::new(x, 0, w, 100),
            scale: Scale::of(scale),
            primary: x == 0,
            builtin: false,
        }
    }

    fn snap_two() -> ScreenSnapshot {
        let mut left = Frame::new(200, 100).unwrap();
        left.fill_rect(&PhysRect::new(0, 0, 200, 100), [255, 0, 0, 255]);
        let right = Frame::filled(100, 100, [0, 0, 255, 255]).unwrap();
        ScreenSnapshot {
            monitors: vec![
                FrozenMonitor {
                    info: mon("a", 0, 200, 1.0),
                    frame: left,
                },
                FrozenMonitor {
                    info: mon("b", 200, 100, 2.0),
                    frame: right,
                },
            ],
            virtual_bounds: PhysRect::new(0, 0, 300, 100),
            taken_at_ms: 0,
            backend: "test",
        }
    }

    #[test]
    fn stitch_spans_two_monitors_without_stretch() {
        let s = snap_two();
        let c = s.capture(&PhysRect::new(180, 10, 40, 20)).unwrap();
        assert_eq!((c.frame.width, c.frame.height), (40, 20));
        assert_eq!(c.frame.get(0, 0), [255, 0, 0, 255]);
        assert_eq!(c.frame.get(20, 0), [0, 0, 255, 255]);
        assert_eq!(c.monitors, vec!["a".to_string(), "b".to_string()]);
        assert!(!c.clipped);
    }

    #[test]
    fn out_of_range_is_clipped_and_reported() {
        let s = snap_two();
        let c = s.capture(&PhysRect::new(290, 95, 50, 50)).unwrap();
        assert_eq!(c.rect, PhysRect::new(290, 95, 10, 5));
        assert!(c.clipped);
        assert!(s.capture(&PhysRect::new(1000, 0, 10, 10)).is_err());
        assert!(s.capture(&PhysRect::new(0, 0, 0, 10)).is_err());
    }

    #[test]
    fn colour_and_scale_follow_the_monitor() {
        let s = snap_two();
        assert_eq!(s.color_at(PhysPoint::new(250, 50)), Some([0, 0, 255, 255]));
        assert_eq!(s.scale_at(PhysPoint::new(250, 50)), Scale::of(2.0));
        assert_eq!(s.scale_at(PhysPoint::new(50, 50)), Scale::ONE);
        assert_eq!(s.color_at(PhysPoint::new(999, 0)), None);
        assert_eq!(
            s.monitor_for_rect(&PhysRect::new(190, 0, 40, 40))
                .unwrap()
                .id,
            "b"
        );
    }

    #[test]
    fn magnifier_zooms_with_hard_edges() {
        let s = snap_two();
        // Straddle the seam: monitor a (red) ends at x=200, b (blue) starts there.
        let f = s.magnifier(PhysPoint::new(200, 5), 2, 3).unwrap();
        assert_eq!((f.width, f.height), (15, 15));
        assert_eq!(f.get(0, 6), [255, 0, 0, 255]);
        assert_eq!(f.get(5, 6), [255, 0, 0, 255]);
        assert_eq!(f.get(6, 6), [0, 0, 255, 255]);
        assert_eq!(f.get(14, 6), [0, 0, 255, 255]);
        // Off-snapshot source pixels keep the frame filler instead of reading
        // another monitor's data.
        let edge = s.magnifier(PhysPoint::new(0, 0), 2, 2).unwrap();
        assert_eq!((edge.width, edge.height), (10, 10));
        assert_eq!(edge.get(0, 0), [0, 0, 0, 255]);
        assert_eq!(edge.get(2, 2), [0, 0, 0, 255]);
        assert_eq!(edge.get(4, 4), [255, 0, 0, 255]);
        assert_eq!(edge.get(9, 9), [255, 0, 0, 255]);
    }

    /// §5.4.1 第 3 步：一次装配同时说清"哪一块像素、坐标、颜色值"，且坐标与颜色字符串
    /// 按 cfg 里那三枚位走（`magnifier_enabled` / `_coordinates` / `_color_format`）。
    /// 判据都落在**字符串**与两枚开关位上，因为放大块的像素本体由上面的
    /// `magnifier_zooms_with_hard_edges` 作证，不在此重复。
    #[test]
    fn magnifier_view_names_the_pixel_the_coordinate_and_the_colour() {
        let s = snap_two();
        let cfg = crate::config::Capture::default();
        // 落在蓝屏 (x=250) 上：像素 [0,0,255,255]、坐标 "250, 50"、颜色 HEX 大写。
        let v = s.magnifier_view(PhysPoint::new(250, 50), &cfg);
        assert!(v.show, "出厂 magnifier_enabled = true 那一格");
        assert!(v.grid, "出厂 magnifier_grid = true 那一格");
        assert_eq!(v.color, Some([0, 0, 255, 255]));
        assert_eq!(v.coord_text, "250, 50");
        assert_eq!(v.color_text, "#0000FF");
        // 放大块的边长 = (2r+1) × z，与 magnifier() 共用同一把尺；这里不重算放大算法。
        let side = (cfg.magnifier_radius * 2 + 1) * cfg.magnifier_zoom;
        let f = v.frame.expect("冻帧内的一枚点必然画得出一块放大像素");
        assert_eq!((f.width, f.height), (side, side));
    }

    /// §5.19.3「是否显示放大镜」/ §5.4.1 第 4 步「隐藏放大镜」，与另两枚格式开关：关掉
    /// 什么都不该假装。`show=false` 只该翻转那一枚位，读数照旧算（遮罩据此整块不画）；
    /// 坐标关 ⇒ 空串；颜色格式换成 hsl ⇒ 那一句跟着变。三格互不牵连，正是「关掉放大镜
    /// 不等于关掉其它逻辑」的写法。
    #[test]
    fn magnifier_view_follows_the_three_config_knobs() {
        let s = snap_two();
        let mut cfg = crate::config::Capture::default();
        cfg.magnifier_enabled = false;
        let off = s.magnifier_view(PhysPoint::new(10, 50), &cfg);
        assert!(
            !off.show,
            "关着的那一枚必须被读到，而不是等到画的时候才发现"
        );
        assert_eq!(
            off.color_text, "#FF0000",
            "隐藏放大镜不该改变颜色读数本身；那一格只管画不画"
        );
        assert!(
            off.frame.is_some(),
            "关着只该藏那一格，不该把取像素的手也一并停掉"
        );

        cfg.magnifier_enabled = true;
        cfg.magnifier_coordinates = false;
        cfg.magnifier_color_format = "hsl".into();
        let hsl = s.magnifier_view(PhysPoint::new(10, 50), &cfg);
        assert_eq!(
            hsl.coord_text, "",
            "坐标关掉那一句就该是空，而不是 0 或上一句"
        );
        assert!(
            hsl.color_text.starts_with("hsl("),
            "格式认的是配置那一格：{:?}",
            hsl.color_text
        );
    }

    /// §5.4.2 第 3-4 步：按 `C` 复制的那一串就是这个函数，认不出的格式落回 HEX 大写
    /// （与 `validate` 对同一枚键的兜底同一格）；缝外没有任何一块屏时答 `None`，
    /// 调用方据此什么也不复制，而不是复制一个伪造的黑。
    #[test]
    fn colour_label_is_the_string_the_picker_copies() {
        let s = snap_two();
        let on_blue = PhysPoint::new(250, 50);
        assert_eq!(
            s.color_label(on_blue, "hex_upper").as_deref(),
            Some("#0000FF")
        );
        assert_eq!(
            s.color_label(on_blue, "rgb").as_deref(),
            Some("rgb(0, 0, 255)")
        );
        assert_eq!(
            s.color_label(on_blue, "crayon").as_deref(),
            Some("#0000FF"),
            "认不出的格式名不该报错，落回 PRD 明示的第一格"
        );
        assert_eq!(s.color_label(PhysPoint::new(999, 0), "hex_upper"), None);
    }

    /// §5.4.1「颜色值」与 §5.4.2「复制当前颜色值」读的是同一枚函数、同一份像素：这里钉
    /// 住那条跨屏缝上两侧各读各的 monitor 时两句同字，防止日后有一头漂成"另一个颜色字符串"。
    #[test]
    fn colour_label_and_magnifier_view_agree_at_the_seam() {
        let s = snap_two();
        let cfg = crate::config::Capture::default();
        for x in [199, 200] {
            let p = PhysPoint::new(x, 50);
            let v = s.magnifier_view(p, &cfg);
            assert_eq!(
                v.color_text,
                s.color_label(p, &cfg.magnifier_color_format)
                    .expect("on a monitor"),
                "放大镜里的颜色句与取色器复制的颜色句必须同一来源：x={x}"
            );
        }
        // 缝两侧各读各的 monitor：199 在 a（红）、200 在 b（蓝），不是同一句。
        assert_eq!(
            s.magnifier_view(PhysPoint::new(199, 50), &cfg).color_text,
            "#FF0000"
        );
        assert_eq!(
            s.magnifier_view(PhysPoint::new(200, 50), &cfg).color_text,
            "#0000FF"
        );
    }

    // -------------------------------------------------- PRD §5.2.6 的三项按设置走

    /// 一扇窗的可见边（DWM 扩展框架边界）与它的真实窗口框（GetWindowRect）：
    /// 后者每边宽出 8 px，那一条带就是 DWM 画投影的地方。
    fn visible_rect() -> PhysRect {
        PhysRect::new(100, 100, 20, 10)
    }

    fn real_rect() -> PhysRect {
        PhysRect::new(92, 92, 36, 26)
    }

    /// 20x10 的纯色"窗口内容"，每格都能和自己比对出来。
    fn window_frame() -> Frame {
        Frame::filled(20, 10, [10, 20, 30, 255]).unwrap()
    }

    /// §5.2.6 的默认那一侧：三项设置都不开，这张路上交回的必须是**进来那一张**，
    /// 一个像素不动、矩形一字不改。这一格是「新键不许挪出厂字节」的写法。
    #[test]
    fn a_window_shot_with_no_settings_is_the_picture_it_always_was() {
        let src = window_frame();
        let want = visible_rect();
        let (out, rect) = window_picture(
            &src,
            &want,
            Some(&real_rect()),
            WindowShadow::Off,
            WindowCorner::Off,
            None,
        )
        .unwrap();
        assert_eq!(out, src, "关着的时候这一路动过像素");
        assert_eq!(rect, want, "关着的时候这一路动过矩形");
    }

    /// 规则第一句的可测形状：**开与关得到的框尺寸不同**。
    ///
    /// 关着就是今天那张 20x10；开着必须长出阴影圈，而且那圈里得有阴影（不是留白），
    /// 窗口自己的内容还得在偏移之后的位置上 —— 只把框报大而图上没有那一圈，
    /// 是这一句判据最容易被糊过去的一种写法。
    #[test]
    fn shadow_on_and_shadow_off_do_not_agree_on_the_box() {
        let src = window_frame();
        let want = visible_rect();
        let (off, off_rect) = window_picture(
            &src,
            &want,
            Some(&real_rect()),
            WindowShadow::Off,
            WindowCorner::Off,
            None,
        )
        .unwrap();
        let (on, on_rect) = window_picture(
            &src,
            &want,
            Some(&real_rect()),
            WindowShadow::On {
                configured: 0,
                color: [0, 0, 0, 200],
            },
            WindowCorner::Off,
            None,
        )
        .unwrap();
        assert_ne!(
            (off.width, off.height),
            (on.width, on.height),
            "开关两边交回同一张尺子，这一格就没有读者"
        );
        // 关着的那一侧：还是今天那张 DWM 可见框，一个像素都不动。
        assert_eq!(off_rect, want, "关着的时候框被挪了");
        assert_eq!(off, src, "关着的时候像素被动了");
        // 真实框每边宽出 8 px ⇒ 那一条带就是这里的花销。
        assert_eq!((on.width, on.height), (20 + 16, 10 + 16));
        assert_eq!(on_rect, want.inflate(8), "报出去的框得对上这张图");
        assert_eq!(on_rect, real_rect(), "阴影圈该落在窗口真实框那一格");
        // 内容被整圈推走了，位置仍在（(0,0) 的内容现在在 (8,8)）。
        assert_eq!(on.get(8, 8), [10, 20, 30, 255]);
        assert_eq!(on.get(27, 17), [10, 20, 30, 255]);
        // 最外一圈留白（阴影不许把自己的边距填满），贴着内容的那一圈真的有阴影。
        assert_eq!(on.get(0, 13), [0, 0, 0, 0], "最外一圈不该有阴影");
        let near = on.get(7, 13);
        assert_eq!(&near[..3], &[0, 0, 0]);
        assert!(near[3] > 0, "圈里没有阴影，只是把图放大了：{near:?}");
    }

    /// §5.2.6 那一腿**像素**用的到底是哪一块矩形，以及那一枚键改的是哪一格 —— 这一格把三段
    /// 证据接在一起，因为它们单独哪一段都不算答案：
    ///
    /// 1. **位图来自 `PrintWindow`**（[`FrameSource::capture_window`]）。读 xcap 0.9.8 的
    ///    Windows 后端可以证明出厂那一侧落在哪：`get_window_bounds` 问的就是
    ///    `DWMWA_EXTENDED_FRAME_BOUNDS`（所以 [`WindowInfo::bounds`] 交回来就已经是可见边），
    ///    而 `gdi::capture_window` 在 `PrintWindow` 之后按 `(dwm - GetWindowRect)` 的偏移把
    ///    那一圈不可见收缩边裁掉了。**那一刀已经有人切过，这里不再切第二刀** —— 本仓「一件事
    ///    一个作者」在几何上的写法。
    /// 2. [`CaptureService::capture_window`] 再把位图对到 [`WindowInfo::visible_bounds`]：
    ///    一个也不肯照 `GetWindowRect` 交宽（后端换了、或 P5 量的那种尺寸漂移），所以阴影
    ///    **关**的那一侧，图上不可能有那一圈带宽度的像素。
    /// 3. 键开时变的不是那一刀，是**外面的一圈**（[`WINDOW_SHADOW_MARGIN`] 头上写的理由：
    ///    DWM 的投影画在窗口自己那张位图之外，PrintWindow 拿不到它，所以「含阴影」只能是
    ///    「按 §5.5.7 那一只手把那一圈渲染出来」）。上面那条 box 测试钉的是 3，这一条钉的是
    ///    1+2 与 3 接得上。
    #[test]
    fn the_window_shot_is_the_visible_rect_and_the_key_only_moves_the_ring() {
        /// 一台会交回像素的后端：位图按递进来的尺寸开，窗口列表按 `bounds`/`dwm_bounds` 说。
        struct ShotSource {
            info: WindowInfo,
            /// 位图尺寸：`Some` 用来假装后端交回了**更大**（或更小）的一张。
            bitmap: Option<(u32, u32)>,
        }

        impl FrameSource for ShotSource {
            fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
                Ok(Vec::new())
            }
            fn capture_monitor(&self, id: &str) -> Result<Frame, CaptureError> {
                Err(CaptureError::UnknownMonitor(id.into()))
            }
            fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
                Ok(vec![self.info.clone()])
            }
            fn capture_window(&self, _: u32) -> Result<Frame, CaptureError> {
                let (w, h) = self.bitmap.unwrap_or_else(|| {
                    (self.info.visible_bounds().w, self.info.visible_bounds().h)
                });
                // 整张都是「窗口自己的内容」，所以尺寸对没对上就是这一格的判据。
                Ok(Frame::filled(w, h, [10, 20, 30, 255])?)
            }
        }

        let vis = visible_rect();
        let real = real_rect();
        let info = WindowInfo {
            bounds: vis,
            dwm_bounds: Some(vis),
            ..win(7, 0, vis)
        };
        let svc = CaptureService::new(Box::new(ShotSource {
            info: info.clone(),
            bitmap: None,
        }));
        let cap = svc.capture_window(7).unwrap();
        // 2. 出厂那一侧：像素与矩形都是 DWM 可见边，不是 `GetWindowRect`。
        assert_eq!(cap.rect, vis, "报出去的矩形必须是可见边");
        assert_eq!(
            (cap.frame.width, cap.frame.height),
            (vis.w, vis.h),
            "阴影关着的那一侧不许拿到窗口框大小的像素"
        );
        // 后端交宽了（真的按 `GetWindowRect` 给，36x26）也一样被对回可见边。
        let wide = CaptureService::new(Box::new(ShotSource {
            info,
            bitmap: Some((real.w, real.h)),
        }))
        .capture_window(7)
        .unwrap();
        assert_eq!(
            (wide.frame.width, wide.frame.height),
            (vis.w, vis.h),
            "后端交回窗口框大小的位图时，多出来那一圈没被对掉"
        );

        // 3. 接到那一枚键上：关着的仍然是那 20x10，开着的长出的那一圈落在真实框那一格。
        let (off, off_rect) = window_picture(
            &cap.frame,
            &cap.rect,
            Some(&real),
            WindowShadow::Off,
            WindowCorner::Off,
            None,
        )
        .unwrap();
        assert_eq!((off.width, off.height), (vis.w, vis.h));
        assert_eq!(off_rect, vis);
        let (on, on_rect) = window_picture(
            &cap.frame,
            &cap.rect,
            Some(&real),
            WindowShadow::On {
                configured: 0,
                color: [0, 0, 0, 200],
            },
            WindowCorner::Off,
            None,
        )
        .unwrap();
        assert_eq!(
            (on.width, on.height),
            (real.w, real.h),
            "开着要给阴影留的那一圈"
        );
        assert_eq!(on_rect, real);
        // 键关着 = 图上没有那一圈；键开着 = 有，而且是阴影而不是留白（中间那一格实心内容）。
        assert_eq!(off.get(0, 0)[3], 255);
        assert_eq!(on.get(0, 0)[3], 0, "真实框的角上不该有窗口内容");
        assert_eq!(
            on.get(8, 8),
            [10, 20, 30, 255],
            "内容被整圈推走之后位置仍在"
        );
    }

    /// 阴影那圈有多宽：**量出来的带、配过的宽度、常数，三者取最宽**，且永不为 0。
    /// 「开着一定变大」就是被这一条下界钉住的。
    #[test]
    fn the_shadow_margin_is_the_widest_of_three_claims() {
        let vis = visible_rect();
        // 什么也量不到、用户也没配 ⇒ 常数。
        assert_eq!(
            window_shadow_margin(&vis, None, 0),
            WINDOW_SHADOW_MARGIN,
            "两样都缺时不能退成「没有阴影」"
        );
        // 窗口自己那条带（8 px）比常数宽时用带：这里让带更宽一点。
        let wide = PhysRect::new(88, 88, 44, 34); // 每边 12
        assert_eq!(window_shadow_margin(&vis, Some(&wide), 0), 12);
        // 用户写的 24 比带宽 ⇒ 用户赢。
        assert_eq!(window_shadow_margin(&vis, Some(&wide), 24), 24);
        // 带比用户写的宽 ⇒ 机器赢（§5.2.6 问的是"窗口阴影"，不是"配置里的阴影"）。
        assert_eq!(window_shadow_margin(&vis, Some(&wide), 4), 12);
        // 真实框反而比可见边窄（DWM 偶尔对某些窗口交回相反的包含关系）：读成 0 而不是负数，
        // 于是仍退到常数。
        let narrow = PhysRect::new(104, 104, 8, 4);
        assert_eq!(
            window_shadow_margin(&vis, Some(&narrow), 0),
            WINDOW_SHADOW_MARGIN
        );
        assert_eq!(window_shadow_margin(&vis, Some(&narrow), 20), 20);
    }

    /// 规则第一句的「圆角」+ §5.5.6 步骤 3 的两读法：配过的半径优先，其次系统那枚，
    /// 系统答不出就不切 —— `Some(0)`（这扇窗本来就是方角）是答案，不是缺答。
    #[test]
    fn the_corner_is_the_configured_radius_or_the_systems_or_none() {
        assert_eq!(
            resolved_corner_radius(12, Some(8)),
            12,
            "用户写的数不该被系统顶掉"
        );
        assert_eq!(
            resolved_corner_radius(0, Some(8)),
            8,
            "0 那一读法问的是系统"
        );
        assert_eq!(
            resolved_corner_radius(0, None),
            0,
            "系统答不出时不许猜一个半径"
        );
        assert_eq!(resolved_corner_radius(0, Some(0)), 0, "这扇窗本来就是方角");

        let src = window_frame();
        let want = visible_rect();
        // 系统圆角那一臂：四角变透明，中心仍是内容。
        let (cut, rect) = window_picture(
            &src,
            &want,
            None,
            WindowShadow::Off,
            WindowCorner::On {
                requested: 0,
                system: Some(4),
                outside: crate::imageops::Outside::Transparent,
            },
            None,
        )
        .unwrap();
        assert_eq!((cut.width, cut.height), (20, 10), "切圆角不改尺子");
        assert_eq!(rect, want);
        assert_eq!(cut.get(0, 0), [0, 0, 0, 0], "角没被切掉");
        assert_eq!(cut.get(10, 5), [10, 20, 30, 255]);
        // 「或按配置填充背景色」（§5.5.6 规则第一句的后半）。
        let (filled, _) = window_picture(
            &src,
            &want,
            None,
            WindowShadow::Off,
            WindowCorner::On {
                requested: 4,
                system: None,
                outside: crate::imageops::Outside::Fill([255, 255, 255, 255]),
            },
            None,
        )
        .unwrap();
        assert_eq!(filled.get(0, 0), [255, 255, 255, 255]);
        // 开关关着 / 系统也不答：一格都不切。
        for (corner, note) in [
            (WindowCorner::Off, "开关关着"),
            (
                WindowCorner::On {
                    requested: 0,
                    system: None,
                    outside: crate::imageops::Outside::Transparent,
                },
                "系统答不出",
            ),
        ] {
            let (out, _) = window_picture(&src, &want, None, WindowShadow::Off, corner, None)
                .unwrap_or_else(|e| panic!("{note}: {e}"));
            assert_eq!(out, src, "{note} 时这一路动了像素");
        }
    }

    /// §5.4.3 第 419 行「指针热点位置应与实际屏幕位置一致」在这条路上的那一半：
    /// 平台交回的是**指针图左上角**的屏幕坐标（热点已经减过），这里要的偏移是相对
    /// 这张截图的左上角，所以减的是截图的矩形。
    ///
    /// 顺带钉住顺序：指针先进画面，再长阴影圈 —— 阴影开着时内容（和指针）一起被推走，
    /// 而指针不会因此跑到圈外去。
    #[test]
    fn the_pointer_lands_where_the_screen_put_it() {
        let mut src = Frame::filled(10, 10, [0, 0, 0, 255]).unwrap();
        let want = PhysRect::new(50, 40, 10, 10);
        // 指针图 4x4，纯红；它的左上角在屏幕上 (52,41) ⇒ 图内 (2,1)。
        let pointer = Frame::filled(4, 4, [255, 0, 0, 255]).unwrap();
        let at = PhysPoint::new(52, 41);

        let (out, _) = window_picture(
            &src,
            &want,
            None,
            WindowShadow::Off,
            WindowCorner::Off,
            Some((&pointer, at)),
        )
        .unwrap();
        assert_eq!(
            out.get(2, 1),
            [255, 0, 0, 255],
            "指针没落在 at-矩形左上角那一格"
        );
        // 4x4 从 (2,1) 起，盖住的是 x=2..5、y=1..4 这 16 格：**右下界不进**（`blend_over`
        // 走的是矩形交集，端点开闭写死在 frame.rs，不在这里再猜一遍）。
        assert_eq!(
            out.get(5, 4),
            [255, 0, 0, 255],
            "4x4 的指针盖住了它的右下角那一格"
        );
        assert_eq!(out.get(5, 5), [0, 0, 0, 255], "端点外那一格被多盖了一格");
        assert_eq!(out.get(1, 0), [0, 0, 0, 255], "指针盖到了它不该在的地方");

        // 阴影同时开着：内容+指针都平移一个边距，位置关系不变。
        src.set(0, 0, [7, 7, 7, 255]);
        let (grown, rect) = window_picture(
            &src,
            &want,
            Some(&PhysRect::new(42, 32, 26, 26)), // 每边 8
            WindowShadow::On {
                configured: 0,
                color: [0, 0, 0, 200],
            },
            WindowCorner::Off,
            Some((&pointer, at)),
        )
        .unwrap();
        assert_eq!(rect, want.inflate(8));
        assert_eq!(
            grown.get(8 + 2, 8 + 1),
            [255, 0, 0, 255],
            "阴影圈把指针挤出画面了，顺序写反了"
        );
        // 指针有一部分在截图之外：由 blend_over 自己裁，不许 panic 也不许挪进画面。
        let (edge, _) = window_picture(
            &src,
            &want,
            None,
            WindowShadow::Off,
            WindowCorner::Off,
            Some((&pointer, PhysPoint::new(48, 38))),
        )
        .unwrap();
        assert_eq!(edge.get(0, 0), [255, 0, 0, 255], "图内的部分该留下");
        assert_eq!(edge.get(4, 4), [0, 0, 0, 255], "图外的那半不该被挪进来");
    }

    /// §5.5.6 规则第二句 + §5.4.5 规则第二句的**触发条件**在这一侧的形状：透明角切出来之后
    /// 这张图必须被 `has_transparency` 认出来，否则那一扇门（`qt_bridge::save::reminder`，
    /// 贴图那一路由 `state::remind_with` 读）就没有任何东西可提示，而 JPG 会把四角填白。
    #[test]
    fn a_rounded_window_shot_is_a_picture_a_jpg_has_to_fill() {
        let src = window_frame();
        let want = visible_rect();
        let clear = window_picture(
            &src,
            &want,
            None,
            WindowShadow::Off,
            WindowCorner::On {
                requested: 4,
                system: None,
                outside: crate::imageops::Outside::Transparent,
            },
            None,
        )
        .unwrap()
        .0;
        assert!(
            clear.has_transparency(),
            "透明角没做成透明的图，保存那一扇门就不会提示 JPG 会填背景"
        );
        // 填过背景的圆角、以及带阴影圈的图（阴影那一圈本身就是半透明）都在这一格里。
        let shadowed = window_picture(
            &src,
            &want,
            Some(&real_rect()),
            WindowShadow::On {
                configured: 0,
                color: [0, 0, 0, 200],
            },
            WindowCorner::Off,
            None,
        )
        .unwrap()
        .0;
        assert!(shadowed.has_transparency(), "阴影圈不该是不透明的");
        // 关着的时候这张图仍是纯不透明的，那一句提示也就不会莫名出现。
        let plain = window_picture(
            &src,
            &want,
            Some(&real_rect()),
            WindowShadow::Off,
            WindowCorner::Off,
            None,
        )
        .unwrap()
        .0;
        assert!(!plain.has_transparency(), "默认那一侧不该触发透明提示");
    }

    /// §5.2.6 规则第二句「最小化窗口不可作为活动窗口截图对象」的判据表，以及它的**次序**：
    /// 查不到主人的把手排在最小化之前（那句 doc 说的"可信度次序"）。
    #[test]
    fn a_minimised_window_is_refused_and_says_which_rule_it_broke() {
        let me = 4_100;
        assert_eq!(
            active_window_block(Some(4_200), me, true, false),
            Some(ActiveWindowBlock::Minimized),
            "最小化必须被拒（规则第二句）"
        );
        assert!(ActiveWindowBlock::Minimized.is_minimised());
        assert!(!ActiveWindowBlock::SelfForeground.is_minimised());
        // 次序：主人不明 ⇒ 后面那三格一概不作数。
        assert_eq!(
            active_window_block(None, me, true, true),
            Some(ActiveWindowBlock::UnknownOwner)
        );
        assert_eq!(
            active_window_block(Some(me), me, true, false),
            Some(ActiveWindowBlock::SelfForeground),
            "自己截自己排在最小化之前"
        );
        // 没最小化时，锁屏那一格仍拒；两格都相安才放行。
        assert_eq!(
            active_window_block(Some(4_200), me, false, true),
            Some(ActiveWindowBlock::LockedSession)
        );
        assert_eq!(active_window_block(Some(4_200), me, false, false), None);
    }

    // ------------------------------------------------------- PRD §5.3.2 窗口检测
    //
    // 两条规则各有一枚判据，两枚都在 `capture.rs` 的最上面（[`window_is_shootable`] 与
    // [`window_at_in`]），所以这一组的 fixture 递的是**候选表**而不是一个桌面：不需要开
    // 一扇真的窗，也不需要 `#[ignore]`。末尾那枚 `service_*` 把这两手接回
    // [`CaptureService`]，判的是「规则真的走在这条路上」而不是「规则写得对」。

    /// 一扇窗的默认形状：可见、100×80 在 (0,0)、不透明、别人的进程。
    /// 每一枚测试只改它要判的那一格，这样「选不出来」永远只有一个可能原因。
    fn win(hwnd: u32, z_order: usize, bounds: PhysRect) -> WindowInfo {
        WindowInfo {
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

    const FULL: PhysRect = PhysRect::new(0, 0, 200, 200);
    const COVER: PhysRect = PhysRect::new(20, 20, 100, 100);

    /// 规则第一句「应排除不可见窗口」那一半：最小化的、量不出矩形的、DWM 说可见内容为空的。
    #[test]
    fn rule_one_excludes_the_windows_with_nothing_on_screen() {
        let mut mini = win(1, 0, FULL);
        mini.minimized = true;
        let empty = win(2, 1, PhysRect::new(0, 0, 0, 0));
        // `bounds` 有尺寸而 DWM 那张是空的：窗框在，画出来的内容一点也没有。
        let mut blank = win(3, 2, FULL);
        blank.dwm_bounds = Some(PhysRect::new(0, 0, 0, 0));
        let ok = win(4, 3, FULL);

        assert!(!window_is_shootable(&mini), "最小化 = 屏幕上没有这块");
        assert!(!window_is_shootable(&empty), "矩形为空 = 说不出截哪一块");
        assert!(
            !window_is_shootable(&blank),
            "DWM 那张为空 = 没有可截的内容，两路都得查"
        );
        assert!(window_is_shootable(&ok), "什么都没改的一扇窗就该留下");
    }

    /// 规则第一句「完全透明窗口」那一半：`Some(0)` 排除，`Some(1..=255)` 与 `None` 放行。
    ///
    /// 反例钉的是两条容易写反的：**半透**（毛玻璃、贴图 60% 不透明度）屏幕上看得见，必须
    /// 能截；**平台没说**（`None`）不能当成「全透」误杀，也不能当成「不透」替平台编答案。
    #[test]
    fn rule_one_excludes_only_a_window_the_platform_says_is_fully_transparent() {
        let mut gone = win(1, 0, COVER);
        gone.opacity = Some(0);
        let mut smudge = win(2, 1, COVER);
        smudge.opacity = Some(150);
        let mut thin = win(3, 2, COVER);
        thin.opacity = Some(1);
        let mute = win(4, 3, COVER);
        let mut solid = win(5, 4, COVER);
        solid.opacity = Some(255);

        assert!(!window_is_shootable(&gone), "Some(0) 就是「完全透明」");
        assert!(window_is_shootable(&smudge), "半透的窗看得见，就该能截");
        assert!(window_is_shootable(&thin), "1/255 也是看得见，不是全透");
        assert!(window_is_shootable(&mute), "None = 平台没说，不是全透");
        assert!(window_is_shootable(&solid), "255 = 完全不透");
    }

    /// [`layered_window_opacity`] 的读数表 —— 也就是 `crates/platform-windows/src/win.rs`
    /// 里那一手在**没有真窗口**的这一侧必须遵守的映射。三格读数与四种窗口（普通、整体半透、
    /// 整体全透、只有 color key）各有其位，而**只有** `answered && alpha_flag && alpha == 0`
    /// 这一种会走到「排除」那一格。
    ///
    /// 为什么这一格值得单独钉，而不只是把上面那条 `rule_one_excludes_only_...` 的输入换个写法：
    /// `None` 与
    /// `Some(0)` 的区别就是「系统不肯说」与「系统说了全透」，而在 Win32 那一侧它们长得几乎一样
    /// （一次失败的调用 vs 一次成功的调用交出 0）。写反一侧的症状是整批正常窗口从 §5.3.2 的
    /// 候选表里消失，或一扇全透的窗仍然被当成可见者 —— 两种都只在真机上看得到，所以先在表上钉住。
    #[test]
    fn the_layered_read_answers_one_number_and_no_invention() {
        // 整体 alpha 交回来多少就是多少，0 到 255 之间不换算。
        assert_eq!(layered_window_opacity(true, true, 0), Some(0));
        assert_eq!(layered_window_opacity(true, true, 1), Some(1));
        assert_eq!(layered_window_opacity(true, true, 150), Some(150));
        assert_eq!(layered_window_opacity(true, true, 255), Some(255));
        // 那一问失败：普通窗（没有 WS_EX_LAYERED）与 `UpdateLayeredWindow` 逐像素那一类。
        // 不许读成「全透」。
        assert_eq!(layered_window_opacity(false, false, 0), None);
        assert_eq!(layered_window_opacity(false, true, 0), None);
        // 答出来了但只有 color key：整体 alpha 没被设过，交回的数是无意义的。
        assert_eq!(layered_window_opacity(true, false, 0), None);
        assert_eq!(layered_window_opacity(true, false, 255), None);
    }

    /// 上面那张表接到 §5.3.2 的排除上：一扇窗能不能截，取决于平台层**按那一手的形状**答出来的
    /// 那枚数。`LayeredExtras` 就是 `win.rs::window_opacity` 的形状（三格读数 →
    /// [`layered_window_opacity`]），所以这一格测的是「生产那一腿接上真 Win32 之后，规则第一句
    /// 到底生不生效」，而不只是「假把手递 Some(0) 会不会排除」。
    ///
    /// Win32 那一问本身（`GetLayeredWindowAttributes`）仍然只有真窗口能给答案，那一格在
    /// `crates/platform-windows/src/win.rs` 的 `#[ignore]` 证人里。
    #[test]
    fn rule_one_has_teeth_once_the_platform_answers_the_three_reads() {
        let opaque = win(20, 0, COVER);
        let mut glass = win(21, 1, COVER);
        glass.opacity = layered_window_opacity(true, true, 120);
        let mut ghost = win(22, 2, COVER);
        ghost.opacity = layered_window_opacity(true, true, 0);
        // 逐像素透的那一类：那一手问不出，于是仍然可截（已知欠账，不是这条规则要排除的那一格）。
        let mut keyed = win(23, 3, COVER);
        keyed.opacity = layered_window_opacity(true, false, 0);

        assert!(window_is_shootable(&opaque));
        assert!(window_is_shootable(&glass), "半透看得见");
        assert!(
            !window_is_shootable(&ghost),
            "整体 alpha = 0 就是「完全透明」"
        );
        assert!(window_is_shootable(&keyed), "问不出不等于全透");

        // 经过 `windows()` 那一趟 fill：ghost 从候选表里消失，光标那一点因此落到它**后面**那扇。
        let under = PhysPoint::new(60, 60);
        let svc = CaptureService::with_extras(
            Box::new(ListSource {
                windows: vec![opaque.clone(), glass, ghost.clone(), keyed],
            }),
            Box::new(LayeredExtras),
        );
        let kept = svc.windows(false).unwrap();
        assert!(
            !kept.iter().any(|w| w.hwnd == 22),
            "§5.3.2 规则第一句在生产那一腿上没把全透的窗排掉"
        );
        let behind = win(24, 9, PhysRect::new(10, 10, 200, 200));
        let svc = CaptureService::with_extras(
            Box::new(ListSource {
                windows: vec![behind.clone(), ghost],
            }),
            Box::new(LayeredExtras),
        );
        assert_eq!(
            svc.window_at(under).map(|w| w.hwnd),
            Some(24),
            "全透的封面没被看穿"
        );
    }

    /// 规则第二句「优先选择光标最直接指向的可见窗口」= z-order 最靠前的可见者。
    ///
    /// 两条反例把「不是另外两种读法」钉住：① 候选表的**数组顺序**不算数（这里更靠前的那扇
    /// 排在后面递进来，仍然赢）；② 「矩形更贴」也不算数（下面那扇小得多、把光标包住得更紧，
    /// 但它在 z-order 上被盖住，屏幕上看不见它）。
    #[test]
    fn rule_two_takes_the_topmost_visible_window_not_the_first_or_the_tightest() {
        let p = PhysPoint::new(60, 60);
        let bottom_big = win(10, 5, FULL);
        let bottom_tight = win(11, 4, PhysRect::new(50, 50, 20, 20));
        let top = win(12, 0, COVER);
        // 故意按 z-order **升序之外**的三种排法递进来，其中一种还把赢者放在最后。
        for table in [
            vec![top.clone(), bottom_big.clone(), bottom_tight.clone()],
            vec![bottom_big.clone(), bottom_tight.clone(), top.clone()],
            vec![bottom_tight.clone(), bottom_big.clone(), top.clone()],
        ] {
            assert_eq!(
                window_at_in(&table, p).map(|w| w.hwnd),
                Some(12),
                "z-order 最小（最靠前）的那一扇赢，与数组顺序无关"
            );
        }
    }

    /// 两条规则撞在一起时的次序：**先排除，再论先后**。
    ///
    /// 最上面那一扇是全透的（`Some(0)`），它挡不住后面那一扇 —— 屏幕上此刻看见的就是后面
    /// 那一个，所以「光标最直接指向的可见窗口」说的是它。写成另一样（先取最上面、再判它
    /// 可不可截、不可截就答没有）会让用户 hover 在一扇看得见的内容上却选不中。
    #[test]
    fn rule_two_looks_through_an_excluded_cover() {
        let p = PhysPoint::new(60, 60);
        let mut veil = win(20, 0, COVER);
        veil.opacity = Some(0);
        let mut hidden = win(21, 1, COVER);
        hidden.minimized = true;
        let shown = win(22, 2, FULL);
        let table = vec![veil, hidden, shown.clone()];

        assert_eq!(window_at_in(&table, p).map(|w| w.hwnd), Some(22));
    }

    /// 光标落在桌面上（没有一扇候选盖住它）时答 `None`，那是答案不是失败：界面那一腿
    /// 因此知道要退回自由选区，而不是选一张盖住半屏的窗给用户看。
    #[test]
    fn rule_two_answers_nothing_when_nothing_is_under_the_cursor() {
        // `FULL` 是 (0,0,200,200)，所以「桌面上那一格」要取在它外面 —— 取 (180,180) 这种
        // 数会测成叠放而不是空表：那一点真的被 `FULL` 那一扇盖着。
        let table = vec![win(30, 0, COVER), win(31, 1, FULL)];
        assert_eq!(window_at_in(&table, PhysPoint::new(250, 250)), None);
        assert_eq!(window_at_in(&[], PhysPoint::new(0, 0)), None);
        assert_eq!(
            window_at_in(&table, PhysPoint::new(0, 0)).map(|w| w.hwnd),
            Some(31)
        );

        // 边缘那一格单独摆一扇：旁边还站着一张全屏窗的话，落空的那一点会被它接住，
        // 测的就又变成叠放而不是边线了。`contains` 左闭右开，所以右/下边线不属于这扇窗、
        // 左/上边线属于。
        let one = [win(30, 0, COVER)];
        assert_eq!(
            window_at_in(&one, PhysPoint::new(119, 119)).map(|w| w.hwnd),
            Some(30)
        );
        assert_eq!(
            window_at_in(&one, PhysPoint::new(120, 120)),
            None,
            "右下角的边线在外面"
        );
        assert_eq!(
            window_at_in(&one, PhysPoint::new(20, 20)).map(|w| w.hwnd),
            Some(30),
            "左上角在里面"
        );
        assert_eq!(window_at_in(&one, PhysPoint::new(19, 19)), None);
    }

    /// 规则第二句用的是**可截图那一块**边界（[`WindowInfo::visible_bounds`]），不是
    /// `GetWindowRect` 那条带不可见收缩边的框：光标落在那条带里不算落在这扇窗上，否则
    /// 高亮框会比用户看见的内容大一圈，单击选中的也就是那一圈。
    #[test]
    fn rule_two_hits_the_dwm_rect_and_not_the_window_rect() {
        // 上面那一扇：`GetWindowRect` 有 200×200，DWM 说画出来的只有 (10,10,180,180)。
        let mut cover = win(40, 0, FULL);
        cover.dwm_bounds = Some(PhysRect::new(10, 10, 180, 180));
        // 下面那一扇把 (5,5) 这块真的盖住。
        let under = win(41, 1, PhysRect::new(0, 0, 60, 60));
        let table = vec![cover.clone(), under.clone()];

        assert_eq!(
            window_at_in(&table, PhysPoint::new(5, 5)).map(|w| w.hwnd),
            Some(41),
            "不可见收缩边里那一点不算落在 40 上，屏幕上此处真的是 41"
        );
        assert_eq!(
            window_at_in(&table, PhysPoint::new(20, 20)).map(|w| w.hwnd),
            Some(40),
            "DWM 那张矩形盖得住的一点，还是最上面那一扇赢"
        );
        // 后端没答 DWM（`None`）时退回 `bounds`，不是退回「什么都没有」。
        let plain = win(42, 0, FULL);
        assert_eq!(
            window_at_in(&[plain], PhysPoint::new(5, 5)).map(|w| w.hwnd),
            Some(42)
        );
    }

    /// 端到端：[`CaptureService::window_at`] 走的就是上面两枚判据，而
    /// [`CaptureService::windows`] 会把平台答出来的不透明度与 DWM 矩形**填进候选表**再筛。
    /// 这一枚测试判的是接线 —— 规则只写在纯函数里而这条路不调它，§5.3.2 就还是不成立。
    #[test]
    fn the_service_fills_the_platforms_answers_and_applies_both_rules() {
        let svc = CaptureService::with_extras(
            Box::new(ListSource {
                windows: vec![
                    // 最上面：一扇全透的遮罩（平台答 Some(0)），必须被排除。
                    win(50, 0, COVER),
                    // 第二层：一扇 DWM 说「可见内容为空」的窗，也必须被排除。
                    win(51, 1, FULL),
                    // 第三层：用户此刻真正看见的那一扇。
                    win(52, 2, FULL),
                ],
            }),
            Box::new(AnsweringExtras {
                opacity: vec![(50, Some(0)), (51, None), (52, None)],
                dwm: vec![(51, PhysRect::new(0, 0, 0, 0))],
            }),
        );
        let p = PhysPoint::new(60, 60);

        let list = svc.windows(false).unwrap();
        assert_eq!(
            list.iter().map(|w| w.hwnd).collect::<Vec<_>>(),
            vec![52],
            "排除在 fill 之后，所以平台才答得出的那一格也在候选表里生效"
        );
        // 全表只剩一扇 ⇒ 这一格同时证明了 `window_at` 与 `windows` 用的是同一份筛过的表。
        assert_eq!(svc.window_at(p).map(|w| w.hwnd), Some(52));
        // 52 是 (0,0,200,200) 那一扇，所以「桌面上」那一格要取在 200 之外。
        assert_eq!(svc.window_at(PhysPoint::new(250, 250)), None);
    }

    /// `own_process` 那一问**不是** §5.3.2 的排除项，是调用方的策略（`include_own_process`
    /// 那一格），所以它不能被并进 [`window_is_shootable`]：并进去了，`--list-windows` 这类
    /// 「就是要看见自己那些窗」的读者就再也问不到答案了。
    #[test]
    fn own_process_stays_a_callers_choice_not_a_rule() {
        let mut mine = win(60, 0, COVER);
        mine.own_process = true;
        let table = vec![mine.clone()];
        assert!(window_is_shootable(&mine), "自己这一扇窗在屏幕上是看得见的");
        assert_eq!(
            window_at_in(&table, PhysPoint::new(50, 50)).map(|w| w.hwnd),
            Some(60)
        );

        let svc = CaptureService::new(Box::new(ListSource {
            windows: vec![mine, win(61, 1, FULL)],
        }));
        assert_eq!(svc.windows(true).unwrap().len(), 2, "点名要自己的窗就给");
        assert_eq!(
            svc.windows(false)
                .unwrap()
                .iter()
                .map(|w| w.hwnd)
                .collect::<Vec<_>>(),
            vec![61],
            "没点名要就不给（遮罩那一路用的就是这个）"
        );
    }

    /// 一扇只回列表、不回像素的后端。§5.3.2 的判据全在几何与标志位上，用不着真的抓屏。
    struct ListSource {
        windows: Vec<WindowInfo>,
    }

    impl FrameSource for ListSource {
        fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
            Ok(Vec::new())
        }
        fn capture_monitor(&self, id: &str) -> Result<Frame, CaptureError> {
            Err(CaptureError::UnknownMonitor(id.into()))
        }
        fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
            Ok(self.windows.clone())
        }
        fn capture_window(&self, hwnd: u32) -> Result<Frame, CaptureError> {
            Err(CaptureError::Backend(format!("no such window {hwnd}")))
        }
    }

    /// 一台会答「这扇窗全透 / 这扇窗 DWM 框为空」的平台层，用来走 `windows()` 里那个 fill。
    struct AnsweringExtras {
        opacity: Vec<(u32, Option<u8>)>,
        dwm: Vec<(u32, PhysRect)>,
    }

    impl WindowExtras for AnsweringExtras {
        fn dwm_bounds(&self, hwnd: u32) -> Option<PhysRect> {
            self.dwm.iter().find(|(h, _)| *h == hwnd).map(|(_, r)| *r)
        }
        fn element_chain(&self, _: PhysPoint) -> Vec<PhysRect> {
            Vec::new()
        }
        fn cursor_image(&self, _: PhysPoint) -> Option<(Frame, PhysPoint)> {
            None
        }
        fn process_path(&self, _: u32) -> Option<(String, String)> {
            None
        }
        fn window_rect(&self, _: u32) -> Option<PhysRect> {
            None
        }
        fn window_corner_radius(&self, _: u32) -> Option<u32> {
            None
        }
        fn window_opacity(&self, hwnd: u32) -> Option<u8> {
            self.opacity
                .iter()
                .find(|(h, _)| *h == hwnd)
                .and_then(|(_, a)| *a)
        }
    }

    /// `win.rs::window_opacity` 的形状：手里只有 `GetLayeredWindowAttributes` 的**三格读数**，
    /// 交回的必须过 [`layered_window_opacity`] 这一道才算 [`WindowInfo::opacity`]。生产那一腿
    /// 就是这个形状，所以这一枚假把手测的是那条腿接上真 Win32 之后的**判定**，
    /// 而那一问本身仍然只有真窗口能答（`win.rs` 的 `#[ignore]` 证人）。
    struct LayeredExtras;

    /// 四种窗在该手那一问下的真实读数形状：普通窗答不出、整体半透、整体全透、只有 color key。
    const LAYERED_READS: &[(u32, (bool, bool, u8))] = &[
        (21, (true, true, 120)),
        (22, (true, true, 0)),
        (23, (true, false, 0)),
    ];

    impl WindowExtras for LayeredExtras {
        fn dwm_bounds(&self, _: u32) -> Option<PhysRect> {
            None
        }
        fn element_chain(&self, _: PhysPoint) -> Vec<PhysRect> {
            Vec::new()
        }
        fn cursor_image(&self, _: PhysPoint) -> Option<(Frame, PhysPoint)> {
            None
        }
        fn process_path(&self, _: u32) -> Option<(String, String)> {
            None
        }
        fn window_rect(&self, _: u32) -> Option<PhysRect> {
            None
        }
        fn window_corner_radius(&self, _: u32) -> Option<u32> {
            None
        }
        fn window_opacity(&self, hwnd: u32) -> Option<u8> {
            // 表里没有的把手 = 那一问失败（普通窗就是这一格），读数一样走同一道判定。
            let (answered, alpha_flag, alpha) = LAYERED_READS
                .iter()
                .find(|(h, _)| *h == hwnd)
                .map(|(_, r)| *r)
                .unwrap_or((false, false, 0));
            layered_window_opacity(answered, alpha_flag, alpha)
        }
    }

    /// Real hardware, so the assumptions above get checked against a desktop
    /// instead of a fixture. Run with `cargo test -p falcon-core -- --ignored`.
    #[test]
    #[ignore]
    fn freezes_the_actual_desktop() {
        use crate::capture::{CaptureService, XcapCapture};
        use std::time::Instant;

        let svc = CaptureService::new(Box::new(XcapCapture));
        let ms = svc.monitors().unwrap();
        println!("backend = {}", svc.source.name());
        for m in &ms {
            println!(
                "monitor {} {:?} bounds={} scale={:?} primary={} builtin={}",
                m.id, m.name, m.bounds, m.scale, m.primary, m.builtin
            );
        }
        assert!(!ms.is_empty(), "no monitors enumerated");

        let t = Instant::now();
        let snap = svc.freeze().unwrap();
        let freeze_ms = t.elapsed().as_millis();
        println!(
            "freeze: {} ms, virtual {:?}, {} frame(s), backend {}",
            freeze_ms,
            snap.virtual_bounds,
            snap.monitors.len(),
            snap.backend
        );
        for fm in &snap.monitors {
            assert_eq!(fm.frame.width, fm.info.bounds.w, "{} width", fm.info.id);
            assert_eq!(fm.frame.height, fm.info.bounds.h, "{} height", fm.info.id);
        }

        let t = Instant::now();
        let want = PhysRect::new(
            snap.virtual_bounds.x,
            snap.virtual_bounds.y,
            512.min(snap.virtual_bounds.w),
            512.min(snap.virtual_bounds.h),
        );
        let c = snap.capture(&want).unwrap();
        println!("crop {want:?}: {} ms", t.elapsed().as_millis());
        assert_eq!(c.frame.width, want.w);
        assert_eq!(c.frame.height, want.h);

        let t = Instant::now();
        let ws = svc.windows(true).unwrap();
        let enum_ms = t.elapsed().as_millis();
        println!("windows: {} in {} ms", ws.len(), enum_ms);
        for w in ws.iter().take(12) {
            println!(
                "  z={:<2} hwnd={:#08x} focus={} bounds={:?} vis={:?} {:?}",
                w.z_order,
                w.hwnd,
                w.focused,
                w.bounds,
                w.visible_bounds(),
                w.app_name
            );
        }
        let focused = ws.iter().position(|w| w.focused);
        println!("focused window sits at enumeration index {focused:?}");

        let probe = PhysPoint::new(snap.virtual_bounds.x + 40, snap.virtual_bounds.y + 40);
        println!(
            "window_at({probe:?}) = {:?}",
            svc.window_at(probe).map(|w| (w.hwnd, w.app_name, w.bounds))
        );

        let png =
            crate::encode::encode(&c.frame, &crate::encode::EncodeOptions::default()).unwrap();
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.qoder/scratch/m1-freeze.png");
        crate::encode::save(&c.frame, &out, &crate::encode::EncodeOptions::default()).unwrap();
        println!(
            "wrote {} ({} bytes of PNG, has_transparency={})",
            out.display(),
            png.len(),
            c.frame.has_transparency()
        );
    }
}
