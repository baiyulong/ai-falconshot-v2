// SPDX-License-Identifier: MIT OR Apache-2.0
#include "frozen_source.h"

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>

#include <cstring>
#include <vector>

#include <QtCore/QByteArray>
#include <QtCore/QElapsedTimer>
#include <QtCore/QMutex>
#include <QtCore/QMutexLocker>
#include <QtGui/QGuiApplication>
#include <QtGui/QImage>
#include <QtGui/QScreen>
#include <QtQml/QQmlEngine>
#include <QtQml/qqml.h>

// ---------------------------------------------------------------------------
// The frame buffer.
//
// BitBlt writes straight into the memory the QImage already points at, through a
// DIB section: no GetDIBits, no second copy. Two buffers alternate so the frame QML
// still holds a shallow reference to is never the one being captured into. When the
// size changes the old buffer is retired rather than freed, because a QImage built
// over foreign memory has no ownership and QML may still be holding the pixels.
// That is the lifetime cost plan 3.3 rule 2 talks about, measured here.
// ---------------------------------------------------------------------------

namespace {

struct Frame {
    HBITMAP bitmap = nullptr;
    HDC dc = nullptr;
    uint8_t *bits = nullptr;
    QImage image;
    int width = 0;
    int height = 0;
};

QQuickWindow *g_maskWindow = nullptr;

QMutex g_mutex;
Frame *g_slots[2] = { nullptr, nullptr };
int g_flip = 0;
std::vector<Frame *> g_retired;
int64_t g_providerCalls = 0;
int64_t g_providerTotalUs = 0;
int64_t g_providerWorstUs = 0;
bool g_providerInstalled = false;

Frame *makeFrame(int width, int height)
{
    auto *frame = new Frame;
    frame->width = width;
    frame->height = height;

    BITMAPINFO info{};
    info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height; // top-down, matching QImage row order
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;

    HDC screen = GetDC(nullptr);
    void *bits = nullptr;
    frame->bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &bits, nullptr, 0);
    if (frame->bitmap == nullptr || bits == nullptr) {
        ReleaseDC(nullptr, screen);
        delete frame;
        return nullptr;
    }
    frame->bits = static_cast<uint8_t *>(bits);
    frame->dc = CreateCompatibleDC(screen);
    ReleaseDC(nullptr, screen);
    if (frame->dc == nullptr) {
        DeleteObject(frame->bitmap);
        delete frame;
        return nullptr;
    }
    SelectObject(frame->dc, frame->bitmap);
    frame->image = QImage(frame->bits, width, height, width * 4, QImage::Format_ARGB32);
    return frame;
}

/// The buffer the last capture filled. Callers hold g_mutex.
Frame *currentFrame()
{
    return g_slots[g_flip];
}

/// Next buffer to capture into, (re)allocated when the size changed. The other slot
/// keeps whatever frame QML is still displaying, so the two never alias.
Frame *acquireFrame(int width, int height)
{
    g_flip ^= 1;
    QMutexLocker lock(&g_mutex);
    Frame *&slot = g_slots[g_flip];
    if (slot != nullptr && (slot->width != width || slot->height != height)) {
        g_retired.push_back(slot); // never freed while QML may read the pixels
        slot = nullptr;
    }
    if (slot == nullptr) {
        slot = makeFrame(width, height);
    }
    return slot;
}

constexpr int kStep = 4;   // sampled pixels between measurements, in shot pixels
constexpr int kBand = 160; // device px outside the hole searched for the control

struct Regions {
    double ratio[3] = { -1.0, -1.0, -1.0 };
    double luma[3] = { -1.0, -1.0, -1.0 };
    qint64 count[3] = { 0, 0, 0 };
    bool valid = false;
};

/// Format_ARGB32 on a little-endian machine is B, G, R, A.
double lumaOf(const uint8_t *p)
{
    return 0.114 * p[0] + 0.587 * p[1] + 0.299 * p[2];
}

/// Mean luma of @a shot over mean luma of @a base, over three regions: 0 = inside the
/// hole (given in base pixels), 1 = the band around it, 2 = everywhere else.
///
/// Comparing against a baseline of the *same* pixels instead of against an absolute
/// brightness is what makes this usable: a photograph varies far more across a desktop
/// than over 160 px, which is what made the first version of this instrument lie.
/// Callers hold g_mutex.
Regions measureAgainst(const QImage &shot, const Frame *base, int x0, int y0, int x1, int y1)
{
    Regions out;
    if (shot.isNull() || shot.format() != QImage::Format_ARGB32 || base == nullptr) {
        return out;
    }
    const double mapX = double(base->width) / shot.width();
    const double mapY = double(base->height) / shot.height();
    if (mapX <= 0.0 || mapY <= 0.0) {
        return out;
    }
    const int bx0 = qMax(0, x0 - kBand), by0 = qMax(0, y0 - kBand);
    const int bx1 = qMin(base->width, x1 + kBand), by1 = qMin(base->height, y1 + kBand);

    double sumShot[3] = { 0.0, 0.0, 0.0 };
    double sumBase[3] = { 0.0, 0.0, 0.0 };
    for (int sy = 0; sy < shot.height(); sy += kStep) {
        const uint8_t *shotRow = shot.constBits()
            + static_cast<size_t>(sy) * static_cast<size_t>(shot.bytesPerLine());
        const int by = qMin(base->height - 1, int(sy * mapY));
        const uint8_t *baseRow = base->bits + static_cast<size_t>(by) * base->width * 4;
        for (int sx = 0; sx < shot.width(); sx += kStep) {
            const int bx = qMin(base->width - 1, int(sx * mapX));
            const int region = (bx >= x0 && bx < x1 && by >= y0 && by < y1)
                                 ? 0
                                 : (bx >= bx0 && bx < bx1 && by >= by0 && by < by1 ? 1 : 2);
            sumShot[region] += lumaOf(shotRow + static_cast<size_t>(sx) * 4);
            sumBase[region] += lumaOf(baseRow + static_cast<size_t>(bx) * 4);
            ++out.count[region];
        }
    }
    for (int i = 0; i < 3; ++i) {
        if (out.count[i] > 0 && sumBase[i] > 1.0) {
            out.ratio[i] = sumShot[i] / sumBase[i];
            out.luma[i] = sumShot[i] / out.count[i];
        }
    }
    out.valid = out.count[0] > 0 && out.count[1] > 0;
    return out;
}

/// One line of report. The dim is #99000000 over opaque pixels, so the ratio outside
/// the hole should land near 1 - 0x99/0xff = 0.40 and inside it near 1.00.
QString describeRegions(const QString &tag, const Regions &r, int x0, int y0, int x1, int y1)
{
    if (!r.valid) {
        return QStringLiteral("%1 unreadable (no frame, wrong format, or empty region)").arg(tag);
    }
    const bool ok = r.ratio[0] > 0.85 && r.ratio[0] < 1.15 && r.ratio[1] > 0.30 && r.ratio[1] < 0.55;
    return QStringLiteral("%1 hole=(%2,%3)-(%4,%5) hole=%6 band=%7 rest=%8 luma=%9/%10/%11 n=%12/%13/%14 %15")
        .arg(tag)
        .arg(x0).arg(y0).arg(x1).arg(y1)
        .arg(r.ratio[0], 0, 'f', 3)
        .arg(r.ratio[1], 0, 'f', 3)
        .arg(r.ratio[2], 0, 'f', 3)
        .arg(r.luma[0], 0, 'f', 1)
        .arg(r.luma[1], 0, 'f', 1)
        .arg(r.luma[2], 0, 'f', 1)
        .arg(r.count[0])
        .arg(r.count[1])
        .arg(r.count[2])
        .arg(ok ? QStringLiteral("DIM-OK") : QStringLiteral("DIM-WRONG"));
}

} // namespace

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

int64_t frozenCapture()
{
    QElapsedTimer timer;
    timer.start();

    const int x = GetSystemMetrics(SM_XVIRTUALSCREEN);
    const int y = GetSystemMetrics(SM_YVIRTUALSCREEN);
    const int width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
    const int height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    if (width <= 0 || height <= 0) {
        return -1;
    }

    Frame *frame = acquireFrame(width, height);
    if (frame == nullptr) {
        return -2;
    }

    HDC screen = GetDC(nullptr);
    BitBlt(frame->dc, 0, 0, width, height, screen, x, y, SRCCOPY);
    GdiFlush(); // the bits are not guaranteed visible to the CPU before this
    ReleaseDC(nullptr, screen);
    return timer.nsecsElapsed() / 1000;
}

int64_t frozenSynthesize(int32_t width, int32_t height)
{
    if (width <= 0 || height <= 0) {
        return -1;
    }
    QElapsedTimer timer;
    timer.start();

    Frame *frame = acquireFrame(width, height);
    if (frame == nullptr) {
        return -2;
    }

    // Touch every byte, with a per-row value so the write cannot be folded away.
    const int stride = width * 4;
    for (int row = 0; row < height; ++row) {
        std::memset(frame->bits + static_cast<size_t>(row) * static_cast<size_t>(stride),
                    row & 0xFF, static_cast<size_t>(stride));
    }
    return timer.nsecsElapsed() / 1000;
}

int32_t frozenWidth()
{
    QMutexLocker lock(&g_mutex);
    const Frame *frame = currentFrame();
    return frame != nullptr ? frame->width : 0;
}

int32_t frozenHeight()
{
    QMutexLocker lock(&g_mutex);
    const Frame *frame = currentFrame();
    return frame != nullptr ? frame->height : 0;
}

// P6: prove the dim layer really paints where the code thinks it does. Two
// instruments, because the first one turned out to be blind: a GDI BitBlt of the
// screen DC does not contain this window's own swapchain content, so it can only ever
// report ratio 1.000 everywhere. The scene-graph grab reads what Qt actually rendered.
// Beware while editing either: `near` and `far` are #defines in winnt.h, so they may
// not be used as identifiers anywhere in this translation unit.

/// Instrument 1: the last GDI capture against the pre-mask frame in the other slot.
QString frozenSelfCheck(int32_t x0, int32_t y0, int32_t x1, int32_t y1)
{
    QMutexLocker lock(&g_mutex);
    const Frame *shot = currentFrame();
    const Frame *base = g_slots[g_flip ^ 1];
    if (shot == nullptr || base == nullptr) {
        return describeRegions(QStringLiteral("gdi"), Regions(), x0, y0, x1, y1);
    }
    if (base->width != shot->width || base->height != shot->height) {
        return QStringLiteral("gdi baseline %1x%2 != shot %3x%4")
            .arg(base->width).arg(base->height).arg(shot->width).arg(shot->height);
    }
    const QImage image(shot->bits, shot->width, shot->height, shot->width * 4,
                       QImage::Format_ARGB32);
    return describeRegions(QStringLiteral("gdi shot=%1x%2")
                               .arg(shot->width).arg(shot->height),
                           measureAgainst(image, base, x0, y0, x1, y1), x0, y0, x1, y1);
}

/// Instrument 2: the mask window's own rendered pixels, same baseline. Runs on the GUI
/// thread only; grabWindow() re-renders the scene graph and blocks on the readback.
/// Also writes mask-grab.png so a human can see what the numbers describe.
QString frozenWindowCheck(int32_t x0, int32_t y0, int32_t x1, int32_t y1)
{
    if (g_maskWindow == nullptr) {
        return QStringLiteral("sg no mask window registered");
    }
    // Outside the mutex on purpose: rendering the scene graph may reach the image
    // provider, which takes g_mutex, and QMutex is not recursive.
    QImage grab = g_maskWindow->grabWindow();
    const QSize grabbed = grab.size();
    grab.convertTo(QImage::Format_ARGB32);
    const QString saved = grab.save(QStringLiteral("mask-grab.png"))
                            ? QStringLiteral("saved=mask-grab.png")
                            : QStringLiteral("saved=FAILED");
    // QImage::save is banned for product bitmap encoding (plan 3.3); this call is a
    // spike diagnostic artifact only, and stays inside spike/.

    QMutexLocker lock(&g_mutex);
    const Frame *base = g_slots[g_flip ^ 1];
    return describeRegions(QStringLiteral("sg grab=%1x%2 %3")
                               .arg(grabbed.width()).arg(grabbed.height()).arg(saved),
                           measureAgainst(grab, base, x0, y0, x1, y1), x0, y0, x1, y1);
}

void FrozenInstaller::setWindow(QQuickWindow *window)
{
    g_maskWindow = window;
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

FrozenProvider::FrozenProvider()
    : QQuickImageProvider(QQuickImageProvider::Image)
{
}

QImage FrozenProvider::requestImage(const QString &id, QSize *size, const QSize &requestedSize)
{
    Q_UNUSED(id)
    Q_UNUSED(requestedSize)

    QElapsedTimer timer;
    timer.start();

    QImage image;
    {
        QMutexLocker lock(&g_mutex);
        const Frame *frame = currentFrame();
        if (frame != nullptr) {
            image = frame->image; // refcounted shallow copy, no pixel memcpy
        }
    }
    if (size != nullptr) {
        *size = image.size();
    }

    const int64_t us = timer.nsecsElapsed() / 1000;
    g_providerCalls += 1;
    g_providerTotalUs += us;
    g_providerWorstUs = g_providerWorstUs < us ? us : g_providerWorstUs;
    return image;
}

int FrozenInstaller::install(QQuickWindow *window)
{
    if (window == nullptr) {
        return 0;
    }
    QQmlEngine *engine = qmlEngine(window);
    if (engine == nullptr) {
        return 0;
    }
    engine->addImageProvider(QStringLiteral("frozen"), new FrozenProvider);
    g_providerInstalled = true;
    return 1;
}

bool frozenProviderInstalled()
{
    return g_providerInstalled;
}

int64_t frozenProviderCalls()
{
    return g_providerCalls;
}

int64_t frozenProviderAvgUs()
{
    return g_providerCalls > 0 ? g_providerTotalUs / g_providerCalls : 0;
}

int64_t frozenProviderWorstUs()
{
    return g_providerWorstUs;
}

// ---------------------------------------------------------------------------
// Describing
// ---------------------------------------------------------------------------

QString frozenDescribe()
{
    QString out = QStringLiteral("virtualScreen=%1x%2@(%3,%4)")
                      .arg(GetSystemMetrics(SM_CXVIRTUALSCREEN))
                      .arg(GetSystemMetrics(SM_CYVIRTUALSCREEN))
                      .arg(GetSystemMetrics(SM_XVIRTUALSCREEN))
                      .arg(GetSystemMetrics(SM_YVIRTUALSCREEN));

    HDC screen = GetDC(nullptr);
    out += QStringLiteral(" logPixels=%1x%2")
               .arg(GetDeviceCaps(screen, LOGPIXELSX))
               .arg(GetDeviceCaps(screen, LOGPIXELSY));
    ReleaseDC(nullptr, screen);

    if (qApp != nullptr) {
        if (const QScreen *primary = qApp->primaryScreen()) {
            out += QStringLiteral(" qtGeom=%1x%2 dpr=%3 refresh=%4Hz")
                       .arg(primary->geometry().width())
                       .arg(primary->geometry().height())
                       .arg(primary->devicePixelRatio(), 0, 'f', 2)
                       .arg(primary->refreshRate(), 0, 'f', 1);
        }
    }

    const QByteArray rhi = qgetenv("QSG_RHI_BACKEND");
    const QByteArray quick = qgetenv("QT_QUICK_BACKEND");
    out += QStringLiteral(" frame=%1x%2 providerInstalled=%3 providerCalls=%4")
               .arg(frozenWidth())
               .arg(frozenHeight())
               .arg(g_providerInstalled ? 1 : 0)
               .arg(g_providerCalls);
    out += QStringLiteral(" rhi=%1 quick=%2")
               .arg(QString::fromLatin1(rhi.isEmpty() ? "default" : rhi.constData()))
               .arg(QString::fromLatin1(quick.isEmpty() ? "default" : quick.constData()));
    return out;
}
