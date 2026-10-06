// SPDX-License-Identifier: MIT
#include "pin_shim.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QHash>
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
    Q_UNUSED(context);
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
            g_messages.append(QString::fromLatin1(level) + QLatin1String(": ") + msg);
        }
    }
    // Straight to stderr, flushed: a GUI probe run has no console of its own, and
    // a buffered message that dies with the process is the failure this exists to
    // avoid.
    fprintf(stderr, "%s: %s\n", level, qPrintable(msg));
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

// ---------------------------------------------------------------- geometry

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
