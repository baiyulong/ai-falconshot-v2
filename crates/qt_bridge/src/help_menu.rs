//! §5.21.2 / §5.18.1 的那一扇门：托盘第 1440 行那枚「帮助」有没有归宿，以及 PRD 那一节点名的
//! 四行是不是**真的各有作者**（而不是这一句「帮助」自己把四行重新发明一遍）。
//!
//! 这一档不开窗、也不等任何人：它读的全是「一份名册 → 一个决定」——真装配出来的那份托盘菜单、
//! [`crate::hotkey::Work`] 那张路由表本身、[`crate::help::Counts`] 从四位作者那里现取的四枚数，
//! 以及需求文档 §5.18.1「**菜单建议**」那一列的原文（编译期 `include_str!` 进来，见 [`DOC`]）。
//! 锁屏会话也判得出，也不需要有人的手放在鼠标上。
//!
//! ## 这一档不声称的那三件事
//!
//! 一，它一次也没真往那枚菜单项投过一次点击（`WM_TRAY` 那一发要在跑的泵上）。所以「真人点中它
//! 走的是不是 [`crate::hotkey::Work::Help`] 这一臂」这一格发 **Blocked**，不是删掉的一格，也不
//! 是拿上面那几格的读数冒充——这一档因此交回 BLOCKED（退出码 3），不是 PASS。
//!
//! 二，它不证明那四行「在屏幕上好看」。第一行的钮↔提示普查住在
//! [`crate::annotate_check::tests::every_toolbar_button_is_handed_a_hint`] 那里，第二行的说明
//! 上屏由 `settings_view` 与 `qml/SettingsWindow.qml` 量过；这一档只问「那位作者还在不在、
//! 托盘那枚有没有把人送到作者那里去」。
//!
//! 三，它不声称文档那一列**每一行**都对上了。文档写「贴图」与「显示或隐藏全部贴图」，这份装配
//! 交回的名字是「贴出剪贴板」与「隐藏/显示全部贴图」——按名相等判不到它们，所以那一格在文档
//! 那一列没有全部对上时交 **Blocked** 而不是 Pass。这份欠账不是本轮新造的，它记在
//! [`crate::hotkey::tests::every_work_is_named_by_the_one_table`] 那里；本轮只是把它量出来了。

use crate::help::{self, TRAY_WORD};
use crate::hotkey::{tray_labels, tray_routes, Work};
use crate::r13::Report;
use crate::state::Check;

/// 全部功能需求文档，编译期取进来的一份。这一格要判「菜单的次序由文档那一列决定」，那么那一列
/// 就不能住在这一档的记忆里：抄一份进 `help_menu.rs`，文档改了而这份不改，判的就是一份已经不
/// 存在的次序。`include_str!` 让文档不在盘上时这一档根本编不出来，rustc 也会把它记进依赖里。
const DOC: &str = include_str!("../../../AI Falconshot 全部功能需求文档.md");

/// §5.18.1「**菜单建议**」那一列的十行原文，从上面那一份文档里取（1433–1442 行）。
///
/// 取法是「`### 5.18.1` 到 `### 5.18.2` 之间、以 `- ` 开头的那些行」，不是按行号切片：行号会
/// 因为文档别处加了一行而全体错位，而节名不会。
fn doc_menu_rows() -> Vec<String> {
    let Some(start) = DOC.find("### 5.18.1 托盘菜单") else {
        return Vec::new();
    };
    let body = &DOC[start..];
    let end = body.find("### 5.18.2").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|line| line.trim().to_string())
        .collect()
}

/// 逐格判定，交回（最坏判决，那份报告）。
pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let labels = tray_labels();
    let routes = tray_routes();
    let at = labels.iter().position(|n| n == TRAY_WORD);
    let counts = help::Counts::now();

    // 第 1440 行那一枚自此在名册里，而且只出现一次。
    rep.row(
        "tray names 帮助",
        Check::from(at.is_some() && labels.iter().filter(|n| *n == TRAY_WORD).count() == 1),
        format!(
            "菜单 {} 项，{TRAY_WORD} 在第 {:?} 位（0 起）",
            labels.len(),
            at
        ),
    );

    // 次序由文档那一列决定 —— 而那一列是从**文档自己身上**取下来的（`DOC` 是 `include_str!`），
    // 不是这一轮抄的一份名单（#281 的另一面：把判据的字句抄第二遍，抄的那一遍就会先烂）。
    // P174 之前这一格只 spot-check 了「帮助」上下两行，而它正是靠那一格当场判出 FAIL：装配把
    // P142 的「首选项」排在 P171 的「历史记录」之前，文档那一列（1438 行、1439 行）是反的。
    let rows = doc_menu_rows();
    let pos = |w: &str| labels.iter().position(|n| n == w);
    let mut matched: Vec<(&str, usize)> = Vec::new();
    let mut worded_elsewhere: Vec<&str> = Vec::new();
    for doc in &rows {
        match pos(doc) {
            Some(i) => matched.push((doc.as_str(), i)),
            None => worded_elsewhere.push(doc.as_str()),
        }
    }
    let inverted: Vec<String> = matched
        .windows(2)
        .filter(|pair| pair[0].1 > pair[1].1)
        .map(|pair| format!("{}({})→{}({})", pair[0].0, pair[0].1, pair[1].0, pair[1].1))
        .collect();
    // 文档那一列里没被这份装配叫作同一枚名的行，不是「缺行」——「贴图」在这份菜单上叫
    // 「贴出剪贴板」、「显示或隐藏全部贴图」叫「隐藏/显示全部贴图」，两枚都有手有位置。这一格
    // 因此不许为它们交 Pass：按名相等判不到它们，就照实记 Blocked，把话说完再降级。
    let ordered = !matched.is_empty() && inverted.is_empty();
    let graded = if !ordered {
        Check::Fail
    } else if worded_elsewhere.is_empty() {
        Check::Pass
    } else {
        Check::Blocked
    };
    rep.row(
        "order follows the doc",
        graded,
        format!(
            "文档那一列 {} 行，菜单 {} 项；按名对上的 {} 行 {}，反序 {}；名字与文档不同的 {} 行 {}",
            rows.len(),
            labels.len(),
            matched.len(),
            matched
                .iter()
                .map(|(n, i)| format!("{n}@{i}"))
                .collect::<Vec<_>>()
                .join(" "),
            if inverted.is_empty() {
                "无".to_string()
            } else {
                inverted.join("、")
            },
            worded_elsewhere.len(),
            worded_elsewhere.join("、")
        ),
    );
    // 「帮助」必须夹在第 1439 行「首选项」与第 1441 行「检查更新」之间，而不是排到末尾凑数。
    // 上面那一格判的是整列的相对次序，这一格判的是它自己那一行的左右邻居。
    rep.row(
        "the row sits where the doc puts it",
        Check::from(
            at.is_some()
                && pos("检查更新") == at.map(|i| i + 1)
                && at.is_some_and(|i| i > 0 && labels[i - 1] == "首选项"),
        ),
        format!(
            "首选项={:?} {TRAY_WORD}={:?} 检查更新={:?}",
            pos("首选项"),
            at,
            pos("检查更新")
        ),
    );

    // 它是一枚有归宿的工作，而不是一行死菜单：路由表里要有 `Work::Help` 这一枚 id。
    rep.row(
        "the row has a work",
        Check::from(routes.iter().any(|(_id, w)| *w == Work::Help)),
        format!(
            "routes={:?} 里 Help 的枚数={}",
            routes.len(),
            routes.iter().filter(|(_id, w)| *w == Work::Help).count()
        ),
    );

    // §5.16.1 那张可绑定名册里没有「帮助」这一行：不给一枚窗/一次查询发明第十三名。
    // 这一格同时判两件事——`action()` 交回 None，且 `Work::of("help")` 认不出这个名字。
    rep.row(
        "no invented key name",
        Check::from(Work::Help.action().is_none() && Work::of("help").is_none()),
        format!(
            "action()={:?} Work::of(\"help\")={:?}",
            Work::Help.action(),
            Work::of("help").is_some()
        ),
    );

    // PRD 那一节的四行，此刻每一行都还有一位作者。少一位，这一句就换成「欠」的那一句。
    // 这里印的是四枚数本身，不重抄那四行的名字：名字的作者住在 [`crate::help::Counts::incomplete`]
    // 那里，这一档抄一遍就会有两个作者（#281）。
    rep.row(
        "four rows, four authors",
        Check::from(counts.incomplete().is_empty()),
        format!(
            "四枚数 {:?}，缺={:?}",
            (
                counts.toolbar_tools,
                counts.hotkey_rows,
                counts.cli_params,
                counts.guide_pages
            ),
            counts.incomplete()
        ),
    );

    // #281：那一句是 `Counts::sentence()` 自己写的，本档不抄一遍字——所以这里判的是「同一枚
    // Counts 交回同一句」，而不是「这句话长什么样」。句子里的内容对不对，由 help.rs 那几枚单测判。
    rep.row(
        "one author for the line",
        Check::from(counts.sentence() == help::Counts::now().sentence()),
        counts.sentence(),
    );

    // 少一行时不许什么都不说，也不许把那一行悄悄漏过去、照报「四行齐全」。这里抹零的是**此刻
    // 那一份**计数里第一枚非零的数：把 15/16/6 抄进这一格，作者名单一改，这格判的就是旧名单。
    // 「少了哪一行」也不抄 —— 它从两次 `incomplete()` 相减里出来，那一列名字的作者只有
    // [`crate::help::Counts`] 一位（#281）。
    let mut short = help::Counts::now();
    if short.toolbar_tools != 0 {
        short.toolbar_tools = 0;
    } else if short.hotkey_rows != 0 {
        short.hotkey_rows = 0;
    } else if short.cli_params != 0 {
        short.cli_params = 0;
    } else {
        short.guide_pages = 0;
    }
    let named: Vec<&'static str> = short
        .incomplete()
        .into_iter()
        .filter(|row| !counts.incomplete().contains(row))
        .collect();
    let said = short.sentence();
    rep.row(
        "a short row still says so",
        Check::from(named.len() == 1 && said.contains(named[0]) && said != counts.sentence()),
        format!(
            "抹掉一枚数后 incomplete 多了 {}，那句＝{said}（齐全那句＝{}）",
            named.join("、"),
            counts.sentence()
        ),
    );

    // 那一枚真的被点过吗：这一档没有跑着的泵，投不出那一发 `WM_TRAY`。
    rep.row(
        "the tray row was clicked",
        Check::Blocked,
        "本档一次也没往第 1440 行那枚投过点击；「按下去真的走 Work::Help」仍记在结转里".to_string(),
    );

    // 用户那份档：这一档只读名册与四枚计数，一个字节也不写（§9.1 第十五条），照实印一枚读数。
    rep.note(
        "user config",
        format!(
            "这一档不读也不写 %APPDATA%\\ai-falconshot\\config.toml；行数 {}",
            rep.row_count()
        ),
    );

    rep.finish("help door")
}
