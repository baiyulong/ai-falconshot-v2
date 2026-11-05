import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import dev.falconshot 1.0

// §5.7's annotation toolbar, as a panel that floats next to the selection.
//
// It owns no state at all. Every tick, every highlighted chip and every greyed-out
// action is read off `MaskView`, because the layer behind it remembers a style per
// tool (§5.7.1) - a toolbar that kept its own idea of the current pen would be
// showing one colour while Rust drew with another. So the buttons only ever *call*,
// and the picture they show afterwards comes back through `view.reload()`.
//
// Plain `Rectangle`/`Text` items rather than `Controls.Button`: a row of tool buttons
// whose count grows with `TOOLS`, and the width of each one has to be its own label.
// A styled Button adds padding, an implicit size and a background that differs with
// the platform style, none of which is asked for here.
Rectangle {
    id: bar

    required property MaskView view
    required property Session session

    /// The panel's own parent - the mask window's content item. Bounds come from
    /// here rather than from a passed-in `win`, so the bar can be dropped into any
    /// screen's window and still know how big that window is.
    readonly property Item host: parent

    /// §5.7.21 的 快速切换 那一排 —— 颜色不再长在这一页上。档里 `[annotation] palette`
    /// 写了几枚、按什么顺序写，屏幕上就造几枚、按那个顺序造（`bandColors()`，其主人是
    /// `crate::settings::band_colors_joined` 与 `crate::config` 的那台解析器）。本轮之前
    /// 这一行是六枚本仓自己挑的字面量，而用户档里那十枚从来没有读者：改档不动屏幕。
    ///
    /// 递过来的是 `RRGGBB`（大写、无 `#`）：`colors::parse` 认的六族拼法里 `rgb()` / `hsl()`
    /// 两族 QML 不认，所以归一那一步在 Rust —— 同一件事不许有两个主人。alpha 在这一格落地为
    /// 丢弃，因为递下去的门把手本来就只交 RGB（`mask_view.rs` 的 `setColor` 硬编码 255），
    /// §5.7.21 第三步「滚动调整透明度」是那一枚门把手之后的账。
    ///
    /// 与 `send_to_menu` 同一件要求：`"".split("|")` 在 JS 里数出**一段空串**而不是零段，
    /// 所以先滤零长段。两枚都随本面板的创建求值一次，与遮罩同生同灭。
    ///
    /// 第一行那句 `bar.view.band_revision` 不是多余的一读，它是这一行唯一的重发理由：QML 的绑定
    /// 只为**属性**重发、不为函数调用重发，而 `bandColors()` 每读一次都是现问档。§5.7.21 第二步
    /// 要的正是「拖完一次，这一排当场就是新顺序」，所以那一枚钟（Rust 每改一次加一格）必须被绑
    /// 进来 —— 否则页上停在面板创建那一刻的名单，改档要重截一张才看得见。
    readonly property var band_said: {
        bar.view.band_revision
        return bar.view.bandColors().split("|")
                                       .filter(function (s) { return s.length > 0 })
    }
    readonly property var palette: bar.band_said.map(function (s) { return Number.parseInt(s, 16) })

    /// The three named widths (§5.7.20 step 1), in device pixels. The wheel over them
    /// is step 3's fine-tune, so this row is a shortcut and not the only way.
    readonly property var pens: [4, 8, 16]

    /// §5.7.13 step 5's 放大倍数, same shape: three named values plus a wheel that
    /// reaches the rest of the range Rust clamps to.
    readonly property var magnifications: [100, 200, 400]

    /// §5.7.11 step 4's 字号, in device pixels - the middle cell is the model's own
    /// default, so the row says what the pen actually is the first time it appears.
    readonly property var font_sizes: [12, 18, 28]

    /// 「发送到」与「分享」那两行的模型。节号按需求文档现读：§5.5.10 发送到其他应用、
    /// §5.5.9 原生分享 —— 而本仓库产品码与计划档里惯用的 §5.5.11 / §5.5.12 那一对与文档
    /// 不同码（那一笔错号是结转中的账，本轮不改邻处，只保证新落的这一行不再添一枚错的）。
    /// 名字一律由 Rust 现读
    /// (`sendToMenuNames()` / `shareMenuNames()`)。空集合返回空串，而 `"".split("|")` 在 JS
    /// 里数出**一段空串**而不是零段（`mask_view.rs` 里那句注释钉的就是这一格），所以先滤零长段：
    /// 空配置 = 零枚芯片，不是一枚空芯片。两枚都随本面板的创建求值一次，与遮罩同生同灭。
    readonly property var send_to_menu: bar.view.sendToMenuNames().split("|")
                                                      .filter(function (s) { return s.length > 0 })
    readonly property var share_menu: bar.view.shareMenuNames().split("|")
                                                  .filter(function (s) { return s.length > 0 })

    /// Whether the last thing that left the program - 复制 or 保存 - got through. Kept
    /// for one thing only: the colour of the sentence Rust wrote beside it. It is this
    /// panel's own answer to the call this panel made, not a second copy of a Rust fact,
    /// which is why it is a property of the bar rather than a `MaskView` one.
    property bool delivered: false

    /// Set by the 存完再钉图 cell and read once by [`Self.saveAs`]'s `onAccepted`, which
    /// clears it. The dialog is the same door for both acts - §10 第 43 项 ① asks that the
    /// pin be made *out of the file that landed*, and the file's name is only known after
    /// the user has typed it - so the flag carries the second half across that gap rather
    /// than a second dialog being built. It is cleared on rejection too: a cancelled 另存为
    /// must not leave the next plain 保存 quietly pinning.
    property bool pinAfterSave: false

    /// The `--ink` gauge's plan for this door, read by index in
    /// [`crate::annotate_check::mask_gauge_plan`]'s order: 0 目录, 1 对话框默认名,
    /// 2 延迟 ms, 3 推开尝试上限, 4 窗 index. One field when nobody asked for the leg,
    /// which is what keeps it out of a product session and out of a `--mask` run.
    property var gauge: []

    /// True only between the moment the leg asked for a real dialog and the moment it
    /// stopped watching. The gauge reads the *first* line it was given under a key, so this
    /// is what keeps `dlg-accepted` the click's own word rather than a reading from some
    /// other completion of this door.
    property bool dlgLeg: false

    /// How many times the leg may ask the shell for its button, and how many times it has.
    /// The count comes from the plan because the pin's three runs put that window's arrival
    /// anywhere between the third wait and past the fourth, and a cap written in two
    /// languages is two numbers that can disagree.
    readonly property int pushTriesMax: Number(bar.gauge[3]) || 10
    property int pushTries: 0

    /// Every attempt's own answer, kept apart so the count is not the only evidence.
    property string pushTriesLog: ""

    /// 遮罩窗口自己，只给 `--ink` 量具的一条腿用（第 46 项 ②）。`Ctrl+…` 的匹配在
    /// `CaptureMask.qml` 的 `chord()` 里，那是窗口那一层的函数，面板够不着 `win` 这个 id：
    /// 别的属性都是窗口把对象**递进来**（`view`、`session`），这一条同规矩，递的是宿主本身。
    /// 产品会话里没人绑它，于是它是 null，而那条腿也就不会跑。
    property var chordHost: null

    color: "#f0232326"
    border.color: "#55555c"
    border.width: 1
    radius: 6

    // Only once there is something to annotate: the pen clips to the selection, so
    // a toolbar before the first drag offers a stroke that cannot be seen.
    visible: view.selected

    width: body.implicitWidth + 12
    height: body.implicitHeight + 12

    /// §5.19.4「支持调整工具栏方向和位置」里**落点**那一半的读者。档里
    /// `[appearance] toolbar_placement` 那枚词经 `Session.toolbar_placement` 递到页上；`auto`
    /// 到底落在哪一边**不在页上判** —— 页上现问 `Session.toolbarEdge(选区宽, 选区高)`，那一手
    /// 读的是 core `config::toolbar_edge` 的「长边优先」（#281：一句判断只许一处回答）。绑
    /// `toolbar_placement` 这一格是为了登记依赖：值一变（§5.20.2 的即时生效）这一问就重跑，
    /// 页上不缓存落点。`""` 那一臂 = 桥还没 publish（第一帧），走画师原本写死的那一边。
    readonly property string edge: bar.session.toolbar_placement === ""
        ? "bottom"
        : String(bar.session.toolbarEdge(view.hole_w, view.hole_h))

    // Under the selection, centred on it, and above it when the bottom edge of the
    // screen leaves no room. Clamped on both axes, because a selection that touches
    // a screen edge is the common case, not the odd one.
    //
    // 上下那一头保持原样（横向居中、放不下就翻对面）；左右那一头是纵向居中。摆到左右时这一排
    // 仍是横排：「方向」那一半（`left`/`right` ⇒ 一列）本轮决定不做，尺寸算的账、以及真做之前
    // 要先定的那一件产品决定，都写在 PRD §5.19.4 下面那条**实现口径（2026-11-03）**里（#281：
    // 一处判断一个作者，这里不重述它）。
    x: {
        if (edge === "left") {
            return Math.max(4, view.hole_x - width - 10)
        }
        if (edge === "right") {
            return Math.min(view.hole_x + view.hole_w + 10, host.width - width - 4)
        }
        const cx = view.hole_x + view.hole_w / 2
        return Math.max(4, Math.min(cx - width / 2, host.width - width - 4))
    }
    y: {
        if (edge === "left" || edge === "right") {
            const cy = view.hole_y + view.hole_h / 2
            return Math.max(4, Math.min(cy - height / 2, host.height - height - 4))
        }
        const below = view.hole_y + view.hole_h + 10
        const above = Math.max(4, view.hole_y - height - 10)
        if (edge === "top") {
            return view.hole_y - height - 10 >= 4
                ? above
                : Math.min(below, host.height - height - 4)
        }
        return below + height <= host.height - 4 ? below : above
    }

    /// The mask window has to reload the *other* screen's window after a click that
    /// changed Rust's answer - the selection and the ink are one desktop-space state
    /// drawn by two windows. Same reason `CaptureMask` has its own `grabbed`.
    signal grabbed()

    // The plan is read once, at the moment this toolbar comes into being: `plant` has
    // already run - it has to, because the freeze happens before any window exists - so a
    // session that asked for the leg has an answer here, and every other session gets the
    // one field that turns the leg's timer off.
    //
    // `historyGauge` is the same shape from a different door (`--autosave`, §10 第 99 项) and
    // it has to stay a separate field: the two plans report to two different sinks, and one
    // toolbar answering two gauges with one plan is how a reading ends up booked to the door
    // that never asked for it.
    property var historyGauge: []

    Component.onCompleted: {
        bar.gauge = bar.session.maskGaugePlan().split("|")
        bar.historyGauge = bar.session.historyGaugePlan().split("|")
        bar.readHistory()
        bar.dropHistory()
        bar.restoreHistory()
        bar.batchHistory()
    }

    /// §5.14.2 step 1「打开历史记录」的第一条**界面**腿，P98 起：这一格自己问一次清单，
    /// 再拿档那一条腿种下的那一行回放一次。两件事都只在这枚档说"要量历史"时跑
    /// （`historyGaugePlan()` 在产品会话和另外六道门里都是空串，而空串 `split` 出来是一段空的
    /// `[0]` 而不是一段没有 ⇒ 判的是那一段等不等 `"hist"`，不是数组长不长）。
    ///
    /// 报回去的是**段数**和第一段那句话，不是整份清单：`historyGaugeReport` 那一条通道是
    /// `key|value` 一行一条，而清单自己也是 `|` 分隔的，两份 `|` 混在一行里数出来的段就既不是
    /// 清单的也不是报告的了。
    function readHistory() {
        if (bar.historyGauge[0] !== "hist") {
            return
        }
        const limit = Number(bar.historyGauge[1]) || 8
        const row = Number(bar.historyGauge[2])
        if (!(row > 0)) {
            return
        }
        const said = bar.session.historyRows(limit)
        const seg = said === "" ? [] : said.split("|")
        bar.session.historyGaugeReport("rows|" + seg.length + "|" + seg[0])
        // 屏幕那句话从这里走：`historyReplay` 的答法就是 `status`，所以这一格递回去的是产品
        // 当场说的那一句，而不是工具条自己复述一遍。
        bar.session.historyGaugeReport("replay|" + bar.session.historyReplay(row))
    }

    /// §5.14.4 的第一条**界面**腿，P101 起：把「没点头的清空 → 锁一行 → 删这一行」三条产品桥
    /// 各走一次。它们自 P99/P100 建起来之后，读它们的一直是量具自己与 Rust 单测，`qml/` 里 0 处。
    ///
    /// 点名的是计划**第四段**那一枚行号——档在 `arm()` 里替这一条腿专门种下的那一行，不是第三段
    /// 那一枚：那一枚是上面清单与回放那两格的前置，界面把它锁掉或删掉，量具读回来的就成了
    /// 「自己的前置被自己判的那条腿改过」，那两格就此不再说话。
    ///
    /// 顺序是定的，而锁与删落在同一行上也是故意的：`false` 那一次递的是规则 1 的二次确认没点头
    /// （产品那一头连库都不打开），接着把这一行锁上，再按名字删掉它 —— §5.14.4 那两半相反的口径
    /// 「清空不许碰锁定项」与「手动删除点名时锁定项也在点名里」自此在同一条界面腿上各走了一次。
    ///
    /// 三条报回去的都是产品当场说的那句话，不是工具条自己的复述（同 `replay|` 那条的理由）。
    function dropHistory() {
        if (bar.historyGauge[0] !== "hist") {
            return
        }
        const row = Number(bar.historyGauge[3])
        if (!(row > 0)) {
            return
        }
        bar.session.historyGaugeReport("clearunconfirmed|" + bar.session.historyClear(false))
        bar.session.historyGaugeReport("lock|" + bar.session.historyLock(row, true))
        bar.session.historyGaugeReport("delete|" + bar.session.historyDelete(String(row)))
    }

    /// §5.14.3 step 4「重新进入原选区」的第一条**界面**腿，P103 起：把遮罩的选区交出去一次、
    /// 再收回原位一次。`restore_history_selection` 自 P102 建起来之后，读它的一直是量具自己与
    /// Rust 单测，`qml/` 里 0 处 —— 而 §5.14.3 那句话对面本来就是屏幕上那个框。
    ///
    /// 点名的是计划**第五、第六段**那两枚行号，都是档在 `arm()` 里替这一条腿专门种下的：第三段
    /// 那一枚是上面清单与回放两格的前置，第四段那一枚已经被 `dropHistory()` 删掉了（删掉的行恢复
    /// 不回来，那条腿只剩"读不到"一臂可走，什么也证不了）。
    ///
    /// 顺序是定的，而第二次回到**原位**也是定的：第六段那一行记的矩形就是 `arm()` 读到的那只框，
    /// 所以两次恢复加起来，遮罩此刻的选区必须和这一格开跑之前一字不差。这不是收尾好看的问题——
    /// `Component.onCompleted` 跑在 `engine.load()` 里，也就是比本档所有 `measure()` 的读数都早，
    /// 这条腿要是把框改了就再也改不回来，别的格会集体答非所问。
    ///
    /// 每一段报回去的是**两句**：产品当场说的话（同 `replay|` 那一条的理由，工具条不复述），加上
    /// 那一刻 `maskHole()` 读回来的框。只要前一句的话，量具就只能信产品的自称；后一句才是遮罩
    /// 真拿着的东西，选区到底动没动，判的是这一枚。
    function restoreHistory() {
        if (bar.historyGauge[0] !== "hist") {
            return
        }
        const pick = Number(bar.historyGauge[4])
        const back = Number(bar.historyGauge[5])
        if (!(pick > 0) || !(back > 0)) {
            return
        }
        bar.session.historyGaugeReport(
            "restorepick|" + bar.session.historyRestore(pick) + "|" + bar.session.maskHole())
        bar.session.historyGaugeReport(
            "restoreback|" + bar.session.historyRestore(back) + "|" + bar.session.maskHole())
        // P113：上面那两段是**代码直接调用** `historyRestore`；这两段起的是 `chord()`，也就是
        // 一次真按键在 Qt 那里被翻成 (key, modifiers) 之后走的同一个函数，递的正是
        // `Ctrl+Up` / `Ctrl+Down` 那两个数值。差别就是这一格唯一新东西：直接按桥只能证明桥通，
        // 从 `chord()` 走才能证明**这枚按键会被认领、并且派发到那一档**。合成按键本轮仍不做
        // （第 46 项 ② 那句理由一字未改：注入的按键会落进当时有焦点的应用，而跑级时焦点在谁那里
        // 产品说不出）。锚点因此不是工具条给的 —— 它由 Rust 从遮罩自己读，工具条连行号也不递。
        //
        // 每段报回三样：`chord()` 的返回值（这枚按键认没认领）、屏幕上此刻那句话
        // （`session.status` 原文，工具条不复述，同 `replay|` 那条理由）、以及那一刻的框。
        // 顺序也是判据的一部分：先翻回去、再翻回来，两次之后遮罩的选区必须仍是上面那一段留下的
        // 那一枚 —— 本档其余各格的前置同 P103 那两段的理由，一次也没被这条腿改过。
        const upActed = bar.chordHost
            ? bar.chordHost.chord(Qt.Key_Up, Qt.ControlModifier)
            : false
        bar.session.historyGaugeReport(
            "flipup|" + String(upActed) + "|" + bar.session.status + "|" + bar.session.maskHole())
        const downActed = bar.chordHost
            ? bar.chordHost.chord(Qt.Key_Down, Qt.ControlModifier)
            : false
        bar.session.historyGaugeReport(
            "flipdown|" + String(downActed) + "|" + bar.session.status
            + "|" + bar.session.maskHole())
        // P114：上面四条腿把这一场带到了历史里某一行的原选区上，而这一条问的是**那一行自己的**
        // 左右 —— `historyNeighborsAt` 自 P112 建起来之后，`qml/` 里 0 处读者，判它的一直是
        // Rust 单测与启动 banner（那一枚拿的是库里最新那一行，不是屏幕上这一行）。
        //
        // 行号从 `session.historyAnchor()` 取，工具条一个号也不自己编：它与上面两次翻页用的是
        // 同一枚锚点（Rust 从遮罩读），所以这一句问的必然是"此刻这只框所在的那一行"。这一格是
        // 这条腿唯一新东西 —— 桥收 id，而 id 的来源第一次由产品当场答，不由界面回忆。
        //
        // 报回两段：产品那句话原文（同前面每一段的理由，工具条不复述），加上读完之后那一刻的框。
        // 后一段是本档唯一能给「这一枚读侧是不是纯读」的证人：它必须与上面 `flipdown` 那一段
        // 留下的那一枚一字不差，读邻居读动了选区就是这一腿自己撒的谎。
        const anchorRow = bar.session.historyAnchor()
        bar.session.historyGaugeReport(
            "neighbors|" + bar.session.historyNeighborsAt(anchorRow) + "|"
            + bar.session.maskHole())
    }

    /// §5.14.4 步骤 2 那句「选择单条或多条记录删除」里的**多条**那一半，P115 起才有第一条界面腿：
    /// 上面 `dropHistory()` 让界面删过**一行**，而产品那一头三处本来就收得下一串 ——
    /// `Session::history_delete` 的 `split('|')`、`MaskState::delete_history` 的 `&[i64]`、
    /// `History::delete` 的 `&[i64]`。一次递两枚行号过去，读到的才是「多条」；递一枚过去，
    /// 「多条」与「单条」在盘上长得一模一样，而工具条自己删两次不算那一问的对面。
    ///
    /// 点名的是计划**第七、第八段**那两枚行号，都是档在 `arm()` 里专门替这一条腿种下的：第三段
    /// 那一枚是清单与回放两格的前置，第四段那一枚已经被 `dropHistory()` 删掉了，第五、六段那两枚
    /// 是恢复、翻页与邻居三格的行号。界面碰掉别人的行号，其余各格读回来的就成了「前置被自己判的
    /// 那条腿改过」。
    ///
    /// 报回的是产品当场说的那一句原文（同前面每一段的理由，工具条不复述）。这条腿挂在这条链的
    /// **末尾**：它一执行就把两行收走，放在任何一格之前都会让那一格读不到自己的前置。
    function batchHistory() {
        if (bar.historyGauge[0] !== "hist") {
            return
        }
        const first = Number(bar.historyGauge[6])
        const second = Number(bar.historyGauge[7])
        if (!(first > 0) || !(second > 0)) {
            return
        }
        bar.session.historyGaugeReport(
            "deletebatch|" + bar.session.historyDelete(first + "|" + second))
    }

    function commit() {
        if (view.commitHole()) {
            bar.grabbed()
        }
    }

    function cancel() {
        // Through the event loop, exactly as the Esc key does: `endMask` destroys
        // these windows from the model, and doing that from inside a handler that is
        // still running on one of them is a use-after-free QML will not warn about.
        Qt.callLater(() => session.endMask())
    }

    /// §5.5.2's 复制, from this cell or from `Ctrl+C` - both come through here, so
    /// there is one gesture with one owner rather than two paths that can disagree
    /// about what a copy does to the flow.
    ///
    /// It does not close the mask, whatever the outcome: see
    /// [`crate::mask::MaskState::copy_out`]. `grabbed()` fires either way, because the
    /// sentence is on the one mask state both windows read and the toolbar has to
    /// re-read it after the write rather than before.
    function copy() {
        bar.delivered = bar.view.copySelection()
        bar.grabbed()
    }

    /// §5.5.5's 另存为, from this cell or from `Ctrl+S`: the dialog is opened, and the
    /// answer comes back through [`Self.delivered`] when the user names a file.
    ///
    /// Nothing is decided here about *which* format - the extension the user typed is
    /// the format, and a name without one becomes the configured default in Rust. The
    /// prompt that §5.4.5 and §5.5.5 ask for when that format cannot hold the
    /// selection's transparency is Rust's sentence too, for the same reason the size of
    /// a copy is: two panels that each read the picture and each decided what to say
    /// about it would say two different things.
    ///
    /// `saveAs` is named unqualified because it is a document-scope id, not a member of
    /// this Rectangle: `bar.saveAs` is a name qmllint reports as
    /// `Member "saveAs" not found on type "Rectangle"`, and every other id reference in
    /// this file is written the short way for the same reason.
    function save() {
        saveAs.open()
    }

    /// §5.5.4's 快速保存, from this cell or from `Ctrl+Shift+S`: the same stitched picture
    /// 保存 writes, into the file `[output]` names, with no dialog at all.
    ///
    /// Nothing is asked of the user, which is the feature rather than a shortcut for it -
    /// §5.5.4 step 3 is "无需打开文件对话框，直接保存截图". What the settings have to have
    /// is step 1's 目录 and 格式, and a machine that cannot name either one answers in
    /// Rust's sentence instead of writing somewhere nobody chose.
    function quickSave() {
        bar.delivered = bar.view.quickSave()
        bar.grabbed()
    }

    /// §5.5.9 的替代分支：这一枚按下去，先把这一张写成一份文件，再把那枚路径交给 shell 的
    /// 默认关联程序。三道梯（落盘、命名、话）全在 `MaskState::share_out` 那一格，页上一步
    /// 也不自己判。
    ///
    /// 返回值不进 `delivered`，走的是上面 `revealLanded` 那条同一个理由：那一面答的是
    /// 「这一张给出去了吗」，而这里可能失败的是「对面那个程序收没收下这条请求」——文件其实
    /// 已经落成了。把一次失败的揭示写成一次失败的保存，就是屏幕上有一句假话。
    function shareOut() {
        bar.view.shareOut()
        bar.grabbed()
    }

    /// §5.5.4 step 4：那条通知被点了一下，把刚落盘的那张图在它自己的目录里亮出来。
    ///
    /// 一次点击恰好一次调用，没有别的入口：这条行没有键位，`Cell` 也不替它代发。
    /// 返回值不进 `delivered`——那一面是"这一笔给出去了吗"，而这里"给出去"是 shell
    /// 收没收下请求，两句不是一个意思，混进同一个布尔会把一次失败的揭示显示成一次
    /// 失败的保存。话在 `reveal_last`，行上就写着它。
    function revealLanded() {
        bar.view.revealLanded()
        bar.grabbed()
    }

    /// PRD 433 / 459's 完成后可…贴图, in the one order that makes the floating picture an
    /// artifact rather than a second rendering: 另存为 first, then a 贴图 made from the file
    /// that landed (§10 第 43 项 ①).
    ///
    /// The dialog is opened by [`Self.pinAfterSave`] rather than by a second door, because
    /// the name the pin must be made from is not known until the user has typed it. Both
    /// halves are Rust's sentence: `MaskState::save_then_pin` answers the write and the pin
    /// in one line, and the pin half is the only place a user learns that the file exists
    /// while nothing floated.
    function saveAndPin() {
        bar.pinAfterSave = true
        saveAs.open()
    }

    /// §5.8.1 步 2 与步 3 的一枚钮：把这一张直接浮成一扇贴图，一份文件也不写。
    ///
    /// 它挨着「存完再钉图」而不是替了它，因为那是 PRD 433 那一节的两件不同的事：那一枚要的是
    /// **用户挑中过的那份文件**浮起来（§10 第 43 项 ①），这一枚要的只是屏幕上这一块画面，
    /// 用户没打算为它交出目录、名字和一次磁盘。判据全在 `MaskState::pin_selection` 那一格，
    /// 页上这一步只是把它的返回值交给 `delivered`——那一面答的就是"这一刻屏幕上多了一张贴图吗"。
    function pin() {
        bar.delivered = bar.view.pinSelection()
        bar.grabbed()
    }

    // The `--ink` gauge at this door. Everything above is the product's own path; what
    // follows only ever runs when a plan was planted, and it asks the one question
    // [`crate::annotate_check::save_rows`]'s two rows cannot: those name the file and call
    // the encoder from Rust, so `onAccepted` above is never entered and a handler that does
    // not work would pass them anyway. Here the door opens by `save()` and closes by a
    // click on the shell's own default button.
    //
    // One line to the gauge, in the `key|value` shape `annotate_check` grades.
    function gaugeSay(key, value) {
        bar.session.maskGaugeReport(key + "|" + value)
    }

    // A local path as the text a `QUrl` arrives in, because `currentFile` and
    // `currentFolder` take URLs and the plan carries a native path.
    function dlgUrl(local) {
        return "file:///" + String(local).replace(/\\/g, "/").replace(/ /g, "%20")
    }

    // `selectedNameFilter` is read-only and hands back an object, not a string, so it is
    // flattened here the way the pin window's same probe found it.
    function readFilter(key) {
        const chosen = saveAs.selectedNameFilter
        gaugeSay(key, chosen.name + "|" + chosen.extensions.join(",") + "|" + chosen.index)
    }

    // The extension text of one arm of the menu, spelled the way `selectedNameFilter`
    // spells the arm actually selected: `PNG 图片 (*.png)` → `*.png`. That member cannot be
    // pointed at an arm of this choosing - assigning to it is the hard `TypeError` P38's
    // probe E found - so a leg that wants to ask what the JPG arm does reads the list
    // instead, and hands the door the same text `onAccepted` would hand it for that arm.
    function filterExt(i) {
        const text = String(saveAs.nameFilters[i])
        const from = text.lastIndexOf("(") + 1
        const to = text.lastIndexOf(")")
        return to > from ? text.substring(from, to) : ""
    }

    // §5.7.21 / §5.7.22 的第一位读者在屏幕上留下的一格。三段各有一枚主人：`handed` 是桥递到
    // 页上的那一份的段数（`bandColors()` 滤掉零长段之后），`drew` 是 Repeater 真的造出几枚
    // （`band.count`，不是名单长度），第三段是页手里那份名单本身，一枚一个 `RRGGBB`、逗号相连。
    // Rust 那侧拿档里的 `palette` 走**同一台**解析器算出期待值再逐段比，所以这一格不是拿页上的
    // 读数和自己比，是拿场景里的读数和档比 —— 旧页上那六个字面量在这一格会当场发红。
    //
    // 补零走 `slice(-6)` 而不是别的写法：少一位就是一枚读不出来的颜色（`0a84ff` 会掉成
    // `a84ff`），而档里那一枚的期待值由 `format!("{r:02X}…")` 补着零。
    //
    // 它单独是一枚函数，因为下面那一格 `band-move` 要读的正是**同一份**名单：一份名单写两遍
    // 就是两把会各自漂移的尺子（#281 同一口径）。
    function bandHexes() {
        return bar.palette.map(function (v) {
            return ("000000" + v.toString(16).toUpperCase()).slice(-6)
        }).join(",")
    }

    function bandSaid() {
        return String(bar.band_said.length) + "|" + String(band.count) + "|" + bar.bandHexes()
    }

    // §5.7.21 第二步留在屏幕上的一格。`moveColor` 正是页上拖完一次色块之后被按下的那一只手
    // —— 量具在这里按**两次**：把排头那一枚挪到第二格，再挪回来。两次都真落盘，所以这一趟走完
    // 之后档里那一排与出发时一字不差，上面那一格 `band` 的期待值也不会被它挪走。
    //
    // 九段各有主人：拖过去之后的「名单 + Repeater 真的造出几枚」、拖回来之后的同一对读数、
    // 出发前/第一次挪后/第二次挪后的那枚钟，`last` 是「那两次各自的返回值与窗上 `band_last` 那一格
    // 是不是同一句话」，最后一格才是那两句话本身（`splitn` 收着它，句子里出现 `|` 也不会把前八段挤位）。
    // 那一句必须**紧跟着**每一次按下去读：`band_last` 是一格而不是两格，拖回来那一次会盖掉拖过去
    // 那一句 —— 在腿末才读它，比的就是「第一次的返回值」与「第二次的句子」，两句话天生不同，
    // 这一格会红得毫无道理（2026-11-03 真机第一次跑就是这么红的）。
    //
    // **这一格说的是什么、不说什么**：它说的是「那只手改得动档、页上跟着换了顺序、那句话走到了
    // 窗上」，**不是**「一次真的指针拖动会走到这只手」—— 后者要在这条腿上装一只合成拖动，这一族
    // 还没有那样的基础设施（§10 第 35 项 ⑤ 那一族「要往屏幕上取样」欠的是同一只手）。页上那段
    // 换算（`slotOf` 把指针落到第几格）因此也只被这一格的**前半**碰到：差值由量具直接给。
    function bandMoveLeg() {
        const rev0 = bar.view.band_revision
        const after = String(bar.view.moveColor(0, 1))
        const afterLast = String(bar.view.band_last)
        const hexAfter = bar.bandHexes()
        const countAfter = String(band.count)
        const rev1 = bar.view.band_revision
        const back = String(bar.view.moveColor(1, -1))
        const backLast = String(bar.view.band_last)
        const hexBack = bar.bandHexes()
        const countBack = String(band.count)
        const rev2 = bar.view.band_revision
        return [
            hexAfter, countAfter, hexBack, countBack,
            String(rev0), String(rev1), String(rev2),
            after === afterLast && back === backLast ? "yes" : "no",
            after + " :: " + back
        ].join("|")
    }

    // Phase 1: name the file, then open the door the way the cell does. The name is the
    // gauge's and the extension is the shell's, which is the point - the file
    // `mask save via real click` reads back is the one this run could only predict.
    function openDialogLeg() {
        // 日程的第一句，问的是这一页**已经**造出来的东西：面板随遮罩窗创建，比这一枚 Timer
        // 早，而下面每一步都要往 `message` 那一格里写字，所以它只能站在这一步之前。
        bar.gaugeSay("band", bar.bandSaid())
        // 紧跟着的那一句是「拖得动」那一格（§5.7.21 第二步）。它必须站在**这一行**的位置：
        // 日程要求它在 `band` 之后、在任何一次往 `message` 格里写字之前，而它自己动的是档
        // 不是那句话，所以它插不进更下面任何一站。
        bar.gaugeSay("band-move", bar.bandMoveLeg())
        const stem = String(bar.gauge[0]) + "/" + String(bar.gauge[1])
        saveAs.currentFolder = bar.dlgUrl(String(bar.gauge[0]) + "/")
        saveAs.currentFile = bar.dlgUrl(stem)
        gaugeSay("dlg-folder", String(saveAs.currentFolder))
        gaugeSay("dlg-name", String(saveAs.currentFile))
        // §10 第 40 项 ② at this door, before the dialog is in the way: a name with no
        // extension takes the format of the arm the dialog was showing, so the leg hands the
        // door the JPG arm's own text and a bare stem. JPG is the arm the configured
        // `[output] format` would never pick on this machine, which is what tells a
        // forwarded filter from one dropped at the boundary - and the boundary is the thing
        // being asked, since every leg that runs in Rust cannot see whether QML passes the
        // argument at all, and a real dialog click cannot either (the shell suffixes the
        // name before `onAccepted` reads it).
        //
        // It runs *above* the refusal underneath on purpose: `message` is one slot, and
        // P39's `accept-was`/`pre-save` pair has to stay that refusal's two halves.
        const bareName = bar.dlgUrl(String(bar.gauge[0]) + "/" + String(bar.gauge[1]) + "-f")
        const arm = bar.filterExt(1)
        const wrote = bar.view.saveSelection(bareName, arm)
        bar.gaugeSay("f-jpg", arm + "|" + String(wrote) + "|" + bar.view.message)

        // 存完再钉图 (§10 第 43 项 ①, PRD 433 / 459), asked at this same door one rung
        // further along: the file is written first and the pin is made *out of that file*,
        // so what floats afterwards is the artifact rather than a second reading of the
        // selection. `saveAndPin` is the cell's own invokable and the name is the gauge's,
        // so no dialog has to be in the way to ask it. `message` is read back because the
        // door answers both acts in one sentence, and the id in that sentence is the only
        // handle the report has on the pin the product says it made.
        const pinName = bar.dlgUrl(String(bar.gauge[0]) + "/" + String(bar.gauge[1]) + "-p.png")
        const pinned = bar.view.saveAndPin(pinName, "")
        bar.gaugeSay("pin-after", String(pinned) + "|" + bar.view.message)
        // 这一腿刚钉的那一枚**此刻就飘在桌面上**：`saveAndPin` 现在会把名册的读数搬过桥，窗随之
        // 长出来（规矩住在 `crate::hotkey::republish_roster` 头顶那段）。而本门的 `no ghost` 那一行
        // 判的是「屏幕上那枚墨擦干净了没有」，拿一扇盖住整块选区的贴图去量桌面，量到的是贴图自己。
        // 于是这一趟把**门自己造出来的那一枚**收起来：§5.15 的全局收起不碰名册，只让它不在屏幕上
        // 站着 —— `mask pin via qml` 读的是状态机那份渲染帧，与可见与否两件事，收起来它照判。
        bar.session.hideAllPins()

        // The click's before-half has to be a sentence, not silence. P38 read
        // `accept-was ""` here - a true reading, and an empty one, so the graded pair
        // measured "from nothing to 已保存到" and could not tell an `onAccepted` that wrote
        // the product's sentence from one that merely happened to be the first to write
        // anything. So the leg asks the product to refuse a save first: `saveSelection` →
        // `MaskState::save_out` → `save::write` → `Refusal::BadName` writes
        // `MaskState::message` in the product's own words, and `gaugeSay` only ever reads it.
        //
        // An extension this program cannot write is the refusal chosen, not an unwritable
        // directory: this run's first attempt named a directory this run never made and the
        // save *succeeded*, because `core::encode::save` makes the parent before writing. A
        // refused name leaves nothing on disk and its sentence carries no operating-system
        // wording, so "the product wrote this" is readable off the sentence itself.
        const refused = String(bar.gauge[0]) + "/" + String(bar.gauge[1]) + ".not-a-format"
        // The empty filter is honest here: no dialog is open, so no arm is showing - and the
        // name carries an extension of its own, which is what the owner reads first.
        bar.view.saveSelection(bar.dlgUrl(refused), "")
        gaugeSay("pre-save", bar.view.message)
        bar.dlgLeg = true
        saveAs.open()
        // Read in the turn that asked for it, and again from the phase that presses:
        // whether `open()` comes back with the dialog up is a fact about this Qt, and the
        // graded one is whether the window exists when a button is looked for.
        gaugeSay("dlg-open", String(saveAs.visible))
        dlgPush.restart()
    }

    // Phase 2: press the dialog's own default button, once it is on the screen. The same
    // measurement as the pin's leg: Qt marks the dialog visible inside `open()` while Win32
    // has not made the `#32770` yet, so the attempt repeats until it has, up to the cap the
    // plan carries. `accept-was` is taken on the first attempt only, because it is the
    // before half of a pair graded across the click rather than across the waiting.
    function pushDialogLeg() {
        bar.pushTries += 1
        if (bar.pushTries === 1) {
            gaugeSay("dlg-shown", String(saveAs.visible))
            bar.readFilter("dlg-shown-filter")
            gaugeSay("accept-was", bar.view.message)
        }
        const said = bar.session.pushDialogButton()
        bar.pushTriesLog += (bar.pushTries === 1 ? "" : " ~ ") + String(said)
        if (said.startsWith("pushed=yes") || bar.pushTries >= bar.pushTriesMax) {
            gaugeSay("dlg-push", said)
            gaugeSay("dlg-push-tries", String(bar.pushTries))
            gaugeSay("dlg-push-log", bar.pushTriesLog)
            dlgTail.restart()
        } else {
            dlgPush.restart()
        }
    }

    // Phase 3: what the click left behind. The dismiss is guarded on `visible` because a
    // door the click already closed is gone, and `reject()` on it would be a second
    // completion this run did not measure.
    function finishDialogLeg() {
        gaugeSay("accept", bar.view.message)
        bar.readFilter("dlg-filter")
        gaugeSay("dlg-after", String(saveAs.visible))
        if (saveAs.visible) {
            gaugeSay("dlg-dismiss", "reject")
            saveAs.reject()
        }
        bar.dlgLeg = false
        bar.quickLeg()
        gaugeSay("done", bar.gauge[0])
    }

    // §5.5.4's 快速保存, asked the way the cell asks it: `quickSave` is the very function
    // the 快速保存 `Cell` taps, so this leg covers the panel's own handler and not only the
    // invokable beneath it - which is the one rung every Rust-side leg cannot see.
    //
    // The census that follows is this run's own definition of "opens no dialog": the probe
    // counts this process's `#32770` windows and, when the count is 0, presses nothing at
    // all. A quick door that did open one would be answered by `pushed=yes`, and the window
    // dismissed - §10 第 38 项 ①'s rule that a probe always cleans up after itself, kept
    // even at a door whose whole claim is that there is nothing to clean up.
    //
    // It runs *last*, after the dialog's before/after pair has been read, because `message`
    // is one slot and P39's `accept-was`/`pre-save` comparison has to finish before anything
    // else speaks through it.
    function quickLeg() {
        // §5.5.4 step 4 的那一次点击，问在还没有任何东西落盘的时刻：这条腿是整条
        // 日程里第一个按 快速保存 的，所以此刻 `landed` 是空的，点击必然在 shell 之前
        // 就被拒掉——一个窗也不会开。要问"点成了会开哪个目录"必须真去开，那一格这一轮
        // 没问（`annotate_check.rs` 把它作为发出来的 Blocked 带着，而不是删掉）。
        //
        // 按两下，中间前后各读一次计数：一下证明这条按得到 Rust 那一条路，两下证明
        // 那个数不是"这条腿只记一次"的记法。`revealLanded` 正是通知上那个 MouseArea
        // 的 `onClicked` 调的同一个函数——面板这一层的转发只有这里看得见。
        const clicksBefore = bar.view.reveal_clicks
        bar.revealLanded()
        const clicksMid = bar.view.reveal_clicks
        const saidFirst = bar.view.reveal_last
        bar.revealLanded()
        // 值先在本地拼好，下面那一句的键留在同一行上：`pin_check::qml_leg_keys` 是从
        // 文件里**找子串**的（它找的是那条发消息的语句紧跟着一个开引号），参数被拆到下一行
        // 之后那条腿就从日程里"消失"了。它消失的方式是响一声（日程有而 QML 不写），
        // 不是悄悄绿——而这一族还有另一面：注释里写下那个形状，扫描就会把它读成一条腿。
        const clickLine = String(clicksBefore) + "|" + String(clicksMid) + "|"
            + String(bar.view.reveal_clicks) + "|" + String(bar.view.reveal_acted)
            + "|" + saidFirst + "|" + String(bar.view.reveal_last)
        bar.gaugeSay("q-click", clickLine)
        bar.quickSave()
        bar.gaugeSay("q-cell", String(bar.delivered) + "|" + bar.view.message)
        // 上面那句是面板函数被**直接调用**的样子；这一句起的是 Cell 自己的 `tapped` 信号，
        // 也就是 `MouseArea.onReleased` 在一次真释放里做的唯一一件事（第 46 项 ②）。两句之差
        // 才是这条判定的内容：`onTapped` 那根线到底接上没有——接上没有过一次点，就没人能替它作证。
        quickCell.tapped()
        bar.gaugeSay("q-cellown", String(bar.delivered) + "|" + bar.view.message)
        // 第三段从窗口那层走进来：`chord()` 拿到与真按键完全相同的两个数值，判完再把它自己的
        // 分派交给 `inkBar.quickSave()`。合成的按键本轮不做（注入的按键会落进当时有焦点的应用，
        // 而跑级时焦点在谁那里产品说不出），所以这条覆盖"匹配 + 分派"那两步，欠的那一步写在
        // `CaptureMask.qml` 的注释里：Qt 把 WM_KEYDOWN 翻成 event.key / event.modifiers。
        const chordSaid = bar.chordHost
            ? bar.chordHost.chord(Qt.Key_S, Qt.ControlModifier | Qt.ShiftModifier)
            : "nohost"
        bar.gaugeSay("q-chord", String(chordSaid) + "|" + bar.view.message)
        bar.gaugeSay("q-census", bar.session.pushDialogButton())
    }

    Timer {
        id: gaugeLeg
        interval: Number(bar.gauge[2]) || 400
        repeat: false
        // Stopped for every window but the one the plan named - several screens bind this
        // same document, and a leg that ran on each would open the dialog once per monitor -
        // and for every run that planted no plan at all.
        running: bar.gauge.length === 5 && String(bar.gauge[4]) === String(bar.view.index)
        onTriggered: bar.openDialogLeg()
    }

    Timer {
        id: dlgPush
        interval: Number(bar.gauge[2]) || 400
        repeat: false
        onTriggered: bar.pushDialogLeg()
    }

    Timer {
        id: dlgTail
        interval: Number(bar.gauge[2]) || 400
        repeat: false
        onTriggered: bar.finishDialogLeg()
    }

    // The gauge's witness that the signal fired, taken from outside the door so the
    // `onAccepted` below stays the one statement §5.5.5 asks for. It is not the whole
    // claim: the file it names is the rest of it.
    Connections {
        target: saveAs
        function onAccepted() {
            if (bar.gauge.length === 5 && bar.dlgLeg) {
                bar.gaugeSay("dlg-accepted", String(saveAs.selectedFile))
            }
        }
        function onRejected() {
            if (bar.gauge.length === 5 && bar.dlgLeg) {
                bar.gaugeSay("dlg-rejected", String(saveAs.selectedFile))
            }
        }
    }

    // One button. `tapped` is the only thing a delegate has to say; everything the
    // button *looks* like is a property the caller binds to `view`.
    component Cell: Rectangle {
        id: cell

        property string label: ""
        property bool active: false
        property bool usable: true
        // §5.21.2 第一枚的入口。`required` 而不是 `property string hint: ""`：空串是一枚
        // 钮可以悄悄欠着的形状，而 required 让「漏了一枚」在构造期就发红，不在悬停那一刻。
        // 句子的内容不在这档 —— 工具那一排的 15 句在 Rust 的 `TOOL_TEXTS` 里（#281 同一口径：
        // 一份清单只长一处），QML 只负责按 index 递给它自己那一枚。
        required property string hint
        signal tapped()

        width: cap.implicitWidth + 14
        height: 24
        radius: 3
        // The checked state is a fill, not a border: at 24 px tall a 1 px border is
        // most of the difference between "this one" and "those ones".
        color: cell.active ? "#2d7ff7"
                           : (area.containsMouse && cell.usable ? "#3a3a41" : "transparent")

        Text {
            id: cap
            anchors.centerIn: parent
            text: cell.label
            // `font.pixelSize` and not a point size: this panel is drawn at the
            // window's device ratio, and a point size would make the bar's height
            // depend on the system font DPI rather than on the screen it is on.
            font.pixelSize: 12
            color: cell.usable ? "#f2f2f2" : "#6a6a70"
        }

        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            enabled: cell.usable
            cursorShape: cell.usable ? Qt.PointingHandCursor : Qt.ArrowCursor
            // Release-on-target, not press: a press that slides off the button is a
            // pointer that wanted to draw on the picture under the panel.
            onReleased: if (containsMouse) cell.tapped()
        }

        // §5.21.2 第一枚的落点：一只气泡长在钮本体上，而不是每一枚实例各开一只。可见性
        // 绑的是这枚钮自己的 hover，所以 `enabled: cell.usable` 那枚灰掉的钮没有气泡——
        // 与它自己的底色（`containsMouse && usable` 才亮）同一个口径：按不动的钮不假装
        // 正在被指着。
        ToolTip.text: cell.hint
        ToolTip.visible: area.containsMouse
        ToolTip.delay: 300
    }

    // A row's own name, so a row of 小/中/大 cells does not have to guess which ladder it
    // is lighting. Not a `Cell`: it takes no click, and a button that cannot be pressed is
    // the shape that promises one.
    component Caption: Text {
        id: caption

        property string label: ""

        anchors.verticalCenter: parent.verticalCenter
        text: caption.label
        font.pixelSize: 12
        color: "#8e8e96"
    }

    // One colour. A square would read as a second tool button; the thing users look
    // for is the colour itself, with the ring as the "this is the pen" mark.
    component Swatch: Rectangle {
        id: swatch

        property int rgb: 0
        property bool active: false
        // 与 `Cell` 同一件要求：这枚也是一枚钮，§5.21.2 那句「工具栏按钮应提供提示文本」
        // 不问它画的是颜色还是字。
        required property string hint
        /// §5.7.21 第二步的那一只手：「指针在**本枚**局部坐标的 (`mx`,`my`)，这是这一排的第几格？
        /// `-1` 是不落在任何一格上」。换算只有色带自己会做（它知道自己有几枚、每一枚在哪、多宽），
        /// 所以页上递进来的是这只手而不是一个格宽 —— 格宽在这里没有第二把尺（18 px 与 `spacing`
        /// 各归各的主人）。`required` 而不是默认 `null`：漏了一只手就是一枚拖不动的色块，而那
        /// 正是这一句需求要的整件事，它应当在构造期发红，不在拖不动的那一刻。
        required property var slotOf
        /// 点了一下：下一笔的笔色就是这一块（§5.7.21 第一步）。
        signal tapped()
        /// 拖到了第 `to` 格（§5.7.21 第二步）。只在松手那一格**不是**按下那一格时被发，发出去的是
        /// 页上算好的目标格号，不是像素。
        signal dragged(int to)

        width: 18
        height: 18
        radius: 9
        // `Qt.rgba`, not `Qt.rgb`: there is no `Qt.rgb` in QML - it is a C++ macro -
        // and a plain int would be read as 0xAARRGGBB, so the palette's 0x00RRGGBB
        // values would each draw a fully transparent chip. Nothing to click.
        color: Qt.rgba(((rgb >> 16) & 255) / 255, ((rgb >> 8) & 255) / 255,
                       (rgb & 255) / 255, 1.0)
        border.color: swatch.active ? "#ffffff" : "#00000000"
        border.width: swatch.active ? 2 : 0
        scale: area2.containsMouse ? 1.15 : 1.0

        MouseArea {
            id: area2
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            // 拖的判据是「松手那一格不是按下的那一格」，不是「按过而且动过」：指针滑回自己
            // 那一格松手仍然是一次点选 —— §5.7.21 第一步那颗色块不能被一次没走成的拖吞掉。
            // `from` 记的是按下那一刻落在第几格（-1 = 没落在任何格上，也就没有拖可谈）。
            property int from: -1
            onReleased: (mouse) => {
                const to = swatch.slotOf(mouse.x, mouse.y)
                if (area2.from >= 0 && to >= 0 && to !== area2.from) {
                    swatch.dragged(to)
                } else if (area2.containsMouse) {
                    swatch.tapped()
                }
                area2.from = -1
            }
            onPressed: (mouse) => { area2.from = swatch.slotOf(mouse.x, mouse.y) }
            // 一次被取消的拖（指针跑出窗、面板被关掉……）不许留下一个还亮着的 `from`：下一次
            // 按下如果没落到任何格上，它会拿**上一次**那一格当起点，把一次点选读成一次拖。
            onCanceled: { area2.from = -1 }
        }

        ToolTip.text: swatch.hint
        ToolTip.visible: area2.containsMouse
        ToolTip.delay: 300
    }

    Column {
        id: body
        x: 6
        y: 6
        spacing: 4

        // Tools. The labels are Rust's (`toolNames()`, in `TOOLS` order), so the
        // button at index `i` cannot name the tool at index `i + 1`.
        Row {
            spacing: 2

            Repeater {
                model: bar.view.toolNames().split("|")

                delegate: Cell {
                    required property int index
                    required property var modelData

                    label: modelData
                    hint: bar.view.toolHints().split("|")[index]
                    active: bar.view.tool === index
                    onTapped: bar.view.selectTool(index)
                }
            }
        }

        // Pen: colour, then the three widths, then the two style switches. One row
        // because they are one decision - what the next stroke looks like.
        Row {
            spacing: 6

            // §5.7.21 step 3's wheel, scoped to the colour band and to nothing else. A
            // `WheelHandler` is a QObject with no geometry of its own, so it hears over
            // its parent item — hence the band, its readout and the handler are wrapped in
            // one `Row` (`colorBand`, the 「主颜色按钮」) rather than the handler being a
            // child of the whole pen row, which would turn a wheel over a width cell into
            // an alpha change nobody asked for (the same reason `pens` gives below).
            Row {
                id: colorBand
                spacing: 2

                Repeater {
                    // 这一枚 id 长在 Repeater 上而不是长在 Row 上：`count` 是「页上真的造出几枚」
                    // 的读数，不是名单长度（与 `HistoryWindow.qml` 的 `buttonRows` 同一枚读数）。
                    id: band
                    model: bar.palette

                    delegate: Swatch {
                        id: chip

                        required property var modelData
                        required property int index

                        rgb: modelData
                        hint: "颜色：点一下，下一笔的笔色就是这一块；按住拖到别的色块那一格，这一排就换成那个顺序"
                        active: bar.view.pen_rgb === modelData
                        // 「指针这一落是第几格」逐枚问 Repeater 自己：第 i 枚在哪、多宽都是页上
                        // **画出**的那两个数，这里不写第二把尺（18 px 与 `spacing: 2` 各归各的主人，
                        // 把它们抄成一枚 `20` 就是一把会自己漂移的尺子）。只算横轴：拖的落点按列读，
                        // 指针往上或往下飘一点不该让一次拖作废。
                        slotOf: (mx, my) => {
                            const at = chip.mapToItem(colorBand, mx, my)
                            for (let i = 0; i < band.count; ++i) {
                                const it = band.itemAt(i)
                                if (it !== null && at.x >= it.x && at.x < it.x + it.width) {
                                    return i
                                }
                            }
                            return -1
                        }
                        onTapped: bar.view.setColor((modelData >> 16) & 255,
                                                 (modelData >> 8) & 255,
                                                 modelData & 255)
                        // 递出去的是**两个格号之差**而不是新的名单：换算、拒绝与落盘只有设置页
                        // 「上一格／下一格」那两只钮在用的那一只手（`moveColor` 走的就是它）。
                        // 页上不再排第二遍顺序，所以「页上拖成了、档里那一排没改」这一族没有出口。
                        onDragged: (to) => {
                            bar.view.moveColor(chip.index, to - chip.index)
                            bar.grabbed()
                        }
                    }
                }

                // §5.7.21 step 3's readout — 第四字节，band 那一排色块本身画不出来的那一个。
                // 绑 `pen_alpha`（Rust 现读的 qproperty，`stepAlpha` 之后 reload 把它刷新）而不是
                // 从 `pen_rgb` 里 decode：色块的名单不带 alpha，从色块上读百分比是读错了对象。
                // 它一直在这一排里，所以 `pen_for` 给马克笔种下的默认半透明一进来就看得见，
                // 不必等人滚过一次轮。
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: bar.view.pen_alpha + "%"
                    font.pixelSize: 12
                    color: bar.view.pen_alpha < 100 ? "#ff9f0a" : "#8e8e96"
                }

                WheelHandler {
                    // `notch > 0` 递 `+1`、否则 `-1`：滚一格走一格梯， rung 大小、下限与上限
                    // 都归 `Layer::step_alpha`（`ALPHA_STEP_PCT` / `ALPHA_MIN_PCT`），这一页只交
                    // 方向与格数，不再数第二把梯（与 `pens` / `fonts` / `magnifications` 每一枚
                    // 轮同一口径）。
                    onWheel: (event) => {
                        const notch = event.angleDelta.y
                        if (notch === 0) {
                            return
                        }
                        bar.view.stepAlpha(notch > 0 ? 1 : -1)
                        bar.grabbed()
                    }
                }
            }

            Item { width: 1; height: 18 }

            // Its own Row so the wheel belongs to the widths and to nothing else:
            // §5.7.20 step 3 is "hover *the button* and roll to fine-tune", and a
            // handler over the whole pen row would turn a wheel by a colour chip into
            // a line-width change nobody asked for.
            //
            // A handler installed on the Row it is declared in, which is why this one
            // is a child of `pens`: a pointer handler is a QObject, not an Item - it
            // has no width, no height and no `anchors` to fill - so the area it hears
            // over is its parent item's, and the Row's is exactly the three buttons.
            Row {
                id: pens
                spacing: 2

                Repeater {
                    model: bar.pens

                    delegate: Cell {
                        required property int index
                        required property var modelData

                        // 细 / 中 / 粗, indexed against the same list the click sends.
                        label: ["细", "中", "粗"][index]
                        hint: ["细：下一笔最窄", "中：下一笔中等宽", "粗：下一笔最宽"][index]
                        active: bar.view.pen_width === modelData
                        onTapped: bar.view.setWidth(modelData)
                    }
                }

                WheelHandler {
                    // `wheel` is this type's signal, and `angleDelta` is on the event
                    // it hands over: the handler has no `angleDelta` property of its
                    // own, and `onAngleChanged` names nothing that exists.
                    onWheel: (event) => {
                        const notch = event.angleDelta.y
                        if (notch === 0) {
                            return
                        }
                        bar.view.setWidth(bar.view.pen_width + (notch > 0 ? 1 : -1))
                        bar.grabbed()
                    }
                }
            }

            Item { width: 1; height: 18 }

            Cell {
                label: "虚线"
                hint: "虚线：把接下来画的线改成虚线"
                active: bar.view.dashed
                onTapped: {
                    bar.view.applyDashed(!bar.view.dashed)
                    bar.grabbed()
                }
            }

            Cell {
                label: "填充"
                hint: "填充：闭形标注里灌上笔色"
                active: bar.view.filled
                onTapped: {
                    bar.view.applyFilled(!bar.view.filled)
                    bar.grabbed()
                }
            }
        }

        // §5.7.9 步骤 2「设置笔刷大小和形状」与 §5.7.10 步骤 2「设置模糊强度、笔刷大小和形状」
        // 里那一枚**强度**控件：只有马赛克与模糊两枚工具显示它。门由 `effect_tool` 把 ——
        // 那一枚布尔问的是「此刻手里的工具有没有强度可设」，而工具在 `TOOLS` 里排第几是 Rust
        // 的号（QML 自己比索引就是第二把会各自漂移的梯）。标题用 `effect_caption`：同一格数字
        // 在马赛克那一排叫「块大小」、在模糊那一排叫「模糊强度」，字不同，也不许在这儿猜。
        //
        // 取值走 `effect_intensity`（Rust 现读），落值走 `applyEffectIntensity`。形态沿本文件
        // 已有的一族（轮 + 读数，与线宽、字号同写法）而不是滑杆：滑杆要 `from`/`to` 两枚边界，
        // 而那两个端点是 `annotate` 夹 intensity 的那两枚（`EFFECT_MIN..EFFECT_MAX`），从没跨过
        // 在这儿写死 1..96 就是抄散文。轮递 ±1、由 Rust 夹到它自己那两把梯的同一端点上，于是
        // 屏上的梯与盘上的夹是**同一把**梯，这一页一个边界也不持有。
        //
        // 「笔刷大小」那一半由上一排的 细/中/粗 + 轮（`setWidth`，`brush` 直径随它联动）与橡皮那
        // 一排负责，本行不重做；「形状」那一半（`brush_tool` / `brush_shape` / `applyBrushShape`）
        // 是 §5.7.10 步骤 2 的另一枚控件，紧接本行下面那一排「笔刷」接的就是它。
        Row {
            spacing: 6
            visible: bar.view.effect_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: bar.view.effect_caption
            }

            // Owned by this Row for the reason `pens` and `fonts` give: the handler hears
            // over its parent item, which is this strength's caption, readout and nothing
            // else. `notch > 0` 递 +1、否则 -1：一格一 rung，上下端点归 `annotate`。
            WheelHandler {
                onWheel: (event) => {
                    const notch = event.angleDelta.y
                    if (notch === 0) {
                        return
                    }
                    bar.view.applyEffectIntensity(bar.view.effect_intensity + (notch > 0 ? 1 : -1))
                    bar.grabbed()
                }
            }

            // The number the wheel agrees on, for the reason the 放大 row gives: between the
            // clamp's two ends nothing is lit, and a user who rolled has to be able to see
            // what the pen actually is. Read back off `effect_intensity`, not off a QML
            // running tally — §5.7.1's per-tool memory can hand either tool a strength this
            // panel never clicked.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: String(bar.view.effect_intensity)
                font.pixelSize: 12
                color: "#f2f2f2"
            }
        }

        // §5.7.8 步骤 2「设置颜色、透明度和笔刷形状」与 §5.7.9 / §5.7.10 步骤 2 里那一枚
        // **形状**：`brush_tool` / `brush_shape` / `applyBrushShape` 三样早就跨过了这道桥，
        // 界面上一直缺的是这一排（强度那一行头上那句「另账处理」记的就是它，下面接着）。三枚互斥
        // 的钮而不是一枚开关：圆头、方头与扁头（§5.7.8 规则那点名的那一枚椭圆）是三个名字，亮着
        // 的那一枚就是此刻的那一个；序号是 `annotate` 里 `BRUSH_SHAPES` 那张表数的，这一页只管名字。
        Row {
            spacing: 6
            visible: bar.view.brush_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: "笔刷"
            }

            Cell {
                label: "圆头"
                hint: "笔刷形状：圆的笔头，荧光笔与马赛克都跟着它"
                active: bar.view.brush_shape === 0
                onTapped: {
                    bar.view.applyBrushShape(0)
                    bar.grabbed()
                }
            }

            Cell {
                label: "方头"
                hint: "笔刷形状：方的笔头，涂一块是一块"
                active: bar.view.brush_shape === 1
                onTapped: {
                    bar.view.applyBrushShape(1)
                    bar.grabbed()
                }
            }

            Cell {
                label: "扁头"
                hint: "笔刷形状：§5.7.8 那句「椭圆形笔刷」—— 扁着走的那一支，垂直于走笔方向那一侧宽出一倍。标签不叫「椭圆」：那是上面工具排里那枚画椭圆的工具的名字"
                active: bar.view.brush_shape === 2
                onTapped: {
                    bar.view.applyBrushShape(2)
                    bar.grabbed()
                }
            }
        }

        // §5.7.6 步骤 2「选择单向、双向或其他箭头样式」里那**其他样式**的一排。单向与双向不在
        // 这里：那是 `TOOLS` 上两枚不同的工具，归最上面那一排，样式这一族不重做它们。三枚钮亮
        // 的是 `arrow_head` 那一个序号，句子在这一页、梯子与「出格就不动笔」在 `annotate`
        // —— 与「对齐」那一排同一形状。整行在 `arrow_tool` 那扇门后面（成对写 height 与 visible
        // 的口径同上），门是 Rust 现读的：工具在 `TOOLS` 里排第几是 Rust 的数。
        Row {
            spacing: 6
            visible: bar.view.arrow_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: "箭头"
            }

            Repeater {
                model: ["三角", "尖角", "圆点"]

                delegate: Cell {
                    required property int index
                    required property var modelData

                    // `index` 是 Rust 那张表里的序号：0 实心三角、1 空心 V、2 圆点。
                    label: modelData
                    hint: "箭头样式：箭头那头画成" + modelData
                    active: bar.view.arrow_head === index
                    onTapped: {
                        bar.view.applyArrowHead(index)
                        bar.grabbed()
                    }
                }
            }
        }

        // §5.7.3 步骤 2「配置圆角半径」：只有手里是圆角矩形时这一行才占高度。两个方向各递 ±1，
        // 一格有多宽、地板与天花板各是多少都在 `annotate`（透明度那把阶梯同一分工，这一页一个
        // 边界也不持有）。读数 `corner_radius` 是读回来的：§5.7.1 的每工具记忆可能交回一个这排
        // 从没按过的半径。步骤 4 那句「通过控制点调整……圆角」要的仍是把手那一族，本轮没接。
        Row {
            spacing: 6
            visible: bar.view.rounded_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: "圆角"
            }

            Cell {
                label: "更小"
                hint: "圆角：把这枚矩子的圆角收小一格"
                onTapped: {
                    bar.view.stepCornerRadius(-1)
                    bar.grabbed()
                }
            }

            Cell {
                label: "更大"
                hint: "圆角：把这枚矩子的圆角放大一格"
                onTapped: {
                    bar.view.stepCornerRadius(1)
                    bar.grabbed()
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "半径 " + bar.view.corner_radius + " px"
                font.pixelSize: 12
                color: "#f2f2f2"
            }
        }

        // §5.7.12 步骤 2「设置起始编号」：只有手里是编号时这一行才占高度。与上面那两把阶梯
        // 同一分工 —— 一格一步、地板 1、天花板 9999 都住在 `annotate`，这一排只递方向。
        // 读数 `counter_start` 是读回来的：它是 `Document` 计数器**此刻**的那一格，所以撤销掉
        // 一枚号之后这一格跟着往回走的是"下一枚是几"，而不是界面自己记的那个数。
        // 步骤 5「删除中间编号时可选择是否自动重排」不在这一排里：那一枚开关改的是「删除」怎么被
        // 记，所以它跟着下面「对象」那一排的删除走（`auto_renumber` / `applyAutoRenumber`）。
        Row {
            spacing: 6
            visible: bar.view.number_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: "编号"
            }

            Cell {
                label: "更小"
                hint: "编号：把下一枚号往回退一步"
                onTapped: {
                    bar.view.stepCounterStart(-1)
                    bar.grabbed()
                }
            }

            Cell {
                label: "更大"
                hint: "编号：把下一枚号往前进一步"
                onTapped: {
                    bar.view.stepCounterStart(1)
                    bar.grabbed()
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "下一枚 " + bar.view.counter_start
                font.pixelSize: 12
                color: "#f2f2f2"
            }
        }

        // §5.7.13 step 5's three settings, and only for the tool they belong to. The
        // row collapses instead of greying out: a 边框 switch is not a rectangle's
        // switch, and offering one that is lit for a tool with no border is a knob that
        // lies about what it tunes. `height` goes to 0 as well as `visible`, because the
        // `Column` above adds its spacing per child and a hidden row should not leave a
        // gap the user can see.
        Row {
            spacing: 6
            visible: bar.view.zoom_tool
            height: visible ? implicitHeight : 0

            Repeater {
                model: bar.magnifications

                delegate: Cell {
                    required property int index
                    required property var modelData

                    label: modelData + "%"
                    hint: "放大到 " + modelData + "%：把手里那张放大图按这个倍数重画"
                    active: bar.view.zoom_percent === modelData
                    onTapped: {
                        bar.view.applyZoomPercent(modelData)
                        bar.grabbed()
                    }
                }
            }

            // Owned by this Row for the reason `pens` gives: the handler hears over its
            // parent item, and that item is the magnification cells and nothing else.
            WheelHandler {
                onWheel: (event) => {
                    const notch = event.angleDelta.y
                    if (notch === 0) {
                        return
                    }
                    bar.view.applyZoomPercent(bar.view.zoom_percent + (notch > 0 ? 50 : -50))
                    bar.grabbed()
                }
            }

            // The number the buttons and the wheel agree on, because between two named
            // values none of the three cells is lit and the user is left aiming blind.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: bar.view.zoom_percent + "%"
                font.pixelSize: 12
                color: "#f2f2f2"
            }

            Item { width: 1; height: 18 }

            Cell {
                label: "边框"
                hint: "边框：给放大图描一圈边"
                active: bar.view.zoom_border
                onTapped: {
                    bar.view.applyZoomBorder(!bar.view.zoom_border)
                    bar.grabbed()
                }
            }

            Cell {
                label: "连接线"
                hint: "连接线：从源区到放大图之间牵一条线"
                active: bar.view.connection_line
                onTapped: {
                    bar.view.applyConnectionLine(!bar.view.connection_line)
                    bar.grabbed()
                }
            }
        }

        // §5.7.11 step 4's row, and only for the tool it belongs to. 颜色 is not here
        // because every pen already has one two rows up; 字体 is here but not as a `Cell`,
        // because a family name is a string the user types rather than a rung on a ladder.
        //
        // What this row cannot preview is 背景 and 描边. Step 4 comes before step 5's
        // click-away, and while the box is open its pixels belong to the `TextEdit`, not
        // to the ink layer - so the first sight of a plate or a ring is the committed
        // object. 字号 and 字体 are the exceptions, and the two that make this row more than
        // a promise about the next object: `text_font` and the field's own `font.family` are
        // the same style numbers the cells light and the string box types, so the box the
        // user is typing into changes with the click.
        Row {
            spacing: 6
            visible: bar.view.text_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: "字号"
            }

            Row {
                id: fonts
                spacing: 2

                Repeater {
                    model: bar.font_sizes

                    delegate: Cell {
                        required property int index
                        required property var modelData

                        // 小 / 中 / 大 against the same list the click sends, the shape
                        // `pens` gives 细 / 中 / 粗.
                        label: ["小", "中", "大"][index]
                        hint: ["小：框里的字最小", "中：框里的字中等大", "大：框里的字最大"][index]
                        active: bar.view.pen_font === modelData
                        onTapped: {
                            bar.view.setFontSize(modelData)
                            bar.grabbed()
                        }
                    }
                }

                // Owned by `fonts`, for the reason the pen row gives: the handler hears
                // over its parent item, which is the three size cells and nothing else.
                WheelHandler {
                    onWheel: (event) => {
                        const notch = event.angleDelta.y
                        if (notch === 0) {
                            return
                        }
                        bar.view.setFontSize(bar.view.pen_font + (notch > 0 ? 2 : -2))
                        bar.grabbed()
                    }
                }
            }

            // The number the cells and the wheel agree on, bound to Rust because between
            // two named sizes no cell is lit and the user would be left aiming blind.
            // 字号 alone: the family has its own cell below, and a readout that repeats
            // what the field beside it says is two displays of one number with two
            // lifetimes.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: bar.view.pen_font + "px"
                font.pixelSize: 12
                color: "#f2f2f2"
            }

            Item { width: 1; height: 18 }

            Caption {
                label: "字体"
            }

            // §5.7.11 step 4's 字体. A name is a string, so this is the row's one field
            // rather than a fourth cell, and it is the *only* place on the page that
            // says which family the pen carries.
            //
            // The current answer rides on `placeholderText`, which stays a binding: Qt
            // shows a placeholder only while `text` is empty, so what the user reads
            // when the box is untouched is Rust's own string, and what they type is
            // nobody else's business until they hand it over. Binding `text` instead
            // would be the trap `sizeBar` records - the first keystroke destroys the
            // binding, and the box goes on showing a family Rust never accepted.
            // Committing therefore clears the box, which puts Rust's answer back.
            //
            // Focus is this cell's one cost, and it is the one every field on these
            // windows pays: while the box holds the keys, `surface`'s arrows and chords
            // hear nothing. A click back on the mask returns them, and §5.7.11 step 5
            // is a click on the mask anyway.
            TextField {
                id: familyField

                width: 130
                height: 22
                anchors.verticalCenter: parent.verticalCenter
                font.pixelSize: 12
                color: "#f2f2f2"
                placeholderTextColor: "#8a8a90"
                placeholderText: bar.view.pen_family

                // `Layer::set_font_family` trims and keeps the pen when what arrives is
                // blank, so an empty box is 没改 rather than 改成默认 - and the box says
                // so, because after the clear the placeholder is the family again.
                onEditingFinished: {
                    bar.view.setFamily(text)
                    familyField.text = ""
                    bar.grabbed()
                }
            }

            Item { width: 1; height: 18 }

            Caption {
                label: "对齐"
            }

            Repeater {
                model: ["左对齐", "居中", "右对齐"]

                delegate: Cell {
                    required property int index
                    required property var modelData

                    // `index` is the code Rust's ladder answers to: 0 左, 1 中, 2 右.
                    label: modelData
                    hint: "对齐：这段文字" + modelData
                    active: bar.view.text_align === index
                    onTapped: {
                        bar.view.setAlign(index)
                        bar.grabbed()
                    }
                }
            }

            Item { width: 1; height: 18 }

            Cell {
                label: "背景"
                hint: "背景：给文字垫一块底板"
                active: bar.view.text_bg
                onTapped: {
                    bar.view.applyTextBg(!bar.view.text_bg)
                    bar.grabbed()
                }
            }

            Cell {
                label: "描边"
                hint: "描边：给文字勾一圈边"
                active: bar.view.text_outline
                onTapped: {
                    bar.view.applyTextOutline(!bar.view.text_outline)
                    bar.grabbed()
                }
            }

            // §5.7.11 那句「文本描边支持颜色和宽度设置」里缺的那两只手 —— 开关只是把那一圈边
            // 画出来，颜色与宽度各归这一排。整排的门是 `text_outline`：圈不画的时候摆六枚色块
            // 和一格宽度，就是让读数指着屏幕上不存在的东西（与 放大／橡皮 那两排的门同规）。
            Row {
                spacing: 6
                visible: bar.view.text_outline
                // 关着的时候这一排在横向上不占一格：`Row` 只按 `visible` 决定画不画，照旧给
                // 不可见的孩子留位置，所以宽度自己归零（Column 里那几排归零的是高度，同一件事
                // 的两个方向）。
                width: visible ? implicitWidth : 0
                height: visible ? implicitHeight : 0

                Caption {
                    label: "圈色"
                }

                // 借的是主色带那六枚芯片，但只借**点**的那一半：`slotOf` 一律答 -1，所以按住
                // 拖到别的色块那一格不会重排 —— 换算只有主色带自己会做（它知道自己几枚、每枚
                // 在哪），而这里拖出个顺序来没有任何一只手会读。
                Row {
                    spacing: 2

                    Repeater {
                        model: bar.palette

                        delegate: Swatch {
                            required property var modelData

                            rgb: modelData
                            hint: "描边颜色：点一下，那一圈边就是这一块"
                            // 亮的是**渲染那一层正在用的那一枚**（`outline_for` 的答案），不是
                            // 「用户配过没有」：没人配过时屏幕上是一圈让位给笔色的边，那一排
                            // 芯片要亮的就是那一圈，否则面板与画布会各报一枚（#281）。
                            active: bar.view.outline_rgb === modelData
                            slotOf: () => -1
                            onTapped: {
                                bar.view.setOutlineColour((modelData >> 16) & 255,
                                                          (modelData >> 8) & 255,
                                                          modelData & 255)
                                bar.grabbed()
                            }
                        }
                    }
                }

                Caption {
                    label: "圈宽"
                }

                // 与 细/中/粗 那一排同一份名单（`bar.pens`）：一圈边就是一条线，core 给线的那把
                // 尺只有一把，这里不另立第二梯。滚轮是同一句「悬停那一枚钮再滚」的微调。
                Row {
                    id: rings
                    spacing: 2

                    Repeater {
                        model: bar.pens

                        delegate: Cell {
                            required property int index
                            required property var modelData

                            label: ["细", "中", "粗"][index]
                            hint: ["细：那一圈边最窄", "中：那一圈边中等宽", "粗：那一圈边最宽"][index]
                            active: bar.view.outline_width === modelData
                            onTapped: {
                                bar.view.applyOutlineWidth(modelData)
                                bar.grabbed()
                            }
                        }
                    }

                    WheelHandler {
                        onWheel: (event) => {
                            const notch = event.angleDelta.y
                            if (notch === 0) {
                                return
                            }
                            bar.view.applyOutlineWidth(bar.view.outline_width + (notch > 0 ? 1 : -1))
                            bar.grabbed()
                        }
                    }
                }

                // 三枚有名字的芯片之外的那个读数：出厂那一档（2px）不亮任何一枚，没有它用户就
                // 只能盲滚。`outline_width` 是 Rust 现读的，滚一格 reload 之后跟着变。
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: bar.view.outline_width + "px"
                    font.pixelSize: 12
                    color: "#f2f2f2"
                }
            }
        }

        // §5.7.14's 擦除到透明, and only for the tool it belongs to - the third row of
        // the shape 放大 and 文本 already give.
        //
        // The words beside the switch are the rule, not decoration: PRD §5.7.14 lets the
        // mode exist *only* if its effect is stated, and nothing on this screen can show
        // it. QML draws the frozen desktop *under* the ink layer, so a pixel the eraser
        // refilled with the screenshot and a pixel the eraser cleared both read as the
        // desktop. The difference is in the picture that leaves the mask.
        Row {
            spacing: 6
            visible: bar.view.eraser_tool
            height: visible ? implicitHeight : 0

            Caption {
                label: "橡皮"
            }

            Cell {
                label: "擦除到透明"
                hint: "擦除到透明：橡皮擦过的地方连原始像素一起擦掉"
                active: bar.view.erase_transparent
                onTapped: {
                    bar.view.applyEraseTransparent(!bar.view.erase_transparent)
                    bar.grabbed()
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                // One line for each state, because the switch off is the state the rule
                // calls 默认 and a user who turned it on once has to be able to see that
                // they are back in it. The effect is stated as the picture rather than as
                // a file: what the hole becomes on disk is the encoder's answer (a PNG
                // keeps it, a format without alpha fills it with white).
                text: bar.view.erase_transparent
                      ? "会连原始截图的像素一起擦掉"
                      : "只擦标注，原始截图的像素保留"
                font.pixelSize: 12
                color: bar.view.erase_transparent ? "#ff9f0a" : "#8e8e96"
            }
        }

        // §5.7.16 / §5.7.17 / §5.7.18 在箭头工具下的那一行：面板报的是**这一串对象**，按下去动的
        // 也是这一串。框选那一枚是 §5.7.17 第一步的「进入选择模式」，复制与删除是第四步那四件事里
        // 能按的两枚，左转右转是 §5.7.15 第二步的「拖动旋转控制点」之外那一半「输入旋转角度」。
        // 样式那一族不在这一行里：笔上那十几枚旋钮已经顺带改这一串（`crates::annotate::Layer` 的
        // set_* 那一批），一行里再摆一套样式钮就是第二套面板。
        //
        // 整行在 `picked_count > 0 || marquee_mode` 那扇门后面（与橡皮、字号那两行同一口径：
        // visible 与 height 成对写）。枚数、角度、档开着没有，三样都从 `reload` 读回来 —— 界面
        // 一处也不自己数、自己加。
        Row {
            spacing: 6
            visible: bar.view.picked_count > 0 || bar.view.marquee_mode
            height: visible ? implicitHeight : 0

            Caption {
                label: "对象"
            }

            Cell {
                label: "框选"
                hint: "框选：拖一个框，框到的都进这一串"
                active: bar.view.marquee_mode
                onTapped: {
                    bar.view.applyMarquee(!bar.view.marquee_mode)
                    bar.grabbed()
                }
            }

            Cell {
                label: "复制"
                hint: "复制：给这一串做一份副本，副本接着被选中"
                usable: bar.view.picked_count > 0
                onTapped: {
                    bar.view.clonePicked()
                    bar.grabbed()
                }
            }

            Cell {
                label: "删除"
                hint: "删除：抹掉这一串，一次撤销整组拿回"
                usable: bar.view.picked_count > 0
                onTapped: {
                    bar.view.deletePicked()
                    bar.grabbed()
                }
            }

            // §5.7.12 步骤 5「删除中间编号时可选择是否自动重排」里的那一枚选择键。它改的是左边
            // 那一枚「删除」怎么被记 —— 亮着时那一次删除与它带着的重排是栈上的**一条**编辑 ——
            // 所以摆在这一枚旁边，而不是编号那一排：把开关门在「手里是编号」后面，用户在真正按
            // 删除的那一刻（箭头工具在手）反而找不着它。亮与灭读 `auto_renumber` 这一枚 qproperty，
            // 界面不另记一份（同一档开关的另一种写法见上面「框选」）。
            Cell {
                label: "重排"
                hint: "自动重排：删掉中间一号后，后面的号自己往前补上那个空档"
                active: bar.view.auto_renumber
                onTapped: {
                    bar.view.applyAutoRenumber(!bar.view.auto_renumber)
                    bar.grabbed()
                }
            }

            Cell {
                label: "左转"
                hint: "左转：每一枚绕它自己的中心转一格"
                usable: bar.view.picked_count > 0
                onTapped: {
                    bar.view.stepRotation(-1)
                    bar.grabbed()
                }
            }

            Cell {
                label: "右转"
                hint: "右转：每一枚绕它自己的中心转一格"
                usable: bar.view.picked_count > 0
                onTapped: {
                    bar.view.stepRotation(1)
                    bar.grabbed()
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                // 只在真有色枚可数时报这一行：那一档单独开着的时候，「0 枚 · 转 0°」是一句
                // 关于不存在的东西的读数。
                visible: bar.view.picked_count > 0
                // 那一个度数是读回来的而不是这里加的：屏幕上写的与撤销栈上落的那一条必须是同一
                // 份，一格「转了 15° 而屏上还停在 0°」没人能核对（`Layer::step_rotation` 头上
                // 那句阶梯分工讲的正是这件事）。
                text: bar.view.picked_count + " 枚 · 转 " + bar.view.picked_rotation + "°"
                font.pixelSize: 12
                color: "#8e8e96"
            }
        }

        // 那两行菜单的名字，P122 起第一次走到屏幕上：`sendToMenuNames()` 与 `shareMenuNames()`
        // 自 P91/P92 建起，`qml/` 里 0 处读者，判它们的一直是 Rust 单测与启动 banner（那两枚读的是
        // 库里那份解析结果，不是此刻这一行）。模型是 `[output] send_to_apps` / `share_targets`
        // 校验过的那一段，第 i 枚芯片只能是第 i 个目标。
        //
        // P154 起 send-to 那一半是**能按的** Cell：派发那一条腿（`sendToTarget(index)` →
        // [`crate::mask::MaskState::send_to_target`] → `falcon_core::sendto::plan` →
        // `platform_windows::open::dispatch`）已经长在 Rust 上，而 §5.5.10 步骤 2 要的正是
        // 「完成截图后点击"发送到应用"」——一枚点不动的按钮就是在邀请人点它，那一格如今反过来钉
        // 的是"按得动而没处可去"。share 那一半仍是 `Text`：provider 派发（把这一张**递给 slack**
        // 而不是递给关联表）到现在还没有 owner，所以它只把"有哪些目标"摆在屏幕上，不假装能按。
        // §5.5.9 的替代分支那一枚入口不住在这一排，住在下面那一路的「打开方式」里
        // （`bar.shareOut()`）——两处别读成两处入口：这一排是名单，那一枚才是按得动的手。
        //
        // 话（`sendToLast`）写在同一行的末尾而不是那条通知行里：通知那一行整块被
        // `MouseArea` 盖着（§5.5.4 步骤 4 的"点击打开所在目录"），把派发那句放进去会让人
        // 按一次发送却揭示一张别的文件。
        //
        // 两枚名单都空、且这一次派发也没说话时整行不占高度（与橡皮、字号那两行同一口径：
        // visible 与 height 成对写）。
        Row {
            spacing: 6
            visible: bar.send_to_menu.length > 0 || bar.share_menu.length > 0
                         || bar.view.send_to_last.length > 0
            height: visible ? implicitHeight : 0

            Caption {
                label: "发送到"
            }

            Repeater {
                model: bar.send_to_menu

                delegate: Cell {
                    required property int index
                    required property var modelData

                    label: modelData
                    hint: "发送到" + modelData + "：把这张图交给它"
                    onTapped: bar.view.sendToTarget(index)
                }
            }

            Item { width: 8; height: 1 }

            Caption {
                label: "分享"
            }

            Repeater {
                model: bar.share_menu

                delegate: Text {
                    required property var modelData

                    anchors.verticalCenter: parent.verticalCenter
                    text: modelData
                    font.pixelSize: 12
                    color: "#f2f2f2"
                }
            }

            Item { width: 8; height: 1 }

            // §5.5.10 的那一句话写在**这一排**的末尾：递出去了的那一句，与"目标不在这台机器
            // 上，请重新设置路径"等三种拒绝，说的都是刚才按的那一枚芯片。它不进下面那条通知行，
            // 因为那一整块归"点击打开所在目录"那枚 MouseArea 盖着 —— 把派发那句放进去，按一次
            // 发送就会揭示一张别的文件。
            Text {
                visible: bar.view.send_to_last.length > 0
                anchors.verticalCenter: parent.verticalCenter
                text: bar.view.send_to_last
                font.pixelSize: 12
                color: "#8e8e96"
            }
        }

        // Edits, then the ways out. 确认选区 is the same thing Enter already
        // does; the crop-and-new-pin that §5.7 asks for after it is M4b, which is why
        // this says 选区 and not 完成.
        //
        // 复制 and 保存 are the two that actually leave the program with something
        // (§5.5.2, §5.5.5), and they are greyed off with the selection rather than left
        // clickable, because a delivery with nothing selected is a refusal the user has
        // to be handed a sentence for. `Ctrl+C` and `Ctrl+S` still reach Rust's answer.
        // 快速保存 joins them from §5.5.4 with the same greying and the same chord rule,
        // and with no dialog to open - which is why its refusal has to be a sentence too:
        // a door that opens nothing leaves the message line as the only thing a user sees.
        Row {
            spacing: 2

            Cell {
                label: "撤销"
                hint: "撤销：收回最后一步改动"
                usable: bar.view.can_undo
                onTapped: {
                    if (bar.view.undoStep()) {
                        bar.grabbed()
                    }
                }
            }

            Cell {
                label: "重做"
                hint: "重做：把刚收回的那一步放回去"
                usable: bar.view.can_redo
                onTapped: {
                    if (bar.view.redoStep()) {
                        bar.grabbed()
                    }
                }
            }

            Cell {
                label: "全清"
                hint: "全清：一次抹掉这一层所有标注"
                usable: bar.view.objects > 0
                onTapped: {
                    if (bar.view.clearInk()) {
                        bar.grabbed()
                    }
                }
            }

            Item { width: 8; height: 1 }

            Cell {
                label: "复制"
                hint: "复制：把选区里的图送进剪贴板"
                usable: bar.view.selected
                onTapped: bar.copy()
            }

            // §5.6.1 步骤 2：「文字提取」挨着「复制」放 —— 两枚都对同一块选区做事，一枚交字到
            // 剪贴板、一枚交图，用户找的时候是在同一排里找「把这块选区变成能用的东西」。
            //
            // `asked` 是**递进去**而不是绑出来的（`ocrPanel.asked = true`）：绑成一串就会和面板
            // 自己那颗「收起」打架 —— 收起把 `asked` 写成假的同时把绑定踩掉，此后再按「文字提取」
            // 只是把 `bar` 上那一格重新写成它本来就是的那个值，谁也不重新求值，面板再也开不了。
            Cell {
                label: "文字提取"
                hint: "文字提取：读出选区里的字，交进剪贴板"
                usable: bar.view.selected
                onTapped: {
                    ocrPanel.asked = true
                    bar.session.extractMaskText()
                }
            }

            // §5.6.2 步骤 1/2 与 §5.6.3 步骤 1/2 在这一条栏上是**同一枚**钮。PRD 把它写成两节
            // （一节二维码、一节条形码），而读的那一只手交回的是「这一张图里所有的码」，每行自己
            // 宣布它是 QR_CODE 还是 CODE_128：两枚钮各叫一次同一只手，就是两套「读到没读到」的判据
            // 挤在同一块选区上（#281）。
            //
            // 名字取「扫码」而不是「识别二维码」：按下去读到一枚 Code 128 时，那一格标签就说了
            // 一句假话。两节的名字都在面板与菜单那一行里写全（`PinWindow.qml` 那一条是
            // 「扫码（二维码 / 条形码）」，那里有宽度）。
            //
            // `asked` 的写法与上面「文字提取」那一封同一封信：递进去而不是绑出来，绑成一串就会
            // 被面板自己那颗「收起」踩掉。
            Cell {
                label: "扫码"
                hint: "扫码：读出选区里的二维码与条形码"
                usable: bar.view.selected
                onTapped: {
                    codesPanel.asked = true
                    bar.session.readMaskCodes()
                }
            }

            // §5.5.5: the same stitched picture, to a file. Greyed with the selection
            // for the reason 复制 is - a save with nothing selected is a refusal the
            // user has to be handed a sentence for, not a dialog.
            Cell {
                label: "保存"
                hint: "保存：把选区里这张图写进一个文件"
                usable: bar.view.selected
                onTapped: bar.save()
            }

            // §5.5.4: the same write with no dialog in front of it. Next to 保存 because
            // that is where the user looks for a save, and named 快速保存 because the two
            // have to be tellable apart on screen - one asks for a name, this one takes
            // the one the settings already hold.
            Cell {
                id: quickCell
                label: "快速保存"
                hint: "快速保存：不问名字，按设置里的路径直接落盘"
                usable: bar.view.selected
                onTapped: bar.quickSave()
            }

            // §5.5.9 的替代分支。原生分享面板那一臂按 2026-11-02 的划界不做（Windows 那一片
            // 是 WinRT 的 `DataTransferManager`，要应用以包身份运行，而本程序是一枚未打包 exe），
            // 于是那一节的「隐藏入口或提供替代方案」两读里走后者：交给系统默认的看图程序。
            //
            // 名字取「打开方式」而不是「分享」：这一枚按下去**不**认目标——它把文件交给 shell 的
            // 关联表，交给的到底是哪一扇窗由那台机器说了算。把它叫「分享」而下面那一排又写着
            // slack / teams 这些**具体目标**的名字（那一排是 `[output] share_targets` 的名单，
            // 仍是 `Text`、仍不可按），用户就会以为自己递给了那一枚目标。挨着 快速保存 放，
            // 是因为两枚按的都是「先把这一张写成文件」这一条腿。
            Cell {
                label: "打开方式"
                hint: "打开方式：把这一张写成文件，再交给系统默认的看图程序"
                usable: bar.view.selected
                onTapped: bar.shareOut()
            }

            // §5.8.1 步 2：这一枚钮按下去就是「把这一块贴到屏幕上」，中间没有对话框、没有文件名。
            //
            // 名字取「贴图」而不是「立即贴图」：PRD 990 行写的就是这两个字，而它下面那一枚已经
            // 把自己说清楚了（「存完再钉图」——五个字里带着那份文件）。两枚共用的画面是同一张，
            // 分开的是「要不要一份文件」这一件事，屏幕上读得出。
            Cell {
                label: "贴图"
                hint: "贴图：把选区里这张图浮成一张贴图，不写成文件"
                usable: bar.view.selected
                onTapped: bar.pin()
            }

            // PRD 433's "完成后可复制、保存或贴图" read as one flow instead of two: the
            // picture goes to a file and the file comes back as a 贴图, so what floats is
            // the artifact the user named (§10 第 43 项 ①). Greyed with the selection for
            // the same reason 保存 is.
            Cell {
                label: "存完再钉图"
                hint: "存完再钉图：先落成文件，再把这个文件贴成浮图"
                usable: bar.view.selected
                onTapped: bar.saveAndPin()
            }

            Cell {
                label: "确认选区"
                hint: "确认选区：收下这块选区，走完成后的那一步"
                usable: bar.view.selected
                onTapped: bar.commit()
            }

            Cell {
                label: "取消"
                hint: "取消：什么都不留下，关掉这一层"
                onTapped: bar.cancel()
            }

            // What the last 复制 or 保存 did, in Rust's words. Visible only once there is
            // something to say: an empty label would take width from a sentence that
            // is not there, and the row would sit one cell further left than it did.
            //
            // §5.5.4 step 4 asks for that sentence to be *clickable*, so the click lives
            // on the line that already carries it rather than on a toast of its own: the
            // mask is still open and the user has not decided the next thing yet, so there
            // is nothing for a self-disappearing bubble to protect them from, and a second
            // window would be one more window for `--pin`'s census to have an opinion about.
            // The target only exists while there is a path：一个点了不会有任何反应的框，
            // 读起来就是一行在邀请人点的字。
            Rectangle {
                id: notice
                visible: bar.view.message.length > 0 || bar.view.pre_notice.length > 0
                         || bar.view.history_last.length > 0
                anchors.verticalCenter: parent.verticalCenter
                color: "transparent"
                radius: 3
                border.width: noticeClick.enabled ? 1 : 0
                border.color: "#5a5a63"
                width: noticeRow.implicitWidth + 12
                height: noticeRow.implicitHeight + 6

                Row {
                    id: noticeRow
                    anchors.centerIn: parent
                    spacing: 6

                    // 写之前那一句排最左。PRD 447 的措辞是"将被填充"，它要在文件落地之前
                    // 就有自己的一行，而不是被右边那句"已保存到 …"顶掉一次才算说过；两条都
                    // 留着 —— 左边是提醒的记录，右边是结果，一句不挤掉另一句。
                    Text {
                        visible: bar.view.pre_notice.length > 0
                        anchors.verticalCenter: parent.verticalCenter
                        text: bar.view.pre_notice
                        font.pixelSize: 12
                        color: "#ff9f0a"
                    }

                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: bar.view.message
                        font.pixelSize: 12
                        color: bar.delivered ? "#8e8e96" : "#ff9f0a"
                    }

                    // 只在真有路径可点时说出这一句；没有的时候那行字就是全部。
                    Text {
                        visible: bar.view.landed.length > 0
                        anchors.verticalCenter: parent.verticalCenter
                        text: "点击打开所在目录"
                        font.pixelSize: 11
                        color: "#8e8e96"
                    }

                    // 上一次揭示的话并排写着，不替换那一句：保存的证据和揭示的证据是
                    // 两句，一句都不许被另一句挤掉。
                    Text {
                        visible: bar.view.reveal_last.length > 0
                        anchors.verticalCenter: parent.verticalCenter
                        text: bar.view.reveal_last
                        font.pixelSize: 12
                        color: "#8e8e96"
                    }

                    // §5.14.1 的那一句同理：历史那一行记没记住、没记住是因为什么，是第三
                    // 件事，既不该顶掉"已保存到 …"，也不该只留在 Rust 侧那枚没人读的字段里。
                    Text {
                        visible: bar.view.history_last.length > 0
                        anchors.verticalCenter: parent.verticalCenter
                        text: bar.view.history_last
                        font.pixelSize: 12
                        color: "#8e8e96"
                    }
                }

                MouseArea {
                    id: noticeClick
                    anchors.fill: parent
                    enabled: bar.view.landed.length > 0
                    hoverEnabled: true
                    cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                    onClicked: bar.revealLanded()
                }
            }
        }

        // §5.6.1 步骤 4 的那一面板体，长在**这一条 Column 里**而不是浮在窗上：面板的宽与高都
        // 经 `body` 进 `bar` 的 `implicitWidth`/`implicitHeight`，于是头顶那两条 `x`/`y` 绑定
        // 会自动把它一起clamp 回屏内 —— 自己锚一次位置就是自己再数一遍屏幕（#281）。
        //
        // 「重新识别」在这一臂取的是**遮罩**那张：选区可能已经被改过，而桥每次都重新读
        // `delivered()`，所以这一声按下去拿的就是此刻那一张，不是上一次那一张的副本。
        OcrPanel {
            id: ocrPanel
            session: bar.session
            onReRecognized: bar.session.extractMaskText()
        }

        // §5.6.2 与 §5.6.3 共用的一面板体，理由与上面那一条一模一样：宽高等 `body` 一样经
        // 面板进 `bar` 的 `implicitWidth`/`implicitHeight`，位置由那两条绑定 clamp，不在这里
        // 自己数屏幕。这一臂的「重新扫码」重读的还是**遮罩**此刻那一张。
        CodesPanel {
            id: codesPanel
            session: bar.session
            onReRecognized: bar.session.readMaskCodes()
        }
    }

    // §5.5.5 step 2-4. The dialog decides the name and the extension, and
    // `saveSelection` reads the extension to pick the encoder, so the filter list below
    // is the list of formats this app writes from the mask - the same floor §5.5.5's
    // "至少支持 PNG、JPG、BMP" asks for, and the same three the pin menu offers.
    //
    // The list carries a second duty since P40: step 3 counts 格式 as one of the three
    // things the user chooses, so the arm that was showing decides the format of a name
    // typed without an extension, and `[output] format` only speaks when no arm names one.
    FileDialog {
        id: saveAs
        title: "另存为"
        fileMode: FileDialog.SaveFile
        nameFilters: ["PNG 图片 (*.png)", "JPG 图片 (*.jpg)", "BMP 图片 (*.bmp)"]
        // §5.5.11 步骤 3/4 的事件源，与 `PinWindow.qml` 那一张同一条腿、同一个理由：绑
        // `visible` 而不是 `accepted`/`rejected`，因为"这张对话框不在了"就是那一步的全部
        // 条件，而谁藏、藏几层、什么时候放回一律由 Rust 那一头记账（`state::PinState::dialog_opened`），
        // 这里只报"有一张开了 / 有一张关了"。遮罩这一侧也照样要报：贴图藏起来之后，
        // 用户在这张框上能看见的就没有自己刚才那一屏。
        onVisibleChanged: visible ? bar.session.dialogOpened() : bar.session.dialogClosed()
        onAccepted: {
            // `String()` rather than `.toLocalFile()`: this Qt hands JS a `QUrl` object
            // with no members at all - `toLocalFile`, `isFile` and `path` all measured
            // `undefined`, and the call threw a TypeError that §8-R14 keeps off stdout -
            // so the URL text is what travels and `save::local_path` reads it as a path.
            //
            // The second argument is the arm of the menu the user was looking at, in the
            // same flattened shape `readFilter` reports: §5.5.5 step 3 counts 格式 as one of
            // the three things chosen, so a name typed without an extension is that arm's
            // and not the setting's. An empty text is what a dialog that has selected no
            // filter answers with (`||-1`, read here the first time), and `save::write`
            // treats it as "the menu named nothing".
            // 存完再钉图 (§10 第 43 项 ①) is the same answer with one more act: the file
            // this name became is what gets pinned, so `saveAndPin` is handed exactly what
            // `saveSelection` would have been and the flag is put down before anything else
            // can read it. A pin that refused leaves the file behind and says so in the
            // same line - which is why the cell's verdict is not `bar.delivered` alone.
            const picked = String(saveAs.selectedFile)
            const chosen = saveAs.selectedNameFilter.extensions.join(",")
            bar.delivered = bar.pinAfterSave
                ? bar.view.saveAndPin(picked, chosen)
                : bar.view.saveSelection(picked, chosen)
            bar.pinAfterSave = false
            bar.grabbed()
        }
        onRejected: bar.pinAfterSave = false
    }
}
