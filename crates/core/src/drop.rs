//! §5.8.4 方式二「用户将网页图片拖到已存在的贴图窗口」里的**两判**：拖进来的那几格各自是什么、
//! 这一趟该落到哪一枚贴图上。本模块只判这两件事，一只也不碰网络、不读盘、不画像素 ——
//! 取字节的那一只手是 `platform_windows::fetch::bytes`（与 `[about] release_feed` 那份清单共用的
//! 同一只手，理由写在那一模块的头部），读文件的那一只手是 [`crate::pin::read_picture_batch`]，
//! 译出像素的那一只手是 [`crate::encode`]。
//!
//! # 为什么「换还是新钉」要在这一层判，而不是在窗上判
//!
//! §5.8.4 第 3 步的原话是「更新当前贴图或创建新贴图，具体行为由设置决定」—— 它把选择权给了
//! 一枚设置（[`crate::config::Pin::drop_replaces_current`]），却没说清**一次拖进多张**时那一枚
//! 设置还成不成立。拖两张贴一张是「换」：另一张要么被咽下去（#433 不许静默），要么无处可去。
//! 于是这一层把那句「由设置决定」补成一张穷尽的表，四种情形各有一句说法，而**页、桥、门共读这
//! 一张表**：桥里再写一遍 `if replaces && …` 就是第二个作者（#281），而第二个作者只会先在某些
//! 情形上不一致 —— 偏偏「多张」那一格就是最容易不一致的那一格。
//!
//! # 为什么取回上限立在这一层
//!
//! [`IMAGE_LIMIT`] 是产品的一格（「一张图片多大才算过大」），不是那一只手的一条参数：它跟着
//! PRD 那句「获取失败时，不得用低质量占位图静默替代」走 —— 过大是**失败**，说清它为什么不是
//! 一次成功的取回，是 [`crate::encode`] 与窗都做不到的事。

use std::path::PathBuf;

use crate::link::{self, Web};

/// 一次拖进来的**一句**网页地址，取回那一份图片允许的最大字节数。
///
/// 为什么这一枚比 [`crate::update::FEED_LIMIT`]（一份人写的版本清单）大一档：一份清单是几十个
/// 产品写的几十行字，而一张图片是**内容本身** —— 网页上一张 2560×1440 的截图压成 PNG 是几兆到
/// 几十兆，把上限贴着它写，用户撞到的就是「图片过大」那一枚拒绝，而那正是这一节要 sparingly
/// 用的分支。为什么不是无限：`fetch::bytes` 那一只手超限是失败而非截断，而一张**真的**过大的
/// 图（比如一张几十兆的原始 TIFF）喂进解码器，代价是这台机器上的一次内存抖动。取这一枚数是
/// 一次产品决定，所以它只有这一处（#281）。
pub const IMAGE_LIMIT: usize = 32 * 1024 * 1024;

/// 拖进来的**一格**是什么。三堆各说一件事，因为三件事在屏幕上要说的话**不同**（与
/// [`link::Target`] 四格各说一句同一个理由）：文件要说「读不进」，地址要说「取不回」，
/// 而「这一格压根不是图」是第三种坏 —— 把三者挤成一格，用户只会看到同一句话对三种情况。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// 这台机器上的一份文件。`file:` 那一份百分号编码与盘符形状由桥里那一只手认好递进来
    /// （`qt_bridge::save::local_path` —— 它是这一整个程序里「对话框与拖放交回来的那串 URL 字
    /// 怎么变成一枚路径」的唯一作者），本层不自己切一遍。
    File(PathBuf),
    /// 浏览器那一类、且 host 过得了 [`link`] 那道闸的一句地址。带的是 [`Web`] 本体而不是那串字：
    /// 要取的那一句（[`Web::url`]）与要跟用户说的那一枚域名（[`Web::display_host`]）得从同一次
    /// 解析里出来，否则「说的是 A、取的是 B」这一族坏就有了两个作者。
    Web(Web),
    /// 这一格这一趟做不了，带回来的是**为什么**做不了的那一句（给用户读，不参与任何判决）。
    Refused(String),
}

impl Dropped {
    /// 这一格是不是一句可取的地址。判的**正是** [`link::classify`] 那一道闸 —— 与 §5.6.2 打开
    /// 链接、与 [`crate::update::net_notice`] 要不要提示联网、与 `fetch::bytes` 自己进门时再问
    /// 的那一遍，是同一只手：所以不会出现「提示了要联网，结果一句拒绝说它不是地址」，也不会出现
    /// 「页上看着像地址、取回时才发现不是」。
    ///
    /// `file:` 那一支**不在这里**认：那一串的解码规矩（百分号、盘符、`file://主机/路径`）在 Qt
    /// 那一份对话框与拖放交回来的字串里只由一只手认（`qt_bridge::save::local_path` —— 本机这代
    /// Qt 给 JS 的并不是 `QUrl` 的方法而是整句 URL，P37 量过），本层收到的是它已经解好的
    /// [`PathBuf`]（见 [`Dropped::File`]）。
    pub fn classify(raw: &str) -> Self {
        match link::classify(raw) {
            link::Target::Web(web) if web.openable() => Dropped::Web(web),
            // IDN：`link` 那一份表里写着「不拿同形字替用户点头」，而取回一枚看着像 `paypal` 的
            // 域名指向的字节，比打开它更坏 —— 这里守的是同一条，措辞也只用那一处说过的道理。
            link::Target::Web(web) => Dropped::Refused(format!(
                "那句地址的域名里有非 ASCII 字符（{}），同形字那一族本程序不替用户点头取它。",
                shown(&web.host)
            )),
            link::Target::Other(scheme) => Dropped::Refused(format!(
                "那一格是 {scheme} 那一类地址，本程序只取浏览器那一类（http 与 https）的图片。"
            )),
            link::Target::NotALink => Dropped::Refused(format!(
                "那一格既不是一份文件，也不是一句可以取回的图片地址：{}",
                shown(raw)
            )),
        }
    }
}

/// 那一句要给用户看的话里，原文最多引这么多字。与 [`crate::pin::FILE_PIN_SAID_MAX`] 同一枚
/// 道理：屏幕上那一行是提示，不是日志。
fn shown(raw: &str) -> String {
    const MAX: usize = 120;
    let mut out = String::new();
    for ch in raw.chars().take(MAX) {
        out.push(ch);
    }
    if raw.chars().count() > MAX {
        out.push('…');
    }
    out
}

/// 一次拖进来的那几格分成的**三堆**。各堆保持拖进来的次序：同一批里第 i 格落在第 i 格上，
/// 这一条与 §5.8.3「落位点按挑的顺序排开」是同一个承诺（`pin::cascade_at`），换一种次序就是
/// 用户看不见的那一种「对不上号」。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Piles {
    pub files: Vec<PathBuf>,
    pub web: Vec<Web>,
    pub refused: Vec<String>,
}

impl Piles {
    /// 这一趟有没有**任何**可用的一格。两堆全空就是「拖进来的是文本、是一枚不认识的 scheme」
    /// 那一族 —— 那一族不许当成一次成功的落点（PRD 那句「不得静默替代」在这一支上的形状）。
    pub fn usable(&self) -> usize {
        self.files.len() + self.web.len()
    }
}

/// 把拖进来的那几格归堆，并且**在这一趟就把这批的大小判掉**。
///
/// 收的是 [`Dropped`] 而不是原始串：`file:` 那一支要在桥里认好成 [`PathBuf`] 才递进来（见
/// [`Dropped::File`] 头顶那一句），本层不认识百分号。
///
/// 上限用的是 [`crate::pin::FILE_PIN_MAX`]，与 §5.8.3 那一批对话框挑出来的文件同一枚数：两批要
/// 挡的是同一件事（每一枚成了图的都是名册里的一格、一扇窗、一份纹理），而一句「拖进来的和挑出来
/// 的该不该有不同上界」在产品里没有第二个答案。判在**这里**而不是判在桥里，是因为「第 33 格连读
/// 都不读」这一条得在取回**之前**生效：让 33 句地址各付一次网络的代价、再把它们逐句退回屏幕上那
/// 一行，是最贵的那一种坏法。超出部分归进 `refused`，话由 [`crate::pin::too_many_why`] 那一个
/// 作者说（#281）。
pub fn sort(items: impl IntoIterator<Item = Dropped>) -> Piles {
    let mut out = Piles::default();
    for item in items {
        match item {
            Dropped::File(path) if out.usable() >= crate::pin::FILE_PIN_MAX => {
                out.refused
                    .push(crate::pin::too_many_why(&path.display().to_string(), "拖进来"));
            }
            Dropped::Web(web) if out.usable() >= crate::pin::FILE_PIN_MAX => {
                out.refused.push(crate::pin::too_many_why(
                    &shown(&web.url),
                    "拖进来",
                ));
            }
            Dropped::File(path) => out.files.push(path),
            Dropped::Web(web) => out.web.push(web),
            Dropped::Refused(why) => out.refused.push(why),
        }
    }
    out
}

/// §5.8.4 第 3 步那一句「更新当前贴图或创建新贴图」的两种落点，加上它们都不成立的那一格。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    /// 换掉**这一枚**贴图现在的那一张。
    Replace,
    /// 新钉一枚（或多枚）贴图，现有的那一张不动。
    New,
    /// 哪一枚都不动：这一批里**一格可用的都没有**。
    ///
    /// 为什么这一格不并进 [`Landing::New`]：那两种在屏幕上都算「这一趟落了一次」，而这一趟什么都没
    /// 落。把「不动」说成「另钉一枚」，屏幕上那一行就成了「另钉一枚：这一趟没有装上任何东西」——
    /// 前半句承诺了一次落点、后半句否认它，正是 #453 拦的那件事（两种坏各说各的）。
    Nothing,
}

impl Landing {
    /// 那一枚设置 + 这一批的枚数 → 落点，和**给这一格的那个决定的一句说法**。
    ///
    /// **什么时候问它**：落下那一刻、发起取回之前，问一次，然后把交回的那一枚 `Landing` 带过整趟
    /// —— 不在取回之后再问一遍。那一枚 `count` 数的是「这一批里有几格可用」，而取回之后它只会
    /// 变小：开着那一枚设置、拖进来两句地址、其中一句 404，取回后再问一遍就会从 `New` 翻成
    /// `Replace` —— 屏幕上说的是「各钉一枚新的」，真的做的却是换掉这一枚。判决跟着读数漂是
    /// 最坏的那一种（#261）。
    ///
    /// 四格穷尽：设置关着、设置开着且正好一枚、设置开着却不止一枚、一格也没有。为什么每种都要
    /// 带一句：这一趟结束后屏幕上那一行说的是「这一趟做了什么」，而「换这一枚」与「又钉了三枚」
    /// 是两件事，「设置明明开着却没换」更是那一种用户会反复试的第三种 —— 它必须有自己的话。
    pub fn plan(replaces: bool, count: usize) -> (Landing, &'static str) {
        if count == 0 {
            return (
                Landing::Nothing,
                "那一格里没有可用的图片：不换现有的那一枚，也不新钉一枚。",
            );
        }
        if !replaces {
            return (
                Landing::New,
                "那枚设置没开，拖进来的每一张都另钉一枚新贴图，原本这一枚不动。",
            );
        }
        if count == 1 {
            return (
                Landing::Replace,
                "那枚设置开着，拖进来的那一张换到原本这一枚贴图上。",
            );
        }
        (
            Landing::New,
            "那枚设置开着，但一次只换得起一枚，换一枚就得把其余的咽下去：这一批还是各钉一枚新贴图。",
        )
    }

    /// 屏幕上那一行里指认落点的两个字。桥拼那一句结果时读它，不自己再写一遍「换/新钉」——
    /// 「这一趟落到哪一格」在进程里只有一个名字可以说。
    pub fn word(&self) -> &'static str {
        match self {
            Landing::Replace => "换这一枚",
            Landing::New => "另钉一枚",
            Landing::Nothing => "不动",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 归堆只分三堆、且**不重排**：一次拖三格，第 i 格说第 i 格的话。这一格钉的是次序，因为
    /// 次序在窗上是要被对上的（阶梯落位、逐行说明），而「保持原序」这件事在代码里最容易在
    /// 某天为了「先取文件更省一次网络」被改掉。
    #[test]
    fn the_piles_keep_the_order_they_came_in() {
        let piles = sort([
            Dropped::classify("https://img.example.invalid/a.png"),
            Dropped::File(PathBuf::from("C:/x/b.png")),
            Dropped::classify("mailto:nobody@example.invalid"),
            Dropped::classify("https://img.example.invalid/c.png"),
            Dropped::File(PathBuf::from("C:/x/d.png")),
        ]);
        assert_eq!(piles.web.len(), 2, "{piles:?}");
        assert_eq!(piles.files.len(), 2, "{piles:?}");
        assert_eq!(piles.refused.len(), 1, "{piles:?}");
        assert_eq!(piles.web[0].url, "https://img.example.invalid/a.png");
        assert_eq!(piles.web[1].url, "https://img.example.invalid/c.png");
        assert_eq!(piles.files[0], PathBuf::from("C:/x/b.png"));
        assert_eq!(piles.files[1], PathBuf::from("C:/x/d.png"));
        assert_eq!(piles.usable(), 4, "可用的是四格，不是五格");
    }

    /// 三格各说自己的事（#453）：不是地址、不是浏览器那一类、同形字域名 —— 把三者挤成一句话，
    /// 用户就没法知道该改哪一头。判据共用 [`crate::link`] 那道闸，所以不会出现「这里说是地址、
    /// `fetch` 那里说不是」。
    #[test]
    fn the_three_kinds_of_not_a_picture_say_three_different_things() {
        let not = match Dropped::classify("只是一段文字") {
            Dropped::Refused(s) => s,
            other => panic!("一段文字不该被认成可用的一格：{other:?}"),
        };
        let mail = match Dropped::classify("mailto:nobody@example.invalid") {
            Dropped::Refused(s) => s,
            other => panic!("{other:?}"),
        };
        let idn = match Dropped::classify("https://exämple.invalid/a.png") {
            Dropped::Refused(s) => s,
            other => panic!("同形字那一支不该被取：{other:?}"),
        };
        assert!(not.contains("既不是一份文件"), "{not}");
        assert!(mail.contains("mailto"), "{mail}");
        assert!(mail.contains("http"), "{mail}");
        assert!(idn.contains("非 ASCII"), "{idn}");
        assert_ne!(not, mail);
        assert_ne!(mail, idn);
        // 好的一句：拿回的是**那一次解析**的 `Web`，取的那一句与说的那枚域名同出一格。
        let good = match Dropped::classify("https://user@pass@img.example.invalid/a.png") {
            Dropped::Web(web) => web,
            other => panic!("{other:?}"),
        };
        assert_eq!(good.host, "img.example.invalid");
        assert_eq!(good.display_host(), "img.example.invalid");
    }

    /// 一批 40 格：只有 32 格进得了堆，另外 8 格各有一句「再挑少一点」。这一格钉的是**判在哪一
    /// 步**：上限归归堆这一步管，桥里就不必替第 33 句地址付一次网络的代价；而超出的一枚也不许
    /// 静默（#433），每一枚都得在 `refused` 里有自己的那句。
    #[test]
    fn a_batch_bigger_than_a_pinning_is_refused_before_anything_is_fetched() {
        let items: Vec<Dropped> = (0..40)
            .map(|i| Dropped::classify(&format!("https://img.example.invalid/{i}.png")))
            .collect();
        let piles = sort(items);
        assert_eq!(piles.usable(), crate::pin::FILE_PIN_MAX, "{piles:?}");
        assert_eq!(piles.refused.len(), 40 - crate::pin::FILE_PIN_MAX, "{piles:?}");
        for why in &piles.refused {
            assert!(why.contains("一次最多贴"), "{why}");
            assert!(why.contains("再挑少一点"), "{why}");
        }
        // 混着拖按**总数**判：文件那 10 格先占掉 10 格名额，地址那 30 格里只有剩下的 22 格进得来
        // —— 两批共用一枚上界，所以谁先来谁先占，不为「文件更便宜」开第二条规矩。
        let mut mixed: Vec<Dropped> = (0..10)
            .map(|i| Dropped::File(PathBuf::from(format!("C:/x/{i}.png"))))
            .collect();
        mixed.extend(
            (0..30)
                .map(|i| Dropped::classify(&format!("https://img.example.invalid/m{i}.png"))),
        );
        let piles = sort(mixed);
        assert_eq!(piles.files.len(), 10, "{piles:?}");
        assert_eq!(piles.web.len(), crate::pin::FILE_PIN_MAX - 10, "{piles:?}");
        assert_eq!(piles.refused.len(), 8, "{piles:?}");
    }

    /// 原文过长的那一格不配把屏幕上那一行拉成一段日志。截，是 [`shown`] 一只手的事。
    #[test]
    fn a_long_piece_of_text_is_quoted_short() {
        let long = "x".repeat(400);
        let why = match Dropped::classify(&long) {
            Dropped::Refused(s) => s,
            other => panic!("{other:?}"),
        };
        assert!(why.chars().count() < 200, "{why}");
        assert!(why.contains('…'), "截了却没有说截了：{why}");
        assert!(!why.contains(&"x".repeat(121)));
    }

    /// 那一枚设置 × 这一批的枚数，四种组合各有一句、且落点各自对得上：这一张表就是 §5.8.4 第
    /// 3 步那句「具体行为由设置决定」的**全部**展开，页与桥不再各判一遍。
    #[test]
    fn every_setting_and_count_combination_lands_somewhere_explained() {
        let off = Landing::plan(false, 1);
        assert_eq!(off.0, Landing::New);
        assert!(off.1.contains("另钉"), "{}", off.1);
        let on_one = Landing::plan(true, 1);
        assert_eq!(on_one.0, Landing::Replace);
        assert!(on_one.1.contains("换到原本这一枚"), "{}", on_one.1);
        // 「设置开着」+「一次多张」不许静默丢下其余的：换成各钉一枚，并把这一格为什么这样
        // 决定说出来。这一枚是这一节唯一「设置说了但没全听」的一格，所以它必须有话。
        let on_many = Landing::plan(true, 3);
        assert_eq!(on_many.0, Landing::New);
        assert!(on_many.1.contains("咽下去"), "{}", on_many.1);
        let none = Landing::plan(true, 0);
        assert!(none.1.contains("没有可用的图片"), "{}", none.1);
        assert_eq!(none.0, Landing::Nothing, "一格也没有时不许冒充钉了一枚");
        assert_eq!(Landing::plan(false, 0).1, none.1, "一格也没有时那枚设置不该改变说法");
        // 四句互不相同（#453），三枚落点各有一个名字。
        let words: Vec<&str> = vec![off.1, on_one.1, on_many.1, none.1];
        for i in 0..words.len() {
            for j in i + 1..words.len() {
                assert_ne!(words[i], words[j]);
            }
        }
        assert_eq!(Landing::Replace.word(), "换这一枚");
        assert_eq!(Landing::New.word(), "另钉一枚");
        assert_eq!(Landing::Nothing.word(), "不动");
    }

    /// 取回上限是产品的一格：它必须比一份**清单**大得多（图片不是清单），又不能大到把整台机器的
    /// 内存当赌注。这一格钉的是数量级，写死的数一旦改小到贴着清单，这里就红。
    #[test]
    fn the_image_ceiling_is_its_own_size_class() {
        assert!(IMAGE_LIMIT > crate::update::FEED_LIMIT * 16);
        assert!(IMAGE_LIMIT <= 128 * 1024 * 1024);
    }
}
