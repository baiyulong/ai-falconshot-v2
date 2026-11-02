//! §5.5.8 刷新截图 的那一扇门：`--selftest --refresh`。
//!
//! 它判的是七件不需要人坐在对面的事，逐条对着 PRD 那四步与两条规则：
//!
//! | 本档那一格 | 需求里的那一句 |
//! | --- | --- |
//! | `screen pin carries it` | 步骤 3「按**原截图坐标**」—— 这一枚贴图到底带不带那块坐标 |
//! | `overwrite reads now` | 步骤 3+4「重新读取屏幕内容」「用新内容更新结果」（覆盖那一臂） |
//! | `new copy leaves it` | 步骤 4 的另一臂 + 规则 1「或生成新图」，以及「原图」那一枚不许被顺手动到 |
//! | `gone region asks` / `half region asks` | 规则 2「区域失效时应提示重新选择」 |
//! | `real layout back` | 那一手拒绝是**那块桌面**的答复，不是贴图自己记下的一件事 |
//! | `file pin has no row` | 菜单那两行的**可见**与按下后的**拒绝**读的是同一格（#494） |
//!
//! ## 「新」字怎么问得出对面
//!
//! 桌面在本档手里一次也没有变过，所以「读回来的那颗像素与一秒钟前那颗不同」这一问在这一台
//! 机器上问不出来 —— 而它不是 §5.5.8 要的那件事。需求要的是「贴图现在显示的是屏幕上此刻的
//! 内容」，于是本档判的是那条**关系**：把覆盖之后的那一张与**另一次独立重读**逐颗比（48×32
//! 全比，不抽样 —— 抽样只能答"采到的几颗一样"），并要求纹理号上跳（`revision` 只在真推出一张
//! 新纹理时才加一，把旧像素原地扣住不动就会停在那一格）。这与 `--capture` 那一族从 P1 起用的
//! 分工同一枚：判关系，不判绝对像素。
//!
//! ## 区域失效这一臂怎么在这一台机器上造得出来
//!
//! 拔不掉显示器，也不必拔：§8.5 的门早就把「换一张显示器清单」做成了产品自己的一只手
//! （[`crate::state::PinState::refit_onto`] 收的就是那份新名单，而它真会改 `desk`），本档借它
//! 递上两块**装不下那一块原坐标**的桌面 —— 一块完全在外面，一块只盖得住半张。于是规则 2 那两
//! 句（「请重新选择」与「不许悄悄裁」）读的是产品自己的拒绝，而不是本档替它写的一句。
//!
//! ## 这一档不声称的那件事
//!
//! 它一次也没有开出一扇真窗，所以菜单上那两行**画在哪儿、按下去是不是真的走到 `refreshPin`**
//! 不在这里判 —— 那一半的对面是 `PinWindow.qml` 的那两枚 `MenuItem` 与 `view.refreshable` 那
//! 一格的绑定。本档判的是它们背后那一个条件成不成立，以及那一只手交回来的话。

use std::path::PathBuf;

use falcon_core::encode::{self, EncodeOptions, Format};
use falcon_core::frame::Frame;
use falcon_core::geometry::{PhysPoint, PhysRect, Scale};
use falcon_core::pin::{Desktop, Monitor, PinId};

use crate::r13::Report;
use crate::state::{self, Check};

/// 本档自己的目录：`file pin has no row` 那一份 PNG fixture 落在这里。同一台机器上两趟并行时
/// 各自的 fixture 必须是各自的文件（与 `whiteboard_check::door_dir` 同一枚理由、同一枚形状）。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_REFRESH_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-refresh-door");
    dir
}

/// 两张图是不是同一张：逐颗比，报出前几枚不相等的坐标。
///
/// 全比：本档的贴图是 48×32 = 1 536 颗，逐颗问一遍比抽样便宜，而抽样的那一句只能答"采到的
/// 几颗一样" —— 那一半恰好放不进「用新内容更新结果」。
fn same_picture(pin: &Frame, other: &Frame, limit: usize) -> (String, bool) {
    if pin.width != other.width || pin.height != other.height {
        return (
            format!(
                "尺寸就不是同一张：{}x{} vs {}x{}",
                pin.width, pin.height, other.width, other.height
            ),
            false,
        );
    }
    let mut bad = 0;
    let mut spots: Vec<String> = Vec::new();
    for y in 0..pin.height {
        for x in 0..pin.width {
            let (a, b) = (pin.get(x, y), other.get(x, y));
            if a != b {
                bad += 1;
                if spots.len() < limit {
                    spots.push(format!("({x},{y}) 贴图 {a:?} / 另一张 {b:?}"));
                }
            }
        }
    }
    if bad == 0 {
        (format!("{}x{} 逐颗相同", pin.width, pin.height), true)
    } else {
        (
            format!("{bad} 颗不同，前 {} 枚 {spots:?}", spots.len()),
            false,
        )
    }
}

/// 一块桌面说成一句：它自己那一格 `bounds` —— 本档要拿它对着原坐标比，而那句话得让读数的人
/// 一眼看出这块桌面装不装得下 `want`。
fn desk_line(desk: &Desktop) -> String {
    let b = desk.bounds;
    format!(
        "{}x{}@{},{}（{} 屏）",
        b.w,
        b.h,
        b.x,
        b.y,
        desk.monitors.len()
    )
}

/// §5.5.8 的门。开头那一行 `desktop scene` 是每扇门共同的上下文（[`crate::r13::scene_row`]），
/// 它之后是上表那七格判定，最后一格收梢；屏幕冻不下来时提前收，那时印出的只有上下文与那一格
/// BLOCKED。
pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    // 每一格都要真读一次屏幕，所以冻不下画面时后面不必假装跑过 —— 与 `--capture` 同一枚形状：
    // 那是这台机器不肯被看，不是一条测失败的测试。
    let frozen = crate::capture::freeze();
    let snap = match frozen.as_ref() {
        Ok(f) => f.snap.clone(),
        Err(e) => {
            rep.row(
                "refresh needs a screen",
                Check::Blocked,
                format!("冻不下画面：{e} —— 本档每一格都要真读一次屏幕"),
            );
            return rep.finish("refresh door");
        }
    };
    let Some(primary) = snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .or_else(|| snap.monitors.first())
    else {
        rep.row(
            "refresh needs a screen",
            Check::Blocked,
            "屏幕上没有一块可钉的：freeze 交回 0 屏".to_string(),
        );
        return rep.finish("refresh door");
    };
    let pb = primary.info.bounds;
    let want = PhysRect::new(pb.x + 40, pb.y + 40, 48, 32);
    let at = PhysPoint::new(want.x, want.y);
    let scale = Scale::of(primary.info.scale.ratio());

    // ---- 起点：走产品自己的那一条 `--rect` 路钉一块真区域。下面每一格读的都是这一枚贴图。
    // 开工前先把名单抄下来：本档不假设自己跑在一张空桌上（`--selftest --clipboard --capture
    // --history --refresh` 那一串是合法调用，前面每一档留下的贴图都不是本档该关的东西）。
    let start = state::with(|s| s.ids());
    let id: PinId = match crate::capture::pin_rect(&want) {
        Ok((id, _)) => id,
        Err(e) => {
            rep.row(
                "screen pin carries it",
                Check::Fail,
                format!("区域 {want:?} 钉不起来：{e} —— 后面每一格都没有主体"),
            );
            return rep.finish("refresh door");
        }
    };
    let mut mine: Vec<PinId> = vec![id];

    // ---- 步骤 3 的那一枚原坐标：钉起来那一手有没有把它记进贴图自己。`refreshable` 是菜单那
    // 两行的可见性读的那一格，与这里问的是同一枚答案（不是两处各判一遍，#281）。
    let carries = state::with(|s| (s.view_data(id).refreshable, s.captured_of(id)));
    rep.row(
        "screen pin carries it",
        Check::from(carries.0 && carries.1 == Some(want)),
        format!(
            "refreshable={} 原坐标={:?}（要求 true 与 {want:?} —— 差一枚就说明记的是贴图**现在**\
             待着的地方而不是它被截下来的地方，那一手会把刷新送到用户没看的那一块上）",
            carries.0, carries.1
        ),
    );

    // ---- 规则 1 的第一臂：覆盖。答了话、话里写着它去了哪一块、贴图上现在这一张与另一次独立
    // 重读逐颗相同、纹理号上跳 —— 四件一起才算「用新内容更新结果」。
    let rev0 = state::with(|s| s.view_data(id).revision);
    let done = state::with(|s| s.refresh(id, true));
    let said = match &done {
        Ok(line) => line.clone(),
        Err(e) => e.clone(),
    };
    let live = crate::capture::re_read(&want, "本档另读一次");
    let rev1 = state::with(|s| s.view_data(id).revision);
    let picture = match (&done, &live) {
        (Ok(_), Ok((frame, _, _))) => {
            let frame = frame.clone();
            state::with(|s| s.with_rendered(id, |pin| same_picture(pin, &frame, 3)))
        }
        _ => None,
    };
    rep.row(
        "overwrite reads now",
        Check::from(done.is_ok() && picture.as_ref().is_some_and(|(_, ok)| *ok) && rev1 > rev0),
        format!(
            "{said}；纹理号 {rev0} -> {rev1}（要求上跳：原地扣住旧的那一张就只会停在 {rev0}，\
             「用新内容更新结果」那句就成了没说出来的话）；与另一次独立重读比：{}",
            picture
                .as_ref()
                .map(|(line, _)| line.clone())
                .unwrap_or_else(|| "两边有一边没答，见上面那句".to_string())
        ),
    );

    // ---- 规则 1 的第二臂：另钉一张。它多出一枚贴图，而**原来那一枚一个字节也不许被动到** ——
    // 那句「或生成新图」的另一半正是"我这张还在"，所以本格两半都判。
    let before = state::with(|s| s.ids());
    let copied = state::with(|s| s.refresh(id, false)).ok().and_then(|line| {
        let after = state::with(|s| s.ids());
        after
            .iter()
            .find(|x| !before.contains(x))
            .copied()
            .map(|n| (n, line))
    });
    if let Some((n, _)) = &copied {
        mine.push(*n);
    }
    let rev_copy = state::with(|s| s.view_data(id).revision);
    let compare = match (&copied, &live) {
        (Some((n, _)), Ok((frame, _, _))) => {
            let frame = frame.clone();
            Some(state::with(|s| {
                (
                    s.with_rendered(*n, |pin| same_picture(pin, &frame, 3)),
                    s.with_rendered(id, |pin| same_picture(pin, &frame, 3)),
                    s.view_data(*n).refreshable,
                )
            }))
        }
        _ => None,
    };
    let both_same_and_flagged = compare.as_ref().is_some_and(|(n, o, shows)| {
        n.as_ref().is_some_and(|(_, ok)| *ok) && o.as_ref().is_some_and(|(_, ok)| *ok) && *shows
    });
    rep.row(
        "new copy leaves it",
        Check::from(
            copied.is_some()
                && state::with(|s| s.ids().len()) == before.len() + 1
                && rev_copy == rev1
                && both_same_and_flagged,
        ),
        format!(
            "{}名单 {} 枚 -> {} 枚，原贴图纹理号留在 {rev1}（要求不动）；新那一张 {}；原那一张 {}；\
             新那枚自己带不带原坐标={:?}（要求两张都逐颗相同、那一枚也 Some(true) —— 一张从屏幕上\
             读回来的图，钉出来就该能再刷一次）",
            copied
                .as_ref()
                .map(|(_, line)| format!("{line}；"))
                .unwrap_or_else(|| "另钉一张这一手没答上话；".to_string()),
            before.len(),
            state::with(|s| s.ids().len()),
            compare
                .as_ref()
                .and_then(|(n, _, _)| n.clone())
                .map(|(line, _)| line)
                .unwrap_or_else(|| "没有第二枚可比".to_string()),
            compare
                .as_ref()
                .and_then(|(_, o, _)| o.clone())
                .map(|(line, _)| line)
                .unwrap_or_else(|| "原那一枚没读到".to_string()),
            // 这一枚读成 `Option` 而不是替它答 `false`：没有第二枚时那是一格**没读到**，把它写成
            // 「读到了假」是一句读数的假话（#453）。上面两处同理由，各自印"没读到"那句话。
            compare.as_ref().map(|(_, _, shows)| *shows),
        ),
    );

    // ---- 规则 2 的两臂：整块不在任何屏幕上、以及只剩一块。两者都只许**说**，不许悄悄读回一点
    // 别的（读回半张就是把「失效」读成「成功」，那是 #302 放宽判据的那一族）。判"没动"用的是
    // 手势**之前**那一张自己的副本而不是又一次屏幕重读：这里要问的只有"这一手有没有改贴图"，
    // 让屏幕上那 40 毫秒里可能发生的任何变化掺进来，红的就不是这一格。
    let gone = Desktop::from_monitors(vec![Monitor::new(PhysRect::new(pb.x, pb.y, 20, 20), scale)]);
    let half = Desktop::from_monitors(vec![Monitor::new(
        PhysRect::new(want.x, want.y, want.w / 2, want.h),
        scale,
    )]);
    for (label, desk) in [("gone region asks", gone), ("half region asks", half)] {
        let held = state::with(|s| s.with_rendered(id, |pin| pin.clone()));
        let rev_before = state::with(|s| s.view_data(id).revision);
        state::with(|s| s.refit_onto(desk.clone()));
        let refused = state::with(|s| s.refresh(id, true));
        let rev_after = state::with(|s| s.view_data(id).revision);
        let untouched = match (&refused, &held) {
            (Err(_), Some(h)) => state::with(|s| {
                s.with_rendered(id, |pin| same_picture(pin, h, 1).1)
                    .unwrap_or(false)
            }),
            _ => false,
        };
        rep.row(
            label,
            Check::from(
                refused.as_ref().is_err_and(|e| e.contains("请重新选择"))
                    && rev_before == rev_after
                    && untouched,
            ),
            format!(
                "递上 {} 这块桌面（原坐标 {want:?}）-> {}；纹理号 {rev_before} -> {rev_after}\
                 （要求相等），贴图上那一张还是覆盖后那一枚={untouched}",
                desk_line(&desk),
                refused.unwrap_or_else(|e| e.clone())
            ),
        );
    }

    // ---- 那一手拒绝是**那块桌面**的答复，不是贴图自己记下的一件事：把机器自己那份名单经
    // [`crate::state::desktop_changed`] 装回去，同一枚贴图、同一块区域就必须又刷得动了。这一格
    // 是上一格的反证 —— 少了它，「拒绝过一次之后从此再也不刷」也照样能过前面两格。
    let restored = state::desktop_changed();
    let back = state::with(|s| s.refresh(id, true));
    let rev_after2 = state::with(|s| s.view_data(id).revision);
    rep.row(
        "real layout back",
        Check::from(back.is_ok() && rev_after2 > rev1),
        format!(
            "装回真名单（{}）之后再按一次覆盖：{}",
            restored.unwrap_or_else(|| "一句也没说 —— 没有一枚贴图需要被搬回来".to_string()),
            back.map(|line| format!("又刷得动了，纹理号 {rev1} -> {rev_after2}；{line}"))
                .unwrap_or_else(|e| format!("仍然被拒：{e}"))
        ),
    );

    // ---- 菜单那两行的可见性那一臂：从文件来的贴图压根没有原坐标。它同时也是 `refreshable`
    // 为假时那一手**按下**会说的话 —— 两半读的是同一格，所以「看得见按不动」组不出来（#494）。
    let dir = door_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("not-from-screen.png");
    let seed = match Frame::filled(8, 6, [10, 20, 30, 255])
        .map_err(|e| e.to_string())
        .and_then(|f| {
            encode::encode(
                &f,
                &EncodeOptions {
                    format: Format::Png,
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())
        })
        .and_then(|bytes| std::fs::write(&path, &bytes).map_err(|e| e.to_string()))
    {
        Ok(()) => "种下了".to_string(),
        Err(e) => format!("种不下去：{e}"),
    };
    let from_file = state::with(|s| s.add_from_file(&path.to_string_lossy(), at));
    if let Ok(n) = &from_file {
        mine.push(*n);
    }
    let (shows, refusal) = match &from_file {
        Ok(n) => {
            let shows = state::with(|s| s.view_data(*n).refreshable);
            let said = state::with(|s| s.refresh(*n, true)).unwrap_or_else(|e| e);
            (Some(shows), said)
        }
        // 钉不起来时不说 `true`：那一格根本没被读到，而本格早已因为 `from_file.is_ok()` 为假而红 ——
        // 红的原因写在句子前一半，不必替后面两格编一个读数（#453）。
        Err(e) => (None, format!("文件钉不起来，所以没读到：{e}")),
    };
    rep.row(
        "file pin has no row",
        Check::from(
            from_file.is_ok() && shows == Some(false) && refusal.contains("不是从屏幕上截下来的"),
        ),
        format!(
            "{}：{} 那份档钉出来 refreshable={shows:?}（要求 Some(false)），按下那一句：{refusal}",
            path.display(),
            seed
        ),
    );

    // ---- 收梢：本档钉的贴图与那份 fixture 都不留下，名单回到开工时那一枚也不差的样子 ——
    // 判的是"回到本档自己的起点"，不是"归零"：归零会把别人那一档留下的东西也算成本档的债。
    mine.sort_unstable();
    mine.dedup();
    let closed = state::with(|s| s.close(&mine));
    let left = state::with(|s| s.ids());
    let gone_file = match std::fs::remove_file(&path) {
        Ok(()) => "撤掉了".to_string(),
        Err(e) => format!("撤不掉：{e}"),
    };
    rep.row(
        "no residue",
        Check::from(closed == mine.len() && left == start),
        format!(
            "关掉本档钉的 {} 枚，交回 {closed} 枚；名单剩 {} 枚，与开工时那 {} 枚一致={}（fixture {gone_file}）",
            mine.len(),
            left.len(),
            start.len(),
            left == start
        ),
    );

    rep.finish("refresh door")
}
