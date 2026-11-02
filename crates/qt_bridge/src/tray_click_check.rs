//! §5.18.2 的那一扇门：四枚鼠标键各跑哪一枚动作，以及那一格「不许把所有入口都关死」的规则
//! 到底修不修得回来。
//!
//! 四枚键在 P144 之前不是一份配置，而是 `hotkey.rs` 里写死的一次 `Event::TrayLeft ⇒
//! Work::Capture`：左键与右键的行为在码上，中键与「双击」根本没有归宿（平台层把
//! `WM_LBUTTONUP` 与 `WM_LBUTTONDBLCLK` 折成同一枚事件，所以「左键=截图、双击=贴图」这种
//! 配法连被写出来的地方都没有）。文档第 1449 行点名的正是这四枚，所以这一档判的是**四枚
//! 各自有没有归宿**，而不是其中一枚。
//!
//! ## 为什么这一档不开窗、也不等任何人
//!
//! 它读的全是「一份档 → 一个决定」这条路：真写三份档到本档自己的目录、真走产品那个
//! [`Settings::open`] 读回来（`Config::load` ⇒ `validate` ⇒ 归一与规则，一次都不绕），再把
//! 读到的动作名递给**产品那一层用的同一张表** [`tray_run_of`]。于是锁屏会话也判得出，也不必
//! 有人的手放在鼠标上。
//!
//! ## 这一档不声称的那件事
//!
//! 它一次也没有往消息窗投过 `WM_LBUTTONUP` / `WM_LBUTTONDBLCLK` / `WM_MBUTTONUP` /
//! `WM_RBUTTONUP`，所以它说不出「真点下去时走的是不是这一臂」。那一格发的是 Blocked，不是
//! 删掉的一格，也不是拿上面的读数冒充——这一档的天花板写在报告里那一行，而不是只写在注释里
//! 等有人去读源码。另有一条已知的代价记在同一格：托盘用的是 shell **Version 0** 结构
//! （`platform-windows/hotkey.rs` 里那处注释写明），双击会同时送来一次 UP，所以「左键与双击
//! 配成不同动作」时双击会先跑一次左键那一格。
//!
//! ## 用户那份档
//!
//! 三份 fixture 都落在本档目录里，`%APPDATA%\ai-falconshot\config.toml` 一个字节也不动
//! （§9.1 第十五条）。三枚 `pinned` 各自钉在自己那份 fixture 上，于是「这一趟改了用户的档」
//! 连结构上都不成立；用户档里那四枚此刻是什么，只作为读数印在最后一行，不参与任何判定。

use std::path::PathBuf;

use falcon_core::config::{LoadSource, Tray, TRAY_ACTIONS, TRAY_ENTRY_ACTIONS};

use crate::hotkey::{tray_run_of, TrayRun};
use crate::r13::Report;
use crate::settings::Settings;
use crate::state::Check;

/// 本档自己的目录。
///
/// `FALCONSHOT_TRAY_DOOR_DIR` 是给跑级档用的：同一台机器上两趟并行时各自的 fixture 得是各自
/// 的文件，否则后写的那一份会替前一趟改题。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_TRAY_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-tray-door");
    dir
}

/// 一份 fixture：文件名、要写进去的那几行、以及这一格要求它读回来成什么样子。
struct Fixture {
    name: &'static str,
    text: &'static str,
    /// 读回来之后四枚键该是什么，顺序就是 [`Tray::entries`] 的顺序。
    want: [(&'static str, &'static str); 4],
}

/// A：四枚全在名单上、且右键开菜单 —— §5.18.2 第二步「分别设置」那一问的形状。
///
/// 挑的值刻意**不等于默认**（默认是 `capture / capture / none / menu`）：四枚里三枚与默认不
/// 同，才问得出「读回来的是这份档还是 `Config::default()`」。右键留成 `menu` 是必需的，因为
/// 这一份要的是「规则不该出手」那半边。
const A: Fixture = Fixture {
    name: "a-routed.toml",
    text: "[tray]\nleft = \"paste_pin\"\ndouble = \"show_all_pins\"\n\
           middle = \"hide_all_pins\"\nright = \"menu\"\n",
    want: [
        ("left", "paste_pin"),
        ("double", "show_all_pins"),
        ("middle", "hide_all_pins"),
        ("right", "menu"),
    ],
};

/// B：四枚**全都**进不了菜单也进不了设置页 —— 文档第 1455–1456 行那两句规则说的就是这一份。
const B: Fixture = Fixture {
    name: "b-no-way-in.toml",
    text: "[tray]\nleft = \"capture\"\ndouble = \"paste_pin\"\n\
           middle = \"hide_all_pins\"\nright = \"toggle_pins\"\n",
    // 规则交回的那一手：只有右键被落回 menu，其余三枚一动不动。
    want: [
        ("left", "capture"),
        ("double", "paste_pin"),
        ("middle", "hide_all_pins"),
        ("right", "menu"),
    ],
};

/// C：两枚键写的是名单外的名字，其中一枚正是**规则要看的那一枚**。
///
/// 这一份不是为了测「打错字会怎样」才挑的，是为了量一句先后：`validate` 先归一、后判规则，
/// 那么 `right = "menuu"` 会先落回它自己的默认 `menu`，规则看到的就是一扇开着的门 ⇒ 只该有
/// 「不是托盘动作」那两句，**不该**有「四枚都进不去」那一句。反过来先判规则再归一，两句都会
/// 在。所以判据是「规则那句缺席」，而不是「值等于 menu」——两种顺序都修得出同一个值，只有
/// 那句的缺席分得出先后。
const C: Fixture = Fixture {
    name: "c-typo.toml",
    text: "[tray]\nleft = \"hide_all_pins\"\ndouble = \"capture\"\n\
           middle = \"history\"\nright = \"menuu\"\n",
    want: [
        ("left", "hide_all_pins"),
        ("double", "capture"),
        ("middle", "none"),
        ("right", "menu"),
    ],
};

/// 把归宿那一臂说成一句可比对的话。名字来自码，不是这一档另起的一张表。
fn arm(run: &TrayRun) -> String {
    match run {
        TrayRun::Menu => "菜单".to_string(),
        TrayRun::Nothing => "不动手".to_string(),
        TrayRun::Work(work) => format!("Work({work:?})"),
        TrayRun::Unwired(other) => format!("未接线({other:?})"),
    }
}

/// 四枚键逐枚对，交回**不对**的那几枚的说法。空 = 全对上。
///
/// 对的是 [`Tray::entries`] 那一份顺序——页、归一、规则、门读的都是它，所以这一格红的时候没
/// 有「另一处读的是别的顺序」可推。
fn off(tray: &Tray, want: &[(&'static str, &'static str); 4]) -> Vec<String> {
    let mut bad: Vec<String> = Vec::new();
    for ((name, got), (_, exp)) in tray.entries().into_iter().zip(want.iter().copied()) {
        if got != exp {
            bad.push(format!("{name}={got}(要求 {exp})"));
        }
    }
    bad
}

/// 四枚键说成一句，与 [`roster_vals`] 同形状，好让「读回来」与「要求」并排看。
fn roster(tray: &Tray) -> String {
    tray.entries()
        .into_iter()
        .map(|(name, action)| format!("{name}={action}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn roster_vals(want: &[(&'static str, &'static str); 4]) -> String {
    want.iter()
        .map(|(name, action)| format!("{name}={action}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 「这一格要求的四枚」与「这一格读回来的四枚」并排说一句。
fn pair(got: &Tray, want: &[(&'static str, &'static str); 4]) -> String {
    format!("读回来 {}；要求 {}", roster(got), roster_vals(want))
}

/// 把四枚键连同各自的归宿说一句。
fn with_arms(tray: &Tray) -> String {
    tray.entries()
        .into_iter()
        .map(|(name, action)| format!("{name}={action}→{}", arm(&tray_run_of(action))))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 写下 fixture 并从它读回来。第三个数是「种下去这一步自己成不成」——写不下去时不能拿一份
/// 上一趟留下的同名文件替本趟作证，所以那一句要跟路径一起印在第一格上。
fn planted(fixture: &Fixture) -> (PathBuf, Settings, String) {
    let dir = door_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(fixture.name);
    let seed = match std::fs::write(&path, fixture.text) {
        Ok(()) => "种下了".to_string(),
        Err(e) => format!("种不下去：{e}"),
    };
    let settings = Settings::open(&path, Some(&path));
    (path, settings, seed)
}

/// 这一档：三格判据 + 一格名单一致性 + 一格发出来的 Blocked，外加两行读数。
pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (path_a, a, seed_a) = planted(&A);
    let (path_b, b, seed_b) = planted(&B);
    let (path_c, c, seed_c) = planted(&C);

    // ---- 第一格先让后面几格有得问：三份档得是真从文件里读出来的，而不是解析失败之后那一份
    // `Config::default()`。`source` 与 `backup` 两枚一起看，因为 Recovered 也留着同一个路径、
    // 读回来却是默认值——只看路径会读出一句假绿。
    let source = |s: &Settings| match s.source {
        LoadSource::Defaults => "defaults",
        LoadSource::File => "file",
        LoadSource::Recovered => "recovered",
    };
    let loaded = |s: &Settings| s.source == LoadSource::File && s.backup.is_none();
    rep.row(
        "tray fixtures load",
        Check::from(loaded(&a) && loaded(&b) && loaded(&c)),
        format!(
            "三份 fixture：{} {}（{seed_a}）；{} {}（{seed_b}）；{} {}（{seed_c}）——要求 from=file \
             且没被挪走成 broken 档，种子那一步也各自说出成败。本档目录 {}；用户那份档不在这里、也\
             不被写（三枚 pinned 各钉在自己那份上）",
            path_a.display(),
            source(&a),
            path_b.display(),
            source(&b),
            path_c.display(),
            source(&c),
            door_dir().display()
        ),
    );

    // ---- §5.18.2 第二步那一句「分别设置左键、中键、右键或双击对应的动作」：四枚各问一次，
    // 递出去的是产品那张表，不是这一档自己抄的一份。
    let arms_a = with_arms(&a.config.tray);
    let arms_want = A
        .want
        .iter()
        .map(|(name, action)| format!("{name}={action}→{}", arm(&tray_run_of(action))))
        .collect::<Vec<_>>()
        .join(" ");
    let base = Tray::default();
    let mut differs: Vec<String> = Vec::new();
    for ((name, got), (_, default)) in a.config.tray.entries().into_iter().zip(base.entries()) {
        if got != default {
            differs.push(format!("{name}:{default}→{got}"));
        }
    }
    rep.row(
        "tray four entries",
        Check::from(
            off(&a.config.tray, &A.want).is_empty() && arms_a == arms_want && differs.len() >= 3,
        ),
        format!(
            "{}；归宿那一路是 {arms_a}（要求 {arms_want}）；跟默认不同的 {} 枚 {:?}（默认是 \
             {}）——少于 3 枚就说明这份档没改出什么，那几枚同名的键也就证不出「跟着档走」",
            pair(&a.config.tray, &A.want),
            differs.len(),
            differs,
            roster(&base)
        ),
    );

    // ---- 名单与那张表的一致性。这一格是这一档唯一能替 #281 站岗的地方：`TRAY_ACTIONS` 加一
    // 枚而 `Work` 那边漏一手时，只有这里会红——门与 `route` 读的是同一张表，所以那张表自己缺
    // 臂不会被任何一次点击测出来，只会让用户配上去什么也不发生（P140 在热区那头关掉的就是这
    // 一族）。反向也判：规则用来放行那两名（`TRAY_ENTRY_ACTIONS`）必须是**用户配得到**的名，
    // 且递进表里真的落在「开菜单」或「开设置页」那一臂上，否则第 1455 行那句规则是在许一个
    // 路由兑现不了的诺。
    let mut unwired: Vec<String> = Vec::new();
    let mut not_offered: Vec<String> = Vec::new();
    let mut entry_wrong: Vec<String> = Vec::new();
    let mut entry_arms: Vec<String> = Vec::new();
    for action in TRAY_ACTIONS {
        if matches!(tray_run_of(action), TrayRun::Unwired(_)) {
            unwired.push(action.to_string());
        }
    }
    for action in TRAY_ENTRY_ACTIONS {
        entry_arms.push(format!("{action}→{}", arm(&tray_run_of(action))));
        if !TRAY_ACTIONS.contains(action) {
            not_offered.push(action.to_string());
        }
        if !matches!(
            tray_run_of(action),
            TrayRun::Menu | TrayRun::Work(crate::hotkey::Work::Settings)
        ) {
            entry_wrong.push(format!("{action}→{}", arm(&tray_run_of(action))));
        }
    }
    rep.row(
        "tray action roster",
        Check::from(unwired.is_empty() && not_offered.is_empty() && entry_wrong.is_empty()),
        format!(
            "{} 枚动作逐名递进表里：没有归宿 {} 枚 {unwired:?}；配不到的入口名 {} 枚 \
             {not_offered:?}；入口落在错臂上 {} 枚 {entry_wrong:?}（要求三处都空）。入口那两名读\
             回来是 {}；名单 {TRAY_ACTIONS:?}",
            TRAY_ACTIONS.len(),
            unwired.len(),
            not_offered.len(),
            entry_wrong.len(),
            entry_arms.join(" ")
        ),
    );

    // ---- 规则本体走一次**真加载**：`Config::load` 里那句 `cfg.validate()` 是唯一会替用户改
    // 值的地方，所以在单测里直接调 `validate` 不是这一格的对面——那一条走不到「档上真写着四枚
    // 都进不去」这份文件。
    let notes_b: Vec<String> = b
        .notes()
        .into_iter()
        .filter(|w| w.contains("tray"))
        .collect();
    let repaired_b = notes_b.iter().any(|w| {
        w.contains("opens the tray menu") && w.contains("tray.right falls back to \"menu\"")
    });
    rep.row(
        "tray rule repairs",
        Check::from(off(&b.config.tray, &B.want).is_empty() && repaired_b && notes_b.len() == 1),
        format!(
            "{}；差额 {:?}（要求空——只有右键被改，其余三枚一动不动）；说出来的托盘话 {} 句 \
             {notes_b:?}（要求 1 句、同时点名规则与 falls back）；其余 notes {:?}",
            pair(&b.config.tray, &B.want),
            off(&b.config.tray, &B.want),
            notes_b.len(),
            b.notes()
                .into_iter()
                .filter(|w| !w.contains("tray"))
                .collect::<Vec<_>>()
        ),
    );

    // ---- 上面 C 那一段写的那句先后：两句归一在、规则那句**缺席**。
    let notes_c: Vec<String> = c
        .notes()
        .into_iter()
        .filter(|w| w.contains("tray"))
        .collect();
    let normalized_c = notes_c
        .iter()
        .filter(|w| w.contains("is not a tray action"))
        .count();
    let rule_spoke_c = notes_c
        .iter()
        .filter(|w| w.contains("opens the tray menu"))
        .count();
    rep.row(
        "tray normalizes first",
        Check::from(off(&c.config.tray, &C.want).is_empty() && normalized_c == 2 && rule_spoke_c == 0),
        format!(
            "{}；差额 {:?}（要求空：middle 落回本键默认 none、right 落回 menu，两枚各回各的，不是一\
             枚通用值）；归一说了 {normalized_c} 句、规则说了 {rule_spoke_c} 句（要求 2 与 0——规则那\
             一句在这里是**不该出现**的那一句：先判规则它会看见 `menuu` 而当场出手，两种顺序都修得出\
             同样的值，只有它的缺席分得出先后）；notes {notes_c:?}",
            pair(&c.config.tray, &C.want),
            off(&c.config.tray, &C.want)
        ),
    );

    // ---- 这一格发出来是为了让这一档的天花板出现在报告里，而不是只出现在源码注释里。
    rep.row(
        "tray click reaches",
        Check::Blocked,
        "这一档一次也没有往消息窗投 `WM_LBUTTONUP` / `WM_LBUTTONDBLCLK` / `WM_MBUTTONUP` / \
         `WM_RBUTTONUP`，所以它说不出真点下去时走的是哪一臂：上面几格量的是「这份档 + 这张表 = \
         哪个归宿」，不是「那一次点击真的到了泵」。已知的另一条代价记在这里：托盘结构体是 shell \
         Version 0（见 `platform-windows/hotkey.rs` 那句注释），双击会同时送来一次 UP，所以左键与\
         双击配成不同动作时，双击会先跑一次左键那一格"
            .to_string(),
    );

    // ---- 读数：这一趟进程自己那份档里那四枚此刻是什么。上面几格判的全是 fixture，用户档的
    // 值一次也没被判过；它印在这里，是为了让跑级的人看得出「这一台机器上现在配的是什么」，而
    // 不是让这一行替任何人作证。
    let live = crate::settings::tray_cfg();
    rep.note(
        "tray live",
        format!(
            "{}；进程自己那份档读回来 {}（本档不判它）",
            crate::settings::with(|s| s.line()),
            with_arms(&live)
        ),
    );
    rep.note(
        "tray labels",
        TRAY_ACTIONS
            .iter()
            .map(|action| format!("{action}={}", Tray::label(action)))
            .collect::<Vec<_>>()
            .join(" "),
    );
    rep.finish("tray door")
}
