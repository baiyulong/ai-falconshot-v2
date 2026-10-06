mod mask_probe;
mod overlay_probe;
mod probe;
mod pump;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    println!("[rust] main entered");

    // Must precede the first window, and QSurfaceFormat isn't bound in cxx-qt-lib.
    probe::qobject::enable_alpha_buffer_by_default();
    let mut app = QGuiApplication::new();

    // P5: headless capability dump. No QML engine, no window, exits.
    if std::env::args().any(|a| a == "--audit") {
        println!("[shim]\n{}", probe::qobject::audit_qt_capabilities().to_string());
        return;
    }

    let mut engine = QQmlApplicationEngine::new();
    println!(
        "[rust] app is_null: {}  engine is_null: {}",
        app.is_null(),
        engine.is_null()
    );

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from(
            "qrc:/qt/qml/dev/falconshot/spike/qml/main.qml",
        ));
        println!("[rust] engine.load() returned");
    }

    if let Some(app) = app.as_mut() {
        println!("[rust] entering exec()");
        app.exec();
        println!("[rust] exec() returned");
    }
}
