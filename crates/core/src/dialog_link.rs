//! §5.5.11 文件保存对话框联动 —— the deciding half of that section, with no window in
//! sight.
//!
//! The section's four steps split by what can be graded. Step 1 is one `[output]` switch
//! ([`crate::config::Output::hide_pins_on_dialog`]) and step 2 is the user pressing 保存;
//! both live on the calling side. Steps 3 and 4 are two questions - 谁可能被这张对话框压住,
//! 什么时候才该把它们放回来 - and they are answered here, on numbers, so that `cargo test`
//! can grade them on a machine with no display. The same rule [`crate::pin`] works under.

use crate::geometry::PhysRect;
use crate::pin::PinId;

/// 一张对话框落下来之后，哪些贴图**可能**压住它（§5.5.11 功能说明的原话是"可能遮挡"）。
///
/// 问的是"有任何一角相交"而不是"盖住大半"：一张只压住对话框右下角的贴图，照样让那一角的
/// "保存"按钮点不到，而按钮消失的原因用户看不见。反过来，贴边共线不算遮挡 ——
/// [`PhysRect::intersection`] 把"两枚窗各占其半、中间不留缝"读成 `None`，那种情形谁也没压谁。
///
/// 名单按传入顺序交回，去留由 [`Hidden`] 记账；这一枚函数不判开关，也不记得上一次问过它什么。
pub fn covering(pins: &[(PinId, PhysRect)], dialog: PhysRect) -> Vec<PinId> {
    pins.iter()
        .filter(|(_, rect)| rect.intersection(&dialog).is_some())
        .map(|(id, _)| *id)
        .collect()
}

/// §5.5.11 步骤 3 与步骤 4 之间的那一格账：此刻为对话框藏起来的贴图，以及为它们记的层数。
///
/// 记层数而不是记一枚布尔，是因为一次保存流程里可以叠开两张对话框（"另存为"之后再弹一次
/// 覆盖确认），第一张关掉时第二张还开着 —— 那时候放回贴图，贴图的窗口就压回用户正要按的
/// 第二张框上。恢复只该发生在最后一枚关掉那一刻。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hidden {
    enabled: bool,
    dialogs: usize,
    ids: Vec<PinId>,
}

impl Hidden {
    /// 开关由调用方递进来（`[output] hide_pins_on_dialog`）。关着的时候 [`Self::opened`] 与
    /// [`Self::closed`] 一律交空名单：整段联动是步骤 1 那一格的用户决定，不是一枚默认行为，
    /// 也不是这一层能替用户否决的事。
    pub const fn new(enabled: bool) -> Self {
        Self {
            enabled,
            dialogs: 0,
            ids: Vec::new(),
        }
    }

    /// 对话框此刻开着吗（`dialogs > 0`，与开关无关地诚实）。量具侧用它核对"事件进过这一层"。
    pub const fn is_hiding(&self) -> bool {
        self.dialogs > 0
    }

    /// 步骤 1 那一格此刻生效的是哪一份。运行期把它读进账里，是为了让"横幅显示的那份"与
    /// "联动应用的那份"能被同一行引用（#281 那一族）：开关关着时 `dialogs` 恒 0，
    /// 那一格光靠层数分不出"用户没开对话框"与"用户把这一格关了"。
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 此刻藏着的那些枚，按藏起来的先后。
    pub fn hidden(&self) -> &[PinId] {
        &self.ids
    }

    /// 开着几层对话框。
    pub const fn dialogs(&self) -> usize {
        self.dialogs
    }

    /// 一枚对话框出现。交回**此刻新藏**的那些枚：已经在藏的不重复报一次，否则调用方会把
    /// 同一枚窗藏两遍，而"藏两遍"在窗口那一侧是一次多余的 setVisible(false)。
    ///
    /// 第二枚对话框可能落在另一个位置、压住另一批贴图，所以每一层都重新问一遍谁遮挡；
    /// 名单是并集，恢复仍然只由最后一枚关掉那一次交回。
    pub fn opened(&mut self, candidates: &[PinId]) -> Vec<PinId> {
        if !self.enabled {
            return Vec::new();
        }
        self.dialogs += 1;
        let mut fresh = Vec::new();
        for id in candidates {
            if !self.ids.contains(id) {
                self.ids.push(*id);
                fresh.push(*id);
            }
        }
        fresh
    }

    /// 一枚对话框关闭。还留着别层时交空；最后一枚关掉时交回它藏过的那些枚，并把账清零 ——
    /// 下一次出现要重新问一遍谁遮挡，因为期间用户可能挪走过贴图（§5.9.1）也可能关掉过一枚
    /// 贴图（§5.9.10），旧名单会把没藏过的窗放回来、或者把已经不在的窗再放一次。
    ///
    /// 没有对话框开着时的这一声关闭不当回事：藏过 0 枚，也就没有可恢复的那些枚。事件流里
    /// 一次关闭可能到达两次（点"保存"时对话框自己关、量具随后再关一次），判红那一声会让
    /// 每一趟门都留下一枚没人能复现的红格。
    pub fn closed(&mut self) -> Vec<PinId> {
        if self.dialogs == 0 {
            return Vec::new();
        }
        self.dialogs -= 1;
        if self.dialogs > 0 {
            return Vec::new();
        }
        std::mem::take(&mut self.ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> PhysRect {
        PhysRect::new(x, y, w, h)
    }

    /// 三枚贴图：完全不相交的、只压住一角的那一枚、把整张框盖住的那一枚。后两枚都要藏。
    #[test]
    fn covering_hides_the_one_that_barely_touches_it_too() {
        let dialog = rect(100, 100, 200, 100);
        let pins = vec![
            (1, rect(0, 0, 50, 50)),     // 离得远
            (2, rect(280, 180, 40, 40)), // 只压住右下角那一小块
            (3, rect(90, 90, 300, 300)), // 整张盖住
        ];
        assert_eq!(
            covering(&pins, dialog),
            vec![2, 3],
            "「可能遮挡」问的是一角有没有相交，不是盖住多少"
        );
    }

    /// 贴边共线不是遮挡：一张窗结束在 x=100、另一张从 x=100 起，中间不留缝，谁也没压谁。
    #[test]
    fn covering_leaves_a_pin_that_only_shares_an_edge() {
        let dialog = rect(100, 100, 200, 100);
        let pins = vec![(7, rect(0, 100, 100, 100)), (8, rect(300, 100, 100, 100))];
        assert_eq!(
            covering(&pins, dialog),
            Vec::<PinId>::new(),
            "共边那一侧交出来的是 0 宽的一条线，不是遮住的一块像素"
        );
    }

    /// 一张零面积的对话框谁也不遮挡 —— `is_empty` 那一路在 intersection 里同样是 `None`。
    #[test]
    fn covering_asks_nothing_of_a_zero_area_dialog() {
        let pins = vec![(1, rect(0, 0, 10, 10)), (2, rect(5, 5, 10, 10))];
        assert_eq!(
            covering(&pins, rect(5, 5, 0, 0)),
            Vec::<PinId>::new(),
            "零面积的框压不住任何东西；这一格是判据的空集一侧，不是漏判"
        );
    }

    /// 步骤 1 那一格关着 ⇒ 整段联动不发生：藏 0 枚、恢复 0 枚，连层数也不记。
    #[test]
    fn the_switch_off_hides_nothing_and_restores_nothing() {
        let mut link = Hidden::new(false);
        assert_eq!(link.opened(&[1, 2]), Vec::<PinId>::new());
        assert_eq!(
            link.dialogs(),
            0,
            "关着时连「有几层」都不记，事件压根没进这一层"
        );
        assert!(!link.is_hiding());
        assert_eq!(link.closed(), Vec::<PinId>::new());
        assert!(link.hidden().is_empty());
    }

    /// 叠开两层的保存流程：第一张框关掉时不放回贴图，最后一张关掉才放。
    #[test]
    fn restore_waits_for_the_last_dialog_to_close() {
        let mut link = Hidden::new(true);
        assert_eq!(link.opened(&[1, 2]), vec![1, 2]);
        assert_eq!(link.opened(&[3]), vec![3], "第二层新压住的这一枚也藏起来");
        assert_eq!(link.dialogs(), 2);
        assert_eq!(
            link.closed(),
            Vec::<PinId>::new(),
            "第一张关掉时第二张还开着，放回贴图就压回用户正要按的那张框上"
        );
        assert_eq!(link.hidden(), vec![1, 2, 3], "还开着 ⇒ 名单不动");
        assert_eq!(link.closed(), vec![1, 2, 3]);
        assert_eq!(link.dialogs(), 0);
    }

    /// 两层压住同一批贴图 ⇒ 第二层交空名单，同一扇窗不被藏第二遍。
    #[test]
    fn a_second_dialog_over_the_same_pins_hides_no_extra_window() {
        let mut link = Hidden::new(true);
        assert_eq!(link.opened(&[4, 5]), vec![4, 5]);
        assert_eq!(link.opened(&[4, 5, 6]), vec![6]);
        assert_eq!(
            link.hidden(),
            vec![4, 5, 6],
            "名单是并集，且保持藏起来的先后"
        );
        assert_eq!(link.closed(), Vec::<PinId>::new());
        assert_eq!(link.closed(), vec![4, 5, 6]);
    }

    /// 没有对话框开着时的一声关闭不当回事：不崩、不记负数、也不凭空放回贴图。
    #[test]
    fn a_close_with_nothing_open_invents_no_restore() {
        let mut link = Hidden::new(true);
        assert_eq!(link.closed(), Vec::<PinId>::new());
        assert_eq!(link.dialogs(), 0);
        link.opened(&[9]);
        assert_eq!(link.closed(), vec![9]);
        assert_eq!(
            link.closed(),
            Vec::<PinId>::new(),
            "点保存时对话框自己关一次、量具随后再关一次 —— 第二声不该把 9 放两遍"
        );
    }

    /// 全部关掉之后账清零：重新出现的那一张框按它自己的位置问一遍谁遮挡。
    #[test]
    fn a_reopened_dialog_is_asked_again() {
        let mut link = Hidden::new(true);
        link.opened(&[1, 2]);
        link.closed();
        assert!(link.hidden().is_empty(), "上一轮的名单不带进下一轮");
        assert_eq!(link.opened(&[2]), vec![2], "这一轮只压住一枚，就只藏那一枚");
    }
}
