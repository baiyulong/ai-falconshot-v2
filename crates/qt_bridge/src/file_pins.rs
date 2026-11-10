//! §5.8.3「从图片文件创建贴图」在桥侧的那几只手。
//!
//! 四只手：
//!
//! - [`classify`]：这一趟 argv 是什么 —— 「产品本体那一趟」「被拖到应用图标上的那一批路径」还是
//!   「点名了旗标的那一档」；
//! - [`land`]：把一批路径贴成贴图，并把**屏幕上那一行**交回 —— §5.8.3 落贴图的那**唯一**作者
//!   （托盘菜单那一支与拖到图标那一支都走它，两处各写一份读盘与落窗的规矩，那两份就会各自漂移）；
//! - [`hand_over`]：把那一批路径递到**已经在跑的那一个**手里；
//! - [`serve`]：接住递进来的那一批，把它贴到屏幕上。
//!
//! # 为什么认的是 argv 而不是拖放事件
//!
//! Explorer 把文件拖到 `falconshot.exe`（或它的快捷方式）上时，Windows 起的是**新的一趟进程**，
//! 把被拖的那几枚路径当 argv 递进来 —— 托盘图标那一枚收不到拖放（`Shell_NotifyIcon` 没有放文件
//! 的目标接口，见 PRD §5.8.3 的实现口径注）。所以「拖到应用图标」这一档认的是 argv，不是拖放事件。
//!
//! # 三条路，三种答（#453）
//!
//! 已经有人跑着且接得住 ⇒ 递过去，这一趟进程交完就退（屏幕上不多一枚托盘图标，也不多一扇贴图窗）；
//! 没人跑 ⇒ 这一趟进程**自己**变成那一个（托盘照旧挂上）并把这一批就地贴出来；递不过去 ⇒ 一句
//! 拒绝，退 1。

use std::path::PathBuf;

use cxx_qt_lib::QString;

/// 这一趟 argv 是什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invocation {
    /// 产品本体那一趟：argv 是空的（托盘常驻、无参启动）。
    Resident,
    /// 被拖到应用图标上的那一趟：argv 里每一枚都不是旗标，它们是**被拖进来的那几枚路径**。
    ///
    /// 「真的存在」这一半**不**在这里判：那一枚文件读不读得出来，由
    /// [`falcon_core::pin::read_picture_batch`] 说（§8.2：打不开 / 太大 / 不是本程序会读的格式 /
    /// 解不出来，四句各说各话），它带的那一句里已经有文件名与原因，所以这里不重述它的话（#281）。
    Dropped(Vec<PathBuf>),
    /// 点名了旗标的那一趟：那是量具门、§5.17 的命令或 §5.20.3 的那几条命令 —— 它们自己走各自
    /// 那一路（旗标在 argv 里，这一档不参与）。
    Door,
}

/// 这一趟 argv 是「被拖到应用图标上的那一批」还是「点名了旗标的」。
///
/// 判据只有一条：**每一枚都不是旗标**。
///
/// 旗标那一半是必须的：§5.17 的动词（`--new` 之类）与 §5.20.3 的那几条命令（`--export-config
/// <路径>`）都是 argv 里的旗标 —— 混进旗标时这一档就不是「拖到图标上的那一批」，而是脚本或命令
/// 在说话，那些档自己走 [`crate::cli`] 与 [`crate::settings`]。
///
/// 「旗标」的样子是**双横线**，与这个程序其余各处一致（[`crate::main`] 里那一把
/// `path_after` 用的也是 `starts_with("--")`）：一份叫 `-x.png` 的文件是可以被拖到图标上的，
/// 而按双横线去认它，那一份就仍是「被拖来的那一批」，不会退化成一次什么都不干的空会话。
pub fn classify(args: &[String]) -> Invocation {
    if args.is_empty() {
        return Invocation::Resident;
    }
    let mut dropped = Vec::new();
    for arg in args {
        if arg.is_empty() || arg.starts_with("--") {
            // 旗标在里头 ⇒ 这一趟是「点名了旗标的那一档」。
            return Invocation::Door;
        }
        dropped.push(PathBuf::from(arg));
    }
    Invocation::Dropped(dropped)
}

/// 把那一批路径贴成贴图，并把**屏幕上那一行**交回。
///
/// 这一手是 §5.8.3 落贴图的那**唯一**作者：托盘菜单那一支（[`crate::hotkey`] 里的
/// `open_picture_pins`）与拖到图标那一支都走它，所以「读几枚、坏几枚、超上界几枚、说哪一句」
/// 只有一份。坏的那几枚逐枚落一行运行历史（#433：一句提示到第四句就不像提示、像日志）。
///
/// 只能在 GUI 线程上叫：它动 [`crate::state`] 里的贴图名册与纹理。
pub fn land(
    paths: &[PathBuf],
    mut session: std::pin::Pin<&mut crate::session::qobject::Session>,
    tag: &str,
) -> String {
    let batch = crate::state::pin_pictures(paths);
    // 每一枚没成的都要单独落一行，而这一行说的话**全部来自那一批自己**：`why` 是
    // [`crate::state::read_refusal`]（读坏的）或 [`falcon_core::pin::read_picture_batch`]
    // （超上界的）写好的那一句，两处的句子里都已经带上路径，所以这里递 `why` 而不是再拼一遍
    // 「{path}：{why}」—— 同一句话说两遍就是 #281。
    for miss in batch.failed.iter().chain(batch.declined.iter()) {
        crate::hotkey::note(format!("GUI 线程 {tag}：没有贴出来 {}", miss.why));
    }
    let said = batch.said();
    session.as_mut().set_status(QString::from(&*said));
    if batch.windows() > 0 {
        // 与 `pin_clipboard` 同一条腿：Rust 改完名册要把读数搬过桥，窗才会长出来。一枚也没有
        // 时不叫它 —— `refresh` 会让每一扇已有的窗把自己的那一份重求值一遍，而这一趟什么也没改。
        session.as_mut().refresh();
    }
    crate::hotkey::note(format!(
        "GUI 线程 {tag}：挑了 {} 枚，贴出 {} 扇，读坏 {} 枚，超上界 {} 枚 → {said}",
        paths.len(),
        batch.windows(),
        batch.failed.len(),
        batch.declined.len()
    ));
    said
}

/// 那一批路径递过去了没有。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// 交给了已经在跑的那一个。
    Handed,
    /// 递不过去，且**等过了**：一句拒绝。
    Refused(String),
}

/// 等多少拍再交出拒绝（一拍 250 ms）。四十拍 = 十秒 —— 那是「对方刚起、接收口还没开出来」
/// 那一种坏法的耐心，不是「一直等下去」的许可。
const HANDOVER_TRIES: u32 = 40;

/// 把这一批路径交给已经在跑的那一个（交给产品那一根管道上的那一个）。
pub fn hand_over(paths: &[PathBuf]) -> Delivery {
    hand_over_to(&platform_windows::instance::drop_pipe_name(), paths)
}

/// 把这一批路径交给**那根管道**上的那一个。
///
/// 等的是「对方刚起、接收口还没开出来」那一种：那一档与「没人跑」在产品那边是两句不同的
/// 话（#453），所以等不到才交出拒绝，而等的时候要把这两种坏法分开 —— 机器上那一层
/// （[`platform_windows::instance::handover_to`]）把「没人接」「正忙着」「递不过去」分开答，
/// 这里只管等。
pub fn hand_over_to(pipe: &str, paths: &[PathBuf]) -> Delivery {
    wait_and_hand_over(pipe, paths, HANDOVER_TRIES)
}

/// 等那一拍的那一手（[`hand_over_to`] 与单测共用同一个实现）。
///
/// 拍的数是参数：产品那一档等四十拍，而单测只等一拍 —— 一枚「没人接」的探针要等四十拍才
/// 交出拒绝，那一格单测就得跑十秒，而那十秒里它什么也没证到。
fn wait_and_hand_over(pipe: &str, paths: &[PathBuf], tries: u32) -> Delivery {
    for _ in 0..tries {
        match platform_windows::instance::handover_to(pipe, paths) {
            platform_windows::instance::Handover::Handed => return Delivery::Handed,
            platform_windows::instance::Handover::NoReceiver
            | platform_windows::instance::Handover::Busy(_) => {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            platform_windows::instance::Handover::Failed(why) => return Delivery::Refused(why),
        }
    }
    Delivery::Refused("已经在跑的那一个还没把接收口开出来：这一批路径没能递过去".to_string())
}

/// 接活的那一端：等着别人把路径递进来。
///
/// 递进来的那一批要**贴到屏幕上**，而贴图是 GUI 线程的事 —— 所以这一段走
/// [`crate::hotkey::post_to_gui`]（排进事件环那一手只有它一个作者）。排不进去时那一句说清是
/// 「还没有 GUI 那一头」还是「Qt 拒了这一次排队」，不合成一句「起不来」。
///
/// 而「还没有 GUI 那一头」不是终点：这一枚进程才起、QML 还没把 GUI 线程递过来时，接收口已经在
/// 开着等了 —— 所以这里**等它**（最多 40 拍），而不是把这一批丢在地板上（那一批是用户拖进来的
/// 文件，丢掉它等于用户拖了个寂寞）。
pub fn serve(receiver: platform_windows::instance::Receiver) {
    std::thread::spawn(move || loop {
        match receiver.accept() {
            Ok(Some(paths)) => {
                for _ in 0..40 {
                    let batch = paths.clone();
                    let posted = crate::hotkey::post_to_gui(move |mut session| {
                        land(&batch, session.as_mut(), "拖到应用图标");
                    });
                    match posted {
                        crate::hotkey::Posted::In => break,
                        crate::hotkey::Posted::NoHand => {
                            // GUI 那一头还没到手：等它（这一枚进程才起，QML 还没加载完）。
                            std::thread::sleep(std::time::Duration::from_millis(250));
                        }
                        crate::hotkey::Posted::Refused(why) => {
                            eprintln!("[drop icon] 递进来的那一批没能贴到屏幕上：{why}");
                            break;
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("[drop icon] 接收口出错了：{e}");
                break;
            }
        }
    });
}

/// 把这一批路径贴到屏幕上（GUI 那一头还没到手就等它）。
///
/// 与 [`serve`] 里那一支是同一只手：递进来的那一批与这一枚自己带的那一批，走的是同一条路。
pub fn land_when_ready(paths: Vec<PathBuf>, tag: &str) -> bool {
    for _ in 0..40 {
        let batch = paths.clone();
        let tag = tag.to_string();
        let posted = crate::hotkey::post_to_gui(move |mut session| {
            land(&batch, session.as_mut(), &tag);
        });
        match posted {
            crate::hotkey::Posted::In => return true,
            crate::hotkey::Posted::NoHand => {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            crate::hotkey::Posted::Refused(why) => {
                eprintln!("[drop icon] 这一批没能贴到屏幕上：{why}");
                return false;
            }
        }
    }
    eprintln!("[drop icon] 这一批没能贴到屏幕上：GUI 那一头一直不来");
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 那一趟 argv 的认法：**每一枚都得是「不是旗标」**。
    ///
    /// 四种坏法各不同（混进一枚旗标 / 一枚空的 / 空的 argv / 名字像旗标的路径），而它们合起来是
    /// 同一句话：「这一趟不是拖到图标上的那一批」。**存在与否不在这里判** —— 那一枚读不读得出来
    /// 由 [`falcon_core::pin::read_picture_batch`] 说（§8.2 那四句），它带的那一句里已经有文件名
    /// 与原因，所以这里不重述它的话（#281）。
    #[test]
    fn the_drop_batch_is_only_recognised_when_no_token_is_a_flag() {
        assert_eq!(
            classify(&[r"C:\Users\baiyulong\图片\a b.png".to_string()]),
            Invocation::Dropped(vec![PathBuf::from(r"C:\Users\baiyulong\图片\a b.png")]),
            "一枚不是旗标的东西就是那一批"
        );
        assert_eq!(classify(&[]), Invocation::Resident, "空的 argv 是产品本体那一趟");
        assert_eq!(
            classify(&["--new".to_string(), r"C:\a.png".to_string()]),
            Invocation::Door,
            "混进一枚旗标 ⇒ 这一趟是脚本在说话，不是拖到图标"
        );
        assert_eq!(
            classify(&["--new".to_string()]),
            Invocation::Door,
            "一枚旗标单独在里头也是旗标那一路"
        );
        assert_eq!(
            classify(&[String::new()]),
            Invocation::Door,
            "一枚空的不是一枚文件"
        );
        assert_eq!(
            classify(&["-x.png".to_string()]),
            Invocation::Dropped(vec![PathBuf::from("-x.png")]),
            "一枚单横线的不是旗标：这一整个程序的旗标都是双横线（`path_after` 那一处也是这么认的）"
        );
    }

    /// 「没人接」与「接住了」是**两句不同的话**：这一格钉的是**桥这一层**的判定。
    ///
    /// 没人接 ⇒ 等过之后交出「还没把接收口开出来」那一句（**不是**机器上那一层的「写不过去」
    /// —— 那是另一个作者、另一件事，#281）；有人接 ⇒ 答「递过去了」。
    ///
    /// 字节那一格不在这里证（那是 `platform_windows::instance` 那一层的作者），这里只钉判定与
    /// 枚数 —— 同一句判据写两遍，总有一天两遍对新过一遍。
    #[cfg(windows)]
    #[test]
    fn the_handover_answers_kept_and_missing_separately() {
        // 没人接：那一根管道压根不存在。
        let missing = format!(
            r"\\.\pipe\ai-falconshot-drop-proof-missing-{}",
            std::process::id()
        );
        match wait_and_hand_over(&missing, &[PathBuf::from(r"C:\a.png")], 1) {
            Delivery::Refused(why) => {
                assert!(why.contains("还没把接收口开出来"), "{why}");
                assert!(
                    !why.contains("写不过去"),
                    "那是机器上那一层的另一句，不许在这里冒出来：{why}"
                );
            }
            other => panic!("没人接却答了别的：{other:?}"),
        }

        // 有人接：答「递过去了」，而且那一批是**两枚**。
        let pipe = format!(r"\\.\pipe\ai-falconshot-drop-proof-{}", std::process::id());
        let receiver = platform_windows::instance::Receiver::bind_named(&pipe).expect("管道造得起来");
        let got = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let sink = got.clone();
        let reader = std::thread::spawn(move || {
            let arrived = receiver.accept().expect("接得住");
            *sink.lock().unwrap() = arrived.map(|a| a.len()).unwrap_or(0);
        });
        match wait_and_hand_over(
            &pipe,
            &[PathBuf::from(r"C:\a.png"), PathBuf::from(r"C:\b.png")],
            3,
        ) {
            Delivery::Handed => {}
            other => panic!("有人接却答了别的：{other:?}"),
        }
        reader.join().expect("接收那一头不炸");
        assert_eq!(*got.lock().unwrap(), 2, "递过去的是两枚");
    }
}
