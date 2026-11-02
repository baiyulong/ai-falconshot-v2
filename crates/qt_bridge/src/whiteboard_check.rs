//! §5.4.4 白板模式与 §5.4.5 透明白板模式的那一扇门。
//!
//! 它判的是四件不需要人坐在对面的事：档上那两枚键读回来是什么（步骤 2「选择白板背景颜色」
//! 与步骤 3「与屏幕或指定区域匹配」）、那一身底色做出来的画布**真的是什么**（逐像素，不是
//! 一句"应该是白"）、一块透明的板子交出去时哪一种格式会被填、填成哪一枚颜色（§5.4.5 那两条
//! 规则），以及「白板」这一枚动作名有没有被偷偷编进可绑定名单（§5.16.1 那张表是文档的，
//! 不是这里的发明）。
//!
//! 画布那两格走的是产品自己那一路：真写三份 fixture 到本档目录、真从文件读回来
//! （`Settings::open` ⇒ `Config::load` ⇒ `validate`），再递给 [`falcon_core::whiteboard`]
//! 那两枚解析器。所以这里印的是"这一份档读出来开得起哪一块板"，而不是"这个函数写得对"——
//! 后者由 `cargo test -p falcon-core` 那一族 7 格负责，它们在本档目录里不留一行。
//!
//! 编码那两格更进一层：它们真的把一块透明板交给产品那一路的编码器（`encode::encode`，P5 起
//! 这是唯一一位碰过像素的编码器），再把交回来的字节**读回去看那颗像素**。§5.4.5 第二条要的
//! 是「提醒用户背景将被填充」，而一句提醒只有配上"填出来的确实是那一枚颜色"才算说了真话 ——
//! 填成黑的与填成白的在日志里都是"成功了"。
//!
//! ## 分母从哪儿来
//!
//! 那一格把每一种格式各问一次，而"每一种"的名单不另立：它取 `SAVE_FORMATS` 与
//! `IMPORT_FORMATS` 的并集 —— 产品真交得出去的两张表。若日后有第五种格式两表都不进，
//! 它在白板这一路上根本到不了用户手里，本档不测它是对的；那一份"到不了"由这一行的读数
//! 自己印出来，不靠一句注释被相信。
//!
//! ## 这一档不声称的那件事
//!
//! 它一次也没有开出一扇真遮罩。[`crate::mask::open_board`] 那一步要 Qt 报出屏幕、要一块与
//! 屏幕同尺寸的画布、要 `shim::store_raw` 把像素递过去，而那三样在锁屏会话里都问不到对面；
//! 「屏幕上此刻是不是一块白布」与「透明那一身时用户看得穿自己的画布吗」这两格发的是 BLOCKED
//! —— 与上面那些读数分开写，不拿"画布像素是 [255,255,255,255]"冒充"屏幕上有一块白布"。
//! §5.4.4 第五步那三枚出口（复制 / 保存 / 固定）同理：它们骑的是遮罩上已有的那三枚，
//! 这一档只在透明→JPG 那一格里读到了「保存」那句提醒，另外两枚的对面要一扇真窗。

use std::path::PathBuf;

use falcon_core::config::LoadSource;
use falcon_core::encode::{self, EncodeOptions, Format, FLATTEN_FILL};
use falcon_core::whiteboard::{self, Size, Surface};

use crate::r13::Report;
use crate::settings::Settings;
use crate::state::Check;

/// 本档自己的目录。同一台机器上两趟并行时各自的 fixture 必须是各自的文件，否则后写的那一份
/// 会替前一趟改题（与 `tray_click_check::door_dir` 同一枚理由、同一枚形状）。
fn door_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FALCONSHOT_BOARD_DOOR_DIR") {
        let p = PathBuf::from(&d);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let mut dir = std::env::temp_dir();
    dir.push("falconshot-board-door");
    dir
}

/// 一份 fixture：文件名、写进去的那几行、以及这一格要求它读回来成什么样子。
struct Fixture {
    name: &'static str,
    text: &'static str,
    want_surface: Surface,
    want_size: Size,
    /// 这一份要求 `validate` 报出几句。C 那一份是两枚键各一句，A 与 B 都不该有。
    want_warnings: usize,
}

/// A：两枚键都写在与默认**不同**的一边上。挑默认值等于没测 —— 读回来还是那一身时，"跟着档
/// 走"与"跟着 `Config::default()` 走"是同一个读数。
const A: Fixture = Fixture {
    name: "a-solid.toml",
    text: "[whiteboard]\nbackground = \"#102030\"\nsize = \"1280x720\"\n",
    want_surface: Surface::Solid([0x10, 0x20, 0x30]),
    want_size: Size::Canvas {
        width: 1280,
        height: 720,
    },
    want_warnings: 0,
};

/// B：§5.4.5 那一身。`size` 留在默认，这样这一份只改一枚键，红的时候分得出是哪一枚。
const B: Fixture = Fixture {
    name: "b-clear.toml",
    text: "[whiteboard]\nbackground = \"transparent\"\n",
    want_surface: Surface::Transparent,
    want_size: Size::Screen,
    want_warnings: 0,
};

/// C：两枚键都写读不出的值。这一份判的是**落回与说出**，不是拒绝 —— 一份开不出板子的档
/// 不该让整个应用起不来，但它必须把"你的 `beige` 我读不出、已经落回白"这句话留给用户。
const C: Fixture = Fixture {
    name: "c-typo.toml",
    text: "[whiteboard]\nbackground = \"beige\"\nsize = \"0x0\"\n",
    want_surface: Surface::Solid([255, 255, 255]),
    want_size: Size::Screen,
    want_warnings: 2,
};

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

fn source(s: &Settings) -> &'static str {
    match s.source {
        LoadSource::Defaults => "defaults",
        LoadSource::File => "file",
        LoadSource::Recovered => "recovered",
    }
}

/// 产品交得出去的每一种格式，去重后按两张表的原次序。
fn every_format() -> Vec<Format> {
    let mut out: Vec<Format> = Vec::new();
    for f in encode::SAVE_FORMATS
        .iter()
        .chain(encode::IMPORT_FORMATS.iter())
    {
        if !out.contains(f) {
            out.push(*f);
        }
    }
    out
}

/// 一块板子说成一句：底色、尺寸、以及那块 4x3 画布的**那颗像素**。
///
/// 像素是这一档唯一能替 §5.4.4 第四步"绘制内容"站岗的读数 —— 底色与尺寸都只是档上的两串字，
/// 而 `canvas()` 交回来的才是要贴进遮罩的东西。4x3 而不是 1x1：一枚角上的像素看不出"每一颗
/// 都是这一身"，`is_uniform` 数的就是这一件事。
fn board_of(surface: Surface, size: Size) -> String {
    let text = size.text();
    let canvas = whiteboard::canvas(surface, 4, 3);
    match canvas {
        Ok(frame) => format!(
            "底色 {} 尺寸 {text}；画布 {}x{} 每颗 {:?}，透明={}",
            arm_name(surface),
            frame.width,
            frame.height,
            frame.get(0, 0),
            frame.has_transparency()
        ),
        Err(e) => format!("底色 {} 尺寸 {text}；画布开不出来：{e}", arm_name(surface)),
    }
}

fn arm_name(surface: Surface) -> String {
    match surface {
        Surface::Transparent => "transparent".to_string(),
        Surface::Solid([r, g, b]) => format!("#{r:02X}{g:02X}{b:02X}"),
    }
}

/// 读回来的一身怎么说成一句；解析失败时不许沉默 —— 把失败本身印出来，而不是顶一块黑的进去。
fn read_board(section: &falcon_core::config::Whiteboard) -> String {
    match (section.surface(), section.size()) {
        (Ok(surface), Ok(size)) => board_of(surface, size),
        (Err(surface), _) | (_, Err(surface)) => format!("读不出：{surface}"),
    }
}

/// 这一档：三格档上的键 + 两格编码 + 一格路由，加一格发出去的 BLOCKED。
pub fn measure() -> (Check, String) {
    let mut rep = Report::new();
    crate::r13::scene_row(&mut rep);

    let (path_a, a, seed_a) = planted(&A);
    let (path_b, b, seed_b) = planted(&B);
    let (path_c, c, seed_c) = planted(&C);
    let loaded = |s: &Settings| s.source == LoadSource::File && s.backup.is_none();
    rep.row(
        "board fixtures load",
        Check::from(loaded(&a) && loaded(&b) && loaded(&c)),
        format!(
            "三份 fixture：{} {seed_a}（{}）；{} {seed_b}（{}）；{} {seed_c}（{}）——要求 from=file \
             且没被挪走成 broken 档。本档目录 {}；用户那份档不在这里、也不被写（三枚 pinned 各自\
             钉在自己那份上）",
            path_a.display(),
            source(&a),
            path_b.display(),
            source(&b),
            path_c.display(),
            source(&c),
            door_dir().display()
        ),
    );

    // ---- §5.4.4 步骤 2 与步骤 3：两份合规的档各读回来自己那一身，并开出自己那块画布。
    for (name, settings, fixture) in [("A", &a, &A), ("B", &b, &B)] {
        let got = (
            settings.config.whiteboard.surface(),
            settings.config.whiteboard.size(),
        );
        let ok = got.0 == Ok(fixture.want_surface) && got.1 == Ok(fixture.want_size);
        rep.row(
            &format!("board {name} reads"),
            Check::from(ok),
            format!(
                "读回来 {}；要求 {}",
                read_board(&settings.config.whiteboard),
                board_of(fixture.want_surface, fixture.want_size)
            ),
        );
    }

    // ---- C：读不出的两枚键各落回各的默认，各说一句。格数在这里是判据而不是修辞：合并成
    // 一句就答不出是哪一枚键出的错，而多出一句就是把 [tray] 那一段的规则也算进来。
    let named: Vec<&String> = c
        .warnings
        .iter()
        .filter(|l| l.starts_with("whiteboard."))
        .collect();
    let both = ["whiteboard.background:", "whiteboard.size:"];
    let repaired = c.config.whiteboard.violations();
    rep.row(
        "board C falls back",
        Check::from(
            c.config.whiteboard.surface() == Ok(C.want_surface)
                && c.config.whiteboard.size() == Ok(C.want_size)
                && named.len() == C.want_warnings
                && both
                    .iter()
                    .all(|needle| named.iter().any(|l| l.starts_with(needle)))
                && repaired.is_empty(),
        ),
        format!(
            "读回来 {}；warning {} 句 {named:?}（要求 {} 句，各以 {both:?} 起头）；归一之后再问一次\
             违规，交出 {} 枚（要求 0 —— 落回后的值必须是解析器自己认得的，否则那句话每开一次档\
             就说一遍）",
            read_board(&c.config.whiteboard),
            named.len(),
            C.want_warnings,
            repaired.len()
        ),
    );

    // ---- §5.4.5 第一条与第二条：一块透明板对每一种格式各问一次，判据是 `supports_alpha`
    // 那唯一的一位读者。分母是 `SAVE_FORMATS ∪ IMPORT_FORMATS`，不是手抄的。
    let formats = every_format();
    let mut lossy: Vec<String> = Vec::new();
    let mut safe: Vec<String> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();
    for format in &formats {
        let plan = whiteboard::plan_export(Surface::Transparent, Some(*format));
        let says = format!(
            "{}→{} warn={} fill={:?}",
            format.ext(),
            plan.format.ext(),
            plan.warn,
            plan.filled_with
        );
        // 三条一起判：格式不许被改写、该说时要说、说不说必须与 `supports_alpha` 一致。
        let agree = plan.format == *format
            && plan.warn == !format.supports_alpha()
            && plan.filled_with == if plan.warn { Some(FLATTEN_FILL) } else { None };
        if !agree {
            wrong.push(says.clone());
        }
        if plan.warn {
            lossy.push(says);
        } else {
            safe.push(says);
        }
    }
    let solid_warns: Vec<String> = formats
        .iter()
        .filter(|f| whiteboard::plan_export(Surface::Solid([1, 2, 3]), Some(**f)).warn)
        .map(|f| f.ext().to_string())
        .collect();
    rep.row(
        "5.4.5 export rules",
        Check::from(
            wrong.is_empty() && !lossy.is_empty() && !safe.is_empty() && solid_warns.is_empty(),
        ),
        format!(
            "问的格式 {} 枚（SAVE ∪ IMPORT）；透明板 {} 枚存不下 {lossy:?}、{} 枚留得住 {safe:?}；\
             实心板报出要填的格式 {} 枚；不符的 {wrong:?}。两边都不许空：`supports_alpha` 若整族\
             都是同一边，这一格就只是复述自己",
            formats.len(),
            lossy.len(),
            safe.len(),
            solid_warns.len()
        ),
    );

    // ---- 第二条的那句提醒，配上它说的那一件事真的发生：透明板进 JPG，读回来看那颗像素到底
    // 被填成了什么。填成黑与填成白在"编码成功"这一格里长得一样，而 §5.4.5 说的是白色。
    let frame = whiteboard::canvas(Surface::Transparent, 2, 2).expect("2x2 board");
    let jpg = encode::encode(
        &frame,
        &EncodeOptions {
            format: Format::Jpg,
            ..Default::default()
        },
    );
    let jpg_read = jpg
        .as_ref()
        .ok()
        .and_then(|bytes| encode::decode(bytes).ok())
        .map(|back| back.get(0, 0));
    let reminder = crate::save::reminder_for_format(&frame, Format::Jpg, "这块白板");
    let filled_white = matches!(
        jpg_read,
        Some(pixel)
            if pixel[0] == FLATTEN_FILL[0]
                && pixel[1] == FLATTEN_FILL[1]
                && pixel[2] == FLATTEN_FILL[2]
    );
    rep.row(
        "jpg fills white",
        Check::from(filled_white && reminder.contains("白色") && reminder.contains("PNG")),
        format!(
            "编码 {} 字节，读回来那颗像素 {jpg_read:?}（要求 {:?} 的前三枚）；那句提醒 {reminder:?}",
            jpg.as_ref().map(|b| b.len()).unwrap_or(0),
            FLATTEN_FILL
        ),
    );

    // ---- 第一条的那一格对面：同一块板子进 PNG，那颗像素必须**还是**透明的。上面那一格证明
    // 了"存不下会填"，这一格证明"存得下就不填" —— 两句合起来才是 §5.4.5 那两条规则的形状。
    let png = encode::encode(
        &frame,
        &EncodeOptions {
            format: Format::Png,
            ..Default::default()
        },
    );
    let png_read = png
        .as_ref()
        .ok()
        .and_then(|bytes| encode::decode(bytes).ok())
        .map(|back| back.get(0, 0));
    rep.row(
        "png keeps alpha",
        Check::from(matches!(png_read, Some(pixel) if pixel[3] == 0)),
        format!(
            "编码 {} 字节，读回来那颗像素 {png_read:?}（要求 alpha 仍是 0）——没人点名时交回的默认\
             那枚格式 {:?}，与 §5.4.5「优先使用 PNG」同一名",
            png.as_ref().map(|b| b.len()).unwrap_or(0),
            Format::default().ext()
        ),
    );

    // ---- 「白板」不许被编进可绑定名单：它是只从托盘抵达的那一枚，与「首选项」「退出」同族。
    let mut bindable: Vec<&str> = Vec::new();
    for action in falcon_core::config::ACTIONS {
        if crate::hotkey::Work::of(action) == Some(crate::hotkey::Work::Whiteboard) {
            bindable.push(action);
        }
    }
    let to_who = crate::hotkey::Work::Whiteboard.action();
    rep.row(
        "board is tray-only",
        Check::from(bindable.is_empty() && to_who.is_none()),
        format!(
            "ACTIONS {} 枚里通向「白板」的 {} 枚 {bindable:?}；`Work::Whiteboard.action()` 交回 \
             {to_who:?}（要求 None，与 Settings / Quit 同族 —— §5.16.1 那张表里没有第十三名）",
            falcon_core::config::ACTIONS.len(),
            bindable.len()
        ),
    );

    rep.row(
        "board on screen",
        Check::Blocked,
        format!(
            "这一档一次也没有开出一扇真遮罩：上面 {} 格判的都是档上的两枚键与编码器交回来的像素。\
             「屏幕上此刻是不是一块白布」与「透明那一身时看得穿自己的画布吗」（`board_dim()` 交 0、\
             `board_clear()` 交 true，QML 那侧 `color: view.board_clear` 绑上去）要的是 Qt 报出的\
             屏幕、一扇真窗与一次采样；§5.4.4 第五步那三枚出口里，这一格只在上一格读到了「保存」\
             那句提醒，「复制」与「固定」的对面同样是窗。那一格发的是 BLOCKED，不是删掉的一格，\
             也不拿上面那些像素读数冒充。",
            rep.row_count()
        ),
    );

    rep.finish("whiteboard board")
}
