use cxx_qt_build::{CxxQtBuilder, QmlModule};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let shader_qrc = bake_shaders(Path::new(&out_dir));

    // The recipe M0 proved, applied for real: one QML module holding the three
    // documents, the three cxx-qt bridges, and the thin C++ shim nothing else can
    // be (QQuickImageProvider, QWindow::winId, the Win32 extended styles).
    CxxQtBuilder::new_qml_module(
        QmlModule::new("dev.falconshot")
            .qml_file("qml/main.qml")
            .qml_file("qml/PinWindow.qml")
            .qml_file("qml/CaptureMask.qml"),
    )
    .files(["src/session.rs", "src/pin_view.rs", "src/mask_view.rs"])
    .include_dir("cpp")
    .cpp_files(["cpp/pin_shim.cpp", "cpp/pin_shim.h"])
    .qt_module("Gui")
    .qt_module("Quick")
    .qt_module("Qml")
    .qrc(shader_qrc)
    .build();

    // A .cpp_file shim that calls Win32 gets no import lib for free under a
    // Cargo-led build (CMake's target_link_libraries would have handled it).
    println!("cargo:rustc-link-lib=user32");
}

/// Bakes the GLSL into `.qsb` and hands back a `.qrc` that puts the results next
/// to the QML that references them.
///
/// Failure is fatal, deliberately: a missing `qtshadertools` or a `qsb` that
/// errors produces a build that compiles, runs, and silently draws the mask
/// without the shader - `ShaderEffect.status` becomes `Error` at runtime, which
/// is exactly the "green CI, no shader in the package" hole plan §9.1 closes.
/// The spike returned `None` and warned; the product does not.
fn bake_shaders(out_dir: &Path) -> PathBuf {
    const SOURCES: [&str; 1] = ["dim_hole.frag"];
    let qsb = find_qsb().expect("qsb.exe: install the qtshadertools module (aqt install-qt ... -m qtshadertools) or point QMAKE at a Qt that has it");

    let mut files = Vec::new();
    for shader in SOURCES {
        let source = format!("shaders/{shader}");
        println!("cargo:rerun-if-changed={source}");
        let name = format!("{shader}.qsb");
        let baked = out_dir.join(&name);
        let status = Command::new(&qsb)
            .args(["--qt6", "-o"])
            .arg(&baked)
            .arg(&source)
            .status()
            .unwrap_or_else(|e| panic!("running {}: {e}", qsb.display()));
        if !status.success() {
            panic!("qsb failed for {source} ({status}) - the mask's ShaderEffect would be missing at runtime");
        }
        files.push((name, baked));
    }

    // main.qml is loaded from qrc:/qt/qml/dev/falconshot/qml/main.qml, so a
    // relative "dim_hole.frag.qsb" only resolves if the pack sits in that same
    // prefix - the one thing about this arrangement that is not guessable.
    let mut qrc = String::from("<RCC>\n  <qresource prefix=\"/qt/qml/dev/falconshot/qml\">\n");
    for (name, path) in files {
        let path = path.display().to_string().replace('\\', "/");
        qrc.push_str(&format!("    <file alias=\"{name}\">{path}</file>\n"));
    }
    qrc.push_str("  </qresource>\n</RCC>\n");

    let qrc_path = out_dir.join("shaders.qrc");
    std::fs::write(&qrc_path, &qrc).expect("writing shaders.qrc");
    qrc_path
}

/// `qsb` sits next to `qmake` (scripts/env.cmd exports QMAKE); fall back to
/// whatever is on PATH, which env.cmd also arranges.
fn find_qsb() -> Option<PathBuf> {
    if let Ok(qmake) = std::env::var("QMAKE") {
        let sibling = Path::new(&qmake).parent()?.join("qsb.exe");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    let on_path = PathBuf::from(if cfg!(windows) { "qsb.exe" } else { "qsb" });
    Some(on_path)
}
