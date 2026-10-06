// SPDX-License-Identifier: MIT OR Apache-2.0
#include "audit_source.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QDir>
#include <QtCore/QFileInfo>
#include <QtCore/QLibraryInfo>
#include <QtCore/QStringList>
#include <QtGui/QGuiApplication>
#include <QtGui/QImageReader>
#include <QtGui/QImageWriter>

namespace {

QString joinFormats(const QList<QByteArray> &formats)
{
    QStringList list;
    list.reserve(formats.size());
    for (const QByteArray &format : formats) {
        list << QString::fromLatin1(format.toUpper());
    }
    list.sort(Qt::CaseInsensitive);
    return list.join(QLatin1Char(','));
}

QStringList uniq(const QStringList &dirs)
{
    QStringList out;
    for (const QString &dir : dirs) {
        if (!dir.isEmpty() && !out.contains(dir)) {
            out << dir;
        }
    }
    return out;
}

// QLibraryInfo describes the *configured* layout, which for a windeployqt output is
// fiction: it reports <appdir>/plugins, <appdir>/bin and <appdir>/lib, and windeployqt
// creates none of them -- it flattens each plugin type to <appdir>/<type> and drops the
// DLLs next to the exe. A single-path probe therefore reads a healthy deployment as
// crippled, which is how a false capability negative gets laundered into a plan.
QStringList pluginRoots()
{
    QStringList roots;
    roots << QCoreApplication::applicationDirPath();
    roots << QCoreApplication::libraryPaths();
    roots << QLibraryInfo::path(QLibraryInfo::PluginsPath);
    return uniq(roots);
}

QStringList binaryRoots()
{
    const QString app = QCoreApplication::applicationDirPath();
    QStringList roots;
    roots << app;
    roots << app + QStringLiteral("/bin");
    roots << app + QStringLiteral("/lib");
    roots << QLibraryInfo::path(QLibraryInfo::BinariesPath);
    roots << QLibraryInfo::path(QLibraryInfo::LibrariesPath);
    return uniq(roots);
}

QStringList qmlRoots()
{
    QStringList roots;
    roots << QCoreApplication::applicationDirPath() + QStringLiteral("/qml");
    roots << QLibraryInfo::path(QLibraryInfo::QmlImportsPath);
    return uniq(roots);
}

// Report every root that actually has the directory, not the first one: during a run
// against a live Qt install both the app dir and the install are present, and which one
// answered is itself the fact worth logging.
QString dirListingAny(const QStringList &roots, const QString &relDir)
{
    QStringList hits;
    for (const QString &root : roots) {
        QDir dir(root + QLatin1Char('/') + relDir);
        if (!dir.exists()) {
            continue;
        }
        const QStringList entries = dir.entryList(QDir::Files, QDir::Name);
        hits << QStringLiteral("%1[%2]")
                    .arg(QDir::toNativeSeparators(dir.absolutePath()),
                         entries.isEmpty() ? QStringLiteral("(empty)")
                                           : entries.join(QLatin1Char(' ')));
    }
    return hits.isEmpty() ? QStringLiteral("NO(not in any root)") : hits.join(QStringLiteral(" ;; "));
}

QString subdirsAny(const QStringList &roots, const QString &relDir)
{
    QStringList hits;
    for (const QString &root : roots) {
        QDir dir(root + QLatin1Char('/') + relDir);
        if (!dir.exists()) {
            continue;
        }
        const QStringList entries = dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot, QDir::Name);
        hits << QStringLiteral("%1[%2]")
                    .arg(QDir::toNativeSeparators(dir.absolutePath()),
                         entries.isEmpty() ? QStringLiteral("(empty)")
                                           : entries.join(QLatin1Char(' ')));
    }
    return hits.isEmpty() ? QStringLiteral("NO(not in any root)") : hits.join(QStringLiteral(" ;; "));
}

QString exists(const QString &path)
{
    return QFileInfo::exists(path) ? QStringLiteral("yes") : QStringLiteral("NO");
}

// A shared Windows install puts DLLs under BinariesPath, not LibrariesPath, so a
// single-path probe reports "NO" for components that are actually installed.
QString existsAny(const QStringList &dirs, const QString &file)
{
    for (const QString &dir : dirs) {
        if (QFileInfo::exists(dir + QLatin1Char('/') + file)) {
            return QStringLiteral("yes@%1").arg(dir);
        }
    }
    return QStringLiteral("NO");
}

} // namespace

QString auditQtCapabilities()
{
    const QStringList pRoots = pluginRoots();
    const QStringList bRoots = binaryRoots();
    const QStringList qRoots = qmlRoots();

    QStringList out;
    out << QStringLiteral("runtime_qt=%1").arg(QString::fromLatin1(qVersion()));
    out << QStringLiteral("built_qt=%1").arg(QStringLiteral(QT_VERSION_STR));
    out << QStringLiteral("platform_name=%1").arg(QGuiApplication::platformName());

    // Self-identification: a capability log that cannot say which layout it was read
    // from is indistinguishable from a stale-binary log (P5 rule).
    out << QStringLiteral("app_dir=%1").arg(QCoreApplication::applicationDirPath());
    out << QStringLiteral("plugins_path=%1").arg(QLibraryInfo::path(QLibraryInfo::PluginsPath));
    out << QStringLiteral("qml_imports_path=%1").arg(QLibraryInfo::path(QLibraryInfo::QmlImportsPath));
    out << QStringLiteral("library_paths=%1").arg(QCoreApplication::libraryPaths().join(QLatin1Char(' ')));

    // PRD 5.8.3 promises reading PNG/JPG/BMP/TGA/ICO/TIFF/GIF; the export dialog
    // promises writing at least PNG/JPG/BMP. Both are plugin-gated, not version-gated.
    out << QStringLiteral("reader_formats=%1").arg(joinFormats(QImageReader::supportedImageFormats()));
    out << QStringLiteral("writer_formats=%1").arg(joinFormats(QImageWriter::supportedImageFormats()));
    for (const char *needle : {"png", "jpg", "jpeg", "bmp", "tga", "ico", "tif", "tiff", "gif", "webp", "svg", "qoi"}) {
        const QByteArray format(needle);
        const bool r = QImageReader::supportedImageFormats().contains(format);
        const bool w = QImageWriter::supportedImageFormats().contains(format);
        out << QStringLiteral("fmt_%1=reader=%2 writer=%3").arg(needle).arg(r).arg(w);
    }

    // Plugin directories are <root>/imageformats in a deployment and <install>/plugins/imageformats
    // in an install, so the relative part must not carry the "plugins" prefix.
    out << QStringLiteral("imageformats_dir=%1").arg(dirListingAny(pRoots, QStringLiteral("imageformats")));
    out << QStringLiteral("platforms_dir=%1").arg(dirListingAny(pRoots, QStringLiteral("platforms")));
    out << QStringLiteral("install_plugin_subdirs=%1")
               .arg(subdirsAny({QLibraryInfo::path(QLibraryInfo::PluginsPath)}, QString()));
    out << QStringLiteral("qml_top_level=%1").arg(subdirsAny(qRoots, QString()));

    // P1's outstanding tail: the ShaderEffect variant needs qsb generation. These are
    // build-host tools; an empty answer in a deployed app is the correct answer, not a gap.
    out << QStringLiteral("glslc_exe=%1").arg(existsAny(bRoots, QStringLiteral("glslc.exe")));
    out << QStringLiteral("qsb_exe=%1").arg(existsAny(bRoots, QStringLiteral("qsb.exe")));
    out << QStringLiteral("shader_tools_dll=%1").arg(existsAny(bRoots, QStringLiteral("Qt6ShaderTools.dll")));
    out << QStringLiteral("quick3d_dll=%1").arg(existsAny(bRoots, QStringLiteral("Qt6Quick3D.dll")));
    out << QStringLiteral("svg_dll=%1").arg(existsAny(bRoots, QStringLiteral("Qt6Svg.dll")));
    out << QStringLiteral("svg_plugin_dll=%1")
               .arg(existsAny(pRoots, QStringLiteral("imageformats/qsvg.dll")));
    out << QStringLiteral("tiff_plugin_dll=%1")
               .arg(existsAny(pRoots, QStringLiteral("imageformats/qtiff.dll")));
    out << QStringLiteral("webp_plugin_dll=%1")
               .arg(existsAny(pRoots, QStringLiteral("imageformats/qwebp.dll")));
    out << QStringLiteral("gif_plugin_dll=%1")
               .arg(existsAny(pRoots, QStringLiteral("imageformats/qgif.dll")));
    out << QStringLiteral("tga_plugin_dll=%1")
               .arg(existsAny(pRoots, QStringLiteral("imageformats/qtga.dll")));

    return out.join(QLatin1Char('\n'));
}
