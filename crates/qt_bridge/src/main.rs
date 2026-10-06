//! AI Falconshot - the process that owns the pin windows.
//!
//! Three modes, because "does it work" has three different answers:
//! * `--selftest` drives the state machine and the pixel pipeline and prints
//!   PASS/FAIL per step, without ever creating a window.
//! * `--probe <ms>` shows the windows for real, prints what the Win32 side of
//!   them actually looks like after ms, then quits. Frameless, per-pixel alpha and
//!   StaysOnTop are checked as numbers rather than by someone squinting.
//! * no flags: the app.

mod pin_view;
mod session;
mod state;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    // Before anything Qt has to say: a QML document that will not compile says so
    // only through the message handler, and a headless run would otherwise see a
    // silent zero.
    state::install_message_capture();

    // Before the first window, or the pins come back with black corners.
    session::qobject::enable_alpha_buffer_by_default();
    let mut app = QGuiApplication::new();
    if app.is_null() {
        eprintln!("[falconshot] could not create QGuiApplication");
        std::process::exit(2);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let after = |name: &str| -> Option<i32> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
    };

    if flag("--selftest") {
        let (ok, report) = state::selftest();
        println!("{report}");
        // The clipboard checks take the user's own clipboard over, so they run
        // when asked for and are reported as skipped otherwise.
        let mut code = if ok { 0 } else { 1 };
        if flag("--clipboard") {
            let (verdict, creport) = state::clipboard_selftest();
            println!("{creport}");
            // 1 is this code failing, 3 is the machine refusing to let it be
            // tested at all. Collapsing the two is what sent the last run looking
            // for a bug that was never there.
            code = match verdict {
                state::Check::Fail => 1,
                state::Check::Blocked => {
                    if code == 0 {
                        3
                    } else {
                        code
                    }
                }
                state::Check::Pass => code,
            };
        } else {
            println!("clipboard selftest: SKIPPED (pass --clipboard to run it; it overwrites the clipboard)");
        }
        std::process::exit(code);
    }

    if flag("--desktop") {
        println!("{}", state::desktop_summary());
        return;
    }

    // A pin to look at, whether or not one was asked for: with no capture service
    // yet (§6.1) there is otherwise nothing the window path can be shown doing.
    if flag("--paste") {
        // §5.8.2 verified on a real machine, ahead of the global hotkey that will
        // normally trigger it (M5).
        match state::paste_from_clipboard() {
            Ok(id) => println!(
                "[falconshot] clipboard pinned as {id} {}",
                state::desktop_summary()
            ),
            Err(e) => println!("[falconshot] clipboard paste: {e}"),
        }
    }
    let demo = flag("--demo") || flag("--probe") || flag("--paste");
    if demo {
        let (w, h) = (
            after("--demo-w").unwrap_or(320).max(4) as u32,
            after("--demo-h").unwrap_or(200).max(4) as u32,
        );
        match state::add_demo(w, h) {
            Ok(id) => println!("[falconshot] demo pin {id} {}", state::desktop_summary()),
            Err(e) => eprintln!("[falconshot] demo pin failed: {e}"),
        }
    }

    let probe = after("--probe");
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/dev/falconshot/qml/main.qml"));
    } else {
        eprintln!("[falconshot] could not create QQmlApplicationEngine");
        std::process::exit(2);
    }

    if let Some(ms) = probe {
        state::quit_after(ms);
    }

    let mut code = 0;
    if let Some(app) = app.as_mut() {
        code = app.exec();
    }

    if let Some(ms) = probe {
        let report = state::window_report();
        let windows = report
            .split_whitespace()
            .find_map(|token| token.strip_prefix("topLevelWindows="))
            .and_then(|n| n.parse::<i32>().ok())
            .unwrap_or(-1);
        println!(
            "[probe after {ms} ms] qml_loaded={} pins={} {report}",
            state::qml_loaded(),
            state::with(|s| s.ids().len())
        );

        let mut problems = state::with(|s| s.problems.clone());
        // Debug chatter from Qt is normal in a debug build. A warning is not: that
        // is the QML engine reporting a document it could not build.
        let messages = state::messages();
        let bad: Vec<&str> = messages
            .lines()
            .filter(|line| line.starts_with("warn:") || line.starts_with("error:"))
            .collect();
        if !bad.is_empty() {
            println!("[probe qml] {}", bad.join("\n"));
            problems.push(format!("{} Qt warning(s)", bad.len()));
        }
        if demo && !state::qml_loaded() {
            problems.push("main.qml never reached Component.onCompleted".to_string());
        }
        if demo && windows < 1 {
            problems.push(format!("the demo pin produced {windows} windows"));
        }
        if !problems.is_empty() {
            println!("[probe problems] {}", problems.join(" | "));
            code = 1;
        }
    }
    std::process::exit(code);
}
