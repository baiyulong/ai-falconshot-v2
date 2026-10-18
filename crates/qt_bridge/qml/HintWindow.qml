import QtQuick
import QtQuick.Window
import dev.falconshot 1.0

// 屏幕上那一句话的家。
//
// §5.9.13 asks a successful 另存为 to be able to 「显示短暂状态提示」 and §8.5 asks a screen
// change to 「并提示」. Both have been satisfied in Rust since P21 - the sentence is produced
// and kept on `Session.status` - and unsatisfied in the only place a user looks: the app's
// root is a headless `Item` (§5.1 无主窗口), and nothing on the pin side bound to that
// property. This window is that binding.
//
// It takes no input and owns no state beyond when to hide. It is also the one window in this
// process that may not exist: `Session.hintAnchor()` answers empty for every measuring door,
// because a toast that appears whenever a leg writes `status` would be a second visible
// top-level for `pin_check::pin_window` to choose from while that row is grading a pin. The
// gate is Rust's, not this file's guess.
Window {
    id: hint

    required property Session session

    /// `主屏 x | 主屏 y | 主屏 w | 主屏 h | dpr | hold 毫秒`, read once. Empty means this run
    /// gets no hint surface at all, which is what keeps `armed` false and `visible` false in
    /// every gauge that does not measure this window.
    ///
    /// Read in `Component.onCompleted` rather than bound: which screen a window is on is
    /// resolved from its own x/y/width/height, so a `Screen` attached property feeding a
    /// geometry that feeds that resolution back is the binding loop `PinWindow.qml` names in
    /// its own `dpr`. Qt's primary screen is asked once, in its own device-independent space,
    /// which is the space these four numbers are written in.
    property var anchor: []

    /// The door's plan: `延迟 ms | 贴图 id | 保存路径`. Empty in a product session, which is
    /// what keeps the leg below inert for a user. Same shape and same reason as
    /// `PinWindow.qml`'s `gauge`.
    property var plan: []

    readonly property bool armed: hint.anchor.length === 6

    /// Whether a sentence is on the desktop *right now*. Kept as a property rather than as a
    /// `show()` call in two places, because the timer below has to be able to take the window
    /// away and `armed` gets a say as well: one condition written twice is a pair that drifts
    /// into two different window states.
    property bool shown: false

    /// 倒计时有没有在数。绑的是 Session 那枚 qproperty 的**值**（`main.qml` 里那枚 `Timer` 的
    /// `running` 走的同一条路），所以 `set_delay_left` 每一次落地都会把这枚布尔重新求值一次；
    /// 归零那一刻它翻成 false，本文件**根上**那条 `onCountingChanged` 才收窗 —— 根上，不是
    /// `Connections` 里，理由和这一路的两封报错信都写在那条 handler 的注释里。
    readonly property bool counting: hint.session.delay_left > 0

    /// The warning that belonged to the sentence now showing. Captured rather than bound:
    /// `pre_notice` is written *before* a save and `status` after it, so binding both live
    /// would pair a stale warning with the next unrelated sentence. It is read at the moment
    /// the sentence arrives, which is the pairing PRD 447 and §5.9.13 together describe.
    property string warn: ""

    // A banner, not a dialog: fixed size, centred on the primary screen's bottom edge, and
    // never where a pin happens to be. The text elides rather than growing the window, so one
    // width is the width of every sentence this product says.
    readonly property int band_w: 460
    readonly property int band_h: 58
    readonly property int bottom_gap: 28

    // §5.19.1 步骤 2 的那套餐具。这一扇窗**会贡献像素的**颜色就是这四枚（窗体自己是
    // `color: "transparent"`，它不留颜色、只留 alpha=0 的像素），所以这里写的是**两套完整
    // 的**，而不是同一套里换掉一格 —— 半深半浅是比不切更坏的界面。`AnnotationToolbar.qml` 有
    // 十几处颜色散在各工具里，本轮没有跟着改，因为只切一半等于把 §5.19.1 那句「整体」拆开交付。
    //
    // 两枚 card_fill 只差 RGB、alpha 同为 `E6`：`hint_check` 的 `hint palette` 那一行量的是这张
    // 照片里最占多的那一枚不透明色，而 PrintWindow 还回来的可能是预乘过的值 —— alpha 写成同一个，
    // 两套餐具被缩放之后的亮度差就只由 RGB 决定，那枚 128 的分界才与 alpha 无关。
    //
    // `theme` 绑的是**值**而不是调用：Rust 在 `refreshTheme()` 里往那一格写一次，notify 让下面
    // 四条绑定重新求值（同 `main.qml` 里 `warmRounds` 钉下的那条教训 —— 一次 invokable 的绑定
    // 只会跑一遍）。空串是启动后、`refreshTheme()` 落地前的那一格，它不是第三臂：`dark` 因此为
    // false，而 Rust 侧 `resolve_theme` 的 `None => Light` 兜底给出的正是 "light"，两边同值。
    readonly property bool dark: hint.session.theme === "dark"
    readonly property color card_fill: dark ? "#E612151B" : "#E6F2F4F7"
    readonly property color card_edge: dark ? "#33596B" : "#B4BEC9"
    readonly property color word_fill: dark ? "#F2F4F7" : "#12151B"
    readonly property color warn_fill: dark ? "#F2B04C" : "#8A5A00"

    color: "transparent"
    // No spaces: `pinTopLevels` is a whitespace-split field list and `hint_check` finds this
    // window by the whole title.
    title: "falconshot-hint"

    // 下面这一行写的是五枚旗位，它们兑的是 §5.9.13 的「短暂」与 §8.5 的「提示」对桌面其余部分
    // 的承诺：无边框、不进任务栏、不抢焦点、盖在它正在汇报的那些贴图之上（`Qt.Window` 只是
    // 告诉 Qt 这是一扇顶层窗）。`WindowDoesNotAcceptFocus` 是用户唯一会感觉到的一枚 —— 一句
    // 提示在用户别处打字时把键盘拿走，是比不提示更坏的产品。**判据不在这几枚 QML 旗位上**：
    // `hint_check` 的 `hint styles` 那一格问的是 shim 从 Win32 读回的窗口样式，它把
    // `WS_CAPTION`、`WS_EX_TOOLWINDOW`、`WS_EX_NOACTIVATE`、`WS_EX_APPWINDOW`、
    // `WS_EX_TOPMOST` 与那枚「不是子窗」的 `FRAMELESS` 一并判（**六枚**），外加 `WS_EX_LAYERED`
    // 只作读数不判 —— 比这一行多出来的两枚（`CAPTION=0` 与 `APPWINDOW=0`）是「无边框」「不进
    // 任务栏」这两句承诺在原生位上的真身，`Qt.FramelessWindowHint` 与 `Qt.Tool` 本身不证明它们。
    flags: Qt.Window | Qt.FramelessWindowHint | Qt.Tool
           | Qt.WindowDoesNotAcceptFocus | Qt.WindowStaysOnTopHint

    // 场上与场下只有 `show()` 与 `hide()` 两个入口，`visible` 一处也不写——连写带绑定都试过：
    // `p54_hint_d.log` 里 `hint.visible = true` 之后紧接着读回它自己，答案是 false，`visibility`
    // 停在 0、原生 style 里始终没有 WS_VISIBLE；`p54_hint_e.log` 里同一个窗换 `show()` 走同一条
    // 时间线，当场 visible=true、style=0x96000000、PrintWindow 从 1 种颜色变成 4598 种。判定权
    // 全在 Rust 那侧，这两个 handler 只是把 `shown` 与 `armed` 的与值交给 Qt。
    onShownChanged: showOrHide()
    onArmedChanged: showOrHide()

    // §5.2.4 步骤 4 / P109：倒计时归零 ⇒ 收走屏幕上那一句「还剩 N 秒」。**这一句不是可选**——
    // Rust 那侧在 `SessionRust::delay_round` 的 `Go` 那一臂里先叫 `set_delay_left(0)`、再叫
    // `open_mask`，而 `open_mask` 底下就是 `capture::freeze()`。cxx-qt 的 qproperty setter
    // 是**同步** notify（同线程 direct connection），所以这条 handler 走完之后 `hint.hide()`
    // 已经叫过，`freeze()` 那一刻桌面上才没有我们的窗（`crates/qt_bridge/src/capture.rs`
    // 头顶那条「画面到手之前不存在任何我们的窗」的规则）。反过来把 hide 挪到 status 那一支
    // 走不通：把 status 设成空串会**先**触发 `Connections` 里那条 `onStatusChanged`、把
    // `shown` 又设回 true，等于屏幕上一格也没少。所以「收回」这件事只能长在 `delay_left`
    // 归零那一支。
    //
    // P119 第 #316 封信：这一支此前**一次也没跑过**，而证人不是源码扫描，是 `--autosave`
    // 门档里 Qt 自己在 `Connections` 创建那一刻印出的一行：
    // `HintWindow.qml:166:5: QML Connections: Detected function "onDelayLeftChanged"
    // … no signal of the target matches the name`。名字写错在这一路是**静默**的：Qt 只把
    // `on` 之后第一枚字母的大小写归一，而 cxx-qt 根本不做驼峰化 ——
    // `#[qproperty(i32, delay_left)]` 的 notify 就叫 `delay_leftChanged`，
    // `onDelayLeftChanged` 剥掉 `on` 得到 `delayLeftChanged`，场上没有那一枚信号，
    // 于是 handler 挂在那儿当一个普通的成员函数，谁也不叫它。P109 起那条「归零收回」的
    // 运行期读数因此一直是零，而它自己的判据（`autosave_check.rs` 那一格）判的是 Rust 侧
    // `hint_text` 的五件返回值，从来没有问过屏幕上那句有没有被收走 —— 两处都在场、两处都
    // 没有互相问过，正是 #228 那一族。
    //
    // 修法不写 `onDelay_leftChanged`：那一枚能中（`PinWindow.qml` 的
    // `onClick_throughChanged` 就是这个形状），但把一枚大小写混着的名字留在产品码里，下一
    // 次手写仍旧会错。改成绑**值**：`counting` 是本文件自己声明的 property，它的 notify 由
    // Qt 生成、就叫 `countingChanged`，与上面那两条已被运行期读过的 `onShownChanged` /
    // `onArmedChanged` 同一种形状 —— 而 `delay_left` 这一侧仍然只被读、不被改名，
    // `main.qml` 那枚 `Timer` 的 `running` 走的本来就是同一条值绑定。
    //
    // P119 第 #321 封信：上面这套道理第一次落地时**仍然没跑**，尽管判据词从
    // `onDelayLeftChanged` 换成了 `onCountingChanged`、源码扫描那一格当场说绿。Qt 当场又印了
    // 同一形状的一行，只是行号换了：`HintWindow.qml:178:5: QML Connections: Detected function
    // "onCountingChanged" in Connections element. This is probably intended to be a signal
    // handler`。原因不在名字而在**归属**：`Connections` 把 `on…` 前缀拿去问它的 `target`
    // （那一枚是 `hint.session`），而 `counting` 声明在本窗的根上、从来不是 Session 的信号。
    // 一枚 handler 写在哪个对象里，和它叫什么名字，是两件独立的事 —— 上一轮的判据只问了姓
    // 名，姓名这一轮答得又对又没用；这是 #302 的反面那一型：判据**过宽**，被一枚错归属满足。
    // 所以这一条自此长在根上，而 `hint_check` 的那一格判的不再是「串在不在文件里」，是
    // 「串在不在任何一枚 `Connections { … }` 的括号里外」。
    onCountingChanged: if (!hint.counting) hint.shown = false

    /// Both properties feed one function because either can be the last to change: `armed`
    /// arrives from `Component.onCompleted`, `shown` from the first sentence. The other half
    /// asks the window whether it is on the desktop, because `armed` turning true is a routine
    /// part of every session and a `hide()` aimed at a window that was never shown would be
    /// this file asking Qt to do nothing in the loudest possible way.
    function showOrHide() {
        if (hint.armed && hint.shown)
            hint.show()
        else if (hint.visible)
            hint.hide()
    }

    width: hint.band_w
    height: hint.band_h
    x: hint.armed ? Number(hint.anchor[0]) + (Number(hint.anchor[2]) - hint.band_w) / 2 : 0
    y: hint.armed ? Number(hint.anchor[1]) + Number(hint.anchor[3]) - hint.band_h - hint.bottom_gap : 0

    // 不跟着 `show()` 走：这一句是「谁按了托盘里的退出」之类的事，浮窗自己不该拦着任何人。
    // `Qt.QuitOnLastWindowClosed` is already off for the resident session (`main.rs`), so the
    // only thing hiding this window can do is take the toast away.

    Rectangle {
        id: card

        anchors.fill: parent
        anchors.margins: 2
        radius: 8
        color: hint.card_fill
        border.color: hint.card_edge
        border.width: 1

        Column {
            anchors.centerIn: parent
            width: parent.width - 24
            spacing: 3

            Text {
                width: parent.width
                // The live binding is the point of this file: `Session.status` is the slot
                // every output path already answers with, and this is the surface that makes
                // answering it mean something on the screen.
                text: hint.session.status
                color: hint.word_fill
                font.pixelSize: 13
                elide: Text.ElideMiddle
                maximumLineCount: 1
                horizontalAlignment: Text.AlignHCenter
            }

            Text {
                width: parent.width
                text: hint.warn
                color: hint.warn_fill
                font.pixelSize: 11
                elide: Text.ElideMiddle
                maximumLineCount: 1
                horizontalAlignment: Text.AlignHCenter
                visible: text.length > 0
            }
        }
    }

    Component.onCompleted: {
        hint.anchor = hint.session.hintAnchor().split("|")
        hint.plan = hint.session.hintGaugePlan().split("|")
    }

    Connections {
        target: hint.session

        // 一句新话把它自己那一刻的提醒带上来，然后重新计时。`pre_notice` 与 `status` 是同一个
        // Session 上的两格，而只有这句话到达的时刻能决定哪一格属于它。
        function onStatusChanged() {
            hint.warn = hint.session.pre_notice
            hint.shown = true
            hold.restart()
        }
    }

    Timer {
        id: hold

        // The lifetime comes from the anchor rather than being written here too; the fallback
        // is `hint_check::HOLD_MS`, which is the arrangement `PinWindow.qml` already has with
        // `pin_check::LEG_DELAY_MS`.
        interval: Number(hint.anchor[5]) || 2600
        repeat: false
        onTriggered: hint.shown = false
    }

    // The `--hint` door's own leg: it asks the product to save a pin, which is the path that
    // produces the sentence, so the surface is measured with a line the product wrote rather
    // than one this file invented. What it hands back is reported, and Rust photographs the
    // window.
    Timer {
        id: gaugeLeg

        interval: Number(hint.plan[0]) || 400
        repeat: false
        running: hint.plan.length === 3
        onTriggered: hint.session.hintGaugeReport("saved|" + hint.session.savePin(Number(hint.plan[1]), hint.plan[2], ""))
    }
}
