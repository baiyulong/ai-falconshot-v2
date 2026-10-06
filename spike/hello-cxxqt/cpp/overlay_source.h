#pragma once

#include <cstdint>

#include <QtCore/QObject>
#include <QtCore/QRectF>
#include <QtCore/QString>
#include <QtGui/QImage>
#include <QtQuick/QQuickImageProvider>
#include <QtQuick/QQuickPaintedItem>
#include <QtQuick/QQuickWindow>
#include <QtQml/qqml.h>

// The P4 shim: the annotation overlay layer.
//
// Rust owns the overlay pixels and rasterizes 图元 into them; C++ only publishes a
// non-owning view so QML can pull it. That split is deliberate - plan 3.3 rule 2
// says the buffer is immutable once published, and P4's job is to price the pull.

/// Publish the Rust-owned overlay surface. Non-owning: Rust keeps the Vec alive.
void overlayPublish(const uint8_t *bits, int32_t width, int32_t height);

/// Provider statistics, so "how many bytes actually went to the GPU" is auditable.
int64_t overlayProviderCalls();
int64_t overlayProviderAvgUs();
int64_t overlayProviderWorstUs();
int64_t overlayProviderBytes();

/// Canvas size and one line of env text, for the report.
QString overlayDescribe();

/// The QQuickPaintedItem fallback that plan 3.5(2) owes a measured comparison:
/// whole-item CPU raster + full upload, every update.
class PaintedOverlay : public QQuickPaintedItem {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(int seq READ seq WRITE setSeq NOTIFY seqChanged)
    /// The 图元 in item coordinates. Bound from QML straight out of the probe's
    /// committed rect, so the QPainter path draws exactly what the Rust path did.
    Q_PROPERTY(QRectF marker READ marker WRITE setMarker NOTIFY markerChanged)
public:
    explicit PaintedOverlay(QQuickItem *parent = nullptr);

    int seq() const { return m_seq; }
    void setSeq(int seq);

    QRectF marker() const { return m_marker; }
    void setMarker(const QRectF &marker);

    void paint(QPainter *painter) override;

signals:
    void seqChanged();
    void markerChanged();

private:
    int m_seq = 0;
    QRectF m_marker;
};

/// Registered into dev.falconshot.spike by cxx-qt-build (QML_ELEMENT, same route as
/// the P1 installer - qmlRegisterType is rejected for a Cargo-built module URI).
class OverlayInstaller : public QObject {
    Q_OBJECT
    QML_ELEMENT
public:
    explicit OverlayInstaller(QObject *parent = nullptr) : QObject(parent) {}
    Q_INVOKABLE int install(QQuickWindow *window);
};

class OverlayProvider : public QQuickImageProvider {
public:
    OverlayProvider();
    QImage requestImage(const QString &id, QSize *size, const QSize &requestedSize) override;
};
