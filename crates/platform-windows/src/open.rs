//! 本模块只有**一只手在碰 shell**，两只不同的事从它走：
//!
//! - §5.6.2 步骤 5 的那一只手：把一枚**已经向用户显示过完整域名**的链接交给系统默认的浏览器
//!   （[`browser`]）；
//! - §5.5.10 步骤 3 的那一只手：把一枚图片作为**一条参数**交给一枚程序（[`dispatch`]）——
//!   「哪一臂、目标在不在、参数怎么拼」都不在这里判，那一处写在
//!   [`falcon_core::sendto`]。
//!
//! 走 `ShellExecuteW(…, "open", …)` 而不是 `start` / `cmd.exe /C start`：后者要把这一句
//! 交回给一套命令行解析规则，而二维码里的字符是**外来数据** —— `&`、引号、`^` 在 cmd 那一头
//! 各自有含义，一条命令行表达不了一句用户可能扫到的地址。`ShellExecuteW` 把 URL 当数据交出去，
//! 一次解析都没有（与 [`crate::reveal`] 避开 `explorer.exe /select` 同一个理由）。
//!
//! 为什么这里还要**再判一遍**是不是 http(s)：`ShellExecuteW` 的 `open` 动词对一枚路径是「按
//! 关联程序执行」，对 `file:` 是「开那个文件」，也就是说这一只手天然能拿去过一层它自己的门槛。
//! 判它的规则只有一份 —— [`falcon_core::link`]，本模块只**引用**它，不自己再切一遍字符串
//! （自己切一遍就是第二条会各自漂移的域名解析路，而这条路的终点是一枚真浏览器）。

/// [`browser`] 的答。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// 不是 http(s)，或者是 IDN：这一句是本模块**不**开的那些形态，而它由 [`falcon_core::link`]
    /// 判定 —— 界面那一只钮灰掉的理由与这里的理由必须是同一句原话，不能各写一遍。
    #[error("这一句不是可以交给浏览器的链接：{0}")]
    Refused(String),
    /// shell 回了 `SE_ERR_*` 那一组码里的一枚。带回来的是那枚码的名字，不是编出来的解释。
    ///
    /// 这句话两只手共用（[`browser`] 与 [`dispatch`]）：同一枚 `SE_ERR_FNF` 不该因为调用者
    /// 是浏览器还是目标程序而说出两句话来。
    #[error("shell 不肯收下这一次请求：{0}")]
    Shell(String),
    #[error("这一台不是 Windows，没有可以打开链接的 shell")]
    Unsupported,
}

/// 一枚 NUL 结尾的 UTF-16 串。`ShellExecuteW` 的四个字符串参数都是这个形状，而本模块两只手
/// 用的编码没有第二套写法 —— 编码规则各写一遍，漂移的那一处通常正好是特殊字符。
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// `SE_ERR_*` 那组码的名字。
///
/// 这条 API 的返回形状是历史遗留的：`HINSTANCE` 里塞的是「成功 = 一个真的实例句柄 /
/// 失败 = 一枚 ≤ 32 的 SE_ERR_ 码」，既不是 HRESULT 也不置 last error。判据是数值大小，
/// 因此下面这些码只有名字可用，没有 `FormatMessage` 可问。
#[cfg(windows)]
fn shell_code(value: isize) -> String {
    use windows::Win32::UI::Shell::{
        SE_ERR_ACCESSDENIED, SE_ERR_ASSOCINCOMPLETE, SE_ERR_DDEBUSY, SE_ERR_DDEFAIL,
        SE_ERR_DDETIMEOUT, SE_ERR_DLLNOTFOUND, SE_ERR_FNF, SE_ERR_NOASSOC, SE_ERR_OOM, SE_ERR_PNF,
        SE_ERR_SHARE,
    };

    match value as u32 {
        SE_ERR_FNF => "SE_ERR_FNF（找不到文件）".to_string(),
        SE_ERR_PNF => "SE_ERR_PNF（找不到路径）".to_string(),
        SE_ERR_ACCESSDENIED => "SE_ERR_ACCESSDENIED（ access 被拒）".to_string(),
        SE_ERR_OOM => "SE_ERR_OOM（内存不够）".to_string(),
        SE_ERR_SHARE => "SE_ERR_SHARE（共享冲突）".to_string(),
        SE_ERR_ASSOCINCOMPLETE => "SE_ERR_ASSOCINCOMPLETE（关联不完整）".to_string(),
        SE_ERR_DDETIMEOUT => "SE_ERR_DDETIMEOUT".to_string(),
        SE_ERR_DDEFAIL => "SE_ERR_DDEFAIL".to_string(),
        SE_ERR_DDEBUSY => "SE_ERR_DDEBUSY".to_string(),
        SE_ERR_NOASSOC => "SE_ERR_NOASSOC（这台机器上没有能开 http(s) 的关联程序）".to_string(),
        SE_ERR_DLLNOTFOUND => "SE_ERR_DLLNOTFOUND".to_string(),
        other => format!("SE_ERR_{other}"),
    }
}

/// 交给默认浏览器。`Ok(())` 只到 shell 收下这句请求为止：浏览器归**它自己**那个进程管，
/// 本进程举不到那个窗，也就没有资格说页面已经打开。
///
/// 正面那一臂**不在单测里**：一次真的 `browser("https://…")` 会在用户的桌面上开出一枚标签页，
/// 而那是本轮唯一能拦的事 —— 单测跑的是这台机器，跑完关不掉那一扇。它由手测矩阵那一格覆盖。
#[cfg(windows)]
pub fn browser(url: &str) -> Result<(), OpenError> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let web = match falcon_core::link::classify(url) {
        falcon_core::link::Target::Web(web) if web.openable() => web,
        falcon_core::link::Target::Web(web) => {
            return Err(OpenError::Refused(format!(
                "{}：域名含非 ASCII 字符，本轮不代用户点头",
                web.display_host()
            )))
        }
        falcon_core::link::Target::Other(scheme) => {
            return Err(OpenError::Refused(format!(
                "scheme 是 {scheme}，不是浏览器那一类"
            )))
        }
        falcon_core::link::Target::NotALink => {
            return Err(OpenError::Refused("读不出主机名".to_string()))
        }
    };

    // 三个 NUL 结尾的 UTF-16 串：动词、目标、以及空的工作目录与参数（`PCWSTR::null()` 让 shell
    // 自己按关联程序的决定走，我们不替它补一个目录）。
    let verb = wide("open");
    let target = wide(&web.url);

    let opened = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(verb.as_ptr()),
            PCWSTR::from_raw(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let value = opened.0 as isize;
    if value > 32 {
        return Ok(());
    }
    Err(OpenError::Shell(shell_code(value)))
}

/// §5.5.10 步骤 3 的那一只手：把一枚程序连同**一条已经拼好**的参数交给 shell。
///
/// 走 `ShellExecuteW` 与 [`browser`] 同一只 API，同一个理由：不经过 `start` / `cmd.exe /C`
/// 那一层命令行解析。参数在这里是**原样递出去**的成品串 —— 加不加引号、路径里带引号时递不递
/// 是 [`falcon_core::sendto::quote`] 的判断，本模块只搬运它；两处各拼一遍就会有一处漏掉
/// 那枚空格。
///
/// 目标程序由调用方给，而「它在不在这台机器上」也是调用方判过的（[`falcon_core::sendto::plan`]
/// 的 `Repath` 那一臂就是为这一问准备的）—— 所以这里没有第二道存在性检查：shell 那一头还
/// 有 App Paths、还有关联表，我们那一道按 PATH 找不到的东西它未必找不到，而两道判据一旦
/// 不一致，用户看到的提示就成了一句谎话。
///
/// `Ok(())` 同样只到 shell 收下请求为止：对面那个进程归**它自己**管，本进程举不到那个窗，
/// 也就没有资格说图已经被人接过去。
#[cfg(windows)]
pub fn dispatch(exe: &str, parameters: &str) -> Result<(), OpenError> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb = wide("open");
    let target = wide(exe);
    let args = wide(parameters);

    let opened = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(verb.as_ptr()),
            PCWSTR::from_raw(target.as_ptr()),
            PCWSTR::from_raw(args.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let value = opened.0 as isize;
    if value > 32 {
        return Ok(());
    }
    Err(OpenError::Shell(shell_code(value)))
}

#[cfg(not(windows))]
pub fn browser(_url: &str) -> Result<(), OpenError> {
    Err(OpenError::Unsupported)
}

#[cfg(not(windows))]
pub fn dispatch(_exe: &str, _parameters: &str) -> Result<(), OpenError> {
    Err(OpenError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 反面那一臂**可以**测，而且必须测：这一族的承诺是「过不了 `link` 那道判的那一句，一次
    /// shell 调用都不会发」。正面对照探针在 Windows 上另测（见下）。
    #[test]
    fn a_non_browser_scheme_never_reaches_the_shell() {
        for text in [
            "file:///C:/Windows/system32/cmd.exe",
            "javascript:alert(1)",
            "mailto:someone@example.com",
            "cmd.exe",
            "",
            "notion://x",
        ] {
            let err = browser(text).unwrap_err();
            assert!(
                matches!(err, OpenError::Refused(_) | OpenError::Unsupported),
                "wrong answer for {text:?}: {err}"
            );
        }
    }

    /// IDN 那一支：它是链接，但本轮不开。这里的格子与 `link` 那一头是同一句判据，界面那一只
    /// 钮灰掉的原因也就与这一只手拒绝的原因同一句话。
    #[test]
    fn an_idn_link_is_refused_rather_than_resolved() {
        let err = browser("https://exämple.com/").unwrap_err();
        assert!(
            matches!(err, OpenError::Refused(_) | OpenError::Unsupported),
            "wrong: {err}"
        );
        if let OpenError::Refused(why) = err {
            assert!(
                why.contains("非 ASCII"),
                "the sentence must name the reason: {why}"
            );
        }
    }

    /// 正面对照：一枚真该被**送进**那只 shell 的链接，绝不该拿 `Refused` 回话。本轮跑到这一步
    /// 要么 `Ok`（真开了标签页，手测那一格负责看见），要么 `Shell`（这台机器没有关联程序）——
    /// 两者都不是本轮的判据，判据只有「不是 Refused」这一条。
    ///
    /// 这一枚单测**会**在跑测试的那台机器上开一枚浏览器标签页。它挂着 `#[ignore]`：默认那一趟
    /// 不开别人的窗，而要证这一臂的人自己决定开。
    #[test]
    #[ignore = "会在跑测试的这台机器上真开一枚浏览器标签页"]
    fn an_https_link_is_not_refused() {
        let outcome = browser("https://example.com/");
        assert!(
            !matches!(outcome, Err(OpenError::Refused(_))),
            "a well-formed https URL must not be refused: {outcome:?}"
        );
    }

    /// 判据只有 core 那一份。这一枚**不**调 [`browser`] 的正面那一臂（那会真开一枚标签页），
    /// 它钉的是三件两侧必须一致的事：显示的那一枚域名就是浏览器要去的那一枚、这一枚过得了
    /// `openable` 那道闸、以及交回给 shell 的是原句而不是重新拼过的那一句。
    #[test]
    fn the_gate_is_the_one_in_core() {
        use falcon_core::link::{classify, Target};
        let text = "https://paypal.com@evil.com/x";
        let Target::Web(web) = classify(text) else {
            panic!("the fixture stopped being a link");
        };
        assert_eq!(web.host, "evil.com");
        assert!(web.openable());
        assert_eq!(web.url, text);
    }

    /// §5.5.10 的反面那一臂：这台机器上没有的那枚目标，绝不该被报成「已经递出去了」。
    ///
    /// 这一枚**会**走到 shell（一次真 `ShellExecuteW`），但它开不出任何东西：找不到文件时那只
    /// API 回的是 `SE_ERR_FNF`，不建进程也不建窗，所以默认那一趟里它碰不到别人的桌面 —— 与
    /// 上面那枚 `#[ignore]` 的正面格子正相反，那一格要的是一次真的标签页。
    #[cfg(windows)]
    #[test]
    fn a_target_that_is_not_here_is_never_reported_as_delivered() {
        let err = dispatch(
            "C:\\falconshot-not-installed\\receiver.exe",
            "\"C:\\falconshot-not-here\\shot.png\"",
        )
        .unwrap_err();
        assert!(matches!(err, OpenError::Shell(_)), "wrong: {err}");
        // 那句话得说得出是哪一层回的话：`SE_ERR_` 在串里，才读得出这不是我们编的解释。
        assert!(err.to_string().contains("SE_ERR"), "the sentence: {err}");
    }
}
