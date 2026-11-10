//! §5.8.4 方式二「把一张图拖到已存在的贴图窗上」那一支的**真机一档**。
//!
//! `--drop-in` 那一门量的是这一支**调用的那三手**（`pieces` / `one` / `running_line`）与**交给的
//! 那两手**（core `drop::sort`、`state::land_drop`），而它自己明确写着「这一档没有跑过
//! [`crate::drop_in::start`] 本体」—— 那半笔账的成因是：本体要的那枚 `Pin<&mut Session>` 只有
//! QML 线程拿得到，而 `--drop-in` 在 `engine.load()` 之前就退。
//!
//! 这一档来把那半笔账收掉：它在**真 Session** 上真跑一遍本体 —— 真图片、真落点、真换图、
//! 真那一行字。**剩下那半笔账是 OS 那一手拖放**（按住浏览器里的一张图挪到窗上再松手）：
//! 那一手要的是一次真的鼠标动作，本档替不了它，也不拿别处的读数冒充它。
//!
//! # 这一档跑起来是什么样
//!
//! 1. 进程起来时（**在 `settings::start()` 之前**）：本档造一份自己的 config（只写
//!    `[pin] drop_replaces_current = true` 那一枚键）并把 `FALCONSHOT_CONFIG` 指到它 —— 于是
//!    「换不换」那一格是定的，而用户的档一个字也没动（§9.1 第十五条）。
//! 2. 种两张图：一张**真的** PNG（60×40），一张**后缀认得却解不出来**的坏档（§8.2 那四句里的
//!    第四句）。
//! 3. `engine.load()` 之后：在 GUI 线程上真递两趟
//!    [`crate::session::qobject::Session::pin_drop`] —— 先种一枚 40×30 的演示贴图当「要被换掉的
//!    那一枚」，再递那张真图（该换），再递那张坏档（不该换，且说的是第四句）。
//! 4. 闹钟响、进程退之后：把两趟的读数与那一枚贴图此刻的源图尺寸读出来，印成一份报告。
//!
//! # 三件不许静默的事
//!
//! - 本体没跑成（GUI 那一头没到手 / Qt 拒了排队）⇒ 那一格发 BLOCKED，不是拿「没跑」冒充「跑过」；
//! - 坏档那一趟必须说**第四句**（§8.2 那四句不重叠：后缀认得却解不出来 ⇒「可能已损坏」，不是
//!   「不支持」），说错了就 FAIL —— 那一句话的作者是 [`crate::state::corrupt_refusal`]，
//!   门里不抄一遍（#281）；
//! - OS 那一手拖放本档**没有**跑过 —— 那一格永远发 BLOCKED，并写明欠的是什么（[`remaining_leg`]）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cxx_qt_lib::{QString, QUrl};

use crate::r13::Report;
use crate::state::Check;

/// 本档种下的那几样东西（跑之前备好，跑的时候只递它们）。
pub struct Prepared {
    /// 那张真图片的 URL（Qt 自己编的那一句，与 QML 递过来的是同一种形状）。
    pub good: String,
    /// 那张坏档的 URL。
    pub broken: String,
    /// 那张真图片的尺寸（换成功之后，那一枚贴图的源图该是这个尺寸）。
    pub good_size: (u32, u32),
    /// 那张坏档在盘上的名字（拒绝句里带的就是它）。
    pub broken_shown: String,
}

/// 本体跑完之后的读数（GUI 线程那一趟交回的）。
#[derive(Clone, Debug, Default)]
struct Outcome {
    /// 真图那一趟屏幕上那一行。
    good_said: String,
    /// 真图那一趟之后那一枚贴图的源图尺寸。
    good_size: (u32, u32),
    /// 坏档那一趟屏幕上那一行。
    broken_said: String,
    /// 坏档那一趟之后那一枚贴图的源图尺寸。
    broken_size: (u32, u32),
    /// 本体有没有真跑起来（跑不起来的那一句也是读数，不是空）。
    fault: String,
}

/// 本档的读数放在哪儿：GUI 线程写、`app.exec()` 回来之后读。
///
/// 一格 `Mutex` 而不是通道：那一头只写一次、这一头只读一次，中间只需要「写完了没有」这一句。
static OUTCOME: Mutex<Option<Outcome>> = Mutex::new(None);

/// 演示贴图那一张的尺寸（换之前它是这一枚的样子）。
const DEMO_SIZE: (u32, u32) = (40, 30);

/// 这一档的地板（毫秒）：QML 起来、贴图窗长出来、本体两趟各过一次桥、读数写回来。
///
/// 不给数字就退化成一次等真人的会话，而 stdout 是行缓冲的 —— 那与「零字节日志」分不开（⑭）。
pub const MIN_MS: i32 = 1200;

/// 本档的目录（自己的，不与 `--drop-in` 那一门共用 —— 同一台机器上两趟并行时各自的图片
/// fixture 得是各自的文件，与 [`crate::drop_check`] 那一门同一形状）。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_DROP_LIVE_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-drop-live-door");
    dir
}

/// 本档那一份 config 的路径（`--drop-live` 那一门把它指给 `FALCONSHOT_CONFIG`）。
///
/// 造这一份档的手是 [`crate::drop_check::store_switch`] —— 「只写 `[pin]
/// drop_replaces_current` 那一枚键」这件事只有一份写法，两处各写一份就会各自漂移（#281）。
pub fn plant_config() -> Result<PathBuf, String> {
    let dir = door_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("造不出本档目录（{}）：{e}", dir.display()))?;
    crate::drop_check::store_switch(&dir, "drop-live.toml", "true")
        .map(|(path, _)| path)
        .ok_or_else(|| format!("写不出本档那一份 config（{}）", dir.display()))
}

/// 本档种下的那几样东西（图片；演示贴图在 GUI 线程上种 —— 那时要问屏幕）。
pub fn prepare() -> Result<Prepared, String> {
    let dir = door_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("造不出本档目录（{}）：{e}", dir.display()))?;
    let (good_path, _) = crate::drop_check::plant_picture(
        &dir,
        "drop-live-good.png",
        60,
        40,
        [200, 40, 40, 255],
    )
    .ok_or("种不出那张真图片（编码或落盘失败）")?;
    // 坏档：后缀认得（png）却解不出来 —— §8.2 那四句里的第四句。
    let broken_path = dir.join("drop-live-broken.png");
    std::fs::write(&broken_path, b"\x89PNG\r\n\x1a\n not a picture at all")
        .map_err(|e| format!("写不出那张坏档：{e}"))?;
    Ok(Prepared {
        good: file_url(&good_path),
        broken: file_url(&broken_path),
        good_size: (60, 40),
        broken_shown: broken_path.display().to_string(),
    })
}

/// 一条路径 → 一句 URL。
///
/// 这一句是 **Qt 编的**（`QUrl::fromLocalFile` + `toString`），与 QML 那一头 `String(url)` 递过来
/// 的是**同一种形状** —— 两者走的都是 Qt 的 `toString()`（PrettyDecoded：空格与中文留着不编码）。
/// 手写一份「file:///C:/…」就是第二个会各自漂移的作者（#281），而这一族最容易错的就是「自己拼
/// 一份 URL」与「照抄 Qt 编的那一句」之间的那点差别。
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    QUrl::from_local_file(&QString::from(text.as_str()))
        .to_qstring()
        .to_string()
}

/// 在 GUI 线程上真跑两趟本体，把读数写进 [`OUTCOME`]。
///
/// 只能这么跑：本体要的那枚 `Pin<&mut Session>` 只有 QML 线程拿得到（§5.8.4 方式二第 3 步的
/// 接线位置），而「排进 Qt 的事件环」那一手只有 [`crate::hotkey::post_to_gui`] 一个作者。
///
/// 排不进去时 [`OUTCOME`] 里**什么也不写** —— 那一格于是发 BLOCKED（「本体没跑过」），而不是
/// 拿一组没跑出来的数字冒充跑过（#453）。
pub fn drive(prepared: &Prepared) {
    let good = prepared.good.clone();
    let broken = prepared.broken.clone();
    crate::hotkey::post_to_gui(move |mut session| {
        let mut outcome = Outcome::default();
        // 一枚演示贴图：要被换掉的那一枚（40×30 的格子图）。
        match crate::state::add_demo(DEMO_SIZE.0, DEMO_SIZE.1) {
            Ok(pin) => {
                let id = pin as i64;
                // 第一趟：那张真图（`file:` 那一支不经网络 ⇒ 这一趟当场落）。
                let said = session.as_mut().pin_drop(id, &QString::from(good.as_str()));
                outcome.good_said = said.to_string();
                outcome.good_size = source_size(pin);
                // 第二趟：那张坏档（后缀认得却解不出来 ⇒ §8.2 那四句里的第四句）。
                let said = session.as_mut().pin_drop(id, &QString::from(broken.as_str()));
                outcome.broken_said = said.to_string();
                outcome.broken_size = source_size(pin);
                session.as_mut().refresh();
            }
            Err(e) => outcome.fault = format!("种不出那一枚演示贴图：{e}"),
        }
        *OUTCOME.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
    });
}

/// 那一枚贴图此刻的源图尺寸（宽×高）。名册上压根没有那一枚时答 (0, 0)。
fn source_size(pin: falcon_core::pin::PinId) -> (u32, u32) {
    crate::state::with(|s| {
        s.set
            .get(pin)
            .map(|p| (p.source_size.w, p.source_size.h))
            .unwrap_or((0, 0))
    })
}

/// 这一支**还欠着**的那半笔账：**OS 那一手拖放**。
///
/// 单独一只手写，是因为它同时是本档那一格的正文与 [`crate::drop_check`] 那一门「没量过的事」
/// 的落笔处（#281：同一句欠账在两处各写一遍，总有一天一处新过一处）。
///
/// 它**只说还欠着的那一半**：「本体那一半已经跑过」是本档的成绩，不是欠账 —— 把成绩与欠账写进
/// 同一句，那一句就既夸了自己又替自己开脱，而读它的人分不出哪半句是哪半句（#453）。
pub fn remaining_leg() -> String {
    "OS 那一手拖放这一半**没有跑过** —— 那要的是一次真的鼠标动作：按住浏览器里的一张图，挪到一扇 \
     贴图窗上，松手。QML 那一侧（`PinWindow.qml` 的 `DropArea` → `session.pinDrop`）与 OS 那一侧的 \
     接线形状由 `--drop-in` 那一门的源码形状读。本档替不了那一次鼠标动作，也不拿上面那几格的读数 \
     冒充它（#453）。"
        .to_string()
}

/// 那一趟的读数 → 一份报告（`--drop-live` 那一门印的就是它）。
///
/// 每一格都是**读数**：本体有没有真跑、那一枚贴图有没有真换、屏幕上那一行说的是不是那一件事、
/// 坏档说的是不是第四句、坏档有没有把贴图换掉。而「OS 那一手拖放」那一格永远 BLOCKED ——
/// 本档替不了 一次真的鼠标动作（#453：没量过的事不许拿别处的读数冒充）。
pub fn measure(prepared: &Prepared) -> (Check, String) {
    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let outcome = OUTCOME.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(outcome) = outcome else {
        rep.row(
            "本体跑过",
            Check::Blocked,
            "GUI 那一头没到手（或 Qt 拒了这一次排队），本体一次也没跑过".to_string(),
        );
        rep.row("OS 拖放", Check::Blocked, remaining_leg());
        return rep.finish("drop-live door");
    };

    // ① 本体跑过没有：跑起来了、且没有故障。
    rep.row(
        "本体跑过",
        if outcome.fault.is_empty() {
            Check::Pass
        } else {
            Check::Fail
        },
        if outcome.fault.is_empty() {
            format!("真跑了一遍：屏幕上那一行 = {}", outcome.good_said)
        } else {
            outcome.fault.clone()
        },
    );

    // ② 那一枚贴图有没有真换：源图从演示那一张的 40×30 变成真图的 60×40。
    let swapped = outcome.good_size == prepared.good_size;
    rep.row(
        "贴图真换了",
        Check::from(swapped),
        format!(
            "源图 {}×{} → 真图 {}×{}",
            DEMO_SIZE.0, DEMO_SIZE.1, outcome.good_size.0, outcome.good_size.1
        ),
    );

    // ③ 屏幕上那一行说的是不是「换这一枚」—— 那句话的作者是
    // [`falcon_core::drop::Landing::plan`]，门里不抄一遍（#281）。
    let (landing, said) = falcon_core::drop::Landing::plan(true, 1);
    let right = outcome.good_said.contains(said) && outcome.good_said.contains(landing.word());
    rep.row("那一行说的是换", Check::from(right), outcome.good_said.clone());

    // ④ 坏档那一句是不是第四句：后缀认得却解不出来 ⇒「可能已损坏」，不是「不支持」。
    let expected = crate::state::corrupt_refusal(&prepared.broken_shown);
    let corrupt_right = outcome.broken_said.contains(&expected);
    rep.row(
        "坏档是第四句",
        Check::from(corrupt_right),
        format!(
            "屏幕上那一行 = {}（要的那一句 = {expected}）",
            outcome.broken_said
        ),
    );

    // ⑤ 坏档那一趟没有把贴图换掉：还是真图那一张。
    let stayed = outcome.broken_size == prepared.good_size;
    rep.row(
        "坏档没换掉",
        Check::from(stayed),
        format!(
            "坏档那一趟之后源图 {}×{}",
            outcome.broken_size.0, outcome.broken_size.1
        ),
    );

    // ⑥ OS 那一手拖放：这一档替不了。
    rep.row("OS 拖放", Check::Blocked, remaining_leg());

    rep.finish("drop-live door")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 那一句 URL 是 **Qt 编的**，而且是 QML 递过来的**同一种形状**。
    ///
    /// 这一格钉的是「门递过去的那一句与 QML 递过来的是同一种形状」—— 手写一份百分号编码就是
    /// 第二个作者（#281）。而形状是什么：Qt 的 `QUrl::toString()` 是 **PrettyDecoded**（空格与中文
    /// 留着不编码），QML 那一头的 `String(url)` 走的正是同一个 `toString()` —— 所以空格**不该**被
    /// 编成 `%20`（本程序 `save::local_path` 那一边解的是 `%XX`，而 QML 递过来的那一句里本来就没有
    /// `%XX`）。这一格钉的正是「Qt 编的那一句能被本程序原样读回同一条路径」。
    #[test]
    fn the_url_is_the_one_qt_writes() {
        let dir = std::env::temp_dir().join("falconshot drop live 形状");
        let path = dir.join("a b.png");
        let url = file_url(&path);
        assert!(url.starts_with("file:///"), "{url}");
        assert!(
            url.contains("a b.png"),
            "Qt 编的那一句是 PrettyDecoded（空格留着不编码），与 QML 的 `String(url)` 同一形状：{url}"
        );
        assert_eq!(
            crate::save::local_path(&url),
            path.to_string_lossy().to_string(),
            "Qt 编的那一句要能被本程序读回同一条路径"
        );
    }

    /// 本档的目录是**自己的**：与 `--drop-in` 那一门不共用一份，两趟并行时各自的图片 fixture
    /// 得是各自的文件（与 `drop_check::door_dir` 同一形状）。
    #[test]
    fn the_live_door_has_a_directory_of_its_own() {
        let dir = door_dir();
        assert!(!dir.as_os_str().is_empty());
        assert_ne!(
            dir.file_name().and_then(|n| n.to_str()),
            crate::drop_check::door_dir().file_name().and_then(|n| n.to_str()),
            "两档共用一份目录 ⇒ 并行时各自的 fixture 会打架"
        );
    }
}
