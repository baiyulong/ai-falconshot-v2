//! AI Falconshot - the process that owns the pin windows.
//!
//! Eight modes, because "does it work" has eight different answers:
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
//! * `--pin <ms>` plants a pin and has *that window's own QML* walk the two items its
//!   menu offers, then grades the sentences the door answered with and the files it
//!   wrote. `--ink` measures what a pin looks like; this measures what pressing 另存为
//!   says, which nothing had ever read back (§10 第 37 项 ①). Since P38 one leg of it
//!   opens the real dialog and presses that window's own default button, so a sentence
//!   can be graded as arriving through the path a user's click takes (§10 第 38 项 ①),
//!   and the run prints the `<ms>` it was asked for next to the one it waited (②).
//! * `--reveal <ms>` is the one door that is *not* in `mask::GAUGE_FLAGS`: it freezes and
//!   opens the mask, writes one file through the same `save_out`, and then presses the
//!   notification's own 落在一次点击 — through [`platform_windows::reveal::select`], for
//!   real (§10 第 46 项 ③). That means it leaves one Explorer window on the user's desktop
//!   which this process cannot close, so it is never implied by another door and never run
//!   by default.
//! * no flags: the app.

mod annotate;
mod annotate_check;
mod capture;
mod mask;
mod mask_check;
mod mask_view;
mod pin_check;
mod pin_view;
mod r13;
mod reveal_check;
mod save;
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

/// §10 第 40 项 ③ - which binary the footer's numbers came out of.
///
/// A judgement row is evidence about one build, and the ladder writes a new log every
/// rung while the old ones stay readable, so a line has to say what exe printed it or a
/// PASS from the rung before the fix is indistinguishable from the one after. Seconds
/// since the epoch rather than a wall-clock string, because the number is what a reader
/// compares - against `stat -c %Y`, and against the same field in the next log to see
/// whether the rung actually rebuilt anything.
///
/// Module level, not inside `main`: a binary's `fn main` is unreachable to `cargo test`,
/// so a footer that is only assembled there is a footer whose format nothing checks.
fn exe_stamp() -> String {
    let Ok(meta) = std::env::current_exe().and_then(std::fs::metadata) else {
        return "exe=?".to_string();
    };
    // A platform that will not hand over an mtime folds to 0, which is 1970 - and 1970
    // printed as `{}` looks exactly like a timestamp. The unit test is the guard.
    let when = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("exe={} B @{when}s", meta.len())
}

/// Every way this binary can be asked to measure something and be handed an argument it
/// cannot act on, collected as sentences.
///
/// Returned rather than printed, for the reason 第 47 项 ⑭ names: while this lived inline in
/// `main`, the only way to ask any of it was to run the binary and read its exit code - so
/// the predicate that decides whether a run produces evidence at all had no test, and a
/// seventh door would have joined by being bolted onto the end of a stretch nobody could
/// check. `gauge_mode` is the product's own `MaskState::gauge_run`, passed in rather than
/// called, so this list cannot quietly become a second copy of the thing it reports on.
fn refusals(args: &[String], gauge_mode: bool) -> Vec<String> {
    let flag = |name: &str| args.iter().any(|a| a == name);
    let after = |name: &str| -> Option<i32> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
    };

    let mut orphans: Vec<String> = Vec::new();
    if flag("--mask-soft") && !flag("--mask") && !flag("--ink") {
        orphans.push("--mask-soft 需要和 --mask <ms>（或 --ink <ms>）一起用".to_string());
    }
    if (flag("--clipboard") || flag("--capture")) && !flag("--selftest") {
        orphans.push("--clipboard / --capture 是 --selftest 的附加档".to_string());
    }
    if after("--warm-rounds").unwrap_or(0) > 0 && !flag("--mask") && !flag("--ink") {
        orphans.push("--warm-rounds 需要和 --mask <ms> 一起用".to_string());
    }
    // These modes are named by a flag *and* a number, and the number is the one that
    // decides whether the mode runs at all: `after` answers `None` for a missing or
    // unparseable `<ms>`, and every `if x_ms.is_some()` in `main` then skips itself. What
    // runs instead is `app.exec()` waiting for a human to close a window, printing
    // nothing - and because Rust's stdout is line-buffered, that is a zero-byte log
    // from a live process, which is indistinguishable from a gauge that hung. A run
    // whose evidence looks exactly like a defect is worse than a crash, so the
    // mis-invocation is refused in the same voice as the orphans above.
    //
    // `--reveal` joins this list without joining `GAUGE_FLAGS`: it runs no gauge (it is
    // kept out of that array precisely because it leaves an Explorer window on the user's
    // desktop), but it is named by a flag and a number and degrades the same way. Before
    // ⑭ it had its own hand-written arm, which is the shape that lets a new door be
    // refused by one path and not by the other.
    let mut numbered: Vec<&str> = mask::GAUGE_FLAGS.to_vec();
    numbered.push("--reveal");
    let mut missing_ms = Vec::new();
    for name in numbered {
        if flag(name) && after(name).is_none() {
            missing_ms.push(name);
        }
    }
    if !missing_ms.is_empty() {
        orphans.push(format!(
            "{} 少了毫秒数（例如 `{} 1500`）：不带数字时这一档什么都不跑，只会退化成一次等真人的产品 GUI 会话",
            missing_ms.join(" / "),
            missing_ms[0]
        ));
    }
    // A combination rather than a mis-spelling, and refused for the same reason a
    // missing `<ms>` is: the reading would belong to neither mode. The mask covers
    // every screen with its own window, so `pin window` - which picks the top-level
    // that overlaps the pin's rect - would be measuring the wrong window, and
    // `--mask` closes the freeze it is scored against.
    if flag("--pin") && (flag("--mask") || flag("--ink")) {
        orphans.push(
            "--pin 不能和 --mask / --ink 一起用：遮罩的窗会盖住贴图，那几格读的就不是这块窗了"
                .to_string(),
        );
    }
    // 这一条不是拼错，是把那一档要量的东西关掉：`gauge_run` 是 argv 前缀匹配，所以
    // `--reveal --mask 1500` 里那一步 `select` 会被结构性地拒，于是这一档报的是自己没跑，
    // 而不是产品拒绝。要么单独点它，要么什么都别点。
    if flag("--reveal") && gauge_mode {
        orphans.push(
            "--reveal 不能和 --probe / --r13 / --mask / --ink / --pin 一起用：那些前缀会让 \
             gauge_run 在 select 之前就把这一步拒掉，那一趟读到的只是腿没跑"
                .to_string(),
        );
    }
    orphans
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
    //
    // `gauge_mode` is passed in rather than read inside because it is the product's own
    // answer (`MaskState::gauge_run`), not a copy this function could drift from - and a
    // copy is exactly what 第 47 项 ⑭ is here to stop.
    let orphans = refusals(&args, mask::MaskState::gauge_run());
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
    // The ink run carries a dialog leg of its own now, and that leg needs the same
    // backstop §10 第 38 项 ② made `--pin` print two numbers for: a run asked for shorter
    // than the leg's budget is raised rather than refused, and says what it was asked for.
    let ink_want = after("--ink");
    let ink_ms = ink_want.map(|ms| ms.max(annotate_check::DIALOG_MIN_MS));
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

    // §5.9.16 and §5.9.17 walked by the pin window's own QML. The pin and the plan
    // both have to exist before the engine: the delegate reads the plan once, in
    // `Component.onCompleted`, and nothing else in this process can hand a QML object
    // a piece of Rust state later without a binding that would fire in every session.
    // §10 第 38 项 ②: the number that was asked for and the number the run waits are
    // two readings, and keeping only the second is how a `--pin 100` came to print
    // itself as an 800 ms run. Both are printed below, and the floor is the gauge's.
    let pin_want = after("--pin");
    let pin_ms = pin_want.map(|ms| ms.max(pin_check::MIN_MS));
    if pin_ms.is_some() {
        match pin_check::plant() {
            Ok(line) => println!("[pin] {line}"),
            Err(e) => println!("[pin] 种不下贴图：{e}"),
        }
    }

    // §10 第 46 项 ③：这一档要的是 `MaskState::landed` 与那一次点击，所以它得先有一张开得起
    // 的遮罩；它与 `--mask` 走同一个 `open`，只是 argv 里没有那道拒绝（上面刚为此拒过一次
    // 组合）。它不画标注，也不种量具的命名：那一格由 `reveal_check` 自己挑名字。
    let reveal_want = after("--reveal");
    let reveal_ms = reveal_want.map(|ms| ms.max(reveal_check::MIN_MS));
    let mut reveal_opened = false;
    if let Some(ms) = reveal_ms {
        // 钉在开遮罩之前，和 `annotate_check::plant` 同一个理由：`close()` 里那句
        // `write_back` 是产品的，腿拦不住它，只能给它换地方。
        let pinned = reveal_check::pin();
        println!("[reveal pin] 配置写回钉到 {}", pinned.display());
        match mask_check::open(!flag("--mask-soft")) {
            Ok(line) => {
                reveal_opened = true;
                println!("[reveal mask] {line}（被要求等 {ms} ms）");
            }
            Err(e) => println!("[reveal] 遮罩开不起来：{e}"),
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
    if let Some(ms) = pin_ms {
        // The leg runs in phases of [`pin_check::LEG_DELAY_MS`] each, and the floor
        // above is `pin_check::LEG_WAITS` of them plus the grace the engine gets to
        // reach `Component.onCompleted` at all - the watchdog is armed before the QML
        // document is loaded, so the leg's own budget starts late. A `--pin` run that was
        // asked for therefore has the time it needs rather than reporting its own
        // absence, and the report prints the gap it actually measured.
        state::quit_after(ms);
    }
    if let Some(ms) = reveal_ms {
        // 这一档等的不是腿的相位，是 shell 把那个目录窗端起来的时间——而它端起来之后本进程
        // 也举不到它，所以这个数只决定遮罩在屏幕上留多久，不决定任何一格的成功。
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
        // §10 第 38 项 ②'s pair, at this gauge too. Only the ink run has a floor raised
        // under it - `--mask` asks for a freeze and a read-back, and gets the milliseconds
        // it named - so only its line grows the two numbers.
        let raised = if ink {
            format!("要求 {} ms 生效 {ms} ms ", ink_want.unwrap_or(ms))
        } else {
            String::new()
        };
        println!(
            "[{} after {ms} ms] {}qml_loaded={} {} {}",
            if ink { "ink" } else { "mask" },
            raised,
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }

    if let Some(ms) = pin_ms {
        // The report is QML's own lines, graded against the files on disk and against
        // the same door called from Rust. Nothing here samples the screen: a pin's
        // doors answer with a sentence and a file, and both are readable without
        // trusting a capture path at all.
        let (verdict, report) = pin_check::measure();
        println!("{report}");
        println!(
            "[pin after {ms} ms] 要求 {} ms 生效 {ms} ms qml_loaded={} {} {}",
            pin_want.unwrap_or(ms),
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = reveal_ms {
        let (verdict, report) = if reveal_opened {
            reveal_check::measure(ms)
        } else {
            reveal_check::open_failed("上面那行 [reveal] 说了为什么")
        };
        println!("{report}");
        println!(
            "[reveal after {ms} ms] 要求 {} ms 生效 {ms} ms qml_loaded={} {} {}",
            reveal_want.unwrap_or(ms),
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::{exe_stamp, refusals};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// The field's only job is to make a log attributable to a build, and the way it
    /// fails quietly is a zero: `@0s` is 1970, which is a date, and two logs sharing it
    /// read like two runs of one binary. So both numbers are checked against the file
    /// the string claims to name - the byte count for equality, the mtime for being this
    /// century's and not in the future.
    #[test]
    fn the_footer_names_the_binary_that_printed_it() {
        let stamp = exe_stamp();
        let Some(rest) = stamp.strip_prefix("exe=") else {
            panic!("{stamp:?} 不以 `exe=` 开头");
        };
        let Some((bytes, rest)) = rest.split_once(" B @") else {
            panic!("{stamp:?} 里没有 ` B @`");
        };
        let Some(seconds) = rest.strip_suffix('s') else {
            panic!("{stamp:?} 不以 `s` 收尾");
        };
        let Ok(bytes) = bytes.parse::<u64>() else {
            panic!("{bytes:?} 不是体积的数字");
        };
        let Ok(seconds) = seconds.parse::<u64>() else {
            panic!("{seconds:?} 不是 mtime 的数字");
        };
        let measured = std::fs::metadata(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(bytes, measured.len(), "{stamp:?} 的体积不是这个文件的");
        assert!(seconds > 1_577_836_800, "{stamp:?} 的 mtime 早于 2020 年");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(seconds <= now + 60, "{stamp:?} 的 mtime 在未来");
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    /// The 第 47 项 ⑭ debt was not that the refusals were wrong but that they could not be
    /// asked: they lived in `main`, so checking one meant running the binary, which means a
    /// desktop, a timer and a log - and the question worth asking has no witness in any of
    /// those. It is *completeness*. A door that needs a number and is not on the list is not
    /// a wrong verdict, it is the zero-byte log from a live process that 第 41 项 closed out,
    /// reopened one flag at a time.
    ///
    /// So each door is asked twice. The number case must come back empty: a refusal that
    /// fires on a correct invocation is how a gauge stops running while its ladder still
    /// reports exit 3 as "the desktop's fault". The bare case must name that door in its own
    /// sentence, because a shared sentence about some other flag is how a mis-typed call
    /// gets archived against the wrong mode.
    #[test]
    fn every_door_that_needs_a_number_says_so_when_it_has_none() {
        let mut doors: Vec<&str> = crate::mask::GAUGE_FLAGS.to_vec();
        doors.push("--reveal");
        assert_eq!(
            doors.len(),
            6,
            "本轮的门是六枚：多一枚少一枚都要先改这条再改代码"
        );

        for name in doors {
            let with = refusals(&argv(&[name, "1500"]), false);
            assert!(with.is_empty(), "{name} 带着毫秒数仍被拒：{with:?}");

            let bare = refusals(&argv(&[name]), false);
            assert_eq!(bare.len(), 1, "{name} 不带数字只该有一句拒绝：{bare:?}");
            assert!(
                bare[0].contains(name),
                "{name} 没在自己的那句里被点名：{}",
                bare[0]
            );
            assert!(
                bare[0].contains("少了毫秒数"),
                "{name} 那句不是同一族的说法：{}",
                bare[0]
            );
        }
    }

    /// The other arms, each in both directions, plus the one invocation that must never be
    /// refused at all: no arguments is the product, not a mis-measurement. Exiting 3 on that
    /// would take the interactive app down with the gauge's guardrail.
    #[test]
    fn a_mis_tuned_invocation_is_refused_before_the_first_window() {
        assert!(refusals(&argv(&[]), false).is_empty());
        assert!(refusals(&argv(&["--selftest", "--clipboard", "--capture"]), false).is_empty());
        assert!(refusals(
            &argv(&["--mask", "1500", "--mask-soft", "--warm-rounds", "3"]),
            false
        )
        .is_empty());

        let soft = refusals(&argv(&["--mask-soft"]), false);
        assert_eq!(soft.len(), 1, "{soft:?}");
        assert!(soft[0].contains("--mask-soft"), "{}", soft[0]);

        let clip = refusals(&argv(&["--clipboard"]), false);
        assert!(clip.iter().any(|s| s.contains("--selftest")), "{clip:?}");
        let cap = refusals(&argv(&["--capture"]), false);
        assert_eq!(cap.len(), 1, "{cap:?}");

        let warm = refusals(&argv(&["--warm-rounds", "3"]), false);
        assert_eq!(warm.len(), 1, "{warm:?}");
        assert!(warm[0].contains("--warm-rounds"), "{}", warm[0]);

        // The combination that reads neither mode, named by both of its flags.
        let clash = refusals(&argv(&["--pin", "1500", "--mask", "1500"]), true);
        assert!(
            clash
                .iter()
                .any(|s| s.contains("--pin") && s.contains("--mask")),
            "{clash:?}"
        );

        // `--reveal` is refused by the *product's* answer, not by this list's own reading of
        // argv: the boolean is what `gauge_run` said, and a reveal door with a gauge prefix in
        // front of it reports "the leg did not run" rather than a refusal.
        assert!(refusals(&argv(&["--reveal", "1500"]), true)
            .iter()
            .any(|s| s.contains("gauge_run")));
        assert!(refusals(&argv(&["--reveal", "1500"]), false).is_empty());
    }
}
