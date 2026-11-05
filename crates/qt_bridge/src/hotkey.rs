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
//! only the ones with an arm in [`Work::action`] get registered: a combination the
//! shell holds for us while nothing answers it does not merely sit
//! idle, it takes the key away from whatever the user expected it to do.
//! `capture_custom` used to be the clearest case - the coordinates dialog it names
//! (§5.3.5) did not exist, so registering `Ctrl+Shift+F` left that chord with no
//! answer at all; P166 built that dialog and the row moved to the registered side.
//! `switch_group` walked out of that column in P186 with the 出厂键 `Ctrl+Shift+G` it names.
//! 最后离开的是这两枚：`pin_edit` —— 名册给它的那件事（让**选中的贴图**进入标注编辑）今天走的是
//! 历史那一族早就落好的那副次序，把洞交回贴图占的那一块而**不铺像素**，理由写在
//! [`Work::PinEdit`] 那一臂头顶；`custom_task` —— 它的那件事拆成两半，判归 [`falcon_core::tasks`]、
//! 跑归 [`platform_windows::open::dispatch`]，两处都是已有的那一位，本层一个字也没新写。
//! §5.16.1 那九行出厂绑定自此**全**有对面。Every unregistered row still gets its own sentence in
//! the report, so the census is the nine the plan counts rather than the ones that
//! happen to work.
//!
//! 名单上还有**第十三级**那一枚：`release_click_through`（§5.9.13 第三步那一条「取消穿透」）
//! 在 [`falcon_core::config::ACTIONS`] 里、在这里有一臂、在托盘菜单上有一项，却不在这九行
//! *默认绑定*里 —— 出厂不替它占一枚键，与 `show_all_pins` / `hide_all_pins` / `custom_task`
//! 同一形状（两枚理由都写在那份默认表旁边）。所以「这一枚动作有没有一只手」在这一档里问的
//! 是 [`Work::action`] 与 [`Work::of`]，而不是「这台机器上有没有一把键」；§5.9.13 规则第一句
//! 那一条「必须提供明确且可靠的退出穿透方式」由 [`click_through_exits`] 判，两半各一次。

use std::pin::Pin;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use cxx_qt::CxxQtThread;
use cxx_qt_lib::QString;
use falcon_core::frame::Frame;
use falcon_core::hotkeys;
use platform_windows::hotkey::{bind, Event, Pump, Request, Started, TrayClick, TraySpec};

use crate::capture;
use crate::delay;
use crate::hot_corner::{self, Runner};
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
///
/// `pub(crate)` 而不是模块私有：§5.14.2 step 4 那两枚要**开一扇遮罩**的钮（标注、重进选区）
/// 落在 [`crate::session`] 上，它们走的必须是与热键同一扇门 —— 而一扇门上「用不用 shader」
/// 只能有一位作者，第二处写 `true` 就是有一天 `--mask-soft` 那族改分时漏掉的一条腿。
pub(crate) const SHADER: bool = true;

/// The Qt thread, as the type another thread is allowed to hold on to. `Session`
/// opted into `Threading` (`session.rs`), which is what makes this name writable
/// from outside the bridge module.
type Gui = CxxQtThread<session::qobject::Session>;

/// A hot key or menu command, and the work it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// §5.2.1: freeze and cover the screens.
    Capture,
    /// §5.3.5：PRD 第 298 行那一扇「自定义截图」对话框。P166 之前这一枚动作名（默认
    /// `ctrl+shift+f`）是**未接线**那一族里最清楚的一例 —— 它名的那扇窗不存在，注册了就是把
    /// 那串组合从用户手里拿走而没人应答。窗现在住在 [`crate::custom`] 与
    /// `qml/CustomCaptureWindow.qml`，这一臂写的仍是「该不该有一扇窗」那一格，造与毁归
    /// `main.qml`（与 [`Work::Settings`] 同一分工）。
    CustomCapture,
    /// §5.2.6: the window the shell is giving keystrokes to, as its own picture.
    ActiveWindow,
    /// §5.2.5: the region the last capture used, on a frame taken now.
    Repeat,
    /// §5.8.2: turn the clipboard into a pin.
    PastePin,
    /// §5.8.3 第 1 步那一枚「打开图片并贴图」：挑文件 → 读文件 → 建贴图。
    ///
    /// 与 [`Work::Whiteboard`]、[`Work::Settings`]、[`Work::History`]、[`Work::Groups`]、
    /// [`Work::Update`]、[`Work::Help`]、[`Work::Quit`] 同族 —— **只从托盘抵达**，也不给它编一枚
    /// 可绑定动作名：§5.16.1 那张表里最短的一条是「一次按下 → 一扇窗」，而这一枚按下去要的先是
    /// **一双手挑文件**，给它发一枚键就是把 §5.16.1 那张表添了第十五名而没人许过。
    ///
    /// **冲突记在这里，不当作已解决**：§5.18.1 那一列十行菜单建议里没有这一行（它有的是
    /// 「贴图」，而那一行早就落在 [`Work::PastePin`] 上、屏幕上写的是「贴出剪贴板」）。两处不一致
    /// 时按**功能那一节**办 —— §5.8.3 第 1 步点名的就是「托盘菜单」那一枚入口，那张表是建议名单；
    /// 与 [`Work::Whiteboard`]、[`Work::ReleaseClickThrough`] 同一枚理由、同一笔结转。
    ///
    /// 归宿是 [`crate::state::pin_pictures`]：对话框本体在 [`platform_windows::pick`]（只做
    /// 「取路径」这一手），收哪几种格式、一次收几枚、坏的那几枚怎么归账、屏幕上一句话怎么说
    /// 全在 [`falcon_core::pin::read_picture_batch`] 那一批里 —— 那一批有单测，这一枚入口没有，
    /// 而它存在的意义正是让那一批在生产路径上被叫到（本仓反复出现的那一族缺陷：逻辑齐备、
    /// 生产路径零调用）。
    ///
    /// §5.8.3 第 1 步的另一半「或将文件拖到应用图标」**不在这一臂上**，也没有别处接住它：
    /// 那一手要的是 shell 那一头的文件关联/`open` 动词，不是这一张路由表上的一枚工作。缺的那
    /// 样东西记在 [`crate::cli`] 头顶那一段与本轮的报告里，不冒充成已完成。
    OpenPicturePins,
    /// §5.11: put every pin the current list does not keep out of the way - and take
    /// them back on the same key. The list is the whole input, which is why this work
    /// reaches [`session::qobject::Session::toggle_solo_selection`] rather than an id:
    /// a keystroke has no 被点中的那一枚窗.
    Solo,
    /// §5.15「隐藏 / 显示全部贴图」，也是快捷动作表里那枚 `toggle_pins`（默认 `ctrl+shift+p`）
    /// 一直悬空的那条路由：P137 之前它列在 [`crate::config`] 的 `ACTIONS` 里、能配键，却没有任何
    /// 一只 `Work` 接得住 ⇒ 按下去走「未接线」。这一臂落成的就是 [`session::qobject::Session::toggle_all_pins`]：
    /// 一次按下翻一次全局收起，同键再按翻回来 —— 与 §5.11 独显同一形状（一枚键两态），无被点中的窗。
    TogglePins,
    /// §5.15「显示全部贴图」—— 热区那五枚动作里的第二枚。`toggle_pins` 是「翻」，而角落要的
    /// 是「置」：一次扫过角落不能像一次按键那样被读成「再翻回来」，因为角落没有按下与抬起，
    /// 只有停留。归宿是 [`session::qobject::Session::show_all_pins`]，恒交回 0（已放开）。
    ShowAllPins,
    /// §5.15「隐藏全部贴图」，[`session::qobject::Session::hide_all_pins`] 的归宿，恒交回
    /// 1（已收起）。与上一枚成对：五枚 `HOT_CORNER_ACTIONS` 自此每一枚都有一只手接得住，
    /// 而「配得上却什么都不做」那一族在热区这一头被关闭（`hot_corner::check` 里逐名判）。
    HideAllPins,
    /// §5.9.13 第三步那一枚「取消穿透」，也是这一节规则第一句「必须提供明确且可靠的退出穿透
    /// 方式」在这一张路由表上唯一的对面。
    ///
    /// **它只解开始终不开启**，这一条是照着文档那三个字（「取消穿透」）而不是照着 `TogglePins`
    /// 那枚「一枚键两态」的形状写的：一枚能把贴图按成点不到的键，不算是可靠的退出 —— 按下去
    /// 该收回什么，由这一枚键**自己**说得定，而不是由场上此刻的态决定。开启仍是 §5.9.13 第一步
    /// 「用户对贴图启用」那一路，走贴图自己的菜单（`qml/PinWindow.qml` 那一枚 MenuItem）。
    ///
    /// 归宿是 [`crate::state`] 里那一批 `PinItem` 的 `click_through` 那一格（core
    /// [`falcon_core::pin::PinItem::set_click_through`] —— 它头顶那句注释早就在等这一只手：
    /// 「the tray and the hotkey both read this back to turn it off again」），改完发一枚
    /// [`session::qobject::Session::pins_epoch`] 让**每一扇**窗重读自己那一格：被穿透的那些枚
    /// 恰恰是这一手的目标，它们收不到鼠标，所以不会自己醒过来。
    ///
    /// 它同时是这一族里第一枚**可绑定又不带默认键**的动作：名单上、名册里、能配、出厂不占键，
    /// 与 `show_all_pins` / `hide_all_pins` / `custom_task` 同一形状（理由与代价都写在
    /// [`falcon_core::config`] 那一份默认表旁边）。
    ReleaseClickThrough,
    /// §5.12.4 第 1251 行那三个入口里的**第三个**「快捷键」：`switch_group` 自 P186 起有手了。
    ///
    /// 前面两枚（托盘菜单、分组面板）自 P172 起有对面 —— [`crate::group_window`] 与
    /// `qml/GroupWindow.qml`；第三枚一直是「`config::ACTIONS` 名单上、设置页里成行、按下去
    /// 什么都不做」那一族里最后一枚还活着的（`capture_custom` 靠 P166 那扇窗出了这一列，
    /// `solo`/`toggle_pins` 靠 P135/P137 出了）。台账 ⑫ 记的就是这一枚。
    ///
    /// 做的确实是那一枚动作**本来**的样子，而不是「开那一扇管理窗」：[`falcon_core::config`]
    /// 名册里 `switch_group` 的说明写的就是「在贴图分组之间轮流切换显示哪一组」—— 叫
    /// [`Work::Groups`] 那一手就把它读成了第二枚入口，而 §5.12.4 第 1252 行「执行显示、隐藏或
    /// 切换」与第 1253 行「系统统一更新该分组内贴图的可见状态」两句里都没有「窗」。轮流的那一判
    /// 住在 [`group_cycle`]（纯函数、有单测），动手的那一只手是 [`crate::session::qobject::
    /// Session::toggle_group_shown`] —— 分组窗上「切换显隐」那枚钮叫的就是它，本层不拧模型、
    /// 不数第二遍成员（#281）。
    ///
    /// 默认绑定 `ctrl+shift+g` 在 [`falcon_core::config`] 那份出厂表里，所以出厂这一枚键**就**
    /// 有对面；注册与否仍由 [`plan`] 那一判答（reserved / 解析不了 / 被前台静音都各有自己那一行）。
    SwitchGroup,
    /// §5.16.1「可绑定范围」第 1410 行那一句「贴图编辑操作」，名册给它的是「让**选中的贴图**进入
    /// 标注编辑」（[`falcon_core::config::ACTION_TEXTS`] 里 `pin_edit` 那一枚，默认 `ctrl+shift+e`）。
    ///
    /// 做的是那一句话**字面**上的事，走的是这一 build 已有的那一条标注路，而**不铺任何像素**：
    /// 冻一帧 → 开遮罩 → 把洞交回贴图占的那一块桌面。两条理由，两条都是这一格的全部形状：
    /// ① 贴图本身就是一扇窗，冻下来的那一帧里已经有它此刻画在屏幕上的样子 —— 缩放、旋转、翻转、
    /// 灰度、反色、裁剪、缩略图那一身全都在那一帧里，替它再铺一遍就是给同一张图造第二个作者，而
    /// 那一位作者永远画不出「用户此刻看见的那一枚」；② `historyAnnotate` 之所以要 `stamp_row`，
    /// 是因为历史那一行**不在场上**，这一枚要的恰恰相反 —— 用户点中的这一枚正在场上。
    ///
    /// 洞由 [`falcon_core::pin::PinSet::selected_bounds`] 那一判给（选中几枚就并成一块）。一枚也
    /// 没选中时**直接拒绝**，与 [`Work::Solo`] 同一形状：把整屏那一框当成「选中的贴图」交出去，
    /// 说的是「编辑屏幕上的一切」，那不是这一枚键的意思。
    ///
    /// 它是 `only_wired_actions_are_asked_for` 那一格里最后还活着的一枚「配得上、按下去什么都不
    /// 做」——`capture_custom` 靠 P166 那扇窗出的列、`switch_group` 靠 P186 那一手出的列，这一枚
    /// 靠的是把历史那一族的腿（[`crate::mask::MaskState::annotate_history_row`] 同一副次序）搬到
    /// 贴图这一头来，一条也没新造。
    PinEdit,
    /// §5.16.1「可绑定范围」第 1411 行那一句「自定义命令行任务」，名册给它的是「跑一条用户自己
    /// 写好的命令行任务」。那一串字住在档上 `[hotkey] command`（现问的读者是
    /// [`crate::settings::hotkey_cfg`]），出厂**空着** —— 替用户写一条示例就是让一枚出厂动作
    /// 对着别人机器上的程序。
    ///
    /// 分工只有一刀：**判**归 [`falcon_core::tasks`]（怎么把一串字拆成「要跑的那一枚」与「递给它的
    /// 那一段」、拆不出来要怎么说法，§7.5「命令行参数应避免命令注入和路径注入」判的就是它，它有
    /// 单测），**跑**归 [`platform_windows::open::dispatch`] —— 这一档里唯一的那位进程发起者
    /// （§5.5.10「发送到应用」走的也是它，程序名与参数块是分开的两枚宽字串，中间没有 `cmd`）。
    /// 在这里另拼一条 `cmd /C` 就是当着 §7.5 的面把那句话说反。
    ///
    /// 它**不**上托盘菜单：§5.18.1 那一列十行没有这一行，而 [`tray_menu`] 头顶那条「只接有归宿的」
    /// 反过来读也一样成立 —— 有归宿不等于该长出一枚菜单项。要跑它，先给它配一枚键。
    CustomTask,
    /// §5.4.4 / §5.4.5 的那一块板子：同一套遮罩、同一套标注，背后的像素是做的而不是抓的。
    /// 与 [`Work::Settings`]、[`Work::Quit`] 同族 —— **只从托盘抵达**：`config::ACTIONS` 里
    /// 没有一枚叫 whiteboard 的动作，给一扇窗编一枚可绑定动作是替 §5.16.1 那张表发明第十三名。
    /// §5.4.4 第一步写的是"用户从托盘菜单或截图工具栏选择白板"，两处入口里这一层接得住前者，
    /// 后者（截图进行中的工具栏）是 [`Work::Capture`] 那一路里的一枚钮，不在这一张路由表上。
    Whiteboard,
    /// §5.20.1 那一页的入口，托盘里那一枚「首选项」。与 [`Work::Quit`] 同族：只从托盘抵达，
    /// 因为 `config::ACTIONS` 里压根没有一枚叫 settings 的动作 —— 给一扇窗编一枚可绑定动作，
    /// 是替 §5.16.1 那一张表发明第十二名。归宿是 [`session::qobject::Session::settings_open`]
    /// 那一格：Rust 只写「该不该有一扇窗」，造与毁都在 `main.qml`。
    Settings,
    /// §5.18.1 第 1438 行那枚「历史记录」。与 [`Work::Settings`]、[`Work::Whiteboard`]、
    /// [`Work::Quit`] 同族：**只从托盘抵达**、也不给它编一枚可绑定动作名 —— §5.16.1 那张表里
    /// 没有「打开历史记录」这一行，而给一扇窗发一个第十四名就是替那张表决定它自己没决定的事。
    /// 归容是 [`session::qobject::Session::history_window_open`] 那一格（§5.14.4 第 1309 行「用户打开
    /// “历史记录管理”」的那扇窗，落点是 [`crate::history_window`]）。
    History,
    /// §5.18.1 第 1437 行那枚「图片分组」。与上面三臂同族：**只从托盘抵达**，也不给它编一枚
    /// 可绑定动作名 —— §5.16.1 那张表里没有「打开分组管理」这一行（那张表有的是 `switch_group`
    /// 「切换分组」，那是 §5.12.4 第 1251 行那三个入口里的**第三个**「快捷键」，自 P186 起有手，
    /// 见 [`Work::SwitchGroup`]），把两者混成一枚就是替那张表决定它自己没决定的事。归宿是
    /// [`session::qobject::Session::group_window_open`] 那一格（窗本体在 [`crate::group_window`]
    /// 与 `qml/GroupWindow.qml`）。
    Groups,
    /// §5.18.1 第 1441 行那枚「检查更新」，也是 §5.21.1 那四步的第一到第三步。与上面四臂同族：
    /// **只从托盘抵达**，不给它编一枚可绑定动作名 —— §5.16.1 那张表里没有「检查更新」这一行，
    /// 而给一次查询发一个可绑键，是替那张表决定它自己没决定的事（与 `settings` / `history` /
    /// `groups` 同一枚理由）。归宿是 [`falcon_core::update::check`] 那一枚纯函数：读的是档上那
    /// 两枚 `[about]` 键（`release_feed` 与 `channel`）与包自己的版本号，交回产品自己那句判决
    /// （[`falcon_core::update::Answer::sentence`]）—— 它写进 `status` 那一格，而屏幕上那扇
    /// [`crate::hint_check`] 量过的浮窗绑的就是 `status`。第 4 步「显示」的另一半（一列可点的
    /// 下载入口）不在这一臂里，记作本轮结转。
    Update,
    /// §5.18.1 第 1440 行那枚「帮助」，也是 §5.21.2 那四行的唯一入口。与上面五臂同族：
    /// **只从托盘抵达**，也不给它编一枚可绑定动作名 —— §5.16.1 那张表里没有「打开帮助」这一行。
    /// 归宿是 [`crate::help`]：四行各自的作者现取来的四枚数拼成一句话写进 `status`，它不把那四行重新
    /// 发明一遍（#281）。「真人点中它」仍没有证人，记在 [`crate::help_menu`] 的 Blocked 那一格里。
    Help,
    /// §5.1.1's tray menu entry. Tray-only: no key binding means "quit", because a
    /// binding that could end the resident process from the keyboard is not
    /// something to hand out before a settings page has asked about it.
    Quit,
}

/// 一枚动作名的中文名，读的是那张唯一的表 [`falcon_core::config::ACTION_TEXTS`]。
///
/// 表里没有这一枚 id 时交回 **id 本身**而不是空串：空串在托盘菜单上是一枚看不见的名，而原样
/// 印出 `capture_thing` 是任何人一眼就能看出「这张表漏了一行」的样子。这一格不是兜底 ——
/// `[`Work::label`]` 与设置页那一行都从它读名，两处要的必须是同一个名（#281）。
///
/// `pub(crate)` 自 P166 起多了一位读者：[`crate::custom::title`] 要的是同一枚「自定义截图」——
/// 对话框自己写一个标题，就成了托盘那一项、设置页那一行与窗标题三种写法（#281）。
pub(crate) fn name_of(id: &'static str) -> &'static str {
    falcon_core::config::action_texts(id).map_or(id, |(name, _)| name)
}

impl Work {
    /// The config action this work answers, or `None` for the tray-only one. This
    /// match arm is the whole wiring table: an action with no arm here is reported
    /// as 未接线 and its key is left to the system.
    ///
    /// `pub(crate)` 与 [`Work::of`] 同一枚理由：§5.4.4 那一扇门要反过来问一次「这一枚工作
    /// 有没有名字可绑」，而它问的必须是这一张表本身 —— 另抄一份"哪些工作是托盘专属"的名单
    /// 就是 #281，两份会在下一枚 `Work` 进来那天分家。
    pub(crate) fn action(self) -> Option<&'static str> {
        match self {
            Work::Capture => Some("capture"),
            Work::CustomCapture => Some("capture_custom"),
            Work::ActiveWindow => Some("capture_active_window"),
            Work::Repeat => Some("capture_repeat"),
            Work::PastePin => Some("paste_pin"),
            Work::Solo => Some("solo"),
            Work::TogglePins => Some("toggle_pins"),
            Work::ShowAllPins => Some("show_all_pins"),
            Work::HideAllPins => Some("hide_all_pins"),
            Work::ReleaseClickThrough => Some("release_click_through"),
            Work::SwitchGroup => Some("switch_group"),
            Work::PinEdit => Some("pin_edit"),
            Work::CustomTask => Some("custom_task"),
            Work::OpenPicturePins => None,
            Work::Settings => None,
            Work::Whiteboard => None,
            Work::History => None,
            Work::Groups => None,
            Work::Update => None,
            Work::Help => None,
            Work::Quit => None,
        }
    }

    /// What the tray menu calls it, and what the history calls the work.
    ///
    /// 有可绑定动作名的那九臂**不在这里写名字**：那一名一实的东西住在
    /// [`falcon_core::config::ACTION_TEXTS`] 里，设置页那一行的说明、托盘菜单上那一项、历史里的
    /// 那一格读的都是同一枚门面 [`falcon_core::config::action_texts`]（#281）。在这里再抄一句
    /// 中文，抄的那一份不会错，它只会先旧 —— 与 `settings_view.rs` 头部记过的 `RESTART_KEYS`
    /// 那一枚是同一族。
    ///
    /// 剩下这八臂没有可绑定动作名（[`Work::action`] 交回 `None`），§5.16.1 那张表里也压根没有
    /// 对应的行，所以它们的名字只能住在这里，一处：
    fn label(self) -> &'static str {
        match self.action() {
            Some(id) => name_of(id),
            None => match self {
                // §5.8.3 第 1 步写的就是这七个字（「用户从托盘菜单选择“打开图片并贴图”」），所以
                // 这里用的必须是那七个字而不是把它顺成「打开图片」「贴图文件」—— #455：措辞在这一
                // 档里是读数。与 [`Work::Whiteboard`] 同一处结转：§5.18.1 那一列十行没有这一行。
                Work::OpenPicturePins => "打开图片并贴图",
                // §5.4.4 第一步的那两个字。文档在 §5.18.1 那张菜单建议表里没有列「白板」，而
                // §5.4.4 要的正是这一枚托盘入口 —— 两处不一致时按**功能那一节**办：那张表是建议
                // 名单，这一节是一条要有对面的操作。冲突记进台账，不当作已解决。
                Work::Whiteboard => "白板",
                // PRD 第 1439 行那一列菜单建议里的写法就是「首选项」，而 §5.15 步骤 1 写的也是
                // 「用户打开"首选项 > 热区"」。这一枚名字不叫「设置」：文档自己用的那一列字，
                // 在这一档里就是判据的一部分（#455 的反面 —— 措辞在这里是读数，不是读数之外的修饰）。
                // 但那三个字不住在这一行，它住在 core 那张表里 `settings` 那一枚：页上那一行与
                // 托盘里那一项问的是同一枚名，这里递出去的只是**词**。
                Work::Settings => name_of("settings"),
                // §5.18.1 第 1438 行那一列写的就是这四个字，而唯一作者住在
                // [`crate::history_window::TRAY_WORD`] 那里：托盘这一项与那一档的 ⑨ 那一格问的是
                // 同一枚名，这里再抄一遍中文就是两个作者（#281）。
                Work::History => crate::history_window::TRAY_WORD,
                // §5.18.1 第 1437 行那一列写的就是「图片分组」四个字，唯一作者住在
                // [`crate::group_window::TRAY_WORD`]：托盘这一项与 `--group-window` 门里 ⑪ 那一格
                // 问的是同一枚名，这里再抄一遍中文就是两个作者（#281，与上面那一臂同一枚理由）。
                Work::Groups => crate::group_window::TRAY_WORD,
                // §5.18.1 第 1441 行那一列写的就是「检查更新」四个字，唯一作者住在
                // [`falcon_core::update::TRAY_WORD`]：托盘这一项与 `--update-check` 门里那一格问的
                // 是同一枚名（#281，与上面两臂同一枚理由）。
                Work::Update => falcon_core::update::TRAY_WORD,
                // §5.18.1 第 1440 行写的就是「帮助」两个字，唯一作者住在 [`crate::help::TRAY_WORD`]：
                // 托盘这一项与 `--help-menu` 门里那一格问的是同一枚名（#281）。
                Work::Help => crate::help::TRAY_WORD,
                Work::Quit => "退出",
                // 上面这八臂（含 `OpenPicturePins`）就是 `action()` 为 `None` 的全部；这一臂只为让
                // 编译器闭上分岔，走到这里意味着有人给一枚新工作漏了 `action()` 的臂。
                other => unreachable!("action() 为 None 的工作只剩那八臂：{other:?}"),
            },
        }
    }

    /// The work an action id names, if this build performs it. `pub(crate)` 不是因为外部要构造
    /// 一只手，而是因为 §5.15 那一族热区动作名来自 `config::HOT_CORNER_ACTIONS` 而不是来自
    /// [`Work`] —— [`crate::hot_corner::check`] 要在装配时逐名问一次「这枚名字有没有一只手」，
    /// 而那是这一张路由表唯一的真相来源，另抄一份名单就是 #281。
    pub(crate) fn of(action: &str) -> Option<Work> {
        [
            Work::Capture,
            Work::CustomCapture,
            Work::ActiveWindow,
            Work::Repeat,
            Work::PastePin,
            Work::OpenPicturePins,
            Work::Solo,
            Work::TogglePins,
            Work::ShowAllPins,
            Work::HideAllPins,
            Work::ReleaseClickThrough,
            Work::SwitchGroup,
            Work::PinEdit,
            Work::CustomTask,
            Work::Whiteboard,
            Work::Settings,
            Work::History,
            Work::Groups,
            Work::Update,
            Work::Quit,
        ]
        .into_iter()
        .find(|work| work.action() == Some(action))
    }

    /// How one line of the history names this work: the config action when there is
    /// one, since the door reads back by action.
    /// 这一枚工作递进日志与对账的那枚 tag。`pub(crate)` 是给 `--update-check` 那一档的：它要问
    /// 「手上任何一枚能按得到的工作，有没有哪一枚的名字恰好就是自动那一支的名字」——那一问决定
    /// 托盘与热键会不会误点自动检查那枚戳（[`crate::update_auto::stamped_by`]）。
    pub(crate) fn tag(self) -> String {
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

/// 装配一枚键时写下的一行读数：那串字，加上**它在回答什么**。
///
/// kind 不是修饰而是必需：`report` 的普查要「每个配置里的动作一行」那一列的分母，§8.4 屏幕那一行
/// 要「用户要过一枚键而它没成」那一列，而这两问在过去只有一串字可读 —— 拿 `starts_with("冲突")`
/// 去数就是给同一句话造第二个作者（#281），改一天措辞普查当场少数一枚。kind 由写下那一句子的
/// 同一手当场定：它比任何后来的读客都清楚自己刚说的是哪一种事。
#[derive(Debug)]
pub struct Row {
    pub said: String,
    pub kind: RowKind,
}

/// 一行的四种身份。两轴都在这里定死，读的人不需要再猜：
/// [`per_action`] 数的是「某个动作的答复」（`Conflict` 不是答复，它说的是一枚键被几个动作共用），
/// [`to_be_seen`] 数的是「要用户看一眼」（`Taken` 与 `Idle` 不需要）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// 这一枚键注册上了，shell 答好。
    Taken,
    /// 用户在这一格里写过键，而它没成：系统保留、语法坏、递不出注册、shell 回绝、没有归宿。
    Trouble,
    /// 这一格压根没写过键。不是坏，所以没有一句要打扰屏幕的话。
    Idle,
    /// 同一串键被两枚动作共用。它是档里的一条事实，不是某个动作的答复。
    Conflict,
}

impl Row {
    fn taken(said: String) -> Self {
        Self {
            said,
            kind: RowKind::Taken,
        }
    }
    fn trouble(said: String) -> Self {
        Self {
            said,
            kind: RowKind::Trouble,
        }
    }
    fn idle(said: String) -> Self {
        Self {
            said,
            kind: RowKind::Idle,
        }
    }
    fn conflict(said: String) -> Self {
        Self {
            said,
            kind: RowKind::Conflict,
        }
    }
}

/// 「每个配置里的动作一行」那一列的分母。
fn per_action(rows: &[Row]) -> usize {
    rows.iter().filter(|row| row.kind != RowKind::Conflict).count()
}

/// §8.4 要用户看一眼的那几行。
fn to_be_seen(rows: &[Row]) -> Vec<&str> {
    rows.iter()
        .filter(|row| matches!(row.kind, RowKind::Trouble | RowKind::Conflict))
        .map(|row| row.said.as_str())
        .collect()
}

/// §8.4 落在屏幕上的那一行字。
///
/// 引文有上限（[`SAID_MAX`]），理由与 `falcon_core::pin::FILE_PIN_SAID_MAX` 同一枚：浮窗那一格读
/// 的是**一句话**，到第四句就不像提示、像日志。多出来的每一行都没被抹掉：全列原样交给「快捷键」
/// 那一页（[`WATCHED`] → [`crate::settings_view::conflicts`]），所以这里指的是一个用户真能打开的地方，
/// 而不是终端里那几行 stdout。
///
/// 每一句引文都是 `Row::said` 那一串字**本身**，本函数不改写一个字（#281）；数目读的是行数，
/// 不读在任何散文里。
fn notice_line(seen: &[&str]) -> String {
    let quotes: Vec<&str> = seen.iter().take(SAID_MAX).copied().collect();
    let mut line = format!("{} 枚快捷键要你看一眼：{}", seen.len(), quotes.join("；"));
    let rest = seen.len().saturating_sub(quotes.len());
    if rest > 0 {
        line.push_str(&format!("…其余 {rest} 行见「快捷键」设置页"));
    }
    line
}

/// 屏幕那一行最多原样引几枚读数。上限管的是**屏幕上那一行**，不是账本。
const SAID_MAX: usize = 3;

/// What phase 1 decided, before the shell was asked anything. Separate from
/// [`install`] because a sentence about a config file is a pure function of that
/// config file, and only a pure function can be tested without a desktop.
struct Plan {
    /// One row per configured action, in the config's own order.
    rows: Vec<Row>,
    requests: Vec<Request>,
    routes: Vec<Route>,
    /// The next free id, so a tray entry that has no key still gets one.
    next: u32,
}

/// What one run installed. The report reads this alongside the history: a count on
/// one side of the chain is not evidence until the other side says the same number.
pub struct Launch {
    pub started: Started,
    pub rows: Vec<Row>,
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
/// §8.4：本次启动里「要用户看一眼」的那几行，原样留着给「快捷键」那一页整列画出来。
///
/// 浮窗那一格读的是**一句话**，引文有上限（[`SAID_MAX`]），而账不能停在屏幕上放不下的那三句：
/// 页上那一列才是全部。留在这里的是 `Row::said` 本来的字，一格也没重写（#281）。
///
/// 只有产品本体那一趟会写它（`plain`，见 [`install`] 里那一格）—— 量具那一趟读的是自己的判据，
/// 不该顺手把一页的内容换成一次测量的中间数。
static WATCHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn held<R>(cell: &Mutex<R>) -> MutexGuard<'_, R> {
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One line of the chain. Kept short: it is read as evidence, and the sentence is
/// what says which link of the chain it came from.
///
/// `pub(crate)` 自检查更新那一趟走到另一条线程上起：那一次的判决几百毫秒后才落地，而对账那一行
/// 写的必须是判决那一刻的读数，所以它由 [`crate::session::update_landed`] 递进来，而不是由
/// 按下去的那一臂当场写。
pub(crate) fn note(line: String) {
    held(&HISTORY).push(line);
}

pub fn history() -> Vec<String> {
    held(&HISTORY).clone()
}

/// 本次启动那一列「要用户看一眼」的行，逐字交回。「快捷键」那一页画的就是这一串
/// （[`crate::settings_view::conflicts`]），浮窗只引前几枚，所以这里必须是**全列**而不是屏幕上那三句。
pub fn watched() -> Vec<String> {
    held(&WATCHED).clone()
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

/// 改完了贴图名册、但它自己不是 `Session` 的那一类腿要的现取句柄。为什么要把这一只手开出去
/// 而不是让页上补叫第二句：贴图那批窗的唯一听众是 `main.qml` 的 `onCountChanged` → `pinSync`
/// → `syncPins`，而 `count` 只由 `Session` 的 `refresh` 写 —— 于是「钉成了就要
/// 重读名册」这一条规矩必须住在**那一条腿的终点**上；摊到页上，同一扇门的两枚入口（工具条那一格
/// 与另存为那个对话框）就要各记一遍，记漏一遍就多一枚只存在于名册里的贴图（#281）。
///
/// `queue` 落进 Qt 的环里下一拍才跑，与 `pinSync` 那枚 `interval: 0` 的 Timer 同一形状；从 GUI
/// 线程上叫它也只是排队，不会重入 `refresh`。句柄还没到手（`handThread` 没跑）时**不**另报一句：
/// 那一刻屏幕上压根还没有窗，而名册下一趟启动自会从持久化里读回来。
pub(crate) fn republish_roster() {
    // 这一位不读结果：句柄还没到手（`handThread` 没跑）时那一刻屏幕上压根还没有窗，而名册下一趟
    // 启动自会从持久化里读回来。要读结果的那一位是 [`crate::update_auto`]，它得把「没起」说成
    // 「没起」，所以它走 [`post_to_gui`]，这一位也走同一枚 —— 排进事件环这一手只有一个作者。
    post_to_gui(|mut session| {
        session.as_mut().refresh();
    });
}

/// 一件只能在 GUI 线程上做的事，交回**排进去没有**。
///
/// 三种结果分开写（#453）：`In` 是这一趟真的会跑；`NoHand` 是 QML 还没把手递过来（`main.qml` 的
/// `handThread` 排在 `Component.onCompleted` 里，engine.load 之前一切皆无）；`Refused` 是 Qt 收了
/// 这个对象却拒了这次排队。合成一句「起不动」就分不出该改启动次序还是该改这一趟的工作。
pub(crate) enum Posted {
    In,
    NoHand,
    Refused(String),
}

pub(crate) fn post_to_gui<F>(work: F) -> Posted
where
    F: FnMut(std::pin::Pin<&mut session::qobject::Session>) + Send + 'static,
{
    let Some(handle) = gui() else {
        return Posted::NoHand;
    };
    match handle.queue(work) {
        Ok(()) => Posted::In,
        Err(e) => Posted::Refused(format!("{e}")),
    }
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
                .push(Row::idle(format!("{action} 未绑定（配置里这格是空的）")));
            continue;
        }
        let parsed = match hotkeys::parse_bindable(text) {
            Ok(spec) => spec,
            Err(e) => {
                out.rows
                    .push(Row::trouble(format!("{action} {text:?} 注册不了：{e}")));
                continue;
            }
        };
        let canonical = parsed.canonical();
        let display = hotkeys::display(parsed);
        if let Some(why) = parsed.reserved_reason() {
            out.rows.push(Row::trouble(format!(
                "{action} {display} 是系统保留组合：{why}（不注册）"
            )));
            continue;
        }
        let Some(work) = Work::of(action) else {
            out.rows.push(Row::trouble(format!(
                "{action} {display} 未接线（不注册，键留给系统）"
            )));
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
            Err(e) => out.rows.push(Row::trouble(format!(
                "{action} {display} 递不出注册：{e}"
            ))),
        }
    }
    out
}

/// §5.18.1 的托盘菜单：把「本 build 真做得动、且 PRD 明写要一个托盘入口」的那几件逐一列出，
/// 末尾接 退出。P138 之前这里只有 截图 / 贴出剪贴板 两只可键启动的工作，于是 PRD 菜单建议里
/// 「显示或隐藏全部贴图」那一条、以及 §5.2.5 第 215 行明写「重复上次截图快捷键**或托盘菜单**」
/// 那一条，都在托盘上无处可点 —— P137 已给 `toggle_pins` 装上真归宿（[`Work::TogglePins`]），
/// 这一轮把这两枚接到托盘。
///
/// **只接有归宿的**：这一条从 P138 起一直有效，而它当时挡在门外的是「帮助」与「检查更新」两枚。
/// 现在两枚都有了手 —— 「检查更新」自 P173（[`Work::Update`]），「帮助」自 P174（[`Work::Help`]，归宿是
/// [`crate::help`] 那句由四位作者现取拼成的话）。挂一行点了什么都不干的死菜单仍是 #433 那一族：死行是
/// 装饰，不是功能。**PRD §5.18.1 那一列的十行自此全有对面**，这封信从此不再欠哪一行；它继续留在这里，
/// 是因为它是下一枚菜单项进门时的那道门，不是因为还欠着东西。
///
/// 「检查更新」自 P173 起不算在这一列里了：它的手是 [`Work::Update`]，落点是 §5.21.1 那四步里的
/// 前三步（第 1441 行那一枚菜单项 → [`falcon_core::update::check`] → 产品自己那句判决写进
/// `status`，而屏幕上那扇浮窗绑的就是它）。第 4 步那句「显示版本号、更新说明和下载入口」里那一枚
/// **可以点的**入口，自 P179 起住在「关于与更新」那一页的页脚（`qml/SettingsWindow.qml` 的
/// `updateRow`，地址出自同一次判决的 [`falcon_core::update::Answer::download_url`]）；托盘这一臂
/// 落地的仍是那句判决写进 `status` —— 它不替用户把那扇页打开，也不在菜单里长出一枚下载钮。
///
/// 「图片分组」自 P172 起不算在这一列里了：它的手是 [`Work::Groups`]、落点是 §5.12.3 / §5.12.4
/// 那一扇 [`crate::group_window`] 与 `qml/GroupWindow.qml`，第 1437 行那一行自此在托盘上有点得动的
/// 一项。§5.12.4 第 1251 行那三个入口自此三个都有对面：托盘这一枚、分组面板那一枚，与
/// 「快捷键」那一枚 —— 后者自 P186 起是 [`Work::SwitchGroup`]，它做的是名册说的那件事（在几组之间
/// 轮流切显），不是「开那一扇管理窗」。它**不在这一列菜单里**，理由写在 `run` 那一臂头顶。
///
/// 「自定义截图」自 P166 起不在这份欠账里了：它的那只手是 [`Work::CustomCapture`]，落点是 §5.3.5
/// 那扇对话框窗（[`crate::custom`]），而 §5.16.1 的默认组合 `ctrl+shift+f` 自此也真的有东西应答。
///
/// 「历史记录」自 P171 起不在这份欠账里了：它的那只手是 [`Work::History`]，落点是 §5.14.4
/// 第 1309 行那扇「历史记录管理」窗（[`crate::history_window`]）。
///
/// 「首选项」自 P142 起不在这份欠账里了：它的那只手是 [`Work::Settings`]，落点是 §5.20.1 那扇
/// 窗。它进的是下面那个**有归宿者**的循环，而不是一枚 Rust 自己 `show()` 的窗 —— 托盘写的只是
/// 「该不该有一扇窗」那一格。
///
/// An entry whose key never got an id (unbound, reserved, unwired) is given a menu-only id,
/// because the icon is the other way to reach the same feature - and the only way to reach 退出.
/// 装配出来的那份托盘菜单里每一项的**原名**（不带快捷键尾巴的那一枚 `work.label()`）。
///
/// §5.18.1 第 1438 行那枚「历史记录」到底在不在菜单上，问的必须是这一份——即
/// [`tray_menu`] 真的把它装配出来了，而不是 `Work::History` 这一枞枚举存在。另抄一份
/// 「托盘里有哪些名字」就是 #281；这里走的是同一只函数，它多一行这里就多一行。
///
/// 这里递的是一份**空路由表**上的菜单：读的是「这一果工作进不进得了菜单」这一格，
/// 而不是「这台机器上那果键绑到了谁」。后者的读数在 [`install`] 那一路，与本档无关。
pub fn tray_labels() -> Vec<String> {
    let mut plan = Plan {
        rows: Vec::new(),
        requests: Vec::new(),
        routes: Vec::new(),
        next: 1,
    };
    tray_menu(&mut plan)
        .into_iter()
        .map(|(_, name)| name.split('（').next().unwrap_or_default().to_string())
        .collect()
}

/// 菜单上那一项**按下去走的是哪一手**：`(id, work)` 那一份名单，读的还是上面那只唯一的
/// [`tray_menu`]。[`tray_labels`] 交回的是同一份装配的名字，两枚问的是同一枚作者的两侧——
/// 「托盘上看得见这一行」与「这一行有归宿」是两件事，`--update-check` 门要的是后一件，
/// 而它不许自己另装一份菜单（#281）。
pub fn tray_routes() -> Vec<(u32, Work)> {
    let mut plan = Plan {
        rows: Vec::new(),
        requests: Vec::new(),
        routes: Vec::new(),
        next: 1,
    };
    tray_menu(&mut plan)
        .into_iter()
        .map(|(id, _)| {
            let work = plan
                .routes
                .iter()
                .find(|route| route.id == id)
                .map(|route| route.work)
                .unwrap_or(Work::Quit);
            (id, work)
        })
        .collect()
}

fn tray_menu(plan: &mut Plan) -> Vec<(u32, String)> {
    let mut menu = Vec::new();
    for work in [
        Work::Capture,
        Work::CustomCapture,
        Work::Repeat,
        Work::PastePin,
        // §5.8.3 第 1 步那一枚「打开图片并贴图」。它紧跟在 [`Work::PastePin`] 后面的理由是
        // §5.8 自己的次序：§5.8.2「从剪贴板创建贴图」与 §5.8.3「从图片文件创建贴图」是同一节里
        // 相邻的两段，两段的入口在菜单上也就挨着站。§5.18.1 那一列十行没有这一行（它只有
        // 「贴图」，而那一行早就落在上面那一臂上）—— 与白板、解除穿透同一处结转：两处不一致时
        // 按**功能那一节**办，冲突记在 [`Work::OpenPicturePins`] 那一臂与本档报告里，不当作已解决。
        // 它也不带括起来的按键名：`action()` 是 `None`，场上没有一枚键通向它（理由见那一臂）。
        Work::OpenPicturePins,
        Work::TogglePins,
        // §5.9.13 规则第一句那一条退出。PRD 第 1436 行「显示或隐藏全部贴图」与第 1437 行
        // 「图片分组」之间没有这一行 —— 它不在 §5.18.1 那一列里，它来自 §5.9.13 第三步点名的
        // 「托盘菜单」那一枚入口，与下面那枚白板同族（两处不一致时按**功能那一节**办，冲突记进
        // 报告、不当作已解决）。站在这里而不贴在菜单末尾的理由是形状：这一枚与 `TogglePins`
        // 都是「场上那一批贴图」的全局一手，而它是这一列里唯一一枚不必点中贴图就能按到的撤销。
        Work::ReleaseClickThrough,
        // PRD 第 1436 行「显示或隐藏全部贴图」与第 1438 行「历史记录」之间那一行就是第 1437 行
        // 「图片分组」，所以它排在 TogglePins 之后 —— 菜单这一列的次序由文档那一列决定，不是由
        // 哪一枚先落地决定。（白板不在那一列里，它来自 §5.4.4 那一节，见上面那臂记的冲突。）
        Work::Groups,
        Work::Whiteboard,
        // 第 1438 行「历史记录」在文档那一列里就排在第 1439 行「首选项」**之前**，这里因此先排
        // 历史记录。P174 之前这两行的次序是按谁先落地排的（首选项 P142、历史记录 P171），而
        // `--help-menu` 那扇门把「次序由文档那一列决定」这条一直写在注释里的判据真的量了一遍，
        // 它当场判出 FAIL：帮助 落在第 9 位、它上面那一行是历史记录而不是首选项。修的是装配的
        // 次序，不是那一格的判据。
        Work::History,
        Work::Settings,
        // §5.18.1 第 1440 行「帮助」自 P174 起有了手（[`Work::Help] → [`crate::help`]），就排回文档
        // 给它的那一列上：首选项之后、检查更新之前。次序仍由那一列决定，不是由谁先落地决定。
        Work::Help,
        // 第 1441 行那一枚排在第 1442 行「退出」之前；P173 时它上面那行「帮助」没有手，所以它直接
        // 接在 History 之后。现在那一行有手了，它回到自己原来的位子上。
        Work::Update,
    ] {
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

/// §5.9.13 规则第一句「必须提供明确且可靠的退出穿透方式」写成的**一判**，而不是一句注释。
///
/// 判据的形状来自这一条要防的那一件具体的事：贴图一开始穿透，鼠标就落不到它身上 —— 连它自己的
/// 右键菜单（`qml/PinWindow.qml` 里那枚「鼠标穿透」勾，开启走的就是那一枚）都点不到。所以
/// 「可靠的退出」可判的内容不是「有一个地方能关」，而是：**取消穿透这一枚动作必须至少有一个
/// 入口，而这个入口不穿过贴图**。全局热键（shell 把 `WM_HOTKEY` 送到那枚消息窗，从不经过贴图）
/// 与托盘（点在通知区域的图标上）都算；贴图的菜单不算。
///
/// 交回的是**此刻真成立**的那些入口名；空 = 这一句在这一份装配上不成立。两半各问一次，问的都是
/// 装配本身而不是这一档的措辞：
/// * 快捷键那一半要两件事同时成立 —— `routes` 里有这一枚 `Work`（有一只手）且 `requests` 里有
///   同一枚 id（真的把那把键从 shell 手里要来了）。只有前者是「配得上却什么都不做」那一族，
///   只有后者是「拿了一串键而没人应答」那一族，两样都算不上一条退出。
/// * 托盘那一半问 [`tray_routes`]，即 [`tray_menu`] 真装配出来的那一份，而不是这里另列一张
///   「菜单里该有什么」（#281）。它还要 `tray_mounted` 同时成立：[`super::tray_from`] 那一枚
///   分岔说的是「这一场托盘**挂没挂上**」—— `[general] show_tray = false` 时
///   `Shell_NotifyIcon` 根本不挂、通知区域里没有一枚图标可以点（`main.rs` 那一头读的是同一枚
///   布尔），此时把菜单上那一行算成一条退出就是把一件不存在的事说成救生门。装配与安装是两件事，
///   这一判要的是两样都成立。
///
/// 纯函数、不问 Win32，为的是 §9.1 第十条那一族 headless 的半边：这一条判据值得由 `cargo test`
/// 而不是由一次桌面跑来说话。
///
/// **这一格判不到的那一条**要写着：§5.9.13 第三步还点了「热区」。热区那五枚动作的名单是
/// [`falcon_core::config::HOT_CORNER_ACTIONS`]，里面没有这一枚，而那一族的接线点在
/// `hot_corner.rs` / `hotcorner.rs` 与 `qml/SettingsWindow.qml` 三处 —— 这一轮没接，交回的名字
/// 因此最多两枚，不是三枚。
///
/// 私有到本模块，理由与 [`menu_id_of`] 同一枚：[`Plan`] 是本层的私有类型，把它写进
/// `pub(crate)` 的签名就是让外面看得见一个用不上的名字（`private_interfaces` 那一纸正是这一句）。
/// 门要的读数走 [`install`] 交回来的那份 [`Launch`]，这一格要的判据在下面的 `tests` 里，两处
/// 都不必把 `Plan` 递出去。
fn click_through_exits(plan: &Plan, tray_mounted: bool) -> Vec<&'static str> {
    let mut ways: Vec<&'static str> = Vec::new();
    if plan.routes.iter().any(|route| {
        route.work == Work::ReleaseClickThrough
            && plan.requests.iter().any(|ask| ask.id == route.id)
    }) {
        ways.push("预设快捷键");
    }
    if tray_mounted
        && tray_routes()
            .into_iter()
            .any(|(_, work)| work == Work::ReleaseClickThrough)
    {
        ways.push("托盘菜单");
    }
    ways
}

/// 遮罩正开着时要不要挡下这一枚工作。`Quit` 与 `ReleaseClickThrough` 不挡，其余一律挡。
///
/// 这一枚分岔从 [`gate`] 里抠出来单独成函数，是为了让 §5.9.13 那一句「可靠」有地方可判：
/// `gate` 那一手要问 `mask::with`（一次真桌面的读数），单测跑不动；而「一条退出会不会被一件
/// 不相干的事挡在门外」这一问压根不需要知道遮罩开没开 —— 它要的是分岔本身。与 [`tray_from`]
/// 同一族（分岔本体自己作证）。
///
/// `Quit` 那一半是原来的写法（`work != Work::Quit`），一个字没改语义；`ReleaseClickThrough`
/// 跟着进来的理由与它同一条，而且更硬：挡在一条退出前面的，不该是另一件正开着的事。一次解除
/// 既不冻画面、也不碰剪贴板、也不动成员（§5.3 那一句「不许再冻一次遮罩」管的三样它一样不占）。
const fn mask_blocks(work: Work) -> bool {
    !matches!(work, Work::Quit | Work::ReleaseClickThrough)
}

/// §5.9.13 规则第二句「可显示短暂状态提示」在这一手**解除**那一边说出的那句话。
///
/// 句子在这里、不在 [`run`] 那一臂里，是因为屏幕上那一扇窗读的是 `status` 而这一层的日志读的
/// 是同一个人说的话：两处各拼一遍中文，早晚会有一句在骗另一句（#281）。
///
/// 0 枚那一支必须自己有一句：按下去什么都不做而屏幕什么也不说，用户读到的是「这一枚键坏了」，
/// 而它没坏 —— 场上压根没有穿透中的贴图（#433：不许静默）。
fn click_through_said(released: usize) -> String {
    if released == 0 {
        "没有处于鼠标穿透的贴图".to_string()
    } else {
        format!("已解除 {released} 枚贴图的鼠标穿透")
    }
}

/// §5.9.13 规则第一句与第二句在**启用**那一边的句子：贴图一旦穿透，鼠标就落不到它身上、连它
/// 自己的菜单都点不到，所以「启用时可显示短暂状态提示」在这一节里不能只报「开了」——它必须把
/// **怎么出去**一并报出来，否则那句话只是把用户关在里面的那一声关门响。
///
/// 出口那一列读的是 [`click_through_exits`] 在 `install` 那一刻判出来的那一份（见
/// [`CLICK_EXITS`]），不是这里现编的名单：PRD 第三步点名的三枚入口（预设快捷键、托盘菜单、热区）
/// 里此刻真成立的有哪几枚，问的是装配，而这一格只把它们念出来。
///
/// 空列是**最要紧的那一支**，不许退成静默、也不许退成一句假话（说「按某某键」而那枚键没注册）：
/// 它照实说「此刻没有已配好的退出入口」，并把唯一那位作者的名字递出去
/// （[`Work::ReleaseClickThrough::action`] 那一枚 id，与设置页、名册同一名，#281），因为
/// 「该配哪一格」这件事只有档上那一行答得出。
fn click_through_entry_hint(count: usize, exits: &[&'static str]) -> String {
    let opened = format!("已对 {count} 枚贴图开启鼠标穿透");
    if exits.is_empty() {
        return format!(
            "{opened}；此刻没有已配好的退出入口：请在「首选项 > 快捷键」给 {} 配一枚键，\
             或把 [general] show_tray 打开走托盘菜单。",
            Work::ReleaseClickThrough.action().unwrap_or_default()
        );
    }
    format!("{opened}；要收回：{}", exits.join(" 或 "))
}

/// 启用那一边的读数：场上从 `prev` 枚变成 `now` 枚时，值不值得说一句。
///
/// 只有**变多**才算一次启用。一枚贴图被关掉（`now < prev`）不该飘出一句「已开启」，而一枚也没
/// 变的那些格是绝大多数 —— 这一枚分岔存在的理由就是让每秒十二次的采样只在真的有事那一刻说话
/// （#433 的反面：一条没有输出的噪声不算功能）。
const fn click_through_watch(prev: usize, now: usize) -> Option<usize> {
    if now > prev {
        Some(now)
    } else {
        None
    }
}

/// [`click_through_exits`] 在 `install` 那一刻判出来的那一份，留着给采样腿念。
///
/// 为什么是快照而不是每格现判：那一判要 `&Plan`，而 `Plan` 的路由表在 `install` 结束时就被
/// `mem::take` 搬进 [`ROUTES`] 与泵里（每格重建一份 `Plan` 就是每格重算一遍装配，还会为菜单
/// 发一批新的 id）。启动到现在那几枚键有没有真从 shell 手里要来、托盘有没有真挂上，这一场里
/// 不会变 —— 要变的话得重新 `install`，而那正是这一枚快照被重写的时刻。
static CLICK_EXITS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn click_exits() -> Vec<&'static str> {
    held(&CLICK_EXITS).clone()
}

// ── §5.12.4 第 1251 行那三个入口里的第三枚「快捷键」 ─────────────────────────────────
//
// 三格里前两格（托盘菜单、分组面板）自 P172 起有对面，第三格一直是台账 ⑫。下面这三枚纯函数
// 要的就是「轮流」那一判能在一台没有屏幕、也没有一双手的机器上被问一遍（§9.1 第十条那一族）。

/// 这一枚键上一次把显示切到了第几组，[`group_cycle`] 的输入。`None` = 这一场还没切过。
///
/// 为什么这一枚记性留在这一层而不是搬进名册：core 那一份 `PinSet` 记的是**每组显不显**
/// （[`falcon_core::pin::PinSet::group_shown`]），压根没有「第几组是当前组」那一格 —— 而
/// 「轮流」要的恰恰是那个**上一次**。替别人家添一枚没人要的字段是 #281 的另一面，留在这一层
/// 就是这一枚键自己的记性：删了组、换了序，下一按从第一组重新开始。
static GROUP_CURSOR: Mutex<Option<usize>> = Mutex::new(None);

/// §5.12.4 那一按把显示切到哪一组：交回 `(目标那一组的序号, 要顺手关掉的序号)`。
/// `None` = 场上压根没有分组，这一按没事可做（那一支有自己的那句话，见 [`no_group_said`]）。
///
/// 读的是 [`crate::state::PinState::group_rows`] 交回的那一份，**顺序就是 core `groups` vec
/// 本体** —— PRD 第 1245 行那句「排序变化应同步反映到快捷切换菜单」要的就是这一条：分组窗里把
/// 乙组移到甲组之前，下一按轮到的就是新的那一个后继，这一层一格也不重排（#281）。
///
/// 「切到哪一组」与「哪些组要关掉」分两样交回，是因为 §5.12.4 第 1253 行那句「系统**统一**更新
/// 该分组内贴图的可见状态」在这一枚键上有两种读法，而名册里那句说明（「轮流切换显示**哪一组**」）
/// 定死了这一种：一次只有一组在场。目标本来就在显示中时 `off` 也不把它算进来，于是 [`switch_group`]
/// 那一手一枚也不叫，交回的是「已经就在这一组上」那句实话（[`already_on_said`]）。
fn group_cycle(rows: &[(u64, String, bool, usize)], cursor: Option<usize>) -> Option<(usize, Vec<usize>)> {
    if rows.is_empty() {
        return None;
    }
    // 上一场记着的那一格可能已经不存在（真人把组删了）：从第一组重新开始，而不是拿一枚越界的
    // 序号去取模出一个看起来随机的后继。
    let at = match cursor {
        Some(last) if last < rows.len() => (last + 1) % rows.len(),
        _ => 0,
    };
    let off = (0..rows.len())
        .filter(|idx| *idx != at && rows[*idx].2)
        .collect();
    Some((at, off))
}

/// 切成了的那一句。组名读的是 core 那一格（用户在分组窗里亲手写的那一串字），这一层只加动词。
///
/// 「此刻场上只显示这一组」不是修辞而是那一判的读数：`off` 列出的就是**除目标之外所有**正显示
/// 的组，它们都在这一趟里被关掉了，所以这句话在事后一定复查得回来。
fn group_switch_said(name: &str) -> String {
    format!("已切到分组「{name}」，此刻场上只显示这一组")
}

/// 场上没有分组时那一按说的话。「去哪儿建组」这一问读的是 [`crate::group_window::TRAY_WORD`]
/// 那一枚唯一作者，不在这里重打一遍「图片分组」四个字（#281）；而静默会把「按了没反应」说成
/// 「这一枚键坏了」，那一支才有这一句（#433）。
fn no_group_said() -> String {
    format!(
        "还没有分组：托盘的「{}」里新建一组，再按这一枚键轮流切换。",
        crate::group_window::TRAY_WORD
    )
}

/// 目标那一组本来就在显示中、而场上也没有别的组要关：这一按什么也没改，照实说什么也没改。
fn already_on_said(name: &str) -> String {
    format!("分组「{name}」已经在显示中，场上也没有别的分组可切")
}

/// Register what this build can do, install the icon, and start the listener.
///
/// Returns the census rather than printing it, so the caller decides what a census
/// is for: a door grades the rows, the app just tells the user what it took over. An
/// `Err` is a run that installed nothing.
///
/// `plain` 是 `main` 那一枚「这一趟是产品会话，不是量具门」的读数，原样递给角落那条采样腿：
/// 一枚绑了 `capture` 的角落会在 `--hotkeys` 的历史里添上它自己的行，而那一门回读的就是那些行。
/// 测量跑动了被测的东西，读回来的就不是那一门自己的数 —— 与 [`crate::hot_corner::measure`]
/// 一次也不派发是同一条规矩（#281：一句规矩写一处，两半都从这一枚布尔读数）。
pub fn install(plain: bool) -> Result<Launch, String> {
    if held(&PUMP).is_some() {
        return Err("这个进程已经有一个热键泵了：一条线程一个窗口，不注册两次".to_string());
    }
    let Some(handle) = gui() else {
        return Err("QML 还没交出它的 GUI 线程（handThread 没跑），热键一个也不注册".to_string());
    };
    let actions = crate::settings::with(|s| s.config.hotkey.actions.clone());

    let mut planned = plan(&actions);
    let menu = tray_menu(&mut planned);
    // §5.9.13 规则第一句「必须提供明确且可靠的退出穿透方式」的那一判，趁 [`Plan`] 还在手里判一次：
    // 下一句就把路由表搬进 [`ROUTES`] 与泵了。判出来的这一列同时是**屏幕上那一行**在贴图被启用
    // 穿透时要念的那一列出口名（[`CLICK_EXITS`]）—— 「启用时告诉用户怎么出去」这句话用的因此是
    // 此刻装配的读数，而不是本层另养的第二份名单（#281）。
    //
    // `general` 在这里读，是因为 `show_tray` 那一格既决定托盘挂不挂、也决定「托盘菜单」这一条
    // 退出成不成立；读两遍就是拿两次可能不一致的答案拼一个判断（[`CornerPoll::refresh_snapshot`]
    // 头顶记的是同一条规矩）。下面 `tray_from` 用的就是这一枚。
    let general = crate::settings::general_cfg();
    let exits = click_through_exits(&planned, general.show_tray);
    *held(&CLICK_EXITS) = exits.clone();
    note(format!(
        "§5.9.13 退出穿透：{}",
        if exits.is_empty() {
            "成立的入口一个也没有（show_tray=false 且没配 release_click_through 的键）—— \
             贴图一旦被启用穿透，此刻没有任何不穿过它的手"
                .to_string()
        } else {
            format!("成立的入口 {}（show_tray={}）", exits.join(" + "), general.show_tray)
        }
    ));
    let routes = std::mem::take(&mut planned.routes);

    // Two bindings that mean one thing is a fact about the config, reported whether
    // or not the shell then accepted both. Core owns that rule, so this is the same
    // sentence the settings page will show (§5.16.1).
    let mut rows = planned.rows;
    for clash in hotkeys::conflicts(&actions) {
        if clash.actions.len() > 1 {
            rows.push(Row::conflict(format!(
                "冲突 {}：{} 都在用这一串键",
                clash.display,
                clash.actions.join(" + ")
            )));
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
    // `general` 那一次现读自 P186 起在上面判退出入口时就已经取了 —— 同一场里读两遍 `[general]`
    // 不是两枚读数而是一次机会，`show_tray` 在两遍之间被人改掉的话，托盘挂的就是后一遍、
    // 退出判的就是前一遍。
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
        rows.push(Row::trouble(format!(
            "{} {display} 注册失败：{}（Win32 {}）",
            refusal.action, refusal.why, refusal.win32_error
        )));
    }
    for (id, action) in &started.registered {
        let display = routes
            .iter()
            .find(|route| route.id == *id)
            .map(|route| route.display.clone())
            .unwrap_or_default();
        rows.push(Row::taken(format!("{action} {display} 已注册")));
    }

    // §5.1.1's "正确反映应用运行状态", as far as a tooltip can: the count the shell
    // actually took, which is not the count this process asked for until `started`
    // says so. 句子由 [`tip_line`] 拼（那一枚也是 §5.11 规则③ 事后重写时读同一只手的同一枚
    // 作者）；这一枚枚数同时存进 [`TIP_KEYS`]，因为 `Started` 到 `Launch` 就交出去了，而独显
    // 那一次改写要说的还是同一个数 —— 现读自这里，不另数一遍 `requests`。
    let tip = tip_line(started.registered.len(), None);
    *held(&TIP_KEYS) = started.registered.len();
    let tip_set = pump.set_tip(&tip);
    *held(&TIP_LAST) = tip.clone();
    // §8.4 第一句「显示冲突的组合键和相关功能」自此有屏幕上的一格。此前那几行只走 stdout
    // （`main.rs` 的 `println!("[hotkey] {row}")`），而 §5.1 的根是一枚看不见的 `Item`：一个人
    // 不开终端启动本程序时，他唯一看得见的地方是 `Session.status` 那一行浮窗
    // （[`qml/HintWindow.qml`] 绑的就是它，理由与时长都归那扇窗自己管，见
    // [`CornerPoll::watch_click_through`] 头顶那一段）。
    //
    // **改道而不是另拼一句**：屏幕与日志读的是同一枚 `Row::said`，一个字也不重打（#281），
    // 而那一句说什么由写下它的那一手定 —— 本格只挑「要用户看一眼」的那几行，判据住在
    // [`RowKind`] 里而不是这里的字符串匹配（#453：几种坏就几种说法，不重叠也不共用）。
    //
    // 两处屏幕各放得下多少是不同的：浮窗那一格放一句（[`SAID_MAX`] 句引文），「快捷键」那一页放
    // 全列（[`WATCHED`]）。页因此不是浮窗的副本，而是浮窗放不下那几行的**去处** —— 屏幕上那句
    // 「其余几行见…」必须指一个用户真能打开的地方（#433）。
    //
    // 只在产品本体那一趟写（`plain`）：量具档里 `status` 是**给判据用的**，这里替它写一句就会
    // 污染那一档正在读的那一行（`tick` 头顶记的是同一条规矩）。出厂那九枚键全成时这一列是空的，
    // 一个字也不写 —— §8 整章判的是出了问题时说什么，不是为「一切正常」闪一下浮窗。
    if plain {
        let seen = to_be_seen(&rows);
        // 先记全列，再拼屏幕上那一句：浮窗放不下四句，页上放得下，两处读的是同一枚 `Vec<&str>`
        // ——「哪几行要用户看一眼」这件事仍然只由 [`RowKind`] 判一次（#281）。
        let full: Vec<String> = seen.iter().map(|line| (*line).to_string()).collect();
        note(format!(
            "§8.4 交给「快捷键」页的那一列：{} 行（全列在页上，屏幕上那行只引前 {SAID_MAX} 枚）",
            full.len()
        ));
        *held(&WATCHED) = full;
        if !seen.is_empty() {
            let said = notice_line(&seen);
            note(format!("§8.4 交给屏幕的那一行：{said}"));
            if let Err(e) = handle.queue(move |mut session| {
                session.as_mut().set_status(QString::from(&*said));
                note("GUI 线程 §8.4：那一行已落在 Session.status 上".to_string());
            }) {
                note(format!("§8.4 那一行没能交给 GUI 线程：{e}"));
            }
        } else {
            note("§8.4 交给屏幕的那一行：没有一行要用户看（这一趟不写 status）".to_string());
        }
    }
    let Some(events) = pump.take_events() else {
        pump.stop();
        return Err("事件流被取走了两次，这一趟没人读它".to_string());
    };
    let listener = std::thread::Builder::new()
        .name("falconshot-hotkey-listen".into())
        .spawn(move || listen(events, handle, plain));
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
///
/// P140 起这一手不再「一直睡着等人敲门」：`recv()` 换成 `recv_timeout(POLL_MS)`，于是**没人敲
/// 门的那些格**也有一次读数。这是 §5.15 唯一可能的形状 —— 热区不产生任何一条 Win32 消息，
/// 它要的是一次**问**（指针此刻在哪），而这一枚泵的循环是全进程里唯一既常驻、又不占 GUI 线程、
/// 又肯每秒醒十几次的地方。没有新增线程，也没有新增 `Event` 变体：一台机器借了另一台的闹钟。
fn listen(events: Receiver<Event>, handle: Gui, plain: bool) {
    let mut corner = CornerPoll::new(plain);
    loop {
        match events.recv_timeout(Duration::from_millis(hot_corner::POLL_MS)) {
            Ok(event) => route(event, &handle),
            Err(RecvTimeoutError::Timeout) => corner.tick(&handle),
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    note("事件流结束：泵停了，监听线程收工".to_string());
}

/// §5.18.2 那一枚动作名落地成什么后果。四臂：一只 `Work`、开托盘菜单、什么都不做、没有归宿。
pub enum TrayRun {
    Work(Work),
    Menu,
    Nothing,
    Unwired(String),
}

/// 动作名 → 后果，这一层**唯一**的一张表。
///
/// 门读它、`route` 也读它：两处各写一份 match 的话，加一枚动作而漏掉门那一侧，门仍然是绿的
/// —— 那正是 #281 说过的那种坏。
///
/// 六枚可绑定动作不在这里逐名列举，走的是 [`Work::of`]：那张表已经是热键与热区两路在用的归宿表，
/// 此处再抄一份就成了「同一个动作两张表」——`Work` 加一枚而这里漏一枚时这里交出 `Unwired`，也就
/// 是**配得上却什么都不做**那一族。
///
/// 剩下的三臂是**不在 §5.16.1 那十二名表里**的名字，只能自己写：`Work::of` 是按可绑定动作名查表
/// 的，而 `settings` 从来不是可绑定动作（给一扇窗编一枚可绑定动作＝替那张表发明第十二名，见
/// [`Work::Settings`] 那一段），于是它交回 `None`。把 `settings` 塞进那条通路上，读到的会是
/// 「未接线」——一张假红，红的是查表的方式而不是产品。`menu` 与 `none` 同理：它们没有 `Work`。
pub fn tray_run_of(action: &str) -> TrayRun {
    match action {
        "settings" => TrayRun::Work(Work::Settings),
        "menu" => TrayRun::Menu,
        "none" => TrayRun::Nothing,
        other => Work::of(other).map_or(TrayRun::Unwired(other.to_string()), TrayRun::Work),
    }
}

/// 一次敲门怎么办：认门牌（`ROUTES`）→ 过闸（[`gate`]）→ 排队到 GUI 线程。
///
/// `Event::SettingChange` 是这一手里唯一不进 [`Work`] 表的一枚，因为它没有「哪个动作」的归宿，
/// 也不该被遮罩挡住（见下面那一格的注释）。
fn route(event: Event, handle: &Gui) {
    let through = matches!(event, Event::Menu(_));
    let (what, work) = match &event {
        Event::Tray(kind) => {
            // §5.18.2 第四步：这一击跑什么，问的是档上那四枚键，不是这一层写死的常量。
            // 每一次点击现问一次，所以改了档不必重启也不必等下一次开机 —— 与那一族
            // `*_cfg()` 快照同一形状（见 `settings::tray_cfg`）。
            let action = crate::settings::tray_cfg()
                .entries()
                .iter()
                .find(|(name, _)| *name == kind.id())
                .map_or("none", |(_, action)| *action)
                .to_string();
            let what = format!("托盘{} {action}", kind.label());
            match tray_run_of(&action) {
                TrayRun::Menu => {
                    // 菜单归拥有那枚窗口的线程去 track，所以这里交出去的是"投一发"：
                    // 真假说的是投没投出去，而不是菜单开没开（后者在另一条线程上）。
                    let sent = held(&PUMP)
                        .as_ref()
                        .is_some_and(|pump| pump.show_tray_menu());
                    note(format!(
                        "{what}：{}",
                        if sent {
                            "已投给泵线程开菜单"
                        } else {
                            "泵不在，这一发投不出去"
                        }
                    ));
                }
                TrayRun::Nothing => note(format!("{what}：这一格什么都不做")),
                TrayRun::Work(work) => dispatch(&what, work, handle),
                // 名单外的名字到这里 = 有人绕过了 `config::validate` 那一层归一。不静默：
                // 静默会把"配得上却什么都不做"那一族重新打开，而 P140 刚在热区那头把它关掉。
                TrayRun::Unwired(other) => note(format!("{what}：动作 {other:?} 没有归宿，不动手")),
            }
            return;
        }
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
            return;
        }
    };
    let Some(work) = work else {
        note(format!("{what}：没有归宿，不动手"));
        return;
    };
    dispatch(&what, work, handle);
}

/// 一把键或一枚角落进门后要过的两道关，和过关之后那一句排队。
///
/// 这一枚函数是 P140 从 `route` 的手心里抠出来的：§5.16.2 的前台静音与 §5.3 的「遮罩已经开着
/// 就不许再冻一次」这两判，热键要过、热区也要过，而**一枚判据只许写一份**（#281）。留在原来
/// 那一只手里的话，角落那一路就得另抄一份两份都改不动的名单。
fn dispatch(what: &str, work: Work, handle: &Gui) {
    if let Some(why) = gate(work) {
        note(format!("{what} {}：{why}", work.tag()));
        return;
    }
    note(format!(
        "{what} {}：排队到 GUI 线程（{}）",
        work.tag(),
        work.label()
    ));
    // `queue` neither blocks this thread nor panics on a destroyed object when
    // its result is read rather than unwrapped - cxx-qt's own guidance.
    let tagged = what.to_string();
    if let Err(e) = handle.queue(move |session| run(work, session)) {
        note(format!("{tagged} {}：GUI 线程没接住（{e}）", work.tag()));
    }
}

/// 两道关。交回 `None` = 放行；交回 `Some` 就是要打进历史的那半句，所以措辞与 P140 之前
/// `listen` 里那两行逐字相同 —— `--hotkeys` 门读的就是这些行。
fn gate(work: Work) -> Option<String> {
    if let Some(why) = silenced(work) {
        return Some(format!("被静音（{why}）"));
    }
    if mask_blocks(work) && mask::with(|m| m.shown) {
        // A second freeze would photograph the mask it is about to cover, which
        // is the one thing §5.3's whole design avoids.
        return Some("遮罩已经开着，这一次不动手".to_string());
    }
    None
}

/// §5.15 的采样腿：把「问 Windows」这一件事集中到三处，算术全部留给 core 那台机器。
///
/// * 配置、显示器、全屏那三问**一秒一次**（[`hot_corner::SNAPSHOT_MS`]）。角落方块的几何是插
///   拔显示器才会动的东西，一台游戏也不会在一格里起落，而全屏那一问要两枚 syscall。
/// * 指针与修饰键**每格一次**（[`hot_corner::POLL_MS`]）：「停留」由这两枚的时序定义，别的都
///   不是。
///
/// 每格都重建机器是一个 bug 装扮成配置刷新：[`hot_corner::Runner::refresh`] 因此只在快照真的
/// 变了才重建，而「没换快照不许清停留」由 `hot_corner::check` 里按名字判的两行盯着。
///
/// 整段关掉（`enabled = false`，出厂默认那一侧）走最短的一条路：数一下时间就回去，既不读指针
/// 也不落一行历史 —— 这一族唯一的输出就是历史，一条都没有的噪声不算功能（#433 的反面）。
struct CornerPoll {
    /// 这一趟是不是产品会话。假 = 一扇量具门借用了这条泵的闹钟，角落一律不问也不动。
    plain: bool,
    /// `None` = 这一段没开，或这一台机器连显示器都读不出来。
    runner: Option<Runner>,
    /// 与 `runner` 同一节奏重问的一次，缓存下来供每一格的 `poll` 用。
    screen: hot_corner::Fullscreen,
    /// 上一次问那三问的时刻；`None` = 这一条腿还没问过，所以第一格就要问。
    ///
    /// 「没有机器就每一格重问」是这段代码自己之前的写法，而它违反本档注释钉的节奏：配置那一问
    /// 一秒一次。出厂默认是 `enabled = false`，那一写法等于在整场会话里每 80 ms 复制一遍
    /// `[hot_corner]` 的表 —— 一件没人要求的事，而且它不会让角落更早生效到值得付。
    last_snapshot: Option<u64>,
    /// 「显示器读不出来」那一句说过没有。一台永远答不出的机器每秒会被再问一次，而这一族的唯一
    /// 输出就是历史：同一句话落十二行不叫读数。改口（读回来了）才是第二件事实，所以这一枚只在
    /// 两问之间翻转。
    missing_said: bool,
    /// §5.9.13 第二句那一条「启用时可显示短暂状态提示」要的那一枚**上一格的读数**：场上有几枚
    /// 贴图此刻正穿透鼠标。只有它**变大**的那一格才说一句（[`click_through_watch`]），所以这一枚
    /// 不是缓存、是判据的另一半 —— 没有它，每一格都会把同一句「已开启」飘到屏幕上。
    ///
    /// 为什么这一问长在采样腿上而不是在贴图自己的菜单那一手（`pin_view.rs` 的
    /// `apply_click_through`）：那一手是**窗**的一格手势，它改的是 `PinItem` 而一句话也不写
    /// `status`（本轮的边界不许动那一家），而这条腿是全进程里唯一既常驻、又不占 GUI 线程、
    /// 又肯每秒醒十二次的地方。§5.9.13 要的「启用时」那一眼，只有这里给得出。
    last_through: usize,
}

impl CornerPoll {
    fn new(plain: bool) -> Self {
        CornerPoll {
            plain,
            runner: None,
            screen: hot_corner::Fullscreen::Unknown,
            last_snapshot: None,
            missing_said: false,
            last_through: 0,
        }
    }

    /// §5.9.13 规则第一句 + 第二句在**启用**那一边的那一只手：数一下场上有几枚贴图正穿透鼠标，
    /// 与上一格比，只在**变多**的那一格把「已开启 + 怎么出去」那一句话交给 GUI 线程写进
    /// `status`。
    ///
    /// 屏幕上那一扇 [`qml/HintWindow.qml`] 绑的就是 `status`（`Connections` 里那条
    /// `onStatusChanged` → `shown = true` → `hold` 重新计时），所以这一格不造窗、不定时长，
    /// 只递那一句话；「短暂」那一格由那扇窗自己管（`anchor[5]`，出厂 2600 ms）。
    ///
    /// 出口那一列读 [`CLICK_EXITS`]，也就是 `install` 那一刻 [`click_through_exits`] 判出来的
    /// 那一份：一个入口都没成立时，那一句话照实说没有，而不是编一枚没注册的键（#433 不许静默，
    /// #281 不许第二份名单）。
    fn watch_click_through(&mut self, handle: &Gui) {
        let now = state::with(|s| s.set.items().iter().filter(|p| p.click_through).count());
        let prev = self.last_through;
        self.last_through = now;
        let Some(opened) = click_through_watch(prev, now) else {
            return;
        };
        let said = click_through_entry_hint(opened, &click_exits());
        if let Err(e) = handle.queue(move |mut session| {
            session.as_mut().set_status(QString::from(&*said));
            note(format!("GUI 线程 启用穿透：{prev} → {opened} 枚 → 已递出那一句怎么出去"));
        }) {
            note(format!("启用穿透 {prev} → {opened} 枚：GUI 线程没接住（{e}）"));
        }
    }

    fn tick(&mut self, handle: &Gui) {
        if !self.plain {
            // 一扇门借了这条泵的闹钟，不等于这一扇门的读数里该有角落。见 [`install`] 的 `plain`。
            // 上面那一格 `watch_click_through` 也一起不做：`status` 在量具门里是给判据用的，
            // 采样腿往里写一句「已开启穿透」就会污染那一档正在读的那一行。
            return;
        }
        // 这一问排在角落那三问**之前**，因为它要的只是名册里那一格布尔，而下面那条腿在
        // `[hot_corner] enabled = false`（出厂默认那一侧）时会整个 `return`。§5.9.13 那一句
        // 「启用时可显示短暂状态提示」不该跟着热区那一族的开关一起关掉 —— 两件事没有因果关系。
        self.watch_click_through(handle);
        let now = delay::clock_ms();
        let due = self
            .last_snapshot
            .is_none_or(|at| now.saturating_sub(at) >= hot_corner::SNAPSHOT_MS);
        if due {
            self.last_snapshot = Some(now);
            self.refresh_snapshot();
        }
        let Some(runner) = self.runner.as_mut() else {
            return;
        };
        let point = platform_windows::proc::cursor_pos();
        let sample = point.map(|p| (p, platform_windows::proc::modifiers_down()));
        if let Some(firing) = runner.poll(self.screen, sample, now) {
            let what = format!("热区 {}", firing.corner.key());
            let Some(work) = Work::of(&firing.action) else {
                // core 只把档里认过的动作名递出来，所以走到这里就是名单与路由表脱了钩：
                // 说出来当证人，而不是替用户动一枚别的什么手。
                note(format!("{what} {}：没有归宿，不动手", firing.action));
                return;
            };
            dispatch(&what, work, handle);
        }
    }

    /// 重问那三问，并按需要建/换机器。只在**真的换了**的时候往历史落一行。
    fn refresh_snapshot(&mut self) {
        let cfg = crate::settings::hot_corner_cfg();
        if !cfg.enabled {
            if self.runner.is_some() {
                note("热区：这一场把 [hot_corner] enabled 关了，机器放下".to_string());
            }
            self.runner = None;
            self.screen = hot_corner::Fullscreen::Unknown;
            return;
        }
        let monitors = match capture::monitor_table() {
            Ok(m) => m,
            Err(e) => {
                // 读不出显示器就没法算出角落方块。这不是「角落坏了」，是「这一问没答」：
                // 落下一次证人，然后让每一格的指针那一问自然作废 —— 一次，不是每一格一次。
                if !self.missing_said {
                    self.missing_said = true;
                    note(format!("热区：显示器读不出来（{e}），这一场不建机器"));
                }
                self.runner = None;
                self.screen = hot_corner::Fullscreen::Unknown;
                return;
            }
        };
        self.missing_said = false;
        // 一次问，两枚读数都用它：问两遍就是拿两次可能不一致的答案拼一个判断。
        let foreground = platform_windows::proc::foreground_window();
        self.screen = hot_corner::fullscreen_of(
            foreground,
            foreground.and_then(platform_windows::proc::window_rect),
            &monitors,
        );
        match self.runner.as_mut() {
            None => {
                let (count, screen) = (monitors.len(), self.screen);
                let runner = Runner::new(cfg, monitors);
                let (dwell, guard, pause) = (
                    runner.config().dwell_ms,
                    runner.config().repeat_guard_ms,
                    runner.config().pause_on_fullscreen,
                );
                note(format!(
                    "热区：建机器，{count} 屏，dwell={dwell} ms，护栏={guard} ms，\
                     全屏暂停={pause}，全屏={screen:?}"
                ));
                self.runner = Some(runner);
            }
            Some(runner) => {
                if runner.refresh(cfg, monitors) {
                    note(format!("热区：快照换了，机器重开第 {} 次", runner.rebuilds));
                }
            }
        }
    }
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

/// §5.4.4 / §5.4.5 的开板：与 [`open_mask`] 同一副形状，只有像素的来历不同。
///
/// 两条腿各印各的那一句而不是共用一句，是因为「开了几屏」在这一枚门上问的是**板子铺到了
/// 几块屏**，而 `freeze ms` 说的是那把尺子（[`mask::open_board`] 为什么还要冻一次画面）。
/// 失败那一支同形：板子开不起来的话，屏幕上头什么都没有，而这句话就是唯一的面。
pub(crate) fn open_board(tag: &str, mut session: Pin<&mut session::qobject::Session>) {
    match mask::open_board(SHADER) {
        Ok(opened) => {
            session.as_mut().refresh_masks();
            note(format!(
                "GUI 线程 {tag}：白板开了 {} 屏，freeze {} ms",
                opened.slots, opened.ms
            ));
        }
        Err(e) => note(format!("GUI 线程 {tag}：白板开不起来：{e}")),
    }
}

/// §5.8.3 那三步在拥有 Qt 的这一条线程上的那一趟：开对话框（第 1 步的托盘项已经按下）→
/// 用户挑文件（第 2 步）→ 一批读成贴图（第 3 步）→ 屏幕上那一行由那一批自己说。
///
/// 三样东西各有各的作者，这一手只把它们接起来，一个字也不重写：对话框本体是
/// [`platform_windows::pick`]（只做「取路径」），收哪几种格式 / 一次收几枚 / 坏的那几枚怎么
/// 归账 / 那一句话怎么说，全是 [`falcon_core::pin::read_picture_batch`] 与
/// [`falcon_core::pin::FilePinBatch`] 那一批的事（core 那边有单测）；读盘与拒绝那四句话仍是
/// [`crate::state`] 那一处（#281）。这一层只剩两笔：现问一次指针、把账念到 `status` 上。
///
/// 两条硬规矩在这一手上是判得出的：
///
/// 1. **取消不是一句静默**。[`platform_windows::pick::Answer::Cancelled`] 走的是**空列表**那一条腿，
///    于是屏幕上那句出自 [`falcon_core::pin::FilePinBatch::said`] 的空账（「没有选中文件，一张
///    也没有贴。」），而不是这里编的第二句 —— 也绝不是「什么都不写」。#433 在这一档的形状就是
///    一枚点下去屏幕上不动一下。
/// 2. **一枚也没挑成也不许只留一句散文**：`failed` 与 `declined` 里每一枚都单独进一行运行历史，
///    因为屏幕上那一行只引前几枚（[`falcon_core::pin::FILE_PIN_SAID_MAX`]），多出来那些必须在
///    日志里数得出来，否则「34 枚里被上界挡下的那 2 枚」到了第二天就查无此事。
///
/// 模态那一条是这个形状存在的理由，也是它的代价：`pick` 阻塞这一条线程直到用户按下打开或取消
/// （[`platform_windows::pick`] 头顶写的「必须在拥有 Qt 的线程上被叫」），期间热键泵那条线程照常
/// 收 `WM_HOTKEY`，而 Qt 的事件循环停在这一次调用里 —— 所以这一臂不能挪到泵线程去跑，也不能在
/// `cargo test` 里被叫到（§9.1 第十五条：真对话框不许出现在单测里，「取路径」这一手因此留在桥外，
/// 单测从 [`crate::state::pin_pictures`] 那一层往上都不必碰它）。
fn open_picture_pins(tag: &str, mut session: Pin<&mut session::qobject::Session>) {
    let patterns = falcon_core::pin::picture_filter_patterns();
    let request = platform_windows::pick::Request {
        title: Work::OpenPicturePins.label(),
        filters: vec![platform_windows::pick::Filter {
            // 「图片」两个字是本层写的，那一串模式不是：七种格式住在
            // [`falcon_core::pin::picture_filter_patterns`]，改那一天只改一处。本层**只**给这一行
            // 过滤器、不给「所有文件」—— §5.8.3 那一列没有许过的东西不在这里递出去。
            name: "图片",
            patterns: patterns.as_str(),
        }],
        multi: true,
    };

    let paths = match platform_windows::pick::pick(&request) {
        Ok(platform_windows::pick::Answer::Picked(paths)) => paths,
        Ok(platform_windows::pick::Answer::Cancelled) => Vec::new(),
        Err(e) => {
            // 对话框开不起来（shell 的组件取不到、策略禁了它、或这一台压根不是 Windows）与
            // 「用户没挑」是两件事，所以这一臂单独一条腿：话由 `PickError` 自己说，这里只把它
            // 搬到 `status` 上，不替它编第二句。
            let said = e.to_string();
            session.as_mut().set_status(QString::from(&*said));
            note(format!("GUI 线程 {tag}：对话框开不起来 → {said}"));
            return;
        }
    };

    let batch = state::pin_pictures(&paths);
    // 每一枚没成的都要单独落一行，而这一行说的话**全部来自那一批自己**：`why` 是
    // [`crate::state::read_refusal`]（读坏的）或 [`falcon_core::pin::read_picture_batch`]
    // （超上界的）写好的那一句，两处的句子里都已经带上路径，所以这里递 `why` 而不是再拼一遍
    // 「{path}：{why}」—— 同一句话说两遍就是 #281。
    for miss in batch.failed.iter().chain(batch.declined.iter()) {
        note(format!("GUI 线程 {tag}：没有贴出来 {}", miss.why));
    }
    let said = batch.said();
    session.as_mut().set_status(QString::from(&*said));
    if batch.windows() > 0 {
        // 与 `pin_clipboard` 同一条腿：Rust 改完名册要把读数搬过桥，窗才会长出来。一枚也没有
        // 时不叫它 —— `refresh` 会让每一扇已有的窗把自己的那一份重求值一遍，而这一趟什么也没改。
        session.as_mut().refresh();
    }
    note(format!(
        "GUI 线程 {tag}：挑了 {} 枚，贴出 {} 扇，读坏 {} 枚，超上界 {} 枚 → {said}",
        paths.len(),
        batch.windows(),
        batch.failed.len(),
        batch.declined.len()
    ));
}

/// §5.12.4 第 1251 行那三个入口里的第三枚，在拥有 Qt 的这一条线程上的那一趟：现读那一份分组
/// 快照 → [`group_cycle`] 定这一按轮到哪一组 → 叫**已经存在**的那一只手把组切过去。
///
/// 一格模型也不拧：`off` 里每一组与目标那一组走的都是
/// [`crate::session::qobject::Session::toggle_group_shown`] —— 分组窗上「切换显隐」那枚钮叫的
/// 就是它（[`crate::group_window`] 头顶列的那六只手之一）。它自己会同步组内每一枚的
/// `group_hidden`、重写 `groups` 那一串字、并在那一格真的动了时发 `group_epoch`，所以这一层
/// 既不必数成员、也不必补发号 —— 补一遍就是第二份实现（#281）。
///
/// `status` 那一格在这一趟里会被写好几枚：session 那一只手每次切换都先落一句「已显示该分组」/
/// 「已隐藏该分组」，而那两句**不认组名**；本层在最后写一句完整的（[`group_switch_said`]），因为
/// §5.12.4 第 1253 行要用户看得见的那一件事是「哪一组在场」。屏幕那一侧读的是最后一格，
/// `qml/HintWindow.qml` 的 `onStatusChanged` 每写一次重新计时一次 —— 浮窗因此不会带着上一组的
/// 名字停在那里。
fn switch_group(tag: &str, mut session: Pin<&mut session::qobject::Session>) {
    let rows = state::with(|s| s.group_rows());
    let cursor = *held(&GROUP_CURSOR);
    let Some((at, off)) = group_cycle(&rows, cursor) else {
        let said = no_group_said();
        session.as_mut().set_status(QString::from(&*said));
        note(format!("GUI 线程 {tag}：{said}"));
        return;
    };
    *held(&GROUP_CURSOR) = Some(at);
    let target = &rows[at];
    // 什么也没改的那一支：目标已在显示中，而场上没有别的组要关。仍要有话（#433），仍不许叫
    // 那一手 —— 叫了就会把一组本来显示着的贴图闪一次隐藏再显示，而那一闪是这一按凭空造出来的。
    if off.is_empty() && target.2 {
        let said = already_on_said(target.1.as_str());
        session.as_mut().set_status(QString::from(&*said));
        note(format!("GUI 线程 {tag}：{said}"));
        return;
    }
    // 先关那些正显示的、再把目标切过来：顺序反过来的话，中间那一格里两组同时在场上，而
    // `group_epoch` 会跟着那一眼刷一次屏 —— 用户看到的是一次闪动而不是「换了一组」。
    let mut closed = 0usize;
    for idx in &off {
        if session.as_mut().toggle_group_shown(rows[*idx].0 as i64) >= 0 {
            closed += 1;
        }
    }
    let opened = if target.2 {
        // 目标本来就在显示中（这一按只关掉了别的组）：一句「切过来」不需要再动它，动了就把它关掉。
        1
    } else {
        session.as_mut().toggle_group_shown(target.0 as i64)
    };
    if opened < 0 {
        // 被拒时 session 那一只手已经把**它那一句原因**写进 `status` 了（`group_close` 那一腿），
        // 本层因此不再覆盖屏幕上那一格，只往历史里补一行这一趟的读数（#281：拒绝的话由拧模型的
        // 那一家说，不由这里编第二句）。
        note(format!(
            "GUI 线程 {tag}：切到第 {at} 组「{}」被拒（关掉 {closed} 组），屏幕上说的是那一只手自己的话",
            target.1
        ));
        return;
    }
    let said = group_switch_said(target.1.as_str());
    session.as_mut().set_status(QString::from(&*said));
    note(format!(
        "GUI 线程 {tag}：切到第 {at} 组「{}」，关掉 {closed} 组，场上 {} 组 → {said}",
        target.1,
        rows.len()
    ));
}

/// §5.16.1 第 1410 行那一枚「贴图编辑操作」：一枚也没被选中时的那一句拒绝。
///
/// 纯函数、与 [`no_group_said`] 同族 —— 「按下去而场上没有对象」这一问不需要桌面，而这一档里
/// 每一句要递给屏幕的话都得先在单测里被读一遍（#433 的形状不是「没做事」，是「没做事且没话说」）。
///
/// 它说的是**这一枚键的意思**而不是「贴图不存在」：名册那一句写的是「让**选中的**贴图进入标注
/// 编辑」，所以没选中是这一枚键的一种合法按下，拒绝它的那一句要把对象那一问答清楚 —— 与
/// [`Work::Solo`] 那一条「一枚也没选中时直接拒绝」同一个判据、同一个形状。
fn no_pin_selected_said() -> String {
    "贴图编辑没跑：此刻一枚贴图也没被选中，而这一枚键要的是「选中的那一枚」——屏幕上点中谁，编辑的就是谁。"
        .to_string()
}

/// 把那一块交进遮罩之后那句成功的话。数目、那块地方、开了几屏各是一个读数，缺一枚就少一件
/// 可核对的事；`count` 与 `rect` 出自**同一次锁**里的那一读（见 [`pin_edit`]），所以「几枚」与
/// 「哪一块」不可能一个是刚才那一批、另一个是现在这一批。
fn pin_edit_said(count: usize, rect: &falcon_core::geometry::PhysRect, slots: usize) -> String {
    format!(
        "贴图编辑：把这 {count} 枚占的那一块 {rect} 原样框成选区，遮罩开了 {slots} 屏——画下去的每一笔都算在这一块上。"
    )
}

/// §5.16.1「贴图编辑操作」在拥有 Qt 的这一条线程上的那一趟。
///
/// **次序与 [`crate::mask::MaskState::annotate_history_row`] 一模一样**（开遮罩在前、把洞交回去
/// 在后），差别只有一处：这里**不铺任何像素**。为什么可以不铺 —— 贴图本身就是一扇窗，[`mask::
/// open_from_freeze`] 冻下来的那一帧里已经有它此刻画在屏幕上的那一身（缩放、旋转、翻转、灰度、
/// 反色、裁剪、缩略图全都算），替它再铺一遍等于给同一张图造第二个作者，而那一位永远画不出
/// 「用户此刻看见的那一枚」。这也正是它与历史那一族的分工：那一头的图**不在场上**，所以非铺不可。
///
/// `refresh_masks()` 之所以可以排在交回洞**之前**（历史那一头要求排在之后），是因为那一头要等的
/// 是纹理、这一头什么都不等：窗一落地读的是那一刻的洞，而 `set_hole` 自己会 notify 每一扇已存在
/// 的窗 —— 与 [`crate::session::qobject::Session::history_reenter`] 那条腿同一判据。
///
/// 两枚读数出自同一次 `state::with`：先读名单、再读那一块，中间若被人点掉一枚，交出去的就是
/// 「3 枚」配着两枚的矩形。一次锁把这两问绑成一问。
fn pin_edit(tag: &str, mut session: Pin<&mut session::qobject::Session>) {
    let (count, rect) = state::with(|s| (s.selection_count(), s.set.selected_bounds()));
    let Some(rect) = rect else {
        let said = no_pin_selected_said();
        session.as_mut().set_status(QString::from(&*said));
        note(format!("GUI 线程 {tag}：{said}"));
        return;
    };
    // 与 Work::Capture 同一枚印章：这一按走的是同一条热路径，`--hotkeys` 与 `--mask` 量的因此
    // 是同一扇门，而不是两扇长得像的门。
    state::stamp("asked");
    match mask::open_from_freeze(SHADER) {
        Ok(opened) => {
            session.as_mut().refresh_masks();
            mask::with(|m| m.set_hole(rect));
            let said = pin_edit_said(count, &rect, opened.slots);
            session.as_mut().set_status(QString::from(&*said));
            note(format!("GUI 线程 {tag}：{said} freeze {} ms", opened.ms));
        }
        // 开不起来时屏幕上头本来就没有遮罩，这一臂因此**不**叫 `refresh_masks()`：那一手只是
        // 白造一遍窗。失败的那一句照旧要递到屏幕上（#433）。
        Err(e) => {
            let said = format!("贴图编辑没跑：遮罩开不起来（{e}），屏幕上什么也没变。");
            session.as_mut().set_status(QString::from(&*said));
            note(format!("GUI 线程 {tag}：{said}"));
        }
    }
}

/// §5.16.1 第十三名那一枚键的**跑**的那一半：现读档上 `[hotkey] command` 那一串字，交给
/// [`falcon_core::tasks::plan`] 判，判得过就把两枚串递给 [`platform_windows::open::dispatch`]，
/// 判不过就把 core 那一句原话递到屏幕上。
///
/// 本层只有三件事：取那一串字、递一只定位判据、把话说出口。切刀、引号、长度、控制字符那几道
/// 闸在这里一句也不重判（#281：判据住在 core 并且在那里有单测，这里再判一遍就是同一件事的两处
/// 主人，而漂移只会出在最坏的那一枚字符上）。递给 `plan` 的定位判据也不是新写的一只手 ——
/// [`falcon_core::sendto::locate`] 早就是本仓库「这个名字在不在这台机器上」的唯一主人
/// （§5.5.10 那一路问的正是它），原样递进去即可。在这里图省事递一枚「总是答有」的桩，就是把
/// §7.5 那句「外部应用调用和文件路径应经过校验」当装饰：闸还在，只是永远开。
///
/// **不铺像素、不动模型**：这一枚键的对面是一枚**别的**进程，本进程能说出口的最远只到「shell
/// 收下了这句请求」（[`platform_windows::open::dispatch`] 头顶那句 `Ok(())` 的定义）。所以屏幕
/// 上那句话由两半拼成、各归各的主人：判过的那一句是 [`falcon_core::tasks::Launch::sentence`]，
/// 没递出去时说的是 [`platform_windows::open::OpenError`] 自己那一句 —— 本层不替一枚 `SE_ERR_*`
/// 编解释。拒绝那一臂更一个字也不补：那一句自己就分得清「这一格没填」「这一条坏了」「这台机器
/// 上没有」。
///
/// 一笔形状上的账，写在这里而不是藏起来：这一臂**会真起一枚进程**，而起完了本进程关不掉它
/// （§10 第 38 项那条探针规矩管不了别人的窗）。`--hotkeys` 那一档按的是**注册成功的**出厂键，
/// 而 [`falcon_core::config`] 给 `custom_task` 留的默认是「一枚键也没配、档上那一格也空着」，
/// 所以量具跑到这一臂的前提是用户自己既配了键又写了命令 —— 那时候按下去该跑，正是他要量的那件
/// 事。把这一臂在量具里改成「只判不跑」会交回一句假话：屏幕上写着跑过了，而盘上什么也没发生。
fn run_custom_task(tag: &str, mut session: Pin<&mut session::qobject::Session>) {
    let command = crate::settings::hotkey_cfg().command;
    match falcon_core::tasks::plan(&command, falcon_core::sendto::locate) {
        Ok(launch) => {
            let judged = launch.sentence();
            match platform_windows::open::dispatch(&launch.exe, &launch.parameters) {
                Ok(()) => {
                    session.as_mut().set_status(QString::from(&*judged));
                    note(format!(
                        "GUI 线程 {tag}：{judged}（shell 已收下这一次请求）"
                    ));
                }
                Err(e) => {
                    let said = format!("{judged}——可这一次没递出去：{e}");
                    session.as_mut().set_status(QString::from(&*said));
                    note(format!("GUI 线程 {tag}：{said}"));
                }
            }
        }
        Err(refusal) => {
            let said = refusal.sentence();
            session.as_mut().set_status(QString::from(&*said));
            note(format!("GUI 线程 {tag}：{said}"));
        }
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
        // §5.4.4 / §5.4.5：托盘那一枚「白板」。与上面那一臂同一条腿、同一个 `asked` 戳，
        // 只是背后的像素是做的而不是抓的 —— 差别只在 [`mask::open_board`] 那一步，而门读的
        // 仍是同一句「开了几屏」，因为对面是同一套遮罩、同一套标注。
        Work::Whiteboard => {
            state::stamp("asked");
            open_board(&tag, session.as_mut());
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
        // §5.8.3 第 1 步那一枚托盘项的对面。整趟（对话框 → 那一批 → 屏幕上那一行）住在
        // [`open_picture_pins`]，理由与模态那一条都写在那一手的头顶；这一臂只把它请进这张路由表，
        // 于是「托盘上看得见这一行」与「这一行有归宿」自此是同一件事（#433 不许死行）。
        //
        // **这一臂跑在 GUI 线程上，正是那条规矩要的形状**：`run` 只由 [`dispatch`] 经
        // `handle.queue` 落到拥有 Qt 的那条线程，所以 `pick` 那一手 `Show` 阻塞的是 Qt 的循环
        // 而不是热键泵 —— 泵照常收 `WM_HOTKEY` 与托盘那一击（[`platform_windows::pick`] 头顶
        // 记的就是这一条）。挪到泵线程去跑会把整条链锁死，这也是这一臂不叫 `state::` 而叫一只
        // `session` 的手的原因。
        Work::OpenPicturePins => open_picture_pins(&tag, session.as_mut()),
        // §5.12.4 第 1251 行那三个入口里的第三枚。整趟（现读快照 → 轮到哪一组 → 叫那五只已有的
        // 手之一切过去）住在 [`switch_group`]，那一手头顶记着为什么本层一格模型也不拧。
        //
        // 它**不在托盘菜单那一列里**，这不是漏：`Work::Groups`（「图片分组」）那一行才是
        // §5.18.1 第 1437 行点名的那一枚，而把 `switch_group` 也列成一枚扁平的菜单项，读回来的
        // 会是「切一次」而用户以为自己在「选一组」—— 逐组那一列要的是 shell 那一头的子菜单
        // （`TraySpec.items` 是一列扁平的 `(id, 名)`，泵那一侧 `AppendMenuW` 一枚 `POPUP` 也不建，
        // 且菜单是 `install` 那一刻定死的），本轮没有那一件东西，记在报告里而不是冒充成有。
        Work::SwitchGroup => switch_group(&tag, session.as_mut()),
        // §5.16.1 第 1410 行那一枚「贴图编辑操作」，出厂 `ctrl+shift+e`。它开的是**遮罩**，所以
        // 与 [`Work::Capture`] 同一族：必须在拥有 Qt 的这一条线程上跑（`open_from_freeze` 要铺
        // 纹理、要 `show()` 窗），本层只剩两问 —— 选中了哪几枚、它们占桌面上哪一块。
        Work::PinEdit => pin_edit(&tag, session.as_mut()),
        // §5.16.1 第 1411 行那一枚「自定义命令行任务」。整趟（现读档上那一串字 → 判 → 递）住在
        // [`run_custom_task`]，那一手头顶记着本层只剩哪三件事、以及为什么这一臂可以真起一枚进程。
        //
        // 它**不在托盘菜单那一列里**，与 `switch_group` 同一枚理由：§5.18.1 那一列十行没有这一行，
        // 而 [`tray_menu`] 头顶「只接有归宿的」反读也成立 —— 有归宿不等于该长出一枚菜单项。
        Work::CustomTask => run_custom_task(&tag, session.as_mut()),
        // §5.11：`ctrl+shift+s` 自此真的有归宿（P135 之前这一枚动作名排在下面那张「未接线」
        // 名单里，按下去什么都没发生）。这一支不冻画面、不碰剪贴板、也不动成员，所以它没有
        // 任何一枚 `id=` 可交回：它交回的是**改了几枚**，而「留哪几枚」由此刻的名单答
        // （[`crate::state::PinState::solo_kept_table`] 那一条 `None` 臂）。一枚也没选中时 core
        // 直接拒绝，`n=0` 在那里是实话而不是「没做」——屏幕上出现的是那句拒绝，而 #455 要的
        // 就是这两件事不许读成同一个数。
        Work::Solo => {
            let n = session.as_mut().toggle_solo_selection();
            let kept = *session.solo_kept();
            note(format!("GUI 线程 {tag}：独显改动 {n} 枚，留 {kept} 枚"));
        }
        // §5.15：`ctrl+shift+p`（`toggle_pins`）自此真的有归宿。与 Solo 同一形状 —— 一枚键两态、
        // 无被点中的窗，交回的是**翻之后的全局态**（1 藏 / 0 显）而不是「改了几枚」：全局收起不进
        // core 的自身可见性，`ids()` 一枚也不会少，所以「几枚」在这里是误导性读数。`all_hidden`
        // 那格由 [`crate::session::SessionRust::refresh`] 在同一只手内写回，读它就是把「翻了没有」
        // 交给 qproperty 那一侧复核，而不是只信这只手的返回值。
        Work::TogglePins => {
            let flipped = session.as_mut().toggle_all_pins();
            let said = *session.all_hidden();
            note(format!(
                "GUI 线程 {tag}：全局收起翻成 {flipped}，qproperty 读回 {said}"
            ));
        }
        // §5.15 热区那五枚动作里的另外两枚。与 `toggle_pins` 同一读数形状（交回的是置完之后
        // 的全局态，而 `all_hidden` 那一格由同一只手内的 `refresh` 写回，读它就是把「置了没有」
        // 交给 qproperty 那一侧复核），但语义不同一处：角落只能「置」不能「翻」—— 一次扫过没有
        // 按下与抬起，若走 `toggle_all_pins`，同一个角落扫两次就会把刚收起的又放开，而用户要的
        // 是「隐藏」。所以这一对是两枚独立的 `Work`，不是 `TogglePins` 的别名（#316 的反面：
        // 两名一实才该合并，一实两名不该）。
        Work::ShowAllPins => {
            let put = session.as_mut().show_all_pins();
            let said = *session.all_hidden();
            note(format!(
                "GUI 线程 {tag}：全局收起置成 {put}，qproperty 读回 {said}"
            ));
        }
        // 与上一枚成对，恒交回 1（已收起）。
        Work::HideAllPins => {
            let put = session.as_mut().hide_all_pins();
            let said = *session.all_hidden();
            note(format!(
                "GUI 线程 {tag}：全局收起置成 {put}，qproperty 读回 {said}"
            ));
        }
        // §5.9.13 第三步那一条退出的落地。这一手不冻画面、不碰剪贴板、也不动成员，交回的是
        // **带走几枚**（与 Solo 同一读数形状：一枚键没有「被点中的那一枚窗」，见
        // [`Work::ReleaseClickThrough`] 那一段），而它落成的那一句就是规则第一句要的对面。
        //
        // 两半各有一只手，缺一不可：改模型（`click_through` 那一格）与**让窗重读模型**。后者
        // 走的正是 §5.15 那三族共用的同一枚号 —— 被解除的那些枚恰恰是收不到鼠标的窗，它们不会
        // 自己醒过来；不发号，屏幕上仍然穿得过去，而日志已经说了「已解除」，那就是一句假话。
        //
        // 记一笔形状上的债：这里直接走 [`crate::state::with`] 拧 core 那一批 `PinItem`，而不像
        // 上面三臂那样叫一只 `session` 的手。理由是这一轮的边界不许动 `session.rs` / `state.rs`
        // / `pin_view.rs`，而那三家里该长的那一只叫
        // `PinState::release_all_click_through`（core 那边 `PinItem::set_click_through` 头顶
        // 那句「the tray and the hotkey both read this back to turn it off again」等的就是这一手）。
        // 落进 `session.rs` 之后，`pins_fanout` 那一枚私有的收尾也能一并接走 —— 现在这里 inline
        // 的是它同一条腿的三个动作，那一处才是它的家（#281 意义上的第二份实现，记着，不藏着）。
        Work::ReleaseClickThrough => {
            let released = state::with(|s| {
                let mut done = 0usize;
                for p in s.set.items_mut() {
                    if p.click_through {
                        p.set_click_through(false);
                        done += 1;
                    }
                }
                done
            });
            let said = click_through_said(released);
            session.as_mut().set_status(QString::from(&*said));
            let next = *session.pins_epoch() + 1;
            session.as_mut().set_pins_epoch(next);
            note(format!(
                "GUI 线程 {tag}：解除穿透 {released} 枚，发 pins_epoch={next} → {said}"
            ));
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
        // §5.20.1 / §5.1.1：托盘那一枚「首选项」接的是**那一格**，不是这扇窗。Rust 这一路
        // 一次 `show()` 也不叫：谁造这扇窗，销毁就归谁（`main.qml` 的 `syncSettings`），而
        // 从这里替它 `show()` 就是同一件事的两处做。交回的是写之前与写之后各读一次的
        // qproperty —— 「托盘点了一下，屏幕上那扇窗该在了没有」这件事只有这一格答得出，
        // 而它是同步 notify，所以下一环 `syncSettings` 一定看得见。
        Work::Settings => {
            let before = *session.settings_open();
            session.as_mut().set_settings_open(1);
            let said = *session.settings_open();
            note(format!(
                "GUI 线程 {tag}：设置窗 settings_open {before} -> {said}"
            ));
        }
        // §5.3.5 第一步的那一扇「自定义截图」对话框，与上面那一臂同一分工：这一路只写「该不该有一扇
        // 窗」那一格，造与毁都在 `main.qml`。区别在起手：设置页读的是整份档，这一扇窗的五格里只有
        // 延时那一枚是持久键，所以递过去之前先由 [`crate::custom::seed`] 把「这一刻档里等几秒」填进
        // 第五格、前四格留空 —— 替用户填坐标是编一个他没写的数。
        Work::CustomCapture => {
            crate::custom::seed();
            let before = *session.custom_open();
            session.as_mut().set_custom_open(1);
            let said = *session.custom_open();
            note(format!(
                "GUI 线程 {tag}：自定义截图窗 custom_open {before} -> {said}（延时那格起自 [capture] delay_seconds）"
            ));
        }
        // §5.14.4 第 1309 行的那扇窗，与上面两臂同一分工：这一路只写「该不该有一扇窗」那一格，
        // 造与毁都在 `main.qml`。这一扇窗**不需要一次 seed()**：页上唯一的起手数是那份清单，
        // 而它每一次出生都现问 SQLite（[`crate::session::qobject::Session::historyRows`]），
        // 没有一格可以在这里替用户先填上。
        Work::History => {
            let before = *session.history_window_open();
            session.as_mut().set_history_window_open(1);
            let said = *session.history_window_open();
            note(format!(
                "GUI 线程 {tag}：历史记录管理窗 history_window_open {before} -> {said}"
            ));
        }
        // §5.12.3 / §5.12.4 第 1218 行那一扇「图片分组管理」窗，与上面两臂同一分工：只写
        // 「该不该有一扇窗」那一格，造与毁都在 `main.qml`。这一臂比它们多一只手 ——
        // 页上那一列的名字来自 [`crate::session::qobject::Session::groups`] 那一串字，而它只由
        // 分组那五只手与 [`crate::session::qobject::Session::refresh`] 写；托盘这一径上没有任何
        // 一只手改过模型，所以开之前先重读一次，否则真人从贴图菜单建好组再点托盘时，页上列的
        // 可能是**上一次**那一份快照（`refresh` 只搬读数，不改任何东西）。
        Work::Groups => {
            session.as_mut().refresh();
            let before = *session.group_window_open();
            session.as_mut().set_group_window_open(1);
            let said = *session.group_window_open();
            note(format!(
                "GUI 线程 {tag}：图片分组管理窗 group_window_open {before} -> {said}（清单先重读 session.groups）"
            ));
        }
        // §5.21.1 那四步里的前三步：第 1441 行那一枚按下 → 读**此刻**档上那两枚 `[about]` 键 →
        // 交给 core 那一层唯一的判决函数 → 产品自己那句话写进 `status`。这里一次也不写档。
        //
        // 「获取」自取回那一只手落地起**会**联网（[`platform_windows::fetch`]：档上那枚键写成
        // `http(s)` 地址时真的去取），因此这一臂不再自己跑完那一趟，只把它**起出去** ——
        // 一次托盘点击跑在 GUI 线程上，在它上面等一次最坏三段超时的网络取回，僵住的是桌上
        // 每一扇窗。屏幕上先看到 [`falcon_core::update::running_line`] 那一句（含 §6.3 那句
        // 「应明确提示网络请求」），判决那一句随后由投回来的那一步写进 `status`。
        //
        // 现问那一只手住在 [`crate::session::about_keys`] 与 [`crate::session::check_with`]：
        // 「关于与更新」页上那两枚钮（`updateAsk` / `updateOpen`）与这一臂起的是同一次检查、
        // 用的是同一份读数（#281）。对账那一行 `note` 的原文一个字没动，只是搬到了落地那一刻
        // （[`crate::session::update_landed`]）—— 它带的是这里的 `tag`，读的仍是「清单／通道／
        // 包里的版本号 → 那句判决」这四样。
        //
        // 每一次按下都现问一次档，与托盘那四枚鼠标键同一形状（[`tray_run_of`] 那边写的理由）。
        // 上面那一臂 Work::Settings 之类写的是「该不该有一扇窗」，这一臂写的是一件**起出去的事**：
        // 它没有窗要造、也没有模型要改，唯一交给屏幕的是那两句话 —— 先一句「在跑」，后一句判决。
        Work::Update => {
            // 这一臂的 tag 由 `work.tag()` 交回，而它不是 [`crate::update_auto::AUTO_TAG`]：托盘
            // 那一次按的是手指，它不该替用户把自动那一支往后按掉 `check_every_days` 天 —— 那一格
            // 由 [`crate::update_auto::stamped_by`] 单处回答，三条入口共用同一枚闸（#281）。
            session.as_mut().start_update_check(tag);
        }
        Work::Help => {
            // §5.21.2 那四行的入口：按下去不造窗、不改模型，只把四位作者**此刻**的枚数汇成一句
            // 话写进 `status`（与上一臂 Work::Update 同一形状：一句做完的事，屏幕上那扇浮窗绑的就是
            // 它）。四行里少一行，sentence 自己换成「那一行还没有内容」，这里不替它圆场。
            let counts = crate::help::Counts::now();
            let said = counts.sentence();
            session.as_mut().set_status(QString::from(&*said));
            note(format!(
                "GUI 线程 {tag}：帮助四行现取 工具栏 {}｜快捷键 {}｜命令行 {}｜引导 {}",
                counts.toolbar_tools, counts.hotkey_rows, counts.cli_params, counts.guide_pages
            ));
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
            Ok((id, _)) => said(
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

/// 本层最后一次交给泵的那句 tooltip，原样留着。
///
/// 它是**自己那一手的读数**，不是第二份运行状态：独显在不在、留几枚，永远由调用方从 core 那张
/// `Solo::kept` 表上现读现递（见 [`crate::state::PinState::solo_pin`]），这一格只回答「屏幕上那一枚
/// 图标此刻写的是哪一句」。留这一格为的是 §9.1 第十条之外还有一句真话要说：`Pump::set_tip` 投出的
/// 是消息，`apply_tip` 在另一条线程上把它 `take()` 走，本层没有任何把手能问回来，于是门要读的那一格里
/// 「这一层写了什么」只能由写的那一手自己记（与 `MaskState::history_list_last` 同一形状）。
static TIP_LAST: Mutex<String> = Mutex::new(String::new());
/// 外壳真的收下几枚键，供 Solo 那一手事后重写同一句话时不必再问一遍 `Started`（那一份在 `install`
/// 返回后就搬进 [`Launch`] 交出去了，不是本层的常驻读数）。
static TIP_KEYS: Mutex<usize> = Mutex::new(0);
/// 独显那一句里「留着几枚」以外的那半句。整句只有一个作者（[`tip_line`]），单测
/// `the_tip_sentence_has_one_author` 数的是这一串字在本文件里出现的次数。
const TIP_TAIL: &str = "菜单里有截图";
/// Win32 `NOTIFYICONDATAW.szTip` 那一格的宽度（`TCHAR[128]`，留一格给终止符）。`write_wide` 是按
/// UTF-16 单元**静默截断**的：句子写长了，用户看到的是前半句而不是任何一句报错，所以这一枚上限由
/// 单测钉住，不许哪天有人往 tip 上再挂一件事。
const TIP_CAP: usize = 127;

/// tooltip 那一句的**唯一作者**：`keys`＝外壳此刻真收下的全局热键枚数（§5.1.1「正确反映应用运行
/// 状态」），`solo`＝这一场在不在独显里、在的话留着几枚（§5.11 规则③「应显示当前处于 Solo 状态」）。
///
/// `None` 是那一场**没开**独显：这一句与本轮改动前逐字相同，`Some(kept)` 才添上独显那一截。为什么
/// 两档共用一枚函数而不是各拼一句：托盘图标是同一枚，两桩事实（几枚键待命 / 在不在独显）必须从同一
/// 只手写进同一格，否则后来那一次改写会把前一句抹掉 —— 退出独显时若只写「留着 0 枚」而忘了改回热键
/// 那一档，用户读到的就是一枚谁也没设过的图标（#281）。
fn tip_line(keys: usize, solo: Option<usize>) -> String {
    match solo {
        None => format!("Falconshot 正在运行（{keys} 个全局热键待命，{TIP_TAIL}）"),
        Some(kept) => format!(
            "Falconshot 正在运行（独显中：留着 {kept} 枚，其余已隐藏；{keys} 个全局热键待命，{TIP_TAIL}）"
        ),
    }
}

/// §5.11 规则③ 的那一手：把 core 那张 solo 表此刻的读数投到托盘图标上。
///
/// 它**不判断**独显在不在、也不数留几枚 —— 那两问的答案由调用方从 [`crate::state::PinState`] 递进来
/// （读的是 `solo_active` 与 `solo_keeps`，都不是本层养的第二份状态）。它只做本层能做的那一件事：
/// 拼句子（[`tip_line`]）、交给泵（`Pump::set_tip`）、把交出去的那一句与自己那一手的答复记进账。
///
/// 三种结果分开记（#453）：泵不在（`install` 没成，这一场压根没有图标）／投出去了（图标那一刻改没改
/// 归泵线程上的 `apply_tip` 判，本层读不到）——合成一句「改了」就是替没发生的事做证。
pub fn replay_tip(solo: Option<usize>) {
    let tip = tip_line(*held(&TIP_KEYS), solo);
    let sent = {
        let pump = held(&PUMP);
        match pump.as_ref() {
            Some(pump) => pump.set_tip(&tip),
            None => false,
        }
    };
    *held(&TIP_LAST) = tip.clone();
    note(format!(
        "托盘 tip 改写：{tip}（{}）",
        match (solo, sent) {
            (Some(kept), true) => format!("独显中，留着 {kept} 枚，已投给泵线程"),
            (None, true) => "退出独显，已投给泵线程改回原句".to_string(),
            (_, false) => "泵不在（install 没成），这一交不出去".to_string(),
        }
    ));
}

/// 本层最后一次交出去的那一句，逐字交回（[`TIP_LAST`] 头顶记的是为什么非得由写的那一手记）。
pub fn tip_last() -> String {
    held(&TIP_LAST).clone()
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

/// 一枚替手投出去的键，连着它按下那一刻的两件读数。
///
/// 并列成一枚 struct 而不是往 [`report`] 的名单后面再挂一枚布尔：`Legs` 头顶记的是同一条账
/// （那一格到第八枚参数时 clippy 会拒 8/7，而它说的是真话 —— 靠位置读的名单没人能核对调用方
/// 填的顺序），而 `Vec<(String, bool, bool)>` 就算过得去也是**两枚同型的布尔并排**：谁把
/// `posted` 与在场写反了，读数只会红得没道理。名字与答复成对、缺席写成话而不是零，这两条与
/// [`Legs`] 是同一个形状。
pub struct Press {
    /// 档上那一枚键的动作名，也是 `press_verdict` 那根针的前半。
    pub action: String,
    /// [`press`] 的答复：这一枚 id 在不在本进程的管辖权里。
    pub posted: bool,
    /// 这一枚键要的**对象**在不在场上，与不在的话缺席的是谁 —— 由 [`object_of`] 在投递之前
    /// 现读，所以这一读说的是「按下那一刻」而不是「报告这一刻」。
    pub object: (bool, String),
}

/// 替手投一枚键，并在投出去**之前**先读一次它要的对象。
///
/// 次序是读数的一部分：投递把工作排进 GUI 线程，而那一趟会改场上有没有遮罩、有没有选中。
/// 排在投递之后去读，读到的可能是自己这一发造成的那一身。
pub fn press_leg(action: &str) -> Press {
    let object = object_of(action);
    Press {
        action: action.to_string(),
        posted: press(action),
        object,
    }
}

/// 这一枚键要的对象在不在场上 —— 问的是**产品自己的那一判**，门不另抄一份「哪些句子算拒绝」。
///
/// 这一枚存在的理由是一格坏读数：`pin_edit` 场上没有选中的贴图时说的是「贴图编辑没跑：此刻一
/// 枚贴图也没被选中」，`custom_task` 档上没写字时说的是「这一格里还没写过任何命令」。这两句与
/// 成功那一句在形向上完全同款 —— 都是 `GUI 线程 {动作}：…` 一行、都不带 `press_verdict`
/// 认的那两个记号 —— 于是光读链路的门会把一次**拒绝**判成 Pass。给门补一张「拒绝长什么样」的
/// 字面名单就是给每一句话造第二个作者，而漂移只会出在最不像的那一句上；正确的接口是「这一枚键
/// 要什么」，而那一句产品已经写过一次了（`pin_edit` 读 `selected_bounds`、
/// `run_custom_task` 把整串命令递给 [`falcon_core::tasks::plan`]）。这里原样再问一次同样的
/// 判据，判据的主人还是那一处（#281），而 `object_cap` 只负责把「对象缺席时的那一次成功读数」
/// 改成 Blocked。
///
/// 名单外的动作一律 `(true, …)`：`capture` 那一族要的是屏幕、`paste_pin` 要的是剪贴板，而这两
/// 件本来就由 [`Clip::plant`] / [`Region::plant`] 各自那一读管着，`report` 里的 `setup_ok`
/// 说的就是它们 —— 这一枚不再插一手，免得同一件事有两处主人。
pub fn object_of(action: &str) -> (bool, String) {
    match action {
        // 两枚读数出自同一次锁（与 [`pin_edit`] 里那一读同一判据）：先读几枚、再读几枚被选中，
        // 中间被人点掉一枚的话，说出口的就是「场上 3 枚、选中 2 枚」配着一块根本不属于这 2 枚
        // 的地方。
        "pin_edit" => {
            let (pins, picked) = state::with(|s| (s.ids().len(), s.selection_count()));
            (
                picked > 0,
                format!("贴图编辑要的是「选中的那一枚」：场上 {pins} 枚、被选中 {picked} 枚"),
            )
        }
        "custom_task" => {
            let command = crate::settings::hotkey_cfg().command;
            // 递给 `plan` 的定位判据与产品那一臂是同一枚 `sendto::locate`：这里不铺像素也不起
            // 进程，只把那一位的答复读回来当证人 —— 读一次，两问（在不在、为什么不在）出自同
            // 一次判，免得一条两边都填得进的字幕把同一串命令判出两个答案。
            let plan = falcon_core::tasks::plan(&command, falcon_core::sendto::locate);
            let said = match &plan {
                Ok(launch) => launch.sentence(),
                Err(refusal) => refusal.sentence(),
            };
            (
                plan.is_ok(),
                format!(
                    "自定义命令要的是档上那一格写过字：command = {command:?}，core 的答复：{said}"
                ),
            )
        }
        other => (true, format!("{other} 这一枚键不要对象，屏幕与剪贴板那两件另有读数")),
    }
}

/// 菜单里那一枚项的 id：动作名 → [`Work`] → 那张路由表里对应一行的 id。
///
/// 纯函数，名单当参数进来 —— 「哪一项是这个动作」这一判断在门与产品两边都要用，而它一次都
/// 不该问 Win32；三臂 `Menu` / `Nothing` / `Unwired` 的答复尤其不该由一条真托盘来证明。
/// 这一枚存在的另一半理由是 [`press`] 那一枚查不了 `settings`：它按 `route.action` 查，而
/// `Work::Settings` 没有可绑定动作名（`action()` 是 `None`，见那一段），它在表里那一行的
/// `action` 是空串。按空串查会把所有菜单专属行都捞进同一格，所以这里按 `work` 查。
///
/// 私有到本模块：`Route` 是这一层的私有类型，把它写进 `pub(crate)` 的签名就是让外面看得见
/// 一个用不上的名字（编译器那条 `private_interfaces` 正是这一句）。
fn menu_id_of(action: &str, routes: &[Route]) -> Option<u32> {
    match tray_run_of(action) {
        TrayRun::Work(work) => routes
            .iter()
            .find(|route| route.work == work)
            .map(|route| route.id),
        TrayRun::Menu | TrayRun::Nothing | TrayRun::Unwired(_) => None,
    }
}

/// 替真人点一次托盘菜单里的那一项，交回的是**这一枚 id**（或这一发为什么没投出去的那句原因）。
///
/// 交回 id 而不是 `bool`，为的是下面那一格读数得有一根不会指错的针：note 行写的是
/// `菜单 id={id} `，而 `Work::Settings` 那一行的动作名是空串 —— 按名字找等于拿空串去
/// `contains`，它会命中**每一枚** `菜单 id=` 行。三种失败分开说，是因为「点了没反应」在这一档
/// 里永远不许静默（#433），而把它们压成一枚 `false` 就是把三种不同的缺席读成同一种。
pub fn press_menu(action: &str) -> Result<u32, &'static str> {
    let id = {
        let routes = held(&ROUTES);
        menu_id_of(action, &routes)
    };
    let Some(id) = id else {
        return Err("这个名字在这一层没有 work 归宿（menu / none / 名单外），或它压根不在表里");
    };
    let pump_cell = held(&PUMP);
    match pump_cell.as_ref() {
        // `Pump::press_menu` 自己还会再拒一次：这一枚 id 不在当前那枚菜单上（菜单是
        // `install` 时按当时的装配列出来的，替手不许投一枚真人点不出来的项）。
        Some(pump) if pump.press_menu(id) => Ok(id),
        Some(_) => Err("这一枚 id 不在当前那枚菜单上"),
        None => Err("泵不在（install 没成）"),
    }
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

/// §5.18.2 第四步的替手：向泵自己那枚消息窗口投一次「外壳会替这一击投的那一发」。
///
/// 与 [`press`] / [`press_menu`] / [`notify_setting_change`] 同一形 —— **窗口、图标、消息号、
/// 分类那一臂全是真的，只有手指是替的**。`false` 只有一种含义：泵不在（`install` 没成），
/// 与那一发投没投进队列在 `--hotkeys` 那格的读数里是分得开的。
pub fn press_tray_click(click: TrayClick) -> bool {
    let pump_cell = held(&PUMP);
    match pump_cell.as_ref() {
        Some(pump) => pump.notify_tray_click(click),
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

/// 一条读完了的链路，再过一遍「它要的对象当时在不在场上」。
///
/// 只管把 Pass 改下来，Fail 与 Blocked 一律原样交回，这一条界线是有对象的：`Fail` 在那一位眼里
/// 是「投出去了而链路吞了这条消息」，一件与场上有没有选中的贴图无关的缺陷 —— 拿一次缺席去盖住
/// 它，就是给缺陷找一个体面的借口。而 `Pass` 不同：对象不在时那一行**只可能**是拒绝（[`pin_edit`]
/// 与 [`run_custom_task`] 每一臂都说话，#433），把拒绝读成做成了是这一档唯一会读错的那一型。
fn object_cap(verdict: state::Check, held: bool) -> state::Check {
    if held || verdict != state::Check::Pass {
        verdict
    } else {
        state::Check::Blocked
    }
}

/// 门替机器做的两件事，和每一件自己的答复。
///
/// 并列成一枚参数而不是往 [`report`] 的名单后面再挂一格：那一格到第八枚时 clippy 会拒（8/7），
/// 而它说的是真话 —— 一份靠位置读的名单长过七格，就没有人能一眼核对调用方填的顺序。这两件本来就是
/// 同一族（都是「本进程替外面那一层投了一发」），名字与答复成对，缺席写成 `("", false)` /
/// `("", Err(..))` 而不是零。
#[derive(Clone, Copy)]
pub struct Legs {
    /// §5.19.1：门替系统播的那一发叫什么名字，以及那一发投出去了没有。
    pub setting: (&'static str, bool),
    /// §5.1.1 / §5.20.1：门替真人点的那一项叫什么，以及 [`press_menu`] 交回的是哪一件 ——
    /// 真投出去的那枚 id，还是这一发为什么没投出去的那句原因。
    pub menu: (&'static str, Result<u32, &'static str>),
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
/// `injected` is one row per key this door pressed, each carrying the two readings the
/// chain itself cannot give: whether this process had jurisdiction over that id, and
/// whether the object the key acts on was on the table at the moment of the press
/// ([`object_of`]). A `pin_edit` with nothing selected and a `custom_task` with an empty
/// 档 both answer out loud, and the door has no business reading either sentence as done -
/// the chain lines are printed either way, so the row still says how far the message got.
///
/// `legs` is the pair above: a run that neither broadcast nor picked anything passes the
/// two empty shapes, which each row reads as its own answer rather than as a zero.
/// 那一格自己等的那一枚线程：地板与步长都写在这里，而不是散在循环里。上限是这一档最长的等待，
/// 超了就照实把「等了多久」印进读数——读不到就是读不到，不能把一格降成一枚赌博。
const CLICK_WAIT_MS: u64 = 2000;
const CLICK_POLL_MS: u64 = 25;

pub fn report(
    launch: &Launch,
    asked_ms: i32,
    ran_ms: i32,
    injected: &[Press],
    clip: &Clip,
    region: &Region,
    legs: Legs,
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
    let named = per_action(&launch.rows);
    let counted = started.registered.len() + started.refused.len();
    let seen = to_be_seen(&launch.rows).len();
    rep.row(
        "census adds up",
        state::Check::from(counted == launch.attempts && named == launch.actions),
        format!(
            "配置里 {} 个动作，逐个有一行（读到 {named} 行，冲突 {} 行另计）；递出去注册 {} 个，\
             shell 答 registered={} + refused={}（合计 {counted}）；没要那把键的动作 {} 个\
             （未绑定/保留/未接线，它们的键留在系统那边）；§8.4 要用户看一眼的 {seen} 行——这一列的\
             身份写在各枚 `Row` 自己的 kind 上，不是这里按字面数出来的；门这一趟**不**把它写给屏幕\
             （`install` 里那一判 `plain`：量具档的 `status` 是给判据用的），所以这一格读的是行数，\
             屏幕那一格由产品本体那一趟落",
            launch.actions,
            launch.rows.len() - named,
            launch.attempts,
            started.registered.len(),
            started.refused.len(),
            launch.actions.saturating_sub(launch.attempts)
        ),
    );
    for row in &launch.rows {
        rep.note("install", row.said.clone());
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
                .map(|(id, label)| {
                    format!(
                        "{id}={label}→{}",
                        work_of(*id)
                            .map_or("没有归宿".to_string(), |work| work.label().to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join(" ")
        ),
    );
    rep.note(
        "menu not clicked",
        "这一档不开那枚菜单：TrackPopupMenu 对泵线程是模态的，点开它就是等真人，而这一桌面自 \
         P48 起的每一批读数都是 LockApp 在前的会话。自本轮起它**点得动**了 —— 下一行那一枚是\
         替真人选的一项（与 press 替一次键盘组合同一式），缺的仍是「一只手在屏幕上选中了它」\
         与 PRD §5.2.2 的左键那一次（Event::TrayLeft → 截图，同样不在能注入的范围里）"
            .to_string(),
    );

    // §5.1.1 / §5.20.1：托盘菜单里那一枚「首选项」第一次被**选中**（手指是替的）。三格各有
    // 对面的读数：投没投出去（本进程有没有管辖权）、链路走没走完（监听线程 + GUI 线程那两行
    // note）、以及桌面上到底多没多出一枚顶层窗 —— 第三格是本轮新添的那一层，因为它才是
    // 「用户看得见」的对面：`settings_open` 写了 1 只证明 Rust 点过头。
    let (menu_action, menu_pick) = legs.menu;
    // 三格读数各自一根针，而第一格是「这一发投出去了吗」：`Err` 的那句原因原样印出来，
    // 三种失败因此在这一行里读得开，不会被压成同一种缺席（#453）。
    let picked_id = menu_pick.as_ref().ok().copied();
    let tag = picked_id
        .and_then(work_of)
        .map_or(String::new(), |work| work.tag());
    // 链路那一格的针是 `菜单 id={id} `（带上那枚分隔空格）而不是名字：`Work::Settings`
    // 在表里那一行的动作名是空串，拿空串去 `contains` 会命中场上**每一枚** `菜单 id=` 行，
    // 于是别的项的链路会被读成这一项的。带空格的 id 才是唯一的。
    let key = picked_id.map_or(String::new(), |id| format!("菜单 id={id} "));
    let picked: Vec<String> = if key.is_empty() {
        Vec::new()
    } else {
        history
            .iter()
            .filter(|line| line.contains(&key))
            .cloned()
            .collect()
    };
    let queued = picked.iter().any(|line| line.contains("排队到 GUI 线程"));
    // 两道关里任何一道的答复（`gate` 交回的那半句总是以「不动手」收尾）。它是产品按 §5.3
    // 的设计**拒了**这一击，不是链路断了 —— 与 `press_verdict` 把机器拒冻记成 Blocked 同理。
    let gated = picked.iter().any(|line| line.contains("不动手"));
    // GUI 线程那一行不带 id（`run` 只认得 work），所以它由 tag 钉：`GUI 线程 {tag}：设置窗 …`。
    let landed = !tag.is_empty()
        && history
            .iter()
            .any(|line| line.starts_with("GUI 线程 ") && line.contains(&format!("{tag}：设置窗")));
    // 顶层窗普查从前是「一个瞬间」：报告落到这一格的那一刻只数一次。P176 的门里这一格在六代
    // 都数回 0 枚，而产品自己日志里那行 `settings_open 0 -> 1` 在 P174 与 P176 两代都写着——
    // 「窗开没开」与「普查数没数到」自此是两个问题。把瞬间换成有截止的等，并在交回读数时把场上
    // 标题含 falconshot 的那几枚顶层窗原样印出来：判的还是同一件事（那扇窗在不在屏幕上），
    // 收窄的是仪器。等满仍不在，就照旧判 FAIL，而这一次账里知道它等过、也知道屏幕上是什么。
    const WINDOW_WAIT_MS: u64 = 2_000;
    const WINDOW_POLL_MS: u64 = 50;
    let mut window_waited_ms = 0u64;
    let levels = loop {
        let found = crate::r13::parse_levels(&crate::session::shim::top_levels());
        if found
            .iter()
            .any(|top| top.title == crate::settings_view::TITLE)
            || window_waited_ms >= WINDOW_WAIT_MS
        {
            break found;
        }
        // P176 的这一格在这里睡。睡着的那一只手，正是它要等的那一支：`syncSettings()` 跑在
        // `settingsSync` 那枚 `interval: 0` 的 Timer 上，而本档整趟都跑在 `app.exec()` **返回
        // 之后** —— 没人跑腿，那枚 Timer 就永远排不上，等满 2000 ms 交回 0 枚说的不是「窗不在」，
        // 而是「这一格自己把页腿饿死了」。改成带着超时的 `processEvents`：截止与步长一字未动，
        // 判的那件事也一字未动，只是等待的对象从「睡着」换成「替这一线程跑腿」。Qt 6 的
        // `processEvents` 交回 void，所以这一格不印「跑了几枚事件」—— 环到底答没答应，读的是
        // 页那一侧自己递回来的那几行，不是本格自报的一个数。
        crate::session::shim::pump_events(WINDOW_POLL_MS);
        window_waited_ms += WINDOW_POLL_MS;
    };
    let onshelf: Vec<String> = levels
        .iter()
        .filter(|top| top.title.contains("falconshot"))
        .map(|top| top.title.clone())
        .collect();
    let drawn = levels
        .iter()
        .filter(|top| top.title == crate::settings_view::TITLE)
        .collect::<Vec<_>>();
    // 页那一侧自己递回来的两族行：`sync` 是 `main.qml` 那一支腿到没到（`born` 只有窗真被造出来
    // 才可能存在，所以它答不了「腿没跑」与「造出来是 null」这两种坏）。这本账在 `--hotkeys` 这一趟
    // 里没人 armed，所以每行末尾那枚 `@0ms` 就是它自己的意思：没有闹钟可减。
    let qml_lines: Vec<String> = crate::settings_view::seen()
        .into_iter()
        .filter(|line| line.starts_with("sync|") || line.starts_with("born|"))
        .collect();
    rep.row(
        "tray menu opens the settings window",
        if menu_pick.is_err() || gated {
            state::Check::Blocked
        } else {
            state::Check::from(queued && landed && !drawn.is_empty())
        },
        format!(
            "点名 {menu_action:?}：投递={:?}，链路 {} 行（排队到 GUI 线程={queued}，GUI 线程接住\
             ={landed}，过闸拒出={gated}），顶层窗普查里 {:?} 共 {} 枚{}（这一格等了 {} ms；场上\
             标题含 falconshot 的顶层窗 {} 枚 {:?}）；这一格等的每一步都替 GUI 线程跑一次事件环，\
             页那一侧递回 {} 行 {:?}；读数的最后一行 {:?}。\
             这一格分不清真手与替手：它证的是「菜单里那一项被选中之后屏幕上出现那扇窗」，缺的仍是\
             一只手在 `TrackPopupMenu` 里选中它",
            menu_pick,
            picked.len(),
            crate::settings_view::TITLE,
            drawn.len(),
            drawn
                .iter()
                .map(|top| format!("（hwnd={} visible={}）", top.hwnd, top.visible))
                .collect::<Vec<_>>()
                .join(""),
            window_waited_ms,
            onshelf.len(),
            onshelf,
            qml_lines.len(),
            qml_lines,
            picked.last().map(String::as_str).unwrap_or("")
        ),
    );

    // §5.18.2 第四步：四枚键各投一发真的 `TRAY_CALLBACK`（`lparam` 低 16 位是那枚鼠标消息号），
    // 判的是这一进到 [`route`] 之后、写下来的那一行说的是不是**档上这一枚键**那个动作。针由两
    // 块拼成：`托盘{label}` 是产品自己那句 `format!` 的形状，`{action}` 是 [`tray_cfg`] 现读的
    // 那份档 —— 两边都不是这一档另抄的名单，所以红了推不掉「门读的是别一份配置」。
    //
    // 这四发投在这一格里，而不是像六把键那样投在 `exec()` 之前，是本轮 A/B 两代量出来的次序：那
    // 两趟把四发行完（`capture` 那一支真开了遮罩），同一趟 `tray menu opens the settings window`
    // 那一格的顶层窗普查就从 P174 那一代读回的 1 枚变成 0 枚 —— `p176_hotkeys_A.txt` 与
    // `p176_hotkeys_B.txt` 各留一份现场，一枚不删。两格共读同一个场景时，谁后跑谁改写谁的读数；
    // 这与上面「菜单那一发排在六把键之前」那条注释立的是同一件事。所以这一格排在所有读场景的格子
    // 之后：它自己的那一行由泵那一层写、不需要 GUI 线程再跑一趟，而它也再不改变别人的读数。
    let live_tray = crate::settings::tray_cfg();
    let mut click_reads: Vec<String> = Vec::new();
    let mut posted_keys = 0usize;
    let mut hit_keys = 0usize;
    let mut held_keys = 0usize;
    let mut waited_ms = 0u64;
    for click in TrayClick::ALL {
        let action = live_tray
            .entries()
            .into_iter()
            .find(|(name, _)| *name == click.id())
            .map_or(String::new(), |(_, action)| action.to_string());
        let needle = format!("托盘{} {action}", click.label());
        if matches!(tray_run_of(&action), TrayRun::Menu) {
            held_keys += 1;
            click_reads.push(format!(
                "{}（{}）有意没投：这一枚键的动作是开那枚菜单，投出去就是给这一趟按下一只等不来的 \
                 手（`TrackPopupMenu` 对泵线程是模态的，见上面 `menu not clicked` 那格同一条纪律）",
                click.id(),
                click.label()
            ));
            continue;
        }
        posted_keys += 1;
        let sent = press_tray_click(click);
        // 投出去与那一行落到 HISTORY 之间隔着另一枚线程，所以这里等的是自己那一枚：等满这一枚键
        // 的那一行出现就走，出现不了就把「等过了多久」印出来，而不是把这一格降成一枚赌博。
        let mut lines: Vec<&String>;
        let mut fresh = crate::hotkey::history();
        let mut spent = 0u64;
        loop {
            lines = fresh
                .iter()
                .filter(|line| line.starts_with(&needle))
                .collect();
            if !lines.is_empty() || spent >= CLICK_WAIT_MS {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(CLICK_POLL_MS));
            spent += CLICK_POLL_MS;
            fresh = crate::hotkey::history();
        }
        waited_ms += spent;
        if sent && lines.len() == 1 {
            hit_keys += 1;
        }
        click_reads.push(format!(
            "{}（{}）投出={} 链路 {} 行 {:?}（等了 {} ms）",
            click.id(),
            click.label(),
            sent,
            lines.len(),
            lines.first().map(|line| line.as_str()).unwrap_or(""),
            spent
        ));
    }
    rep.row(
        "tray four clicks reach the arm",
        if posted_keys == 0 {
            state::Check::Blocked
        } else {
            state::Check::from(hit_keys == posted_keys && hit_keys > 0)
        },
        format!(
            "投出去 {posted_keys} 枚、有意不投 {held_keys} 枚、对上 {hit_keys} 枚（要求 = 投出去那 \
             {posted_keys} 枚、每枚恰好一行）：{}。针 `托盘{{那一击}} {{那一格的动作}}` 里，动作取自 \
             `tray_cfg()` 现读的档、名字取自 `TrayClick::label()`，两边都不是这一档另抄的名单；档上 \
             那四枚此刻是 {}——本档一字不改它（`--tray` 那一档拿三份 fixture 判规则，这里判的是真链路）。 \
             有意不投那一枚的理由与上面 `menu not clicked` 那格是同一条：`TrackPopupMenu` 对泵线程是 \
             模态的，点开它就是等一只手。这一格合计等了 {waited_ms} ms 等那一行从泵那一层落进链路，而 \
             这四发投在这里（所有读场景的格子之后）不是省事：A/B 两代把它们投在 `exec()` 之前，同一趟 \
             `tray menu opens the settings window` 的顶层窗普查就从 1 枚读成 0 枚——两格共读一个场景时 \
             谁后跑谁改写谁的读数。此刻事件环已经结束，所以本格证的是「那一击进到 `route`、按档上那一 \
             枚键写下了归宿」，而不是「有人按了图标之后屏幕上真的动了」：后者那一半由同一趟那六把键的格 \
             子替同一个动作名证过（`GUI 线程 capture：遮罩开了`），两半在同一次里同时对上仍欠，账里写着。 \
             缺的仍是一只真的手按在图标上；而 shell 那枚结构是 Version 0， \
             双击会连带送来一次 UP（见 `platform-windows/hotkey.rs` 那句注释）",
            click_reads.join(" ； "),
            live_tray
                .entries()
                .into_iter()
                .map(|(name, action)| format!("{name}={action}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
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

    for press in injected {
        let action = press.action.as_str();
        let setup_ok =
            (action == "paste_pin" && planted) || (action == "capture_repeat" && region_planted);
        let (verdict, lines) = press_verdict(action, press.posted, &history, setup_ok);
        // 两枚旋钮各管一件事，各有一位主人：`setup_ok` 管门自己铺过的那两件（剪贴板、区域），
        // 这一枚管这一枚键当下要的那个对象。后者只许把成功那一格压下来，Fail 原样过 ——
        // 界线写在 [`object_cap`] 头顶。
        let verdict = object_cap(verdict, press.object.0);
        rep.row(
            &format!("press {action}"),
            verdict,
            format!(
                "PostMessageW 的答复={}（WM_HOTKEY，只对本进程注册过的 id 有效）；按下那一刻的对象：\
                 {}；链路读数 {:?}。真按键要键盘驱动经 shell 投给这个窗口，这一档缺的就是那一步：\
                 注册是真的，手指是替的。对象不在时那一行写的只会是拒绝（产品每一臂都说话，#433），\
                 而拒绝在这一格读成 Blocked —— 缺的是那只手，不是这条链路",
                press.posted,
                press.object.1,
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
    let (setting_name, setting_sent) = legs.setting;
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

/// Which [`Work`] a menu id leads to, out of the one routing table. `None` = 这一枚 id 不在
/// 表里。
///
/// 它是表而非名字：调用方要的措辞由这一枚查回来再取（`menu routes` 那一格要 `label()`，
/// 而下面那一格「谁被点中了」要的是 note 行里出现的 `tag()`），因为按名字查会把同一枚
/// 路由表的第二个副本写进代码里（#281）。
fn work_of(id: u32) -> Option<Work> {
    held(&ROUTES)
        .iter()
        .find(|route| route.id == id)
        .map(|route| route.work)
}

#[cfg(test)]
mod tests {
    use super::*;
    use falcon_core::config::{AppExclusion, Config};
    use platform_windows::hotkey::{virtual_key, MOD_CONTROL, MOD_SHIFT};

    /// §5.21.2 那一枚名分的家。P158 之前这十一句中文写在 [`Work::label`] 里，设置页要递说明
    /// 就只能再抄一份；现在名与说明都住在 `config::ACTION_TEXTS`，这里读它。
    ///
    /// 下面那十四对是**改码之前**从这些字符串里逐字抄下来的原话，不是改完之后回填的读数 ——
    /// 改名要过这一格，因为托盘上那一项（`tray_menu` 里那两行 `work.label()`，第 400 / 402 行）
    /// 与按下之后运行日志里那一句排队（`dispatch` 那一行）印的就是这些字（#455 —— 措辞在这里
    /// 是读数）。P171 那一轮把 `History` 加进来（十二对）、P172 把 `Groups` 加进来（十三对）、
    /// 本轮把 `ReleaseClickThrough` 加进来（十四对），
    /// 每一枚都是「先有那件产品的事、名册才多一行」，不是先把名字抄进去再补一只 nonexistent 手。
    ///
    /// **早先的草稿在这里还写着两枚别的读者，两枚都没经现读，所以划掉**：`pin_check.rs` 那两格
    /// 比的是贴图自己那排菜单的字（含 `独显` 那一句，见该档 3122 / 3157 行），与一枚*动作*叫什么
    /// 无关；历史里那一行印的是 `rec.source`（`settings::history_rows_joined`），压根没有动作名。
    /// 与下面 `the_config_side_dictionary_is_the_table_too` 那一格的自注释同一族：一句话替别人家
    /// 认领一枚读者，就再造一份没人核对的名册。
    #[test]
    fn every_work_is_named_by_the_one_table() {
        let want = [
            (Work::Capture, "截图"),
            (Work::ActiveWindow, "活动窗口截图"),
            (Work::Repeat, "重复上次截图"),
            (Work::PastePin, "贴出剪贴板"),
            (Work::Solo, "独显"),
            (Work::TogglePins, "隐藏/显示全部贴图"),
            (Work::ShowAllPins, "显示全部贴图"),
            (Work::HideAllPins, "隐藏全部贴图"),
            // §5.9.13 第三步那一枚「取消穿透」。它的名与说明都不在本行：与上面八枚同一张表。
            (Work::ReleaseClickThrough, "解除鼠标穿透"),
            // §5.12.4 第 1251 行那三个入口里的第三枚。自 P186 起它有手（[`switch_group`]），
            // 而它的名字早就在 `ACTION_TEXTS` 里 —— 这一对判的是「接上的那一天名与说明不必再改」，
            // 因为名册那一行写的本来就是它要做的那件事。
            (Work::SwitchGroup, "切换分组"),
            (Work::Whiteboard, "白板"),
            (Work::Settings, "首选项"),
            (Work::History, "历史记录"),
            // 这一枚的名不在本行：它住的是 [`crate::group_window::TRAY_WORD`]，本格判的是
            // 「托盘那一项递出来的字」与「门里 ⑪ 那一格找的字」是同一枚（#281）。
            (Work::Groups, "图片分组"),
            (Work::Quit, "退出"),
        ];
        assert_eq!(want.len(), 15, "Work 的臂数变了，十五对得重数一遍");
        for (work, name) in want {
            assert_eq!(work.label(), name, "{work:?} 改名了");
            if let Some(id) = work.action() {
                // 有可绑定动作名的那八臂，名必须与那张表逐字相同 —— 分家就是同一个动作两个名。
                assert_eq!(name_of(id), name, "{id} 的名与 {work:?} 的名分家了");
            } else {
                // 托盘专属那一族（有手、能落到一件产品的事上，但 §5.16.1 那张表里没有它的名字）：
                // `Whiteboard` / `Settings` / `Quit` / `History`，以及自 P172 起的 `Groups` ——
                // 这一枚的手是 [`crate::group_window`] 那扇窗，所以「没有动作名」不是漏了一行。
                assert!(
                    matches!(
                        work,
                        Work::Whiteboard
                            | Work::Settings
                            | Work::Quit
                            | Work::History
                            | Work::Groups
                    ),
                    "{work:?} 既没有可绑定动作名、也不在本档认下的托盘专属那一族里"
                );
            }
        }
        // 托盘独有的那两枚写法（不在 §5.16.1 十二名里）也从同一张表读名；`Work::Settings` 走
        // 的就是这一枚，它不该在 label 里另留一份「首选项」。
        assert_eq!(name_of("settings"), "首选项");
        assert_eq!(name_of("menu"), "托盘菜单");
        assert_eq!(name_of("none"), "不绑定");
        // 名单外的名字交回 **id 本身**：空串在菜单上是一枚看不见的名，原样印出来才看得出漏了一行。
        assert_eq!(name_of("capture_thing"), "capture_thing");
    }

    /// §5.18.1 那张菜单表用的**第二本名册**，本轮也交还给同一张表。
    ///
    /// `config::Tray::label` 从前自己写着八句中文，其中三句与表里同名的几枚**不一样**
    /// （`paste_pin` 那里叫「贴图」、这里叫「贴出剪贴板」；`settings` 是「打开设置页」对「首选项」；
    /// `none` 是「不做任何事」对「不绑定」），而名单只有八枚 —— 其余四枚落进 `other` 那一臂，
    /// 交回英文 id 本身。它在整个仓里只有一枚读者：`--tray` 那份报告里的 `tray labels` 一行
    /// （`tray_click_check.rs` 第 373 行），而且改码之前**没有任何一格断言比过它**，所以这一族
    /// 分歧不会红，只会让同一份证词与同一只托盘各叫一个名。
    ///
    /// 这一格走的是两条不同的路再合到一处：配置那一层拿 id 直接问表，装配那一层拿 `Work` 反查
    /// id 再问表。这里**不**与 `ACTION_TEXTS` 直接比 —— 比完就是把「读同一张表」当成证据，
    /// 而它在那一刻是同一枚表达式的两个写法（#281 的反面：断言不许只是重说一遍实现）。
    #[test]
    fn the_config_side_dictionary_is_the_table_too() {
        use falcon_core::config::{Tray, TRAY_ACTIONS};
        let mut with_work = 0usize;
        for id in TRAY_ACTIONS {
            let labeled = Tray::label(id).to_string();
            match Work::of(id) {
                Some(work) => {
                    assert_eq!(
                        labeled,
                        work.label(),
                        "{id}：配置那一层与托盘上那一项各叫一个名"
                    );
                    with_work += 1;
                }
                // 托盘独有那三枚没有 `Work`（见 `tray_run_of` 的自注释），所以这一路只能问表
                // 认不认得它：认不得就会把英文 id 印进证词。
                None => {
                    assert_ne!(labeled, *id, "{id}：表里没有这一枚，页上会露出英文 id");
                }
            }
        }
        assert!(with_work > 0, "一枚 `Work` 也没比到 = 这一格没有对面");
        // 名单外仍然原样交回：这一格比的是「不起一枚假名」，不是「一定有名字」。
        assert_eq!(Tray::label("capture_thing"), "capture_thing");
    }

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

    /// 托盘那句 tip 的**作者**只有一个，而独显那一截说的确实是独显。
    ///
    /// 三条判据全在 headless 里跑（§9.1 第十条），因为分岔本体是 [`tip_line`] 这一枚纯函数：
    /// 扫源码那一格钉「谁拼这一句」（两处各拼一句就是两个作者，而「退出独显时把热键那一档抹了」
    /// 正是这么来的），后两格钉这一句**说什么**。图标那一刻改没改归泵线程上的 `apply_tip` 判，
    /// 本层没有把手能问回来，所以这一格不判它 —— PRD 口径里也照实这么记。
    #[test]
    fn the_tip_sentence_has_one_author() {
        // 只看装配代码：本测试模块自己那份字面不许算进作者名单（算进来就是判据被证人污染）。
        let src = include_str!("hotkey.rs")
            .split("mod tests")
            .next()
            .expect("装配代码里没有 mod tests 这一行");
        let body = src
            .split("fn tip_line(")
            .nth(1)
            .expect("装配代码里没有 tip_line 这一枚函数")
            .split("\n}\n")
            .next()
            .expect("tip_line 没有尾巴");
        let needle = "Falconshot 正在运行（";
        assert!(
            !body.is_empty() && src.matches(needle).count() == body.matches(needle).count(),
            "tip 那句被装配代码里的第二处拼了：那一处不许存在（#281）"
        );

        // 没开独显 = 与接上这一手之前逐字相同的那一句，一个记号也不添。
        let off = tip_line(9, None);
        assert!(off.starts_with("Falconshot "), "{off}");
        assert!(
            off.ends_with("9 个全局热键待命，菜单里有截图）"),
            "热键那一档的字面与枚数都要在：{off}"
        );
        assert!(!off.contains("独显"), "没开独显时不许提独显：{off}");

        // 开着独显 = 添独显那一截，而热键那一档**仍在**：两桩事实写进同一格，后来那一次改写
        // 不许盖掉前一次（§5.1.1 与 §5.11 规则③ 共用同一枚图标）。
        let on = tip_line(9, Some(2));
        assert!(on.contains("独显中"), "§5.11 规则③ 要的就是这一截：{on}");
        assert!(on.contains("留着 2 枚"), "留几枚的数得跟着读数走：{on}");
        assert!(on.contains("9 个全局热键待命"), "独显那一句不许抹掉热键：{on}");

        // 长度：`win::write_wide` 按 UTF-16 单元**静默截断**，句子胖过 `szTip` 那一格就是用户
        // 读到半句而没有任何一处报错（#433）—— 所以这一枚上限由单测钉住，不许哪天有人再往
        // tip 上挂第三件事。按最坏的读数（四位数）量。
        let fat = tip_line(9999, Some(9999));
        assert!(
            fat.encode_utf16().count() <= TIP_CAP,
            "tip 长过 szTip 那一格（{} 单元 > {TIP_CAP}），会被静默截断",
            fat.encode_utf16().count()
        );
    }

    /// §5.11 规则③ 那半句常驻读数的**接线**，两条各在 headless 里问：
    /// ① 写者只有一处、且恰好覆盖独显的两支（进入与退出）—— 挂在 [`crate::state::PinState::solo_pin`]
    ///    的尾巴上，于是菜单那一支与 `ctrl+shift+s` 那一支共用同一条尾巴，而独显期间没人再动它
    ///    也不会读到一句过期的话（这一格挡的是「只在其中一条入口改 tip」那一族缺陷：两条入口
    ///    各自改一遍就是两个作者，漏改一条就是图标与屏幕各说一套）；
    /// ② 递下去的是 core 的读数而不是桥侧新存的布尔（`Some(self.solo_kept())` 那一枚），
    ///    泵不在时账上说的是「泵不在」而不是「改了」。
    #[test]
    fn the_solo_hand_is_the_only_writer_of_the_tip() {
        let state = include_str!("state.rs");
        let at = state
            .find("pub fn solo_pin(")
            .expect("solo 那一手不叫 solo_pin 了？");
        let (head, tail) = state.split_at(at);
        assert!(
            !head.contains("crate::hotkey::replay_tip("),
            "solo 之外还有一处在改写 tip：那一处就是第二份 solo 真相"
        );
        let calls = tail.matches("crate::hotkey::replay_tip(").count();
        assert_eq!(
            calls, 2,
            "进入与退出两支各要恰好一枚改写，读到 {calls} 枚"
        );
        assert!(
            tail.contains("Some(self.solo_kept())") && tail.contains("self.set.solo_active()"),
            "递下去的必须是 core 那张表的读数（solo_active / solo_kept），不许是桥侧新存的一枚布尔"
        );

        // 泵不在（这一场压根没 install 成，`cargo test` 里就是这样）：这一交不出去，而账上必须
        // 这么说 —— 不静默（#433），也不替没发生的事做证（#261）。
        replay_tip(Some(7));
        assert!(
            tip_last().contains("留着 7 枚"),
            "本层写出去的那一句读不回来：{}",
            tip_last()
        );
        assert!(
            history()
                .iter()
                .any(|l| l.contains("托盘 tip 改写：") && l.contains("泵不在")),
            "泵不在时那一句没记账"
        );
    }

    /// §5.9.13 规则第一句「必须提供明确且可靠的退出穿透方式」在这一张路由表上的那一判。
    ///
    /// 三趟各问一次，问的都是**装配**而不是这一档的措辞：
    /// ① 出厂默认档 ⇒ 成立的入口只有「托盘菜单」。这一格是**读数**而不是判据：出厂不占一枚键的
    ///    两样理由写在 `config::Hotkey::default` 旁边，而这里把那一句印出来，是为了让报告里
    ///    「开箱那一条退出走的是托盘」这句话有一个证人，不靠注释里的一句愿话。同一趟顺手问掉
    ///    一枚只在名册上的空名：`routes` 里**有**这一枚工作（托盘那一项要有归宿，#433 不许死行），
    ///    `requests` 里**没有**它 —— 只有前者是「配得上却什么都不做」那一族，所以 [`click_through_exits`]
    ///    的快捷键那一半要两件事同时成立，这一趟就是那两件事分开的现场。
    /// ② 有人把 `release_click_through` 配上一枚键（§5.16.1 第二行「用户自定义组合键」那一手）
    ///    ⇒ 两枚入口同时成立。
    /// ③ 把 ② 的路由表里那一枚工作抹掉 ⇒ 快捷键那一半必须**不**成立。这是 ② 的那一枚条件反着
    ///    走：那把键真的从 shell 手里要来了，而这一枚工作没有手应答，它落在 `dispatch` 的
    ///    「没有归宿」分支上 —— 拿了一串键而没人应答，不是一条退出。这一趟手工改 `plan()` 交回的
    ///    那份 `Plan`，因为正常装配装配不出它（`plan` 只会为它认得的工作发路由）。
    ///
    /// 三趟都不问 Win32、不问屏幕：为的是 §9.1 第十条那一族 headless 的半边。
    #[test]
    fn the_click_through_exit_never_needs_a_click_on_the_pin() {
        // ① 出厂：托盘成立、快捷键不成立，而名册上那一枚工作并没有偷偷配好一只假手。
        let mut shipped = plan(&Config::default().hotkey.actions);
        tray_menu(&mut shipped);
        assert_eq!(
            click_through_exits(&shipped, true),
            vec!["托盘菜单"],
            "开箱那一条退出：§5.9.13 第三步点名的三枚入口里，此刻真成立的只有托盘菜单"
        );
        assert!(
            shipped
                .routes
                .iter()
                .any(|route| route.work == Work::ReleaseClickThrough),
            "托盘菜单上那一行没有归宿 = 死行（#433）"
        );
        assert!(
            !shipped
                .requests
                .iter()
                .any(|request| request.action == "release_click_through"),
            "出厂默认替这一枚动作占了一枚键 —— 名册那一头的读数注释要跟着改"
        );

        // ② 有人配了键：两枚入口同时成立。
        let mut bound = Config::default().hotkey.actions;
        bound.insert(
            "release_click_through".to_string(),
            "ctrl+shift+t".to_string(),
        );
        let mut planned = plan(&bound);
        tray_menu(&mut planned);
        assert_eq!(
            click_through_exits(&planned, true),
            vec!["预设快捷键", "托盘菜单"],
            "配上手之后，§5.9.13 第三步点名的两枚入口都要真成立"
        );
        // ②b 同一份 `Plan`、把托盘换成「没挂上」（`[general] show_tray = false`）⇒ 托盘那一半
        //     必须跟着倒下去。这一枚布尔是新加的第二把闸，不递它一次 `false` 就等于没判过它。
        assert_eq!(
            click_through_exits(&planned, false),
            vec!["预设快捷键"],
            "图标压根没挂上时，把菜单上那一行算成一条退出就是把一件不存在的事说成救生门"
        );

        // ③ 只有注册、路由表上没人应答：快捷键那一半必须倒下去，而不是跟着亮。
        let mut orphan = plan(&bound);
        orphan.routes.retain(|route| route.work != Work::ReleaseClickThrough);
        assert!(
            orphan
                .requests
                .iter()
                .any(|request| request.action == "release_click_through"),
            "这一趟要问的是「拿了键而没有人」，先把键留下"
        );
        assert!(
            !click_through_exits(&orphan, true).contains(&"预设快捷键"),
            "只有注册没有归宿的一串键被判成了一条退出"
        );
    }

    /// §5.9.13 那一句「**可靠**」里可判的那一截：一条退出不能被一件不相干的正开着的事
    /// 挡在门外。四臂各一判，而这一格压根不问遮罩开没开 —— 它问的是 [`mask_blocks`] 这一枚
    /// 分岔本体，`gate` 那一手要问 `mask::with`（一次真桌面的读数），单测跑不动。
    ///
    /// 为什么这一问值得单独成格：贴图一旦穿透，鼠标落不到它身上，能救它的只剩热键与托盘两条
    /// 不经过贴图的路；而这两条里任意一枚要是被「遮罩正开着」挡在门外，用户在遮罩后面看到的
    /// 就是一枚按了没反应的键 —— 与名册上压根没有这一枚动作是同一个结果，两种坏法。
    #[test]
    fn an_exit_is_not_blocked_by_something_else_being_open() {
        assert!(
            !mask_blocks(Work::ReleaseClickThrough),
            "解除穿透被遮罩挡在门外：那就不是一条可靠的退出"
        );
        assert!(
            !mask_blocks(Work::Quit),
            "`Quit` 那一半是原来的写法，一个字不许改语义"
        );
        // 反面对照：一次冻结会把将要盖住它的遮罩拍进图里（§5.3），所以截图仍该被挡。
        assert!(
            mask_blocks(Work::Capture),
            "截图被放开了 = §5.3 那一句「不许再冻一次遮罩」没人守了"
        );
        assert!(
            mask_blocks(Work::TogglePins),
            "隐藏/显示全部贴图仍该被挡：这一轮放开门的只有退出那一族"
        );
    }

    /// §5.9.13 规则第二句「可显示短暂状态提示」在**解除**这一边的句子。判的是屏幕上那一扇
    /// 早就长在身上的浮窗要的那一句字（`session.status` → `qml/HintWindow.qml`），两臂各一判：
    /// 一句不许撒谎（数目得在句子里，屏幕上说的是这一趟而不是上一趟），另一句不许沉默
    /// （#433：一枚也没有时按下去什么都不做而屏幕什么也不说，用户读到的是「这枚键坏了」）。
    #[test]
    fn the_release_says_how_many_it_took_or_says_none() {
        let some = click_through_said(3);
        assert!(some.contains('3'), "{some}：三枚得数得在句子里");
        assert!(some.contains("穿透"), "{some}：状态浮窗那一行要认得出是哪件事");
        let none = click_through_said(0);
        assert!(!none.is_empty(), "一枚也没有时那句是空串 = 静默");
        assert!(
            none.contains("没有"),
            "{none}：要说的是「场上压根没有穿透中的贴图」，不是「解除 0 枚」"
        );
        assert!(
            !none.contains('0'),
            "{none}：「解除 0 枚」把一件没发生的事说成了发生了"
        );
        // 两臂各说各的事，所以不是一句加一个数：这一条也是「句子住在一处」的那位作者自己核对。
        assert_ne!(some, none, "0 枚与 3 枚同一句 = 数目是装饰");
    }

    /// The wiring table, and the promise it keeps: an action this build cannot
    /// perform is left alone rather than taken over. `capture_custom` used to be the
    /// row that made the difference visible - a shipped default whose coordinates
    /// dialog (§5.3.5) did not exist. P166 built that dialog, so the row moved to the
    /// registered side; P186 moved `switch_group` with it, and this round moved the
    /// last two that stayed the user's —— `pin_edit` 与 `custom_task`。§5.16.1 那九行出厂
    /// 绑定自此**全**有对面，而这一格从「谁的键还留给系统」那一份名单，变成了「一枚不许漏」
    /// 那一份名单：名单空了，它才第一次真的能挡住下一枚漏网的。
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
                "capture_custom",
                "capture_repeat",
                "paste_pin",
                "pin_edit",
                "solo",
                "switch_group",
                "toggle_pins"
            ],
            "接线表变了"
        );
        assert_eq!(planned.routes.len(), 9, "路由表该和注册表一样长");
        assert_eq!(per_action(&planned.rows) + planned.requests.len(), actions.len());
        // 出厂那一列里自此一枚也没有「未接线」：任何一行冒出来，都是有人往默认表里添了一枚
        // 还没归宿的动作名 —— 那正是要拦的那件事（一把被壳 hold 住、没人应答的键）。
        assert!(planned.rows.is_empty(), "出厂绑定里还有一行没归宿：{:?}", planned.rows);
        assert_eq!(planned.next, 10, "id 从 1 连续发到注册的下一个");
        // `solo` 走出这一列靠 P135、`toggle_pins` 靠 P137、`capture_custom` 靠 P166 那扇对话框窗
        // （§5.3.5）、`switch_group` 靠 P186 那一手（§5.12.4 第 1251 行那三个入口里的第三枚）、
        // `pin_edit` 靠 [`Work::PinEdit`]（把洞交回选中的那几枚占的那一块，不铺像素）、`custom_task`
        // 靠 [`Work::CustomTask`]（判归 [`falcon_core::tasks`]，跑归 `open::dispatch`）。
        let unwired: Vec<&str> = planned
            .rows
            .iter()
            .map(|row| row.said.split(' ').next().unwrap_or_default())
            .collect();
        assert!(unwired.is_empty(), "还有出厂键没有归宿：{unwired:?}");
        // 出厂那一趟屏幕上**不该**出现 §8.4 那一行：九枚键全成，一列是空的。这一判读的是
        // `to_be_seen` 的行数而不是「有没有人写过一句道歉」，所以它同时钉住反方向 —— 哪天
        // 有人把 `Taken` 也交给屏幕，这一格当场红。
        assert_eq!(to_be_seen(&planned.rows).len(), 0, "出厂键里有一枚要用户看");
    }

    /// §5.16.1「可绑定范围」那十三名，**每一枚**都要有一只手。
    ///
    /// 这一格与上面那一格问的不是同一件事，两枚都要留着：上面那一格问的是「出厂那九枚键有没有
    /// 归宿」（读数 = `Config::default().hotkey.actions`，改默认表就会变），这一格问的是「名册上
    /// 有没有一枚名字是空的」（读数 = [`falcon_core::config::ACTIONS`]，与默认表无关）。
    /// `release_click_through` / `show_all_pins` / `hide_all_pins` / `custom_task` 四枚出厂不占键，
    /// 上面那一格永远看不见它们，而它们恰恰是这一族最容易烂掉的那一枚：有名字、能配、按下去什么
    /// 都不做 —— 台账 ⑫ 从头到尾记的都是这一形状。
    #[test]
    fn every_named_action_has_a_hand() {
        for id in falcon_core::config::ACTIONS {
            assert!(
                Work::of(id).is_some(),
                "名册上 {id} 这一枚没有归宿：按下去走「未接线」"
            );
        }
        // 反过来也要成立：一张路由表上不许有名册外的名字，否则「可绑定范围」就成了页上写着十二行、
        // `plan` 却认得十三枚那种事。
        for work in [
            Work::Capture,
            Work::CustomCapture,
            Work::ActiveWindow,
            Work::Repeat,
            Work::PastePin,
            Work::Solo,
            Work::TogglePins,
            Work::ShowAllPins,
            Work::HideAllPins,
            Work::ReleaseClickThrough,
            Work::SwitchGroup,
            Work::PinEdit,
            Work::CustomTask,
        ] {
            let id = work.action().expect("这一枚工作没有可绑定名");
            assert!(
                falcon_core::config::ACTIONS.contains(&id),
                "{id} 在路由表上却不在名册上"
            );
            assert_eq!(Work::of(id), Some(work), "{id} 走不到自己身上");
        }
        // 那八枚托盘专属的工作**不许**有可绑定名：§5.16.1 那张表里没有它们的行，替它们各发一枚
        // 键就是给那张表发明第十四、十五名。
        for work in [
            Work::OpenPicturePins,
            Work::Whiteboard,
            Work::Settings,
            Work::History,
            Work::Groups,
            Work::Update,
            Work::Help,
            Work::Quit,
        ] {
            assert_eq!(work.action(), None, "{work:?} 多了可绑定名");
        }
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
        // 第四种形状要有它自己的一枚。名册上那十三枚自此每一枚都有手（`every_named_action_has_a_hand`
        // 逐枚判的就是那一句），所以 `未接线` 那一句在这里只能由一枚**名册外**的名字引出来 ——
        // 而那恰好就是这一句将来要拦的形状：有人往 [`falcon_core::config::ACTIONS`] 添了一枚新名、
        // 还没在这里给它一只臂。红的是那张表，不是页上的写法。
        actions.insert("capture_extra".to_string(), "ctrl+shift+q".to_string());
        let planned = plan(&actions);
        // One layer at a time: `plan` writes the row for every action it has *finished*
        // talking about, and hands the ones it wants registered to `install`, which
        // writes their row only once the shell has answered. So the census of this
        // layer is rows + requests, and the census of the report is rows alone.
        assert_eq!(
            per_action(&planned.rows) + planned.requests.len(),
            actions.len()
        );
        // The four shapes that stop a registration are said in four voices, and each
        // one names the action it refused.
        let text: Vec<&str> = planned.rows.iter().map(|row| row.said.as_str()).collect();
        let text = text.join("\n");
        for (needle, action) in [
            ("未绑定", "solo"),
            ("系统保留组合", "pin_edit"),
            ("注册不了", "toggle_pins"),
            ("未接线", "capture_extra"),
        ] {
            assert!(
                planned
                    .rows
                    .iter()
                    .any(|row| row.said.contains(needle) && row.said.contains(action)),
                "{needle} 的那行没说到 {action}"
            );
            assert!(text.contains(needle), "{needle} 一行都没有");
        }
        // 四形在 §8.4 那一行上分两族，而这一判住在 kind 上、不住在「措辞里有没有那三个字」：
        // 一格里根本没写过键（`Idle`）不占屏幕，要过键而没成的三形（`Trouble`）占。哪天有人把
        // 「未绑定」也交给屏幕、或把保留组合那句改写到不成 `Trouble`，这一格当场红 —— 那正是
        // 一行浮窗最容易变成噪音的那两种走法。
        let seen = to_be_seen(&planned.rows);
        assert_eq!(seen.len(), 3, "屏幕那一行该只数要用户看的三形：{seen:?}");
        assert!(
            !seen.iter().any(|line| line.contains("未绑定")),
            "没写过键的那一行被交给了屏幕：{seen:?}"
        );
        for needle in ["系统保留组合", "注册不了", "未接线"] {
            assert!(
                seen.iter().any(|line| line.contains(needle)),
                "{needle} 那一行没进屏幕那一列"
            );
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
        // P166 之前这一格交回 `None` —— 那一串组合在表里、有默认值，却没有任何一只手接得住。
        // 现在它交的正是那一扇对话框的工作，而这一格读的是路由表本身，不是页上有没有那扇窗。
        assert_eq!(Work::of("capture_custom"), Some(Work::CustomCapture));
    }

    /// The tray menu offers every wired, tray-reachable work plus 退出, and every one of
    /// them routes: an entry with no destination would be a menu item that does nothing
    /// when clicked, which is the failure this table exists to avoid. §5.18.1 的建议清单里
    /// 那件曾经没有归宿的手（帮助）自 P174 起也在列，因为 [`Work::Help`] 那一只手此刻真住在
    /// [`crate::help`] 里 —— 「历史记录」自 P171 起
    /// 在列，因为那一枚 [`Work::History`] 有真手 —— 有真手的才列，列一枚没有归宿的进来就是这一句
    /// 要防的死行（#433）。「图片分组」自 P172 起在列，按的是 PRD 第 1437 行那一列的**位置**：
    /// 它紧跟第 1436 行「显示或隐藏全部贴图」，所以排在 `TogglePins` 之后、白板之前
    /// （白板不在那一列里，它来自 §5.4.4，见 [`Work::Whiteboard`] 那一臂记的两处冲突）。
    /// 「检查更新」自 P173 起在列，按的是第 1441 行那一列的**位置**：它上面那一行是第 1440 行「帮助」，
    /// 而 P173 那一代「帮助」还没有手、不列，所以它当时直接接在第 1438 行「历史记录」之后、
    /// 第 1442 行「退出」之前。
    /// 「首选项」自 P142 起、「自定义截图」自 P166 起在列：
    /// 它们列的是 [`Work::Settings`] 与 [`Work::CustomCapture`] 那两枚有归宿的手；前者**不带括起来的
    /// 按键名** —— `action()` 是 `None`，场上没有一枚键通向它，写一个名字进去就是替 §5.16.1 那张表
    /// 发明第十二名 —— 后者带，因为 `ctrl+shift+f` 是表里那一行的默认值。
    /// 「打开图片并贴图」自本轮起在列，按的还是同一条规矩：它列的是 [`Work::OpenPicturePins`]
    /// 那一只有归宿的手（§5.8.3 第一步点名的那一枚托盘项），也不带括起来的按键名 —— §5.16.1
    /// 那张表里没有这一名。它不在 §5.18.1 那一列十行里，与白板、解除穿透记的是同一处冲突，
    /// 见上面 `tray_menu` 与那一臂各自记下的那句。
    ///
    /// 这一格下面那份名单的**次序**也是判据的一部分，而 P174 使它露了一次真：那一列里第 1438 行
    /// 「历史记录」排在第 1439 行「首选项」之前，而这份装配到 P174 之前一直按落地先后排（P142 的
    /// 首选项在 P171 的历史记录之前）。`--help-menu` 门把「次序由文档那一列决定」量了一遍，当场
    /// FAIL；改的是上面 `tray_menu` 的装配次序。名字与文档不同的那两行（「贴图」在这里叫
    /// 「贴出剪贴板」、文档「显示或隐藏全部贴图」在这里叫「隐藏/显示全部贴图」）不在本格之内，
    /// 它们记在 [`every_work_is_named_by_the_one_table`] 那一格与 `--help-menu` 门的读数里。
    #[test]
    fn every_menu_entry_has_a_destination() {
        let mut planned = plan(&Config::default().hotkey.actions);
        let menu = tray_menu(&mut planned);
        assert_eq!(menu.len(), 14);
        assert_eq!(
            menu.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(),
            vec![
                "截图（Ctrl+Shift+A）",
                "自定义截图（Ctrl+Shift+F）",
                "重复上次截图（Ctrl+Shift+X）",
                "贴出剪贴板（Ctrl+Shift+V）",
                // §5.8.3 第一步那七个字。它紧跟「贴出剪贴板」的位子由 §5.8 自己的次序定：
                // 5.8.2 从剪贴板、5.8.3 从图片文件，两段是相邻的（装配那一列头顶记的就是这一句）。
                "打开图片并贴图",
                "隐藏/显示全部贴图（Ctrl+Shift+P）",
                // §5.9.13 第三步那一条退出。它不带括起来的按键名：出厂不占一枚键（见
                // `config::Hotkey::default` 旁边那一段），而这一行没有键名恰恰是那一节规则
                // 第一句要的形状 —— 这一枚入口不靠键盘上的一把键成立，它靠的是托盘。
                "解除鼠标穿透",
                "图片分组",
                "白板",
                "历史记录",
                "首选项",
                "帮助",
                falcon_core::update::TRAY_WORD,
                "退出",
            ]
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
        // 托盘里那两枚没有键的手：退出与首选项。它们的 id 一律不是热键 id（下面这两段是同一条
        // 判据的两个名字，不是一项判据写两遍 —— #281）。
        let quit = planned
            .routes
            .iter()
            .find(|route| route.work == Work::Quit)
            .unwrap();
        assert_eq!(quit.action, "");
        assert!(quit.display.is_empty());
        assert!(!planned.requests.iter().any(|request| request.id == quit.id));
        let settings = planned
            .routes
            .iter()
            .find(|route| route.work == Work::Settings)
            .unwrap();
        assert_eq!(settings.action, "");
        assert!(settings.display.is_empty());
        assert!(!planned
            .requests
            .iter()
            .any(|request| request.id == settings.id));
        // 一枚也没有任何动作名指向它：`Work::of` 是那张路由表唯一的真相来源，而它递不出
        // 一只手来开设置窗 —— 「配得上却什么都不做」那一族在设置页这一头同样不许存在。
        for action in falcon_core::config::ACTIONS {
            assert_ne!(
                Work::of(action),
                Some(Work::Settings),
                "{action} 通向设置窗"
            );
        }

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

    /// 替手那一点纯函数：一个名字 → 菜单里那一项的 id。三臂各问一次，而重点问的是
    /// [`press`] 够不着的那一臂。
    ///
    /// 这一枚分岔本体存在的理由，[`menu_id_of`] 的注释里已经写好：`Work::Settings` 在路由表里
    /// 那一行的 `action` 是空串，按名字查它等于按空串查 —— 后者会把场上所有菜单专属行捞成
    /// 同一格，于是「点了首选项」可以靠别人走通的链路交出 Pass。这里把它钉成一条判据，
    /// 因为这一格在真机上只有这一枚函数能区分。
    #[test]
    fn the_menu_reaches_an_item_by_work_not_by_action_name() {
        let mut planned = plan(&Config::default().hotkey.actions);
        tray_menu(&mut planned);
        let routes = planned.routes.clone();
        let settings = routes
            .iter()
            .find(|route| route.work == Work::Settings)
            .unwrap();
        assert_eq!(menu_id_of("settings", &routes), Some(settings.id));
        // 按动作名查的那一枚 `press` 在这一行上是空的：不是它坏了，而是场上压根没有一把键
        // 通向设置窗（§5.16.1 那张表里没有第十二名）。
        assert!(routes.iter().any(|route| route.action.is_empty()));
        let unnamed = routes
            .iter()
            .filter(|route| route.action.is_empty())
            .map(|route| route.work)
            .collect::<Vec<_>>();
        assert_eq!(
            unnamed.len(),
            8,
            "没有动作名的行只有八枚 —— 多一枚就是有人给一件没有手的事编了个名字"
        );
        // 七枚的名字逐个判，而不是只数一个格数：`len()==N` 单独站着时，一枚挤进来一枚溜出去
        // 会互相抵消成一句假绿。这一族此刻是 `Settings`（§5.20.1 那扇窗）、`Quit`（§5.1.1）、
        // `Whiteboard`（§5.4.4 第一步的托盘入口）、`History`（§5.14.4 那扇窗）、自 P172 起的
        // `Groups`（§5.12.3 那扇窗）、自 P173 起的 `Update`（§5.21.1 那一次查询，第 1441 行那一枚）
        // 、自 P174 起的 `Help`（§5.21.2 那四行的入口，第 1440 行那一枚），与自本轮起的
        // `OpenPicturePins`（§5.8.3 第一步那一枚托盘项）—— 八枚都有真手，都没有可绑定动作名。
        for work in &unnamed {
            assert!(
                matches!(
                    work,
                    Work::Settings
                        | Work::Quit
                        | Work::Whiteboard
                        | Work::History
                        | Work::Groups
                        | Work::Update
                        | Work::Help
                        | Work::OpenPicturePins
                ),
                "{work:?} 不在这一族里：它要么先要一只手，要么要先有文档点名的那一枚名字"
            );
        }
        // 有归宿又有菜单项的一枚：走的是同一条按 work 查的路。
        let capture = routes
            .iter()
            .find(|route| route.work == Work::Capture)
            .unwrap();
        assert_eq!(menu_id_of("capture", &routes), Some(capture.id));
        // 三臂：开菜单、什么都不做、名单外的名字，一律没有可点的项（它们不是「没点到」，
        // 而是这一层压根没有一件这样的工作）。
        for action in ["menu", "none", "print_screen", "settings_"] {
            assert_eq!(
                menu_id_of(action, &routes),
                None,
                "{action:?} 不该有一枚 id"
            );
        }
        // 表里没有这一枚 work 时，名字认得也不许凭空造出一枚 id。
        assert_eq!(menu_id_of("settings", &[]), None);
        // 替手投出去的 id 必须是 shell 从没被问过的那一枚：菜单专属 id 不在注册里，所以
        // 「这一发投给了谁」不能由注册表反推。
        assert!(!planned
            .requests
            .iter()
            .any(|request| request.id == settings.id));
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

    /// 上一位的那格读数在这里**红了也没用**：`pin_edit` 与 `custom_task` 两句拒绝，形向上与
    /// 成功那一句完全同款（都是 `GUI 线程 {动作}：…` 一行，都不带 `遮罩开不起来` / `id=-1` 那
    /// 两个记号），所以只读链路的判据会把一次「场上没有选中的贴图」判成 Pass。这一格把两件分开
    /// 钉：链路那一判确实答了 Pass（门不许假装看不见自己会错），而 [`object_cap`] 把它压回
    /// Blocked —— 压它的是「按下那一刻有没有选中的贴图」那一读，判据住在产品自己那里
    /// （[`object_of`] 问的是 `selected_bounds` 与 `tasks::plan`，门不抄一句字面）。
    ///
    /// 界线另一侧同样钉死：`Fail` 与 `Blocked` 原样过。对象缺席不是「链路吞了这条消息」那件缺陷
    /// 的借口，也不是把一次已经缺席的读数再宣布一遍。
    #[test]
    fn a_refusal_without_its_object_is_never_read_as_done() {
        let history = vec![
            "热键 id=9 pin_edit：排队到 GUI 线程（贴图编辑）".to_string(),
            "GUI 线程 pin_edit：贴图编辑没跑：此刻一枚贴图也没被选中，而这一枚键要的是「选中的那 \
             一枚」——屏幕上点中谁，编辑的就是谁。"
                .to_string(),
            "热键 id=10 custom_task：排队到 GUI 线程（自定义命令）".to_string(),
            "GUI 线程 custom_task：自定义命令：这一格里还没写过任何命令：第一个词该是程序，后面跟它 \
             的参数。"
                .to_string(),
        ];
        for action in ["pin_edit", "custom_task"] {
            assert_eq!(
                press_verdict(action, true, &history, false).0,
                state::Check::Pass,
                "{action}：这一格要钉的是门的第二读，不许顺手改链路那一判的判据"
            );
            assert_eq!(
                object_cap(state::Check::Pass, false),
                state::Check::Blocked,
                "{action}：对象不在时那一行只会是拒绝，拒绝不是做成了"
            );
        }
        // 对象在场时门一字不改产品的账，缺席时也不许盖住另一型读数。
        assert_eq!(object_cap(state::Check::Pass, true), state::Check::Pass);
        assert_eq!(object_cap(state::Check::Fail, false), state::Check::Fail);
        assert_eq!(
            object_cap(state::Check::Blocked, false),
            state::Check::Blocked
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
        // The two the previous round left unwired are wired now, and P135 wired the third
        // one (`solo`), so a config that wants *no* registrations has to refuse those as
        // well - the denominator this test counts is the config's nine, not the six this
        // build can perform. P137 wired `toggle_pins` too, so it joins the refusal set:
        // left at its default it would register and the row count would drop below nine.
        //
        // P166 adds the fourth: `capture_custom` got its `Work` arm with the 自定义截图 dialog,
        // so at its default chord it now *binds* - and a bound action pushes no row at all
        // (`plan` pushes one per refusal, not one per success). Emptying the cell puts it back
        // into the refusal set: the row count is the config's nine again, and `requests` stays
        // the zero this test's second half needs.
        actions.insert("capture_active_window".to_string(), "".to_string());
        actions.insert("capture_repeat".to_string(), "".to_string());
        actions.insert("solo".to_string(), "".to_string());
        actions.insert("toggle_pins".to_string(), "".to_string());
        actions.insert("capture_custom".to_string(), "".to_string());
        // P186 是第五枚：`switch_group` 有了 `Work` 那一臂，出厂那一串 `ctrl+shift+g` 自此
        // 真能绑上 —— 留着默认值它就是这一趟唯一一枚注册成功的键，行数当场少一枚，而这一格
        // 要的读数是「九行对九枚」。清空它，它回到拒绝那一族里。
        actions.insert("switch_group".to_string(), "".to_string());
        // 第六枚同一笔账：`pin_edit` 自 [`Work::PinEdit`] 起接得住，出厂那串 `ctrl+shift+e`
        // 会注册成功，于是它也不推行。`custom_task` 不在这道闸里 —— 出厂默认压根没给它配键，
        // 它不在 `actions` 这一张表上，分母也就没它。
        actions.insert("pin_edit".to_string(), "".to_string());
        let planned = plan(&actions);
        assert_eq!(per_action(&planned.rows), actions.len());
        assert_eq!(planned.requests.len(), 0, "这趟一个都注册不了");
        // 两轴各问各的。`冲突 …` 说的是一枚键被两枚动作共用，不是「某个动作的答复」，普查的分母
        // 不含它；而 `… 已注册` 是答复、却不该出现在屏幕那一行上。旧写法按 `starts_with("冲突")`
        // 数 —— 那一判就是同一句话的第二个作者（#281），改一天措辞普查当场少数一枚；现在两轴读的
        // 是写下那一句的同一手当场定的 kind。
        let rows = vec![
            Row::conflict("冲突 Ctrl+Shift+A：capture + solo 都在用这一串键".to_string()),
            Row::taken("capture Ctrl+Shift+A 已注册".to_string()),
        ];
        assert_eq!(per_action(&rows), 1, "冲突那一行被算进了「每个动作一行」的分母");
        assert_eq!(to_be_seen(&rows).len(), 1, "已注册那一行被交给了屏幕");
    }

    /// §8.4 那一行浮窗的字：数目读行数、引文逐字取自 `Row::said`、过上限的那几行说清去哪看。
    #[test]
    fn the_notice_line_counts_rows_and_caps_the_quotes() {
        // 一枚：没有「其余」那一截。
        let one = vec!["capture Ctrl+Shift+A 是系统保留组合：已被占用（不注册）"];
        assert_eq!(
            notice_line(&one),
            "1 枚快捷键要你看一眼：capture Ctrl+Shift+A 是系统保留组合：已被占用（不注册）"
        );
        // 三枚：正好到上限，引文全部原样在，分隔符一枚不多一枚不少。
        let three = vec!["甲", "乙", "丙"];
        assert_eq!(notice_line(&three), "3 枚快捷键要你看一眼：甲；乙；丙");
        // 五枚：屏幕上只引三句，其余两句**给出去向**而不是抹掉 —— 浮窗那一格读的是一句话，到
        // 第四句就不像提示、像日志（同一条理由见 `falcon_core::pin::FILE_PIN_SAID_MAX`）。
        let five = vec!["甲", "乙", "丙", "丁", "戊"];
        let said = notice_line(&five);
        assert_eq!(
            said,
            "5 枚快捷键要你看一眼：甲；乙；丙…其余 2 行见「快捷键」设置页"
        );
        assert!(!said.contains("丁") && !said.contains("戊"), "那两行没被引文挤掉：{said}");
        // 「去哪看」必须指一个用户真能打开的地方：页上那一列拿的是全列（[`WATCHED`]），而运行历史
        // 与 stdout 都不是页 —— 指错地方等于让那两行没有去处（#433）。
        assert!(said.contains("设置页"), "过上限的那几行没说去哪看：{said}");
        // 空列由 `install` 那一格挡在门外（出厂九枚全成时屏幕不该闪一句），这一枚函数自己
        // 不为它编一句「一切正常」—— §8 整章判的是出了问题时说什么，不是报功。
        assert_eq!(notice_line(&[]), "0 枚快捷键要你看一眼：");
        // 上限读 [`SAID_MAX`] 而不是抄一个数字：改天放宽到四句，上面 3/5 两例的算术当场对不上，
        // 而这一例跟着常量走 —— 一句措辞的第二个作者就是这么来的（#455）。
        assert_eq!(three.len(), SAID_MAX, "上限变了，上面那两例的算术也要跟着改");
    }
}
