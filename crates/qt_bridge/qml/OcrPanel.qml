import QtQuick
import dev.falconshot 1.0

// §5.6.1 步骤 4 的那一「识别结果面板」。
//
// 它一枚也不判：识别的三格读数（`ocr_text` / `ocr_busy` / `ocr_error`）由桥写，这一层把它们
// 摆出来，再把手势递回桥（复制 → `copyOcrText`，重新识别 → [`Session::extractMaskText`] /
// [`Session::extractPinText`] 由主人接）。
//
// 为什么是一枚**组件**而不是两扇窗各抄一段：遮罩那一臂与贴图那一臂要的读数完全相同，抄两遍
// 就会有一遍在下一轮改码时忘了改（#281）。两臂不同的只有「重新识别取哪张图」，那一问递成
// [`Self::reRecognized`] 这一声，由主人决定 —— 面板没有那张图的地址。
Rectangle {
    id: panel

    required property Session session

    /// 主人有没有在**这一扇窗**里点过「文字提取」。三格读数挂在 Session 上是**全局**的一格，
    /// 而面板是每扇窗各一枚：少了这一格，一场多屏截图里另一扇窗会凭空摆出别人那一屏的结果。
    /// 点下时由主人置真，「收起」置假 —— 于是「重新识别」之后它自己还开着，能关它的只有用户。
    property bool asked: false

    /// 「复制全部」/「复制选中」按完，桥自己那句话落在这儿。为什么不并 [`Session::status`]：
    /// 那一句是全场的（`HintWindow.qml` 读它），而这一句只关于这一块面板，两者各说各的事 ——
    /// 挤进一格就会有一件没人说（#281）。
    property string said: ""

    /// 用户按了「重新识别」。来源由主人决定，面板不猜。
    signal reRecognized()

    width: 380
    height: col.implicitHeight + 16
    // 「问过了」才算这一屏有事：没点过的窗一块也不摆。识别完结果是空串、错误也是空串时照样
    // 开 —— 那正是「跑了、图上没字」那一格，`#col` 里第三行说得出它，藏着反而说不清。
    visible: panel.asked && (panel.session.ocr_busy
                             || panel.session.ocr_text.length > 0
                             || panel.session.ocr_error.length > 0
                             || panel.said.length > 0)

    color: "#f0232326"
    border.color: "#55555c"
    border.width: 1
    radius: 6

    component Chip: Rectangle {
        id: chip

        property string label: ""
        property bool usable: true
        signal clicked()

        width: cap.implicitWidth + 14
        height: 22
        radius: 3
        color: area.containsMouse && chip.usable ? "#3a3a41" : "transparent"

        Text {
            id: cap
            anchors.centerIn: parent
            text: chip.label
            font.pixelSize: 12
            color: chip.usable ? "#f2f2f2" : "#6a6a70"
        }

        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            enabled: chip.usable
            cursorShape: chip.usable ? Qt.PointingHandCursor : Qt.ArrowCursor
            // 释放落在钮上才算一次：与 `AnnotationToolbar.qml` 那一族 `Cell` 同一枚理由 ——
            // 按下去又滑出钮外的指针是想在底下那张图上画东西，不是想按这颗钮。
            onReleased: if (containsMouse) chip.clicked()
        }
    }

    Column {
        id: col
        x: 8
        y: 8
        width: panel.width - 16
        spacing: 6

        // 标题那一行。`本机识别，不联网` 是给 §5.6.1 最后那条规则回的话：那一条写的是「若识别
        // 依赖在线服务，应明确显示网络和隐私提示」，本产品不依赖在线服务，于是这里明说的是
        // "没有那一回事"，而不是留白让人自己猜。
        Row {
            spacing: 8

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "识别结果"
                font.pixelSize: 13
                color: "#f2f2f2"
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "本机识别，不联网"
                font.pixelSize: 11
                color: "#8e8e96"
            }
        }

        // 进度那一格（§5.6.1 规则「识别期间应显示进度状态」）。它读的是桥那一格
        // `ocr_busy`，不是窗自己点下去那一刻置的旗：几百毫秒里活儿归桥管。
        Text {
            visible: panel.session.ocr_busy
            text: "识别中…"
            font.pixelSize: 12
            color: "#2d7ff7"
        }

        // 失败那一行写的是平台层的原话（崩在哪一步就在里面），这一层不替它想一句好听的话。
        // 「允许重新框选」那一半由遮罩本来就还开着来满足，所以这里只指一次路，不再开一扇门。
        Text {
            visible: !panel.session.ocr_busy && panel.session.ocr_error.length > 0
            text: panel.session.ocr_error + "（可重新框选后再点一次「文字提取」）"
            font.pixelSize: 12
            color: "#ff9f0a"
            wrapMode: Text.WordWrap
            width: col.width
        }

        // 跑了、图上没有字：那是一个答案而不是故障 —— 平台层对这种情况交回的是 `Ok("")`。
        Text {
            visible: !panel.session.ocr_busy && panel.session.ocr_error.length === 0
                     && panel.session.ocr_text.length === 0
            text: "图中未识别到文字"
            font.pixelSize: 12
            color: "#8e8e96"
        }

        // 结果本体。折行是**视觉**的：那一串字里的 `\n` 是平台层按 `OcrLine` 一行一行拼出来的
        // （§5.6.1 规则「保留自然段和换行」），这里一个字也不改，而 `selectedText` 交回的
        // 也就是原文的那几行。
        //
        // 这一枚不写 `boundsBehavior`：本机 qmllint 报 `Member "StopAtEdges" not found on type
        // "QQuickFlickable"`，而两档探针（设与不设）在 `qml` 那一头回的是同一个数，量不出差别
        // —— 一枚分不清两态的量规不钉（#433）。默认那档（可越界回弹）对一块文字面板不是故障。
        Flickable {
            id: scroll
            visible: panel.session.ocr_text.length > 0
            width: col.width
            height: Math.min(ocrField.implicitHeight, 180)
            contentWidth: width
            contentHeight: ocrField.height
            clip: true

            TextEdit {
                id: ocrField
                width: scroll.width
                text: panel.session.ocr_text
                font.pixelSize: 12
                color: "#f2f2f2"
                wrapMode: TextEdit.Wrap
                readOnly: true
                selectByMouse: true
                // 按下不抢焦点：焦点在遮罩那一侧的 root 上，箭头键微调选区与 Esc 全靠它。字照样
                // 选得（`selectByMouse` 不依赖焦点），键也不抢 —— 两侧都在。所以部分复制走下面
                // 那颗「复制选中」，而不是等 Ctrl+C：那枚键在这一屏归遮罩的键处理。
                activeFocusOnPress: false
            }
        }

        Text {
            visible: panel.said.length > 0
            text: panel.said
            font.pixelSize: 12
            color: "#8e8e96"
        }

        // 忙的时候这一整行不摆：`复制` 与 `重新识别` 在此刻都还没有可复制的字、也没有可等的第二趟
        // —— 再点一次会起第二条线程，而两趟哪一趟先落地没人知道。
        Row {
            spacing: 4
            visible: !panel.session.ocr_busy

            Chip {
                label: "复制全部"
                usable: panel.session.ocr_text.length > 0
                // 桥说的是它自己那一格；窗递过去的也是那一格，两处读的是同一份（#281）。
                onClicked: panel.said = panel.session.copyOcrText(panel.session.ocr_text)
            }

            Chip {
                label: "复制选中"
                usable: ocrField.selectedText.length > 0
                onClicked: panel.said = panel.session.copyOcrText(ocrField.selectedText)
            }

            Chip {
                label: "重新识别"
                onClicked: panel.reRecognized()
            }

            // 关的是**这一扇窗**里这一块面板，不是那三格读数：读数归桥，别的窗要是自己点过，
            // 它那一块照旧开着自己的。
            Chip {
                label: "收起"
                onClicked: panel.asked = false
            }
        }
    }
}
