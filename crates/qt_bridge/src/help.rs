//! §5.21.2「帮助文档」的那一只手：托盘第 1440 行那枚「帮助」按下去到底做什么。
//!
//! PRD 那一节列了四行（第 1559–1562 行）：工具栏按钮的提示文本、快捷键设置页的功能说明、命令行
//! 配置处的参数文档入口、复杂功能的首次使用引导。这四行**早就各有唯一作者**——
//! [`crate::annotate`] 的 `TOOL_TEXTS`、[`falcon_core::config`] 的 `ACTION_TEXTS`、
//! [`crate::cli`] 的 `PARAM_DOC` / `doc_entry`、[`crate::guide`] 的 `ENTRIES`。所以这一档不写第五
//! 份说明文字：它只做两件 PRD 已经点名、而本 build 之前没有人做的事——
//!
//! 一，给「帮助」那枚菜单项一个名字的唯一作者（[`TRAY_WORD`]），与 `history` / `groups` /
//! `update` 三臂同族（#281：托盘这一项与门里那一格问的是同一枚名，抄第二份中文就会先旧）。
//!
//! 二，把那四行的**去处**汇成一句能上屏的话。这里逐枚数的是那四位作者自己交回的长度，不是本档
//! 记的枚数：任何一行将来长了、短了、空了，这一句跟着变，而 [`Counts::incomplete`] 会把空的那
//! 一行点名出来。一句「帮助」不该由第五个作者重新发明一遍内容，它该把人送到有内容的那四处去。
//!
//! ## 这一档不声称的那件事
//!
//! 它一次也没真往那枚菜单项投过点击（`WM_TRAY` 那一发要在跑的泵上），所以「真人点中它走的是不是
//! [`crate::hotkey::Work::Help`] 这一臂」仍记在结转里，由 [`crate::help_menu`] 那一档发 Blocked，
//! 不拿这里能算出来的四枚数冒充。

use falcon_core::config::ACTION_TEXTS;

use crate::annotate::tool_names;
use crate::cli::{doc_entry, PARAM_DOC};
use crate::guide::ENTRIES;

/// §5.18.1 第 1440 行那一列的两个字。唯一作者在这里，托盘那一项与 `--help-menu` 门里那一格读的
/// 都是它（#281，与 `history_window::TRAY_WORD` / `group_window::TRAY_WORD` / `update::TRAY_WORD`
/// 同一枚理由）。
pub const TRAY_WORD: &str = "帮助";

/// 四行各自此刻的枚数，逐枚从**那一行的作者**那里取，不由本档记一份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    /// §5.21.2 第一行：工具栏上有多少枚工具各带一句提示（作者 = `annotate::TOOL_TEXTS`）。
    pub toolbar_tools: usize,
    /// §5.21.2 第二行：快捷键设置页上有多少个动作名各带一句功能说明（作者 = `config::ACTION_TEXTS`）。
    pub hotkey_rows: usize,
    /// §5.21.2 第三行：命令行参数文档里有多少条参数（作者 = `cli::PARAM_DOC`）。
    pub cli_params: usize,
    /// §5.21.2 第四行：首次使用引导铺了几页（作者 = `guide::ENTRIES`）。
    pub guide_pages: usize,
}

impl Counts {
    /// 此刻这一句里点名的四枚数。取的是四位作者的 `len()`，因此这一只手不会在任何一处发明内容。
    pub fn now() -> Self {
        Self {
            toolbar_tools: tool_names().split('|').filter(|s| !s.is_empty()).count(),
            hotkey_rows: ACTION_TEXTS.len(),
            cli_params: PARAM_DOC.len(),
            guide_pages: ENTRIES.len(),
        }
    }

    /// 四行里有哪一行此刻**没有内容**：交回它的名字，而不是交回一个笼统的「不齐」。
    ///
    /// 这一格存在的理由是让 `--help-menu` 那一档能把「作者还在不在」判成一条可失败的读数——
    /// 一名作者消失，用户按「帮助」看到的那一句就会少一行，而这正是 PRD 那一节要的四行。
    pub fn incomplete(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.toolbar_tools == 0 {
            missing.push("工具栏提示");
        }
        if self.hotkey_rows == 0 {
            missing.push("快捷键说明");
        }
        if self.cli_params == 0 || doc_entry().is_empty() {
            missing.push("命令行参数文档");
        }
        if self.guide_pages == 0 {
            missing.push("首次使用引导");
        }
        missing
    }

    /// 那句上屏的话。与 [`falcon_core::update::Answer::sentence`] 同一分工：谁要显示，谁就调用它，
    /// 不许把这段字抄第二遍。
    pub fn sentence(&self) -> String {
        if let Some(first) = self.incomplete().first() {
            return format!("{TRAY_WORD}：这一行还没有内容（{first}），其余三行见设置页与 --help");
        }
        format!(
            "{TRAY_WORD}：工具栏提示 {} 枚｜快捷键说明 {} 条｜命令行参数文档 {} 条（入口 --help）｜首次使用引导 {} 页",
            self.toolbar_tools, self.hotkey_rows, self.cli_params, self.guide_pages
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 四行此刻都得有内容——这一条不是给本档看的装饰：`sentence()` 在缺任何一行时会换成另一句，
    /// 而那一句是「欠」的读数，不是「齐」的读数。
    #[test]
    fn the_four_help_rows_all_have_an_author_right_now() {
        let c = Counts::now();
        assert_eq!(c.incomplete(), Vec::<&str>::new(), "{c:?}");
        assert!(c.toolbar_tools > 0 && c.hotkey_rows > 0 && c.cli_params > 0 && c.guide_pages > 0);
    }

    /// 那一句里的四个数必须是**现取的**：改一枚计数，句子就跟着改，句子不会自己记一份。
    #[test]
    fn the_sentence_carries_the_counts_it_was_handed_not_a_copy() {
        let said = Counts {
            toolbar_tools: 1,
            hotkey_rows: 2,
            cli_params: 3,
            guide_pages: 4,
        }
        .sentence();
        assert!(
            said.contains("工具栏提示 1 枚")
                && said.contains("快捷键说明 2 条")
                && said.contains("命令行参数文档 3 条")
                && said.contains("首次使用引导 4 页"),
            "{said}"
        );
    }

    /// 缺行时那一句不许继续报数：它得点名少的是哪一行，否则「四行齐了」会由一句话冒充。
    #[test]
    fn a_missing_row_is_named_instead_of_a_short_sentence() {
        let c = Counts {
            toolbar_tools: 15,
            hotkey_rows: 15,
            cli_params: 0,
            guide_pages: 6,
        };
        assert_eq!(c.incomplete(), vec!["命令行参数文档"]);
        assert!(c.sentence().contains("命令行参数文档"), "{}", c.sentence());
    }

    /// 唯一作者那一格：`TRAY_WORD` 就是 PRD 第 1440 行那一列的两个字，且这一档不说「设置」/「说明」。
    #[test]
    fn the_tray_word_is_the_docs_own_two_characters() {
        assert_eq!(TRAY_WORD, "帮助");
    }
}
