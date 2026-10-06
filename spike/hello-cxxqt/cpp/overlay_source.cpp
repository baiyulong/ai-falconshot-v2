// SPDX-License-Identifier: MIT OR Apache-2.0
#include "overlay_source.h"

#include <cstring>
#include <vector>

#include <QtCore/QElapsedTimer>
#include <QtCore/QMutex>
#include <QtCore/QMutexLocker>
#include <QtCore/QRect>
#include <QtCore/QStringList>
#include <QtGui/QColor>
#include <QtGui/QPainter>
#include <QtGui/QPen>
#include <QtQml/QQmlEngine>
#include <QtQml/qqml.h>

// ---------------------------------------------------------------------------
// The published surface.
//
// Rust owns the pixels (a Vec<u8> in ARGB32 premultiplied order, B/G/R/A in
// memory) and rasterizes each 图元 into the dirty rect. C++ never copies the whole
// layer unless the request asks for it - which is precisely the difference P4 is
// set up to measure. The pointer is non-owning: the spike keeps the Vec alive for
// the whole run, which is exactly the lifetime shortcut plan 3.3 rule 2 forbids in
// production. Nothing here is thread-safe by design; the provider runs on the
// render thread and only reads.
// ---------------------------------------------------------------------------

namespace {

QMutex g_mutex;
const uint8_t *g_bits = nullptr;
int g_width = 0;
int g_height = 0;
bool g_installed = false;

int64_t g_calls = 0;
int64_t g_totalUs = 0;
int64_t g_worstUs = 0;
int64_t g_bytes = 0;

QImage surfaceImage()
{
    if (g_bits == nullptr) {
        return QImage();
    }
    return QImage(g_bits, g_width, g_height, g_width * 4, QImage::Format_ARGB32_Premultiplied);
}

/// The same 图元 either way: a 3 px outline plus one diagonal, so the Rust
/// rasterizer and the QPainter fallback draw comparable work.
void drawMarker(QPainter &painter, const QRectF &area)
{
    painter.setPen(QPen(QColor(255, 40, 40, 255), 3.0));
    painter.drawRect(area.adjusted(2, 2, -2, -2));
    painter.setPen(QPen(QColor(40, 120, 255, 255), 2.0));
    painter.drawLine(area.topLeft(), area.bottomRight());
}

} // namespace

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

OverlayProvider::OverlayProvider()
    : QQuickImageProvider(QQuickImageProvider::Image)
{
}

QImage OverlayProvider::requestImage(const QString &id, QSize *size, const QSize &requestedSize)
{
    Q_UNUSED(requestedSize)

    QElapsedTimer timer;
    timer.start();

    // id = "token/x/y/w/h"; w or h of 0 means "the whole layer".
    const QStringList parts = id.split(QLatin1Char('/'));
    QRect rect;
    if (parts.size() >= 5) {
        rect = QRect(parts.at(1).toInt(), parts.at(2).toInt(), parts.at(3).toInt(), parts.at(4).toInt());
    }

    QImage image;
    qint64 bytes = 0;
    {
        QMutexLocker lock(&g_mutex);
        const QImage full = surfaceImage();
        if (!full.isNull()) {
            if (rect.width() > 0 && rect.height() > 0 && full.rect().contains(rect)) {
                // Dirty-rect path: a deep copy of just that area, so the uploaded
                // texture really is rect-sized and the memory is Qt-owned.
                image = full.copy(rect);
            } else {
                // Whole-layer path: shallow, refcounted - what a plain QML Image
                // re-request costs.
                image = full;
            }
            bytes = static_cast<qint64>(image.sizeInBytes());
        }
    }

    if (size != nullptr) {
        *size = image.size();
    }

    const qint64 us = timer.nsecsElapsed() / 1000;
    QMutexLocker lock(&g_mutex);
    ++g_calls;
    g_totalUs += us;
    g_bytes += bytes;
    if (us > g_worstUs) {
        g_worstUs = us;
    }
    return image;
}

// ---------------------------------------------------------------------------
// Painted fallback
// ---------------------------------------------------------------------------

PaintedOverlay::PaintedOverlay(QQuickItem *parent)
    : QQuickPaintedItem(parent)
{
    // Default (Image) mode: CPU raster into an item-sized QImage, then upload the
    // whole thing. That is the behaviour plan 3.5(2) calls the slower alternative,
    // so it is left at the default on purpose.
    setRenderTarget(Image);
}

void PaintedOverlay::setSeq(int seq)
{
    if (m_seq == seq) {
        return;
    }
    m_seq = seq;
    emit seqChanged();
    update(); // schedules the repaint; paint() runs later in the frame's sync phase
}

void PaintedOverlay::setMarker(const QRectF &marker)
{
    if (m_marker == marker) {
        return;
    }
    m_marker = marker;
    emit markerChanged();
}

void PaintedOverlay::paint(QPainter *painter)
{
    // The whole item is cleared and re-rastered on every update() - that full-item
    // CPU pass plus the full upload is exactly the cost plan 3.5(2) prices, so it is
    // left unoptimised on purpose. The geometry drawn is one 图元, same as the
    // variants that rasterize in Rust.
    painter->fillRect(QRectF(QPointF(0, 0), QSizeF(width(), height())), Qt::transparent);
    if (!m_marker.isEmpty()) {
        drawMarker(*painter, m_marker);
    }
}

// ---------------------------------------------------------------------------
// Installer + published surface + describe
// ---------------------------------------------------------------------------

int OverlayInstaller::install(QQuickWindow *window)
{
    if (window == nullptr) {
        return 0;
    }
    QQmlEngine *engine = qmlEngine(window);
    if (engine == nullptr) {
        return 0;
    }
    engine->addImageProvider(QStringLiteral("overlay"), new OverlayProvider);
    g_installed = true;
    return 1;
}

void overlayPublish(const uint8_t *bits, int32_t width, int32_t height)
{
    QMutexLocker lock(&g_mutex);
    g_bits = bits;
    g_width = width;
    g_height = height;
}

int64_t overlayProviderCalls()
{
    QMutexLocker lock(&g_mutex);
    return g_calls;
}

int64_t overlayProviderAvgUs()
{
    QMutexLocker lock(&g_mutex);
    return g_calls > 0 ? g_totalUs / g_calls : 0;
}

int64_t overlayProviderWorstUs()
{
    QMutexLocker lock(&g_mutex);
    return g_worstUs;
}

int64_t overlayProviderBytes()
{
    QMutexLocker lock(&g_mutex);
    return g_bytes;
}

QString overlayDescribe()
{
    qint64 layerBytes = 0;
    int width = 0;
    int height = 0;
    {
        QMutexLocker lock(&g_mutex);
        width = g_width;
        height = g_height;
        layerBytes = static_cast<qint64>(width) * static_cast<qint64>(height) * 4;
    }
    return QStringLiteral(" overlayCanvas=%1x%2 layerBytes=%3MB providerInstalled=%4")
        .arg(width)
        .arg(height)
        .arg(static_cast<double>(layerBytes) / (1024.0 * 1024.0), 0, 'f', 1)
        .arg(g_installed ? 1 : 0);
}
