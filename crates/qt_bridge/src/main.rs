//! AI Falconshot - the process that owns the pin windows.
//!
//! Twelve modes, because "does it work" has twelve different answers:
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
//! * `--hint <ms>` measures the *sentence* rather than a window's pixels: it plants a pin, lets
//!   `qml/HintWindow.qml` ask the product to save it, and then photographs the toast that
//!   §5.9.13's 短暂状态提示 and §8.5's 并提示 both promised - the top-level titled
//!   `falconshot-hint`, its native style bits, who owns the keyboard while it is up, and how
//!   many colours it drew. Until P54 nothing on the pin side was bound to `Session.status` at
//!   all, so every one of those readings was a string no screen had ever carried
//!   (§10 第 54 项 ②). `--hint-hold <ms>` moves the lifetime, which is how one door measures
//!   both halves of 短暂: up while its hold has not come round, gone once it has.
//! * `--hotkeys <ms>` measures the *resident* chain instead of a window: it registers the two
//!   actions this build can perform, puts the icon in the notification area, then posts the
//!   delivery the shell would have posted for each registered key and grades what the product
//!   did about it — mask opened, pin pasted. Only the fingertip is imitated, so the combination
//!   itself stays unmeasured and the report says so (§10 第 51 项). The paste leg needs
//!   something to paste, so this door writes a two-pixel picture through the same call a pin's
//!   复制 uses and hands the user's own clipboard back at the end — the one door that takes the
//!   clipboard besides `--selftest --clipboard` and `--ink`'s 复制, and it pays into the same
//!   `state::put_back`.
//! * `--autosave <ms>` is §5.5.3's 自动保存 read off the filesystem rather than off a
//!   struct field: it opens one mask, presses the same `Enter` door (确认选区) and counts the
//!   directory before and after, so the claim under test is "a file appeared" instead of "a
//!   bool was read". Like `--reveal` it stays out of `mask::GAUGE_FLAGS` — that refusal is
//!   what its first row is about — and it writes only into `FALCONSHOT_AUTOSAVE_DOOR_DIR`,
//!   never into the user's pictures folder (§9.1 第十五条).
//! * `--export-config <档>` / `--import-config <档>` are §5.20.3's two file-facing commands, and the
//!   only ones that finish before a window exists: one writes this machine's settings out, the other
//!   reads a file, validates it and says which keys it *would* overwrite (§5.20.3 step 4). Nothing is
//!   applied until `--confirm` - that flag is step 5's 用户确认, and an import without it exits 0
//!   having changed nothing, which is why its own line says so. `--import-all` widens the range from
//!   the PRD's 建议范围 to every key. Exit codes follow §5.17: 0 ok, 1 a real read/write failure,
//!   3 a mis-invocation refused before any file was touched (§10 第 118 项).
//! * no flags: the app. The same install, but nobody measures it and nothing quits: the keys and
//!   the icon stay up until 退出 is asked for. That is also why Qt's quit-on-last-window-closed
//!   is turned off here — the mask is the only window, and closing it must not take the
//!   registered keys and the icon away with it.

mod annotate;
mod annotate_check;
mod autosave_check;
mod capture;
mod delay;
mod hint_check;
mod hotkey;
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

/// 要点名、要给数字、但**不进** `mask::GAUGE_FLAGS` 的门。写两次是这一整段的病根，所以这张
/// 表是它们唯一的名单：`refusals` 的 `numbered` 与 `main` 结尾那条"点了就得有闹钟"的判定都
/// 读它。（`--hotkeys` 也要数字、也在 `numbered` 里，但它不在这一列：它不进那串 `quit_after`
/// 的对照表，是 §10 第 58 项带着走的一笔账。）
const EXTRA_DOORS: [&str; 2] = ["--reveal", "--autosave"];

/// 点了门而没人设闹钟 ⇒ 当场退出 3，而不是把这一趟交给一个看不见的等待。
///
/// 名单只有一个来源（`GAUGE_FLAGS` 加 `EXTRA_DOORS`），判定本身是纯函数：⑭ 那一条欠的就是
/// "决定一趟有没有证据的那句话，只有跑起二进制才问得出"，这一格同一个理由。
fn unalarmed(args: &[String], armed: &[&str]) -> Vec<&'static str> {
    let flag = |name: &str| args.iter().any(|a| a == name);
    mask::GAUGE_FLAGS
        .iter()
        .copied()
        .chain(EXTRA_DOORS)
        .filter(|name| flag(name) && !armed.contains(name))
        .collect()
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
    // 浮窗存活的时间是那一档要量的东西的刻度，单独点它没有刻度可量：`--hint` 不在 argv 里时
    // `hint_surface` 为假，窗根本不会开，这一趟就退化成上面 ⑭ 拒的那种零字节日志。
    if flag("--hint-hold") && !flag("--hint") {
        orphans
            .push("--hint-hold 需要和 --hint <ms> 一起用：没有那一档，这个数没人在读".to_string());
    }
    if (flag("--clipboard") || flag("--capture") || flag("--history")) && !flag("--selftest") {
        orphans.push("--clipboard / --capture / --history 是 --selftest 的附加档".to_string());
    }
    if after("--warm-rounds").unwrap_or(0) > 0 && !flag("--mask") && !flag("--ink") {
        orphans.push("--warm-rounds 需要和 --mask <ms> 一起用".to_string());
    }
    // §5.20.3 的两条命令各点一份文件。文件路径缺失时它们退化成上面 ⑭ 那一副样子：名了
    // 一档而那一档没有可读的东西 —— 导出无处可写、导入无处可读，两者都只会印一行错误而
    // 且退出码非零，但那一行说的是「路径是个 flag」，不是「你少给了一份档」。
    // `--confirm` 与 `--import-all` 是修饰档不是门：单独点它们没有任何东西在被修饰。
    let path_after = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .filter(|v| !v.starts_with("--"))
            .cloned()
    };
    let cfg_export = flag("--export-config");
    let cfg_import = flag("--import-config");
    if cfg_export && path_after("--export-config").is_none() {
        orphans.push("--export-config 需要在它后面给一份要写出的文件路径".to_string());
    }
    if cfg_import && path_after("--import-config").is_none() {
        orphans.push("--import-config 需要在它后面给一份要读入的文件路径".to_string());
    }
    if (flag("--confirm") || flag("--import-all")) && !cfg_import {
        orphans.push("--confirm / --import-all 是 --import-config <档> 的附加档".to_string());
    }
    if cfg_export && cfg_import {
        orphans.push(
            "--export-config 与 --import-config 不同时点：一次调用只做一件配置档的事".to_string(),
        );
    }
    if cfg_export || cfg_import {
        let mut others: Vec<&str> = mask::GAUGE_FLAGS.to_vec();
        others.extend(EXTRA_DOORS);
        others.extend([
            "--selftest",
            "--desktop",
            "--hotkeys",
            "--snap",
            "--rect",
            "--snap-window",
        ]);
        let hit: Vec<&str> = others.into_iter().filter(|n| flag(n)).collect();
        if !hit.is_empty() {
            orphans.push(format!(
                "--export-config / --import-config 只动配置档，不与 {:?} 同开",
                hit
            ));
        }
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
    numbered.extend(EXTRA_DOORS);
    // `--hotkeys` is named by a flag *and* a number and degrades the same way as the
    // doors above do: with no number the pump installs, the tray icon goes up, and
    // the run becomes a resident session that prints nothing - which is exactly the
    // zero-byte log from a live process that 第 47 项 ⑭ refuses on sight.
    numbered.push("--hotkeys");
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
    // 同一进程里 `Session.status` 只有一条线，而这一档量的就是"那句话有没有被看见"。和
    // `--pin` 并列时那条线被两档同时改写（贴图的腿也在写它），和开遮罩的三档并列时遮罩的窗
    // 盖在浮窗上面、`hint on screen` 与 `hint bitmap` 读的是别人的场次。要么单独点它，
    // 要么什么都别点——和上面那一条同一个理由。
    if flag("--hint")
        && (flag("--pin")
            || flag("--mask")
            || flag("--ink")
            || flag("--reveal")
            || flag("--autosave")
            || flag("--r13"))
    {
        orphans.push(
            "--hint 不能和 --r13 / --mask / --ink / --pin / --reveal / --autosave 一起用：那些档要么也写 \
             session.status，要么在浮窗上面盖一张自己的窗，读回来的就不是这一句了"
                .to_string(),
        );
    }
    // 这一条不是拼错，是把那一档要量的东西关掉：`gauge_run` 是 argv 前缀匹配，所以
    // `--reveal --mask 1500` 里那一步 `select` 会被结构性地拒，于是这一档报的是自己没跑，
    // 而不是产品拒绝。要么单独点它，要么什么都别点。
    //
    // `--autosave` 和 `--reveal` 并进来是同一条理由的两面：前者第一格量的就是"我不在
    // GAUGE_FLAGS 里"，被后者关掉的话那一格读的是腿没跑；而两档各自开一张遮罩、各自数自己
    // 的目录，并列时那两次普查就不是那一张遮罩的账了。
    if (flag("--reveal") || flag("--autosave")) && gauge_mode {
        orphans.push(
            "--reveal / --autosave 不能和 --probe / --r13 / --mask / --ink / --pin / --hint 一起用：那些前缀会让 \
             gauge_run 在那一步之前就把它拒掉，那一趟读到的只是腿没跑"
                .to_string(),
        );
    }
    if flag("--autosave") && flag("--reveal") {
        orphans.push(
            "--autosave 不能和 --reveal 一起用：两档各自开遮罩、各自数目录，并列时那两次普查数的不是同一张图"
                .to_string(),
        );
    }
    // 装热键的那一步在 `engine.load()` 之后，而这两类档都在它之前就走完了：`--selftest`
    // 与 `--desktop` 打印完就 `exit`，量具档读的是自己开的那张遮罩。和它们并列时，这一档
    // 报的是自己没跑——和上面每一条被拒的组合同一个理由。
    // `--snap` / `--paste` / `--demo` 不在这一列：它们留下窗口然后进入事件循环，那正是常驻
    // 会话的样子。
    if flag("--hotkeys")
        && [
            "--selftest",
            "--desktop",
            "--probe",
            "--r13",
            "--mask",
            "--ink",
            "--pin",
            "--reveal",
            "--autosave",
            "--hint",
        ]
        .iter()
        .any(|name| flag(name))
    {
        orphans.push(
            "--hotkeys 不能和 --selftest / --desktop / --probe / --r13 / --mask / --ink / \
             --pin / --reveal / --autosave / --hint 一起用：那些档各自读完就退，热键泵要到 engine 加载完才装，\
             并列时读数是别人的"
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
    // §5.14 / §5.16 那两段 config 自此有非 test 调用点（P72 起）：`history_cfg` + `advanced_cfg`
    // 各读一次、`format_startup_banner` 里的空/非空 `data_dir` 分岔本体打印 `(platform default)`
    // 或用户填的路径。`history_open_in` 不调 ⇒ disk 不动、SQLite 句柄留给 P73+。
    println!("[startup cfg] {}", settings::startup_banner());
    // §5.15 那 8 枚 `[appearance]` 键自此有非 test 调用点（P73 起）：`appearance_cfg()` 一次读整段快照、
    // `format_appearance_banner()` 逐字回读 8 枚（`theme` 走 `{:?}`、其余 7 枚 String / u32 直读）。
    // 本轮没有分岔本体 ⇒ 证人 dichotomy 回到"读者-only ⇒ 撤单测"一侧（P72 换到另一侧一次、本轮再回来）。
    println!("[appearance cfg] {}", settings::startup_appearance_banner());
    // P118：§5.19.1 第 1 步「读取系统主题（注册表 AppsUseLightTheme）」自此有非 test 调用点 ——
    // `platform_windows::theme::apps_use_light_theme()` 读 HKCU 那一枚 DWORD、
    // `format_theme_banner()` 把「用户钉的」「机器答的」「本程序生效的」三格各印一次。
    // 刻意不走 `QGuiApplication::styleHints()->colorScheme()`：那一枚要求 Qt 已存在，而这一段
    // 读数留在 settings::start() 那一段（Qt 之前）。§5.19.1 第 3 步「切换时同步更新」自 P120 起
    // 有监听腿 —— `platform_windows::hotkey` 那枚 pump 收 `WM_SETTINGCHANGE` 只交出
    // `Event::SettingChange(name)`，叫 `theme::apps_use_light_theme_invalidate()` 重新问一次机器
    // 的是 GUI 线程那一侧（`hotkey.rs` 消费那枚事件之处；证人 = 门那一格
    // `wm_settingchange reaches the theme`）⇒ 本行印的仍是启动那一刻的那一枚读数，而这一枚键
    // 自此会跟着机器动。P118 当时写的「本轮不做、无 WM_SETTINGCHANGE 监听」已被 P120 的读数作废（#332）。
    println!("[theme cfg] {}", settings::startup_theme_banner());
    // §5.17 那 4 枚 `[about]` 键自此有非 test 调用点（P74 起）：`about_cfg()` 一次读整段快照、
    // `format_about_banner()` 逐字回读 4 枚（2 枚 bool + 1 枚 u32 + 1 枚 String；`channel`
    // 三臂 clamp 已由 `Config::normalize` 在 cfg 加载时完成 ⇒ 本轮没有分岔本体）。证人 dichotomy
    // 沿用 P73 那一侧 ⇒ 本轮不加 cargo 单测。§5 那 7 段 config 至此**全部**跨过产品路径门槛。
    println!("[about cfg] {}", settings::startup_about_banner());

    // P76：§5.5 `[output]` 段那 17 枚键（枚数至此过期：P123 现读 19 枚，读数在
    // `settings::output_cfg` 那段的 docstring 里）的第一位整段读者自此跨产品路径门槛 —— 尤其
    // §5.5.11 `share_targets` 与 §5.5.12 `send_to_apps` 这两枚自 第 63 项 ⑥ 起整批结转的死键
    // 自此有首位读者（`format_output_banner` 里两枚 `is_empty()` 分岔各走一臂、本轮的分岔
    // 本体是纯函数 ⇒ 独立 cargo 单测 `banner_names_none_only_when_output_list_is_empty`、
    // 证人 dichotomy 的第 69 项 ⑫ 那一族本轮换回 P67/P68/P72/P75 一侧、qt 计数 208→209）。
    println!("[output cfg] {}", settings::startup_output_banner());

    // P77：§5.5.12 那批 `name|executable` 条目的**解析侧**自此跨产品路径门槛 ——
    // `parse_send_to` 是本轮的分岔本体（四臂：无 `|` / 空 name / 空 exe / 首拆之后原样保留
    // 三段与两侧空格 ⇒ 前 3 臂拒、后一臂收），纯函数 ⇒ **第六次**走"分岔本体是纯函数
    // 就不撤单测"那一族（P67 tray_from / P68 history_open_in / P72 format_startup_banner
    // / P75 refuse 规则 / P76 format_output_banner 之后）；本轮加独立 cargo 单测
    // `parse_send_to_splits_on_first_pipe_and_refuses_the_flawed` ⇒ qt 计数 209→210、
    // 三枚之和 501→502。UI 面与真 spawn 留下一轮（本轮**不** fork 外部程序、避开用户
    // 会话锁状态未知那一族）。
    println!("[send_to cfg] {}", settings::startup_send_to_banner());

    // P78：§5.5.11 那一批 share target 条目的**解析侧**自此跨产品路径门槛 ——
    // `parse_share_targets` 本轮走**最保守解析**（trim 空即拒、其余一律收并保留 trim
    // 后的字符串；PRD §5.5.11 未定 `name|exe` 那一族格式 ⇒ 白名单与去重留下一轮），
    // 分岔本体是纯函数 ⇒ **第七次**走"分岔是纯函数就不撤单测"那一族（P67 / P68 / P72 /
    // P75 / P76 / P77 之后）；本轮加独立 cargo 单测 `parse_share_targets_trims_and_
    // refuses_blank_only` ⇒ qt 计数 210→211、三枚之和 502→503。UI 面（工具条上的分享
    // 按钮）留下一轮。
    println!("[share cfg] {}", settings::startup_share_banner());

    // P79：§5.5.11 share_targets 的**分派侧**第二道门槛 —— `filter_share_targets` 把
    // [`startup_share_banner`] 的解析输出与内置 5 枚白名单（`slack / teams / wecom /
    // dingtalk / feishu`）按 `eq_ignore_ascii_case` 对照，known/unknown 计数由这一行
    // 宣布。分岔本体是纯函数 ⇒ 独立 cargo 单测
    // `filter_share_targets_splits_by_lowercase_against_allow_list`（证人 dichotomy
    // 第八次走到 P67/P68/P72/P75/P76/P77/P78 一侧、qt 计数 211→212）。UI 面与真
    // share-provider 集成留下一轮。
    println!("[share filter] {}", settings::startup_share_filter_banner());

    // P80：§5.5.12 send_to_apps 的**校验侧**分派 —— `validate_exe_paths` 把 P77
    // `parse_send_to` 输出的 `(name, exe)` 对按 Windows 可执行扩展名（`.exe` / `.cmd` /
    // `.bat`、大小写不敏感）分派，validated/refused 计数由这一行宣布。分岔本体是纯函数
    // ⇒ 独立 cargo 单测 `validate_exe_paths_accepts_exe_cmd_bat_and_refuses_others`
    // （证人 dichotomy 第九次走到 P67/P68/P72/P75/P76/P77/P78/P79 一侧、qt 计数 212→213）。
    // UI 面 + 真 `Command::spawn()` 留下一轮。
    println!(
        "[send_to validate] {}",
        settings::startup_send_to_validate_banner()
    );

    // P81：§5.5.12 send_to_apps 的**复合** dispatch —— `pipeline_send_to` 把 P77
    // `parse_send_to` 与 P80 `validate_exe_paths` 一次跑完、final / parse_refused /
    // validate_refused 三枚计数由这一行宣布。分岔本体是纯函数 ⇒ 独立 cargo 单测
    // `pipeline_send_to_composes_parse_then_validate`（证人 dichotomy 第十次走到
    // P67/P68/P72/P75/P76/P77/P78/P79/P80 一侧、qt 计数 213→214）。UI 面 + 真 spawn 留下一轮。
    println!(
        "[send_to pipeline] {}",
        settings::startup_send_to_pipeline_banner()
    );

    // P82：§5.5.11 share_targets 的**复合** dispatch —— `pipeline_share_targets` 把
    // P78 `parse_share_targets` 与 P79 `filter_share_targets` 一次跑完、final /
    // parse_refused / unknown 三枚计数由这一行宣布。分岔本体是纯函数 ⇒ 独立 cargo
    // 单测 `pipeline_share_targets_composes_parse_then_filter`（证人 dichotomy
    // 第十一次走到 P67/P68/P72/P75/P76/P77/P78/P79/P80/P81 一侧、qt 计数 214→215）。
    // UI 面 + 真 provider 派发留下一轮。
    println!(
        "[share pipeline] {}",
        settings::startup_share_pipeline_banner()
    );

    // P83：§5.5.12 send_to_apps 的**显示侧** dispatch —— `send_to_menu_entries`
    // 把 P81 `pipeline_send_to` 的 final_ok 映射成 `"{name} ({exe_stem})"` UI 串、
    // 条数由这一行宣布。分岔本体是纯函数 ⇒ 独立 cargo 单测
    // `send_to_menu_entries_strips_extension_and_preserves_case`（证人 dichotomy
    // 第十二次走到 P67/P68/P72/P75/P76/P77/P78/P79/P80/P81/P82 一侧、qt 计数 215→216）。
    // 真 `Command::new(exe).arg(path).spawn()` 与工具条按钮留下一轮。
    println!("[send_to menu] {}", settings::startup_send_to_menu_banner());

    // P84：§5.5.11 share_targets 的**显示侧** dispatch —— `share_menu_entries`
    // 把 P82 `pipeline_share_targets` 的 known 列表映射成首字母大写 UI 串、条数由
    // 这一行宣布。分岔本体是纯函数 ⇒ 独立 cargo 单测
    // `share_menu_entries_capitalizes_first_letter_preserves_rest`（证人 dichotomy
    // 第十三次走到 P67/P68/P72/P75/P76/P77/P78/P79/P80/P81/P82/P83 一侧、qt 计数
    // 216→217）。真 provider 派发与工具条按钮留下一轮。
    println!("[share menu] {}", settings::startup_share_menu_banner());

    // P85：§5.5.12 send_to_apps 的**反查侧** dispatch —— `resolve_send_to` 按显示
    // 名大小写不敏感查回 exe；本轮 selftest 冒烟 = 拿 pipeline 首条通过的 name 反查、
    // 比对 exe 是否同一枚。分岔本体是纯函数 ⇒ 独立 cargo 单测
    // `resolve_send_to_finds_exe_by_name_case_insensitive`（证人 dichotomy 第十四次
    // 走到 P67/P68/P72/P75/P76/P77/P78/P79/P80/P81/P82/P83/P84 一侧、qt 计数 217→218）。
    println!(
        "[send_to resolve] {}",
        settings::startup_resolve_send_to_banner()
    );

    // P86：§5.5.11 share_targets 的**scheme 拼装** —— `share_scheme_for` 把 known 一侧
    // 的 target 名映射到一条 URL scheme 串（五枚内置 slack / teams / wecom / dingtalk /
    // feishu 各一条模板，大小写不敏感），未注册或空 path ⇒ None。分岔本体是纯函数 ⇒ 独立
    // cargo 单测 `share_scheme_for_maps_all_five_targets_case_insensitive`（证人 dichotomy
    // 第十五次走到 P67/P68/P72/P75/P76/P77/P78/P79/P80/P81/P82/P83/P84/P85 一侧、qt 计数
    // 218→219）。真 ShellExecute / 浏览器打开留下一轮。
    println!("[share scheme] {}", settings::startup_share_scheme_banner());

    // P87：§5.5.12 send-to-app 前的 argv 拼装 —— `send_to_exec_argv` 走 P85 `resolve_send_to`
    // 拿 exe、返回 `[exe, path]` 两槽位 argv 数组（模拟 `Command::new(exe).arg(path)`、
    // **不 spawn**）。分岔本体是纯函数 ⇒ 独立 cargo 单测
    // `send_to_exec_argv_returns_exe_then_path_or_none`（证人 dichotomy 第十六次走到
    // P67/P68/P72/P75/P76/P77/P78/P79/P80/P81/P82/P83/P84/P85/P86 一侧、qt 计数 219→220）。
    // 真 `Command::spawn()` 留下一轮。
    println!(
        "[send_to argv] {}",
        settings::startup_send_to_exec_argv_banner()
    );

    // P88：§5.5.11 known 一侧全体 scheme 拼装（P86 的复数版）—— `share_scheme_for_all`
    // 逐条走 P86 `share_scheme_for`、返回 `Vec<(target, url)>`、跳过未在白名单项、顺序与
    // 原大小写保留。分岔本体是纯函数 ⇒ 独立 cargo 单测
    // `share_scheme_for_all_maps_in_order_and_skips_unknown`（证人 dichotomy 第十七次）。
    // 真 ShellExecute / 浏览器打开留下一轮。
    println!(
        "[share scheme all] {}",
        settings::startup_share_scheme_all_banner()
    );

    // P89：§5.5.12 send-to-app 前的**自动首条** argv 拼装。走 P81 pipeline 拿
    // final_ok、取首条 exe、返回 `vec![exe, path]` argv 数组（模拟
    // `Command::new(exe).arg(path)` 的参数、**不 spawn**）。与 P87 `send_to_exec_argv`
    // 的差别：name 参数消失、由流水线自动决定首条 ⇒ 用户点击 "Send to ▸" 时可
    // 一步走而不必展开子菜单。分岔本体是纯函数 ⇒ 独立 cargo 单测
    // `send_to_exec_argv_first_picks_pipeline_head_or_none`（证人 dichotomy 第十八次）。
    // 真 `Command::spawn()` 留下一轮。
    println!(
        "[send_to argv first] {}",
        settings::startup_send_to_exec_argv_first_banner()
    );

    // P90：§5.5.11 known 一侧的**自动首条** provider scheme（P86 的 name-less 版、与
    // P89 `send_to_exec_argv_first` 对称）。逐条走 `share_scheme_for`、返回第一条
    // 命中模板的 `(target, url)`；全体未注册或空 path ⇒ None。分岔本体是纯函数
    // ⇒ 独立 cargo 单测 `share_scheme_first_returns_head_or_none`（证人 dichotomy
    // 第十九次）。真 ShellExecute / 浏览器打开留下一轮。
    println!(
        "[share scheme first] {}",
        settings::startup_share_scheme_first_banner()
    );

    // P91：§5.5.12 send-to 显示条目第一次跨过 Rust→QML 边界 —— `pipeline_send_to` 校验
    // 通过的那一段 `(name, exe)` 二元组，`send_to_menu_entries` 映射成 UI 显示串，
    // `send_to_menu_names_joined` 用 `|` 联成一枚纯串。分岔本体是纯函数 ⇒ 独立 cargo
    // 单测 `send_to_menu_names_joined_uses_pipe_separator`（证人 dichotomy 第二十次）。
    // 同一枚串由 MaskView 的 `#[qinvokable] sendToMenuNames` 递给 QML 层，Repeater
    // 接线留下一轮（P93；P92 走的是 share 一侧的对称桥）。
    println!(
        "[send_to names] {}",
        settings::startup_send_to_menu_names_banner()
    );

    // P92：§5.5.11 share 显示条目第一次跨过 Rust→QML 边界 —— P82 `pipeline_share_targets`
    // 白名单内的那一段 known，`share_menu_entries` 映射成 UI 显示串（P84），
    // `share_menu_names_joined` 用 `|` 联成一枚纯串。这一半与 P91 的 send-to 一侧对称。
    // 分岔本体是纯函数 ⇒ 独立 cargo 单测 `share_menu_names_joined_uses_pipe_separator`
    // （证人 dichotomy 第二十一次）。同一枚串由 MaskView 的
    // `#[qinvokable] shareMenuNames` 递给 QML 层；真 Repeater 接线与 provider 派发留下一轮。
    println!(
        "[share names] {}",
        settings::startup_share_menu_names_banner()
    );

    // P110：§6.2「显示历史记录占用多少空间」那四枚数第一次跨过 `History::usage()` 的产品
    // 门槛。分岔本体是纯函数 ⇒ 独立 cargo 单测两枚（`history_usage_line_adds_the_three_byte_columns`
    // 与 `history_usage_line_reports_zero_when_everything_is_empty`，见 settings.rs tests 模块）；
    // 三臂各有一行返回：开着 / 关着 / 开不上，都在启动 banner 上印得出东西。
    println!(
        "[history usage] {}",
        settings::startup_history_usage_banner()
    );

    // P111：§5.14.2「按上下方向键在历史里前后翻」那两条腿第一次跨过 `History::prev` 与
    // `History::next` 的产品门槛——两枚此前只被 history.rs 自己的 cargo 单测读过、产品这一头
    // 零读者。本条 banner 拿库里最新那一行作 anchor，把它的 prev / next 各读一次；分岔本体
    // 是纯函数 ⇒ 独立 cargo 单测三枚（`history_neighbors_line_shows_both_sides_for_a_middle_row`
    // / `_dashes_a_missing_side` / `_dashes_both_sides_of_a_single_row`）；三臂各有一行返回：
    // 开着 / 关着 / 开不上，都在启动 banner 上印得出东西。跳到那一行的**动**腿与界面桥留下一轮。
    println!(
        "[history neighbors] {}",
        settings::startup_history_neighbors_banner()
    );

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
    // §5.20.3 的两条命令点的是一份文件而不是一个毫秒数，所以它们要一把自己的取法：紧跟的
    // 下一枚 token 是路径，除非那一枚自己又是一档 —— 否则 `--export-config --confirm` 会把
    // `--confirm` 当成文件名去写盘。
    let path_after = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .filter(|v| !v.starts_with("--"))
            .cloned()
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

    // §5.20.3 的两条命令排在一切窗、屏、剪贴板之前，也排在 `--selftest` 之前：它们只读这一台
    // 机器的 settings、只碰用户点名的那一份文件。写在 refusals 之后是必须的 —— 一句写错的调用
    // 应当先被拒、退 3、说出缺的是什么，而不是先把盘改了再被拒。
    //
    // 退出码照 §5.17 的三条规则：成功 0、失败 1、参数错 3；「文件写入失败时不得返回成功状态」
    // 在这里不是口号 —— `config_export` 把 `std::fs::write` 的 Err 原样递回来，那一支退 1。
    if let Some(p) = path_after("--export-config") {
        match settings::config_export(std::path::Path::new(&p)) {
            Ok(line) => {
                print!("{line}");
                std::process::exit(0);
            }
            Err(e) => {
                println!("[export] 失败：{e}");
                println!("用法：falconshot --export-config <文件路径>");
                std::process::exit(1);
            }
        }
    }
    if let Some(p) = path_after("--import-config") {
        let scope = if flag("--import-all") {
            falcon_core::config::ImportScope::All
        } else {
            falcon_core::config::ImportScope::Suggested
        };
        match settings::config_import(std::path::Path::new(&p), scope, flag("--confirm")) {
            Ok(line) => {
                print!("{line}");
                std::process::exit(0);
            }
            Err(e) => {
                println!("[import] 失败：{e}");
                println!("用法：falconshot --import-config <文件路径> [--import-all] [--confirm]");
                std::process::exit(1);
            }
        }
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
        // P75：`--history` 那一档真开 `history.db`、disk 会长一份，所以它照 `--clipboard` /
        // `--capture` 的同一族做成**opt-in**：不加这一档就 SKIPPED、加了就走
        // `settings::startup_history_probe` ⇒ `history_open_in`（`history_cfg().enabled` 一侧
        // 真调 `History::open(dir)`、另一侧 `Ok(None)`）。dir 来自 `advanced.data_dir`，
        // 空 ⇒ `%APPDATA%\ai-falconshot\`。撤 `history_open_in` 头上 `#[allow(dead_code)]`
        // 的唯一入口、也是 §5 config dispatch 层第一位非 test 调用点（P75 起）。
        if flag("--history") {
            let (verdict, hreport) = state::history_selftest();
            println!("{hreport}");
            code = verdict_code(code, verdict);
        } else {
            println!(
                "history selftest: SKIPPED (pass --history to run it; it opens or creates history.db)"
            );
        }
        std::process::exit(code);
    }

    if flag("--desktop") {
        println!("{}", state::desktop_summary());
        // The clamp a pin is put through is a different reading of the same desktop:
        // the platform's table, physical, one ratio per monitor.
        println!("{}", state::pin_desktop_summary());
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

    // §5.9.13 的「短暂状态提示」和 §8.5 的「并提示」自此有了一扇可以被拍照的窗，而 `--hint`
    // 就是那台相机：它种一张贴图，让浮窗自己的腿请产品存它，然后读那句话有没有到屏上。
    // 和 `--pin` 同一个理由，贴图与计划都必须在 engine 之前存在——`HintWindow.qml` 只在
    // `Component.onCompleted` 读一次，而 hold 是那一行里的第六格，晚一步就没有浮窗了。
    let hint_want = after("--hint");
    let hint_ms = hint_want.map(|ms| ms.max(hint_check::MIN_MS));
    hint_check::set_hold(after("--hint-hold").unwrap_or(hint_check::HOLD_MS).max(200));
    if hint_ms.is_some() {
        match hint_check::plant() {
            Ok(line) => println!("[hint] {line}"),
            Err(e) => println!("[hint] 种不下贴图：{e}"),
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

    // §10 第 58 项的自动保存那一档：与 `--reveal` 同一副骨架——先钉写回，再开遮罩，最后由
    // `autosave_check` 在进程里数目录。它不复用 `--mask` 的量具计划，因为那一套九行是钉住的
    // 清单，而这一档要的是"这一次按完盘上多了一张"这种对面是文件系统的读数。
    let auto_want = after("--autosave");
    let auto_ms = auto_want.map(|ms| ms.max(autosave_check::MIN_MS));
    let mut auto_opened = false;
    if let Some(ms) = auto_ms {
        let pinned = autosave_check::pin();
        println!("[autosave pin] 配置写回钉到 {}", pinned.display());
        match mask_check::open(!flag("--mask-soft")) {
            Ok(line) => {
                auto_opened = true;
                println!("[autosave mask] {line}（被要求等 {ms} ms）");
                // P98：界面那一条腿的前置，跑在这里而不是 `measure()` 里，理由只有一条——QML 的
                // `Component.onCompleted` 在下面 `engine.load()` 那一刻就把清单读走了。钉落点与
                // 种那一行都必须赶在那一次读之前，否则工具条读的就是用户
                // `%APPDATA%\ai-falconshot\history\` 里那些行（§9.1 第十五条禁的是这一件）。
                // 摆不出前置时不退出：`plan_line()` 保持空串 ⇒ 那条腿整条不跑 ⇒ 那两格判 BLOCKED
                // 而不是替本档假装成功。
                match autosave_check::arm() {
                    Ok(said) => println!("{said}"),
                    Err(e) => {
                        println!("[autosave arm] 前置没摆出来：{e}（界面那两格据此判 BLOCKED）")
                    }
                }
            }
            Err(e) => println!("[autosave] 遮罩开不起来：{e}"),
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
    // §5.2's front door, and §5.1's resident process. `--hotkeys <ms>` is the door
    // that measures the chain; no flags at all is the app the plan ships, which
    // registers its keys, puts the icon in the notification area and stays up.
    let hot_want = after("--hotkeys");
    let hotkeys_ms = hot_want.map(|ms| ms.max(hotkey::MIN_MS));
    let plain = args.is_empty();
    // 浮窗只在两种运行里存在：产品本身，和点名要量它的那一档。别的量具档也写
    // `session.status`（`--pin` 的另存为就写），而它们读的是自己那扇窗——一句写给全进程的
    // 话要是每次都开一扇顶层窗，`pin window` 挑到的就可能不是它想的那一块。这不是"浮窗礼貌
    // 一点"的约定，是一个由 Rust 一侧回答的判定：`HintWindow.qml` 只有在这一格为真时才拿到
    // 锚点，`state::hint_surface` 的注释写着它的理由。
    state::set_hint_surface(plain || hint_ms.is_some());
    state::stamp("engine");
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/dev/falconshot/qml/main.qml"));
    } else {
        eprintln!("[falconshot] could not create QQmlApplicationEngine");
        std::process::exit(2);
    }
    state::stamp("loaded");

    // The pump is built here rather than at the top of `main` for one reason: QML
    // hands the GUI thread to the hot key layer inside `Component.onCompleted`,
    // which runs during the `load` call above. A pump installed before that owns a
    // thread with nothing at the other end of it, and every key it catches would be
    // a message with nowhere to go.
    let mut launch = None;
    if plain || hotkeys_ms.is_some() {
        // §5.1's root is an invisible `Item` and Qt's default ends the process when
        // the last visible window closes - which, for a tool whose one window is the
        // mask, would take the registered keys and the icon away after a single
        // screenshot.
        session::qobject::set_quit_on_last_window_closed(false);
        launch = match hotkey::install() {
            Ok(done) => Some(done),
            Err(e) => {
                println!("[hotkeys] 一个也没装：{e}");
                None
            }
        };
        if let Some(done) = launch.as_ref() {
            for row in &done.rows {
                println!("[hotkey] {row}");
            }
        }
    }

    // The presses, and the order: 贴出剪贴板 first, because the capture leg leaves the
    // mask covering the screen and a second freeze would photograph the mask itself -
    // the listener refuses that on purpose, so a door that pressed capture first would
    // end up measuring its own refusal.
    // The clipboard is taken by the door and by nothing else — the app's own path never
    // writes it. Paste needs something to paste, and the door is the one that says
    // whether it put it there, so `press paste_pin` grades `id=-1` against that answer
    // instead of against an excuse.
    let clip = match hotkeys_ms {
        Some(_) => hotkey::Clip::plant(),
        None => hotkey::Clip::untouched(),
    };
    // §5.2.5 repeats the region of the last capture, and a process that has captured
    // nothing has nothing to repeat. So the door gives it one, through the product's
    // own leg, before any press - the same shape as the clipboard above.
    let region = match hotkeys_ms {
        Some(_) => hotkey::Region::plant(),
        None => hotkey::Region::untouched(),
    };
    let mut injected: Vec<(String, bool)> = Vec::new();
    let mut setting: (&str, bool) = ("", false);
    if hotkeys_ms.is_some() {
        for action in [
            "paste_pin",
            "capture_repeat",
            "capture_active_window",
            "capture",
        ] {
            injected.push((action.to_string(), hotkey::press(action)));
        }
        // §5.19.1 第 3 步：门替系统播一次「设置改了」，好让 §5.19.1 那三行读数（顶层窗口
        // 在不在、监听线程有没有接到、GUI 线程重问了什么）在这一趟里有供应商。这一发必须
        // 在 `exec()` 之前递出去：GUI 线程那一半是排在事件环里跑的，而这一趟跑的就是那个环。
        setting = (
            "ImmersiveColorSet",
            hotkey::notify_setting_change("ImmersiveColorSet"),
        );
    }

    // 谁点了、谁就有了闹钟。这张表存在的唯一理由：P58 新加的那一道门进了 `numbered`（所以
    // 少给数字会被拒）却没进下面这串 `quit_after`，于是那一趟遮罩开起来了、打印完
    // `[autosave mask]` 就再也不回来了——`app.exec()` 等的是真人，而 stdout 是行缓冲的，
    // 那正是一枚零字节日志。⑭ 那一串拒的是"没给数字"，下面那一格拒的是"给了数字没闹钟"。
    let mut armed: Vec<&str> = Vec::new();
    if let Some(ms) = probe {
        state::quit_after(ms);
        armed.push("--probe");
    }
    if let Some(ms) = r13_ms {
        state::quit_after(ms);
        armed.push("--r13");
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
        // 一次闹钟、两枚门：这一支是 `or`，所以点了谁就记谁，两枚都点就两枚都记。
        if mask_ms.is_some() {
            armed.push("--mask");
        }
        if ink_ms.is_some() {
            armed.push("--ink");
        }
    }
    if let Some(ms) = pin_ms {
        // The leg runs in phases of [`pin_check::LEG_DELAY_MS`] each, and the floor
        // above is `pin_check::LEG_WAITS` of them plus the grace the engine gets to
        // reach `Component.onCompleted` at all - the watchdog is armed before the QML
        // document is loaded, so the leg's own budget starts late. A `--pin` run that was
        // asked for therefore has the time it needs rather than reporting its own
        // absence, and the report prints the gap it actually measured.
        state::quit_after(ms);
        armed.push("--pin");
    }
    if let Some(ms) = hint_ms {
        // 地板只有被要求的那个 `<ms>`，一格宽限都不加：这一档的腿不宣布结束，而「短暂」要量
        // 哪一面本来就是由 hold 与这趟总时长谁大谁小决定的。给读回加宽限等于替 `--hint-hold`
        // 那一半把窗等到离场之外——那一格就再也量不到"还在"了。
        state::quit_after(ms);
        armed.push("--hint");
    }
    if let Some(ms) = reveal_ms {
        // 这一档等的不是腿的相位，是 shell 把那个目录窗端起来的时间——而它端起来之后本进程
        // 也举不到它，所以这个数只决定遮罩在屏幕上留多久，不决定任何一格的成功。
        state::quit_after(ms);
        armed.push("--reveal");
    }
    if let Some(ms) = auto_ms {
        // 这一格是 P58 自己踩出来的：`--autosave` 第一次跑，遮罩开起来了、日志停在
        // `[autosave mask]` 那一行、然后再也没有回来 —— 因为它进了 `numbered`（少了 `<ms>`
        // 会被拒）却没进这张"谁点了就设闹钟"的表，于是 `app.exec()` 变成一次等真人的产品会话，
        // 正是第 47 项 ⑭ 那一串拒绝要挡的样子的另一半：拒绝管的是**没给数字**的那一趟，这一趟
        // 给了数字，欠的是闹钟。等的是 `commit()` 之后 Qt 把那张选区拼出来的时间。
        state::quit_after(ms);
        armed.push("--autosave");
    }
    if let Some(ms) = hotkeys_ms {
        // The presses are already posted by now, so the wait is the chain's: pump to
        // listener, listener to the Qt queue, and a freeze plus N presented frames.
        state::quit_after(ms);
    }
    // 点了门而没人设闹钟 ⇒ 当场退出 3，而不是把这一趟交给一个看不见的等待。名单只有一个来源：
    // `GAUGE_FLAGS` 加 `EXTRA_DOORS`，与 `refusals` 里那张 `numbered` 同源。
    let unalarmed = unalarmed(&args, &armed);
    if !unalarmed.is_empty() {
        println!(
            "[falconshot] {} 被点了，却没有一格 `quit_after` 是替它设的：这一趟会开一张窗然后等真人，\
             打印不出任何东西。给它在 `main` 的闹钟串里加一行，别放它进 `app.exec()`",
            unalarmed.join(" / ")
        );
        std::process::exit(3);
    }

    let mut code = 0;
    if let Some(app) = app.as_mut() {
        // The alarm is armed; this is where the process stops being the one that decides
        // when the run ends. `--hotkeys` reduces "等到的 4000 ms" against this stamp, and
        // a number shorter than the alarm says something else took the process - the only
        // candidate in this door is "the last window closed". No other door asks for it,
        // so no other door's cold-path line can grow a `hotkeys loop=`.
        if hotkeys_ms.is_some() {
            state::stamp("hotkeys loop");
        }
        code = app.exec();
    }

    if let Some(ms) = probe {
        let report = state::window_report();
        let windows = report
            .split_whitespace()
            .find_map(|token| token.strip_prefix("topLevelWindows="))
            .and_then(|n| n.parse::<i32>().ok())
            .unwrap_or(-1);
        // §8.5's watch, counted by the C++ that made the connections. This is the
        // only half of that clause a single-monitor machine can be graded on: the
        // number says the wiring is listening, and no run here can say what it would
        // do when a screen actually goes away.
        let watch = state::screen_watch_connections();
        println!(
            "[probe after {ms} ms] qml_loaded={} pins={} screen_watch={watch} {report}",
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
        if state::qml_loaded() && watch == 0 {
            problems.push("the screen watch connected to nothing".to_string());
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
    if let Some(ms) = auto_ms {
        let (verdict, report) = if auto_opened {
            autosave_check::measure(ms)
        } else {
            autosave_check::open_failed("上面那行 [autosave] 说了为什么")
        };
        println!("{report}");
        println!(
            "[autosave after {ms} ms] 要求 {} ms 生效 {ms} ms qml_loaded={} {} {}",
            auto_want.unwrap_or(ms),
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = hint_ms {
        // 报告读的是浮窗那扇窗自己：Win32 的旗位、前台归属、PrintWindow 的像素，加上产品
        // 那句话的返回值。hold 印在这一行里，因为「短暂」分成两趟量——批处理必须能从日志
        // 本身读出这一趟量的是哪一面，而不是回头去猜当时传了什么。
        let (verdict, report) = hint_check::measure();
        println!("{report}");
        println!(
            "[hint after {ms} ms] 要求 {} ms 生效 {ms} ms，hold {} ms，qml_loaded={} {} {}",
            hint_want.unwrap_or(ms),
            hint_check::hold(),
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = hotkeys_ms {
        // The report is the pump's own numbers - what the shell took, what the icon
        // says, and what each injected press did to the product - graded against the
        // mask and pin state the chain left behind.
        let (verdict, report) = match launch.as_ref() {
            Some(done) => hotkey::report(
                done,
                hot_want.unwrap_or(ms),
                ms,
                &injected,
                &clip,
                &region,
                setting,
            ),
            None => (
                state::Check::Blocked,
                "hotkeys door: BLOCKED - 上面那行 [hotkeys] 说了为什么；装了 0 个，链路没有起点"
                    .to_string(),
            ),
        };
        println!("{report}");
        println!(
            "[hotkeys after {ms} ms] 要求 {} ms 生效 {ms} ms，读回时泵仍活着={}，qml_loaded={} {} {}",
            hot_want.unwrap_or(ms),
            hotkey::is_running(),
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    // The last thing before the process ends, whatever the mode: an unregistered key
    // and a removed icon are the difference between this run and one that left the
    // user's PrintScreen held by a process that no longer exists. A no-op when
    // nothing was installed.
    hotkey::shutdown();
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::{exe_stamp, refusals, unalarmed};
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
        doors.extend(crate::EXTRA_DOORS);
        assert_eq!(
            doors.len(),
            8,
            "本轮的门是八枚：多一枚少一枚都要先改这条再改代码"
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

    /// 上面那条拒的是"没给数字"，这一条拒的是"给了数字、可没人替它设离场闹钟"。它是 P58
    /// 自己踩出来的：`--autosave` 加进 `numbered` 的那一次没加进 `main` 的 `quit_after` 串，
    /// 于是那一趟开完遮罩就再也不说话——不是失败的读数，是根本没有读数。八枚门各问两次：
    /// 没闹钟时必须点名自己，有闹钟时必须沉默（否则产品每一趟都退 3）。
    #[test]
    fn a_named_door_without_an_alarm_is_named_back() {
        let mut doors: Vec<&str> = crate::mask::GAUGE_FLAGS.to_vec();
        doors.extend(crate::EXTRA_DOORS);

        for name in doors {
            let args = argv(&[name, "1500"]);
            let bare = unalarmed(&args, &[]);
            assert_eq!(
                bare,
                vec![name],
                "{name} 被点了而没人设闹钟，却没能点名自己：{bare:?}"
            );
            assert!(
                unalarmed(&args, &[name]).is_empty(),
                "{name} 已经有人设了闹钟，却还是被点名"
            );
        }

        // 什么都没有点：常驻会话不是漏设闹钟。
        assert!(unalarmed(&argv(&[]), &[]).is_empty());
        // `--mask` 与 `--ink` 共用一次 `quit_after`，所以两枚都点了就得两枚都记。
        let both = argv(&["--mask", "1500", "--ink", "1500"]);
        assert_eq!(
            unalarmed(&both, &["--mask"]),
            vec!["--ink"],
            "那一次 `or` 只替一枚门设了闹钟时，另一枚必须被点名"
        );
        assert!(unalarmed(&both, &["--mask", "--ink"]).is_empty());
    }

    /// The other arms, each in both directions, plus the one invocation that must never be
    /// refused at all: no arguments is the product, not a mis-measurement. Exiting 3 on that
    /// would take the interactive app down with the gauge's guardrail.
    #[test]
    fn a_mis_tuned_invocation_is_refused_before_the_first_window() {
        assert!(refusals(&argv(&[]), false).is_empty());
        assert!(refusals(
            &argv(&["--selftest", "--clipboard", "--capture", "--history"]),
            false
        )
        .is_empty());
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

        // §10 第 54 项 ②'s two new arms. `--hint-hold` is the first 附加档 that names no
        // door of its own: `after` matches whole tokens, so it cannot be mistaken for
        // `--hint`, and a run that sets only a lifetime opens no window to time.
        let hold = refusals(&argv(&["--hint-hold", "500"]), false);
        assert_eq!(hold.len(), 1, "{hold:?}");
        assert!(hold[0].contains("--hint-hold"), "{}", hold[0]);

        let hint_clash = refusals(&argv(&["--hint", "2000", "--pin", "2000"]), true);
        assert_eq!(hint_clash.len(), 1, "{hint_clash:?}");
        assert!(
            hint_clash[0].contains("--hint") && hint_clash[0].contains("--pin"),
            "{}",
            hint_clash[0]
        );
        assert!(
            refusals(&argv(&["--hint", "2000", "--hint-hold", "500"]), false).is_empty(),
            "--hint 带着自己的附加档仍被拒"
        );

        // `--reveal` is refused by the *product's* answer, not by this list's own reading of
        // argv: the boolean is what `gauge_run` said, and a reveal door with a gauge prefix in
        // front of it reports "the leg did not run" rather than a refusal.
        assert!(refusals(&argv(&["--reveal", "1500"]), true)
            .iter()
            .any(|s| s.contains("gauge_run")));
        assert!(refusals(&argv(&["--reveal", "1500"]), false).is_empty());
    }

    /// P75：`--history` 那一枚 opt-in 档与 `--clipboard` / `--capture` 共用同一句 orphan ——
    /// 那一句里三个 flag 名各出现一次、`--selftest` 作为 fix 也被点名。单独 `--history`
    /// 被拒、拒绝里同时含 `--selftest`。这一枚分岔本体是纯函数（`flag("--history") &&
    /// !flag("--selftest")`）⇒ 证人 dichotomy 的第 69 项 ⑫ 那一族本轮从 P73/P74 的
    /// "读者-only ⇒ 撤单测"一侧**再回到 P67/P68/P72 那一侧**（分岔本体是纯函数 ⇒ 不撤单测）；
    /// refuse 侧的证人自此是 §9.1 单测而不是梯 rc，而 dispatch 侧的证人是 `--history` 那条腿。
    #[test]
    fn refusals_reject_history_without_selftest() {
        let hist = refusals(&argv(&["--history"]), false);
        assert_eq!(hist.len(), 1, "{hist:?}");
        assert!(
            hist[0].contains("--history") && hist[0].contains("--selftest"),
            "{}",
            hist[0]
        );
        assert!(refusals(&argv(&["--selftest", "--history"]), false).is_empty());
    }

    /// §5.20.3 的两条命令各点一份文件。少了路径要拒，而且那句里得点名自己那一枚 flag —— 否则
    /// 一句写错的调用会被归档到别的模式名下（⑭ 的那条规则就是为了这件事写的）。`--confirm` 与
    /// `--import-all` 是修饰档不是门：单独出现时没有被修饰的东西，拒。两条命令同开也拒：一次调用
    /// 只做一件配置档的事，退出码才只说得清一件结果。**带上路径的每一支必须沉默** —— 一条把正确
    /// 调用拒掉的规则，会让运行证人那一趟退 3 而梯上依然报绿。
    #[test]
    fn refusals_name_each_config_command_and_stay_quiet_when_it_has_its_file() {
        let e = refusals(&argv(&["--export-config"]), false);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("--export-config"), "{}", e[0]);

        let i = refusals(&argv(&["--import-config"]), false);
        assert_eq!(i.len(), 1, "{i:?}");
        assert!(i[0].contains("--import-config"), "{}", i[0]);

        assert!(refusals(&argv(&["--export-config", "out.toml"]), false).is_empty());
        assert!(refusals(
            &argv(&["--import-config", "in.toml", "--import-all", "--confirm"]),
            false
        )
        .is_empty());

        let c = refusals(&argv(&["--confirm"]), false);
        assert_eq!(c.len(), 1, "{c:?}");
        assert!(
            c[0].contains("--confirm") && c[0].contains("--import-config"),
            "{}",
            c[0]
        );

        let b = refusals(
            &argv(&["--export-config", "a.toml", "--import-config", "b.toml"]),
            false,
        );
        assert_eq!(b.len(), 1, "{b:?}");
        assert!(
            b[0].contains("--export-config") && b[0].contains("--import-config"),
            "{}",
            b[0]
        );

        let m = refusals(
            &argv(&["--export-config", "a.toml", "--mask", "1500"]),
            false,
        );
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("--mask"), "{}", m[0]);

        // 路径那一位被另一枚 flag 占住：等于没给文件，而不是把 `--confirm` 当成档名去写盘。
        let p = refusals(&argv(&["--export-config", "--confirm"]), false);
        assert_eq!(p.len(), 2, "{p:?}");
    }
}
