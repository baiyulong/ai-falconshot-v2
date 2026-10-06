#pragma once

#include <cstdint>

#include <QtCore/QObject>
#include <QtCore/QString>
#include <QtQml/QQmlImageProviderBase>
#include <QtQml/qqml.h>
#include <QtQuick/QQuickImageProvider>
#include <QtQuick/QQuickWindow>

// P1 harness: one frozen desktop frame, served to QML as a texture.
//
// Everything here is unreachable from Rust otherwise: QQuickImageProvider has to
// be subclassed in C++, BitBlt is Win32, and the QQmlEngine that owns the provider
// is only reachable from a QML-created object (qmlEngine(window)). Plan 3.6 shim.

/// BitBlt the whole virtual desktop into the shared frame. Returns microseconds.
int64_t frozenCapture();

/// Fabricate a frame of the given size, so the 4K texture path can be measured on a
/// machine whose panel is not 4K. Returns microseconds.
int64_t frozenSynthesize(int32_t width, int32_t height);

int32_t frozenWidth();
int32_t frozenHeight();

/// Mean luma inside vs outside a device-pixel rect of the last captured frame. Used
/// by P1 to prove the dim layer really paints where the code thinks it does.
/// GDI-only, and therefore blind to this window's own swapchain content.
QString frozenSelfCheck(int32_t x0, int32_t y0, int32_t x1, int32_t y1);

/// The same measurement on what the scene graph actually rendered: grabs the mask
/// window, compares it against the pre-mask frame, and writes mask-grab.png.
/// GUI thread only.
QString frozenWindowCheck(int32_t x0, int32_t y0, int32_t x1, int32_t y1);

/// Did the QML side manage to install the provider at all?
bool frozenProviderInstalled();

/// Cost of the provider callback itself (mutex + shallow QImage copy).
int64_t frozenProviderCalls();
int64_t frozenProviderAvgUs();
int64_t frozenProviderWorstUs();

/// Screen metrics, DPI awareness, captured frame geometry.
QString frozenDescribe();

/// Registered into dev.falconshot.spike by cxx-qt-build: moc writes the metatypes
/// json, qmltyperegistrar turns it into the module's registration file. The old
/// qmlRegisterType route is rejected here ("namespace already used for type
/// registration"), because the URI is already a static QML module.
class FrozenInstaller : public QObject
{
    Q_OBJECT
    QML_ELEMENT
public:
    explicit FrozenInstaller(QObject *parent = nullptr) : QObject(parent) {}

    /// Installs the "frozen" image provider on the engine that created @a window.
    /// QML calls this as installer.install(root).
    Q_INVOKABLE int install(QQuickWindow *window);

    /// Registers the window frozenWindowCheck() reads back. QML calls this with the
    /// mask window, which is the one whose pixels P1 has to verify.
    Q_INVOKABLE void setWindow(QQuickWindow *window);
};

class FrozenProvider : public QQuickImageProvider
{
public:
    FrozenProvider();

    QImage requestImage(const QString &id, QSize *size, const QSize &requestedSize) override;
};
