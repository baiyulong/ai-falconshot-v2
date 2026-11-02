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

/// §5.21.2 第三枚「命令行配置处应提供参数文档入口」的那一份名单，也是 §5.17 语法的**唯一**
/// 拼写处：`classify` 认哪些旗标，是从这张表现读出来的，不是从另一份 `const [&str]` 抄的。
///
/// 上一版这里是两本账 —— 语法读 `VERBS`/`OUTPUTS` 那两串裸旗标，[`help`] 另写一遍带说明的整行。
/// 名单不会错，它只会先旧：加一枚动词而忘了加一行说明，帮助就少讲一枚真能被接受的旗标，而
/// 今天没有任何一道门会发现。实测这一枚坏**已经发生**： shipped 的 `--help` 从头到尾没有提过
/// `--output` 与 `-h`，而 `classify` 两只手都认（本轮把这两枚补进文档，见 ③）。
///
/// 段位（`cmd` / `out` / `misc` / `eg`）指向 [`DOC_SECTIONS`] 里的段标题；**写法**那一列的第一个
/// 空白分隔的记号就是语法认的那枚旗标，所以「文档点名的」与「语法接受的」不是两份对齐过的名单，
/// 而是同一份的两种看法。
///
/// `code` 那一段的三行不在这里：状态码的号码只许有 [`EXIT_OK`]/[`EXIT_FAIL`]/[`EXIT_USAGE`]
/// 三个出处，把 `0`/`1`/`3` 再抄进表格就成了第四份，所以那三行由 [`help`] 直接从三枚常量拼。
pub const PARAM_DOC: [(&str, &str, &str); 16] = [
    (
        "cmd",
        "--new",
        "启动普通截图（交互式遮罩；落点由遮罩工具条决定，不吃 -o）",
    ),
    (
        "cmd",
        "--region x,y,w,h",
        "截取指定坐标区域（物理像素，例如 10,20,300,400）",
    ),
    ("cmd", "--last", "截取上一次区域"),
    ("cmd", "--active", "截取活动窗口"),
    ("out", "-o pin", "执行截图后贴图（默认）"),
    ("out", "-o clipboard", "输出到剪贴板"),
    (
        "out",
        "-o file <路径>",
        "保存到指定文件（.png / .jpg / .bmp）",
    ),
    (
        "out",
        "--output",
        "与 -o 同义，值仍是 pin / clipboard / file",
    ),
    (
        "snap",
        "--snap",
        "直接钉屏幕正中一块（400x250 物理像素），不走遮罩、不吃 -o",
    ),
    (
        "snap",
        "--snap-window",
        "直接钉指针下面那一扇窗，不走遮罩、不吃 -o",
    ),
    (
        "snap",
        "--rect x,y,w,h",
        "与 --snap 同开，把正中那一块换成这四个整数（可负）",
    ),
    ("misc", "--help", "打印本参数文档后退出（状态码 0）"),
    ("misc", "-h", "与 --help 同义"),
    ("eg", "falconshot --last -o clipboard", ""),
    (
        "eg",
        "falconshot --region 10,20,300,400 -o file C:\\Users\\me\\shot.png",
        "",
    ),
    ("eg", "falconshot --active", ""),
];

/// 段位在文档里的那一句标题，与文档里的先后。
const DOC_SECTIONS: [(&str, &str); 6] = [
    ("cmd", "截图命令（选一）："),
    ("out", "输出（可选，默认贴图）："),
    (
        "snap",
        "贴图那一族（自己就是收梢，不与上面四条同开，也不读 -o）：",
    ),
    ("misc", "取文档（不截图）："),
    ("code", "状态码（§5.17 规则）："),
    ("eg", "示例："),
];

/// 文档块的第一行。它同时是 [`doc_entry`] 递给设置页的那一句里对本文档的称呼，所以只写一次。
const DOC_TITLE: &str = "AI Falconshot 命令行截图（§5.17）";

/// 写法那一列里，语法真正认的那枚旗标：第一个空白之前的记号。
/// `--region x,y,w,h` → `--region`；`-o file <路径>` → `-o`。
fn flag_of(syntax: &'static str) -> &'static str {
    syntax.split_ascii_whitespace().next().unwrap_or(syntax)
}

/// 表里某一段的旗标，按表里的先后去掉重复。§5.17 能接受的旗标**只有**从这里数得出来。
fn flags_of(section: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (sec, syntax, _) in PARAM_DOC.iter() {
        if *sec == section {
            let flag = flag_of(syntax);
            if !out.contains(&flag) {
                out.push(flag);
            }
        }
    }
    out
}

/// 四条截图动词。过去这里是另一串裸旗标，与帮助各写各的。
pub fn verbs() -> Vec<&'static str> {
    flags_of("cmd")
}

/// `-o` 与其全名。
pub fn outputs() -> Vec<&'static str> {
    flags_of("out")
}

/// 取文档的那两枚（`--help` / `-h`）。
pub fn help_flags() -> Vec<&'static str> {
    flags_of("misc")
}

/// 贴图那一族（`--snap` / `--snap-window` / `--rect`）的旗标，按表里的先后。
///
/// 它们**不是** [`classify`] 的截图动词：这一族落在 `main` 自己那一只手上，单独喂给 `classify`
/// 的答案是 `NotCli`。它们住进 [`PARAM_DOC`] 是因为 §5.17 规则 2 说的是「参数错误时应输出帮助
/// 信息」，而 P170 之前 `--snap --rect` 交回 rc=3、打印的那一份帮助对这三个记号一个字不讲 ——
/// 拒了用户刚敲的那枚旗标，却不告诉他它怎么使。自本轮起这一枚名单是**唯一出处**：`main` 开这一
/// 只手、`main` 把它留给 `classify` 的那份 `cli_reserved`、以及帮助里那一段，三处都从这里数。
pub fn snap_flags() -> Vec<&'static str> {
    flags_of("snap")
}

/// argv 里第一枚真开得着贴图那一族的旗标，名字取自表、不取自手抄。`None` = 这一族整只手不动。
pub fn snap_verb(args: &[String]) -> Option<&'static str> {
    snap_openers()
        .into_iter()
        .find(|f| args.iter().any(|a| a == f))
}

/// 那一族的两枚入口，`main` 那两条臂唯一的比处。它们是**名字**，不是行为；行为仍住在 `main`。
/// 之所以住在这里，是因为表里那一族的 `--rect` 那一行写着「与 --snap 同开」——一句文档说的话，
/// 得有一枚常量能让语法照着它拒。这两枚必须真在 [`snap_openers`] 的名单上，那一句由 [`check`] 量。
pub const SNAP_CENTRE: &str = "--snap";
pub const SNAP_WINDOW: &str = "--snap-window";

/// 那一族的**入口**：表里写法不带第二个记号的那几行。`--rect x,y,w,h` 落不进来，因为它是一枚
/// 修饰档，单独点它不开任何一只手 —— 这一枚判据与 [`output_targets`] 数 `-o` 的值用的是同一件
/// 东西（写法里的第二个记号 = 这一枚还要吃一个值），不是又一串手抄的名字。
pub fn snap_openers() -> Vec<&'static str> {
    PARAM_DOC
        .iter()
        .filter(|(sec, _, _)| *sec == "snap")
        .filter(|(_, syntax, _)| syntax.split_ascii_whitespace().nth(1).is_none())
        .map(|(_, syntax, _)| flag_of(syntax))
        .collect()
}

/// 那一族里「钉哪一块」由谁决定：入口是 [`SNAP_WINDOW`] 时读指针下面那一扇，否则读 [`RectSlot`]。
/// 这一枚只比名字，不复制 `main` 的那三条臂。
pub fn is_snap_window(flag: Option<&str>) -> bool {
    flag == Some(SNAP_WINDOW)
}

/// 文档那一行「与 --snap 同开」的反面：`--rect` 在场，而它的入口不在场。
///
/// 两种落错都算：一枚入口都没点（`falconshot --rect 1,2,3,4`，这一族整只手不动，那四个整数谁也
/// 不读），以及点了**另一枚**入口（`--snap-window --rect …`，窗那一臂读的是指针下面那一扇，从来
/// 不读 [`RectSlot`]）。P168 关掉的形状是「点了 `--rect` 不给值被读成没给」，这一枚关掉的是它的
/// 另一半：给了值而没有入口，一样是安静地什么也不发生。
pub fn rect_orphan(args: &[String], snap_open: Option<&str>) -> bool {
    !matches!(rect_slot(args), RectSlot::Absent) && snap_open != Some(SNAP_CENTRE)
}

/// 规则 2 的那一腿欠不欠一格闹钟。
///
/// 钉成的那一腿等真人是产品行为：窗还开着，后面另有腿要说话。被参数错拒掉的这一腿什么都没钉、
/// 屏幕上一扇窗也没有，让它落进 `app.exec()` 就是把一句已经打印完的拒绝拖成一次看不见的等待——
/// 帮助印出来了，那个 3 却永远交不回 cmd.exe，规则 2 于是只剩半句。本轮门梯 A 代腿② 的证人正是
/// 这一格：`falconshot --snap --rect` 五分钟不退出，档案止于帮助块，`[falconshot] snap 收梢` 那
/// 一行从来没有出现过（rc 记成 `-1`，是本轮自己把那一趟杀掉的）。
///
/// `alarmed` 是 `main` 那张闹钟表点过名的格数：别的门（`--probe` / `--mask` / `--pin` 与其余）本来
/// 就会结束那一趟，这时这里不叠加，那一趟的墙钟仍由它自己那一档决定。
pub fn snap_refusal_needs_alarm(snap: Option<Snap>, alarmed: usize) -> bool {
    matches!(snap, Some(Snap::BadParam)) && alarmed == 0
}

/// 那一趟欠的闹钟有多长。它等的不是任何一条腿 —— 只保证「这一趟会自己结束」，与 `--hotcorner`
/// 那一档用的是同一个理由；号码取 `main` 里各档共用的那枚地板（`ms.max(300)`），不另开一格。
pub const SNAP_REFUSE_MS: i32 = 300;

/// `-o` 认的三枚值：输出那一段里写法的第二个记号（`-o pin` → `pin`，`--output` 那一行没有第二
/// 记号，所以不落进来）。`classify` 收下的值与这一枚名单是否一致，由 `--selftest` 那一格
/// **逐枚把值喂给 classify 问**（不是比句子）：拒绝的句子是人手写的措辞，比句子只会量到字，
/// 而这一档要量的是「文档说了的，语法真收」。
pub fn output_targets() -> Vec<&'static str> {
    PARAM_DOC
        .iter()
        .filter(|(sec, _, _)| *sec == "out")
        .filter_map(|(_, syntax, _)| syntax.split_ascii_whitespace().nth(1))
        .collect()
}

/// 每一枚文档点名的 `-o` 值，配一条**真喂得进 classify 的完整 argv**，交回 `(值, argv)`。
///
/// 这一枚函数存在的理由是本轮判据行自己踩的坑：把 `-o file` 与 `-o pin` 当成同一种东西去问
/// 「语法收不收」，`file` 那一路必然是拒——文档那一行写的就是 `-o file <路径>`，第三个记号是
/// 一个占位，说的正是「这一枚还欠一个值」。所以量之前得把欠的那个补上。
///
/// 补的路径不在这里编造：从文档自己的示例行里取（那一行 `-o file C:\\Users\\me\\shot.png` 本来
/// 就是本文档给出的一个合法写法），所以「用什么样本喂」这句话也只有一处出处。
pub fn output_cases() -> Vec<(String, Vec<String>)> {
    let mut sample: Option<String> = None;
    for (sec, syntax, _) in PARAM_DOC.iter() {
        if *sec != "eg" {
            continue;
        }
        let toks: Vec<&str> = syntax.split_ascii_whitespace().collect();
        if let Some(i) = toks.iter().position(|t| *t == "file") {
            if let Some(path) = toks.get(i + 1) {
                sample = Some((*path).to_string());
                break;
            }
        }
    }
    let mut cases: Vec<(String, Vec<String>)> = Vec::new();
    for (sec, syntax, _) in PARAM_DOC.iter() {
        if *sec != "out" {
            continue;
        }
        let toks: Vec<&str> = syntax.split_ascii_whitespace().collect();
        if toks.len() < 2 {
            continue; // `--output` 那一行点名的是选项本身，不是一枚值
        }
        let mut argv: Vec<String> = vec![
            "--last".to_string(),
            toks[0].to_string(),
            toks[1].to_string(),
        ];
        // 第三个记号在文档里是 `<路径>` 这种占位，说明这一枚值后面还得跟东西
        if let Some(need) = toks.get(2) {
            if need.starts_with('<') {
                if let Some(path) = &sample {
                    argv.push(path.clone());
                }
            }
        }
        cases.push((toks[1].to_string(), argv));
    }
    cases
}

/// 本文档点名的参数行数（`code` 段那三行由常量拼、不在表里，所以不计）。
pub fn doc_param_count() -> usize {
    PARAM_DOC.iter().filter(|(sec, _, _)| *sec != "eg").count()
}

/// §5.21.2 第三枚递给「命令行配置处」的那一句**入口**。住在这里是因为入口指的是本文档，而文档
/// 的主人只有这一个；设置页要拼这一句时问这一枚函数，不在那边再抄一遍 `--help` 的拼写。
/// 旗标名与参数枚数都是从 [`PARAM_DOC`] 现读的，所以文档改了入口跟着改。
pub fn doc_entry() -> String {
    let flag = help_flags().first().copied().unwrap_or("--help");
    format!(
        "{DOC_TITLE} 的参数文档：falconshot {flag}（本文档共 {} 枚参数）",
        doc_param_count()
    )
}

/// 写法那一列的列宽（显示列，不是字节数）。
const SYNTAX_COL: usize = 25;

/// 一格占几个显示列：ASCII 一枚一列，其余按全角算两列。
///
/// 这一枚规则只够用在这张表的**写法**列上 —— 那一列里的非 ASCII 只有 `路径` 两个汉字，而它们
/// 在等宽终端里正是两列。描述列不补齐（它已经贴到行尾），所以全角标点混在里面不影响对齐。
/// 「补齐按显示列而不是按字符数」这件事不是审美：`-o file <路径>` 差一列，整块帮助与 shipped
/// 的那一份就不再逐字节相同，本轮的证人（pre/post 两份档案对拷）当场就红。
fn width_of(s: &str) -> usize {
    s.chars()
        .map(|c| if (c as u32) < 0x80 { 1 } else { 2 })
        .sum()
}

/// 把一行的写法补到 [`SYNTAX_COL`] 列。描述为空（示例那几行）就不补，尾巴上也不留空格。
fn doc_line(syntax: &str, desc: &str) -> String {
    if desc.is_empty() {
        return format!("  {syntax}");
    }
    let pad = SYNTAX_COL.saturating_sub(width_of(syntax));
    format!("  {syntax}{}{desc}", " ".repeat(pad))
}

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
    let verb_set = verbs();
    let out_set = outputs();
    let help_set = help_flags();
    if args.iter().any(|a| help_set.contains(&a.as_str())) {
        return Decision::Help;
    }
    let has_verb = args.iter().any(|a| verb_set.contains(&a.as_str()));
    let has_output = args.iter().any(|a| out_set.contains(&a.as_str()));
    if !has_verb && !has_output {
        return Decision::NotCli;
    }
    // The door first: a §5.17 verb next to a measurement flag would run neither — the
    // mask covers the screen the door wants to read, and the door's `quit_after` ends
    // the run before the CLI's output could happen. Refuse the whole call as a
    // mis-invocation, the same voice as every other orphan.
    if let Some(hit) = args.iter().find(|a| reserved.contains(&a.as_str())) {
        // Two sentences, because they are two different mistakes. A measurement door beside a
        // capture verb is "one call does one thing"; `--snap` beside one is "that family already
        // ended the call itself" -- and 量具档 was never a true word about `--snap`, which measures
        // nothing. Which flags belong to that family is read off the doc table, not re-typed here.
        if snap_flags().contains(&hit.as_str()) {
            return Decision::Error(format!(
                "{hit} 属于贴图那一族，它自己就是收梢：不与 §5.17 的截图命令同开，也不读 -o"
            ));
        }
        return Decision::Error(format!(
            "{hit} 是量具档，不与 §5.17 的截图命令同开：一次调用要么截图、要么量一次"
        ));
    }
    let verbs: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| verb_set.contains(&a.as_str()))
        .map(|(i, _)| i)
        .collect();
    if verbs.len() > 1 {
        return Decision::Error(format!(
            "至多点一条截图命令：{} 只能选一",
            verb_set.join(" / ")
        ));
    }
    if verbs.is_empty() {
        // `-o` names where a picture goes, and there is no picture without a verb.
        return Decision::Error(format!(
            "只给了输出（{}）而没有截图命令：先点 {}",
            out_set.join(" / "),
            verb_set.join(" / ")
        ));
    }
    let vi = verbs[0];
    let verb = args[vi].as_str();
    let select = if verb == "--region" {
        // Two refusals, not one: no value after the verb is the arity case, and a value that is four
        // integers with a 0 in the width is §5.3.5's 「宽度和高度必须大于 0」. `rect_arg` owns both
        // sentences because it is the one reading the numbers, and it is handed this verb's name so the
        // line the user reads names the flag they typed (`--rect` has its own caller for the same text).
        match value_at(args, vi + 1) {
            None => {
                return Decision::Error(
                    "--region 后面要跟四个整数 x,y,w,h（例如 `--region 10,20,300,400`）"
                        .to_string(),
                )
            }
            Some(raw) => match crate::capture::rect_arg("--region", raw) {
                Ok(rect) => Select::Region(rect),
                Err(why) => return Decision::Error(why),
            },
        }
    } else if verb == "--last" {
        Select::Last
    } else if verb == "--active" {
        Select::Active
    } else {
        Select::New
    };

    let mut output = Output::Pin;
    if let Some(oi) = args.iter().position(|a| out_set.contains(&a.as_str())) {
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

/// P169：这一枚 `Decision` 该不该把「上一次区域」那份跨进程状态接进本进程。
///
/// 只有**真的点名了一枚截图动词、并且语法过得去**的那一支算。`Help` / `Error` / `NotCli`
/// 三趟一次也不该动盘：`--help` 写一份上一次区域，等于凭空给下一趟 `--last` 造出一个没有
/// 任何手势对应的矩形；`NotCli` 那一支更是整族的门口：遮罩、托盘、`--snap` 量具档、
/// `--selftest` 与 `cargo test` 全从这里出去，一次也到不了 `Run`，所以盘对它们一直是关着的
/// （P169 之前它们看见什么，P169 之后仍看见什么）。
pub fn opens_region_store(decision: &Decision) -> bool {
    matches!(decision, Decision::Run { .. })
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

/// `--snap` / `--snap-window` 这一族的三种收梢。
///
/// 这一族**不在** [`classify`] 的四个动词里：`--snap`、`--snap-window`、`--rect` 三枚被 `main` 的
/// `cli_reserved` 留给测量门，所以 §5.17 那三条规则对它一直没有人读——P167 ⑬ 记的就是这一格（拒掉
/// `--rect 10,20,0,-40` 那一腿，cmd.exe 交回的 rc 是 **0**，与钉成的那一腿同一个数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Snap {
    /// 钉成了：收梢留给整趟运行（[`snap_code`] 交回 `None`）——窗还没关，后面另有腿要说话。
    Pinned,
    /// 参数错：`--rect` 点了却没有值，或值读不成四个整数，或宽高里有一位不是正数。规则 2 的那一枚
    /// 码，调用者另打印 [`help`]。
    BadParam,
    /// 参数没毛病，是屏幕那一只手拒的：指针下面没有可截取的窗，或采集自己失败。规则 1 的失败码。
    CaptureFailed,
}

/// 同一族的状态码。号码仍然只从 [`EXIT_OK`] / [`EXIT_FAIL`] / [`EXIT_USAGE`] 三枚常量来，`main`
/// 里不另抄 `1` / `3`——这张表要是有两个出处，就没有任何东西能发现其中一格是过期的。
pub fn snap_code(snap: Snap) -> Option<i32> {
    match snap {
        Snap::Pinned => None,
        Snap::BadParam => Some(EXIT_USAGE),
        Snap::CaptureFailed => Some(EXIT_FAIL),
    }
}

/// argv 里那枚 `--rect` 的三种位置。
///
/// 上一版在 `main` 里直接 `position(...).and_then(|i| args.get(i + 1))`，于是两种缺值长成两种样子：
/// `--snap --rect` 后面什么都没有，被读成「没给」，静默变成中心截图；`--snap --rect --probe 1200`
/// 把 `--probe` 当成一串坐标去读，读不成报一句。两句都没有状态码。
///
/// 这里**不复用** [`value_at`]：那一枚以 `-` 开头就不认，而负坐标是合法的（`--rect -100,-50,400,300`
/// 在这一版读得成，混合 DPI 多屏那一档还开着，不能顺手把它关掉）。这一枚只认 `--` 开头的那一位是
/// 别的旗标，其余都当值交给 [`crate::capture::rect_arg`] 去读——读不成的那一句由它说，不由这里猜。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RectSlot<'a> {
    /// `--rect` 不在 argv 里：这一族自己的默认（屏幕正中一块）。
    Absent,
    /// `--rect` 在场，后面那一位不是另一枚旗标。
    Given(&'a str),
    /// `--rect` 在场而后面那一位不存在、或自己是一枚 `--` 旗标。
    Orphan,
}

/// 读 argv 里那枚 `--rect`。`--region` 那一路的同一件事在 [`classify`] 里，两处点的是不同的旗标名。
pub fn rect_slot(args: &[String]) -> RectSlot<'_> {
    match args.iter().position(|a| a == "--rect") {
        None => RectSlot::Absent,
        Some(i) => match args.get(i + 1) {
            Some(t) if !t.starts_with("--") => RectSlot::Given(t),
            _ => RectSlot::Orphan,
        },
    }
}

/// §5.17 rule 2's help text. Returned as one string so the `--help` leg and the
/// error leg print the *same* block, and the selftest gauge can check its shape with no
/// screen behind it.
///
/// §5.21.2 第三枚把这一枚函数从「手写十二行整串」改成了「把 [`PARAM_DOC`] 渲染一遍」：过去
/// 帮助里那四行动词、三行 `-o` 是**另抄的一份**，语法那一头改了什么这里不会跟着改，也没有
/// 一道门会发现。现在段落、写法、说明都只有一处出处，`code` 那一段是唯一由 `EXIT_*` 三枚常量
/// 现拼的例外（号码不许有第四份出处）。
///
/// 本轮的证人是「渲染出来的块与 shipped 的那一份逐字节相同」——那五枚新增行除外（`--output`、
/// `--help`、`-h` 三枚参数行，加一段标题与一行空行），因为补的就是文档漏讲的旗标。
pub fn help() -> String {
    let mut lines: Vec<String> = vec![DOC_TITLE.to_string(), String::new()];
    for (sec, header) in DOC_SECTIONS.iter() {
        lines.push(header.to_string());
        if *sec == "code" {
            lines.push(format!("  {EXIT_OK} 成功"));
            lines.push(format!(
                "  {EXIT_FAIL} 失败（截图没截成、或写文件/写剪贴板没落地——文件写入失败绝不返回成功）"
            ));
            lines.push(format!("  {EXIT_USAGE} 参数错误（打印本帮助）"));
        } else {
            for (row_sec, syntax, desc) in PARAM_DOC.iter() {
                if row_sec == sec {
                    lines.push(doc_line(syntax, desc));
                }
            }
        }
        lines.push(String::new());
    }
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
        // P165: four integers that include a 0 width is not an arity problem, and until this round it
        // was not a refusal at all — the number was rewritten to 1 and the capture ran.
        (
            "region without size",
            named(&["--region", "10,20,0,-40"]),
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
    // P170: rule 2's payload must name the family it refuses. Every flag the doc table puts under
    // `snap` has to (a) really belong to that family, (b) stay out of §5.17's own grammar, and
    // (c) be named by the sentence that turns it down beside a verb -- without being called a
    // gauge, which it is not. The list `main` reserves is rebuilt from snap_flags(), so a row
    // added here is a flag the door refuses the same moment, with no second copy to keep honest.
    let snap_reserved: Vec<&str> = snap_flags();
    for c in [SNAP_CENTRE, SNAP_WINDOW] {
        if !snap_openers().contains(&c) {
            bad.push(format!("{} 不在表里那一族的入口名单上", c));
        }
    }
    if snap_openers() != vec![SNAP_CENTRE, SNAP_WINDOW] {
        bad.push(format!(
            "表里那一族的入口不止/不是这两枚：{:?}",
            snap_openers()
        ));
    }
    for f in snap_reserved.iter().copied() {
        if snap_openers().contains(&f) {
            if snap_verb(&named(&[f])) != Some(f) {
                bad.push(format!("文档点名的 {f} 开不了贴图那一族"));
            }
            if !matches!(classify(&named(&[f]), &snap_reserved), Decision::NotCli) {
                bad.push(format!("{f} 单独喂给 §5.17 语法竟然不是 NotCli"));
            }
        } else if rect_slot(&named(&["--snap", f, "1,2,3,4"])) != RectSlot::Given("1,2,3,4") {
            bad.push(format!(
                "文档点名的修饰档 {f} 在语法里读不出它自己那行的写法"
            ));
        }
        match classify(&named(&[f, "--last"]), &snap_reserved) {
            Decision::Error(m) => {
                if !m.contains(f) || m.contains("量具档") {
                    bad.push(format!("{f} 与动词同开时那句拒绝没有点名它本身：{m}"));
                }
            }
            other => bad.push(format!("{f} 与 --last 同开竟然没被拒：{other:?}")),
        }
    }
    if snap_verb(&named(&["--last", "-o", "clipboard"])).is_some() {
        bad.push("§5.17 的动词被读成了贴图入口".to_string());
    }
    if rect_slot(&named(&["--snap", "--rect"])) != RectSlot::Orphan {
        bad.push("文档说了的「点了 --rect 不给值」那一格读不出 Orphan".to_string());
    }
    if verbs().iter().any(|v| snap_reserved.contains(v)) {
        bad.push("贴图那一族的旗标漏进了截图动词".to_string());
    }
    // 文档那一行说的「与 --snap 同开」，反面也得真被拒：没入口、以及落在那枚**不读矩形**的入口上。
    for (argv, open, want, why) in [
        (vec!["--rect", "1,2,3,4"], None, true, "一枚入口都没点"),
        (
            vec!["--snap-window", "--rect", "1,2,3,4"],
            Some(SNAP_WINDOW),
            true,
            "点的是不读矩形的那枚入口",
        ),
        (
            vec!["--snap", "--rect", "1,2,3,4"],
            Some(SNAP_CENTRE),
            false,
            "正着使的那一腿",
        ),
        (vec!["--snap"], Some(SNAP_CENTRE), false, "根本没点 --rect"),
    ] {
        let a: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        if rect_orphan(&a, open) != want {
            bad.push(format!("--rect 修饰档{why}那一格读反了：{argv:?}"));
        }
    }
    // 规则 2 交回的那个 3 得真的有人接得住：被参数错拒掉的贴图腿如果落进没有任何闹钟的事件环，
    // 那一趟就永不结束，码也永远回不到 cmd.exe。四格都在这一张表里，`main` 只照着它加闹钟。
    for (snap, alarmed, want, why) in [
        (
            Some(Snap::BadParam),
            0usize,
            true,
            "拒掉的贴图腿、且这张闹钟表一格都没点",
        ),
        (
            Some(Snap::BadParam),
            1usize,
            false,
            "别的门已经设了闹钟，这里不叠加",
        ),
        (
            Some(Snap::Pinned),
            0,
            false,
            "钉成的那一腿本来就是等真人的产品会话",
        ),
        (None, 0, false, "没走贴图这一族，欠不欠闹钟与它无关"),
    ] {
        if snap_refusal_needs_alarm(snap, alarmed) != want {
            bad.push(format!(
                "贴图腿缺闹钟那一格读反了：{snap:?} / alarmed={alarmed} / {why}"
            ));
        }
    }
    if SNAP_REFUSE_MS <= 0 {
        bad.push("贴图拒档那格闹钟是个非正数，等于没有闹钟".to_string());
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
    // The help is rule 2's payload, and it must actually carry the verbs -- and since P170, the
    // snap family too: a call that is refused for a mistyped `--rect` used to be handed a block
    // that had never heard of `--rect`.
    let h = help();
    for needle in [
        "--region",
        "--last",
        "--active",
        "--new",
        "-o clipboard",
        "--snap",
        "--snap-window",
        "--rect x,y,w,h",
    ] {
        if !h.contains(needle) {
            bad.push(format!("帮助里没有 {needle}"));
        }
    }
    let verdict = bad.is_empty();
    (
        crate::state::Check::from(verdict),
        format!(
            "{} 例 argv 判决 + 三条状态码规则 + 贴图那一族的 {} 枚旗标（文档点名即语法认）；{}（截屏像素本身在真机上量，非本档 §5.17 的 headless 半边）",
            cases.len(),
            snap_reserved.len(),
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

    /// §5.21.2 第三枚：语法是从 [`PARAM_DOC`] 数出来的，这一格钉住「数出来的正是 §5.17 那几枚」。
    /// 表里改一枚写法（多一枚、少一枚、拼错），这里先红 —— 而不是让语法安静地多认或少认一枚旗标，
    /// 帮助与语法各说各话那一枚坏就是这么来的。
    #[test]
    fn the_grammar_is_read_off_the_doc_table() {
        assert_eq!(
            verbs(),
            vec!["--new", "--region", "--last", "--active"],
            "截图动词与文档不同源了"
        );
        assert_eq!(outputs(), vec!["-o", "--output"], "输出选项与文档不同源了");
        assert_eq!(
            help_flags(),
            vec!["--help", "-h"],
            "取文档的旗标与文档不同源了"
        );
        assert_eq!(
            output_targets(),
            vec!["pin", "clipboard", "file"],
            "-o 的值与文档不同源了"
        );
        // 参数行只数 cmd/out 里点名的旗标与值：`eg` 那三行是整条示例，不是一枚参数。
        assert_eq!(
            snap_flags(),
            vec!["--snap", "--snap-window", "--rect"],
            "贴图那一族与文档不同源了"
        );
        assert_eq!(
            snap_openers(),
            vec!["--snap", SNAP_WINDOW],
            "那一族的入口不再是「写法不带第二个记号的那几行」了"
        );
        // P170：参数行从 10 枚长到 13 枚，长的三枚全是文档新点名的旗标 —— 是加了行才动的数，
        // 不是把这一格放低。
        assert_eq!(doc_param_count(), 13);
        // 文档点名的每一枚 `-o` 值，都得有一条**真喂得进语法**的完整 argv。`-o file <路径>`
        // 那一行的第三个记号是占位，说的正是「这一枚还欠一个值」——欠的那一枚从文档自己的
        // 示例行取，不在这里编造。
        let cases = output_cases();
        assert_eq!(
            cases.iter().map(|(v, _)| v.as_str()).collect::<Vec<&str>>(),
            vec!["pin", "clipboard", "file"],
            "可喂的输出案例与文档不同源了"
        );
        for (value, args) in cases.iter() {
            assert!(
                matches!(classify(args, R), Decision::Run { .. }),
                "文档点名的 -o {value} 用 {args:?} 喂不进语法"
            );
        }
        let file_case = cases
            .iter()
            .find(|(v, _)| v == "file")
            .unwrap_or_else(|| panic!("文档的 -o file 没有配案例"));
        assert_eq!(
            file_case.1.len(),
            4,
            "-o file 那一条没把文档示例里的路径补进来：{:?}",
            file_case.1
        );
        // 入口句子点的必须是真旗标，而且喂回语法真能取到文档。
        let entry = doc_entry();
        let flag = help_flags()
            .into_iter()
            .find(|f| entry.contains(f))
            .unwrap_or_else(|| panic!("入口没有点名任何一枚取文档的旗标：{entry}"));
        assert_eq!(classify(&argv(&[flag]), R), Decision::Help, "{entry}");
    }

    /// 渲染那一半的算术：写法列补到 **显示列**（全角算两列），行首两空格，描述一律落在第 27 列。
    /// 这些数是从 shipped 的 `--help` 那份档案上读出来的（本轮 pre 档案与 `p159_cols.log`），
    /// 按字符数补会在 `-o file <路径>` 那一行多出一个空格，整块就不再逐字节相同。
    #[test]
    fn the_syntax_column_is_padded_by_display_columns() {
        assert_eq!(SYNTAX_COL, 25);
        for (syntax, want) in [
            ("--new", 5),
            ("--region x,y,w,h", 16),
            ("--last", 6),
            ("--active", 8),
            ("-o pin", 6),
            ("-o clipboard", 12),
            ("-o file <路径>", 14),
            ("--output", 8),
            ("--help", 6),
            ("-h", 2),
        ] {
            assert_eq!(width_of(syntax), want, "写法 {syntax} 的显示列数");
            let desc = "测";
            let line = doc_line(syntax, desc);
            assert!(line.starts_with(&format!("  {syntax}")), "{line}");
            assert_eq!(
                width_of(&line),
                2 + SYNTAX_COL + width_of(desc),
                "补齐没落到第 {} 列：{line:?}",
                2 + SYNTAX_COL
            );
        }
        // 示例那几行没有描述：不补齐，尾巴上也不留空格。
        assert_eq!(doc_line("falconshot --active", ""), "  falconshot --active");
        for ln in help().lines() {
            assert!(!ln.ends_with(' '), "行尾留了空格：{ln:?}");
        }
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
            argv(&["--region", "10,20,0,-40"]),
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

    /// PRD 第 307 行「宽度和高度必须大于 0」落在 argv 这一头的形状：这是一次拒绝，不是一次改写，
    /// 而拒绝的话必须说得出它拒的是哪一枚参数、是哪一对数。`--region 10,20,0,-40` 在本轮之前是四个
    /// 整数里最合法的一条命令（它会真的产出一张 1×1 的图并退 0），所以这一格判的不是「有没有错」，
    /// 而是**错的那一句有没有把用户刚打的那两个字念回去**。
    #[test]
    fn a_region_without_size_is_refused_by_the_pair_that_broke() {
        for raw in ["10,20,0,40", "10,20,3,0", "10,20,0,-40", "10,20,-1,-1"] {
            let got = classify(&argv(&["--region", raw]), R);
            let Decision::Error(ref why) = got else {
                panic!("{raw} 的宽度或高度不是正数，却被答成 {got:?}");
            };
            assert!(
                why.contains("--region"),
                "{raw} 的拒绝没点名 --region：{why}"
            );
            assert!(why.contains("必须大于 0"), "{raw}：{why}");
            assert!(
                why.contains("w=") && why.contains("h="),
                "{raw} 没念出是哪一对：{why}"
            );
            // The arity sentence would be a lie here: these ARE four integers.
            assert!(
                !why.contains("四个整数"),
                "{raw} 是四个整数，不该被说成拼错：{why}"
            );
            assert_eq!(code_of(&got), Some(EXIT_USAGE), "{why}");
        }
        // One pixel is the smallest legal region and stays legal: the rule is 大于 0, not 够大.
        assert_eq!(
            classify(&argv(&["--region", "10,20,1,1"]), R),
            Decision::Run {
                select: Select::Region(PhysRect::new(10, 20, 1, 1)),
                output: Output::Pin
            }
        );
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

    /// §5.17 规则 1 在 `--snap` 这一族的落点：钉成的那一腿把码留给整趟运行，参数错拿规则 2 的那一
    /// 枚，屏幕那一只手拒的拿规则 1 的那一枚。号码只许来自 [`EXIT_OK`] / [`EXIT_FAIL`] /
    /// [`EXIT_USAGE`] 三枚常量，且两种失败读成两个数——把它们并成一个，账上就分不开「你打错了」与
    /// 「这台机器不给截」。
    #[test]
    fn the_snap_family_gets_its_codes_from_the_same_three_constants() {
        assert_eq!(snap_code(Snap::Pinned), None);
        assert_eq!(snap_code(Snap::BadParam), Some(EXIT_USAGE));
        assert_eq!(snap_code(Snap::CaptureFailed), Some(EXIT_FAIL));
        assert_ne!(
            snap_code(Snap::CaptureFailed),
            Some(EXIT_OK),
            "§5.17 规则 1 被写反了：拒掉的腿交回成功"
        );
        assert_ne!(
            snap_code(Snap::BadParam),
            snap_code(Snap::CaptureFailed),
            "参数错与采集失败不许读成同一个数"
        );
    }

    /// 点了 `--rect` 就是给了参数。缺值那一格上一版读成「没给」，于是 `--snap --rect` 静默截了屏幕
    /// 正中一块；这里把它读成参数错。反过来，负起点那一串仍是一位**值**而不是旗标——混合 DPI 多屏
    /// 那一档还开着，`--rect -100,-50,400,300` 不能在这一轮被顺手关掉。
    #[test]
    fn a_named_rect_without_a_value_is_orphan_while_a_negative_origin_stays_a_value() {
        assert_eq!(rect_slot(&argv(&["--snap"])), RectSlot::Absent);
        assert_eq!(
            rect_slot(&argv(&["--snap", "--rect", "1,2,3,4"])),
            RectSlot::Given("1,2,3,4")
        );
        assert_eq!(
            rect_slot(&argv(&["--snap", "--rect", "-100,-50,400,300"])),
            RectSlot::Given("-100,-50,400,300"),
            "负坐标被读成了旗标"
        );
        assert_eq!(rect_slot(&argv(&["--snap", "--rect"])), RectSlot::Orphan);
        assert_eq!(
            rect_slot(&argv(&["--snap", "--rect", "--probe", "1200"])),
            RectSlot::Orphan
        );
    }

    /// P169：谁能动「上一次区域」那份盘。答案是**只有语法过得去的那一枚截图动词**，而这一格
    /// 就是 `main` 里那一句 `if` —— 拆出来是为了让 `cargo test` 问得到（§10 第 40 项 ③）。
    /// 三枚 `false` 各有其所以坏：`--help` 写一份上一次区域等于凭空造出一枚没有手势对应的矩形；
    /// 拒掉的调用一次屏幕都没碰；遮罩、`--snap` 量具档与 `cargo test` 自 P169 之前就没动过盘，
    /// 之后也不许动 —— 它们眼里的「上一次区域」仍是各自那一份进程内记忆。
    #[test]
    fn only_a_named_capture_verb_opens_the_cross_process_state() {
        let snap_reserved: &[&str] = &["--snap", "--snap-window", "--rect", "--probe"];
        for (words, reserved, want) in [
            (vec!["--last", "-o", "clipboard"], R, true),
            (vec!["--last"], R, true),
            (vec!["--new"], R, true),
            (vec!["--region", "10,20,300,400", "-o", "pin"], R, true),
            (vec!["--active", "-o", "file", "C:/x.png"], R, true),
            (vec!["--help"], R, false),
            (vec!["--region"], R, false),
            (vec!["--last", "-o", "wat"], R, false),
            (vec!["--selftest", "--mask"], R, false),
            (
                vec!["--snap", "--rect", "1,2,3,4", "--probe", "1200"],
                snap_reserved,
                false,
            ),
            (vec![], R, false),
        ] {
            let owned = argv(&words);
            let got = opens_region_store(&classify(&owned, reserved));
            assert_eq!(got, want, "{} 这一串的落点读成了 {got}", words.join(" "));
        }
    }
}
