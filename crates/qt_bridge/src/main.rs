//! AI Falconshot - the process that owns the pin windows.
//!
//! Seven modes, because "does it work" has seven different answers:
//! * `--selftest` drives the state machine and the pixel pipeline and prints
//!   PASS/FAIL per step, without ever creating a window. `--clipboard` and
//!   `--capture` add the two checks that take over something real - the user's
//!   clipboard, and the screen as it is right now - and answer three ways:
//!   `PASS`=0, `BLOCKED`=3 (the machine refused to be measured), `FAIL`=1.
//! * `--snap` / `--rect x,y,w,h` / `--snap-window` put *actual screen pixels* into a
//!   pin window and print what Win32 measured, so a wrong crop is a number.
//! * `--probe <ms>` shows the windows for real, prints what the Win32 side of
//!   them actually looks like after ms, then quits. Frameless, per-pixel alpha and
//!   StaysOnTop are checked as numbers rather than by someone squinting.
//! * `--r13 <ms>` plants a control patch, lets Qt paint it, and then reads those
//!   same pixels back through every capture path - the visibility matrix of R13.
//! * `--mask <ms>` freezes the desktop, covers every screen with its own dimmed
//!   copy for ms, and reads the dim and the hole back off those windows.
//!   `--mask-soft` takes the four-rectangle path instead of the shader.
//! * `--ink <ms>` opens that same mask and then *draws on it* - three scripted
//!   strokes and one undo, through the same pointer calls a real drag makes - and
//!   reads the ink back off the window, pixel by pixel, alongside the dim it must
//!   not have disturbed.
//! * no flags: the app.

mod annotate;
mod annotate_check;
mod capture;
mod mask;
mod mask_check;
mod mask_view;
mod pin_view;
mod r13;
mod session;
mod settings;
mod state;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

/// Turn a check's verdict into a process exit code.
///
/// 1 is this code failing, 3 is the machine refusing to let it be tested at all.
/// Collapsing the two is what sent the last run looking for a bug that was never
/// there: an environment block is not a defect, and a defect is not an excuse.
fn verdict_code(code: i32, verdict: state::Check) -> i32 {
    match verdict {
        state::Check::Fail => 1,
        state::Check::Blocked => {
            if code == 0 {
                3
            } else {
                code
            }
        }
        state::Check::Pass => code,
    }
}

fn main() {
    // The clock the cold-path split is measured from, stamped before Qt says
    // anything at all. See `state::stamp`.
    state::stamp("main");
    // Before anything Qt has to say: a QML document that will not compile says so
    // only through the message handler, and a headless run would otherwise see a
    // silent zero.
    state::install_message_capture();
    // The settings file, read before Qt exists rather than at the first window: it is
    // pure Rust, it may one day decide how that window looks, and putting it here
    // keeps the read off the hot path §9.2 caps at 400 ms. `--mask` and `--ink` print
    // what each touch point cost.
    settings::start();
    state::stamp("cfg");

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

    // Three flags tune a mode instead of being one, and `after` matches whole
    // tokens, so `--mask-soft 1500` alone names nothing: it fell through into the
    // interactive app and ran until the process was killed - with an empty log,
    // because the app prints no report of its own. A mis-invocation is a measurement
    // that cannot be taken, which is the third verdict, not a crash and not a pass.
    let mut orphans = Vec::new();
    if flag("--mask-soft") && !flag("--mask") && !flag("--ink") {
        orphans.push("--mask-soft 需要和 --mask <ms>（或 --ink <ms>）一起用");
    }
    if (flag("--clipboard") || flag("--capture")) && !flag("--selftest") {
        orphans.push("--clipboard / --capture 是 --selftest 的附加档");
    }
    if after("--warm-rounds").unwrap_or(0) > 0 && !flag("--mask") && !flag("--ink") {
        orphans.push("--warm-rounds 需要和 --mask <ms> 一起用");
    }
    if !orphans.is_empty() {
        println!("[falconshot] {}", orphans.join("；"));
        std::process::exit(3);
    }

    if flag("--selftest") {
        let (ok, report) = state::selftest();
        println!("{report}");
        // The clipboard checks take the user's own clipboard over, so they run
        // when asked for and are reported as skipped otherwise.
        let mut code = if ok { 0 } else { 1 };
        if flag("--clipboard") {
            let (verdict, creport) = state::clipboard_selftest();
            println!("{creport}");
            code = verdict_code(code, verdict);
        } else {
            println!("clipboard selftest: SKIPPED (pass --clipboard to run it; it overwrites the clipboard)");
        }
        // The capture rows read the screen as it is right now, so they are asked
        // for rather than always run: a headless CI box has no desktop to describe.
        if flag("--capture") {
            let (verdict, creport) = state::capture_selftest();
            println!("{creport}");
            code = verdict_code(code, verdict);
        } else {
            println!(
                "capture selftest: SKIPPED (pass --capture to run it; it reads the real screen)"
            );
        }
        std::process::exit(code);
    }

    if flag("--desktop") {
        println!("{}", state::desktop_summary());
        return;
    }

    // The capture path on pixels that really exist, ahead of the mask UI that will
    // normally choose the rectangle: `--snap` takes the middle of the primary
    // monitor, `--rect x,y,w,h` takes exactly that, `--snap-window` takes the
    // window under the pointer. Each prints what the Win32 side measured, so a
    // wrong crop is a number to read rather than a picture to squint at.
    let snapping = flag("--snap") || flag("--snap-window");
    if snapping {
        let outcome = if flag("--snap-window") {
            match capture::window_under_cursor() {
                Some(w) => capture::pin_window(w.hwnd).map(|id| {
                    (
                        id,
                        format!(
                            "窗口「{}」[{}] {:?}",
                            w.title,
                            w.app_name,
                            w.visible_bounds()
                        ),
                    )
                }),
                None => Err("指针下面没有可截取的窗口".to_string()),
            }
        } else {
            let rect = args
                .iter()
                .position(|a| a == "--rect")
                .and_then(|i| args.get(i + 1))
                .and_then(|raw| capture::rect_arg(raw));
            match rect {
                Some(r) => capture::pin_rect(&r).map(|id| (id, format!("{r:?} 自已冻结的整屏"))),
                None => capture::snap_centre(400, 250),
            }
        };
        match outcome {
            Ok((id, what)) => println!(
                "[falconshot] snapped pin {id}: {what} {}",
                state::desktop_summary()
            ),
            Err(e) => println!("[falconshot] snap failed: {e}"),
        }
    }

    // A pin to look at, whether or not one was asked for: with no capture path
    // running there is otherwise nothing the window path can be shown doing - but
    // a real snap above already produced one.
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
    let demo = (flag("--demo") || flag("--probe") || flag("--paste")) && !snapping;
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

    // R13's matrix can only be measured on a window that is really on screen, so
    // this mode is a probe run whose subject is a control patch this process drew
    // at a size and place it chose. The patch has to exist before QML builds the
    // windows, and the reading happens after the event loop has painted them.
    let r13_ms = after("--r13").map(|ms| ms.max(300));
    let mut control = None;
    if r13_ms.is_some() {
        match r13::plant() {
            Ok(id) => control = Some(id),
            Err(e) => println!("[r13] 种不下对照色块：{e}"),
        }
    }

    // §5.2's mask, for the same reason: the dim is only a number once something has
    // painted it. The freeze and the textures have to be published before the QML
    // engine exists, because a mask window that captures the screen it is about to
    // cover would capture itself.
    //
    // `--ink` is this mode plus a script drawn on the same freeze, so it opens the
    // mask through the same call and owns the flow from there: the two gauges both
    // close it at the end, and running both would double-close.
    let mask_ms = after("--mask").map(|ms| ms.max(300));
    let ink_ms = after("--ink").map(|ms| ms.max(300));
    let shader_on = !flag("--mask-soft");
    if mask_ms.is_some() {
        match mask_check::open(shader_on) {
            Ok(line) => println!("[mask] {line}"),
            Err(e) => println!("[mask] 开不起来：{e}"),
        }
    } else if ink_ms.is_some() {
        match annotate_check::plant(shader_on) {
            Ok(line) => println!("[ink planted] {line}"),
            Err(e) => println!("[ink] 画不下去：{e}"),
        }
    }

    // Extra rounds of the same freeze-and-open, run once the process is warm. The
    // plan has to be written before the engine loads, because QML reads it in
    // `Component.onCompleted` and only a QML timer can fire afterwards.
    let warm_rounds = after("--warm-rounds").unwrap_or(0).max(0);
    let warm_step = after("--warm-step").unwrap_or(1500).max(200);
    if warm_rounds > 0 {
        state::warm(|w| {
            w.left = warm_rounds;
            w.step_ms = warm_step;
            w.shader = !flag("--mask-soft");
        });
    }

    let probe = after("--probe");
    state::stamp("engine");
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/dev/falconshot/qml/main.qml"));
    } else {
        eprintln!("[falconshot] could not create QQmlApplicationEngine");
        std::process::exit(2);
    }
    state::stamp("loaded");

    if let Some(ms) = probe {
        state::quit_after(ms);
    }
    if let Some(ms) = r13_ms {
        state::quit_after(ms);
    }
    if let Some(ms) = mask_ms.or(ink_ms) {
        // A warm round needs the loop to still be running when its timer fires, and
        // one more step after the last round before the read-back: the frame it
        // published is what `--mask` measures.
        let rounds_ms = if warm_rounds > 0 {
            (warm_rounds + 1) * warm_step
        } else {
            0
        };
        state::quit_after(ms.max(rounds_ms));
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

    if let Some(ms) = r13_ms {
        let (verdict, report) = match control {
            Some(id) => r13::measure(id),
            None => (
                state::Check::Fail,
                "r13 matrix: FAIL - 没有对照色块，四路采集无从比对".to_string(),
            ),
        };
        println!("{report}");
        println!(
            "[r13 after {ms} ms] qml_loaded={} {}",
            state::qml_loaded(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }

    if let Some(ms) = mask_ms.or(ink_ms) {
        // The last round's first frames, read while its windows are still up -
        // `measure()` closes the flow and the slots go with it.
        let warm_swaps = if warm_rounds > 0 {
            mask_check::swap_line()
        } else {
            String::new()
        };
        let ink = ink_ms.is_some();
        let (verdict, report) = if ink {
            annotate_check::measure()
        } else {
            mask_check::measure()
        };
        println!("{report}");
        // One cold path, split into the parts a resident process has already paid
        // before any hot key (Qt start-up, the QML engine, the document compile) and
        // the parts it has not (the freeze, the texture hand-off, showing N windows
        // and presenting their first frame). §10-14 ② is only reducible against the
        // second group, so the two are printed apart rather than as one total.
        println!("[mask cold] {}", state::stamp_line());
        // The settings file's own costs, split the way the plan asks for them: the
        // read is paid at start-up, the hand-over is paid inside the hot key, and the
        // write is paid after the user has already gone. `applies` and `saves` say how
        // many times each ran, because one round of a gauge proves less than four.
        println!("[settings] {}", settings::with(|s| s.line()));
        let notes = settings::with(|s| s.notes());
        if !notes.is_empty() {
            println!("[settings notes] {}", notes.join(" | "));
        }
        let part = |a: &str, b: &str| match state::between(a, b) {
            Some(ms) => format!("{ms} ms"),
            None => "-".to_string(),
        };
        println!(
            "[mask cold split] qt+freeze={} (of which freeze+publish={}) engine+qml={} qml->first-frame={} total={}",
            part("main", "engine"),
            part("asked", "opened"),
            part("engine", "loaded"),
            part("qml", "swap"),
            part("main", "swap")
        );
        // The rounds themselves, and the one number they were run for: milliseconds
        // from *that* round's open to its first presented frame, which is what a hot
        // key costs in a process that is already up. Each line names the round before
        // it, because a round's own frame only lands after its handler has returned.
        for line in state::warm(|w| w.rounds.clone()) {
            println!("[mask warm] {line}");
        }
        if warm_rounds > 0 {
            // The two halves added up, against the ceiling: the freeze and publish is
            // the `freeze` field of the round's own line, the window and first frame is
            // the slot figure. `first_swap_ms` starts its clock at the end of `open`,
            // so the freeze is deliberately not inside it and the sum is.
            println!("[mask warm {warm_rounds} round(s)] last: {warm_swaps}, window+frame vs ceiling {} ms (add each round's freeze ms for the hot-key total)", mask_check::FIRST_FRAME_MS);
        }
        println!(
            "[{} after {ms} ms] qml_loaded={} {}",
            if ink { "ink" } else { "mask" },
            state::qml_loaded(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    std::process::exit(code);
}
