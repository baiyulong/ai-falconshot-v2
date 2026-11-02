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
        /// Give the GUI thread up to `ms` of turns. A door that waits after `app.exec()` has
        /// returned has to pump to see anything an `interval: 0` QML Timer would have done -
        /// see `pinPumpEvents` in `cpp/pin_shim.h`, which names the P176 census row that slept
        /// instead. Qt 6 returns void here, so nothing is handed back but the turn taken.
        #[rust_name = "pin_pump_events"]
        fn pinPumpEvents(ms: i32);
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
        /// §5.10.1 的那份名单此刻有几枚。与 [`Self::count`] 同一形状：绑的是**值**，每次
        /// [`Self::refresh`] 都跟着重读，所以窗侧、批量菜单与门读到的是同一份长度 —— 而
        /// [`Self::selection_epoch`] 只管"有事发生了"，两格各答各的问题，合起来才是多选。
        #[qproperty(i32, selection_count)]
        /// 「选中变了」那一枚号：每一手加/减/清都 +1。它是**扇给所有窗**的那一手 —— 被点中的
        /// 那一扇自己会 `reload`，另一些窗的边却不会自己消失，所以这一格借的是 `dialog_epoch`
        /// 那条已经跑通的腿：`main.qml` 把别名绑在**值**上，notify 一到就 `redrawPins()`。
        /// 计数而不是一枚 bool，理由与 [`Self::dialog_epoch`] 同一封信：同一枚贴图按两下
        /// Ctrl 是加又是减，`true` 在那两手之间压根没变，号却每手都在走。
        #[qproperty(i32, selection_epoch)]
        /// §5.10.2 的「有一扇窗被搬走了」那一枚号：每一手**真的**搬动了贴图，它才 +1。
        ///
        /// 为什么这一手不能搭 [`Self::selection_epoch`] 那条腿：拖动不改名单，`selectPin` 一次
        /// 也没被叫，notify 也就不发 —— 而批量要的恰恰是**没被按住的那几扇窗**也跟着重读自己
        /// 该画在哪儿。为什么不搭 [`Self::count`]：搬动一枚贴图，集里还是那些枚，`count` 一动
        /// 不动，`Instantiator` 也就不会重建任何窗。所以借的还是 [`Self::dialog_epoch`] 那条
        /// 已跑通的腿：`main.qml` 把别名绑在**值**上，notify 一到就 `redrawPins()`。
        ///
        /// 搬动 0 枚时**不发号**（同 [`Self::clear_selection`] 那一判据）：一扇已关的窗剩下的
        /// 鼠标事件不该让全场重读一遍。
        #[qproperty(i32, move_epoch)]
        /// §5.10.3 的「有一批窗的不透明度变了」那一枚号：每一手**真的**调到了贴图，它才 +1。
        ///
        /// 为什么它不搭 [`Self::move_epoch`] 那条腿：搬动与调透明度是两只手，而号是**读者**在数的
        /// 那件事。`PinWindow.qml` 的量规就各读各的（`mov-epoch-*` 读搬动、`op-epoch-*` 读透明度）：
        /// 同一枚号被两只手各 +1，读到的那个「走了几步」就答不出是**哪一**手走的 —— 两件事挤进
        /// 一格（#281）。借的还是 [`Self::dialog_epoch`] 那条已跑通的腿：`main.qml` 把别名绑在
        /// **值**上，notify 一到就 `redrawPins()`，于是名单里**每一扇**窗重读自己的 `view.opacity`，
        /// 而不只是被按住那扇。
        ///
        /// 调 0 枚时**不发号**（同 [`Self::drag_pin`] 那一判据）：一扇已关的窗剩下的那一次滚轮
        /// 不该让全场重读一遍。
        #[qproperty(i32, opacity_epoch)]
        /// §5.10.4 的「有一批窗的视图被还原了」那一枚号：每一手**真的**重置到贴图，它才 +1。
        ///
        /// 为什么不搭 [`Self::move_epoch`]：还原视图改的是「缩放/旋转/位置」这一整套，而搬动那一枚
        /// 号只回答「窗挪了没挪」——同一枚号被两只手各 +1，读到的「走了几步」就答不出是哪一
        /// 手走的（#281，与 [`Self::opacity_epoch`] 头顶那封信同一枚）。为什么不搭
        /// [`Self::count`]：重置不带走任何一枚贴图，集里还是那些枚。借的还是
        /// [`Self::dialog_epoch`] 那条已跑通的腿：`main.qml` 把别名绑在**值**上，notify 一到就
        /// `redrawPins()`。
        ///
        /// 重置 0 枚时**不发号**（同 [`Self::drag_pin`] 那一判据）。
        #[qproperty(i32, reset_epoch)]
        /// §5.9.18 的「此刻还欠着几枚撤销」：[`Self::destroy_pin`] 带走几枚，这一格就是几；
        /// [`Self::undo_destroy`] 还回去就清零。窗上那一枚「撤销销毁」的 `visible` 读它，
        /// 于是「有没有回头路」由桥侧那一格账答，而不是由哪扇窗自己猜（#281）。
        #[qproperty(i32, trashed)]
        /// §5.11 独显的「有一批窗被带走/被放回了」那一枚号：每一手**真的**改到了别的窗的
        /// 可见性或不透明度，它才 +1。
        ///
        /// 为什么不搭 [`Self::opacity_epoch`] 那条腿：压暗那一臂确实改了不透明度，但藏起来
        /// 那一臂改的是 `live`，而同一枚号被两只手各 +1，量规读到的「走了几步」就答不出是
        /// 哪一手走的（#281，与 [`Self::reset_epoch`] 头顶那封信同一枚道理）。为什么不搭
        /// [`Self::count`]：独显一枚也不从集里带走，`count` 一动不动，`Instantiator` 也就不
        /// 会重建任何窗。借的还是 [`Self::dialog_epoch`] 那条已跑通的腿 —— 它同为「一批窗的
        /// 可见性变了」而设：`main.qml` 把别名绑在**值**上，notify 一到就 `redrawPins()`，
        /// 于是**场上每一扇**窗重读自己的 `view.live`，而不只是被点中那扇（被带走的那些枚
        /// 恰恰不在名单里，这是 §5.11 与 §5.10 那三手最大的不同）。
        ///
        /// 改 0 枚时**不发号**（同 [`Self::drag_pin`] 那一判据）：一扇已关的窗剩下的那一次
        /// 点击不该让全场重读一遍。
        #[qproperty(i32, solo_epoch)]
        /// §5.11 规则③ 落到界面那一侧的那一格：**独显留着几枚**。`0` 是「此刻不在独显里」，
        /// 而它不可能是「开着独显却留 0 枚」—— core 的 `enter_solo` 对空名单直接交回 0 且
        /// 不记账，所以这一格与「在不在独显里」是同一件事的两面，不必再来一枚 bool（#281）。
        ///
        /// 它是 §5.11 里「屏幕提示」那一臂的**唯一**读数：窗上菜单那句字面、`HintWindow.qml`
        /// 里那一句话、以及量规的每一行读的都是这一格，所以「当前处于 Solo 状态」这件事在
        /// 全场只有一个数可读。托盘那一臂本轮没做（见台账项 136 的欠账）。
        #[qproperty(i32, solo_kept)]
        /// §5.12 分组清单落到界面那一侧的**唯一**读数：一整串字，把 core `group_rows()` 那张表
        /// 逐组编码成 `编号\u{1c}名称\u{1c}显不显(0/1)\u{1c}几枚`，组与组之间用 `\u{1d}` 分隔。
        /// 用两枚控制字符而不是 `,`／`|`：分组名是用户自己打的，任何可见分隔符都可能出现在名字里，
        /// 而名字里出现控制字符的可能性能忽略 —— 这一串是给机器读的，不是给人读的。
        ///
        /// 顺序就是那一格返回的顺序（core 的 `groups` vec 本体），于是 §5.12.3 那句「排序变化
        /// 同步反映到快捷切换菜单」在界面一侧只是**重读**这一串，不重排。谁改了这一串：下面六只
        /// 分组手各改一次，改完重写它 ⇒ 每次重写自会 notify，QML 的 `Repeater` 就重建那些菜单项。
        #[qproperty(QString, groups)]
        /// §5.12.4「显示 / 隐藏整组」的那一声「有一批窗的可见性被这一手改过了」，形状与
        /// [`Self::solo_epoch`] 同一枚：别名绑在**值**上，handler 长在根上，场上不存在的驼峰名
        /// 一枚也不写（#316）。为什么独显那一族与分组这一族各要一枚号：它们改的虽然都是 `live`，
        /// 但是**两件事** —— 借同一枚号，量规就答不出「这一拍是独显走的还是分组走的」（#281）。
        /// 只有真的改到可见性（切换显隐、随组销毁）才 +1；改名、排序、只改归属那些都不发号。
        #[qproperty(i32, group_epoch)]
        /// §5.15「隐藏 / 显示 / 切换**全部**贴图」这一族改的是 `live`，但是**第三件事** —— 与独显
        /// 那一族、分组那一族各不相干，所以自要一枚号（#281：借同一枚，量规就答不出这一拍走的
        /// 是哪一族）。形状仍与 [`Self::solo_epoch`] 同一枚：别名绑**值**、handler 长在根上。
        /// 只有 [`Self::hide_all_pins`] / [`Self::show_all_pins`] / [`Self::toggle_all_pins`]
        /// 这三只手会 +1。
        #[qproperty(i32, pins_epoch)]
        /// §5.15 全局收起落到界面那一侧的**唯一**读数：`1` 是「此刻全场被全局收起」，`0` 是「没有」。
        /// 它由 [`Self::refresh`] 与那三只手各写一次，于是托盘那句「显示全部 / 隐藏全部」的字面、
        /// `PinWindow.qml` 菜单里那一臂、以及量规每行读的都是同一格（#281）。它绑**值**不绑调用，
        /// 所以热键按下之后界面那一眼自会跟上（P37 那条口径）。
        /// §5.20.1 那一扇设置窗「该不该在场」的唯一读数：`1` 是开，`0` 是关。写它的人有两个，
        /// 读的只有 `main.qml` 那一条 `syncSettings`：桥（真人点了托盘里的「首选项」，
        /// [`crate::hotkey::Work::Settings`]）与窗自己（用户按了关闭、或量具那几条腿走完自己收）。
        ///
        /// 为什么是一枚数而不是一只 `showSettings()`：Session 那一个实例归 `main.qml` 拿着，
        /// 桥那边没有它的指针，能走得通的只有「写一格、让绑着它的 QML 自己动」——`mask_count`
        /// 与 `all_hidden` 打 P37 起走的就是这条路。窗不常驻而是按需造，理由与浮窗那一格相同：
        /// 桌面上多一扇我们的窗，`pin window` 那一档挑到的就可能不是它想的那一块。
        #[qproperty(i32, settings_open)]
        /// §5.3.5 那一扇「自定义截图」对话框「该不该在场」的唯一读数：`1` 是开，`0` 是关。与
        /// 上面那一格同一条路 —— 写它的是桥（[`crate::hotkey::Work::CustomCapture`]：一次热键或
        /// 托盘里那一点）与窗自己（关掉、或量具那几条腿走完自己收），读的只有 `main.qml` 那一条
        /// `syncCustom`。桥这侧一次 `show()` 也不叫：谁造这扇窗，销毁就归谁。
        #[qproperty(i32, custom_open)]
        /// 对话框**自己那一场**倒计时还剩几枚数。它与 [`Self::delay_left`] 不是一格：那一格数的是
        /// 「按下截图到遮罩出现」那一段（§5.2.4，数完开遮罩），这一格数的是「按下开始截图到真的
        /// 截这一块」（§5.3.5 第 301 行 + 第 302 行，数完截填好的那块）。两场的算术同源
        /// （[`crate::delay::Countdown`]、同枚 `STEP_MS`），实例各是一个。
        ///
        /// 页上那枚 `Timer` 的 `running` 绑的是这枚**值**而不是一次调用（[`Self::warm_left`] 那条
        /// 教训）：数满那一枚落地成 0，定时器就此自己停，不会多截一张。
        #[qproperty(i32, custom_left)]
        /// 页上那一句结果 / 拒绝的**原文**。它只有两位作者 —— [`crate::custom`] 里那一格，与
        /// 把那一格搬到这里的这三只手（[`Self::custom_set`] / [`Self::custom_start`] /
        /// [`Self::custom_tick`]）。窗不许自己拼一句「已截图」：那句子的作者在上面那一档，而门判的
        /// 就是窗递回来的那一串与现算的那一串逐字节相同（#281）。
        #[qproperty(QString, custom_note)]
        /// §5.14.4 第 1309 行那扇「历史记录管理」窗「该不该在场」的唯一读数：`1` 是开、`0` 是关。
        /// 与 [`Self::settings_open`]、[`Self::custom_open`] 同一副骨架：写它的是桥
        /// （[`crate::hotkey::Work::History`]：托盘里那一点）与窗自己（「关闭」那一枚钮），读的只有
        /// `main.qml` 那一条 `syncHistory`。桥这侧一次 `show()` 也不叫：谁造这扇窗，销毁就归谁。
        #[qproperty(i32, history_window_open)]
        /// §5.12.3 / §5.12.4 第 1218 行与第 1236 行那一扇「图片分组管理」窗「该不该在场」的唯一
        /// 读数：`1` 是开、`0` 是关。与上面两格同一副骨架 —— 写它的是桥
        /// （[`crate::hotkey::Work::Groups`]：§5.18.1 第 1437 行托盘里那一点）与窗自己（那枚「关闭」钮，
        /// 名字出自 [`crate::group_window::CLOSE_WORD`]），读的只有 `main.qml` 那一条 `syncGroup`。PRD 第 1251 行那三个入口
        /// （托盘菜单 / 分组面板 / 快捷键）里，「分组面板」这一格自此在盘上有一扇窗可对，
        /// 「快捷键」那一格仍归未接线的 `switch_group`（台账 ⑫）。
        #[qproperty(i32, group_window_open)]
        #[qproperty(i32, all_hidden)]
        /// §5.6.1 步骤 2 的**结果**那一格：识别出来的整段字，`\n` 分行。
        ///
        /// 绑**值**而不是一枚 invokable（同 [`Self::theme`] 那条教训）：面板要在结果落地那一刻
        /// 自己把「识别中…」换成字，而一次调用不会重新求值。空串在这里是**两种**状态共用的读数
        /// —— "这一趟还没跑" 与 "跑了、图上没有字"（平台层的 `extract` 对后者交回 `Ok("")`，
        /// 那是识别成功的答案而不是故障），两者靠 [`Self::ocr_busy`] 分开，所以不必再要第四格
        /// （#281：两件事挤进一格，读的人就分不出是哪一件）。
        #[qproperty(QString, ocr_text)]
        /// 「这一趟识别还没回来」那一格 —— PRD §5.6.1 规则「识别期间应显示进度状态」落到的
        /// 那唯一一位读数。
        ///
        /// 为什么由桥写而不是窗自己点下就置真：一次识别要几百毫秒，期间用户可能改选区、可能关
        /// 遮罩，而**只有桥知道活儿什么时候完**（结果是工作线程投回来的）。置真的只有
        /// [`Self::extract_mask_text`] / [`Self::extract_pin_text`] 那两只手，置假的是投回来的
        /// 那一个闭包与「线程起不动」的那一格补救 —— 一处开始、一处结束，不会有一路把它挂在
        /// `true` 上没人管。用 `bool` 而不是本文件惯用的 0/1：这一格只有两态，而多出来的那一档
        /// 「2 是什么」在这一件事上压根没人答得出。
        #[qproperty(bool, ocr_busy)]
        /// 失败的那半句，与 [`Self::ocr_text`] 按 [`Self::ocr_busy`] 一起填成互斥的两格。
        /// §8.6 要「显示失败类型」：这一格写的就是平台层那句原话（`OcrError` 四支各带自己崩在
        /// 哪一步），桥不替它想一句好听的话；§5.6.1 规则要「允许重新框选」—— 那一半由遮罩
        /// 本来就还开着来满足，不在这里另开一扇门。空串 = 这一趟没失败。
        #[qproperty(QString, ocr_error)]
        /// §5.6.2 步骤 4 与 §5.6.3 步骤 3 的那一张表：一枚码一行。编码与 [`Self::groups`]
        /// 同一族 —— 字段之间 `\u{1c}`、行之间 `\u{1d}`，一串机器字给 QML 的 `Repeater` 读，
        /// 人要读的那句在 [`Self::codes_error`] 与 [`Self::status`]。
        ///
        /// 每行的形状：`序号\u{1c}类型\u{1c}内容\u{1c}域名\u{1c}能不能开(1/0)\u{1c}为什么不能开`。
        /// 六格各说一件事（#281）：`类型` 是 §5.6.3 步骤 3 点名要的「条码类型」，`域名` 是
        /// §5.6.2 安全规则点名要的「打开前显示的完整域名」，最后那一格说的是**这一枚链接为什么
        /// 不开**（IDN 那一支）—— 与「这不是链接」分得开，因为后者是 `域名` 空且第五格假、第六格也空。
        ///
        /// `内容` 那一格里出现 `\u{1c}` `\u{1d}` NUL 时换成 U+FFFD：码的内容是**外来数据**，一枚
        /// 二维码里带 0x1C 是完全可能的，而分隔符一旦被内容顶开，界面读出的每一格都错位。这一处
        /// 替换只发生在**显示**那一路 —— 复制与打开走的是 [`Self::copyCodeAt`] / [`Self::openCodeAt`]
        /// 那两只按序号的手，它们碰的是 Rust 自己存着的那一串原样字节，一个字符也不改。
        #[qproperty(QString, codes_rows)]
        /// 「这一趟读码还没回来」。与 [`Self::ocr_busy`] 同一形状、同一理由：读码在另一条线程上跑，
        /// 而**只有桥知道活儿什么时候完**。§5.6.1 那条「识别期间应显示进度状态」的规则对本节同样
        /// 成立，只是 PRD 把那一节写在了文字提取那一条下面。
        #[qproperty(bool, codes_busy)]
        /// 这一趟为什么没有表可摆。§5.6.2 安全规则「对无法识别或损坏的二维码显示明确提示」那一格
        /// 就落在这里：`rxing` 答「这一张里一枚也没有」与答「引擎读不动」是两句原话，桥不替它们
        /// 合并成一句好听的话（[`falcon_core::barcode::BarcodeError`] 两支各一支）。空串 = 有表、没故障。
        #[qproperty(QString, codes_error)]
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

        /// §5.10.1 步骤 1/2 的那一手，两只手势共用一个落点：`add` 非零就是 Ctrl 按着 ——
        /// 加进名单，而同一枚再按一次是**减**；零是普通单击，只留这一枚。交回按完之后名单里
        /// 有几枚。贴图不在集里（菜单陈旧、窗已关）时名单**一枚也不动**，交回的是原本那个数，
        /// 原因走 stderr 而不是 `status`：那是调用方的 bug，不是给用户看的一句「失败」。
        #[qinvokable]
        #[cxx_name = "selectPin"]
        fn select_pin(self: Pin<&mut Self>, id: i64, add: i32) -> i32;

        /// §5.10.1 步骤 4：退出多选。「点空白区域」与「按 Esc」两扇门走的都是这一只手，
        /// 所以场上一处也不会多。交回的是**这一手清掉了多少枚**，不是清完再数（那样永远是 0）。
        #[qinvokable]
        #[cxx_name = "clearSelection"]
        fn clear_selection(self: Pin<&mut Self>) -> i32;

        /// §5.10.2 的**唯一**一只手：一次拖动，`dx`/`dy` 是物理像素的位移，搬动的是**名单**而
        /// 不是被按住的那一扇窗 —— 这一点由 [`crate::state::PinState::drag_pin`] 判，界面一侧
        /// 不许自己分「拖一枚」和「拖一批」（窗的章程写着它不决定这件事）。
        ///
        /// 交回的是**搬动了几枚**：1 是普通的一枚（名单里只有它，或它压根不在名单里 —— §5.9.1
        /// 那一手），2 以上是那一批，0 是这一枚贴图已经不在集里了。0 不发号，也不扮成一次成功
        /// （#455：把「没有搬成」读成「搬成了」，用户就以为自己的拖动搬走了并不存在的一扇窗）。
        #[qinvokable]
        #[cxx_name = "dragPin"]
        fn drag_pin(self: Pin<&mut Self>, id: i64, dx: i32, dy: i32) -> i32;

        /// §5.10.3 的绝对值那一手：`pct` 是新的不透明度百分比，改的是**名单**而不是被按住的那一扇
        /// 窗 —— 这一点由 [`crate::state::PinState::set_pin_opacity`] 判（判据 [`crate::state::PinState::batch_hand`]
        /// 全仓只写那一次），界面一侧不许自己分「调一枚」和「调一批」（窗的章程写着它不决定这件事）。
        ///
        /// 交回的是**调了几枚**，形状与 [`Self::drag_pin`] 那一式相同：1 单枚（名单里只有它，或它
        /// 压根不在名单里 —— §5.9.4 那一手），2 以上是那一批，0 是这一枚贴图已经不在集里了。
        #[qinvokable]
        #[cxx_name = "setPinOpacity"]
        fn set_pin_opacity(self: Pin<&mut Self>, id: i64, pct: i32) -> i32;

        /// §5.10.3 的阶梯那一手：「更透明 / 更实」与 Ctrl+滚轮走的都是这一只落点。它与上一枚的
        /// 分工是**这一枚按每一窗自己当前那一级迈步**（`OPACITY_STEPS`），所以 30% 与 80% 两枚
        /// 一起按「更透明」得到的是 20% 与 70% —— 压成同一个数是上一枚那手的事。
        ///
        /// `up` 非零是「更实」。交回的是**调了几枚**，判据与 [`Self::set_pin_opacity`] 同一式。
        #[qinvokable]
        #[cxx_name = "stepPinOpacity"]
        fn step_pin_opacity(self: Pin<&mut Self>, id: i64, up: bool) -> i32;

        /// §5.10.4「重置」那一手：把视图（缩放 / 旋转 / 位置）还给 `[pin]` 里那些默认值。
        /// 改的是**名单**而不是被点中的那一扇窗，判据仍是 [`crate::state::PinState::batch_hand`]
        /// 那一份 —— 窗一侧不许自己分「重置一枚」和「重置一批」（窗的章程；P134 之前走的正是
        /// `PinView::resetView` 那一条单枚旧路，本轮把它退役）。
        ///
        /// 交回的是**重置了几枚**，形状与 [`Self::drag_pin`] 同一式：1 单枚，2 以上那一批，
        /// 0 是这一枚已经不在集里（0 不发号，也不扮成一次成功 —— #455）。
        #[qinvokable]
        #[cxx_name = "resetPin"]
        fn reset_pin(self: Pin<&mut Self>, id: i64) -> i32;

        /// §5.10.4 / §5.9.18「销毁」那一手：从集里彻底拿走**名单**里的那些枚，唯一回头路是
        /// [`Self::undo_destroy`]。批量与否仍由 [`crate::state::PinState::batch_hand`] 判。
        ///
        /// 交回的是**带走了几枚**；`0` 同上。带走的枚数就是 [`Self::trashed`] 此后那个数，
        /// 而用户在点下去之前读的是 [`Self::destroy_count_for`] —— 三处问的是同一格的账。
        #[qinvokable]
        #[cxx_name = "destroyPin"]
        fn destroy_pin(self: Pin<&mut Self>, id: i64) -> i32;

        /// PRD 第 1149 行「批量销毁时应提示影响数量」的那一位读者：**这一手**会带走几枚。
        ///
        /// 它按 id 问而不是全局报一个数，因为「销毁这一枚」在名单里有一枚和有两枚时带走的
        /// 数量不同；而窗上不许自己算（那是 `batch_hand` 的第二份，#281）。它与
        /// [`Self::destroy_pin`] 读的是同一格 [`crate::state::PinState::destroy_answer`]，
        /// 所以菜单上印的数与真被带走的那些枚不会各走各的。
        #[qinvokable]
        #[cxx_name = "destroyCountFor"]
        fn destroy_count_for(self: Pin<&mut Self>, id: i64) -> i32;

        /// §5.9.18「支持撤销」的那一只手：把最近一手销毁带走的那些枚放回**它们各自离开的那
        /// 一格**（core 的 `PinSlot.index` 记的就是那个位置，所以撤销回来还是原来那堆）。
        ///
        /// 交回的是**还回了几枚**；账是空的就交回 0 且**不发号**（那一手压根没有撤销可做）。
        #[qinvokable]
        #[cxx_name = "undoDestroy"]
        fn undo_destroy(self: Pin<&mut Self>) -> i32;

        /// §5.11 独显：窗上菜单那一句走的手。留哪几枚由 [`crate::state::PinState::hand_targets`]
        /// 答（名单里的一枚＝那一批，名单外的一枚＝只这一枚），所以这一格与 §5.10 那四手读的是
        /// **同一张**表 —— 界面一侧不许自己决定「独显该留几枚」（#281）。
        ///
        /// 交回的是**这一手带走/放回了几枚**；`0` 有三种可能（被拒、退出时没有要放回的、场上
        /// 只剩被留着的那一枚），三者由这一格写进 [`Self::status`] 的那句分开的话说清楚 ——
        /// 一个数分不开的三件事，不该让门或用户去猜（#455）。
        #[qinvokable]
        #[cxx_name = "toggleSolo"]
        fn toggle_solo(self: Pin<&mut Self>, id: i64) -> i32;

        /// §5.11 独显：`ctrl+shift+s` 那一句走的手。它与 [`Self::toggle_solo`] 差的就是**输入**：
        /// 热键没有「被点中的那一枚」，PRD 步骤 1 那句「用户选中一张或多张贴图」就是它的全部
        /// 输入，所以留的是此刻的名单本身；一枚也没选中时 core 直接拒绝，屏幕上出现的是那句
        /// 拒绝而不是「全场暗掉」。
        #[qinvokable]
        #[cxx_name = "toggleSoloSelection"]
        fn toggle_solo_selection(self: Pin<&mut Self>) -> i32;

        /// §5.11 菜单字面与量规共用的那一枚答案：**这一手**会带走几枚（点下去之前问）。
        /// 与 [`Self::toggle_solo`] 读的是同一张 [`crate::state::PinState::solo_kept_table`]，
        /// 所以字面上印着的数与真被带走的那些枚不可能各走各的 —— 与
        /// [`Self::destroy_count_for`] 同一封信、同一个形状。
        #[qinvokable]
        #[cxx_name = "soloCountFor"]
        fn solo_count_for(self: Pin<&mut Self>, id: i64) -> i32;

        /// §5.11「其余那几枚」在这一场是**压暗**还是**隐藏**：读的是配置键
        /// `pin.solo_dim_opacity` 的本体（默认 35 ⇒ 压暗），菜单那句字面靠它才不写死一句假话。
        #[qinvokable]
        #[cxx_name = "soloDims"]
        fn solo_dims(self: Pin<&mut Self>) -> bool;

        /// §5.12.1 步骤 2/3/4：新建一枚空分组。交回**新分组的编号**；失败（空名 / 重名）交回 -1，
        /// 并把 core 那句失败话写进 [`Self::status`]。名字是用户打的 `QString`，桥侧只 trim、
        /// 只判空与重（都走 core 那一格），绝不在界面一侧另立一份命名规则（#281）。
        #[qinvokable]
        #[cxx_name = "createGroup"]
        fn create_group(self: Pin<&mut Self>, name: &QString) -> i64;

        /// §5.12.3「重命名」。成功交回 0、失败交回 -1 并把话写进 `status`。改名不动像素，
        /// 所以这一手**不发** [`Self::group_epoch`]，只重写 [`Self::groups`] 那一串字。
        #[qinvokable]
        #[cxx_name = "renameGroup"]
        fn rename_group(self: Pin<&mut Self>, id: i64, name: &QString) -> i32;

        /// §5.12.3「删除」，带上那句「贴图如何处理」的答案：`destroy` 非 0 ⇒ 随组销毁（成员变，
        /// 走 `count` 收窗），0 ⇒ 退回未分组（成员不变）。交回**被处理了几枚贴图**，失败交回 -1。
        #[qinvokable]
        #[cxx_name = "deleteGroup"]
        fn delete_group(self: Pin<&mut Self>, id: i64, destroy: i32) -> i32;

        /// §5.12.2 步骤 2/3/4：把此刻名单里的那些枚移进 `id` 指的那组；`id` 传 0 是「移出分组」。
        /// 交回**被改归属的枚数**，失败（那组不存在）交回 -1。只改归属，不发 [`Self::group_epoch`]。
        #[qinvokable]
        #[cxx_name = "moveSelectionToGroup"]
        fn move_selection_to_group(self: Pin<&mut Self>, id: i64) -> i32;

        /// §5.12.3「拖动排序」：`delta` 是 ±1（这一组与相邻那组换一格）。成功交回 0，只重写
        /// [`Self::groups`] 那一串字的**顺序**，不动像素也不发号。
        #[qinvokable]
        #[cxx_name = "moveGroup"]
        fn move_group(self: Pin<&mut Self>, id: i64, delta: i32) -> i32;

        /// §5.12.4 步骤 2/3「显示 / 隐藏 / 切换」整组。交回**切换之后显不显**（1 显 / 0 藏），
        /// 失败交回 -1。这一手真的改了组内每一枚的可见性 ⇒ 发 [`Self::group_epoch`]，让全场每扇窗
        /// 重读自己的 `view.live`。
        #[qinvokable]
        #[cxx_name = "toggleGroupShown"]
        fn toggle_group_shown(self: Pin<&mut Self>, id: i64) -> i32;

        // ── §5.12.3 / §5.12.4 那一扇「图片分组管理」窗的**读数**那几枚 ──────────────────────────
        // 它们一只也不改模型：改模型的是上面那五枚（新建 / 重命名 / 排序 / 删除 / 切换）。这一族
        // 存在的理由是 #281 —— 页上那些中文若由 `.qml` 自己写，「钮上写着删除、递进产品的却是
        // 重命名」只能靠数出来；名字的作者在 [`crate::group_window`]，页与门共读同一处。
        // 与 [`Self::historyWindowTitle`] 那一族同一分工。

        /// 窗标题（PRD 第 1218 行那一串字「图片分组管理」的唯一作者）。
        #[qinvokable]
        #[cxx_name = "groupWindowTitle"]
        fn group_window_title(self: &Self) -> QString;

        /// 页上那六枚钮的名字，`|` 分隔。
        #[qinvokable]
        #[cxx_name = "groupWindowButtons"]
        fn group_window_buttons(self: &Self) -> QString;

        /// 「显示中 / 已隐藏」那两串字（第 1252 行那一读的两种态）。
        #[qinvokable]
        #[cxx_name = "groupWindowStateWords"]
        fn group_window_state_words(self: &Self) -> QString;

        /// 规则 1（第 1243 行）那一句问与两枚答钮：`问|退回未分组|随组销毁`。
        #[qinvokable]
        #[cxx_name = "groupWindowDeleteWords"]
        fn group_window_delete_words(self: &Self) -> QString;

        /// 窗自己那枚「关闭」钮的名字。
        #[qinvokable]
        #[cxx_name = "groupWindowCloseWord"]
        fn group_window_close_word(self: &Self) -> QString;

        /// 量具那两条腿要打进那两枚输入框的名字（`新建名|改名名`，空串 = 这一趟没有量具的腿）。
        /// 名字由本档起、由本档判，页上不自己拼一名（那样门判的就成了页自己写的字）。
        #[qinvokable]
        #[cxx_name = "groupWindowNames"]
        fn group_window_names(self: &Self) -> QString;

        /// 那扇窗量具那十条腿的计划（空串 = 这一趟没有量具的腿要跑）。
        #[qinvokable]
        #[cxx_name = "groupWindowPlan"]
        fn group_window_plan(self: &Self) -> QString;

        /// 页上那十条腿读回来的一行原文。
        #[qinvokable]
        #[cxx_name = "groupWindowReport"]
        fn group_window_report(self: &Self, line: &QString);

        /// 一枚贴图**此刻**显不显（1/0）：第 1253 行「系统统一更新该分组内贴图的可见状态」在界面
        /// 一侧唯一可读的那一眼，读的是 [`crate::state::PinState::view_data`] 的 `live` 本体 ——
        /// 与 `PinWindow.qml` 那一眼同一格，桥侧不数第二遍。不在集里的那一枚也答 0，那是「没了」
        /// 与「藏起来」共用的一眼，分界问下面那一枚。
        #[qinvokable]
        #[cxx_name = "groupWindowLive"]
        fn group_window_live(self: &Self, id: i64) -> i32;

        /// 一枚贴图**还在不在集里**（1/0）：§5.12.3 那两臂「退回未分组 / 随组销毁」的唯一分界 ——
        /// 退回的还在场上、销毁的不在了。这一格界面自己读不到（`live` 对两者都答 0），所以它是
        /// 本窗独有的一枚读数，而不是上面那一枚的第二份。
        #[qinvokable]
        #[cxx_name = "groupWindowHeld"]
        fn group_window_held(self: &Self, id: i64) -> i32;

        /// §5.15「隐藏全部贴图」。置起全局收起那枚理由，扇出全场每扇窗 ⇒ 发 [`Self::pins_epoch`]。
        /// 交回切换之后的全局态（恒 `1` = 已收起）。这一手不碰任何一枚贴图自身的可见性。
        #[qinvokable]
        #[cxx_name = "hideAllPins"]
        fn hide_all_pins(self: Pin<&mut Self>) -> i32;

        /// §5.15「显示全部贴图」。落下全局收起那枚理由，扇出全场 ⇒ 发 [`Self::pins_epoch`]。交回
        /// 全局态（恒 `0` = 已放开）。为对话框或整组单独藏起来的那些枚**不因这一手回到桌面**。
        #[qinvokable]
        #[cxx_name = "showAllPins"]
        fn show_all_pins(self: Pin<&mut Self>) -> i32;

        /// §5.15「切换全部贴图」，也是 `toggle_pins` 热键走的那一臂：读当前全局态、取反、扇出 ⇒
        /// 发 [`Self::pins_epoch`]。交回**取反之后**的全局态（1 藏 / 0 显）。
        #[qinvokable]
        #[cxx_name = "toggleAllPins"]
        fn toggle_all_pins(self: Pin<&mut Self>) -> i32;

        /// §5.9.16 - this pin's pixels onto the clipboard, as the user sees them.
        #[qinvokable]
        #[cxx_name = "copyPin"]
        fn copy_pin(self: Pin<&mut Self>, id: i64) -> QString;

        /// §5.9.17 - this pin out to a file; the extension picks the format, and `filter`
        /// is what the dialog's menu says for a name that has none.
        #[qinvokable]
        #[cxx_name = "savePin"]
        fn save_pin(self: Pin<&mut Self>, id: i64, path: &QString, filter: &QString) -> QString;

        /// §5.5.8「刷新截图」的两枚落点共用的一只手。`overwrite` 是**规则 1 那一枚选择**本身
        /// （覆盖 = `true`，另钉一张 = `false`），由菜单那两行各自钉死：桥不替用户决定要哪一种，
        /// PRD 两句都是「可选」。
        ///
        /// 「这一枚配不配刷」「那块区域还在不在屏幕上」都不在这里判 —— 判的人是
        /// [`crate::state::PinState::refresh`] 与 [`falcon_core::refresh::plan`]，连「请重新
        /// 选择」那半句也是那里写的。这一格只做两件事：把答复搬到 [`Self::status`]，以及把名单
        /// 的账重读一遍。
        ///
        /// 收尾那一句 `refresh()` 是**两行一起叫**的：另钉一张会多出一枚贴图，新那扇窗靠 `count`
        /// 那一枚 notify 由 `main.qml` 造出来（与 `destroy_pin` / `undo_destroy` 同一式）；覆盖
        /// 那一路不换成员，`refresh()` 只把四格账重读一遍，幂等。让两行各判一次「要不要扇」，
        /// 就是「这一手有没有改变名单」的第二处（#281）。
        #[qinvokable]
        #[cxx_name = "refreshPin"]
        fn refresh_pin(self: Pin<&mut Self>, id: i64, overwrite: bool) -> QString;

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

        /// 那扇窗的标题（PRD 第 1309 行那一串字）、那四枚钮的名字、表头那枚全选的两个字、
        /// 二次确认那一问与两答：全部出自 [`crate::history_window`]。页上抄一份中文名单，
        /// 改一天「钮上写着删除、递进产品的却是清空」只能靠数出来（#281，与
        /// [`Self::customLabels`] 那一族同一个理由）。
        #[qinvokable]
        #[cxx_name = "historyWindowTitle"]
        fn history_window_title(self: &Self) -> QString;

        #[qinvokable]
        #[cxx_name = "historyWindowButtons"]
        fn history_window_buttons(self: &Self) -> QString;

        #[qinvokable]
        #[cxx_name = "historyWindowSelectAll"]
        fn history_window_select_all(self: &Self) -> QString;

        #[qinvokable]
        #[cxx_name = "historyWindowConfirmWords"]
        fn history_window_confirm_words(self: &Self) -> QString;

        /// §6.2「显示历史记录占用多少空间」在**这一扇窗上**的读法。算术与措辞的唯一作者
        /// 仍是 [`crate::settings::history_usage_line`]；读的落点是 `MaskState` 那两枚**会话快照**
        /// 而不是全局那一枚（理由写在 [`crate::history_window::usage_line`] 头顶）。
        #[qinvokable]
        #[cxx_name = "historyWindowUsage"]
        fn history_window_usage(self: &Self) -> QString;

        /// 那扇窗量具那几条腿的计划（空串 = 这一趟没有量具的腿要跑）。
        #[qinvokable]
        #[cxx_name = "historyWindowPlan"]
        fn history_window_plan(self: &Self) -> QString;

        /// 页上那几条腿读回来的一行原文。
        #[qinvokable]
        #[cxx_name = "historyWindowReport"]
        fn history_window_report(self: &Self, line: &QString);

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

        // ── §5.20.1 的那一扇窗：页上那一排标签、那一页的行、点中的那一枚键 ──────────────
        //
        // 这些入口没有一枚自己判事。类与行的名单来自 [`crate::settings_view`]（它来自
        // `Config::flat()` 与那份名单），改值的闸门来自 `Config::set_str`，落盘来自
        // [`crate::settings::Settings::save`]。界面拿到的只是这些格子读出来的**字串**，
        // 于是「页上写着 5000」与「内存里是 5000」之间不存在第二条路可以各说一面（#281）。

        /// 页顶那排标签：那份名单（§5.20.1 的十一类加「托盘」），顺序就是文档里那一列数字。
        /// `Repeater` 那侧 `split("\u{1d}")` —— 这所房子把 Rust 的列表交给界面的唯一写法，
        /// 而分隔符是控制字符而不是 `|`：行里的**值是用户自己打的**（`annotation.font_family`
        /// 里出现一个 `|` 完全合法），可见分隔符会把一行切成两行。
        #[qinvokable]
        #[cxx_name = "settingsCategories"]
        fn settings_categories(self: &Self) -> QString;

        /// 第 `which` 类那一页的全部行。每行已是页上那一格的样子（`Row::text` 原文，
        /// `（需重启）` 在其内），类号越界交回空串而不是崩：窗不该为一枚不可能的号停机。
        #[qinvokable]
        #[cxx_name = "settingsPage"]
        fn settings_page(self: &Self, which: i32) -> QString;

        /// 页上第 `index` 行管的是哪一枚键、此刻写着什么，两样用 `\u{1d}` 连着交回。窗点行时要的
        /// 是这一对，而不是"把那一格从第一个 `=` 切开"——那是同一份语法的第二份实现（#281），而
        /// 值里出现 `=`（保存路径、字体名）当场切错。空串 = 这一行不存在。
        #[qinvokable]
        #[cxx_name = "settingsRowAt"]
        fn settings_row_at(self: &Self, which: i32, index: i32) -> QString;

        /// 改一行：过语法闸、原子落盘，交回 `Written::text` 那一整句（成了什么、被门改成了
        /// 什么、要不要重启、盘花几毫秒，拒了的话内存里仍是哪一个数）。窗把这句原样印在自己的
        /// 状态行上，不复述、不改写 —— 页上说的话与内存里发生的事因此只能是同一句。
        #[qinvokable]
        #[cxx_name = "settingsSet"]
        fn settings_set(self: &Self, key: &QString, value: &QString) -> QString;

        /// 第 `which` 类每一行要哪一只手：`on` / `off` / `field`，用 `\u{1d}` 连着，与
        /// [`Self::settingsPage`] 切出来的行**一一对应**（长度差一位就是把某行的手型安到了邻居头上）。
        ///
        /// 窗自己不判这一件事：按页上那串文本猜会把一枚恰好写着 `true` 的字体名画成开关，而
        /// 按下去就是把那一格写成 `false` —— 那一步在语法闸上是过得去的。判的是档里存着的**类型**
        /// （`Config::get_bool`），界面只画。
        #[qinvokable]
        #[cxx_name = "settingsRowKinds"]
        fn settings_row_kinds(self: &Self, which: i32) -> QString;

        /// 按下第 `which` 类第 `index` 行上那一枚开关：翻一头，走的与「按保存」**同一条路**
        /// （语法闸、原子落盘、`Written::text` 那一整句）—— 这里不开第二条落盘的路，否则「页上
        /// 按一下」与「页上按保存」会落成两件不同的事。
        ///
        /// 那一行不是一枚开关时，这一枚一处也不动，交回的那一句里写着为什么（页上印的就是那句）。
        #[qinvokable]
        #[cxx_name = "settingsToggle"]
        fn settings_toggle(self: &Self, which: i32, index: i32) -> QString;

        /// `--settings-window` 那一档给窗自己的计划。空串在产品会话与另外七道门里，
        /// 于是 `SettingsWindow.qml` 里那几条腿整条不跑（同 `pinGaugePlan` /
        /// `hintGaugePlan` 那一族的口径：量具的代码不许长成用户看得见的一次点击）。
        #[qinvokable]
        #[cxx_name = "settingsGaugePlan"]
        fn settings_gauge_plan(self: &Self) -> QString;

        /// 窗递回来的一行，进的是那一档自己的账（[`crate::settings_view::report`]）。
        ///
        /// **两档共用这一枚**：`--settings-window` 的四条腿与 §5.20.3 那两枚钮都从这里递。桥上
        /// 要是给它们各开一枚，页上就得自己判断「我是哪一档」—— 那是措辞不是读数，所以分派在
        /// [`crate::settings_view::report`] 那一侧按「哪一档 armed」做。
        #[qinvokable]
        #[cxx_name = "settingsGaugeReport"]
        fn settings_gauge_report(self: &Self, line: &QString);

        /// §5.20.3 那一档给窗的计划：三段（腿等待 ms、导出落到哪份档、导入读哪份档）。
        /// 空串在产品会话与其余各档里，于是页上那两条量具腿整条不跑（与
        /// [`Self::settings_gauge_plan`] 同一个口径）。
        #[qinvokable]
        #[cxx_name = "settingsIoPlan"]
        fn settings_io_plan(self: &Self) -> QString;

        /// 「导出配置」那枚钮背后的一只手。递过去的是路径或 `file:///…`（真人从 `FileDialog`
        /// 拿到的就是后者），Rust 那一头认两种（[`crate::save::local_path`]）。交回的是
        /// [`crate::settings::config_export`] 那一整句 —— 页上只是把它画出来。
        #[qinvokable]
        #[cxx_name = "settingsExportConfig"]
        fn settings_export_config(self: &Self, target: &QString) -> QString;

        /// 「导入配置」那枚钮的第一步（§5.20.3 步骤 4）：总数一句 + 一项一行，`\u{1d}` 连着。
        /// 页拿 [`Self::settings_page`] 同一把刀切开，第 0 枚当抬头、其余画成一列行。
        #[qinvokable]
        #[cxx_name = "settingsImportPreview"]
        fn settings_import_preview(self: &Self, target: &QString) -> QString;

        /// §5.20.3 步骤 5：用户确认之后才真写。与 `--import-config --confirm` 同一只手。
        #[qinvokable]
        #[cxx_name = "settingsImportApply"]
        fn settings_import_apply(self: &Self, target: &QString) -> QString;

        // ── §5.7.22 步骤 1/2/3 那一格「调色板管理」。桥这一侧一处也不判事：哪一枚色在排里、
        // 能不能加、能不能挪、方案存不存得进，全在 [`crate::settings_view`] 那几只手里，而它们
        // 背后是 `colors::Palette`（模型）与 `Config`（语法闸）。页上拿到的每一句都是那几只手
        // 自己说的话（#281）。

        /// `--settings-palette` 那一档给窗的计划：十段。空串在产品会话与其余各档里，于是页上
        /// 那几条量具腿整条不跑（与 [`Self::settings_gauge_plan`] 同一个口径）。
        #[qinvokable]
        #[cxx_name = "settingsPalettePlan"]
        fn settings_palette_plan(self: &Self) -> QString;

        /// 这一格上该写的九枚字（第 0 枚是抬头），与 [`Self::settings_page`] 同一把刀切开。
        #[qinvokable]
        #[cxx_name = "settingsPaletteWords"]
        fn settings_palette_words(self: &Self) -> QString;

        /// 现读的那一排色：一枚一行 `序号|色`。
        #[qinvokable]
        #[cxx_name = "settingsPaletteBand"]
        fn settings_palette_band(self: &Self) -> QString;

        /// 现读的那一列方案：一条一行 `名字|枚数|色 色 …`。
        #[qinvokable]
        #[cxx_name = "settingsPaletteSchemes"]
        fn settings_palette_schemes(self: &Self) -> QString;

        /// 现读的那一排色，**色与色之间一个空格**：量具腿报回的那一行是按 `|` 切的，页上那一串
        /// `序号|色` 会撑爆段，所以报回走这一枚（[`crate::settings_view::pal_band_flat`]）。
        #[qinvokable]
        #[cxx_name = "settingsPaletteBandFlat"]
        fn settings_palette_band_flat(self: &Self) -> QString;

        /// 步骤 2 的四只手。交回的都是 [`crate::settings_view::Written::text`] 那一整句。
        #[qinvokable]
        #[cxx_name = "settingsPaletteAdd"]
        fn settings_palette_add(self: &Self, hex: &QString) -> QString;

        #[qinvokable]
        #[cxx_name = "settingsPaletteDel"]
        fn settings_palette_del(self: &Self, at: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "settingsPaletteMove"]
        fn settings_palette_move(self: &Self, at: i32, delta: i32) -> QString;

        #[qinvokable]
        #[cxx_name = "settingsPaletteEdit"]
        fn settings_palette_edit(self: &Self, at: i32, hex: &QString) -> QString;

        /// 步骤 3 的三只手：把此刻这一排存成一名方案、应用一名方案（动的是排）、删掉一名方案。
        #[qinvokable]
        #[cxx_name = "settingsPaletteSaveScheme"]
        fn settings_palette_save_scheme(self: &Self, name: &QString) -> QString;

        #[qinvokable]
        #[cxx_name = "settingsPaletteApplyScheme"]
        fn settings_palette_apply_scheme(self: &Self, name: &QString) -> QString;

        #[qinvokable]
        #[cxx_name = "settingsPaletteDropScheme"]
        fn settings_palette_drop_scheme(self: &Self, name: &QString) -> QString;

        /// §5.21.2 第四枚：第 `which` 类此刻该不该递那一句「首次使用引导」。**只读**：翻一页
        /// 不算「用户读完了」，所以这一枚一处也不写盘。空串 = 不递（名单外的页、或已经收过的页），
        /// 窗按空串把那一块整个不画。
        #[qinvokable]
        #[cxx_name = "settingsGuide"]
        fn settings_guide(self: &Self, which: i32) -> QString;

        /// 「知道了」那枚钮背后的一只手：把这一页记成「引导递过了」，并落盘。交回的是
        /// [`crate::guide::ack`] 那一整句（收下没收下、写没写成、落在哪份档），页上只是画它。
        #[qinvokable]
        #[cxx_name = "settingsGuideAck"]
        fn settings_guide_ack(self: &Self, which: i32) -> QString;

        /// 那一枚钮上该写的**那两个字**：[`crate::guide::ACK_WORD`] 原文。页上那个 `Text` 画的
        /// 就是这一枚的返回值，所以「引导句里说的是按『知道了』、钮上却写着别的字」这一种坏只有
        /// 从这里过一次桥才读得出来（§5.20.1 单源那一族的屏幕侧，与 `settingsRowKinds` 同一次
        /// 取行的道理一样：窗不自己抄第二份措辞，#281）。
        #[qinvokable]
        #[cxx_name = "settingsGuideAckWord"]
        fn settings_guide_ack_word(self: &Self) -> QString;

        // ── §5.21.1 第 2 步与第 4 步在「关于与更新」那一页页脚的那两枚钮。判决、句子、地址、
        // 闸，一处都不在窗里：问的是 [`falcon_core::update`] 那一层唯一的函数，递回的字来自
        // [`crate::settings_view::update_words`]，开浏览器走的是与条码那一行同一道闸（
        // [`crate::session::open_web`]）。窗自己数类号、自己起钮名、自己想一句「现在不能下载」，
        // 都是这一族要修的坏（#281）。

        /// 第 `which` 类页脚那两枚钮此刻该写的字，`RECORD` 连着两段；**不是**那一页时空串，
        /// 窗按空串把整块不画（与 [`Self::settings_guide`] 同一形状）。
        #[qinvokable]
        #[cxx_name = "updateWords"]
        fn update_words(self: &Self, which: i32) -> QString;

        /// 第 2 步那一次「点击」在页上的归宿：现问一次，把产品那句判决写进 `status`，并把**同
        /// 一次判决**交回的那枚入口地址用 `RECORD` 跟在句子后面。分两段而不是分两次问，是为了
        /// 不让窗拿到「一句新的话、一枚旧的地址」。没有入口时后一段是空串，那枚钮因此画不出来。
        #[qinvokable]
        #[cxx_name = "updateAsk"]
        fn update_ask(self: Pin<&mut Self>) -> QString;

        /// 第 4 步那枚入口按下去的一只手：现问拿**此刻**的地址（清单在两趟之间改了，开的就必须
        /// 是新那一枚，或者照实说没有可开），再过与「打开这一枚」同一道闸。
        #[qinvokable]
        #[cxx_name = "updateOpen"]
        fn update_open(self: Pin<&mut Self>) -> QString;

        // ── §5.3.5「自定义截图」对话框的那一组桥。这一扇窗的**名单、句子、判据、算术**全部住在
        // [`crate::custom`] 与它复用的那几只手里（`capture::size_guard` / `capture::clip_note` /
        // `delay::Countdown` / `config::DELAY_SECONDS_*`），页上只画递回来的东西 —— 与设置页那
        // 一整族同一分工：把 ["X","Y","宽度","高度","延时"] 抄进 `.qml`，改一天「框上写着宽、递进
        // Rust 的是高」就只能靠数出来（#281）。

        /// 五格的名字，`|` 连接（[`crate::custom::labels`]）。页上那五格的标签与门里
        /// 「页上格数 == 名单长度」那一格读的都是这一串。
        #[qinvokable]
        #[cxx_name = "customLabels"]
        fn custom_labels(self: &Self) -> QString;

        /// 每一格旁边那句话，`|` 连接，与 [`Self::custom_labels`] **同序同长**。
        #[qinvokable]
        #[cxx_name = "customHints"]
        fn custom_hints(self: &Self) -> QString;

        /// 窗标题与那一枚钮上的字：分别是 `capture_custom` 在唯一名册里的那一枚名（与托盘那一项、
        /// 设置页那一行同一个名）与「开始截图」那两个字。
        #[qinvokable]
        #[cxx_name = "customTitle"]
        fn custom_title(self: &Self) -> QString;

        #[qinvokable]
        #[cxx_name = "customStartWord"]
        fn custom_start_word(self: &Self) -> QString;

        /// 第 `which` 格**此刻**的文本。窗出生时读它起手（延时那一格开在 `[capture] delay_seconds`
        /// 上），此后不绑 —— `TextField` 里一次赋值就摧毁绑定，屏幕上会留一对说谎的数字
        /// （`CaptureMask.qml` 的 `sizeBar` 记过的那一族）。
        #[qinvokable]
        #[cxx_name = "customCell"]
        fn custom_cell(self: &Self, which: i32) -> QString;

        /// 填第 `which` 格。**只记账不抓图**：一边打字一边冻画面不是谁要的行为。交回的是这一格
        /// 形状的毛病（空串 = 这一格没毛病），并把它写进 [`Self::custom_note`]。
        #[qinvokable]
        #[cxx_name = "customSet"]
        fn custom_set(self: Pin<&mut Self>, which: i32, text: &QString) -> QString;

        /// 按下「开始截图」（PRD 第 302 行）。交回这一刻说给用户的那句话：延时为 0 就是截出来的
        /// 那一块（或那一句拒绝），延时 > 0 就是「还剩几枚数」并挂上 [`Self::custom_left`]。
        #[qinvokable]
        #[cxx_name = "customStart"]
        fn custom_start(self: Pin<&mut Self>) -> QString;

        /// 那枚 `Timer` 每走一步叫一次；数满的那一枚交回截出来的那一块，还在数就交回空串。
        #[qinvokable]
        #[cxx_name = "customTick"]
        fn custom_tick(self: Pin<&mut Self>) -> QString;

        /// 第 `which` 格往上（`delta = 1`）或往下（`delta = -1`）挪一枚 —— PRD 第 309 行「应支持
        /// 键盘方向键微调数值」。页上那两枚键 handler 只叫这一句并把交回的字回填进那一格，
        /// **不在页里算**：算术留在 [`crate::custom::nudge`]，那里它有单测、也有门可问。
        #[qinvokable]
        #[cxx_name = "customNudge"]
        fn custom_nudge(self: Pin<&mut Self>, which: i32, delta: i32) -> QString;

        /// 数满那一刻的「实测 | 计划」两枚毫秒，`|` 连接。页递回 `delay-fired` 那一行时接在后面 ——
        /// 「等了多久」不能让窗自己拿表算，重算的那一刻已经不是数满的那一刻。
        #[qinvokable]
        #[cxx_name = "customWaitLine"]
        fn custom_wait_line(self: &Self) -> QString;

        /// 每枚数之间隔多久，直接给 `Timer.interval`（[`crate::delay::STEP_MS`] 本身）。
        #[qinvokable]
        #[cxx_name = "customStepMs"]
        fn custom_step_ms(self: &Self) -> i32;

        /// `--custom-window` 那一份计划，原文；空串 = 这一趟不是量具跑的，页上那几条腿一条也不跑。
        #[qinvokable]
        #[cxx_name = "customGaugePlan"]
        fn custom_gauge_plan(self: &Self) -> QString;

        /// 窗递回来的一行（`腿名|读数|…`）。打印与判定都在 [`crate::custom::measure`]。
        #[qinvokable]
        #[cxx_name = "customGaugeReport"]
        fn custom_gauge_report(self: &Self, line: &QString);

        /// 把「这一趟该不该有一扇对话框窗」取进 [`Self::custom_open`]，并把 [`crate::custom`] 此刻
        /// 的注脚与倒计时搬到那两格上。与 [`Self::refresh_settings`] 同一个入口形状：档那一头在
        /// `engine.load()` 之前写下计划，而 Session 要等 QML 建出来才存在。
        #[qinvokable]
        #[cxx_name = "refreshCustom"]
        fn refresh_custom(self: Pin<&mut Self>);

        /// 把「这一趟该不该有一扇设置窗」取进 [`Self::settings_open`]。只有 `main.qml` 在
        /// `Component.onCompleted` 里叫一次：档那一头在 `engine.load()` 之前就被
        /// `--settings-window` 写下计划，而 Session 这个实例要等 QML 建出来才存在，
        /// 中间那条路只有 invokable 走得通（`refreshWarm` / `refreshDelay` 同一形状）。
        #[qinvokable]
        #[cxx_name = "refreshSettings"]
        fn refresh_settings(self: Pin<&mut Self>);

        /// 把「这一趟该不该有一扇历史记录管理窗」取进 [`Self::history_window_open`]，与
        /// [`Self::refresh_custom`] 同一枚理由：`--history-window` 那一份计划写在 `engine.load()`
        /// 之前，而 Session 这一枚实例要等 `main.qml` 建出来才存在。少了这一行声明，QML 叫到的
        /// 就是「没有这只手」，那一句只在运行时才说得出。
        #[qinvokable]
        #[cxx_name = "refreshHistoryWindow"]
        fn refresh_history_window(self: Pin<&mut Self>);

        /// 把「这一趟该不该有一扇图片分组管理窗」取进 [`Self::group_window_open`]，与上面两枚
        /// 同一枚理由：`--group-window` 那一份计划写在 `engine.load()` 之前，而 Session 这一枚
        /// 实例要等 `main.qml` 建出来才存在。
        #[qinvokable]
        #[cxx_name = "refreshGroupWindow"]
        fn refresh_group_window(self: Pin<&mut Self>);

        /// §5.6.1 步骤 1：把**此刻缝合完的那一块选区**送去识别。识别在另一条线程上跑，结果从
        /// [`Self::ocr_text`] / [`Self::ocr_error`] 投回来，所以这一只手当场返回、什么都不交：
        /// 它在 GUI 线程上，而让 GUI 线程等一次几百毫秒的识别，僵住的是桌上每一扇窗。
        #[qinvokable]
        #[cxx_name = "extractMaskText"]
        fn extract_mask_text(self: Pin<&mut Self>);

        /// 同一件事，另一个来源：贴图窗递进来的那一枚 id。§5.6.1 的主入口是遮罩，这一条是
        /// 已经钉在桌上的那张图也要能读 —— 与 [`Self::copy_pin`] 同一族「两只门、一个落点」。
        ///
        /// 贴图取的是 `rendered` 那一格，也就是 [`falcon_core::pin::PinItem::render_shown`] 交回的
        /// 那一张：裁切、翻转、旋转、灰度/反色都已经在里面，用户眼里那样就是识别的输入。窗上那枚
        /// **缩放**不在里面（它是 QML 的 transform，不进 `render_shown`），所以放大到 400% 看糊字
        /// 那一档对识别没有影响 —— 这一条边界写在这儿，是因为它看着像 bug 而不是像设计。
        #[qinvokable]
        #[cxx_name = "extractPinText"]
        fn extract_pin_text(self: Pin<&mut Self>, id: i64);

        /// 面板上「复制全部」与「复制选中」那两只钮走的**同一只手**：那一串字由窗递进来，桥只
        /// 负责放上剪贴板并说出结果。不在这里读 [`Self::ocr_text`]：选中那一截只有 QML 知道，
        /// 而一格文本被两只钮各读一遍就会有一遍是过期的（#281）。
        #[qinvokable]
        #[cxx_name = "copyOcrText"]
        fn copy_ocr_text(self: Pin<&mut Self>, text: &QString) -> QString;

        /// §5.6.2 步骤 1 与 §5.6.3 步骤 1 的**同一个**入口（遮罩那一臂）：把此刻缝合完的选区交给
        /// [`falcon_core::barcode`]。一节写「识别二维码」、一节写「识别条形码」，两只手各开一扇门
        /// 就会有两套「读到没读到」的判据，而 `rxing` 那一头答的是一次「这一张里所有的码」——
        /// 一门、一张表、每行自己宣布它是 QR_CODE 还是 CODE_128（§5.6.3 步骤 3 那一问的答）。
        ///
        /// 与 [`Self::extractMaskText`] 同一个理由**当场什么都不交**：读码要跑几十到几百毫秒，
        /// 让 GUI 线程等它，僵住的是桌上每一扇窗。
        #[qinvokable]
        #[cxx_name = "readMaskCodes"]
        fn read_mask_codes(self: Pin<&mut Self>);

        /// 同一件事，另一个来源：贴图窗递进来的那一枚 id。§5.6.2 步骤 1 明写了「或在贴图上打开
        /// 识别菜单」，而这一条与 [`Self::extractPinText`] 取的是**同一格** `rendered`（裁切、翻转、
        /// 旋转、灰度/反色都在里面，窗上那枚缩放不在）。
        #[qinvokable]
        #[cxx_name = "readPinCodes"]
        fn read_pin_codes(self: Pin<&mut Self>, id: i64);

        /// 表里每一行「复制」那一只钮：交回剪贴板的是**序号那一枚码的原样内容**，不是界面上那串
        /// 显示字。按序号而不是把字递过来，为的正是这一句 —— 显示那一格被 [`Self::codes_rows`]
        /// 的转义碰过，而外来数据里那几个字符碰不得（剪贴板上多一个 U+FFFD 就是内容被改了）。
        #[qinvokable]
        #[cxx_name = "copyCodeAt"]
        fn copy_code_at(self: Pin<&mut Self>, index: i32) -> QString;

        /// §5.6.2 步骤 5 与安全规则的那一只手：把序号那一枚码的内容交给默认浏览器。**这一只手
        /// 必须在界面已经把域名摆给用户看之后**才被调用 —— 那一格读数在 [`Self::codes_rows`]
        /// 的第四格里，它与本只手算出来的那枚域名同源（都出自 [`falcon_core::link`]），窗不许
        /// 自己拼一句「确定要打开吗」。
        ///
        /// 交出去的是 Rust 存着的那一串，不是窗递回来的一串：窗要是递了别的，开掉的就不是用户
        /// 看过域名的那一枚。
        #[qinvokable]
        #[cxx_name = "openCodeAt"]
        fn open_code_at(self: Pin<&mut Self>, index: i32) -> QString;
    }

    // Threading is opt-in per QObject; without this declaration `qt_thread()` does
    // not exist on `Session`. It is what lets a message that arrived on the pump
    // thread ask this object - which lives on the GUI thread and is not `Send` - to
    // open a mask. Proven against 6.10.1 in `spike/hello-cxxqt/src/pump.rs`.
    impl cxx_qt::Threading for Session {}
}

use core::pin::Pin;

use cxx_qt::CxxQtType;
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
    selection_count: i32,
    selection_epoch: i32,
    move_epoch: i32,
    opacity_epoch: i32,
    reset_epoch: i32,
    trashed: i32,
    solo_epoch: i32,
    solo_kept: i32,
    groups: QString,
    group_epoch: i32,
    pins_epoch: i32,
    all_hidden: i32,
    settings_open: i32,
    custom_open: i32,
    custom_left: i32,
    custom_note: QString,
    history_window_open: i32,
    group_window_open: i32,
    ocr_text: QString,
    ocr_busy: bool,
    ocr_error: QString,
    /// §5.6.2 / §5.6.3 那三格读数（表、忙、故障），加上下面那一份权威向量。
    codes_rows: QString,
    codes_busy: bool,
    codes_error: QString,
    /// 这一趟读到的码，**权威的那一份**。[`Self::codes_rows`] 是它的显示形（外来数据里那几个
    /// 分隔字符在显示形里被换掉），而 [`Self::copy_code_at`] 与 [`Self::open_code_at`] 读的
    /// 是这里 —— 剪贴板与浏览器拿到的必须是原样那一串。
    ///
    /// 它与那一格 qproperty 由**同一个 GUI 线程回调**一次写定（[`codes_landed`]），所以窗上摆的
    /// 那张表与按序号取到的那一行永远同一趟：不存在「表是新的、向量是旧的」那一格。
    codes: Vec<falcon_core::barcode::Code>,
}

/// §5.12 分组快照的编码：把 core `group_rows()` 那张表编成一整串字，交给 [`Self::groups`]
/// 那一格。字段之间 `\u{1c}`（文件分隔）、组之间 `\u{1d}`（组分隔）—— 两枚控制字符不会出现在
/// 用户打的分组名里，而任何可见分隔符都会（名字是自由文本）。这是给 QML 的 `Repeater` 读的机器
/// 串，不是给人读的一句话；人要读的那句在 [`Self::status`]。
///
/// 每组的形状：`编号\u{1c}名称\u{1c}显不显(1/0)\u{1c}几枚`。顺序就是那张表本身的顺序
/// （core 的 `groups` vec 本体），所以排序在这里是**透传**，不是这一格重新决定。
fn group_encode(rows: &[(u64, String, bool, usize)]) -> String {
    rows.iter()
        .map(|(id, name, shown, count)| {
            format!(
                "{id}\u{1c}{name}\u{1c}{}\u{1c}{count}",
                if *shown { 1 } else { 0 }
            )
        })
        .collect::<Vec<_>>()
        .join("\u{1d}")
}

impl qobject::Session {
    pub fn refresh(mut self: Pin<&mut Self>) {
        // 四格一起重读：`selection_count` 若只由多选那两只 invokable 写，`close` 里 core 顺手
        // 从名单上摘掉的那一枚就永远不会传到这里 —— 屏幕上少了一扇窗，计数却还指着它。
        //
        // `trashed` 走的是同一条腿，理由同一枚：销毁那一只手清的是桥侧那一格账，而它总是与
        // 「少了几扇窗」同桌发生。把它单写在一处 invokable 里，就会有一路的 `close` 把名单
        // 摘掉而窗上那枚「撤销销毁」还亮着（#281 的另一半：同一件事两处各数一遍）。
        let (n, sel, trash, solo, hidden) = state::with(|s| {
            (
                s.ids().len(),
                s.selection_count(),
                s.trashed(),
                s.solo_kept(),
                s.all_pins_hidden(),
            )
        });
        self.as_mut().set_count(n as i32);
        self.as_mut().set_selection_count(sel as i32);
        self.as_mut().set_trashed(trash as i32);
        // §5.11 规则③ 那一格走的是同一条腿，理由也同一枚：独显开着与「少了几扇窗」同桌发生
        // （藏起来的那些枚还在集里，只是不再 `live`），把它单写在某一只 invokable 里，就会有
        // 一路改动让菜单上那句「退出独显」对着一个已经不在独显里的场子亮着（#281 的另一半）。
        self.as_mut().set_solo_kept(solo as i32);
        // §5.15 全局收起那一格走的仍是同一条腿：整场收起与「少了几扇窗」同桌发生，把它单写在
        // 某一只 invokable 里，就会有一路改动（比如随后销毁一枚）让那句「显示全部」对着一个
        // 早已不在收起态的场子亮着（同一封信）。它绑的是**值**，热键按下后界面那一眼自会跟上。
        self.as_mut().set_all_hidden(hidden as i32);
        // §5.12 分组那一串字走的仍是同一条腿：成员（谁进了哪组、有几枚）与「少了几扇窗」同桌
        // 发生 —— 随组销毁会带走贴图，那批贴图所在的组计数就跟着变。把它单写在某一只 invokable
        // 里，就会有一路改动让菜单上那句「组里几枚」对着一个已经少了几枚的场子亮着（同一封信）。
        let rows = state::with(|s| s.group_rows());
        self.as_mut()
            .set_groups(QString::from(group_encode(&rows).as_str()));
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

    /// §5.9.18「关闭」：窗上那一枚菜单项与量规那一拖走的都是这一只手。P134 起它交回的是
    /// **这一手关掉了几枚** —— 名单里有第二枚且被点中的那枚在名单里，关的就是那一批（判据
    /// [`crate::state::PinState::batch_hand`] 全仓只写那一次，这里不再分「一枚/一批」）。
    pub fn close_pin(mut self: Pin<&mut Self>, id: i64) -> i32 {
        let n = state::with(|s| s.close_pin(id as u64)) as i32;
        self.as_mut().refresh();
        n
    }

    /// §5.10.4「重置」：只把视图交给那只手，批量与否由 [`crate::state::PinState::reset_pin`]
    /// 判（判据同一枚 `batch_hand`）。这里**只**管两件事：把 id 交给那只手，和「真重置到了
    /// 才发号」——与 [`Self::drag_pin`] 同一形状，理由也同一封信（#281）。
    pub fn reset_pin(mut self: Pin<&mut Self>, id: i64) -> i32 {
        let done = state::with(|s| s.reset_pin(id as u64)) as i32;
        if done == 0 {
            return 0;
        }
        let next = *self.reset_epoch() + 1;
        self.as_mut().set_reset_epoch(next);
        done
    }

    /// §5.9.18 / §5.10.4「销毁」：带走的枚数由 [`crate::state::PinState::destroy_pin`] 答，
    /// 而**成员**变了（不是「那些枚各自的样子」变了），所以这一手不另发一枚号 —— `count`
    /// 一动，`main.qml` 的 `pinSync` 就把走掉的那些窗收掉。发号是给「集里还是那些枚」那三手
    /// 用的（搬动 / 透明度 / 重置），把它借给销毁就是让两种因由挤进同一格（#281）。
    ///
    /// 唯一额外要做的是把「此刻还欠着几枚撤销」送到界面那一侧：`refresh()` 读的三格里就有它。
    pub fn destroy_pin(mut self: Pin<&mut Self>, id: i64) -> i32 {
        let gone = state::with(|s| s.destroy_pin(id as u64)) as i32;
        self.as_mut().refresh();
        gone
    }

    /// PRD 第 1149 行那一句「批量销毁时应提示影响数量」的读者：菜单在点下去**之前**问一次。
    ///
    /// 这一格不重算任何事，它只是把 [`crate::state::PinState::destroy_answer`] 那一格的答复
    /// 抬过桥 —— 与 [`Self::destroy_pin`] 读的是同一份 `hand_targets`，所以字面上印着的数与
    /// 真被带走的那些枚不可能各走各的（两处各数一遍就是 #281 那一族）。
    pub fn destroy_count_for(self: Pin<&mut Self>, id: i64) -> i32 {
        state::with(|s| s.destroy_answer(id as u64)) as i32
    }

    /// §5.9.18「支持撤销」那一只门后的手。还回来的是**成员**，所以与销毁那一手同样不发号，
    /// 由 `count` 那一枚 notify 让 `main.qml` 重新造窗。
    pub fn undo_destroy(mut self: Pin<&mut Self>) -> i32 {
        let back = state::with(|s| s.restore_pins()) as i32;
        self.as_mut().refresh();
        back
    }

    /// §5.11 独显：两条入口共用的落点。菜单给 `Some(id)`、热键给 `None`，两者在
    /// [`crate::state::PinState::solo_pin`] 那一格并成一条；这里**只**管三件事 —— 发哪一枚号、
    /// 说哪一句话、把 id 与枚数翻过桥。「留哪几枚」「算几枚」一概不在这里判（#281）。
    ///
    /// 三枚号各管各的（#281）：`solo_epoch` 是「有一批窗被独显带走/放回了」，
    /// `selection_epoch` 是「名单被这一手顺手摆过了」，而 `count` 一动不动 —— 独显不带走任何一枚
    /// 贴图，所以 `main.qml` 的 `pinSync` 那一条腿根本不参与这一手（与 [`Self::destroy_pin`]
    /// 那封信恰好相反）。改 0 枚**不发号**，同 [`Self::drag_pin`] 那一判据。
    fn solo_turn(mut self: Pin<&mut Self>, id: Option<u64>) -> i32 {
        // 「这一手之前独显开不开」与这一手本身在同一格问一次：它把「退出时场上没有要放回的
        // 那几枚」与「一枚也没选中、被 core 直接拒绝」分得开 —— core 那枚 `toggle_solo` 的
        // `bool` 把这两件事压成同一个 `false`，正是本轮不拿它当唯一那只手的理由（#455）。
        let (out, open) = state::with(|s| {
            let open = s.solo_kept() > 0;
            (s.solo_pin(id), open)
        });
        self.as_mut().refresh();
        if out.list_moved {
            let next = *self.selection_epoch() + 1;
            self.as_mut().set_selection_epoch(next);
        }
        let said = if out.active {
            if out.changed == 0 {
                "已进入独显：场上没有别的贴图要带走".to_string()
            } else {
                let verb = if state::with(|s| s.solo_dims()) {
                    "压暗"
                } else {
                    "隐藏"
                };
                format!("已进入独显：其余 {} 枚{}", out.changed, verb)
            }
        } else if open {
            format!("已退出独显：放回 {} 枚", out.changed)
        } else if id.is_some() {
            "独显没做成：那一扇窗已经不在了".to_string()
        } else {
            "独显没做成：先选中至少一枚贴图".to_string()
        };
        // §5.11 规则③ 的「屏幕提示」那一臂就写在这一格：`status` 是全场唯一那一句活话，
        // `HintWindow.qml` 已经绑着它（P54/P119 那条腿），所以「当前处于 Solo 状态」这件事
        // 不需要为独显另开一扇窗、也不需要第二份句子（#281）。
        self.as_mut().say(&said);
        if out.changed == 0 {
            return 0;
        }
        let next = *self.solo_epoch() + 1;
        self.as_mut().set_solo_epoch(next);
        out.changed as i32
    }

    /// §5.11 菜单那一句走的桥：把 id 交给共用的那一格，别的什么都不判。
    pub fn toggle_solo(mut self: Pin<&mut Self>, id: i64) -> i32 {
        self.as_mut().solo_turn(Some(id as u64))
    }

    /// §5.11 `ctrl+shift+s` 走的桥：`None` 就是「没有被点中的那一枚，只有此刻的名单」。
    pub fn toggle_solo_selection(mut self: Pin<&mut Self>) -> i32 {
        self.as_mut().solo_turn(None)
    }

    /// 菜单字面在用户点下去**之前**问的那一枚数。这一格不重算任何事，只把
    /// [`crate::state::PinState::solo_answer`] 抬过桥 —— 与 [`Self::toggle_solo`] 读同一张表，
    /// 所以印着的数与真被带走的那些枚不可能各走各的（形状与 [`Self::destroy_count_for`] 同一枚）。
    pub fn solo_count_for(self: Pin<&mut Self>, id: i64) -> i32 {
        state::with(|s| s.solo_answer(Some(id as u64))) as i32
    }

    /// 「其余那几枚」是压暗还是隐藏：`pin.solo_dim_opacity` 那一枚配置键的第一位桥侧读者。
    /// 菜单字面问它，为的是不把 PRD 步骤 3 那句「隐藏」写死在自己的字面上。
    pub fn solo_dims(self: Pin<&mut Self>) -> bool {
        state::with(|s| s.solo_dims())
    }

    /// §5.12 六只手共用的收尾：重写 [`Self::groups`] 那一串字（`refresh` 里做），把一句成功/
    /// 失败的话写进 [`Self::status`]，只在**真的改到可见性**时发 [`Self::group_epoch`]。
    /// `bump` 由调用方交进来 —— 是哪一手改的可见性，只有那一手自己知道，这里不猜（#281）。
    fn group_close(mut self: Pin<&mut Self>, message: &str, bump: bool) {
        self.as_mut().refresh();
        self.as_mut().say(message);
        if bump {
            let next = *self.group_epoch() + 1;
            self.as_mut().set_group_epoch(next);
        }
    }

    /// §5.12.1 新建：成功把新号交回、`status` 说「已新建」；失败交回 -1、`status` 说 core 那句
    /// （空名 / 重名各一样）。新建的是一枚**空组**，不动任何一扇窗的可见性 ⇒ 不发号。
    pub fn create_group(mut self: Pin<&mut Self>, name: &QString) -> i64 {
        let raw = name.to_string();
        match state::with(|s| s.create_group(&raw)) {
            Ok(id) => {
                self.as_mut()
                    .group_close(&format!("已新建分组 {raw}"), false);
                id as i64
            }
            Err(e) => {
                self.as_mut().group_close(&e, false);
                -1
            }
        }
    }

    /// §5.12.3 重命名：成功交回 0、失败交回 -1，两边都把话写进 `status`。改名不动像素 ⇒ 不发号。
    pub fn rename_group(mut self: Pin<&mut Self>, id: i64, name: &QString) -> i32 {
        let raw = name.to_string();
        match state::with(|s| s.rename_group(id as u64, &raw)) {
            Ok(()) => {
                self.as_mut()
                    .group_close(&format!("已重命名为 {raw}"), false);
                0
            }
            Err(e) => {
                self.as_mut().group_close(&e, false);
                -1
            }
        }
    }

    /// §5.12.3 删除 + 那句「贴图如何处理」。随组销毁会带走贴图 ⇒ 成员变，`refresh` 收窗，
    /// 同时那批贴图的可见性状态也随它们没了 —— 但收窗走的是 `count` 那条腿，不借 `group_epoch`
    /// （与 [`Self::destroy_pin`] 同一封信）。退回未分组不动成员也不动可见性 ⇒ 更不发号。
    pub fn delete_group(mut self: Pin<&mut Self>, id: i64, destroy: i32) -> i32 {
        let fate = destroy != 0;
        match state::with(|s| s.delete_group(id as u64, fate)) {
            Ok(n) => {
                // 那两串字的唯一作者是 [`crate::group_window`]：页上那两枚答钮写的就是它们，
                // 产品这句话里出现的也必须是同一串 —— 这里再打一遍中文，「钮上写的」与「产品说的」
                // 就成了两个作者（#281），而 `--group-window` 门里 ⑨⑩ 两格正是拿这句话找归宿的。
                let how = if fate {
                    crate::group_window::DESTROY_WORD
                } else {
                    crate::group_window::KEEP_WORD
                };
                let msg = if n == 0 {
                    "已删除分组（组里没有贴图）".to_string()
                } else {
                    format!("已删除分组：{n} 枚贴图{how}")
                };
                self.as_mut().group_close(&msg, false);
                n as i32
            }
            Err(e) => {
                self.as_mut().group_close(&e, false);
                -1
            }
        }
    }

    /// §5.12.2 把名单移进某组（`id`=0 是移出）。只改归属、不动像素 ⇒ 不发号；但重写 `groups`
    /// 让「组里几枚」那几个数跟着变。没选中任何一枚时 core 交回 0 枚，那句实话照说（0 枚不是错）。
    pub fn move_selection_to_group(mut self: Pin<&mut Self>, id: i64) -> i32 {
        let target = if id == 0 { None } else { Some(id as u64) };
        match state::with(|s| s.assign_selection_to_group(target)) {
            Ok(n) => {
                let msg = match target {
                    None => format!("已把 {n} 枚移出分组"),
                    Some(_) => format!("已把 {n} 枚移入分组"),
                };
                self.as_mut().group_close(&msg, false);
                n as i32
            }
            Err(e) => {
                self.as_mut().group_close(&e, false);
                -1
            }
        }
    }

    /// §5.12.3 拖动排序：`delta` ±1。成功交回 0、只让那一串字换个顺序；失败（那组不在）交回 -1。
    /// 都不动像素 ⇒ 不发号。
    pub fn move_group(mut self: Pin<&mut Self>, id: i64, delta: i32) -> i32 {
        match state::with(|s| s.move_group(id as u64, delta)) {
            Ok(()) => {
                self.as_mut().group_close("已调整分组顺序", false);
                0
            }
            Err(e) => {
                self.as_mut().group_close(&e, false);
                -1
            }
        }
    }

    /// §5.12.4 显示/隐藏/切换整组：这一手**真的**改了组内每一枚的可见性 ⇒ 发 `group_epoch`，
    /// 让全场每扇窗重读 `view.live`。交回切换之后显不显（1/0），失败交回 -1。
    pub fn toggle_group_shown(mut self: Pin<&mut Self>, id: i64) -> i32 {
        match state::with(|s| s.toggle_group_shown(id as u64)) {
            Ok(shown) => {
                let msg = if shown {
                    "已显示该分组"
                } else {
                    "已隐藏该分组"
                };
                self.as_mut().group_close(msg, true);
                i32::from(shown)
            }
            Err(e) => {
                self.as_mut().group_close(&e, false);
                -1
            }
        }
    }

    pub fn group_window_title(&self) -> QString {
        QString::from(crate::group_window::title().as_str())
    }

    pub fn group_window_buttons(&self) -> QString {
        QString::from(crate::group_window::buttons().as_str())
    }

    pub fn group_window_state_words(&self) -> QString {
        QString::from(crate::group_window::state_words().as_str())
    }

    pub fn group_window_delete_words(&self) -> QString {
        QString::from(crate::group_window::delete_words().as_str())
    }

    pub fn group_window_close_word(&self) -> QString {
        QString::from(crate::group_window::CLOSE_WORD)
    }

    pub fn group_window_names(&self) -> QString {
        QString::from(crate::group_window::planted_names().as_str())
    }

    pub fn group_window_plan(&self) -> QString {
        QString::from(crate::group_window::plan_line().as_str())
    }

    pub fn group_window_report(&self, line: &QString) {
        crate::group_window::report(&line.to_string());
    }

    pub fn group_window_live(&self, id: i64) -> i32 {
        i32::from(state::with(|s| s.view_data(id as u64).live))
    }

    pub fn group_window_held(&self, id: i64) -> i32 {
        i32::from(state::with(|s| s.set.get(id as u64).is_some()))
    }

    /// §5.15 三手共用的收尾：先 [`Self::refresh`]（它已把 `all_hidden` 那一格重读上），再报一句
    /// `status`，最后 +1 号让**每一扇**窗重读自己那格 `view.live`。发号是这一族唯一的收窗路径 ——
    /// 全局收起动的是所有窗，被藏起来的那些恰恰是这一手的目标，只刷被点中的那扇会放过它们。
    fn pins_fanout(mut self: Pin<&mut Self>, message: &str) {
        self.as_mut().refresh();
        self.as_mut().say(message);
        let next = *self.pins_epoch() + 1;
        self.as_mut().set_pins_epoch(next);
    }

    /// §5.15「隐藏全部贴图」：置起全局收起那枚理由，扇出全场。恒交回 `1`（已收起）。这一手不碰
    /// 任何一枚贴图自身可见性，也不与对话框遮挡／整组隐藏共用那一格（#281 各占一格）。
    pub fn hide_all_pins(mut self: Pin<&mut Self>) -> i32 {
        state::with(|s| s.hide_all_pins());
        self.as_mut().pins_fanout("已隐藏全部贴图");
        1
    }

    /// §5.15「显示全部贴图」：落下全局收起那枚理由，扇出全场。恒交回 `0`（已放开）。为对话框或
    /// 整组单独藏起来的那些枚**不因这一手回到桌面** —— 那是另外两桩事。
    pub fn show_all_pins(mut self: Pin<&mut Self>) -> i32 {
        state::with(|s| s.show_all_pins());
        self.as_mut().pins_fanout("已显示全部贴图");
        0
    }

    /// §5.15「切换全部贴图」，也是 `toggle_pins` 热键那一臂：取反全局态、扇出。交回**取反之后**的
    /// 全局态（1 藏 / 0 显），让界面那一眼与桥侧那句号读同一个数。
    pub fn toggle_all_pins(mut self: Pin<&mut Self>) -> i32 {
        let hidden = state::with(|s| s.toggle_all_pins());
        let msg = if hidden {
            "已隐藏全部贴图"
        } else {
            "已显示全部贴图"
        };
        self.as_mut().pins_fanout(msg);
        i32::from(hidden)
    }

    /// §5.10.1 步骤 1/2 的两只手势在这一格合成一只手：`add` 就是「Ctrl 按着没按」。
    /// 名单变了就 +1 号，让**每一扇**窗重读自己那一格 `selected` —— 被点中的那扇自己会
    /// `reload`，别的窗可不会自己把描边摘掉。
    pub fn select_pin(mut self: Pin<&mut Self>, id: i64, add: i32) -> i32 {
        match state::with(|s| s.select_pin(id as u64, add != 0)) {
            Ok(n) => {
                let next = *self.selection_epoch() + 1;
                self.as_mut().set_selection_epoch(next);
                self.as_mut().set_selection_count(n as i32);
                n as i32
            }
            Err(e) => {
                eprintln!("[session] selectPin refused: {e}");
                *self.selection_count()
            }
        }
    }

    /// §5.10.1 步骤 4：退出多选，两扇门（点空白 / Esc）共用这一只手。
    ///
    /// 本来一枚也没有时**不发号**：那一手没有改变任何一扇窗的样子，让所有窗为它重读一遍像素
    /// 是把一次空手势算成一次事件 —— 与 [`Self::dialog_opened`] 里「没有贴图回来就一次
    /// `reload` 也不发」同一判据。交回的枚数仍是 0，那是实话而不是「没做」。
    pub fn clear_selection(mut self: Pin<&mut Self>) -> i32 {
        let took = state::with(|s| s.clear_selection());
        if took == 0 {
            return 0;
        }
        let next = *self.selection_epoch() + 1;
        self.as_mut().set_selection_epoch(next);
        self.as_mut().set_selection_count(0);
        took as i32
    }

    /// §5.10.2：一次拖动只到这一格，批量与否由 [`crate::state::PinState::drag_pin`] 判。
    ///
    /// 这里**只**管两件事：把位移交给那只手，和「真搬动了才发号」。不在这里分一枚/一批就是
    /// 第二处判断 —— 而第二处判断会与这只手在 selftest 里那四臂各读到的数对不上（#281 那一族：
    /// 同一件事两处各数一遍）。
    pub fn drag_pin(mut self: Pin<&mut Self>, id: i64, dx: i32, dy: i32) -> i32 {
        let moved = state::with(|s| s.drag_pin(id as u64, dx, dy)) as i32;
        if moved == 0 {
            return 0;
        }
        let next = *self.move_epoch() + 1;
        self.as_mut().set_move_epoch(next);
        moved
    }

    /// §5.10.3：绝对值那一手只到这一格，批量与否由 [`crate::state::PinState::set_pin_opacity`] 判，
    /// 判据全仓只有那一份（`batch_hand`）。这里**只**管两件事：把百分比交给那只手，和「真调到了
    /// 才发号」——与 [`Self::drag_pin`] 同一形状，理由也同一封信（#281）。
    pub fn set_pin_opacity(mut self: Pin<&mut Self>, id: i64, pct: i32) -> i32 {
        let done = state::with(|s| s.set_pin_opacity(id as u64, pct.max(0) as u32)) as i32;
        if done == 0 {
            return 0;
        }
        let next = *self.opacity_epoch() + 1;
        self.as_mut().set_opacity_epoch(next);
        done
    }

    /// §5.10.3：阶梯那一手（「更透明 / 更实」与 Ctrl+滚轮共用的那只落点）。发号与上一枚同一判据。
    pub fn step_pin_opacity(mut self: Pin<&mut Self>, id: i64, up: bool) -> i32 {
        let done = state::with(|s| s.step_pin_opacity(id as u64, up)) as i32;
        if done == 0 {
            return 0;
        }
        let next = *self.opacity_epoch() + 1;
        self.as_mut().set_opacity_epoch(next);
        done
    }

    /// The three output paths and the one input path all answer the same way: a
    /// line of Chinese the user can read, kept on `status` so a hint widget can
    /// bind to it later without this call being re-run.
    fn say(mut self: Pin<&mut Self>, message: &str) -> QString {
        self.as_mut().set_status(QString::from(message));
        QString::from(message)
    }

    /// §5.9.16 step 2's door: the menu's 复制图片 and the pin's `Ctrl+C` both come here,
    /// so neither one gets its own idea of what "copied" is worth reporting.
    ///
    /// `[output] copy_as_file` (§5.9.17 step 1) chooses which copy runs, and the setting
    /// is read here rather than at each of the two doors - one door, one reading, so a
    /// menu click and a key press cannot answer from different snapshots of the file.
    pub fn copy_pin(mut self: Pin<&mut Self>, id: i64) -> QString {
        let message = if crate::settings::output_cfg().copy_as_file {
            match state::with(|s| s.copy_image_with_files(id as u64)) {
                Ok((pixels, path)) => {
                    format!("已复制贴图（{pixels} 像素），文件引用 {}", path.display())
                }
                Err(e) => format!("复制失败：{e}"),
            }
        } else {
            match state::with(|s| s.copy_image(id as u64)) {
                Ok(pixels) => format!("已复制贴图（{pixels} 像素）"),
                Err(e) => format!("复制失败：{e}"),
            }
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

    /// §5.5.8 的手柄：菜单那两行按下的一行决定 `overwrite` 是哪一枚，其余全在
    /// [`crate::state::PinState::refresh`] 那一格 —— 这里既不算坐标，也不写拒绝那半句。
    ///
    /// 两臂都到 [`Self::status`]：成功那句带着「读回了哪一块、哪个后端、多少 ms」，失败那句
    /// 是规则 2 要的「请重新选择」。用户读的是同一格，量规读的也是同一格。
    pub fn refresh_pin(mut self: Pin<&mut Self>, id: i64, overwrite: bool) -> QString {
        let message = match state::with(|s| s.refresh(id as u64, overwrite)) {
            Ok(done) => done,
            Err(e) => e,
        };
        self.as_mut().refresh();
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

    pub fn history_window_title(&self) -> QString {
        QString::from(crate::history_window::title().as_str())
    }

    pub fn history_window_buttons(&self) -> QString {
        QString::from(crate::history_window::buttons().as_str())
    }

    pub fn history_window_select_all(&self) -> QString {
        QString::from(crate::history_window::select_all().as_str())
    }

    pub fn history_window_confirm_words(&self) -> QString {
        QString::from(crate::history_window::confirm_words().as_str())
    }

    pub fn history_window_usage(&self) -> QString {
        QString::from(crate::history_window::usage_line().as_str())
    }

    pub fn history_window_plan(&self) -> QString {
        QString::from(crate::history_window::plan_line().as_str())
    }

    pub fn history_window_report(&self, line: &QString) {
        crate::history_window::report(&line.to_string());
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

    // ── §5.20.1 那一扇窗的读侧。每一枚都只是把 [`crate::settings_view`] 那一格抬过桥，
    // 一处也不在这里判事：类、行、键、闸门、落盘全在那一档，所以窗上印着的话与内存里发生的
    // 事之间没有第二条路可以各说一面（#281）。

    pub fn settings_categories(&self) -> QString {
        QString::from(crate::settings_view::category_line().as_str())
    }

    pub fn settings_page(&self, which: i32) -> QString {
        // 负数与越界都交回空串：那一页压根不存在，而窗不该为一枚不可能的号停机。
        // 「这一类有页但一行也没有」走的是同一条路（`page_line` 自己编成空串），
        // 那种事在 `--settings` 那一档的头一格是红的，不靠这里分。
        match usize::try_from(which) {
            Ok(i) => QString::from(crate::settings_view::page_line(i).as_str()),
            Err(_) => QString::default(),
        }
    }

    pub fn settings_row_at(&self, which: i32, index: i32) -> QString {
        let (Ok(i), Ok(k)) = (usize::try_from(which), usize::try_from(index)) else {
            return QString::default();
        };
        QString::from(
            crate::settings_view::row_pair(i, k)
                .unwrap_or_default()
                .as_str(),
        )
    }

    pub fn settings_set(&self, key: &QString, value: &QString) -> QString {
        // 闸门与落盘都在 `settings_view::write` 那一格里（它调 `Config::set_str` 与
        // `Settings::save`），本条只把那句它自己说的话交回去。窗不许在这里替失败想一句好听的话。
        let said = crate::settings_view::write(&key.to_string(), &value.to_string()).text();
        QString::from(said.as_str())
    }

    pub fn settings_row_kinds(&self, which: i32) -> QString {
        // 与 `settings_page` 同一个口径：越界与负数交回空串，那一页压根不存在，窗因此一格开关
        // 也不画。长度对不对不是这里判的 —— `kinds_line` 与 `page_line` 出自同一份档的同一趟
        // 枚举，那一件事有单测盯着（`the_kind_line_lines_up_with_the_page_line`）。
        match usize::try_from(which) {
            Ok(i) => QString::from(crate::settings_view::kinds_line(i).as_str()),
            Err(_) => QString::default(),
        }
    }

    pub fn settings_toggle(&self, which: i32, index: i32) -> QString {
        // 翻一头之后落不落盘、落成什么，全在 `settings_view::toggle` 与它背后的 `write` 里；
        // 本条只把那句它自己说的话交回去 —— 与 `settings_set` 同一只手，不是第二只手。
        let said = crate::settings_view::toggle_row(which, index).text();
        QString::from(said.as_str())
    }

    pub fn settings_gauge_plan(&self) -> QString {
        QString::from(crate::settings_view::plan_line().as_str())
    }

    pub fn settings_gauge_report(&self, line: &QString) {
        crate::settings_view::report(&line.to_string());
    }

    // ── §5.20.3 那两枚钮。与上面同一族：桥这一侧一处也不判事 —— 写哪份档、会盖掉哪几枚、
    // 应用之后校正了什么，全在 `settings_view` 与它背后的 `settings` / `Config` 里。
    // 页上拿到的每一句都是那几只手自己说的话（#281）。

    pub fn settings_io_plan(&self) -> QString {
        QString::from(crate::settings_view::io_plan_line().as_str())
    }

    pub fn settings_export_config(&self, target: &QString) -> QString {
        QString::from(crate::settings_view::io_export(&target.to_string()).as_str())
    }

    pub fn settings_import_preview(&self, target: &QString) -> QString {
        QString::from(crate::settings_view::io_preview_line(&target.to_string()).as_str())
    }

    pub fn settings_import_apply(&self, target: &QString) -> QString {
        QString::from(crate::settings_view::io_apply(&target.to_string()).as_str())
    }

    // ── §5.7.22 那一格「调色板管理」。与上面同一族：桥这一侧只把 `settings_view` 那几只手自己
    // 说的话交回去，页上因此不需要知道「加一枚色要不要先去重」。

    pub fn settings_palette_plan(&self) -> QString {
        QString::from(crate::settings_view::pal_plan_line().as_str())
    }

    pub fn settings_palette_words(&self) -> QString {
        QString::from(crate::settings_view::pal_word_line().as_str())
    }

    pub fn settings_palette_band(&self) -> QString {
        QString::from(crate::settings_view::pal_band_now().as_str())
    }

    pub fn settings_palette_schemes(&self) -> QString {
        QString::from(crate::settings_view::pal_scheme_now().as_str())
    }

    pub fn settings_palette_band_flat(&self) -> QString {
        QString::from(crate::settings_view::pal_band_flat_now().as_str())
    }

    pub fn settings_palette_add(&self, hex: &QString) -> QString {
        let said = crate::settings_view::pal_add(&hex.to_string()).text();
        QString::from(said.as_str())
    }

    pub fn settings_palette_del(&self, at: i32) -> QString {
        let said = crate::settings_view::pal_remove(at).text();
        QString::from(said.as_str())
    }

    pub fn settings_palette_move(&self, at: i32, delta: i32) -> QString {
        let said = crate::settings_view::pal_move(at, delta).text();
        QString::from(said.as_str())
    }

    pub fn settings_palette_edit(&self, at: i32, hex: &QString) -> QString {
        let said = crate::settings_view::pal_edit(at, &hex.to_string()).text();
        QString::from(said.as_str())
    }

    pub fn settings_palette_save_scheme(&self, name: &QString) -> QString {
        let said = crate::settings_view::pal_save_scheme(&name.to_string()).text();
        QString::from(said.as_str())
    }

    pub fn settings_palette_apply_scheme(&self, name: &QString) -> QString {
        let said = crate::settings_view::pal_apply_scheme(&name.to_string()).text();
        QString::from(said.as_str())
    }

    pub fn settings_palette_drop_scheme(&self, name: &QString) -> QString {
        let said = crate::settings_view::pal_drop_scheme(&name.to_string()).text();
        QString::from(said.as_str())
    }

    // ── §5.21.2 第四枚：复杂页那一句「首次使用引导」。窗拿到的还是**一句字串**，判「这一页
    // 复杂吗、这一页递过没有」的全在 [`crate::guide`] 那一头 —— 窗里再判一次就是第二份实现，
    // 而第二份实现会的坏是「页上还有那句、盘上已经记成收过了」这一种。

    pub fn settings_guide(&self, which: i32) -> QString {
        QString::from(
            crate::guide::line_for(
                &crate::guide::current(),
                usize::try_from(which).unwrap_or(usize::MAX),
            )
            .as_str(),
        )
    }

    pub fn settings_guide_ack(&self, which: i32) -> QString {
        QString::from(crate::guide::ack(usize::try_from(which).unwrap_or(usize::MAX)).as_str())
    }

    pub fn settings_guide_ack_word(&self) -> QString {
        QString::from(crate::guide::ACK_WORD)
    }

    // ── §5.21.1 那两枚钮的桥。与上面同一族：这里一处也不判事 —— 哪一页递、递什么字在
    // [`crate::settings_view`]，判决与那句产品话在 [`falcon_core::update`]，闸在
    // [`falcon_core::link`] 与 [`platform_windows::open`]。

    pub fn update_words(&self, which: i32) -> QString {
        // 负数与越界交回空串，与 `settings_page` 同一个口径：那一页压根不存在，页脚也就没有
        // 任何东西可画。「有这一页但这一页不递更新」走的是同一条路（`update_words` 自己编空串），
        // 那一件事有单测盯着（`only_the_about_page_offers_the_update_buttons`）。
        match usize::try_from(which) {
            Ok(i) => QString::from(crate::settings_view::update_words(i).as_str()),
            Err(_) => QString::default(),
        }
    }

    pub fn update_ask(mut self: Pin<&mut Self>) -> QString {
        let (_feed, _channel, answer) = ask_update_now();
        // 句子与地址出自这一枚 `answer`，中间不第二次问档：第 4 步要显示的三样内容自此是
        // **同一次**判决的三样。窗拿到的第一段就是要写进 `status` 的那句原话（`say` 已经写了）。
        let sentence = answer.sentence();
        let url = answer.download_url().unwrap_or_default().to_string();
        self.as_mut().say(&sentence);
        QString::from(&*format!("{sentence}{}{url}", crate::settings_view::RECORD))
    }

    pub fn update_open(mut self: Pin<&mut Self>) -> QString {
        let (_feed, _channel, answer) = ask_update_now();
        let said = match answer.download_url() {
            Some(url) => open_web(url),
            // 没有地址可开：让产品自己那句话去说为什么（`Available` 那一格写「清单没写入口」、
            // `NoFeed` 写「还没配发布清单」）。窗不许在这里另想一句「现在不能下载」——那一句会
            // 是第二份措辞，而它说的明明是同一件事。
            None => answer.sentence(),
        };
        self.as_mut().say(&said)
    }

    pub fn refresh_settings(mut self: Pin<&mut Self>) {
        self.as_mut()
            .set_settings_open(i32::from(crate::settings_view::window_wanted()));
    }

    // ── §5.3.5 那一扇对话框的桥。每一枚都只是**搬**：名单、句子、判据、算术在 [`crate::custom`]，
    // 画在 `qml/CustomCaptureWindow.qml`，这一层把它们对上号。

    pub fn custom_labels(&self) -> QString {
        QString::from(crate::custom::labels().as_str())
    }

    pub fn custom_hints(&self) -> QString {
        QString::from(crate::custom::hints().as_str())
    }

    pub fn custom_title(&self) -> QString {
        QString::from(crate::custom::title().as_str())
    }

    pub fn custom_start_word(&self) -> QString {
        QString::from(crate::custom::START_WORD)
    }

    pub fn custom_cell(&self, which: i32) -> QString {
        QString::from(crate::custom::text(usize::try_from(which).unwrap_or(usize::MAX)).as_str())
    }

    pub fn custom_set(mut self: Pin<&mut Self>, which: i32, text: &QString) -> QString {
        let said = crate::custom::set(
            usize::try_from(which).unwrap_or(usize::MAX),
            &text.to_string(),
        );
        self.as_mut().set_custom_note(QString::from(said.as_str()));
        QString::from(said.as_str())
    }

    pub fn custom_start(mut self: Pin<&mut Self>) -> QString {
        let said = crate::custom::start();
        self.as_mut().set_custom_note(QString::from(said.as_str()));
        self.as_mut().set_custom_left(crate::custom::left() as i32);
        QString::from(said.as_str())
    }

    pub fn custom_tick(mut self: Pin<&mut Self>) -> QString {
        let said = crate::custom::tick();
        // 还在数的那一枚交回空串，而**那句「还剩几秒」不在这里说**：页上那一行绑的是
        // `custom_left` 这枚值，每数一枚自会重新求值。这里若也写一次注脚，空串就会把按下那一刻
        // 那句「延时 N 秒开始截图」擦掉，屏幕上倒计时那一行反倒成了唯一的读数。
        if !said.is_empty() {
            self.as_mut().set_custom_note(QString::from(said.as_str()));
        }
        self.as_mut().set_custom_left(crate::custom::left() as i32);
        QString::from(said.as_str())
    }

    pub fn custom_nudge(mut self: Pin<&mut Self>, which: i32, delta: i32) -> QString {
        let moved = crate::custom::nudge(usize::try_from(which).unwrap_or(usize::MAX), delta);
        // 挪过的那一格仍走 `set`，所以那一格的形状注脚已经在 `nudge` 里改过一次；这里把它搬上
        // qproperty，页上那一句读数自会跟上 —— 页不自己判「这一格对不对」。
        self.as_mut()
            .set_custom_note(QString::from(crate::custom::note().as_str()));
        QString::from(moved.as_str())
    }

    pub fn custom_wait_line(&self) -> QString {
        QString::from(crate::custom::wait_line().as_str())
    }

    pub fn custom_step_ms(&self) -> i32 {
        crate::custom::step_ms() as i32
    }

    pub fn custom_gauge_plan(&self) -> QString {
        QString::from(crate::custom::plan_line().as_str())
    }

    pub fn custom_gauge_report(&self, line: &QString) {
        crate::custom::report(&line.to_string());
    }

    pub fn refresh_custom(mut self: Pin<&mut Self>) {
        self.as_mut()
            .set_custom_open(i32::from(crate::custom::window_wanted()));
        self.as_mut().set_custom_left(crate::custom::left() as i32);
        self.as_mut()
            .set_custom_note(QString::from(crate::custom::note().as_str()));
    }

    /// 把「这一趟该不该有一扇历史记录管理窗」取进 [`Self::history_window_open`]。
    /// 与 [`Self::refresh_custom`] 同一分工：只搬读数，造不造窗归 `main.qml`。
    pub fn refresh_history_window(mut self: Pin<&mut Self>) {
        self.as_mut()
            .set_history_window_open(i32::from(crate::history_window::window_wanted()));
    }

    /// 把「这一趟该不该有一扇图片分组管理窗」取进 [`Self::group_window_open`]。同一分工：
    /// 只搬读数，造不造窗归 `main.qml`。
    pub fn refresh_group_window(mut self: Pin<&mut Self>) {
        self.as_mut()
            .set_group_window_open(i32::from(crate::group_window::window_wanted()));
    }

    // ── §5.6.1 文字提取。那三格读数只由下面这一条腿写：两只入口各起一次线程，结果投回
    // GUI 线程落地。识别本体在 [`platform_windows::ocr`] —— 桥这一侧不判「认得出认不出」，
    // 只搬像素和那一句话（#281）。

    pub fn extract_mask_text(mut self: Pin<&mut Self>) {
        let frame = mask::with(|m| m.delivered());
        self.as_mut().run_ocr(frame, "选区");
    }

    pub fn extract_pin_text(mut self: Pin<&mut Self>, id: i64) {
        let frame = state::with(|s| s.with_rendered(id as u64, |f| f.clone()));
        self.as_mut().run_ocr(frame, "贴图");
    }

    /// 「复制全部」与「复制选中」那两只钮共用的那一只：那一串字由窗递进来，桥不判它是全部
    /// 还是半截，也不去读 [`Self::ocr_text`]（读一遍就有一遍是过期的）。
    pub fn copy_ocr_text(mut self: Pin<&mut Self>, text: &QString) -> QString {
        let said = copy_words(&text.to_string());
        self.as_mut().say(&said)
    }

    /// 两条入口共用的那一段：置忙，把这张 `Frame` 交给一条起了名的工作线程，结果投回 GUI
    /// 线程写那三格。
    ///
    /// 为什么非要另起线程：`extract` 在系统线程池上要跑几百毫秒，而这一只手是从 QML 的一次
    /// 点击里调进来的 —— 那就是 GUI 线程。在它上面等结果，僵住的是桌上**每一扇**窗（遮罩、
    /// 贴图、气泡都泵在同一条线程上）。工作线程只碰它自己拥有的两样：那份像素（`Frame` 是
    /// `Send`）与投回去的那一句。
    fn run_ocr(
        mut self: Pin<&mut Self>,
        frame: Option<falcon_core::frame::Frame>,
        label: &'static str,
    ) {
        let Some(frame) = frame else {
            // 一张都没喂进去，这不许算「识别到零行」：空结果必须是引擎真跑过一遍交回的答案，
            // 否则 §8.6 要的失败类型就被写成了一句成功的话。
            self.as_mut().set_ocr_busy(false);
            self.as_mut().set_ocr_text(QString::default());
            self.as_mut().set_ocr_error(QString::from(
                format!("{label}：此刻没有可识别的像素。").as_str(),
            ));
            return;
        };
        self.as_mut().set_ocr_busy(true);
        self.as_mut().set_ocr_text(QString::default());
        self.as_mut().set_ocr_error(QString::default());
        let handle = self.qt_thread();
        let spawned = std::thread::Builder::new()
            .name("falconshot-ocr".to_string())
            .spawn(move || {
                let outcome = platform_windows::ocr::extract(&frame);
                // `queue` 既不阻塞这条线程，也不在对象已毁时 panic —— 条件是读它的结果而不是
                // unwrap（cxx-qt 自己的口径，`hotkey.rs` 那一路用的是同一枚写法）。
                if let Err(e) = handle.queue(move |session| ocr_landed(outcome, session)) {
                    eprintln!("falconshot: 识别结果投不回 GUI 线程（{e}）");
                }
            });
        if let Err(e) = spawned {
            // 线程起不动就当场把忙放下来：挂在 `true` 的那一格会让面板一直说「识别中…」而
            // 什么都不等 —— 那一格的本份是「有活儿在跑」，没有活儿就得是假。
            self.as_mut().set_ocr_busy(false);
            self.as_mut().set_ocr_error(QString::from(
                format!("{label}：起不动识别线程（{e}）。").as_str(),
            ));
        }
    }

    // ── §5.6.2 与 §5.6.3 读码。那一格表、那一格忙、那一格故障，加上 Rust 自己存着的那一份
    // 权威内容，只由下面这一条腿写：两只入口各起一次线程，结果投回 GUI 线程落地。读的本体在
    // [`falcon_core::barcode`] —— 桥这一侧不判「读得出读不出」，只搬像素和那一句话（#281）。

    pub fn read_mask_codes(mut self: Pin<&mut Self>) {
        let frame = mask::with(|m| m.delivered());
        self.as_mut().run_codes(frame, "选区");
    }

    pub fn read_pin_codes(mut self: Pin<&mut Self>, id: i64) {
        let frame = state::with(|s| s.with_rendered(id as u64, |f| f.clone()));
        self.as_mut().run_codes(frame, "贴图");
    }

    /// 与 [`Self::run_ocr`] 同一形状、同一理由（另起一条起了名的线程、结果投回、起不动就当场
    /// 把忙放下来），不同的只有落到哪三格。
    ///
    /// 为什么不复用 `run_ocr`：那一手落的是 `ocr_*` 三格，这一手落的是 `codes_*` 三格，而两趟
    /// 各自跑得完 —— 并成一格就会有一趟的结果被另一趟踩掉（#281 那一族），省下的那段代码买不回
    /// 那一格。
    fn run_codes(
        mut self: Pin<&mut Self>,
        frame: Option<falcon_core::frame::Frame>,
        label: &'static str,
    ) {
        let Some(frame) = frame else {
            // 与 `run_ocr` 同一格理由：一张都没喂进去，不许算「读到零枚码」。零枚必须是引擎真跑
            // 过一遍交回的答案（`BarcodeError::NotFound`），否则 §8.6 要的失败类型就被写成了
            // 一句成功的话。
            self.as_mut().set_codes_busy(false);
            self.as_mut().set_codes_rows(QString::default());
            self.as_mut().set_codes_error(QString::from(
                format!("{label}：此刻没有可读的像素。").as_str(),
            ));
            return;
        };
        self.as_mut().set_codes_busy(true);
        self.as_mut().set_codes_rows(QString::default());
        self.as_mut().set_codes_error(QString::default());
        let handle = self.qt_thread();
        let spawned = std::thread::Builder::new()
            .name("falconshot-codes".to_string())
            .spawn(move || {
                let outcome = falcon_core::barcode::read(&frame);
                if let Err(e) = handle.queue(move |session| codes_landed(outcome, session)) {
                    eprintln!("falconshot: 读码结果投不回 GUI 线程（{e}）");
                }
            });
        if let Err(e) = spawned {
            self.as_mut().set_codes_busy(false);
            self.as_mut().set_codes_error(QString::from(
                format!("{label}：起不动读码线程（{e}）。").as_str(),
            ));
        }
    }

    /// 表里第 `index` 行，取的是 [`SessionRust::codes`] 那一份权威向量而不是界面那一串显示字。
    /// 序号是窗递进来的：它要是与这一趟不同步（表比向量旧、或用户在两趟之间按了一次），这一只手
    /// 答的是「这一行不在了」，不猜它想指哪一行。
    fn code_at(&self, index: i32) -> Option<&falcon_core::barcode::Code> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.rust().codes.get(i))
    }

    /// 「复制这一枚」：交回剪贴板的是原样那一串，一个字符也不改。
    pub fn copy_code_at(mut self: Pin<&mut Self>, index: i32) -> QString {
        let said = match self.code_at(index) {
            Some(code) => copy_words(&code.text),
            None => "这一行不在了，没有可复制的内容。".to_string(),
        };
        self.as_mut().say(&said)
    }

    /// §5.6.2 步骤 5 与安全规则那一只手。域名那一格与这里算出来的是同一只手（
    /// [`falcon_core::link`]），所以窗在用户点头之后按序号叫这一只，摆出去的与开掉的就是同一枚。
    ///
    /// 递不递得给浏览器那一判在 [`open_web`] 里，与 §5.21.1 那枚下载入口共用同一只手。
    pub fn open_code_at(mut self: Pin<&mut Self>, index: i32) -> QString {
        let Some(code) = self.code_at(index) else {
            return self.as_mut().say("这一行不在了，没有可打开的内容。");
        };
        // 克隆出来再判：`open_web` 要把这一句交出去，而 `code` 借的是 `self`，下面 `say` 要写 `self`。
        let text = code.text.clone();
        let said = open_web(&text);
        self.as_mut().say(&said)
    }
}

/// 「这一串字递不递得给浏览器」那一道闸的**唯一**作者：条码那一行（
/// `qobject::Session::open_code_at`）与 §5.21.1 第 4 步那枚下载入口
/// （`qobject::Session::update_open`）问的都是它。
///
/// 收在一处不是因为两处像，是因为「这一枚开得了吗」只许有一个答案（#281）：清单上那串地址是
/// 别人写的一份档里的字符串，与用户扫到的一枚码在闸面前是同一个东西 —— 先过
/// [`falcon_core::link::classify`]，只有 `http`/`https` 且域名全 ASCII 才递
/// [`platform_windows::open::browser`]。
fn open_web(text: &str) -> String {
    match falcon_core::link::classify(text) {
        falcon_core::link::Target::Web(web) if web.openable() => {
            match platform_windows::open::browser(&web.url) {
                Ok(()) => format!("已在默认浏览器打开 {}。", web.display_host()),
                Err(e) => format!("打开失败：{e}"),
            }
        }
        // IDN 那一支：`browser` 自己也会拒（同一份判据），这里先答是为了那句里能带上域名 ——
        // 用户有权知道是哪一枚没开。
        falcon_core::link::Target::Web(web) => format!(
            "{}：域名含非 ASCII 字符，本轮不代用户点头。",
            web.display_host()
        ),
        falcon_core::link::Target::Other(scheme) => {
            format!("这不是浏览器链接（scheme 是 {scheme}）。")
        }
        falcon_core::link::Target::NotALink => "这一枚码的内容不是链接。".to_string(),
    }
}

/// §5.21.1 第 3 步那一次「获取」在本 build 唯一的起手形状：**现问**档上那两枚 `[about]` 键，
/// 递给 core 那一层唯一的判决函数 [`falcon_core::update::check`]。托盘那一臂（
/// `crate::hotkey` 的 `Work::Update`）与「关于与更新」页上那两枚钮走的都是这一枚（#281）——
/// 「同名的两只手读到两份不同的清单」这一种坏因此没有立足处。
///
/// 一次也不写档、一次也不联网（[`falcon_core::update`] 头顶记的就是这一条边界）。交回三元组是
/// 为了让托盘那一臂那句 `note` 的原文一个字不变地继续印它自己的读数：窗这一侧只取第 3 段。
pub(crate) fn ask_update_now() -> (String, String, falcon_core::update::Answer) {
    let (feed, channel) = crate::settings::with(|s| {
        (
            s.config.about.release_feed.clone(),
            s.config.about.channel.clone(),
        )
    });
    let answer = falcon_core::update::check(&feed, &channel, env!("CARGO_PKG_VERSION"));
    (feed, channel, answer)
}

/// 工作线程投回来的那一步：三格一次写定、忙放下来。与 `hotkey::run` 同一族 —— 那一头的结果
/// 也是从别的线程排队进来的，写的也是 GUI 线程上的格子。
fn ocr_landed(
    outcome: Result<String, platform_windows::ocr::OcrError>,
    mut session: Pin<&mut qobject::Session>,
) {
    let (text, error) = match outcome {
        Ok(text) => (text, String::new()),
        Err(err) => (String::new(), err.to_string()),
    };
    session.as_mut().set_ocr_text(QString::from(text.as_str()));
    session
        .as_mut()
        .set_ocr_error(QString::from(error.as_str()));
    session.as_mut().set_ocr_busy(false);
}

/// 工作线程投回来的那一步：表、权威向量、故障三样一次写定、忙放下来。与 [`ocr_landed`] 同一族。
///
/// 为什么 `codes` 与 `codes_rows` 必须在**同一个**回调里写：那两只按序号的手读的是向量，界面上
/// 摆的是表 —— 分开写就会有一刻表是这一趟的、向量是上一趟的，用户按第 0 行复制到的却是上一枚码
/// 的内容。这一枚回调跑在 GUI 线程上，两次 `set` 之间进不来点击，所以「一次写定」在这一层是真的
/// 一次，不是一句希望。
fn codes_landed(
    outcome: Result<Vec<falcon_core::barcode::Code>, falcon_core::barcode::BarcodeError>,
    mut session: Pin<&mut qobject::Session>,
) {
    let (codes, error) = match outcome {
        Ok(codes) => (codes, String::new()),
        Err(err) => (Vec::new(), err.to_string()),
    };
    let rows = codes_encode(&codes);
    session.as_mut().rust_mut().codes.clone_from_slice(&codes);
    session
        .as_mut()
        .set_codes_rows(QString::from(rows.as_str()));
    session
        .as_mut()
        .set_codes_error(QString::from(error.as_str()));
    session.as_mut().set_codes_busy(false);
}

/// §5.6.2 步骤 4 / §5.6.3 步骤 3 那张表的编码：字段之间 `\u{1c}`（文件分隔）、行之间 `\u{1d}`
/// （组分隔），与 [`group_encode`] 同一族机器串，给 QML 的 `Repeater` 读，不是给人读的一句话。
///
/// 每行的形状：`序号\u{1c}类型\u{1c}内容\u{1c}域名\u{1c}能不能开(1/0)\u{1c}为什么不能开`。
///
/// [`group_encode`] 同一族机器串。这里的 `内容` 是**外来数据** —— 一枚二维码里带 0x1C 是完全
/// 可能的，分隔符一旦被内容顶开，界面读出的每一格都错位。所以那一格在**显示形**里把 `\u{1c}`
/// `\u{1d}` 与 NUL 换成 U+FFFD；复制与打开不经这一串（`copyCodeAt` / `openCodeAt` 那两只手读的
/// 是 `SessionRust::codes` 里那一份原样的），所以被换掉的那几个字符永远进不了剪贴板，也永远
/// 进不了浏览器。
fn codes_encode(codes: &[falcon_core::barcode::Code]) -> String {
    codes
        .iter()
        .enumerate()
        .map(|(index, code)| {
            let (host, openable, why) = match falcon_core::link::classify(&code.text) {
                falcon_core::link::Target::Web(web) => (
                    web.display_host(),
                    web.openable(),
                    if web.openable() {
                        String::new()
                    } else {
                        "域名含非 ASCII 字符，本轮不打开".to_string()
                    },
                ),
                falcon_core::link::Target::Other(scheme) => (
                    String::new(),
                    false,
                    format!("scheme 是 {scheme}，不是浏览器那一类"),
                ),
                falcon_core::link::Target::NotALink => (String::new(), false, String::new()),
            };
            format!(
                "{index}\u{1c}{}\u{1c}{}\u{1c}{host}\u{1c}{}\u{1c}{why}",
                code.format,
                sanitize(&code.text),
                i32::from(openable)
            )
        })
        .collect::<Vec<_>>()
        .join("\u{1d}")
}

/// 把会顶开分隔符的那三个字符换成 U+FFFD。**只**用于显示形，见 [`codes_encode`] 那一封信。
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{1c}' | '\u{1d}' | '\0' => '\u{fffd}',
            other => other,
        })
        .collect()
}

/// 「把一串字放上剪贴板，并说出结果」那一只手的身子。§5.6.1 那两只钮与 §5.6.2 那一行「复制」
/// 用的是同一句话：同一件事在两个入口各写一遍，就会有两套「复制成功」的说法（#281）。
fn copy_words(words: &str) -> String {
    if words.is_empty() {
        "没有可复制的字。".to_string()
    } else {
        match platform_windows::clip::write_text(words) {
            Ok(()) => format!("已复制 {} 字。", words.chars().count()),
            Err(e) => format!("复制失败：{e}"),
        }
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

    /// Give the GUI thread up to `ms` of turns. A row of [`crate::hotkey::report`] that waits
    /// for something QML does has to call this: the report runs after `app.exec()` returned,
    /// so a bare sleep holds the thread the Timer would have run on. Qt 6 hands back no count,
    /// so the answer to "did the loop turn" is what QML pushed for itself, not a number here.
    /// GUI thread only.
    pub fn pump_events(ms: u64) {
        qobject::pin_pump_events(ms.clamp(0, i32::MAX as u64) as i32);
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

#[cfg(test)]
mod tests {
    use super::codes_encode;
    use super::group_encode;
    use super::sanitize;
    use falcon_core::barcode::Code;

    /// §5.12 那一串字的编码，两枚控制字符的分隔不会撞进用户打的分组名里：名字里带 `|`、带 `,`、
    /// 甚至带空格都要能原样解回来 —— 这正是选控制字符而不是可见分隔符的理由。这一格是 §9.1
    /// 第十条的 headless 半边：真机扇出走 `group_epoch`，命名与编码的这两件事不必开一扇窗就能证。
    #[test]
    fn group_encoding_survives_every_visible_separator() {
        let rows = vec![
            (1, "A|B, C".to_string(), true, 3usize),
            (2, "第二组".to_string(), false, 0),
        ];
        let line = group_encode(&rows);
        // 组之间只有一枚 `\u{1d}`，字段之间只有 `\u{1c}` —— 名字里那些可见字符都不得被当成结构。
        let recs: Vec<&str> = line.split('\u{1d}').collect();
        assert_eq!(recs.len(), 2, "{line}");
        let first: Vec<&str> = recs[0].split('\u{1c}').collect();
        assert_eq!(first, ["1", "A|B, C", "1", "3"], "{line}");
        let second: Vec<&str> = recs[1].split('\u{1c}').collect();
        assert_eq!(second, ["2", "第二组", "0", "0"], "{line}");
    }

    /// 空表编成空串，而不是某种「零枚」的占位串 —— QML 那一侧的 `decodeGroups` 把空串解成空数组，
    /// 于是「一个组也没有」与「有一枚空名的组」在两处都分得开（#433 那一族：恒等形状不算数）。
    #[test]
    fn an_empty_group_table_encodes_to_nothing() {
        assert_eq!(group_encode(&[]), "");
    }

    /// §5.6.2 步骤 4 与 §5.6.3 步骤 3 那一张表的形状：一枚码一行、六格，且**类型**与**域名**
    /// 各占自己那一格 —— 界面拼不出一句「类型是 https、域名是 QR_CODE」那种话。
    #[test]
    fn a_code_row_carries_its_type_its_content_and_its_host() {
        let line = codes_encode(&[Code {
            text: "https://paypal.com@evil.com/x".to_string(),
            format: "QR_CODE".to_string(),
        }]);
        let fields: Vec<&str> = line.split('\u{1c}').collect();
        assert_eq!(
            fields,
            [
                "0",
                "QR_CODE",
                "https://paypal.com@evil.com/x",
                // 显示的那一枚域名就是浏览器要去的那一枚：userinfo 顶不掉它。
                "evil.com",
                "1",
                "",
            ],
            "{line}"
        );
    }

    /// 一维码不是链接：第五格假、第四与第六格空，界面于是摆得出「复制」而不摆「打开」。
    /// 而 IDN 那一支**是**链接、有域名，只是不开 —— 第六格说得出为什么。两格分得开是这一枚
    /// 单测的存在理由（#281：把「不是链接」和「是链接但不开」并成一格，界面就只能说一句话）。
    #[test]
    fn a_bar_code_and_an_idn_link_are_two_different_rows() {
        let line = codes_encode(&[
            Code {
                text: "12345678".to_string(),
                format: "CODE_128".to_string(),
            },
            Code {
                text: "https://exämple.com/".to_string(),
                format: "QR_CODE".to_string(),
            },
        ]);
        let rows: Vec<Vec<&str>> = line
            .split('\u{1d}')
            .map(|row| row.split('\u{1c}').collect())
            .collect();
        assert_eq!(rows.len(), 2, "{line}");
        assert_eq!(
            &rows[0][1..],
            ["CODE_128", "12345678", "", "0", ""],
            "{line}"
        );
        assert_eq!(rows[1][0], "1", "序号跟着行走，不跟着重数");
        assert_eq!(rows[1][3], "exämple.com", "{line}");
        assert_eq!(rows[1][4], "0", "IDN 不开");
        assert!(rows[1][5].contains("非 ASCII"), "{line}");
    }

    /// 外来数据顶分隔符：一枚内容里带 0x1C 的码，**显示形**必须还是两行六格。这一枚钉的是编码
    /// 不会因用户扫到的东西而错位；它同时钉住「被换掉的只有显示形」——`sanitize` 改那三个字符，
    /// 别的字符一个不碰。
    #[test]
    fn content_cannot_push_a_separator_around() {
        let line = codes_encode(&[
            Code {
                text: "a\u{1c}b\u{1d}c\0d".to_string(),
                format: "QR_CODE".to_string(),
            },
            Code {
                text: "second".to_string(),
                format: "QR_CODE".to_string(),
            },
        ]);
        let rows: Vec<Vec<&str>> = line
            .split('\u{1d}')
            .map(|row| row.split('\u{1c}').collect())
            .collect();
        assert_eq!(rows.len(), 2, "两枚码还是两行：{line}");
        assert_eq!(rows[0][2], "a\u{fffd}b\u{fffd}c\u{fffd}d", "{line}");
        assert_eq!(rows[1][0], "1", "{line}");
        assert_eq!(
            sanitize("保留：中文 \n\t 与 emoji 🎯"),
            "保留：中文 \n\t 与 emoji 🎯"
        );
    }

    /// 空表编成空串，与 `codes_error` 那一格分得开：「一枚也没有」是引擎答的话（它答的是
    /// `NotFound`，落进故障那一格），不是界面自己数出来的一张空表。
    #[test]
    fn no_codes_encodes_to_nothing() {
        assert_eq!(codes_encode(&[]), "");
    }

    /// 面板上那颗「复制」与那颗「打开」递回桥的只有**序号**，而桥拿它去 `codes.get(i)` 取原样字节。
    /// 所以这一格钉的是两只手共用的那一件地址：`codes_encode` 写进第一格的那个数字，必须正好是同一枚
    /// `Code` 在表里的下标 —— 差一格，用户按第一行复制到的就是第二枚码，而界面上一切看起来都对
    /// （#433 那一族：形状对不等于读的是同一枚）。域名与「能不能开」也跟着同一格走，否则「确认打开」
    /// 显示的域名就不是真被打开的那一枚。
    #[test]
    fn the_row_number_a_chip_hands_back_addresses_the_same_code() {
        let rows = vec![
            Code {
                text: "https://example.com/a".to_string(),
                format: "QRCode".to_string(),
            },
            Code {
                text: "CODE128 payload".to_string(),
                format: "Code128".to_string(),
            },
            Code {
                text: "第三枚：不是链接".to_string(),
                format: "EAN13".to_string(),
            },
        ];
        let line = codes_encode(&rows);
        let got: Vec<Vec<&str>> = line
            .split('\u{1d}')
            .map(|r| r.split('\u{1c}').collect())
            .collect();
        assert_eq!(got.len(), rows.len(), "{line}");
        for (i, f) in got.iter().enumerate() {
            assert_eq!(f[0].parse::<usize>().unwrap(), i, "{line}");
            assert_eq!(f[1], rows[i].format, "{line}");
            assert_eq!(f[2], rows[i].text, "{line}");
        }
        assert_eq!(got[0][3], "example.com", "{line}");
        assert_eq!(got[0][4], "1", "{line}");
        assert_eq!(got[1][3], "", "{line}");
        assert_eq!(got[2][4], "0", "{line}");
    }
}
