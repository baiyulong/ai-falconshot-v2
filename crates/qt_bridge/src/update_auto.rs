//! 本项目自己加的那两枚 `[about]` 键的读者：`auto_check_updates` 说的是「启动那一趟要不要自己起
//! 一次检查」，`check_every_days` 说的是「起过之后隔几天再起」。PRD §5.21.1 那四步写的是**用户按**
//! 的那一次（第 2 步「点击检查更新」），这一支不在那四步里 —— 它是档里早已写着、设置页早已画出来
//! 的两枚开关，此前一次也没有人读过：读不到的键与读得到而什么都不做的键，在用户手里是同一枚坏。
//!
//! 判决不在这里 —— [`falcon_core::update::auto_plan`] 是「这一趟起不起」与「那句为什么」唯一的
//! 作者（#281）。本层只把它要的第三枚读数（**上一次自动检查距今天数**）从盘上取来，并把起出去的
//! 那一只手交给 `Session::start_update_check` —— 页上那枚钮、托盘那一项与这一支自动，三条入口
//! 共用同一枚闸、同一条工作线程，彼此的差别只剩递进去的 tag 与「落地时点不点那枚戳」两格参数。
//!
//! 盘上那一枚戳只写**一天里的第几天**（自纪元起的天数），不写时刻：`check_every_days` 的量纲是
//! 「天」，而写时刻的那一份要顺手回答「同一趟启动里点了两次算不算两次」这种这一版压根不管的事。
//! 落点先问 [`ENV_DIR`]、再问 `advanced.data_dir`，与 `capture.rs` / `guide.rs` 同一条规矩 ——
//! 于是门可以把落点换进自己那一枚目录，「这一趟改了用户的档」连结构上都不成立（§9.1 第十五条）。
//!
//! 这一层不联网、不起线程、不开窗：它答的是「这一趟起不起」与「这一趟到底起了没有」。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 换落点用的环境变量，与 `FALCONSHOT_GUIDE_DIR` / `FALCONSHOT_REGION_DIR` 同一族：给了非空就用
/// 它，否则用 `advanced.data_dir` 答的那一枚目录。`--update-check` 那一档靠它把戳写进自己的目录，
/// 而那一档里 `attempts()` 必须是 0 —— 那一句的证人不是注释，是这一枚计数器。
pub const ENV_DIR: &str = "FALCONSHOT_UPDATE_STAMP_DIR";

/// 那份文件的名字。整本一行一个整数；读不出整数与没有文件是同一个读数（**没有记录**），那一支
/// 与「从没检查过」走 core 那一条同一个分支：起跑一次，比替用户按住一整族更新少坏一些。
///
/// `pub` 是给 `--update-check` 那一档的：它要往这一枚名字里写「读不出的一行」与「未来的那一天」，
/// 而自己另抄一份文件名的话，门量的就是另一份文件（#281）。
pub const STAMP_FILE: &str = "last_auto_check.txt";

/// 这一支递进 `start_update_check` 的那枚 tag。对账那几行因此读得出这一趟不是用户按的 ——
/// 「页」与「托盘」两枚说的是手指，「自动」这一枚说的是这台机器刚开起来。
pub const AUTO_TAG: &str = "自动";

/// 一枚天数的字面写法。跨实现可读优先于紧凑：这一份文件坏了的代价是一次多余的网络请求，
/// 而「看不懂盘上那一行」的代价是一次没人知道的静默。
const SECONDS_PER_DAY: u64 = 86_400;

/// 这一趟的落点目录。`--update-check` 把它的 fixture 目录递给这里，产品那一趟走的是数据目录。
///
/// 那一条决定住在 [`dir_in`] 里，这里只负责去问两位主人 —— 单测碰得到前者，碰不到 env
/// （`cargo test` 是多线程的，改进程环境是另一码事）。与 `capture.rs` / `guide.rs` 同一枚拆法。
pub fn dir() -> PathBuf {
    let data = crate::settings::with(|s| falcon_core::config::data_dir(&s.config));
    dir_in(std::env::var_os(ENV_DIR).as_deref(), &data)
}

/// 环境变量给了非空就用它，否则用 `advanced.data_dir` 答的那一枚目录。拆成纯函数是为了让
/// 「门把落点换到自己那一枚目录里」这一件 §9.1 第十五条要求的事，有一次不带盘的证人。
fn dir_in(env: Option<&std::ffi::OsStr>, data: &Path) -> PathBuf {
    match env {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => data.to_path_buf(),
    }
}

fn path_in(dir: &Path) -> PathBuf {
    dir.join(STAMP_FILE)
}

/// 今天自纪元起第几天。读不出（这台机器的钟早于纪元）交回 `None`。
///
/// 只到天、不到秒：`check_every_days` 说的是天，把时刻算进来就是让「同一天里第二次启动」读成
/// 「还不够数」，而那一句产品的话会是「这一趟跳过」——用户看到的是重启两次也查不了一次更新。
pub fn today() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs() / SECONDS_PER_DAY)
}

/// 盘上那一枚天数。**只读，一次也不写**；文件不在、读不开、写的不是整数，交回的都是 `None`。
fn stamped_day(dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(path_in(dir)).ok()?;
    text.trim().parse::<u64>().ok()
}

/// 「上一次自动检查距今天数」。四件事交回的都是 `None`：盘上没记录、那行不是数、钟读不出、
/// 戳上的那一天比今天还晚（用户把钟往回拨过，或者上一趟写的是别的机器的天）。
///
/// 前三件与「从没检查过」同一读数已经写在 core 那一层的注释里；第四件在这里判而不是交给减法
/// panic 或 `saturating_sub` 成 0：那两种写法都会把「这台机器的钟不可信」读成「今天刚查过」，
/// 于是**往后每一趟**都不再自己查 —— 静默一整族更新正是这一支最坏的坏。
pub fn last_days(dir: &Path, now: Option<u64>) -> Option<u32> {
    let now = now?;
    let then = stamped_day(dir)?;
    if then > now {
        return None;
    }
    Some(u32::try_from(now - then).unwrap_or(u32::MAX))
}

/// 这一趟起不起，连同那句说得出为什么的话。三枚读数各归一位作者：档里那两枚键由调用方递进来
/// （[`crate::session::about_auto`] 是唯一的现问处），盘上那一枚戳与今天的天数问这里，
/// **起不起与那句为什么**问 core。
pub fn plan(
    auto: bool,
    every_days: u32,
    dir: &Path,
    now: Option<u64>,
) -> falcon_core::update::Auto {
    falcon_core::update::auto_plan(auto, every_days, last_days(dir, now))
}

/// 本进程往那枚戳伸过几回手（每一次**尝试**算一回，没写成也算）。`--update-check` 那一档问的
/// 必须是 0：那一趟压根不经过 [`startup`]（它不是 `plain`），计数器把「不经过」变成读数。
static ATTEMPTS: AtomicU32 = AtomicU32::new(0);

pub fn attempts() -> u32 {
    ATTEMPTS.load(Ordering::SeqCst)
}

/// 落那一枚戳：今天第几天。先写同名 `.tmp` 再改名，与 `guide.rs` 同一枚理由 —— 一份读到一半被
/// 截断的戳会让下一次「够不够数」答成半句，而那半句要么多问一次网络、要么少问一整族。
///
/// 交回的是**落点与那一枚数**（成功），或者为什么没落（失败）。这一趟检查已经做完了，所以点戳
/// 失败不改成判决、只把「下一次启动会再问一遍」这件事说出来：那是那一次启动唯一听得见的信息。
pub fn mark(dir: &Path, now: Option<u64>) -> Result<String, String> {
    let Some(day) = now else {
        return Err("这台机器的钟读不出天数，这一趟没有点戳".to_string());
    };
    let target = path_in(dir);
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    let name = target
        .file_name()
        .ok_or_else(|| "这一枚路径连文件名都没有".to_string())?;
    let tmp = target.with_file_name(format!("{}.tmp", name.to_string_lossy()));
    std::fs::write(&tmp, format!("{day}\n")).map_err(|e| format!("写 {tmp:?}：{e}"))?;
    std::fs::rename(&tmp, &target).map_err(|e| format!("改名到 {target:?}：{e}"))?;
    Ok(format!("{} 写下第 {day} 天", target.display()))
}

/// 那枚戳归谁点：**只有** tag 是 [`AUTO_TAG`] 的那一支。
///
/// 这一格不做成 `start_update_check` 的布尔参数，而是从**已经存在的那枚 tag** 里读出来，因为
/// 「哪一只入口按下去的」与「这一趟要不要按掉往后 `check_every_days` 天」本来就是同一件事的两面：
/// 参数是第二份说法，就会有一天出现「页，但请按掉后面七天」那种谁也没同意过的组合（#281）。
pub fn stamped_by(tag: &str) -> bool {
    tag == AUTO_TAG
}

/// 这一趟落地到底点不点那枚戳：两位各答一半，合起来才是那句规矩。
///
/// 一半是入口（[`stamped_by`]）：手动那两支不点，用户按一次「检查更新」不该顺手把自动那一支按掉。
/// 另一半是结果：**真去取了而没取回来**（[`falcon_core::update::Answer::FetchFailed`]）那一支不点
/// —— 那一句说的是这台机器此刻到不了那一句地址，而下一次启动它可能就是通的。与之相对，清单取回
/// 来了却读不出（`Unreadable`）、通道配错、甚至压根没配清单，这些都是**问完了**的形状，答的是
/// 什么都不再问也不会变，所以它们点戳：不点的那一版会让一台配错了 `release_feed` 的机器在每次
/// 启动时都发一次注定白发的请求。
pub fn should_stamp(tag: &str, answer: &falcon_core::update::Answer) -> bool {
    stamped_by(tag) && !matches!(answer, falcon_core::update::Answer::FetchFailed { .. })
}

/// 启动那一趟的那一只手，`main.rs` 只在产品本体那一趟（`plain`）叫它。
///
/// 交回一句**这一趟做了什么**的话：跳过那一句是 core 的 `why` 原话，起出去那一句带上 core 的
/// `why` 与落在哪一枚路径，起不动那一句说的是哪一格断的（句柄还没到手 / Qt 拒了这次排队）。
/// 三种坏各说各的（#453）：把「这一趟没起」说成「已经是最新」，用户等的是一句永不会来的话。
///
/// 一次也不写档、也不联网：联网在那条工作线程上，写戳在它落地那一刻，两处各有自己的证人。
pub fn startup() -> String {
    let (auto, every_days) = crate::session::about_auto();
    let here = dir();
    let now = today();
    let plan = plan(auto, every_days, &here, now);
    crate::hotkey::note(format!(
        "启动自动检查：档里 auto_check_updates={auto} check_every_days={every_days}，戳={} \
         距今天数={:?} → {}",
        path_in(&here).display(),
        last_days(&here, now),
        plan.why
    ));
    if !plan.wanted {
        return plan.why.to_string();
    }
    match crate::hotkey::post_to_gui(move |mut session| {
        session.as_mut().start_update_check(AUTO_TAG.to_string());
    }) {
        crate::hotkey::Posted::In => format!("{}；这一趟的检查已排进 GUI 线程", plan.why),
        crate::hotkey::Posted::NoHand => format!(
            "{}，但 GUI 线程句柄还没到手（QML 的 `handThread` 还没跑），这一趟没起，也没有点戳",
            plan.why
        ),
        crate::hotkey::Posted::Refused(why) => format!(
            "{}，但这次排不进 Qt 的事件环（{why}），这一趟没起，也没有点戳",
            plan.why
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本档那枚目录：`temp_dir()` 下一个跨趟的名字，与 `update_check::door_dir()` 同一形状。
    /// 顺手把上一趟留下的那枚戳删掉 —— 这一族的 fixture 是**跨趟**的，不删的话上一趟写下的
    /// 「第 20000 天」会让下一趟的第一枚读数（「还没有记录」）读成「有记录」，红在一格与产品
    /// 毫无关系的地方。
    fn scratch(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-auto-stamp-{}", tag));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(path_in(&dir));
        dir
    }

    /// 戳的往返：写下第 N 天，之后第 M 天读出来的是 M-N。这一格钉的是「点戳」与「读戳」说的是
    /// 同一枚数 —— 一边写时刻一边读天数，够数那一支会永远不够。
    #[test]
    fn a_stamp_round_trips_through_days() {
        let dir = scratch("round");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(last_days(&dir, Some(20_001)), None);
        mark(&dir, Some(20_000)).unwrap();
        assert_eq!(last_days(&dir, Some(20_000)), Some(0));
        assert_eq!(last_days(&dir, Some(20_007)), Some(7));
        // 往回拨的钟：戳上那一天还在「未来」，这一格读成「没有记录」而不是负数或 0。
        assert_eq!(last_days(&dir, Some(19_999)), None);
    }

    /// 坏了的那一行与没有那一行是同一个读数。写 `saturating_sub` 的人会把「读不出」当成 0，
    /// 于是每一次启动都跳过；写 panic 的人会让产品在开机那一刻倒下。两支都要在这一格里挡住。
    #[test]
    fn an_unreadable_stamp_is_the_same_reading_as_no_stamp() {
        let dir = scratch("broken");
        std::fs::create_dir_all(&dir).unwrap();
        for junk in ["", "   \n", "not-a-day", "20000.5", "-3"] {
            std::fs::write(path_in(&dir), junk).unwrap();
            assert_eq!(last_days(&dir, Some(20_005)), None, "{junk:?} 被读成了天数");
        }
    }

    /// 判决的四支在这一层各走一次（core 那一层另有它自己那四支）：这里要的是**盘上那一枚戳**
    /// 与档里那两枚键对上号以后交回什么，而不是 core 的分支表。
    #[test]
    fn the_plan_follows_the_stamp_and_the_two_keys() {
        let dir = scratch("plan");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::remove_file(path_in(&dir)).ok();
        assert!(!plan(false, 7, &dir, Some(20_000)).wanted);
        assert!(plan(true, 7, &dir, Some(20_000)).wanted);
        mark(&dir, Some(20_000)).unwrap();
        assert!(!plan(true, 7, &dir, Some(20_003)).wanted);
        assert!(plan(true, 7, &dir, Some(20_007)).wanted);
        // 钟读不出：与「没有记录」同一支，起跑。
        assert!(plan(true, 7, &dir, None).wanted);
    }

    /// 点不点戳的两半各判一次：入口那一半只认 [`AUTO_TAG`]（页、托盘各是手指按的，不许替用户
    /// 按掉后面几天）；结果那一半只放过「真去取了而没取回来」。这一格是 `should_stamp` 唯一的
    /// 直接证人 —— 门里那一档量的是它对着**真判决**的读数，这里量的是它对每一枚变体的形状。
    #[test]
    fn only_the_auto_arm_consumes_the_interval_and_only_after_a_finished_ask() {
        use falcon_core::update::Answer;
        assert!(stamped_by(AUTO_TAG));
        assert!(!stamped_by("页"));
        assert!(!stamped_by("托盘"));
        assert!(!stamped_by(""));

        let auto = AUTO_TAG.to_string();
        let page = "页".to_string();
        for answer in [
            Answer::NoFeed,
            Answer::Unreadable("这里没有那份文件".to_string()),
            Answer::NoChannel("beta".to_string()),
            Answer::UpToDate {
                current: "1.0.0".into(),
                listed: "1.0.0".into(),
            },
            Answer::Available {
                current: "1.0.0".into(),
                release: Box::new(falcon_core::update::Release {
                    version: "9.9.9".into(),
                    notes: "测试用的那一枚".into(),
                    url: "https://example.invalid/x".into(),
                }),
            },
        ] {
            assert!(
                should_stamp(&auto, &answer),
                "自动那一趟跑完了却没点戳：{answer:?}"
            );
            assert!(!should_stamp(&page, &answer), "手动那一趟点了戳");
        }
        // 唯一不点的那一支。
        let gone = Answer::FetchFailed {
            feed: "https://example.invalid/x".into(),
            why: "那边没答".into(),
        };
        assert!(!should_stamp(&auto, &gone));
    }

    /// 落点那一条决定的两支：env 给了非空就用它，没给、或给的是空串，都回到数据目录。这一格是
    /// 「门能把戳换进自己那枚目录里」这句话唯一的、不带盘的证人（§9.1 第十五条），也是
    /// `capture.rs` / `guide.rs` 同一枚拆法的同一个理由：`dir()` 自己碰不到 env，因为改进程环境
    /// 在多线程的单测里是另一码事。
    #[test]
    fn the_landing_site_follows_the_env_override_and_only_a_real_one() {
        use std::ffi::OsStr;
        let data = PathBuf::from("C:\\data");
        assert_eq!(dir_in(None, &data), data);
        assert_eq!(
            dir_in(Some(OsStr::new("")), &data),
            data,
            "空串那一支必须回到数据目录，而不是落到根上"
        );
        assert_eq!(
            dir_in(Some(OsStr::new("D:\\door")), &data),
            PathBuf::from("D:\\door"),
            "门递了目录而落点还在数据目录，那一趟写的就是用户的文件"
        );
    }

    /// `today()` 用的是**这台机器的钟**，与它自己算出来的那一枚数必须是同一天。这一格挡的是
    /// 「除以 3600 忘了除以 24」那一型：它不会让任何一支判决变红，只会让每一趟启动都以为
    /// 隔了很久（或从未隔过），而那是没有屏幕、没有日志形状可看的坏。
    #[test]
    fn today_counts_days_not_hours() {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / SECONDS_PER_DAY;
        assert_eq!(today(), Some(wall));
    }

    /// 点戳失败要说清楚是哪儿失败的，且**不**改这一趟已经落地的判决。这里的 fixture 是把「目录」
    /// 那一格占成一份**文件**：`path_in` 于是变成 `文件/last_auto_check.txt`，那一趟写在第一步
    /// 就断（父路径不是一枚目录），而断的那一句带着它要去的那枚文件名。
    ///
    /// 不拿一枚不存在的盘符做 fixture：那一格会在有的机器上绿、在别的机器上红，而红的这一格
    /// 说不出产品坏在哪。
    #[test]
    fn a_stamp_that_cannot_be_written_says_where() {
        let mut held = std::env::temp_dir();
        held.push("falconshot-auto-stamp-blocked");
        std::fs::remove_dir(&held).ok();
        std::fs::write(&held, b"this path is a file, not a directory").unwrap();
        let why = mark(&held, Some(20_000)).unwrap_err();
        assert!(
            why.contains("last_auto_check.txt"),
            "那一句没说出它要去哪一枚路径：{why}"
        );
    }
}
