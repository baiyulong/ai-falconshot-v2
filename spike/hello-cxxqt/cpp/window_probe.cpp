// SPDX-License-Identifier: MIT OR Apache-2.0
#include "window_probe.h"

#include <QtGui/QGuiApplication>
#include <QtGui/QSurfaceFormat>
#include <QtGui/QWindow>

#ifdef Q_OS_WIN
#include <windows.h>
#endif

static QString describeWindow(QWindow *window)
{
    QString out = QString::fromLatin1("\n  class=%1")
                      .arg(QString::fromLatin1(window->metaObject()->className()));
    out += QString::fromLatin1(" title=%1").arg(window->title());
    out += QString::fromLatin1(" visible=%1").arg(window->isVisible() ? 1 : 0);
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
    out += QString::fromLatin1(" LAYERED=%1").arg((exStyle & WS_EX_LAYERED) ? 1 : 0);
    out += QString::fromLatin1(" TRANSPARENT=%1").arg((exStyle & WS_EX_TRANSPARENT) ? 1 : 0);
    out += QString::fromLatin1(" TOPMOST=%1").arg((exStyle & WS_EX_TOPMOST) ? 1 : 0);
    out += QString::fromLatin1(" TOOLWINDOW=%1").arg((exStyle & WS_EX_TOOLWINDOW) ? 1 : 0);
#endif
    return out;
}

QString probeAllWindows()
{
    const auto windows = QGuiApplication::topLevelWindows();
    QString out = QString::fromLatin1("topLevelWindows=%1").arg(windows.size());
    for (QWindow *window : windows) {
        out += describeWindow(window);
    }
    return out;
}

void enableAlphaBufferByDefault()
{
    // Qt 6.10 note: setAlphaBuffer(bool) and QWindow::supportsAlphaBuffer() are gone;
    // the format now asks for a bit depth.
    QSurfaceFormat format = QSurfaceFormat::defaultFormat();
    format.setAlphaBufferSize(8);
    QSurfaceFormat::setDefaultFormat(format);
}
