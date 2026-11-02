//! §5.5.10「发送到其他应用」的那一扇门：`--selftest --sendto`。
//!
//! 它判的是下面那张表里的事，逐条对着 PRD 562-575 那三步与两条规则（条数由那张表自己数，写在
//! 句子只会漂）：
//!
//! | 本档那一格 | 需求里的那一句 |
//! | --- | --- |
//! | `targets configured` | 步骤 1「用户在设置中配置目标应用」+ 规则 1「应允许配置多个目标应用」 |
//! | `arm in sentence` | 步骤 3「系统将图片作为剪贴板内容、**文件参数**或输入对象传递」—— 三臂里挑了哪一臂，读产品自己说的那一句 |
//! | `argument arrives whole` | 步骤 3 的**对面**：目标程序自己报出它收到几条参数、收到的是哪一条 |
//! | `second own file` | 规则 1 的另一半：两枚各得各的一张；而只写了名字的条目由产品自己在这台机器上找到 |
//! | `missing target asks` | 规则 2「目标程序不存在时应提示重新设置路径」 |
//! | `user config untouched` | §9.1 第十五条：走过产品的门，就得把产品的副作用一起带走 |
//! | `chip wired in qml` | 步骤 2「点击"发送到应用"」—— 那枚芯片按下去走的是不是本档按的那一手（源码扫描） |
//!
//! ## 对面那一枚程序是真的
//!
//! 前五道门的对面都在本进程里（剪贴板、盘、屏幕、名单）。这一档不是：`--sendto` 要证的正是
//! 「一条参数交到了**别人手里**」，而"交到"这件事只有接的那一位能作证。所以本档在自家目录里
//! 种一枚 `receiver.cmd`，它把 `%*`（shell 收到的整串参数）与 `[%2]`（第二条参数，没有就只剩
//! 一对空括号）写进 `marker.txt` 的两行，本档再逐字符读那两行。判据是**对面报告的字节**与本档
//! 递给产品的那一枚路径拼出来的期望串相等，不是"退出码为 0"，也不是产品自己那句"已按文件参数…"
//! ——后者是 `arm in sentence` 那一格读的，两格各问一件事，同一句不许当两枚证据（#281 的反面：
//! 一句证据只算一次账）。
//!
//! 第一行连着产品加的那对引号一起比，所以它判的是**补引号这一手真补上了**；第二行的空括号判的
//! 是**只递到一条**。这一趟落盘的那枚路径在这台机器上没有空格（`%TEMP%` 这一串就没有），所以
//! 本档不许在这里说"带空格的路径也没被拆开"——那一半由 core 的单测
//! `a_path_that_will_not_quote_is_refused_rather_than_spliced` 盯着（它喂的是 `C:\My Shots\a b.png`）。
//!
//! ## 为什么这里值得真开一次 shell
//!
//! `ShellExecuteW` 是异动的：递出去就回 `Ok`，对面什么时候写下 marker 不归它管。于是本档**等**
//! （[`WAIT`]），并把"等不到"判成红而不是判成阻塞 —— 递出去了而对面一个字也没写，那是这一档
//! 要抓的那一种错，不是这台机器不肯被看。反面那一臂（目标不在盘上）反之：它一次 shell 也不许
//! 碰，所以它判的是 marker **没动**，而那一句"没动"要有一段观察窗口才说得出（[`SILENCE`]）。
//!
//! ## 这一档的代价，写在报告里而不是藏起来
//!
//! 每一次真派发都会闪一枚控制台窗（`.cmd` 由 cmd 解释器跑，闪一下即关）。它与 `--reveal` 那
//! 一档留下的 explorer 窗同一族：本进程举不到它，也就没有"跑完什么都没发生"可说。因此这一档
//! 不给默认值、只在 `--selftest --sendto` 被单独点名的那一趟里跑。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use falcon_core::geometry::PhysRect;

use crate::mask;
use crate::r13::Report;
use crate::state::Check;

/// 这一档自己的目录：种 `receiver.cmd`、收 `marker.txt`、钉这一趟的写回。名字从
/// `FALCONSHOT_SENDTO_DOOR_DIR` 来，与别的档各用各的那一枚（同一台机器上两趟并行时，marker
/// 必须是各自的文件，否则后跑的那一趟读到的是前一趟的证据）。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_SENDTO_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-sendto-door");
    dir
}

/// 这一趟的写回落点，`pin` 与 `writes pinned` 那一格各算一次同一个数。
fn pinned_path() -> PathBuf {
    door_dir().join("pinned-config.toml")
}

/// 对面那一枚程序：两行——**整串参数**与**第二条参数在不在**。
///
/// 三行各自成立的理由，每一行都是这一档踩过之后才写下的：
///
/// - **一行只放一枚 `%x`**。`echo "%# %~1"` 那一版在这台机器上写下的是 `~1`：cmd 把 `%# %`
///   当成一对变量引用吞了，而 `%#` 本身不是 batch 参数。两枚读数因此分家成两行，各写各的。
/// - **`%*` 而不是 `%1`**：`%*` 是 shell 收到的原话，引号在不在都原样留着，所以它是"补引号
///   这一手"的证人；`%~1` 会把引号剥掉，那就问不出这一件事。
/// - **每行末尾不带空格**：`echo` 把行尾空格也一起写下去，期望串里多一枚空格这一格就永远红。
///   重定向紧贴 `>`（中间没有空格），探针（`%TEMP%\p154_probe\recv2.cmd` + 一枚带空格的路径）
///   实测那两行就是 `"…\a dir\shot.png"` 与 `[]`。
///
/// 引号包住整个 echo 的内容那一版已废弃（见上一条）。现在 `%*` 自己带着产品补的那对引号，
/// 本档的期望串连引号一起拼，逐字符比，不去"读懂"它（读法就是断言，断言就是读法）。
const RECEIVER: &str =
    "@echo off\r\necho %*>\"%~dp0marker.txt\"\r\necho [%2]>>\"%~dp0marker.txt\"\r\n";

/// 递出去之后最多等多久：`ShellExecuteW` 立刻回话，cmd 写 marker 在它自己的时间里。
const WAIT: Duration = Duration::from_secs(8);
/// 「什么都没有发生」那一格的观察窗口：等过这一段 marker 还是原样，才说得出"没有走到 shell"。
const SILENCE: Duration = Duration::from_millis(400);

/// `Settings::line()` 里那两枚**路径**字段：`from=` 是本进程真读了哪一份，`write_to=` 是允许
/// 写去哪一份。按字段名切而不是按空格切 —— 这两枚是路径，路径里可以有空格（`reveal_check` 的
/// `read_and_write` 读的是 `read=`，那一枚是 `file`/`defaults` 这一枚 tag，不是路径）。
fn read_and_write() -> (PathBuf, PathBuf) {
    let line = crate::settings::with(|s| s.line());
    let after = |name: &str, stop: Option<&str>| -> String {
        let rest = line
            .split_once(name)
            .map(|(_, r)| r)
            .unwrap_or_default()
            .trim_start();
        match stop {
            Some(s) => rest
                .split_once(s)
                .map(|(v, _)| v)
                .unwrap_or(rest)
                .trim_end()
                .to_string(),
            None => rest.to_string(),
        }
    };
    (
        PathBuf::from(after("from=", Some(" write_to="))),
        PathBuf::from(after("write_to=", None)),
    )
}

/// 一份档的字节，读不到就当它不在（首趟运行的 `%APPDATA%` 里可以压根没有 config.toml ——
/// 「没有这份档」与「这份档被本档改过」是两件事，但都能用同一枚 `Option` 说清）。
fn bytes_of(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// 一个目录里的文件名清单，排好序。判"没多出一张"用的是清单而不是计数：计数相等而名字换了，
/// 是同一批文件被收回又写了一张别的，那一趟的证据得能在报告里被看见。
fn census(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// 等对面写下与 `want` 逐字符相同的一句。返回**最后读到的那一句**与等了多久：等不到时报告里
/// 印的是对面真正说了的话，而不是"没读到"。
fn wait_marker(marker: &Path, want: &str) -> (Option<String>, u64) {
    let started = Instant::now();
    let mut last: Option<String> = None;
    loop {
        if let Ok(raw) = std::fs::read(marker) {
            let line = String::from_utf8_lossy(&raw).trim().to_string();
            if !line.is_empty() {
                if line == want {
                    return (Some(line), started.elapsed().as_millis() as u64);
                }
                last = Some(line);
            }
        }
        if started.elapsed() >= WAIT {
            return (last, started.elapsed().as_millis() as u64);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// 对面那两行的期望串：第一行是产品补过引号的整串参数，第二行是一对空括号（没有第二条）。
/// `\r\n` 由 [`wait_marker`] 的 `trim()` 收掉行尾那一枚，行间那一枚留着——它是证据的**形状**。
fn want_line(spelled: &str) -> String {
    format!("\"{spelled}\"\r\n[]")
}

/// 报告里一行就是一格，所以对面那两行要**说**而不是**印**：`\r\n` 写成可见的两枚转义，读者对得
/// 回档里的字节，而矩阵不会被一格的读数劈成两行。
fn shown(raw: &str) -> String {
    raw.replace('\r', "\\r").replace('\n', "\\n")
}

/// 步骤 2 在窗里的那一段，源码这一头能问的部分：AnnotationToolbar 那一排芯片**接的是不是
/// 本档按的那枚 invokable**。`--sendto` 不开窗，所以这一格只能扫档；[`crate::hint_check::qml_code`]
/// 是全档唯一一把剥注释的尺子（两把必然漂移，漂移的方向是把一句散文判成代码）。
///
/// P157 起它另有第二枚读者：默认 `--selftest` 报告里 §5.21.2 那一格也要读同一份源码。它因此
/// 从档内私有提成 `pub(crate)`，并且**从 `const` 换成 `static`** —— `const` 在每个使用点重新
/// 展开，两处读就在出货 exe 里多落一份 73,220 B 的明文（本轮实测：`const` 那一代比上一代重
/// 75,776 B、整档明文在 exe 里出现 3 次；换 `static` 的那一代回落 74,752 B、出现 2 次）。
/// 而那 2 次不是「两枚读者各带一份」：Qt 的资源管线本来就把这份 `.qml` 编进 exe —— 全档 8 枚
/// `.qml` 里 7 枚各在 exe 现读 1 份、`SettingsWindow.qml` 现读 0 份 —— `include_str!` 只是又
/// 添一份能让 `&'static str` 直接指的字节。两个读者读同一份字节，才谈得上「不一致时有一方是
/// 错的」（#281 同一族）。
pub(crate) static TOOLBAR_QML: &str = include_str!("../qml/AnnotationToolbar.qml");

/// 剥过注释再把连续空白压成一枚空格。归一化的是**排版**：`Row { spacing: 6 }` 换不换行、委托
/// 里 `label:` 与 `onTapped:` 谁先写，都不该让这一格红；而界限符与括号才是判据。
fn one_space(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending = true;
            continue;
        }
        if !out.is_empty() && pending {
            out.push(' ');
        }
        pending = false;
        out.push(c);
    }
    out
}

/// 扫的是 `model: bar.send_to_menu` 与 `model: bar.share_menu` 之间那一段——两枚 Repeater 是同
/// 一排里的兄弟，这一道界把"芯片那串"与"分享那一串"分开，于是 `sendToTarget(index)` 若是被挪到
/// 分享那一排去接，本档读到的那一段里就没有它，这一格会红。
///
/// 两个界符有一个找不到就是 **FAIL** 而不是绿：空切片会让所有 needle 都"不在场"，而形状变了与
/// 接线断了是两件事，报告里得说清是哪一件（#269 那一族：尺子读回空集时的绿最没有分量）。
fn chip_wiring() -> (Check, String) {
    let flat = one_space(&crate::hint_check::qml_code(TOOLBAR_QML));
    let begin = "model: bar.send_to_menu";
    let end = "model: bar.share_menu";
    let at = flat.find(begin);
    let stop = flat.find(end);
    let slice = match (at, stop) {
        (Some(a), Some(b)) if b > a => Some(flat[a + begin.len()..b].to_string()),
        _ => None,
    };
    let needles = [
        ("名单那一手 = sendToMenuNames()", r#"property var send_to_menu: bar.view.sendToMenuNames().split("|")"#, false),
        ("这一排有条件才被画", "visible: bar.send_to_menu.length > 0 || bar.share_menu.length > 0 || bar.view.send_to_last.length > 0", false),
        ("派发那句回到同一排", "text: bar.view.send_to_last", false),
        ("可点的是 Cell", "delegate: Cell {", true),
        ("委托带序号", "required property int index", true),
        ("按下走 invokable", "onTapped: bar.view.sendToTarget(index)", true),
    ];
    match &slice {
        None => (
            Check::Fail,
            format!(
                "两枚界符没成对：`{begin}` 在={}、`{end}` 在={}（去注释后 {} 字符）——那一排的**形状**变了，\
                 这一格问不出接线，判红不判绿",
                at.is_some(),
                stop.is_some(),
                flat.chars().count()
            ),
        ),
        Some(body) => {
            let missing: Vec<&str> = needles
                .iter()
                .filter(|(_, needle, in_body)| {
                    // `in_body` 为真的那几枚要求在**界内**（那一排自己的委托）；其余写在界外——
                    // 属性声明在整档的上头、`visible` 在那一排的头上、派发那句在 share 之后。
                    !(if *in_body { body.contains(needle) } else { flat.contains(needle) })
                })
                .map(|(label, _, _)| *label)
                .collect();
            (
                Check::from(missing.is_empty()),
                format!(
                    "扫的是 {} 与 {} 之间那 {} 字符（全档去注释后 {} 字符、原档 {} 字符），针 {} 枚：{}。\
                     这一格答的是\"按下去走的是哪一手、这一排按什么条件被画\"，答不了\"这一排此刻在不在\
                     屏幕上\"——后者记在下面那一条读数里",
                    begin,
                    end,
                    body.chars().count(),
                    flat.chars().count(),
                    TOOLBAR_QML.chars().count(),
                    needles.len(),
                    if missing.is_empty() {
                        "全在场".to_string()
                    } else {
                        format!("缺 {:?}", missing)
                    },
                ),
            )
        }
    }
}

/// 按一次芯片：走的是产品那一手 [`mask::MaskState::send_to_target`]，交回它自己的读数 ——
/// 成没成、说了哪句、三枚计数、递出去的那一枚落在哪。本档不替它判任何一件事。
fn click(index: usize) -> (bool, String, u32, u32, u32, Option<PathBuf>) {
    mask::with(|m| {
        let done = m.send_to_target(index);
        (
            done,
            m.send_to_last.clone(),
            m.send_to_clicks,
            m.send_to_acted,
            m.send_to_writes,
            m.send_to_landed.clone(),
        )
    })
}

/// §5.5.10 的门。开头那一行 `desktop scene` 是每扇门共同的上下文
/// （[`crate::r13::scene_row`]）；冻不下屏幕时后面各格提前收，那时印出的只有上下文与那一格
/// BLOCKED —— 每一臂都要一张真拼出来的图，那是这一档与 `--whiteboard` 那一族不同的地方。
pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let dir = door_dir();
    let receiver = dir.join("receiver.cmd");
    let marker = dir.join("marker.txt");
    let planted =
        std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&receiver, RECEIVER.as_bytes()));
    // marker 是**对面的话**：本档不许带着上一趟（甚至上一臂）留下的那句话去读下一次派发。
    // 它不在盘上是正常起手，所以这一步不进 `planted` 那一串 —— 一次"本来就没有"不能被读成
    // "种不下去"，那是把环境的事实报成这一档的错（#453 的同一族）。
    let cleared = std::fs::remove_file(&marker);
    let absent = dir.join("not-here-on-this-machine.exe");
    // 反面那一臂的 fixture 得先证明它真的不在：一台上一趟留下的同名文件会让"提示重新设置路径"
    // 那一格读到一次真派发，而那句红会被读成产品的错。
    let _ = std::fs::remove_file(&absent);
    if planted.as_ref().err().is_some() || !absent.try_exists().is_ok_and(|v| !v) {
        rep.row(
            "receiver planted",
            Check::Blocked,
            format!(
                "对面种不下去或它已经在盘上：{}（{:?}）；{} 不存在={}",
                receiver.display(),
                planted,
                absent.display(),
                absent.try_exists().map(|v| !v).unwrap_or(false)
            ),
        );
        return rep.finish("sendto door");
    }

    // ---- 开工前把用户那份档的字节抄下来。本档随后会经产品的门改**内存里**那一枚键，所以这一格
    // 必须在任何一手之前取，事后同一枚数再取一次才叫"逐字节没动"。
    let (from, _) = read_and_write();
    let user_before = bytes_of(&from);

    // ---- 钉写回：`settings_view::write` 会落盘，落点不钉就是拿用户的 config.toml 当草稿纸。
    let pinned = crate::settings::pin_writes(&pinned_path());

    // ---- 步骤 1 + 规则 1：三枚条目各问一件事——一枚给全路径（正面那一臂）、一枚只给名字
    // （[`falcon_core::sendto::locate`] 在 PATH 里找）、一枚给一台机器上没有的路径（规则 2）。
    // 走的是设置页那一手（[`crate::settings_view::write`]），不是本档自己往文件里追加一行：
    // 「用户在设置中配置目标应用」的对面是那枚闸，而不是那份档。
    let asked = format!(
        "整路径那枚|{}, 只写名字那枚|receiver.cmd, 缺席那枚|{}",
        receiver.display(),
        absent.display()
    );
    let written = crate::settings_view::write("output.send_to_apps", &asked);
    let live = crate::settings::output_cfg();
    let (final_ok, parse_refused, validate_refused) =
        crate::settings::pipeline_send_to(&live.send_to_apps);
    let chips = crate::settings::send_to_menu_entries(&final_ok);
    rep.row(
        "targets configured",
        Check::from(
            written.ok()
                && final_ok.len() == 3
                && parse_refused == 0
                && validate_refused == 0
                && chips.len() == 3
                && final_ok.iter().position(|(n, _)| n == "整路径那枚") == Some(0)
                && final_ok.iter().position(|(n, _)| n == "只写名字那枚") == Some(1)
                && final_ok.iter().position(|(n, _)| n == "缺席那枚") == Some(2),
        ),
        format!(
            "页那一手：{}；盘上条目 {} 枚、pipeline 收下 {} 枚（parse 拒 {parse_refused}、扩展名拒 \
             {validate_refused}），芯片那一串 {:?}。序号是这一档唯一的坐标：第 i 枚芯片按下去叫的\
             是第 i 枚条目，所以这里判的是**顺序**而不是集合。那一手要不要重启={}",
            written.text(),
            live.send_to_apps.len(),
            final_ok.len(),
            chips,
            written.restart,
        ),
    );

    let (_, write_to) = read_and_write();
    rep.row(
        "writes pinned",
        Check::from(write_to == pinned && from != write_to),
        format!(
            "本进程读 {}（存在={:?}），允许写 {}（存在={:?}）；钉到 {} ——两者不同才说明这一档改的是\
             内存里那一枚键，而不是用户的档",
            from.display(),
            from.exists(),
            write_to.display(),
            write_to.exists(),
            pinned.display(),
        ),
    );

    // ---- 主体：一张真的遮罩流程。`send_to_target` 递的是 [`MaskState::delivered`] 那一张，
    // 而那一手要有冻过的画面与一块选区才拼得出来 —— 本档不拿一张造出来的图冒充用户的截图。
    let opened = match mask::open_from_freeze(false) {
        Ok(o) => o,
        Err(e) => {
            rep.row(
                "mask flow opened",
                Check::Blocked,
                format!("遮罩开不起来（{e}）：后面每一格要的都是一张真拼出来的图"),
            );
            return rep.finish("sendto door");
        }
    };
    let Some(primary) = opened
        .snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .or_else(|| opened.snap.monitors.first())
    else {
        rep.row(
            "mask flow opened",
            Check::Blocked,
            format!("冻到 {} 屏但没有一块可选的", opened.snap.monitors.len()),
        );
        return rep.finish("sendto door");
    };
    let pb = primary.info.bounds;
    let hole = PhysRect::new(pb.x + 40, pb.y + 40, 64, 48);
    mask::with(|m| m.set_hole(hole));
    let product_dir = mask::MaskState::sendto_dir();
    let _ = std::fs::create_dir_all(&product_dir);
    let start_left = census(&product_dir);
    rep.row(
        "mask flow opened",
        Check::from(mask::with(|m| m.has_selection())),
        format!(
            "冻了 {} ms、覆盖 {} 屏，选区递的是 {hole:?}；发送用的临时目录是 {}，开工时那里已有 {} 份：{:?}；\
             对面那一份话（marker.txt）起手擦过={:?}（NotFound 才是正常的开局）",
            opened.ms,
            opened.slots,
            product_dir.display(),
            start_left.len(),
            start_left,
            cleared,
        ),
    );

    // ---- 第一臂（正面，全路径）：产品挑了哪一臂，读它自己说的那一句。
    let (done0, said0, clicks0, acted0, writes0, landed0) = click(0);
    let spelled0 = landed0.as_ref().map(|p| p.to_string_lossy().into_owned());
    rep.row(
        "arm in sentence",
        Check::from(done0 && said0.starts_with("已按文件参数") && clicks0 == 1 && acted0 == 1),
        format!(
            "第 0 枚按下去={done0}，话={said0:?}；计数 clicks={clicks0} acted={acted0} writes={writes0}\
             （各要求 1），递出的那一枚={spelled0:?}。要求那句以「已按文件参数」开头：三臂的名字写在\
             [`falcon_core::sendto::Handover`] 里，本档读的是产品印出来的那一串而不是再判一遍"
        ),
    );

    // ---- 步骤 3 的对面：那一条参数有没有**整条**到达。期望串由本档递给产品的那个路径拼出来，
    // 所以这一格红的时候，红的是"路上掉了字"，不是"这一档不会判"。
    let want0 = spelled0.as_ref().map(|p| want_line(p)).unwrap_or_default();
    let (got0, ms0) = wait_marker(&marker, &want0);
    rep.row(
        "argument arrives whole",
        Check::from(!want0.is_empty() && got0.as_deref() == Some(want0.as_str())),
        format!(
            "对面写下 {}（等了 {ms0} ms，上限 {} ms）；要求逐字符等于 {want0:?}。第一行是 `%*`——shell \
             收到的**整串**参数，产品补的那对引号在不在里面一并对账；第二行是 `[%2]`，一对**空**括号说明\
             这条路上只递到一条参数。这一趟落盘的那枚路径在这台机器上没有空格，所以「带空格的路径也不被\
             拆开」那一问不在这里答（它归 core 的 `a_path_that_will_not_quote_is_refused_rather_than_spliced`）",
            got0.as_deref().map(shown).unwrap_or_else(|| "一个字也没有".to_string()),
            WAIT.as_millis()
        ),
    );

    // ---- 第二臂（规则 1 的另一半）：清单上的第二枚只写了名字。本档把这一档目录放进 PATH，
    // 于是 [`falcon_core::sendto::locate`] 找的是这台机器真的搜得到的那一条；递成功后对面写下的
    // 是**另一张**文件——两枚各得各的图，才是"可以配置多枚目标应用"的意思。
    let _ = std::fs::remove_file(&marker);
    let path_before = std::env::var_os("PATH");
    let with_dir = match path_before.clone() {
        Some(p) => {
            std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(&p)))
        }
        None => std::env::join_paths(std::iter::once(dir.clone())),
    };
    let armed = match with_dir {
        Ok(joined) => {
            std::env::set_var("PATH", joined);
            true
        }
        Err(_) => false,
    };
    let (done1, said1, clicks1, acted1, writes1, landed1) = click(1);
    if let Some(p) = path_before.clone() {
        std::env::set_var("PATH", p);
    }
    let spelled1 = landed1.as_ref().map(|p| p.to_string_lossy().into_owned());
    let want1 = spelled1.as_ref().map(|p| want_line(p)).unwrap_or_default();
    let (got1, ms1) = wait_marker(&marker, &want1);
    // 两枚路径各自来自一次真派发，所以"两张不同的图"是盘上的读数而不是推断；PATH 在按完之后就
    // 换回去了，本档不在自己之外留改动。
    rep.row(
        "second own file",
        Check::from(
            armed && done1 && clicks1 == 2 && acted1 == 2 && writes1 == 2
                && spelled1.is_some()
                && spelled0 != spelled1
                && got1.as_deref() == Some(want1.as_str()),
        ),
        format!(
            "只写名字的条目（`receiver.cmd`）在 PATH 里{}；第 1 枚按下去={done1}，计数 \
             clicks={clicks1} acted={acted1} writes={writes1}（各要求 2），话={said1:?}；这一枚递的是 \
             {spelled1:?}，与第 0 枚那一张不同={}；对面写下 {}（等了 {ms1} ms），要求逐字符等于 {want1:?}",
            if armed { "被本档放进去了" } else { "放不进去" },
            spelled0 != spelled1,
            got1.as_deref().map(shown).unwrap_or_else(|| "一个字也没有".to_string()),
        ),
    );

    // ---- 规则 2 那一臂：目标不在盘上。三件事一起才算：说了「重新设置路径」、一次 shell 也没碰、
    // 白写的那一张收回了。第二件判的是 marker **没动**（含 mtime），所以它需要一个观察窗口——
    // 递出去之后立刻回头看一眼就说"没发生"，那句是没有分量的（#455）。
    let marker_state = || {
        (
            std::fs::read(&marker).ok(),
            std::fs::metadata(&marker).and_then(|m| m.modified()).ok(),
        )
    };
    let before_state = marker_state();
    let before_left = census(&product_dir);
    let (done2, said2, clicks2, acted2, writes2, _) = click(2);
    std::thread::sleep(SILENCE);
    let after_state = marker_state();
    let after_left = census(&product_dir);
    // 判的是**字节加 mtime**（`before_state == after_state`），印的是那句话与那个时间：整条字节
    // 数组印两遍，读者要在一行里数几百枚数字，而这一格的对面本来只有一句 marker。
    let marker_now = after_state
        .0
        .as_ref()
        .map(|b| shown(&String::from_utf8_lossy(b)))
        .unwrap_or_else(|| "(这一枚 marker 文件不在)".to_string());
    rep.row(
        "missing target asks",
        Check::from(
            !done2
                && said2.contains("重新设置路径")
                && clicks2 == 3
                && acted2 == acted1
                && writes2 == writes1
                && before_state == after_state
                && after_left == before_left,
        ),
        format!(
            "第 2 枚按下去={done2}（要求 false），话={said2:?}（要求含「重新设置路径」）；计数 \
             clicks={clicks2} 而 acted 停在 {acted2}、writes 停在 {writes2}（各要求不动）；marker 的字节\
             与 mtime 在 {} ms 里前后两次读数一致={}（现在是 {marker_now}，mtime {:?}），临时目录 {} 里还是 \
             {} 份={before_left:?}——先落盘后问目标在不在的那一枚白写的图，收回来了",
            SILENCE.as_millis(),
            before_state == after_state,
            after_state.1,
            product_dir.display(),
            after_left.len(),
        ),
    );

    // ---- §9.1 第十五条：这一趟走了产品的两道门（设置页那一手、遮罩那一路），用户的档得逐字节
    // 还是开工前那一枚。`saves=` 是产品自己数的写盘次数，与"用户的档没动"不矛盾——矛盾的那一版
    // 是没钉。
    let user_after = bytes_of(&from);
    let saved_to = write_to.exists();
    rep.row(
        "user config untouched",
        Check::from(user_after == user_before && saved_to),
        format!(
            "{} 的字节前后一致={}（前 {} B / 后 {} B）；本档的写全落在 {}（存在={saved_to}），\
             本进程 saves={}",
            from.display(),
            user_after == user_before,
            user_before.as_ref().map(|b| b.len()).unwrap_or(0),
            user_after.as_ref().map(|b| b.len()).unwrap_or(0),
            pinned.display(),
            crate::settings::with(|s| s.line())
                .split("saves=")
                .nth(1)
                .and_then(|t| t.split(' ').next())
                .unwrap_or("?"),
        ),
    );

    // ---- 步骤 2 的那一半源码问得出：芯片按下去走的是不是上面那几格读过的那一手。
    let (wire_check, wire_line) = chip_wiring();
    rep.row("chip wired in qml", wire_check, wire_line);

    // ---- 而"屏幕上有一排能点的芯片"这一问，源码与 invokable 都答不了：它的对面是一扇真窗里
    // 的 `visible` 与 `height` 成对写。那一排绑定在场（上面那一格的读数之一），在场不等于被画
    // 出来，所以这里记的是**读数**而不是判定——本档不开窗，这是这台机器的这一档的盲处，不是
    // 一条永远红掉的判据（#433 不许留 dead instrument row，#269 不许留空集上的绿）。
    rep.note(
        "chip on screen",
        "本档一次窗也没开：那一排的 `visible` 绑定与委托接线都在场（上一格数的那几枚针），但\"\
         这一排此刻被画出来了、那枚 Cell 接受了一次真鼠标\"这一问只有真窗答得出。同族的前例是 \
         `--hint` 那几格：运行期证人只有门跑起来才在场。"
            .to_string(),
    );

    // ---- 收梢：本档递出去而产品留着的那一张撤掉，临时目录回到开工时那一份清单。对面那枚
    // `receiver.cmd` 与 `marker.txt` **留着**：那是这一档自己的证据，与 `--refresh` 留下的
    // fixture 同一族（档案归跑级的人收，不在这里顺手销毁读数）。
    let reaped = mask::with(|m| m.send_to_landed.take())
        .as_ref()
        .map(|p| std::fs::remove_file(p).is_ok())
        .unwrap_or(true);
    let left = census(&product_dir);
    rep.row(
        "nothing of ours left",
        Check::from(left == start_left && reaped),
        format!(
            "撤掉留着的派发货={reaped}；{} 现在 {} 份，与开工时那 {} 份一致={}（{:?}）；本档目录留着 \
             receiver.cmd 与 marker.txt 作证据",
            product_dir.display(),
            left.len(),
            start_left.len(),
            left == start_left,
            left
        ),
    );

    rep.note(
        "sendto door cost",
        format!(
            "这一档真派发过 {} 次，每次都闪一枚控制台窗（`.cmd` 由 cmd 解释器跑）。它不给默认值、\
             不进跑级的默认几趟，只在被点名时跑——与 `--reveal` 留下一扇 explorer 窗同一族。",
            acted2
        ),
    );
    rep.note(
        "sendto door live settings",
        format!(
            "本趟把内存里的 output.send_to_apps 换成了这一串 fixture（{}），pipeline 收下 {} 枚；写只落在\
             钉过的 {}。进程一退，内存那份就没了——下一次真的产品会话读的仍是用户档里那一枚键。",
            asked,
            final_ok.len(),
            pinned.display()
        ),
    );
    rep.finish("sendto door")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`one_space`] 自己那一腿的反证：上面那一格整个建立在"归一化只动排版、不动内容"上。
    /// 首跑写过一版把空格插进**每一枚字符之间**的尺子，于是两枚界符一律找不到、那一格判红——
    /// 红是红了，红的原因却是尺子而不是窗。这一格把两边各钉一枚形状。
    #[test]
    fn one_space_folds_layout_but_not_letters() {
        assert_eq!(one_space("  a\n\tb   c  "), "a b c");
        assert_eq!(one_space("\n\n"), "");
        // 归一化不认识字符串：它把**连续**空白压成一枚，所以一枚空格（字符串里那种）留下来，
        // 两枚以上（排版那种）压掉。竖线那格钉的是"串本身不动"。
        assert_eq!(one_space("split(\"|\")"), "split(\"|\")");
        assert_eq!(one_space("a  \"b c\"  d"), "a \"b c\" d");
    }

    /// 芯片接线那一格在**默认 `cargo test`** 里也读得到。理由与 `--hint` 那几格同一句：运行期
    /// 证人只在门跑起来时在场，而 `--sendto` 是 opt-in 的一档（它会真派发、闪一扇控制台窗）。
    /// 这一格不开窗、不落盘，读的是编进二进制的那份 QML。
    #[test]
    fn the_toolbar_chip_press_hands_over_to_the_same_hand_the_door_presses() {
        let (check, line) = chip_wiring();
        assert_eq!(check, Check::Pass, "{line}");
    }
}
