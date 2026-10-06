#pragma once

#include <cstdint>

#include <QtCore/QByteArray>
#include <QtCore/QObject>
#include <QtCore/QString>
#include <QtGui/QImage>
#include <QtQml/QQmlImageProviderBase>
#include <QtQml/qqml.h>
#include <QtQuick/QQuickImageProvider>
#include <QtQuick/QQuickWindow>

// The product's thin C++ shim (plan §3.6). Everything here is unreachable from
// Rust otherwise: QSurfaceFormat, QWindow::winId, the Win32 extended styles and
// the QQuickImageProvider subclass. The M0 spike (spike/hello-cxxqt) proved this
// exact route against Qt 6.10.1 msvc2022_64, so nothing new is being attempted
// here - only the spike's harness turned into the thing it was measuring.
//
// Pixels travel one way: Rust renders a pin through falcon_core::pin, encodes it
// with the `image` crate (plan §3.3 bans QImage::save for product bitmaps) and
// hands the PNG over. Qt only ever *decodes*.

/// QSurfaceFormat::setDefaultFormat with an 8-bit alpha buffer. Must run before
/// the first window or the pin windows come back with a black rectangle where
/// the transparent margin should be.
void enableAlphaBufferByDefault();

/// Chain Qt's own message handler and keep a copy of what it was told. A QML
/// document that fails to compile reports through that handler, which a console
/// run otherwise never shows; without this, `--probe` can only see that no
/// window appeared.
void pinInstallMessageCapture();
/// Everything captured so far, newest last, at most 64 lines.
QString pinMessages();

/// The rendered PNG of one pin, keyed by id. Replaces any previous frame.
void pinStoreFrame(std::int64_t id, const QByteArray &png);
/// A pin left the screen: drop its pixels rather than leak them.
void pinDropFrame(std::int64_t id);

/// Decode report for the stored frame - `--selftest` asserts on this string, so
/// the render → encode → store → decode round trip is checked without a screen.
QString pinSelfCheck(std::int64_t id);

/// One pixel of the stored frame as "r,g,b,a", or "oob". This is how §5.9.15's
/// alpha board and the transparent margin get asserted instead of eyeballed.
QString pinPixel(std::int64_t id, std::int32_t x, std::int32_t y);

/// The virtual desktop and the primary screen, in device pixels. §5.9.1's
/// "keep some of the pin on screen" needs real geometry, not a guess.
std::int32_t pinDesktopX();
std::int32_t pinDesktopY();
std::int32_t pinDesktopW();
std::int32_t pinDesktopH();
std::int32_t pinPrimaryX();
std::int32_t pinPrimaryY();
std::int32_t pinPrimaryW();
std::int32_t pinPrimaryH();
/// Device pixel ratio of the primary screen, so the physical↔logical conversion
/// in QML can be verified from the console instead of by eye.
double pinDevicePixelRatio();

/// className / visible / opacity / alphaBufferSize / hwnd / GWL_STYLE / GWL_EXSTYLE
/// for every top-level window. This is the evidence that frameless + alpha +
/// StaysOnTop actually landed on the native window.
QString pinWindowReport();

/// Quit the event loop after ms, so a probe run cannot hang a CI job.
void pinQuitAfter(std::int32_t ms);

/// Installs the "falconshot" image provider on the engine that owns the window
/// QML passes in, and applies the native styles. Both are Q_INVOKABLE rather
/// than cxx functions because a QWindow* only exists on the C++ side of a QML
/// object.
class PinShim : public QObject
{
    Q_OBJECT
    QML_ELEMENT
public:
    explicit PinShim(QObject *parent = nullptr) : QObject(parent) {}

    /// 1 when the provider was added by this call, 0 when it was already there.
    Q_INVOKABLE int install(QQuickWindow *window);

    /// The three styles §5.9 asks a pin for. Qt already gives frameless and
    /// tool-window through Window.flags; WS_EX_TRANSPARENT is the one Qt has no
    /// property for, and topmost has to survive a SetWindowPos.
    Q_INVOKABLE void applyStyle(QQuickWindow *window, bool topmost, bool clickThrough);

    /// The same field dump as pinWindowReport(), for one window.
    Q_INVOKABLE QString describe(QQuickWindow *window);
};

class PinImageProvider : public QQuickImageProvider
{
public:
    PinImageProvider();

    QImage requestImage(const QString &id, QSize *size, const QSize &requestedSize) override;
};
