// SPDX-License-Identifier: MIT
#include "pin_shim.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QHash>
#include <QtCore/QMap>
#include <QtCore/QMutex>
#include <QtCore/QMessageLogContext>
#include <QtCore/QMutexLocker>
#include <QtCore/QStringList>
#include <QtCore/QTimer>
#include <QtGui/QGuiApplication>
#include <QtGui/QPixmap>
#include <QtGui/QScreen>
#include <QtGui/QSurfaceFormat>
#include <QtGui/QWindow>
#include <QtQml/QQmlEngine>

#include <cstdio>
#include <cstring>

#ifdef Q_OS_WIN
#include <windows.h>
#endif

// ---------------------------------------------------------------- diagnostics

namespace {
QMutex g_msgMutex;
QStringList g_messages;

void captureMessage(QtMsgType type, const QMessageLogContext &context, const QString &msg)
{
    // The location travels with the text: a message that says `TypeError: Cannot
    // read property 'pressed'` without the file and line it came from is not
    // something a run can be pointed at.
    QString text;
    if (context.file != nullptr) {
        text = QString::fromUtf8(context.file) + QLatin1Char(':')
             + QString::number(context.line) + QLatin1String(": ") + msg;
    } else {
        text = msg;
    }
    const char *level = "info";
    switch (type) {
    case QtDebugMsg:
        level = "debug";
        break;
    case QtInfoMsg:
        break;
    case QtWarningMsg:
        level = "warn";
        break;
    case QtCriticalMsg:
        level = "critical";
        break;
    case QtFatalMsg:
        level = "fatal";
        break;
    }
    {
        QMutexLocker lock(&g_msgMutex);
        if (g_messages.size() < 64) {
            g_messages.append(QString::fromLatin1(level) + QLatin1String(": ") + text);
        }
    }
    // Straight to stderr, flushed: a GUI probe run has no console of its own, and
    // a buffered message that dies with the process is the failure this exists to
    // avoid.
    fprintf(stderr, "%s: %s\n", level, qPrintable(text));
    fflush(stderr);
}
}

void pinInstallMessageCapture()
{
    qInstallMessageHandler(captureMessage);
}

QString pinMessages()
{
    QMutexLocker lock(&g_msgMutex);
    return g_messages.join(QLatin1Char('\n'));
}

// ---------------------------------------------------------------- pixels

namespace {
QMutex g_mutex;
QHash<qint64, QImage> g_frames;

// The mask's frozen desktop, keyed by string. Its own mutex because the two maps
// are looked up in order and a lock held across the second lookup would be a
// lock-ordering bug waiting for a second thread.
QMutex g_maskMutex;
QMap<QString, QImage> g_maskFrames;

/// "7-3" -> 7. The revision suffix is what makes QML re-ask for a URL it has
/// already cached; the digits before it are the pin.
qint64 pinIdOf(const QString &id)
{
    QString head = id.section(QLatin1Char('-'), 0, 0);
    head.remove(QLatin1Char('/'));
    bool ok = false;
    const qint64 n = head.toLongLong(&ok);
    return ok ? n : -1;
}

QString describeWindow(QWindow *window)
{
    if (!window) {
        return QStringLiteral("  (null window)");
    }
    QString out = QString::fromLatin1("\n  class=%1")
                      .arg(QString::fromLatin1(window->metaObject()->className()));
    out += QString::fromLatin1(" title=%1").arg(window->title());
    out += QString::fromLatin1(" visible=%1").arg(window->isVisible() ? 1 : 0);
    out += QString::fromLatin1(" size=%1x%2").arg(window->width()).arg(window->height());
    out += QString::fromLatin1(" opacity=%1").arg(window->opacity(), 0.0, 'f', 2);
    out += QString::fromLatin1(" alphaBufferSize=%1").arg(window->format().alphaBufferSize());
    out += QString::fromLatin1(" qtFlags=0x%1")
               .arg(static_cast<qulonglong>(window->flags()), 0, 16);

#ifdef Q_OS_WIN
    const HWND hwnd = reinterpret_cast<HWND>(window->winId());
    const qint64 style = static_cast<qint64>(GetWindowLongPtrW(hwnd, GWL_STYLE));
    const qint64 exStyle = static_cast<qint64>(GetWindowLongPtrW(hwnd, GWL_EXSTYLE));
    out += QString::fromLatin1(" hwnd=0x%1").arg(reinterpret_cast<qulonglong>(hwnd), 0, 16);
    out += QString::fromLatin1(" style=0x%1").arg(style, 0, 16);
    out += QString::fromLatin1(" exStyle=0x%1").arg(exStyle, 0, 16);
    out += QString::fromLatin1(" FRAMELESS=%1").arg((style & WS_CHILDWINDOW) ? 0 : 1);
    out += QString::fromLatin1(" CAPTION=%1").arg((style & WS_CAPTION) ? 1 : 0);
    out += QString::fromLatin1(" LAYERED=%1").arg((exStyle & WS_EX_LAYERED) ? 1 : 0);
    out += QString::fromLatin1(" TRANSPARENT=%1").arg((exStyle & WS_EX_TRANSPARENT) ? 1 : 0);
    out += QString::fromLatin1(" TOPMOST=%1").arg((exStyle & WS_EX_TOPMOST) ? 1 : 0);
    out += QString::fromLatin1(" TOOLWINDOW=%1").arg((exStyle & WS_EX_TOOLWINDOW) ? 1 : 0);
    out += QString::fromLatin1(" APPWINDOW=%1").arg((exStyle & WS_EX_APPWINDOW) ? 1 : 0);
#endif
    return out;
}
} // namespace

// ---------------------------------------------------------------- surface

void enableAlphaBufferByDefault()
{
    // Qt 6.10 (measured in P1): setAlphaBuffer(bool) and supportsAlphaBuffer() are
    // gone, the format asks for a bit depth instead.
    QSurfaceFormat format = QSurfaceFormat::defaultFormat();
    format.setAlphaBufferSize(8);
    QSurfaceFormat::setDefaultFormat(format);
}

// ---------------------------------------------------------------- pixels

void pinStoreFrame(std::int64_t id, const QByteArray &png)
{
    QImage img;
    // Decoding only - the encode happened in Rust with the `image` crate.
    if (!img.loadFromData(png)) {
        qWarning("pinStoreFrame(%lld): Qt could not decode the PNG we encoded",
                 static_cast<long long>(id));
        return;
    }
    QMutexLocker lock(&g_mutex);
    g_frames.insert(id, img);
}

void pinDropFrame(std::int64_t id)
{
    QMutexLocker lock(&g_mutex);
    g_frames.remove(id);
}

QString pinSelfCheck(std::int64_t id)
{
    QImage img;
    int stored = 0;
    {
        QMutexLocker lock(&g_mutex);
        img = g_frames.value(id);
        stored = g_frames.size();
    }
    return QString::fromLatin1("id=%1 null=%2 w=%3 h=%4 alpha=%5 stores=%6")
        .arg(id)
        .arg(img.isNull() ? 1 : 0)
        .arg(img.width())
        .arg(img.height())
        .arg(img.hasAlphaChannel() ? 1 : 0)
        .arg(stored);
}

QString pinPixel(std::int64_t id, std::int32_t x, std::int32_t y)
{
    QImage img;
    {
        QMutexLocker lock(&g_mutex);
        img = g_frames.value(id);
    }
    if (x < 0 || y < 0 || x >= img.width() || y >= img.height()) {
        return QStringLiteral("oob");
    }
    const QRgb px = img.pixel(x, y);
    return QString::fromLatin1("%1,%2,%3,%4")
        .arg(qRed(px))
        .arg(qGreen(px))
        .arg(qBlue(px))
        .arg(qAlpha(px));
}

// ---------------------------------------------------------------- mask pixels

void maskStoreRaw(const QString &key, const QByteArray &rgba, std::int32_t width, std::int32_t height)
{
    if (width <= 0 || height <= 0 || rgba.size() < width * height * 4) {
        qWarning("maskStoreRaw(%s): %dx%d needs %d bytes, got %d",
                 qPrintable(key), width, height, width * height * 4, rgba.size());
        return;
    }
    // Borrowed first, then copied: the QByteArray is Rust's and dies when this
    // returns, so the store must own its pixels.
    const QImage borrowed(reinterpret_cast<const uchar *>(rgba.constData()),
                          width,
                          height,
                          width * 4,
                          QImage::Format_RGBA8888);
    const QImage owned = borrowed.copy();
    if (owned.isNull()) {
        qWarning("maskStoreRaw(%s): Qt refused to take %dx%d RGBA8", qPrintable(key), width, height);
        return;
    }
    QMutexLocker lock(&g_maskMutex);
    g_maskFrames.insert(key, owned);
}

void maskDropFrame(const QString &key)
{
    QMutexLocker lock(&g_maskMutex);
    g_maskFrames.remove(key);
}

QString maskSelfCheck(const QString &key)
{
    QImage img;
    int stored = 0;
    {
        QMutexLocker lock(&g_maskMutex);
        img = g_maskFrames.value(key);
        stored = g_maskFrames.size();
    }
    return QString::fromLatin1("key=%1 null=%2 w=%3 h=%4 stores=%5")
        .arg(key)
        .arg(img.isNull() ? 1 : 0)
        .arg(img.width())
        .arg(img.height())
        .arg(stored);
}

// ---------------------------------------------------------------- geometry

namespace {
QScreen *screenAt(std::int32_t index)
{
    const auto screens = QGuiApplication::screens();
    if (index < 0 || index >= screens.size()) {
        return nullptr;
    }
    return screens.at(index);
}

/// Qt's DIP rect for one screen. `QScreen::geometry()` is already in
/// device-independent pixels and already relative to Qt's virtual desktop, which
/// is the space a `Window`'s x/y live in - so a mask is placed from here and never
/// from the Win32 enumeration.
QRect screenGeometry(std::int32_t index)
{
    const QScreen *screen = screenAt(index);
    return screen ? screen->geometry() : QRect();
}
} // namespace

std::int32_t pinScreenCount()
{
    return QGuiApplication::screens().size();
}

QString pinScreenName(std::int32_t index)
{
    const QScreen *screen = screenAt(index);
    return screen ? screen->name() : QString();
}

std::int32_t pinScreenX(std::int32_t index) { return screenGeometry(index).x(); }
std::int32_t pinScreenY(std::int32_t index) { return screenGeometry(index).y(); }
std::int32_t pinScreenW(std::int32_t index) { return screenGeometry(index).width(); }
std::int32_t pinScreenH(std::int32_t index) { return screenGeometry(index).height(); }

double pinScreenDevicePixelRatio(std::int32_t index)
{
    const QScreen *screen = screenAt(index);
    return screen ? screen->devicePixelRatio() : 1.0;
}

// ---------------------------------------------------------------- desktop

namespace {
QRect desktopBounds()
{
    QRect bounds;
    const auto screens = QGuiApplication::screens();
    for (const QScreen *screen : screens) {
        bounds = bounds.isNull() ? screen->geometry() : bounds.united(screen->geometry());
    }
    return bounds;
}

QRect primaryBounds()
{
    const QScreen *screen = QGuiApplication::primaryScreen();
    return screen ? screen->geometry() : QRect();
}
} // namespace

std::int32_t pinDesktopX() { return desktopBounds().x(); }
std::int32_t pinDesktopY() { return desktopBounds().y(); }
std::int32_t pinDesktopW() { return desktopBounds().width(); }
std::int32_t pinDesktopH() { return desktopBounds().height(); }
std::int32_t pinPrimaryX() { return primaryBounds().x(); }
std::int32_t pinPrimaryY() { return primaryBounds().y(); }
std::int32_t pinPrimaryW() { return primaryBounds().width(); }
std::int32_t pinPrimaryH() { return primaryBounds().height(); }

double pinDevicePixelRatio()
{
    const QScreen *screen = QGuiApplication::primaryScreen();
    return screen ? screen->devicePixelRatio() : 1.0;
}

QString pinWindowReport()
{
    const auto windows = QGuiApplication::topLevelWindows();
    QString out = QString::fromLatin1("topLevelWindows=%1 dpr=%2\n")
                      .arg(windows.size())
                      .arg(pinDevicePixelRatio());
    for (QWindow *window : windows) {
        out += describeWindow(window);
    }
    return out;
}

void pinQuitAfter(std::int32_t ms)
{
    QTimer::singleShot(ms, [] { QCoreApplication::exit(0); });
}

// ---------------------------------------------------------------- grab

namespace {
QImage g_grab;
}

QByteArray pinScreenGrab()
{
    QScreen *screen = QGuiApplication::primaryScreen();
    g_grab = QImage();
    if (!screen) {
        return {};
    }
    // Whole screen, no window id, no offset: the question this leg answers is
    // "does Qt's own backend see what the Win32 legs see", and an offset here
    // would only make a coordinate-space disagreement look like a blank capture.
    const QPixmap raw = screen->grabWindow(0);
    if (raw.isNull()) {
        return {};
    }
    g_grab = raw.toImage().convertToFormat(QImage::Format_RGBA8888);
    if (g_grab.isNull()) {
        return {};
    }
    QByteArray out;
    out.resize(static_cast<int>(g_grab.sizeInBytes()));
    std::memcpy(out.data(), g_grab.constBits(), static_cast<size_t>(out.size()));
    return out;
}

QString pinScreenGrabInfo()
{
    if (g_grab.isNull()) {
        return QStringLiteral("null");
    }
    return QString::fromLatin1("%1x%2 dpr=%3")
        .arg(g_grab.width())
        .arg(g_grab.height())
        .arg(g_grab.devicePixelRatio(), 0.0, 'f', 2);
}

QString pinTopLevels()
{
    QString out;
    const auto windows = QGuiApplication::topLevelWindows();
    for (QWindow *window : windows) {
        if (!window) {
            continue;
        }
        out += QString::fromLatin1("class=%1 visible=%2")
                   .arg(QString::fromLatin1(window->metaObject()->className()))
                   .arg(window->isVisible() ? 1 : 0);
#ifdef Q_OS_WIN
        const HWND hwnd = reinterpret_cast<HWND>(window->winId());
        RECT r{};
        GetWindowRect(hwnd, &r);
        out += QString::fromLatin1(" hwnd=0x%1")
                   .arg(reinterpret_cast<qulonglong>(hwnd), 0, 16);
        out += QString::fromLatin1(" phys=%1,%2,%3,%4")
                   .arg(r.left)
                   .arg(r.top)
                   .arg(r.right - r.left)
                   .arg(r.bottom - r.top);
#endif
        const QString title = window->title();
        if (!title.isEmpty()) {
            out += QString::fromLatin1(" title=%1").arg(title);
        }
        out += QLatin1Char('\n');
    }
    return out;
}

// ---------------------------------------------------------------- native dialog

#ifdef Q_OS_WIN
namespace {

/// One window's caption or class name, bracketed. The brackets are the field: a
/// dialog with an empty title and a query that returned nothing are two different
/// readings, and a report that folds them together cannot be graded.
///
/// Only ever called on this process's own windows, for the reason `walkDialog`
/// below gives - a foreign handle's text is a foreign process's business.
QString winField(HWND hwnd, bool title)
{
    wchar_t buf[512]{};
    const int n = title ? GetWindowTextW(hwnd, buf, 512) : GetClassNameW(hwnd, buf, 512);
    if (n <= 0) {
        return QStringLiteral("[?]");
    }
    // The count is the return value rather than a NUL search: `wchar_t` is 16 bits
    // on MSVC, which is what these two APIs write, and a caption may legitimately
    // contain a NUL the API chose not to report.
    return QLatin1Char('[') + QString::fromWCharArray(buf, n) + QLatin1Char(']');
}

/// The `#32770` dialog boxes the walk found, plus a census of what it did *not*
/// match so a miss is self-explaining. `seen`/`first` are the click target; the rest
/// answers "why zero": how many top-levels this pid owns at all, how many are visible,
/// how many visible `#32770` dialogs another pid owns (the foreign-modal case the pid
/// filter exists to reject), and a bounded list of this pid's own visible top-levels with
/// the title `openDialogLeg` asked for, so a Qt dialog that does not report class
/// `#32770` shows up here rather than as a bare zero.
struct DialogWalk {
    int seen = 0;
    HWND first = nullptr;
    int pidTops = 0;          // top-levels owned by this process
    int pidVisible = 0;       // …of those, IsWindowVisible
    int foreignDialogs = 0;   // visible #32770 owned by another process
    QString pidClasses;       // bounded "[class]caption " list of this pid's visible tops
};

/// Everything under one dialog, plus the one button this run wants.
struct ButtonWalk {
    int descendants = 0;
    int buttons = 0;
    HWND ok = nullptr;
};

BOOL CALLBACK walkDialog(HWND hwnd, LPARAM lp)
{
    auto *walk = reinterpret_cast<DialogWalk *>(lp);
    const bool visible = IsWindowVisible(hwnd);
    DWORD pid = 0;
    GetWindowThreadProcessId(hwnd, &pid);
    const bool mine = pid == GetCurrentProcessId();
    wchar_t cls[64]{};
    const int clsLen = GetClassNameW(hwnd, cls, 64);
    const bool isDialog = clsLen > 0 && QString::fromWCharArray(cls, clsLen) == QLatin1String("#32770");

    if (mine) {
        ++walk->pidTops;
        if (visible) {
            ++walk->pidVisible;
            // The census is the answer to `dialogs=0`: the classes this process really
            // had a window for, with the caption next to them. Bounded because this runs
            // on a machine with the user's whole desktop in it - and only this process's
            // windows are ever named, for the reason below.
            if (walk->pidClasses.size() < 400) {
                walk->pidClasses += winField(hwnd, false) + winField(hwnd, true) + QLatin1Char(' ');
            }
        }
    } else if (visible && isDialog) {
        // Counted, never touched: another process can and does own a visible `#32770` on
        // the same desktop - an activation prompt, a consent box - and pressing its
        // default button would be a write to a window this project has no claim on.
        ++walk->foreignDialogs;
    }

    // The click target keeps the three filters it had: visible, ours, and a real Win32
    // dialog box.
    if (!visible || !mine || !isDialog) {
        return TRUE;
    }
    ++walk->seen;
    if (walk->first == nullptr) {
        walk->first = hwnd;
    }
    return TRUE;
}

BOOL CALLBACK walkButton(HWND hwnd, LPARAM lp)
{
    auto *walk = reinterpret_cast<ButtonWalk *>(lp);
    ++walk->descendants; // `EnumChildWindows` walks descendants, not children only.
    wchar_t cls[64]{};
    if (GetClassNameW(hwnd, cls, 64) <= 0
        || QString::fromWCharArray(cls) != QLatin1String("Button")) {
        return TRUE;
    }
    ++walk->buttons;
    // Matched on the control id, never on the caption: the measured button is
    // `title=[&Save]` on an English shell and 保存 on a Chinese one, and §5.9.17's
    // dialog is localized by the shell. `IDOK` is what makes it the default button.
    if (!walk->ok && GetDlgCtrlID(hwnd) == IDOK && IsWindowVisible(hwnd)) {
        walk->ok = hwnd;
    }
    return TRUE;
}
} // namespace
#endif

QString pinPushDialogButton()
{
#ifdef Q_OS_WIN
    DialogWalk dialogs;
    EnumWindows(walkDialog, reinterpret_cast<LPARAM>(&dialogs));
    if (dialogs.first == nullptr) {
        return QString::fromLatin1(
                   "pushed=no reason=no-visible-#32770-of-this-pid dialogs=%1 "
                   "mine_tops=%2 mine_visible=%3 foreign_dialogs=%4 mine=%5")
            .arg(dialogs.seen)
            .arg(dialogs.pidTops)
            .arg(dialogs.pidVisible)
            .arg(dialogs.foreignDialogs)
            .arg(dialogs.pidClasses.trimmed().isEmpty() ? QStringLiteral("[-]")
                                                         : dialogs.pidClasses.trimmed());
    }
    const HWND dialog = dialogs.first;

    ButtonWalk buttons;
    EnumChildWindows(dialog, walkButton, reinterpret_cast<LPARAM>(&buttons));
    if (buttons.ok == nullptr) {
        return QString::fromLatin1(
                   "pushed=no reason=no-enabled-idok-button dialogs=%1 desc=%2 buttons=%3 title=%4")
            .arg(dialogs.seen)
            .arg(buttons.descendants)
            .arg(buttons.buttons)
            .arg(winField(dialog, true));
    }
    const HWND button = buttons.ok;

    // Same-thread `SendMessageW`: the leg that calls this runs from a QML timer
    // inside the dialog's own nested loop, so the button's window procedure is
    // entered directly rather than queued behind anything. `BM_CLICK` is what the
    // shell's mouse handling sends, so what arrives afterwards is the dialog's
    // normal `accepted`, not a completion Qt was talked into.
    //
    // `BM_CLICK`'s return value is undefined by the API, so it is printed as what it
    // is (`ret=`) and the caller grades the click by what the dialog did next.
    const LRESULT clicked = SendMessageW(button, BM_CLICK, 0, 0);
    return QString::fromLatin1(
               "pushed=yes dialog=0x%1 class=%2 title=%3 dialogs=%4 desc=%5 buttons=%6 "
               "button=0x%7 btitle=%8 bid=%9 ret=%10 visible_after=%11")
        .arg(reinterpret_cast<qulonglong>(dialog), 0, 16)
        .arg(winField(dialog, false))
        .arg(winField(dialog, true))
        .arg(dialogs.seen)
        .arg(buttons.descendants)
        .arg(buttons.buttons)
        .arg(reinterpret_cast<qulonglong>(button), 0, 16)
        .arg(winField(button, true))
        .arg(GetDlgCtrlID(button))
        .arg(static_cast<qint64>(clicked))
        .arg(IsWindowVisible(dialog) ? 1 : 0);
#else
    return QStringLiteral("pushed=no reason=not-windows");
#endif
}

// ---------------------------------------------------------------- provider

PinImageProvider::PinImageProvider()
    : QQuickImageProvider(QQuickImageProvider::Image)
{
}

QImage PinImageProvider::requestImage(const QString &id, QSize *size, const QSize &requestedSize)
{
    Q_UNUSED(requestedSize) // The window stretches; the bitmap is already the shown size.
    QImage img;
    {
        // Mask keys first, as exact strings: a pin's id is `7-3`, which is never a
        // key in this map, and a mask's `m0-display1-1` is never parseable as one.
        QMutexLocker lock(&g_maskMutex);
        img = g_maskFrames.value(id);
    }
    if (img.isNull()) {
        QMutexLocker lock(&g_mutex);
        img = g_frames.value(pinIdOf(id));
    }
    if (img.isNull()) {
        // A pin with no pixels yet still has to be a valid texture, or the Image
        // element logs an error every time a window is created.
        img = QImage(1, 1, QImage::Format_ARGB32_Premultiplied);
        img.fill(Qt::transparent);
    }
    if (size) {
        *size = img.size();
    }
    return img;
}

// ---------------------------------------------------------------- shim object

int PinShim::install(QQuickWindow *window)
{
    if (!window) {
        return 0;
    }
    QQmlEngine *engine = qmlEngine(window);
    if (!engine) {
        return 0;
    }
    if (engine->imageProvider(QStringLiteral("falconshot"))) {
        return 0; // every window after the first shares the one engine
    }
    engine->addImageProvider(QStringLiteral("falconshot"), new PinImageProvider);
    return 1;
}

void PinShim::applyStyle(QQuickWindow *window, bool topmost, bool clickThrough)
{
    if (!window) {
        return;
    }
#ifdef Q_OS_WIN
    const HWND hwnd = reinterpret_cast<HWND>(window->winId());
    if (!hwnd) {
        return;
    }
    qint64 ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    ex = topmost ? (ex | WS_EX_TOPMOST) : (ex & ~static_cast<qint64>(WS_EX_TOPMOST));
    if (clickThrough) {
        // WS_EX_TRANSPARENT is only ignored-by-the-mouse for real if the window
        // is layered; Qt makes pins layered as soon as alpha or opacity < 1 is in
        // play, and asking for it here keeps the click-through promise when a pin
        // is fully opaque.
        ex |= WS_EX_LAYERED | WS_EX_TRANSPARENT;
    } else {
        ex &= ~static_cast<qint64>(WS_EX_TRANSPARENT);
    }
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, static_cast<LONG_PTR>(ex));
    SetWindowPos(hwnd, topmost ? HWND_TOPMOST : HWND_NOTOPMOST, 0, 0, 0, 0,
                 SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED);
#else
    Q_UNUSED(window)
    Q_UNUSED(topmost)
    Q_UNUSED(clickThrough)
#endif
}

QString PinShim::describe(QQuickWindow *window)
{
    return describeWindow(window);
}
