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

/// One frozen monitor for the capture mask, as raw RGBA8.
///
/// A string key, not an id: `pinIdOf` reads anything it cannot parse as -1, so a
/// negative id is not addressable through the provider and a mask - which has no
/// id at all - cannot share the pin store. Two maps, one lookup order.
///
/// Raw rather than PNG-encoded because §3.3's rule is about *product bitmaps*
/// (files, clipboard), and this is a texture crossing from one module to another
/// inside one process. Encoding 3072x1920 to PNG and back to feed the GPU is pure
/// cost on the path the 150 ms promise is about.
void maskStoreRaw(const QString &key, const QByteArray &rgba, std::int32_t width, std::int32_t height);
/// The mask is closed: release the frame, which for a 4K desktop is 24 MB.
void maskDropFrame(const QString &key);
/// `key=.. null=.. w=.. h=.. stores=..`, for `--selftest` to assert on.
QString maskSelfCheck(const QString &key);

/// Qt's own screen list in device-independent pixels. A `Window`'s `x`/`y`/`width`
/// are in that space, so a mask placed from the Win32 enumeration would land in
/// the wrong pixels on any monitor that is not at 100% - the geometry has to come
/// from Qt. Index 0 is whatever Qt ordered first.
std::int32_t pinScreenCount();
QString pinScreenName(std::int32_t index);
std::int32_t pinScreenX(std::int32_t index);
std::int32_t pinScreenY(std::int32_t index);
std::int32_t pinScreenW(std::int32_t index);
std::int32_t pinScreenH(std::int32_t index);
double pinScreenDevicePixelRatio(std::int32_t index);

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

/// The primary screen as Qt can produce it: `QScreen::grabWindow` with no window
/// id and no offset, returned as raw RGBA8. This is the fourth leg of the R13
/// matrix - the one that asks Qt's own backend rather than Win32 - and it is
/// decode-only in both directions: the bytes leave here as pixels, never as an
/// encoded product bitmap (plan §3.3).
QByteArray pinScreenGrab();
/// What that grab actually produced, as `WxH dpr=R`. The size is reported rather
/// than assumed because the same call answers in logical pixels on some backends
/// and device pixels on others, and a matrix row has to say which it was.
QString pinScreenGrabInfo();

/// One line per top-level window: `hwnd=0x.. class=.. title=.. phys=x,y,w,h
/// visible=0/1`, the rect in *device* pixels from `GetWindowRect`. R13 has to
/// point `PrintWindow` at a window this process created, and it can only do that
/// from a handle - which the human-readable report below carries in a field
/// order that is a debugging aid, not a contract.
QString pinTopLevels();

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
