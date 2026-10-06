#pragma once

#include <QtCore/QString>

// The spike's thin C++ shim. Nothing here is bound by cxx-qt-lib or qtbridge:
// QWindow, the native handle and the Win32 extended styles are all C++-only,
// which is exactly why plan section 3.6 declares a shim mandatory.
QString probeAllWindows();
void enableAlphaBufferByDefault();
