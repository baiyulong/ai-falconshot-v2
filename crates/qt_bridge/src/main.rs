//! AI Falconshot - the process that owns the pin windows.
//!
//! Twelve modes, because "does it work" has twelve different answers:
//! * `--selftest` drives the state machine and the pixel pipeline and prints
//!   PASS/FAIL per step, without ever creating a window. Five companions add the checks
//!   that take over something real - the user's clipboard (`--clipboard`), the screen as it
//!   is right now (`--capture`), `history.db` on disk (`--history`), §5.5.8's 刷新截图
//!   (`--refresh`: since P153, it pins regions of the real screen and hands the state machine two
//!   desktops that no longer contain them) and, since P154, §5.5.10's 发送到其他应用
//!   (`--sendto`: it hands a real picture to a real program through `ShellExecuteW`, so it is the
//!   one door whose other side is another process - and the one that flashes a console window) -
//!   and answer three ways:
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
//!   Since P151 it also *picks the tray menu*: the run posts the delivery the menu would have
//!   made for its 首选项 entry, through the tail the shell's own `TrackPopupMenu` return uses,
//!   and grades whether a top-level settings window then appeared in a census of Qt's window
//!   list. The pick goes first, before the presses, because the mask gate would otherwise answer
//!   for it — and the fingertip inside a modal menu is still what is imitated, not the hand.
//! * `--autosave <ms>` is §5.5.3's 自动保存 read off the filesystem rather than off a
//!   struct field: it opens one mask, presses the same `Enter` door (确认选区) and counts the
//!   directory before and after, so the claim under test is "a file appeared" instead of "a
//!   bool was read". Like `--reveal` it stays out of `mask::GAUGE_FLAGS` — that refusal is
//!   what its first row is about — and it writes only into `FALCONSHOT_AUTOSAVE_DOOR_DIR`,
//!   never into the user's pictures folder (§9.1 第十五条).
//! * `--export-config <档>` / `--import-config <档>` are §5.20.3's two file-facing commands, and with
//!   P182's `--export-palette <档>` / `--import-palette <档>` (§5.7.22's 导出调色板文件 / 导入文件后应用)
//!   they are the four that finish before a window exists - the roster is [`FILE_COMMANDS`], and the
//!   refusals that keep them apart from every measurement door read that one list instead of four
//!   copies of it. One config command writes this machine's settings out, the other reads a file,
//!   validates it and says which keys it *would* overwrite (§5.20.3 step 4). The palette pair does the
//!   same for one row of colours: `--export-palette` writes what [`settings::palette_export`] renders
//!   through `colors::Palette::to_text` (one `#RRGGBB` per line, hand-editable), `--import-palette`
//!   reads it back through `Palette::from_text` and refuses a file that yields nothing rather than
//!   letting `Config::validate` refill the factory row. Nothing is applied until `--confirm` - that
//!   flag is step 5's 用户确认, and an import without it exits 0
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
mod cli;
mod custom;
mod delay;
mod group_window;
mod guide;
mod help;
mod help_menu;
mod hint_check;
mod history_window;
mod hot_corner;
mod hotkey;
mod mask;
mod mask_check;
mod mask_view;
mod pin_check;
mod pin_view;
mod r13;
mod refresh_check;
mod reveal_check;
mod save;
mod sendto_check;
mod session;
mod settings;
mod settings_view;
mod state;
mod tray_click_check;
mod update_check;
mod whiteboard_check;

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

/// §5.17's frame for a named capture, with the sentence that says what it is — the
/// clipboard and file legs' shared first step, kept out of `main` so both arms ask the
/// capture layer the same one question rather than spelling the freeze twice.
///
/// Module level, not inside `main`, for the reason [`exe_stamp`] gives: `fn main` is
/// invisible to `cargo test`. The contract this reaches is tested in [`cli`]; what is
/// tested *here* is only that the two output arms go through one frame owner. The pixels
/// still need a desktop, so a real-machine run grades them, not a unit test.
///
/// P165 adds the third slot. The file leg's sentence is `save`'s, so 「你要的是 …已裁掉」 cannot ride
/// inside it the way it rides inside [`capture::region_line`]; it has to be handed to that arm as a
/// separate tail, or the leg that a human actually types would carry the correction without the
/// report, leaving the report only in a selftest line.
fn cli_frame(select: cli::Select) -> Result<(falcon_core::frame::Frame, String, String), String> {
    match select {
        cli::Select::Region(rect) => capture::region_frame(&rect).map(|(frame, got, clipped)| {
            (
                frame,
                capture::region_line(&rect, &got, clipped),
                capture::clip_tail(&rect, clipped),
            )
        }),
        cli::Select::Last => {
            capture::last_frame().map(|(frame, _, line)| (frame, line, String::new()))
        }
        cli::Select::Active => {
            capture::active_frame().map(|(frame, _, line)| (frame, line, String::new()))
        }
        // `--new` is refused any `-o` at [`cli::classify`], so no argv reaches here with
        // New; the arm exists because the match has to be total, and it says so plainly
        // rather than pretending a mask has a finished picture to hand over.
        cli::Select::New => Err("--new 开的是遮罩，没有可直接写出的画面".to_string()),
    }
}

/// 「刚钉上一枚贴图」那一行的形状：[`capture::pinned_line`] 说谁钉的、钉在哪一块，这里挂上命令行
/// 这一路自己的那半句。拆成一枚纯函数是为了让 `cargo test` 问得到这一句 —— `cli_capture_to_pin`
/// 自己得先抓一次真屏幕，那一半归真机门问，而「id 有没有被丢掉、尾巴有没有被重拼」不需要屏幕。
fn cli_pinned_line(id: falcon_core::pin::PinId, line: &str) -> String {
    format!("{}（关掉那扇窗即退出）", capture::pinned_line(id, line))
}

/// §5.17's 贴图 output for the three named captures: put the picture in the pin table
/// before the QML engine exists, so the window renders when the event loop starts —
/// exactly the ordering `--snap` uses (freeze and `state::add` ahead of `engine.load`).
/// It returns the line to print; the caller then falls through to the loop rather than
/// exiting, because a pin is only a promise until a window shows it.
fn cli_capture_to_pin(select: cli::Select) -> Result<String, String> {
    let (id, line) = match select {
        cli::Select::Region(rect) => capture::pin_rect(&rect)?,
        cli::Select::Last => capture::pin_last_region()?,
        cli::Select::Active => capture::pin_active_window()?,
        // `--new` is refused any `-o` upstream, so nothing reaches here with New; the arm says so
        // rather than pretending a mask has a finished picture to hand over.
        cli::Select::New => return Err("--new 走遮罩".to_string()),
    };
    Ok(cli_pinned_line(id, &line))
}

/// 要点名、要给数字、但**不进** `mask::GAUGE_FLAGS` 的门。写两次是这一整段的病根，所以这张
/// 表是它们唯一的名单：`refusals` 的 `numbered` 与 `main` 结尾那条"点了就得有闹钟"的判定都
/// 读它。（`--hotkeys` 也要数字、也在 `numbered` 里，但它不在这一列：它不进那串 `quit_after`
/// 的对照表，是 §10 第 58 项带着走的一笔账。）
///
/// P140 进来的是 `--hotcorner`：它要给数字（不给就退化成一次等真人的零字节日志，同 ⑭），
/// 但它不开遮罩 —— 角落读的是指针、显示器清单与前台矩形，三样都不经屏幕采集那一条路，
/// 留在 `GAUGE_FLAGS` 之外也就是留在 `gauge_run` 那道结构性拒绝之外。
///
/// P142 进来的是 `--settings-window`：它也不开遮罩，它开的是**一扇正常的窗**（有边框、接键盘），
/// 而那一扇窗是 QML 造的 —— Rust 只写「这一趟该不该有一扇设置窗」那一格。它要给数字同 ⑭：
/// 不给就没有 `quit_after`，`app.exec()` 等的是真人，而 `settings_view::window_measure` 一行也读不到。
///
/// P143 进来的是 `--settings-io`：同一扇窗、同一副骨架，量的是 §5.20.3 那五步 —— 页上那两枚钮
/// 「导出配置 / 导入配置」。它比上面那一档多两件事：真的往盘上写一份导出的档、真的把一份档
/// 应用进 settings，所以它的写回同样要钉起来（`--settings-io` 那一档自己印 `[settings-io pin]`）。
/// 两档点的是**同一枚 `settings_open`**，所以并列要拒（见 `refusals`）。
///
/// P166 进来的是 `--custom-window`：§5.3.5 那一扇「自定义截图」对话框，与上面两档同一副骨架
/// （Rust 只写「这一趟该不该有一扇窗」那一格，造与毁在 `main.qml`）。它**不进** [`WINDOW_DOORS`]：
/// 那一格数的是「这一趟被点名了几扇**设置**窗」，而这一档写的是 `custom_open` 而不是
/// `settings_open` —— 两扇不同的窗同开不该被读成同一扇窗开了两回。它要数字同 ⑭。
///
/// P172 进来的是 `--group-window`：§5.12.3 / §5.12.4 那一扇「图片分组管理」窗，与前几档同一副
/// 骨架、写的仍是第三枚 `*_open`（`group_window_open`），因此也**不进** [`WINDOW_DOORS`]。它与
/// `--history-window` 不同的一件事值得写下来：它**没有落点要铉** —— 分组与贴图不住在
/// `config.toml` 里（`crates/core/src/config.rs` 那一行），本档种的三组三枚全在进程私有的
/// `PinSet` 内存里，所以它也不进 [`HISTORY_DOORS`]：那一格拦的是「两档各铉各自的历史落点」。
///
/// P183 进来的是 `--settings-palette`：§5.7.22 步骤 1/2/3 那一格「调色板管理」，与 `--settings-io`
/// 同一副骨架（同一扇设置窗、另一副计划、写回钉在同一枚落点上），因此**进** [`WINDOW_DOORS`]：
/// 它写的仍是那一枚 `settings_open`，三档点的是同一扇窗 —— 并列时窗那侧说的话没有作者。
const EXTRA_DOORS: [&str; 9] = [
    "--reveal",
    "--autosave",
    "--hotcorner",
    "--settings-window",
    "--settings-io",
    "--settings-palette",
    "--custom-window",
    "--history-window",
    "--group-window",
];

/// `EXTRA_DOORS` 里那**三**档**要一扇真窗**的：`--settings-window` 量页上的一行跟不跟着值走
/// （§5.20.2），`--settings-io` 量页上那两枚钮交不交得出、收不收得回一份档（§5.20.3），
/// `--settings-palette` 量的是同一扇页上那一格调色板板条与那八枚钮（§5.7.22 步骤 1/2/3）。
///
/// 这一枚名单存在的理由是 `refusals` 那一格问的问题：那里判的不是「某一档有没有并列」，而是
/// **「这一趟被点名了几扇窗那一档」** —— 给每档各写一条 `if`，两档同开时就会交两句同样的话
/// （P142 那一个 `2 != 1` 正是这个形状）。名单从 `EXTRA_DOORS` 里挑，挑的是「谁开窗」这一件事，
/// 而不是三档各自的名字。
const WINDOW_DOORS: [&str; 3] = ["--settings-window", "--settings-io", "--settings-palette"];

/// §5.14 那一族里要求**同一对会话落点**的两档：`--autosave` 与 P171 进来的
/// `--history-window` 都要把 [`crate::mask::MaskState`] 的 `history` / `history_dir` 两格
/// 钉进**自己**的沙盒并在里头种行。并列时后钉的那一枚赢，两档各自读到的行数与字节数
/// 就都不是自己种的那一份。
///
/// 这一枚名单存在的理由与 [`WINDOW_DOORS`] 同一条：给每档各写一条 `if`，两档同开就会拿到
/// 两句各说各的话（P142 / P143 那两对 `2 != 1` 正是这一个形状），而其中一句说的还是别人。
const HISTORY_DOORS: [&str; 2] = ["--autosave", "--history-window"];

/// `--settings`、`--tray`、`--whiteboard` 与 `--update-check` 这四档的共同点不是「它们读的都是配置」，而是**它们在
/// `engine.load()` 之前打完自己的报告就 `exit`**：四档都不开遮罩、不要闹钟（所以都不在
/// [`EXTRA_DOORS`] 里），也因此都不许有同伴 —— 并列时后点名的那一档一次也没跑，而那一趟印出来
/// 的只是前一档的报告，看上去像一次成功的测量。
///
/// 这一枚名单存在的理由与 [`WINDOW_DOORS`] 同一个：`refusals` 里判的不是「某一档有没有并列」，
/// 而是「这一趟被点名了几扇**打完就退**的门」。给每档各写一条 `if`，`--settings --tray` 就会拿到
/// 两句各说各的话（P142/P143 那两对 `2 != 1` 正是这个形状），而其中一句说的还是别人那一档。
const EXIT_DOORS: [&str; 5] = [
    "--settings",
    "--tray",
    "--whiteboard",
    "--update-check",
    "--help-menu",
];

/// 只碰「用户点名的那一份文件」的四档命令：§5.20.3 的两档动整份 settings，§5.7.22 的两档
/// 动那一排色。它们与 [`EXIT_DOORS`] 的共同点是「打完自己那一句话就 `exit`」，而它们自己的
/// 共同点是**各点一份文件路径** —— 所以拒绝那一族必须写成一张名单而不是四条 `if`：
/// `--export-config a --import-palette b` 并列时，一人一条规矩就会交两句各说各的话
/// （P142 / P143 那两对 `2 != 1` 正是这个形状），而两句里说的还是别人那一档。
///
/// 这一族也不进 [`EXTRA_DOORS`] 与 [`WINDOW_DOORS`]：那两张表数的是「这一趟有没有一扇窗、
/// 被点名了几扇窗」，而这四档从不开窗。
const FILE_COMMANDS: [&str; 4] = [
    "--export-config",
    "--import-config",
    "--export-palette",
    "--import-palette",
];

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
    if (flag("--clipboard")
        || flag("--capture")
        || flag("--history")
        || flag("--refresh")
        || flag("--sendto"))
        && !flag("--selftest")
    {
        orphans.push(
            "--clipboard / --capture / --history / --refresh / --sendto 是 --selftest 的附加档"
                .to_string(),
        );
    }
    if after("--warm-rounds").unwrap_or(0) > 0 && !flag("--mask") && !flag("--ink") {
        orphans.push("--warm-rounds 需要和 --mask <ms> 一起用".to_string());
    }
    // §5.20.3 的两条命令与 §5.7.22 的两条命令各点一份文件，所以四档共用这一族规则（名单
    // [`FILE_COMMANDS`]）。文件路径缺失时它们退化成上面 ⑭ 那一副样子：名了一档而那一档没有
    // 可读可写的东西 —— 两者都只会印一行错误而且退出码非零，但那一行说的是「路径是个 flag」，
    // 不是「你少给了一份档」。`--confirm` 与 `--import-all` 是修饰档不是门：单独点它们没有
    // 任何东西在被修饰。
    let path_after = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .filter(|v| !v.starts_with("--"))
            .cloned()
    };
    let named: Vec<&str> = FILE_COMMANDS.iter().copied().filter(|n| flag(n)).collect();
    for name in &named {
        if path_after(name).is_none() {
            orphans.push(format!(
                "{name} 需要在它后面给一份要{}的文件路径",
                if name.starts_with("--export") {
                    "写出"
                } else {
                    "读入"
                }
            ));
        }
    }
    if named.len() > 1 {
        orphans.push(format!(
            "{} 不同时点：一次调用只做一件档的事",
            named.join(" / ")
        ));
    }
    // `--import-all` 是「整份 settings 都盖」的范围档，而一份调色板档里只有那一排色、没有
    // 范围可言 —— 让它去修饰 `--import-palette` 会许一件那一档做不到的事。
    if flag("--import-all") && !flag("--import-config") {
        orphans.push(
            "--import-all 是 --import-config <档> 的附加档：调色板档只有那一排色，没有范围"
                .to_string(),
        );
    }
    if flag("--confirm") && !flag("--import-config") && !flag("--import-palette") {
        orphans.push("--confirm 是 --import-config / --import-palette <档> 的附加档".to_string());
    }
    if !named.is_empty() {
        let mut others: Vec<&str> = mask::GAUGE_FLAGS.to_vec();
        others.extend(EXTRA_DOORS);
        others.extend(["--selftest", "--desktop", "--hotkeys"]);
        // P170：贴图那一族的三枚名字从 [`cli::PARAM_DOC`] 数，不在这里再抄一遍——抄的这两处
        // （这一处与 `cli_reserved` 那一处）与帮助那一段是同一份名单的三个用处。
        others.extend(cli::snap_flags());
        let hit: Vec<&str> = others.into_iter().filter(|n| flag(n)).collect();
        if !hit.is_empty() {
            // 点名的只有**在场的那几档**：`--export-config a --mask 1500` 那一句里替
            // `--import-config` 也道个名字，是一句拒绝替一扇没被点的门许了一件它没做的事
            // （上面 `EXIT_DOORS` 那一族为同一件事写过一条）。
            orphans.push(format!(
                "{} 只动用户点名的那一份档，不与 {:?} 同开",
                named.join(" / "),
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
    // §5.15 的两条腿。`--hotcorner` 观测而**不派发**，产品那一趟（`--hotkeys` 装起来的常驻采样
    // 腿）会真派发：并列时这一档读到的触发次数就不是它自己那台机器的读数，那一次停留已经被
    // 别人消耗掉了。`--selftest` / `--desktop` 是同一条理由的另一面——它们在 `engine.load()`
    // 之前就 exit，这一档的报告永远排不到，并列时印出来的只是别人的读数。
    if flag("--hotcorner")
        && ["--hotkeys", "--selftest", "--desktop"]
            .iter()
            .any(|name| flag(name))
    {
        orphans.push(
            "--hotcorner 不能和 --hotkeys / --selftest / --desktop 一起用：那一档里常驻的采样腿会真派发角落动作，\
             而 --selftest / --desktop 在这档报告之前就退了；并列时印出来的都不是这一档自己的读数"
                .to_string(),
        );
    }
    // §5.20 的那一档，与 §5.18.2 的那一档 —— 它们点的是同一族门：**打完报告就退**。两条理由都
    // 在上面的注释里写过了，而这一格判的是「这一趟被点名了几扇那样的门」：两扇同开时只会有一扇
    // 的报告印出来，另一扇连跑都没跑，而退出码是前一扇的 —— 那是一次看起来成功的空测量，不是
    // 一句「这一档没跑」。名单从 [`EXIT_DOORS`] 来，所以加第三扇时这里不会漏（漏过一次的红是
    // P142/P143 那两对 `2 != 1`）。
    // `--settings`、`--tray` 与 `--whiteboard` 也不在 [`WINDOW_DOORS`] 那一格的名单里，理由是 #281：那一格的
    // `others` 读的也是这一族的名字，本档一进去它就先拦了，那一句照样点名双方；同一件并列在此处
    // 再拦一次就是同一枚判据写两份。
    let exit_doors: Vec<&str> = EXIT_DOORS.into_iter().filter(|n| flag(n)).collect();
    if exit_doors.len() > 1 {
        orphans.push(format!(
            "{} 不同时点：它们都在 engine.load() 之前打完自己的报告就退，并列时后点名的那一扇一次也没跑",
            exit_doors.join(" / ")
        ));
    }
    if !exit_doors.is_empty() {
        let mut others: Vec<&str> = mask::GAUGE_FLAGS.to_vec();
        others.extend(EXTRA_DOORS);
        others.extend(
            EXIT_DOORS
                .iter()
                .copied()
                .filter(|n| !exit_doors.contains(n)),
        );
        others.extend(["--selftest", "--desktop", "--hotkeys"]);
        others.extend(FILE_COMMANDS);
        others.extend(cli::snap_flags());
        let hit: Vec<&str> = others.into_iter().filter(|n| flag(n)).collect();
        if !hit.is_empty() {
            // 「会真写设置档」只有 `--settings` 那一扇成立，所以那句跟着它在不在场走：一句拒绝
            // 替另一扇门许一件它没做的事，读日志的人会照着那句去找一个不存在的落盘。
            let writes = if exit_doors.contains(&"--settings") {
                "会真写设置档、且"
            } else {
                ""
            };
            orphans.push(format!(
                "{} {writes}在 engine 之前就退，不与 {:?} 同开：并列时落盘次数与别人那趟的报告都会算到这一档头上",
                exit_doors.join(" / "),
                hit
            ));
        }
    }
    // §5.20.1 的那一扇窗，与 §5.20.3 的那两枚钮 —— 它们点的是同一扇窗。骨架与上面那一档相同
    // （会真写设置档、且它自己的报告要等 `app.exec()` 之后才读得到），多出来的是**一扇正常的窗
    // 在整个这一趟里都摆在桌面上**。场上每一扇别的门都在问「屏幕上此刻有哪些顶层窗」或「整屏
    // 照片里有什么」：`--pin` 挑的是与贴图矩形重叠的那一枚，`--hint` 按标题去照片里找它自己那一
    // 扇，`--mask` / `--ink` / `--autosave` / `--reveal` 拍的是整屏，`--probe` 与 `--r13` 数的是
    // 窗的清点，`--hotkeys` 按下那一瞬「前台是谁」决定静音。并列时这些读数里都多了一扇谁也没要
    // 的窗，而 `saves=` 那一格（本档「拒了的写不落盘」的读数）也会被别人的那一次写变成一句假话。
    // 所以两档与 `--settings` 同一条规矩：只许单独点。`--settings` 本身不在下面这张名单里 ——
    // 那一档的规矩是「拒一切并列」，而它的名单读的正是 `EXTRA_DOORS`，本档一进去它就先拦了，那
    // 一句也照样点名双方；同一件并列在此处再拦一次，是同一枚判据写两份（#281）。
    // `--export-config` / `--import-config` 也不在这张名单里，同一条理由：§5.20.3 那一族自己那条
    // 「不与任何门同开」的规矩读的也是 `GAUGE_FLAGS` + `EXTRA_DOORS`，本档一进去那一句就先拦了。
    // 这一枚不是事后补的说法——本轮第一次把两条并列写进单测时，那一对各交了**两句**话，红的就是
    // 那一个 `2 != 1`。
    //
    // P143 把第二扇窗那一档带进来时，同一枚 `2 != 1` 在这里等着：两档各写一个 `if`，并列时每个
    // `if` 都会拦一次、各交一句。所以这一格判的是**「这一趟被点名了几扇窗那一档」**，一句里把
    // 点名的都写出来，而不是给每档各修一条自己的规矩。两档同开还要另外拦一句：`settings_open`
    // 只有一枚、窗也只有一扇，两副计划递进同一扇窗是「谁的说法作数」没有答案的那种坏。
    let window_doors: Vec<&str> = WINDOW_DOORS.into_iter().filter(|n| flag(n)).collect();
    if window_doors.len() > 1 {
        orphans.push(format!(
            "{} 不同时点：一趟只有一扇设置窗、也只有一副计划，两档并列时窗那侧说的话没有作者",
            window_doors.join(" / ")
        ));
    }
    let history_doors: Vec<&str> = HISTORY_DOORS.into_iter().filter(|n| flag(n)).collect();
    if history_doors.len() > 1 {
        orphans.push(format!(
            "{} 不同时点：两档各钉各的历史落点，并列时后钉的那一枚替前一枚说话",
            history_doors.join(" / ")
        ));
    }
    if !window_doors.is_empty() {
        let mut others: Vec<&str> = mask::GAUGE_FLAGS.to_vec();
        others.extend(
            EXTRA_DOORS
                .iter()
                .copied()
                .filter(|n| !window_doors.contains(n)),
        );
        others.extend(["--selftest", "--desktop", "--hotkeys"]);
        let hit: Vec<&str> = others.into_iter().filter(|n| flag(n)).collect();
        if !hit.is_empty() {
            orphans.push(format!(
                "{} 会开一扇真的窗、也会真写设置档，不与 {:?} 同开：\
                 并列时窗清点、整屏照片与落盘次数都不是这一档自己的读数",
                window_doors.join(" / "),
                hit
            ));
        }
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
    // §5.7.22 步骤 4 与步骤 5 的两档，与上面两档同一副骨架、同一族拒绝（`FILE_COMMANDS`）、
    // 同一个「写失败不退 0」的规矩：`palette_export` 把 `std::fs::write` 的 Err 原样递回来，
    // `palette_import` 则在 [`Settings::save`] 把失败记进 `problems` 时递回那一句 —— 两者都
    // 走这一支退 1。退 1 而不是退 3 的分界与 §5.17 那三条规则一致： argv 是齐的，坏的是盘。
    if let Some(p) = path_after("--export-palette") {
        match settings::palette_export(std::path::Path::new(&p)) {
            Ok(line) => {
                print!("{line}");
                std::process::exit(0);
            }
            Err(e) => {
                println!("[palette] 失败：{e}");
                println!("用法：falconshot --export-palette <文件路径>");
                std::process::exit(1);
            }
        }
    }
    if let Some(p) = path_after("--import-palette") {
        match settings::palette_import(std::path::Path::new(&p), flag("--confirm")) {
            Ok(line) => {
                print!("{line}");
                std::process::exit(0);
            }
            Err(e) => {
                println!("[palette] 失败：{e}");
                println!("用法：falconshot --import-palette <文件路径> [--confirm]");
                std::process::exit(1);
            }
        }
    }

    // §5.17's command-line capture interface. Placed after the two config commands and
    // before `--selftest`: a bad parameter has to exit the process (rule 2) before any
    // window, screen, clipboard or gauge gets a turn — and after `refusals`, so a
    // mis-typed call is still refused in the door's own voice first. The reserved list is
    // built from the SAME `GAUGE_FLAGS`/`EXTRA_DOORS`/`EXIT_DOORS` the other refusals read,
    // so a §5.17 verb sharing a flag with a measurement door is refused rather than run
    // twice. `EXIT_DOORS` joins it in P144 because that "run twice" was real for this family:
    // the CLI branch exits inside its own `match`, so `--new --settings` used to capture a
    // picture and never print the door's report — a silent skip, which is exactly the shape
    // 第 47 项 ⑭ was written to refuse.
    let cli_reserved: Vec<&str> = mask::GAUGE_FLAGS
        .iter()
        .copied()
        .chain(EXTRA_DOORS)
        .chain(EXIT_DOORS)
        .chain(["--selftest", "--desktop", "--hotkeys"])
        .chain(FILE_COMMANDS)
        .chain(cli::snap_flags())
        .collect();
    let cli_decision = cli::classify(&args, &cli_reserved);
    // P169: PRD §5.17's 截取上一次区域 is addressed to "scripts, shortcuts and third
    // parties", and a script's call is a new process each time. The one line this opens
    // is what lets `--last` in process two repeat the rectangle process one captured;
    // every other way through this `match` leaves the store shut. Why the gate is a
    // function of the decision rather than a stanza inside the `Run` arm: `fn main` is
    // invisible to `cargo test` (§10 第 40 项 ③), so the rule "only a named capture verb
    // may touch that file" is graded in [`cli::opens_region_store`] instead.
    if cli::opens_region_store(&cli_decision) {
        capture::enable_region_store();
    }
    match cli_decision {
        cli::Decision::Help => {
            print!("{}", cli::help());
            std::process::exit(cli::EXIT_OK);
        }
        cli::Decision::Error(msg) => {
            println!("[falconshot] {msg}");
            print!("{}", cli::help());
            std::process::exit(cli::EXIT_USAGE);
        }
        cli::Decision::Run { select, output } => match output {
            // Clipboard: no window, so it finishes here. A frame that could not be
            // captured is `run_code(false, _)` = EXIT_FAIL; a clipboard the holder would
            // not take is `run_code(_, false)` = EXIT_FAIL. Neither is ever a success.
            cli::Output::Clipboard => {
                let code = match cli_frame(select) {
                    Err(e) => {
                        println!("[cli] 截图失败：{e}");
                        std::process::exit(cli::run_code(false, false));
                    }
                    Ok((frame, what, _)) => match platform_windows::clip::write_image(&frame) {
                        Ok(_) => {
                            println!("[cli] 已把{what}写入剪贴板。");
                            cli::run_code(true, true)
                        }
                        Err(e) => {
                            println!("[cli] 写剪贴板失败：{e}");
                            cli::run_code(true, false)
                        }
                    },
                };
                std::process::exit(code);
            }
            // File: same shape, with §5.17's two failure kinds split. A name the encoder
            // cannot write (`BadName`) is a parameter error — rule 2's code and the help.
            // A write that actually ran and failed (`Write`) is rule 3's: EXIT_FAIL, and
            // under no reading a success.
            cli::Output::File(path) => {
                let code = match cli_frame(select) {
                    Err(e) => {
                        println!("[cli] 截图失败：{e}");
                        std::process::exit(cli::run_code(false, false));
                    }
                    Ok((frame, _, tail)) => {
                        match save::write(&frame, &path, &settings::export(), "截图", "") {
                            // §5.3.5 校验规则第二行的「提示」在写文件这一腿的落点：`save` 那句是
                            // 「已保存到 …（截图 2972×1820 像素）。」，它只报改完之后的数，不报改过。
                            // 裁过就把那句接在后面，没裁它是空串 —— 完全在界内的那一趟一字不多。
                            Ok(said) => {
                                println!("[cli] {said}{tail}");
                                cli::run_code(true, true)
                            }
                            Err(save::Refusal::BadName(why)) => {
                                println!("[falconshot] {why}");
                                print!("{}", cli::help());
                                cli::EXIT_USAGE
                            }
                            Err(save::Refusal::Write(why)) => {
                                println!("[cli] 写文件失败：{why}");
                                cli::run_code(true, false)
                            }
                        }
                    }
                };
                std::process::exit(code);
            }
            // 贴图 and 启动普通截图 both open a window, so neither can finish before the
            // event loop. The picture goes into the pin table now (the ordering `--snap`
            // already proves — freeze and `state::add` before `engine.load`), then the run
            // falls through to `engine.load` + `app.exec` to show it; `--new` opens the
            // interactive mask instead. `plain` is false here, so Qt keeps its default of
            // quitting when the last window closes — closing the pin (or finishing the
            // mask) ends the run. The interactive completion is a real-machine A/B leg, not
            // this round's headless verifier.
            cli::Output::Pin => {
                if select == cli::Select::New {
                    match mask::open_from_freeze(true) {
                        Ok(opened) => println!(
                            "[cli] 启动普通截图：遮罩开了 {} 屏（{} ms）——拖完或关掉即退出",
                            opened.slots, opened.ms
                        ),
                        Err(e) => {
                            println!("[cli] 启动普通截图开不起来：{e}");
                            std::process::exit(cli::EXIT_FAIL);
                        }
                    }
                } else {
                    match cli_capture_to_pin(select) {
                        Ok(line) => println!("[cli] {line}"),
                        Err(e) => {
                            println!("[cli] 截图失败：{e}");
                            std::process::exit(cli::run_code(false, false));
                        }
                    }
                }
            }
        },
        // No §5.17 token in argv: selftest, the measurement doors, the resident app and
        // the config commands all land here and continue exactly as they did before this
        // round — the guarantee that makes the whole module additive.
        cli::Decision::NotCli => {}
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
        // §5.5.8「刷新截图」的那一档：它读的也是此刻的屏幕（钉一块真区域、按原坐标再读一次、
        // 再递上装不下那块区域的桌面），所以与 `--capture` 同一族做成 **opt-in** —— 不加这一档
        // 就 SKIPPED。它还会在名单上钉贴图（覆盖那一臂、另钉一张那一臂、从文件来的那一枚），
        // 收梢那一格判的就是"名单回到本档开工时的样子"，因此绝不在默认那一趟里跑。
        if flag("--refresh") {
            let (verdict, rreport) = refresh_check::measure();
            println!("{rreport}");
            code = verdict_code(code, verdict);
        } else {
            println!(
                "refresh selftest: SKIPPED (pass --refresh to run it; it pins regions of the real screen)"
            );
        }
        // §5.5.10「发送到其他应用」的那一档。它与上面四档同一族（opt-in、SKIPPED 那一行照印），
        // 但它是这一台机器上**唯一一档对面在别的进程里**的门：真走一次 `ShellExecuteW`，把一张
        // 真截图的落盘路径作为参数交给一枚真 `.cmd`，再读那一位自己写下的话。所以它闪控制台窗
        // （`.cmd` 由 cmd 解释器跑），也所以它排在 `--refresh` 之后 —— 那一档要动屏幕与名单，
        // 这一档要动 PATH 与内存里的 output.send_to_apps，两档各自收梢后才不会读到彼此的账。
        // 写只落在 `FALCONSHOT_SENDTO_DOOR_DIR` 里钉过的那一份，用户的 config.toml 逐字节不动
        // （§9.1 第十五条，那一格就在报告里）。
        if flag("--sendto") {
            let (verdict, sreport) = sendto_check::measure();
            println!("{sreport}");
            code = verdict_code(code, verdict);
        } else {
            println!(
                "sendto selftest: SKIPPED (pass --sendto to run it; it dispatches a real file to a real program and flashes a console window)"
            );
        }
        std::process::exit(code);
    }

    // §5.20.1 / §5.20.2 的那一扇门：设置页的模型。它不开窗，因为在它这一层上「页上显示的是
    // 哪一行」、「改完之后谁读到了它」、「拒了的写动没动过值」三件事都不必经过像素；一扇真窗
    // 接的是同一套读数，那是下一轮的账。
    //
    // 排在 `--selftest` 之后、`--desktop` 之前：这一档会**真写一份文件**，所以它必须在任何
    // 会开遮罩、会走 `write_back` 的腿之前把落点钉走（[`settings_view::pin`]），也必须在这
    // 一台机器开始等真人之前退。
    if flag("--settings") {
        let pinned = settings_view::pin();
        println!("[settings pin] 写回钉到 {}", pinned.display());
        let (verdict, report) = settings_view::measure();
        println!("{report}");
        std::process::exit(verdict_code(0, verdict));
    }

    // §5.18.2 的那一扇门：四枚鼠标键各跑哪一枚动作。它与上面那一档同族（不开遮罩、不写用户的
    // 档、报告打完就退），所以它共享 `EXIT_DOORS` 那一条拒绝；它也不等任何人 —— 四枚键在
    // 这一层读到的全是「一份档 → 一个决定」，锁屏会话也判得出。真往消息窗投一次点击的那一腿
    // 这一档不装作量过：那一格发的是 BLOCKED，写在那份报告的最后。
    if flag("--tray") {
        let (verdict, report) = tray_click_check::measure();
        println!("{report}");
        std::process::exit(verdict_code(0, verdict));
    }

    // §5.4.4 / §5.4.5 的那一扇门：白板那两枚键读回来是什么、那一身底色做出来的画布真的是什么、
    // 一块透明板交给每一种格式各会发生什么。与上面两档同族（不开遮罩、不写用户的档、报告打完
    // 就退），所以它共享 `EXIT_DOORS` 那一条拒绝，也同 `--tray` 一样不带毫秒数、不进闹钟名单。
    // 它这一趟发的是 BLOCKED（退出码 3）而不是 PASS：§5.4.4 第五步那三枚出口要一扇真窗才问得
    // 出对面，这一档不拿画布像素冒充屏幕上的一块白布。
    if flag("--whiteboard") {
        let (verdict, report) = whiteboard_check::measure();
        println!("{report}");
        std::process::exit(verdict_code(0, verdict));
    }

    // §5.21.1 的那一扇门：托盘那枚「检查更新」有没有归宿、那份档读不读得到、那句判决里三样东西
    // 齐不齐。与上面三档同族（不开遮罩、不写用户的档、报告打完就退），所以它共享 `EXIT_DOORS`
    // 那一条拒绝，也同 `--tray` 一样不带毫秒数、不进闹钟名单。
    // 它这一趟发的是 BLOCKED（退出码 3）而不是 PASS：那枚菜单项这一档一次也没有真点过，第 4 步
    // 「显示」里那一枚**可以点的**下载入口也还没有 —— 那两格写在自己报告的最后，不拿上面的读数冒充。
    if flag("--update-check") {
        let (verdict, report) = update_check::measure();
        println!("{report}");
        std::process::exit(verdict_code(0, verdict));
    }

    // §5.21.2 / §5.18.1 的那一扇门：托盘第 1440 行那枚「帮助」有没有归宿、PRD 那一节点名的四行是不是
    // 还各有作者。与上面四档同族（不开遮罩、不写用户的档、报告打完就退），所以它共享 `EXIT_DOORS`
    // 那一条拒绝，也不带毫秒数、不进闹钟名单。它发 BLOCKED（退出码 3）而不是 PASS：那枚菜单项本档
    // 一次也没真点过，那一格写在自己报告的最后。
    if flag("--help-menu") {
        let (verdict, report) = help_menu::measure();
        println!("{report}");
        std::process::exit(verdict_code(0, verdict));
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
    let snap_open = cli::snap_verb(&args);
    // 本轮把「与 --snap 同开」这句话写进了文档那一行，所以它得有人照着拒。P168 关掉的是「点了
    // `--rect` 不给值，被读成没给」；这是它的另一半——值给了、入口没给（一枚都不点，或点的是那枚
    // 从不读矩形的 `--snap-window`）。那两种以前都安静地什么也不发生，而现在它们与规则 2 同一副
    // 嗓子：一句点名的拒绝、一份现在认得 `--rect` 的帮助、一个 3。
    if cli::rect_orphan(&args, snap_open) {
        println!(
            "[falconshot] --rect 是贴图那一族的修饰档：它只在 {} 那一臂被读，单独点它不钉任何东西",
            cli::SNAP_CENTRE
        );
        print!("{}", cli::help());
        std::process::exit(cli::EXIT_USAGE);
    }
    let snapping = snap_open.is_some();
    // §5.17 规则 1/2 在这一族的落点。上一版这三条臂的 `Err` 只印一句「snap failed」，收梢留给
    // `code = 0`，于是拒掉的那一腿与钉成的那一腿在 cmd.exe 手里是同一个数（P167 ⑬ 记的就是这一格）。
    let mut snap_status: Option<cli::Snap> = None;
    if snapping {
        // P170：哪一枚旗标开得着这一族、哪一枚说的是「指针下面那一扇」，都从 [`cli::PARAM_DOC`]
        // 那一族数出来 —— 以前这里手抄两枚名字，帮助里却一个字不讲，于是规则 2 交回的那一份文档
        // 解释不了它刚刚拒掉的那枚旗标。
        let outcome = if cli::is_snap_window(snap_open) {
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
            // `--rect` absent is the flag's documented default (a centre snap); `--rect` present and
            // unreadable is a refusal, and until P165 it silently became the default too — a typo like
            // `--rect 10,20,0,0` pinned a 1×1 square and printed the centre rectangle's timing line, so
            // nothing on the terminal said the number was the problem. P168 closes the other half of
            // that shape: `--rect` **present with nothing after it** read as "absent" too, so the call
            // took the middle of the screen while the user had asked for a rectangle by name.
            match cli::rect_slot(&args) {
                cli::RectSlot::Absent => capture::snap_centre(400, 250),
                cli::RectSlot::Orphan => {
                    snap_status = Some(cli::Snap::BadParam);
                    Err(
                        "--rect 后面要跟四个整数 x,y,w,h（例如 `--rect 100,100,400,300`）：点了这一枚旗标就不当作没给"
                            .to_string(),
                    )
                }
                cli::RectSlot::Given(raw) => match capture::rect_arg("--rect", raw) {
                    Ok(r) => capture::pin_rect(&r)
                        .map(|(id, line)| (id, format!("{line} 自己冻结的整屏"))),
                    Err(why) => {
                        snap_status = Some(cli::Snap::BadParam);
                        Err(why)
                    }
                },
            }
        };
        match outcome {
            // The id used to be spelled a second time here (`snapped pin {id}: {what}`) while the
            // sentence in `{what}` said nothing about a pin at all. Now this arm says the pin and its
            // id through the one author every other leg that pins something already goes through.
            Ok((id, what)) => {
                snap_status = Some(cli::Snap::Pinned);
                println!(
                    "[falconshot] {} {}",
                    capture::pinned_line(id, &what),
                    state::desktop_summary()
                );
            }
            Err(e) => {
                // 上面那两支参数错自己点过名；剩下的 `Err` 都是屏幕那一只手拒的，拿规则 1 的码。
                snap_status.get_or_insert(cli::Snap::CaptureFailed);
                println!("[falconshot] snap failed: {e}");
                if snap_status == Some(cli::Snap::BadParam) {
                    // §5.17 规则 2：参数错误时应输出帮助信息。
                    print!("{}", cli::help());
                }
            }
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

    // §5.15 热区那一档。它与上面几档最不一样的一处是**它不需要一张遮罩**：角落读的是指针、
    // 显示器清单和前台窗的矩形，三样都不经屏幕采集那一条路，也就没有 `gauge_run` 可挡它。
    // 它也不派发任何动作 —— 派发是产品那一条常驻采样腿的事，量它的那一趟要是顺手收起一次
    // 全桌贴图，这一趟读的就不是桌面而是自己的手。
    let corner_want = after("--hotcorner");
    let corner_ms = corner_want.map(|ms| ms.max(hot_corner::MIN_LIVE_MS));

    // §5.20.1 的那一扇窗那一档。它与 `--hotcorner` 同一族（不开遮罩、不冻屏），与 `--autosave`
    // 同一副骨架（**前置必须写在 `engine.load()` 之前**：`SettingsWindow.qml` 只在
    // `Component.onCompleted` 读一次计划，晚一行窗就照着没有计划的那一面长）。
    // 写回先钉到本档自己的档上 —— 这一档那五条腿里有两条是真写档里的值（保存那一腿与开关那一
    // 腿；第五条是 §5.21.2 那一腿，它写的不是档而是引导状态那份文件，量具用 `FALCONSHOT_GUIDE_DIR`
    // 把它的落点也钉走），用户的
    // `%APPDATA%\ai-falconshot\config.toml` 不是这一趟的草稿纸（§9.1 第十五条）。
    let window_want = after("--settings-window");
    let window_ms = window_want.map(|ms| ms.max(settings_view::MIN_WINDOW_MS));
    if let Some(ms) = window_ms {
        let pinned = settings_view::pin();
        println!("[settings-window pin] 配置写回钉到 {}", pinned.display());
        // 计划里那一枚腿等待与本档等的总时长是两个数：前者是窗开起来之后隔多久跑腿，后者是
        // `quit_after` 的闹钟。各印在各自行里，不合并成一枚读数（同 `--hotcorner` 那一档）。
        println!("[settings-window arm] {}", settings_view::arm());
        println!(
            "[settings-window wait] 窗与五条腿等 {ms} ms（计划给的那枚腿等待 {} ms，地板 {} ms）",
            settings_view::LEG_MS,
            settings_view::MIN_WINDOW_MS
        );
    }

    // §5.20.3 那一档：同一扇窗，另一副计划。前置与上面那一档同一形状（**写在 `engine.load()`
    // 之前**，窗只在 `Component.onCompleted` 读一次计划），写回也钉在同一枚落点上 —— 这一档的
    // 腿里有一次是真的 `apply_import` 加一次真的 `save`，用户那份 `%APPDATA%` 档不是草稿纸。
    //
    // `arm_io` 会把要导入的那份档**现在就种在盘上**（一行日志说出种了哪两枚键），因为步骤 3 说
    // 的是「在另一台设备上选一份档」：本机上没有第二台，只有种出来的这一份能问出「会盖掉哪几枚」。
    let io_want = after("--settings-io");
    let io_ms = io_want.map(|ms| ms.max(settings_view::MIN_IO_MS));
    if let Some(ms) = io_ms {
        let pinned = settings_view::pin();
        println!("[settings-io pin] 配置写回钉到 {}", pinned.display());
        println!("[settings-io arm] {}", settings_view::arm_io());
        println!(
            "[settings-io wait] 窗与那两枚钮等 {ms} ms（计划给的那枚腿等待 {} ms，地板 {} ms）",
            settings_view::LEG_MS,
            settings_view::MIN_IO_MS
        );
    }

    // §5.7.22 步骤 1/2/3 那一档：还是那一扇窗、第三副计划。前置与上面两档同一形状（**写在
    // `engine.load()` 之前**，窗只在 `Component.onCompleted` 读一次计划），写回也钉在同一枚落点
    // 上 —— 这一档的每一条腿都是真写 `annotation.palette` / `annotation.palette_schemes` 两枚键，
    // 用户那份 `%APPDATA%\ai-falconshot\config.toml` 不是草稿纸（§9.1 第十五条）。
    //
    // `arm_pal` 会先把方案名单里**种进一枚**（日志说出种的是哪一枚名字与哪三枚色），因为步骤 3
    // 说的是「保存为调色板方案」、而这一档还要量「应用那一枚已有方案」：场上没有已有的方案，
    // 「应用」那一条腿就没有对面，只有种出来的那一枚能问出「板条换成了哪几枚色」。
    let pal_want = after("--settings-palette");
    let pal_ms = pal_want.map(|ms| ms.max(settings_view::MIN_PAL_MS));
    if let Some(ms) = pal_ms {
        let pinned = settings_view::pin();
        println!("[settings-palette pin] 配置写回钉到 {}", pinned.display());
        println!("[settings-palette arm] {}", settings_view::arm_pal());
        println!(
            "[settings-palette wait] 窗与那八枚钮里的六按等 {ms} ms（计划给的那枚腿等待 {} ms，地板 {} ms）",
            settings_view::LEG_MS,
            settings_view::MIN_PAL_MS
        );
    }

    // §5.14.4 那一档：又一扇由 Rust 决定存在与否的窗，第三副计划。前置与上面两档同一形状
    // （**写在 `engine.load()` 之前**，`HistoryWindow.qml` 只在 `Component.onCompleted` 读一次
    // 计划），而钉落点与种那五行也必须在那一刻之前 —— 窗一出生就要在清单里看见它们。
    //
    // 这一档**把历史落点钉进本档自己的沙盒**：一枚带 pid 的临时目录（`FALCONSHOT_HISTORY_DOOR_DIR`
    // 可覆盖），钉之前改掉的是 `MaskState` 那两枚**会话快照**，用户那份
    // `%APPDATA%\ai-falconshot\config.toml` 与 `history\` 目录在这一趟里一字也不动（§9.1 第十五条）。
    // 它与 `--autosave` 的并列由上面 `HISTORY_DOORS` 那一句拦下：两档各钉各自的历史落点，
    // 并列时后钉的那一枚替前一枚说话。
    let history_want = after("--history-window");
    let history_ms = history_want.map(|ms| ms.max(history_window::MIN_WINDOW_MS));
    if let Some(ms) = history_ms {
        println!("[history-window arm] {}", history_window::arm());
        println!(
            "[history-window wait] 窗与它那些腿等 {ms} ms（计划给的那枚腿等待 {} ms，地板 {} ms）",
            history_window::LEG_MS,
            history_window::MIN_WINDOW_MS
        );
    }

    // §5.3.5 那一档：又一扇由 Rust 决定存在与否的窗，另一副计划。前置与上面两档同一形状
    // （**写在 `engine.load()` 之前**，`CustomCaptureWindow.qml` 只在 `Component.onCompleted`
    // 读一次计划）。
    //
    // 这一档**不钉写回落点**：对话框那五格里没有任何一枚是持久键，按下「开始截图」做的是一次
    // 冻面加一枚贴图，而延时那一格读的是 `[capture] delay_seconds` —— 读，不是写。用户那份
    // `%APPDATA%\ai-falconshot\config.toml` 在这一趟前后应当一字同（§9.1 第十五条），落账时
    // 由门外的尺量那一枚 sha256，而不是靠这一句注释。
    let custom_want = after("--custom-window");
    let custom_ms = custom_want.map(|ms| ms.max(custom::MIN_WINDOW_MS));
    if let Some(ms) = custom_ms {
        println!("[custom-window arm] {}", custom::arm());
        println!(
            "[custom-window wait] 窗与它那些腿等 {ms} ms（计划给的那枚腿等待 {} ms，地板 {} ms）",
            custom::LEG_MS,
            custom::MIN_WINDOW_MS
        );
    }

    // §5.12.3 / §5.12.4 那一档：又一扇由 Rust 决定存在与否的窗，第四副计划。前置与上面三档同一
    // 形状（**写在 `engine.load()` 之前**，`GroupWindow.qml` 只在 `Component.onCompleted` 读一次
    // 计划），而种那三组三枚也必须在那一刻之前 —— 窗一出生就要在那一列里看见甲乙丙三行。
    //
    // 这一档**不钉任何落点**：分组与贴图**不住在 `config.toml` 里**（`crates/core/src/config.rs`
    // 那一行「groups 不住 config.toml、没有键供它们过档」），本档种的一切都在进程私有的 `PinSet`
    // 内存里、门打完结散；它一次也不叫 `write_back`。用户那份
    // `%APPDATA%\ai-falconshot\config.toml` 在这一趟前后应当一字同（§9.1 第十五条），落账时
    // 由门外的尺量那一枚 sha256，而不是靠这一句注释。
    let group_want = after("--group-window");
    let group_ms = group_want.map(|ms| ms.max(group_window::MIN_WINDOW_MS));
    if let Some(ms) = group_ms {
        println!("[group-window arm] {}", group_window::arm());
        println!(
            "[group-window wait] 窗与它那些腿等 {ms} ms（计划给的那枚腿等待 {} ms，地板 {} ms）",
            group_window::LEG_MS,
            group_window::MIN_WINDOW_MS
        );
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
        launch = match hotkey::install(plain) {
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
    let mut legs = hotkey::Legs {
        setting: ("", false),
        menu: ("", Err("这一档没点过菜单")),
    };
    if hotkeys_ms.is_some() {
        // §5.1.1 / §5.20.1：这一发排在四把键之前，而次序是读数的一部分。`gate` 会让任何一次
        // 「遮罩已经开着」的工作原地退回，而 `capture` 那一支正是开遮罩的 —— 把这一发放到它们
        // 之后，这一格就成了两条线程之间谁先跑的一次赌博，而它要证的是「菜单里那一项被选中
        // 之后屏幕上出现那扇窗」本身，不是赌赢了。
        legs.menu = ("settings", hotkey::press_menu("settings"));
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
        legs.setting = (
            "ImmersiveColorSet",
            hotkey::notify_setting_change("ImmersiveColorSet"),
        );
        // §5.18.2 第四步那四发不投在这里。A/B 两代投在这里时，同一趟 `tray menu opens the
        // settings window` 的顶层窗普查从 P174 那一代的 1 枚读成 0 枚——`capture` 那一支把遮罩
        // 开起来，而那一格读的就是屏幕上此刻的窗。两格共读一个场景，谁后跑谁改写谁的读数，所以
        // 那一格连它自己那四发一起搬到 [`hotkey::report`] 里所有读场景的格子之后（那里的注释写着
        // 为什么它不需要事件环再跑一趟）。
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
    if let Some(ms) = corner_ms {
        // 这一档等的不是任何一条腿：它要的只是「这一趟会自己结束」，而不是把一个只读指针的门
        // 变成一次等真人的会话。真正的采样跑在 `exec()` 之后，所以这一趟的墙钟是 `ms + live`
        // —— 那两个数各自印在自己的行里，不在这里合并成一枚读数。
        state::quit_after(ms);
        armed.push("--hotcorner");
    }
    if let Some(ms) = hotkeys_ms {
        // The presses are already posted by now, so the wait is the chain's: pump to
        // listener, listener to the Qt queue, and a freeze plus N presented frames.
        state::quit_after(ms);
    }
    if let Some(ms) = window_ms {
        // 这一档等的是窗自己：`createObject` → 第一帧 → 计划里那一枚腿等待 → 五条腿过一次桥
        // （逐页那一趟自 P160 起每页两次过桥，引导那一腿还要真写一份文件，所以地板从 1600 抬到
        // 2400 —— 见 `settings_view::MIN_WINDOW_MS` 头顶那句）。
        // 地板 `MIN_WINDOW_MS` 已经把这三段都算在里面（它比 `LEG_MS` 宽四倍），所以这里加的
        // 不是宽限而是**这一趟会自己结束**那一格 —— 缺了它，`app.exec()` 就变成一次等真人的
        // 会话，而那一副样子正是 ⑭ 与上面那张闹钟表一起挡的。
        state::quit_after(ms);
        armed.push("--settings-window");
    }
    if let Some(ms) = io_ms {
        // 与上面那一档同一个理由：这一趟会自己结束，而不是开一张窗等真人。窗那一侧的那几条腿
        // 与这一档的四步（导出、预览、确认、还原）都在 `MIN_IO_MS` 这块地板里算过。
        state::quit_after(ms);
        armed.push("--settings-io");
    }
    if let Some(ms) = pal_ms {
        // 与上面两档同一个理由：这一趟会自己结束，而不是开一扇窗等真人。这一档的地板比
        // `MIN_IO_MS` 高一截，多出来的那一截是**六条腿各要过一次桥**（加、挪、删、改、存、
        // 应用），每一按都真的碰盘，收尾还原那两次也算在里面。
        state::quit_after(ms);
        armed.push("--settings-palette");
    }
    if let Some(ms) = history_ms {
        // 这一档等的是窗自己：造窗 → 第一帧 → 八条腿过一次桥（每条都是同步的一问一答，
        // 没有一枚要真数完的倒计时，所以地板比 `custom` 低一截）。缺了这一行，`app.exec()`
        // 就变成一次等真人的会话，而那一副样子正是 ⑭ 与钟表上面那张表一起挡的。
        state::quit_after(ms);
        armed.push("--history-window");
    }
    if let Some(ms) = custom_ms {
        // 这一档的地板比上面两档高一截，多出来的那一截是**延时那一条腿**：它要真的数满
        // `DELAY_LEG_SECONDS` 枚秒（`delay::STEP_MS` 一枚一秒），而不是把倒计时写成一句读数。
        // 「画面冻在数满那一刻」这一条只有让表真的走完才量得到（§5.2.4 第 420 行同一条时刻）。
        state::quit_after(ms);
        armed.push("--custom-window");
    }
    if let Some(ms) = group_ms {
        // 这一档等的也是窗自己：造窗 → 第一帧 → 十条腿过一次桥。那六只手全是同步的一问一答，
        // 没有一枚倒计时要数完，也没有一格要真的把一扇贴图窗搬上屏幕（种下的那三枚贴图开自己的
        // 窗，走的是 `count` 那条腿）。缺了这一行就是 ⑭ 挡的那一副样子。
        state::quit_after(ms);
        armed.push("--group-window");
    }
    // 贴图那一族被参数错拒掉、而这趟没有任何闹钟：给它一格，否则规则 2 的那一份帮助打印完之后就
    // 是一次等真人的会话，那个 3 交不回 shell。判据住在 [`cli::snap_refusal_needs_alarm`]，四格由
    // `--selftest` 量；钉成的那一腿（`Pinned`）不在此列，它等真人是产品行为。
    if cli::snap_refusal_needs_alarm(snap_status, armed.len()) {
        state::quit_after(cli::SNAP_REFUSE_MS);
        armed.push("snap-refused");
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

    // §5.17 规则 1：`--snap` 那一族被拒的那一腿，到此不再交回 0。这一格排在 `app.exec()` 与
    // `--probe` 之后有两个理由：一是 P167 ⑤ 那一份「pins=0」的普查证人要在同一趟里读得到，早退就
    // 没有了；二是别的腿已经报过非零时不该被这一格盖掉，所以只在 `code == 0` 时落下去。
    if let Some(snap_exit) = snap_status.and_then(cli::snap_code) {
        if code == 0 {
            code = snap_exit;
        }
        println!("[falconshot] snap 收梢 = {snap_exit}（本趟交回 {code}）");
    } else if snapping {
        println!("[falconshot] snap 收梢 = 钉成的那一腿，码留给整趟运行（本趟交回 {code}）");
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
    if let Some(ms) = corner_ms {
        // 报告的四格里只有两格是判据（headless 与 sweep），另外两格印的是这台机器此刻的几何
        // 与指针 —— 「这一台答不出」在那两格里是 BLOCKED 而不是 FAIL，锁屏与会话 0 都读得出
        // 前两格，正是 §9.1 那一族门要的分工。
        let (verdict, report) = hot_corner::measure(corner_want.unwrap_or(ms));
        println!("{report}");
        println!(
            "[hotcorner after {ms} ms] 要求 {} ms 生效 {ms} ms，派发=0（本档只观测），qml_loaded={} {} {}",
            corner_want.unwrap_or(ms),
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = window_ms {
        // 六格里读的全是**窗那一侧**递回来的话：本档手里那份模型只用来对，不用来说（与
        // `--settings` 那一档的分工写在 `settings_view::window_measure` 头顶）。窗没递回任何
        // 一行 ⇒ BLOCKED 而不是 FAIL：那一格问的是「这一台机器上有没有那扇窗」，而 QML 文档
        // 编译不过与页上少一行是两种坏。
        let (verdict, report) = settings_view::window_measure(window_want.unwrap_or(ms));
        println!("{report}");
        println!(
            "[settings-window after {ms} ms] 要求 {} ms 生效 {ms} ms，腿等待 {} ms，qml_loaded={} {} {}",
            window_want.unwrap_or(ms),
            settings_view::LEG_MS,
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = io_ms {
        // 这一档读的还是**窗那一侧**递回来的话，但记的是另一批行名（`settings io` 开头那五格）。
        // 与上面那一档同一分工：本档手里那份模型只用来对，不用来说。窗没递回任何一行 ⇒
        // BLOCKED —— 这一档的对面同样是那扇真窗，而 QML 编译不过与页上少一格是两种坏。
        let (verdict, report) = settings_view::io_measure(io_want.unwrap_or(ms));
        println!("{report}");
        println!(
            "[settings-io after {ms} ms] 要求 {} ms 生效 {ms} ms，腿等待 {} ms，qml_loaded={} {} {}",
            io_want.unwrap_or(ms),
            settings_view::LEG_MS,
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = pal_ms {
        // 这一档读的还是**窗那一侧**递回来的话（`settings palette` / `palette …` 那八格），加上
        // 本档现读的那份钉住的档。与上面两档同一分工：`settings_view::pal_measure` 手里那份模型
        // 只用来对，不用来说。窗一行也没递回来 ⇒ BLOCKED。
        let (verdict, report) = settings_view::pal_measure(pal_want.unwrap_or(ms));
        println!("{report}");
        println!(
            "[settings-palette after {ms} ms] 要求 {} ms 生效 {ms} ms，腿等待 {} ms，qml_loaded={} {} {}",
            pal_want.unwrap_or(ms),
            settings_view::LEG_MS,
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = history_ms {
        // 九格里读的全是**窗那一侧**递回来的话，与上面两档同一分工：`history_window` 手里那份
        // 模型只用来对（它种下的行号与字节数），不用来说。窗一行也没递回来 ⇒ BLOCKED。
        let (verdict, report) = history_window::measure(history_want.unwrap_or(ms));
        println!("{report}");
        println!(
            "[history-window after {ms} ms] 要求 {} ms 生效 {ms} ms，腿等待 {} ms，qml_loaded={} {} {}",
            history_want.unwrap_or(ms),
            history_window::LEG_MS,
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = custom_ms {
        // 七格里读的全是**窗那一侧**递回来的话，与上面两档同一分工：`custom` 手里那份模型只用
        // 来对，不用来说。窗一行也没递回来 ⇒ BLOCKED —— QML 文档编译不过与页上少一格是两种坏。
        let (verdict, report) = custom::measure(custom_want.unwrap_or(ms));
        println!("{report}");
        println!(
            "[custom-window after {ms} ms] 要求 {} ms 生效 {ms} ms，腿等待 {} ms，qml_loaded={} {} {}",
            custom_want.unwrap_or(ms),
            custom::LEG_MS,
            state::qml_loaded(),
            exe_stamp(),
            state::desktop_summary()
        );
        code = verdict_code(code, verdict);
    }
    if let Some(ms) = group_ms {
        // 十一格里读的全是**窗那一侧**递回来的话，与上面三档同一分工：`group_window` 手里那份
        // 模型只用来对（它种下的编号），不用来说。窗一行也没递回来 ⇒ BLOCKED —— QML 文档编译
        // 不过与页上少一格是两种坏。
        let (verdict, report) = group_window::measure(group_want.unwrap_or(ms));
        println!("{report}");
        println!(
            "[group-window after {ms} ms] 要求 {} ms 生效 {ms} ms，腿等待 {} ms，qml_loaded={} {} {}",
            group_want.unwrap_or(ms),
            group_window::LEG_MS,
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
                legs,
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
    use super::{capture, cli_pinned_line, exe_stamp, refusals, unalarmed, FILE_COMMANDS};
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
            15,
            "本轮的门是十五枚（P183 加进 `--settings-palette`）：多一枚少一枚都要先改这条再改代码"
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
            &argv(&[
                "--selftest",
                "--clipboard",
                "--capture",
                "--history",
                "--refresh",
                "--sendto"
            ]),
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

        // §5.15 那一档的拒绝臂。它自己那一枚数是被 ⑭ 那条通用规则接住的（少了 `<ms>` 就落在
        // `numbered` 那句里），这一条管的是**组合**：`--hotkeys` 那一趟里常驻的采样腿会真派发，
        // `--selftest` 与 `--desktop` 则在这档报告之前就 exit —— 两种并列都印不出这一档自己的
        // 读数，而那正是 第 47 项 ⑭ 那一族"看着像测量、其实是别人的场次"。
        for (other, number) in [
            ("--hotkeys", Some("1500")),
            ("--selftest", None),
            ("--desktop", None),
        ] {
            // 双方都要带齐自己那一枚数：`--hotkeys` 少了自己的毫秒数时，⑭ 那条通用规则也会
            // 拒一句，于是这一格读的是「少数字」而不是「并列」——证人要证的那一件事只有一件。
            let mut flat = vec!["--hotcorner", "1500", other];
            if let Some(ms) = number {
                flat.push(ms);
            }
            let clash = refusals(&argv(&flat), false);
            assert_eq!(clash.len(), 1, "{other} 并列时拒了几句：{clash:?}");
            assert!(
                clash[0].contains("--hotcorner") && clash[0].contains(other),
                "{other} 的并列没在自己那句里点名双方：{}",
                clash[0]
            );
        }
        // 单独点它、带着数字，必须一句都不拒：这一档不开遮罩，也就没有 `gauge_run` 那道结构性
        // 拒绝——上面 `--reveal` 那两条正是反面的样子。
        assert!(refusals(&argv(&["--hotcorner", "1500"]), false).is_empty());
        assert!(
            refusals(&argv(&["--hotcorner", "1500"]), true).is_empty(),
            "--hotcorner 读的是指针与显示器，不该被量具模式拒"
        );
    }

    /// §5.20 那一档两条理由都在上一条注释里写过了，所以它的判据也就那两件：**单独点它必须
    /// 沉默**（否则这一档在真机上跑不起来，而它是这一轮唯一的运行证人），**与别人并列必须点名
    /// 双方且只拒一句**（否则一句写错的调用会被归档到别的模式名下，⑭ 那一条讲的正是这件事）。
    /// `--mask` 那一条走的是 `GAUGE_FLAGS` 那张表，`--selftest` 走的是显式名单，两族各测一枚 ——
    /// 这张表以后少列了一族时，红的会是这一格而不是下一次真机运行。
    #[test]
    fn the_settings_door_refuses_company_and_names_both_sides() {
        assert!(
            refusals(&argv(&["--settings"]), false).is_empty(),
            "单独点 --settings 被拒了：它不开遮罩，也不该被量具模式拒"
        );
        assert!(
            refusals(&argv(&["--settings"]), true).is_empty(),
            "--settings 读的是配置模型自己，gauge_run 那道结构性拒绝与它无关"
        );
        for (other, extra) in [
            ("--selftest", None),
            ("--desktop", None),
            ("--hotkeys", Some("1500")),
            ("--export-config", Some("out.toml")),
        ] {
            // 双方都要带齐自己那一份参数：`--hotkeys` 少了自己的毫秒数时，⑭ 那条通用规则也会
            // 拒一句，于是这一格读的是「少数字」而不是「并列」——证人要证的那一件事只有一件。
            let mut flat = vec!["--settings", other];
            if let Some(x) = extra {
                flat.push(x);
            }
            let clash = refusals(&argv(&flat), false);
            assert_eq!(clash.len(), 1, "{other} 并列时拒了几句：{clash:?}");
            assert!(
                clash[0].contains("--settings") && clash[0].contains(other),
                "{other} 的并列没在自己那句里点名双方：{}",
                clash[0]
            );
        }
        let mask = refusals(&argv(&["--settings", "--mask", "1500"]), false);
        assert_eq!(mask.len(), 1, "{mask:?}");
        assert!(
            mask[0].contains("--settings") && mask[0].contains("--mask"),
            "{}",
            mask[0]
        );
    }

    /// §5.18.2 那一档与上面那一档同族：**报告打完就退**，所以它的拒绝同形——单独点它必须沉默，
    /// 与别人并列必须点名双方且只拒一句。这一族特有的是第一条组合：`--settings --tray` 是同族
    /// 并列，两扇同开时只有一句（P142/P143 那两对 `2 != 1` 都是「给每档各写一条规矩」的形状，
    /// 而这一格的名单 `EXIT_DOORS` 就是为了让第三扇加进来时这里不会漏）。
    ///
    /// 还有一条与 `EXTRA_DOORS` 那几档相反的：`--tray` **不带毫秒数**，所以⑭ 那条通用规则不许
    /// 拒它，闹钟名单里也不许点它 —— 它打完报告自己退，没人替它等。这两格问的就是「它没进那两
    /// 张表，是不是真的没进」。
    #[test]
    fn the_tray_door_refuses_company_and_needs_no_alarm() {
        assert_eq!(
            crate::EXIT_DOORS.len(),
            5,
            "这一族的门是五枚（`--settings`、P144 的 `--tray`、P147 的 `--whiteboard`、P173 的 \
             `--update-check` 与 P174 的 `--help-menu`）：多一枚少一枚都要先改这条再改码"
        );
        assert!(
            refusals(&argv(&["--tray"]), false).is_empty(),
            "单独点 --tray 被拒了：{:?}",
            refusals(&argv(&["--tray"]), false)
        );
        assert!(
            refusals(&argv(&["--tray"]), true).is_empty(),
            "--tray 读的是配置模型与那张路由表，gauge_run 那道结构性拒绝与它无关"
        );
        for (other, extra) in [
            ("--settings", None),
            ("--selftest", None),
            ("--desktop", None),
            ("--hotkeys", Some("1500")),
            ("--export-config", Some("out.toml")),
            ("--import-config", Some("in.toml")),
            ("--mask", Some("1500")),
            ("--settings-window", Some("1500")),
            ("--hint", Some("1500")),
        ] {
            let mut flat = vec!["--tray", other];
            if let Some(x) = extra {
                flat.push(x);
            }
            let clash = refusals(&argv(&flat), false);
            assert_eq!(clash.len(), 1, "{other} 并列时拒了几句：{clash:?}");
            assert!(
                clash[0].contains("--tray") && clash[0].contains(other),
                "{other} 的并列没在自己那句里点名双方：{}",
                clash[0]
            );
        }
        // 它不在 `numbered` 里：不带数字是这一族的正确点法，⑭ 那句「少了毫秒数」在这里是假红。
        let bare = refusals(&argv(&["--tray", "--selftest"]), false);
        assert!(
            !bare.iter().any(|s| s.contains("少了毫秒数")),
            "--tray 被当成了一枚需要毫秒数的门：{bare:?}"
        );
        // 也没人替它设闹钟，而它不需要：打完就退。
        assert!(
            unalarmed(&argv(&["--tray", "--mask", "1500"]), &["--mask"]).is_empty(),
            "--tray 被闹钟名单点名了，可它自己会退"
        );
    }

    /// §5.4.4 / §5.4.5 那一档进来的那一族第三条。它要的读数与上面那一档同形（沉默地点名自己、
    /// 并列时一句点名双方），多出来的是**同族两枚并列**这一格：`--tray --whiteboard` 是这一轮
    /// 才存在的一种坏 —— 两扇打完就退的门同开时，后点名的那一扇一次也没跑，而印出来的报告
    /// 看着像两次测量的合果。格数在这里是判据：给每档各写一条 `if`，交回来的就是两句（P142/P143
    /// 那两对 `2 != 1`），而这一族的门数由上面那一档那一条 `EXIT_DOORS.len()` 钉着，这一格不重
    /// 复数一遍 —— 同一个数写两处就是 #281。
    #[test]
    fn the_whiteboard_door_refuses_company_and_needs_no_alarm() {
        assert!(
            refusals(&argv(&["--whiteboard"]), false).is_empty(),
            "单独点 --whiteboard 被拒了：{:?}",
            refusals(&argv(&["--whiteboard"]), false)
        );
        assert!(
            refusals(&argv(&["--whiteboard"]), true).is_empty(),
            "--whiteboard 读的是档上的两枚键与编码器，gauge_run 那道结构性拒绝与它无关"
        );
        for (other, extra) in [
            ("--settings", None),
            ("--tray", None),
            ("--selftest", None),
            ("--desktop", None),
            ("--hotkeys", Some("1500")),
            ("--export-config", Some("out.toml")),
            ("--mask", Some("1500")),
            ("--settings-window", Some("1500")),
            ("--hint", Some("1500")),
        ] {
            let mut flat = vec!["--whiteboard", other];
            if let Some(x) = extra {
                flat.push(x);
            }
            let clash = refusals(&argv(&flat), false);
            assert_eq!(clash.len(), 1, "{other} 并列时拒了几句：{clash:?}");
            assert!(
                clash[0].contains("--whiteboard") && clash[0].contains(other),
                "{other} 的并列没在自己那句里点名双方：{}",
                clash[0]
            );
        }
        // 三枚同族一起点名时也只有一句，且那一句把三枚都写出来 —— 只写前两枚的那一句会让读日志
        // 的人以为第三扇跑了。
        let three = refusals(&argv(&["--settings", "--tray", "--whiteboard"]), false);
        assert_eq!(
            three
                .iter()
                .filter(|s| s.contains("--settings")
                    && s.contains("--tray")
                    && s.contains("--whiteboard"))
                .count(),
            1,
            "三枚同族并列时那句点名不全：{three:?}"
        );
        let bare = refusals(&argv(&["--whiteboard", "--selftest"]), false);
        assert!(
            !bare.iter().any(|s| s.contains("少了毫秒数")),
            "--whiteboard 被当成了一枚需要毫秒数的门：{bare:?}"
        );
        assert!(
            unalarmed(&argv(&["--whiteboard", "--mask", "1500"]), &["--mask"]).is_empty(),
            "--whiteboard 被闹钟名单点名了，可它自己会退"
        );
    }

    /// P171：`--autosave` 与 `--history-window` 都要把历史那两枚会话落点钉进自己的沙盒。
    /// 并列时后钉的那一枚赢，两档读到的行数与字节数就都不是自己种的那一份。这一格重点问的
    /// 是**一句还是两句**：给每档各写一条 `if` 时，流行的是两句各说各的话（P142 / P143 那对
    /// `2 != 1` 的形状），而且其中一句说的还是别人。
    #[test]
    fn the_history_door_refuses_the_autosave_door_and_names_both_sides() {
        for door in ["--autosave", "--history-window"] {
            let alone = refusals(&argv(&[door, "3000"]), false);
            assert!(
                alone.iter().all(|s| !s.contains("不同时点")),
                "{door} 单独点被拦了：{:?}",
                alone
            );
        }
        for pair in [
            vec!["--autosave", "3000", "--history-window", "3000"],
            vec!["--history-window", "3000", "--autosave", "3000"],
        ] {
            let both = refusals(&argv(&pair), false);
            let clash: Vec<&String> = both.iter().filter(|s| s.contains("不同时点")).collect();
            assert_eq!(clash.len(), 1, "{:?} 交回 {:?}，应当只有一句", pair, both);
            assert!(
                clash[0].contains("--autosave") && clash[0].contains("--history-window"),
                "{} 这句没有同时点名双方",
                clash[0]
            );
        }
    }

    /// §5.20.1 那一扇窗那一档：与上面那一档同样的两条，外加一条它自己独有的。**单独点它、带着
    /// 数字必须沉默**（这一趟唯一的运行证人就是它自己），**与别人并列必须点名双方且只拒一句**。
    /// 多出来的那一条是：它与 `--settings` 是两档、读的同一枚模型却各问各的问题，所以并列时
    /// `saves=` 那一格会被别人那一次写变成假话 —— 这一格专门盯着那一枚组合，因为它是这一轮新
    /// 加的门与上一轮那一档之间唯一真的会互相污染的一处。
    /// 一枚并列只交一句话，且那一句点名双方。这一格本轮真的抓到过一次双份：`--export-config`
    /// 与 `--settings-window` 各说了一句（两条规则读着同一批门名），红的是 `2 != 1`——修的是码
    /// （把那条已经覆盖这一对的名单从这里摘掉），不是把断言改成 2。
    #[test]
    fn the_settings_window_door_refuses_company_and_names_both_sides() {
        assert!(
            refusals(&argv(&["--settings-window", "1500"]), false).is_empty(),
            "单独点 --settings-window 带着数字被拒了：{:?}",
            refusals(&argv(&["--settings-window", "1500"]), false)
        );
        assert!(
            refusals(&argv(&["--settings-window", "1500"]), true).is_empty(),
            "那一扇窗不开遮罩，gauge_run 那道结构性拒绝与它无关"
        );
        for (other, extra) in [
            ("--settings", None),
            ("--selftest", None),
            ("--desktop", None),
            ("--hotkeys", Some("1500")),
            ("--export-config", Some("out.toml")),
            ("--import-config", Some("in.toml")),
            ("--mask", Some("1500")),
            ("--hint", Some("1500")),
            ("--autosave", Some("1500")),
        ] {
            let mut flat = vec!["--settings-window", "1500", other];
            if let Some(x) = extra {
                flat.push(x);
            }
            let clash = refusals(&argv(&flat), false);
            assert_eq!(clash.len(), 1, "{other} 并列时拒了几句：{clash:?}");
            assert!(
                clash[0].contains("--settings-window") && clash[0].contains(other),
                "{other} 的并列没在自己那句里点名双方：{}",
                clash[0]
            );
        }
        // `--settings` 那一条规则是反向的（它自己拒一切），所以并列的那一句由它那侧先说；
        // 这一格问的是「两种点法都得有人拦」，而不是谁拦的。
        let flipped = refusals(&argv(&["--settings", "--settings-window", "1500"]), false);
        assert_eq!(flipped.len(), 1, "{flipped:?}");
        assert!(flipped[0].contains("--settings-window"), "{}", flipped[0]);
    }

    /// §5.20.3 那一档点的是**同一扇窗**，所以它的拒绝与上面那一档同形。多出来的那一条是这一轮
    /// 特有的：两档同开时只能有**一句**。P142 那一个 `2 != 1` 就埋在这里 —— 给每档各写一条自己
    /// 的规矩时，`--settings-window --settings-io` 会拿到两句各说各的话，而 `settings_open` 只有
    /// 一枚、窗也只有一扇，「窗那侧说的话是谁的」当场没有作者。这一格不改判据的宽严，它问的是
    /// 那句话的数量与点名的双方。
    #[test]
    fn the_settings_io_door_refuses_company_once_and_names_both_windows() {
        assert!(
            refusals(&argv(&["--settings-io", "3000"]), false).is_empty(),
            "单独点 --settings-io 带着数字被拒了：{:?}",
            refusals(&argv(&["--settings-io", "3000"]), false)
        );
        assert!(
            refusals(&argv(&["--settings-io", "3000"]), true).is_empty(),
            "那一扇窗也不开遮罩，gauge_run 那道结构性拒绝与它无关"
        );
        for (other, extra) in [
            ("--settings", None),
            ("--selftest", None),
            ("--desktop", None),
            ("--hotkeys", Some("1500")),
            ("--export-config", Some("out.toml")),
            ("--import-config", Some("in.toml")),
            ("--mask", Some("1500")),
            ("--hint", Some("1500")),
            ("--autosave", Some("1500")),
        ] {
            let mut flat = vec!["--settings-io", "3000", other];
            if let Some(x) = extra {
                flat.push(x);
            }
            let clash = refusals(&argv(&flat), false);
            assert_eq!(clash.len(), 1, "{other} 并列时拒了几句：{clash:?}");
            assert!(
                clash[0].contains("--settings-io") && clash[0].contains(other),
                "{other} 的并列没在自己那句里点名双方：{}",
                clash[0]
            );
        }
        // 两扇窗那一档同开：一句，且那一句里两个名字都在 —— 次序换过来还是那一句。
        let both = refusals(
            &argv(&["--settings-window", "1500", "--settings-io", "3000"]),
            false,
        );
        assert_eq!(both.len(), 1, "两档同开拒了几句：{both:?}");
        assert!(
            both[0].contains("--settings-window") && both[0].contains("--settings-io"),
            "那一句没点名双方：{}",
            both[0]
        );
        let flipped = refusals(
            &argv(&["--settings-io", "3000", "--settings-window", "1500"]),
            false,
        );
        assert_eq!(flipped.len(), 1, "次序反过来又拒了几句：{flipped:?}");
    }

    /// P183 把**第三**副计划带进同一扇窗。这一格问的不是「那一档有没有自己的规矩」——它没有，
    /// 它进的是 [`WINDOW_DOORS`] 那张名单，所以拦它的句子与前两档共用一条。多出来要盯的是
    /// 三档同开时**还是一句**：给每档各写一条 `if`，三档同开就会拿到三句，而窗只有一扇。
    #[test]
    fn the_settings_palette_door_refuses_company_once_and_names_both_windows() {
        assert!(
            refusals(&argv(&["--settings-palette", "5000"]), false).is_empty(),
            "单独点 --settings-palette 带着数字被拒了：{:?}",
            refusals(&argv(&["--settings-palette", "5000"]), false)
        );
        assert!(
            refusals(&argv(&["--settings-palette", "5000"]), true).is_empty(),
            "那一扇窗不开遮罩，gauge_run 那道结构性拒绝与它无关"
        );
        for (other, extra) in [
            ("--settings", None),
            ("--selftest", None),
            ("--desktop", None),
            ("--hotkeys", Some("1500")),
            ("--export-palette", Some("pal.txt")),
            ("--import-palette", Some("pal.txt")),
            ("--mask", Some("1500")),
            ("--hint", Some("1500")),
            ("--autosave", Some("1500")),
        ] {
            let mut flat = vec!["--settings-palette", "5000", other];
            if let Some(x) = extra {
                flat.push(x);
            }
            let clash = refusals(&argv(&flat), false);
            assert_eq!(clash.len(), 1, "{other} 并列时拒了几句：{clash:?}");
            assert!(
                clash[0].contains("--settings-palette") && clash[0].contains(other),
                "{other} 的并列没在自己那句里点名双方：{}",
                clash[0]
            );
        }
        // 三档同开：还是一句，且那一句把三个名字都点出来 —— 名单从 2 长到 3 时，
        // `window_doors.join(" / ")` 是唯一的一处作者。
        let three = refusals(
            &argv(&[
                "--settings-window",
                "1500",
                "--settings-io",
                "3000",
                "--settings-palette",
                "5000",
            ]),
            false,
        );
        assert_eq!(three.len(), 1, "三档同开拒了几句：{three:?}");
        for name in ["--settings-window", "--settings-io", "--settings-palette"] {
            assert!(three[0].contains(name), "那一句没点名 {name}：{}", three[0]);
        }
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

    /// §5.5.8 那一档（`--refresh`）走的是上面那一封信的同一格、同一句 orphan，所以本格只多问
    /// 两件：**第四枚进来之后那一句还点名四方**（一句只报三个名字的拒绝会把第四趟调用归档到
    /// 别人名下，正是 ⑭ 那一条写这件事的原因），以及**带着 `--selftest` 时沉默**（运行证人那一
    /// 趟要的就是这一条）。分岔本体同样是纯函数 ⇒ 单测不撤，与 P75 那一族同一枚 dichotomy。
    #[test]
    fn refusals_reject_refresh_without_selftest_and_name_all_four() {
        let r = refusals(&argv(&["--refresh"]), false);
        assert_eq!(r.len(), 1, "{r:?}");
        for name in [
            "--refresh",
            "--clipboard",
            "--capture",
            "--history",
            "--selftest",
        ] {
            assert!(r[0].contains(name), "那一句没点名 {name}：{}", r[0]);
        }
        assert!(refusals(&argv(&["--selftest", "--refresh"]), false).is_empty());
    }

    /// §5.5.10 那一档（`--sendto`）是同一句 orphan 的**第五枚**进来。本格判两件事，都是上一格
    /// 那四枚覆盖不到的：句子跟着名单长到了五枚名字（`refusals_reject_refresh_…_all_four` 那一格
    /// 数的是 P153 时的四枚，它还过、但它不再是一个字不差的那一句 —— 名字数以五枚为准看这里），
    /// 以及这一档自己**不带毫秒数**：它不给闹钟名单，因为 `--selftest` 在 `engine.load()` 之前就
    /// 退，那一串 `numbered` 的门是量具档的事。
    #[test]
    fn refusals_name_all_five_selftest_companions_once_sendto_joined() {
        let s = refusals(&argv(&["--sendto"]), false);
        assert_eq!(s.len(), 1, "{s:?}");
        for name in [
            "--sendto",
            "--clipboard",
            "--capture",
            "--history",
            "--refresh",
            "--selftest",
        ] {
            assert!(s[0].contains(name), "那一句没点名 {name}：{}", s[0]);
        }
        assert!(refusals(&argv(&["--selftest", "--sendto"]), false).is_empty());
        // 少了毫秒数那一格对它沉默：这一档不是量具档，不进 `numbered`，也不进闹钟名单。
        assert!(unalarmed(&argv(&["--selftest", "--sendto"]), &[]).is_empty());
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

    /// P182：这一族由两枚变四枚，而那一族拒绝读的是 [`FILE_COMMANDS`] 这一张名单而不是四条
    /// `if`。这一格钉三件事：**枚数**（多一枚少一枚都要先改这条再改码，与 [`EXIT_DOORS`] 那五枚
    /// 同一条规矩）、**每一档单独点名自己**（少一份档时那句里得写在场的那一枚，不能替没被点的
    /// 同族说话），以及**任何两档并列只拒一句、双方都在句里**。上一格测的是 §5.20.3 那两枚之间
    /// 的组合；跨族的那两对（`--export-config` 与 `--import-palette`）此前不存在，本轮起它是一句
    /// 写错的调用，而写错的调用必须被拦在盘之前。
    #[test]
    fn the_file_command_family_names_four_and_refuses_company_once() {
        assert_eq!(
            FILE_COMMANDS.len(),
            4,
            "档命令这一族是四枚：§5.20.3 的两枚与 §5.7.22 的两枚"
        );
        for name in FILE_COMMANDS {
            let e = refusals(&argv(&[name]), false);
            assert_eq!(e.len(), 1, "{name} 少一份档时拒了几句：{e:?}");
            assert!(e[0].contains(name), "{}", e[0]);
            assert!(
                refusals(&argv(&[name, "f.txt"]), false).is_empty(),
                "{name} 带着档却被拒：{:?}",
                refusals(&argv(&[name, "f.txt"]), false)
            );

            let clash = refusals(&argv(&[name, "f.txt", "--mask", "1500"]), false);
            assert_eq!(clash.len(), 1, "{name} 与 --mask 并列时拒了几句：{clash:?}");
            assert!(
                clash[0].contains(name) && clash[0].contains("--mask"),
                "{}",
                clash[0]
            );
            let other = FILE_COMMANDS.iter().copied().find(|n| *n != name).unwrap();
            assert!(
                !clash[0].contains(other),
                "{} 替没被点名的 {other} 说了一句话",
                clash[0]
            );
        }
        for pair in [
            ("--export-palette", "--import-palette"),
            ("--export-config", "--import-palette"),
        ] {
            let b = refusals(&argv(&[pair.0, "a.txt", pair.1, "b.txt"]), false);
            assert_eq!(b.len(), 1, "{pair:?} 并列时拒了几句：{b:?}");
            assert!(
                b[0].contains(pair.0) && b[0].contains(pair.1),
                "{pair:?} 的并列没点名双方：{}",
                b[0]
            );
        }
        // `--confirm` 自此有两档可修饰；`--import-all` 仍只有那一档 —— 让「整份 settings 都盖」
        // 那枚范围档去修饰一份只有一排色的档，是替一扇做不到的门许一件事。
        assert!(refusals(&argv(&["--import-palette", "p.txt", "--confirm"]), false).is_empty());
        let all = refusals(&argv(&["--import-palette", "p.txt", "--import-all"]), false);
        assert_eq!(all.len(), 1, "{all:?}");
        assert!(all[0].contains("--import-all"), "{}", all[0]);
    }

    /// 「刚钉上一枚贴图」这句话在 P167 之前有两种写法，而两种各丢一样东西：`--last` 与 `--active`
    /// 把 `id` 丢在 `|(_, line)|` 里，终端因此从来没说过刚钉的是哪一枚；`--snap` 那一档自己拼一句
    /// `snapped pin {id}: …`，于是同一件事在终端上有两种说法。这两件事 `cli_capture_to_pin` 自己都
    /// 问不到 —— 它得先抓一次真屏幕，那一半归真机门问。而「id 有没有被丢掉、尾巴有没有被重拼、递
    /// 进去的那句有没有被改一个字」不需要屏幕，所以拆成 `cli_pinned_line` 就是为了在这里问得到。
    #[test]
    fn the_cli_pin_line_keeps_the_id_and_the_sentence_it_was_handed() {
        let said = "区域 PhysRect { x: 100, y: 100, w: 200, h: 100 }";
        let line = cli_pinned_line(7, said);

        // 那一位作者的整句在前，命令行的半句在后 —— 反过来就是把别人的话重排了一遍。
        assert_eq!(
            line,
            format!("{}（关掉那扇窗即退出）", capture::pinned_line(7, said)),
            "{line}"
        );
        assert!(line.contains("贴图 7 已钉："), "{line}");
        assert!(line.ends_with("（关掉那扇窗即退出）"), "{line}");
        // 递进去的那句原样在内，一个标点也不重拼。
        assert!(line.contains(said), "{line}");
        // 「刚钉的是哪一枚」换人得换字：丢 id 的那一形在这里就答不上来。
        assert_ne!(
            line,
            cli_pinned_line(8, said),
            "换一枚 id 句子不动，等于没说"
        );
    }
}
