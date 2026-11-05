//! §5.16.1 名册里「自定义命令行任务」那一行中**能被算出来的那一半**。
//!
//! 名单上早就有它一枚位置（`config::ACTION_TEXTS` 的 `custom_task`，说明是「跑一条用户自己
//! 写好的命令行任务」），缺的一直是那只手：把**一行用户写的字**变成 `open::dispatch` 要的
//! 两枚串 —— 程序，和参数块。启动那一只手在本仓库只有一处主人（`platform_windows::open::dispatch`，
//! 一枚 `ShellExecuteW("open", exe, parameters)`），本模块不开第二条 spawn 路，只判「递不递、
//! 递哪两枚串、不递的时候用户听到哪一句」。分工与 §5.5.10 完全同形：判在 core，搬在
//! `platform_windows::open`（那里连存在性都不再判第二遍，见其模块头部），开口的动作在桥。
//!
//! # 为什么切法是一刀，而不是一套 shell 语法
//!
//! `ShellExecuteW` 的 `parameters` 是**原样**接到目标自己的命令行上的，解析它的那套 argv 规则
//! 属于目标程序（多数是 MSVC CRT 那一套），不属于我们，也不属于 cmd。于是两条：
//!
//! - **只在第一处空白切一刀**：左边是程序，右边整段留着当参数块。不重排、不补引号、不折叠
//!   中间的空格、不把切出来的词再拼成一句 —— 我们拼出来的任何一段字都是**我们**写给对面解析器
//!   的，而 §7.5「命令行参数应避免命令注入和路径注入」要防的正是这一手：拿一套 cmd 味道的规则
//!   分词、再把分词结果拼成一条串交回 ShellExecute，等于让同一句话过两套语义不同的解析，坏就
//!   出在两套不一致的那一处（`&`、`^`、引号在 cmd 那头各有含义，在目标的 argv 那头只是字符）。
//! - **参数块里不判元字符**：这一条路上没有任何 shell 可过（没有 `cmd /C`，与 `open.rs` 头部
//!   避开 `start` 同一个理由），把 `&` `|` `^` 列进黑名单只会先打死「传一个带 `&` 的地址」这类
//!   正当参数。真正需要判的位置只有一个：**程序那一段** —— 那一段是我们亲手放进 `object` 参数
//!   的，多一枚引号就能让它从「一枚路径」变成「一段我们没写过的话」。
//!
//! # 程序名上的引号：拒，不救
//!
//! 以引号开头的程序（`"C:\Program Files\X\tool.exe" /p` 是用户真会写出来的形状）看着只差去掉
//! 一对字符，但「哪一对才是包封」正是要猜的那一句：`"a"b" c` 里去掉首尾那两枚与去掉第一对，
//! 交出的是两枚不同的路径；而第一处空白落在引号里的那一刻，连"程序那一段是哪一截"都还没定下来。
//! 猜一次就是替目标重新解析一遍这句话，而 §7.5 给这一族的定位是「应经过校验」，不是「应尽量
//! 救回」。所以开头一枚引号当场拒（不去找它在哪一格合上）、切出来那截里夹着引号也拒，两句各有
//! 各自的话。代价写在话里而不是藏起来：**路径里带空格的程序，本轮没有写法** —— 宁可拒到用户去
//! 改那一格设置，也不要一只有猜的嘴。
//!
//! # 存在性问在前，而且问的是**注入进来的那一只手**
//!
//! 定位不到的程序绝不送进 shell：§7.5 那句「外部应用调用和文件路径应经过校验」在本产品里只有
//! 一处落得下这一判，就是这里。这一层比 shell 自己的本事更严（`ShellExecuteW` 还会查 App Paths
//! 与关联表，一个我们按 PATH 找不到的名字它未必找不到），严的代价由 `Refusal::Unresolved` 那句
//! 话付：请用户把程序写成完整路径。而「按 PATH 找」这件事本仓库已有一处主人
//! [`crate::sendto::locate`]，所以 [`plan`] 把这一问收成**一枚传进来的判据**：决策这一半因此完全
//! 不碰盘（没有文件系统、没有注册表、没有进程），每一臂都能在没有任何桌面的机器上测；桥那侧要
//! 真找时把同一枚 `sendto::locate` 递进来即可，本模块不再写第二个 PATH 遍历器。

/// shell 那一只手能一次收下的命令行长度上限（按宽字符计）。
///
/// `ShellExecuteW` 最终走到 `CreateProcessW`，那里 `lpCommandLine` 的文档值就是 32767；越过它
/// 的这一次不是"跑得慢"而是**根本不会开始**，所以宁可在这里先说完，也不让 shell 回一枚读不出
/// 意义的 `SE_ERR_*`。按 UTF-16 单元数而不是按 `chars()` 数，是因为递给 shell 的正是 `open.rs`
/// 里那一只 `wide()` 编出的串 —— 判据要量的就是真被送出去的那个长度。
pub const SHELL_LIMIT: usize = 32_767;

/// 判出来的一次「可以跑」。三枚字段就是 `open::dispatch` 那一只手收到的、以及门要核对的。
///
/// `exe` 是**定位到的**那一枚（可能是用户写的原话，也可能是 PATH 里找到的全路径），`spelled`
/// 是用户命令行里那第一个词的原话，`parameters` 是原样递给对面的参数块（不带参数时是空串）。
/// 把 `spelled` 也带回去，与 `sendto::Delivery::Dispatch` 同一理由：门要能问「提示里说出的那
/// 一枚，和用户写的那一句，是不是同一件事」，只回一枚布尔或只回一枚路径就问不了。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub exe: String,
    pub spelled: String,
    pub parameters: String,
}

impl Launch {
    /// 有没有参数块。桥只用到这一处分支（话里要不要报出参数），其余都不必知道。
    pub fn has_arguments(&self) -> bool {
        !self.parameters.is_empty()
    }

    /// 这一条判决的一句话：跑哪一枚程序、它是从哪一句定位来的、参数是不是原样递出去。
    ///
    /// 话在这里拼，不在桥那侧拼（#281 那条分工，与 [`crate::update::Answer::sentence`] 同一枚
    /// 形状）：不带参数与带参数说的是两件事，所以也是两句话 —— 门读回时要能分得出用户那一行
    /// 到底有没有留下参数块。
    pub fn sentence(&self) -> String {
        if self.has_arguments() {
            format!(
                "自定义命令：跑 {}（由 {} 定位而来），参数原样递过去：{}",
                self.exe, self.spelled, self.parameters
            )
        } else {
            format!(
                "自定义命令：跑 {}（由 {} 定位而来），这一条不带参数。",
                self.exe, self.spelled
            )
        }
    }
}

/// 不递的八种理由。每一枚说的是**各自那一件**事，绝不两枚共用一句 —— 这一族的测试拿那句
/// 话当分支读数，共用一句就等于把两臂合成一臂。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 一个字都还没有：这一格没填过命令。与「填了，但填坏了」必须分成两句，否则用户读不出
    /// 自己到底是忘了填还是填错。
    NothingWritten,
    /// 越过了 [`SHELL_LIMIT`]：这一次 shell 根本不会开始。
    TooLong { units: usize },
    /// 行里混进了换行。多行不是「第一行加剩下几行」：取第一行就是一种猜。
    MultiLine { breaks: usize },
    /// 一枚看不见的字符：控制符（含 NUL —— 它会把 `wide()` 编出的那串宽字符**提前截断**，是
    /// 这一族里最实际的那把刀），或空格与制表符以外的空白（NBSP、零宽空格、BOM 那一类跟着
    /// 复制粘贴进来的字符）。它们在别的解析器里会被悄悄当成边界。
    Invisible { shown: String },
    /// 这一条以引号开头：程序位置被引号包住了。见模块头部「拒，不救」那一节。
    ProgramOpensQuoted { line: String },
    /// 程序那一段（切出来之后）中间夹着引号。
    ProgramQuoteInside { program: String },
    /// 第一个词是开关（`/C`、`-h`、`--help`）而不是程序：这一条里没有要运行的东西。
    /// 「把开关当程序」正是一只讲道理的解析器会自己造出一枚程序名的形状。
    SwitchOnly { first: String },
    /// 注入进来的那枚判据答"没有这枚程序"：既不是盘上的一条路径，也不在递给它的目录里。
    Unresolved { program: String },
}

impl Refusal {
    /// 那一句产品的话。`pub` 的意义在于**只此一处**：谁要显示，谁就调用它，不许抄一遍字。
    pub fn sentence(&self) -> String {
        match self {
            Refusal::NothingWritten => {
                "自定义命令：这一格里还没写过任何命令：第一个词该是程序，后面跟它的参数。"
                    .to_string()
            }
            Refusal::TooLong { units } => format!(
                "自定义命令：这一条是 {units} 个宽字符，超过 shell 能一次收下的 {SHELL_LIMIT} 个：请把参数写短，或让程序自己去读一个文件。"
            ),
            Refusal::MultiLine { breaks } => format!(
                "自定义命令：这一条里混进了 {breaks} 次换行：自定义命令只认一行，请把要跑的那一行单独留下。"
            ),
            Refusal::Invisible { shown } => format!(
                "自定义命令：这一条里有一枚看不见的字符 {shown}：这里只认空格和制表符两处分隔，看不见的那一枚不送去当程序名。"
            ),
            Refusal::ProgramOpensQuoted { line } => format!(
                "自定义命令：这一条是以引号开头的（{line:?}）：引号里包到哪一格才算程序名要猜，本轮不猜，请把程序名两侧的引号删掉再存。"
            ),
            Refusal::ProgramQuoteInside { program } => format!(
                "自定义命令：程序名 {program:?} 中间夹着引号：补一枚或删一枚都是替目标重新解析这句话，这一条不予执行。"
            ),
            Refusal::SwitchOnly { first } => format!(
                "自定义命令：第一个词 {first} 是一枚开关而不是程序：这一条里没有要运行的东西，请把程序写在最前面。"
            ),
            Refusal::Unresolved { program } => format!(
                "自定义命令：{program} 在这台机器上定位不到（既不是盘上的一条路径，也不在 PATH 里）：请把程序写成完整路径。"
            ),
        }
    }
}

/// 只有这两枚字符算作分隔符。别家 `split_whitespace` 会认下的那些（`\u{0b}`、`\u{0c}`、NBSP、
/// 各种 Zs）在这里都不算 —— 它们要么已经被 [`Refusal::Invisible`] 拒掉，要么就该留在字里。
fn separator(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

/// 这一枚字符会不会被某一套解析器当成"空白"或"边界"，却又不在我们认的那两枚分隔符里。
///
/// 几族里有两族 std 给了现成谓词（`is_control` = Cc，`is_whitespace` = White_Space，它连 Zs 里
/// 的 NBSP 与 Zl/Zp 的行、段分隔符一起认下），所以那两族不列名单。剩下的是 Cf —— std 那个能
/// 一次覆盖 Cf 的 `is_print` 还没稳定，所以这一段是**列出来**的：清一色是跟着一次复制粘贴进来
/// 的字符（软连字符、零宽与双向控制、BOM、tags）。名单漏掉的那些（未分配的 Cn）不会变成一句
/// 谎话：它只会留在程序名里，然后被定位那一问答"没有这个东西" —— 那一问确实没有。中文、日文、
/// 重音字母这些**真该留在路径里**的字，一枚也不在这一族里。
fn invisible(ch: char) -> bool {
    if separator(ch) {
        return false;
    }
    ch.is_control()
        || ch.is_whitespace()
        || matches!(
            ch,
            '\u{00ad}' // 软连字符
            | '\u{180e}' // 蒙古文元音分隔符
            | '\u{200b}'..='\u{200f}' // 零宽空格、半零宽、LRM/RLM
            | '\u{202a}'..='\u{202e}' // 双向格式控制
            | '\u{2060}'..='\u{2064}' // 词连符与不可见结合符
            | '\u{206a}'..='\u{206f}'
            | '\u{feff}' // BOM
            | '\u{fff9}'..='\u{fffb}' // 行内注释控制
            | '\u{e0001}' | '\u{e0020}'..='\u{e007f}' // tags
        )
}

/// 一行用户写的命令 → `open::dispatch` 要的两枚串，或者一句拒绝的话。
///
/// `resolve` 是「这个名字在不在这台机器上，在的话全路径是哪一枚」这一问的主人：桥那侧递
/// [`crate::sendto::locate`]，测试那侧递一枚闭包。本函数不碰盘、不碰注册表、不起进程。
///
/// 各道闸的**次序**是有意为之，每一道都只在自己答得出的那一问之前不问下一问：先问有没有写
/// （一个字、长度），再问这是不是一行（换行、看不见的字符 —— 这两道必须排在切刀之前，因为切点
/// 自己对这些字符就没有免疫力），然后问程序位置是不是被引号包住了（也排在切刀之前，理由写在
/// 那一臂旁边），切完了才问切出来的那一截交不交得出去（夹着引号、开关），最后才问定位。
/// 两臂引号都排在定位之前是因为好意会撒谎：一枚带引号的 token 走到 `resolve` 那里必然报
/// "找不到"，于是用户听到的是一句关于 PATH 的假话，而真正的问题是那对引号。
pub fn plan<F>(command: &str, resolve: F) -> Result<Launch, Refusal>
where
    F: Fn(&str) -> Option<String>,
{
    let line = command.trim_matches(separator);
    if line.is_empty() {
        return Err(Refusal::NothingWritten);
    }
    let units = line.encode_utf16().count();
    if units > SHELL_LIMIT {
        return Err(Refusal::TooLong { units });
    }
    let breaks = line.chars().filter(|ch| matches!(ch, '\n' | '\r')).count();
    if breaks > 0 {
        return Err(Refusal::MultiLine { breaks });
    }
    if let Some(ch) = line.chars().find(|ch| invisible(*ch)) {
        return Err(Refusal::Invisible {
            shown: ch.escape_debug().to_string(),
        });
    }

    // 引号开头的那一道排在**切刀之前**：一刀切在第一处空白上，而 `"C:\Program Files\X\t.exe" /p`
    // 的第一处空白正好落在引号里面 —— 先切再判，报出去的程序名就是一段 `"C:\Program` 那样的
    // 碎片，那句关于引号的话也就说不准了。开头一枚引号本身就是"程序位置被包住了"这一件事，
    // 不需要知道它在哪一格合上才判得出来，而不合上的那些同样不救。
    if line.starts_with('"') {
        return Err(Refusal::ProgramOpensQuoted {
            line: line.to_string(),
        });
    }

    let cut = line.find(separator).unwrap_or(line.len());
    let (program, rest) = line.split_at(cut);
    let parameters = rest.trim_matches(separator);

    if program.contains('"') {
        return Err(Refusal::ProgramQuoteInside {
            program: program.to_string(),
        });
    }
    // 一枚程序名不会以 `-` 或 `/` 开头，会这么开头的只有开关。放它去定位，就是让「没有要跑的
    // 东西」那一格长成一句关于 PATH 的话。
    if program.starts_with('-') || program.starts_with('/') {
        return Err(Refusal::SwitchOnly {
            first: program.to_string(),
        });
    }

    match resolve(program) {
        Some(found) => Ok(Launch {
            exe: found,
            spelled: program.to_string(),
            parameters: parameters.to_string(),
        }),
        None => Err(Refusal::Unresolved {
            program: program.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sendto;
    use std::cell::{Cell, RefCell};

    /// 一枚"这台机器上什么都有"的判据：交出的是**拼出来的**全路径，好让测试看得见 `exe` 与
    /// `spelled` 是两枚各自的东西，而不是同一枚字符串被复制了两遍。
    fn always(name: &str) -> Option<String> {
        Some(format!("C:\\Windows\\System32\\{name}"))
    }

    /// 一枚"这台机器上什么都没有"的判据。
    fn never(_name: &str) -> Option<String> {
        None
    }

    /// 一格都没填 ⇒ 「还没写过任何命令」，不是「这一条坏了」。
    #[test]
    fn an_empty_cell_says_nothing_was_written() {
        for blank in ["", "   ", "\t", " \t "] {
            assert_eq!(
                plan(blank, never),
                Err(Refusal::NothingWritten),
                "wrong arm for {blank:?}"
            );
        }
        assert_eq!(
            Refusal::NothingWritten.sentence(),
            "自定义命令：这一格里还没写过任何命令：第一个词该是程序，后面跟它的参数。"
        );
    }

    /// 第一个词是程序、剩下的整段是参数块：三枚字段各归各的位，连那句判决话也一起钉住。
    #[test]
    fn the_first_word_is_the_program_and_the_rest_is_the_block() {
        let got = plan("tool.exe C:\\My Shots\\a b.txt", always).unwrap();
        assert_eq!(got.exe, "C:\\Windows\\System32\\tool.exe");
        assert_eq!(got.spelled, "tool.exe");
        assert_eq!(got.parameters, "C:\\My Shots\\a b.txt");
        assert!(got.has_arguments());
        assert_eq!(
            got.sentence(),
            "自定义命令：跑 C:\\Windows\\System32\\tool.exe（由 tool.exe 定位而来），参数原样递过去：C:\\My Shots\\a b.txt"
        );
    }

    /// 光一枚程序名也是完整的一条命令：参数块是空串，而话里必须说「不带参数」，不能留一段
    /// 「参数原样递过去：」后面跟着空 —— 那一格读起来就像参数丢了。
    #[test]
    fn a_bare_program_carries_an_empty_parameter_block() {
        let got = plan("tool.exe", always).unwrap();
        assert_eq!(got.parameters, "");
        assert!(!got.has_arguments());
        assert_eq!(
            got.sentence(),
            "自定义命令：跑 C:\\Windows\\System32\\tool.exe（由 tool.exe 定位而来），这一条不带参数。"
        );
    }

    /// 首尾的空格与制表符不属于程序名也不属于参数块，而**中间**的空格属于参数块：折叠它就是
    /// 在我们这一侧改写用户写给对面的那句话。
    #[test]
    fn spaces_at_the_edges_are_not_part_of_either_half() {
        let got = plan(" \t tool.exe   --flag   \t", always).unwrap();
        assert_eq!(got.spelled, "tool.exe");
        assert_eq!(got.parameters, "--flag");

        let spaced = plan("tool.exe  a   b", always).unwrap();
        assert_eq!(spaced.parameters, "a   b");
    }

    /// 制表符算分隔符，所以它是边界而不是程序名的一部分。
    #[test]
    fn a_tab_separates_the_program_from_its_block() {
        let got = plan("tool.exe\t--flag", always).unwrap();
        assert_eq!(got.spelled, "tool.exe");
        assert_eq!(got.parameters, "--flag");
    }

    /// 参数块**逐字节**原样递出：`&`、`|`、`^`、成对引号都不改写、不转义、不重排。这一格钉的
    /// 是本模块的形状本身 —— 中间没有我们拼出来的任何一段字，也就没有第二句话等着被解析。
    #[test]
    fn the_parameter_block_reaches_the_shell_byte_for_byte() {
        let line = r###"tool.exe --url https://x.test/?a=1&b=2 --mask "a  b" ^tail|one"###;
        let got = plan(line, always).unwrap();
        assert_eq!(
            got.parameters,
            r###"--url https://x.test/?a=1&b=2 --mask "a  b" ^tail|one"###
        );
    }

    /// 程序位置被引号包住 ⇒ 拒，不救。带空格的整条路径也算在这一臂里：开头那一枚引号已经足够
    /// 说明"程序是哪一截"要猜，而判这一件事不需要找到合上的那一枚。参数块那一侧的成对引号照
    /// 原样走（见 `the_parameter_block_reaches_the_shell_byte_for_byte` 那一格），程序这一侧直接拒 ——
    /// 两臂分开，才读得出坏在哪一段。
    #[test]
    fn a_program_position_opened_by_a_quote_is_refused_not_rescued() {
        for line in [
            r#""C:\Program Files\X\tool.exe" /p"#,
            r#""tool.exe""#,
            r#""a"b" c"#,
            r#""""#,
            r#"""#,
        ] {
            match plan(line, always) {
                Err(Refusal::ProgramOpensQuoted { line: said }) => {
                    assert_eq!(said, line, "话里报的该是那一整行")
                }
                other => panic!("the opened-quote arm for {line:?}: {other:?}"),
            }
        }
        assert_eq!(
            Refusal::ProgramOpensQuoted {
                line: "\"tool.exe\"".to_string()
            }
            .sentence(),
            "自定义命令：这一条是以引号开头的（\"\\\"tool.exe\\\"\"）：引号里包到哪一格才算程序名要猜，本轮不猜，请把程序名两侧的引号删掉再存。"
        );
    }

    /// 引号夹在切出来的那一截程序名中间：另一枚臂，另一句话。判据不是"看起来能修"，而是这一枚
    /// 引号让交给 `object` 参数的那一段不再是用户写的那一段。
    #[test]
    fn a_quote_inside_the_program_token_is_refused_not_repaired() {
        for (line, program) in [(r#"no"t.exe a"#, r#"no"t.exe"#), (r#"x".exe"#, r#"x".exe"#)] {
            match plan(line, always) {
                Err(Refusal::ProgramQuoteInside { program: said }) => {
                    assert_eq!(said, program, "wrong token for {line:?}")
                }
                other => panic!("the inside-quote arm for {line:?}: {other:?}"),
            }
        }
        assert_eq!(
            Refusal::ProgramQuoteInside {
                program: "no\"t.exe".to_string()
            }
            .sentence(),
            "自定义命令：程序名 \"no\\\"t.exe\" 中间夹着引号：补一枚或删一枚都是替目标重新解析这句话，这一条不予执行。"
        );
    }

    /// 只有开关的一行没有要运行的东西：`/C`、`-h`、`--help`。这是"讲道理的解析器自己造出一枚
    /// 程序名"那一族形状，所以它必须排在定位之前就被拒。
    #[test]
    fn a_line_whose_only_content_is_a_switch_names_the_switch() {
        for (line, first) in [("/C", "/C"), ("-h", "-h"), ("--help -q", "--help")] {
            match plan(line, always) {
                Err(Refusal::SwitchOnly { first: said }) => assert_eq!(said, first),
                other => panic!("the switch arm for {line:?}: {other:?}"),
            }
        }
        assert_eq!(
            Refusal::SwitchOnly {
                first: "/C".to_string()
            }
            .sentence(),
            "自定义命令：第一个词 /C 是一枚开关而不是程序：这一条里没有要运行的东西，请把程序写在最前面。"
        );
    }

    /// 定位不到的程序，在任何一次 shell 问答之前就被拒，而话里报的是用户写的那一枚名字。
    #[test]
    fn an_unresolvable_program_is_refused_before_the_shell_is_asked() {
        match plan("falconshot-not-installed.exe a.txt", never) {
            Err(Refusal::Unresolved { program }) => {
                assert_eq!(program, "falconshot-not-installed.exe")
            }
            other => panic!("wrong arm: {other:?}"),
        }
        assert_eq!(
            Refusal::Unresolved {
                program: "tool.exe".to_string()
            }
            .sentence(),
            "自定义命令：tool.exe 在这台机器上定位不到（既不是盘上的一条路径，也不在 PATH 里）：请把程序写成完整路径。"
        );
    }

    /// 问定位之前就被拒的那些行，一次都不许摸判据：这一格钉的是「次序」本身，而不只是某一句
    /// 话 —— 摸过一次判据，就意味着某条臂上存在第二道存在性检查。
    #[test]
    fn a_refused_line_never_reaches_the_resolver() {
        let huge = "tool.exe ".to_string() + &"x".repeat(SHELL_LIMIT + 1);
        for line in [
            "",
            "   ",
            "/C",
            "-h",
            r#""tool.exe" x"#,
            r#"no"t.exe"#,
            "tool.exe a\nb",
            "tool.exe \u{7}",
            "\u{feff}tool.exe",
            huge.as_str(),
        ] {
            let asks = Cell::new(0u32);
            let counted = |_name: &str| {
                asks.set(asks.get() + 1);
                Some("C:\\x\\tool.exe".to_string())
            };
            assert!(plan(line, counted).is_err(), "{line:?} 本该被拒");
            assert_eq!(asks.get(), 0, "{line:?} 去问了判据");
        }
    }

    /// 判据收到的那一句必须正好是程序 token（trim 过、切过、不含参数块的字）。否则「定位到的」
    /// 与「用户写的」中间还夹着一层谁也没写过的加工。
    #[test]
    fn the_resolver_is_asked_about_the_program_token_only() {
        let asked = RefCell::new(Vec::new());
        let got = plan("  tool.exe a b  ", |name: &str| {
            asked.borrow_mut().push(name.to_string());
            Some(format!("C:\\x\\{name}"))
        })
        .unwrap();
        assert_eq!(asked.borrow().as_slice(), &["tool.exe".to_string()]);
        assert_eq!(got.exe, "C:\\x\\tool.exe");
        assert_eq!(got.parameters, "a b");
    }

    /// 混进换行的整条拒，不取第一行。取第一行就是一种猜：用户贴进来三段，执行的是他没打算跑
    /// 的那一段。
    #[test]
    fn a_line_break_is_refused_rather_than_truncated() {
        for (line, breaks) in [
            ("tool.exe a\nb", 1),
            ("tool.exe\r\nx", 2),
            ("\ntool.exe", 1),
            ("tool.exe \n\n", 2),
        ] {
            match plan(line, always) {
                Err(Refusal::MultiLine { breaks: said }) => assert_eq!(said, breaks),
                other => panic!("the multiline arm for {line:?}: {other:?}"),
            }
        }
        assert_eq!(
            Refusal::MultiLine { breaks: 1 }.sentence(),
            "自定义命令：这一条里混进了 1 次换行：自定义命令只认一行，请把要跑的那一行单独留下。"
        );
    }

    /// 控制字符留在程序名或参数块里 ⇒ 拒，不截断。NUL 尤其：`wide()` 编出的那串宽字符到它就
    /// 结束了，对面收到的是句子前段，而不是我们判过的那一整句。
    #[test]
    fn a_control_character_never_becomes_part_of_the_program() {
        for (line, shown) in [
            ("tool.exe \u{7}", "\\u{7}"),
            ("tool\0.exe", "\\0"),
            // 尾随的那一枚也算：`\u{0b}` `\u{0c}` 正是 `split_whitespace` 会当空格切开的两枚。
            ("tool.exe x\u{b}", "\\u{b}"),
            ("tool.exe x\u{c}", "\\u{c}"),
        ] {
            match plan(line, always) {
                Err(Refusal::Invisible { shown: said }) => {
                    assert_eq!(said, shown, "wrong char named for {line:?}")
                }
                other => panic!("the invisible arm for {line:?}: {other:?}"),
            }
        }
        assert_eq!(
            Refusal::Invisible {
                shown: "\\u{7}".to_string()
            }
            .sentence(),
            "自定义命令：这一条里有一枚看不见的字符 \\u{7}：这里只认空格和制表符两处分隔，看不见的那一枚不送去当程序名。"
        );
    }

    /// 看着像空格的字符不当边界：NBSP、零宽空格、BOM、行分隔符。它们落进程序名里只会变成一句
    /// 「定位不到」的谎话，所以当场说清是这一枚字符。
    #[test]
    fn a_space_that_is_not_a_space_is_refused_too() {
        for line in [
            "tool.exe\u{a0}a",
            "tool\u{200b}.exe",
            "\u{feff}tool.exe",
            "tool.exe \u{2028}a",
            "tool.exe \u{2060}a",
            "tool\u{ad}.exe",
        ] {
            match plan(line, always) {
                Err(Refusal::Invisible { .. }) => {}
                other => panic!("the invisible arm for {line:?}: {other:?}"),
            }
        }
    }

    /// 越过 shell 上限的一行 ⇒ 拒，而报出的数是**宽字符**那一个（判据量的就是送出去的长度）。
    #[test]
    fn a_line_past_the_shell_limit_is_refused() {
        let long = "tool.exe ".to_string() + &"x".repeat(SHELL_LIMIT);
        match plan(&long, always) {
            Err(Refusal::TooLong { units }) => {
                assert_eq!(units, long.encode_utf16().count())
            }
            other => panic!("the length arm: {other:?}"),
        }
        assert_eq!(
            Refusal::TooLong { units: 40000 }.sentence(),
            "自定义命令：这一条是 40000 个宽字符，超过 shell 能一次收下的 32767 个：请把参数写短，或让程序自己去读一个文件。"
        );
        // 差一枚就还在限内：这一格钉的是"边界是这一枚数，不是随手一个 32768"。
        let fits = "tool.exe".to_string() + &"x".repeat(SHELL_LIMIT - 8);
        assert_eq!(fits.encode_utf16().count(), SHELL_LIMIT);
        assert!(plan(&fits, always).is_ok());
    }

    /// 八枚臂，八句话，两两不同。这一格是「话就是分支读数」那条规矩自身的测试：哪天两枚共用
    /// 了一句，这里先红。判决那两句也在同一张表里各占一格。
    #[test]
    fn every_refusal_says_a_different_sentence() {
        let all = [
            Refusal::NothingWritten,
            Refusal::TooLong { units: 40000 },
            Refusal::MultiLine { breaks: 2 },
            Refusal::Invisible {
                shown: "\\u{7}".to_string(),
            },
            Refusal::ProgramOpensQuoted {
                line: "\"tool.exe\"".to_string(),
            },
            Refusal::ProgramQuoteInside {
                program: "no\"t.exe".to_string(),
            },
            Refusal::SwitchOnly {
                first: "/C".to_string(),
            },
            Refusal::Unresolved {
                program: "tool.exe".to_string(),
            },
        ];
        let said: std::collections::HashSet<String> = all.iter().map(Refusal::sentence).collect();
        assert_eq!(said.len(), all.len(), "两枚臂共用了同一句话");

        let with = Launch {
            exe: "C:\\x\\tool.exe".to_string(),
            spelled: "tool.exe".to_string(),
            parameters: "a.txt".to_string(),
        };
        let bare = Launch {
            exe: with.exe.clone(),
            spelled: with.spelled.clone(),
            parameters: String::new(),
        };
        let other = Launch {
            exe: with.exe.clone(),
            spelled: with.spelled.clone(),
            parameters: "另一个参数".to_string(),
        };
        assert_ne!(with.sentence(), bare.sentence());
        assert_ne!(other.sentence(), bare.sentence());
        assert_ne!(with.sentence(), other.sentence());
        assert!(!said.contains(&with.sentence()));
        assert!(!said.contains(&bare.sentence()));
    }

    /// 判据这一枚接口就是 `sendto::locate` 的形状。这一格钉的是「本仓库只有一个 PATH 遍历器」：
    /// 桥那侧递进来的那一只手签名对得上，就不必再写第二个。
    #[test]
    fn the_resolver_interface_is_the_one_sendto_already_owns() {
        let as_sendto: fn(&str) -> Option<String> = sendto::locate;
        match plan("falconshot-definitely-not-installed.exe a.txt", as_sendto) {
            Err(Refusal::Unresolved { program }) => {
                assert_eq!(program, "falconshot-definitely-not-installed.exe")
            }
            other => panic!("a name that is nowhere must be refused: {other:?}"),
        }
    }
}
