import QtQuick
import QtQuick.Controls
import QtQuick.Window
import dev.falconshot 1.0

// §5.3.5 的那一扇「自定义截图」对话框（PRD 第 294–309 行）。
//
// 这一页上**没有一份名单、没有一句判据、也没有一枚算术**。五格叫什麼（`customLabels()`）、每格旁边
// 那句提示（`customHints()`）、窗标题（`customTitle()` = 唯一名册里 `capture_custom` 那一枚名，与托盘
// 那一项、设置页那一行同一个名）、「开始截图」那两个字（`customStartWord()`），全部出自
// [`crate::custom`]；在这里抄一份中文名单，改一天「框上写着宽、递进 Rust 的是高」就只能靠数出来
// （#281，与 `SettingsWindow.qml` 头顶那封信同一个理由，而门里 `custom window` 那一格判的正是
// 「页上画的格数 == 名单长度」）。
//
// 校验也一次也不在这一页上：
// * 第 307 行「宽度和高度必须大于 0」那句话的作者是 `crate::capture::size_guard`，对话框把自己的
//   名字递进去当 `what`，所以页上那句拒绝与终端那句拒绝是**同一句话**而不是它的译文；
// * 「这一格写的不是整数」「延时要在界内」那两句的作者分别是 `custom::int_note` /
//   `custom::delay_note`，而那一枚「界」读的是档里那两枚常数，页上不知道也不该知道它是 300；
// * 第 309 行「应支持键盘方向键微调数值」的那一枚 **+1 住在 `custom::nudge`**：页上那两枚键 handler
//   只做一次调用与一次回填。算术若写在这一页里，这一条规则在单测与门两头都是空的 —— 本仓没有输入
//   注入器（§10 第 150 项③：量具的手 ≠ 用户的手），量具递不进真按键，但量具能叫同一只手。
//
// 这一扇窗与 `SettingsWindow.qml` 一样，是本进程里由 **Rust 决定存在与否**的窗：`Session.custom_open`
// 那一格是 0 就没有这个对象（`main.qml` 的 `syncCustom`），所以产品会话与另外七道门不会被一扇没人要的
// 顶层窗搅掉各自的窗清点。它**不**看 `settings_open`：两扇窗各自的理由不同，一方开着不该把另一方带出来。
Window {
    id: win

    required property Session session

    /// `--custom-window` 那一档的计划，十五段（`customGaugePlan()` 原文）：`腿等待 ms | 出界那块的
    /// x y w h | 零宽高那对的 w h | 不是整数那一格的下标与写的字 | 越界那一枚延时 | 在界内那块的
    /// x y w h | 延时腿的秒数`。空串 = 这一趟没有量具的腿要跑：产品会话里真人自己填，另外七道门里
    /// 整条躲开（同 `settingsGaugePlan` / `pinGaugePlan` 那一族的口径 —— 量具的代码不许长成用户
    /// 看得见的一次点击）。
    ///
    /// 只在 `Component.onCompleted` 读一次：Rust 那一头在 `engine.load()` 之前就把计划写好，而绑一枚
    /// invokable 的绑定只会跑一遍（`main.qml` 里 `warmRounds` 钉下的教训）。
    property var plan: []

    /// 五格的名字与每一格那句提示，与 Rust 那两串**同序同长**：两串都是 `|` 连接（labels 那五枚、
    /// hints 那五句里没有任何一句含 `|`），所以这一页只需要一把刀 —— 而计划那十五段用的也是同一把，
    /// 切错了会当场少一段、`armed` 不成立、整趟一条腿也不跑（`SettingsWindow.qml` 里那两枚函数分成
    /// 两把刀，是因为那一档的行走的是 `RECORD`；这里三串同行，硬造第二把刀才是多余的权威）。
    property var labels: []
    property var hints: []

    /// 上面两串合成的一次 model：名字与提示**成对**递到 delegate 手里。分开两串递，就会有一个瞬间是
    /// 「名字是这一格的、提示是邻居那一格的」—— delegate 只该拿到自己那格的两样东西。
    property var cells: []

    /// 窗标题与那一枚钮上的字，桥递回来的原文（页上不许多写一份措辞）。
    property string headerText: ""
    property string startWord: ""

    /// 每枚数之间隔多久：`customStepMs()` 交的是 [`crate::delay::STEP_MS`] 本身，页上读一次给
    /// 下面那一枚 `Timer`。这里不写 1000 —— 写了就是第二个作者，改一天 Rust 那枚常数而页上还在
    /// 数 1000，倒计时就成了页与桥各数一半。
    property int step: 0

    /// 延时那一格在名单里的下标。它从**名单长度**推出来而不是抄一个 4：`custom::read` 认的是名单里
    /// 最后一格，页上抄第二个 4 就是第二份权威，而名单将来长一格时抄的那一枚会悄悄指错格。
    readonly property int delayCell: win.labels.length > 0 ? win.labels.length - 1 : 0

    /// 页上那一句结果 / 拒绝与那一枚倒计时。绑的是两枚 qproperty 的**值**（notify 一到就重新求值），
    /// 不是 invokable 调用 —— 这一行与那一枚数每按一次、每数一枚都要跟着动。
    readonly property string resultLine: win.session.custom_note
    readonly property int countdown: win.session.custom_left

    /// 计划够不够跑那几条腿。绑**值**不绑长度：`plan` 是本文件自己声明的属性，它的 notify 由 Qt 生成。
    readonly property bool armed: win.plan.length === 15

    // §5.19.1 的那套餐具，与 `SettingsWindow.qml` / `HintWindow.qml` 同一族：靠绑定，不靠造窗腿的次序。
    readonly property bool dark: win.session.theme === "dark"
    readonly property color page_fill: dark ? "#12151B" : "#F2F4F7"
    readonly property color card_fill: dark ? "#1B1F27" : "#FFFFFF"
    readonly property color word_fill: dark ? "#F2F4F7" : "#12151B"
    readonly property color faint_fill: dark ? "#8B95A5" : "#5A6472"
    readonly property color edge_fill: dark ? "#39424F" : "#B4BEC9"
    // §5.19.2 主题色：绑 `Session.accent` 那枚**值**（桥那侧已按此刻那套餐具夹过 3:1），`=== ""`
    // 那一臂只兜第一帧。三窗与此同一只口径，判据不许在这儿再写一遍（#281）。
    readonly property color accent_fill: win.session.accent !== ""
        ? win.session.accent
        : (dark ? "#4C8DF6" : "#2563B8")

    // §5.19.4 界面字体与字号：这一页上那几枚写死的 `font.pixelSize` 自此乘的是共享对象那一份
    // 倍数（`uiFont.px(...)`），页上不自己算 `ui_font ÷ 出厂磅`，也不自己兜空族 —— 四扇画着字的
    // 窗共读同一份（#281）。P149 之前这一格只长在设置页那一栋里，改档不动这一扇窗的像素。
    UiFont {
        id: uiFont
        session: win.session
    }

    // 一扇正常的对话框：有边框、进任务栏、接键盘。第 298 行写的是「对话框」，不是盖在桌面上的浮层，
    // 所以这里**不**写 `flags`：`Qt.Tool` / `FramelessWindowHint` 属于贴图与遮罩那一族。
    title: win.headerText
    width: 420
    height: 360
    color: win.page_fill

    function splitPipe(s) {
        const text = String(s)
        return text === "" ? [] : text.split("|")
    }

    /// 把名字与提示配成一次 model。长度取的是**名单**那串的长度：提示短一截时最后一格写着
    /// `undefined`，那是看得见的坏，而这里不替它编一句「没有提示」。
    function buildCells() {
        const out = []
        for (let i = 0; i < win.labels.length; i++)
            out.push({ name: String(win.labels[i]), hint: String(win.hints[i]) })
        return out
    }

    function report(line) {
        win.session.customGaugeReport(line)
    }

    /// 填一格：叫的是页上那枚输入框 `onEditingFinished` 叫的**同一枚** invokable，量具不另开一条
    /// 只有它能走的路（#281 的另一半）。它**不**往输入框里打字：量具的手不是用户的手
    /// （§10 第 150 项③），把计划里的数写进屏幕上那一格，真人下一次编辑就从量具留下的草稿起步 ——
    /// 而那一格的值与那句拒绝的对面始终是桥里那五格，不是输入框里那串像素。
    function fill(i, v) {
        win.session.customSet(i, String(v))
    }

    /// `--custom-window` 的那几条腿，一次跑完（延时那一条交给下面那一枚 `Timer`，它要真的数一枚）。
    ///
    /// 次序是有意的：**完全出桌面那一腿排在出界那一腿之前**。⑥ 判的是「那一刻在册贴图数与开工时
    /// 一样」，而 ⑤ 会真的多钉一枚贴图；两腿颠倒过来，⑥ 读到的分母就成了「开工时 + 1」，红的不是
    /// 产品而是这条腿自己的次序。
    function runGauge() {
        // ① 窗在不在、页上几格。`visible` 是窗自己问 Qt 的，`cells.count` 是 delegate 真的造出来
        //    几格 —— 两个数都从页这一侧读，不是「我们叫过 show()」。
        win.report("open|" + (win.visible ? 1 : 0) + "|" + cellRows.count + "|" + win.labels.length)

        // ② 第 307 行：宽高那一对递上去，页上那句拒绝就是 `size_guard` 的原话。前四格里 X / Y
        //    必须填得像样，不然 `read` 先说「这一格还空着」，那一趟量的就不是宽高那一条规则了。
        win.fill(0, win.plan[1])
        win.fill(1, win.plan[2])
        win.fill(2, win.plan[5])
        win.fill(3, win.plan[6])
        win.fill(win.delayCell, "")
        win.report("badsize|" + String(win.session.customStart()))

        // ③ 一格写的不是整数：下标与那串字都来自计划，页上不自己挑一格、也不自己写「abc」。
        win.report(
            "badint|" + String(win.session.customSet(Number(win.plan[7]), String(win.plan[8]))))

        // ④ 延时的界：越界那一枚也来自计划，页上不知道界是 300。
        win.report("baddelay|" + String(win.session.customSet(win.delayCell, String(win.plan[9]))))

        // ⑤ 第 309 行：方向键往上那一枚。先读「挪之前那一格此刻写着什么」，再叫 `customNudge` ——
        //    两枚都是页上那两枚键 handler 叫的同一对手，判的是「交回的要真的是挪之前那个 + 1」。
        win.fill(0, win.plan[10])
        win.fill(1, win.plan[11])
        win.fill(2, win.plan[12])
        win.fill(3, win.plan[13])
        const before = String(win.session.customCell(1))
        const after = String(win.session.customNudge(1, 1))
        win.report("nudge|" + before + "|" + after)

        // ⑥ 完全出桌面：不许假装成功，也不许多一枚贴图。
        win.fill(0, win.plan[3])
        win.fill(1, win.plan[4])
        win.fill(2, win.plan[12])
        win.fill(3, win.plan[13])
        win.fill(win.delayCell, "")
        win.report("offscreen|" + String(win.session.customStart()))

        // ⑦ 第 308 行：出界的那一块。裁与说都在 `pin_rect` / `clip_note` 那一头，页上画的只是那句
        //    原文 —— 这一格是「提示还是自动修正」那一条规则的全部对面。
        win.fill(0, win.plan[1])
        win.fill(1, win.plan[2])
        win.fill(2, win.plan[3])
        win.fill(3, win.plan[4])
        win.fill(win.delayCell, "")
        win.report("clip|" + String(win.session.customStart()))

        // ⑧ 第 301 / 302 行：延时 > 0 的那一支。按下这一刻只该挂上倒计时、说一句「还剩几枚数」，
        //    一次冻结也不许有（PRD 第 420 行：冻面是数满那一刻的事）。数满那一行由 Timer 递。
        win.fill(0, win.plan[10])
        win.fill(1, win.plan[11])
        win.fill(2, win.plan[12])
        win.fill(3, win.plan[13])
        win.fill(win.delayCell, win.plan[14])
        win.report("delay-armed|" + String(win.session.customStart()))
    }

    // 五格。delegate 是一枚 inline component：名字、提示、那一格的序号与那座桥**一次递齐**，
    // 页上的配色再从窗那一头一次递下来（`skin: win`）—— delegate 里每多一处跨出自己那一层去取
    // 窗的 id，qmllint 就报一处 unqualified access，而它报的正是「这一格靠的是外面那一刻的顺序，
    // 不是靠它自己拿到的东西」这一种坏。
    Column {
        anchors.fill: parent
        anchors.margins: 16
        spacing: 6

        Repeater {
            id: cellRows

            model: win.cells

            delegate: FieldRow {
                required property var modelData
                required property int index

                slot: index
                name: modelData.name
                hint: modelData.hint
                bridge: win.session
                skin: win
            }
        }

        Rectangle {
            id: startBtn

            width: startBtnWord.implicitWidth + 26
            height: startBtnWord.implicitHeight + 14
            radius: 4
            color: startArea.containsMouse ? win.accent_fill : win.card_fill
            border.width: 1
            border.color: win.edge_fill

            Text {
                id: startBtnWord

                anchors.centerIn: parent
                text: win.startWord
                font.pixelSize: uiFont.px(13)
                font.family: uiFont.family
                color: win.word_fill
            }

            // 第 302 行那一枚「开始截图」：按下去叫的是 `customStart`，页上不判形状、不算区域、
            // 也不数秒 —— 那三件事住在 `custom::read` / `capture::pin_rect` / `delay::Countdown`。
            MouseArea {
                id: startArea

                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: win.session.customStart()
            }
        }

        Text {
            // 结果行：交回什么就画什么。这一格不复述、不改写、不重新格式化（与设置页那枚状态行同一
            // 条规矩：页上说的话与内存里发生的事因此只能是同一句）。
            width: parent.width
            text: win.resultLine
            wrapMode: Text.Wrap
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
            color: win.word_fill
        }

        Text {
            width: parent.width
            visible: win.countdown > 0
            // `height: visible ? implicitHeight : 0` 是 `Row` 那一族的写法（AnnotationToolbar 里四处），
            // 抄到 `Text` 上会成环：Qt 6 的 QQuickText 算 implicitHeight 时反过来读 height，运行时因此
            // 每场印一句 "Binding loop detected for property height"（`p166_window_F.txt` 里 :249 那条）。
            // `contentHeight` 是文本自己排版出来的高度、不看 height，环就此断，占位行为一个字没变。
            height: win.countdown > 0 ? contentHeight : 0
            text: "倒计时：" + win.countdown
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
            color: win.faint_fill
        }

        Text {
            id: closeBtn

            text: "关闭"
            font.pixelSize: uiFont.px(12)
            font.family: uiFont.family
            color: win.faint_fill

            // 关窗那一枚写的是 `custom_open`，不是 `visible`：谁造这扇窗，销毁就归谁。这里 `hide()`
            // 一句也不写 —— 写完 0 之后 `main.qml` 那一条腿会 `close()` 再 `destroy()`，把「关掉」
            // 这件事只留一处做（#281，与 `SettingsWindow.qml` 那一枚同一封信）。
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: win.session.custom_open = 0
            }
        }
    }

    // 延时那一条腿：数满之前什么都别说（页上那一行跟着 `custom_left` 自己走），数满的那一刻把
    // 「实测 | 计划」两枚毫秒与截出来的那一行一起递回桥。
    //
    // `repeat: true` 而 `running` 绑**值**：`custom_left` 归零的那一瞬定时器自己停，不需要谁去
    // `stop()`（`main.qml` 给全局那一枚倒计时的定时器立的同一形状）。计划不够长时这一枚整条不跑。
    Timer {
        id: delayLeg

        interval: win.step
        repeat: true
        running: win.armed && win.countdown > 0
        onTriggered: {
            const said = String(win.session.customTick())
            if (said === "")
                return
            // 两枚毫秒从 `customWaitLine()` 取，不在页上拿表算：数满的那一刻已经过去了。
            win.report("delay-fired|" + String(win.session.customWaitLine()) + "|" + said)
            win.report("close|" + win.session.custom_open)
            win.session.custom_open = 0
        }
    }

    // 量具那一条腿的起跑线：等窗真的在桌面上（`LEG_MS` 由计划给），然后一次跑完。
    Timer {
        id: gaugeLeg

        interval: Number(win.plan[0])
        repeat: false
        running: win.armed
        // `tick` 是这一枚 Timer 真的走到过的记号：它把「Timer 没触发」与「`runGauge` 半路抛」这两种
        // 坏分开印 —— 后面那几条腿少一条时，读得出该修哪一个。它不进任何一格的判定。
        onTriggered: {
            win.report("tick|" + win.plan.length)
            win.runGauge()
        }
    }

    Component.onCompleted: {
        win.labels = win.splitPipe(win.session.customLabels())
        win.hints = win.splitPipe(win.session.customHints())
        win.cells = win.buildCells()
        win.plan = win.splitPipe(win.session.customGaugePlan())
        win.step = Number(win.session.customStepMs())
        win.headerText = String(win.session.customTitle())
        win.startWord = String(win.session.customStartWord())
        // 场上与场下只有 `show()` 与 `hide()` 两个入口，`visible` 一处也不写 —— 与
        // `HintWindow.qml` 同一封报错信（`p54_hint_d.log`：赋值之后 visible 仍是 false）。
        win.show()
        // 这一行不是判据，是**到此一处的记号**：窗没递回任何一行时，`measure` 得能分清「窗压根没被
        // 造出来」与「窗造出来了、腿没跑」。`born` 不进任何一格的判定（`SettingsWindow.qml` 立的形）。
        win.report("born|" + win.plan.length + "|" + win.labels.length)
    }

    // 一格：名字、输入框、旁边那句提示。五格的**值**住在桥里（`custom::text`），这一层只递字、
    // 只回填 —— 所以页上没有第二份「此刻填了什么」。
    component FieldRow: Row {
        id: row

        required property int slot
        required property string name
        required property string hint
        required property var bridge
        required property var skin

        spacing: 8

        Text {
            width: 40
            text: row.name
            font.pixelSize: uiFont.px(13)
            font.family: uiFont.family
            color: row.skin.word_fill
            horizontalAlignment: Text.AlignRight
            verticalAlignment: Text.AlignVCenter
        }

        TextField {
            id: cellField

            width: 110
            height: 26
            font.pixelSize: uiFont.px(13)
            font.family: uiFont.family
            color: row.skin.word_fill
            placeholderText: row.hint
            placeholderTextColor: row.skin.faint_fill
            // 这里不写 `background: Rectangle`：本机跑的是 windows 原生 style，它明说「当前 style 不支持
            // 定制这个控件的 background」，那块 Rectangle 一个像素都不画，只让运行时每枚格子印一行警告
            // （`p166_window_F.txt` 里 :357 那五条）。场上其余七页也没有一处定制控件背景 —— 页要跟页面
            // 一致，而不是跟一个被 style 拒掉的意图一致。

            // 出生时读一次桥里那一格的值，此后**不绑也不写**：`TextField` 里一次赋值就摧毁绑定，
            // 绑一枚值会让用户打的字与桥里记的字在某一格上各活一半（`CaptureMask.qml` 的 `sizeBar`
            // 记过的那一族）。延时那一格此刻开着 `[capture] delay_seconds` —— PRD 第 301 行「可选
            // 输入」的对面正是「上次那个数还在」，而不是每场归零。
            text: row.bridge.customCell(row.slot)

            onEditingFinished: row.bridge.customSet(row.slot, cellField.text)

            // 第 309 行「应支持键盘方向键微调数值」：键 handler 只做两件事 —— 叫 `customNudge`、
            // 把交回的那串字填回自己这一格。**这里不写 +1 / -1 之外的任何算术**：挪出来的数对不对、
            // 空格该从几起步、写着「abc」的那一格该不该被悄悄改掉，全在 `custom::nudge` 里判，
            // 那里这一条有单测、也有门可问。
            Keys.onUpPressed: (event) => {
                event.accepted = true
                cellField.text = String(row.bridge.customNudge(row.slot, 1))
            }
            Keys.onDownPressed: (event) => {
                event.accepted = true
                cellField.text = String(row.bridge.customNudge(row.slot, -1))
            }
        }

        Text {
            // 那句提示画在框**右边**而不是只当 placeholder：placeholder 一打字就没，而第 307 行那句
            // 「要大于 0」与「物理像素」这两件事，用户正在打字的那一刻最需要看见。宽度是排版，不是
            // 判据 —— 它可以在这一页上钉死，因为没有任何一格读数读它。
            width: 190
            text: row.hint
            wrapMode: Text.Wrap
            font.pixelSize: uiFont.px(11)
            font.family: uiFont.family
            color: row.skin.faint_fill
            verticalAlignment: Text.AlignVCenter
        }
    }
}
