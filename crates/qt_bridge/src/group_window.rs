//! §5.12.3 与 §5.12.4 的那一扇「图片分组管理」窗（PRD 第 1232–1253 行），与 §5.18.1 第 1437 行
//! 那枚托盘项。
//!
//! **这一档存在的原因**：PRD 第 1218 行写「用户打开“图片分组管理”。」、第 1236 行写
//! 「用户进入分组管理页。」、第 1251 行写「用户从托盘菜单、分组面板或快捷键选择分组。」，而
//! `hotkey.rs` 头顶那封欠账信自己记着 §5.18.1 那十枚菜单建议里的「图片分组」在本 build 没有对面
//! （「菜单建议里的 图片分组 / 帮助 / 检查更新 三条，本 build 都还没有能跑的那只手」，
//! 前置档 `p172_pre_A.txt` 第 41–46 行逐档现读它那六处）。要说清楚的是：**§5.12 那四节的每一条
//! 规则都不缺一只手**，缺的是那扇窗：
//! * 「新建分组 / 输入分组名称 / 保存后创建空分组」（第 1219–1221 行）对面是
//!   [`falcon_core::pin::PinSet::create_group`]，桥侧读者是
//!   [`crate::session::qobject::Session::create_group`]；
//! * 「将贴图加入分组」那四步（第 1227–1230 行）对面是 [`crate::state::PinState::assign_selection_to_group`]；
//! * 「执行删除、重命名或拖动排序」（第 1238 行）对面是 `rename_group` / `delete_group` / `move_group`；
//! * 「删除非空分组时，应询问贴图如何处理」（第 1243 行）对面是 [`crate::state::PinState::delete_group`]
//!   的 `destroy` 那一枚布尔；
//! * 「排序变化应同步反映到快捷切换菜单」（第 1245 行）对面是 [`crate::state::PinState::group_rows`]
//!   —— 它返回的就是 core 那个 `groups` vec 本体的顺序，桥侧一格不重排；
//! * 「执行显示、隐藏或切换」与「系统统一更新该分组内贴图的可见状态」（第 1252–1253 行）对面是
//!   [`falcon_core::pin::PinSet::toggle_group_shown`] 加 `PinItem::group_hidden`。
//!
//! 也就是说：这六格全部只被**量具与贴图窗自己**递过（`PinWindow.qml` 那一枚「分组」子菜单、
//! `state.rs` 的 selftest 那一格、以及 `the_group_hands_cross_the_bridge_and_answer_apart` 那枚单测），
//! 一次也没有被**一枚用户能从托盘点开的窗**递过。PRD 第 1218 行与第 1251 行要的是后者。
//! 这一档因此不重新发明任何一只手：窗上那几枚钮叫的是 `session.createGroup / renameGroup /
//! deleteGroup / moveGroup / toggleGroupShown` 那五枚**已经存在**的 invokable，本页**不写第二份
//! 判据、不数第二遍成员**。
//!
//! **本档手里唯一的新东西**是三格：
//! 1. 「用户打开」这一格第一次有了被打开的东西 —— 一扇由 Rust 决定存在与否的窗
//!    （[`crate::session::qobject::Session::group_window_open`]，与 `settings_open` / `custom_open` /
//!    `history_window_open` 同一副骨架：谁造这扇窗，销毁就归 `main.qml`）；
//! 2. 规则 1（第 1243 行）自此有了**页上那一问**：窗上的「删除」不直接递 `destroy`，它先亮出
//!    [`DELETE_ASK`] 那一句，真人点两枚答钮之一才轮到产品收到那枚布尔。产品那枚 `bool` 与页上
//!    那一问是**两把闸**而不是同一把闸写两遍；取消那一臂递回一句实话而不是把这一次点击吞掉。
//!    两枚答钮的名字住在本档（[`KEEP_WORD`] / [`DESTROY_WORD`]），而 `session.rs` 里那句成功话
//!    **也读这两枚常数** —— 一处写「随组销毁」另一处写「随组销毁」就是两个作者（#281）。
//! 3. §5.12.4 第 1251 行那三个入口里的**两个**（托盘菜单、分组面板）自此有对面；第三个
//!    「快捷键」仍归 `switch_group` 那一枚未接线的动作名，本轮不装（见台账 ⑫ 那一格）。
//!
//! **它不碰用户的一字节**（§9.1 第十五条）：分组与贴图**不住在 `config.toml` 里**
//! （`crates/core/src/config.rs` 第 1493–1494 行「groups 不住 config.toml、没有键供它们过档」），
//! 本档种的三组三枚贴图全在进程私有的 `PinSet` 内存里，门打完结散；它一次也不叫 `write_back`，
//! 也不像 `--history-window` 那样需要钉一枚沙盒落点 —— 没有落点可钉才是这一档的实话，
//! 那句话由门外的尺量用户 `config.toml` 的 sha 来证，而不是靠这一句注释。

use std::sync::{Mutex, OnceLock, PoisonError};

use crate::state::Check;

/// PRD 第 1218 行那一串字，**窗标题的唯一作者**。这一档把它当判据的一部分而不只是措辞：那一行
/// 写的不是「分组管理」也不是「图片分组」，而是「图片分组管理」六个字；门里 `open` 那一格判的
/// 正是「页上那扇窗的 title 是不是这一串」。§5.12.3 第 1236 行那句「分组管理页」要的那一页，
/// 在盘上就是这一扇窗，两处 PRD 共用一个对面。
pub const TITLE: &str = "图片分组管理";

/// §5.18.1 第 1437 行那一列菜单建议里的写法就是「图片分组」四个字，**托盘那一项的名字唯一作者**
/// 是这一枚常数：[`crate::hotkey::Work::Groups`] 的 `label()` 读它，门里 ⑩ 那一格也判它。
/// 窗标题（[`TITLE`]）与菜单名分家是有意的两件事 —— 菜单要说「这里是分组」，窗标题要念的是
/// PRD 第 1218 行给那一页起的名。
pub const TRAY_WORD: &str = "图片分组";

/// 页上那六枚钮的名字，加窗自己那一枚「关闭」：次序就是 §5.12 那四节说话的样子 ——
/// 新建（第 1219 行）、重命名 / 上移 / 下移 / 删除（第 1238 行那三件，排序拆成两臂）、
/// 切换显隐（第 1252 行）、关闭。名单住在这一档而不长在 `.qml` 里，与 [`crate::history_window::BUTTONS`]
/// 同一理由：页上抄一份名单，改一天「钮上写着删除、递进产品的却是重命名」只能靠数出来（#281）。
pub const BUTTONS: [&str; 6] = ["新建分组", "重命名", "上移", "下移", "切换显隐", "删除"];

/// 页上那一列「关闭」那一枚：它与上面六枚不是一族（六枚改的是模型，这一枚改的是窗自己在不在），
/// 所以不混进 [`BUTTONS`] 让门数错 delegate 的枚数。
pub const CLOSE_WORD: &str = "关闭";

/// 每行右边那一句「这一组此刻显不显」（第 1252 行「显示、隐藏」在两态上的读数）。两个词都由
/// 本档递出去：页上写死一份，`group_shown` 与界面那一眼就有两套词汇。
pub const SHOWN_WORD: &str = "显示中";
pub const HIDDEN_WORD: &str = "已隐藏";

/// 规则 1（第 1243 行「删除非空分组时，应询问贴图如何处理」）在**页上那一半**的形状：
/// 一句问，两枚答。答钮的两串字同时是产品成功话里的那两串（见本档头顶第 2 格），
/// 所以「钮上写的」与「产品说的」自始至终是一个名。
pub const DELETE_ASK: &str = "这一组里还有贴图，删除时它们怎么处理？";
pub const KEEP_WORD: &str = "退回未分组";
pub const DESTROY_WORD: &str = "随组销毁";

/// 那一问的**第三**枚答：不删。它与上面两枚归宿不同族 —— 两枚归宿递进产品的是一枚 `bool`，
/// 这一枚什么都不递（页上把问收起来，一次调用也没有）。门里 ⑧ 那一格判的正是这一臂：
/// 「按下了删除而一次也没答」之后清单少一行就是页自己删了东西。
///
/// 名字与 `history_window::CONFIRM_NO` 同为「取消」是有意的两件事而不是抄了一份：两扇窗各是
/// 一处页面上的字（与 [`CLOSE_WORD`] 对 `history_window::BUTTONS[3]` 同一先例），
/// 要防的是**同一页上**两处作者，而不是全工程只许有一枚「取消」。
pub const CANCEL_WORD: &str = "取消";

/// 本档自己给新分组起的名（量具那几条腿的输入）。`甲乙丙` 而不是「测试分组 1」：这一趟种的是
/// 三枚**角色**不同的组（两枚有贴图、一枚空），名字要能在报告里点名它是哪一枚角色。
const PLANTED: [&str; 3] = ["甲组（两枚）", "乙组（一枚）", "丙组（空）"];

/// 门给新建那一格起的名（§5.12.1 步骤 3「输入分组名称」的对面）。
pub const NEW_NAME: &str = "丁组（新建）";

/// 改名那一格的第二个名（§5.12.3 步骤 3「执行删除、重命名或拖动排序」里的重命名那一件）。
pub const RENAMED: &str = "丁组（已改名）";

/// 量具那几条腿之间隔多久。与 [`crate::history_window::LEG_MS`] 同一枚理由：分组那六只手全是
/// 同步过桥的一问一答，不等动画。它是本档自己的读数，印在 `[group-window wait]` 那一行。
pub const LEG_MS: u64 = 120;

/// 种下的组数与贴图枚数：三组（两枚有贴图、一枚空）与三枚贴图（甲两枚、乙一枚、丙没有）。
/// 空的那一枚是 §5.12.1 步骤 4「保存后创建空分组」在门里的**第二**证人，不只是新建那一格。
pub const GROUPS: usize = 3;
pub const PINS: usize = 3;

/// 闹钟地板：造窗 + 十条腿过一次桥 + 三贴图各自的窗 + 销毁。与
/// [`crate::history_window::MIN_WINDOW_MS`] 同量级。
pub const MIN_WINDOW_MS: i32 = 6_000;

/// 窗递一行回来的**那一刻**，模型自己那几格读数。
///
/// 为什么要在递的那一刻抄，而不是在 `measure` 里重读：`measure` 跑在十条腿之后，那时 ⑥ 藏下去
/// 的两枚早被 ⑦ 翻回来了、⑩ 销毁的那一枚已经不在集里、⑤ 挪过的那一行后面又被 ⑨⑩ 删掉两组
/// —— 事后重读答的是「最后对不对」，而不是「那一拍对不对」。这一份快照让 ⑤⑥⑦⑨⑩ 五格各多一个
/// 证人：页上说的数与模型在同一拍说的数必须一样。
///
/// （前一版 ⑤ 那格就是在 `measure` 里现读 `order_text()` 的：跑到那一步时清单已被 ⑨⑩ 删掉两组、
/// 只剩两行，而页递回来的是挪过那一刻的四行次序，两串永远对不上 —— 抓到它的正是上面那句「事后
/// 重读答的不是那一拍」，而不是真机门里那一格的红。）
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snap {
    /// 三枚贴图（甲两枚、乙一枚）此刻各自的 `live`。
    live: [bool; PINS],
    /// 同一时刻各自还在不在集里 —— ⑨ 与 ⑩ 那一问两臂的分界就在这里。
    held: [bool; PINS],
    /// 同一时刻 core 那份清单的次序（[`order_text`] 的原样）。⑤ 那格比的就是它。
    order: String,
}

struct GateLines {
    /// 窗递回来的每一行原文。
    lines: Vec<String>,
    /// 与 `lines` 同序的模型快照（[`report`] 每次抄一份）。
    snaps: Vec<Option<Snap>>,
}

/// 这一档在这一趟里记住的东西。与 [`crate::history_window`] 的 `Gate` 同一分工：
/// 计划在前、读数在后，而 `measure` 只读窗递回来的那些行。
struct Gate {
    /// 这一趟有没有一份计划。`false` 时 `measure` 交回 BLOCKED 而不是 FAIL —— 那一格问的是
    /// 「这一趟是产品会话还是本档」，而不是「页上少了一行」。
    armed: bool,
    /// 没 armed 的那一句理由，原样印进报告。
    reason: String,
    /// `腿等待 ms|甲组号|乙组号|丙组号|贴图A|贴图B|贴图C`（7 段）。
    plan: String,
    /// 窗递回来的每一行原文 + 同序的模型快照。
    got: GateLines,
    /// 三枚组的编号，次序与 [`PLANTED`] 同序。
    gids: [u64; GROUPS],
    /// 三枚贴图的编号：前两枚在甲组、第三枚在乙组。
    pids: [i64; PINS],
}

fn gate() -> &'static Mutex<Gate> {
    static G: OnceLock<Mutex<Gate>> = OnceLock::new();
    G.get_or_init(|| {
        Mutex::new(Gate {
            armed: false,
            reason: String::new(),
            plan: String::new(),
            got: GateLines {
                lines: Vec::new(),
                snaps: Vec::new(),
            },
            gids: [0; GROUPS],
            pids: [0; PINS],
        })
    })
}

fn with<R>(f: impl FnOnce(&mut Gate) -> R) -> R {
    let mut g = gate().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut g)
}

/// 窗标题（页上 `title:` 绑的就是这一枚，桥是唯一作者）。
pub fn title() -> String {
    TITLE.to_string()
}

/// 六枚钮的名字，`|` 连接 —— 与 `history_window::buttons()` 同一把刀（名字里没有 `|`，
/// 页上只需要 `split("|")`）。
pub fn buttons() -> String {
    BUTTONS.join("|")
}

/// 「显示中 / 已隐藏」那两串字，按 [`SHOWN_WORD`]、[`HIDDEN_WORD`] 次序 `|` 连成一串。
pub fn state_words() -> String {
    [SHOWN_WORD, HIDDEN_WORD].join("|")
}

/// 删除那一问与三枚答钮的四串字，按 [`DELETE_ASK`]、[`KEEP_WORD`]、[`DESTROY_WORD`]、
/// [`CANCEL_WORD`] 次序。第四枚不是第三种归宿：它什么都不递（见 [`CANCEL_WORD`] 头顶那封信）。
pub fn delete_words() -> String {
    [DELETE_ASK, KEEP_WORD, DESTROY_WORD, CANCEL_WORD].join("|")
}

/// core `groups` 那个 vec 本体此刻的次序，编码成 `id,id,id`。门里 ⑤ 那一格把它与**页上**
/// 递回来的那一串逐位对 —— 两侧各读各的（一侧过桥、一侧直接 [`crate::state::PinState::group_rows`]），
/// 桥侧重排一次就露馅。分隔符用 `,` 而不用 `|`：`|` 是本档每行报告的字段刀，
/// 一串次序里再混进它是把两个口径搅在一起。
pub fn order_text() -> String {
    crate::state::with(|s| {
        s.group_rows()
            .iter()
            .map(|g| g.0.to_string())
            .collect::<Vec<String>>()
            .join(",")
    })
}

/// 门给新建与改名两格要用的两名，`|` 连起来。页上不写死中文，也不自己拼一枚名 ——
/// 那两名由本档起，门的 `create` / `rename` 两格判的就是它们**原样**出现在页上那一列里。
pub fn planted_names() -> String {
    [NEW_NAME, RENAMED].join("|")
}

/// 窗在 `Component.onCompleted` 读的那一串。空串 = 这一趟没有量具的腿要跑（产品会话里真人自己点，
/// 另外几道门里整条躲开 —— 量具的代码不许长成用户看得见的一次点击）。
pub fn plan_line() -> String {
    with(|g| g.plan.clone())
}

/// 这一趟该不该有一扇「图片分组管理」窗 —— `main.qml` 那一条 `syncGroup` 唯一的读数，
/// 与 [`crate::history_window::window_wanted`] 同一分工。
pub fn window_wanted() -> bool {
    with(|g| g.armed)
}

/// 窗递回来的一行。前缀之后那一段由 [`rows_of`] 挑出来。
///
/// 递的那一刻顺带替本档抄一份模型自己的读数（[`Snap`]），所以页上说的数与模型当时说的数
/// 在门里对得上；抄在 `report` 里而不是 `measure` 里的理由见 [`Snap`] 头顶那封信。
/// 先取快照再上锁：`state::with` 与 [`with`] 各持一把锁，把顺序钉死在这一处，
/// 免得日后有人在模型的锁里喊这句。
pub fn report(line: &str) {
    let snap = snapshot();
    with(|g| {
        g.got.lines.push(line.to_string());
        g.got.snaps.push(snap);
    });
}

pub fn seen() -> Vec<String> {
    with(|g| g.got.lines.clone())
}

/// 同一个前缀递回来的那几段（每一段前缀之后的全部内容），按递上来的次序。
fn rows_of(prefix: &str) -> Vec<String> {
    with(|g| {
        g.got
            .lines
            .iter()
            .filter_map(|l| l.split_once('|'))
            .filter(|(k, _)| *k == prefix)
            .map(|(_, v)| v.to_string())
            .collect()
    })
}

/// 那一拍模型的读数。`counted` 已经保证这一格只递过一次，所以这里取的是**唯一**那一枚的配对快照；
/// 取不到（行与快照不同序）就当 `None`，格子当场红 —— 本档缺一个证人时不许把那一格当成对上。
///
/// 分前缀用 `split('|').next()` 而不是 [`rows_of`] 那枚 `split_once`：`born` 那一行一个字段也没有，
/// 却照样有一拍要抄。本档从不按 `born` 取值，两把刀在这一格上读得出同一件事。
fn snap_of(prefix: &str) -> Option<Snap> {
    with(|g| {
        let hits: Vec<usize> = g
            .got
            .lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                if l.split('|').next()? == prefix {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        if hits.len() != 1 {
            return None;
        }
        g.got.snaps.get(hits[0]).cloned().flatten()
    })
}

/// 同一格只该被答一遍：递上来两枚就是页上有两处跑了同一条腿，而本档读的是第一枚 ——
/// 让它悄悄数第一枚，第二枚就永远读不到。
fn counted(rep: &mut crate::r13::Report, prefix: &str) -> Option<String> {
    let got = rows_of(prefix);
    if got.len() != 1 {
        rep.row(
            prefix,
            Check::Fail,
            format!("这条腿递了 {} 枚（应当一枚）", got.len()),
        );
        return None;
    }
    got.into_iter().next()
}

/// 计划里那一段读成一枚编号。读不出来 ⇒ `-1`，那一枚在集里压根不存在，于是产品交回的是
/// 「没有这组 / 没这枚贴图」而不是把判据蒙过去。
fn seg(plan: &[&str], i: usize) -> i64 {
    plan.get(i)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(-1)
}

/// 从产品那句「已删除分组：N 枚贴图随组销毁」里读出那两样。读不出来 ⇒ `None`，格子当场红 ——
/// 把一句读不出来的话当成一次成功的删除，比一次失败的删除更坏。
///
/// 数完贴图枚数还要**点名它的归宿**：`退回未分组` 与 `随组销毁` 是两件事，门里 ⑦ 与 ⑧ 两格
/// 判的就是它们各归各（读反了就是把销毁读成退回）。
fn said_fate(said: &str) -> Option<(usize, bool)> {
    let head = said.split("已删除分组：").nth(1)?;
    let digits: String = head.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let tail = &head[digits.len()..];
    let destroy = if tail.starts_with(" 枚贴图") && tail.contains(DESTROY_WORD) {
        true
    } else if tail.starts_with(" 枚贴图") && tail.contains(KEEP_WORD) {
        false
    } else {
        return None;
    };
    Some((digits.parse::<usize>().ok()?, destroy))
}

/// 一枚贴图此刻在不在场上。走的是 [`crate::state::PinState::view_data`] 那一格 `live`
/// —— 它与 `PinWindow.qml` 里那一眼绑的是同一格（第 1253 行「统一更新该分组内贴图的可见状态」
/// 在界面上的落点就是这一枚 `live`），本档不数第二遍。
fn live_of(id: i64) -> bool {
    crate::state::with(|s| s.view_data(id as u64).live)
}

/// 集里还有没有那一枚贴图（销毁与退回的**唯一**分界：退回的还在集里、销毁的不在了）。
fn held_of(id: i64) -> bool {
    crate::state::with(|s| s.set.get(id as u64).is_some())
}

/// 模型此刻对那三枚贴图的读数，外加 core 清单此刻的次序（[`Snap`]）。
///
/// 没 armed 时交回 `None` 而不是「全 false」：产品会话与另外几道门里本档一枚贴图也没种，
/// 那三枚编号是 0 —— 拿 0 去问模型会答出三个假，而三个假与页上说的真对不上就是门自己造假证人。
/// 先解锁再问模型，与 [`report`] 头顶那句同一枚顺序。
fn snapshot() -> Option<Snap> {
    let (armed, pids) = with(|g| (g.armed, g.pids));
    if !armed {
        return None;
    }
    Some(Snap {
        live: [live_of(pids[0]), live_of(pids[1]), live_of(pids[2])],
        held: [held_of(pids[0]), held_of(pids[1]), held_of(pids[2])],
        order: order_text(),
    })
}

/// 在**引擎之前**种下三组三枚贴图、把六枚编号写进计划。答的是这一趟印出来的那句话；
/// 前置摆不出来 ⇒ `armed = false` 而 `reason` 是那句原话 —— 计划保持空串，窗上一条腿也不跑，
/// `measure` 就此交回 BLOCKED 而不是拿不存在的编号去删。
pub fn arm() -> String {
    let mut gids = [0u64; GROUPS];
    let mut pids = [0i64; PINS];
    for (i, name) in PLANTED.into_iter().enumerate() {
        match crate::state::with(|s| s.create_group(name)) {
            Ok(id) => gids[i] = id,
            Err(e) => {
                with(|g| {
                    g.armed = false;
                    g.reason = format!("{name}：{e}");
                });
                return gate_reason();
            }
        }
    }
    // 三枚贴图各落在桌面上三个不同的点，为的是「隐藏整组」之后剩下的那一枚不与被藏的那两枚
    // 叠在同一处 —— 叠起来的三枚在顶层窗普查里答不清是谁。尺寸取 `state::demo_frame`，
    // 与 `pin_check` 那一档同一只手（本档不另拼一份像素）。
    let spots = [(24i32, 24i32), (300, 24), (576, 24)];
    for (i, (x, y)) in spots.into_iter().enumerate() {
        match crate::state::with(|s| {
            s.add(
                crate::state::demo_frame(96, 72),
                falcon_core::PhysPoint::new(x, y),
            )
        }) {
            Ok(id) => pids[i] = id as i64,
            Err(e) => {
                with(|g| {
                    g.armed = false;
                    g.reason = format!("第 {} 枚贴图种不下：{e}", i + 1);
                });
                return gate_reason();
            }
        }
    }
    // 归属：甲组两枚、乙组一枚、丙组零枚。走的是 core 那枚 `assign_group`，桥侧这一档是本 round
    // 新接的那只手（[`crate::state::PinState::assign_group`]），它不在页上——页上那一只手是
    // `moveSelectionToGroup`，读的也是同一枚 core 手。
    //
    // **逐段点名**而不是「取前 take 枚」：前一版写的是 `pids[..take]`，于是乙组拿到的是甲组的
    // 第一枚 —— `assign_group` 不是「加入」而是「改归属」，那一次调用把这一枚从甲组**搬走**了。
    // 结果清单上甲只剩一枚、乙那一枚本不该在乙、第三枚一次也没进过组，而 `arm` 自己那两行
    // `n == ids.len()` 全都答「对」：它数的是「这一手交回几枚」，不是「这一组此刻几枚」。
    // 抓到它的是单测 `the_plan_names_the_groups_and_pins_it_planted` 那句 `rows[0].3 == 2`。
    let wants = [(0usize, 0usize..2usize), (1, 2..3), (2, 3..3)];
    for (group, take) in wants {
        let ids: Vec<u64> = pids[take].iter().map(|p| *p as u64).collect();
        if ids.is_empty() {
            continue;
        }
        match crate::state::with(|s| s.assign_group(&ids, Some(gids[group]))) {
            Ok(n) if n == ids.len() => {}
            Ok(n) => {
                with(|g| {
                    g.armed = false;
                    g.reason = format!(
                        "第 {} 组要移 {} 枚而 core 只收了 {n} 枚",
                        group + 1,
                        ids.len()
                    );
                });
                return gate_reason();
            }
            Err(e) => {
                with(|g| {
                    g.armed = false;
                    g.reason = format!("第 {} 组移不进去：{e}", group + 1);
                });
                return gate_reason();
            }
        }
    }
    // 前置的最后一道：种完**自己数一遍**。上面那两行 `n == ids.len()` 答的是「这一手交回几枚」，
    // 答不出「这一组此刻几枚」—— 而门里 ① 与 ⑥⑦ 那三格要的是后者。把一份没数过的摆法递给窗，
    // 等于让窗去替本档发现前置错了（那一格会红，但红的是页，不是这一处真写错的地方）。
    //
    // 数的是**本档自己那三枚编号**（按 `pins_in_group` 逐组现问），不是「清单上此刻有几组」：
    // 单测并行跑在同一份 `PinState` 上，别的格子也在那里种组（第一趟读回 `[2,1,0,2,1]`，
    // 后面那两行是别人的），拿整张表比对本档的三名就是让兄弟单测来决定这一格红不红。
    let counts: Vec<usize> =
        crate::state::with(|s| gids.iter().map(|g| s.set.pins_in_group(*g).len()).collect());
    let want_counts = [PINS - 1, 1, 0];
    if counts != want_counts {
        with(|g| {
            g.armed = false;
            g.reason = format!("摆出来的成员数 {counts:?} 不是 {want_counts:?}");
        });
        return gate_reason();
    }
    with(|g| {
        g.armed = true;
        g.reason = String::new();
        g.gids = gids;
        g.pids = pids;
        g.plan = format!(
            "{LEG_MS}|{}|{}|{}|{}|{}|{}",
            gids[0], gids[1], gids[2], pids[0], pids[1], pids[2]
        );
    });
    format!(
        "种下 {} 组（甲 {} 枚 / 乙 {} 枚 / 丙 0 枚）与 {} 枚贴图，编号 {}|{}|{}|{}|{}|{}",
        GROUPS, 2, 1, PINS, gids[0], gids[1], gids[2], pids[0], pids[1], pids[2]
    )
}

fn gate_reason() -> String {
    with(|g| {
        if g.reason.is_empty() {
            "理由没写".to_string()
        } else {
            g.reason.clone()
        }
    })
}

/// 报告头那一格：这一趟到底是不是本档。
///
/// 十条腿逐条判，判的全是**窗那一侧**递回来的话；本档手里那份模型只用来对（它种下的编号），
/// 不用来说（与 `--history-window` 同一分工）。窗一行也没递回来 ⇒ BLOCKED：那一格问的是
/// 「这一台机器上有没有那扇窗」，而 QML 文档编译不过与页上少一格是两种坏。
pub fn measure(want_ms: i32) -> (Check, String) {
    use crate::r13::Report;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (armed, reason, plan) = with(|g| (g.armed, g.reason.clone(), g.plan.clone()));
    if !armed {
        rep.row(
            "group window",
            Check::Blocked,
            format!("这一趟没有 {want_ms} ms 的计划：{reason}"),
        );
        return rep.finish("group-window door");
    }
    if seen().is_empty() {
        rep.row(
            "group window",
            Check::Blocked,
            "窗没有递回任何一行 —— 「窗压根没被造出来」与「造出来了而腿没跑」在这一趟还没分开过；\
             分界是页上那行 `born`（见 `GroupWindow.qml`）"
                .to_string(),
        );
        return rep.finish("group-window door");
    }
    let p: Vec<&str> = plan.split('|').collect();
    let (ga, gb, gc, pa, pb, pc) = (
        seg(&p, 1),
        seg(&p, 2),
        seg(&p, 3),
        seg(&p, 4),
        seg(&p, 5),
        seg(&p, 6),
    );

    // ① 第 1218 行「用户打开“图片分组管理”」与第 1236 行「分组管理页」那一格的全部对面：
    //    窗在不在、标题是不是那一串字、页上真的造出几枚钮、这一趟列出几行。`visible` 是窗自己
    //    问 Qt 的那一枚数而不是「我们叫过 show()」，`btns` 是 delegate 的 `count` 而不是名单长度。
    //    尾两枚是**页自己数的那两行的成员数**（甲组几枚、丙组几枚）：本档在 `arm()` 里数过一遍
    //    摆法，而这一格要的是「页看见的摆法」与它同一枚 —— 第 1236 行「显示分组列表（名称、
    //    缩略图、图片数量）」里那枚数量第一次有了对面的读数。
    if let Some(open) = counted(&mut rep, "open") {
        let s: Vec<&str> = open.split('|').collect();
        let visible = s.first().copied().unwrap_or("");
        let btns = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let titled = s.get(2).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let rows = s.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let page_full = s.get(4).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let page_empty = s.get(5).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let want_full = (PINS - 1) as i64;
        rep.row(
            "the page opens",
            if visible == "1" && btns == BUTTONS.len() as i64 && titled == 1
                && rows == GROUPS as i64
                && page_full == want_full
                && page_empty == 0
            {
                Check::Pass
            } else {
                Check::Fail
            },
            format!(
                "visible={visible} 钮 {btns} 枚（应当 {}）、标题对上 {titled}（应当 1）、\
                 页上 {rows} 行（应当 {GROUPS}：{}）；页数的甲组 {page_full} 枚（应当 {want_full}）、\
                 丙组 {page_empty} 枚（应当 0）",
                BUTTONS.len(),
                PLANTED.join(" / ")
            ),
        );
    }

    // ② 第 1219–1221 行「点击“新建分组”→ 输入分组名称 → 保存后创建空分组」：新组真的**空**。
    //    判的是页上那一新行的成员数读到 0，而不是 Rust 说「我建了一枚」。编号也不许是种下的那三枚
    //    之一：「新建」如果是把已有的一组读了回来，页上那一行照样是空的、照样能骗过 `count == 0`。
    //    这一枚编号留到 ⑤ 用：那一腿挪的必须是新建出来的这一组。
    let mut made_id = -1i64;
    if let Some(created) = counted(&mut rep, "create") {
        let s: Vec<&str> = created.split('|').collect();
        let id = s.first().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        made_id = id;
        let count = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let named = s.get(2).map(|v| v.to_string()).unwrap_or_default();
        let rows = s.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let fresh = id != ga && id != gb && id != gc;
        rep.row(
            "create an empty group",
            if id > 0 && fresh && count == 0 && named == NEW_NAME && rows == GROUPS as i64 + 1 {
                Check::Pass
            } else {
                Check::Fail
            },
            format!(
                "新建交回编号 {id}（应当 >0 且不是种下的 {ga}/{gb}/{gc} 里任何一枚：{fresh}）、\
                 页上那一行写着 {named:?} 成员 {count} 枚（应当 0 枚）、清单 {rows} 行（应当 {}）",
                GROUPS + 1
            ),
        );
    }

    // ③ 第 1244 行「分组名称不能为空」：页上那一只手交回 -1，而清单**没有**多出一行。
    //    这一格判的是拒绝的那一臂 —— 只测成功就等于没测那条规则。
    if let Some(blank) = counted(&mut rep, "create-blank") {
        let s: Vec<&str> = blank.split('|').collect();
        let rc = s.first().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let rows = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let said = s.get(2).map(|v| v.to_string()).unwrap_or_default();
        rep.row(
            "an empty name is refused",
            if rc == -1 && rows == GROUPS as i64 + 1 && !said.is_empty() {
                Check::Pass
            } else {
                Check::Fail
            },
            format!(
                "空名交回 {rc}（应当 -1）、清单仍是 {rows} 行（应当没多一行）、产品那句 {said:?}"
            ),
        );
    }

    // ④ 第 1238 行「重命名」：改完页上那一行**自己**写着新名，而且写的是产品那句话里的名。
    if let Some(rename) = counted(&mut rep, "rename") {
        let s: Vec<&str> = rename.split('|').collect();
        let rc = s.first().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let shown = s.get(1).map(|v| v.to_string()).unwrap_or_default();
        let said = s.get(2).map(|v| v.to_string()).unwrap_or_default();
        rep.row(
            "rename",
            if rc == 0 && shown == RENAMED && said.contains(RENAMED) {
                Check::Pass
            } else {
                Check::Fail
            },
            format!("改名交回 {rc}（应当 0）、页上那一行是 {shown:?}（应当 {RENAMED:?}）、产品那句 {said:?}")
        );
    }

    // ⑤ 第 1245 行「排序变化应同步反映到快捷切换菜单」：页面上移一次之后，页上的次序与
    //    core `groups` 那个 vec 本体的次序**逐位相同**。这一格判的是「两处一样」，而两处读的
    //    是同一拍 —— 桥侧一重排就露馅（`group_rows` 头顶那句 #281）。
    //
    //    模型那一侧的次序不收页上递来的那一段：让页替本档念模型，量的就成了「页会不会抄」而不是
    //    「两处是否一致」（§9.1 那条「量具不许替被量者说话」）。本档读的是 [`Snap::order`] —— 递
    //    那一行**那一刻**抄下的 core 次序；在 `measure` 里现读的那一串已被 ⑨⑩ 删掉两组，与页递的
    //    四行永远对不上，那是本档自己的读数错了位置，不是产品坏。
    //    第二段是**动之前**的次序，用来钉住「这一手真的挪了一格」——`move_group` 越界时交回
    //    `Ok(())` 而一动不动，只比对两处次序就放过了那一臂。第四段是页挪的哪一组，与 ② 新建回来
    //    的那枚编号对：挪错一组（甚至挪了种下的三组之一）这一格就红。
    if let Some(moved) = counted(&mut rep, "move") {
        let s: Vec<&str> = moved.split('|').collect();
        let rc = s.first().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let page = s.get(1).map(|v| v.to_string()).unwrap_or_default();
        let before = s.get(2).map(|v| v.to_string()).unwrap_or_default();
        let about = s.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        // core 那一串的**挪过那一刻**的次序（[`Snap::order`]），不是 `measure` 跑到的此刻：
        // 十条腿跑完时 ⑨⑩ 已经删掉两组，事后现读的那一串只有两行，永远对不上页递的四行。
        let beat = snap_of("move");
        let model = beat.map(|sn| sn.order).unwrap_or_default();
        let moved_one = !before.is_empty() && before != page;
        let about_ok = about == made_id && made_id > 0;
        let rows_ok = page.split(',').count() == GROUPS + 1;
        rep.row(
            "the order moves together",
            if rc == 0 && moved_one && !page.is_empty() && about_ok && rows_ok && page == model {
                Check::Pass
            } else {
                Check::Fail
            },
            format!(
                "上移交回 {rc}（应当 0）；挪过之前 {before:?} 与之后 {page:?}{}；\
                 页挪的是第 {about} 组（应当是 ② 新建那枚 {made_id}：{about_ok}）；\
                 那一拍的清单应有 {} 行而这一串是 {rows_ok}；core 那一拍的次序 {model:?} 逐位",
                if moved_one { "" } else { " 没挪动" },
                GROUPS + 1
            ),
        );
    }

    // ⑥⑦ 第 1252–1253 行「执行显示、隐藏或切换」与「系统统一更新该分组内贴图的可见状态」。
    //    两臂各判一次：藏下去那一拍，甲组那两枚自己 `live=false` 而**不属于甲组的第三枚（在乙组）
    //    照旧**；翻回来那一拍三枚全在场上。缺第二臂就是只测了一半。
    //
    //    页上那三枚 `live` 是它自己过桥问的（`groupWindowLive`），本档另有一份**同一拍**抄下的
    //    模型读数（[`Snap`]）：两处不一样就是页与模型各说一件事，那一格照样红。
    for (which, want_live) in [("toggle-hide", false), ("toggle-show", true)] {
        let Some(got) = counted(&mut rep, which) else {
            continue;
        };
        let s: Vec<&str> = got.split('|').collect();
        let shown = s.first().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let word = s.get(1).map(|v| v.to_string()).unwrap_or_default();
        let la = s.get(2).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let lb = s.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let lc = s.get(4).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        // 页交回来的那四枚编号：它按的是哪一组、问的是哪三枚。与本档写进计划的那四枚逐个对 ——
        // 少了这一步，页去点乙组、读甲组，这一格也会绿（它绿的是别人家的行为）。
        let about = s.get(5).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let q_a = s.get(6).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let q_b = s.get(7).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let q_c = s.get(8).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let about_ok = about == ga && q_a == pa && q_b == pb && q_c == pc;
        let want_shown = i64::from(want_live);
        let want_model = [want_live, want_live, true];
        let snap_live = snap_of(which).map(|sn| sn.live);
        let snap_ok = snap_live == Some(want_model);
        let ok = shown == want_shown
            && word == if want_live { SHOWN_WORD } else { HIDDEN_WORD }
            && la == i64::from(want_live)
            && lb == i64::from(want_live)
            && lc == 1
            && about_ok
            && snap_ok;
        rep.row(
            if want_live { "show takes them back" } else { "hide covers the whole group" },
            if ok { Check::Pass } else { Check::Fail },
            format!(
                "切换交回显不显 = {shown}（应当 {want_shown}）、页上那行写 {word:?}；\
                 甲组那两枚 live = {la}/{lb}（应当 {}）、乙组那枚 = {lc}（应当 1：它不在这组里）；\
                 页点的组 {about} 问的三枚 {q_a}/{q_b}/{q_c}（应当 {ga} 与 {pa}/{pb}/{pc}：{about_ok}）；\
                 本档在那一拍抄下的模型 live = {snap_live:?}（应当 {:?}）",
                i64::from(want_live),
                want_model
            ),
        );
    }

    // ⑧ 规则 1（第 1243 行）的第一臂：按下「删除」而**没答**那一问 ⇒ 产品一次也没收到。
    if let Some(asked) = counted(&mut rep, "delete-asked") {
        let s: Vec<&str> = asked.split('|').collect();
        let asking = s.first().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let rows = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let held = s.get(2).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let about = s.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let about_ok = about == ga;
        rep.row(
            "the ask comes before the cut",
            if asking == 1 && rows == GROUPS as i64 + 1 && held == 1 && about_ok {
                Check::Pass
            } else {
                Check::Fail
            },
            format!(
                "那一问亮着 {asking}（应当 1）、清单仍是 {rows} 行（应当 {}）、甲组还在场上 {held}\
                 （应当 1）—— 没答之前一枚也不许少；页按下去的那一组是 {about}（应当 {ga}：{about_ok}）",
                GROUPS + 1
            ),
        );
    }

    // ⑨⑩ 那一问的两臂。`held` 是**这一枚编号还在不在集里**：退回的还在、销毁的没了，
    //    这正是第 1243 行「贴图如何处理」要分的两件事。`live` 是页上现读的那一眼。
    //    两臂也各对一份本档在那一拍抄下的模型快照：⑨ 之后三枚都还在集里、⑩ 之后第三枚没了 ——
    //    页上说「退回了」而模型说「销毁了」是这一档最坏的一种对上。
    if let Some(ungroup) = counted(&mut rep, "delete-ungroup") {
        let s: Vec<&str> = ungroup.split('|').collect();
        let said = s.first().map(|v| v.to_string()).unwrap_or_default();
        let rows = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let la = s.get(2).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let live_a = s.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let about = s.get(4).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let q = s.get(5).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let about_ok = about == ga && q == pa;
        let snap = snap_of("delete-ungroup");
        let want_held = [true, true, true];
        let want_live = [true, true, true];
        let snap_ok = snap
            .as_ref()
            .map(|sn| sn.held == want_held && sn.live == want_live)
            == Some(true);
        match said_fate(&said) {
            Some((n, false)) => rep.row(
                "pins go back, not away",
                if n == 2 && rows == GROUPS as i64 && la == 1 && live_a == 1 && about_ok && snap_ok {
                    Check::Pass
                } else {
                    Check::Fail
                },
                format!(
                    "确认后递回：{said:?}（应当 2 枚退回）｜清单 {rows} 行（应当 {GROUPS}）、\
                     那两枚仍在集里 {la}（应当 1）、此刻 live = {live_a}（应当 1）；\
                     页删的是第 {about} 组、问的是 {q}（应当 {ga} 与 {pa}：{about_ok}）；\
                     本档在那一拍抄的模型 held = {:?} live = {:?}（应当 {want_held:?} / {want_live:?}）",
                    snap.as_ref().map(|sn| sn.held),
                    snap.as_ref().map(|sn| sn.live)
                ),
            ),
            other => rep.row(
                "pins go back, not away",
                Check::Fail,
                format!("产品那句话读不出归宿与枚数：{other:?} ← {said:?}"),
            ),
        }
    }
    if let Some(destroy) = counted(&mut rep, "delete-destroy") {
        let s: Vec<&str> = destroy.split('|').collect();
        let said = s.first().map(|v| v.to_string()).unwrap_or_default();
        let rows = s.get(1).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let held = s.get(2).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let live = s.get(3).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let about = s.get(4).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let q = s.get(5).and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1);
        let about_ok = about == gb && q == pc;
        let snap = snap_of("delete-destroy");
        let want_held = [true, true, false];
        let want_live = [true, true, false];
        let snap_ok = snap
            .as_ref()
            .map(|sn| sn.held == want_held && sn.live == want_live)
            == Some(true);
        match said_fate(&said) {
            Some((n, true)) => rep.row(
                "pins go with the group",
                if n == 1
                    && rows == (GROUPS - 1) as i64
                    && held == 0
                    && live == 0
                    && about_ok
                    && snap_ok
                {
                    Check::Pass
                } else {
                    Check::Fail
                },
                format!(
                    "确认后递回：{said:?}（应当 1 枚随组销毁）｜清单 {rows} 行（应当 {}）、\
                     那一枚还在集里 {held}（应当 0）、此刻 live = {live}（应当 0）；\
                     页删的是第 {about} 组、问的是 {q}（应当 {gb} 与 {pc}：{about_ok}）；\
                     本档在那一拍抄的模型 held = {:?} live = {:?}（应当 {want_held:?} / {want_live:?}）",
                    GROUPS - 1,
                    snap.as_ref().map(|sn| sn.held),
                    snap.as_ref().map(|sn| sn.live)
                ),
            ),
            other => rep.row(
                "pins go with the group",
                Check::Fail,
                format!("产品那句话读不出归宿与枚数：{other:?} ← {said:?}"),
            ),
        }
    }

    // ⑪ §5.18.1 第 1437 行那枚「图片分组」：判的是**装配出来的那份菜单**里有它，而不是
    //    `Work::Groups` 这一枚枚举存在。菜单的唯一作者是 `hotkey::tray_menu`，本档只问它要一份名字。
    let menu = crate::hotkey::tray_labels();
    let has = menu.iter().any(|name| name == TRAY_WORD);
    rep.row(
        "tray 图片分组",
        if has { Check::Pass } else { Check::Fail },
        format!(
            "菜单 {} 项：{}（找的是「{TRAY_WORD}」那一枚原名）",
            menu.len(),
            menu.join(" / ")
        ),
    );

    rep.note("计划", plan);
    rep.note("窗递回的行", format!("{} 枚", seen().len()));
    for line in seen() {
        rep.note("", line);
    }
    rep.finish("group-window door")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这一族改的是**同一份全局模型**（`crate::state` 那一枚 `PinState`）与**同一枚全局 gate**，
    /// 并行跑就是互相顶：本 round 第一趟读回 `arm` 数到兄弟那格种下的组（`[2,1,0,2,1]`），
    /// 而 `report` 递上来的行被另一格收尾时的 `lines.clear()` 抹掉，快照读成 `None`。
    /// 串行一把锁是这一族唯一的实话 —— 与 `pin_check.rs` 那一族「一扇门一份全局状态」同一分工；
    /// 只有两枚不碰模型也不碰 gate 的字面单测（`the_two_fates_are_read_out_of_the_products_own_sentence`
    /// 与 `the_words_the_page_shows_have_one_author`）不取这枚锁。
    static SHARED: Mutex<()> = Mutex::new(());

    fn shared() -> std::sync::MutexGuard<'static, ()> {
        SHARED.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 「已删除分组：N 枚贴图随组销毁」与「…退回未分组」两臂读得出、别的读不出。
    /// 第三臂**必须**是 `None`：一句读不出归宿的话被当成成功，比一次失败的删除更坏 ——
    /// 门里那两格正是靠这一枚分岔的。
    #[test]
    fn the_two_fates_are_read_out_of_the_products_own_sentence() {
        assert_eq!(
            said_fate("已删除分组：2 枚贴图退回未分组"),
            Some((2, false))
        );
        assert_eq!(said_fate("已删除分组：1 枚贴图随组销毁"), Some((1, true)));
        assert_eq!(said_fate("已删除分组（组里没有贴图）"), None);
        assert_eq!(said_fate("分组名不能为空"), None);
        // 归宿两串都不写的，不配被读成任何一臂。
        assert_eq!(said_fate("已删除分组：3 枚贴图"), None);
    }

    /// 窗上那些名**一处一个作者**：钮名单、状态两词、那一问与两答都从本档递出，
    /// 而本档自己的字面不许与 §5.12 / §5.18.1 的原词分家。这一枚单测不跑桌面。
    #[test]
    fn the_words_the_page_shows_have_one_author() {
        assert_eq!(title(), TITLE);
        assert_eq!(buttons().split('|').count(), BUTTONS.len());
        assert_eq!(state_words().split('|').count(), 2);
        assert_eq!(delete_words().split('|').count(), 4);
        assert_eq!(planted_names().split('|').count(), 2);
        // 名字里不许有 `|`：页上只需要 `split("|")`，混进去就少一枚钮。
        for w in BUTTONS.iter().copied().chain([
            TITLE,
            TRAY_WORD,
            CLOSE_WORD,
            KEEP_WORD,
            DESTROY_WORD,
            CANCEL_WORD,
            SHOWN_WORD,
            HIDDEN_WORD,
        ]) {
            assert!(!w.contains('|'), "{w} 里混进了分隔符");
        }
        assert_eq!(TRAY_WORD, "图片分组");
        assert_eq!(TITLE, "图片分组管理");
    }

    /// 门的前置：种三组三枚、按角色分好，计划七段读回来正是那六枚编号。跑完把这一档钉回
    /// 没 armed 的样子，免得 `window_wanted()` 在后面的单测里替产品会话招出一扇窗。
    #[test]
    fn the_plan_names_the_groups_and_pins_it_planted() {
        let _serial = shared();
        let said = arm();
        assert!(said.contains("种下 "), "{said}");
        assert!(window_wanted(), "{said}");
        let planned = plan_line();
        let p: Vec<&str> = planned.split('|').collect();
        assert_eq!(p.len(), 7, "{p:?}");
        assert_eq!(p[0], LEG_MS.to_string());
        // 上一档写的是 `seg(&p, i) > 0`，那一判有两处错：① `seg` 读不成时交回 `-1`，「读不成」
        // 本来就有人管，`> 0` 不是解析守卫；② 它是一道本档从没许过的要求 —— 全新进程里头一枚
        // 编号就是 0。这一格此前一直绿，绿的是**兄弟单测先在这份 `PinState` 上把计数器顶上去
        // 了**，不是这一格自己种的东西（P173 那条腿把并行顺序挪了一下就现了形）。换成硬一档
        // 的读数：每段都要等于 [`arm`] 那句「编号」后面本档亲手写下的那六枚，0 自此是合法编号，
        // 而「读回来别人的编号」这一档以前放过的走法现在过不去。
        let planted: Vec<&str> = said
            .split("编号 ")
            .last()
            .unwrap_or("")
            .split('|')
            .collect();
        assert_eq!(planted.len(), 6, "「编号」那一句读不成六枚：{said}");
        for i in 1..=6 {
            assert!(seg(&p, i) >= 0, "第 {i} 段读不成编号（{}）：{said}", p[i]);
            assert_eq!(
                p[i],
                planted[i - 1],
                "第 {i} 段不是本档种下的那枚编号：{said}"
            );
        }
        // 甲两枚、乙一枚、丙零枚 —— 与 [`arm`] 里那份 `wants` 同一枚读数，而不是本句自己数。
        // 只挑**这一格种下的那三组**：`group_rows()` 是整张表，而并行跑的兄弟单测也在同一份
        // `PinState` 里种组（第一趟读回五行，多出来的两行是别人的），整张表比三名就是
        // 让兄弟单测决定这一格红不红。
        let mine = {
            let want = [seg(&p, 1) as u64, seg(&p, 2) as u64, seg(&p, 3) as u64];
            crate::state::with(|s| {
                s.group_rows()
                    .into_iter()
                    .filter(|g| want.contains(&g.0))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(mine.len(), GROUPS, "{mine:?}");
        assert_eq!(mine[0].3, 2, "{mine:?}");
        assert_eq!(mine[1].3, 1, "{mine:?}");
        assert_eq!(mine[2].3, 0, "{mine:?}");
        assert!(mine[0].2, "种下时整组该是显示的：{mine:?}");
        with(|g| {
            g.armed = false;
            g.plan = String::new();
            g.got.lines.clear();
            g.got.snaps.clear();
        });
        assert!(!window_wanted());
    }

    /// 第 1253 行那句「统一更新该分组内贴图的可见状态」在**没有窗的一趟**里也判得出来：
    /// 藏下去那一拍组内两枚 `live=false`、组外那一枚照旧。这一枚是本档 `live_of` 那位读者的
    /// 单测证人（与门里 ⑥⑦ 两格同一判据、同一只手）。
    #[test]
    fn hiding_a_group_leaves_the_ungrouped_pin_alone() {
        let _serial = shared();
        let g = crate::state::with(|s| s.create_group("戊组（单测）")).expect("create");
        let inside = crate::state::with(|s| {
            s.add(
                crate::state::demo_frame(64, 48),
                falcon_core::PhysPoint::new(400, 400),
            )
        })
        .expect("plant");
        let outside = crate::state::with(|s| {
            s.add(
                crate::state::demo_frame(64, 48),
                falcon_core::PhysPoint::new(480, 400),
            )
        })
        .expect("plant");
        crate::state::with(|s| s.assign_group(&[inside], Some(g))).expect("assign");
        assert!(live_of(inside as i64) && live_of(outside as i64));
        let shown = crate::state::with(|s| s.toggle_group_shown(g)).expect("toggle");
        assert!(!shown, "切换之后该是藏：{shown}");
        assert!(!live_of(inside as i64), "组内那一枚该跟着藏");
        assert!(live_of(outside as i64), "组外那一枚不该被这一手带走");
        let back = crate::state::with(|s| s.toggle_group_shown(g)).expect("toggle back");
        assert!(back && live_of(inside as i64));
        // 收干净：销毁这一组的成员，再把组删掉，免得这两枚贴图留在后面每一格的普查里。
        crate::state::with(|s| s.assign_group(&[outside], None)).expect("unassign");
        assert!(held_of(outside as i64), "退组那一手不许把成员从集里带走");
        crate::state::with(|s| s.delete_group(g, true)).expect("delete");
        assert!(!held_of(inside as i64), "随组销毁的那一枚该不在集里");
        // `held_of` 问的是「集里还有没有」，所以清理那一臂收的是**集里**那一枚，而不是任何一扇窗：
        // `destroy_pin` 才是桥侧那一只手（`state.rs` 里没有 `delete`，别照着 core 的名字猜）。
        crate::state::with(|s| s.destroy_pin(inside));
        crate::state::with(|s| s.destroy_pin(outside));
        with(|g| {
            g.armed = false;
            g.plan = String::new();
            g.got.lines.clear();
            g.got.snaps.clear();
        });
    }

    /// [`Snap`] 那一份「同一拍的模型读数」自己得有人量：递一行回来时抄的到底是不是**那一拍**的。
    /// 这一格是量具的仪器自检（§9.1 那条「改仪器只许改硬不许改软」的另一半：改硬了的东西
    /// 也要有一句「它真的会红」）。三条各钉一件事：
    /// ① 藏起来不改归属也不毁东西 ⇒ `held` 三枚全真、`live` 只有组外那一枚真；
    /// ② 前缀递两枚 ⇒ 读不到快照，而不是悄悄挑第一份（与 [`counted`] 同一分工）；
    /// ③ 没 armed 时 [`snapshot`] 交回 `None`：产品会话里本档一枚贴图也没种，
    ///    拿编号 0 去问模型会答出三个假，那三个假是不存在的证人；
    /// ④ 同一枚函数在**两只手前后**抄出的 `order` 不一样：删掉一组之前它点名这一组、之后就没了 ——
    ///    ⑤ 那格「页上的次序 vs core 的次序」要的就是这种「各自那一拍」的读数，事后现读会读成另一趟。
    #[test]
    fn the_snapshot_is_the_model_reading_of_that_very_beat() {
        let _serial = shared();
        let g = crate::state::with(|s| s.create_group("戊组（快照单测）")).expect("group");
        let spots = [(620i32, 420i32), (700, 420), (780, 420)];
        let mut ids = [0i64; PINS];
        for (i, (x, y)) in spots.into_iter().enumerate() {
            ids[i] = crate::state::with(|s| {
                s.add(
                    crate::state::demo_frame(64, 48),
                    falcon_core::PhysPoint::new(x, y),
                )
            })
            .expect("plant") as i64;
        }
        crate::state::with(|s| s.assign_group(&[ids[0] as u64, ids[1] as u64], Some(g)))
            .expect("assign");
        // 这一格**不**调 [`arm`]：`arm` 写的是那一份全局计划，而单测并行跑在同一枚 `gate` 上
        // （`the_plan_names_the_groups_and_pins_it_planted` 也在写它）。这一档要的只是 armed 与三枚编号。
        with(|gate| {
            gate.armed = true;
            gate.pids = ids;
            gate.plan = format!("{LEG_MS}|{g}|{g}|{g}|{}|{}|{}", ids[0], ids[1], ids[2]);
            gate.got.lines.clear();
            gate.got.snaps.clear();
        });

        report("born");
        let born = snap_of("born").expect("开工那一拍的快照");
        assert_eq!(born.live, [true, true, true], "{born:?}：三枚都该在场上");
        assert_eq!(born.held, [true, true, true], "{born:?}：三枚都该还在集里");
        // `order` 那一份要点名**这一趟这一拍**清单里的那几组：⑤ 那格比的就是它。这里不写
        // `assert_eq!(born.order, order_text())` —— 那是把「读同一枚函数」当证据（#281 的反面），
        // 判的是快照里有没有这一组这一枚名字，而它正是「事后重读」与「那一拍抄下」的分别所在。
        let named = born.order.split(',').collect::<Vec<_>>();
        assert!(
            named.contains(&g.to_string().as_str()),
            "{born:?}：快照那份次序里没点名这一组"
        );

        crate::state::with(|s| s.toggle_group_shown(g)).expect("toggle");
        report("toggle-hide|0|已隐藏|0|0|1");
        let sn = snap_of("toggle-hide").expect("那一拍的快照");
        assert_eq!(
            sn.live,
            [false, false, true],
            "{sn:?}：组外那一枚不该被带走"
        );
        assert_eq!(sn.held, [true, true, true], "{sn:?}：藏起来一枚也不毁");

        report("toggle-hide|0|已隐藏|0|0|1");
        assert_eq!(
            snap_of("toggle-hide"),
            None,
            "同一条腿递了两枚，本档不挑第一份"
        );

        // 收干净：随组销毁甲那一组的两枚，再摘掉没进组的那一枚。
        crate::state::with(|s| s.delete_group(g, true)).expect("delete");
        // 删完再递一行：这一拍的快照里**没有**这一组，而 `born` 那一份里**有** —— 两份快照出自
        // 同一枚函数、隔着一只手，它们读出不一样，才是「抄在那一拍」而不是「在 measure 里现读」。
        report("group-gone");
        let gone = snap_of("group-gone").expect("删完之后那一拍的快照");
        assert!(
            !gone.order.split(',').any(|id| id == g.to_string()),
            "{gone:?}：这一组已经删了，快照还点名它"
        );
        assert_eq!(
            gone.held,
            [false, false, true],
            "{gone:?}：随组销毁的那两枚该不在集里"
        );
        assert!(
            born.order.split(',').any(|id| id == g.to_string()),
            "{born:?}：开工那一拍的快照读不到这一组"
        );
        for id in ids {
            crate::state::with(|s| s.destroy_pin(id as u64));
            assert!(!held_of(id), "销毁之后集里还留着 {id}");
        }
        with(|gate| {
            gate.armed = false;
            gate.plan = String::new();
            gate.got.lines.clear();
            gate.got.snaps.clear();
        });
        assert_eq!(snapshot(), None, "没 armed 的那一趟不该有证人");
    }
}
