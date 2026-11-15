//! §5.21.2 帮助文档 第四行「复杂功能可提供首次使用引导。」—— 引导句的家，
//! 与「只递一次」那枚状态的家。
//!
//! 前三枚递的都是**一行**上的字：工具栏的 tooltip（第一枚）、快捷键那一行的功能说明
//! （第二枚，P158）、命令行那一行的文档入口（第三枚，P159）。第四枚问的是另一件事：
//! 一页需要人先看懂才用得起来的时候，那句"先看懂"由谁在什么时候说。本轮之前整个仓库里
//! 没有一处答过它 —— `首次` 与 `引导` 两个词在产品码里一枚也不出现（只有
//! `settings.rs` 里那枚同名的单测，讲的是配置文件的头一趟），所以这一档不是给已有的东西
//! 补尺子，它是那件东西本身。
//!
//! ## 两件事分开住
//!
//! * **哪一页算复杂、复杂那页说什么** —— [`ENTRIES`]，一张表，键名是名单里那几类
//!   的**类名**（[`crate::settings_view::CATEGORIES`] 是唯一的名单，这里不抄第二份）；
//! * **这一页递过没有** —— [`State`]，落在 [`path`] 指的那份文件里。
//!
//! 状态**不**住 `config.toml`。这不是省事：§9.1 第十五条要求量具不写用户那份档，而引导
//! 状态是这一轮唯一**必须真写一次盘**才能证出「第二次不递」的东西 —— 把它塞进 config，
//! 门要么写用户的档，要么这一枚判据永远只能靠内存里的一次性布尔。分开之后门钉
//! [`ENV_DIR`] 那一枚，写的量具自己的草稿纸，用户的 `config.toml` 逐字节不动。
//!
//! ## 「首次」的两个半
//!
//! 一次运行只能读到**一个时刻**，所以「递过」这一件事必须由盘上那份状态决定，而不是由
//! 进程决定：[`line_for`] 是一枚纯函数，同一份 `State` 问多少次答多少次，`State::default()`
//! （= 还没收过）永远答得出句子。--selftest 那两格因此能在不碰任何文件的地方把「第一趟递、
//! 收下之后不递、换一份全新状态又递」三件事问完；窗那一侧的读数（`--settings-window` 的
//! 逐页引导长度与那一腿收下前后的两串）量的是同一份纯函数**真的被界面读到过**。
//!
//! 「按了「{ACK_WORD}」才算收到」是有意的：窗一开就把它看见的东西记成"已经讲过了"，等于
//! 替用户决定他读没读。所以下一次打开同一页，那句话还在。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};

use falcon_core::config::Config;

/// 页上那一块抬头：它得让人认出这是引导，而不是页上又长了一行设置。
pub const GREETING: &str = "首次使用引导";

/// 收下它的那枚钮上的字。与 [`crate::settings_view::UNWIRED_MARK`] 同处一处家：窗要画这两个
/// 字、`--selftest` 那一格要问「这句里有没有它」，写第二遍就是第二份措辞（#281）。
pub const ACK_WORD: &str = "知道了";

/// 那份状态在盘上的名字。
pub const SEEN_FILE: &str = "guide_seen.txt";

/// 量具钉落点用的环境变量，与 `FALCONSHOT_SETTINGS_DOOR_DIR` / `FALCONSHOT_AUTOSAVE_HIST_DIR`
/// 同一族：给了就往那里写，没给才用 `advanced.data_dir` 那一位主人答的那枚目录。
pub const ENV_DIR: &str = "FALCONSHOT_GUIDE_DIR";

/// 复杂页的名单，每一枚后面跟的是那一页**第一次打开时递出去的那一句**。
///
/// 为什么是这六枚而不是整份名单：「复杂」在这一轮有一条能说出口判据 —— **那一页上至少有一项
/// 设置，改了之后要么不会立刻生效、要么用户看不出自己按对了**。`常规`/`截图`/`输出`/`外观`/
/// `关于与更新`答不出这一句（那一页按下去就在当前一格里改动可见），所以不递；读的人若不同意
/// 这一条读法，改这一张表就行，不必改任何状态判据。
///
/// 引导面对的是第一次使用功能的人，因此只说设置页上看得到的名称与效果，不显示配置文件里的
/// 点分键。内部键是否意外混进文案由本文件的单测逐枚对照默认键名单。
pub const ENTRIES: [(&str, &str); 6] = [
    (
        "快捷键",
        "每一行对应一项动作：点选一行，在输入框中按下组合键，再保存。暂时无法执行的动作会在行尾\
         标出「未接线」，不会占用系统快捷键。",
    ),
    (
        "热区",
        "先打开「启用热区」，再为四个屏幕角分别选择动作。把鼠标停在角落达到设定时间就会触发，\
         不需要点击；若启用了 Ctrl 或 Shift 限制，停留时还要同时按住对应按键。",
    ),
    (
        "标注",
        "标注会画在已经截取的图片上。你可以选择进入标注时默认使用的工具，并调整线条粗细、文字\
         字号和最多可撤销的步数。",
    ),
    (
        "贴图",
        "贴图会把图片留在屏幕上。这里可以决定新贴图是否默认置顶、能否用滚轮缩放，以及独显时\
         其他贴图变暗的程度；文字卡片的宽度、字体和背景也在这一页调整。",
    ),
    (
        "历史记录",
        "历史记录会保存截图信息，并可选择保留完整原图。你可以限制保留数量、天数和占用空间；\
         自动清理时可跳过锁定项与收藏项。",
    ),
    (
        "高级与命令行",
        "「允许命令行控制」决定应用是否接受命令行参数，修改后需要重启才生效，行尾会显示\
         「需重启」；日志级别等即时设置保存后立即生效。",
    ),
];

/// 第 `which` 类在名单里的类名；越界答 `None`（窗递回来的类号可能是任何数）。
pub fn name(which: usize) -> Option<&'static str> {
    crate::settings_view::CATEGORIES.get(which).map(|(n, _)| *n)
}

/// 复杂页的类号，升序。判据与窗都从这里数，不各数一遍。
pub fn complex_pages() -> Vec<usize> {
    (0..crate::settings_view::CATEGORIES.len())
        .filter(|i| body(*i).is_some())
        .collect()
}

/// 第 `which` 类是不是复杂页；是就交回它那一句。
///
/// 认的是**类名**，所以换掉 `CATEGORIES` 里的顺序不会让某页的引导跑到邻居那一页上。
pub fn body(which: usize) -> Option<&'static str> {
    let page = name(which)?;
    ENTRIES.iter().find(|(n, _)| *n == page).map(|(_, b)| *b)
}

/// 引导句里像配置键的点分词。如今引导不应点名任何内部键，所以不再只检查反引号：裸写或放进
/// 反引号都能被发现。网址与状态文件名是精确例外；它们含点但不是配置键。
pub fn dotted(body: &str) -> Vec<String> {
    body.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-')))
        .map(|token| token.trim_matches('.'))
        .filter(|token| {
            token.contains('.')
                && *token != SEEN_FILE
                && !token.starts_with("http://")
                && !token.starts_with("https://")
                && token.split('.').all(|part| {
                    !part.is_empty()
                        && part
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                })
        })
        .map(str::to_string)
        .collect()
}

/// 已经收下过引导的那些页（类名）。存**名字**而不是存类号：类号是 §5.20.1 那张表的行序，
/// 文档改了次序就会把「收下过」记到邻居那一页头上。
#[derive(Clone, Debug, Default)]
pub struct State {
    names: BTreeSet<String>,
}

impl State {
    pub fn has(&self, which: usize) -> bool {
        name(which).is_some_and(|n| self.names.contains(n))
    }

    /// 收下这一页。返回**这一趟有没有改变它**：已经收过的那一页第二次收，不算一次改变，
    /// 窗上那一格因此能把「按了两回」与「按了一回」分开印。
    pub fn mark(&mut self, which: usize) -> bool {
        match name(which) {
            Some(n) => self.names.insert(n.to_string()),
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// 一页一行。结尾那枚换行是有对方的：`decode` 按行切，而一份写了三行的文件在盘上
    /// 就是三行 —— 少了最后那一枚，下一次读回来的行数与这一趟写出去的行数就不是一件事。
    pub fn encode(&self) -> String {
        let mut out = String::new();
        for n in &self.names {
            out.push_str(n);
            out.push('\n');
        }
        out
    }

    pub fn decode(text: &str) -> State {
        let mut names = BTreeSet::new();
        for line in text.lines() {
            let t = line.trim();
            if !t.is_empty() {
                names.insert(t.to_string());
            }
        }
        State { names }
    }

    /// 名单里那些名字此刻**不在这份状态里**的枚数 —— 也就是「这一趟还欠着几句引导」。
    ///
    /// 这一格要的是与 [`complex_pages`] 不同的一个问题：`complex_pages().len()` 说这一轮认下
    /// 了几枚复杂页，这一枚说的是这份状态里还有几枚没被收过。合成就变成把两件事量成一件。
    pub fn pending(&self) -> usize {
        complex_pages().iter().filter(|i| !self.has(**i)).count()
    }
}

/// 那一页此刻该递出去的那一句；不递就是空串（窗按空串不画那一块）。
///
/// 纯函数，一次文件也不读：页上每翻一类都问一次，让那一趟去开一次盘等于把「用户翻了一页」
/// 写成「用户动了一次盘」。状态由调用方拿着（[`current`] 取，[`ack`] 存）。
pub fn line_for(state: &State, which: usize) -> String {
    match body(which) {
        Some(b) if !state.has(which) => {
            format!("{GREETING}｜{b}（按「{ACK_WORD}」之后这一页以后不再递）")
        }
        _ => String::new(),
    }
}

/// 页上「知道了」那枚钮旁边该写什么：这一页此刻在名单里、还是已经收过、还是压根不是复杂页。
///
/// 三件事分开答是必须的：窗把「这一页没有引导」画成「按了也没用」，用户读到的就是「我按错了」
/// —— 而这一页压根不欠他一句引导。
pub fn why_empty(state: &State, which: usize) -> &'static str {
    if body(which).is_none() {
        "这一页不在复杂页的名单里"
    } else if state.has(which) {
        "这一页的引导已经收过了"
    } else {
        "这一页还欠着一句引导"
    }
}

// ─────────────────────────────────────────────── 盘上那一份

/// 这份状态住哪：[`ENV_DIR`] 给了就用它（门与级档各用各的那一枚目录），否则问
/// `advanced.data_dir` 那一位主人 —— 与 `config.toml`、`history.db` 同一个落点，不另立门户。
pub fn path() -> PathBuf {
    let dir = match std::env::var_os(ENV_DIR) {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => crate::settings::with(|s| falcon_core::config::data_dir(&s.config)),
    };
    dir.join(SEEN_FILE)
}

/// 本进程往那份文件伸过几回手。`--selftest` 那一格问的就是这个数必须是 0：量具的默认那一趟
/// 一次也不该动盘（§9.1 第十五条）。
static ATTEMPTS: AtomicU32 = AtomicU32::new(0);

/// 本进程手里那一份状态。`None` = 还没从盘上取过。
///
/// 缓存而不是每次读盘：翻一页开一次文件，读的开销换不来任何读数。代价是**别的进程**在这中间
/// 收下的一页，本进程看不见 —— 这一轮的引导只有这一扇窗会写，而它写的就是它自己递出去的那一句，
/// 所以那一件坏事在这一版里没有发生的通路（下一版若把托盘也接上，这一句就得重看）。
static CACHE: Mutex<Option<State>> = Mutex::new(None);

fn with_cache<R>(f: impl FnOnce(&mut Option<State>) -> R) -> R {
    let mut held = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut held)
}

/// 读盘上的那一份（没有文件 = 一份空状态，这不是坏，这是新用户）。**只读，一次也不写。**
pub fn current() -> State {
    with_cache(|slot| {
        if slot.is_none() {
            let text = std::fs::read_to_string(path()).unwrap_or_default();
            *slot = Some(State::decode(&text));
        }
        slot.clone().unwrap_or_default()
    })
}

/// 页上那一枚真的被按下时的落点：先记在进程里，再落盘，交回的那一句把两件事各自的结果说完。
///
/// 落盘失败**不**回滚内存里那一份：用户已经按了「知道了」，下一次翻这一页还看见同一句，比
/// 「按了没反应但盘上记了」更难解释。失败印在这一句里，读档的人因此分得清哪一趟是写了的。
pub fn ack(which: usize) -> String {
    let page = match name(which) {
        Some(n) => n,
        None => return format!("[引导] 第 {which} 类压根不在名单里：没有可收下的引导"),
    };
    if body(which).is_none() {
        return format!("[引导] {page}：这一页不在复杂页的名单里，收下它等于什么都没收");
    }
    let first = with_cache(|slot| {
        let mut st = slot
            .clone()
            .unwrap_or_else(|| State::decode(&read_at(&path())));
        let fresh = st.mark(which);
        *slot = Some(st.clone());
        (st, fresh)
    });
    let (state, fresh) = first;
    let target = path();
    let wrote = write_at(&target, &state.encode());
    let (said, wrote_ms) = match wrote {
        Ok(ms) => ("已落盘".to_string(), ms),
        Err(why) => (format!("落盘失败：{why}"), 0u128),
    };
    format!(
        "[引导] {page}：{}；{said} {} µs；落点={}；这一页以后{}再递",
        if fresh {
            "收下（这一趟是新收的一枚）"
        } else {
            "本来就已经收过（不是新收的一枚）"
        },
        wrote_ms,
        target.display(),
        if state.has(which) { "不" } else { "还" }
    )
}

/// 一份文件现在写着什么；不存在 = 空串（新用户），读不开 = 空串并把那句坏留给调用方去问。
fn read_at(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// 写那份文件，交回花掉的微秒数。先写同名 `.tmp` 再改名：一份读到一半被截断的状态文件会让
/// 下一次「这一页收过没有」答成半句 —— 那比不写更坏。
fn write_at(path: &Path, text: &str) -> Result<u128, String> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let name = path
        .file_name()
        .ok_or_else(|| "这一枚路径连文件名都没有".to_string())?;
    let tmp = path.with_file_name(format!("{}.tmp", name.to_string_lossy()));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(started.elapsed().as_micros())
}

/// 本进程动过那份文件几回（每一次**尝试**算一回，写了没成也算 —— 这一格问的是「这一趟有没有
/// 伸过手到盘上去」，不是「伸成了几次」）。
pub fn attempts() -> u32 {
    ATTEMPTS.load(Ordering::SeqCst)
}

/// 那份文件此刻在不在、多大。窗那一格与 `--selftest` 那一格都问它，两处不各拼一次路径。
pub fn file_summary() -> String {
    let p = path();
    match std::fs::metadata(&p) {
        Ok(m) => format!(
            "{} 存在 bytes={}；收下过={} 枚 还欠={} 枚 本趟动过盘={} 回",
            p.display(),
            m.len(),
            current().len(),
            current().pending(),
            attempts()
        ),
        Err(_) => format!(
            "{} 不存在（还没人收下过任何一句）；收下过={} 枚 还欠={} 枚 本趟动过盘={} 回",
            p.display(),
            current().len(),
            current().pending(),
            attempts()
        ),
    }
}

/// 交给 `--selftest` 与单测的那一份**默认配置**的键名单：引导句里点名的每一枚键都得在这上面。
///
/// 取默认而不是取本进程手里那一份，是因为引导说的是「这一页管哪些事」，那是档的结构、不是
/// 这台机器此刻的数。用户把 `history.max_items` 改成 0，那一枚键仍然在档里、那句话仍然该提它。
pub fn default_keys() -> Vec<String> {
    Config::default()
        .flat()
        .unwrap_or_default()
        .into_keys()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 名单里每一枚类名都真在 §5.20.1 那张表上：抄错一个字，那一页就永远不递引导，而没有任何
    /// 一处会报错 —— 这类坏只有名单自己问得出来。
    #[test]
    fn every_entry_names_a_real_category() {
        for (page, line) in &ENTRIES {
            let which = crate::settings_view::index_of(page);
            assert!(which.is_some(), "{page} 不在名单里");
            assert!(!line.is_empty(), "{page} 的引导句是空的");
            assert!(!line.contains('\n'), "{page} 的引导句换行了");
            assert_eq!(body(which.unwrap()), Some(*line));
        }
        // 一枚不重。
        let mut seen: Vec<&str> = Vec::new();
        for (page, _) in &ENTRIES {
            assert!(!seen.contains(page), "{page} 出现两次");
            seen.push(page);
        }
        assert_eq!(complex_pages().len(), ENTRIES.len());
    }

    /// 首次使用引导只说用户在页面上看得到的名称，不显示任何内部点分键。默认键逐枚检查，另外用
    /// `dotted` 防住不在默认名单里的新内部键；网址和状态文件名只按精确例外放行。
    #[test]
    fn guide_lines_use_only_user_language() {
        let keys = default_keys();
        for (page, body) in &ENTRIES {
            for key in &keys {
                assert!(
                    !body.contains(key),
                    "{page} 的引导露出了内部键 {key}：{body}"
                );
            }
            assert!(dotted(body).is_empty(), "{page} 的引导含点分内部词：{body}");
        }
        let corner = body(crate::settings_view::CORNER).expect("热区引导缺失");
        assert!(corner.contains("启用热区"), "{corner}");
        assert!(
            corner.contains("Ctrl") && corner.contains("Shift"),
            "{corner}"
        );
        assert!(!corner.contains("hot_corner"), "{corner}");

        assert!(dotted("详见 https://example.com/help 与 guide_seen.txt").is_empty());
        assert_eq!(
            dotted("不要显示 hot_corner.enabled"),
            ["hot_corner.enabled"]
        );
    }

    /// 「只递一次」的四张脸，全在内存里问：默认那份空状态递、收下之后不递、换一份全新状态又递、
    /// 名单外的页压根不递。
    #[test]
    fn guide_hands_once_per_page() {
        let fresh = State::default();
        let complex = complex_pages();
        assert!(!complex.is_empty());
        for i in complex.iter() {
            let line = line_for(&fresh, *i);
            assert!(line.starts_with(GREETING), "第 {i} 类那一句没有抬头");
            assert!(line.contains(ACK_WORD), "第 {i} 类那一句没说怎么收");
        }
        // 名单外那一页压根不递。
        for i in 0..crate::settings_view::CATEGORIES.len() {
            if !complex.contains(&i) {
                assert_eq!(line_for(&fresh, i), "", "第 {i} 类不是复杂页却递了");
            }
        }
        // 收下之后：这一页不再递，别页照旧。
        let mut kept = fresh.clone();
        let first = complex[0];
        assert!(kept.mark(first), "第一回收下应当算一次改变");
        assert!(!kept.mark(first), "第二回不该再算一次改变");
        assert_eq!(line_for(&kept, first), "");
        assert!(line_for(&kept, complex[1]).starts_with(GREETING));
        // 换成另一份全新状态又递 ⇒ 「一次」住在状态里，不是住在进程里。
        assert!(line_for(&State::default(), first).starts_with(GREETING));
        assert_eq!(kept.pending(), complex.len() - 1);
        assert_eq!(fresh.pending(), complex.len());
    }

    /// 那份文件写得回来、也读得回来；一页一行，读回来的枚数与写出去的一致。
    #[test]
    fn seen_state_round_trips_through_text() {
        let mut st = State::default();
        for i in complex_pages() {
            st.mark(i);
        }
        let text = st.encode();
        assert_eq!(text.lines().count(), st.len());
        assert!(text.ends_with('\n'), "最后一行没有换行就少一枚");
        let back = State::decode(&text);
        assert_eq!(back.len(), st.len());
        for i in complex_pages() {
            assert!(back.has(i));
        }
        // 空串是一份空状态：新用户那一份文件压根不存在，读回来的不该是「收过一句」。
        assert_eq!(State::decode("").len(), 0);
        assert_eq!(State::decode("\n \n").len(), 0);
    }

    /// 越界的类号哪儿也不能崩：窗递回来的是一枚 `i32`，任何数都可能进来。
    #[test]
    fn impossible_index_answers_nothing() {
        assert!(name(crate::settings_view::CATEGORIES.len() + 7).is_none());
        assert!(body(usize::MAX).is_none());
        assert_eq!(line_for(&State::default(), usize::MAX), "");
        assert_eq!(
            why_empty(&State::default(), usize::MAX),
            "这一页不在复杂页的名单里"
        );
    }
}
