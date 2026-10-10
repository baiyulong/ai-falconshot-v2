//! §10 第 46 项 ③ 的那一扇门：把"揭示"这一步真的开一次窗。
//!
//! 前面五道门都写在 `mask::GAUGE_FLAGS` 里，而 `MaskState::gauge_run` 在
//! [`platform_windows::reveal::select`] 之前把那一步结构性地拒掉。于是
//! `quick save reveal opened a window` 那一格自 P44 起**永远**是 Blocked：它说的话是真的，
//! 但它一次也没有被证伪过——"开关开着会走几次"量得到次数，量不到那一次到底开不开了窗。
//! 这一档不在那张名单里，所以它是这台机器上唯一一条能让 `reveal_acted` 从 0 动起来的自动化
//! 路径，也正是为此它必须写清楚自己欠的那一句代价：
//!
//! **跑完这一档，用户的桌面上会多一个 explorer 的窗。** 本进程举不到它，也就关不掉它；
//! 别的进程的窗只数不碰（第 38 项 ① 那条"探针要能收掉自己留下的东西"，对不归自己管的窗
//! 只能兑现成"别制造"——这里制造了一个，是用户点头之内的），所以这一档不给默认值、不进
//! 跑级梯的默认几趟，只在被单独点名的那一趟里跑。

use std::path::PathBuf;

use crate::mask;
use crate::r13::Report;
use crate::state::Check;

/// 这一档自己的目录：`quick_out` 的命名被指到这里，而不是用户图片文件夹里的一张。
/// 与量具的目录分开，是因为这一趟的落盘会**真开一个窗**，那个窗里显示的东西不该是别人
/// 的截图集合。
///
/// `FALCONSHOT_REVEAL_DOOR_DIR` 是给跑级档用的，不是给用户用的：资源管理器**按目录去重**，
/// `p46_reveal5` 三次普查都读回同一枚 hwnd（`53745054`，shell 自己打印的那个数）、同一个
/// `LocationURL`，因为上一趟
/// 留下的那扇还开着 —— 于是那一趟能证的是"桌面上有一扇指向本档目录的窗"，证不出"这扇窗是
/// 这一趟开出来的"。换一枚每次都不同的目录，前后两次普查的差才是读数而不是推断。只有这一档
/// 读它：别的腿的落点不跟着它动。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_REVEAL_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-reveal-door");
    dir
}

/// 这一档自己的前缀。`Collision::Increment` 加 `%p-%i` 让每一次落盘各得一个名字，所以两趟
/// 之间这个目录里会攒出 `reveal-001.png`、`reveal-002.png`——名字从盘上数出来，不预设。
const PREFIX: &str = "reveal";

/// 这一趟的配置落点，由 `main` 在遮罩开起来**之前**钉下。
///
/// 不钉它就没有"逐字节改回"这一说：`MaskView::close()` 走的是产品自己那句
/// [`crate::settings::write_back`]，遮罩一关就把本轮的笔写回**读来的那份文件**——
/// 而这一趟读来的正是被临时翻成 `reveal_after_save = true` 的那份量具配置。翻档的协议
/// 要求跑完原字节改回，可一轮自己写过的文件改回去就不是那一版了。`annotate_check::plant`
/// 为同一件事钉（§9.1 第十五条），这一档是第二条走产品门的腿，也就欠同一条。
pub fn pin() -> PathBuf {
    // 目录可能比这一趟更老、也可能根本不存在（`FALCONSHOT_REVEAL_DOOR_DIR` 给的是本轮独有的
    // 一枚）；`pin_writes` 不负责替创建者 mkdir，所以这里先把它开出来。
    let _ = std::fs::create_dir_all(door_dir());
    crate::settings::pin_writes(&pinned_path())
}

/// 钉的那个落点，`pin` 与下面那一格各自算一次同一个数——所以它得是函数而不是 `pin` 的返回值。
fn pinned_path() -> PathBuf {
    let mut dir = door_dir();
    dir.push("pinned-config.toml");
    dir
}

/// `Settings::line()` 里那两个字段：读的是哪份、写的是哪份。
fn read_and_write() -> (String, String) {
    let line = crate::settings::with(|s| s.line());
    let field = |name: &str| {
        line.split_whitespace()
            .find_map(|tok| tok.strip_prefix(name))
            .unwrap_or_default()
            .to_string()
    };
    (field("read="), field("write_to="))
}

/// 落盘一次、点一次通知，把两件事各自读回来。`asked_ms` 是这一档被要求等的毫秒数，只印不判。
pub fn measure(asked_ms: i32) -> (Check, String) {
    let mut rep = Report::new();

    // ---- 这条腿得先证明自己不是量具：下面每一格的"没跑"与"被拦"全靠这一格分开。
    let argv: Vec<String> = std::env::args().collect();
    let gauge = mask::MaskState::gauge_run();
    rep.row(
        "reveal door is not a gauge",
        Check::from(!gauge),
        format!(
            "argv {argv:?}；gauge_run 答 {gauge}（要求 false）——那一道结构性拒绝拦的是量具，\
             这一档若也在 GAUGE_FLAGS 里，下面每一格读到的都是自己没跑"
        ),
    );

    // ---- 翻档协议的另一半在这一格：`main` 在遮罩开起来之前钉过写回落点，所以本轮哪怕
    // 走完 `close()` 也不会碰到那份被临时翻成 true 的配置。读与写两个数都从
    // `Settings::line()` 里取——`pin_writes` 只动destination，read 仍是本轮真读的那份。
    let (read, write) = read_and_write();
    let want = pinned_path().display().to_string();
    rep.row(
        "reveal door pinned its settings writes",
        Check::from(write == want && read != write),
        format!(
            "settings line 的 read={read:?} 与 write_to={write:?}（要求后者 == {want:?} 且两者不同）；\
             读只认这两个字段名后面的那一段，路径里有空格时这条读法会断——本轮三份路径都没有"
        ),
    );

    // ---- 先落一张盘，走的是 `MaskState::quick_out`。这一档第一次跑的时候递的是
    // `save_out`（另存为那一腿），于是两格当场红了：文件确实写成了、产品也说了"已保存到
    // …"，可 `landed` 仍是 `None`、两个计数一动不动。那两句红说的是腿而不是产品 ——
    // 通知上那一次点击读的只有 `landed`，而填它的那一步整个产品里只有快速保存有（`mask.rs`
    // 里 `self.landed = Some(…)` 坐在 `quick_out` 内），所以 PRD 492 那句"可点击"必须
    // 对着 `quick_out` 问，对着别的腿问出来的红都是量具自己的。
    let dir = door_dir();
    let made = std::fs::create_dir_all(&dir);
    // 命名换成这一档自己的，只换目的地：`[output] save_dir` 在那份量具配置里是空的，空着
    // 的答案就是用户的图片文件夹（§6.2），量具的快速保存不该往里写（§9.1 第十五条，
    // `annotate_check` 为同一件事做过同一次替换）。换掉的那份留着，下面 note 印它。
    let from_file = mask::with(|m| m.naming.clone());
    mask::with(|m| {
        m.naming = crate::settings::Naming {
            dir: dir.to_string_lossy().into_owned(),
            template: "%p-%i".to_string(),
            prefix: PREFIX.to_string(),
            collision: falcon_core::naming::Collision::Increment,
        }
    });
    let (switch_before, auto_before, acts_before) =
        mask::with(|m| (m.reveal_after_save, m.reveal_auto, m.reveal_acted));
    let saves_before = mask::with(|m| m.quick_saves);
    let wrote = mask::with(|m| m.quick_out());
    let said = mask::with(|m| m.message.clone());
    let landed = mask::with(|m| m.landed.clone());
    let (auto_after_save, acts_after_save, saves_after) =
        mask::with(|m| (m.reveal_auto, m.reveal_acted, m.quick_saves));
    let in_dir = landed
        .as_ref()
        .is_some_and(|p| p.parent() == Some(dir.as_path()) && p.exists());
    rep.row(
        "reveal door wrote a file",
        Check::from(wrote && landed.is_some() && in_dir && saves_after == saves_before + 1),
        format!(
            "quick_out={wrote}（{} 准备起来={}），landed={landed:?} 在本档目录里且在盘上={in_dir}，\
             产品自己数的落盘次数 {saves_before}→{saves_after}，那句 {said:?}；命名从配置那份 \
             {from_file:?} 换成本档的 {PREFIX}（只换目的地，模板与碰撞规则写死在这里）——\
             这一格不判成败以外的账，它只保证下面两格问的是真落过盘的那一张",
            dir.display(),
            made.is_ok()
        ),
    );

    // ---- 开关那一半（第 46 项 ③ 的第二问）：`[output] reveal_after_save` 开着时，落盘
    // 自己就该走一次 `reveal_now`。这一句以前只有"次数"读法（`quick save reveal follows the
    // switch` 那句自己就承认了），这一档第一次把它读到 shell：开关开着 ⇒ 这一次落盘之后
    // `reveal_auto` 与 `reveal_acted` 各 +1。
    let auto_delta = auto_after_save.saturating_sub(auto_before);
    let acts_delta = acts_after_save.saturating_sub(acts_before);
    let switch_ok = if switch_before {
        auto_delta == 1 && acts_delta == 1
    } else {
        auto_delta == 0
    };
    rep.row(
        "reveal switch acts by itself",
        Check::from(wrote && switch_ok),
        format!(
            "mask open 拿到的开关={switch_before}（本轮这份配置里的值，这一档不替用户改它）；\
             这一次落盘让 reveal_auto {auto_before}→{auto_after_save}（+{auto_delta}）、\
             到 shell 的次数 {acts_before}→{acts_after_save}（+{acts_delta}）。要求开着时两个都 \
             +1、关着时 auto 不动——这是这台机器上第一次读到\"开着会怎样\"，之前那一格只说得出次数"
        ),
    );

    // ---- 手指那一次：通知上的 MouseArea 调的就是 `reveal_landed`，这一档直接递同一次。
    let clicks_before = mask::with(|m| m.reveal_clicks);
    let acted = mask::with(|m| m.reveal_landed());
    let (last, acts_now, clicks_now) =
        mask::with(|m| (m.reveal_last.clone(), m.reveal_acted, m.reveal_clicks));
    let planned = landed
        .as_ref()
        .and_then(|p| platform_windows::reveal::plan(p).ok());
    let names_folder = planned
        .as_ref()
        .is_some_and(|t| last.contains(&t.folder.display().to_string()));
    rep.row(
        "reveal act reached the shell",
        Check::from(
            acted
                && clicks_now == clicks_before + 1
                && acts_now == acts_after_save + 1
                && last.starts_with("已经在 ")
                && names_folder,
        ),
        format!(
            "这一次点击递出去={acted}（要求 true），计数 {clicks_before}→{clicks_now} 与 \
             {acts_after_save}→{acts_now}（各要求 +1），话={last:?}（要求以「已经在 」开头且点名 \
             那个目录）；plan 答 {planned:?}，开的窗是这张图自己的目录、选中的是它自己"
        ),
    );

    // ---- 这一格是发出来的 Blocked，不是删掉的一格，而且它说的不是"没做"：窗已经开了，
    // 是本进程没有资格替它作证。`reveal.rs` 顶上那句天花板写得很清楚——`Ok(())` 只到 shell
    // 收下这条请求为止。桌面侧的前后两次普查由跑级的 `.cmd` 在进程外面读，归档在
    // `p46_reveal_windows*.txt`，产品这一格不替那份读数担保。
    //
    // 那两次普查读的是 `Shell.Application.Windows()`，不是本档第一版挑的
    // `Get-Process explorer | MainWindowTitle`：后者在 `p46_reveal4` 那趟前后三次都答 0，
    // 而同一个时刻 shell 自己数得出 2 扇、其中一扇的 `LocationURL` 正是本档的目录 ——
    // 那台仪器的 0 说的是"我这台机器上没有一个 explorer 进程带非零 MainWindowHandle"，
    // 不是"桌面上没有窗"。句子因此点名仪器，不点数字。
    rep.row(
        "reveal window counted on the desktop",
        Check::Blocked,
        format!(
            "本进程举不到 explorer 那个窗，所以这一格不由产品判：到 shell {acts_now} 次，最近一句 \
             {last:?}。桌面上那个窗在不在，看跑级档里进程外那两次普查——读的是 \
             `Shell.Application.Windows()` 的条数与每扇的 `LocationURL`（只数不碰，跑完也不关；\
             `Get-Process explorer` 那一台在本机恒答 0，已被 `p46_reveal4` 证伪）；这一档被要求等 {asked_ms} ms"
        ),
    );
    rep.note(
        "reveal left a window",
        "这一档故意在用户桌面上留一个关不掉的窗（本进程无权关别人的窗）。它不给默认值，\
         只在被点名时跑"
            .to_string(),
    );
    rep.note(
        "reveal subject",
        format!("landed={landed:?} 开关={switch_before} 到 shell={acts_now}"),
    );
    rep.finish("reveal door")
}

/// 这一档自己的下限：遮罩要开得起来、`save_out` 要在事件循环里跑一次，短于此就是量具自己
/// 没跑到，而不是产品拒绝了。`i32` 与 [`crate::pin_check::MIN_MS`] 同口径：`main` 里
/// `after()` 交回的就是它，把两种数在 `.max()` 上撞一次是第三次踩第 38 项 ② 那条账。
pub const MIN_MS: i32 = 900;

/// 与另外几道门同一句收尾：这一档开着遮罩，所以它报的是遮罩开不起来时的那句话。
pub fn open_failed(e: &str) -> (Check, String) {
    let mut rep = Report::new();
    rep.row(
        "reveal door is not a gauge",
        Check::Blocked,
        format!("遮罩开不起来（{e}），这一档的三格全都没得问"),
    );
    rep.finish("reveal door")
}
