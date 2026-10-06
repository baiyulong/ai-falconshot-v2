use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    // The recipe M0 proved, applied for real: one QML module holding the two
    // documents, the two cxx-qt bridges, and the thin C++ shim nothing else can
    // be (QQuickImageProvider, QWindow::winId, the Win32 extended styles).
    CxxQtBuilder::new_qml_module(
        QmlModule::new("dev.falconshot")
            .qml_file("qml/main.qml")
            .qml_file("qml/PinWindow.qml"),
    )
    .files(["src/session.rs", "src/pin_view.rs"])
    .include_dir("cpp")
    .cpp_files(["cpp/pin_shim.cpp", "cpp/pin_shim.h"])
    .qt_module("Gui")
    .qt_module("Quick")
    .qt_module("Qml")
    .build();

    // A .cpp_file shim that calls Win32 gets no import lib for free under a
    // Cargo-led build (CMake's target_link_libraries would have handled it).
    println!("cargo:rustc-link-lib=user32");
}
