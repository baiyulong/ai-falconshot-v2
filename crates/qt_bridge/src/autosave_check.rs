//! §5.5.3 的那一格开关在**运行期**的读数：`[output] auto_save` 开着时，一次 `Enter` 就
//! 该在盘上多出一张图；§5.5.1 的第 3 步那一格"按默认行为复制"（`[output] auto_copy`）自
//! P59 起由同一档、同一道门一起读。
//!
//! 上一轮的 `--reveal`（§10 第 46 项 ③）给的是"落盘之后顺手开窗"那半条；这一档给的是
//! §5.5.3 那一半：PRD 说"完成截图后，系统自动生成文件"，而这一句在 P58 之前只有**配置
//! 结构里的一个 bool**，整个仓库里没有一处读它——写单测能证明"字段被读了"，证不了"这台
//! 机器上真有一个文件因为这次按下而出现"。所以这一档不在 `mask::GAUGE_FLAGS` 里，它自己
//! 开一张遮罩，走产品的 [`crate::mask::MaskState::commit`]，然后数目录。复制那一半同理：
//! 单测里把剪贴板写穿一次只能证明"这段代码会调 `write_image`"，所以这一档把剪贴板**读回**
//! 来对尺寸，并在离开时把用户原来那份还回去（§9.1 第十条：拿走的要还得上）。
//!
//! 代价写清楚：这一档**会**在 `FALCONSHOT_AUTOSAVE_DOOR_DIR` 指的那个目录里留下文件，
//! 那是 scratch 底下的一个目录，跑完不删（文件本身就是读数）。自 P94 起它也会在
//! `FALCONSHOT_AUTOSAVE_HIST_DIR` 那枚 temp 沙盒里留一份 `history.db` 与其中的行——那是
//! §5.14.1 第一条的运行期读数，同样跑完不删（行数本身就是读数）。自 P96 起同一份沙盒也接
//! 第二条：一次 `escape()` 把选区清掉，那一格该留下的是一部**标成已取消的行**、而不是盘上
//! 多出一张图。它不会碰用户的图片文件夹，
//! 也不会碰用户的 `%APPDATA%\ai-falconshot\history\`：两个落点都在本档里换掉，理由和
//! `reveal_check` 换它是同一条（§9.1 第十五条）。开关开着时它
//! 还会碰剪贴板——那是这一格唯一的面，本档因此必须先读、后还，还不上就在判据里说。
//!
//! 自 P97 起这档再多一件事，而它和前几件**不同族**：前面量的都是"往盘上写"的那几条腿，
//! 这一件是**从库里读回来再落一次盘**（[`crate::mask::MaskState::replay_history`]，
//! §5.14.2「从历史记录中……保存」的第一条产品腿）。它因此第一次需要本档把某一行的行号交回
//! 产品，而不只是数一数库里长了几行：读侧的账只有在图真能取回来时才是账。放在这档而不是新开
//! 一枚 CLI 门，是 P96 那条决定的续（§9.1 第十五条 P96 补：新开一道的代价约十处 `main.rs`
//! 落点，而 `numbered` 少了 `quit_after` 会挂住整个进程）。代价也写清楚：这档自此同时在册
//! 三件不同 PRD 小节的事（§5.5.3、§5.14.1、§5.14.2），**判据必须逐格印自己的数**，不能由
//! 整档那一句 verdict 代答。
//!
//! 自 P98 起这档再多第五件，而它与前面四件的**区别不在判的是什么，而在谁去判**：前面每一格
//! 都是本档拿 `mask::with` 直接按产品的腿，这两格按的是**界面**那条腿 —— QML 的
//! `Component.onCompleted` 经 [`crate::session`] 的 `historyRows` / `historyReplay` 走到
//! [`crate::mask::MaskState::list_history`]（那是 P95 普查点名的 `History::newest` 在产品里的
//! 第一位界面读者）。它因此第一次要求本档**跑到 QML 引擎之前**去做前置：[`arm`] 在
//! `engine.load()` 之前钉走两枚落点并在沙盒库里种下一行 37×23 的历史，否则工具条在引擎加载
//! 那一刻读的就是用户 `%APPDATA%\ai-falconshot\history\` 里那些行（§9.1 第十五条）。同一趟
//! 里 `measure()` 在事件循环之后再钉一次是幂等的，所以下面每一格的差值算术一位也不动。
//!
//! 但**幂等的是落点，不是计数器**：界面那一条腿跑在 `engine.load()` 里面，也就跑在
//! `measure()` 读到的每一个数**之前**。本档第一次跑就是这么红的 —— 回放那一格拿"本档自己
//! 那几条腿之后"的计数当基准，于是界面那一次永远算不到账上。基准因此只能取在 [`arm`]
//! 那一刻（§9.1 第十条续）：一条腿读单调递增的计数器时，先问它读的是事件之前还是之后。
//!
//! 自 P99 起这档再多四格，而它们的**符号与前面每一件都相反**：§5.5.3、§5.14.1、§5.14.2
//! 那几件都在往盘上**加**东西，判据所以能问"这个文件在不在"；§5.14.4 的删除与清空是这座
//! 沙盒里第一次**减**，一次成功的判据会把行连同它的原图一起弄没，"在不在"于是问不出答案
//! —— 这四格全部取前后两次读数做差（库里行数与 `history/` 下的张数各数两遍），而**不是**
//! 只问事后那一眼。四格里只有一格允许真删：正面那一格删的是本档自己种下的那一行，其余三格
//! 判的恰恰是"不许动"，所以空选区、未确认、锁定项三格各自的差值都要求为 0。两枚计数
//! （[`crate::mask::MaskState::history_deletes`] / [`crate::mask::MaskState::history_clears`]）
//! 分开的理由也印在判据里：手动删除**不**看锁（core 的 `delete` 压根不读那一列），而清空
//! **必须**看锁（`clear_all(false)`），两条口径相反的两个数并成一个，就再也说不清是哪一个
//! 走岔了。最严的那一格是未确认：它要求产品**连一次库都不打开**，所以前置给的是一枚全新落点
//! （那里本就没有 `history.db`），判据问的是"这一趟跑完之后它长出来了没有"。最后一格的前置
//! 由本档经 core 打上锁（产品里至今没有"锁定"那一条腿），所以它先把"锁上没上"问一次盘上，
//! 控制组自己没摆出来就 BLOCKED —— 拿没立起来的对照组判一条规则，判的是本档自己。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::PoisonError;

use crate::capture;
use crate::delay;
use crate::mask;
use crate::r13::Report;
use crate::state::Check;

/// 本档的落点，和 `reveal_check::door_dir` 同一个理由：每次跑一枚新的，前后两次普查的差
/// 才是读数而不是上一趟的遗留。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_AUTOSAVE_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-autosave-door");
    dir
}

/// §5.14.1 那一行历史在这档里的落点，和 [`door_dir`] 分开的理由有两条：那条尺子
/// `count(&door_dir())` 数的是**目录里的文件**，把 `history.db`、缩略图与原图混进去
/// 就等于把两件事量成一件；而历史那一头要的是**行数**，它得对着 SQLite 问，不能对着
/// 目录猜。沙盒也在 temp 下 ⇒ 这档一次也不碰用户的 `%APPDATA%\ai-falconshot\history\`。
fn hist_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_AUTOSAVE_HIST_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-autosave-history");
    dir
}

/// 那把数行数的尺子：自己开一次的 SQLite 读回，不经 `MaskState` 里那枚 `Option<History>`
/// ——要问的是**盘上此刻有几行**，而不是产品自己数了几次（那两个数各判一格，见下）。
fn history_rows(dir: &Path) -> Result<u64, String> {
    falcon_core::history::History::open(dir)
        .and_then(|h| h.count())
        .map_err(|e| e.to_string())
}

/// 数完行数之后，这一把尺子问的是**具体那一行**在盘上写着什么：`cancelled` 那一格，
/// 以及它记下的选区。仍是自己开一次 SQLite、不读 `MaskState` 里那两个字段——屏幕上
/// 那句话与盘上那一行各说各话，正是 §5.14.3 里「找到标记为已取消的记录」要能分开的东西。
fn history_row(dir: &Path, id: i64) -> Result<Option<(bool, falcon_core::PhysRect)>, String> {
    falcon_core::history::History::open(dir)
        .and_then(|h| h.get(id))
        .map(|rec| rec.map(|rec| (rec.cancelled, rec.rect)))
        .map_err(|e| e.to_string())
}

/// [`history_row`] 那一把尺子的答法印成一句话。三种答法在判据里是三件事：「读到什么」、
/// 「库里没有那个 id」和「库打不开」混成一条 `{e:?}` 就是把它们说成一样。
fn row_said(r: &Result<Option<(bool, falcon_core::PhysRect)>, String>) -> String {
    match r {
        Ok(Some((cancelled, rect))) => format!("cancelled={cancelled} rect={rect}"),
        Ok(None) => "库里没有那一行".to_string(),
        Err(e) => format!("读不回（{e}）"),
    }
}

/// 库里那一行的左右，本档**自己开一次 SQLite** 问三回（`get` / `prev` / `next`）再自己印成
/// 一句话（P114 起给第 32 格当尺子）。
///
/// 不调 [`crate::settings::neighbors_of_banner_for`]，那是界面这一条腿读的那一枚 —— 拿它的输出
/// 当判据就是让它给自己作证。格式与它同形是**刻意的第二份实现**：两处一旦分岔，第 32 格当场红，
/// 而那正是这一格想要的失败形状（措辞漂移与答案错位在盘上是两件事，在这里由同一枚断言看着）。
///
/// 库里没有那一行、库打不开、锚点是 `-1` 三格都答 `None`。第 32 格据此判 **FAIL 而不是 BLOCKED**：
/// 那三格各有各的话可说，而本档说不出那句就是没有资格替界面断言"这句对"，把没收住的事说成
/// 前置没摆出来，是 [`history_row`] 那把尺子从 P103 起就拒过的形状。
fn neighbors_from_disk(dir: &Path, anchor: i64) -> Option<String> {
    let store = falcon_core::history::History::open(dir).ok()?;
    let cur = store.get(anchor).ok()??;
    let prev = store.prev(anchor).ok()?.map(|rec| rec.id);
    let next = store.next(anchor).ok()?.map(|rec| rec.id);
    let dash = |v: Option<i64>| {
        v.map(|id| id.to_string())
            .unwrap_or_else(|| "-".to_string())
    };
    Some(format!(
        "history anchor={} prev={} next={}",
        cur.id,
        dash(prev),
        dash(next)
    ))
}

/// 界面那条纯读腿递回来的那一句切成**恰好两段**：产品那句话、与那一刻的框。段数不是二的一律当
/// 它不成立（`None`），所以"多塞一段"与"少一段"在判据里都是红而不是被 `unwrap` 掉。
///
/// 写成 `fn` 而不是像 [`measure`] 里那样写成闭包：它要把参数的生命周期带进返回值，而那一格由
/// 一枚具名函数答得最省 —— 本格与它上面每一格的读数都取自同一批 `String`，谁也不许改它们。
fn split_two(l: &str) -> Option<(&str, &str)> {
    let mut seg = l.split('|');
    let said = seg.next().unwrap_or_default();
    let got = seg.next().unwrap_or_default();
    if seg.next().is_some() {
        return None;
    }
    Some((said, got))
}

/// 产品那句「删了历史第 … 行，收回 **N** 字节」里的 N。P115 起，只认那句话自己写出来的数：
/// 这一格问的就是那句自称与 `arm()` 当场从库里问回来的两行字节数对不对得上，拿 `Freed` 结构体
/// 当尺子等于让产品给自己作证。
fn bytes_claimed_in(s: &str) -> Option<u64> {
    let tail = s.split_once("收回")?.1;
    tail.trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<u64>()
        .ok()
}

/// [`history_picture`] 的那一份答案，三件一起回来才够判 §5.14.1 的"截图内容"那一格：
/// `path_of` 指到的**那一个文件**（`None` = 库里这一行没有原图，`[history] keep_originals`
/// 关着就是这个形状，它与"库坏了"是两件事）、它此刻在盘上的**字节数**（读不到算 0），
/// 以及它**解码出来的宽高**（`None` = 那份字节不是本程序能读的图）。
type Picture = (Option<PathBuf>, u64, Option<(u32, u32)>);

/// §5.14.1 那句"历史记录应包含……截图内容"在**盘上**的对面。P93–P96 四轮问的都是"库里
/// 有没有这一行"，这一把问的是"这一行指的图在不在、它是不是那一尺寸的图"——一份行数对得上
/// 而图取不回来的历史，§5.14.2 那一条"从历史里再保存一次"就无从跑起。尺子照旧自己开一次
/// SQLite，不经 `MaskState`：要问的是盘上有没有那张图，而不是产品自己说它写过。
fn history_picture(dir: &Path, id: i64) -> Result<Option<Picture>, String> {
    let store = falcon_core::history::History::open(dir).map_err(|e| e.to_string())?;
    let rec = store.get(id).map_err(|e| e.to_string())?;
    Ok(rec.map(|rec| {
        let abs = store.path_of(&rec);
        let bytes = abs
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(0, |m| m.len());
        let dims = abs
            .as_ref()
            .and_then(|p| falcon_core::encode::decode_file(p).ok())
            .map(|f| (f.width, f.height));
        (abs, bytes, dims)
    }))
}

/// [`history_picture`] 那一把尺子的答法，同 [`row_said`] 的理由分成三种答法印。
fn picture_said(r: &Result<Option<Picture>, String>) -> String {
    match r {
        Ok(Some((abs, bytes, dims))) => format!("文件={abs:?} 字节={bytes} 解码={dims:?}"),
        Ok(None) => "库里没有那一行".to_string(),
        Err(e) => format!("读不回（{e}）"),
    }
}

/// [`history_rows`] 那一把尺子的答法：数到几行，或者**为什么**数不出来。把"库打不开"印成 0
/// 就等于把一次失败说成一次空库 —— 与 [`row_said`]、[`picture_said`] 立的同一条口径。
fn rows_said(r: &Result<u64, String>) -> String {
    match r {
        Ok(n) => n.to_string(),
        Err(e) => format!("读不回（{e}）"),
    }
}

/// §5.14.4 step 4 那句"释放相应磁盘空间"在**沙盒里**的对面：`history/<日期>/` 底下那些原图的
/// 总数。`count(&hdir)` 量不到这件事——它数的是顶层条目（`history.db`、`-wal`、`history/`
/// 本身），删掉一张日期目录里的 PNG 而顶层条目一动不动，在它读起来是"什么都没少"。
/// 递归只走一层，因为 §6.2 定的布局就是 `history/<YYYY-MM-DD>/<id>.png`，多写一层是替未来
/// 某个还没出现的布局猜路。
fn picture_count(dir: &Path) -> usize {
    let Ok(days) = std::fs::read_dir(dir.join(falcon_core::history::PICTURES_DIR)) else {
        return 0;
    };
    days.filter_map(Result::ok)
        .filter_map(|e| std::fs::read_dir(e.path()).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .count()
}

/// 锁上没上，当场问一次盘上——与 P97 那格"先证明那一行确实不在"同一条口径：控制组自己没摆
/// 出来，判据红了也就没分量。P100 起这一把锁由**产品**那条腿打上
/// （[`crate::mask::MaskState::lock_history`]），本档只读、不再替它写：读的那一臂必须独立于
/// 被判的那一臂，否则判那把锁的一格就成了自证。
fn locked_of(dir: &Path, id: i64) -> Result<Option<bool>, String> {
    falcon_core::history::History::open(dir)
        .and_then(|h| h.get(id))
        .map(|rec| rec.map(|rec| rec.locked))
        .map_err(|e| e.to_string())
}

/// 钉的那个落点：本档走的是产品的 `close()` 会写字回的那道门，所以写回必须先钉走
/// （`reveal_check::pin`、`annotate_check::plant` 为同一件事各钉过一次，这是第三次）。
fn pinned_path() -> PathBuf {
    let mut dir = door_dir();
    dir.push("pinned-config.toml");
    dir
}

pub fn pin() -> PathBuf {
    let _ = std::fs::create_dir_all(door_dir());
    crate::settings::pin_writes(&pinned_path())
}

/// 本进程真读进来的那份文件、与写回的那份文件。
///
/// 不解析 `Settings::line()`：那一行的 `read=` 后面是 `Settings::tag()` 给的一个来源词
/// （"file"、"defaults" 之类），不是路径，而 `from=` 才是路径却会被含空格的路径切断。
/// `read_from` 与 `write_to` 是同一份数据的两个公开字段，`annotate_check` 已经是直接取
/// 它们（那两处 `with(|s| s.read_from.clone())`），这一档跟着最新的那一位。
fn read_and_write() -> (PathBuf, PathBuf) {
    crate::settings::with(|s| (s.read_from.clone(), s.write_to.clone()))
}

/// 从**本进程真读过的那份文件**里抠出 `[output] <key>` 的字面值。
///
/// 不用 `toml` 也不用 config 自己的解析器：那一层和被测的读者共用同一个 `Config`，配不上
/// 就成了两边一起错。这里只找那一段表、那一行的等号右边是 `true` 还是 `false`，找不到就
/// 答 `None`——那种答法本身也是这一格要印出来的话。
///
/// 键名是参数而不是硬编码：本档现在问的是 `[output]` 里在那道门上会合的**两枚**开关
/// （`auto_save` 与 P59 起的 `auto_copy`），而两枚都必须由同一把尺子量，否则"这一行在档里
/// 写着 true 而字段是 false"只会在一枚上被抓到。
fn value_in_file(path: &Path, key: &str) -> Option<bool> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_output = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_output = t == "[output]";
            continue;
        }
        if !in_output {
            continue;
        }
        if let Some(rest) = t.strip_prefix(key) {
            let v = rest.split('=').nth(1)?.trim();
            return match v {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        }
    }
    None
}

/// 目录里现在有几张图。数的是**条目**，不是文件名，所以模板换成日期时间也不会数漏。
fn count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).count())
        .unwrap_or(usize::MAX)
}

/// 目录里那些**能解码、且解码出来正好是这一对尺寸**的条目数。P98 的回放普查用的是它：
/// 数"几张 37×23"而不是"多几张文件"，因为 `dir` 里还躺着这一场 `commit()` 自动保存的那一张
/// （尺寸是这一场的选区），而本档要的读数是"界面那一条腿到底写出几张图"。撞不上是设计出来的：
/// [`PLANT_W`]×[`PLANT_H`] 不是任何一块屏可能给出的选区，所以真撞上时这一格是 FAIL 而不是
/// 悄悄少算 —— 对面换成一台 37×23 的机器时，这一格会当场喊出来。
fn count_of_size(dir: &Path, w: u32, h: u32) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return usize::MAX;
    };
    rd.filter_map(Result::ok)
        .filter(|e| {
            falcon_core::encode::decode_file(&e.path())
                .ok()
                .is_some_and(|f| (f.width, f.height) == (w, h))
        })
        .count()
}

// -------------------------------------------------------------- 界面那一条腿（P98 起）
//
// `--hint` 的 `plant()`/`plan_line()`/`report()` 三件是同族做法，这里第四次用：QML 的
// `Component.onCompleted` 跑在 `engine.load()` 那一刻，而本档其余一切读数跑在
// `app.exec()` 之后 —— 中间隔着整个事件循环。一条要在界面里读历史清单的腿，因此必须
// **在引擎之前**把两枚落点钉走、并把库里那一行种好；否则它读的是用户
// `%APPDATA%\ai-falconshot\history\` 里那些行（§9.1 第十五条禁的就是这一件）。

/// 种进沙盒库里那一行的尺寸。37×23 是本档挑的，理由写在 [`count_of_size`] 头上。
const PLANT_W: u32 = 37;
const PLANT_H: u32 = 23;

/// P101 起**替界面那一条腿**另种的一行，尺寸与上面那一对不同。它存在的唯一理由是归属：
/// 界面那条腿自此要锁一行、删一行，而那两枚行号本档另有主人在数（`armed` 是清单、回放、未确认
/// 清空、上锁、确认清空那几格的主人，`commit_row` 是删除那一格的主人）——界面碰掉任何一枚，那几格
/// 读到的就变成「自己的前置被自己判的那条腿改过」的现场，判据不再说话。尺寸分开是第二层：门
/// 落盘目录里数得出几张 37×23（回放那一格）与"界面有没有把自己那一行也回放出去"（新格要求
/// 29×17 在那里是 0 张）是两笔分得开的账。
const PLANT_UI_W: u32 = 29;
const PLANT_UI_H: u32 = 17;

/// P103 起界面那一条腿**先挑**的那一行，尺寸与上面两对又各不同。它存在的理由是判据本身：这一格
/// 要读的是「屏幕上那一只框被界面挪动过」，而挪动只在与**它原来不在的位置**比较时才读得出来 ——
/// 门钉的那只框是这一场选区的唯一一份，界面若只许挑一枚矩形，挑得与它相等就等于什么都没判。
/// 41×29 与 37×23、29×17 一样不是任何一块屏可能给出的选区（[`count_of_size`] 头上那条理由），
/// 所以真撞上时这一格是 FAIL 而不是悄悄少算。
const PLANT_PICK_W: u32 = 41;
const PLANT_PICK_H: u32 = 29;

/// P115 起界面那一条腿**一次点名两行**的那两枚行号，尺寸与上面三对又各不同。
///
/// 这三头早就收得下"多条"：[`falcon_core::history::History::delete`] 的参数是 `&[i64]`、
/// [`crate::mask::MaskState::delete_history`] 收的是一串、`Session` 那一头的 `historyDelete` 桥连
/// `|` 都拆得开（`session.rs` 里那条 `split('|')`），而它头上的注释自 P100 起写的就是 §5.14.4
/// 步骤 2 那句「选择单条、多条」。零的是**读它的那一头**：`AnnotationToolbar.qml` 里唯一一枚界面读者递的是 `String(row)` —— 一次一行，
/// 门侧因此从来没有一次调用带走两行的读数。这两枚行号只为这一件事存在：让「一次点名、两行没了、
/// 字节是两行之和」在盘上留下三笔分得开的账。
///
/// 43×31 与 47×19 同样不是任何一块屏可能给出的选区（[`count_of_size`] 头上那条理由），而且**两枚
/// 互不相等**：同一枚尺寸的话，"两份图都不在了"在盘上就只读成一件事，分不出是两行还是一行。
const PLANT_BATCH1_W: u32 = 43;
const PLANT_BATCH1_H: u32 = 31;
const PLANT_BATCH2_W: u32 = 47;
const PLANT_BATCH2_H: u32 = 19;

/// 界面那一条腿要读的清单最多几行。它同时是 QML 递给 `historyRows()` 的那个参数：工具条
/// 不自己决定读几行，读几行由档这一头钉，判据才知道"数到几段"该是多少。
const HIST_LIMIT: usize = 8;

/// [`Plan::planted`] == 0 是"这一趟不是 `--autosave`"，[`plan_line`] 因此答空串。
struct Plan {
    /// `arm()` 种下的那一行的行号；0 = 没种过。
    planted: i64,
    /// P101 起：`arm()` **替界面**种下的那一行的行号；0 = 没种过。界面那一条腿只许碰它。
    ui_row: i64,
    /// 替界面种的那一行在盘上指的那份原图，`arm()` 当场问回来的。界面删没删成，判据冲这条路
    /// 问文件系统，而不是只信产品那句话。
    ui_file: Option<PathBuf>,
    /// P103 起：`arm()` 替界面**挑**的那一行的行号，它的矩形与这一场的选区不同。0 = 没种过。
    /// 界面那一条腿先恢复这一枚，再恢复 [`Plan::back_row`] 那一枚。
    pick_row: i64,
    /// 同上一条：那一枚的矩形就是 [`Plan::hole`]，界面递回来的第二个读数应当与它一字不差。
    back_row: i64,
    /// P115 起：`arm()` 替界面**一次点名**的那两枚行号（`(0, 0)` = 没种）。§5.14.4 步骤 2 那句
    /// 「选择单条、多条」里「多条」那一半的对面，就是这两个号一起递出去的那一次调用。
    batch_rows: (i64, i64),
    /// 上面那两枚行号各自指着的原图，`arm()` 当场问回来的。界面删没删成，判据冲这条路问文件
    /// 系统，而不是只信产品那句话（同 [`Plan::ui_file`] 那条理由，只是这里有两份）。
    batch_files: (Option<PathBuf>, Option<PathBuf>),
    /// 那两份图在**种下那一刻**各值多少字节（`bytes_of` 那三件：缩略图 + 原图）。基准取在这一刻
    /// 是因为删成之后两份都不在了，而判据对面是产品那句「收回 N 字节」——N 对不对，只有拿删之前
    /// 各自量到的两枚之和去问得出。
    batch_bytes: (u64, u64),
    /// `arm()` 那一刻遮罩上那一只框。这一枚是"界面有没有把选区放回原处"的**唯一**基准：本档
    /// 其余各格读 `m.hole` 全在 `engine.load()` 之后，拿它们当基准就成了让界面自己判自己；而
    /// 上面两枚行号的矩形都由本档在种行那一刻定死（挑那一枚钉一寸不相干的、放那一枚取的正是
    /// 这一刻的框），不取这一枚就分不出「挑走了再放回来」与「一次也没动」。
    hole: falcon_core::PhysRect,
    /// 同一条理由，对 `history_restores` 取的基准：界面那一条腿一次恢复两行，`measure()` 上面
    /// 那一格恢复腿自己也要按一次，加数不取在种行那一刻就分不开是谁递的。
    restores: u32,
    /// P113 起对 `history_flips` 各取一次基准。这一枚是上面那枚的**拆分之一半**（翻页同时加两枚、
    /// 直接按的恢复只加 `restores`），所以两枚必须一起取：只知道总数就知道不了界面那两条腿里
    /// 有没有一条根本没跑。
    flips: u32,
    /// 种完那一刻库里**实际**有几行（同一枚沙盒被跑第二趟时多于 1，判据要的数是它而不是 1）。
    rows: usize,
    /// `arm()` 那一刻产品自己已经数到的回放次数。界面那一条腿跑在 `engine.load()` 里，也就跑在
    /// [`measure`] 的**任何一格之前** ⇒ `measure()` 后来读到的每一个计数都已经把界面那一次算在
    /// 里面了。基准只能取在种行那一刻，否则判据会把「界面排在档之前」当成「界面排在档之后」——
    /// 本档第一次跑就是这么红的，那句要求写在回放那一格里。
    replays: u32,
    /// 同上一条的理由，P101 起对**另外三枚**计数各取一次基准：界面那一条腿自此也走清空、锁与
    /// 删除（P103 起还多走两次恢复），而 `measure()` 下面那几格——删点名那一行、空手删除、没
    /// 点头的清空、上锁、确认过的清空——自己也要走那三条腿。基准不取在种行那一刻，就分不开
    /// 那一枚加数是界面递的还是档递的。
    dels: u32,
    clears: u32,
    locks: u32,
    /// 界面那一头经 `historyGaugeReport` 递回来的每一行，原样留着。
    lines: Vec<String>,
}

impl Plan {
    const fn none() -> Self {
        Plan {
            planted: 0,
            ui_row: 0,
            ui_file: None,
            pick_row: 0,
            back_row: 0,
            batch_rows: (0, 0),
            batch_files: (None, None),
            batch_bytes: (0, 0),
            hole: falcon_core::PhysRect::new(0, 0, 0, 0),
            restores: 0,
            flips: 0,
            rows: 0,
            replays: 0,
            dels: 0,
            clears: 0,
            locks: 0,
            lines: Vec::new(),
        }
    }
}

static PLAN: Mutex<Plan> = Mutex::new(Plan::none());

fn with_plan<R>(f: impl FnOnce(&mut Plan) -> R) -> R {
    let mut plan = PLAN.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut plan)
}

/// 界面那一条腿的计划：`hist | 最多读几行 | 那一行的行号 | 替界面种的那一行的行号 | 替界面挑
/// 的那一行的行号 | 替界面放回原处的那一行的行号 | 替界面一次点名的第一枚 | 替界面一次点名的第二枚`。
/// 空串是
/// "这一趟没种过行"，也就是产品会话与另外六道门 —— `AnnotationToolbar.qml` 的那条腿据此整条
/// 躲开，与 `hint_check::plan_line()`、`annotate_check::mask_gauge_plan()` 同一个口径。
///
/// 第四段是 P101 加的，而且必须是**另一枚**行号：清单与回放那两格读的是第三段那一行，而锁与
/// 删除那两条腿一旦也冲它去，本档后面那几格的前置就被界面改掉了。界面递 `false` 的那次清空
/// 不点名任何行，因此不需要第五段。
///
/// 第五、六段是 P103 加的一对，成对才有意义：恢复那一格要读的是「框被挪走、又被放回」，单独
/// 一枚行号只能读出这两件事中的一件，而**放回来**那一半必须交给本档钉死的那枚矩形（= `arm()`
/// 那一刻的选区），否则界面这条腿走完就把本档其余各格的前置改掉了。
///
/// 第七、八段是 P115 加的一对，同样是成对才有意义：这一格要读的对面是**一次调用带走两行**，
/// 递一枚行号过去，「多条」与「单条」在盘上长得一模一样。两枚都是本档自己的行号（第四段那枚
/// 已经被 P101 的删除腿删掉了，第三段那枚是清单与回放两格的前置），界面碰掉它们之后，剩下各格
/// 读的仍是 `planted`、`pick_row`、`back_row` 三枚 —— 一格没收走另一格的读数。
pub fn plan_line() -> String {
    with_plan(|p| {
        if p.planted == 0 {
            return String::new();
        }
        format!(
            "hist|{HIST_LIMIT}|{}|{}|{}|{}|{}|{}",
            p.planted, p.ui_row, p.pick_row, p.back_row, p.batch_rows.0, p.batch_rows.1
        )
    })
}

/// 界面递回来的一行。`rows|` 与 `replay|` 是清单/回放那两条腿的自述（P98 起），
/// `clearunconfirmed|`、`lock|`、`delete|` 是 P101 起那三条去侧腿的自述，`restorepick|` 与
/// `restoreback|` 是 P103 起那两条恢复腿的自述，`flipup|` 与 `flipdown|` 是 P113 起那两条**按键**
/// 腿的自述（它们递的第一段不是产品的话，而是 `chord()` 自己那句"这枚按键我认领了"），`neighbors|`
/// 是 P114 起那条**纯读**腿的自述（它问的是屏幕上此刻那一行的左右，一件也不改），`deletebatch|`
/// 是 P115 起那条**一次点名两枚**腿的自述（它与 `delete|` 走同一个桥，唯一新东西是递过去的行号有
/// 两枚）。递回去的其余各段
/// 都是**产品当场说的那一句**，不是工具条自己复述的。这十一枚前缀本档一处也不写 ⇒ "这行是本档写的"
/// 与"这行是 QML 经界面递回来的"分得开，而那正是这几格唯一新东西。
pub fn report(line: &str) {
    with_plan(|p| p.lines.push(line.to_string()));
}

/// 界面递回来的那些行，按前缀挑出来。返回的是**每一行前缀之后的那一段**，所以调用点数得到的
/// 是"几条腿各自答了什么"，而不必再在这里切一次字符串。
fn reported(prefix: &str) -> Vec<String> {
    with_plan(|p| {
        p.lines
            .iter()
            .filter_map(|l| l.split_once('|'))
            .filter(|(k, _)| *k == prefix)
            .map(|(_, v)| v.to_string())
            .collect()
    })
}

/// 同一个前缀各占 `Plan::lines` 里的第几枚。要问"界面那几条腿的先后"只有这条路：一条腿先锁
/// 还是先删，判据对面是**追加顺序**，而不是哪一句话的内容——内容本身不带着"它是第几次递上来"
/// 这件事，而后写的那一枚工具条也可能把前一枚的话重复一遍。
fn reported_at(prefix: &str) -> Vec<usize> {
    with_plan(|p| {
        p.lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.split_once('|').map(|(k, _)| (i, k)))
            .filter(|(_, k)| *k == prefix)
            .map(|(i, _)| i)
            .collect()
    })
}

/// 本档的两枚落点钉进 `MaskState`：落盘目录换成 [`door_dir`]、历史换成 [`hist_dir`] 那枚 temp
/// 沙盒，三枚上限钉 0 而 `enabled` 与 `save_all` 钉真（P93–P96 四段话讲的仍是这一件事，
/// 抽出来只是因为它现在有两个调用点：`arm()` 在引擎之前、`measure()` 在事件循环之后）。
///
/// **幂等**：两次钉的是同一对路径与同一份 `[history]` 快照，所以 `measure()` 里
/// `files_before`/`rows_before`/`records_before` 三枚前置的算术一位也不动。
fn retarget() -> (PathBuf, PathBuf) {
    let dir = door_dir();
    let _ = std::fs::create_dir_all(&dir);
    let hdir = hist_dir();
    let _ = std::fs::create_dir_all(&hdir);
    mask::with(|m| {
        m.naming.dir = dir.to_string_lossy().into_owned();
        let mut h = crate::settings::history_cfg();
        h.enabled = true;
        h.max_items = 0;
        h.max_age_days = 0;
        h.max_size_mb = 0;
        h.save_all = true;
        m.history = Some(h);
        m.history_dir = hdir.clone();
    });
    (dir, hdir)
}

/// P103 起给界面那两条恢复腿各补一枚"库里的一行"，走的与 [`arm()`] 里其余两行同一条路
/// （[`falcon_core::history::History::record`]）：差别只在**矩形是递进来的**而不是钉死的：
/// 挑走那一问要一枚与此刻那只框不同的矩形，放回那一问要一枚正好是它自己的矩形，而"此刻那只框
/// 是多大"只有运行期知道 —— 门钉的选区随屏幕变，把它写成常量就等于要求每台机器都长同一张脸。
///
/// 图按那一枚矩形自己拼（`rect.w × rect.h`），所以库里那一行记的图与它记的选区**仍然是同一个
/// 尺寸**：本档不往沙盒里放一枚"图与框不符"的行，那种行一旦被哪一格回放，读回来的尺寸就不该
/// 拿去跟任何选区比。矩形是空的 ⇒ 直接 `Err` —— 空矩形在 [`crate::mask::MaskState::restore_history_selection`]
/// 那一头本来就被拒，种它进库只是把一次拒绝提前摆成一行脏记录。
///
/// `at_ms` 是从 [`arm()`] 头上取的**同一个**戳，不是这里再读一次钟：清单那一格要求清单头一段
/// 点名的仍是 `planted` 那一行，而 [`falcon_core::history::History::newest`] 排的是
/// `captured_at DESC, id DESC` —— 时间戳只有一枚，这一问才轮得到行号来答（见 [`arm()`] 里那段
/// 顺序的注释）。P103 第一次跑让这两枚行号自己读钟，它们就走到了 `planted` **前面**，
/// 清单头一段变成「本档替界面放回原处的那一块」，那一格当场红（`p103_auto_A.log` / `_B.log`）。
fn plant_selection_row(
    store: &mut falcon_core::history::History,
    cfg: &falcon_core::config::History,
    rect: falcon_core::PhysRect,
    monitor_id: &'static str,
    rgba: [u8; 4],
    at_ms: i64,
) -> Result<i64, String> {
    if rect.is_empty() {
        return Err(format!("{monitor_id}：矩形是空的（{rect}），不种。"));
    }
    let frame = falcon_core::frame::Frame::filled(rect.w, rect.h, rgba)
        .map_err(|e| format!("{monitor_id}：{rect} 的图拼不出（{e}）"))?;
    let meta = falcon_core::history::CaptureMeta {
        at_ms,
        offset_min: platform_windows::clock::wall_clock().1,
        monitor_id,
        rect,
        source: falcon_core::history::Source::Manual,
        cancelled: false,
    };
    store
        .record(&meta, &frame, cfg)
        .map_err(|e| format!("{monitor_id}：{rect} 写不进库（{e}）"))
}

/// [`plant_selection_row`] 再往前一步：种完立刻把那一行指的原图，与它**此刻**值多少字节一起问
/// 回来。P115 那一对"一次点名两行"的行号要的不只是行号 —— 判据对面是产品那句「收回 N 字节」，
/// 而 N 对不对只有拿删之前各自量到的两枚之和去问得出；删完再问，两份图都不在了，只剩产品那句话
/// 自己给自己作证。
///
/// 字节那一枚按 [`falcon_core::history`] 里 `bytes_of` 同样那三件算（缩略图 + 原图的 `metadata`
/// 长度），不是只读文件长度：`[history] thumb_px` 在这份 cfg 里是 0，可"这一份沙盒恰好没缩略图"
/// 不该被写进判据的前提 —— 它哪天有了缩略图，这一格就该跟着一起算，否则新格会因为漏了别人的字节
/// 而把一次正确的删除读成失败。读不回那一行、或那一行压根没有图 ⇒ 字节算 0，与 `bytes_of` 的
/// `None => thumb` 同一形状。
fn plant_row_with_bytes(
    store: &mut falcon_core::history::History,
    cfg: &falcon_core::config::History,
    rect: falcon_core::PhysRect,
    monitor_id: &'static str,
    rgba: [u8; 4],
    at_ms: i64,
) -> Result<(i64, Option<PathBuf>, u64), String> {
    let id = plant_selection_row(store, cfg, rect, monitor_id, rgba, at_ms)?;
    let rec = store
        .get(id)
        .map_err(|e| format!("{monitor_id}：种下第 {id} 行却读不回它（{e}）"))?;
    let thumb = rec
        .as_ref()
        .and_then(|r| r.thumb.as_ref())
        .map_or(0, |v| v.len() as u64);
    let path = rec.and_then(|rec| store.path_of(&rec));
    let file = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or(0, |m| m.len());
    Ok((id, path, thumb + file))
}

/// 在**引擎之前**把落点钉走、并在沙盒库里种下一行已知尺寸的历史，把它的行号写进本档的计划。
///
/// 走的是 [`falcon_core::history::History::record`] 而不是
/// [`crate::mask::MaskState::record_history`]：后者要先有画面可拼，而这一步遮罩还没开起来。
/// 也因此本档不碰 `m.history_records` 与 `m.history_last_id` —— `measure()` 里"行数加一"
/// 那一格数的还是它自己的差。`keep_originals` 在这一行上钉真，好让"那一行还带着图"与"那一行
/// 只有缩略图"这两种形状在这一档里各有一格说话。
///
/// 答的是那一行说的话（打印出来给跑门的人看），`Err` 是本档的前置没摆出来 —— 那种情况下
/// `plan_line()` 保持空串，界面那条腿就此整条不跑，而不是带着一个不存在的行号去回放。
pub fn arm() -> Result<String, String> {
    let (_, hdir) = retarget();
    // 四枚种进沙盒的行**共用这一枚戳**。它必须在任何一次 `record` 之前取：清单那一格要求
    // 清单头一段点名的就是 `planted` 那一行，而 `newest` 先按 `captured_at` 降序、再按 `id`
    // 降序 —— 戳若各读各的钟，后种的行就在时间这一维上赢了行号那一维，头一段变成别人的行。
    let at_ms = falcon_core::capture::now_ms();
    let ui_frame = falcon_core::frame::Frame::filled(PLANT_UI_W, PLANT_UI_H, [255, 0, 0, 255])
        .map_err(|e| format!("种不进替界面那一行：图拼不出（{e}）"))?;
    let ui_meta = falcon_core::history::CaptureMeta {
        at_ms,
        offset_min: platform_windows::clock::wall_clock().1,
        monitor_id: "本档替界面种的那一块",
        rect: falcon_core::PhysRect::new(3, 5, PLANT_UI_W, PLANT_UI_H),
        source: falcon_core::history::Source::Manual,
        cancelled: false,
    };
    let frame = falcon_core::frame::Frame::filled(PLANT_W, PLANT_H, [0, 255, 0, 255])
        .map_err(|e| format!("种不进那一行：图拼不出（{e}）"))?;
    let meta = falcon_core::history::CaptureMeta {
        at_ms,
        offset_min: platform_windows::clock::wall_clock().1,
        monitor_id: "本档种的那一块",
        rect: falcon_core::PhysRect::new(7, 9, PLANT_W, PLANT_H),
        source: falcon_core::history::Source::Manual,
        cancelled: false,
    };
    let cfg = falcon_core::config::History {
        enabled: true,
        keep_originals: true,
        thumb_px: 0,
        max_items: 0,
        max_age_days: 0,
        max_size_mb: 0,
        save_all: true,
        exempt_locked: false,
    };
    let mut store = falcon_core::history::History::open(&hdir)
        .map_err(|e| format!("种不进那一行：打不开 {}（{e}）", hdir.display()))?;
    // P115：界面那一条腿自此还要**一次点名两行** —— §5.14.4 步骤 2 那句「选择单条、多条或全部」
    // 里的「多条」。这两枚排在**最前**种，而「排在 `planted` 之前」只是必要条件、不是充分条件：
    // 六枚共用上面那枚 `at_ms`，时间这一维打平，行号就是捕获顺序，而第 32 格那把邻居尺子按 #197
    // 取在 `engine.load()` **之后** —— 它读的是界面那四条腿删完之后的库。首趟门（`p115_auto_A.log`
    // 与 `…_B.log` 的第 60 行）把这点照出来了：那时这两枚的行号是 4、5，正夹在锚点（`back`=3）与
    // `planted`(6) 之间，于是界面那句读 `next=4`（删之前）、尺子读 `next=6`（删之后），第 32 格
    // 因此红了一格 —— 红的不是产品的邻居算法，是本档把两把尺子摆在同一条链的两端。挪到老端 ⇒
    // 锚点的 `next` 自此恒等于 `planted`，这两枚删不删都一样答；`planted` 仍是最大行号，清单头
    // 一段照旧点名它。
    //
    // 一枚摆不出来时也不牵连整条界面腿：两枚行号一起留 0 ⇒ QML 那一头躲开批量删除，新格按
    // 「前置没摆出来」判 BLOCKED，而 P98/P101/P103 那几格照原样跑。
    let ((batch_a, batch_a_file, batch_a_bytes), (batch_b, batch_b_file, batch_b_bytes), batch_extra) =
        match (
            plant_row_with_bytes(
                &mut store,
                &cfg,
                falcon_core::PhysRect::new(51, 3, PLANT_BATCH1_W, PLANT_BATCH1_H),
                "本档替界面一次点名的第一枚",
                [214, 40, 40, 255],
                at_ms,
            ),
            plant_row_with_bytes(
                &mut store,
                &cfg,
                falcon_core::PhysRect::new(5, 61, PLANT_BATCH2_W, PLANT_BATCH2_H),
                "本档替界面一次点名的第二枚",
                [40, 214, 40, 255],
                at_ms,
            ),
        ) {
            (Ok(a), Ok(b)) => (a, b, String::new()),
            (a, b) => (
                (0, None, 0),
                (0, None, 0),
                format!(
                    " 界面那一条腿的批量两枚没摆全（第一枚={a:?}、第二枚={b:?}），一次点名两行那一格据此判 BLOCKED"
                ),
            ),
        };
    // 替界面那一枚在批量两枚**之后**种、在 `planted` **之前**种。顺序不是随手排的：`newest` 排的
    // 是 `captured_at DESC, id DESC`，而清单那一格要求递回来的**第一段**点名的必须是 `planted`
    // 那一行 —— 这一场里六枚行号共用上面那一枚 `at_ms`，时间这一维打平，因此**只有**行号说话，
    // 后种的每一枚都比前一枚大，`planted` 最后种、行号最大，稳稳排在清单头上。换个个儿种，那一
    // 格就会读成「界面把替它种的那一行当成了清单的第一行」。批量那两枚为什么反倒要占最小的两枚
    // 行号，见上面 P115 那一段（首趟门的第 60 行就是它的证人）。
    let ui_row = store
        .record(&ui_meta, &ui_frame, &cfg)
        .map_err(|e| format!("种不进替界面那一行：写不进库（{e}）"))?;
    let ui_file = store
        .get(ui_row)
        .map_err(|e| format!("种下第 {ui_row} 行却读不回它指哪份文件（{e}）"))?
        .and_then(|rec| store.path_of(&rec));
    // P103：界面那一条腿自此还要**恢复**一行，而恢复要读的是「框被挪走、又被放回」。两枚行号
    // 各管一问：[`PLANT_PICK_W`]×[`PLANT_PICK_H`] 那一枚的矩形与此刻这只框不同（挪走那一问），
    // 第二枚的矩形就是此刻这只框本身（放回那一问）。它们排在 `planted` **之前**种，与 ui_row
    // 同一枚理由 —— 六枚共用同一枚 `captured_at`，清单头一段必须仍点名 `planted`。它们与
    // `planted` 之间原先还夹着批量那两枚，P115b 把那一双挪到了最前（理由见上面那段，证人是首趟
    // 门的第 60 行）。
    //
    // 摆不出来时不牵连整条界面腿：行号留 0 ⇒ QML 那一头躲开恢复，恢复那一格按「前置没摆出来」
    // 判 BLOCKED，而 P98/P101 那四格照原样跑。把一枚矩形拼不出图讲成整个门失败，是让新的一格
    // 去没收旧的格子已经挣到的读数。
    let hole = mask::with(|m| m.hole);
    let pick = falcon_core::PhysRect::new(11, 13, PLANT_PICK_W, PLANT_PICK_H);
    let (pick_row, back_row, extra) = match (
        plant_selection_row(
            &mut store,
            &cfg,
            pick,
            "本档替界面挑的那一块",
            [0, 0, 255, 255],
            at_ms,
        ),
        plant_selection_row(
            &mut store,
            &cfg,
            hole,
            "本档替界面放回原处的那一块",
            [255, 255, 0, 255],
            at_ms,
        ),
    ) {
        (Ok(p), Ok(b)) => (p, b, String::new()),
        (p, b) => (
            0,
            0,
            format!(
                " 界面那一条腿的恢复两枚没摆全（挑={p:?}、放={b:?}），恢复那一格据此判 BLOCKED"
            ),
        ),
    };
    let extra = format!("{extra}{batch_extra}");
    let id = store
        .record(&meta, &frame, &cfg)
        .map_err(|e| format!("种不进那一行：写不进库（{e}）"))?;
    let rows = store
        .count()
        .map_err(|e| format!("种下了第 {id} 行却数不出库里有几行（{e}）"))? as usize;
    // 基准取在这一刻：界面那一条腿还没跑（引擎还没 load），而 `measure()` 里任何一次读计数都
    // 已经跑在它后面了。不记这一枚，回放那一格就没有能说的基准。
    let (replays, dels, clears, locks, restores, flips) = mask::with(|m| {
        (
            m.history_replays,
            m.history_deletes,
            m.history_clears,
            m.history_locks,
            m.history_restores,
            m.history_flips,
        )
    });
    with_plan(|p| {
        p.planted = id;
        p.ui_row = ui_row;
        p.ui_file = ui_file.clone();
        p.pick_row = pick_row;
        p.back_row = back_row;
        p.batch_rows = (batch_a, batch_b);
        p.batch_files = (batch_a_file.clone(), batch_b_file.clone());
        p.batch_bytes = (batch_a_bytes, batch_b_bytes);
        p.hole = hole;
        p.restores = restores;
        p.flips = flips;
        p.rows = rows;
        p.replays = replays;
        p.dels = dels;
        p.clears = clears;
        p.locks = locks;
        p.lines.clear();
    });
    Ok(format!(
        "[autosave arm] 种进 {} 的第 {id} 行：{PLANT_W}×{PLANT_H}，库里现有 {rows} 行；\
         替界面种的第 {ui_row} 行：{PLANT_UI_W}×{PLANT_UI_H}，它指 {ui_file:?}；\
         替界面挑的第 {pick_row} 行：{pick}，替界面放回的第 {back_row} 行：{hole}\
         （界面那一条腿自此先挑走、再放回）；替界面**一次点名**的两枚：第 {batch_a} 行\
         （{PLANT_BATCH1_W}×{PLANT_BATCH1_H}，删前 {batch_a_bytes} 字节，指 {batch_a_file:?}）\
         与第 {batch_b} 行（{PLANT_BATCH2_W}×{PLANT_BATCH2_H}，删前 {batch_b_bytes} 字节，\
         指 {batch_b_file:?}）—— §5.14.4 步骤 2 的「多条」自此有对面；产品此刻已数到 {replays} 次回放、\
         {dels} 次删除、{clears} 次清空、{locks} 次上锁、{restores} 次恢复；\
         界面那一条腿读最多 {HIST_LIMIT} 行{extra}",
        hdir.display()
    ))
}

/// `arm()` 种下的那一行的行号（0 = 没种）。
fn planted_row() -> i64 {
    with_plan(|p| p.planted)
}

/// 种完那一刻库里实际有几行。
fn planted_rows() -> usize {
    with_plan(|p| p.rows)
}

/// 种完那一刻产品自己已经数到的回放次数（界面那一条腿的基准）。
fn planted_replays() -> u32 {
    with_plan(|p| p.replays)
}

/// `arm()` 替界面种下的那一行的行号（0 = 没种）。下面新加的那两格只问这一枚，别的行号与它无关。
fn planted_ui_row() -> i64 {
    with_plan(|p| p.ui_row)
}

/// 替界面种的那一行**本来**指的那份原图（`arm()` 当场从库里问回来的路径，不查它此刻在不在）。
fn planted_ui_file() -> Option<PathBuf> {
    with_plan(|p| p.ui_file.clone())
}

/// `arm()` 替界面**挑**的那一行的行号（0 = 没种）。P103 起。
fn planted_pick_row() -> i64 {
    with_plan(|p| p.pick_row)
}

/// `arm()` 替界面**放回原处**的那一行的行号（0 = 没种）。P103 起。
fn planted_back_row() -> i64 {
    with_plan(|p| p.back_row)
}

/// `arm()` 替界面**一次点名**的那两枚行号，成对才有意义（`(0, 0)` = 没种）。P115 起。
fn planted_batch_rows() -> (i64, i64) {
    with_plan(|p| p.batch_rows)
}

/// 那两行**本来**各自指的那份原图（`arm()` 当场从库里问回来的路径，不查它此刻在不在）。P115 起。
fn planted_batch_files() -> (Option<PathBuf>, Option<PathBuf>) {
    with_plan(|p| p.batch_files.clone())
}

/// 那两行删前各占多少字节——缩略图长度加原图文件大小，与 `history.rs` 里 `bytes_of` 同一个口径，
/// 所以产品那句"收回 N 字节"能被本档**独立**加出来对一次。P115 起。
fn planted_batch_bytes() -> (u64, u64) {
    with_plan(|p| p.batch_bytes)
}

/// `arm()` 那一刻遮罩上那一只框。界面那条腿"放回来没有"只与这一枚比：`measure()` 里其余各格
/// 读到的 `m.hole` 全跑在 `engine.load()` **之后**，拿它们当基准就成了让界面自己判自己（上面
/// 三枚计数基准同一条理由）。P103 起。
fn planted_hole() -> falcon_core::PhysRect {
    with_plan(|p| p.hole)
}

/// `arm()` 那一刻的恢复次数基准。P103 起。
fn planted_restores() -> u32 {
    with_plan(|p| p.restores)
}

/// `arm()` 那一刻的翻页次数基准。P113 起：翻页与直接按的恢复吃同一枚 `history_restores`，
/// 界面那两条腿各自要问"我这一次被数进去了没有"，就得把总数拆成两半各问一半。
fn planted_flips() -> u32 {
    with_plan(|p| p.flips)
}

/// `arm()` 那一刻的三枚计数基准（删除 / 清空 / 上锁）。界面那一条腿跑在 `measure()` 之前，
/// 这三枚数就是它能把它们抬高多少的那把尺子的零位。
fn planted_counts() -> (u32, u32, u32) {
    with_plan(|p| (p.dels, p.clears, p.locks))
}

/// §5.14.3「恢复该截图内容」那一条腿**不许动**的那些枚，一次读成一枚字段具名的结构体
/// （P116 起）。不用元组，是因为本格要按**名字**答「这三趟里动的是哪一枚」：元组只能按位置答，
/// 而把位置读成字段正是本轮在册的那族形状（#245/#247 —— 同一枚元组，标签说一个数、值是另一个数）。
/// 每枚字段后面的那一句就是"为什么这一枚在本格对面"，缺了它这十四枚就读成一份 arbitrary 快照。
#[derive(PartialEq)]
struct Untouched {
    /// 屏幕上那一只框：本腿只往剪贴板递一张图，不替用户改选区（改选区是第 17 格那条腿）。
    hole: falcon_core::PhysRect,
    /// 界面此刻停在第几行：本腿不翻页。
    viewed: Option<i64>,
    /// 回进原选区的次数（含翻页那一半）。
    restores: u32,
    flips: u32,
    /// 把图读回来**再落一次盘**的次数：本腿一份也不落，这是它与 §5.14.2 回放那条腿的分界。
    replays: u32,
    records: u32,
    last_id: Option<i64>,
    deletes: u32,
    clears: u32,
    locks: u32,
    /// 默认行为那一次递出去的次数：本腿递的是**用户点名的一行**，记进这一枚就是把两条腿并成一枚
    /// 计数，从此问不出「这一场递出去的哪一张是历史」（`history_copies` 单列的理由在 `mask.rs`）。
    auto_copies: u32,
    quick_saves: u32,
    /// 屏幕上那句话的槽位：本腿不写盘，因此也不占那一行（六条失败各有各的话，各占自己那一格）。
    message: String,
    /// 快速保存的落点：同上，本腿不产生落点。
    landed: Option<std::path::PathBuf>,
}

impl Untouched {
    fn read(m: &mut crate::mask::MaskState) -> Self {
        Untouched {
            hole: m.hole,
            viewed: m.history_viewed,
            restores: m.history_restores,
            flips: m.history_flips,
            replays: m.history_replays,
            records: m.history_records,
            last_id: m.history_last_id,
            deletes: m.history_deletes,
            clears: m.history_clears,
            locks: m.history_locks,
            auto_copies: m.auto_copies,
            quick_saves: m.quick_saves,
            message: m.message.clone(),
            landed: m.landed.clone(),
        }
    }

    /// 两枚读数之间动了哪几枚，按字段名答；空 = 一位也没动。判据要的是这份**名字**而不是
    /// 「快照不相等」：本格对面有十四枚，说不出是哪一枚的红，就无法与上面那 16 格任何一格的
    /// 红区分开，而这道门的每一条腿都靠"红在谁那一格"来归因。
    fn moved(&self, after: &Self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.hole != after.hole {
            out.push("hole");
        }
        if self.viewed != after.viewed {
            out.push("viewed");
        }
        if self.restores != after.restores {
            out.push("restores");
        }
        if self.flips != after.flips {
            out.push("flips");
        }
        if self.replays != after.replays {
            out.push("replays");
        }
        if self.records != after.records {
            out.push("records");
        }
        if self.last_id != after.last_id {
            out.push("last_id");
        }
        if self.deletes != after.deletes {
            out.push("deletes");
        }
        if self.clears != after.clears {
            out.push("clears");
        }
        if self.locks != after.locks {
            out.push("locks");
        }
        if self.auto_copies != after.auto_copies {
            out.push("auto_copies");
        }
        if self.quick_saves != after.quick_saves {
            out.push("quick_saves");
        }
        if self.message != after.message {
            out.push("message");
        }
        if self.landed != after.landed {
            out.push("landed");
        }
        out
    }
}

/// 开一次遮罩、按一次完成、把盘上前后两次普查的差读回来。`asked_ms` 只印不判。
pub fn measure(asked_ms: i32) -> (Check, String) {
    let mut rep = Report::new();

    // ---- P114：第 32 格的那把尺子**一进门就取**，一等不到格子里现取（格子里为什么不敢问，
    // 判据那一行自己说）。`engine.load()` 已经跑完 ⇒ 这一刻是本档离界面那一条腿的尽头最近的
    // 一刻；此后 `measure()` 自己还要删行、清库、再落两张图，而第 32 格排在那一串之后 —— 深处
    // 再开一次库，读回的是本档改过的库。这是 #197 那一课的形状（读数时刻要贴着被问的那件事），
    // 也是 P99 那句「一条腿读单调递增的计数时，先问它读的是事件之前还是之后」的同一条。
    let neighbors_witness = neighbors_from_disk(&hist_dir(), planted_back_row());

    // ---- 先证明自己不是量具：下面每一格区分"没跑"与"被拦"全靠这一格。
    let argv: Vec<String> = std::env::args().collect();
    let gauge = mask::MaskState::gauge_run();
    rep.row(
        "auto door is not a gauge",
        Check::from(!gauge),
        format!(
            "argv {argv:?}；gauge_run 答 {gauge}（要求 false）——这一档若进了 GAUGE_FLAGS，\
             下面每一格读到的都是自己没跑"
        ),
    );

    let (read, write) = read_and_write();
    let want = pinned_path();
    rep.row(
        "auto door pinned its settings writes",
        Check::from(write == want && read != write),
        format!(
            "本进程读自 {}、写回 {}（要求后者 == {} 且两者各是各的文件；钉的是 `main` 在遮罩开起来\
             之前那一次 `pin`，所以走完产品的 `close()` 也不会碰到这份被临时翻成 true 的配置）",
            read.display(),
            write.display(),
            want.display()
        ),
    );

    // ---- 读者与那把键的对应关系，问的是文件而不是默认值：产品 open 时读到的开关，必须
    // 等于这份档里 `[output] auto_save` 那一行此刻写的字。这是这一轮新加的那一行代码在
    // 运行期唯一的直接证据——单测里 `m.auto_save = true` 是手摆的，摆不出"键到字段"这一段。
    let file_save = value_in_file(&read, "auto_save");
    let file_copy = value_in_file(&read, "auto_copy");
    let before = mask::with(|m| {
        (
            m.auto_save,
            m.auto_copy,
            m.hole,
            m.naming.clone(),
            m.quick_saves,
            m.auto_copies,
            m.landed.clone(),
        )
    });
    let (switch, switch_copy, hole, naming_before, saves_before, copies_before, landed_before) =
        before;
    rep.row(
        "auto switch is the file's own row",
        Check::from(file_save == Some(switch)),
        format!(
            "本进程读的是 {read:?}，那一份里 `[output] auto_save` 的字面值={file_save:?}，遮罩 \
             open 拿到的开关={switch}（要求两者相等）；落点从配置那份 {naming_before:?} 换成 \
             {}",
            door_dir().display()
        ),
    );
    // ---- P59 起第二枚开关。档里**没有那一行**是一种合法的读法而不是缺口：`Config` 的
    // `#[serde(default)]` 说「缺的行由 `Default` 答」，所以要求换成「档里那一行，或者没有那一行
    // 时的 `Default`」与字段相等——把缺行直接判成 FAIL，等于要求每个用户的档里都得手写一枚
    // 从没改过的开关。
    let want_copy = file_copy.unwrap_or_else(|| falcon_core::config::Output::default().auto_copy);
    rep.row(
        "auto copy switch is the file's own row or its default",
        Check::from(want_copy == switch_copy),
        format!(
            "同一份 {read:?} 里 `[output] auto_copy` 的字面值={file_copy:?}（缺行时按 `Default` \
             要求 {want_copy}），遮罩 open 拿到的开关={switch_copy}（要求两者相等）；这一格问的\
             不是「剪贴板上有没有图」，而是「档里那一行到不到得了这个字段」"
        ),
    );

    // ---- 剪贴板的**前**一次读数，和这一次确认将要递的那张图的尺寸。尺寸在动手之前取：
    // 事后从剪贴板上量出来的那张图，对面是系统写回来的字节，而"递进去的是不是这张拼出来的图"
    // 要的对照物只能来自缝合那一位本身。
    //
    // `usable()` 先答一次：§7.2 要的是**说出谁占着**，`state::clipboard_selftest` 也是先问这一
    // 格再动手。锁屏会话里这一格答"占用窗口找不到"是对面不给看，不是本程序的剪贴板代码坏了 ⇒
    // 下面三格都判 BLOCKED 而不是 FAIL。
    let blind = platform_windows::clip::usable()
        .err()
        .map(|e| e.to_string());
    let clip_before = platform_windows::clip::read().ok();
    let stitched = mask::with(|m| m.delivered().map(|f| (f.width, f.height)));
    rep.row(
        "auto door read the clipboard before it touched it",
        if blind.is_some() {
            Check::Blocked
        } else {
            Check::from(clip_before.is_some() && stitched.is_some())
        },
        format!(
            "`usable()` 先答={blind_s}；剪贴板读回={}（`None` 是「它没为读而打开」，不是「它是空的」），\
             缝合那一位此刻交回的图={stitched:?}；用户原来的内容若读不到，本档后面就没有东西可以放\
             回去——那一句要在这一格里说，不能等跑完再说",
            match &clip_before {
                None => "None".to_string(),
                Some(c) => format!(
                    "Some(image={:?} text={} chars files={})",
                    c.image_bytes.as_ref().map(|b| b.len()),
                    c.text.as_ref().map(|t| t.chars().count()).unwrap_or(0),
                    c.files.len()
                ),
            },
            blind_s = match &blind {
                None => "能递".to_string(),
                Some(w) => w.clone(),
            },
        ),
    );

    // ---- 换落点：配置的 `[output] save_dir` 是空的，空的答法是用户的图片文件夹（§6.2），
    // 一档自动保存不该往那里写（§9.1 第十五条）。只换 dir，模板、前缀与碰撞规则都留着，
    // 所以盘上那一个名字是**用户的命名规则**给的，而不是本档挑的。
    //
    // P93 起这道门也走 §5.14.1 的那一行历史。P93 把它撤成 `None`（那轮判的是自动保存与
    // 默认复制，不是历史，而撤之前每跑一趟就会在用户的 `%APPDATA%\ai-falconshot\history\`
    // 里长出一行）；**P94 起不再撤，而是引到 [`hist_dir`] 那枚 temp 沙盒** —— §5.14.1 第一条
    // "成功截图应写入历史记录"到 P93 为止只有 cargo 单测证人，没有一次运行期读数，而这台
    // 机器上真长出一行才是那一格的面。两把尺子分开问：`count(&dir)` 数目录里的文件，
    // [`history_rows`] 数 SQLite 里的行——把历史的原图与 `history.db` 混进前者就是把两件事
    // 量成一件。
    //
    // 三枚上限钉成 0（0 = 该条不生效，见 `Retention`）而 `enabled` 钉成 `true`：出厂那三枚
    // （1000 行 / 90 天 / §5.14.4 的清理每次落行都真跑）在这里会把刚写那一枚挤掉，于是
    // "行数加一"量的就不再是写路径而是用户的配额算得对不对。配额与 `enabled` 那两格各有
    // 自己的证人（`Retention` 的单测、P68 的 `history_open_in`），本档只钉这一条：门开了就
    // 该有一行落在盘上。
    //
    // P96 起同处再钉一枚 `save_all`（§5.14.1 第二条，P95 造出来的那枚开关）。它和上面三枚
    // 上限是同一族做法：出厂为关，不钉开这一格就永远只读得到反面。它只管 `escape()` 那一条
    // 腿（`esc_owes_row` 是它唯一的主人，P95 起），`commit()` 一眼也不看它，所以钉它不会动
    // 上面那一格的读数——两条腿共用一个写者而各判一格，正是这一档要把它们分开量的理由。
    //
    // P98 起这一段整段搬进 [`retarget`]，因为它的调用点从一处变成两处（`arm()` 在
    // `engine.load()` 之前也要钉一次）。这里再钉一遍是**幂等**的：同一对路径、同一份
    // `[history]` 快照，所以下面三枚前置 `files_before`/`rows_before`/`records_before` 的
    // 算术一位也不动。
    let (dir, hdir) = retarget();
    let files_before = count(&dir);
    let rows_before = history_rows(&hdir);
    let records_before = mask::with(|m| m.history_records);

    // ---- 一次完成：走的就是 `Enter`、双击和「确认选区」那三样共用的门。
    let returned = mask::with(|m| m.commit());
    let said = mask::with(|m| m.message.clone());
    let (landed, saves_after, records_after, recorded_said) = mask::with(|m| {
        (
            m.landed.clone(),
            m.quick_saves,
            m.history_records,
            m.history_last.clone(),
        )
    });
    let files_after = count(&dir);
    let rows_after = history_rows(&hdir);
    let remembered = capture::last_region();

    let in_dir = landed
        .as_ref()
        .is_some_and(|p| p.parent() == Some(dir.as_path()) && p.exists());
    let named = landed
        .as_ref()
        .and_then(|p| {
            p.file_name()
                .map(|n| said.contains(&n.to_string_lossy().into_owned()))
        })
        .unwrap_or(false);
    let wrote = match switch {
        true => {
            files_after == files_before + 1
                && landed.is_some()
                && in_dir
                && named
                && saves_after == saves_before + 1
        }
        false => {
            files_after == files_before && landed_before.is_none() && saves_after == saves_before
        }
    };
    rep.row(
        "confirmation wrote as the switch says",
        Check::from(wrote),
        format!(
            "开关={switch}；目录 {} 普查 {files_before}→{files_after}，landed={landed:?}（在本档目录里\
             且在盘上={in_dir}），那句话={said:?}（点名那个文件={named}），产品自己数的落盘次数 \
             {saves_before}→{saves_after}。要求开着时盘上多一张且只多一张，关着时一张也不多——这一格\
             对面是文件系统，不是这条进程里的一个数",
            dir.display()
        ),
    );

    // ---- §5.14.1 第一条的运行期读数。三个数各说一件事：SQLite 里的行数是**盘上**的，
    // `history_records` 是产品自己数的，`history_last` 是它对这一格说的话。只有行数真的
    // 加一才算"落了盘"——次数加一而行数不动，就是它自己数了一笔却没写出去。
    let rows_delta = match (&rows_before, &rows_after) {
        (Ok(b), Ok(a)) => Ok(*a as i64 - *b as i64),
        (Err(e), _) => Err(format!("进场就读不回：{e}")),
        (_, Err(e)) => Err(format!("收场读不回：{e}")),
    };
    let rows_b = match &rows_before {
        Ok(n) => n.to_string(),
        Err(e) => format!("读不回（{e}）"),
    };
    let rows_a = match &rows_after {
        Ok(n) => n.to_string(),
        Err(e) => format!("读不回（{e}）"),
    };
    let recorded = rows_delta == Ok(1)
        && records_after == records_before + 1
        && recorded_said.contains("历史记了一行");
    rep.row(
        "confirmation recorded a history row",
        Check::from(recorded),
        format!(
            "沙盒 {}：SQLite 里的行数 {rows_b}→{rows_a}（差={}），产品自己数的记录次数 \
             {records_before}→{records_after}，它说的那句话={recorded_said:?}，这一场落盘的图={landed:?}。\
             要求行数加一、次数加一、那句话点名那一行；§5.14.1 第一条「成功截图应写入历史记录」\
             不看 `[output] auto_save` 那枚开关，开关关着时也照样要落一行——这一格对面是 SQLite，\
             不是这条进程里的一个数",
            hdir.display(),
            match &rows_delta {
                Ok(d) => d.to_string(),
                Err(e) => e.clone(),
            },
        ),
    );

    // ---- P59 起这一格对面是剪贴板本身，一次读回三样：产品自己数的次数、它说的那句话、
    // 系统此刻递回来的那张图的尺寸。尺寸对照的是**动手之前**缝合那一位交回的那一张（`stitched`），
    // 问的是"递进去的是不是这张拼出来的图"，而不是"剪贴板上现在有没有一张图"。
    let clip_after = platform_windows::clip::read().ok();
    let (copies_after, copy_said) = mask::with(|m| (m.auto_copies, m.auto_copy_last.clone()));
    let img_before = clip_before.as_ref().and_then(|c| c.image_bytes.clone());
    let img_after = clip_after.as_ref().and_then(|c| c.image_bytes.clone());
    let dims_after = clip_after
        .as_ref()
        .and_then(|c| c.image_bytes.as_ref())
        .and_then(|b| falcon_core::encode::decode(b).ok())
        .map(|f| (f.width, f.height));
    let copied = match switch_copy {
        false => copies_after == copies_before && copy_said.is_empty() && img_before == img_after,
        true if copies_after == copies_before + 1 => dims_after == stitched,
        true => false,
    };
    // 开关开着而次数没动、话说了：那是本程序当场**拒**了这一次递（剪贴板被别的窗口占着，
    // §7.2 要求说出谁占着）。拒是一次读数，不是这一档坏了 ⇒ BLOCKED 而不是 FAIL。
    let refused = switch_copy && copies_after == copies_before && !copy_said.is_empty();
    rep.row(
        "confirmation copied as the switch says",
        if blind.is_some() || refused {
            Check::Blocked
        } else {
            Check::from(copied)
        },
        format!(
            "开关={switch_copy}；产品自己数的递出次数 {copies_before}→{copies_after}，它说的那句话=\
             {copy_said:?}；剪贴板此刻递回的图={dims_after:?}（字节 {img_after_len} 个），动手之前缝合\
             那一位交回的图={stitched:?}；进场时那份图的字节={img_before_len} 个。要求开着时次数加一\
             且剪贴板上那张图的尺寸就是拼出来的那一张，关着时次数不动、话也没说、那一份字节一个也没\
             变{tail}",
            img_after_len = img_after.as_ref().map(|b| b.len()).unwrap_or(0),
            img_before_len = img_before.as_ref().map(|b| b.len()).unwrap_or(0),
            tail = if blind.is_some() {
                "——对面不为读打开，那一份字节本档看不见，判 BLOCKED 而不是 FAIL"
            } else if refused {
                "——这一趟是本程序自己拒的，理由在上面那句话里"
            } else {
                ""
            }
        ),
    );

    // ---- 欠的那一笔。`put_back` 只在开关开着**且看得见那份东西**时叫：关着时本档既没递也没清，而它的 empty 分支
    // 会来一次 `EmptyClipboard`——那一下能把用户此刻的东西清掉，一次没发生的拿走不该拿"还清全部"
    // 当找回；**看不见**时同样的理由更硬，因为这时候 `auto_copy_out` 自己也先问 `usable()` 而拒了递。
    // 还没还上不由本程序说了算，读回一次系统递的东西对一对。
    let unrepresentable = clip_before
        .as_ref()
        .is_some_and(|c| c.html.is_some() || !c.files.is_empty());
    let restored = if switch_copy && blind.is_none() {
        Some(crate::state::put_back(&clip_before))
    } else {
        None
    };
    let clip_now = platform_windows::clip::read().ok();
    let back_same = match (&clip_before, &clip_now) {
        (Some(a), Some(b)) => a.image_bytes == b.image_bytes && a.text == b.text,
        _ => false,
    };
    let said_back = match &restored {
        None => match &blind {
            Some(who) => format!("没递也没清：对面不为读打开（{who}）"),
            None => "开关关着：这一档没递也没清，欠的是零".to_string(),
        },
        Some(Ok(s)) => s.clone(),
        Some(Err(e)) => format!("没还上：{e}"),
    };
    let owed = match (&restored, &blind) {
        // 看不见那份东西，就既不能说还清也不能说没还。本档确实没欠（递那一步自己拒了），
        // 但这一格量不到 ⇒ BLOCKED，并把"解锁会话再跑一趟"记进本轮欠账。
        (_, Some(_)) => Check::Blocked,
        (None, _) => Check::Pass,
        (Some(Err(_)), _) => Check::Fail,
        // 进场时那份里有 html 或文件列表：`put_back` 表示不了那一份，而递图那一下已经把它顶掉
        // 了。这是这台机器此刻的状态给的限制，不是这档的判定 ⇒ BLOCKED，并把它记进本轮欠账。
        (Some(Ok(_)), _) if unrepresentable => Check::Blocked,
        (Some(Ok(_)), _) => Check::from(back_same),
    };
    rep.row(
        "auto door left the clipboard as it found it",
        owed,
        format!(
            "`put_back` 那句={said_back:?}；还回去之后系统递回的图字节=\
             {now_img}（进场时 {img_before_len2} 个），文本={now_text} 字（进场时 {before_text} 字）\
             ，两份一致={back_same}；进场时那份里有 html={had_html}、文件列表={had_files} 项{tail}",
            now_img = clip_now
                .as_ref()
                .and_then(|c| c.image_bytes.as_ref())
                .map(|b| b.len())
                .unwrap_or(0),
            img_before_len2 = img_before.as_ref().map(|b| b.len()).unwrap_or(0),
            now_text = clip_now
                .as_ref()
                .map(|c| c.text.as_ref().map(|t| t.chars().count()).unwrap_or(0))
                .unwrap_or(0),
            before_text = clip_before
                .as_ref()
                .map(|c| c.text.as_ref().map(|t| t.chars().count()).unwrap_or(0))
                .unwrap_or(0),
            had_html = clip_before.as_ref().map(|c| c.html.is_some()).unwrap_or(false),
            had_files = clip_before
                .as_ref()
                .map(|c| c.files.len())
                .unwrap_or(0),
            tail = match &blind {
                Some(_) => "——看不见那份东西就既没说还清、也没叫 put_back 去清它",
                None if unrepresentable => "——那一份递不回去，判 BLOCKED 并记进本轮欠账",
                None if clip_before.is_none() && switch_copy => {
                    "——进场时剪贴板就没为读打开，那份东西本来就不可知，没得还"
                }
                None => "",
            }
        ),
    );

    // ---- 顺序那一格的反面读法：一次自动保存不吃掉这次确认。选区照旧交回、照旧被记成
    // "上一条在哪里"（§5.2.5 的重复上次截图读的就是这一格）。
    let same_rect = returned == Some(hole);
    let remembered_same = remembered
        .as_ref()
        .is_some_and(|r| r.rect == hole && r.leg == "遮罩选区");
    rep.row(
        "confirmation is still a confirmation",
        Check::from(same_rect && remembered_same),
        format!(
            "open 时的选区={hole}，commit 交回={returned:?}（要求同一个），capture::last_region() \
             答 {remembered:?}（要求同一条矩形、leg=「遮罩选区」）；这一档被要求等 {asked_ms} ms"
        ),
    );

    // ---- §5.14.1 第二条的运行期读数。P95 在这一条上造了 `[history] save_all`、
    // `esc_owes_row` 那道门与 cargo 三腿证人，可它到本轮为止**一次也没在运行期落过一行**：
    // `--selftest --history` 那一档开的是用户 `%APPDATA%\ai-falconshot\history.db`，本档借不了
    // （§9.1 第十五条）。所以这一格在进程里直接调 `escape()`——不是模拟按键、不进热键、
    // 不碰用户会话——判据仍旧是三把尺子各说一件事：盘上的行数、盘上那一行自己写的
    // `cancelled` 与它记下的选区、以及产品对这一格说的那句话。它排在全部既有格之后，
    // 因为 `escape()` 会清掉选区，而上面每一格量的都是「这一次确认」——先让完账，再拆桥。
    let esc_rows_before = history_rows(&hdir);
    let (esc_records_before, commit_row) = mask::with(|m| (m.history_records, m.history_last_id));
    let files_before_esc = count(&dir);
    let (saves_before_esc, landed_before_esc) = mask::with(|m| (m.quick_saves, m.landed.clone()));

    let ate = mask::with(|m| m.escape());
    let (hole_after_esc, esc_said, esc_records_after, esc_row) = mask::with(|m| {
        (
            m.hole,
            m.history_last.clone(),
            m.history_records,
            m.history_last_id,
        )
    });
    let esc_rows_after = history_rows(&hdir);
    let files_after_esc = count(&dir);
    let (saves_after_esc, landed_after_esc) = mask::with(|m| (m.quick_saves, m.landed.clone()));

    // 第一格问的是**反面**：被取消的这一次不该在文件系统上留下任何东西。§5.5.3 那句
    // 「完成截图后，系统自动生成文件」里的"完成"不含 Esc 清掉的那一枚——它的账记在下面
    // 那一格，不记在盘上。选区被吃掉本身也是这一格的读数：`escape()` 返回 `true` 才算
    // 它吃了自己（返回 `false` 时调用方要顺势取消整场，那时下面两格根本不该有行落库）。
    let cleared = ate
        && hole_after_esc.is_empty()
        && files_after_esc == files_before_esc
        && saves_after_esc == saves_before_esc
        && landed_after_esc == landed_before_esc;
    rep.row(
        "escape cleared the selection without writing a file",
        Check::from(cleared),
        format!(
            "进场时选区={hole}，`escape()` 答={ate}（要求 true：这一场有选区可清），收场选区=\
             {hole_after_esc}（要求空）；目录 {} 普查 {files_before_esc}→{files_after_esc}，产品自己数的\
             落盘次数 {saves_before_esc}→{saves_after_esc}，`landed`={landed_after_esc:?}（先前那一份=\
             {landed_before_esc:?}）——三者都要求一动不动。这一格对面也是文件系统：一次取消只欠历史\
             一行，不欠一张图",
            dir.display()
        ),
    );

    // ---- 行数那把尺子先说它自己读不读得回，和上面确认那一格同一口径：读不回不是零行。
    let esc_delta = match (&esc_rows_before, &esc_rows_after) {
        (Ok(b), Ok(a)) => Ok(*a as i64 - *b as i64),
        (Err(e), _) => Err(format!("进场就读不回：{e}")),
        (_, Err(e)) => Err(format!("收场读不回：{e}")),
    };
    let esc_b = match &esc_rows_before {
        Ok(n) => n.to_string(),
        Err(e) => format!("读不回（{e}）"),
    };
    let esc_a = match &esc_rows_after {
        Ok(n) => n.to_string(),
        Err(e) => format!("读不回（{e}）"),
    };
    // 那一行**自己**在库里写着什么：不看产品报的行号是否非空，而看拿那个行号去 `get` 是否
    // 真读得到一行、那一格的 `cancelled` 是不是真、它记的矩形是不是被清掉的那一枚。
    let esc_disk = match esc_row {
        Some(id) => history_row(&hdir, id),
        None => Err("产品没报出取消那一行的行号".to_string()),
    };
    let flagged = esc_delta == Ok(1)
        && esc_records_after == esc_records_before + 1
        && esc_said.contains("历史记了一行")
        && esc_said.contains("已取消")
        && matches!(&esc_disk, Ok(Some((true, rect))) if *rect == hole);
    let esc_delta_s = match &esc_delta {
        Ok(d) => d.to_string(),
        Err(e) => e.clone(),
    };
    let esc_disk_s = row_said(&esc_disk);
    rep.row(
        "escape recorded a cancelled row",
        Check::from(flagged),
        format!(
            "沙盒 {}：SQLite 里的行数 {esc_b}→{esc_a}（差={esc_delta_s}），产品自己数的记录次数 \
             {esc_records_before}→{esc_records_after}，它说的那句话={esc_said:?}，它报的行号={esc_row:?}，\
             拿那个行号去库里读回={esc_disk_s}。要求行数加一、次数加一、那句话点名那一行且带「已取消」，\
             而盘上那一行自己写着 `cancelled=true` 且记的矩形就是被清掉的 {hole}。§5.14.1 第二条「用户可\
             开启『保存所有截图记录』，包括被取消的截图」到本轮为止只有 cargo 证人，这一格是它在运行期\
             第一次真落行；`[history] save_all` 是进场时钉成真的（出厂为关，不钉开就只读得到反面）——\
             这一格对面是 SQLite",
            hdir.display()
        ),
    );

    // ---- 最后那一格问的是 §5.14.3 的前提：用户在历史里要能**找到**标记为已取消的那几条。
    // 那一句只有在两条腿落进库里的东西不一样时才成立——确认那一格标 `false`、取消那一格标
    // `true`、两个行号不是同一个。合在一起看，上面两格才是一组读数而不是同一句重复。
    let commit_disk = match commit_row {
        Some(id) => history_row(&hdir, id),
        None => Err("确认那一格没报出行号（它自己的判据已在上面一格）".to_string()),
    };
    let only_its_own = matches!(&commit_disk, Ok(Some((false, _))))
        && matches!(&esc_disk, Ok(Some((true, _))))
        && commit_row != esc_row;
    rep.row(
        "cancellation flags only its own row",
        Check::from(only_its_own),
        format!(
            "同一份 {} 里两行各是各的：确认那格报的行号={commit_row:?}、库里读回={}；取消那格报的\
             行号={esc_row:?}、库里读回={}（要求前者 `cancelled=false`、后者 `=true`、两个行号不是同\
             一个）。§5.14.3 要用户「找到标记为已取消的记录」，靠的就是这两格在库里是**两个数**——两条腿\
             落进去长得一样的话，屏幕上那句「已取消」就只是安慰",
            hdir.display(),
            row_said(&commit_disk),
            row_said(&esc_disk)
        ),
    );

    // ---- §5.14.2 的第一格在此之前先问一件更硬的事：那一行的**图**取不取得回来。
    // P93–P96 四轮问的都是"库里有没有这一行"，而 §5.14.1 那句话里还剩"截图内容"四个字：
    // 一份行数对得上、图却取不回来的历史，下面那条产品腿（`MaskState::replay_history`）
    // 根本无从跑起。所以先由尺子问盘上，再由产品自己问一次——两问各判一格，合成一问就是
    // 把"我算过"当成"盘上有"。
    let picture = match commit_row {
        Some(id) => history_picture(&hdir, id),
        None => Err("确认那一格没报出行号，没东西可以问盘上".to_string()),
    };
    let picture_ok = matches!(&picture, Ok(Some((Some(path), bytes, Some(dims))))
        if *bytes > 0 && path.exists() && *dims == (hole.w, hole.h));
    let picture_s = picture_said(&picture);
    rep.row(
        "a recorded row still holds its picture on disk",
        Check::from(picture_ok),
        format!(
            "拿确认那一格报的行号 {commit_row:?} 去 {} 里读那一行，`path_of` 指到的那份文件、它的\
             字节数、它解码出来的宽高一起读回={picture_s}（要求字节 > 0、那份文件此刻还在、宽高就是\
             这一场被确认的 {hole}）。这一格里有两个诚实的空：`[history] keep_originals` 关着时\
             `path` 是 `None`（库里只有缩略图，那是用户自己的选择），§5.14.4 的清理把那份图收走时\
             文件不在（那是配额在动）。§5.14.1 那句「截图内容」要求的不是把这两种都说成「有」，\
             也不把它们说成库坏了——这一格对面是文件系统",
            hdir.display()
        ),
    );

    // ---- 本轮真正的新东西：产品自己第一次把某一行的图**读回来再落一次盘**
    // （[`crate::mask::MaskState::replay_history`]，§5.14.2「从历史记录中……保存」的第一条
    // 产品腿）。判据五件并列，缺一件都不算：目录里多且只多一张、它落在本档那个落点里且还在
    // 盘上、它解码出来就是这一场被确认的那一枚尺寸（同一张图，不是一张空白也不是别的图）、
    // 产品自己数的那一枚**独立**计数加一、而 `landed` 一动不动（回放不许去改 §5.5.4 那张
    // 通知的对面——那是 P96 第三格刚立的"新腿不许改老账"的同一条口径）。
    let files_before_replay = count(&dir);
    let (replays_before, landed_before_replay) =
        mask::with(|m| (m.history_replays, m.landed.clone()));
    let replayed = commit_row.and_then(|id| mask::with(|m| m.replay_history(id)));
    let (replays_after, replay_said_s, landed_after_replay) = mask::with(|m| {
        (
            m.history_replays,
            m.history_replay_last.clone(),
            m.landed.clone(),
        )
    });
    let files_after_replay = count(&dir);
    let replay_dims = replayed
        .as_ref()
        .and_then(|p| falcon_core::encode::decode_file(p).ok())
        .map(|f| (f.width, f.height));
    let replay_in_dir = replayed
        .as_ref()
        .is_some_and(|p| p.parent() == Some(dir.as_path()) && p.exists());
    let replay_named_row =
        commit_row.is_some_and(|id| replay_said_s.contains(&format!("第 {id} 行")));
    let replayed_ok = replayed.is_some()
        && replay_in_dir
        && replay_dims == Some((hole.w, hole.h))
        && files_after_replay == files_before_replay + 1
        && replays_after == replays_before + 1
        && landed_after_replay == landed_before_replay
        && replay_named_row;
    rep.row(
        "history replay wrote the recorded picture out again",
        Check::from(replayed_ok),
        format!(
            "产品拿行号 {commit_row:?} 走了一次 `replay_history`：它答落点={replayed:?}（在本档目录里\
             且在盘上={replay_in_dir}），那份文件读回来是 {replay_dims:?}（要求就是 {hole}）；目录 {}\
             普查 {files_before_replay}→{files_after_replay}，它自己数的回放次数 {replays_before}→\
             {replays_after}，那句话={replay_said_s:?}（点名那一行={replay_named_row}），而 `landed`=\
             {landed_after_replay:?}（先前那一份={landed_before_replay:?}，要求一动不动）。§5.14.2 的\
             「从历史记录中……保存」自此不再是一句写在需求里的话：这一格对面是文件系统与 SQLite 两边\
             ——图必须是从库里那一行取出来的，不是本档现拼的（这一场的选区此刻已经被 `escape()` 清掉）",
            dir.display()
        ),
    );

    // ---- 反面控制：库里没有的那一个行号，回放一次该是什么都不多。这一格要先**证明那一行
    // 确实不在**（`Ok(None)`），否则"读不回来"可能只是行号拼错、控制组自己就不成立。行号取
    // 确认那一格 + 1000：一份两行的沙盒里它不可能被清理挤出来，而它真在的话这一格当场作废。
    let missing = commit_row.map_or(1_000, |id| id + 1_000);
    let absent = matches!(history_row(&hdir, missing), Ok(None));
    let files_before_missing = count(&dir);
    let nothing = mask::with(|m| m.replay_history(missing));
    let (replays_after_missing, missing_said) =
        mask::with(|m| (m.history_replays, m.history_replay_last.clone()));
    let files_after_missing = count(&dir);
    let refused = absent
        && nothing.is_none()
        && files_after_missing == files_before_missing
        && replays_after_missing == replays_after
        && missing_said.contains("没有第")
        && missing_said.contains(&missing.to_string());
    rep.row(
        "a row that is not there replays nothing",
        Check::from(refused),
        format!(
            "行号 {missing} 先在 {} 里证一次不在（库里那一行 = `Ok(None)` 这一问的答案是 {absent}，\
             要求 true）；拿它走一次 `replay_history`：它答={nothing:?}（要求 `None`），目录普查 \
             {files_before_missing}→{files_after_missing}（要求不动），回放次数停在 \
             {replays_after_missing}（要求与上一格之后同一个数），它说的那句话={missing_said:?}\
             （要求点名「没有第 {missing} 行」）。这一格挡的是第二条腿只剩一句「我保存了」而谁都数不\
             清它到底写了几张：一个不存在的行号如果能把它推进去，前面那一格的红也就没分量了",
            hdir.display()
        ),
    );

    // ---- P102：§5.14.3 step 4 的第一条产品腿
    // （[`crate::mask::MaskState::restore_history_selection`]）。上面那两格量的是
    // `replay_history`——把库里那一行的**图**取回来、再往本档的落点落一次盘；这一格量的是同一句
    // 需求里另外的那半：「恢复该截图内容或重新进入原选区」里的**选区**。它对面此刻只有屏幕上那一
    // 只框，而那一只框在上面的 `escape()` 那一格里已经被清成空的 ⇒「此刻非空、且恰好等于库里那一行
    // 自己记着的矩形」这一读数本档摆不出来：本档自己一处也不写 `m.hole`，只读（P103 起界面的
    // 那一条腿动它，可那一次在 `engine.load()` 里就走完了，是下面那一格的账）。矩形两边各问一次 ——
    // 调用前那一枚 `esc_disk` 与调用后这一枚 `disk_after_restore` 都是从 SQLite 独立读回的，产品交
    // 回选区之后，库里那一行必须**一字不差**还是原来那一行（顺手把 `cancelled` 抹掉就等于替用户改
    // 账，而不是替他找回记录）。挑 `esc_row` 而不是 `commit_row`：§5.14.3 那四步通篇讲的是「找到
    // 标记为已取消的记录」再把它找回来，已取消的那一行才是这一节的对面，而它此刻还在库里（要等下面
    // 那次确认过的清空才走）。反面那一臂借上面已经证过「确实不在」的那枚 `missing`：一个不存在的
    // 行号如果能把它推进去，前面那一格的红也就没分量了。
    let esc_id = esc_row.unwrap_or(0);
    let want_rect = match &esc_disk {
        Ok(Some((true, rect))) => Some(*rect),
        _ => None,
    };
    let want_s = want_rect.map_or("没有".to_string(), |r| r.to_string());
    let hole_pre_restore = mask::with(|m| m.hole);
    let restores_b102 = mask::with(|m| m.history_restores);
    let (recs_b102, last_b102, replays_b102, dels_b102, clears_b102, locks_b102, saves_b102) =
        mask::with(|m| {
            (
                m.history_records,
                m.history_last_id,
                m.history_replays,
                m.history_deletes,
                m.history_clears,
                m.history_locks,
                m.quick_saves,
            )
        });
    let landed_b102 = mask::with(|m| m.landed.clone());
    let rows_b102 = history_rows(&hdir);
    let pics_b102 = picture_count(&hdir);
    let files_b102 = count(&dir);
    let restored = mask::with(|m| m.restore_history_selection(esc_id));
    let (hole_after_restore, restore_said, restores_after) =
        mask::with(|m| (m.hole, m.history_restore_last.clone(), m.history_restores));
    let disk_after_restore = history_row(&hdir, esc_id);
    let nothing_back = mask::with(|m| m.restore_history_selection(missing));
    let (hole_after_ghost, restores_after_ghost, ghost_said) =
        mask::with(|m| (m.hole, m.history_restores, m.history_restore_last.clone()));
    let (recs_a102, last_a102, replays_a102, dels_a102, clears_a102, locks_a102, saves_a102) =
        mask::with(|m| {
            (
                m.history_records,
                m.history_last_id,
                m.history_replays,
                m.history_deletes,
                m.history_clears,
                m.history_locks,
                m.quick_saves,
            )
        });
    let landed_a102 = mask::with(|m| m.landed.clone());
    let rows_a102 = history_rows(&hdir);
    let pics_a102 = picture_count(&hdir);
    let files_a102 = count(&dir);
    let moved_back = want_rect.is_some() && restored == want_rect;
    let hole_taken_back = want_rect.is_some() && Some(hole_after_restore) == want_rect;
    let still_same_row = disk_after_restore == esc_disk && matches!(&esc_disk, Ok(Some((true, _))));
    let restore_says_it = restore_said.contains("原选区")
        && restore_said.contains(format!("第 {esc_id} 行").as_str())
        && restore_said.contains("已取消");
    let nothing_moved = rows_a102.as_ref().ok() == rows_b102.as_ref().ok()
        && pics_a102 == pics_b102
        && files_a102 == files_b102
        && recs_a102 == recs_b102
        && last_a102 == last_b102
        && replays_a102 == replays_b102
        && dels_a102 == dels_b102
        && clears_a102 == clears_b102
        && locks_a102 == locks_b102
        && saves_a102 == saves_b102
        && landed_a102 == landed_b102;
    let ghost_refused = absent
        && nothing_back.is_none()
        && hole_after_ghost == hole_after_restore
        && restores_after_ghost == restores_after
        && ghost_said.contains("没有第")
        && ghost_said.contains(&missing.to_string());
    let restore_ok = want_rect.is_some()
        && hole_pre_restore.is_empty()
        && moved_back
        && hole_taken_back
        && still_same_row
        && restore_says_it
        && restores_after == restores_b102 + 1
        && nothing_moved
        && ghost_refused;
    rep.row(
        "the cancelled row handed its own selection back to the mask",
        if esc_id == 0 || want_rect.is_none() {
            Check::Blocked
        } else {
            Check::from(restore_ok)
        },
        format!(
            "调用前屏幕上那一只框={hole_pre_restore}（要求空——上面 `escape()` 那一格刚把它清掉；\
             这一格的全部分量都在这一枚「从空到非空」上）；产品拿取消那一格报的行号走了一次 \
             `restore_history_selection({esc_id})`：它答={restored:?}（要求就是库里那一行自己记着的 \
             那枚矩形 {}）；它说的那句话={restore_said:?}（要求同时说到「原选区」、「第 {esc_id} 行」\
             与「已取消」——§5.14.3 要找回的正是标着那一格的几条）；调用之后本档再读一次 `m.hole`= \
             {hole_after_restore}（要求与它交回的同一枚：那条腿得真的把选区放回屏幕上那一只框，而不\
             是只把一个数递给调用方）；调用之后再拿那个行号去 {} 里独立读回={}（要求与调用前那一问\
             一字不差且 `cancelled=true`）。反面：行号 {missing} 上面已证过不在库里（`Ok(None)`=\
             {absent}），走同一条腿它答={nothing_back:?}（要求 `None`），`m.hole` 仍是 \
             {hole_after_ghost}（要求与正面之后一字不差），恢复次数停在 {restores_after_ghost}（要求\
             与正面之后同一个数），那句话={ghost_said:?}（要求点名「没有第 {missing} 行」）。账一枚\
             也不许动：沙盒 {} 里 {rows_b_s}→{rows_a_s} 行、`history/` 下 {pics_b102}→{pics_a102} 张\
             原图、本档落点 {door_s} 普查 {files_b102}→{files_a102} 份（三枚都要求相等——这一条腿\
             一次也不往盘上写）；产品自己数的计数：恢复 {restores_b102}→{restores_after}（要求加一，\
             反面那一次不吃这一枚），记录 {recs_b102}→{recs_a102}、最后行号 {last_b102:?}→\
             {last_a102:?}、回放 {replays_b102}→{replays_a102}、删除 {dels_b102}→{dels_a102}、清空 \
             {clears_b102}→{clears_a102}、上锁 {locks_b102}→{locks_a102}、快速保存 {saves_b102}→\
             {saves_a102}，`landed`={landed_a102:?}（先前那一份={landed_b102:?}）——全部要求一动\
             不动。这一条腿此刻按它的手仍是**量具自己**：P103 起 `Session` 那侧有了 qinvokable、\
             `crates/qt_bridge/qml/` 也有了读者，而那两件的读数在**下面**恢复那一格里判，本格判的\
             与 P102 那一棵树同一件——产品把选区放回去了没有。",
            want_rect.map_or("（库里那一行没带着矩形，本格 BLOCKED）".to_string(), |r| r.to_string()),
            hdir.display(),
            row_said(&disk_after_restore),
            hdir.display(),
            door_s = dir.display(),
            rows_b_s = rows_said(&rows_b102),
            rows_a_s = rows_said(&rows_a102),
        ),
    );

    // ---- P116：上面那一格量的是 §5.14.3 step 4 那半句「重新进入原选区」，这一格量的是同一句里
    // 另外那半「恢复该截图内容」（[`crate::mask::MaskState::copy_history_content`]）——把库里那一行
    // 的图**递回剪贴板**。两条腿共用一句需求、各有各的读数：图递进了剪贴板而选区没回到屏幕上，
    // 与反过来，是两件不同的缺；合起来判就是一格绿盖掉一格红，而 #193 从 P102 起欠的就是这一半。
    // 这一格同时是 §5.14.2 step 4「重新复制」的第一位读者：那个动作到本轮为止产品没有腿。
    //
    // 对照取**尺寸**而不是字节，与上面「确认那一次递出去的是不是拼出来的那一张」那一格同一把尺：
    // 剪贴板交回的是系统自己重新编码的那一份，拿它跟盘上那份 PNG 的字节比，量的是系统的编码器而不是
    // 这条腿。字节这一头只要求 > 0 且解码得开——0 字节的"图"递得出去、尺寸却答不出。
    //
    // 剪贴板这一头本档动两次（递进去、再把**进场那一份**还回去），两次读数各贴着事件取一次（#197）。
    // 还回去交的是 `clip_before` 而不是本格进场那一枚：上面那一格判的就是「这道门把剪贴板留成它找到
    // 的样子」，新腿不许改老账 ⇒ 那一格的红绿到这一趟跑完仍然成立，靠的正是这一次还。`usable()` 先答
    // 「占用」时（`blind`）本档一步也不动这一格：看不见那份东西就既不能说递成、也没得还，判 BLOCKED
    // 而不是 FAIL；进场那份只有 html 或文件列表时同样 BLOCKED——`put_back` 表示不了那一份，本格欠的
    // 那一笔还不上，形状与上面那一格为同一条理由判的形状一致（#192 那笔未结的债）。
    let dims_of = |c: &Option<platform_windows::clip::Contents>| {
        c.as_ref()
            .and_then(|x| x.image_bytes.as_ref())
            .and_then(|b| falcon_core::encode::decode(b).ok())
            .map(|f| (f.width, f.height))
    };
    let bytes_of = |c: &Option<platform_windows::clip::Contents>| {
        c.as_ref()
            .and_then(|x| x.image_bytes.as_ref())
            .map_or(0, |b| b.len())
    };
    let copy_pic = if esc_id == 0 {
        Err("取消那一格没报出行号，没东西可以问盘上".to_string())
    } else {
        history_picture(&hdir, esc_id)
    };
    let want_dims = match &copy_pic {
        Ok(Some((_, bytes, dims))) if *bytes > 0 => *dims,
        _ => None,
    };
    let hcopies_b116 = mask::with(|m| m.history_copies);
    let ledger_b116 = mask::with(Untouched::read);
    let clip_b116 = platform_windows::clip::read().ok();
    let dims_b116 = dims_of(&clip_b116);
    let rows_b116 = history_rows(&hdir);
    let pics_b116 = picture_count(&hdir);
    let files_b116 = count(&dir);
    let pushed = mask::with(|m| m.copy_history_content(esc_id));
    let clip_pushed = platform_windows::clip::read().ok();
    let (hcopies_after, hcopy_said) =
        mask::with(|m| (m.history_copies, m.history_copy_last.clone()));
    let ledger_after = mask::with(Untouched::read);
    let dims_pushed = dims_of(&clip_pushed);
    let bytes_pushed = bytes_of(&clip_pushed);
    // 反面那一臂再借上面两处已独立证过「确实不在」的那枚 `missing`：一个不存在的行号如果能把这枚
    // 计数推进去、或者把剪贴板上那一张顶掉，前面那一格的红也就没分量了。
    let ghost_pushed = mask::with(|m| m.copy_history_content(missing));
    let clip_ghost = platform_windows::clip::read().ok();
    let (hcopies_ghost, ghost_said_116) =
        mask::with(|m| (m.history_copies, m.history_copy_last.clone()));
    let ledger_ghost = mask::with(Untouched::read);
    let dims_ghost = dims_of(&clip_ghost);
    let handed = if blind.is_some() {
        None
    } else {
        Some(crate::state::put_back(&clip_before))
    };
    let clip_final = platform_windows::clip::read().ok();
    let left_found = match (&clip_before, &clip_final) {
        (Some(a), Some(b)) => a.image_bytes == b.image_bytes && a.text == b.text,
        _ => false,
    };
    let rows_116 = history_rows(&hdir);
    let pics_116 = picture_count(&hdir);
    let files_116 = count(&dir);
    let row_116 = history_row(&hdir, esc_id);
    let moved_pos = ledger_b116.moved(&ledger_after);
    let moved_neg = ledger_after.moved(&ledger_ghost);
    let says_it = hcopy_said.contains("递回剪贴板")
        && hcopy_said.contains(format!("第 {esc_id} 行").as_str())
        && hcopy_said.contains("已取消");
    let says_ghost =
        ghost_said_116.contains("没有第") && ghost_said_116.contains(&missing.to_string());
    let nothing_moved = moved_pos.is_empty()
        && moved_neg.is_empty()
        && rows_116.as_ref().ok() == rows_b116.as_ref().ok()
        && pics_116 == pics_b116
        && files_116 == files_b116
        && row_116 == disk_after_restore;
    let same_as_found = dims_b116.is_some() && dims_b116 == want_dims;
    let no_picture = want_dims.is_none();
    let copy_ok = pushed
        && want_dims.is_some()
        && dims_pushed == want_dims
        && bytes_pushed > 0
        && hcopies_after == hcopies_b116 + 1
        && says_it
        && nothing_moved
        && !ghost_pushed
        && dims_ghost == dims_pushed
        && hcopies_ghost == hcopies_after
        && says_ghost
        && matches!(handed, Some(Ok(_)))
        && left_found;
    let blocked_116 = blind.is_some() || unrepresentable || want_dims.is_none() || same_as_found;
    let verdict_116 = if blocked_116 {
        Check::Blocked
    } else {
        Check::from(copy_ok)
    };
    rep.row(
        "the cancelled row handed its own picture back to the clipboard",
        verdict_116,
        format!(
            "本格问的是 §5.14.3 step 4 那半句「恢复该截图内容」：把库里那一行的图递回剪贴板 \
             （产品腿 `copy_history_content`，同时是 §5.14.2 step 4「重新复制」的第一位读者）。正面：\
             拿上面那一格报的行号={esc_id} 走一次，它答={pushed}（要求 true）；那一行的图先在盘上独立\
             问过={copy_pic_s}（要求字节 > 0 且解码得出尺寸——对面是文件系统，不是产品那句话）；调用前\
             系统递回的图={dims_b116:?}（{bytes_b116} 字节），调用之后={dims_pushed:?}（{bytes_pushed} \
             字节）——要求之后那一枚正是盘上问回来的 {want_dims:?} 且字节 > 0。**只比尺寸不比字节**，\
             与上面「确认那一次递出去的是不是拼出来的那一张」那一格同一把尺：剪贴板交回的是系统自己\
             重新编码的那一份，比字节量的是系统的编码器而不是这条腿。计数：`history_copies` \
             {hcopies_b116}→{hcopies_after}（要求加一；失败不吃这一枚，与 `history_records` 那条「失败\
             不加」同口径），它说的那句话={hcopy_said:?}（要求同时说到「递回剪贴板」、「第 {esc_id} 行」\
             与「已取消」——最后那三个字证明这一条腿真读了库里那一枚 `cancelled`，而不是把所有行说成\
             同一句话）。反面：行号 {missing} 上面已独立证过不在库里，走同一条腿它答={ghost_pushed}（要求\
             false），系统递回的那一张仍是 {dims_ghost:?}（要求与正面之后同一枚：一次失败不许把剪贴板上\
             那一张顶掉），计数停在 {hcopies_ghost}（要求与正面之后同一个数），那句话=\
             {ghost_said_116:?}（要求点名「没有第 {missing} 行」——「库里没有那一行」与「那一行的图取不\
             回来」与「剪贴板被占用」是三条不同的话，各写各的）。本腿不许动的十四枚按**字段名**答：正面\
             之后动了={moved_pos:?}、反面之后动了={moved_neg:?}（两枚都要求空——本腿不翻页、不改选区、不\
             落盘、不占屏幕上那句话的槽位，也不许记进 `auto_copies` 那枚「默认行为递出去」的计数里，那\
             样一来就再也问不出「这一场递出去的哪一张是历史」）。账一枚也不许动：沙盒 {hdir_s} 里 \
             {rows_b116_s}→{rows_116_s} 行、`history/` 下 {pics_b116}→{pics_116} 张原图、本档落点 \
             {door_s} 普查 {files_b116}→{files_116} 份、库里那一行={row_116_s}（要求与上面那一格读回的\
             一字不差：一次复制不许顺手把 `cancelled` 抹掉，那是替用户改账而不是替他找回记录）。还清：\
             本腿动了剪贴板两次，最后把**进场那一份** `clip_before` 交回 `put_back`，它答={handed_s}，\
             还回去之后系统递回的图与文本与进场那一份一致={left_found}（要求 true）。判 BLOCKED 的三种\
             对面，各印各的：`usable()` 先答={blind_s}（看不见那份东西就既不能说递成、也没得还）；进场\
             那份里有 html={had_html}、文件列表={had_files} 项（`put_back` 表示不了那一份，本格欠的那笔\
             还不上）；库里那一行没图={no_picture} 或进场那一份的尺寸恰好就是 {want_dims:?}=\
             {same_as_found}（控制组自己没摆出来，本格量不出「换了人」）。",
            copy_pic_s = picture_said(&copy_pic),
            bytes_b116 = bytes_of(&clip_b116),
            hdir_s = hdir.display(),
            door_s = dir.display(),
            rows_b116_s = rows_said(&rows_b116),
            rows_116_s = rows_said(&rows_116),
            row_116_s = row_said(&row_116),
            handed_s = match &handed {
                None => "没叫：看不见那份东西，本档一步也不动这一格".to_string(),
                Some(Ok(s)) => s.clone(),
                Some(Err(e)) => format!("没还上（{e}）"),
            },
            blind_s = match &blind {
                None => "能递".to_string(),
                Some(w) => w.clone(),
            },
            had_html = clip_before
                .as_ref()
                .map(|c| c.html.is_some())
                .unwrap_or(false),
            had_files = clip_before
                .as_ref()
                .map(|c| c.files.len())
                .unwrap_or(0),
        ),
    );

    // ---- P98：这一档第一次经**界面**读一次历史。上面那几格问的都是 Rust 自己调 Rust ——
    // `measure()` 拿 `mask::with` 直接按产品的腿。这两格问的是另一件事：QML 那一头能不能经
    // `Session` 的 qinvokable 走到同一条腿上，而它走的必须**不是**本档替它摆好的那一次。
    // 分辨的方法只有前缀：`rows|` 与 `replay|` 这两枚键本档一处也不写，写它们的只有
    // `AnnotationToolbar.qml` ⇒ `reported()` 数到的每一条都是界面递回来的。
    //
    // 前置没摆出来（`arm()` 失败 ⇒ 库里没有那一行、计划是空串、那条腿整条不跑）判 BLOCKED
    // 而不是 FAIL：这一格对面是本档自己的前置，不是产品的腿。
    let armed = planted_row();
    let armed_rows = planted_rows();
    let seen_rows = reported("rows");
    let seen_replays = reported("replay");
    let (list_said, list_last_said, toolbars) = mask::with(|m| {
        (
            m.history_list.clone(),
            m.history_list_last.clone(),
            m.slots.len(),
        )
    });
    let armed_s = armed.to_string();
    let rows_s = armed_rows.to_string();
    let want_said = format!("{armed_rows} 行");
    let want_label = format!("{armed} · ");
    let every_row_names_it = |lines: &[String]| {
        !lines.is_empty()
            && lines.iter().all(|l| {
                let mut seg = l.split('|');
                seg.next() == Some(rows_s.as_str())
                    && seg.any(|s| s.starts_with(want_label.as_str()))
            })
    };
    let rows_ok = !seen_rows.is_empty()
        && seen_rows.len() == toolbars
        && every_row_names_it(&seen_rows)
        && list_said.contains(armed_s.as_str())
        && list_last_said.contains(want_said.as_str());
    rep.row(
        "the toolbar read the history list through the interface",
        if armed == 0 {
            Check::Blocked
        } else {
            Check::from(rows_ok)
        },
        format!(
            "`arm()` 在 `engine.load()` 之前种进 {} 的第 {armed} 行（那一刻库里有 {armed_rows} 行，\
             界面那一条腿被允许读最多 {HIST_LIMIT} 行）；`AnnotationToolbar.qml` 自己经 \
             `session.historyRows()` 递回来的 `rows|` 那几行={seen_rows:?}（要求每行第一段就是 \
             {armed_rows}、且整行点名第 {armed} 行；本档一处也不写这两枚前缀，所以它们只能是界面\
             写的）；递回来 {} 条 vs 这一场有 {toolbars} 面遮罩（每面一枚工具条、各读一次，要求\
             相等）；而产品自己在 `mask` 里留下的清单={list_said:?}、那句话={list_last_said:?}\
             （要求那份点名第 {armed} 行、那句数到 {armed_rows} 行）。这一格把 P95 普查点名的 \
             `History::newest` 从「只有单测读者」换成「有界面读者」：读侧那条路自此从 QML 一路走到 \
             SQLite，而中间那一站是 `Session` 的一枚 qinvokable",
            hist_dir().display(),
            seen_rows.len(),
        ),
    );

    // 回放那一半判的是**盘上**，而不只是那句话：每一枚工具条各回放一次，落点目录里就该多出
    // 同样数份、且解码出来正好是 `arm()` 种下去的那一对尺寸的图。数尺寸而不是数条目是必须的：
    // `dir` 里此刻还躺着这一场 `commit()` 自动保存的那一张与 `pinned-config.toml`，而这一格要
    // 的读数是"界面那条腿写出几张图"。尺寸撞不上就当场 FAIL —— 那说明这把尺子挑的数字落进了
    // 某台机器可能的选区里，读数就此不再可信。
    let plant_after = mask::with(|m| m.history_replays);
    let replays_at_arm = planted_replays();
    let planted_pictures = count_of_size(&dir, PLANT_W, PLANT_H);
    let said_row = format!("第 {armed} 行");
    let replay_ok = !seen_replays.is_empty()
        && seen_replays.len() == seen_rows.len()
        && seen_replays
            .iter()
            .all(|s| s.starts_with("回放了历史第") && s.contains(said_row.as_str()))
        && planted_pictures == seen_replays.len()
        && replays_before == replays_at_arm + seen_replays.len() as u32
        && plant_after == replays_after_missing;
    rep.row(
        "the toolbar replayed the planted row through the picture it wrote",
        if armed == 0 {
            Check::Blocked
        } else {
            Check::from(replay_ok)
        },
        format!(
            "同一批工具条经 `session.historyReplay({armed})` 递回来的 `replay|` 那几行={seen_replays:?}\
             （要求条数与清单那一格相等、每条都以产品自己那句「回放了历史第 {armed} 行」开头）；{dir} \
             里能解码成 {PLANT_W}×{PLANT_H} 的文件普查={planted_pictures}（要求恰好等于递回来的条数 \
             {}，多一份少一份都不算——这一格对面是文件系统而不是那句话）；产品自己数的回放次数从 \
             `arm()` 那刻的 {replays_at_arm} 走到 `measure()` 第一次读到的 {replays_before}（要求加数\
             就是界面跑掉的那些——界面那一条腿跑在 `engine.load()` 里，因此它记的那一次**先于**本档\
             任何一格，基准只能取在种行那一刻；本档第一次跑把基准取在了自己那几格之后，于是要求 \
             {replays_after}+{n}={plant_after} 当场红）；而 {plant_after} 自此不再动（要求等于反面那\
             一格之后的 {replays_after_missing}，界面之后本档也没背着人再多回放一次）。尺寸钉成 \
             {PLANT_W}×{PLANT_H} 是为了让这一格数得到**界面那条腿**写的文件而不是本档自己那几格写的：\
             这一场的选区是 {hole}，两份账在同一目录里也分得开",
            seen_replays.len(),
            n = seen_replays.len(),
            dir = dir.display(),
        ),
    );

    // ---- P101：界面那一条腿第一次**去掉**东西。上面 P98 那两格只让它读清单、回放一行；未点头
    // 的清空、锁、删除这三枚桥自 P99/P100 建起来之后，读者一直是量具自己与 `mask.rs` 的单测，
    // `crates/qt_bridge/qml/` 里 0 处。这两格把它们交给 `AnnotationToolbar.qml`，而界面动的必须
    // 是 `arm()` 替它自己种的那一枚行号（[`plan_line`] 的第四段）——`armed` 与 `commit_row` 各有
    // 本档自己的格子在数，界面碰掉任何一枚，那些格子读到的就成了「前置被自己判的那条腿改过」。
    //
    // 三枚计数的基准取在 `arm()` 那一刻（上面回放那一格那句教训的续）：这一条腿跑在
    // `engine.load()` 里，从本格往后每一次读数都已经把界面那几次算在里面了。
    let ui_row = planted_ui_row();
    let ui_file = planted_ui_file();
    let (dels_at_arm, clears_at_arm, locks_at_arm) = planted_counts();
    let seen_clear_no = reported("clearunconfirmed");
    let seen_lock = reported("lock");
    let seen_delete = reported("delete");
    let at_clear_no = reported_at("clearunconfirmed");
    let at_lock = reported_at("lock");
    let at_delete = reported_at("delete");
    let (dels_now, clears_now, locks_now) =
        mask::with(|m| (m.history_deletes, m.history_clears, m.history_locks));
    let said_ui = format!("第 {ui_row} 行");
    let every_declines = !seen_clear_no.is_empty()
        && seen_clear_no.len() == toolbars
        && seen_clear_no
            .iter()
            .all(|s| s.contains("二次确认") && s.contains("一行也不动"));
    let clear_before_lock =
        at_clear_no.len() == at_lock.len() && at_clear_no.iter().zip(&at_lock).all(|(c, l)| c < l);
    let clear_no_ok = every_declines && clear_before_lock && clears_now == clears_at_arm;
    rep.row(
        "the toolbar declined a clear it never confirmed",
        if armed == 0 || ui_row == 0 {
            Check::Blocked
        } else {
            Check::from(clear_no_ok)
        },
        format!(
            "`AnnotationToolbar.qml` 自己经 `session.historyClear(false)` 递回来的 \
             `clearunconfirmed|` 那几行={seen_clear_no:?}（要求条数等于这一场的 {toolbars} 面遮罩、\
             每条同时说到「二次确认」与「一行也不动」——那是产品自己那句拒绝的原话，而这四枚前缀\
             本档一处也不写，所以它们只能是界面递回来的）；`clears` 从 `arm()` 那刻的 {clears_at_arm} \
             到这里还是 {clears_now}（要求一动不动：`confirmed` 就是 §5.14.4 规则 1 那枚二次确认落进\
             产品里的形状，界面递 `false` 的那一刻连一次删除都不许发生）；这两组前缀在 `Plan::lines` \
             里的落位={at_clear_no:?} 对 {at_lock:?}（要求每一枚未确认的清空都排在它自己那一次上锁\
             之前：QML 那一条腿按 clear→lock→delete 的顺序递，读数反过来说明界面那一头把顺序改了）。 \
             本档下面还另有一次 `clear_history(false)`，那一格量的是量具自己的手；这一格量的对面是屏幕。",
        ),
    );

    // 第二格是 §5.14.4 那两半**相反**的口径在同一界面腿上的第一次：同一枚行号先由界面锁上、再
    // 由界面按名字删掉——规则 2 的「不得删除锁定的记录」管的是自动清理与清空，而手动删除点名时
    // 锁定项也在点名里（那条口径写在 [`crate::mask::MaskState::delete_history`] 头上，P99 立起来
    // 至今只有单测读过）。锁的落盘证据在这一腿之后拿不到（那一行已经没了），所以这一格把锁交给
    // 计数与产品那句话，把**删成没有**交给文件系统：`arm()` 当场问回来的那份路径此刻不在、库里那
    // 一行读回来是「没有那一行」，而 `armed` 那一行的图还在——界面碰的不是本档自己的前置。
    let every_lock_names_it = !seen_lock.is_empty()
        && seen_lock.len() == toolbars
        && seen_lock
            .iter()
            .all(|s| s.starts_with("锁上了") && s.contains(said_ui.as_str()));
    let took_it = seen_delete
        .iter()
        .filter(|s| s.contains("删了历史") && s.contains(said_ui.as_str()))
        .count();
    let refused_it = seen_delete
        .iter()
        .filter(|s| s.contains("一个也没有"))
        .count();
    let delete_says = seen_delete.len() == toolbars && took_it == 1 && refused_it + 1 == toolbars;
    let lock_before_delete =
        at_lock.len() == at_delete.len() && at_lock.iter().zip(&at_delete).all(|(l, d)| l < d);
    let ui_gone = matches!(history_picture(&hdir, ui_row), Ok(None));
    let ui_file_gone = matches!(&ui_file, Some(p) if !p.exists());
    let armed_still = matches!(history_picture(&hdir, armed), Ok(Some(_)));
    let ui_in_door_dir = count_of_size(&dir, PLANT_UI_W, PLANT_UI_H);
    // P115：这一格的「删除只加一」从此不能钉死成 `+ 1` 了 —— 下面新起的那条**一次点名两枚**的界面腿
    // 也在 `engine.load()` 里跑，同一个 `history_deletes` 会把它的两次也数进来。加数因此交**盘**来定：
    // 那两枚行号里有几枚此刻读回来是「库里没有那一行」，上面就多几次。不这么办会有两种错：继续写
    // `+ 1`，本档每往后加一条删除腿这一格就假 FAIL 一次；写 `+ 1 + 2`，那条腿没跑起来时这一格也跟着
    // 撒一次谎（它自己那一格会 FAIL，但这一格的话已经先说错了）。
    let (batch_a_row, batch_b_row) = planted_batch_rows();
    let batch_gone = [batch_a_row, batch_b_row]
        .iter()
        .filter(|id| **id != 0 && matches!(history_picture(&hdir, **id), Ok(None)))
        .count() as u32;
    let lock_del_ok = every_lock_names_it
        && delete_says
        && lock_before_delete
        && locks_now == locks_at_arm + toolbars as u32
        && dels_now == dels_at_arm + 1 + batch_gone
        && ui_gone
        && ui_file_gone
        && armed_still
        && ui_in_door_dir == 0;
    rep.row(
        "the toolbar locked its own row and then deleted it",
        if armed == 0 || ui_row == 0 || ui_file.is_none() {
            Check::Blocked
        } else {
            Check::from(lock_del_ok)
        },
        format!(
            "同一批工具条接着走的两条腿。`lock|` 那几行={seen_lock:?}（要求条数等于 {toolbars}、每条\
             都以产品那句「锁上了」开头且点名 {said_ui}）；`delete|` 那几行={seen_delete:?}（要求 \
             {toolbars} 条里恰好一条说「删了历史」且点名 {said_ui}，其余每条说「一个也没有」——同一枚 \
             行号被第二枚工具条再删一次时，产品不许把「库里本来没有」转述成成功）；两组前缀的落位=\
             {at_lock:?} 对 {at_delete:?}（要求每一枚上锁都在它自己那次删除之前：界面删的必须是它\
             刚锁上的那一行，而锁定项照样按名字删得掉那条口径，自此在同一条界面腿上走了一次）。计数\
             从 `arm()` 的基准走到这里：锁 {locks_at_arm}→{locks_now}（要求加数就是工具条数——那一枚\
             数的是没报错的调用而不是行数）、删除 {dels_at_arm}→{dels_now}（要求加数是**一**，再加\
             {batch_gone}——那一枚 {batch_gone} 不读任何一句话，读的是盘：本档替界面**一次点名**的那两枚\
             行号里，此刻有几枚在库里读不回来，上面那条删除腿就应当多收几次。P115 起这一格不再钉死\
             `+ 1`：往后每加一条删除腿都钉一次假 FAIL，反之写死加数会在腿没跑时跟着撒谎。{toolbars} \
             次调用里只有一枚行号真的在库里）。盘上三问：第 {ui_row} 行读回来就是「没有那一行」=\
             {ui_gone}；它本来指的那份 {ui_file:?} 此刻已不在={ui_file_gone}（对面是文件系统，不是那句\
             话）；`armed` 那一行的图还在={armed_still}（要求 true——界面碰掉的不是本档自己的前置）。 \
             最后 {dir_s} 里解码成 {PLANT_UI_W}×{PLANT_UI_H} 的文件数={ui_in_door_dir}（要求 0：这一条腿\
             只删不回放，上面那几格数到的 37×23 因此不会被它掺进来）。",
            dir_s = dir.display(),
        ),
    );

    // ---- P103：界面那一条腿第一次**挪动**屏幕上那一只框。上面 P98/P101 那四格让界面读过清单、
    // 回放过一张图、锁过一行、删过一行，而 [`crate::mask::MaskState::restore_history_selection`]
    // 自 P102 建起来之后只被量具自己按过 —— 上面那一格读得到"产品把选区放回去了"，读不到真人那一下。
    // 这一格把它交给 `AnnotationToolbar.qml`，而且一次交**两枚**行号：先挑走（一枚与此刻这只框
    // 不等的矩形），再放回（一枚正好是这只框的矩形）。成对的理由只有一条：本档其余各格的前置就是
    // 屏幕上那一只框，界面只挑不走，"放回原处"这一问就没有对面；只挑走不放回，其余各格读回的
    // 就是一场被界面改过前置的实验。
    //
    // 挪动读自**界面自己递回来的那两枚读数**（`session.maskHole()` 当场答的那一句），而不是读自
    // `measure()`：`measure()` 第一次碰 `m.hole` 已经跑在整条界面腿之后，那时屏幕上那只框要么是
    // 放回来的、要么是本档自己后面几格改的，两种都问不出"界面那一次动过"。基准因此取在 `arm()`
    // 那一刻 —— 上面那三枚计数基准同一条理由的第四次用。
    let pick_row = planted_pick_row();
    let back_row = planted_back_row();
    let hole_at_arm = planted_hole();
    let restores_at_arm = planted_restores();
    let seen_pick = reported("restorepick");
    let seen_back = reported("restoreback");
    let at_pick = reported_at("restorepick");
    let at_back = reported_at("restoreback");
    let pick_disk = history_row(&hdir, pick_row);
    let back_disk = history_row(&hdir, back_row);
    let want_of = |r: &Result<Option<(bool, falcon_core::PhysRect)>, String>| match r {
        Ok(Some((_, rect))) => rect.to_string(),
        // 「库里没有那一行」与「库打不开」两种答法在这一格里都是 `want_pick`/`want_back` 为空
        // ⇒ 下面那几枚要求当场读不成立 ⇒ 本格 FAIL。把它们说成 BLOCKED 等于让本档自己的前置
        // 没收一次界面没走通的事实，而那两枚 `arm()` 明明种成功了（行号不是 0）。
        _ => String::new(),
    };
    let want_pick = want_of(&pick_disk);
    let want_back = want_of(&back_disk);
    let hole_s = hole_at_arm.to_string();
    // P113 起这里一次读**三枚**：总数、翻页那一半、以及屏幕上那一只框。为什么计数要读两枚 ——
    // 界面这一条腿自此走四件事（挑、放、翻回、翻走），后两件也各算一次"回进了原选区"，都记在
    // `history_restores` 上；只读总数就没有"这四次里是谁递的"这一问的答案，判据只能改口要求一个
    // 谁也对不上的更大加数。[`crate::mask::MaskState::history_flips`] 是那枚拆分。
    let (restores_ui_leg, flips_ui_leg, hole_now) =
        mask::with(|m| (m.history_restores, m.history_flips, m.hole));
    let flips_at_arm = planted_flips();
    // 一条腿报回来的两段都得是产品自己的话：前一段点名它恢复的那一行，后一段是那一刻遮罩上那
    // 一只框。多出一段就是界面把别的东西也塞进了这一行 —— 这两枚前缀本档一处也不写。
    let leg_says = |lines: &[String], row: i64, want: &str| -> bool {
        !lines.is_empty()
            && lines.len() == toolbars
            && lines.iter().all(|l| {
                let mut seg = l.split('|');
                let said = seg.next().unwrap_or_default();
                let got = seg.next().unwrap_or_default();
                seg.next().is_none()
                    && said.contains("原选区")
                    && said.contains(format!("第 {row} 行").as_str())
                    && got == want
            })
    };
    let moved = !want_pick.is_empty() && want_pick != want_back;
    let back_is_the_hole = !want_back.is_empty() && want_back == hole_s;
    let pick_ok = leg_says(&seen_pick, pick_row, want_pick.as_str());
    let back_ok = leg_says(&seen_back, back_row, want_back.as_str());
    let pick_before_back =
        at_pick.len() == at_back.len() && at_pick.iter().zip(&at_back).all(|(p, b)| p < b);
    let premise_intact = hole == hole_at_arm && hole_now == hole_at_arm;
    // 加数按**两半各问一次**：`restores_delta` 是 `arm()` 之后所有「回进原选区」的次数，
    // `flips_delta` 是其中由翻页那两条腿递的，两者之差就是由代码直接按的那几次 —— 界面每面各一次
    // 挑、各一次放，再加本档自己在上面 P102 那一格里按的一次。用 saturating 而不是直接相减：
    // 「拆不开」（翻页数大于总数）本身就是产品的一句谎，要它落成 FAIL，而不是把 `measure()` panic 掉。
    let restores_delta = restores_ui_leg.saturating_sub(restores_at_arm);
    let flips_delta = flips_ui_leg.saturating_sub(flips_at_arm);
    let direct_expect = 2 * toolbars as u32 + 1;
    let split_honest = restores_delta >= flips_delta;
    let direct_delta = restores_delta.saturating_sub(flips_delta);
    let spent = split_honest && direct_delta == direct_expect;
    let restore_ui_ok = moved
        && back_is_the_hole
        && pick_ok
        && back_ok
        && pick_before_back
        && premise_intact
        && spent;
    rep.row(
        "the toolbar moved the mask's selection out and back",
        if armed == 0 || pick_row == 0 || back_row == 0 {
            Check::Blocked
        } else {
            Check::from(restore_ui_ok)
        },
        format!(
            "同一批工具条接着走的两条恢复腿，点名的是 `arm()` 替这一条腿专门种下的两枚行号（计划第 5、6 段）。\
             `restorepick|` 那几行={seen_pick:?}（要求 {toolbars} 条、每条前一段同时说到「原选区」与「第 \
             {pick_row} 行」，后一段是它当场经 `session.maskHole()` 读回的矩形，要求等于库里那一行自己记着的 \
             {want_pick}）；`restoreback|` 那几行={seen_back:?}（同一条形状，矩形要求等于 {want_back}）。两枚\
             矩形必须**彼此不等**（读回={moved}，要求 true）：相等就是「把一只本来就在那儿的框放回原处」，\
             这一格什么也没判；第二枚还必须等于 `arm()` 那一刻遮罩上那一只框 {hole_s}（读回={back_is_the_hole}，\
             要求 true）——那是本档其余各格的前置，替界面放回的那一枚种错一寸，后面每一格读的都是另一场实验。\
             两组前缀的落位={at_pick:?} 对 {at_back:?}（要求每一枚挑走都在它自己那一次放回之前：QML 那一条腿\
             按挑→放的顺序递，反过来说明界面把顺序改了）。恢复**总数**从 `arm()` 那刻的 {restores_at_arm}\
             走到这里={restores_ui_leg}，翻页那一半从 {flips_at_arm} 走到这里={flips_ui_leg}（拆开的理由：\
             P113 起界面这一条腿在挑与放之后还翻了两页，那两次也各算一次「回进了原选区」，只读总数就读\
             不出加数是谁递的）。本格要求的是**差之差**：总数加数读回={restores_delta}、翻页加数读回=\
             {flips_delta}、二者之差={direct_delta}，要求等于 {direct_expect}（界面每面各挑一次、各放一次，\
             再加本档自己在上面恢复那一格里按的那一次）；同时要求 {split_honest}=true——翻页那一半不许比\
             总数还大，那是产品自己数错了，这一格宁可 FAIL 也不让 `measure()` 当场 panic。被拒的那一次不吃\
             这一枚计数，而本档不许背着界面多按。`measure()` 一进门读到的那一只框=\
             {hole}，这一格此刻再读一次={hole_now}，两枚都要求与 {hole_s} 一字不差（读回={premise_intact}）：\
             一来一回之后本档其余各格的前置与 P102 那一棵树同形，这是这一格敢让界面动 `m.hole` 的唯一理由。盘上\
             两问：这两行读回来={pick_disk_s} 与 {back_disk_s}（要求都在——这一条腿一次也不写盘，也不许把谁删掉）。\
             §5.14.3 那句「重新进入原选区」自此从 QML 一路走到 `set_hole`，中间那两站是 `Session` 的两枚 \
             qinvokable。",
            pick_disk_s = row_said(&pick_disk),
            back_disk_s = row_said(&back_disk),
        ),
    );

    // ---- P113：上面那一格让**代码**按了一次恢复；这一格让**按键**按一次。这就是这一格全部的
    // 新东西：`AnnotationToolbar.qml` 那两条递的是 `chord(Qt.Key_Up, Qt.ControlModifier)`，也就是
    // 一次真按键在 Qt 那里被翻成 (key, modifiers) 之后走的同一个函数。直接按桥只能证明桥通；从
    // `chord()` 走才证明这枚按键**会被认领**、并派发到"翻页"那一档 —— 而不是掉回 `step()` 那一档
    // 去做一像素微移（这两档共用同一个 `Keys.onUpPressed`，让开的那一条写在 `CaptureMask.qml` 的
    // `step()` 头上）。合成按键本轮仍不做（第 46 项 ② 的理由一字未改：注入的按键会落进当时有焦点
    // 的应用，而跑级时焦点在谁那里产品说不出）。
    //
    // 锚点既不在本档的读数里、也不在界面递进来的东西里：QML 那一条腿只递方向，"此刻停在第几行"
    // 由 Rust 从遮罩自己读（[`crate::mask::MaskState::history_anchor`]）。所以这一格对锚点的证法
    // 是**反推**的：两次翻页交回的矩形必须是 `pick_row` 与 `back_row` 自己记着的那两枚，而这组
    // 答案只可能从 `back_row` 起翻得到 —— 从库里最新那一行起翻交回的是 `back_row`/`planted`，
    // 从 `pick_row` 起翻交回的是 `ui_row`/`back_row`，锚点是 `-1` 时那句根本不含「原选区」。
    // 三种都红，而只有那一种可能不红。
    let seen_up = reported("flipup");
    let seen_down = reported("flipdown");
    let at_up = reported_at("flipup");
    let at_down = reported_at("flipdown");
    let flip_says = |lines: &[String], row: i64, want: &str| -> bool {
        !lines.is_empty()
            && lines.len() == toolbars
            && lines.iter().all(|l| {
                let mut seg = l.split('|');
                let acted = seg.next().unwrap_or_default();
                let said = seg.next().unwrap_or_default();
                let got = seg.next().unwrap_or_default();
                // 三段之外多一段，就是界面把别的东西也塞进了这一行。第一段是 `chord()` 自己的
                // 返回值：它说"这枚按键我认领了"，而它是 QML 那一层交回来的、本档一处也不写。
                seg.next().is_none()
                    && acted == "true"
                    && said.contains("原选区")
                    && said.contains(format!("第 {row} 行").as_str())
                    && got == want
            })
    };
    let up_ok = flip_says(&seen_up, pick_row, want_pick.as_str());
    let down_ok = flip_says(&seen_down, back_row, want_back.as_str());
    let flips_in_order = at_back.len() == toolbars
        && at_up.len() == toolbars
        && at_down.len() == toolbars
        && at_back
            .iter()
            .zip(&at_up)
            .zip(&at_down)
            .all(|((b, u), d)| b < u && u < d);
    // 这一格不第二次读计数、也不第二次读 `m.hole`。理由与 P112 那格拒绝"再数一遍"同一条：上面
    // 那一格的读数与这一格跑在 `measure()` 的同一次调用里，中间一条腿也不跑 ⇒ 第二次读永远等于
    // 第一次，它不是证人，只是把同一句话印两遍。上面那一格读的三枚因此**都是**"四条界面腿全跑完
    // 之后"的状态：`flips_delta` 是翻页吃掉的加数，`premise_intact` 是翻页之后框仍在原处。
    let flips_spent = flips_delta == 2 * toolbars as u32;
    let flip_ui_ok = up_ok
        && down_ok
        && flips_in_order
        && flips_spent
        && premise_intact
        && moved
        && back_is_the_hole;
    rep.row(
        "ctrl+up and ctrl+down page the mask selection through history",
        if armed == 0 || pick_row == 0 || back_row == 0 {
            Check::Blocked
        } else {
            Check::from(flip_ui_ok)
        },
        format!(
            "同一批工具条接着走的两条**按键**腿，方向各一枚，走的是 `chord()` 而不是桥本身。\
             `flipup|` 那几行={seen_up:?}（要求 {toolbars} 条、每条第一段是 `chord()` 自己答的 \
             true、第二段是屏幕上那一刻那句话且同时说到「原选区」与「第 {pick_row} 行」、第三段是\
             那一刻 `session.maskHole()` 读回的矩形，要求等于库里第 {pick_row} 行自己记着的 {want_pick}）；\
             `flipdown|` 那几行={seen_down:?}（同一条形状，行号与矩形换成第 {back_row} 行与 {want_back}）。\
             第一段读回 false 说的是同一件事的两个方向：这枚按键没被认领，或者认领了却没翻 ——\
             两者都不该绿。三枚前缀的落位={at_back:?} → {at_up:?} → {at_down:?}（要求每一面工具条\
             按 挑→放→翻回→翻走 的顺序递；这条腿挂在上面那一格的尾巴上，反过来说明界面把顺序改了）。\
             翻页吃掉的那一半加数={flips_delta}（从 {flips_at_arm} 走到 {flips_ui_leg}，要求正好是界面那 \
             {toolbars} 面各两次）；上一格已经把总数拆成这两半读过一次，这一格**不再读第二遍**——两格跑在 \
             `measure()` 的同一次调用里、中间一条腿也不跑，第二枚必然等于第一枚，那不是证人（读回总数=\
             {restores_ui_leg}、翻页={flips_ui_leg}，两枚同刻）。要求的形状是「总数 = 直接按的那 {direct_expect}\
             次 + 翻页的那几次」，而这一格只钉后半，前半由上一格钉。`arm()` 那一刻遮罩上那一只框={hole_s}，\
             四条界面腿全跑完之后读到的={hole_now}（读回={premise_intact}，要求 true）：一条腿两次翻页之后\
             本档其余各格的前置仍在原处——这与上一格共用 `moved`={moved}、`back_is_the_hole`={back_is_the_hole}\
             那两枚形状判据：两枚矩形彼此不等、且第二枚正是那只框，翻页才谈得上「来回」。§5.14.2 step 2\
             自此从一枚按键一路走到 `set_hole`，中间那几站是 `chord()` → `Session::historyFlip` → \
             `settings::neighbor_id_for` → `MaskState::flip_history_selection` → \
             `MaskState::restore_history_selection`，而锚点那一问全程没有经过界面。",
        ),
    );

    // ---- P114：上面四条腿各按了一次、各翻了一页；这一条**一件也不改**。它把界面读侧那枚一直没有
    // 读者的桥交出去一次：`historyNeighborsAt` 自 P112 建起后 `crates/qt_bridge/qml/` 里 0 处读者，
    // 判它的一直是 settings 那三行单测与 P111 那枚启动 banner —— 而 banner 拿的锚点是「库里最新那一
    // 行」，界面上该问的是「屏幕上这只框所在的那一行」，两枚锚点不是同一件事。
    //
    // 所以这一格唯一的新东西不是那句邻居的话（措辞早被单测钉过），而是**号的来源**：工具条经
    // `session.historyAnchor()` 从遮罩当场取号，一个也不自己编。而"当场"这两枚字是本格头两趟门
    // 教出来的（`p114_auto_A.log` 与 `_B.log` 同形）：本档原本在格子里现问遮罩，读回来是**第 6 行**
    // ——界面报的是 `anchor=3`，`arm()` 种的 4 行之外本档自己那两次落盘又记了两行，而
    // `history_anchor()` 先读 `history_viewed`（每一场会话开头归零，`mask.rs` 的 `open()`）再回退
    // `history_last_id`（每一次 commit 往上顶），所以深处那一眼问的是**另一场**会话的锚点。产品
    // 按它自己的文档答得没错，错的是问的时刻——#197 那一课（读数时刻要贴着被问的那件事）在这里的
    // 第二次现身。尺子因此改到 `measure()` 一进门就抄（`neighbors_witness`），锚点取 `arm()` 替
    // `flipdown` 种下的那一行。界面报回的那句里点名的号不另立读数：第一段是逐字比对的，比中了
    // 就是它问的那一行，再数一遍不是证人（P113 删掉"第二遍读总数"是同一条理由）。
    let seen_neighbors = reported("neighbors");
    let at_neighbors = reported_at("neighbors");
    let want_neighbors = neighbors_witness;
    let n_counted = !seen_neighbors.is_empty() && seen_neighbors.len() == toolbars;
    let n_says = n_counted
        && seen_neighbors
            .iter()
            .all(|l| split_two(l).is_some_and(|(said, _)| Some(said) == want_neighbors.as_deref()));
    // 第二段要求的是**纯读**：这条腿之后没人再按恢复、也没人再翻页，屏幕上那只框必须还停在
    // `flipdown` 留给它的地方 —— 与 `arm()` 那一刻的同一枚，而那一枚是本档其余各格共用的前置。
    // 读邻居读动了选区，先破坏的就是它自己这一格的前提。
    let n_frame_still = n_counted
        && seen_neighbors
            .iter()
            .all(|l| split_two(l).is_some_and(|(_, got)| got == hole_s));
    let after_the_flip_leg = n_counted
        && at_down.len() == toolbars
        && at_neighbors.len() == toolbars
        && at_down.iter().zip(&at_neighbors).all(|(d, n)| d < n);
    let neighbors_ok = n_says && n_frame_still && after_the_flip_leg;
    rep.row(
        "the toolbar read the neighbours of the row its frame is sitting on",
        if armed == 0 || pick_row == 0 || back_row == 0 {
            Check::Blocked
        } else {
            Check::from(neighbors_ok)
        },
        format!(
            "`neighbors|` 那几行={seen_neighbors:?}（要求 {toolbars} 条、每条恰好两段：第一段逐字等于本档\
             自己印的那一句 {want_neighbors:?}，第二段是那一刻 `session.maskHole()` 读回的矩形，要求等于 \
             `arm()` 那一刻的 {hole_s}）。{want_neighbors:?} 由 [`neighbors_from_disk`] 开 {hdir_s} 问 \
             `get`/`prev`/`next` 三次印成，且是在 `measure()` **一进门**抄下的——不是本格现问的：本档从进门到\
             这一格之间自己还要删行、清库、落图，深处再开一次读的就是本档改过的库。全程没有叫 \
             [`crate::settings::neighbors_of_banner_for`] —— 那一枚正是这条腿读的东西，拿它的输出当尺子就是\
             让它给自己作证；两处格式一旦分岔，这一格当场红，而「措辞漂了」与「答错位了」在别处是两件事、\
             在这里由同一枚断言看着。那枚尺子答 `None`（库打不开 / 库里没有那一行 / 锚点是 -1）时本格判 \
             **FAIL 而不是 BLOCKED**：本档说不出那一句，就没有资格替界面断言「这句对」——这是 [`history_row`] \
             那把尺子从 P103 起就拒过的形状。锚点那一枚本档一处也不现问：那一句是为第 {back_row} 行抄的，而\
             {back_row} 是 `arm()` 替上面那一格（`flipdown`）种下的行号；第一段逐字比中了，界面问的就是它\
             自己脚下那一行（本档头两趟门在格子里现问遮罩读到的是第 6 行——`history_anchor()` 是**会话**的\
             读数，先读 `history_viewed`（每场开头归零）再回退 `history_last_id`（每次 commit 往上顶），而\
             本档在它之前又开了一场、落了图——那是另一场的锚点，不是这一问的证人）。第二段那条是纯读的证人：\
             这条腿跑完到本档读数之间没人再动选区，框还在原处={n_frame_still}（要求 true）。\
             落位={at_down:?} → {at_neighbors:?}（要求每一面工具条的邻居那句排在它自己那次 `flipdown` 之后：\
             这条腿挂在界面那条链的末尾，反过来说明界面把顺序改了）。§5.14.2 那两条 prev/next 腿的**界面读法**\
             自此有了第一位读者，`qml/` 里那枚 0 由这一格记第一笔。**本格不判这一问去了哪座库**：沙盒那座与\
             全局那座在同一条 id 序列上完全可能印出同一句 prev/next，所以这一格是**锚点**的证人、不是**库**的\
             证人，而其余 `history_data_dir()` 调用点是否与遮罩同库仍是 #218。",
            hdir_s = hdir.display(),
        ),
    );

    // ---- P99：§5.14.4 的两条**产品**腿（[`crate::mask::MaskState::delete_history`] 与
    // [`crate::mask::MaskState::clear_history`]）。前面每一格问的都是"往盘上写"或"从库里读"，
    // 这四格问的是**去掉**：库里那一行少了没有、它指的那份图少了没有、而空手与没点头那两次是否
    // 一行也没动。四格的基准一律**当场**取——`arm()` 那枚教训的续（一条腿读单调递增的计数时先问
    // 它读的是事件之前还是之后）：这四格全跑在 `measure()` 里，而界面那一条腿（清单、回放、P101
    // 起那把锁与那次删自己那一行、P103 起那两次恢复）跑在 `engine.load()` 里，也就跑在这四枚
    // 基准**之前** ⇒ 基准读回来的每一个数都已经把界面那几次算在里面，判据因此只要求**当场之差**。
    let del_target = match &picture {
        Ok(Some((Some(p), bytes, _))) if *bytes > 0 => Some(p.clone()),
        _ => None,
    };
    let (dels_before, recs_before_del, last_id_before) =
        mask::with(|m| (m.history_deletes, m.history_records, m.history_last_id));
    let rows_before_del = history_rows(&hdir);
    let pics_before_del = picture_count(&hdir);
    let deleted = commit_row.and_then(|id| mask::with(|m| m.delete_history(&[id])));
    let (dels_after, recs_after_del, last_id_after, del_said) = mask::with(|m| {
        (
            m.history_deletes,
            m.history_records,
            m.history_last_id,
            m.history_delete_last.clone(),
        )
    });
    let rows_after_del = history_rows(&hdir);
    let pics_after_del = picture_count(&hdir);
    let want_id = commit_row.unwrap_or(0);
    let gone_ids = deleted.as_ref().map(|f| f.ids.clone()).unwrap_or_default();
    let freed_bytes = deleted.as_ref().map(|f| f.bytes).unwrap_or(0);
    let rows_drop = matches!((&rows_before_del, &rows_after_del), (Ok(b), Ok(a)) if *a + 1 == *b);
    let target_gone = del_target.as_ref().is_none_or(|p| !p.exists());
    let pics_drop = pics_after_del + usize::from(del_target.is_some()) == pics_before_del;
    // 那一行如果本来就没有原图（`[history] keep_originals` 是用户自己的开关），字节数那一问就
    // 只由缩略图说话：这里不替用户把开关拧开，也不把"这一行本来就没占空间"说成一次失败。
    let bytes_ok = del_target.is_none() || freed_bytes > 0;
    let deleted_ok = deleted.is_some()
        && gone_ids == vec![want_id]
        && bytes_ok
        && rows_drop
        && target_gone
        && pics_drop
        && dels_after == dels_before + 1
        && del_said.contains(format!("第 {want_id} 行").as_str())
        && recs_after_del == recs_before_del
        && last_id_after == last_id_before;
    rep.row(
        "deleting the named row gave its picture and its bytes back",
        if commit_row.is_none() {
            Check::Blocked
        } else {
            Check::from(deleted_ok)
        },
        format!(
            "产品拿确认那一格报的行号 {want_id} 走了一次 `delete_history(&[{want_id}])`：它答 \
             {deleted:?}（交回的行号={gone_ids:?}，要求就是那一个；收回的字节={freed_bytes}，要求 \
             {}）。沙盒 {}：库里 {rows_b}→{rows_a} 行（要求少且只少 1），`history/` 底下数得出 \
             {pics_before_del}→{pics_after_del} 张原图（要求少的正是那一份 {del_target:?} 对应的 \
             一张，没有原图时为 0）；那一行指的文件 {del_target:?} 此刻**已不在**={}（`None` 那一臂\
             本来就没有文件可问，这一问为空真）; 产品自己数的删除 \
             行数 {dels_before}→{dels_after}（要求加一），那句话={del_said:?}（要求点名「第 {want_id} \
             行」）；而这一场的 §5.14.1 那格一动不动：记录次数 {recs_before_del}→{recs_after_del}、\
             最后记成的行号 {last_id_before:?}→{last_id_after:?}。§5.14.4 的 step 2–4 自此第一次\
             有一条产品腿按着它——判据对面是文件系统与 SQLite 两边，而不是一句「我删了」。",
            if del_target.is_some() {
                "> 0".to_string()
            } else {
                "这一行本来没有原图，不作要求".to_string()
            },
            hdir.display(),
            target_gone,
            rows_b = rows_said(&rows_before_del),
            rows_a = rows_said(&rows_after_del),
        ),
    );

    // 反面控制之一：**空点名**。UI 一行也没选中时递来的是 `[]`，而 `History::delete(&[])` 会
    // 答一次"成功的零行"——那一答在盘上与"选中的都删成了"长得一模一样，所以本条把它挡在开库
    // 之前。判据看的是三件都不动：行数、产品自己数的删除、以及那句必须说的是"没有点名"。
    let rows_before_empty = history_rows(&hdir);
    let dels_before_empty = mask::with(|m| m.history_deletes);
    let empty_hand = mask::with(|m| m.delete_history(&[]));
    let (dels_after_empty, empty_said) =
        mask::with(|m| (m.history_deletes, m.history_delete_last.clone()));
    let rows_after_empty = history_rows(&hdir);
    let empty_ok = empty_hand.is_none()
        && dels_after_empty == dels_before_empty
        && rows_after_empty.as_ref().ok() == rows_before_empty.as_ref().ok()
        && empty_said.contains("没有点名");
    rep.row(
        "an empty selection deletes nothing and says so",
        if commit_row.is_none() {
            Check::Blocked
        } else {
            Check::from(empty_ok)
        },
        format!(
            "递一个空的行号清单走 `delete_history(&[])`：它答={empty_hand:?}（要求 `None`——§6.2 \
             那把尺子本来会交回一次\"成功的零行\"，本条不许把它转述成成功）；沙盒 {} 里 \
             {rows_b}→{rows_a} 行（要求相等），产品自己数的删除 {dels_before_empty}→{dels_after_empty} \
             （要求不动），它说的那句话={empty_said:?}（要求「没有点名」）。这一格挡的是 §5.14.4 \
             那条流程里最容易漏的一件：用户在历史页上一次也没选，界面上「删除」按下去却报了一句\
             「已删除」。",
            hdir.display(),
            rows_b = rows_said(&rows_before_empty),
            rows_a = rows_said(&rows_after_empty),
        ),
    );

    // 反面控制之二：**没点头的清空**。§5.14.4 规则 1 那句"清空全部历史前应二次确认"在产品里
    // 的形状就是这一枚 `bool`。判据比"库里没少"更强：行数、`history/` 里数得出的原图张数、
    // 界面那一格种下且还没被删的那一行的文件、两枚计数——五件一起不动才算成。
    let armed_picture = history_picture(&hdir, armed);
    let armed_target = match &armed_picture {
        Ok(Some((Some(p), _, _))) => Some(p.clone()),
        _ => None,
    };
    let (clears_before_noc, dels_before_noc) =
        mask::with(|m| (m.history_clears, m.history_deletes));
    let rows_before_noc = history_rows(&hdir);
    let pics_before_noc = picture_count(&hdir);
    let unconfirmed = mask::with(|m| m.clear_history(false));
    let (clears_after_noc, dels_after_noc, noc_said) = mask::with(|m| {
        (
            m.history_clears,
            m.history_deletes,
            m.history_clear_last.clone(),
        )
    });
    let rows_after_noc = history_rows(&hdir);
    let pics_after_noc = picture_count(&hdir);
    let noc_ok = unconfirmed.is_none()
        && clears_after_noc == clears_before_noc
        && dels_after_noc == dels_before_noc
        && rows_after_noc.as_ref().ok() == rows_before_noc.as_ref().ok()
        && pics_after_noc == pics_before_noc
        && armed_target.as_ref().is_some_and(|p| p.exists())
        && noc_said.contains("二次确认");
    rep.row(
        "a clear without the second confirmation moves nothing",
        if armed == 0 {
            Check::Blocked
        } else {
            Check::from(noc_ok)
        },
        format!(
            "`clear_history(false)` 递的是「没点头」那一句：它答={unconfirmed:?}（要求 `None`）；\
             沙盒 {} 里 {rows_b}→{rows_a} 行（要求相等）、`history/` 下数得出 {pics_before_noc}→\
             {pics_after_noc} 张原图（要求相等）、界面那一格种的第 {armed} 行的文件 {armed_target:?} \
             此刻还在={}; 两枚计数：清空 {clears_before_noc}→{clears_after_noc}、删除 \
             {dels_before_noc}→{dels_after_noc}（都要求不动——那两条相反的口径不许并成一个数），\
             它说的那句话={noc_said:?}（要求「二次确认」）。§5.14.4 规则 1 从此在产品里有一个能\
             当场数的形状：没点头的那一次连一次删除都不许发生。",
            hdir.display(),
            armed_target.as_ref().is_some_and(|p| p.exists()),
            rows_b = rows_said(&rows_before_noc),
            rows_a = rows_said(&rows_after_noc),
        ),
    );

    // `locking a named row writes the lock through the product leg` 这一格：§5.14.4 规则 2 那一枚
    // 锁的**产品腿**（P100 起）。P99 那一次这把锁是本档自己
    // 经 core 打的（`History::set_locked` 至今零产品读者），所以那一格量的是"清空绕不绕开
    // 锁定项"，前置归尺子。本格量的换成产品那一条腿：`lock_history(id, true)` 写没写成、
    // 写成的那一次有没有把**别的**两格计数抬高（它动的是一枚布尔，库里一行也不少 —— 并进
    // 删除或清空任何一枚都说明那三格被抹平了），而"锁上没有"仍由本档当场问盘上。
    let (locks_b23, dels_b23, clears_b23) =
        mask::with(|m| (m.history_locks, m.history_deletes, m.history_clears));
    let rows_b23 = history_rows(&hdir);
    let pics_b23 = picture_count(&hdir);
    let armed_lock = mask::with(|m| m.lock_history(armed, true));
    let lock_said = mask::with(|m| m.history_lock_last.clone());
    let (locks_a23, dels_a23, clears_a23) =
        mask::with(|m| (m.history_locks, m.history_deletes, m.history_clears));
    let locked_now = matches!(locked_of(&hdir, armed), Ok(Some(true)));
    let rows_a23 = history_rows(&hdir);
    let pics_a23 = picture_count(&hdir);
    let lock_ok = armed_lock.is_some()
        && locks_a23 == locks_b23 + 1
        && dels_a23 == dels_b23
        && clears_a23 == clears_b23
        && locked_now
        && lock_said.contains(format!("第 {armed} 行").as_str())
        && rows_a23.as_ref().ok() == rows_b23.as_ref().ok()
        && pics_a23 == pics_b23;
    rep.row(
        "locking a named row writes the lock through the product leg and takes nothing away",
        if armed == 0 {
            Check::Blocked
        } else {
            Check::from(lock_ok)
        },
        format!(
            "产品走 `lock_history({armed}, true)`：它答 {armed_lock:?}（要求 `Some(())`），那句话=\
             {lock_said:?}（要求点名「第 {armed} 行」）。三格计数各归各的：锁 {locks_b23}→\
             {locks_a23}（要求加一），而删除 {dels_b23}→{dels_a23}、清空 {clears_b23}→{clears_a23}\
             （两枚都要求**一动不动**——这一格只写一枚布尔）。本档不再替产品打这把锁，改成当场\
             问盘上：第 {armed} 行此刻 locked={locked_now}（要求 true）。沙盒 {} 里 {rows_b}→{rows_a} \
             行、`history/` 下 {pics_b23}→{pics_a23} 张原图（两枚都要求相等——§5.14.4 规则 2 说的\
             「不得删除」对面是文件，而一把锁自己一份文件也不该拿走）。",
            hdir.display(),
            rows_b = rows_said(&rows_b23),
            rows_a = rows_said(&rows_a23),
        ),
    );

    // 正面那一格判的是 §5.14.4 规则 2 在产品里的**后半**：`include_locked` 钉死 `false` ⇒ 库里
    // 剩下的那两行里，锁定那一行**留下**、它的文件也**留下**，而未锁的那一行两件一起走。前置的
    // 锁自此走产品那条腿（上面那一格），判据仍先把"锁上没上"问一次盘上：控制组自己没摆出来，
    // 这一格就该 BLOCKED。
    let (dels_b22, clears_b22) = mask::with(|m| (m.history_deletes, m.history_clears));
    let rows_b22 = history_rows(&hdir);
    let pics_b22 = picture_count(&hdir);
    let cleared = mask::with(|m| m.clear_history(true));
    let (dels_a22, clears_a22, clear_said) = mask::with(|m| {
        (
            m.history_deletes,
            m.history_clears,
            m.history_clear_last.clone(),
        )
    });
    let rows_a22 = history_rows(&hdir);
    let pics_a22 = picture_count(&hdir);
    let kept = history_picture(&hdir, armed);
    let kept_file = match &kept {
        Ok(Some((Some(p), _, _))) => p.clone(),
        _ => PathBuf::new(),
    };
    let armed_kept = matches!(&kept, Ok(Some(_))) && kept_file.exists();
    let took = cleared.as_ref().map(|f| f.ids.clone()).unwrap_or_default();
    let cleared_ok = locked_now
        && cleared.is_some()
        && !took.contains(&armed)
        && matches!(&rows_a22, Ok(1))
        && armed_kept
        && clears_a22 == clears_b22 + took.len() as u32
        && dels_a22 == dels_b22
        && clear_said.contains("锁定");
    rep.row(
        "a confirmed clear takes the unlocked rows and leaves the locked one",
        if armed == 0 || esc_row.is_none() || !locked_now {
            Check::Blocked
        } else {
            Check::from(cleared_ok)
        },
        format!(
            "前置是上面那一格摆的：第 {armed} 行由产品的 `lock_history` 锁上（{lock_said:?}），\
             本档当场问过盘上、它此刻 locked={locked_now}（要求 true）。产品走 `clear_history(true)`：它答 {cleared:?}\
             （删掉的行号={took:?}，要求不含第 {armed} 行，且取消那一格报的 {esc_row:?} 在其内）；\
             沙盒 {} 里 {rows_b}→{rows_a} 行（要求恰好剩 1），`history/` 下数得出 {pics_b22}→{pics_a22} \
             张原图；第 {armed} 行读回来={kept_s}，它指的那份 {kept_file:?} 此刻还在盘上={armed_kept}\
             （要求 true——§5.14.4 规则 2「不得删除用户明确锁定的记录」要的就是这一件）。两枚计数：\
             删除 {dels_b22}→{dels_a22}（要求一动不动，清空不许并进删除），清空 {clears_b22}→{clears_a22}\
             （要求加数就是这次删掉的行数 {}），那句话={clear_said:?}（要求说清锁定项留下）",
            hdir.display(),
            took.len(),
            rows_b = rows_said(&rows_b22),
            rows_a = rows_said(&rows_a22),
            kept_s = picture_said(&kept),
        ),
    );

    // P107 #197：这一格量的不是像素，是**时刻**。§5.4.3 的指针读自本轮起发生在 [`capture::freeze`]
    // 里，而不是 [`mask::MaskState::commit`] 那道门上；两者在这台机器上的差别是一个方向的问题：
    // 画面冻在遮罩开起来之前，而指针在屏幕上是走的，在门上当场问 ⇒ 贴上去的位置对应的是
    // 「按下完成时指针在哪」，图里根本没有那块屏幕的对应物（PRD 第 419 行）。
    //
    // 判据取的是「多做一次 freeze，读数跟着动一格；这中间一次 commit 也没有」：旧写法在这一格
    // 会是红的（freeze 不吃指针，只有 commit 吃），所以它挡得住将来有人把那一问搬回门上去。
    // 反面那一条 `reads == freezes` whole-flow 同数，挡的是「别处也去问一次桌面」。
    let recs_at_row = mask::with(|m| m.history_records);
    let f_b = capture::freeze_count();
    let r_b = capture::cursor_reads();
    let again = capture::freeze();
    let f_a = capture::freeze_count();
    let r_a = capture::cursor_reads();
    let ms_again = again.as_ref().map(|f| f.ms).unwrap_or(0);
    let again_ok = again.is_ok();
    let took_one_more = again_ok && f_a == f_b + 1 && r_a == r_b + 1;
    let rides_alone = r_a == f_a;
    rep.row(
        "the pointer is read when the screen is frozen, not when the selection is confirmed",
        if !again_ok {
            Check::Blocked
        } else {
            Check::from(took_one_more && rides_alone)
        },
        format!(
            "本档在自己末尾多做了一次 `capture::freeze()`（{ms_again} ms 交出画面，{}）。两枚计数器取在\
             多做之前与之后：冻结 {f_b}→{f_a}（要求加一），指针读 {r_b}→{r_a}（**要求也加一**）——这一格\
             的分量全在这个加一上：多做的那一次 freeze 与上一次读数之间，本档一次 `commit` 也没有按，\
             按过的 commit 总数到今天为止是 {recs_at_row} 行历史（记录次数与指针读数是两枚不同的数，\
             这里只印出来供对账，不参与判定）。旧写法（读在 commit 门上）在这一格交回的是 `reads` 不动\
             而 `freezes` 加一 ⇒ 当场红。全流第二问：{r_a} == {f_a}（要求指针读的次数**恰好等于**冻结\
             次数——多一处调用点就是多一处「这枚指针不属于这一枚画面」）。键的状态本轮读的是档里那一份\
             （`[capture] include_cursor`={key}），所以这两枚计数器数的是「代码在哪一格去问」而不是\
             「问到了什么」：键关着时那一问当场返回 `None`、一次平台调用也不发。这一格因此**不是** \
             §5.4.3 键开那一侧的像素证人（那枚欠着的自 P108 起是第 30 格），它是读数时刻的证人；而 \
             PRD 第 420 行「延时截图应捕获倒计时结束时的指针状态」里的那枚 `capture.delay_seconds` \
             自 P108 起有了第一位**行为**读者（`crates/qt_bridge/src/delay.rs` 的 `arm_from_config`），\
             倒计时那一臂自此是建了的、它的运行期证人在第 29 格；本格钉的仍是时刻那一头，两格各判\
             各的。失败那臂本轮现读\
             一次：freeze 交回 `Err` ⇒ 本格 BLOCKED，那句原话就印在这里={err:?}（成时是空串），不假判。",
            if again_ok {
                "成"
            } else {
                "败"
            },
            key = crate::settings::capture_cfg().include_cursor,
            err = match &again {
                Ok(_) => String::new(),
                Err(e) => e.clone(),
            },
        ),
    );

    // P108 #204：§5.2.4 延时截图那一臂的运行期证人。本格量的仍是**时刻**，不过是它的上半：一枚
    // 三秒的倒计时必须真的等满三秒才交出 `Go`，而在它数满之前，本进程一格画面也不许冻。
    //
    // 口径要说清，不许含糊：本格**自己驱动**这枚倒计时（`delay::arm_at` 起手、`delay::tick()` 数枚），
    // 因为产品那条入口是热键，而热键泵要 `--hotkeys` 才跑、那一扇门本轮不开。所以本格证的是
    // 「等满了没」与「提前冻了没」，**不**证「按下热键会走这条分岔」；后者写在
    // `crates/qt_bridge/src/hotkey.rs` 的 `Work::Capture` 那一格里，本轮只有码与单测，没有读数。
    let fz_b = capture::freeze_count();
    let (tk_b, hl_b) = delay::counts();
    let dl_armed = delay::arm_at(3, delay::clock_ms());
    let dl_planned = delay::planned_ms();
    let dl_left_armed = delay::left();
    let dl_step = delay::step_ms();
    // 真实路径是 `main.qml` 那枚 Timer 在第一枚数**之前**就先等了一个 interval，所以这里也先睡再数。
    // 少睡这一枚，三秒的倒计时会被量成两秒，而 `held >= planned` 那条判据当场就是红的。
    std::thread::sleep(std::time::Duration::from_millis(dl_step));
    let dl_one = delay::tick();
    std::thread::sleep(std::time::Duration::from_millis(dl_step));
    let dl_two = delay::tick();
    std::thread::sleep(std::time::Duration::from_millis(dl_step));
    let dl_three = delay::tick();
    let (dl_held, dl_planned_last) = delay::last_hold();
    let (tk_a, hl_a) = delay::counts();
    let fz_a = capture::freeze_count();
    let dl_left_after = delay::left();
    let dl_after_go = delay::tick();
    let dl_zero = delay::arm_at(0, delay::clock_ms());

    let armed_waits = dl_armed
        == delay::Arm::Waiting {
            left: 3,
            step_ms: delay::STEP_MS,
        };
    let waits_before_last = matches!(dl_one, Some(delay::Tick::Waiting { left: 2 }))
        && matches!(dl_two, Some(delay::Tick::Waiting { left: 1 }));
    let goes_only_last = matches!(dl_three, Some(delay::Tick::Go { .. }));
    let held_out_planned = dl_planned == 3 * delay::STEP_MS
        && dl_planned_last == 3 * delay::STEP_MS
        && dl_held >= dl_planned_last;
    let froze_nothing = fz_a == fz_b;
    let accounted = (tk_a, hl_a) == (tk_b + 3, hl_b + 1);
    let one_shutter_only = dl_left_armed == 3 && dl_left_after == 0 && dl_after_go.is_none();
    let zero_immediate = dl_zero == delay::Arm::Immediate;
    let waited = armed_waits
        && waits_before_last
        && goes_only_last
        && held_out_planned
        && froze_nothing
        && accounted
        && one_shutter_only
        && zero_immediate;
    rep.row(
        "a delayed capture waits out the whole countdown before it shoots",
        Check::from(waited),
        format!(
            "三秒倒计时（`capture.delay_seconds`=3）由本格自己驱动：起手 {dl_armed:?}、挂上时说好的计划 \
             {dl_planned} ms、此刻还剩 {dl_left_armed} 枚、每枚 {dl_step} ms。三枚 tick 顺次交回 \
             {dl_one:?} / {dl_two:?} / {dl_three:?}，判据八件：前两张不许是 `Go`={armed_waits}/{waits_before_last}；\
             第三张必须是 `Go`={goes_only_last}；实测等满 {dl_held} ms 不小于计划 {dl_planned_last} ms=\
             {held_out_planned}——这一条就是 PRD 第 420 行那句「倒计时结束时」里「结束」二字的实测部分；\
             整场里 `capture::freeze()` 一格也没被叫过：冻结计数 {fz_b}→{fz_a}={froze_nothing}，因为画面只该\
             在数完之后才冻；账上 tick {tk_b}→{tk_a} 与走满 {hl_b}→{hl_a} 要求 +3 与 +1={accounted}；\
             一枚按下只交一次快门：数完还剩 {dl_left_after} 枚、再数交回 {dl_after_go:?}={one_shutter_only}；\
             对照臂 `delay_seconds`=0 当场交回 {dl_zero:?}={zero_immediate}，那是没写过这枚键的用户此刻正走的\
             路。本格**不**证的是一件事：它没有按热键，而 `Work::Capture` 那格的分岔读的是档里的键 —— 热键泵\
             要 `--hotkeys` 才跑，那一半的运行期读数本轮仍欠着。",
        ),
    );

    // P109 #207：§5.2.4 步骤 4 的显示链。P108 把倒计时接成了行为读者，但屏幕上还没那一枚数字；
    // P109 补的就是那一半，走 [`delay::hint_text`] 与 [`delay::hint_sequence`] 两枚纯函数。本格
    // 判的是**这条显示链的次序**：起手挂上 3 枚 ⇒ 屏幕上出现「还剩 3 秒」，前两枚 tick 让「还剩
    // 2 秒」「还剩 1 秒」依次上位，第三枚 tick 归零 ⇒ `hint_text(0)` 交回 `None` ⇒ **屏幕上不再
    // 写话**。这一格不驱动 Session 那侧的 `set_status` 与 `set_delay_left`（那需要 GUI 线程与一
    // 枚活的 `Pin<&mut Session>`，而这一档不在 GUI 线程上跑），它驱动的是同一条链的**纯函数腿**；
    // 至于「归零之后 QML handler 收走已经写出去的那一句」这件事由代码结构担保：
    // `SessionRust::delay_round` 的 `Go` 那一臂**先**叫 `set_delay_left(0)`（cxx-qt 同步 notify
    // ⇒ `HintWindow.qml` 那条收窗 handler 当场跑 ⇒ `hint.hide()`），
    // **后**才叫 `open_mask`（底下 `capture::freeze()`）。这一格判的就是这条结构所依赖的那一枚
    // 唯一分岔：`hint_text(0) == None`。
    //
    // **P119 更正一处十轮的空口**：上面那句「结构担保」在 P109〜P118 里并不成立 —— 当时那条
    // handler 的名字是 `onDelayLeftChanged`，而 cxx-qt 为 `delay_left` 生成的信号是
    // `delay_leftChanged`，两边对不上 ⇒ notify 发了、没有人接。P119 起 handler 改绑 QML 自己
    // 声明的 `counting`（`onCountingChanged`），这条结构担保才第一次有据；名字这一格的源码扫描
    // 在 [`crate::hint_check`] 的 tests 里。
    let seq_three = delay::hint_sequence(3);
    let seq_one = delay::hint_sequence(1);
    let seq_zero = delay::hint_sequence(0);
    let ht_zero = delay::hint_text(0);
    let ht_three = delay::hint_text(3);
    let expected = vec![
        "还剩 3 秒".to_string(),
        "还剩 2 秒".to_string(),
        "还剩 1 秒".to_string(),
    ];
    let chain_three = seq_three == expected;
    let chain_one = seq_one == vec!["还剩 1 秒".to_string()];
    let chain_zero_empty = seq_zero.is_empty();
    let hide_signal = ht_zero.is_none();
    let display_ready = ht_three.as_deref() == Some("还剩 3 秒");
    let displayed = chain_three && chain_one && chain_zero_empty && hide_signal && display_ready;
    rep.row(
        "the delayed capture writes its countdown on-screen and takes it away before the shutter",
        Check::from(displayed),
        format!(
            "P109 补的是 §5.2.4 步骤 4「系统显示倒计时」那一半表面：三枚数依次上屏、归零之后不再写。\
             本格判据五件：\
             ① 三秒倒计时的完整显示链 `{seq_three:?}` 必须等于 `{expected:?}`={chain_three} —— 顺序\
             不是口味，倒序让第一枚数报 3、最后一枚报 1，正着排就成了「还剩 1 秒 → 2 秒 → 3 秒」\
             那种往回数，PRD 里没有一句支持这种读法；\
             ② 一秒倒计时 `{seq_one:?}`={chain_one}，只有一句「还剩 1 秒」，Go 归零之后收回；\
             ③ 零秒那一臂（P108 之前唯一的用户路径）`{seq_zero:?}` 空={chain_zero_empty}，屏幕上\
             一格也不写，这条与 [`delay::Arm::Immediate`] 那一条老路一致 ⇒ 建了显示腿不会把\
             `delay_seconds`=0 的用户热键路径改掉；\
             ④ `hint_text(0) = {ht_zero:?}`={hide_signal} —— 归零这一支**必须是 None 而不是空串**，\
             因为空串是一次 `set_status` 写入，QML 侧的 `onStatusChanged` 会当场把 `hint.shown` 又\
             设回 true，等于屏幕上一格也没少；\
             ⑤ 起手那一枚 `hint_text(3) = {ht_three:?}`={display_ready}，`Some(\"还剩 3 秒\")` 就是\
             `Session::refresh_delay` 挂上倒计时之后写出去的那第一句话。\
             「归零之后收走」这一半不在 Rust 的返回值里判，由代码结构担保：`SessionRust::delay_round`\
             的 `Go` 那一臂**先** `set_delay_left(0)`（cxx-qt 同线程 direct connection ⇒ QML\
             `HintWindow.qml` 里那条 `onCountingChanged` 当场把 `hint.shown = false`、`hide()`），\
             **后** `open_mask`（底下 `capture::freeze()`）；这一格判的是这条结构依赖的那一枚唯一\
             分岔是否成立。**P119 说破：这一句「结构担保」在 P109〜P118 是空的** —— 那条 handler 当时\
             叫 `onDelayLeftChanged`，而 cxx-qt 生成的信号叫 `delay_leftChanged`，名字对不上 ⇒ notify\
             发出去没人接，运行时只在 stderr 留一行 `warn:`。P119 起 handler 改绑 QML 自己声明的\
             `counting` 值属性（notify 由 Qt 生成、`onCountingChanged` 对得上），这条担保第一次有据。\
             本格**不**证的仍是那两件 P108 就没证的事：屏幕像素本身（这台仪器不能\
             移动用户指针、也不能真开一扇 Session 的窗），热键泵那一半（`--hotkeys` 不开）。",
        ),
    );

    // ---- P115：§5.14.4 步骤 2 那句话里的「多条」，到上一轮为止在界面上一次也没被读过。上面
    // P101 那一格让界面删过**一行**（`historyDelete(String(row))`），而产品那一头三处早收得下一串：
    // `Session::history_delete` 的 `split('|')`、`MaskState::delete_history(&[i64])`、
    // `History::delete(&[i64])`。递一枚行号过去，「多条」与「单条」在盘上长得一模一样 —— 所以
    // 这一格唯一的新东西不是"又删了一次"，而是**一次调用带走两行**。
    //
    // `arm()` 因此在四枚行号之外又替这一条腿专门种了两枚（[`plan_line`] 的第七、八段），两枚尺寸
    // 不等：字节那一问要能被本档**独立加出来**对一次，两枚一模一样就退化成"猜哪一份都行"。删前各
    // 占多少字节是 `arm()` 当场从库里问回来的（缩略图长度 + 原图文件大小，= `history.rs` 里
    // `bytes_of` 的口径），不是事后按尺寸推算的：事后算等于先假设了"这两行确实各占那么大"。
    //
    // 这一格排在所有格子的**最后**，一个前面的号也不占：往界面那几格里插一格，会把本档后面每一格
    // 的格号整体挪一位，而台账里那些「第 N 格」的指针是按号写的（#205→#211 那一条就是这么长出来的）。
    let (batch_a, batch_b) = planted_batch_rows();
    let (batch_a_file, batch_b_file) = planted_batch_files();
    let (batch_a_bytes, batch_b_bytes) = planted_batch_bytes();
    let seen_batch = reported("deletebatch");
    let at_batch = reported_at("deletebatch");
    let said_pair = format!("第 {batch_a}、{batch_b} 行");
    let batch_took = seen_batch
        .iter()
        .filter(|s| s.contains("删了历史") && s.contains(said_pair.as_str()))
        .count();
    let batch_refused = seen_batch
        .iter()
        .filter(|s| s.contains("一个也没有"))
        .count();
    let batch_says = !seen_batch.is_empty()
        && seen_batch.len() == toolbars
        && batch_took == 1
        && batch_refused + 1 == toolbars;
    let claimed = seen_batch
        .iter()
        .map(String::as_str)
        .find_map(bytes_claimed_in);
    let bytes_ok = claimed == Some(batch_a_bytes + batch_b_bytes);
    let a_gone = matches!(history_picture(&hdir, batch_a), Ok(None));
    let b_gone = matches!(history_picture(&hdir, batch_b), Ok(None));
    let a_file_gone = matches!(&batch_a_file, Some(p) if !p.exists());
    let b_file_gone = matches!(&batch_b_file, Some(p) if !p.exists());
    let batch_armed_still = matches!(history_picture(&hdir, armed), Ok(Some(_)));
    let batch_in_door_dir = count_of_size(&dir, PLANT_BATCH1_W, PLANT_BATCH1_H)
        + count_of_size(&dir, PLANT_BATCH2_W, PLANT_BATCH2_H);
    let after_the_delete_leg = at_batch.len() == toolbars
        && at_delete.len() == toolbars
        && at_delete.iter().zip(&at_batch).all(|(d, b)| d < b);
    let batch_ok = batch_says
        && bytes_ok
        && a_gone
        && b_gone
        && a_file_gone
        && b_file_gone
        && batch_armed_still
        && batch_in_door_dir == 0
        && after_the_delete_leg
        && dels_now == dels_at_arm + 1 + 2;
    rep.row(
        "the toolbar deleted two named rows in one call",
        if armed == 0 || batch_a == 0 || batch_b == 0 || batch_a_file.is_none() || batch_b_file.is_none()
        {
            Check::Blocked
        } else {
            Check::from(batch_ok)
        },
        format!(
            "`deletebatch|` 那几行={seen_batch:?}（要求条数等于这一场的 {toolbars} 面遮罩；{toolbars} 条里\
             恰好一条说「删了历史」且点名 {said_pair}，其余每条说「一个也没有」。那一句点名的形状是\
             **带顺序**的：`mask.rs` 把 `freed.ids` 按调用方给的顺序拼进句子，界面递的是 `{batch_a}|{batch_b}`，\
             反过来写就不认 —— 一次调用带走两行这件事，判据得连「哪两行、什么顺序」一起说。本档一处也不写\
             这枚前缀，所以它只能是 `AnnotationToolbar.qml` 递回来的）；那句自称的字节={claimed:?} 对 `arm()`\
             当场从库里问回来的 {batch_a_bytes}+{batch_b_bytes}={}（要求相等：尺子是删前的两份**实际**字节，\
             缩略图长度加原图文件大小，与 `history.rs` 的 `bytes_of` 同口径，不是事后按尺寸推算的，也不是\
             `Freed` 结构体自己——拿它当尺子就是让产品给自己作证）；盘上四问：第 {batch_a} 行读回来是「没有\
             那一行」={a_gone}、第 {batch_b} 行同上={b_gone}，它们本来指的两份图 {batch_a_file:?} 与 \
             {batch_b_file:?} 此刻都已不在={a_file_gone}/{b_file_gone}（对面是文件系统，不是那两句话）；\
             `armed` 那一行的图还在={batch_armed_still}（要求 true——界面碰掉的仍不是本档自己的前置，上面\
             清单、回放、恢复、翻页、邻居那些格的读数因此一格没收走另一格的）；落点目录里解码成 \
             {PLANT_BATCH1_W}×{PLANT_BATCH1_H} 与 {PLANT_BATCH2_W}×{PLANT_BATCH2_H} 的文件数\
             ={batch_in_door_dir}（要求 0：这一条腿只删不回放，两枚尺寸是本档另立的，掺进回放那一格数到的\
             图里就是串门）。删除计数从 `arm()` 的基准走到这里：{dels_at_arm}→{dels_now}（要求加三——上面\
             P101 那条单行腿一次、这一条两枚的腿两次；上面那一格读到的加数是 1+{batch_gone}，而那一枚\
             {batch_gone} 由盘上的「这两行还在不在」定，不由本格的判决定，两格因此各自独立又不互相顶撞）。\
             落位={at_delete:?} → {at_batch:?}（要求每一面工具条的单行那次删除排在它自己的两枚点名之前：\
             QML 那条链是 read→drop→restore→batch，这条腿挂在末尾，读数反过来说明界面把顺序改了）。\
             §5.14.4 步骤 2 的「多条」自此在界面上有了第一次读数；两条腿落在**不同**的行号上，是因为\
             [`plan_line`] 第七、八段那两枚是 `arm()` 专门替这一格种的。",
            batch_a_bytes + batch_b_bytes,
        ),
    );

    rep.note(
        "auto door subject",
        format!(
            "落盘开关={switch} 复制开关={switch_copy} 文件={landed:?} 落点={} 落盘计数={saves_after} \
             递出计数={copies_after} 递出那句话={copy_said:?} 取消开关=钉真 取消行号={esc_row:?} \
             取消行数={esc_b}→{esc_a} 取消那句话={esc_said:?} 回放行号={commit_row:?} \
             回放次数={replays_before}→{replays_after} 回放落到={replayed:?} 那一行的图={picture_s} \
             反面行号={missing} 反面那句话={missing_said:?} 删除行号={commit_row:?} \
             删掉的行={gone_ids:?} 收回字节={freed_bytes} 删除计数={dels_before}→{dels_after} \
             那句话={del_said:?} 清空未确认那句话={noc_said:?} 清空确认删掉的行={took:?} \
             清空计数={clears_b22}→{clears_a22} 锁定那一行={armed} 锁那一格答={armed_lock:?} \
             三格计数：锁 {locks_b23}→{locks_a23}、删除 {dels_b23}→{dels_a23}、清空 \
             {clears_b23}→{clears_a23} 那句话={lock_said:?} 沙盒 {} \
             种行那格={} 界面清单条数={n_rows} 界面回放条数={n_replays} \
             界面那四条去侧腿：未确认清空={n_clear_no} 锁={n_lock} 删={n_delete} 一次点名两枚={n_batch} \
             替界面种的那一行={ui_row} 它本来指的图={ui_file:?} 那一刻的计数=删{dels_now}/\
             清{clears_now}/锁{locks_now}（基准 删{dels_at_arm}/清{clears_at_arm}/锁{locks_at_arm}）\
             恢复那一行={esc_id} 它记的原选区={want_s} 交回={restored:?} 选区 {hole_pre_restore}→\
             {hole_after_restore} 反面行号={missing} 反面那句话={ghost_said:?} 恢复计数=\
             {restores_b102}→{restores_after_ghost} \
             递回内容那一行={esc_id} 它记的图={copy_pic_s} 要求尺寸={want_dims:?} 剪贴板 \
             {dims_b116:?}→{dims_pushed:?}（{bytes_pushed} 字节）反面行号={missing} 反面那句话=\
             {ghost_said_116:?} 反面那一张={dims_ghost:?} 递回计数={hcopies_b116}→{hcopies_after}→\
             {hcopies_ghost} 还清那一份={left_found} 十四枚动了=正{moved_pos:?}/反{moved_neg:?}",
            dir.display(),
            hdir.display(),
            armed_s,
            copy_pic_s = picture_said(&copy_pic),
            n_rows = seen_rows.len(),
            n_replays = seen_replays.len(),
            n_clear_no = seen_clear_no.len(),
            n_lock = seen_lock.len(),
            n_delete = seen_delete.len(),
            n_batch = seen_batch.len(),
        ),
    );
    rep.finish("auto door")
}

/// 本档自己的下限，与 `reveal_check::MIN_MS` 同口径：遮罩要开得起来，`commit` 要在事件
/// 循环跑过一轮之后才有画面可拼。
pub const MIN_MS: i32 = 900;

/// 遮罩开不起来时的那句收尾，与另外几道门同一句。
pub fn open_failed(e: &str) -> (Check, String) {
    let mut rep = Report::new();
    rep.row(
        "auto door is not a gauge",
        Check::Blocked,
        format!("遮罩开不起来（{e}），这一档的几格全都没得问"),
    );
    rep.finish("auto door")
}
