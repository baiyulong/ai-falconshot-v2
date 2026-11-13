//! §7.4 第 3 条「选中、禁用、警告等状态不能只依赖颜色表达」的证人（#261）。
//!
//! ## 为什么这一格自己住一档，而不是并进它量到的那几档
//!
//! 这一条判的不是某一扇窗，而是**全部十二枚 QML 面**：谁声明了「选中」、谁声明了「禁用」，
//! 谁就得在自己那一枚组件里挂一条不是颜色的通道。而这三枚状态今天散在四个档主手里
//! （`annotate_check.rs` 管 AnnotationToolbar 与 CaptureMask、`settings_view.rs` 管
//! SettingsWindow、`pin_check.rs` 管 PinWindow、`history_window.rs` 管 HistoryWindow），
//! 另有两枚面 **`CodesPanel.qml` 与 `OcrPanel.qml` 到今天为止没有任何一档 Rust 证人读过**
//! （它们只在 `build.rs:28-29` 被登记进二进制）。一句话跨越四个档主加两枚无主面，按 #281
//! 就得有个自己的家 —— 并进任何一档，那一档都只会替自己那枚面作证。
//!
//! ## 判据为什么钉在「组件」这一层，而不是「档」这一层
//!
//! `AnnotationToolbar.qml` 一枚档里就有两枚声明 `active` 的组件（`Cell` 与 `Swatch`）与一枚
//! 声明 `usable` 的组件（`Cell`）。按整档数行数，`Swatch` 丢了自己的下划条、而 `Cell` 那里
//! 多长出一行同名的东西，总数照样对得上 —— 那一格就绿着，而用户手里那枚色板已经只剩颜色可
//! 看了。所以尺子一律先按大括号把每一枚 `component` 切出来，再在**它自己那一段**里找通道，
//! 并且通道必须挂在**它自己那枚 `id`** 上（`visible: cell.active` 替不了 `swatch.active`）。
//!
//! ## 三条通道各自的形状，与「不许拿颜色充数」
//!
//! * **选中** = 格内贴底一条下划条（一枚 `Rectangle`，`visible: <id>.active`），或既有的
//!   边框粗细（`border.width: <id>.active ? 2 : 0`）。下划条不参与 `implicitWidth`，所以工具
//!   栏不会因为选中谁而抖一下 —— 这是它被挑中而不是拿 `border` 的理由。
//! * **禁用** = 标签加删除线（`font.strikeout: !<id>.usable`）。删除线不改宽度，同样不抖。
//!   **本档是那一句「禁用 = 删除线」的唯一作者**：用户当轮定的是「由证人测试当唯一作者」，
//!   也就是**不**把它提成一枚共享的 QML 组件或单例 —— 三处各写各的一行，删掉任何一处，
//!   下面 `every_selected_or_disabled_state_has_a_channel_that_is_not_a_colour` 当场变红。
//! * **警告** = 一句人话，而那句话住在 Rust（见 `the_refusal_sentence_has_one_author_and_it_is_rust`）。
//!
//! 页上那三行 `color: cell.active ? "#2d7ff7" : …` 是**第一条**通道、不是第二条：颜色正是
//! §7.4 第 3 条点名不许独挑大梁的那一件。所以本档的针只认 `visible:` 与 `border.width:`
//! 两族，一枚 `color:` 也不数。
//!
//! ## 恒真的通道不是通道
//!
//! 这一格量的不是「有没有一行写着这个状态」，而是「那一行能不能把这个状态与它的邻居分开」。
//! 判据因此一律写成 `<id>.active` / `!<id>.usable` 这种**会随状态翻面**的形状，而不是
//! `visible: true` 或 `font.strikeout: false` 这种谁来都一样的写法 —— 后者在屏幕上永远一个
//! 样子，写上去也等于没写。

/// 十二枚 QML 面。这一份名单不是抄来的，是与 `build.rs` 对过账的（见
/// `the_roster_is_the_one_build_rs_puts_in_the_binary`）：`build.rs:15` 那句「A document that
/// is not listed here is not in the binary」说的就是**它**才是权威名单，所以这里长出一枚新面
/// 而忘了登记，那一格会红；`build.rs` 长出一枚新面而这里忘了抄，那一格也会红。
#[cfg(test)]
const FACES: [(&str, &str); 12] = [
    (
        "AnnotationToolbar.qml",
        include_str!("../qml/AnnotationToolbar.qml"),
    ),
    ("CaptureMask.qml", include_str!("../qml/CaptureMask.qml")),
    ("CodesPanel.qml", include_str!("../qml/CodesPanel.qml")),
    (
        "CustomCaptureWindow.qml",
        include_str!("../qml/CustomCaptureWindow.qml"),
    ),
    ("GroupWindow.qml", include_str!("../qml/GroupWindow.qml")),
    ("HintWindow.qml", include_str!("../qml/HintWindow.qml")),
    ("HistoryWindow.qml", include_str!("../qml/HistoryWindow.qml")),
    ("OcrPanel.qml", include_str!("../qml/OcrPanel.qml")),
    ("PinWindow.qml", include_str!("../qml/PinWindow.qml")),
    ("SettingsWindow.qml", include_str!("../qml/SettingsWindow.qml")),
    ("UiFont.qml", include_str!("../qml/UiFont.qml")),
    ("main.qml", include_str!("../qml/main.qml")),
];

/// 权威名单那一份。抄进来是为了**对账**，不是为了当第二份名单（#281：`build.rs` 是名单的
/// 主人，这一枚只是它的读者）。
#[cfg(test)]
const BUILD_RS: &str = include_str!("../build.rs");

/// 警告那一条通道的桥侧本体。抄进来同样是当尺子用，不当作者用 —— 句子的主人在
/// `crate::mask` 那三支 `return Some(…)`。
#[cfg(test)]
const MASK_VIEW_SRC: &str = include_str!("mask_view.rs");

/// 把一份剥过注释的 QML 切成 (组件名, 组件体) 若干枚。
///
/// 认档的形状是「trim 之后以 `component ` 开头、以 `{` 结尾」，收尾按大括号数出来 —— 与
/// `annotate_check.rs` 里切 `field` 那一档同一把刀法。切完从收尾的下一行接着找，所以**嵌套在
/// 另一枚组件里的组件会被跳过**；那件事由调用方那一枚对账盯着（`component ` 开头的行数必须
/// 等于切出来的组件数），不许它悄悄漏读。
#[cfg(test)]
fn components(src: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_component_decl(lines[i]) {
            i += 1;
            continue;
        }
        let name = lines[i].trim()["component ".len()..]
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let mut depth = 0usize;
        let mut end = None;
        for (k, l) in lines.iter().enumerate().skip(i) {
            depth += l.matches('{').count();
            depth = depth.saturating_sub(l.matches('}').count());
            if depth == 0 {
                end = Some(k);
                break;
            }
        }
        let end = end.unwrap_or_else(|| {
            panic!("组件 {name} 的大括号没有合上：这把尺子切不出它读的是哪一段")
        });
        out.push((name, lines[i..=end].join("\n")));
        i = end + 1;
    }
    out
}

#[cfg(test)]
fn is_component_decl(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("component ") && t.ends_with('{')
}

/// 那一枚组件自己的 `id`：体内第一行 trim 之后以 `id: ` 开头的那一行。
///
/// 读不出 `id` 不是「这一枚没有名字，跳过」—— 是这把尺子说不出下面那些针该挂在谁身上，
/// 于是调用方要当场红（#274：尺子必须能读出"我没有量到"，不许把量不到当成量到了没事）。
#[cfg(test)]
fn own_id(body: &str) -> Option<String> {
    body.lines().find_map(|l| {
        let t = l.trim();
        if let Some(v) = t.strip_prefix("id: ") {
            let v = v.trim();
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        } else {
            None
        }
    })
}

/// 有没有一行 trim 之后以 `head` 开头。用在 `property bool active:` 这种**值可以随便写**的
/// 针上（`false` 是今天的初值，不是契约）。
#[cfg(test)]
fn any_line_starts(body: &str, head: &str) -> bool {
    body.lines().any(|l| l.trim().starts_with(head))
}

/// 有没有一行 trim 之后**正好等于** `needle`（#461：整行，不是前缀 —— 前缀针会把
/// `visible: cell.activeFoo` 也读成一条通道，而那一行谁也没选中过）。
#[cfg(test)]
fn any_line_is(body: &str, needle: &str) -> bool {
    body.lines().any(|l| l.trim() == needle)
}

/// 有没有一行既以 `head` 开头、又含着 `needle`。
///
/// 只给「边框粗细」那一族用：`border.width: swatch.active ? 2 : 0` 里的 `2` 与 `0` 是画得多
/// 粗的审美，不是判据，所以整行钉不死；可只钉 `border.width:` 又会把 `border.width: 1` 这种
/// 谁来都一样的行读成通道（恒真的通道不是通道）。于是两半一起要：开头认它是一枚边框粗细，
/// 含 `needle` 认它真的跟着这个状态翻面。
#[cfg(test)]
fn any_line_starts_and_has(body: &str, head: &str, needle: &str) -> bool {
    body.lines().any(|l| {
        let t = l.trim();
        t.starts_with(head) && t.contains(needle)
    })
}

/// 全场的读数。**只数、不判** —— 判在 `require_channels` 里。
///
/// 分成两枚是因为下面那一格自证伪（#269）要故意把一行删掉再看读数少不少：如果数与判是同一
/// 枚函数，删掉一行只会让它 panic，那一格就永远问不出「读数是不是真的跟着页上走的」。
#[cfg(test)]
struct Tally {
    /// 切出来的组件总数。
    components: usize,
    /// 声明了 `active` 的组件数。
    active: usize,
    /// 声明了 `usable` 的组件数。
    usable: usize,
    /// 挂着删除线的组件数。
    strikeout: usize,
    /// 挂着下划条的组件数。
    underline: usize,
    /// 挂着边框粗细的组件数。
    edge: usize,
}

#[cfg(test)]
fn tally(faces: &[(&str, &str)]) -> Tally {
    let mut t = Tally {
        components: 0,
        active: 0,
        usable: 0,
        strikeout: 0,
        underline: 0,
        edge: 0,
    };
    for (_, raw) in faces {
        let code = crate::hint_check::qml_code(raw);
        for (_, body) in components(&code) {
            t.components += 1;
            let id = match own_id(&body) {
                Some(id) => id,
                None => continue,
            };
            if any_line_starts(&body, "property bool active:") {
                t.active += 1;
                if any_line_is(&body, &format!("visible: {id}.active")) {
                    t.underline += 1;
                }
                if any_line_starts_and_has(&body, "border.width:", &format!("{id}.active")) {
                    t.edge += 1;
                }
            }
            if any_line_starts(&body, "property bool usable:") {
                t.usable += 1;
                if any_line_is(&body, &format!("font.strikeout: !{id}.usable")) {
                    t.strikeout += 1;
                }
            }
        }
    }
    t
}

/// 逐枚组件判它有没有第二条通道，并把「尺子自己有没有读到东西」一起判掉。
#[cfg(test)]
fn require_channels(faces: &[(&str, &str)]) {
    for (name, raw) in faces {
        let code = crate::hint_check::qml_code(raw);
        // #274 的前一半：这一把尺子在这一枚档上真的动过手。十二枚 QML 今天每一枚都带 `//`
        // 注释（最少的一枚 UiFont.qml 也有 25 行），而 `qml_code` 是把注释字符**删掉**、把换行
        // 原样留下的，所以「行数一样、字符更少」两枚一起才说得死"剥过了"。哪天某一枚档的注释
        // 被清空，这里会红 —— 那时该改的是这一枚断言，不是把尺子摘掉。
        assert_eq!(
            code.lines().count(),
            raw.lines().count(),
            "{name}: 剥注释的那把尺子动了行数 —— 它保的是换行，行数必须一模一样"
        );
        assert!(
            code.len() < raw.len(),
            "{name}: 剥过一个字符也没少，这一枚档根本没被尺子读到，下面每一枚针都是空集里的判决"
        );

        // 对账：切出来的组件数必须等于档里 `component ` 开头的行数。`components` 是从收尾的
        // 下一行接着找的，所以一枚**嵌套**在别的组件里的组件会被它跳过 —— 这一行就是那道口子
        // 的报警器，不许它悄悄漏读一枚（漏读的那一枚正好是没有通道的那一枚，这一格就白绿了）。
        let decls = code.lines().filter(|l| is_component_decl(*l)).count();
        let carved = components(&code);
        assert_eq!(
            carved.len(),
            decls,
            "{name}: 档里有 {decls} 行 `component` 声明，尺子只切出 {} 枚 —— 有一枚被跳过了，\
             而跳过的那一枚正好逃掉了下面每一条判据",
            carved.len()
        );

        for (comp, body) in &carved {
            let wants_active = any_line_starts(body, "property bool active:");
            let wants_usable = any_line_starts(body, "property bool usable:");
            if !wants_active && !wants_usable {
                continue;
            }
            let id = own_id(body).unwrap_or_else(|| {
                panic!(
                    "{name} 的组件 {comp} 声明了状态，却读不出它自己的 `id`：\
                     下面那些针不知道该挂在谁身上，这一格量的是空集"
                )
            });
            if wants_active {
                let underline = any_line_is(body, &format!("visible: {id}.active"));
                let edge =
                    any_line_starts_and_has(body, "border.width:", &format!("{id}.active"));
                assert!(
                    underline || edge,
                    "{name} 的组件 {comp}（id: {id}）声明了 `active`，却没有一条不是颜色的通道。\n\
                     页上那一行 `color: {id}.active ? …` 不算 —— §7.4 第 3 条点名的就是「不能只\
                     依赖颜色表达」。\n\
                     要补的是二选一：格内贴底一枚 `Rectangle {{ visible: {id}.active }}`（不参与\
                     implicitWidth，工具栏不会抖），或者把边框写成 `border.width: {id}.active ? 2 : 0`。"
                );
            }
            if wants_usable {
                assert!(
                    any_line_is(body, &format!("font.strikeout: !{id}.usable")),
                    "{name} 的组件 {comp}（id: {id}）声明了 `usable`，却没有一条不是颜色的通道。\n\
                     要补的是给标签加一行 `font.strikeout: !{id}.usable`（删除线不改宽度，格子不会抖）。\n\
                     这一句「禁用 = 删除线」的唯一作者就是本档：不要把它提成共享组件或单例，\
                     也不要写成 `font.strikeout: false` 这种谁来都一样的恒真行。"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 名单对账：`build.rs` 登记进二进制的那十二枚，与本档抄进来的那十二枚，必须是同一批。
    ///
    /// 两边都对，是因为只钉数量是不够的 —— 数量一样而名字换了一枚，本档就会去量一枚不在二进制
    /// 里的档、同时放走一枚真在里面的（#274 的另一种长相：尺子读到了东西，只是读的不是那件东西）。
    #[test]
    fn the_roster_is_the_one_build_rs_puts_in_the_binary() {
        const OPEN: &str = ".qml_file(\"qml/";
        let mut in_binary: Vec<String> = BUILD_RS
            .lines()
            .filter_map(|l| {
                let at = l.find(OPEN)?;
                let rest = &l[at + OPEN.len()..];
                let end = rest.find('"')?;
                Some(rest[..end].to_string())
            })
            .collect();
        in_binary.sort();
        let mut here: Vec<String> = FACES.iter().map(|(n, _)| n.to_string()).collect();
        here.sort();

        assert_eq!(
            in_binary.len(),
            12,
            "`build.rs` 里 `.qml_file(\"qml/…\")` 数到 {} 枚，不是 12：要么这份名单烂了，\
             要么产品长出了第十三枚面 —— 两种都得先看清楚再改这一格",
            in_binary.len()
        );
        assert_eq!(
            here, in_binary,
            "本档的名单与 `build.rs` 登记进二进制的那一份不是同一批：\
             多出来的一枚量的是不进二进制的档，少掉的一枚正好逃掉了 §7.4 第 3 条的全部判据"
        );
    }

    /// §7.4 第 3 条本体：每一枚声明了「选中」或「禁用」的组件，都在**它自己那一段**里挂着
    /// 一条不是颜色的通道。
    #[test]
    fn every_selected_or_disabled_state_has_a_channel_that_is_not_a_colour() {
        require_channels(&FACES);
        let t = tally(&FACES);
        println!(
            "scan: 组件 {} 枚，其中声明 active {} 枚、usable {} 枚；\
             通道 = 下划条 {} 条、边框粗细 {} 条、删除线 {} 条",
            t.components, t.active, t.usable, t.underline, t.edge, t.strikeout
        );

        // 十枚组件是今天实测的全场：AnnotationToolbar 四枚（Cell / Caption / Swatch / ToolIcon）、
        // CaptureMask 两枚（RatioCell / TopField）、CodesPanel 一枚（Chip）、CustomCaptureWindow 一枚
        // （FieldRow）、OcrPanel 一枚（Chip）、PinWindow 一枚（Grip）。钉住它是为了让人数对不上
        // 时先停下来看一眼：新长一枚组件是常事，而**少**一枚通常意味着上面那把尺子切歪了。
        //
        // 9 → 10 是 2026-11-13 那一轮加的 `TopField`（顶部那几格输入框的样子：文字上下垂直居中、
        // 配色与旁边的芯片同一套）。它**不声明** active/usable（那两枚状态在 `Cell` 那一格里），
        // 所以下面 active 与 usable 两个读数一枚没动。
        assert_eq!(t.components, 10, "全场组件数到 {} 枚，不是 10", t.components);
        // 声明「选中」的三枚：AnnotationToolbar 的 Cell 与 Swatch、CaptureMask 的 RatioCell。
        assert_eq!(t.active, 3, "声明 active 的组件数到 {} 枚，不是 3", t.active);
        // 声明「禁用」的三枚：AnnotationToolbar 的 Cell、CodesPanel 的 Chip、OcrPanel 的 Chip。
        // 这三枚就是用户当轮定的「三处全改」，一处一枚，各在自己那一档里。
        assert_eq!(t.usable, 3, "声明 usable 的组件数到 {} 枚，不是 3", t.usable);
        // 删除线三枚：与 usable 一一对应，也就是「三处全改」那一轮落地的三行。
        assert_eq!(
            t.strikeout, 3,
            "删除线数到 {} 条，不是 3 —— 少一条是某一处被删了，多一条是有人把它抄到了\
             没声明 usable 的地方",
            t.strikeout
        );
        // 下划条两条（AnnotationToolbar 的 Cell、CaptureMask 的 RatioCell）+ 边框粗细一条
        // （AnnotationToolbar 的 Swatch，那是本轮之前就在的既有通道）= 三枚 active 各有一条。
        assert_eq!(
            t.underline, 2,
            "下划条数到 {} 条，不是 2",
            t.underline
        );
        assert_eq!(
            t.edge, 1,
            "边框粗细那条通道数到 {} 条，不是 1（它只该是 Swatch 那一枚既有的）",
            t.edge
        );
        assert_eq!(
            t.underline + t.edge,
            t.active,
            "三枚 active 与它们的通道对不上账：一条通道也不许服务两枚组件，\
             也不许一枚组件挂两条（挂两条说明有一枚是恒真的凑数行）"
        );
    }

    /// 自证伪（#269）：把一条通道删掉，读数必须正好少一枚。
    ///
    /// 上面那一格钉的是绝对数，而绝对数最怕的是「尺子根本没在读页上的字」—— 一枚写死的常量
    /// 也能钉出 3/3/3。删一行看它掉一格，才证明那三个数是从十二枚档里量出来的。
    #[test]
    fn the_channel_counts_fall_by_exactly_one_when_a_line_is_dropped() {
        let whole = tally(&FACES);

        /// 把某一枚档里 trim 之后正好等于 `line` 的**头一行**删掉，其余原样。
        fn without_one_line(file: &str, line: &str) -> Vec<(String, String)> {
            let mut hit = false;
            FACES
                .iter()
                .map(|(name, raw)| {
                    if *name != file {
                        return (name.to_string(), raw.to_string());
                    }
                    let kept: Vec<&str> = raw
                        .lines()
                        .filter(|l| {
                            let drop = !hit && l.trim() == line;
                            if drop {
                                hit = true;
                            }
                            !drop
                        })
                        .collect();
                    (name.to_string(), kept.join("\n"))
                })
                .collect()
        }
        fn tally_of(owned: &[(String, String)]) -> Tally {
            let borrowed: Vec<(&str, &str)> = owned
                .iter()
                .map(|(n, r)| (n.as_str(), r.as_str()))
                .collect();
            tally(&borrowed)
        }

        // 三枚各删一行：下划条、删除线、边框粗细。删完那一枚组件就只剩颜色可看了，
        // 于是 require_channels 也必须当场红 —— 两枚一起问，才问得完「这一格是不是活的」。
        for (file, line, dropped) in [
            (
                "AnnotationToolbar.qml",
                "visible: cell.active",
                "下划条（工具栏那一枚 Cell）",
            ),
            (
                "AnnotationToolbar.qml",
                "font.strikeout: !cell.usable",
                "删除线（工具栏那一枚 Cell）",
            ),
            (
                "AnnotationToolbar.qml",
                "border.width: swatch.active ? 2 : 0",
                "边框粗细（色板那一枚 Swatch）",
            ),
            (
                "CaptureMask.qml",
                "visible: cell.active",
                "下划条（比例那一枚 RatioCell）",
            ),
            (
                "CodesPanel.qml",
                "font.strikeout: !chip.usable",
                "删除线（条码那一枚 Chip）",
            ),
            (
                "OcrPanel.qml",
                "font.strikeout: !chip.usable",
                "删除线（OCR 那一枚 Chip）",
            ),
        ] {
            let owned = without_one_line(file, line);
            let after = tally_of(&owned);
            let before = if line.starts_with("font.strikeout") {
                whole.strikeout
            } else if line.starts_with("border.width") {
                whole.edge
            } else {
                whole.underline
            };
            let now = if line.starts_with("font.strikeout") {
                after.strikeout
            } else if line.starts_with("border.width") {
                after.edge
            } else {
                after.underline
            };
            assert_eq!(
                now,
                before - 1,
                "删掉 {file} 里那一行 `{line}`（{dropped}）之后，读数从 {before} 变成 {now}，\
                 不是正好少一枚：这把尺子数的不是页上那一行"
            );
            // 其余几族不许跟着动 —— 删一枚组件的通道却掉出别人家的读数，说明这几枚针其实
            // 在量同一行（那样自证伪就白做了：它证的是"有行数变了"，不是"这一行被读到了"）。
            assert_eq!(
                after.components, whole.components,
                "删掉一行通道不该动组件总数：尺子把通道行当成了一枚组件的开头"
            );
            assert_eq!(
                after.active, whole.active,
                "删掉一行通道不该动 `active` 的声明数 —— 声明与通道是两行，混了就分不出\
                 「没声明这个状态」与「声明了却没通道」"
            );
            assert_eq!(
                after.usable, whole.usable,
                "删掉一行通道不该动 `usable` 的声明数：同上"
            );
        }
    }

    /// 警告那一条通道：句子只有一个作者，而那个作者在 Rust。
    ///
    /// §7.4 第 3 条对「警告」的要求，落到 §5.3.6 尺寸那两格与 §5.3.7 比例那一格上，是同一件
    /// 事：被打回的那一刻，屏幕上得有一句人话说**为什么**，而不只是文本框点灰或者字变红 ——
    /// 「点灰」分不出「还没按回车」与「按了、被拒了」，「变红」更是纯颜色。
    ///
    /// 这一格判三件事：句子不在 QML 里（#281）、句子确实被搬过桥并渲染出来、以及桥那两枚
    /// 签名的返回类型还是 `QString`。第三件最容易被当成多余：如果哪天有人把 `applySize` 改回
    /// `-> bool`，页上 `refusal.length > 0` 不是编译错、也不是本档的红 —— 它是**运行时**一枚
    /// `TypeError`，而那枚句子从此再也不出现。所以签名的形状要在这里钉死。
    #[test]
    fn the_refusal_sentence_has_one_author_and_it_is_rust() {
        let sentences = [
            crate::mask::SIZE_REFUSAL_NO_SELECTION,
            crate::mask::SIZE_REFUSAL_NOT_POSITIVE,
            crate::mask::RATIO_REFUSAL,
        ];

        // #274 的这一半：那把「句子不在页上」的尺子必须能读出"在"来，否则三枚 `!contains`
        // 是零枚读者的判决。证法是同一句话两种住处 —— 写进代码里剥过还在（会被抓住），
        // 写进注释里剥过就没了（不算第二个作者，那是散文在提它）。
        // 页上今天正是后一种：`CaptureMask.qml` 的两行注释里各引了半句，所以本档一律**先剥
        // 再数** —— 直接拿原文数，那三枚断言会红在两句散文上。
        let in_code = format!("Text {{ text: \"{}\" }}\n", crate::mask::RATIO_REFUSAL);
        assert!(
            crate::hint_check::qml_code(&in_code).contains(crate::mask::RATIO_REFUSAL),
            "尺子失灵：一句话写在**代码**里，剥过注释之后居然读不出来 —— \
             那下面每一枚「页上没有这句话」都是空集里的判决"
        );
        let in_comment = format!("// {}\nText {{}}\n", crate::mask::RATIO_REFUSAL);
        assert!(
            !crate::hint_check::qml_code(&in_comment).contains(crate::mask::RATIO_REFUSAL),
            "尺子过头：一句话写在**注释**里也被算成第二个作者 —— 那本档会把散文判成代码"
        );

        // 句子不许在页上被抄第二遍。针取的是每一句 `——` 之前那半句（而不是整句），因为整句
        // 只能抓住逐字照抄，抓不住"改了后半句、前半句照抄"这一族；而前半句正是那三句各自
        // 认得出来的部分。两枚 size 句子共用同一句前半（`没换掉尺寸`），所以这里数的是三枚
        // const、而不同前半只有两枚 —— 共用是 `mask.rs` 那边定的，本档只当读者。
        for (name, raw) in FACES.iter() {
            let code = crate::hint_check::qml_code(raw);
            for s in sentences {
                let head = s.split("——").next().unwrap_or(s).trim();
                assert!(
                    !head.is_empty(),
                    "从 {s:?} 里切不出前半句：这一枚针今天量的是空集"
                );
                assert!(
                    !code.contains(head),
                    "{name} 里写着 {head:?} —— §7.4 那句警告的话有第二个作者了（#281）。\n\
                     句子的主人是 `crate::mask` 那三支 `return Some(…)`，页上只准存下桥交回来的\
                     那一句（`sizeBar.refusal = said`）并按它空不空决定显不显。\n\
                     要提它，请写在 `//` 注释里 —— 那一半会被剥掉，不算第二个作者。"
                );
                assert!(
                    !code.contains(s),
                    "{name} 里逐字抄了整句 {s:?}（#281）"
                );
            }
        }

        // 桥那两枚签名的返回类型。整行钉死（#461）：改回 `bool` 的那一次，这两枚针会同时读 0。
        const RATIO_DECL: &str =
            "fn apply_ratio_spec(self: Pin<&mut Self>, spec: &QString) -> QString;";
        const SIZE_DECL: &str = "fn apply_size(self: Pin<&mut Self>, w: i32, h: i32) -> QString;";
        assert_eq!(
            MASK_VIEW_SRC.matches(RATIO_DECL).count(),
            1,
            "桥上 `applyRatioSpec` 的签名不是 `{RATIO_DECL}` 了：页上 `refusal.length > 0` \
             要的是字符串，改回 bool 会在**运行时**炸，不在任何一格里红"
        );
        assert_eq!(
            MASK_VIEW_SRC.matches(SIZE_DECL).count(),
            1,
            "桥上 `applySize` 的签名不是 `{SIZE_DECL}` 了：同上"
        );

        // 实现那两枚：句子是从状态那边搬过来的，不是桥自己编的。
        let ratio_impl = fn_body(MASK_VIEW_SRC, "pub fn apply_ratio_spec(");
        assert!(
            ratio_impl.contains("mask::RATIO_REFUSAL"),
            "`apply_ratio_spec` 的实现里没有 `mask::RATIO_REFUSAL`：那句话又长出一个作者了"
        );
        let size_impl = fn_body(MASK_VIEW_SRC, "pub fn apply_size(");
        assert!(
            size_impl.contains("size_refusal("),
            "`apply_size` 的实现里没有调 `size_refusal(`：桥在自己挑句子，而不是让状态挑"
        );

        // 页上那两格：存下来、并且渲染出来。四枚针各数一次，少一枚都说明句子到了半路。
        const PAGE: &str = include_str!("../qml/CaptureMask.qml");
        let code = crate::hint_check::qml_code(PAGE);
        // 比例那一枚针连 `onEditingFinished:` 一起钉，是因为页上那一行的**整行**就是它 ——
        // 只钉后半截（`ratioBar.refusal = win.view.applyRatioSpec(text)`）在本档这把「整行相等」
        // 的尺子下面会数出 0 枚，于是这一格会红在一句谎话上（#461：针要么是整行、要么就别用
        // 整行的尺子）。连 handler 名一起钉还白得一枚判据：那一句话必须是**编辑结束**时递过去的。
        for needle in [
            "sizeBar.refusal = said",
            "onEditingFinished: ratioBar.refusal = win.view.applyRatioSpec(text)",
            "visible: sizeBar.refusal.length > 0",
            "text: sizeBar.refusal",
            "visible: ratioBar.refusal.length > 0",
            "text: ratioBar.refusal",
        ] {
            assert_eq!(
                code.lines().filter(|l| l.trim() == needle).count(),
                1,
                "`CaptureMask.qml` 里整行等于 `{needle}` 的行不是正好一行 —— \
                 零行是那一句警告到不了屏幕，多行是这一格又有了第二个作者"
            );
        }
        // 「点灰」与「变红」都还在，但它们不再是唯一的通道：那一行红字才是。所以这里不许把
        // 颜色那两行当成警告态的证据来数（它们本来就在，也本来就不够）。
        println!(
            "scan: 警告那一句 = Rust 三支 return；页上两格各存一枚、各渲染一行；\
             桥的两枚签名都是 QString"
        );
    }

    /// 从 `head` 那一处起到它自己那枚收尾大括号为止。
    ///
    /// 与 `mask_view.rs` 自己 `mod tests` 里那把 `fn_body` 同一条刀法：切在第一个 `"\n    }"`
    /// 上，因为桥那一档的 `impl` 方法就住在四空格那一层。找不到 `head` 或找不到收尾都**panic**
    /// 而不是交回空串 —— 交回空串会让上面每一枚 `contains` 都红在同一句谎话上（#274）。
    fn fn_body(src: &str, head: &str) -> String {
        let at = src
            .find(head)
            .unwrap_or_else(|| panic!("找不到 {head}：这一枚针今天量的是空集"));
        let rest = &src[at..];
        let end = rest
            .find("\n    }")
            .unwrap_or_else(|| panic!("{head} 没有收尾大括号：这把尺子切不出它读的是哪一段"));
        rest[..end].to_string()
    }
}
