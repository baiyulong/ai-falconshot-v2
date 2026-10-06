use cxx_qt_build::{CxxQtBuilder, QmlModule};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let shader_qrc = bake_shaders(Path::new(&out_dir));

    let builder = CxxQtBuilder::new_qml_module(
        QmlModule::new("dev.falconshot.spike").qml_file("qml/main.qml"),
    )
    .files([
        "src/probe.rs",
        "src/pump.rs",
        "src/mask_probe.rs",
        "src/overlay_probe.rs",
    ])
    // The mandatory thin C++ shim: QWindow / winId / Win32 styles are not bound
    // by cxx-qt-lib, so anything that touches them has to come through here.
    // frozen_source.* covers the parts that cannot be Rust at all: a
    // QQuickImageProvider subclass and the QQmlEngine that owns it.
    // overlay_source.* is the P4 side of the same problem, plus the
    // QQuickPaintedItem fallback the plan owes a measured comparison for.
    .include_dir("cpp")
    .cpp_files([
        "cpp/window_probe.cpp",
        "cpp/window_probe.h",
        "cpp/frozen_source.cpp",
        "cpp/frozen_source.h",
        "cpp/overlay_source.cpp",
        "cpp/overlay_source.h",
        "cpp/audit_source.cpp",
        "cpp/audit_source.h",
    ])
    .qt_module("Gui")
    .qt_module("Quick")
    .qt_module("Qml");

    let builder = match shader_qrc {
        Some(qrc) => builder.qrc(qrc),
        None => builder,
    };
    builder.build();

    // A .cpp_file shim that calls Win32 gets no import lib for free under a Cargo-led
    // build (CMake's target_link_libraries would have handled it). Name them here.
    println!("cargo:rustc-link-lib=user32");
    println!("cargo:rustc-link-lib=gdi32");
}

/// P6: `qtshadertools` ships `qsb` but *not* `glslc`, and `qsb --qt6` bakes Vulkan
/// GLSL into a QShader pack on its own - so a Cargo-led build can compile shaders
/// without CMake. Returns the generated .qrc, or `None` when qsb is missing, in
/// which case the dim variant reports ShaderEffect.status=Error instead of the
/// whole build failing.
fn bake_shaders(out_dir: &Path) -> Option<PathBuf> {
    const SOURCES: [&str; 1] = ["shaders/dim_hole.frag"];
    let qsb = find_qsb()?;

    let mut files = Vec::new();
    for source in SOURCES {
        println!("cargo:rerun-if-changed={source}");
        let name = format!("{}.qsb", Path::new(source).file_name()?.to_str()?);
        let baked = out_dir.join(&name);
        let ok = Command::new(&qsb)
            .args(["--qt6", "-o"])
            .arg(&baked)
            .arg(source)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            println!("cargo:warning=qsb failed for {source}; P1 dim variant will report status=Error");
            return None;
        }
        files.push((name, baked));
    }

    // main.qml is loaded from qrc:/qt/qml/dev/falconshot/spike/qml/, so the shaders
    // have to sit in that prefix for a relative "dim_hole.frag.qsb" to resolve.
    let mut qrc = String::from("<RCC>\n  <qresource prefix=\"/qt/qml/dev/falconshot/spike/qml\">\n");
    for (name, path) in files {
        let path = path.display().to_string().replace('\\', "/");
        qrc.push_str(&format!("    <file alias=\"{name}\">{path}</file>\n"));
    }
    qrc.push_str("  </qresource>\n</RCC>\n");

    let qrc_path = out_dir.join("shaders.qrc");
    std::fs::write(&qrc_path, qrc).ok()?;
    Some(qrc_path)
}

/// qsb sits next to qmake (env.cmd exports QMAKE); fall back to whatever is on PATH.
fn find_qsb() -> Option<PathBuf> {
    if let Ok(qmake) = std::env::var("QMAKE") {
        let sibling = Path::new(&qmake).parent()?.join("qsb.exe");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    Some(PathBuf::from(if cfg!(windows) { "qsb.exe" } else { "qsb" }))
}
