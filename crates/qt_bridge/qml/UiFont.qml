import QtQuick
import dev.falconshot 1.0

// §5.19.4「支持调整界面字体和字号」在页上的那一枚**读者**，全仓只有这一份。
//
// 为什么是一枚共享对象而不是每扇窗各写三行：那两枚键说的是**同一件事的两个面**（哪一族字、
// 多大），而「此刻的磅 ÷ 出厂的磅」这一枚倍数、空族走系统字这一枚兜底、以及「别把整页压成一枚
// 字号、层次乘一遍」的取整口径，每抄一次就多一个主人（#281）。P149 之前这件事只长在
// `SettingsWindow.qml` 一栋里，另外三扇窗（历史、分组、自定义截图）把 `font.pixelSize` 写死 ——
// 改档不动那些像素，正是任务表里那一格「逻辑齐备、生产路径零读者」的形状。
//
// 绑的是 `Session` 那两枚**值**（notify 一到就重新求值），不是页面自己再去读档：档那一头只有
// `settings::ui_font_face` 一次读，四扇窗与 `[appearance cfg]` 那行看的是同一趟读数。
//
// 用 `QtObject` 而不是可视项：它不占布局、不接指针，宿主把 `session` 递进来就完事（与
// `AnnotationToolbar.qml` 里那几枚 `readonly property` 同一条规矩 —— 面板不爬窗口）。
QtObject {
    id: ui

    required property Session session

    /// 页上那套字号层次围绕的**基准磅数**，现问桥（`Session.uiFontBaseline()` →
    /// `settings::ui_font_baseline` → core 的出厂值）。这一格随创建求一次就够：它是出厂值而不是
    /// 用户值，用户改档改的是下面那枚倍数。
    readonly property int baseline: ui.session.uiFontBaseline()

    /// 「此刻的磅 ÷ 出厂的磅」。`ui_font` 为 `0` = 桥还没 publish（第一帧，`refreshTheme()` 之前），
    /// 那一臂走 1 —— 也就是页上画好的那套层次原样，不是把字压成 0 磅。分母为 0 同样走 1：
    /// 没有基准就没有倍数可言，而不是除出一个 `Infinity` 把整页挤成一行。
    readonly property real scale: ui.session.ui_font > 0 && ui.baseline > 0
        ? ui.session.ui_font / ui.baseline
        : 1

    /// 界面上的字用哪一族。空串那一臂 = 桥还没 publish，走**应用自己的默认字族**（那些窗原本就
    /// 没写 `font.family`，画的就是它）；从档里读来的那一枚永不为空，因为 core 的 `validate`
    /// 在读档那一步就把空族归成出厂族 —— 页上不兜第二次（#281）。
    readonly property string family: ui.session.ui_font_family !== ""
        ? ui.session.ui_font_family
        : Qt.application.font.family

    /// 页上每一处写死的那枚像素数，乘的是**层次**而不是替换：整页压成一枚磅数会把正文 13、
    /// 说明 12、标题 14/18 那层结构抹平。地板 8 是给倍数压到极小时留的最后一眼可读 —— 与
    /// §7.4「基本可读性」同一句，取的是同一枚口径，所以它也只长在这一格。
    function px(base) {
        return Math.max(8, Math.round(base * ui.scale))
    }
}
