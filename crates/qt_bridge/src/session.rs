//! The session controller: how many pins there are, which one is at which slot,
//! and the only door through which new pins arrive or leave.
//!
//! Membership changes go through here because `count` is a qproperty and the QML
//! `Repeater` is what creates and destroys the windows. Everything else - a wheel
//! turn, a drag, a rotation - is per-pin view state and lives in
//! [`crate::pin_view`].

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qbytearray.h");
        type QByteArray = cxx_qt_lib::QByteArray;
    }

    // The product's thin C++ shim (cpp/pin_shim.h). QSurfaceFormat, QWindow::winId,
    // the Win32 extended styles and the QQuickImageProvider subclass are all
    // C++-only, which is why plan §3.6 declares a shim mandatory.
    unsafe extern "C++" {
        include!("qt_bridge/pin_shim.h");

        /// Must run before the first window or the pins lose their alpha.
        #[rust_name = "enable_alpha_buffer_by_default"]
        fn enableAlphaBufferByDefault();

        #[rust_name = "pin_store_frame"]
        fn pinStoreFrame(id: i64, png: &QByteArray);
        #[rust_name = "pin_drop_frame"]
        fn pinDropFrame(id: i64);
        #[rust_name = "pin_self_check"]
        fn pinSelfCheck(id: i64) -> QString;
        #[rust_name = "pin_pixel"]
        fn pinPixel(id: i64, x: i32, y: i32) -> QString;

        #[rust_name = "pin_desktop_x"]
        fn pinDesktopX() -> i32;
        #[rust_name = "pin_desktop_y"]
        fn pinDesktopY() -> i32;
        #[rust_name = "pin_desktop_w"]
        fn pinDesktopW() -> i32;
        #[rust_name = "pin_desktop_h"]
        fn pinDesktopH() -> i32;
        #[rust_name = "pin_primary_x"]
        fn pinPrimaryX() -> i32;
        #[rust_name = "pin_primary_y"]
        fn pinPrimaryY() -> i32;
        #[rust_name = "pin_primary_w"]
        fn pinPrimaryW() -> i32;
        #[rust_name = "pin_primary_h"]
        fn pinPrimaryH() -> i32;
        #[rust_name = "pin_device_pixel_ratio"]
        fn pinDevicePixelRatio() -> f64;

        #[rust_name = "pin_window_report"]
        fn pinWindowReport() -> QString;
        #[rust_name = "pin_top_levels"]
        fn pinTopLevels() -> QString;
        #[rust_name = "pin_push_dialog_button"]
        fn pinPushDialogButton() -> QString;
        /// Where the native `#32770` dialog this process is showing sits, as
        /// `x,y,w,h` in device pixels. Empty is one honest answer to three different
        /// questions - see the comment above `pinDialogRect` in `cpp/pin_shim.h`.
        #[rust_name = "pin_dialog_rect"]
        fn pinDialogRect() -> QString;
        #[rust_name = "pin_screen_grab"]
        fn pinScreenGrab() -> QByteArray;
        #[rust_name = "pin_screen_grab_info"]
        fn pinScreenGrabInfo() -> QString;
        #[rust_name = "pin_install_message_capture"]
        fn pinInstallMessageCapture();
        #[rust_name = "pin_messages"]
        fn pinMessages() -> QString;
        #[rust_name = "pin_quit_after"]
        fn pinQuitAfter(ms: i32);
        /// Off in a gauge run and on in a resident one: a session whose last mask
        /// closes must still be listening to its hot keys. See [`crate::hotkey`].
        #[rust_name = "set_quit_on_last_window_closed"]
        fn pinSetQuitOnLastWindowClosed(on: bool);

        /// How many screen signals the §8.5 watch is connected to. Zero until QML has
        /// run `watchScreens()`, which is exactly the difference between "the document
        /// compiled" and "the watch is listening" - and the listening is the whole of
        /// what a run can show without unplugging a monitor to get the rest.
        #[rust_name = "pin_screen_watch_connections"]
        fn pinScreenWatchConnections() -> i32;
    }

    extern "RustQt" {
        /// How many pins are on screen - the model the QML Instantiator repeats.
        #[qobject]
        #[qml_element]
        #[qproperty(i32, count)]
        /// How many screens the capture mask covers - the other `Instantiator`'s
        /// model. Zero until a flow freezes the desktop, which is the normal state:
        /// a mask window that exists before the pixels do is a black rectangle.
        #[qproperty(i32, mask_count)]
        /// One line of feedback for the user, and the reason the output calls
        /// below answer with a string: §5.9.13 asks for a brief on-screen hint,
        /// and a hint has to say what actually happened rather than nothing.
        #[qproperty(QString, status)]
        /// The same slot the mask's toolbar binds to as `pre_notice`: what a format
        /// without alpha is about to do to this pin, in the future tense, and empty when
        /// nothing is being lost. Kept off `status` because `status` is one line and the
        /// pair has to survive together - PRD 447 asks for the warning, §5.9.13 for the
        /// result, and a door that overwrites one with the other can only report half.
        #[qproperty(QString, pre_notice)]
        /// Rounds the `--mask` gauge still owes, after the first one. A qproperty and
        /// not a plain read because a QML `Timer`'s `running` has to *re-evaluate*
        /// when it hits zero - an invokable the binding calls once would keep firing.
        #[qproperty(i32, warm_left)]
        /// How far apart those rounds sit, and therefore the `Timer`'s interval.
        #[qproperty(i32, warm_step)]
        /// §5.2.4 延时截图：还剩几枚数没数。和 [`Self::warm_left`] 同一枚形状 —— 绑的是**值**
        /// 而不是一次调用，所以 `main.qml` 那枚 `Timer` 的 `running` 会在归零那一刻重新求值、
        /// 自己停下来；写成 invokable 就会数过零继续数。
        #[qproperty(i32, delay_left)]
        /// 一枚数与下一枚之间隔多少毫秒，也就是那枚 `Timer` 的 `interval`。
        #[qproperty(i32, delay_step)]
        /// P119：§5.19.1 第 2 步交给界面的那一枚词，只有 `"light"` / `"dark"` 两臂。绑的是
        /// **值**而不是一次调用（同 [`Self::warm_left`] / [`Self::delay_left`] 那条教训）：
        /// `HintWindow.qml` 那四枚色挂在它上面，而 `refreshTheme()` 之后必须让那些绑定重新
        /// 求值 —— 写成 invokable 就是一次读死。词本身由 [`crate::settings::theme_face`] 交，
        /// 不在这里现拼。
        #[qproperty(QString, theme)]
        /// §5.5.11 步骤 3/4 的"有事发生了"那一枚号：每次为对话框藏起或放回过贴图，它就 +1。
        ///
        /// 计数而不是一枚 bool，是因为第二张对话框压上来时 `visible` 压根没变 —— bool 会读成
        /// "没变化"，那些贴图的 `live` 就再没人问过一遍。绑的是**值**（同
        /// [`Self::warm_left`] / [`Self::theme`] 那条教训），所以 `main.qml` 那个
        /// `Connections` 的 handler 会在每一次 +1 上重新求值。
        #[qproperty(i32, dialog_epoch)]
        type Session = super::SessionRust;

        /// Re-read the state machine. QML calls this once at start-up and every
        /// mutation below also does it, so the count can never lag the set.
        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        /// 把「用户钉的 `[appearance] theme`」与「机器答的注册表」合成成一枚词写进
        /// [`Self::theme`]。QML 在 `Component.onCompleted` 里叫一次：这是 §5.19.1 第 2 步
        /// 唯一把 HKCU 那枚读数送到界面一侧的路径，而它与 `[theme cfg]` 那行读的是同一枚
        /// 键（两格各走各的 `apps_use_light_theme()` 就是 #281 那一族）。
        #[qinvokable]
        #[cxx_name = "refreshTheme"]
        fn refresh_theme(self: Pin<&mut Self>);

        /// Re-read the mask state, for the same reason [`Self::refresh`] re-reads
        /// the pins. Every mask gesture mutates the shared state, so this is how the
        /// *count* catches up with it - the windows themselves are refreshed by the
        /// `grabbed` signal in `main.qml`.
        #[qinvokable]
        #[cxx_name = "refreshMasks"]
        fn refresh_masks(self: Pin<&mut Self>);

        /// §5.2.1's way out: the overlay is gone, the frozen desktop with it. Esc
        /// reaches this once it has nothing left to un-select.
        #[qinvokable]
        #[cxx_name = "endMask"]
        fn end_mask(self: Pin<&mut Self>);

        /// Take the warm-round plan written from the command line. See
        /// [`crate::state::WarmPlan`] for why a QML call is the only way in.
        #[qinvokable]
        #[cxx_name = "refreshWarm"]
        fn refresh_warm(self: Pin<&mut Self>);

        /// Run one warm round of the `--mask` gauge, and spend one from the budget.
        #[qinvokable]
        #[cxx_name = "warmRound"]
        fn warm_round(self: Pin<&mut Self>);

        /// 把 [`crate::hotkey`] 刚刚挂上的倒计时取进上面那两枚 qproperty。只有热键叫它：
        /// `interval` 必须在第一枚数落地之前就设好，而一次 QML 调用是唯一走得通的路。
        #[qinvokable]
        #[cxx_name = "refreshDelay"]
        fn refresh_delay(self: Pin<&mut Self>);

        /// §5.2.4 步骤 4/5：数一枚。**最后一枚就是快门** —— 它冻画面、开遮罩，于是 PRD 第 420
        /// 行那句「倒计时结束时的指针状态」在像素和指针上同时成立（指针那一问自 P107 起长在
        /// [`crate::capture::freeze`] 里）。前面几枚只报还剩几秒，一格也不许冻。
        #[qinvokable]
        #[cxx_name = "delayRound"]
        fn delay_round(self: Pin<&mut Self>);

        /// Called by `main.qml` once its root is built. See [`crate::state::qml_loaded`].
        #[qinvokable]
        #[cxx_name = "markLoaded"]
        fn mark_loaded(self: &Self);

        /// Give the hot-key pump the only route it has back onto this object's thread.
        /// QML calls it once, in `Component.onCompleted`, immediately after
        /// [`Self::mark_loaded`] - and the pump is only started after that, so a press
        /// can never arrive before there is somewhere to send it. See [`crate::hotkey`].
        #[qinvokable]
        #[cxx_name = "handThread"]
        fn hand_thread(self: Pin<&mut Self>);

        /// The pin in slot `index` of the z order, or 0 when there is none.
        #[qinvokable]
        #[cxx_name = "pinId"]
        fn pin_id(self: Pin<&mut Self>, index: i32) -> i64;

        /// §5.8's "pin this" before the capture service exists: put a checkerboard
        /// on the screen so the window path can be exercised on its own.
        #[qinvokable]
        #[cxx_name = "addDemo"]
        fn add_demo(self: Pin<&mut Self>, width: i32, height: i32) -> i64;

        /// §5.9.18 "关闭" from a pin window's own menu.
        #[qinvokable]
        #[cxx_name = "closePin"]
        fn close_pin(self: Pin<&mut Self>, id: i64) -> i32;

        /// §5.9.16 - this pin's pixels onto the clipboard, as the user sees them.
        #[qinvokable]
        #[cxx_name = "copyPin"]
        fn copy_pin(self: Pin<&mut Self>, id: i64) -> QString;

        /// §5.9.17 - this pin out to a file; the extension picks the format, and `filter`
        /// is what the dialog's menu says for a name that has none.
        #[qinvokable]
        #[cxx_name = "savePin"]
        fn save_pin(self: Pin<&mut Self>, id: i64, path: &QString, filter: &QString) -> QString;

        /// §5.8.2 - whatever is on the clipboard becomes a pin. -1 and a status
        /// line when there was nothing to pin (§8.1: no window, but a message).
        #[qinvokable]
        #[cxx_name = "pinClipboard"]
        fn pin_clipboard(self: Pin<&mut Self>) -> i64;

        /// The `--pin` gauge's plan as one line, and the one way a pin window
        /// learns it is being measured. Empty when nobody asked, which is what keeps
        /// `PinWindow.qml` out of this in a product session. See
        /// [`crate::pin_check::plan_line`] for why it is a string and not four
        /// properties.
        #[qinvokable]
        #[cxx_name = "pinGaugePlan"]
        fn pin_gauge_plan(self: &Self) -> QString;

        /// One line the pin window read off its own doors, into the gauge's report.
        /// The two census keys are the exception, and an empty value: which windows are
        /// on the screen is [`crate::pin_check`]'s reading to take, and the window's part
        /// of that leg is saying *when*.
        #[qinvokable]
        #[cxx_name = "pinGaugeReport"]
        fn pin_gauge_report(self: &Self, line: &QString);

        /// Press the default button of the native file dialog this process is
        /// showing, and answer with what was pressed. Only the two gauges call it -
        /// `--pin`'s own dialog leg and `--ink`'s mask leg - and only while a dialog of
        /// theirs is on the screen; the window it finds is filtered to this pid for the
        /// reason [`crate::pin_check`] spells out.
        #[qinvokable]
        #[cxx_name = "pushDialogButton"]
        fn push_dialog_button(self: &Self) -> QString;

        /// The `--ink` gauge's plan for the mask's own 另存为 leg, and the one way a
        /// toolbar learns it is being measured. Empty in a product session and in a
        /// `--mask` run, which is what keeps the leg out of both.
        #[qinvokable]
        #[cxx_name = "maskGaugePlan"]
        fn mask_gauge_plan(self: &Self) -> QString;

        /// One line the toolbar read off its own door, into the ink report.
        #[qinvokable]
        #[cxx_name = "maskGaugeReport"]
        fn mask_gauge_report(self: &Self, line: &QString);

        /// §8.5 - a screen changed resolution, scaling or existence. Re-read the
        /// desktop, pull back whatever pin is now outside it, and answer with the
        /// sentence the user is owed: empty when nothing moved, which is the ordinary
        /// answer and the reason the caller must not go putting it on a window.
        #[qinvokable]
        #[cxx_name = "desktopChanged"]
        fn desktop_changed(self: Pin<&mut Self>) -> QString;

        /// The hint window's anchor and lifetime, as one line read once at completion:
        /// `主屏 x | 主屏 y | 主屏 w | 主屏 h | dpr | hold 毫秒`. Empty when this run is not
        /// allowed a hint surface.
        ///
        /// The screen's rectangle comes from Rust rather than from the attached `Screen`
        /// object because a `Window`'s geometry decides which screen it is on, and a width
        /// divided by a ratio read from that decision is the loop `PinWindow.qml` names in
        /// its own `dpr`. One call, one reading, no second copy of the desktop.
        #[qinvokable]
        #[cxx_name = "hintAnchor"]
        fn hint_anchor(self: &Self) -> QString;

        /// The `--hint` door's plan for the hint window's own leg. Empty in a product
        /// session, which is what keeps the leg out of the resident app. See
        /// [`crate::hint_check::plan_line`].
        #[qinvokable]
        #[cxx_name = "hintGaugePlan"]
        fn hint_gauge_plan(self: &Self) -> QString;

        /// One line the hint window read off its own door, into that report.
        #[qinvokable]
        #[cxx_name = "hintGaugeReport"]
        fn hint_gauge_report(self: &Self, line: &QString);

        /// §5.14.2 step 1「打开历史记录」的那一张清单：库里最前的 `limit` 行，一行一段、
        /// `|` 分隔 —— 与 `toolNames()` / `sendToMenuNames()` / `shareMenuNames()` 同一个
        /// 形状，因为 QML 那边 `model: x.split("|")` 是这所房子把 Rust 的列表交给界面的
        /// 唯一写法（`AnnotationToolbar.qml` 里已经跑着一枚）。
        ///
        /// **空串是两种不同的事**，而这两种事都在 Rust 那一头分得开：库里没有行，和这一行
        /// 压根读不到（开关关着、库打不开）。分不开的那一句写在
        /// [`crate::mask::MaskState::history_list_last`] 里，界面要说的话去问那一格，不许拿
        /// 空串冒充"用户的历史是空的"。
        #[qinvokable]
        #[cxx_name = "historyRows"]
        fn history_rows(self: &Self, limit: i32) -> QString;

        /// §5.14.2 step 4「用户可重新复制、保存、标注或贴图」里的**保存**那一半（P97 建的
        /// [`crate::mask::MaskState::replay_history`] 自此有了界面读者）：把历史里第 `id` 行
        /// 那张图再落一次盘，并把那一行说的话交给 `status` —— 屏幕上就该看得见。
        #[qinvokable]
        #[cxx_name = "historyReplay"]
        fn history_replay(self: Pin<&mut Self>, id: i64) -> QString;

        /// §5.14.4 step 2「用户可选择单条、多条记录进行删除」里的**删除**那一档，P100 起。
        /// `ids` 是界面点名的行号，`|` 连成一串（[`Self::history_rows`] 头上那段：这所房子
        /// 递列表只有这一种写法），一次递空串就是"一行也没选中"——那一臂由
        /// [`crate::mask::MaskState::delete_history`] 当着界面拒绝，本条不替它圆。
        #[qinvokable]
        #[cxx_name = "historyDelete"]
        fn history_delete(self: Pin<&mut Self>, ids: &QString) -> QString;

        /// §5.14.4 step 2 的**全部**那一档 + 规则 1 的二次确认。`confirmed` 是界面那颗弹窗
        /// 的答案，而它是产品里唯一的门栓：递 `false` 那一刻连库都不打开（P99 第 21 格量的
        /// 就是这一件）。本条不做二次确认，弹窗归界面；本条只保证**没点头就一行也不动**。
        #[qinvokable]
        #[cxx_name = "historyClear"]
        fn history_clear(self: Pin<&mut Self>, confirmed: bool) -> QString;

        /// §5.14.4 规则 2「不得删除用户明确收藏或锁定的记录」里**打上那一枚锁**这半，P100 起。
        /// 规则的前半（清空绕开锁定项）P99 起由 [`crate::mask::MaskState::clear_history`]
        /// 守着，后半一直只有 core 单测和量具的前身在按 —— 界面递不出「收藏这一行」这个动作。
        /// `locked` 两臂都真跑：真锁上、假解开。
        #[qinvokable]
        #[cxx_name = "historyLock"]
        fn history_lock(self: Pin<&mut Self>, id: i64, locked: bool) -> QString;

        /// §5.14.3 step 4「恢复该截图内容或**重新进入原选区**」里**选区**那一半（P102 建的
        /// [`crate::mask::MaskState::restore_history_selection`] 自此有了界面读者，P103 起）：
        /// 把历史里第 `id` 行记着的那一枚矩形交回遮罩，并把那一行说的话交给 `status`。
        /// 它与 [`Self::history_replay`] 是同一句需求里的两半相反的腿：那一枚往盘上多落一张图，
        /// 这一枚一张也不落，只动屏幕上那一只框。
        #[qinvokable]
        #[cxx_name = "historyRestore"]
        fn history_restore(self: Pin<&mut Self>, id: i64) -> QString;

        /// §5.14.2「按上下方向键在历史里前后翻」的**界面读法**：给定历史里那一行的 `id`，
        /// 返回它在这一库里前后各是谁。P111 那一条 startup banner 拿的是「库里最新那一行」这一
        /// 枚 anchor；界面上当用户把选区挪到别的行时，要问的是**那一行自己的**邻居，走的是
        /// [`crate::settings::neighbors_of_banner_for`]。本条**不改**遮罩、不写 `status`、
        /// 只答一句 —— 与 [`Self::mask_hole`] 同一族 `self: &Self` 的读侧。
        #[qinvokable]
        #[cxx_name = "historyNeighborsAt"]
        fn history_neighbors_at(self: &Self, id: i64) -> QString;

        /// 这一场遮罩**此刻停在**历史第几行（P114 起：[`Self::history_neighbors_at`] 自此有了
        /// 界面读者，而那一问的锚点只能从这一枚来）。
        ///
        /// 本体是 [`crate::mask::MaskState::history_anchor`] 的原文，`-1` 说的是"这一场还没停在
        /// 任何一行上"。它存在的理由与 [`Self::history_flip`] 从 P113 起拒绝让界面推行号是同一条：
        /// 屏幕上那只框所在的行只有一个权威，界面一旦自己报号，就多出一整格"报的号与框所在的行
        /// 不是同一行"，而那一格产品里没人能核对。所以读侧也走同一扇门：先问这一枚，再拿它去问
        /// 那一行的左右。纯读，不动选区、不动 `status`、也不碰库。
        #[qinvokable]
        #[cxx_name = "historyAnchor"]
        fn history_anchor(self: &Self) -> i64;

        /// §5.14.2 step 2「使用上一条、下一条快捷键浏览记录」的**输入腿**（P113 起）：
        /// `back = true` 是上一条、`false` 是下一条，遮罩上的框跟着那一行走。
        ///
        /// 锚点**不由界面递进来**：它取的是 [`crate::mask::MaskState::history_anchor`] ——
        /// 屏幕上当下停在哪一行就翻那一行的左右。让 QML 传号进来就会多出一整格
        /// "递的号与框实际所在的行不是同一行"的状态，而那一格产品里没人能核对。
        ///
        /// 它翻过去的那一步**不新开一条通路**，走的是 P102 建的
        /// [`crate::mask::MaskState::restore_history_selection`]（→ 同一枚 `set_hole`），只是经
        /// [`crate::mask::MaskState::flip_history_selection`] 那一层：那一层除了回成之后多按一枚
        /// `history_flips` 之外一件事也不做。§5.14.2 的翻页与 §5.14.3 的「重新进入原选区」在产品里
        /// 对的是同一件事，两条通路就会有两个"什么算回成了"。返回的那句话与屏幕上的 `status`
        /// 一字同（同 [`Self::history_restore`]）。
        #[qinvokable]
        #[cxx_name = "historyFlip"]
        fn history_flip(self: Pin<&mut Self>, back: bool) -> QString;

        /// 此刻遮罩上那一只框，物理桌面像素，`x,y WxH` 一句话说完 ——
        /// [`crate::mask::MaskState::hole`] 的原文。界面要确认「选区回到原来了没有」只能问这一格：
        /// `MaskView` 那四枚 `hole_x…hole_h` 是**这一扇窗的 DIP 投影**，跨屏时还会是空的，那两套
        /// 坐标答不了桌面这一问。空选区答的是 `PhysRect` 自己那句 `0,0 0x0`，不是本条编的。
        #[qinvokable]
        #[cxx_name = "maskHole"]
        fn mask_hole(self: &Self) -> QString;

        /// The `--autosave` door's plan for the history read-side legs, as one line.
        /// Empty in a product session and in every other door, which is what keeps
        /// `AnnotationToolbar.qml`'s history leg out of both.
        #[qinvokable]
        #[cxx_name = "historyGaugePlan"]
        fn history_gauge_plan(self: &Self) -> QString;

        /// One line the toolbar read off its own history legs, into that door's report.
        #[qinvokable]
        #[cxx_name = "historyGaugeReport"]
        fn history_gauge_report(self: &Self, line: &QString);

        /// §5.5.11 步骤 3「隐藏可能遮挡对话框的贴图」的**事件**。界面那一张
        /// `FileDialog` 一变得可见就叫这一枚；矩形由本进程向 Win32 现读（读不到就走
        /// 「整屏都藏」那一臂，理由写在 `cpp/pin_shim.h` 上头）。
        ///
        /// 开关**不由界面递进来**：它取 [`crate::settings::output_cfg`] 里的
        /// `hide_pins_on_dialog`，与启动横幅 `[output cfg]` 那一行读的是同一枚键。让 QML
        /// 传布尔进来就会多出一整格"屏幕上藏了而配置没说藏"的状态（#281 那一族），而那
        /// 一格产品里没人能核对。藏了谁、藏了几层，一律由 [`crate::state::PinState`] 记账。
        #[qinvokable]
        #[cxx_name = "dialogOpened"]
        fn dialog_opened(self: Pin<&mut Self>);

        /// §5.5.11 步骤 4「对话框关闭后恢复贴图显示」的**事件**，与上面那一枚成对：界面那张
        /// `FileDialog` 一变得不可见就叫这一枚，无论用户是按了保存还是按了取消 —— 两条腿
        /// 之后对话框都不在了，贴图就都该回来。
        ///
        /// 叠开的两格（"另存为"之后再弹一次覆盖确认）由 [`falcon_core::dialog_link::Hidden`]
        /// 记层数，本条只报"有一张关掉了"：只有最后一张关掉那一刻才真的放回贴图。
        #[qinvokable]
        #[cxx_name = "dialogClosed"]
        fn dialog_closed(self: Pin<&mut Self>);

        /// §5.5.11 步骤 3 的那一判的**只读**版本：向 Win32 现读一次那扇对话框占的桌面块，把
        /// "谁盖得住它"连同此刻的账印成一行交回。不改任何一笔账、也不弄走任何一枚贴图 ——
        /// 门要引的就是这一枚，因为 [`Self::dialog_opened`] 那一枚一动账，屏幕上就少了
        /// 一格可以被这一格判的东西。行的形状由
        /// [`crate::state::PinState::dialog_probe`] 说了算，本层不拼字。
        #[qinvokable]
        #[cxx_name = "dialogProbe"]
        fn dialog_probe(self: &Self) -> QString;
    }

    // Threading is opt-in per QObject; without this declaration `qt_thread()` does
    // not exist on `Session`. It is what lets a message that arrived on the pump
    // thread ask this object - which lives on the GUI thread and is not `Send` - to
    // open a mask. Proven against 6.10.1 in `spike/hello-cxxqt/src/pump.rs`.
    impl cxx_qt::Threading for Session {}
}

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use falcon_core::geometry::PhysPoint;

use crate::mask;
use crate::state;

#[derive(Default)]
pub struct SessionRust {
    count: i32,
    mask_count: i32,
    status: QString,
    pre_notice: QString,
    warm_left: i32,
    warm_step: i32,
    delay_left: i32,
    delay_step: i32,
    theme: QString,
    dialog_epoch: i32,
}

impl qobject::Session {
    pub fn refresh(mut self: Pin<&mut Self>) {
        let n = state::with(|s| s.ids().len());
        self.as_mut().set_count(n as i32);
    }

    pub fn refresh_theme(mut self: Pin<&mut Self>) {
        // 空串不是第三臂：`SessionRust` 建出来时 `theme` 是空的，而界面那一侧把「不是 dark」
        // 一律读成 light。于是「没人叫过这枚」与「机器压根没答」在屏幕上同色 —— 两样都由
        // `resolve_theme` 的 `None => Light` 那一臂兜底，`settings.rs` 的九臂单测逐臂点名。
        self.as_mut()
            .set_theme(QString::from(crate::settings::startup_theme_face()));
    }

    pub fn refresh_masks(mut self: Pin<&mut Self>) {
        let n = mask::with(|m| m.slots.len());
        self.as_mut().set_mask_count(n as i32);
    }

    pub fn end_mask(mut self: Pin<&mut Self>) {
        // `close` is the only path that hands the frozen frames back: a 4K desktop
        // is 24 MB per screen, and an overlay that ends without it leaks per
        // capture, not per process.
        mask::with(|m| m.close());
        self.as_mut().set_mask_count(0);
    }

    pub fn refresh_warm(mut self: Pin<&mut Self>) {
        let (left, step) = state::warm(|w| (w.left.max(0), w.step_ms.max(200)));
        self.as_mut().set_warm_step(step);
        self.as_mut().set_warm_left(left);
    }

    pub fn warm_round(mut self: Pin<&mut Self>) {
        if *self.warm_left() <= 0 {
            return;
        }
        // The same dim path the cold round drew, so the two numbers describe one
        // drawing path rather than two.
        let shader = state::warm(|w| w.shader);
        let line = crate::mask_check::warm_round(shader);
        state::warm(|w| {
            let n = w.rounds.len() + 1;
            w.rounds.push(format!("round {n}: {line}"));
        });
        let left = *self.warm_left() - 1;
        self.as_mut().set_warm_left(left);
        // The count normally does not move (same screens), so the windows survive
        // the round and nothing rebuilds them. `main.qml` reloads them by hand, which
        // is what gets them the new frame key - the old one is gone from the store.
        self.as_mut().refresh_masks();
    }

    pub fn refresh_delay(mut self: Pin<&mut Self>) {
        // 步长先设、枚数后设，和 `refresh_warm` 同一顺序：`running` 一改成 true，`Timer` 就照着
        // 当下的 `interval` 起表 —— 反过来会先按上一场的间隔走出第一枚数。默认那枚步长不是 0，
        // 因为 0 ms 的 `interval` 在 Qt 里是「尽可能快」，而一枚疯狂自转的定时器会把这场延时
        // 变成一场没有延时的截图。
        self.as_mut().set_delay_step(crate::delay::step_ms() as i32);
        let left = crate::delay::left();
        self.as_mut().set_delay_left(left as i32);
        // §5.2.4 步骤 4：屏幕上此刻就要出现「还剩 N 秒」那一枚数。走的是 [`crate::delay::hint_text`]
        // 那条唯一的分岔 —— `Some(text)` ⇒ `set_status`；`None` ⇒ 一格也不写（`delay_seconds`=0
        // 那一臂走的正是这条路，所以热键按下之后仍是立刻拍、屏幕上不会出现任何倒计时）。
        //
        // 顺序也定在这里：`delay_left` 先落、`status` 后落，两枚 notify 都会同步跑；`delay_left`
        // 的 handler 只在 <= 0 时才 hide，此刻是 `left > 0`，所以 handler 什么都不做；`status`
        // 的 handler 把 `hint.shown = true` 交回 QML，屏幕上就出现了那一枚数。反过来（先写
        // status、再写 left）也是同一件事，但 left 归零那一格就不成立了（下面 Go 那一臂说的就是这个）。
        if let Some(text) = crate::delay::hint_text(left) {
            self.as_mut().set_status(QString::from(text.as_str()));
        }
    }

    pub fn delay_round(mut self: Pin<&mut Self>) {
        // 这一格是 §5.2.4 步骤 4 与步骤 5 之间那道唯一的门：报数的那几枚只改 qproperty，
        // 走完的最后一枚才真的按下快门。次序由 `crate::delay::Countdown` 定，不由这里判 ——
        // 所以「还剩几枚」这件事在单测里就能证，不必等到运行期开一扇窗。
        match crate::delay::tick() {
            Some(crate::delay::Tick::Waiting { left }) => {
                self.as_mut().set_delay_left(left as i32);
                // 屏幕上那句「还剩 N 秒」跟着 left 一起走 —— 这里必须在 `set_delay_left` 之后
                // 再写 status：QML `HintWindow.qml` 那条收窗 handler 绑的是 `counting`
                // （`hint.session.delay_left > 0`，P119 起），只在归零那一支才 hide，正数那一支
                // 什么都不做；把 status 留到之后写，屏幕上的字就会跟着数
                // 一起下台阶（3→2→1）。反过来先写 status 后写 left 也是同一件事，但下面的
                // Go 那一臂就不再成立。
                if let Some(text) = crate::delay::hint_text(left) {
                    self.as_mut().set_status(QString::from(text.as_str()));
                }
            }
            Some(crate::delay::Tick::Go { .. }) => {
                // 归零 → QML handler `hint.shown = false` → `hide()`。cxx-qt 的 qproperty setter
                // 是**同步** notify（同线程 direct connection），所以这一行走完之时 HintWindow
                // 已经叫过 `hide()`；下一行才 `open_mask`，再下一层 `mask::open_from_freeze` 里
                // 才真的 `capture::freeze()`。PRD 第 420 行「倒计时结束时的指针状态」这条时刻
                // 断言之外，还有一件同一轮要保的事：**「画面到手之前不存在任何我们的窗」**
                // （`crates/qt_bridge/src/capture.rs` 头顶那条规则）⇒ 倒计时自己那一句必须在
                // `freeze` 之前收走，否则屏幕上那一枚数字会进图。这一行与下一行之间的这条
                // 同步语义就是那半件事的担保。
                self.as_mut().set_delay_left(0);
                crate::hotkey::open_mask("延时截图", self.as_mut());
            }
            // `Spent` 与 `None` 都只有一件事可做：把 `running` 关掉。槽里已经没有倒计时了，
            // 交回第二个 `Go` 的那条路在 `delay::tick` 里就被堵住了。
            Some(crate::delay::Tick::Spent) | None => self.as_mut().set_delay_left(0),
        }
    }

    pub fn pin_id(self: Pin<&mut Self>, index: i32) -> i64 {
        state::with(|s| s.id_at(index.max(0) as usize)) as i64
    }

    pub fn mark_loaded(&self) {
        state::mark_qml_loaded();
    }

    /// The one thing the pump thread needs and cannot make for itself: a handle on
    /// the thread this object lives on. QML owns the `Session` instance, so Rust has
    /// no other way to reach it - see [`crate::hotkey::hand`].
    pub fn hand_thread(self: Pin<&mut Self>) {
        crate::hotkey::hand(self.qt_thread());
    }

    pub fn add_demo(mut self: Pin<&mut Self>, width: i32, height: i32) -> i64 {
        let frame = state::demo_frame(width.max(4) as u32, height.max(4) as u32);
        // New pins land a little offset from the last one, so a stack of them is a
        // stack of visible edges rather than one pin with three copies of itself.
        let step = state::with(|s| s.ids().len()).min(24) as i32;
        let at = PhysPoint::new(80 + step * 16, 80 + step * 16);
        match state::with(|s| s.add(frame, at)) {
            Ok(id) => {
                self.as_mut().refresh();
                id as i64
            }
            Err(e) => {
                eprintln!("[session] add_demo failed: {e}");
                0
            }
        }
    }

    pub fn close_pin(mut self: Pin<&mut Self>, id: i64) -> i32 {
        let n = state::with(|s| s.close(&[id as u64])) as i32;
        self.as_mut().refresh();
        n
    }

    /// The three output paths and the one input path all answer the same way: a
    /// line of Chinese the user can read, kept on `status` so a hint widget can
    /// bind to it later without this call being re-run.
    fn say(mut self: Pin<&mut Self>, message: &str) -> QString {
        self.as_mut().set_status(QString::from(message));
        QString::from(message)
    }

    pub fn copy_pin(mut self: Pin<&mut Self>, id: i64) -> QString {
        let message = match state::with(|s| s.copy_image(id as u64)) {
            Ok(pixels) => format!("已复制贴图（{pixels} 像素）"),
            Err(e) => format!("复制失败：{e}"),
        };
        self.as_mut().say(&message)
    }

    /// `filter` is the extension text of the format the dialog was showing
    /// (`selectedNameFilter.extensions`, spelled `*.jpg`), and it only matters when the
    /// name the user typed has no extension of its own: §5.5.5 step 3 counts the format as
    /// one of the three things chosen, so a bare 报表 leaves the menu's decision in.
    pub fn save_pin(
        mut self: Pin<&mut Self>,
        id: i64,
        path: &QString,
        filter: &QString,
    ) -> QString {
        let target = path.to_string();
        let chosen = filter.to_string();
        // §10 第 47 项 ② - the reminder goes up *before* the write, while the file the
        // sentence warns about is still not on disk. `save_image` fills the same cell as
        // its own first rung; going through it here too is what keeps the window's slot
        // and the door's sentence one string instead of two spellings of a promise.
        let pre = state::with(|s| s.remind_before_save(id as u64, &target, &chosen));
        self.as_mut().set_pre_notice(QString::from(&*pre));
        let message = match state::with(|s| s.save_image(id as u64, &target, &chosen)) {
            Ok(done) => done,
            Err(e) => format!("保存失败：{e}"),
        };
        self.as_mut().say(&message)
    }

    pub fn pin_clipboard(mut self: Pin<&mut Self>) -> i64 {
        match state::paste_from_clipboard() {
            Ok(id) => {
                self.as_mut().refresh();
                id as i64
            }
            Err(e) => {
                self.as_mut().say(&format!("无法贴图：{e}"));
                -1
            }
        }
    }

    pub fn pin_gauge_plan(&self) -> QString {
        QString::from(crate::pin_check::plan_line().as_str())
    }

    pub fn pin_gauge_report(&self, line: &QString) {
        crate::pin_check::report(&line.to_string());
    }

    pub fn push_dialog_button(&self) -> QString {
        QString::from(crate::pin_check::push_dialog_button().as_str())
    }

    pub fn mask_gauge_plan(&self) -> QString {
        QString::from(crate::annotate_check::mask_gauge_plan().as_str())
    }

    pub fn mask_gauge_report(&self, line: &QString) {
        crate::annotate_check::mask_report(&line.to_string());
    }

    pub fn desktop_changed(mut self: Pin<&mut Self>) -> QString {
        // Rust owns the decision - see [`state::Refit`] for what it counts - and this
        // call is only the route the answer takes to the screen. The sentence is owed
        // by the plan's own line 「屏幕变化：分辨率/插拔时裁剪到有效区并提示」; PRD §8.5
        // itself asks for two behaviours and says nothing about words. So the silent
        // case stays silent - a scaling change moves no pin, and a "已刷新" on every
        // dpi tick is a prompt about the watch rather than about the user's pins.
        match state::desktop_changed() {
            Some(message) => self.as_mut().say(&message),
            None => QString::default(),
        }
    }

    pub fn hint_anchor(&self) -> QString {
        QString::from(crate::hint_check::anchor_line().as_str())
    }

    pub fn hint_gauge_plan(&self) -> QString {
        QString::from(crate::hint_check::plan_line().as_str())
    }

    pub fn hint_gauge_report(&self, line: &QString) {
        crate::hint_check::report(&line.to_string());
    }

    pub fn history_rows(&self, limit: i32) -> QString {
        // 界面传来的可能是 0，也可能是负数（QML 那边没有"至少要有几行"这个概念）。
        // `newest(0)` 是一条合法的 `LIMIT 0` —— 那是"一行也别给"，不是用户问的话，所以在
        // 这条边界上取至少 1，而把"要不要这一行"的判断留在 Rust 这一头。
        QString::from(mask::with(|m| m.list_history(limit.max(1) as usize)).as_str())
    }

    pub fn history_replay(mut self: Pin<&mut Self>, id: i64) -> QString {
        // 落盘在 `mask` 那一头跑完，话在这一头交出去：`history_replay_last` 说的是"回放成了
        // 还是被拒了、拒在哪一行"，而 §5.9.13 要的那句简短的屏幕提示只能从 `status` 走 ——
        // 两条腿各说一次就等于屏幕和量具读的是两句话。
        mask::with(|m| m.replay_history(id));
        let said = mask::with(|m| m.history_replay_last.clone());
        self.as_mut().say(&said)
    }

    pub fn history_delete(mut self: Pin<&mut Self>, ids: &QString) -> QString {
        // 界面递来的是一串 `|` 连起来的行号（`7|9|12`）。读不成整数的片段**丢掉**而不是报错，
        // 也不另起一句"有几枚看不懂"：丢完之后如果一个也不剩，交回的就是
        // `delete_history` 自己那句「没有点名任何一行，一行也不删」。把"看不懂"说成"删好了"
        // 是本条挡的事，把它说成一次崩溃同样不是界面该看到的答复。
        let picked: Vec<i64> = ids
            .to_string()
            .split('|')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect();
        mask::with(|m| m.delete_history(&picked));
        // 话从产品那一格读，不在这里重说一遍：`history_delete_last` 说的"删了第几行、收回
        // 多少字节"和它被拒的理由，屏幕上和量具读回来必须同一句（同 `history_replay` 那段）。
        let said = mask::with(|m| m.history_delete_last.clone());
        self.as_mut().say(&said)
    }

    pub fn history_clear(mut self: Pin<&mut Self>, confirmed: bool) -> QString {
        mask::with(|m| m.clear_history(confirmed));
        let said = mask::with(|m| m.history_clear_last.clone());
        self.as_mut().say(&said)
    }

    pub fn history_lock(mut self: Pin<&mut Self>, id: i64, locked: bool) -> QString {
        mask::with(|m| m.lock_history(id, locked));
        let said = mask::with(|m| m.history_lock_last.clone());
        self.as_mut().say(&said)
    }

    pub fn history_restore(mut self: Pin<&mut Self>, id: i64) -> QString {
        // 交回选区在 `mask` 那一头跑完，话在这一头交出去：`history_restore_last` 说的是"回到了
        // 哪一枚矩形、还是被拒了、拒在哪一行"，屏幕上和量具读回来必须是同一句（同
        // `history_replay` 那段理由）。本条不另报那个矩形：它是产品那句话的一部分，也是
        // [`Self::mask_hole`] 此刻的答案，界面要核对有两处可问，而这一枚桥复述第三遍只会多出
        // 一处能说谎的地方。
        mask::with(|m| m.restore_history_selection(id));
        let said = mask::with(|m| m.history_restore_last.clone());
        self.as_mut().say(&said)
    }

    pub fn history_neighbors_at(&self, id: i64) -> QString {
        // §5.14.2 prev/next 那两条腿的**界面读法**：P111 那条 banner 拿的是「库里最新那一行」
        // 做 anchor；界面把选区挪到别的行时，问的是**那一行自己的**邻居。分岔本体在
        // [`crate::settings::neighbors_of_banner_for`]，本条只把 dir/cfg 换成**这一场遮罩**那
        // 一对会话快照（[`crate::mask::MaskState::history_store`]），把答案原样交给 QML ——
        // 桥不复述、不改写，屏幕读回来的和 settings 那一边判出来的必须一字同。
        //
        // 遮罩没开（或这一场没有历史门）那一支才退回全局 config 那两枚：那时界面问的仍是
        // 「产品默认的库里第 N 行是谁」，而这一场没有任何一座自己的库可问。
        let (dir, cfg) = mask::with(|m| m.history_store()).unwrap_or_else(|| {
            (
                crate::settings::history_data_dir(),
                crate::settings::history_cfg(),
            )
        });
        QString::from(crate::settings::neighbors_of_banner_for(&dir, &cfg, id).as_str())
    }

    pub fn history_anchor(&self) -> i64 {
        // 界面要读某一行的邻居，先得知道它自己停在哪一行 —— 这一枚就是那个号的唯一权威，
        // 由遮罩当场答（[`crate::mask::MaskState::history_anchor`]）。工具条不自报号、也不从
        // 清单的第几行倒推：那种号与屏幕上那只框之间没有任何东西保证相等。
        //
        // 这一枚与 [`Self::history_flip`] 从 P113 起「锚点不由界面递进来」是同一条口径的两面：
        // 那一枚拒绝收号，这一枚把号发出去给读侧用，两腿因此问的永远是同一行。
        mask::with(|m| m.history_anchor())
    }

    pub fn mask_hole(&self) -> QString {
        QString::from(mask::with(|m| m.hole.to_string()).as_str())
    }

    pub fn history_flip(mut self: Pin<&mut Self>, back: bool) -> QString {
        // 锚点从遮罩那一头取：界面停在哪一行就翻那一行的左右。不在这里猜、也不由 QML 递进来，
        // 理由写在 bridge 那一侧的 doc 里（那一格与屏幕上那只框必须是同一件东西的两面）。
        let anchor = mask::with(|m| m.history_anchor());
        if anchor < 0 {
            // 「这一场还没有可翻的行」与「翻到头了」是两件事，所以这一格挡在下面那一问之前：
            // 拿 -1 去库里问，答回来的会是「库里没有第 -1 行」——把一场干净的开始讲成一次故障。
            return self.as_mut().say("翻页没跑：这一场还没有可翻的行。");
        }
        // 库也是从遮罩那一头取，而且**只能**从那一头取：下面那两条腿（`neighbor_id_for` 问行号、
        // `restore_history_selection` 取选区）必须开同一座库，否则一次翻页拿到的是 A 库的行号、
        // 交回的是 B 库的选区。P113 第一次跑门时这一行读的是 `settings::history_data_dir()`，
        // 而 `--autosave` 那档的 `retarget()` 只把 `MaskState` 那一对重钉进沙盒 ⇒ 邻居那一问去了
        // 一座空库，当场答「库里没有第 3 行」（`p113_auto_A.log` 第 31 格）。
        //
        // 这里**不**留 `settings` 兜底：走到这一行，锚点必定是由一次成功的恢复或记录立起来的，
        // 而那两件事都以开得到库为前提。`None` 因此是「这一场翻不动」的另一件原因，各说各话。
        let Some((dir, cfg)) = mask::with(|m| m.history_store()) else {
            return self
                .as_mut()
                .say("翻页没跑：这一场没有可开的库（`[history]` 没落点）。");
        };
        if let Some(id) = crate::settings::neighbor_id_for(&dir, &cfg, anchor, back) {
            // 翻过去 = 重新进入那一行的原选区，走 P102 那一条通路（同一枚 `set_hole`、同一句
            // `history_restore_last`）。翻页因此不会长成第二条"什么算回成了"。多加的那一枚
            // `history_flips` 是 [`crate::mask::MaskState::flip_history_selection`] 在回成之后按的，
            // 本条不自己数 —— 数在哪儿，"什么算翻成了一页"就多出一个第二定义。
            mask::with(|m| m.flip_history_selection(id));
            let said = mask::with(|m| m.history_restore_last.clone());
            return self.as_mut().say(&said);
        }
        // 四格合一（`enabled` 关着 / 库打不开 / 锚点那一行已不在库里 / 那一侧到头了）：本条不编
        // 原因，把负责点名那一职的 [`crate::settings::neighbors_of_banner_for`] 那一行原样附上。
        // 桥只做转发 —— 判它的措辞等于判一句产品自己没说的话。
        let why = crate::settings::neighbors_of_banner_for(&dir, &cfg, anchor);
        let side = if back { "上一条" } else { "下一条" };
        self.as_mut()
            .say(&format!("翻页没跑：{side}没有可去的行（{why}）。"))
    }

    pub fn history_gauge_plan(&self) -> QString {
        QString::from(crate::autosave_check::plan_line().as_str())
    }

    pub fn history_gauge_report(&self, line: &QString) {
        crate::autosave_check::report(&line.to_string());
    }

    pub fn dialog_opened(mut self: Pin<&mut Self>) {
        // 开关只在这一格读，而且读的是启动横幅 `[output cfg]` 那一行用的同一个函数：屏幕上
        // 到底藏没藏，必须由这一枚说了算 —— 界面再递一枚布尔进来就是两份读数
        // （#281 那一族）。
        let enabled = crate::settings::output_cfg().hide_pins_on_dialog;
        // 读不到矩形不等于"谁也没遮挡"。Qt 把 `FileDialog` 标成可见之后约 400 ms，Win32
        // 那一头的 `#32770` 才建出来（`PinWindow.qml` 里量的三趟），所以真事件几乎总是走
        // `None` 这一臂：整屏都藏 —— §5.5.11「可能遮挡」的保守读法。
        let rect = shim::dialog_rect();
        if state::with(|s| s.dialog_opened(enabled, rect)).is_empty() {
            // 一张贴图也没动，屏幕上就没有要重新求值的东西；而一次 +1 会让每一扇贴图窗
            // 把自己的图 `reload()` 一遍。
            return;
        }
        // 先读旧值再写，和 `warm_round` 同一写法：`self.as_mut()` 与 `self.dialog_epoch()`
        // 写在同一枚表达式里是 E0502。
        let next = *self.dialog_epoch() + 1;
        self.as_mut().set_dialog_epoch(next);
    }

    pub fn dialog_closed(mut self: Pin<&mut Self>) {
        // 中间那张关掉时第二张还开着，`closed` 就交空名单（层数由
        // [`falcon_core::dialog_link::Hidden`] 记），于是这一侧也是"没有贴图回来就一次
        // `reload` 也不发"。
        if state::with(|s| s.dialog_closed()).is_empty() {
            return;
        }
        let next = *self.dialog_epoch() + 1;
        self.as_mut().set_dialog_epoch(next);
    }

    pub fn dialog_probe(&self) -> QString {
        // 矩形只在这里读一次，读的就是这一格要判的那一块桌面：把同一次读分给两处（一处判据、
        // 一处印数）就不是同一枚读数了，而与 [`Self::dialog_opened`] 各读一次更是两份事实。
        let rect = shim::dialog_rect();
        let line = state::with(|s| s.dialog_probe(rect));
        QString::from(line.as_str())
    }
}

/// The shim, in Rust shapes. Everything the state machine needs from Qt goes
/// through here, so `state.rs` never has to name a C++ type - and the ids stay
/// `PinId`, which is a `u64`, rather than being cast at every call site.
pub mod shim {
    use cxx_qt_lib::QByteArray;
    use falcon_core::geometry::PhysRect;
    use falcon_core::pin::PinId;

    use super::qobject;

    pub fn store_frame(id: PinId, png: &[u8]) {
        // A real deep copy: the Vec is Rust's again the moment the call ends.
        let mut ba = QByteArray::default();
        ba.resize(png.len() as isize);
        ba.as_mut_slice().copy_from_slice(png);
        qobject::pin_store_frame(id as i64, &ba);
    }

    pub fn drop_frame(id: PinId) {
        qobject::pin_drop_frame(id as i64);
    }

    pub fn self_check(id: PinId) -> String {
        qobject::pin_self_check(id as i64).to_string()
    }

    pub fn pixel(id: PinId, x: i32, y: i32) -> String {
        qobject::pin_pixel(id as i64, x, y).to_string()
    }

    pub fn desktop_bounds() -> (i32, i32, i32, i32) {
        (
            qobject::pin_desktop_x(),
            qobject::pin_desktop_y(),
            qobject::pin_desktop_w(),
            qobject::pin_desktop_h(),
        )
    }

    pub fn primary_bounds() -> (i32, i32, i32, i32) {
        (
            qobject::pin_primary_x(),
            qobject::pin_primary_y(),
            qobject::pin_primary_w(),
            qobject::pin_primary_h(),
        )
    }

    pub fn device_pixel_ratio() -> f64 {
        qobject::pin_device_pixel_ratio()
    }

    pub fn window_report() -> String {
        qobject::pin_window_report().to_string()
    }

    /// One line per top-level window, with its Win32 handle and device-pixel rect.
    pub fn top_levels() -> String {
        qobject::pin_top_levels().to_string()
    }

    /// Press the default button of a native `#32770` dialog *this process* is showing,
    /// and report what was pressed. `top_levels` cannot serve this because it walks
    /// Qt's own `topLevelWindows()`, and a native file dialog is not one of them.
    pub fn push_dialog_button() -> String {
        qobject::pin_push_dialog_button().to_string()
    }

    /// Where that dialog sits, in device pixels. `None` is one honest answer to three
    /// different questions - the window is not created yet (Qt marks the `FileDialog`
    /// visible ~400 ms before Win32 makes the `#32770`), this process owns no such
    /// window, or `GetWindowRect` failed - and §5.5.11 步骤 3 reads all three the same
    /// way, as "no rectangle to intersect with".
    pub fn dialog_rect() -> Option<PhysRect> {
        let said = qobject::pin_dialog_rect().to_string();
        let mut it = said.split(',');
        let (x, y, w, h) = (
            it.next()?.parse::<i32>().ok()?,
            it.next()?.parse::<i32>().ok()?,
            it.next()?.parse::<i32>().ok()?,
            it.next()?.parse::<i32>().ok()?,
        );
        // 零面积或负边长一律算"没有矩形"：一枚零宽的矩形与 [`crate::state`] 那句
        // "没有可藏的贴图" 是同一种缺席，而不是"藏零张"的读数。
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(PhysRect::new(x, y, w as u32, h as u32))
    }

    /// The whole primary screen as Qt's backend produced it, RGBA8. Empty means Qt
    /// produced nothing, which is a measurement and not an error.
    pub fn screen_grab() -> Vec<u8> {
        qobject::pin_screen_grab().as_slice().to_vec()
    }

    pub fn screen_grab_info() -> String {
        qobject::pin_screen_grab_info().to_string()
    }

    pub fn install_message_capture() {
        qobject::pin_install_message_capture();
    }

    pub fn messages() -> String {
        qobject::pin_messages().to_string()
    }

    pub fn quit_after(ms: i32) {
        qobject::pin_quit_after(ms);
    }

    /// How many screen signals the §8.5 watch holds, counted where they were made.
    /// Zero means QML never asked, which a probe line can tell apart from a machine
    /// whose one screen has never moved.
    pub fn screen_watch_connections() -> i32 {
        qobject::pin_screen_watch_connections()
    }
}
