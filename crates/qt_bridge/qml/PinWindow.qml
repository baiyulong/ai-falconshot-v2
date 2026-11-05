import QtQuick
import QtQuick.Window
import QtQuick.Controls
import QtQuick.Dialogs
import dev.falconshot 1.0

// §5.9 as a window. It owns no picture and decides nothing: every gesture is
// handed to a PinView, which is a thin cover over the Rust state machine, and
// the geometry it displays is read back from there. That is what keeps the drag
// and wheel-zoom path from re-encoding a single pixel (plan rule 4).
Window {
    id: win

    required property PinShim shim
    required property Session session

    // Not `required`: this window is made by `main.qml`'s `syncPins` before it is known
    // which pin it will show, and a required property that is missing aborts creation.
    // -1 is a pin id the state machine never hands out, so an unassigned window
    // reads as not-live rather than as somebody else's pin.
    property int pinId: -1

    // The view is filled by `assign`, not by a binding: `syncPins` tells this window
    // which pin it is *after* it is created, and a `pin_id` binding would race its own
    // change handler for who updates first.
    readonly property PinView view: PinView {}
    // The ratio comes from the state machine, not from the `Screen` attached object:
    // that one resolves which screen this window is on, which depends on x/y/width,
    // which are divided by this number - a cycle the QML engine reports as a loop.
    // The pin's own physical rect is what the answer is looked up from.
    readonly property real dpr: view.dpr

    function assign(id) {
        win.pinId = id
        win.view.pin_id = id
        win.view.reload()
    }

    // The `--pin` gauge's plan, read once at completion and split by index. Empty in
    // every session nobody asked to measure, which is what keeps the leg below inert
    // for a user. Read in `Component.onCompleted` rather than in a property
    // initializer on purpose: `session` is a *required* property, and a binding that
    // calls a method on it can be evaluated before the delegate's creator has
    // assigned it - a TypeError that takes the whole document down (§8-R14).
    //
    // The fields are read by index, and the order is `pin_check::plan_line`'s:
    // 0 目录, 1 qml.png, 2 qml.jpg, 3 never.png, 4 贴图 id, 5 一个不存在的 id,
    // 6 延迟 ms, 7 对话框默认名, 8 推开尝试上限, 9 这一腿最后要关掉的那枚 id,
    // 10 会动的那枚贴图 id, 11 那一腿自己的起步 ms.
    property var gauge: []

    // True only between the moment the dialog leg asked for a real dialog and the moment
    // it stopped watching. The gauge reads the *first* line it was given under a key, so
    // a key two phases could both write is a row graded on the wrong phase's answer -
    // and the phase above calls `accept()` directly, which is a completion this run did
    // not press. This property is what keeps `dlg-accepted` a click's word.
    property bool dlgLeg: false

    // How many times phase 2 may ask the shell for its button before it stops asking,
    // and how many times it has actually asked. The first run answered `dialogs=0` 400 ms
    // after `open()` — Qt had marked the dialog visible while Win32 had not made the
    // window yet — so the press waits for the native window instead of failing on the
    // first turn. Three runs put that window's arrival at the third wait, at the fourth,
    // and past the fourth, which is the same measurement passing and failing on a count
    // alone; the count therefore comes from the plan rather than from a second copy here.
    // The cap still has to exist: a dialog the shell never shows must end the run with a
    // `pushed=no`, not with an endless poll.
    readonly property int pushTriesMax: Number(win.gauge[8]) || 10
    property int pushTries: 0

    // Every attempt's own answer, kept apart so the count is not the only evidence: which
    // window the shell had (or did not have) at each wait is what says whether the
    // `pushed=yes` below was earned by waiting or given for free.
    property string pushTriesLog: ""

    // The same attempt's §5.5.11 reading, taken *before* the press rather than after it: the
    // press is what closes that door, so a rect read on the far side of it is a reading of a
    // window that is already gone. One line, reported under the attempt that actually pressed
    // (or under the last one, when none did) - see `dlg-probe` below.
    property string pushProbe: ""

    // The same attempt's two on-screen readings, taken beside the probe above and for the same
    // reason: `pushProbe` is the bridge's own bookkeeping (`enabled / layers / hidden`), while
    // `pushLive` is what *this* window is actually doing about it - the `view.live` that
    // `CaptureMask.qml` and the pin's `visible` bind to. A door that reads only the first can
    // report an account that balances while nothing left the screen. `pushEpoch` rides along so
    // the two can be told apart from a stale binding: the epoch is the §5.5.11 bump that made
    // this window re-read itself, so a live=false with an epoch that never moved would be a
    // coincidence rather than a consequence (#316).
    //
    // `pushLive` names the pin it speaks for, the way `sel-observe-*` does: hide is a per-pin
    // verdict, so a bare true/false from whichever window happens to be holding the gauge
    // could not be lined up against the `hidden=` list that made it.
    property string pushLive: ""
    property string pushEpoch: ""

    // §5.9.14's own leg, and the three numbers it exists to produce.
    //
    // `gifArmed` is the window: the clip's clock runs the whole time this window is alive, so
    // a counter that counted always would count the waits that belong to no leg. It opens when
    // the clip's delegate starts and closes when it reports, and it is the same kind of guard
    // `dlgLeg` is for `dlg-accepted`.
    //
    // `gifTurns` counts `gif_frameChanged` emissions; `gifSeen` keeps the *different frame
    // indices* the window was showing while armed. Both, because the ledger's reason is
    // specific (see `pin gif clock` in `pin_check.rs`): a signal count alone can be produced by
    // a setter that notifies without anything moving, while the set of indices seen cannot be -
    // and the index read at the end of the wait could legitimately be back at 0 for a clip that
    // played perfectly, since the fixture's cycle is 180 ms against this window's 400 ms wait.
    property bool gifArmed: false
    property int gifTurns: 0
    property var gifSeen: ({})

    // §7.1「GIF 不可见或最小化时，可降低刷新频率以节省资源」: 这一格问的是只有窗读得到的一件
    // 事，答的是「此刻有没有人在看这一扇窗」。
    //
    // 两枚读数缺一枚就漏一半，而这不是推想出来的：本机 Qt 6.10.1 的 `qml.exe` 在
    // `QT_QPA_PLATFORM=offscreen` 下（不开窗、不动这台机器的桌面）拿三枚探针读回来——
    // - 一扇 `visible: true` 的窗答 `visibility == Windowed`；
    // - `showMinimized()` 之后同一扇窗答 `visibility == Window.Minimized`(3) 而 `visible`
    //   **仍是 true**：Qt 的 `QWindow::isVisible()` 返回的是 `d->visible`，那是应用自己要求过
    //   的状态，最小化只由 `QWindowPrivate::updateVisibility()` 写进 `visibility`。所以只问
    //   `visible` 的那一版看不见「最小化」这一半，PRD 那句也就只兑了一半。
    // - `visible: false` 的窗答 `visibility == Window.Hidden`(0)，于是 `visible` 那一臂与
    //   `Window.Hidden` 那一臂在同一条线上互相作证，而不是其中一枚是抄来的。
    //
    // 两枚**不能用的读数**同样由探针钉死，写在这里免得下一轮再试一次：`win.visibleDepth` 当场
    // 静默 undefined（6.10.1 的 Window 类型没有这一格：`qml/QtQuick/plugins.qmltypes` 与
    // `include/QtQuick/`、`include/QtGui/` 三处都查过），拿它当读数就是把这一格永远判成「没人
    // 看」；`QWindow.Minimized` 当场 ReferenceError——枚举在这档 QML 里只认 `Window.` 前缀。
    //
    // 读不到的一半写在明处（§433 不许静默）：被另一扇窗**盖住**这一状态，Qt 在这一扇窗上没有
    // 给 QML 留任何读数（`isExposed()` 不是属性，而 Windows 上被盖住的顶层窗照样是 exposed）。
    // 本仓现成的 Win32 那一把是 `crates/platform-windows` 的 `proc::is_minimized(hwnd)`
    // （`IsIconic`，`capture.rs` 那条枚举一路在用），要接它得先把贴图自己的 HWND 交到窗这一侧，
    // 那是 `cpp/pin_shim.cpp` 与 `pin_view.rs` 的活；本档不假造第二枚读数，也不把「隐藏 + 最小化」
    // 两档说成「看不见」。
    readonly property bool gifWatched: win.visible
            && win.visibility !== Window.Minimized
            && win.visibility !== Window.Hidden

    // 这两档间隔的**作者**是 `falcon_core::pin` 的 `GIF_POLL_MS` / `GIF_HIDDEN_POLL_MS` 与那枚
    // 判据 `gif_interval`（#281：一扇窗的帧钟只能有一处说「该多久问一次」；什么时候换帧仍由
    // `PlayState::delay_ms` 一处说，这里不乘第二遍 delay）。窗这一侧把那两个数写成自己的绑定值，
    // 抄得对不对不由这段注释说了算：`falcon_core::pin` 的
    // `the_pin_qml_gif_gate_really_reads_the_window_s_own_visibility` 一格读这份源档，数字漂一格、
    // 或者 `gifClock` 的 `interval` 不再问 `gifWatched`，那一格当场红。
    readonly property int gifPollMs: 20
    readonly property int gifHiddenPollMs: 1000

    color: "transparent"

    // Frameless, no taskbar entry, and optionally top-most - all three as Qt
    // window flags rather than hand-set styles, so Qt owns the native side.
    // `applyStyle` below writes the same bits and the probe reads them back.
    flags: Qt.Window | Qt.FramelessWindowHint | Qt.Tool
           | (view.topmost ? Qt.WindowStaysOnTopHint : 0)
           | (view.click_through ? Qt.WindowTransparentForInput : 0)

    // A closed pin has no window, and neither does a thumbnail that narrowed the
    // shown region to a line - Qt would round that to zero and the pin would
    // look lost rather than small.
    visible: view.live && view.size_w > 1 && view.size_h > 1

    // The state machine is in physical pixels; a QML window is sized in logical
    // ones. Dividing here is the only place that translation happens, and the
    // probe measures whether the native window agrees.
    width: view.size_w / win.dpr
    height: view.size_h / win.dpr
    x: view.pos_x / win.dpr
    y: view.pos_y / win.dpr
    opacity: view.opacity / 100.0

    // No `onPinIdChanged`: `pin_id` is not a binding either, and a reload fired
    // from here would read the *previous* pin's state, because `assign` writes
    // `pinId` before it writes `view.pin_id`.
    Component.onCompleted: {
        // The provider has to be on the engine this window renders with, and it
        // is C++-only, so it is installed from here rather than from main().
        shim.install(win)
        view.reload()
        shim.applyStyle(win, view.topmost, view.click_through)
        win.gauge = win.session.pinGaugePlan().split("|")
    }

    // Style bits are the one thing a property change cannot carry by itself.
    function restyle() {
        shim.applyStyle(win, view.topmost, view.click_through)
    }

    // §5.10.1 的那一手，也是量规那一段走的同一处落点：`selectPin` 在本档只出现这一次，
    // 用户按下的是这一条语句，量的也是这一条（P130 那枚 `copyAction` 教的形状 —— 一只手声明
    // 一次，几个读者都指它）。分成两处写就会有一处迟早是装饰。
    //
    // 落点收的是 id 而不是「本窗」：§5.10.1 第 2 步「依次单击多个贴图窗口」在真机上的形状是同
    // 一句话被不同的窗各叫一次，而量规要问的是**另一扇窗**被选中之后自己说不说得出亮了。把 id
    // 藏进签名里，那一问就只能再写一处 `selectPin` —— 正是上面那句不许的事。
    function selectTarget(id, add) {
        return win.session.selectPin(id, add)
    }

    function selectSelf(add) {
        return win.selectTarget(win.pinId, add)
    }

    // §5.10.2 的那一手，同样在本档只出现一次：`dragPin` 收的是 id，而**搬几枚由名单决定**，
    // 不由这一扇窗决定 —— 所以这一句里不许有「本窗在不在名单上」之类的前置（窗的章程：它不回答
    // "这一拖算几枚"，那一问只有 `state::drag_pin` 一处答案）。上面 `selectTarget`
    // 那封信在这里同样成立：分成两处写就会有一处迟早是装饰，而这里的读者有四枚 —— 鼠标那一腿、
    // 量规的「拖一枚」「拖一枚不存在的」「拖两枚里的一枚」，四枚指的都是下面这一句。
    function dragTarget(id, dx, dy) {
        return win.session.dragPin(id, dx, dy)
    }

    // §5.10.3 的同一封信：透明度那两只手在本档也只各出现一句落点。两只手分工是「阶梯」与
    // 「绝对值」，而不是「一枚」与「一批」—— 分岔由 `state::batch_hand` 判一次，窗不许在这里
    // 问「本窗在不在名单上」（章程同上）。读者：菜单两枚 + Ctrl+滚轮一枚 + 量规那几枚。
    function opacityStepTarget(id, up) {
        return win.session.stepPinOpacity(id, up)
    }

    function opacitySetTarget(id, pct) {
        return win.session.setPinOpacity(id, pct)
    }

    // §5.10.4 的同一封信，三只手各一句落点：重置 / 销毁 / 撤销销毁。带走几枚由**名单**决定，
    // 不由这一扇窗决定 —— 窗在这里不许问「本窗在不在名单上」（章程同上；判据
    // `state::batch_hand` 与那张 `hand_targets` 全仓各只写一处）。关闭那一只手没有新落点：
    // 菜单与量规走的本来就是 `session.closePin`，P134 改的是它背后那只手，不是这两句字面。
    //
    // 撤销不收 id：还回来的是**最近一手**带走的那一批，那一叠槽位是桥侧那一格的账
    // （`state::trash`），窗手里压根没有它 —— 让窗传一个 id 去撤销，就是把「撤销什么」交给
    // 了一处没有这份知识的地方。
    function resetTarget(id) {
        return win.session.resetPin(id)
    }

    function destroyTarget(id) {
        return win.session.destroyPin(id)
    }

    function undoDestroyTarget() {
        return win.session.undoDestroy()
    }

    // §5.11 独显的那一手，同一封信：窗只把 id 交过去，「留哪几枚」由
    // `state::hand_targets` 答（名单里的一枚＝那一批，名单外的一枚＝只这一枚）。窗在这里
    // 不许问「本窗该不该被藏起来」—— 那一问的答案是 `view.live`，而它是桥侧 `p.visible()`
    // 读回来的**结果**，不是本窗自己判的（#281）。
    function soloTarget(id) {
        return win.session.toggleSolo(id)
    }

    // PRD 第 1149 行「批量销毁时应提示影响数量」的那一句字面数。本窗不许自己数名单（那是
    // `batch_hand` 的第二份，#281），它问的是桥侧那一格 `destroy_answer` —— 与按下「销毁」时
    // 真正带走的那些枚读的是同一张表。
    //
    // 问的**时刻**是菜单要出现的那一刻：早了（在绑定里）读到的是上一次打开时的名单，晚了
    // （在 `onTriggered` 里）贴图已经被带走了。所以这一格是显示缓存，不是判据；它由下面
    // `pinMenu` 的 `onAboutToShow` 写，而初值是 1 —— 一枚也没有选时那一手带走的就是本窗这一枚。
    property int destroyReach: 1

    // §5.11 独显那一枚字面的两格显示缓存，与上面同一封信教的形状：都在 `pinMenu` 的
    // `onAboutToShow` 里现问桥侧，窗一个也不数。
    //
    // - `soloReach`：**这一手会带走几枚**（`soloCountFor` → `state::solo_answer` → 同一张
    //   `solo_kept_table`），所以菜单上印着的数与按下之后真被带走的那些枚不可能各走各的。
    //   独显已经开着时它是 0 —— 那不是「这一手不带东西」，而是「放回几枚」的账在 core 的
    //   `Solo::saved` 里、桥侧一层也不留副本，所以退出那一支的字面不带数目（#455）。
    // - `soloWay`：这一场是**压暗**还是**隐藏**，读的是配置键 `pin.solo_dim_opacity` 的本体。
    //   把 PRD 步骤 3 那句「隐藏」写死在自己的字面上，在默认配置（35 ⇒ 压暗）下就是一句假话。
    property int soloReach: 0
    property string soloWay: "压暗"

    // §5.12 分组那一串字在本窗解码成的表：`{id, name, shown, count}` 逐组一枚，顺序与
    // `session.groups` 那串（core 的 `groups` vec 本体）一致。它是**绑定**、不是缓存 ——
    // 绑在 `session.groups` 的值上，notify 一到就重解（#316 那一族：别名/绑定绑值，不绑调用）。
    // 窗自己一个也不数（谁进哪组、有几枚，是 core 答过的事，桥侧只是抬过来）。
    readonly property var groupList: win.decodeGroups(win.session.groups)

    // 两枚控制字符与 `session::group_encode` 一一对上：组内字段用 `\u001c`（文件分隔），
    // 组之间用 `\u001d`（组分隔）。用它们而不是 `,`／`|`：分组名是用户自己打的自由文本，
    // 任何可见分隔符都可能出现在名字里把解码拆坏，而名字里出现控制字符的性能忽略。
    function decodeGroups(raw) {
        var out = []
        if (!raw || raw.length === 0)
            return out
        var recs = raw.split("\u001d")
        for (var i = 0; i < recs.length; ++i) {
            var f = recs[i].split("\u001c")
            if (f.length < 4)
                continue
            out.push({
                id: Number(f[0]),
                name: f[1],
                shown: f[2] === "1",
                count: Number(f[3])
            })
        }
        return out
    }

    // 新建与重命名共用的那一格弹窗的状态：`editId` 是 -1 就是新建、否则是改哪一组；
    // 提交那只手按 `editId` 分派 `createGroup` / `renameGroup`，两只手各一次调用、不合并（#281）。
    property int groupDialogEditId: -1
    property string groupDialogSeed: ""

    // One line to the `--pin` gauge, in the `key|value` shape `pin_check` grades.
    function gaugeSay(key, value) {
        win.session.pinGaugeReport(key + "|" + value)
    }

    // A local path as the text a `QUrl` arrives in. Spaces become `%20` because that is
    // the form a dialog hands over, and it is the reason `save::local_path` decodes: a
    // name that reaches the disk with a literal `%20` in it is a file the user cannot
    // open.
    function dlgUrl(local) {
        return "file:///" + String(local).replace(/\\/g, "/").replace(/ /g, "%20")
    }

    // Walk this window's two output items the way the menu's own handlers do, and
    // report what each one answered.
    //
    // The sentences come back from `session.status` for the dialog leg, because
    // `onAccepted` drops `savePin`'s return value and §5.9.13 says the string has to
    // reach the screen anyway - so the gauge reads the user-visible line rather than
    // a log line, which is the only way an English clause inside a Chinese prefix
    // (`保存失败：pin 3 has never been drawn`, what §10 第 37 项 ① found) shows up as
    // the thing a user would actually have read.
    //
    // This first phase does not open a dialog. Everything after it that does runs in
    // its own timed phase, because a door has to be *on the screen* while something
    // presses its button and that cannot be fitted into this event-loop turn.
    function runGauge() {
        // §5.10.1 的第一次真机对面。这一腿不新开 `win.gauge` 的槽位，也不要求第二扇窗：它问的
        // 是**本窗自己**那两格在三次手势之间怎么变。
        //
        // 判得了的是：`selection_count` 这枚 qproperty 从 QML 读得到数（不是一次数完就不动的
        // 死数）；而 `sel-plain-self` / `sel-cleared-self` 两枚读的是**这一扇窗没有自己 reload 过**
        // 的那一格 —— 手势只叫了 `selectPin`，边会亮、会灭，只能是 `selection_epoch` 那一声
        // 扇到 `main.qml` 的 `redrawPins()` 再回到本窗的 `view.reload()`。扇出没跑，这两枚就停在
        // false 上，档里读得出。
        //
        // 判不了的也点名，不假装：真按键注入没有（本仓没有一枚 SendInput），所以 Ctrl 那一路的
        // 加/减在本腿里是由 `selectPin(…, 0)` 这一手代替的，Esc 那一门是由 `trigger()` 代替的 ——
        // 两枚都只到 Action/invokable 为止，没有摸到鼠标与键盘。逐字记在门档里（#455）。
        // `mov-pos-before` 是本窗在这一段开头自报的**落点**（物理像素，读 `view.pos_x/pos_y` 而不
        // 是 `win.x` —— 后者是前者的商）。它唯一的用处是段末那一枚 `mov-pos-after`：两枚相等才叫
        // 「本轮插进来的这几拖没有把堆挪走」。上一趟真机不相等，盘证是 `pin window` 交回「顶层
        // 320x200@648,400 和要求的 608,380 相差在 2 px 内=false」，648-608=17+23、400-380=9+11 正是
        // 下面那两拖之和（`.qoder\scratch\p132_door_A.txt`）。
        gaugeSay("mov-pos-before", String(win.view.pos_x) + "," + String(win.view.pos_y))
        gaugeSay("sel-before-count", String(win.session.selection_count))
        gaugeSay("sel-before-self", String(win.view.selected))
        gaugeSay("sel-plain", String(win.selectSelf(0)))
        gaugeSay("sel-plain-count", String(win.session.selection_count))
        gaugeSay("sel-plain-self", String(win.view.selected))
        // §5.10.2 的第一次真机对面，长在多选那三腿**中间**而不是另起一段：这三问要的名单长度
        // 恰好就是上面那两手跑出来的状态 —— `sel-plain` 之后场上只有本窗一枚（拖它该交回 1），
        // `sel-add` 之后有本窗与被点名那一枚两枚（拖本窗该交回 2）。另起一段自己再 `selectPin`
        // 会把 `sel-observe-on` 那一声多扇一遍，而 `sel_rows` 数的正是「一次也没被点过的那扇窗
        // 自己说了几次」—— 尺不该给自己的证人添一枚（#281 那一族）。
        //
        // 四问里能被本窗自己判的三件环境不参与的事：**交回几枚**、`move_epoch` 那枚号**走没走**、
        // 以及本窗那枚纹理号**动没动**。为什么号可以当证人而**位置**不行：本窗的 `x` 是
        // `view.pos_x / win.dpr` 那一枚绑定的结果，它在这扇窗被分配给另一枚贴图时也会动 —— 一枚
        // 在别的因由下也会动的读数供不出「这一手扇到了那一扇窗」这一句（#271）。所以「另一扇窗
        // 自己说它被搬走了」在本轮是一问**没有证人**的，逐名记进门档的 blocked，而不是拿一枚弱尺
        // 冒充。
        //
        // 位移本身（含「撞墙那一枚不停下其余」）由 `state.rs` selftest 那四臂判：那里能现读
        // `desk.keep_visible` 算每一枚该落在哪儿，本窗算不出 —— 桌面的界没有送到 QML 这一侧。
        gaugeSay("mov-epoch-before", String(win.session.move_epoch))
        // `revision` 是本窗自己那枚「纹理换了几次」的号：搬动只改位置，而 `content_key` 不含
        // 位置 —— 所以两拖之间它必须一动不动。它动了就是每一次鼠标移动事件都在重画一张 PNG
        // （§5.10.2 那一句「一起移动」在 8 枚贴图上的代价）。
        gaugeSay("mov-rev-before", String(win.view.revision))
        gaugeSay("mov-lone", String(win.dragTarget(win.pinId, 17, 9)))
        gaugeSay("mov-epoch-lone", String(win.session.move_epoch))
        gaugeSay("mov-rev-lone", String(win.view.revision))
        // §5.10.3 的第一问长在 §5.10.2 的 `mov-rev-lone` 之后而不是另起一段：此刻名单里**只有本窗
        // 一枚**（上面 `sel-plain` 点中的就是本窗），所以这一手该交回 1 —— 它正是 Ctrl+滚轮 与菜单
        // 那三枚入口在用户只选了一枚时走的那一路，本腿不叫它就不能算被真机问过。承上面那封信：
        // 另起一段自己再 `selectPin` 会把 `sel-observe-on` 那一声多扇一遍，尺不许给自己的证人添一枚
        // （#281）。
        //
        // 归位读的是本窗自己现读到的那枚 `opLoneStart` 而不是写死 100：默认不透明度是配置里的
        // 那一格，写死就是拿本档的假设去改用户的环境（#261）。目标值 50 是阶梯中间的一格，与
        // 上面那两拖写死 (17,9) / (23,11) 同一族：它不是「环境给的数」，是这一腿自己要的一个
        // **离两端都远**的落点，好让下面那两步永远碰不到阶梯的端（贴了端反手就回不来，那是环境的
        // 答而不是产品的错，#453 那一族）。
        const opLoneStart = win.view.opacity
        gaugeSay("op-lone-start", String(opLoneStart))
        gaugeSay("op-lone-epoch-before", String(win.session.opacity_epoch))
        gaugeSay("op-lone-set", String(win.opacitySetTarget(win.pinId, 50)))
        gaugeSay("op-lone-opacity", String(win.view.opacity))
        gaugeSay("op-lone-restore", String(win.opacitySetTarget(win.pinId, opLoneStart)))
        gaugeSay("op-lone-restored", String(win.view.opacity))
        gaugeSay("op-lone-epoch-end", String(win.session.opacity_epoch))
        // §5.10.4「重置」的单枚那一臂，长在 `op-lone-*` 之后而不是另起一段：此刻名单里只有本窗
        // 一枚，这一手该交回 1 —— 它就是菜单那一枚「重置视图」在用户只选了一枚时走的那一路。
        //
        // 为什么用 `zoom` 而不是 `rotation` 供「几何被重读回来了」这一句：`rotation` 自 P145
        // 起确实送到了 QML 这一侧，但它**进** `content_key`（转一格就是重编像素），而 `zoom` 与
        // `pos` 都不进 —— 所以「重置不重编像素」这一问在这一格才判得动（下面那两枚 `rst-*-rev-*`
        // 读的就是它）。
        // 先把本窗的缩放往下迈一档（§5.9.2 那一列的单枚手势，不是批量）， reset 才有的还；
        // 迈不动（已经贴着下限）时这一格由 `rst_rows` 读成 Blocked，不读成一次成立（#433）。
        const rstLoneBase = win.view.zoom
        gaugeSay("rst-lone-base", String(rstLoneBase))
        win.view.zoomStep(false)
        gaugeSay("rst-lone-turned", String(win.view.zoom))
        gaugeSay("rst-lone-epoch-before", String(win.session.reset_epoch))
        gaugeSay("rst-lone-rev-before", String(win.view.revision))
        gaugeSay("rst-lone", String(win.resetTarget(win.pinId)))
        gaugeSay("rst-lone-zoom", String(win.view.zoom))
        gaugeSay("rst-lone-rev", String(win.view.revision))
        gaugeSay("rst-lone-epoch-end", String(win.session.reset_epoch))
        // 一扇已经关掉的窗剩下的那一次鼠标事件：id 不在集里。取场上最大那枚 id 加一 —— 那是本档
        // 唯一一枚保证还没被发出去的数，写死一枚数是猜（#261）。
        let top = 0
        for (let i = 0; i < win.session.count; ++i) {
            const one = win.session.pinId(i)
            if (one > top) top = one
        }
        gaugeSay("mov-ghost", String(win.dragTarget(top + 1, 5, 5)))
        gaugeSay("mov-epoch-ghost", String(win.session.move_epoch))
        // §5.10.3 的 ghost 那一问长在搬动的 ghost 后面：同一枚还没被发出去的 id，透明度那一手
        // 也必须交 0 **且不发号** —— 一扇已关的窗剩下的那一次滚轮不该让全场重读一遍（承
        // `Session.dragPin` 那一判据，#455 的反面：0 不许扮成一次悄悄的成功）。
        gaugeSay("op-epoch-ghost-before", String(win.session.opacity_epoch))
        gaugeSay("op-ghost", String(win.opacityStepTarget(top + 1, false)))
        gaugeSay("op-epoch-ghost", String(win.session.opacity_epoch))
        // §5.10.4「重置」的 ghost 那一问，与上面两族同一枚还没被发出去的 id：菜单上那一行
        // 是活的（用户可以让一扇已经关掉的窗的最后一次点击落到「重置视图」上），所以那只手
        // 也必须交 0 **且不发号** —— 一扇没有窗的贴图不该让全场重读一遍自己该被转成什么样子。
        // 三枚键同族同一形状：`rst-ghost` 关的是「交回几枚」，`rst-epoch-ghost` 关的是「那一声
        // 扇没扇」，两问在 §5.10.3 那一族已经被拆开判过一次，这里只是换一只手（#281 的反面：
        // 同一格问两只手，不是同一只手问两次）。
        gaugeSay("rst-epoch-ghost-before", String(win.session.reset_epoch))
        gaugeSay("rst-ghost", String(win.resetTarget(top + 1)))
        gaugeSay("rst-epoch-ghost", String(win.session.reset_epoch))
        // §5.10.1 第 2 步「依次单击多个贴图窗口」的那一记 Ctrl+单击。本腿没有鼠标也没有键盘
        // （仓里一枚 SendInput 也没有），所以走的是同一处落点 `selectTarget`，只是把 id 换成
        // **另一扇窗**的：这一格量的不是那句能不能选中，而是选中之外的那一声扇出没扇到。
        //
        // 别的窗的 id 从 `count` / `pinId(i)` 现读，和 `main.qml` 记账用的是同一条问法；本档
        // 不写死任何 id。场上只有一枚贴图时 `other` 是 -1，那三枚键就一次也不说 —— 缺席由
        // `sel_rows` 读成「这一格没有问」，不读成 0（#453）。
        const total = win.session.count
        let other = -1
        for (let i = 0; i < total; ++i) {
            const one = win.session.pinId(i)
            if (one !== win.pinId) {
                other = one
                break
            }
        }
        gaugeSay("sel-add-target", String(other))
        if (other >= 0) {
            // `sel-observe-on` 就在这一行里面说：`selectPin` 返回之前那一声 notify 已经扇完。
            gaugeSay("sel-add", String(win.selectTarget(other, 1)))
            gaugeSay("sel-add-count", String(win.session.selection_count))
            gaugeSay("sel-add-self", String(win.view.selected))
            // 这一格问的是 §5.10.2 那一句里唯一能被本窗判的那半：**这一拖那只手报了几枚**。
            // 名单此刻是两枚（本窗 + 上面刚点名那一枚），被拖的是本窗，所以交回 2 才叫「搬几枚
            // 由名单决定」；交回 1 就是那只手偷偷把批量降成了单枚。`mov-batch-target` 是这一问
            // 的证人是谁，场上只有一枚贴图时这三枚键一次也不说，缺席由 `mov_rows` 读成「这一格
            // 没有问」而不是读成 1（#453）。
            gaugeSay("mov-batch-target", String(other))
            gaugeSay("mov-batch", String(win.dragTarget(win.pinId, 23, 11)))
            gaugeSay("mov-batch-epoch", String(win.session.move_epoch))
            // §5.10.3 的第一次真机对面，长在批量那一拖**之后**而不是另起一段：这一问要的名单长度
            // 恰好就是上面那两手跑出来的状态（本窗 + 刚点名那一枚 = 两枚）。另起一段自己再
            // `selectPin` 会把 `sel-observe-on` 那一声多扇一遍，而尺不该给自己的证人添一枚
            // （#281 那一族，承上面 §5.10.2 那一封信）。
            //
            // 本窗能判的四件：**交回几枚**（名单里有第二枚时必须交 2，交 1 就是那只手偷偷把批量
            // 降成了单枚）、`opacity_epoch` 那枚号**走没走**、本窗那枚纹理号**动没动**（`opacity`
            // 不进 `content_key`，动了就是每按一次「更透明」重编一张 PNG）、以及本窗自己的
            // `view.opacity` 有没有跟着**重读**到那一步 —— 最后这一枚才是 `main.qml` 那一声明的
            // 证人：窗没有自己 reload 过，值只可能是扇出之后 `redrawPins()` 替它问回来的。
            //
            // 判不了的逐名记进门档的 blocked，不拿弱尺冒充：①「另一扇窗自己说它被调了」——QML
            // 里没有别的窗的 opacity，那一半的证人是 `state.rs` selftest 那一只手的四臂；
            // ②「两枚起点不同时各自迈一步」——本窗只能报自己那一级，起点摆不开（这里先用绝对值
            // 那一手把两枚钉在同一步上，反手才判得齐）。
            const opStart = win.view.opacity
            gaugeSay("op-start", String(opStart))
            gaugeSay("op-epoch-before", String(win.session.opacity_epoch))
            gaugeSay("op-rev-before", String(win.view.revision))
            gaugeSay("op-set-batch", String(win.opacitySetTarget(win.pinId, 50)))
            gaugeSay("op-opacity-set", String(win.view.opacity))
            gaugeSay("op-epoch-set", String(win.session.opacity_epoch))
            gaugeSay("op-rev-set", String(win.view.revision))
            // 阶梯的两步走在一枚**内部**的起点（上面那手刚把它钉在 50）上，反手才必定回到原值：
            // 阶梯在两端是停住的（20 再往下还是 20），拿一个可能贴着底的读数判「反手到位」就会
            // 在环境给的那一端红成产品的错（#453 那一族：环境的答与产品的错要分开）。
            gaugeSay("op-step-down", String(win.opacityStepTarget(win.pinId, false)))
            gaugeSay("op-opacity-down", String(win.view.opacity))
            gaugeSay("op-step-up", String(win.opacityStepTarget(win.pinId, true)))
            gaugeSay("op-opacity-up", String(win.view.opacity))
            // 归位用现读到的那枚 `opStart` 而不是写死 100：默认不透明度是配置里的那一格，写死
            // 就是拿本档的假设去改用户的环境（#261）。
            gaugeSay("op-set-back", String(win.opacitySetTarget(win.pinId, opStart)))
            gaugeSay("op-opacity-end", String(win.view.opacity))
            gaugeSay("op-epoch-end", String(win.session.opacity_epoch))
            // §5.10.4「重置」的批量那一臂，长的还是上面那两手跑出来的名单（本窗 + 刚点名那一枚
            // = 两枚）。这一格只问两件「单枚那一臂问不动」的事：**这一手报了几枚**（名单里有
            // 第二枚时必须交 2，交 1 就是那只手偷偷把批量降成了单枚）和**一次批量只发一声**
            //（两枚贴图各 sync 一遍，号却是一手一次 —— 与 `op-epoch-set` 同一形状）。
            //
            // 这里不重读 `zoom`：「把手势改过的那一档还回去」那一问在 `rst-lone-*` 那一格已经判过
            // 一次了（#281：一枚判据一个格），批量这一臂的证据是枚数与号，不是几何。纹理号也不在
            // 这里读，理由同一封。
            gaugeSay("rst-batch-target", String(other))
            gaugeSay("rst-batch-epoch-before", String(win.session.reset_epoch))
            gaugeSay("rst-batch", String(win.resetTarget(win.pinId)))
            gaugeSay("rst-batch-epoch", String(win.session.reset_epoch))
        }
        // 这枚就是「按 Esc」那扇门背后那只手：门（上面那一枚标准取消键序的 Shortcut）本腿按
        // 不了，手按得。于是那只 Action 在场上一共有**三枚读者** —— 空白那一支、Esc 那一门、
        // 和本腿。第三枚不是第三扇门，是同一只手多出来的一个测量读者；这件事由 `pin_check.rs`
        // 那枚判据逐枚点名（它把 Esc 的 handler 单独数一遍），而不是靠把数改回去。
        clearSelectionAction.trigger()
        gaugeSay("sel-cleared-count", String(win.session.selection_count))
        gaugeSay("sel-cleared-self", String(win.view.selected))
        // 反手两拖，长在这里而不是紧跟那两拖：名单此刻是空的（`trigger()` 刚清过），所以每一拖走的
        // 都是**单枚**那一路 —— 一次只搬被拖的那一扇，正好能按每一枚各自的累计位移一枚一枚抹回去
        // （本窗被单枚拖 + 批量拖 = (40,20)，另一枚只被批量拖 = (23,11)）。若把它们挤在名单还两枚
        // 的时候，任何一反手都是「按名单搬两枚」，本窗那一枚多出来的 (17,9) 就永远抹不掉：批量
        // 反手的和只能同时归零两枚中的一枚。
        //
        // 这两拖也各发一次号，所以 `mov-epoch-undo` 读 e0+4（单枚、批量、反手两拖；ghost 那一拖不发）。
        // 另一枚贴图被搬回哪儿，本窗读不到（QML 里没有别的窗的 pos），那一半的证人是 `pin window`
        // 与 `pin pile *` 那三枚行：它们从产品这一侧数场上每一扇的落点。
        if (other >= 0) {
            gaugeSay("mov-undo-self", String(win.dragTarget(win.pinId, -40, -20)))
            gaugeSay("mov-undo-other", String(win.dragTarget(other, -23, -11)))
            gaugeSay("mov-epoch-undo", String(win.session.move_epoch))
            gaugeSay("mov-pos-after", String(win.view.pos_x) + "," + String(win.view.pos_y))
        }
        // §5.9.18 的「销毁」「撤销」「影响数量」三格，加 §5.10.4 那一族里最后两只手。这一腿
        // 取的是场上除本窗以外的那些枚，理由与第 55 项那一封同一枚：**说这些话的窗不能正是被
        // 带走的那一扇** —— 窗一走，`syncPins()` 就把它从名单里摘掉，后面没有腿了。
        //
        // 还有一枚例外，写在这里而不是埋在代码里：`win.gauge[10]` 是那扇会动的贴图，它自己
        // 长着一段腿（§5.9.14），那段腿的计时器绑的是「本窗 id == 那一格」。把它销毁再撤销
        // 回来，新造的那一扇会以为轮到自己说话，于是 `gif-frames` 在 `gif-turns` 之后重说一遍、
        // 还顺手在量规要数窗的时候开着一枚 popup。这一条是**量规**的约束，不是产品的：用户
        // 销毁会动的贴图完全可以（`state::destroy_pin` 对四枚贴图一视同仁），付代价的是这段排期。
        //
        // 名单里点几枚由 `selectTarget` 那两扇门决定，本窗不数（窗的章程）；交回的枚数全部
        // 由桥侧那一只手的答复读，包括菜单字面上那一个数。
        const victim = []
        for (let i = 0; i < win.session.count; ++i) {
            const one = win.session.pinId(i)
            if (String(one) !== String(win.pinId) && String(one) !== String(win.gauge[10])) {
                victim.push(one)
            }
        }
        // 场上凑不出两枚时这一格一次也不说：`des_rows` 把它们读成「这一格没有问」，而不是
        // 读成一次交回 0 的销毁（#453/#433 —— 环境给不出证人，不是产品答错）。
        if (victim.length >= 2) {
            gaugeSay("des-target-a", String(victim[0]))
            gaugeSay("des-target-b", String(victim[1]))
            gaugeSay("des-pile-before", String(win.session.count))
            // 第一问：同一张表的两臂。名单里是**别的**两枚、本窗不在名单里，所以「销毁本窗
            // 这一枚」答 1；而「销毁 victim[0] 这一枚」答 2 —— 带走几枚由名单定，不由哪一扇
            // 窗被点定。
            win.selectTarget(victim[0], 0)
            win.selectTarget(victim[1], 1)
            gaugeSay("des-reach-self", String(win.session.destroyCountFor(win.pinId)))
            gaugeSay("des-reach-batch", String(win.session.destroyCountFor(victim[0])))
            // 第二问：那一枚数**印不印在字面上**。这一手里名单换成 {本窗, victim[0]}，于是
            // `destroyCountFor(本窗)` 答 2，而 `destroyReach` 进门时是 1 —— 字面读回 2 只有
            // `onAboutToShow` 真跑过这一条路（一枚读不出差别的最初值就是 #433 那一族）。
            // 这一手不点下去：点了带走的就是本窗。
            win.selectTarget(win.pinId, 0)
            win.selectTarget(victim[0], 1)
            pinMenu.open()
            gaugeSay("des-menu-shape", "open=" + String(pinMenu.visible)
                     + ";reach=" + String(win.destroyReach)
                     + ";undo=" + String(undoItem.visible))
            gaugeSay("des-menu-label", destroyItem.text)
            gaugeSay("des-undo-absent", String(undoItem.visible))
            pinMenu.close()
            // 第三问：按下去真带走几枚。名单换回 {victim[0], victim[1]}（本窗不在里面），
            // 点的是 `victim[0]` 那一级 —— 批量那一臂的act。
            win.selectTarget(victim[0], 0)
            win.selectTarget(victim[1], 1)
            gaugeSay("des-batch", String(win.destroyTarget(victim[0])))
            gaugeSay("des-pile-gone", String(win.session.count))
            gaugeSay("des-trashed", String(win.session.trashed))
            // 撤销那一行只在账上欠着东西时出现（菜单里那一枚死行是装饰）。
            // 这一枚读数必须**在菜单开着的时候**取：`MenuItem.visible` 跟着祖先走，菜单一关
            // 它就恒为 false，读到的就不是「这一行在不在」而是「菜单开没开」。上一趟就是在
            // 关着的时候读的（`p125_pin_p134doorA.log` 第 140 行：账上欠 2 枚而它答 false，
            // 同一档第 133 行那句 `undo=false` 是账还空着时开菜单读的——两枚读的不是同一件事）。
            pinMenu.open()
            gaugeSay("des-undo-shown", String(undoItem.visible))
            pinMenu.close()

            // ghost：一枚还没被发出去的 id（计划第五格就是这一枚），那一手交 0 **且不清账**
            // —— 清了对用户就是「点了一枚不存在的贴图，结果把刚才那批撤销的机会弄丢了」。
            gaugeSay("des-ghost", String(win.destroyTarget(Number(win.gauge[5]))))
            gaugeSay("des-trashed-kept", String(win.session.trashed))
            // 第四问：回头路。还回来的是**成员**，所以 `count` 跟着走，而 `trashed` 归零；
            // 账空了再按一次撤销交回 0 而不是把上一批还第二次（只认最近一手）。
            gaugeSay("des-undo", String(win.undoDestroyTarget()))
            gaugeSay("des-pile-back", String(win.session.count))
            gaugeSay("des-trashed-after", String(win.session.trashed))
            gaugeSay("des-undo-again", String(win.undoDestroyTarget()))
            // 名单收回到空：本腿之后的每一格（`menu` 往上那几枚读对话框的）都跑在
            // 「没有一枚在名单里」这个前提下，用的是 `sel` 那一腿同一只手。这一句故意不写出
            // 下一行的字面串：那枚判据数的是**读者**，把字面串抄进注释会让尺多读出一枚。
            clearSelectionAction.trigger()
        }
        // §5.11 独显的四臂，长在销毁那一族之后而不是另起一段：此刻场上是满的（上面刚撤销回来）、
        // 名单是空的（`trigger()` 刚清过），而独显要的正是这个起点 —— 四臂的每一枚读数都由它
        // 决定，别处再摆一遍就是给尺多添一枚证人（#281 的反面）。
        //
        // 本窗判得了的，逐件都是 §5.11 自己的话：
        // ①「应记录进入 Solo 前各贴图的显示状态」与 ②「退出后应恢复原状态，而不是无条件显示
        // 全部」—— 第二臂把本窗自己放进**不被留着**那一侧，进门时记下自己的 `view.opacity`，
        // 退出后再读同一格：不等于起点就是没恢复。
        // ③「在屏幕提示中显示当前处于 Solo 状态」—— 读的是 `session.status` 上产品自己那句
        // 活话（`HintWindow.qml` 绑的就是那一格），不是本腿替它编的一句。
        // ④「用户再次执行 Solo 或退出命令」—— 同一枚 `soloTarget` 按两下，第二下交回的是
        // **放回了**几枚，而 `solo_kept` 归零。
        //
        // 判不了的逐名记进门档的 blocked：别的窗各自被压暗/藏成什么样，QML 里没有第二扇窗的
        // `view`；那一半的证人是 `state.rs` 的 selftest，以及 `pin pile *` 那几枚从产品外侧数
        // 桌面上还剩几扇窗的行 —— 藏起来的那些枚自此不在场，那是**别人**也看得见的唯一证据。
        if (victim.length >= 2) {
            gaugeSay("solo-dims", String(win.session.soloDims()))
            gaugeSay("solo-pile", String(win.session.count))
            gaugeSay("solo-reach-self", String(win.session.soloCountFor(win.pinId)))
            gaugeSay("solo-epoch-before", String(win.session.solo_epoch))
            // 第一臂：留着本窗。被带走的是**别人**，所以本窗这一句全程看得见。
            gaugeSay("solo-enter", String(win.soloTarget(win.pinId)))
            gaugeSay("solo-epoch-enter", String(win.session.solo_epoch))
            gaugeSay("solo-kept", String(win.session.solo_kept))
            gaugeSay("solo-hidden-self", String(win.view.solo_hidden))
            gaugeSay("solo-live-self", String(win.view.live))
            gaugeSay("solo-said", win.session.status)
            pinMenu.open()
            gaugeSay("solo-menu-shape", "open=" + String(pinMenu.visible)
                     + ";reach=" + String(win.soloReach)
                     + ";way=" + win.soloWay)
            gaugeSay("solo-menu-label", soloItem.text)
            pinMenu.close()
            // 同一枚入口的第二下：PRD 步骤 4 那一句「再次执行 Solo 或退出命令」。
            gaugeSay("solo-exit", String(win.soloTarget(win.pinId)))
            gaugeSay("solo-epoch-exit", String(win.session.solo_epoch))
            gaugeSay("solo-kept-after", String(win.session.solo_kept))
            // 第二臂：留着**别人**。本窗这一句被带走，于是它读回来的每一格都只可能是扇出替它
            // 问的（本腿一次也没 `reload` 过自己）—— `main.qml` 那声明 `soloTick` 的证人就是这个
            // 形状，与 `op-opacity-set` 同一封信。
            gaugeSay("solo-target-other", String(victim[0]))
            const soloOpStart = win.view.opacity
            gaugeSay("solo-op-start", String(soloOpStart))
            gaugeSay("solo-enter-other", String(win.soloTarget(victim[0])))
            gaugeSay("solo-hidden", String(win.view.solo_hidden))
            gaugeSay("solo-live", String(win.view.live))
            gaugeSay("solo-opacity", String(win.view.opacity))
            gaugeSay("solo-epoch-other", String(win.session.solo_epoch))
            gaugeSay("solo-exit-other", String(win.soloTarget(victim[0])))
            gaugeSay("solo-op-end", String(win.view.opacity))
            gaugeSay("solo-hidden-end", String(win.view.solo_hidden))
            gaugeSay("solo-live-end", String(win.view.live))
            // 第三臂：空名单上按热键那一支（`toggleSoloSelection`，`ctrl+shift+s` 的唯一落点）。
            // core 直接拒绝，于是这一手交 0、**不发号**，屏幕上出现的是那句拒绝而不是
            // 「全场暗掉」—— §5.11 步骤 1 没满足时该产品的是这一句。
            clearSelectionAction.trigger()
            gaugeSay("solo-refuse-before", String(win.session.solo_epoch))
            gaugeSay("solo-refuse", String(win.session.toggleSoloSelection()))
            gaugeSay("solo-refuse-epoch", String(win.session.solo_epoch))
            gaugeSay("solo-refuse-kept", String(win.session.solo_kept))
            gaugeSay("solo-refuse-said", win.session.status)
            // 第四臂：一枚还没被发出去的 id（`win.gauge[5]`，与 `des-ghost` 同一枚证人）。
            // 菜单上那一行对它是活的，所以那只手也必须交 0、不动别人的窗、也不把本窗自己
            // 记成「留着的一枚」（#455：0 不许扮成一次悄悄的成功）。
            gaugeSay("solo-ghost", String(win.soloTarget(Number(win.gauge[5]))))
            gaugeSay("solo-epoch-ghost", String(win.session.solo_epoch))
            gaugeSay("solo-ghost-kept", String(win.session.solo_kept))
            // 名单与独显都收回到本腿进门时的样子：后面每一格（读对话框的那几枚）跑在
            // 「没有一枚在名单里、也没有独显压着」的前提下。
            clearSelectionAction.trigger()
        }
        // §5.9.5–§5.9.8 的六只手，长在独显那一族之后、`menu` 之前：销毁与独显那两族把名单收空、
        // 把场上收回满、也把本窗自己的透明度与显示状态还回进门时的样子，而这一族要的正是
        // 「一枚不被独显压着、名单空着的本窗」这一个起点。它自己同样收梢：四枚开关各按两下
        // （§5.9.7/§5.9.8 的「再次执行恢复」本来就是这一形状），旋转按满一整圈回到进门那个角，
        // 所以后面 `menu` 往上那几枚读对话框的行一个字节也没被本族动过。
        //
        // 每一枚读数读的都是 `view` 上那一格**本体**（P145 新送的 `rotation` / `flipped_h` /
        // `flipped_v` / `grayscale` / `inverted`），不是本腿替它记的一份副本 —— §5.9.6 步骤 3
        // 「系统更新显示状态」与 §5.9.7/§5.9.8 步骤 4「再次执行恢复」这两句，在窗上读不到那一格
        // 之前只有「按下」这一件事可证（#271：不许向一枚从没供过这格的读数要这一句）。
        //
        // 纹理号 `revision` 每按一次必须 +1：这五格全在 `state.rs` 的 `content_key` 里，按一次不
        // 重编就是像素没到屏幕上；反过来按两次 +2 也是错（同一次开关被编了两张）。
        const xfRot0 = win.view.rotation
        gaugeSay("xf-rot-start", String(xfRot0))
        gaugeSay("xf-rot-geo-start", String(win.view.size_w) + "," + String(win.view.size_h))
        gaugeSay("xf-rot-rev-before", String(win.view.revision))
        rotateRightAction.trigger()
        gaugeSay("xf-rot-cw", String(win.view.rotation))
        gaugeSay("xf-rot-rev-cw", String(win.view.revision))
        rotateLeftAction.trigger()
        gaugeSay("xf-rot-ccw", String(win.view.rotation))
        gaugeSay("xf-rot-rev-ccw", String(win.view.revision))
        rotateRightAction.trigger()
        rotateRightAction.trigger()
        gaugeSay("xf-rot-half", String(win.view.rotation))
        rotateRightAction.trigger()
        rotateRightAction.trigger()
        gaugeSay("xf-rot-full", String(win.view.rotation))
        gaugeSay("xf-rot-rev-full", String(win.view.revision))
        gaugeSay("xf-rot-geo-end", String(win.view.size_w) + "," + String(win.view.size_h))
        const xfH0 = win.view.flipped_h
        gaugeSay("xf-fh-start", String(xfH0))
        flipHAction.trigger()
        gaugeSay("xf-fh-on", String(win.view.flipped_h))
        gaugeSay("xf-fh-rev-on", String(win.view.revision))
        flipHAction.trigger()
        gaugeSay("xf-fh-off", String(win.view.flipped_h))
        gaugeSay("xf-fh-rev-off", String(win.view.revision))
        const xfV0 = win.view.flipped_v
        gaugeSay("xf-fv-start", String(xfV0))
        flipVAction.trigger()
        gaugeSay("xf-fv-on", String(win.view.flipped_v))
        gaugeSay("xf-fv-rev-on", String(win.view.revision))
        flipVAction.trigger()
        gaugeSay("xf-fv-off", String(win.view.flipped_v))
        gaugeSay("xf-fv-rev-off", String(win.view.revision))
        const xfGray0 = win.view.grayscale
        gaugeSay("xf-gray-start", String(xfGray0))
        grayscaleAction.trigger()
        gaugeSay("xf-gray-on", String(win.view.grayscale))
        gaugeSay("xf-gray-rev-on", String(win.view.revision))
        grayscaleAction.trigger()
        gaugeSay("xf-gray-off", String(win.view.grayscale))
        gaugeSay("xf-gray-rev-off", String(win.view.revision))
        const xfInv0 = win.view.inverted
        gaugeSay("xf-inv-start", String(xfInv0))
        invertAction.trigger()
        gaugeSay("xf-inv-on", String(win.view.inverted))
        gaugeSay("xf-inv-rev-on", String(win.view.revision))
        invertAction.trigger()
        gaugeSay("xf-inv-off", String(win.view.inverted))
        gaugeSay("xf-inv-rev-off", String(win.view.revision))
        // 六枚字面读的是 Action 自己那一格，不是菜单行：`MenuItem { action: ... }` 的字面就是
        // 从这一格抄来的，读两处会数出两枚读者而它们本来就是同一枚（#281）。
        gaugeSay("xf-label-left", rotateLeftAction.text)
        gaugeSay("xf-label-right", rotateRightAction.text)
        gaugeSay("xf-label-h", flipHAction.text)
        gaugeSay("xf-label-v", flipVAction.text)
        gaugeSay("xf-label-gray", grayscaleAction.text)
        gaugeSay("xf-label-inv", invertAction.text)
        // 三枚数字键自己报的键序与开关。`enabled` 在这一刻必须是真：三扇吃键盘的框都没开，
        // `win.typing` 因此为假，那一处退让没有关掉键。
        // 三枚数字键自己报的**注册情况**：`sequences` 那一枚列表的长度与 `enabled`。为什么不报
        // 键帽本体：`String(shortcut.sequences)` 出来的是 Qt 内部那套 QKeySequence 的调试串，
        // 不是这一档能拿去判的约定（约定变了读数就变，绿的红的都不说明产品）。键帽「1/2/3」
        // 钉在哪一枚键上由源码扫描那枚单测逐字读文件（那是文件的原文，比任何运行时字符串都准），
        // 「按下」那一跳没有注入器可证，逐名记在台账。
        gaugeSay("xf-key-left", String(rotateLeftKey.sequences.length) + "|" + String(rotateLeftKey.enabled))
        gaugeSay("xf-key-right", String(rotateRightKey.sequences.length) + "|" + String(rotateRightKey.enabled))
        gaugeSay("xf-key-gray", String(grayscaleKey.sequences.length) + "|" + String(grayscaleKey.enabled))
        gaugeSay("menu", saveAsItem.text)
        gaugeSay("filters", saveAs.nameFilters.join(";;"))
        // The members are read rather than assumed: `selectedNameFilter` came back as
        // a `QQuickFileNameFilter` object in the first P37 run, and the one probe that
        // enumerated it found `name`, `extensions`, `globs` and `index` - empty here,
        // because a dialog that has never been shown has not selected a filter yet.
        win.readFilter("filter")
        gaugeSay("before", String(saveAs.selectedFile))
        // The question this asks, rather than the answer it assumes: can a
        // `FileDialog` that was never `open()`ed still fire its own `accepted`? The
        // row grades what happened either way, and the two writes below are the same
        // two statements the menu items make, so the door is measured whether or not
        // a dialog that has never been shown is willing to accept.
        const url = win.dlgUrl(win.gauge[1])
        saveAs.currentFile = url
        gaugeSay("current", String(saveAs.currentFile))
        gaugeSay("call-accept", win.session.status)
        saveAs.accept()
        gaugeSay("call-after", win.session.status)
        // The PNG leg goes through the URL text, which is exactly what `onAccepted`
        // passes; the JPG leg keeps the plain path, so both spellings of the name are
        // graded by the same run.
        gaugeSay("png-url", url)
        gaugeSay("png", win.session.savePin(win.pinId, url, ""))
        gaugeSay("jpg", win.session.savePin(win.pinId, win.gauge[2], ""))
        // §5.9.16 的两条门（菜单那一项、Ctrl+C）如今点的是同一枚 Action，所以这一腿不另写
        // 一遍 `copyPin`：它 `trigger()` 的就是那枚 Action，然后把 `session.status` 上那句用户
        // 会看见的话原样交回去——和 §5.9.13 起 `accept-was` 用的是同一种读法。开不开
        // `output.copy_as_file` 由产品自己那份配置决定，这一腿两种话都只负责供出，判那两句不同
        // 的话的是 `pin_check` 的 `copy_rows`（它读的是同一份活配置，不是本轮的记忆）。
        copyAction.trigger()
        gaugeSay("copy-drawn", win.session.status)
        // The two legs that have to fail: an id the set never handed out, so the
        // door answers before it reaches the clipboard or the disk.
        gaugeSay("never-copy", win.session.copyPin(Number(win.gauge[5])))
        gaugeSay("never-save", win.session.savePin(Number(win.gauge[5]), win.gauge[3], ""))
        // §10 第 41 项 ②: the refusal above is this door's click-facing before-state, so its
        // text is read back here and `pin_check` grades the pair the way the mask door has
        // since P39. `session.status` is one slot, so any leg that writes it between this
        // line and `accept-was` replaces the product's sentence with a leg's own - which is
        // exactly what P40's JPG arm did to this door, and why that arm no longer runs here
        // (it is `jpgArmLeg` below, past the click). The schedule order is a judged row now,
        // so this placement has a reader rather than only a comment.
        gaugeSay("pre-save", win.session.status)
        win.openDialogLeg()
    }

    // The dialog leg, phase 1 of 3: hand the native dialog a folder and a name, then
    // open it for real. §10 第 38 项 ① is the reason this exists rather than another
    // `accept()` above - until P38 the gauge had proved what `onAccepted` does with the
    // text it is handed, never that the door opens the way a user's click opens it.
    //
    // `currentFolder` is set and the name is a bare stem, both measured necessities: a
    // `currentFile` naming a folder that is not there makes the dialog answer `accepted`
    // about a millisecond after `open()` and the process dies (P38's probe C,
    // `exit=-1073741819`), and a name without a folder would be resolved against
    // whatever the user's Documents is - which is not a place this run gets to write.
    //
    // The stem goes in through `currentFile` rather than `fileName`, because there is no
    // `fileName` to write: P38's probe F found the assignment answers
    // `Cannot assign to non-existent property "fileName"` and the read-back is
    // `undefined`, exactly as qmllint's `[missing-property]` said. A bare name still
    // reaches the shell - `currentFile` set to a path without an extension came back from
    // a real accept with the selected filter's extension on it (probe E), which is the
    // reading the gauge wants.
    function openDialogLeg() {
        const stem = String(win.gauge[0]) + "/" + String(win.gauge[7])
        saveAs.currentFolder = win.dlgUrl(String(win.gauge[0]) + "/")
        saveAs.currentFile = win.dlgUrl(stem)
        gaugeSay("dlg-folder", String(saveAs.currentFolder))
        gaugeSay("dlg-name", String(saveAs.currentFile))
        win.dlgLeg = true
        saveAs.open()
        // Read in the same turn that asked for it, and again from the phase that
        // presses the button: whether `open()` comes back with the dialog already up
        // is a number about this Qt, not a promise to a user, so the first is reported
        // and the second is graded.
        gaugeSay("dlg-open", String(saveAs.visible))
        dlgPush.restart()
    }

    // Phase 2: press the dialog's own default button - but only once it is actually on
    // the screen, which is not the same moment as Qt saying it opened. The first P38 run
    // answered `dialogs=0 mine_tops=12 mine_visible=1`, and the one visible top-level of
    // this process was the pin window itself: 400 ms after `open()` the shell had not
    // made its window yet. So the attempt repeats until the window is there, up to a cap.
    //
    // One attempt's answer is what the gauge grades, and it is the last one: the earlier
    // waits are the same question asked too early, and their full text travels under a key
    // of its own so that the count and the reasons stay readable. `accept-was` and the
    // other first-attempt readings are taken once, because they are the same members on
    // every attempt and the gauge grades the pair across the click rather than across the
    // waiting - an `onAccepted` that ran leaves a 已保存到 sentence behind, and nothing
    // else in this process writes one at this moment.
    function pushDialogLeg() {
        win.pushTries += 1
        // §5.5.11 的读数取在这一手的**最前**，press 之后不再问一次：那一下按下关的就是这扇窗，
        // 关完再读的矩形属于一扇已经不存在的窗。它随 `dlg-probe` 一起报出去，报的是**真的按下
        // 了那一次**（或一趟也没按下的最后一次尝试）自己的读数，不是上一轮等待留下的旧值。
        win.pushProbe = win.session.dialogProbe()
        // Read in the same breath as the probe, from the same attempt: one is the account, the
        // other is the screen, and the pair is the claim.
        win.pushLive = String(win.view.pin_id) + "=" + String(win.view.live)
        win.pushEpoch = String(win.session.dialog_epoch)
        if (win.pushTries === 1) {
            gaugeSay("dlg-shown", String(saveAs.visible))
            win.readFilter("dlg-shown-filter")
            gaugeSay("accept-was", win.session.status)
        }
        const said = win.session.pushDialogButton()
        win.pushTriesLog += (win.pushTries === 1 ? "" : " ~ ") + String(said)
        if (said.startsWith("pushed=yes") || win.pushTries >= win.pushTriesMax) {
            gaugeSay("dlg-push", said)
            gaugeSay("dlg-push-tries", String(win.pushTries))
            gaugeSay("dlg-push-log", win.pushTriesLog)
            gaugeSay("dlg-probe", win.pushProbe)
            // The account and the screen, under one key each: `dlg-probe` is what the bridge
            // believes, these two are what this window was doing about it at that same breath.
            gaugeSay("dlg-live-open", win.pushLive)
            gaugeSay("dlg-epoch-open", win.pushEpoch)
            dlgTail.restart()
        } else {
            dlgPush.restart()
        }
    }

    // Phase 3: what the click left behind.
    //
    // The dismiss is guarded on `visible` on purpose. A door the click already closed
    // is gone, and calling `reject()` on it would be a second completion this run did
    // not measure - P38's whole claim is that the tail readings come from a click.
    function finishDialogLeg() {
        gaugeSay("accept", win.session.status)
        win.readFilter("dlg-filter")
        gaugeSay("dlg-after", String(saveAs.visible))
        if (saveAs.visible) {
            gaugeSay("dlg-dismiss", "reject")
            saveAs.reject()
        }
        // 上面那一手 reject 与点击那一下关的门，两条腿之后 `dialogClosed` 都已经叫过了；这一行
        // 读的就是"叫过之后账上还剩什么"。它读在 `dlgLeg = false` 之前，是因为那条只是腿的收尾，
        // 而这一枚读数是这一格唯一的证人：读晚了（`jpgArmLeg` 会再落一次盘）就分不清是谁改的账。
        gaugeSay("dlg-probe-gone", win.session.dialogProbe())
        // The same pair on the far side of the close: `dlg-probe-gone` is the account back to
        // zero, this is the pin actually being on the screen again. A run that reports the
        // first without the second hid something it never brought back.
        gaugeSay("dlg-live-gone", String(win.view.pin_id) + "=" + String(win.view.live))
        gaugeSay("dlg-epoch-gone", String(win.session.dialog_epoch))
        // Closed last, after the reject above: that one is this phase's own dismissal and
        // is meant to be reported. From here nothing else may answer under the click's
        // keys.
        win.dlgLeg = false
        win.jpgArmLeg()
        // The leg is not over: `done` moves to the end of the close below, because the run's
        // end is armed off that key and the close is the last thing this door measures.
        win.closeArmLeg()
    }

    // §10 第 40 项 ②, at this door: a name with no extension takes the format of the arm
    // the dialog was showing, so the leg hands the door one arm's own text, read off
    // `nameFilters` because the *selected* arm is read-only (P38's probe E found the
    // assignment). The JPG arm is the one the configured `[output] format` would never
    // pick on this machine, so it is what tells a forwarded filter from one dropped at
    // the boundary - and the boundary is the question, since the legs that call the door
    // with a name already carrying an extension cannot see the difference either, and a
    // real dialog click cannot (the shell suffixes a bare name before `onAccepted` reads it).
    //
    // It runs here, after the click and its tail readings, for the reason §10 第 41 项 ②
    // registered: this leg writes `session.status`, and the graded pair above needs that
    // slot to still hold the product's own refusal when the click arrives. P40 had it
    // first in the schedule, which left `pin dialog accept` comparing two file names
    // instead of two sentences. The mask door solved the same problem by running its arm
    // *above* its refusal; this door's refusal is a leg of its own, further up, so the
    // arm goes below everything the click is measured on.
    function jpgArmLeg() {
        const bareName = win.dlgUrl(String(win.gauge[0]) + "/" + win.gauge[7] + "-f")
        const arm = win.filterExt(1)
        gaugeSay("f-jpg", arm + "|" + win.session.savePin(win.pinId, bareName, arm))
    }

    // §10 第 55 项: 关掉非最上层的一枚, at this door.
    //
    // The doomed pin's id is the plan's (field 9) rather than a number written here, and it
    // is the pin that is neither the bottom of the pile nor the top: the set is bottom-first,
    // so closing it moves the *slot* of every pin after it while leaving those pins where
    // they are. Closing the topmost pin instead would move nothing and prove nothing - and
    // would also destroy the window that the file-pin rows above read, which are this run's
    // other witnesses.
    //
    // The speaker is a different window from the victim, on purpose, and it is the reason the
    // window set is keyed by id in `main.qml`: this leg's own first version ran while an
    // `int` model owned these windows, and that model answered a membership change by
    // rebuilding every one of them - the window saying `close-before` was destroyed inside
    // the `closePin` call it had just made, the leg ended in a `TypeError` and the fresh
    // slot-0 window started the whole run again (P55, `.qoder/scratch/p55_pin_diag4.log`).
    //
    // `close-before` and `close-after` carry this window's own id and nothing else: which
    // windows are on the screen is `pin_check`'s reading to take, and what this leg decides is
    // *when* - and the first run of this leg could not tell, after a delegate was gone, which
    // delegate had spoken. `close` is the door's own answer - how many pins `closePin` removed -
    // so that a pile which did not shrink can still be told apart from a close that never ran.
    function closeArmLeg() {
        gaugeSay("close-before", String(win.pinId))
        gaugeSay("close", String(win.session.closePin(Number(win.gauge[9]))))
        closeTail.restart()
    }

    // One loop turn later, which is the whole of the wait's reason: the retirement of a
    // window whose pin left the set is deferred out of the notify that saw it leave, so this
    // turn is the first one that can see the pile finished.
    function closeAfterLeg() {
        gaugeSay("close-after", String(win.pinId))
        // Closed last, as in the dialog phase: from here nothing else may answer, and this
        // key is what arms the run's own end.
        gaugeSay("done", win.gauge[0])
    }

    // §5.9.14's leg, and the only leg of this run that belongs to a window other than the
    // subject's: it is armed by the delegate whose `pinId` is the plan's field 10, i.e. the one
    // window that has frames at all. It reads nothing Win32 owns - frames, play state and the
    // menu's items are Qt-side values the state machine filled at `assign` - which is why it
    // can start at the plan's eighth-of-a-delay (field 11) instead of the full exposure wait
    // field 6 is for. That early start is what gives these eight keys a place in
    // `pin_check::PIN_SCHEDULE` decided by arithmetic rather than by which of two timers expiring
    // in the same event-loop step Qt dispatches first.
    function gifRunLeg() {
        win.gifArmed = true
        win.gifTurns = 0
        win.gifSeen = ({})
        gaugeSay("gif-frames", String(win.view.gif_frames))
        gaugeSay("gif-frame", String(win.view.gif_frame))
        gaugeSay("gif-playing", String(win.view.gif_playing))
        // The menu's own read-back, taken twice: once with the door shut, which is what
        // P128's C 档 found answering 0 on *both* windows, and once with it open, which is the
        // reading the graded row is allowed to rest on. Opening is the product's own call - a
        // user's right-click reaches the same `Menu.open` - so the second count is not a gauge
        // invention, it is the menu the user would see.
        gaugeSay("gif-items-closed", String(win.gifMenuItemCount()))
        pinMenu.open()
        gifTail.restart()
    }

    // One wait later, which is the whole of the wait's reason: `gifClock` is the product's own
    // timer and this leg does not drive it - it only counts what the event loop did to the
    // frame while nobody was calling anything. A leg that called `gifTick` itself would be
    // measuring this file, exactly as the CLI selftest does; the claim §5.9.14 makes to a user
    // is that the picture changes by itself, and only a witness taken from outside the call
    // can say that.
    function gifTailLeg() {
        gaugeSay("gif-items", String(win.gifMenuItemCount()))
        // The mechanism, in the reading's own words rather than in a comment: whether the
        // popup says it is shown, how many items it counts, and how many children its content
        // item has. Those three are what tells a future round whether `gif-items` is a count
        // of realized rows or of declared ones.
        gaugeSay("gif-menu-shape", "open=" + String(pinMenu.visible)
                 + ";count=" + String(pinMenu.count)
                 + ";cc=" + String(pinMenu.contentChildren.length))
        gaugeSay("gif-turns", String(win.gifTurns))
        gaugeSay("gif-distinct", String(Object.keys(win.gifSeen).length))
        win.gifArmed = false
        // Shut before the census. `close-before` counts this process's top-levels and the
        // `pin pile` rows match them against four rects; a popup this leg opened and nobody
        // closed would be a fifth window, and the row would blame the pin.
        pinMenu.close()
    }

    // The same two readings on the window that has *no* clip. §5.9.14's counter-example is
    // that this pin came from `add_demo`, so the section has to read zero here - and P128's
    // C 档 showed that a shut menu reads zero on *both* windows, which makes a zero taken
    // that way no counter-example at all. So this leg opens its own menu the way the clip's
    // does and reports the pair, two hundred and fifty milliseconds of arithmetic ahead of
    // the clip's tail; `pin_check::GIF_STILL_MS` says why the arm is not the clip's arm.
    function gifStillRunLeg() {
        gaugeSay("gif-items-still-closed", String(win.gifMenuItemCount()))
        // §5.9.15's shut reading, taken on the same turn as the clip's: a row that is not
        // realized cannot be counted, and P128's C 档 is the reason the *closed* reading is
        // kept next to the open one instead of being dropped as noise.
        gaugeSay("alpha-closed", win.alphaReport())
        pinMenu.open()
        stillTail.restart()
    }

    // Shut again on the same turn it speaks, for the same reason the clip's tail does: the
    // census at `close-before` counts this process's top-levels, and a popup nobody closed
    // would be one more window than the four rects it matches.
    function gifStillTailLeg() {
        gaugeSay("gif-items-still", String(win.gifMenuItemCount()))
        // The open reading, which is the graded half: four rows, one of them marked.
        gaugeSay("alpha-open", win.alphaReport())
        // 步骤 2/3 on this window. Three writes rather than one: the board mode, the
        // transparent mode, and the mode the leg found - because §5.9.15's promise is about
        // 点击区域, and only two of those four rows have different answers to give. Reading
        // the before-state instead of setting it would tie the row to whatever `pin.alpha_bg`
        // this user's config says, and a run that reddened because someone prefers a
        // checkerboard is grading a person, not a product.
        const found = win.view.alpha_bg
        const turned = win.view.setAlphaBg(2)
        const board = win.alphaReport()
        const cleared = win.view.setAlphaBg(0)
        const transparent = win.alphaReport()
        const back = win.view.setAlphaBg(found)
        gaugeSay("alpha-turn", "found=" + String(found)
                 + ";took=" + String(turned)
                 + ";board=" + board
                 + ";cleared=" + String(cleared)
                 + ";transparent=" + transparent
                 + ";restored=" + String(back)
                 + ";after=" + win.alphaReport())
        pinMenu.close()
    }

    // How many of §5.9.14's 播放控制 items this window is actually showing. Each of the seven is
    // named by `id` and read by `visible`, so the count is taken from the items themselves
    // rather than from the condition they share - a window that reuses the condition here would
    // be grading this file's arithmetic, not the menu a user sees.
    function gifMenuItemCount() {
        const items = [gifPauseItem, gifPrevItem, gifNextItem, gifFirstItem,
                       gifSlowItem, gifFastItem, gifOriginalItem]
        let n = 0
        for (const it of items) {
            if (it.visible) {
                n += 1
            }
        }
        return n
    }

    // §5.9.15's four 背景模式 rows, counted the same way: from the items' own `visible`. The
    // section is on the menu only for a picture with a hole in it, so the count is the read-back
    // of that condition on a live window rather than a restatement of it.
    function alphaMenuItemCount() {
        const items = [alphaTransparentItem, alphaPseudoItem, alphaDarkItem, alphaLightItem]
        let n = 0
        for (const it of items) {
            if (it.visible) {
                n += 1
            }
        }
        return n
    }

    // How many of the four say "this is the mode". Four is a section that cannot be read and
    // none is a mode the menu does not know it is in, so this is graded as a count rather than
    // as which one - `mode=` in the same report names the one.
    function alphaMarkedCount() {
        const items = [alphaTransparentItem, alphaPseudoItem, alphaDarkItem, alphaLightItem]
        let n = 0
        for (const it of items) {
            if (it.checked) {
                n += 1
            }
        }
        return n
    }

    // One §5.9.15 reading of this window, in one shape so the three places it is taken differ
    // only by the moment. `hole`/`body` are two points of the demo picture: its own 2 px
    // transparent margin, and a pixel inside it - the two answers the section promises change
    // together with the mode, read through the same call the press handler makes. `rev` is in
    // it too, because 显示那一半 of the promise is a new texture going out, and a mode that
    // repaints nothing is the failure this reading exists to catch.
    function alphaReport() {
        return "holes=" + String(win.view.has_transparent_pixels)
                + ";items=" + String(win.alphaMenuItemCount())
                + ";marked=" + String(win.alphaMarkedCount())
                + ";mode=" + String(win.view.alpha_bg)
                + ";rev=" + String(win.view.revision)
                + ";hole=" + String(win.view.hitAt(0, 0))
                + ";body=" + String(win.view.hitAt(4, 4))
    }

    // `selectedNameFilter` three times, because it is read-only (assigning to it is a
    // hard `TypeError`, P38's probe E) and the three readings are the story: empty
    // before any dialog, the shell's default while one is up, and whatever the accepted
    // file's extension says afterwards. The `|` inside the value is what the gauge's
    // `name|extensions|index` shape already uses.
    function readFilter(key) {
        const chosen = saveAs.selectedNameFilter
        gaugeSay(key, chosen.name + "|" + chosen.extensions.join(",") + "|" + chosen.index)
    }

    // One arm of the menu, in the shape the selected arm arrives in: `JPG 图片 (*.jpg)` →
    // `*.jpg`. Same reading as the toolbar's function of this name, for the same reason -
    // an arm cannot be pointed at, so a leg that wants to ask what one of them does reads
    // the list.
    function filterExt(i) {
        const text = String(saveAs.nameFilters[i])
        const from = text.lastIndexOf("(") + 1
        const to = text.lastIndexOf(")")
        return to > from ? text.substring(from, to) : ""
    }

    Timer {
        id: gaugeLeg
        // The delay comes from the plan (`pin_check::LEG_DELAY_MS`) rather than being
        // written twice, and the window's native side is what it waits for: Win32 has
        // no frame for this window until the event loop has exposed it, so a leg run
        // straight from `onCompleted` would report a missing window as a missing
        // feature.
        interval: Number(win.gauge[6]) || 400
        repeat: false
        // Stopped for every delegate that is not the planned pin, and for every
        // ordinary session: `pinId` is -1 until `assign` and a plan with no gauge run
        // is one field long, not ten.
        running: win.gauge.length === 13 && String(win.gauge[4]) === String(win.pinId)
        onTriggered: win.runGauge()
    }

    // The two waits of the dialog leg, same interval from the same plan. The first is
    // the native dialog's own loop coming up after `open()`; the second is the
    // `onAccepted` tail, which cannot have run in the turn that pressed the button -
    // the shell unwinds its modal loop before Qt gets the signal.
    Timer {
        id: dlgPush
        interval: Number(win.gauge[6]) || 400
        repeat: false
        onTriggered: win.pushDialogLeg()
    }

    Timer {
        id: dlgTail
        interval: Number(win.gauge[6]) || 400
        repeat: false
        onTriggered: win.finishDialogLeg()
    }

    // The close leg's one wait (§10 第 55 项), same interval from the same plan as the two
    // above. `pin_check::LEG_WAITS` counts it: the budget and the schedule are read together
    // or the run ends before the census is taken.
    Timer {
        id: closeTail
        interval: Number(win.gauge[6]) || 400
        repeat: false
        onTriggered: win.closeAfterLeg()
    }

    // §5.9.14's two waits. The start comes from field 11 and the tail from field 6, both of
    // them written by `pin_check` - the eighth-of-a-delay is that file's arithmetic, and a
    // second copy here would be a schedule with two authors.
    Timer {
        id: gifLeg
        interval: Number(win.gauge[11]) || 50
        repeat: false
        // Only the window that owns the clip arms this: every other delegate of this run reads
        // a still picture and would report 0 frames for a leg that was never its question.
        running: win.gauge.length === 13 && String(win.gauge[10]) === String(win.pinId)
        onTriggered: win.gifRunLeg()
    }

    Timer {
        id: gifTail
        interval: Number(win.gauge[6]) || 400
        repeat: false
        onTriggered: win.gifTailLeg()
    }

    // §5.9.14's counter-example, on the window that has no clip. Both the arm and the tail
    // take field 12, so the shut reading lands at a hundred and the open one at two hundred:
    // after the clip's shut reading and before this window's own `menu`. Field 11 is not
    // used here because two timers expiring in the same event-loop step have a dispatch order
    // nobody can read off the source - which is the argument `pin_check::GIF_ARM_MS` itself
    // was written for.
    Timer {
        id: stillLeg
        interval: Number(win.gauge[12]) || 100
        repeat: false
        // Only the planned pin reads its own menu for the counter-example: the clip's leg is
        // the graded half, and a third window would speak a key the schedule has no place for.
        running: win.gauge.length === 13 && String(win.gauge[4]) === String(win.pinId)
        onTriggered: win.gifStillRunLeg()
    }

    Timer {
        id: stillTail
        interval: Number(win.gauge[12]) || 100
        repeat: false
        onTriggered: win.gifStillTailLeg()
    }

    // §5.9.14: the clip's clock. This window only ever says "so much time went
    // by" - which frame that adds up to, and whether it is the last of them, is
    // the state machine's answer, and the new picture comes back as a revision.
    //
    // `gifPollMs` is a polling rate, not a frame delay: no clip is asked to hold
    // still for it. `gif_frames > 1` is the half that keeps a still's window from
    // asking at all, so the encoder is never reached fifty times a second for a
    // picture that has no second frame.
    //
    // §7.1 adds the one thing that was missing: 没人看的那一扇不再每秒问五十次，降到
    // `gifHiddenPollMs`。降频而不是停钟，是因为 `onTriggered` 交回 Rust 的永远是窗**真正
    // 等到的那一段**（`gifClock.interval`），而 `PlayState::tick` 早就写着「迟到的一段是
    // 花掉了、不是欠着了，一次 tick 至多绕片子一圈」。于是这一格回来的那一帧是它走开时
    // 站着的那一帧（慢片子除外：一圈比一秒长的片子在看不见的那一段仍然按自己的步子走），
    // 既不中退、也不补一场爆发——读数是 `falcon_core::pin` 的
    // `the_throttled_wait_is_spent_rather_than_owed_when_the_pin_comes_back` 那一格。
    Timer {
        id: gifClock
        interval: win.gifWatched ? win.gifPollMs : win.gifHiddenPollMs
        repeat: true
        running: win.view.gif_playing && win.view.gif_frames > 1
        onTriggered: win.view.gifTick(gifClock.interval)
    }

    // The gauge's witness that the signal fired, taken from outside the door so the
    // `onAccepted` below stays the one statement §5.9.17 asks for. It is not the whole
    // claim: this object cannot write a file, so the bytes at `dialog.png` are what say
    // the product's own handler ran.
    //
    // `dlgLeg` is the half of the condition that says *which* completion is being
    // witnessed. Without it the direct `accept()` in the first phase could answer under
    // this key, and since the gauge reads the first line a key produced, the click's own
    // row would be graded on a sentence the gauge wrote itself.
    Connections {
        target: saveAs
        function onAccepted() {
            if (win.gauge.length === 13 && win.dlgLeg) win.gaugeSay("dlg-accepted", String(saveAs.selectedFile))
        }
        function onRejected() {
            if (win.gauge.length === 13 && win.dlgLeg) win.gaugeSay("dlg-rejected", String(saveAs.selectedFile))
        }
    }

    Connections {
        target: win.view
        function onTopmostChanged() { win.restyle() }
        function onClick_throughChanged() { win.restyle() }
        // §5.9.14's witness, taken from outside every call this leg makes: nothing in
        // `gifRunLeg`/`gifTailLeg` ticks the clip, so what arrives here arrived because
        // `gifClock` and the event loop did it. Counted only while a leg is watching
        // (`gifArmed`), and counted two ways - how many times the signal came, and how many
        // different frame indices were on the window while it did.
        function onGif_frameChanged() {
            if (win.gifArmed) {
                win.gifTurns += 1
                win.gifSeen[String(win.view.gif_frame)] = true
            }
        }
    }

    // The picture. `revision` is inside the URL, so a content change is the only
    // thing that makes Qt ask the provider for a new image; drag and zoom never
    // touch it. `cache: false` because the ids are recycled by the revision, not
    // by the URL.
    // §5.9.9 步骤 1「激活贴图并进入裁剪模式」——这一层的全部状态就这一格。窗只记「在不在裁剪
    // 里」；框映回图里的那一块、这一块能不能裁、裁完怎么撤销，全在 core（`PinItem::crop_window`
    // → `crop` → `crop_undo`），页上一个判据也不写（#281）。入口有两只：菜单那一行「裁剪」与
    // 下面那对 `Return` / `Esc`（步骤 3「按 Enter 确认」，PRD 只点名 Enter；退出这一手 PRD 没
    // 点名，这里给它一枚 `Esc` 加菜单上同一行的字面，因为一扇模式窗没有第二条出路就是牢房）。
    property bool cropMode: false

    // 这扇窗上吃键盘的**四**扇框：§5.12.1 的分组名、§5.9.3 的尺寸与缩放、§5.9.4 的透明度、
    // §5.8.5/§5.8.6 的卡片设置。窗内快捷键的退让只问这一格，不在每一枚键上抄一遍名单——名单抄第二份就是「一件事两个作者」，
    // 而第四框的人只会改自己那一枚键。为什么按「开着」而不是按「谁在焦点上」：§5.9.3 步骤 3 要
    // 用户往框里打的正是宽度与百分比，那串数字里有 1、2、3 —— 三枚数字键的键帽。
    readonly property bool typing: groupPrompt.visible || sizeDialog.visible
            || opacityDialog.visible || cardDialog.visible

    // 起步的框就是整扇窗：不裁任何东西的那一块矩形在 core 那一头不算一次裁剪
    // （`crop` 对 `inside == whole` 直接返回 Ok 而不压栈），所以「进来什么也没拖、直接按
    // Enter」不会在撤销栈里留下一枚空步。框的尺寸是排版不是判据，故在这里一次赋值。
    function enterCrop() {
        cropBox.x = 0
        cropBox.y = 0
        cropBox.width = win.width
        cropBox.height = win.height
        win.cropMode = true
    }

    // 交出去的是**桌面物理像素**的那一块，与 §5.9.11 右拖那一手交出去的是同一个空间、同一套
    // 换算（见 `dragArea.onReleased` 末尾那四行：窗的物理位置 + 框在窗内的逻辑偏移 × dpr）。
    // 先收模式再交矩形：`cropWindow` 之后那一次 reload 会重画窗，模式留着就还有一层压在
    // 新画面上。被 core 拒了（空框）窗也不动，页上读得到的是 reload 回来的那块。
    function commitCrop() {
        win.cropMode = false
        win.view.cropWindow(
            win.view.pos_x + Math.round(cropBox.x * win.dpr),
            win.view.pos_y + Math.round(cropBox.y * win.dpr),
            Math.round(cropBox.width * win.dpr),
            Math.round(cropBox.height * win.dpr))
    }

    // 一枚把手的共用形状：尺寸、光标，以及把这一拖的 `dx`/`dy` 原样交回外面。八枚同一形状，
    // 差别只在外面那三行（位置、光标、`onMoved` 里那一条算式）。声明在根上而不是 `cropLayer`
    // 里：Qt 6 的 inline component 只认**根对象**那一层的 `component` 声明（`Quick::` 前缀
    // 之外没有第二种写法），而 `CustomCaptureWindow.qml` 的 `component FieldRow` 也在根上。
    component Grip: Rectangle {
        id: grip

        width: 10
        height: 10
        color: "#ff1e90ff"
        border.color: "#ffffffff"
        border.width: 1
        property int arrow: Qt.ArrowCursor
        signal moved(real dx, real dy)

        MouseArea {
            anchors.fill: parent
            cursorShape: grip.arrow
            onPositionChanged: (mouse) => grip.moved(mouse.dx, mouse.dy)
        }
    }

    Image {
        id: picture
        anchors.fill: parent
        fillMode: Image.Stretch
        smooth: win.view.smooth
        cache: false
        // No key yet means no request yet: an Image that asks the provider before
        // Component.onCompleted has installed it logs a warning and paints nothing.
        source: win.view.image_key !== ""
                ? "image://falconshot/" + win.view.image_key
                : ""
    }

    // §5.10.1 步骤 3 的「统一选中状态」：这一圈边绑的就是状态机那份名单里属于本窗的那一格。
    // 窗侧**不另存名单**——`visible` 之外没有第二处读它，所以「所有选中对象显示同一种状态」
    // 这件事只有一份数据可跟。填色是 transparent：一枚描边不该把底下那些本来能看穿像素
    // （§5.9.15）堵成不透明。
    Rectangle {
        id: selRing
        anchors.fill: parent
        visible: win.view.selected
        color: "transparent"
        border.color: "#cc1e90ff"
        border.width: 3

        // §5.10.1 步骤 3 的真机证人只能由**被描边的那扇窗**自己开口。主体那一腿读别人的
        // `view.selected` 读到的是 Rust 那份权威值（`view_data`），扇出有没有跑到这一扇窗它
        // 分不出来；而这一枚 handler 读的是本窗自己那枚绑定算完的结果，它亮 = `selection_epoch`
        // 那一声经 `main.qml` 的 `redrawPins()` 回到本窗的 `view.reload()`。
        //
        // 只在「本窗不是量规主体」时说话：主体自己的亮/灭由它那一腿的 `sel-*-self` 两格读，
        // 两处都报就成了同一件事两枚数（#281）。门是既有那两枚形状（日程在场 且 不是主体）。
        //
        // 两枚键而不是一枚 `visible ? "…" : "…"`：`qml_leg_keys` 认的是 `gaugeSay(` 后面紧跟
        // 的那一串字面量，写在三元的分支里日程表就少读两枚键 —— 改名也追不到（#316 同族）。
        onVisibleChanged: {
            if (win.gauge.length === 13 && String(win.gauge[4]) !== String(win.pinId)) {
                if (visible) {
                    win.gaugeSay("sel-observe-on", String(win.pinId) + "=" + String(visible))
                } else {
                    win.gaugeSay("sel-observe-off", String(win.pinId) + "=" + String(visible))
                }
            }
        }
    }

    // §5.9.11 step 2: the right-drag selection, shown while it is being drawn.
    Rectangle {
        id: band
        visible: dragArea.bandOn
        color: "#22ffffff"
        border.color: "#ccffffff"
        border.width: 1
    }

    MouseArea {
        id: dragArea
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        // The drag deltas are kept in global coordinates on purpose: the window
        // moves out from under the cursor, so a local delta would lose the
        // remainder of every pixel and a long drag would drift.
        property real lastX
        property real lastY
        property real startX
        property real startY
        property bool bandOn: false
        // A left press that landed in a hole (§5.9.15): recorded so the rest of that
        // press - the moves that follow it, the release - is refused as one gesture
        // rather than re-asked at every stage.
        property bool refused: false

        // `globalX`/`globalY` were the Qt 5 names and are not on this type - qmllint:
        // `Member "globalX" not found on type "QQuickMouseEvent"`. Its members are x,
        // y, button, buttons, modifiers, source, isClick, wasHeld, accepted, flags, so
        // the global point comes from `Item.mapToGlobal(x, y)`, which answers in the
        // same global device-independent units the two `last*` properties hold.
        function globalAt(event) {
            return dragArea.mapToGlobal(event.x, event.y)
        }

        // §5.9.15's 点击区域 for this window: the cursor arrives in device-independent
        // units and the state machine is asked in physical ones, the same conversion
        // `freeThumbnailWindow` and `wheelZoom` are handed. 透明模式 answers false inside a
        // hole; 伪透明 and the two boards answer true everywhere, so this is the one place
        // the mode is asked about a press.
        function takesClick(event) {
            return win.view.hitAt(Math.round(event.x * win.dpr),
                                  Math.round(event.y * win.dpr))
        }

        onPressed: (event) => {
            win.view.reload()
            dragArea.refused = false
            if (event.button === Qt.LeftButton && !dragArea.takesClick(event)) {
                dragArea.bandOn = false
                dragArea.refused = true
                // §5.10.1 步骤 4 的第一扇门。这一支本来就已经是「这扇窗不吃这一击」——
                // 透明模式里的那个洞就是本窗唯一一块**空白**，所以退出多选就长在这里，
                // 而不是另找一处"空白"（其余三档模式整窗都吃点击，窗内压根没有空白可点）。
                clearSelectionAction.trigger()
                return
            }
            const g = globalAt(event)
            dragArea.lastX = g.x
            dragArea.lastY = g.y
            dragArea.startX = event.x
            dragArea.startY = event.y
            dragArea.bandOn = event.button === Qt.RightButton
            band.x = event.x
            band.y = event.y
            band.width = 1
            band.height = 1
        }

        onPositionChanged: (event) => {
            if (dragArea.refused) {
                return
            }
            if (dragArea.bandOn && (event.buttons & Qt.RightButton)) {
                band.x = Math.min(dragArea.startX, event.x)
                band.y = Math.min(dragArea.startY, event.y)
                band.width = Math.abs(event.x - dragArea.startX)
                band.height = Math.abs(event.y - dragArea.startY)
                return
            }
            if (event.buttons & Qt.LeftButton) {
                // §5.9.1 - the floor that keeps a pin findable is the state
                // machine's, so a drag towards off-screen comes back clamped.
                const g = globalAt(event)
                var dx = Math.round((g.x - dragArea.lastX) * win.dpr)
                var dy = Math.round((g.y - dragArea.lastY) * win.dpr)
                if (dx !== 0 || dy !== 0) {
                    win.dragTarget(win.pinId, dx, dy)
                    dragArea.lastX += dx / win.dpr
                    dragArea.lastY += dy / win.dpr
                }
            }
        }

        onReleased: (event) => {
            // 按下那一击被拒过（落在那个洞里），这一整个手势就不算一次选中 —— 拒的是手势，
            // 不是这一刻的按键，所以要在把 `refused` 抹掉之前问。
            var refusedThis = dragArea.refused
            dragArea.refused = false
            if (event.button === Qt.LeftButton) {
                if (refusedThis) {
                    return
                }
                var moved = Math.abs(event.x - dragArea.startX)
                          + Math.abs(event.y - dragArea.startY)
                // 没拖动才是单击（§5.10.1 步骤 1/2）。拖动不动名单：§5.9.1 那一手要的是位移，
                // 把"拖完一扇窗"也算成一次选中，就会让搬动顺手提走别人的边。
                // Ctrl 按着 = 加/减，没按 = 只留这一枚；两只手势共用这**一处**落点。
                if (moved < 4) {
                    win.selectSelf(event.modifiers & Qt.ControlModifier ? 1 : 0)
                }
                return
            }
            if (!dragArea.bandOn) {
                return
            }
            dragArea.bandOn = false
            var travelled = Math.abs(event.x - dragArea.startX)
                            + Math.abs(event.y - dragArea.startY)
            if (travelled < 4) {
                // No drag means the menu (§5.9.12 "打开贴图菜单").
                pinMenu.popup()
                return
            }
            // The selection is handed over in desktop pixels, the same space a
            // crop arrives in; what it maps to inside the picture is decided in
            // Rust.
            var x = win.view.pos_x + Math.round(band.x * win.dpr)
            var y = win.view.pos_y + Math.round(band.y * win.dpr)
            var w = Math.round(band.width * win.dpr)
            var h = Math.round(band.height * win.dpr)
            win.view.freeThumbnailWindow(x, y, w, h)
        }

        // §5.9.2 - the pixel under the cursor is the one that stays put.
        // §5.9.4 - the same wheel with Ctrl held is opacity instead of zoom, and
        // how low it may go is the state machine's floor, not this file's.
        // §5.10.3 - 这一手从 P133 起走的是 `Session.stepPinOpacity`：滚轮落在哪一扇窗上，不决定
        // 这次调整改几枚贴图，**名单**才决定（判据在 Rust 那一侧只写一次，`batch_hand`）。本窗不
        // 在这里分「调一枚」与「调一批」—— 那件事本窗的章程说了它不决定，而分岔一旦有两处，
        // 场上一处改了判据另一处没改就是 #281 那一族。
        // §5.9.14 - and with Alt held it is the clip's 播放速度: the section asks
        // for the wheel to adjust it, and plain and Ctrl were spoken for first.
        // Alt only answers for a pin that has frames to run through.
        onWheel: (wheel) => {
            if (wheel.modifiers & Qt.AltModifier && win.view.gif_frames > 1) {
                win.view.gifSpeedStep(wheel.angleDelta.y > 0)
            } else if (wheel.modifiers & Qt.ControlModifier) {
                win.opacityStepTarget(win.pinId, wheel.angleDelta.y > 0)
            } else {
                win.view.wheelZoom(wheel.angleDelta.y > 0,
                                   Math.round(wheel.x * win.dpr),
                                   Math.round(wheel.y * win.dpr))
            }
            wheel.accepted = true
        }

        // §5.9.10 - Shift+double-click is the shortcut the requirement names.
        // Leaving a thumbnail mode is the menu's, or another press of the same
        // shortcut for the fixed one; a plain double-click is not spoken for.
        // A double-click the pin refused to take (§5.9.15: both presses fell in a hole)
        // is not a shortcut either - the flag is the second press's answer, which is the
        // one this gesture is made of.
        onDoubleClicked: (event) => {
            if (dragArea.refused) {
                return
            }
            if (event.modifiers & Qt.ShiftModifier) {
                win.view.toggleThumbnail()
            }
        }
    }

    // §5.8.4 方式二：拖进**这一枚**贴图窗的那几格（文件与网页地址混着拖也算）。
    //
    // 这一层只做两件页做得动的事：把 `mimeData.urls` 那几格串成一串（分隔符是
    // `settings_view::RECORD` 那一枚控制字符 `\u001d`——整个程序里列表分隔符只有这一个作者，而选
    // 它的理由在这里同样成立：格子的**值是用户自己拖进来的**，任何可见字符都可能出现在里面），以及
    // 把这一枚窗的 `pinId` 一起递过去。什么算文件、什么算地址、这一趟换不换这一枚、取不取得回，
    // 一句都不在页上判：那是 `falcon_core::drop` 与 `crate::drop_in` 各自的作者（#281）。
    //
    // `String(urls[i])` 而不是 `urls[i].toLocalFile()`：本机这代 Qt 给 JS 的 `QUrl` 一个成员都读不
    // 出来（P37 在 `AnnotationToolbar.qml` 与 `PinWindow.qml` 那两处 `accepted` 上量过 `TypeError`），
    // 所以递过桥的只能是整句 URL，认它的手在 `crate::save::local_path`。
    //
    // 交回的那一句不读：`session.pinDrop` 已经把同一句写进 `session.status`，与 `saveAs` 那一腿同
    // 一形状（`onAccepted` 丢掉返回值，屏幕上那一行由 status 那一格说）。
    //
    // 穿透模式（§5.9.12 那枚 `Qt.WindowTransparentForInput`）下这一格收不到落下：整扇窗对输入是
    // 透明的，PRD 给的出口是 §5.9.13 那一枚「关掉穿透」的入口，不是这里再想一个绕过它的办法。
    DropArea {
        id: dropIn
        anchors.fill: parent
        onDropped: (drop) => {
            const urls = drop.mimeData.urls
            const parts = []
            for (let i = 0; i < urls.length; ++i) {
                parts.push(String(urls[i]))
            }
            drop.accept()
            win.session.pinDrop(win.pinId, parts.join(String.fromCharCode(0x1d)))
        }
    }

    // §5.9.9 步骤 2「拖动裁剪边界」的那一层：一块压暗、一枚裁剪框、八条可拖的边界（四边 +
    // 四角）与整块的移动。它在 `dragArea` **之后**声明，所以同一坐标下这一层吃下鼠标、窗不再
    // 被拖动 —— 模式期间窗属于裁剪，这正是步骤 1 那一句「进入裁剪模式」要的形状；退出（Enter
    // 或 Esc）后 `dragArea` 立刻恢复它原本的那一手，两处不重叠。
    //
    // 八条算式各只写在自己那一行，判的是「这一拖把手挪到窗内的哪一格」，是排版不是裁剪判据：
    // 最小 1 px 只为「别把框抹成一格都摸不着」，而这一块到图里是哪一块、装不装得下，由 core
    // 那一手判（`crop_window` → `crop`，上面 `commitCrop` 那封信）。
    Rectangle {
        id: cropLayer

        visible: win.cropMode
        anchors.fill: parent
        color: "#66000000"

        Rectangle {
            id: cropBox

            color: "transparent"
            border.color: "#ffeeeeee"
            border.width: 1

            // 框体：整块搬动，动的是位置不是尺寸。
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.SizeAllCursor
                onPositionChanged: (mouse) => {
                    cropBox.x = Math.max(0, Math.min(cropBox.x + mouse.dx,
                                                     cropLayer.width - cropBox.width))
                    cropBox.y = Math.max(0, Math.min(cropBox.y + mouse.dy,
                                                     cropLayer.height - cropBox.height))
                }
            }

            Grip {
                x: -5
                y: cropBox.height / 2 - 5
                arrow: Qt.SizeHorCursor
                onMoved: (dx, dy) => {
                    var nx = Math.max(0, Math.min(cropBox.x + dx, cropBox.right - 1))
                    cropBox.width = cropBox.right - nx
                    cropBox.x = nx
                }
            }
            Grip {
                x: cropBox.width - 5
                y: cropBox.height / 2 - 5
                arrow: Qt.SizeHorCursor
                onMoved: (dx, dy) => {
                    cropBox.width = Math.max(1, Math.min(cropBox.width + dx,
                                                         cropLayer.width - cropBox.x))
                }
            }
            Grip {
                x: cropBox.width / 2 - 5
                y: -5
                arrow: Qt.SizeVerCursor
                onMoved: (dx, dy) => {
                    var ny = Math.max(0, Math.min(cropBox.y + dy, cropBox.bottom - 1))
                    cropBox.height = cropBox.bottom - ny
                    cropBox.y = ny
                }
            }
            Grip {
                x: cropBox.width / 2 - 5
                y: cropBox.height - 5
                arrow: Qt.SizeVerCursor
                onMoved: (dx, dy) => {
                    cropBox.height = Math.max(1, Math.min(cropBox.height + dy,
                                                          cropLayer.height - cropBox.y))
                }
            }
            Grip {
                x: -5
                y: -5
                arrow: Qt.SizeFDiagCursor
                onMoved: (dx, dy) => {
                    var nx = Math.max(0, Math.min(cropBox.x + dx, cropBox.right - 1))
                    var ny = Math.max(0, Math.min(cropBox.y + dy, cropBox.bottom - 1))
                    cropBox.width = cropBox.right - nx
                    cropBox.height = cropBox.bottom - ny
                    cropBox.x = nx
                    cropBox.y = ny
                }
            }
            Grip {
                x: cropBox.width - 5
                y: -5
                arrow: Qt.SizeBDiagCursor
                onMoved: (dx, dy) => {
                    var ny = Math.max(0, Math.min(cropBox.y + dy, cropBox.bottom - 1))
                    cropBox.height = cropBox.bottom - ny
                    cropBox.y = ny
                    cropBox.width = Math.max(1, Math.min(cropBox.width + dx,
                                                         cropLayer.width - cropBox.x))
                }
            }
            Grip {
                x: -5
                y: cropBox.height - 5
                arrow: Qt.SizeBDiagCursor
                onMoved: (dx, dy) => {
                    var nx = Math.max(0, Math.min(cropBox.x + dx, cropBox.right - 1))
                    cropBox.width = cropBox.right - nx
                    cropBox.x = nx
                    cropBox.height = Math.max(1, Math.min(cropBox.height + dy,
                                                          cropLayer.height - cropBox.y))
                }
            }
            Grip {
                x: cropBox.width - 5
                y: cropBox.height - 5
                arrow: Qt.SizeFDiagCursor
                onMoved: (dx, dy) => {
                    cropBox.width = Math.max(1, Math.min(cropBox.width + dx,
                                                         cropLayer.width - cropBox.x))
                    cropBox.height = Math.max(1, Math.min(cropBox.height + dy,
                                                           cropLayer.height - cropBox.y))
                }
            }
        }
    }

    // The single-pin menu (§5.9.12 to §5.9.18). §5.10 的批量菜单不是另一期工程，也不是菜单
    // 之外的另一处地方：下面那几行（§5.10.3 的透明度、§5.10.4 的重置/关闭/销毁）就长在这一份菜单里。
    // "这一手带走几枚"由桥侧那一处答（`state::batch_hand`：被叫的那一枚在名单里、且名单不止一枚
    // 时才是批量，否则只叫本窗），窗一枚也不数。

    // §5.9.16 step 2 names two doors to one thing - 「按 `Ctrl+C` 或选择"复制图片"」 - so
    // the thing is declared once and both doors point at it. An `Action` rather than a
    // second copy of the call, because a duplicated `onTriggered` is a second route that
    // can drift, and `trigger()` is the half a `Shortcut` can fire.
    Action {
        id: copyAction

        text: "复制图片"
        onTriggered: win.session.copyPin(win.pinId)
    }

    // `StandardKey.Copy` rather than the string "Ctrl+C": a "copy" chord depends on the
    // platform and the layout, and this type's own answer is the one that already accounts
    // for both. Window scope on purpose - `ApplicationShortcut` would take Ctrl+C away from
    // every other window in the process, and the PRD puts the gesture on the 贴图.
    // 一枚标准键序在 Qt 里可以对应**多枚** native 键绑定（`StandardKey.Copy` 这一路就有
    // Ctrl+C 与 Ctrl+Ins 两枚），而 `sequence:` 只抬第一枚 —— Qt 会为这一手在每趟跑完时抱怨
    // 一句，而那一句正是量规 `no qml complaints` 那枚行判红的证人（盘证：从
    // `p125_pin_p130doorD.log` 起就红，本轮 `p125_pin_p131doorF.log` 两臂皆红）。改用
    // `sequences:`，把这一路的**全部**键绑定都接上，而不是只接第一枚。
    Shortcut {
        sequences: [StandardKey.Copy]
        context: Qt.WindowShortcut
        onActivated: copyAction.trigger()
    }

    // §5.10.1 步骤 4 要的是**两只门、一只手**：「点空白区域」与「按 Esc」都退出多选，所以
    // 与 `copyAction` 同一形状 —— 声明一次，两扇门都 `trigger()`，而 `win.session.clearSelection`
    // 在场上一处也不会多。§5.10 的批量菜单早就在这扇窗的右键菜单上了（「重置视图」「关闭」
    // 「销毁」三枚入口见下面那一段），而这一条依旧不挂进去：§5.10.1 步骤 4 点名的退出入口只有
    // 空白区域与 Esc 两只门。但它**不能**因此
    // `enabled: false`：一枚 disabled 的 Action 的 `trigger()` 是空转，两扇门就一起哑了。
    Action {
        id: clearSelectionAction

        text: "取消选中"
        onTriggered: win.session.clearSelection()
    }

    // Esc 这一支走的是 Qt 的**标准取消键序**，与上面那枚复制用的标准键序同一形状：键位交给
    // Qt 按平台与键盘布局自己答，本档不硬写码点 —— 硬写一枚码点还会把「Esc」这一问写成
    // 只在这一种布局上成立的答案。同一枚理由也把它从 `sequence:` 抬进 `sequences:`。
    Shortcut {
        sequences: [StandardKey.Cancel]
        context: Qt.WindowShortcut
        // 裁剪模式下让给下面那枚 `cropEscapeKey`：那时 `Esc` 说的是「退出这一层」，不是
        // 「清掉多选」（两枚 `enabled` 互斥，同一刻场上只有一枚在听）。
        enabled: !win.cropMode
        onActivated: clearSelectionAction.trigger()
    }

    // §5.9.5 步骤 2「使用数字键或菜单执行向左、向右旋转」和 §5.9.7 步骤 2「使用对应快捷键或菜单
    // 选择"灰度"」各给了两只门，所以形状照上面 `copyAction` 那一封：落点声明一次，菜单那一行
    // `action:` 到它，键也 `trigger()` 它。六枚**全部**提成 Action，而不只提那三枚有键的，是因为
    // `pin_check.rs` 的 `xf-*` 那一族要在真机上按同一只手：菜单那一行自己没有 `onTriggered` 可叫
    // （`action:` 之后 handler 住在 Action 里），量规叫得着的只有 `trigger()` —— 与上面那封信里
    // `clearSelectionAction` 有「三枚读者」是同一件事，多出来的那一枚读者是尺自己。
    //
    // 字面一律取 PRD 那六句里的词：§5.9.5 写的是「向左、向右旋转」而不是「顺时针/逆时针」，
    // §5.9.8 写的是「颜色反转」而不是「反色」。方向和角度对得上是因为 core 的
    // `rotate_cw` 是 +90（顺时针 = 向右），`rotate_ccw` 是 -90（逆时针 = 向左）。
    Action {
        id: rotateLeftAction

        text: "向左旋转"
        onTriggered: win.view.rotateCcw()
    }

    Action {
        id: rotateRightAction

        text: "向右旋转"
        onTriggered: win.view.rotateCw()
    }

    Action {
        id: flipHAction

        text: "水平翻转"
        onTriggered: win.view.flipH()
    }

    Action {
        id: flipVAction

        text: "垂直翻转"
        onTriggered: win.view.flipV()
    }

    Action {
        id: grayscaleAction

        text: "灰度"
        onTriggered: win.view.toggleGrayscale()
    }

    // §5.9.8 步骤 2 给的是「选择"颜色反转"」——那一句里既没有数字键也没有快捷键，所以这一枚
    // Action 只有菜单一只读者（加上量规那一枚）。它仍然提成 Action：六枚同一形状比四枚 Action
    // 加两枚 inline handler 少一处「为什么这两行不一样」。
    Action {
        id: invertAction

        text: "颜色反转"
        onTriggered: win.view.toggleInvert()
    }

    // §5.9.2 步骤 2「滚动鼠标滚轮，或使用 `+`、`-`」：滚轮那一只早就在场（`dragArea.onWheel`），
    // `+` / `-` 这一只到 P18x 之前都没有落点，所以这一节的那半句是空的。与上面那六枚同一形状
    // （声明一次，菜单那一行 `action:` 到它，键 `trigger()` 它），窗侧一算式也不写：梯子上下一
    // 级是什么、上下界在哪，全在 core（`ZOOM_STEPS` / `ZOOM_MIN` / `ZOOM_MAX`，`zoomStep` 那一手
    // 走的就是同一只作者）。
    //
    // 两枚键帽写成 `["+"]` 与 `["-", "="]`：`-` 在主键盘上，`=` 是不按 Shift 时同一枚键的字面
    // （美式布局上 `+` 要 Shift），键帽本身是 PRD 点名的那两枚，`=` 只是同一枚物理键的另一面。
    Action {
        id: zoomInAction

        text: "放大"
        onTriggered: win.view.zoomStep(true)
    }

    Action {
        id: zoomOutAction

        text: "缩小"
        onTriggered: win.view.zoomStep(false)
    }

    // 键帽是本档钉的默认，不是 PRD 钉的：第 1010 行只说「数字键」、第 1026 行只说「对应快捷键」，
    // 而第 7 行明写「快捷键使用常见默认值举例，实际以用户设置和运行平台为准」。这里钉
    // 1=向左旋转、2=向右旋转、3=灰度，即那三行在菜单上的次序；「§5.16.1 用户可在设置中修改
    // 快捷键」那一问目前只覆盖全局表 `hotkey.actions`，窗口内这几枚键还没进那张表，差距逐名记
    // 在台账的欠账里，不在这里冒充已做。
    //
    // 退让问的是上面那一格 `win.typing`：焦点落在窗上任何一扇吃键盘的框里时，用户打的「1」是
    // 名字或宽度的一个字符，不是旋转的手势——§5.12.1 的分组名框之外，§5.9.3 的尺寸三格与
    // §5.9.4 的透明度一格同样要人往里打数字。不做这层退让，重命名「第1组」或输入宽度 1280
    // 就各成一次手势，而窗内的键也不在各枚键上抄一份名单。
    Shortcut {
        id: rotateLeftKey

        sequences: ["1"]
        context: Qt.WindowShortcut
        enabled: !win.typing
        onActivated: rotateLeftAction.trigger()
    }

    Shortcut {
        id: rotateRightKey

        sequences: ["2"]
        context: Qt.WindowShortcut
        enabled: !win.typing
        onActivated: rotateRightAction.trigger()
    }

    Shortcut {
        id: grayscaleKey

        sequences: ["3"]
        context: Qt.WindowShortcut
        enabled: !win.typing
        onActivated: grayscaleAction.trigger()
    }

    // 上面那两枚 Action 的键那一半。§5.9.2 步骤 2 明写「滚动鼠标滚轮，或使用 `+`、`-`」，
    // 所以这两枚键是 PRD 给的手，不是这里自己长出来的。退让与数字键同一封（`win.typing`）：
    // 框里打的 `+` 是宽度的一个字符。裁剪模式下也让开（`!win.cropMode`）：那时这扇窗
    // 属于裁剪层，键盘上那几枚该说的是「确认 / 退出」，而不是在用户没看的时候把底下的画换掉。
    Shortcut {
        id: zoomInKey

        sequences: ["+"]
        context: Qt.WindowShortcut
        enabled: !win.typing && !win.cropMode
        onActivated: zoomInAction.trigger()
    }

    Shortcut {
        id: zoomOutKey

        sequences: ["-", "="]
        context: Qt.WindowShortcut
        enabled: !win.typing && !win.cropMode
        onActivated: zoomOutAction.trigger()
    }

    // §5.9.9 步骤 3「按 `Enter` 确认」。`Return` 与 `Enter` 两枚都接：前者是主键盘那枚，
    // 后者是小键盘那枚，`sequences:` 一次把两枚都接上，与上面 `StandardKey.Copy` 那封信同一
    // 理由（一枚键序在 Qt 里可以对应多枚 native 绑定，只接第一枚就是漏一半）。
    Shortcut {
        id: cropEnterKey

        sequences: ["Return", "Enter"]
        context: Qt.WindowShortcut
        enabled: win.cropMode && !win.typing
        onActivated: win.commitCrop()
    }

    // 裁剪模式的退路。PRD 那一句只点了 Enter，退出这一手是这一层自己欠的：一层压暗吃下了
    // 鼠标，用户拖完想反悔就得有一枚键说话。键帽写死 `"Esc"`，不抬进 Qt 那枚标准取消键序：
    // 那一套标准键序在场上已经有主（上面 `clearSelectionAction` 那一支，`esc == 1` 那枚
    // 尺判的就是它只写一遍），两枚 Shortcut 各叫一次同一套标准键序就是一件事两种说法。
    // 两支的 `enabled` 互斥（上面那一支读 `!win.cropMode`，这一支读 `win.cropMode`），所以同一刻
    // 场上永远只有一支在听 `Esc`：非裁剪时清名单，裁剪时退出这一层。两支都再让一让
    // `win.typing`：框开着的时候 `Esc` 说的是「把这扇框收掉」，Qt 自己会那么办，窗不必趁势
    // 把名单清了或把裁剪层退出。
    Shortcut {
        id: cropEscapeKey

        sequences: ["Esc"]
        context: Qt.WindowShortcut
        enabled: win.cropMode && !win.typing
        onActivated: win.cropMode = false
    }

    Menu {
        id: pinMenu

        // §5.10.4 的影响数量在这一刻问一次桥侧（见上面 `destroyReach` 那一封信）。菜单每开一次
        // 问一次，所以名单在两趟开菜单之间变了也不会印错 —— 这一句不是装饰：量规那一腿就是
        // 先 `pinMenu.open()` 再读 `des-reach-*` 的数，读的正是这一行写进去的那一格。
        //
        // §5.11 的两格显示缓存走的是同一时刻、同一理由：独显开着与关闭着之间隔着的可能就是
        // 一趟开菜单，而「这一手带走几枚」在名单变过之后就不再是上一次那个数了。
        onAboutToShow: {
            win.destroyReach = win.session.destroyCountFor(win.pinId)
            win.soloReach = win.session.soloCountFor(win.pinId)
            win.soloWay = win.session.soloDims() ? "压暗" : "隐藏"
        }

        MenuItem { action: copyAction }
        // `id` for the `--pin` gauge, which reports the text it is shown rather than
        // assuming the menu still says what §5.5.5 calls it.
        MenuItem { id: saveAsItem; text: "另存为…"; onTriggered: saveAs.open() }

        // §5.6.1 步骤 2 在贴图这一臂。挨着「另存为」而不是排在旋转那一族之后：这一手取的是
        // **这张图的内容**（读出来的是字），与「把图交出去」同族，而旋转/缩放那一族改的是窗怎么画。
        //
        // `asked` 是**递进去**而不是绑出来的（理由与 `AnnotationToolbar.qml` 那封信同一封：绑成一串
        // 就会和面板自己那颗「收起」互相踩掉），而 `ocrWindow.visible` 绑在它上面 —— 读的那一头
        // 是绑定，写的那一头不是，两侧才各走各的。
        MenuItem {
            text: "提取文字"
            onTriggered: {
                ocrPanel.asked = true
                win.session.extractPinText(win.pinId)
            }
        }

        // §5.6.2 步骤 1 与 §5.6.3 步骤 1 在贴图这一臂：PRD 那两节各写「点击识别二维码」/
        // 「点击条形码识别」，这里一行都担了 —— 底下是同一只读码的手，一行分成两行只会让用户
        // 先猜图上是哪种码。这一格有宽度（菜单项按最长那行拉），所以写全「扫码（二维码 / 条形码）」
        // 而不像工具条那颗只留两个字。
        MenuItem {
            text: "扫码（二维码 / 条形码）"
            onTriggered: {
                codesPanel.asked = true
                win.session.readPinCodes(win.pinId)
            }
        }

        MenuSeparator {}

        MenuItem { action: zoomInAction }
        MenuItem { action: zoomOutAction }
        MenuItem { text: "重置缩放"; onTriggered: win.view.resetZoom() }

        // §5.9.3 步骤 1/2「用户打开贴图菜单 → 选择"尺寸与缩放"」那一格。字面取 PRD 那一行的
        // 词，`…` 说的是「按下之后还有一扇框要打字」，与本页「另存为…」/「新建分组…」同一
        // 形状。判据一个也不在这里：三格文本原样交给 `applySizeText`，等比锁定、上下界、非法
        // 输入三判全在 core（`size_zoom_request`），拒语也是它那句。
        MenuItem { text: "尺寸与缩放…"; onTriggered: sizeDialog.open() }

        // §5.9.4 步骤 2 的第二扇门「或在菜单中输入透明度」（第一扇是 Ctrl+滚轮，早就在场）。
        // 这一格只**判**那一格的文本（`opacityFromText`），按下之后交回的百分比交给
        // `opacitySetTarget` 那只唯一的落点 —— 透明度是 §5.10.3 的批量一手，窗不数名单。
        MenuItem { text: "透明度…"; onTriggered: opacityDialog.open() }

        // §5.8.5 步骤 4「用户可调整卡片宽度、字号、字体和背景颜色」与 §5.8.6 步骤 4–5 那一扇
        // 就地调整的窗。`visible` 读的是 `view.is_card` —— 判断本体是 core 那一格
        // `PinItem::card` 有没有，桥只搬（#281），于是「这一行看得见、按下只会说这不是卡片」
        // 那一族门在这一层就组不出来（与上面「刷新截图」那两行同一形状，#494）。
        MenuItem {
            text: "卡片设置…"
            visible: win.view.is_card
            onTriggered: cardDialog.open()
        }

        MenuSeparator {}

        MenuItem { action: rotateLeftAction }
        MenuItem { action: rotateRightAction }
        MenuItem { action: flipHAction }
        MenuItem { action: flipVAction }

        MenuSeparator {}

        // 灰度与颜色反转是**图**的开关而不是**窗**的开关，所以这两行不画勾选：§5.9.7 与 §5.9.8
        // 要的是「再次执行恢复」，而菜单上那一格勾是「现在是不是灰的」的另一套说法，PRD 两句都
        // 没要求它。显示状态本身如今是窗上读得到的一格（`view.grayscale` / `view.inverted`，
        // P145 起），所以量规那六枚 `xf-*` 行读的是本体而不是字面 —— 这句话以前写的是「不是
        // 窗的一格所以只能按行动作读」，那一半自 P145 起已经不成立，订正留在台账。
        MenuItem { action: grayscaleAction }
        MenuItem { action: invertAction }
        MenuItem { text: "平滑缩放"; checkable: true; checked: win.view.smooth
                   onTriggered: win.view.applySmooth(checked) }

        MenuSeparator {}

        MenuItem { text: "固定缩略图"; onTriggered: win.view.toggleThumbnail() }
        MenuItem { text: "退出缩略图"; onTriggered: win.view.exitThumbnail() }
        // §5.9.9 步骤 1「激活贴图并进入裁剪模式」的那一枚入口。一条字面两只手（进与退是同一
        // 枚），与上面「独显」/「显示全部贴图」同一形状：字面读的就是 `win.cropMode` 那一格，
        // 窗上压暗层跟着它，所以「菜单说什么」与「窗上是什么」永远是同一刻的同一格。
        // 进去之后鼠标属于裁剪层（`cropLayer` 在 `dragArea` 之后声明），确认是 `Enter`、
        // 退出是 `Esc` 或再按这一行；块内那四行算式只搬框，裁剪判据在 core。
        MenuItem {
            text: win.cropMode ? "退出裁剪" : "裁剪…"
            onTriggered: {
                if (win.cropMode)
                    win.cropMode = false
                else
                    win.enterCrop()
            }
        }
        MenuItem { text: "撤销裁剪"; onTriggered: win.view.undoCrop() }
        MenuItem { text: "恢复原图"; onTriggered: win.view.restoreOriginal() }

        MenuSeparator { visible: win.view.refreshable }

        // §5.5.8 刷新截图。规则 1 写的是「用户**可选择**覆盖原图或生成新图」，所以这里两行而不
        // 是一行加一枚开关：按下哪一行的字就是那一枚选择，读完不需要再回头问「现在是哪一种」。
        //
        // 可见性读的是 `view.refreshable` —— 这一枚贴图记不记得自己的**原截图坐标**。从文件、
        // 从剪贴板、从白板来的那些枚压根没有这块坐标，给它们留一行按下只会回答「不能刷」的菜
        // 单就是 #494 那一族；而它与按下之后那一格回答的是同一件事（`PinState::refreshable` 与
        // `falcon_core::refresh::plan` 的第一臂同一格），所以「看得见按不动」在这一层组不出来。
        //
        // 区域失效**不在**这一格里预判：那块地方还在不在整个屏幕上，只有真要读之前问得出来，
        // 而它的答案是规则 2 那句「提示重新选择」—— 写在 `falcon_core::refresh::plan`，窗不替
        // 它想一句。按下之后 `win.view.reload()` 那一手是覆盖那一路唯一的画上线的机会：新像素
        // 走的是 `image_key` 那一格，而它只有 reload 会重读。
        MenuItem { text: "刷新（覆盖这张）"
                   visible: win.view.refreshable
                   onTriggered: { win.session.refreshPin(win.pinId, true); win.view.reload() } }
        MenuItem { text: "刷新（另钉一张）"
                   visible: win.view.refreshable
                   onTriggered: { win.session.refreshPin(win.pinId, false); win.view.reload() } }

        MenuSeparator { visible: win.view.has_transparent_pixels }

        // §5.9.15 背景模式, the four rows in the order the requirement lists them. The
        // section is on the menu only for a picture that has a hole in it: 步骤 1 opens it
        // from the 贴图菜单 of a 透明图片, and four rows that change nothing on an opaque
        // pin are a section the user has to read to learn it says nothing.
        //
        // `has_transparent_pixels` is the state machine's answer about the picture it
        // renders, not a file-type guess here, and it is read once per content change
        // rather than once per reload.
        //
        // The four are `checked` rather than plain actions because they are one choice, not
        // four effects: a menu that marks nothing cannot be read for which mode the pin is
        // in. `checked` comes back from `alpha_bg`, so a row the pin refused to take leaves
        // the mark where the pin actually is.
        MenuItem { id: alphaTransparentItem
                   text: "背景：透明（洞不吃点击）"
                   visible: win.view.has_transparent_pixels
                   checkable: true; checked: win.view.alpha_bg === 0
                   onTriggered: win.view.setAlphaBg(0) }
        MenuItem { id: alphaPseudoItem
                   text: "背景：伪透明（整窗可点）"
                   visible: win.view.has_transparent_pixels
                   checkable: true; checked: win.view.alpha_bg === 1
                   onTriggered: win.view.setAlphaBg(1) }
        MenuItem { id: alphaDarkItem
                   text: "背景：深色棋盘格"
                   visible: win.view.has_transparent_pixels
                   checkable: true; checked: win.view.alpha_bg === 2
                   onTriggered: win.view.setAlphaBg(2) }
        MenuItem { id: alphaLightItem
                   text: "背景：浅色棋盘格"
                   visible: win.view.has_transparent_pixels
                   checkable: true; checked: win.view.alpha_bg === 3
                   onTriggered: win.view.setAlphaBg(3) }

        MenuSeparator {}

        // §5.9.14 播放控制, the whole of it: 暂停/继续, 上一帧/下一帧, 回到第一帧,
        // and the speed. The section is only on the menu when the pin has frames
        // to run through - `gif_frames` is 0 for every still, and 1 for a GIF the
        // file made only one picture of. 逐帧查看 pauses the clip: that is the
        // state machine's decision (`PlayState::step`), and the timer follows it
        // because `gif_playing` is what it binds `running` to.
        // The seven carry an `id` for one reason: `gifMenuItemCount` above counts *these
        // objects*, so the count is the menu's own visibility read back rather than the
        // condition restated. The condition itself stays written once per item, as it was - a
        // single shared `visible` binding would have been the easier edit and the weaker
        // witness.
        MenuItem { id: gifPauseItem
                   text: win.view.gif_playing ? "暂停播放" : "继续播放"
                   visible: win.view.gif_frames > 1
                   onTriggered: win.view.gifToggle() }
        MenuItem { id: gifPrevItem; text: "上一帧"; visible: win.view.gif_frames > 1
                   onTriggered: win.view.gifStep(-1) }
        MenuItem { id: gifNextItem; text: "下一帧"; visible: win.view.gif_frames > 1
                   onTriggered: win.view.gifStep(1) }
        MenuItem { id: gifFirstItem; text: "回到第一帧"; visible: win.view.gif_frames > 1
                   onTriggered: win.view.gifFirst() }
        MenuItem { id: gifSlowItem; text: "放慢一档（现在 " + win.view.gif_speed + "%）"
                   visible: win.view.gif_frames > 1
                   onTriggered: win.view.gifSpeedStep(false) }
        MenuItem { id: gifFastItem; text: "加快一档"; visible: win.view.gif_frames > 1
                   onTriggered: win.view.gifSpeedStep(true) }
        MenuItem { id: gifOriginalItem; text: "按文件原速播放"; visible: win.view.gif_frames > 1
                   onTriggered: win.view.gifApplySpeed(100) }

        MenuSeparator {}

        // §5.10.3：这三枚从 P133 起叫的是同一列里的两只手（`opacityStepTarget` /
        // `opacitySetTarget`）。名单里有第二枚时它们改的是**那一批**，不是本窗 —— 而"这一手算
        // 几枚"本窗一次也没问（窗的章程；判据 `state::batch_hand` 全仓只写那一次）。
        MenuItem { text: "更透明"; onTriggered: win.opacityStepTarget(win.pinId, false) }
        MenuItem { text: "更实"; onTriggered: win.opacityStepTarget(win.pinId, true) }
        // §5.9.4 asks for a one-key way back to fully opaque, which the two steps
        // above are not: from the floor they need ten presses.
        MenuItem { text: "不透明度 100%"; onTriggered: win.opacitySetTarget(win.pinId, 100) }
        MenuItem { text: "始终置顶"; checkable: true; checked: win.view.topmost
                   onTriggered: win.view.applyTopmost(checked) }
        MenuItem { text: "鼠标穿透"; checkable: true; checked: win.view.click_through
                   onTriggered: win.view.applyClickThrough(checked) }

        MenuSeparator {}

        // §5.10.4 的三只手在这一段落成三枚入口。P134 之前这里的「重置视图」叫的是
        // `win.view.resetView()` —— 窗自己那一格手势，所以名单里有第二枚时它也只改本窗；
        // 现在叫的是 `win.resetTarget`，带走几枚由名单定（判据在 `state::batch_hand`，窗不数）。
        MenuItem { text: "重置视图"; onTriggered: win.resetTarget(win.pinId) }
        // §5.11 独显：一条字面两只手，进与退是**同一枚入口**（PRD 步骤 4「用户再次执行 Solo 或
        // 退出命令」说的就是这一枚）。两格字面都读桥侧的答案，窗一个也不数：
        // `win.session.solo_kept` 答「在不在这 mode」（0＝不在，而它不可能是「开着却留 0 枚」，
        // core 对空名单直接拒绝），`win.soloReach` / `win.soloWay` 答「这一手带走几枚、怎么带走」。
        // 退出那一支不带数目，理由写在上面 `soloReach` 那一封信里。
        MenuItem { id: soloItem
                   text: win.session.solo_kept > 0
                         ? "退出独显"
                         : "独显（" + win.soloWay + " " + win.soloReach + " 枚）"
                   onTriggered: win.soloTarget(win.pinId) }
        // §5.15「隐藏 / 显示全部贴图」：一条字面两只手，与独显同一形状（进与退是同一枚入口）。
        // 字面读的是桥侧那一格 `session.all_hidden`（1 已收起 / 0 没有），窗一个也不数也不判 ——
        // 它叫的是 `toggleAllPins`，翻完由 `pins_epoch` 让全场每扇窗重读 `view.live`。全局收起
        // 与对话框遮挡、整组隐藏各占一格理由（#281），所以这一手放开时那两批不会跟着回来。
        // 注：全部收起后场上没有可见窗可点开这枚菜单，再按一次 `ctrl+shift+p` 热键是回程 —— 这枚
        // 入口负责的是「看得见时收起」，热键负责「看不见时放开」，两半合起来才是 §5.15 的往返。
        MenuItem { id: allPinsItem
                   text: win.session.all_hidden > 0 ? "显示全部贴图" : "隐藏全部贴图"
                   onTriggered: win.session.toggleAllPins() }
        // 这一句字面没动，动的是它背后那只手：`closePin` 自 P134 起走 `state::close_pin`，
        // 名单里有第二枚且本窗在名单里时，关的是那一批而不是本窗一枚。
        MenuItem { text: "关闭"; onTriggered: win.session.closePin(win.pinId) }
        // §5.9.18 / PRD 第 1149 行「批量销毁时应提示影响数量」：字面上印着的数由桥侧那一格答
        // （`destroyCountFor` → `state::destroy_answer` → 同一张 `hand_targets`），所以它与
        // 按下之后真正带走的那些枚不可能各走各的。第 1148 行「应显示确认或支持撤销」走的是
        // 下面那一枚撤销：销毁在状态机里有回头路（`PinSet::restore`），于是这一手不需要一扇
        // 确认对话框替用户兜住一只本可以撤销的手。
        MenuItem { id: destroyItem
                   text: "销毁（影响 " + win.destroyReach + " 枚）"
                   onTriggered: win.destroyTarget(win.pinId) }
        // 撤销的是**最近一手**带走的那一批，而「有几枚还欠着」由桥侧那一格账答
        // （`Session.trashed` = `state::trashed`）；窗自己一个也不数。一枚也没有时这一条不出现，
        // 而不是出现了再答一句「没有可撤销的」—— 那一枚死行是装饰，#433 那一族。
        MenuItem { id: undoItem
                   text: "撤销刚才销毁的 " + win.session.trashed + " 枚"
                   visible: win.session.trashed > 0
                   onTriggered: win.undoDestroyTarget() }

        MenuSeparator {}

        // §5.12 图片分组管理：这一枚子菜单是 §5.12.1 新建、§5.12.2 移动到分组、§5.12.3
        // 删除/重命名/排序、§5.12.4 显示/隐藏 四节在窗上的落点。每一行叫的都是桥侧那一只手，
        // 窗自己一不数名、二不判重名、三不决定「贴图如何处理」—— 那些都是 core 已答过的事
        // （#281）。`model` 绑的是 `win.groupList` 那一枚绑定值，`session.groups` 一到 notify
        // 就重建这些行，所以「排序变化同步反映到快捷切换菜单」在这里只是重新读同一张表。
        Menu {
            id: groupMenu
            title: "分组"

            // §5.12.2 移动到分组：选中若干枚（名单由 core 那一份持有）→ 点其中一组 → 归属变。
            // `（未分组）`那一行叫的是 `moveSelectionToGroup(0)`，0 在桥侧翻成 `None`。
            Menu {
                id: moveToMenu
                title: "移动到分组"
                Repeater {
                    model: win.groupList
                    MenuItem {
                        text: modelData.name
                        onTriggered: win.session.moveSelectionToGroup(modelData.id)
                    }
                }
                MenuItem { text: "（未分组）"; onTriggered: win.session.moveSelectionToGroup(0) }
            }

            // §5.12.4 显示 / 隐藏整组：一行一组，字面把它此刻显不显说出来，点一下切换。这一手
            // 真的改组内每一枚的可见性 ⇒ 桥侧发 `group_epoch`，`main.qml` 那一圈 handler 让全场
            // 重读 `view.live`（被藏起来的那些枚正是这一手的目标，所以扇出走全体）。
            Menu {
                id: shownMenu
                title: "整组显隐"
                // 「有组才出现」问的是这一枚子菜单现在许不许点，不是它要不要**自己弹出来**：
                // `Menu.visible` 是弹窗状态，绑它等于在贴图窗刚构造完那一刻命令一扇子菜单 open 自己。
                // P172 的门先种三组、后造窗，那一次 `pinWindowComponent.createObject()` 当场
                // ACCESS_VIOLATION（`WINDOW_RC=-1073741819`），而 `--pin` 在同一拍造四扇窗、零组，
                // 一次也没坏 —— 坏的是这一格，不是「有几扇窗」。`enabled` 讲的才是「这一行不许点」。
                enabled: win.groupList.length > 0
                Repeater {
                    model: win.groupList
                    MenuItem {
                        text: modelData.name + "（" + (modelData.shown ? "显示中" : "已隐藏") + "）"
                        onTriggered: win.session.toggleGroupShown(modelData.id)
                    }
                }
            }

            MenuSeparator {}

            Menu {
                id: manageMenu
                title: "管理"

                // §5.12.1 新建分组：开一格栅窗问名字，提交走 `createGroup`。空名/重名由 core 判，
                // 失败那句话由 `session.status` 说回来（窗不自己判命名规则）。
                MenuItem {
                    text: "新建分组…"
                    onTriggered: {
                        win.groupDialogEditId = -1
                        win.groupDialogSeed = ""
                        groupPrompt.open()
                    }
                }
                // §5.12.3 重命名：逐组一行，复用同一格栅窗（`editId` 非 -1 就走 `renameGroup`）。
                Repeater {
                    model: win.groupList
                    MenuItem {
                        text: "重命名 " + modelData.name
                        onTriggered: {
                            win.groupDialogEditId = modelData.id
                            win.groupDialogSeed = modelData.name
                            groupPrompt.open()
                        }
                    }
                }
                MenuSeparator {}
                // §5.12.3 排序：逐组「上移 / 下移」各一行（`delta` ∓1，core 饱和夹边）。拖动排序
                // 在窗菜单这一路用「上移/下移」落地；PRD 那句「拖动」的语义就是换相邻两格。
                Repeater {
                    model: win.groupList
                    MenuItem {
                        text: "上移 " + modelData.name
                        onTriggered: win.session.moveGroup(modelData.id, -1)
                    }
                }
                Repeater {
                    model: win.groupList
                    MenuItem {
                        text: "下移 " + modelData.name
                        onTriggered: win.session.moveGroup(modelData.id, 1)
                    }
                }
                MenuSeparator {}
                // §5.12.3 删除 +「删除非空分组应询问贴图如何处理」：这一问不在弹窗里、而在菜单
                // 落成的**两行显式选择**上 —— 「只删组（退回未分组）」交 `destroy=0`，
                // 「连贴图一起销毁」交 `destroy=1`。用户点哪一行就是他的答复；空组时两行结果相同
                // （core 那格交回 0 枚），但字面仍把枚数印出来（`count` 来自 `group_rows`）。
                Repeater {
                    model: win.groupList
                    MenuItem {
                        text: "删除组 " + modelData.name + "（" + modelData.count + " 枚退回未分组）"
                        onTriggered: win.session.deleteGroup(modelData.id, 0)
                    }
                }
                Repeater {
                    model: win.groupList
                    MenuItem {
                        text: "删除组 " + modelData.name + " 并销毁 " + modelData.count + " 枚"
                        visible: modelData.count > 0
                        onTriggered: win.session.deleteGroup(modelData.id, 1)
                    }
                }
            }
        }
    }

    // §5.12.1 / §5.12.3 新建与重命名共用的那一格栅窗。一枚 `Dialog` 两只手：`win.groupDialogEditId`
    // 是 -1 就 `createGroup`、否则 `renameGroup(id, text)`。命名规则（trim、非空、同级不重名）
    // 一律在 core，这里只把用户打的串递过去，失败读 `session.status` 那句话。
    Dialog {
        id: groupPrompt
        title: win.groupDialogEditId < 0 ? "新建分组" : "重命名分组"
        standardButtons: Dialog.Ok | Dialog.Cancel
        modal: true
        anchors.centerIn: parent
        onOpened: {
            groupField.text = win.groupDialogSeed
            groupField.forceActiveFocus()
        }
        onAccepted: {
            if (win.groupDialogEditId < 0)
                win.session.createGroup(groupField.text)
            else
                win.session.renameGroup(win.groupDialogEditId, groupField.text)
        }
        contentItem: Column {
            spacing: 8
            Label { text: "分组名称：" }
            TextField {
                id: groupField
                width: 240
                placeholderText: "输入分组名称"
                onAccepted: groupPrompt.accept()
            }
        }
    }

    // §5.9.3 那一扇「尺寸与缩放」。形状照上面 `groupPrompt`：一枚 `Dialog`、Ok/Cancel 两枚
    // 标准键、`contentItem` 一列 `TextField` —— 页上不多发明一种控件，也不写一个字段的判据。
    //
    // 三格的初值：宽、高填的是**这一刻窗上的物理尺寸**（`view.size_w` / `view.size_h`，桥从
    // 状态机搬出来的那一格），百分比留空。为什么填窗上的数就是「输入窗口尺寸」那一式：core
    // 把宽高读在 `shown_size`（图自己的那块）上算回缩放，于是打进去的那个数正好是窗要变成的
    // 那块物理尺寸，「原样按 Ok」算回同一枚缩放、一处也不动。上下界（§5.9.2 的
    // `ZOOM_MIN`..=`ZOOM_MAX`）页上不抄第二份：越界那句拒语把界自己报出来（`size_zoom_request`）。
    Dialog {
        id: sizeDialog

        title: "尺寸与缩放"
        standardButtons: Dialog.Ok | Dialog.Cancel
        modal: true
        anchors.centerIn: parent
        onOpened: {
            sizeWField.text = String(win.view.size_w)
            sizeHField.text = String(win.view.size_h)
            sizePctField.text = ""
            win.view.dialog_message = ""
            sizeWField.forceActiveFocus()
        }
        // 被拒那一臂把框请回来：Qt 的 `Dialog` 在 `accepted` 之前先把窗收了，而那句拒语要
        // 让用户看得见、改得了。回滚不需要 —— core 被拒时窗一处也不动（`pin.rs` 的
        // `the_size_dialog_refuses_what_was_typed_and_changes_nothing` 判的就是这一臂）。
        // 接受那一臂交回的是**落在窗上的**那枚缩放（core 的答案本体），页上不读它、也不算它。
        onAccepted: {
            if (win.view.applySizeText(sizeWField.text, sizeHField.text,
                                       sizePctField.text) < 0)
                sizeDialog.open()
        }
        contentItem: Column {
            spacing: 8

            Label { text: "宽度、高度（像素）或缩放百分比 —— 只填一种：" }

            Row {
                spacing: 8

                TextField {
                    id: sizeWField

                    width: 88
                    placeholderText: "宽度"
                }
                TextField {
                    id: sizeHField

                    width: 88
                    placeholderText: "高度"
                }
                TextField {
                    id: sizePctField

                    width: 88
                    placeholderText: "百分比"
                    onAccepted: sizeDialog.accept()
                }
            }

            Label { text: "锁定比例：只填宽，高跟着比例走；只填高，宽跟着走；两格都填是一块容器。" }

            // 那一格拒语的唯一读者。串的本体是 core 的 `PinError`（`#[error(…)]` 那六臂中文），
            // 桥只搬运、页只画；绑的是属性本体，不绑 `dialog_messageChanged` 那种驼峰信号名
            // （cxx-qt 不驼峰化 qproperty，名字写错是**静默**的，#316 那一族）。
            Label {
                text: win.view.dialog_message
                visible: text !== ""
                width: 300
                wrapMode: Text.Wrap
                color: "#ffc25b5b"
            }
        }
    }

    // §5.9.4 步骤 2「或在菜单中输入透明度」那一格。同一形状、同一「被拒就把框请回来」的招，
    // 区别只在按下之后那一手交出去的是**判出来的百分比**而不是文本：透明度是 §5.10.3 的批量
    // 一手，「这一手改几枚」由 `state::batch_hand` 判一次，页上把数交给既有的那只落点
    // `opacitySetTarget`（与菜单那三行、Ctrl+滚轮同一只手，窗不数名单）。
    //
    // 界不抄第二份：`typed_opacity` 那一头判的是 `OPACITY_MIN`..=100，拒语里带着这两个数。
    // 0 那一档在这一扇门上到不了 —— 规则第一句「透明度过低时仍应保留找回窗口的方式」要的
    // 就是这一扇门自己走得回来，理由写在 `pin.rs` 那封信里。
    Dialog {
        id: opacityDialog

        title: "透明度"
        standardButtons: Dialog.Ok | Dialog.Cancel
        modal: true
        anchors.centerIn: parent
        onOpened: {
            opacityField.text = String(win.view.opacity)
            win.view.dialog_message = ""
            opacityField.forceActiveFocus()
        }
        onAccepted: {
            const pct = win.view.opacityFromText(opacityField.text)
            if (pct < 0)
                opacityDialog.open()
            else
                win.opacitySetTarget(win.pinId, pct)
        }
        contentItem: Column {
            spacing: 8

            Label { text: "不透明度百分比（100 = 完全不透明）：" }

            TextField {
                id: opacityField

                width: 120
                placeholderText: "百分比"
                onAccepted: opacityDialog.accept()
            }

            Label {
                text: win.view.dialog_message
                visible: text !== ""
                width: 300
                wrapMode: Text.Wrap
                color: "#ffc25b5b"
            }
        }
    }

    // §5.8.5 步骤 4「调整卡片宽度、字号、字体和背景颜色」与 §5.8.6 步骤 4–5 那一扇就地调整的窗。
    // 形状照上面 `sizeDialog`：一枚 `Dialog`、Ok/Cancel 两只标准键、一列 `TextField`、被拒那一臂
    // 把框请回来 —— 页上不发明第二种控件，也不写一个字段的判据（四判全在 core 与 `state.rs`）。
    //
    // 四格的初值是 `readCard` 现读的**这一枚卡片此刻**用的那四枚数（`[card]` 打底、这一枚被调过
    // 的那几格盖上去），不是 `[card]` 的那四枚：打开 A 卡片的窗该看见 A 现在真的是多宽，页上显示
    // 480 而 A 其实是 700 时按 Ok 就是把 A 改回 480 —— 那一手没人要求过。
    //
    // 空着的那一格 = 这一枚在这一格上仍跟着 `[card]`；那一判由 `card_edit` 一名作者判，页上既不
    // 替用户猜一枚颜色、也不把 `[card]` 那一枚数当成「用户刚填的」打回去（#281）。
    Dialog {
        id: cardDialog

        title: "卡片设置"
        standardButtons: Dialog.Ok | Dialog.Cancel
        modal: true
        anchors.centerIn: parent
        onOpened: {
            win.view.readCard()
            cardWField.text = String(win.view.card_width)
            cardFontField.text = String(win.view.card_font_size)
            cardFamilyField.text = win.view.card_family
            cardColourField.text = win.view.card_background
            win.view.dialog_message = ""
            cardWField.forceActiveFocus()
        }
        // 被拒那一臂把框请回来（与 `sizeDialog` 同一招）：core 在被拒时不提交身份，屏幕上留下的
        // 还是旧的那一张，所以这里没有回滚要做，只有那一格拒语要让人看得见、改得了。
        onAccepted: {
            if (!win.view.applyCardEdit(cardWField.text, cardFontField.text,
                                        cardFamilyField.text, cardColourField.text))
                cardDialog.open()
        }
        contentItem: Column {
            spacing: 8

            // 同一格在两张卡上叫不同名字：§5.8.6 那一扇要的是「颜色值」（改它就是步骤 5 那「两半
            // 一起换」），§5.8.5 那一扇要的是「背景颜色」。题头跟着 `card_colour` 走，页上不猜。
            Label {
                text: win.view.card_colour
                        ? "颜色值：改它会把色块与那一行一起换。留空 = 这一格没填。"
                        : "宽度、字号、字体、背景颜色 —— 留空的那一格仍跟着 [card]。"
                width: 320
                wrapMode: Text.Wrap
            }

            Row {
                spacing: 8

                TextField {
                    id: cardWField

                    width: 88
                    placeholderText: "宽度"
                }
                TextField {
                    id: cardFontField

                    width: 88
                    placeholderText: "字号"
                }
                TextField {
                    id: cardFamilyField

                    width: 140
                    placeholderText: "字体"
                }
                TextField {
                    id: cardColourField

                    width: 120
                    placeholderText: win.view.card_colour ? "颜色值" : "背景颜色"
                    onAccepted: cardDialog.accept()
                }
            }

            // 那一格拒语的唯一读者，与上面两扇输入框同一枚绑法：绑属性本体，不绑驼峰信号名。
            Label {
                text: win.view.dialog_message
                visible: text !== ""
                width: 300
                wrapMode: Text.Wrap
                color: "#ffc25b5b"
            }
        }
    }

    // §5.6.1 步骤 4 在贴图这一臂的那一扇窗。
    //
    // 为什么是**一扇窗**而不是贴在 `win` 里的一块面板：这一扇窗的尺寸就是那张图此刻的大小，
    // 一枚 380 宽的面板贴在里头会被窗本身裁掉 —— 小贴图（缩略图那一档）压根装不下它。窗是
    // 按需造的（`Instantiator` 带走这扇窗，那一扇跟着没），产品会话里没人点「提取文字」时
    // 场上就多不出这一扇，`--pin` 那一档数窗的账因此一动不动。
    //
    // `flags: Qt.Tool` 是不进任务栏、也不进 alt-tab 那一列：这扇窗只有关于**别人**那一张图，
    // 自己不是用户正在做的活儿。`transientParent: win` 让它跟着自己的贴图走（贴图关，它也关）——
    // 写 `transient: win` 是**语法错**而不是类型错：本机 qmllint 6.10.1 实测那一句回
    // `Expected token '}'`（两份只差在这一行的探针文件，
    // `.qoder/scratch/p146_lint_ta.txt` 与 `p146_lint_tb.txt`），`qmlcachegen` 同一句、构建 rc=101。
    // 位置贴在贴图右侧 8 px：不做屏幕夹取，因为 `Screen` 那一枚 attached 属性要先靠 x/y 才知道
    // 自己在哪一屏，而 x/y 又得由它来夹 —— 上面 `dpr` 那一封信写的就是这个环，本轮不重踩。
    Window {
        id: ocrWindow

        flags: Qt.Tool
        transientParent: win
        title: "识别结果"
        color: "transparent"

        width: ocrPanel.width + 8
        height: ocrPanel.height + 8
        x: win.x + win.width + 8
        y: win.y
        // 只跟着 `asked` 那一格：它是面板自己的「这一扇贴图的活儿还没收」，写它的人是菜单那一行
        // 与面板上那颗「收起」，读它的只有这一行。
        visible: ocrPanel.asked

        OcrPanel {
            id: ocrPanel

            session: win.session
            x: 4
            y: 4
            // 这一臂的来源是**这一枚贴图**：id 由窗递进来，窗换了贴图（`assign`）下按重新识别
            // 取的就是新那一张 —— 面板不记自己上一次是从哪一枚里读出来的。
            onReRecognized: win.session.extractPinText(win.pinId)
        }
    }

    // §5.6.2 步骤 4 与 §5.6.3 步骤 3 在贴图这一臂的那一扇窗。形状照上面 `ocrWindow` 那一封：
    // 同一族理由不重抄，只记这一处真正不同的地方 —— **它开在贴图的左边**。
    //
    // `ocrWindow` 钉在右边（`win.x + win.width + 8`），这一扇钉在左边，于是两扇窗**各占一侧**：
    // 同一枚贴图既能提取文字又能扫码，而两扇都 `Qt.Tool`、都没有任务栏入口，若叠在一起，后面那一扇
    // 就再也按不到。让它们互相让开要么得绑对方的 `height`（一扇窗的内容变了会挪走另一扇），要么
    // 得有一格「谁先开」的账 —— 两件都不是这一节要的东西，而左右分边一行就够。
    //
    // 依旧不做屏幕夹取，理由与 `ocrWindow` 那一封的最后一句同一封（`Screen` 要先靠 x/y 才认屏，
    // x/y 又要靠它夹）。贴图贴着屏幕左缘时这一扇会出界，本轮承这一笔。
    Window {
        id: codesWindow

        flags: Qt.Tool
        transientParent: win
        title: "扫码结果"
        color: "transparent"

        width: codesPanel.width + 8
        height: codesPanel.height + 8
        x: win.x - width - 8
        y: win.y
        visible: codesPanel.asked

        CodesPanel {
            id: codesPanel

            session: win.session
            x: 4
            y: 4
            // 来源是**这一枚贴图**：与 `ocrPanel` 那一句同一封，面板不记自己上一次是从哪一枚读的。
            onReRecognized: win.session.readPinCodes(win.pinId)
        }
    }

    // §5.9.17. The dialog decides the name and the extension; `savePin` reads the
    // extension to pick the encoder, so the filter order below is also the list of
    // formats this app writes from a pin - and since P40 it is a second time over, because
    // §5.5.5 step 3 counts 格式 among the three things chosen and the arm therefore decides
    // the format of a name typed *without* one. Both halves travel from this one list.
    //
    // The name is handed over as the URL text rather than a local path, because this
    // Qt gives JS no way to convert one: `selectedFile.toLocalFile()` measured
    // `undefined` - a TypeError on every accept, P37 - and `save::local_path` is now
    // the single place that reads a `file:///…` back into the path it names. The arm is
    // handed over flattened the same way `readFilter` reports it, because
    // `selectedNameFilter` is an object and `String()` of one is not the text the owner
    // can read.
    FileDialog {
        id: saveAs
        title: "另存为"
        fileMode: FileDialog.SaveFile
        nameFilters: ["PNG 图片 (*.png)", "JPG 图片 (*.jpg)", "BMP 图片 (*.bmp)"]
        // §5.5.11 步骤 3/4 的事件源。绑 `visible` 而不是 `accepted`/`rejected`，是因为
        // "这张对话框不在了" 就是那一步的全部条件：保存、取消、还是用户直接关掉窗，三条腿
        // 之后贴图都该回来，而 `visible` 只有这一次真的会变的读数。谁藏、藏几层、什么时候
        // 放回，全在 Rust 那一头记账（`state::PinState::dialog_opened`）；这里只报"有一张
        // 开了 / 有一张关了"，连开关都不递 —— 屏幕上藏没藏由配置那一格说了算，只能有一个权威。
        onVisibleChanged: visible ? win.session.dialogOpened() : win.session.dialogClosed()
        onAccepted: {
            win.session.savePin(
                win.pinId,
                String(saveAs.selectedFile),
                saveAs.selectedNameFilter.extensions.join(",")
            )
        }
    }
}
