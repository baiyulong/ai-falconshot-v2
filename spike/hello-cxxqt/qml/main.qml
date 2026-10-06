import QtQuick
import QtQuick.Window
import QtQuick.Controls
import dev.falconshot.spike 1.0

ApplicationWindow {
    id: root
    width: 760
    height: 560
    visible: true
    title: qsTr("AI Falconshot M0 spike - CXX-Qt 0.10 <-> Qt 6.10")

    readonly property Probe probe: Probe {}
    readonly property FramePump pump: FramePump {}
    readonly property MaskProbe mask: MaskProbe {}
    readonly property OverlayProbe overlay: OverlayProbe {}
    // C++ types from cpp/*.h, in the module because QML_ELEMENT + moc + qmltyperegistrar
    // put them there. qmlRegisterType into this URI is rejected: the namespace is taken.
    FrozenInstaller { id: installer }
    OverlayInstaller { id: overlayInstaller }
    property string signalLog: "(no signal yet)"
    property string threadLog: "(thread suite not run)"
    property string p1Report: "(P1 suite not run)"
    property string p4Report: "(P4 suite not run)"

    // Event-loop liveness proxy: interval timers lose ticks when the Qt thread is
    // busy, so ticks-against-wallclock is the deficit the mask/overlay will feel.
    property int pumpTicks: 0
    property int cfgTicks: 0

    Column {
        anchors.fill: parent
        anchors.margins: 16
        spacing: 8

        Label { text: qsTr("Rust qproperty  counter        = %1").arg(probe.counter) }
        Label {
            text: qsTr("shim report via qproperty last_from_qml:\n%1").arg(probe.last_from_qml)
            wrapMode: Label.WordWrap
            width: parent.width
        }
        Label {
            text: qsTr("Rust qsignal    counterBumped  = %1").arg(root.signalLog)
            wrapMode: Label.WordWrap
            width: parent.width
        }

        Row {
            spacing: 8
            Button {
                text: qsTr("bumpCounter()")
                onClicked: probe.bumpCounter()
            }
            Button {
                text: qsTr("recordFromQml()")
                onClicked: probe.recordFromQml("hello from QML at " + Date())
            }
            Button {
                text: qsTr("probeWindows()")
                onClicked: probe.probeWindows()
            }
            Button {
                text: qsTr("quit")
                onClicked: Qt.quit()
            }
        }

        // ---- increment 3: background thread -> CxxQtThread::queue -> QML ----
        Label {
            text: qsTr("worker frame  seq=%1  queue=%2us  copy=%3us  suite_running=%4").arg(pump.last_seq).arg(pump.last_queue_us).arg(pump.last_copy_us).arg(pump.suite_running)
        }
        Label {
            text: qsTr("thread suite (last config):\n%1").arg(root.threadLog)
            wrapMode: Label.WordWrap
            width: parent.width
        }

        Row {
            spacing: 8
            Button {
                text: qsTr("runSuite(quick)")
                enabled: !pump.suite_running
                onClicked: pump.runSuite(true)
            }
            Button {
                text: qsTr("runSuite(full ~12s)")
                enabled: !pump.suite_running
                onClicked: pump.runSuite(false)
            }
            Button {
                text: qsTr("stopSuite()")
                enabled: pump.suite_running
                onClicked: pump.stopSuite()
            }
        }

        // ---- increment 4: P1 mask latency ----
        Label {
            text: qsTr("mask state=%1  session=%2/%3  token=%4  hole=%5,%6 %7x%8")
                      .arg(mask.state).arg(mask.sessions_done).arg(mask.sessions_planned)
                      .arg(mask.token).arg(mask.hole_x).arg(mask.hole_y).arg(mask.hole_w).arg(mask.hole_h)
        }
        Label {
            text: qsTr("P1 report:\n%1").arg(root.p1Report)
            wrapMode: Label.WordWrap
            width: parent.width
        }
        Row {
            spacing: 8
            Button {
                text: qsTr("P1 runSuite(quick)")
                enabled: !mask.running && mask.sessions_done === 0
                onClicked: mask.runSuite(true)
            }
            Button {
                text: qsTr("P1 runSuite(full ~10 sessions)")
                enabled: !mask.running && mask.sessions_done === 0
                onClicked: mask.runSuite(false)
            }
        }
        Label {
            text: qsTr("P4 state=%1 cfg=%2/%3 mode img:%4 painted:%5 rect:%6 token=%7 paint=%8 rect=%9,%10,%11,%12 canvas=%13x%14")
                  .arg(overlay.state).arg(overlay.config_done).arg(overlay.config_planned)
                  .arg(overlay.image_mode).arg(overlay.painted_mode).arg(overlay.rect_mode)
                  .arg(overlay.token).arg(overlay.paint_token)
                  .arg(overlay.rect_x).arg(overlay.rect_y).arg(overlay.rect_w).arg(overlay.rect_h)
                  .arg(overlay.canvas_w).arg(overlay.canvas_h)
        }
        Label {
            text: qsTr("P4 report:\n%1").arg(root.p4Report)
            wrapMode: Label.WordWrap
            width: parent.width
        }
        Row {
            spacing: 8
            Button {
                text: qsTr("P4 runSuite(quick)")
                enabled: !overlay.running && overlay.config_done === 0
                onClicked: overlay.runSuite(true)
            }
            Button {
                text: qsTr("P4 runSuite(full 24/config)")
                enabled: !overlay.running && overlay.config_done === 0
                onClicked: overlay.runSuite(false)
            }
        }
    }

    // P3: per-pixel alpha + whole-window opacity + click-through on one window.
    // Every pin window in AI Falconshot needs these three to coexist.
    Window {
        id: alphaTest
        x: 700
        y: 140
        width: 320
        height: 200
        visible: true
        title: qsTr("pin simulation")
        color: "transparent"
        opacity: 0.55
        flags: Qt.Window | Qt.FramelessWindowHint | Qt.WindowTransparentForInput | Qt.WindowStaysOnTopHint

        Rectangle {
            anchors.fill: parent
            anchors.margins: 18
            radius: 26
            color: "#c02a5c8a"
            border.color: "white"
            border.width: 2

            Text {
                anchors.centerIn: parent
                text: qsTr("click-through pin")
                color: "white"
            }
        }
    }

    // P1: the screenshot mask. Frozen desktop as a texture, a dim layer with a hole,
    // and a selection that moves every 8 ms. Rust owns the timing; this Window only
    // binds properties and reports presented frames.
    Window {
        id: maskWindow
        x: 0
        y: 0
        width: Screen.width
        height: Screen.height
        visible: mask.mask_visible
        color: "black"
        title: qsTr("P1 mask")
        flags: Qt.Window | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint
        onFrameSwapped: mask.noteSwap()

        Image {
            id: frozen
            anchors.fill: parent
            cache: false
            asynchronous: false
            fillMode: Image.Stretch
            // No sourceSize: letting Qt down-scale the 4K frame on the CPU would hide
            // the texture-upload cost this probe exists to measure.
            source: mask.mask_visible ? ("image://frozen/" + mask.token) : ""
        }

        // Variant A: four dim rectangles around the hole. Cheap, but it is four
        // quads over the whole 4K surface and the frozen image is drawn separately.
        Item {
            id: rectDim
            anchors.fill: parent
            visible: !mask.shader_dim

            Rectangle {
                x: 0; y: 0
                width: parent.width
                height: Math.max(0, mask.hole_y)
                color: "#99000000"
            }
            Rectangle {
                x: 0; y: mask.hole_y + mask.hole_h
                width: parent.width
                height: Math.max(0, parent.height - (mask.hole_y + mask.hole_h))
                color: "#99000000"
            }
            Rectangle {
                x: 0; y: mask.hole_y
                width: Math.max(0, mask.hole_x)
                height: Math.max(0, mask.hole_h)
                color: "#99000000"
            }
            Rectangle {
                x: mask.hole_x + mask.hole_w; y: mask.hole_y
                width: Math.max(0, parent.width - (mask.hole_x + mask.hole_w))
                height: Math.max(0, mask.hole_h)
                color: "#99000000"
            }
        }

        // Variant B: one ShaderEffect that samples the frozen frame and writes the
        // dimmed/un-dimmed pixel in a single pass. P5 recorded this as not buildable
        // because the Qt install had no qtshadertools; P6 installed the module and
        // found that qsb needs no glslc, so the variant is now measurable. The .qsb
        // is baked by build.rs and lives beside this file in the qrc, hence the
        // relative URL.
        ShaderEffect {
            id: dimEffect
            anchors.fill: parent
            visible: mask.shader_dim
            supportsAtlasTextures: false

            property ShaderEffectSource src: ShaderEffectSource {
                sourceItem: frozen
                hideSource: mask.shader_dim
            }
            property color dimColor: "#99000000"
            property vector4d hole: Qt.vector4d(mask.hole_x, mask.hole_y, mask.hole_w, mask.hole_h)
            property vector2d deviceSize: Qt.vector2d(width * Screen.devicePixelRatio,
                                                      height * Screen.devicePixelRatio)
            property real dpr: Screen.devicePixelRatio

            fragmentShader: "dim_hole.frag.qsb"
            onStatusChanged: mask.noteShaderStatus(status)
            Component.onCompleted: mask.noteShaderStatus(status)
        }

        Rectangle {
            x: mask.hole_x; y: mask.hole_y
            width: Math.max(0, mask.hole_w)
            height: Math.max(0, mask.hole_h)
            color: "transparent"
            border.color: "white"
            border.width: 2
        }
    }

    // P4: the annotation layer, committed one 图元 at a time. Three ways to put it
    // on screen, switched by the probe's mode flags - texture (whole layer), texture
    // (dirty rect only), QQuickPaintedItem, and a plain Rectangle as the reference.
    Window {
        id: overlayWindow
        x: 0
        y: 0
        width: Screen.width
        height: Screen.height
        visible: overlay.window_visible
        title: qsTr("P4 annotation overlay")
        color: "#202428"
        flags: Qt.Window | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint

        // Canvas pixels -> window DIP. The textures are canvas-sized, so only this
        // mapping changes; the upload volume never depends on DIP size.
        readonly property real sx: overlay.canvas_w > 0 ? width / overlay.canvas_w : 1
        readonly property real sy: overlay.canvas_h > 0 ? height / overlay.canvas_h : 1

        onFrameSwapped: overlay.noteSwap()

        Image {
            id: overlayImage
            visible: overlay.image_mode
            cache: false
            asynchronous: false
            fillMode: Image.Stretch
            // rect_w == 0 is the probe's "re-request the whole layer" marker.
            x: overlay.rect_w > 0 ? overlayWindow.sx * overlay.rect_x : 0
            y: overlay.rect_h > 0 ? overlayWindow.sy * overlay.rect_y : 0
            width: overlay.rect_w > 0 ? overlayWindow.sx * overlay.rect_w : overlayWindow.width
            height: overlay.rect_h > 0 ? overlayWindow.sy * overlay.rect_h : overlayWindow.height
            source: overlay.image_mode
                    ? ("image://overlay/" + overlay.token + "/" + overlay.rect_x
                       + "/" + overlay.rect_y + "/" + overlay.rect_w + "/" + overlay.rect_h)
                    : ""
        }

        PaintedOverlay {
            visible: overlay.painted_mode
            anchors.fill: parent
            marker: Qt.rect(overlayWindow.sx * overlay.rect_x, overlayWindow.sy * overlay.rect_y,
                             overlayWindow.sx * overlay.rect_w, overlayWindow.sy * overlay.rect_h)
            // The binding is the commit: Rust bumping paint_token is what makes the
            // item call update(), so t0 and this repaint are in the same frame.
            seq: overlay.paint_token
        }

        Rectangle {
            visible: overlay.rect_mode
            color: "transparent"
            border.color: "#ff2828"
            border.width: 3
            x: overlayWindow.sx * overlay.rect_x
            y: overlayWindow.sy * overlay.rect_y
            width: overlayWindow.sx * overlay.rect_w
            height: overlayWindow.sy * overlay.rect_h
        }
    }

    Timer {
        id: overlayPacer
        interval: 13
        repeat: true
        running: overlay.running
        onTriggered: overlay.tick()
    }

    Timer {
        id: maskPacer
        interval: 16
        repeat: true
        running: true
        onTriggered: mask.tick()
    }

    Timer {
        id: maskDrag
        interval: 8
        repeat: true
        running: mask.state === 2
        onTriggered: mask.dragStep()
    }

    Connections {
        target: probe
        function onCounterBumped(value, usPerSet) {
            root.signalLog = "counterBumped(value=" + value + ", usPerSet=" + usPerSet + ")"
            // Only reached if the Rust qsignal really crossed into QML.
            probe.confirmSignal(value);
        }
    }

    Connections {
        target: mask

        function onReportReady(report) {
            root.p1Report = report;
            // P4 runs off the same event loop the mask just vacated, so it does not
            // inherit P1's tail frames.
            p4Kick.start();
        }
    }

    Connections {
        target: overlay

        function onReportReady(report) {
            root.p4Report = report;
        }
    }

    Connections {
        target: pump

        function onConfigStarted(idx, name, hz, frames, width, height, copy) {
            root.cfgTicks = root.pumpTicks;
        }

        function onConfigFinished(idx, name, delivered, hzAchieved, avgQueueUs, worstQueueUs, avgCopyUs, worstCopyUs, wallMs) {
            // Expected ticks come from the config's own wall time, not Date.now():
            // a QML `int` property cannot hold epoch milliseconds (it overflows).
            var expected = Math.round(wallMs / 16);
            var got = root.pumpTicks - root.cfgTicks;
            var line = "cfg" + (idx + 1) + " " + name
                    + " | delivered=" + delivered
                    + " hz=" + hzAchieved
                    + " | queue avg=" + avgQueueUs + "us worst=" + worstQueueUs + "us"
                    + " | copy avg=" + avgCopyUs + "us worst=" + worstCopyUs + "us"
                    + " | wall=" + wallMs + "ms"
                    + " | qmlTicks=" + got + "/" + expected + " deficit=" + Math.max(0, expected - got);
            root.threadLog = line;
            // console.log() does not reach a redirected stdout on Windows.
            probe.recordFromQml(line);
        }

        function onSuiteFinished(report) {
            root.threadLog = report;
            probe.recordFromQml("[suite done] tickTotal=" + root.pumpTicks);
        }
    }

    Timer {
        id: probeTimer
        interval: 500
        onTriggered: probe.probeWindows()
    }

    Timer {
        id: tickTimer
        interval: 16
        repeat: true
        running: true
        onTriggered: root.pumpTicks = root.pumpTicks + 1
    }

    // Start only once the self-test above has fully drained the event loop, so the
    // first session measures the mask and not the startup backlog.
    Timer {
        id: suiteKick
        interval: 2500
        onTriggered: mask.runSuite(true)
    }

    // Chained off P1's report so an unattended run leaves both tables in the log.
    Timer {
        id: p4Kick
        interval: 1200
        onTriggered: overlay.runSuite(true)
    }

    // Self test: every crossing the M0 gate cares about, without needing a click.
    Component.onCompleted: {
        // Before any Image asks for image://frozen/...
        var installed = installer.install(root);
        probe.recordFromQml("[P1] addImageProvider via qmlEngine(window) = " + installed);
        installer.setWindow(maskWindow);
        var installed4 = overlayInstaller.install(root);
        probe.recordFromQml("[P4] overlay addImageProvider = " + installed4);
        mask.setViewport(Screen.width, Screen.height);

        probe.recordFromQml("hello from QML");
        probe.bumpCounter();

        // QML -> Rust raw call cost. Reported through recordFromQml because
        // console.log() does not reach a redirected stdout on Windows.
        var n = 200000;
        var t0 = Date.now();
        for (var i = 0; i < n; ++i) {
            probe.noop();
        }
        var dt = Date.now() - t0;
        probe.recordFromQml("QML -> Rust noop x" + n + " = " + dt + " ms (" + (dt * 1000 / n) + " us/call)");

        probe.measureRoundTrips(20000);

        // Let alphaTest finish being created and mapped before reading its styles.
        probeTimer.start();
        // Run after the P3 probe has had a chance to read the window styles.
        suiteKick.start();
    }

    Component.onDestruction: pump.stopSuite()
}
