//! §5.17 command-line interface: the capture verbs, the output targets, the status
//! codes and the help.
//!
//! PRD §5.17 asks for a way for "scripts, shortcuts and third parties" to start a
//! capture and pick where the picture goes, with the example `snip --last -o
//! clipboard`. Its three rules are the whole contract, and all three are decisions a
//! pure function can make with no desktop behind them:
//!
//! 1. 成功与失败必须返回明确状态码 — success and failure each get an explicit code.
//! 2. 参数错误时应输出帮助信息 — a mis-spelled call prints the help.
//! 3. 文件写入失败时不得返回成功状态 — a failed file write is never reported as success.
//!
//! The *pixels* only exist on a real screen, so the act of filling a rectangle is the
//! A/B real-machine door's to grade. What lives here is everything the door cannot
//! take back: the grammar, the exit-code mapping, and the refusal of a bad call before
//! the process ever reaches for the screen or the disk. `main` is a binary's `fn main`
//! and so is invisible to `cargo test` (§10 第 40 项 ③); this module is reachable, and
//! that is why the contract is a function of `argv` rather than a stretch bolted onto
//! the end of `main`.
//!
//! The four capture verbs are mutually exclusive, and the output defaults to a pin. A
//! flag a §5.17 call shares with a measurement door (`--mask`, `--pin`, …) is refused
//! rather than allowed to degrade into the door's own event loop — the same reason
//! 第 47 项 ⑭ refuses a door with no alarm.

use falcon_core::geometry::PhysRect;

/// §5.17 rule 1's two codes, and rule 2's third. The same three numbers the config
/// commands already answer with (`main`'s `--export-config` / `--import-config`), so
/// the CLI does not invent a second exit-code table.
pub const EXIT_OK: i32 = 0;
/// A real failure: the picture was not captured, or an output the call asked for did
/// not land. §5.17 rule 3 lives entirely in this code — a failed write is `1`, never `0`.
pub const EXIT_FAIL: i32 = 1;
/// A mis-invocation: refused before the process touched the screen or the disk, and
/// printed the help (§5.17 rule 2).
pub const EXIT_USAGE: i32 = 3;

/// Which of §5.17's four capture abilities a call named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    /// 启动普通截图 — the interactive mask; its own toolbar decides the output, so this
    /// verb takes no `-o` (see [`classify`]'s refusal of that combination).
    New,
    /// 截取指定坐标区域 — `--region x,y,w,h`.
    Region(PhysRect),
    /// 截取上一次区域 — `--last`, §5.2.5's memory.
    Last,
    /// 截取活动窗口 — `--active`, §5.2.6's foreground window.
    Active,
}

/// Where §5.17's picture goes; a pin unless the call said otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// 执行截图后贴图 — the default, and the one every verb can do today.
    Pin,
    /// 输出到剪贴板.
    Clipboard,
    /// 保存到指定文件 — `-o file <path>`.
    File(String),
}

/// What [`classify`] decided about one argv.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// No §5.17 token appears at all. This is the whole reason the module is additive:
    /// every existing invocation — selftest, the measurement doors, the resident app,
    /// the config commands — reaches `NotCli` and falls through to the code it ran
    /// before this round, so not one of them can be affected by this grammar.
    NotCli,
    /// `--help` / `-h`. Prints the help and exits [`EXIT_OK`].
    Help,
    /// §5.17 rule 2: a call that names §5.17 tokens but cannot be run. The reason is a
    /// sentence; the caller prints it with the help and exits [`EXIT_USAGE`].
    Error(String),
    /// A well-formed request. Running it is [`crate`]’s job in `main` — the pixels need
    /// a desktop — but the grammar it reached through is fully decided here.
    Run { select: Select, output: Output },
}

/// The verbs and the output option, spelled once.
const VERBS: [&str; 4] = ["--new", "--region", "--last", "--active"];
const OUTPUTS: [&str; 2] = ["-o", "--output"];

/// The next token after `i`, unless it is missing or is itself a flag — a value that
/// begins with `-` is a forgotten argument, not a path that happens to start with a dash.
fn value_at(args: &[String], i: usize) -> Option<&str> {
    args.get(i)
        .map(String::as_str)
        .filter(|t| !t.starts_with('-'))
}

/// Read one argv against §5.17. `reserved` names the flags that mean "this is a
/// measurement door, not a §5.17 call" (the gauge and extra-door flags `main` passes
/// in from the same `mask::GAUGE_FLAGS` its other refusal reads) — a §5.17 verb shared
/// with one of them is refused rather than allowed to fall into the door's event loop.
pub fn classify(args: &[String], reserved: &[&str]) -> Decision {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Decision::Help;
    }
    let has_verb = args.iter().any(|a| VERBS.contains(&a.as_str()));
    let has_output = args.iter().any(|a| OUTPUTS.contains(&a.as_str()));
    if !has_verb && !has_output {
        return Decision::NotCli;
    }
    // The door first: a §5.17 verb next to a measurement flag would run neither — the
    // mask covers the screen the door wants to read, and the door's `quit_after` ends
    // the run before the CLI's output could happen. Refuse the whole call as a
    // mis-invocation, the same voice as every other orphan.
    if let Some(hit) = args.iter().find(|a| reserved.contains(&a.as_str())) {
        return Decision::Error(format!(
            "{hit} 是量具档，不与 §5.17 的截图命令同开：一次调用要么截图、要么量一次"
        ));
    }
    let verbs: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| VERBS.contains(&a.as_str()))
        .map(|(i, _)| i)
        .collect();
    if verbs.len() > 1 {
        return Decision::Error(
            "至多点一条截图命令：--new / --region / --last / --active 只能选一".to_string(),
        );
    }
    if verbs.is_empty() {
        // `-o` names where a picture goes, and there is no picture without a verb.
        return Decision::Error(
            "只给了输出（-o / --output）而没有截图命令：先点 --new / --region / --last / --active"
                .to_string(),
        );
    }
    let vi = verbs[0];
    let verb = args[vi].as_str();
    let select = if verb == "--region" {
        match value_at(args, vi + 1).and_then(crate::capture::rect_arg) {
            Some(rect) => Select::Region(rect),
            None => {
                return Decision::Error(
                    "--region 后面要跟四个整数 x,y,w,h（例如 `--region 10,20,300,400`）"
                        .to_string(),
                )
            }
        }
    } else if verb == "--last" {
        Select::Last
    } else if verb == "--active" {
        Select::Active
    } else {
        Select::New
    };

    let mut output = Output::Pin;
    if let Some(oi) = args.iter().position(|a| OUTPUTS.contains(&a.as_str())) {
        match value_at(args, oi + 1) {
            Some("clipboard") => output = Output::Clipboard,
            Some("pin") => output = Output::Pin,
            Some("file") => {
                match value_at(args, oi + 2) {
                    Some(path) => output = Output::File(path.to_string()),
                    None => return Decision::Error(
                        "-o file 后面要跟一个文件路径（例如 `-o file C:\\Users\\me\\shot.png`）"
                            .to_string(),
                    ),
                }
            }
            Some(other) => {
                return Decision::Error(format!("-o 只认 clipboard / pin / file，收到 {other:?}"))
            }
            None => {
                return Decision::Error(
                    "-o / --output 需要一个值：clipboard / pin / file".to_string(),
                )
            }
        }
    }
    // 启动普通截图 opens the interactive mask; where its selection lands is decided by the
    // mask's own toolbar after the user drags. A `-o` beside `--new` names an output the
    // mask will never read, so it is refused rather than silently ignored — a call whose
    // stated output is quietly dropped is a call that lied about what it did.
    if select == Select::New && has_output {
        return Decision::Error(
            "--new 开的是交互式遮罩，落点由遮罩自己的工具条决定，不吃 -o；要指定输出请用 --region / --last / --active"
                .to_string(),
        );
    }
    Decision::Run { select, output }
}

/// §5.17 rule 1, as the exit code of a decision that is already settled before any
/// screen is touched: help is a success, a bad call is the mis-invocation code, and a
/// well-formed request or a plain non-CLI run leave the code to the run itself
/// (`None` — `main` decides by running it).
pub fn code_of(decision: &Decision) -> Option<i32> {
    match decision {
        Decision::NotCli => None,
        Decision::Help => Some(EXIT_OK),
        Decision::Error(_) => Some(EXIT_USAGE),
        Decision::Run { .. } => None,
    }
}

/// §5.17 rules 1 and 3, as a pure function of what the run did: the process exits
/// [`EXIT_OK`] only when a picture was captured **and** the output it named landed.
/// A file write that failed — or a clipboard the holder would not take, or a freeze the
/// desktop refused — is [`EXIT_FAIL`], never a success, which is exactly what "文件写入
/// 失败时不得返回成功状态" forbids. The two booleans are what `main` observed on the real
/// machine; this maps them to the code with no desktop of its own, so rule 3 has a
/// witness that runs headless.
pub fn run_code(captured: bool, output_ok: bool) -> i32 {
    if captured && output_ok {
        EXIT_OK
    } else {
        EXIT_FAIL
    }
}

/// §5.17 rule 2's help text. Returned as one string so the `--help` leg and the
/// error leg print the *same* block, and the selftest gauge can check its shape with no
/// screen behind it.
pub fn help() -> String {
    let mut lines: Vec<String> = vec![
        "AI Falconshot 命令行截图（§5.17）".to_string(),
        String::new(),
        "截图命令（选一）：".to_string(),
        "  --new                    启动普通截图（交互式遮罩；落点由遮罩工具条决定，不吃 -o）"
            .to_string(),
        "  --region x,y,w,h         截取指定坐标区域（物理像素，例如 10,20,300,400）".to_string(),
        "  --last                   截取上一次区域".to_string(),
        "  --active                 截取活动窗口".to_string(),
        String::new(),
        "输出（可选，默认贴图）：".to_string(),
        "  -o pin                   执行截图后贴图（默认）".to_string(),
        "  -o clipboard             输出到剪贴板".to_string(),
        "  -o file <路径>           保存到指定文件（.png / .jpg / .bmp）".to_string(),
        String::new(),
        "状态码（§5.17 规则）：".to_string(),
        format!("  {EXIT_OK} 成功"),
        format!(
            "  {EXIT_FAIL} 失败（截图没截成、或写文件/写剪贴板没落地——文件写入失败绝不返回成功）"
        ),
        format!("  {EXIT_USAGE} 参数错误（打印本帮助）"),
        String::new(),
        "示例：".to_string(),
        "  falconshot --last -o clipboard".to_string(),
        "  falconshot --region 10,20,300,400 -o file C:\\Users\\me\\shot.png".to_string(),
        "  falconshot --active".to_string(),
    ];
    lines.push(String::new());
    lines.join("\n")
}

/// The headless witness `--selftest` adds: run [`classify`], [`code_of`] and
/// [`run_code`] over a table of argv fixtures whose every verdict is written down here,
/// and grade the contract against it. Nothing reads the screen — this checks the grammar
/// and the two code rules, which is the §5.17 half that a locked session can still prove.
pub fn check() -> (crate::state::Check, String) {
    let reserved = ["--mask", "--pin", "--selftest"];
    let named = |a: &[&str]| -> Vec<String> { a.iter().map(|s| s.to_string()).collect() };
    // (case name, argv, expected decision)
    let cases: Vec<(&str, Vec<String>, Decision)> = vec![
        ("help flag", named(&["--help"]), Decision::Help),
        ("not a cli call (empty)", named(&[]), Decision::NotCli),
        (
            "not a cli call (gauge only)",
            named(&["--mask", "1500"]),
            Decision::NotCli,
        ),
        (
            "last to clipboard",
            named(&["--last", "-o", "clipboard"]),
            Decision::Run {
                select: Select::Last,
                output: Output::Clipboard,
            },
        ),
        (
            "active default pin",
            named(&["--active"]),
            Decision::Run {
                select: Select::Active,
                output: Output::Pin,
            },
        ),
        (
            "region to file",
            named(&["--region", "10,20,300,400", "-o", "file", "C:\\x.png"]),
            Decision::Run {
                select: Select::Region(PhysRect::new(10, 20, 300, 400)),
                output: Output::File("C:\\x.png".to_string()),
            },
        ),
        (
            "new opens mask",
            named(&["--new"]),
            Decision::Run {
                select: Select::New,
                output: Output::Pin,
            },
        ),
        (
            "region arity",
            named(&["--region", "1,2"]),
            Decision::Error(String::new()),
        ),
        (
            "file needs path",
            named(&["-o", "file"]),
            Decision::Error(String::new()),
        ),
        (
            "unknown output",
            named(&["--last", "-o", "dropbox"]),
            Decision::Error(String::new()),
        ),
        (
            "two verbs",
            named(&["--last", "--active"]),
            Decision::Error(String::new()),
        ),
        (
            "output no verb",
            named(&["-o", "clipboard"]),
            Decision::Error(String::new()),
        ),
        (
            "new takes no -o",
            named(&["--new", "-o", "clipboard"]),
            Decision::Error(String::new()),
        ),
        (
            "verb with gauge",
            named(&["--last", "--mask", "1500"]),
            Decision::Error(String::new()),
        ),
    ];
    let mut bad: Vec<String> = Vec::new();
    for (name, argv, want) in &cases {
        let got = classify(argv, &reserved);
        let ok = match (want, &got) {
            // The Error arm compares only its shape, not the wording: the message is
            // prose, and #433's rule is that a row that grades wording grades the file's
            // phrasing rather than the grammar.
            (Decision::Error(_), Decision::Error(_)) => true,
            (w, g) => w == g,
        };
        if !ok {
            bad.push(format!("{name}: 期望 {want:?} 收到 {got:?}"));
        }
    }
    // §5.17 rule 1's codes, straight off the decisions above.
    if code_of(&Decision::Help) != Some(EXIT_OK) {
        bad.push("help 不是 0".to_string());
    }
    if code_of(&Decision::Error("x".to_string())) != Some(EXIT_USAGE) {
        bad.push("参数错不是 3".to_string());
    }
    if code_of(&Decision::NotCli).is_some() {
        bad.push("非 CLI 调用不该带码".to_string());
    }
    // §5.17 rule 3's whole content: a failed write is never a success code.
    if run_code(true, true) != EXIT_OK {
        bad.push("截到且写出该 0".to_string());
    }
    if run_code(true, false) != EXIT_FAIL {
        bad.push("写出失败还报成功（§5.17 规则 3）".to_string());
    }
    if run_code(false, true) != EXIT_FAIL {
        bad.push("没截到却报成功".to_string());
    }
    // The help is rule 2's payload, and it must actually carry the verbs.
    let h = help();
    for needle in ["--region", "--last", "--active", "--new", "-o clipboard"] {
        if !h.contains(needle) {
            bad.push(format!("帮助里没有 {needle}"));
        }
    }
    let verdict = bad.is_empty();
    (
        crate::state::Check::from(verdict),
        format!(
            "{} 例 argv 判决 + 三条状态码规则；{}（截屏像素本身在真机上量，非本档 §5.17 的 headless 半边）",
            cases.len(),
            if verdict {
                "全部对上".to_string()
            } else {
                format!("对不上：{}", bad.join(" "))
            }
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }
    const R: &[&str] = &["--mask", "--pin", "--selftest"];

    /// The whole fixture table, graded headless. `check()` is the same function the
    /// `--selftest` gauge runs, so this test and that row read one list rather than two
    /// (§10 第 40 项 ③'s reason the contract lives in a module, not in `main`).
    #[test]
    fn the_contract_table_holds() {
        let (check, said) = check();
        assert_eq!(check, crate::state::Check::Pass, "{said}");
    }

    /// Rule 2's first branch: a call that only wants to know how to be called.
    #[test]
    fn help_short_circuits_everything() {
        assert_eq!(classify(&argv(&["--help"]), R), Decision::Help);
        assert_eq!(classify(&argv(&["-h"]), R), Decision::Help);
        // Even beside a verb: asking for help wins over being refused for a shape.
        assert_eq!(classify(&argv(&["--last", "-h"]), R), Decision::Help);
        assert_eq!(code_of(&Decision::Help), Some(EXIT_OK));
    }

    /// The additive guarantee: without a §5.17 token, this module has no opinion, and
    /// every existing invocation falls through unchanged. A gauge flag alone is the
    /// clearest case — it must NOT be read as a §5.17 error.
    #[test]
    fn anything_that_is_not_a_capture_verb_is_none_of_this_modules_business() {
        assert_eq!(classify(&argv(&[]), R), Decision::NotCli);
        assert_eq!(classify(&argv(&["--mask", "1500"]), R), Decision::NotCli);
        assert_eq!(
            classify(&argv(&["--selftest", "--clipboard"]), R),
            Decision::NotCli
        );
        assert_eq!(code_of(&Decision::NotCli), None);
    }

    /// The four abilities §5.17 names, and the `-o` that picks where the picture goes.
    #[test]
    fn each_named_capture_verb_classifies_with_its_output() {
        assert_eq!(
            classify(&argv(&["--last", "-o", "clipboard"]), R),
            Decision::Run {
                select: Select::Last,
                output: Output::Clipboard
            }
        );
        assert_eq!(
            classify(&argv(&["--active"]), R),
            Decision::Run {
                select: Select::Active,
                output: Output::Pin
            }
        );
        assert_eq!(
            classify(&argv(&["--new"]), R),
            Decision::Run {
                select: Select::New,
                output: Output::Pin
            }
        );
        assert_eq!(
            classify(
                &argv(&["--region", "10,20,300,400", "--output", "file", "C:\\a.png"]),
                R
            ),
            Decision::Run {
                select: Select::Region(PhysRect::new(10, 20, 300, 400)),
                output: Output::File("C:\\a.png".to_string())
            }
        );
        // A long-output call with a default (no `-o`) still means pin.
        assert_eq!(
            classify(&argv(&["--region", "0,0,64,64"]), R),
            Decision::Run {
                select: Select::Region(PhysRect::new(0, 0, 64, 64)),
                output: Output::Pin
            }
        );
    }

    /// Every way a parameter is wrong prints the help and exits 3, and each names the
    /// thing it refused. `--region` arity reuses `capture::rect_arg` rather than a second
    /// rectangle rule (§10 第 40 项 ③'s one-owner point).
    #[test]
    fn every_bad_parameter_is_a_usage_error_that_names_itself() {
        let cases: Vec<Vec<String>> = vec![
            argv(&["--region", "1,2"]),
            argv(&["--region", "1,2,3"]),
            argv(&["--region", "a,b,c,d"]),
            argv(&["--region"]),
            argv(&["--last", "-o", "dropbox"]),
            argv(&["-o", "file"]),
            argv(&["--last", "-o", "file", "--active"]),
            argv(&["-o"]),
            argv(&["--last", "--active"]),
            argv(&["-o", "clipboard"]),
            argv(&["--new", "-o", "clipboard"]),
            argv(&["--last", "--mask", "1500"]),
        ];
        for a in cases {
            let got = classify(&a, R);
            assert!(matches!(got, Decision::Error(_)), "{a:?} → {got:?}");
            assert_eq!(code_of(&got), Some(EXIT_USAGE));
        }
    }

    /// Rule 3 in the only form a headless run can hold it: the code is a pure function of
    /// what happened, and a write that failed is never the success code — even when the
    /// capture itself succeeded.
    #[test]
    fn a_failed_write_is_never_reported_as_success() {
        assert_eq!(run_code(true, true), EXIT_OK);
        assert_eq!(run_code(true, false), EXIT_FAIL);
        assert_eq!(run_code(false, true), EXIT_FAIL);
        assert_eq!(run_code(false, false), EXIT_FAIL);
        assert_ne!(run_code(true, false), EXIT_OK, "§5.17 规则 3 被写反了");
    }

    /// The help is rule 2's payload; a `--help` leg and an error leg print this one block,
    /// so it has to actually say what the verbs and the codes are.
    #[test]
    fn the_help_carries_the_verbs_and_the_three_codes() {
        let h = help();
        for needle in ["--region", "--last", "--active", "--new", "-o clipboard"] {
            assert!(h.contains(needle), "帮助缺 {needle}");
        }
        for code in [EXIT_OK, EXIT_FAIL, EXIT_USAGE] {
            assert!(h.contains(&code.to_string()), "帮助没写状态码 {code}");
        }
    }
}
