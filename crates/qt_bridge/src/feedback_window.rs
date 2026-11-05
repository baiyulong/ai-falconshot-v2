//! §5.21.3「问题反馈」在产品里的那一块：页上那枚钮、它点出来的三步、以及两条隐私规则的落点。
//!
//! ## 这一档为什么存在
//!
//! [`falcon_core::feedback`] 那一腿早就齐了 —— 名单、脱敏、两把闸、单测都有。缺的是**它的对面**：
//! `about.send_diagnostics` 与 `about.feedback_channel` 两枚持久键在全仓没有一位独立读者，而
//! `feedback::payload` 唯一的调用处是 [`crate::state::selftest`]（一格判据，屏幕上没有人能走到）。
//! 这就是本仓反复出现的那一族缺陷：逻辑齐备、生产路径零调用。这一档把那条路修通。
//!
//! ## 四步各自的作者（谁都不在这里抄第二份）
//!
//! * 第 1 步「用户点击『反馈问题』」—— 那一枚钮长在**「关于与更新」那一页**（
//!   [`crate::settings_view::ABOUT_PAGE`]），字由 [`crate::settings_view::feedback_words`] 递给页，
//!   页上一枚字也不自己写。它不在托盘名册里，理由见 [`WORD`] 头顶那段（#494 的反面：不替
//!   §5.18.1 那张建议名单添一名）。
//! * 第 2 步「系统生成可选的诊断信息」—— [`diagnostics`] 就是那一双手：此刻现读的几行机器字。
//! * 第 3 步「填写问题描述并决定是否附加日志」—— 页上那一格填的字 + [`LOGS_LABEL`] 那一枚勾选框；
//!   「决定」是真的有后果：勾与不勾改变的是**出去的那一段字**（[`falcon_core::feedback::Consent::logs`]）。
//! * 第 4 步「跳转到反馈渠道或发送反馈」—— 一次按下把两半都办了：包写成文本进剪贴板（[`falcon_core::feedback::Payload::text`]
//!   是「包写成文本」唯一作者），渠道走 [`crate::session::open_web`] 那一道闸（`about.feedback_channel`
//!   头顶那句「读者是 open_web」自此有了对面的对面）。两半各说各的话，一半不成不冒充整趟不成（#453）。
//!
//! ## 两条隐私规则在这里的形状
//!
//! 一「未经用户确认不得上传截图内容」：[`falcon_core::feedback::Draft`] 一个字节字段也没有，
//! 所以一张图**没法**被夹带进来；而「确认」这一格在页上是第 4 步那枚钮本身 —— 在它之前屏幕上是
//! **预览**（[`PREVIEW_LABEL`]），预览与发送走的是同一个函数、同一份现读，不是两份写法。
//! 二「日志中应移除密码、令牌和其他敏感信息」：机器那两半每一行都过
//! [`falcon_core::feedback::redact_line`]，而脱敏只在 `payload` 那一处发生 —— 这一档不第二遍过刀，
//! 因为「谁被藏了、藏了几枚」只许有一个答案（#281）。
//!
//! ## 本档不声称的事
//!
//! 见 [`measure`] 末尾那三格 **Blocked**：页上那一枚钮有没有真的被手指点过、剪贴板有没有真的被
//! 写过、渠道有没有真的在浏览器里开起来 —— 这三条都要跑的泵与用户的手，这一档（打完就退、不开
//! 遮罩）一件也没有。它不拿结构判据冒充真机读数。

use std::sync::{Mutex, OnceLock, PoisonError};

use falcon_core::feedback::{self, Consent, Draft};

use crate::state::Check;

/// PRD 第 1694 行写的就是这七个字（「用户点击“反馈问题”」），**这一枚名唯一的作者**：页上那枚钮、
/// [`crate::settings_view::feedback_words`]、本档 ① 那一格问的都是它。措辞在这里是读数，不是读数
/// 之外的修饰（#455），所以不把它顺成「意见反馈」「提交问题」，也不抄进 `.qml`。
///
/// 它**不进托盘名册**：§5.18.1 那一列十行没有这一行，而 §5.21.3 第 1 步只说「用户点击」、没像
/// §5.8.3 / §5.9.13 / §5.4.4 那样点名「托盘菜单」那一枚入口。两处不一致时按**点得着的那一节**办，
/// 但本节没点名的地方不替那张表决定它自己没决定的事（与 [`crate::hotkey::Work::OpenPicturePins`]
/// 那一臂记的是同一枚道理，方向相反：那一臂有名字可寻，这一枚没有）。
pub const WORD: &str = "反馈问题";

/// 第 3 步那格填字的门牌（PRD 用的就是「问题描述」那四个字）。
pub const DESC_LABEL: &str = "问题描述";

/// 第 3 步那枚「是否附加」的勾选框。**页上它是页上那一枚，Rust 里它是 [`Consent::logs`]**，
/// 中间没有第二份名字。
pub const LOGS_LABEL: &str = "附加日志";

/// 第 4 步那枚钮（PRD 第 1697 行「发送反馈」）。
pub const SEND_WORD: &str = "发送反馈";

/// 收起这一块的那枚钮。文档没有给这一枚起字，所以用最普通的两个字，不发明一句「稍后再说」。
pub const CLOSE_WORD: &str = "取消";

/// 发送之前屏幕上那一段字 —— 第 1 条隐私规则「先给用户看要递什么」在页上的形状。
pub const PREVIEW_LABEL: &str = "将要递出去的内容";

/// 诊断那一半的标题（第 2 步的产物在页上占的那一行）。
pub const DIAG_LABEL: &str = "可选的诊断信息";

/// 页上这一块要写的每一枚字，次序就是四步说话的样子；
/// [`crate::settings_view::feedback_words`] 把它们用 [`crate::settings_view::RECORD`] 连着递给页，
/// 页按序号取、一个字也不自己写（#281）。
///
/// [`CLOSE_WORD`] 有意排在末尾而不是与 [`SEND_WORD`] 成对：门那一格扫页上不许出现的字时**跳过它**
/// —— 「取消」这两个字在设置页上早就是 §5.20.3 导入那一问的答钮（盘中此刻 12 处），扫它只会红在
/// 别人的句子上。这一格管的是「这一块有没有第二份措辞」，不是「这两个字全页唯一」。
pub const ON_PAGE: [&str; 7] = [
    WORD,
    DESC_LABEL,
    LOGS_LABEL,
    DIAG_LABEL,
    PREVIEW_LABEL,
    SEND_WORD,
    CLOSE_WORD,
];

/// [`ON_PAGE`] 里页上不许自己写出来的那几枚 —— 除 [`CLOSE_WORD`] 之外全在这里。
pub const OWNED: [&str; ON_PAGE.len() - 1] = [
    WORD,
    DESC_LABEL,
    LOGS_LABEL,
    DIAG_LABEL,
    PREVIEW_LABEL,
    SEND_WORD,
];

/// 这一块用到的四条腿：`(页上那枚东西, 桥上的名字)`。表**不实现任何 handler**，它只是把
/// 「钮上写着发送、递进产品的却是取消」这一族缺陷做成一格可扫的判据 —— 与
/// [`crate::history_window::ROUTES`] 同一枚形状，只是这一档的腿全长在设置窗里，不必再抄主人那一列。
pub const LEGS: [(&str, &str); 4] = [
    (WORD, "feedbackAsk"),
    (DESC_LABEL, "feedbackPreview"),
    (SEND_WORD, "feedbackSend"),
    (CLOSE_WORD, "feedbackClose"),
];

/// 这一趟里这一块的状态。与 `custom::Dialog` 同一族：进程级一格，两个实例各填一份就会有一份过期。
#[derive(Debug, Default)]
struct Block {
    /// 第 1 步那枚钮被点了几回（门与 selftest 读的是这一枚，不是「页上有没有红字」）。
    asked: u64,
    /// 第 4 步真的递出去几回。
    sent: u64,
    /// 被拒几回 —— 每一句拒的话都进了运行历史（[`crate::state::PinState::note`] 那一族不在这里，
    /// 这一档的账由 `last` 带着原话）。
    refused: u64,
    /// 最近一次按下交回屏幕的那句原话。
    last: String,
    /// 最近一次递出去的包里藏掉的枚数（`sent` 为 0 时它是 0，不残留上一趟的读数）。
    redactions: usize,
}

static BLOCK: OnceLock<Mutex<Block>> = OnceLock::new();

fn with<R>(f: impl FnOnce(&mut Block) -> R) -> R {
    let mut guard = BLOCK
        .get_or_init(|| Mutex::new(Block::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// `(点过几回, 递出几回, 拒了几回, 最近那句原话, 那包里藏了几枚)`。
pub fn stats() -> (u64, u64, u64, String, usize) {
    with(|b| {
        (
            b.asked,
            b.sent,
            b.refused,
            b.last.clone(),
            b.redactions,
        )
    })
}

/// 第 2 步「系统生成可选的诊断信息」的那一双手：**此刻**现读，不缓存、不抄一份样例。
///
/// 里面只有文本行：横幅由 [`crate::settings`] 那几位主人各交回自己那一行（这一档不重排它们的
/// 措辞，#281），配置解析的毛病逐行进 [`crate::settings::notes`]，名册上此刻有几枚贴图。
/// 截图原图进不来 —— 这一句在类型上就成立（[`Draft`] 没有字节字段），不是这里的担保。
pub fn diagnostics() -> Vec<String> {
    let mut out = vec![
        format!("build={} os=windows", env!("CARGO_PKG_VERSION")),
        crate::settings::startup_banner(),
        crate::settings::startup_about_banner(),
        crate::settings::startup_output_banner(),
        crate::settings::startup_appearance_banner(),
    ];
    out.extend(crate::settings::with(|s| s.notes()));
    // 运行中记下的问题行也属于诊断（它们正是「哪里不对」），但它们是**日志**那一半的货：
    // 递不递由用户那一枚勾选框说，不由这里决定，所以这一列不把它们并进诊断。
    out.push(format!(
        "roster={} history_rows={}",
        crate::state::with(|s| s.ids().len()),
        crate::history_window::rows_now().len()
    ));
    out
}

/// 第 3 步那枚勾选框背后的东西：**本进程这一趟记下的问题行**（[`crate::state::PinState::problems`]
/// 的唯一读数处之一）。这一档不另开一份日志：本 build 里没有日志文件，`advanced.log_level` 至今
/// 只有横幅那一位读者，所以「日志」在这里只能是那些真发生过的行 —— 造一枚假的日志文件来填这一格，
/// 就是让第 3 步的勾选框勾上一件不存在的东西。
pub fn logs() -> Vec<String> {
    crate::state::with(|s| s.problems.clone())
}

/// 把此刻能读到的一切凑成一份草稿。`description` 是用户那一格里的原文，逐字带走。
pub fn draft(description: &str) -> Draft {
    Draft {
        description: description.to_string(),
        diagnostics: diagnostics(),
        logs: logs(),
    }
}

/// 三臂共用的那一次组装：配置那枚开关现读，用户那两样由调用方递进来。
fn build(description: &str, confirmed: bool, logs_box: bool) -> feedback::Payload {
    let send = crate::settings::with(|s| s.config.about.send_diagnostics);
    feedback::payload(
        &draft(description),
        send,
        &Consent {
            confirmed,
            logs: logs_box,
        },
        env!("CARGO_PKG_VERSION"),
        "windows",
    )
}

/// 第 2 步：点开后屏幕上先看见的那一块。交回的是**生成出来的那几行**（已按第 2 条隐私规则脱敏），
/// 不是「一共有五行」这种计数 —— 用户有权在看清楚之后才决定递不递。
///
/// 逐行先过 [`falcon_core::feedback::redact_line`]：这一列在屏幕上就是明文，而「未经用户确认
/// 不得上传」管的是出去那一半，看得见的那一半若还带着原值，用户点头时点的就是一件他没见过全貌的事。
pub fn ask() -> String {
    let lines: Vec<String> = diagnostics()
        .iter()
        .map(|l| falcon_core::feedback::redact_line(l).text)
        .collect();
    let said = if lines.is_empty() {
        format!("{DIAG_LABEL}：这一趟一行也没生成出来。")
    } else {
        format!("{}（{} 行）", lines.join("\n"), lines.len())
    };
    with(|b| {
        b.asked += 1;
        b.last = said.clone();
    });
    said
}

/// 屏幕上那段预览：**「此刻按下去会递出去的那一段字」**，与 [`send`] 用的是同一只手
/// （[`build`]），所以「预览与所递分家」这一种坏在这一档里没有发生的余地。
///
/// 它带的措辞只有一句「这是预览」，因为页上那一行的标题写的就是 [`PREVIEW_LABEL`]（#281）。
pub fn preview(description: &str, logs_box: bool) -> String {
    build(description, true, logs_box).text()
}

/// 渠道那一半：它只把档上那串字递给 [`crate::session::open_web`] 那一道闸，认不认得不由这里判。
///
/// 单独一只手，是因为门那一档要能在**不碰剪贴板**的前提下问出这两句话（⑩ 那一格）：空着与打不开
/// 是两种坏，得各有自己的说法（#453）。空着那一支是本 build 的出厂状态 —— `about.feedback_channel`
/// 出厂为空，config 头顶记了为什么（替用户预填一句地址，就是让一枚默认值把机器往别人那台服务器上指）。
fn channel_leg(channel: &str) -> String {
    if channel.trim().is_empty() {
        "还没配反馈渠道（`[about] feedback_channel` 空着）：这一趟只把包放进剪贴板，没有替你打开任何东西。"
            .to_string()
    } else {
        crate::session::open_web(channel)
    }
}

/// 第 4 步。两半各办各的、各说各的话：
/// 1. 包写成文本进剪贴板 —— 渠道那半不成时，用户手里仍拿着那份字；
/// 2. 渠道那半走 [`channel_leg`]（`about.feedback_channel` 是唯一来源，现问于按下那一刻）。
///
/// 空描述被拒：第 3 步要的是**用户填写**的那句话，一句不填就递出去，收到的那一位无从回话。
pub fn send(description: &str, logs_box: bool) -> String {
    if description.trim().is_empty() {
        let said = format!("「{DESC_LABEL}」那一格还空着 —— 一句话也没写，这一趟没有递出去。");
        with(|b| {
            b.refused += 1;
            b.last = said.clone();
        });
        return said;
    }
    let payload = build(description, true, logs_box);
    let text = payload.text();
    let scrubbed = if payload.redactions == 0 {
        String::new()
    } else {
        format!("（藏掉 {} 枚敏感值）", payload.redactions)
    };
    let clip = match platform_windows::clip::write_text(&text) {
        Ok(()) => format!(
            "反馈包 {} 字节已进剪贴板{}。",
            text.len(),
            if payload.attached() {
                scrubbed
            } else {
                "（这一趟机器那一半一枚也没带）".to_string()
            }
        ),
        Err(e) => format!("递到剪贴板这一半没成：{e}"),
    };
    let door = channel_leg(&crate::settings::with(|s| s.config.about.feedback_channel.clone()));
    let said = format!("{clip}{door}");
    let redactions = payload.redactions;
    with(|b| {
        // 记的是**这一半**成没成：渠道那半红着不影响「包已到手」这一件事实，而两半都算一次 sent
        // 会让「剪贴板写成了、渠道打不开」被读成什么都没发生（#433）。
        if clip.starts_with("反馈包") {
            b.sent += 1;
            b.redactions = redactions;
        } else {
            b.refused += 1;
            b.redactions = 0;
        }
        b.last = said.clone();
    });
    said
}

/// 收起这一块（页上那枚 [`CLOSE_WORD`]）。它不改任何一列盘上的值 —— 这一档从头到尾不写用户的档
/// （§9.1 第十五条）。
pub fn close() {
    with(|b| b.last = format!("「{WORD}」这一块已收起，什么都没递。"));
}

/// `--feedback` 那一扇门。
///
/// 它量的是「名字 → 决定」这一列：四步各自的那位作者、两把闸的四种组合、脱敏在真读的几行上成不
/// 成立、页绑的腿名对不对得上 [`LEGS`]。它**不**开窗、不等手指，也不碰剪贴板（那三件事的读数记在
/// 末尾那三格 Blocked 里）。
///
/// 唯一被真的按过一次的是 [`send`] 的空描述那一支 —— 它在碰到剪贴板之前就走开了，所以这一档
/// 不需要用户的手也能判「这一枚钮不是只会照单收」。
pub fn measure() -> (Check, String) {
    use crate::r13::Report;

    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    // ---- ① 第 1 步那七个字，是从**文档自己身上**读回来的。
    let quoted = doc_step_word(1);
    rep.row(
        "step 1 names the word the button carries",
        Check::from(quoted.as_deref() == Some(WORD)),
        format!(
            "文档第 1 步引起来的那一枚名 {:?}，页上那枚钮带的是 {WORD:?}（唯一作者 [`feedback_window::WORD`]）",
            quoted
        ),
    );

    // ---- ② 页拿到的只有桥递的那一列字：别处不抄第二份。
    let about = crate::settings_view::index_of(crate::settings_view::ABOUT_PAGE);
    let handed = about.map(crate::settings_view::feedback_words).unwrap_or_default();
    let cells: Vec<&str> = handed.split(crate::settings_view::RECORD).collect();
    let only_about = about.map_or(false, |i| {
        (0..crate::settings_view::CATEGORIES.len())
            .filter(|k| *k != i)
            .all(|k| crate::settings_view::feedback_words(k).is_empty())
    });
    let copied: Vec<&str> = OWNED.iter().copied().filter(|w| PAGE.contains(*w)).collect();
    rep.row(
        "the page is handed the words, never a copy",
        Check::from(
            cells == ON_PAGE && only_about && copied.is_empty(),
        ),
        format!(
            "「关于与更新」是第 {:?} 类，那一类递 {} 枚字 {}；其余 {} 类递空串={}；页上自己写出来的这一块的字：{}",
            about,
            cells.len(),
            cells.join("/"),
            crate::settings_view::CATEGORIES.len() - 1,
            only_about,
            if copied.is_empty() { "一枚也没有".to_string() } else { copied.join("、") },
        ),
    );

    // ---- ③ 四条腿：桥上有这个名字的声明，页上有对它的调用。
    let session = include_str!("session.rs");
    let mut missing: Vec<String> = Vec::new();
    for (_, leg) in LEGS {
        // 桥那一头要的是**递到 QML 的那一枚名**（`#[cxx_name = "…"]`，与 `history_window` 的
        // `ROUTES` 同一把刀）：Rust 那侧的本名是 snake_case，扫 `fn feedbackAsk(` 只会红在
        // 「名字换了写法」这一件不存在的事上。页那一头要的是**调用**（`.名字(`）。
        if !session.contains(&format!("#[cxx_name = \"{leg}\"]")) {
            missing.push(format!("{leg} 不在桥上"));
        }
        if !PAGE.contains(&format!(".{leg}(")) {
            missing.push(format!("{leg} 没被页绑上"));
        }
    }
    rep.row(
        "every leg the roster names is bound on the page",
        Check::from(missing.is_empty()),
        format!(
            "名单 {} 行 {}；缺 {}",
            LEGS.len(),
            LEGS.iter()
                .map(|(w, l)| format!("{w}→{l}"))
                .collect::<Vec<_>>()
                .join("、"),
            if missing.is_empty() { "谁也没有".to_string() } else { missing.join("｜") },
        ),
    );

    // ---- ④ 两把闸的四种组合，各答各的问题。这一格用的是**明写的四组布尔**，不是用户档上此刻的值：
    // 门不该因为这台上有人勾了或没勾而变绿变红（§9.1 第十五条只许读、不许写，而读数不是判据的分母）。
    let d = Draft {
        description: "点了没反应".into(),
        diagnostics: vec!["password=x".into()],
        logs: vec!["token=y".into()],
    };
    let arm = |send: bool, confirmed: bool, logs_box: bool| {
        feedback::payload(
            &d,
            send,
            &Consent {
                confirmed,
                logs: logs_box,
            },
            "0.0.0",
            "windows",
        )
    };
    let (all_off, diag_only, logs_only, both) = (
        arm(false, false, false),
        arm(true, true, false),
        arm(false, true, true),
        arm(true, true, true),
    );
    let omitted = |p: &feedback::Payload| p.lines.iter().filter(|l| l.contains("=omitted")).count();
    rep.row(
        "the two gates are two gates",
        Check::from(
            !all_off.diagnostics
                && !all_off.logs
                && !all_off.attached()
                && omitted(&all_off) == 2
                && omitted(&diag_only) == 1
                && omitted(&logs_only) == 1
                && omitted(&both) == 0
                && diag_only.diagnostics
                && !diag_only.logs
                && !logs_only.diagnostics
                && logs_only.logs
                && both.diagnostics
                && both.logs
                && both.lines.len() == 4
                && all_off.lines.len() == 4,
        ),
        format!(
            "四臂（诊断开关, 点下头, 勾日志）= (F,F,F)→diag={} log={} 行{} 省略{}｜(T,T,F)→diag={} log={} 省略{}｜(F,T,T)→diag={} log={} 省略{}｜(T,T,T)→diag={} log={} 行{} 省略{}",
            all_off.diagnostics, all_off.logs, all_off.lines.len(), omitted(&all_off),
            diag_only.diagnostics, diag_only.logs, omitted(&diag_only),
            logs_only.diagnostics, logs_only.logs, omitted(&logs_only),
            both.diagnostics, both.logs, both.lines.len(), omitted(&both),
        ),
    );

    // ---- ⑤ 那枚勾选框改的是**出去的那一段字**，不是屏幕上的一句装饰。
    // 用的是本档此刻真的会递出去的那两包（[`build`]）：日志那一半，勾上时占 `max(在册枚数, 1)` 行
    // （一枚也没有也有那句 `log=none rows=0`），没勾时占一行省略句 —— 这一条只由
    // [`falcon_core::feedback`] 那一手成立，页上没有任何一处参与。
    let on = build("贴图不见了", true, true);
    let off = build("贴图不见了", true, false);
    let real_logs = logs().len();
    rep.row(
        "the logs box moves what leaves",
        Check::from(
            on.logs
                && !off.logs
                && on.text() != off.text()
                && off.text().contains("log=omitted")
                && on.diagnostics == off.diagnostics
                && on.lines.len() + 1 == off.lines.len() + real_logs.max(1),
        ),
        format!(
            "勾上 {} 行 {} 字节 / 没勾 {} 行 {} 字节；本趟在册的日志行 {} 枚；没勾那一包里 log= 行 {}；诊断那一半两臂相同={}（它不归这枚勾选框管，也不归这台机器的档管——门不许因为用户勾没勾过诊断而红）",
            on.lines.len(),
            on.text().len(),
            off.lines.len(),
            off.text().len(),
            real_logs,
            off.lines.iter().filter(|l| l.starts_with("log=")).count(),
            on.diagnostics == off.diagnostics,
        ),
    );

    // ---- ⑥ 第 2 步**真的生成过**：现读的几行在屏幕上递的是脱敏后的文本，而这台机器的账号名
    // 不在其中一条里。这一格读的是本机此刻的档与本机此刻的主目录 —— 它不写、只判。
    let said = ask();
    let account = std::env::var("USERPROFILE")
        .unwrap_or_default()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_string();
    let raw_diag = diagnostics();
    let leak = !account.is_empty() && said.contains(account.as_str());
    rep.row(
        "step 2 generates diagnostics that are already scrubbed",
        Check::from(!said.is_empty() && raw_diag.len() >= 6 && !leak),
        format!(
            "生成 {} 行、屏幕上那一段 {} 字节；主目录名 {}；这段里出现原值 = {}",
            raw_diag.len(),
            said.len(),
            if account.is_empty() {
                "(读不到)".to_string()
            } else {
                format!("{account:?} → 出现={}", leak)
            },
            usize::from(leak),
        ),
    );

    // ---- ⑦ 一条带着口令的行递出去时，原值一枚也不留（第 2 条隐私规则的可数形状）。
    let leaky = Draft {
        description: "打不开".into(),
        diagnostics: vec!["app.password=hunter2 Authorization: Bearer eyJhbGci".into()],
        logs: vec![format!(r#"path=C:\Users\{}\a.png token=abc"#, if account.is_empty() { "nobody" } else { account.as_str() })],
    };
    let p = feedback::payload(
        &leaky,
        true,
        &Consent {
            confirmed: true,
            logs: true,
        },
        "0.0.0",
        "windows",
    );
    let text = p.text();
    let out = ["hunter2", "eyJhbGci", "abc"]
        .iter()
        .filter(|v| text.contains(**v))
        .map(|v| (*v).to_string())
        .collect::<Vec<_>>();
    rep.row(
        "a secret handed over never reaches the text",
        Check::from(
            out.is_empty() && p.redactions >= 3 && text.matches(feedback::MARK).count() >= 3,
        ),
        format!(
            "藏掉 {} 枚、记号 {} 枚；原值漏出的 {}；包 {} 字节",
            p.redactions,
            text.matches(feedback::MARK).count(),
            if out.is_empty() { "无".to_string() } else { out.join("、") },
            text.len(),
        ),
    );

    // ---- ⑧ 截图原图进不来：这一条判的是**类型上**没有装得下它的地方，不是「这里没往里塞」。
    let src = include_str!("../../core/src/feedback.rs");
    let draft_body = src
        .find("pub struct Draft")
        .map(|i| {
            let rest = &src[i..];
            &rest[..rest.find("\n}").unwrap_or(rest.len())]
        })
        .unwrap_or_default();
    let room = ["Vec<u8>", "Frame", "bytes", "pixels", "&[u8]"]
        .iter()
        .filter(|t| draft_body.contains(**t))
        .map(|t| (*t).to_string())
        .collect::<Vec<_>>();
    rep.row(
        "a picture has nowhere to ride",
        Check::from(draft_body.contains("pub description: String") && room.is_empty()),
        format!(
            "Draft 的字段类型 {}；装得下原图的那种类型 {}",
            draft_body
                .lines()
                .filter(|l| l.contains("pub "))
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" / "),
            if room.is_empty() { "一种也没有".to_string() } else { room.join("、") },
        ),
    );

    // ---- ⑨ 那枚钮不是只会照单收：一句没写就按下，它拒、并且**什么都没递**。
    let (r0, s0, f0, _, _) = stats();
    let refused = send("   ", false);
    let (r1, s1, f1, last, _) = stats();
    rep.row(
        "an empty description is refused, and nothing left",
        Check::from(
            refused.contains(DESC_LABEL) && r1 == r0 && s1 == s0 && f1 == f0 + 1 && last == refused,
        ),
        format!(
            "那一问：{refused}｜点过 {r0}→{r1} 回、递出 {s0}→{s1} 回、拒 {f0}→{f1} 回",
        ),
    );

    // ---- ⑩「还没配渠道」与「配的这串字打不开」是两句话。这两支都在碰到剪贴板之前，按得。
    let none = channel_leg("");
    let bad = channel_leg("notaurl");
    rep.row(
        "no channel and an unusable channel are two sentences",
        Check::from(
            none.contains("还没配") && !none.contains("读不出主机名") && !bad.is_empty() && none != bad,
        ),
        format!("空着说的是「{none}」｜notaurl 说的是「{bad}」"),
    );

    // ---- ⑪ 此刻这台机器上那一份档的读数，只作旁证，不进任何一格判定。
    let about_now = crate::settings::about_cfg();
    rep.note(
        "the config this machine runs on",
        format!(
            "用户那份档此刻（只读、只印，§9.1 第十五条）：send_diagnostics={} feedback_channel={:?}",
            about_now.send_diagnostics, about_now.feedback_channel
        ),
    );

    // ---- ⑫⑬⑭ 这一档起不动的那三条腿。
    for (name, why) in [
        (
            "a real finger opens the block on the real page",
            "这一档打完就退、不 `engine.load()`，页上那枚钮一次也没被点过：要它得有跑的泵（与 \
             `--help-menu` 的 ①、`--update-check` 的 ⑪ 同一处结转）。",
        ),
        (
            "the bundle really lands in the clipboard",
            "`clip::write_text` 一步就成了：这一档不替用户把他剪贴板上的东西改掉 —— 那是别人的桌面，\
             不是本档的证人。",
        ),
        (
            "the channel really opens in the browser",
            "同上：`open_web` 递出去的是默认浏览器那一扇真的会亮起来的窗，门不替用户开它。",
        ),
    ] {
        rep.row(name, Check::Blocked, why.to_string());
    }

    rep.finish("feedback door")
}

/// 需求文档 §5.21.3「操作步骤」第 `n` 步里被弯引号括起来的那一枚名字，从 `include_str!` 进来的
/// 那份文档身上取。判据的字句不许抄第二遍（#281）：文档改了而这里不改，① 那一格就该红。
fn doc_step_word(n: usize) -> Option<String> {
    let at = DOC.find("### 5.21.3 问题反馈")?;
    let rest = &DOC[at + "### 5.21.3 问题反馈".len()..];
    let end = rest.find("\n### ").unwrap_or_else(|| rest.find("\n---").unwrap_or(rest.len()));
    let line = rest[..end]
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("{n}.")))?
        .trim();
    let a = line.find('\u{201c}')?;
    let b = line[a..].find('\u{201d}')?;
    Some(line[a + '\u{201c}'.len_utf8()..a + b].to_string())
}

/// 需求文档本体，编译期取进来的一份 —— 与 [`crate::help_menu::DOC`] 同一枚理由。
const DOC: &str = include_str!("../../../AI Falconshot 全部功能需求文档.md");

/// `qml/SettingsWindow.qml`：这一块长在它里面，②③ 两格扫的都是它。
const PAGE: &str = include_str!("../qml/SettingsWindow.qml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_is_read_off_the_document_not_retyped() {
        // ① 那一格的分母：文档第 1 步引起来的就是页上那枚钮带的字。抄一份进 `WORD` 而文档改了，
        // 这一格就该红 —— 它钉的是「措辞是读数」（#455）。
        assert_eq!(doc_step_word(1).as_deref(), Some(WORD));
        // 第 2、3、4 步里一枚被引起来的名字也没有：那三步说的是「生成什么」「填哪一格」「去哪儿」，
        // 只有第 1 步给了字。这一格因此不许顺手替它们编一名（#455 的反面 —— 没有读数的地方不写读数）。
        assert_eq!(doc_step_word(2), None);
        assert_eq!(doc_step_word(3), None);
        assert_eq!(doc_step_word(4), None);
        // 一节之外读不到东西，不是「默认回一个名」。
        assert_eq!(doc_step_word(9), None);
    }

    #[test]
    fn only_the_about_page_is_handed_the_words() {
        let about = crate::settings_view::index_of(crate::settings_view::ABOUT_PAGE)
            .expect("名单里必须有「关于与更新」");
        let handed = crate::settings_view::feedback_words(about);
        let cells: Vec<&str> = handed.split(crate::settings_view::RECORD).collect();
        assert_eq!(cells, ON_PAGE, "页拿到的那一列与名单不等长或不等值");
        assert_eq!(
            cells[0].chars().count(),
            4,
            "页上那枚钮递的是四个字，不是把它顺成的三个字或五个字"
        );
        for i in 0..crate::settings_view::CATEGORIES.len() {
            if i != about {
                assert_eq!(crate::settings_view::feedback_words(i), "", "第 {i} 类递了反馈那一块");
            }
        }
        assert_eq!(crate::settings_view::feedback_words(usize::MAX), "");
    }

    #[test]
    fn the_page_binds_the_leg_the_roster_names() {
        // 页上抄一份 `feedbackSend` 而桥上叫别的名字，屏幕上那枚钮按下去就是一片安静（#433）。
        for (word, leg) in LEGS {
            assert!(
                !word.is_empty(),
                "名单里有一行没有页上那枚东西的名字：{leg}"
            );
            assert!(
                include_str!("session.rs").contains(&format!("#[cxx_name = \"{leg}\"]")),
                "{leg} 没在桥上声明"
            );
            assert!(
                PAGE.contains(&format!(".{leg}(")),
                "{leg} 没被页绑上"
            );
        }
        assert_eq!(LEGS.len(), 4, "四步用到的腿是四条，不多不少");
    }

    #[test]
    fn the_page_never_spells_a_word_of_this_block() {
        // #281：页上是显示器，不是名单的第二位主人。
        for word in OWNED {
            assert!(
                !PAGE.contains(word),
                "{word:?} 在 .qml 里被写了一遍 —— 它唯一的作者在本档"
            );
        }
        // 「取消」有意不在这列里（页上它是导入那一问的答钮），而这一条也得是可数的，不然
        // 「有意不管」与「忘了管」长得一样。
        assert!(!OWNED.contains(&CLOSE_WORD));
        assert!(PAGE.contains(CLOSE_WORD), "结转的那一枚字此刻不在页上，这一格该重看");
    }

    #[test]
    fn no_channel_and_an_unusable_channel_are_two_sentences() {
        let none = channel_leg("");
        let bad = channel_leg("notaurl");
        assert!(none.contains("还没配"), "{none}");
        assert!(none.contains("feedback_channel"), "{none}");
        assert!(!none.contains("读不出主机名"), "没配不能说成地址坏了：{none}");
        assert!(!bad.contains("还没配"), "配了而打不开不能说成没配：{bad}");
        assert_ne!(none, bad);
    }

    #[test]
    fn an_empty_description_leaves_the_clipboard_alone() {
        // 这一支在 [`send`] 里排在剪贴板之前 —— 它是本档唯一敢在单测里按的那一枚钮：按下去
        // 什么都不动，所以「这一枚钮不是只会照单收」这句话在测试里也成立（#494 的那一型）。
        let said = send("   ", true);
        assert!(said.contains(DESC_LABEL), "{said}");
        assert!(said.contains("没有递出去"), "{said}");
        // 空串与全是空白同一判：填了一格空格不等于写了一句话。
        assert_eq!(send("\n\t ", false), said);
    }

    #[test]
    fn the_box_moves_only_the_logs() {
        // 用明写的四组布尔问 core 那一手，不读这台机器的档：门与单测因此不随用户勾没勾而红。
        let d = Draft {
            description: "打不开".into(),
            diagnostics: vec!["password=x".into()],
            // 日志给两行：只给一行的话「没勾」那一臂留一行省略、「勾上」那一臂也正好两行进包，
            // 行数一样，这一格就什么都没判（差值必须是可数的，不是一行换一行）。
            logs: vec!["token=y".into(), "run 2 secret=z".into()],
        };
        let arm = |confirmed: bool, logs_box: bool| {
            feedback::payload(
                &d,
                true,
                &Consent {
                    confirmed,
                    logs: logs_box,
                },
                "0.0.0",
                "windows",
            )
        };
        let (no_box, boxed, unasked) = (arm(true, false), arm(true, true), arm(false, true));
        assert_eq!(
            boxed.lines.len(),
            no_box.lines.len() - 1 + d.logs.len(),
            "没勾那一半留一行省略，勾上就把那一行换成那 {} 行",
            d.logs.len()
        );
        assert!(no_box.diagnostics && boxed.diagnostics, "勾框不动诊断那一半");
        assert!(!no_box.logs && boxed.logs);
        assert!(
            !unasked.logs && !unasked.diagnostics,
            "没点发送时勾着的框也不算点头"
        );
    }
}

