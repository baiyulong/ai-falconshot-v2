//! Which window station and desktop this process is actually on.
//!
//! This exists because a headless run is not the same as a working one: a process
//! launched from a service, a scheduled task or a remote-session agent lands on a
//! non-interactive station, where creating a window still "succeeds" - and the
//! clipboard then refuses every call with `Access is denied.`. Asking the two
//! names is how that gets told apart from a bug in this program (PRD §7.2).

#[cfg(windows)]
use windows::Win32::{
    Foundation::HANDLE,
    System::StationsAndDesktops::{
        GetProcessWindowStation, GetThreadDesktop, GetUserObjectInformationW, UOI_NAME,
    },
    System::Threading::GetCurrentThreadId,
};

/// `WinSta0\Default` reported as one line, or the names that were found instead.
#[cfg(windows)]
pub fn station_summary() -> String {
    // SAFETY: both handles come from the APIs themselves and are only read.
    unsafe {
        let station = object_name(GetProcessWindowStation().ok().map(|h| h.0));
        let desktop = object_name(GetThreadDesktop(GetCurrentThreadId()).ok().map(|h| h.0));
        format!("station={station} desktop={desktop}")
    }
}

/// The name of a window station or a desktop object: both are user objects with
/// the same information layout.
#[cfg(windows)]
fn object_name(raw: Option<*mut core::ffi::c_void>) -> String {
    let Some(ptr) = raw else {
        return "unknown".to_string();
    };
    let mut buf = [0u16; 64];
    let mut needed = 0u32;
    // SAFETY: `ptr` is a live handle from the call above, the buffer travels with
    // its own length, and `needed` is a u32 we own.
    let ok = unsafe {
        GetUserObjectInformationW(
            HANDLE(ptr),
            UOI_NAME,
            Some(buf.as_mut_ptr().cast()),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        )
    };
    if ok.is_err() {
        return "unknown".to_string();
    }
    // The API counts the terminating null in the length it reports.
    let chars = (needed as usize / 2).saturating_sub(1).min(buf.len());
    String::from_utf16_lossy(&buf[..chars])
}

/// The MVP is Windows-only; the rest of the workspace still has to compile here.
#[cfg(not(windows))]
pub fn station_summary() -> String {
    "station=n/a desktop=n/a".to_string()
}

/// 此刻**收着输入**的那一块桌面的名字。它问的只有这一格，别的一格都不问：
///
/// * 它与 [`station_summary`] 里那一格不是一件事 —— 后者问的是本进程挂在哪儿，两个名字可以不一样。
/// * 它**答不了锁屏**。这一格原本被当成「屏幕上显示的是哪一块」来用（`Winlogon` = 锁着），而
///   2026-11-06 在本机实测推翻了那句：Windows 11 的锁屏由 `LockApp` 挂在**同一块** `Default` 桌面上，
///   屏幕显示的是锁屏、`foreground` 那扇窗的标题是 `Windows Default Lock Screen`，而这一手照旧答
///   `Some("Default")`。「这条屏幕读取腿不跟着桌面走」与「这一台此刻锁着」要靠 [`crate::proc`] 那一族
///   （`foreground_window` / `pid_of_window` / `window_title`）分开，[`crate::affinity`] 的实测正是
///   那么分的。这一手只在输入桌面真的换掉那一格上还有用（UAC 的安全桌面正是）。
///
/// `None` = 问不出（没有输入桌面、句柄拿不到、不是 Windows）。它不与任何一个名字等价：
/// 把「问不出」印成 `"Default"`，就等于替这一台宣布屏幕上正显示着本进程那块桌面。
#[cfg(windows)]
pub fn input_desktop() -> Option<String> {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
    };

    // SAFETY: the two flag sets are the API's own constants, the handle is only read (by
    // `object_name`) and then closed on every path out of this function.
    let desk = unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) }.ok()?;
    let name = {
        let n = object_name(Some(desk.0));
        // SAFETY: `desk` is the handle the call above handed us; nothing reads it after this.
        let _ = unsafe { CloseDesktop(desk) };
        n
    };
    // `object_name` 的「读不出名字」是它自己那一格的 `unknown`，不是这一块桌面的名字。
    (name != "unknown").then_some(name)
}

#[cfg(not(windows))]
pub fn input_desktop() -> Option<String> {
    None
}

/// 这一趟进程站的那一块桌面，**屏幕上是不是正在显示它**。三格各问各的，谁也不替谁答。
///
/// * `console_session` —— 物理屏幕接到的那一个会话的号。`None` = 这一趟进程所在的会话没有连着
///   控制台（`WTSGetActiveConsoleSessionId` 交回 `0xFFFFFFFF` 说的就是这一格），于是屏幕上显示的
///   是**别的**会话的桌面，而本进程摆的窗屏幕上没有。它不与某一个会话号等价，也不与「调用失败」等价。
/// * `own_session` —— 本进程的会话号。它只是 `console_session` 的对照物，单独拿出来什么都不判。
/// * `composition` —— DWM 有没有在合成。本仓那一条屏幕读取腿（`xcap` 默认那一臂，
///   `GetWindowDC(GetDesktopWindow())` + `BitBlt`）读回来的是**合成好的**那一张：合成没开着，普通
///   窗只画在自己的 DC 上，那张桌面里就永远不出现它们 —— 于是「屏幕读取不跟着桌面走」有了第三种
///   解释，而那种机器上实时背景同样做不成（读的那一张本来就是死的）。
///
/// 问这三格是为了 [`crate::affinity`] 那一条闸：它的读数必须在「这条读取腿不跟桌面」与「这一趟
/// 进程根本不在屏幕上那一块桌面」之间分得开，而 [`station_summary`] 与 [`input_desktop`] 那两格
/// 分不开 —— **每一个会话都有自己的 `WinSta0\Default`**，名字一模一样，屏幕上显不显示它才是另一件事。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayReach {
    pub console_session: Option<u32>,
    pub own_session: Option<u32>,
    pub composition: Option<bool>,
}

impl DisplayReach {
    /// 本进程与屏幕在不在同一会话。任何一侧问不出，交回的都是「答不出」，而不是替它答一个「在」。
    pub fn same_session(&self) -> Option<bool> {
        Some(self.console_session? == self.own_session?)
    }

    /// 一行的读数，供实测印出来。每一格自己写自己那一句，缺的那一格写 `问不出`：把缺的印成
    /// `none` 或 `0`，读的人就会把它当成一个答。
    pub fn summary(&self) -> String {
        let slot = |v: Option<u32>| match v {
            Some(n) => n.to_string(),
            None => "问不出".to_string(),
        };
        format!(
            "console={} own={} composition={} same={}",
            slot(self.console_session),
            slot(self.own_session),
            match self.composition {
                Some(on) => if on { "on" } else { "off" }.to_string(),
                None => "问不出".to_string(),
            },
            match self.same_session() {
                Some(same) => same.to_string(),
                None => "问不出".to_string(),
            },
        )
    }
}

/// 问那三格。见 [`DisplayReach`] —— 这一手只问只报，不判「这一台的实时背景能不能做」。
#[cfg(windows)]
pub fn display_reach() -> DisplayReach {
    use windows::Win32::Graphics::Dwm::DwmIsCompositionEnabled;
    use windows::Win32::System::RemoteDesktop::{
        ProcessIdToSessionId, WTSGetActiveConsoleSessionId,
    };

    // 「没有连着控制台的会话」那一枚哨兵数，API 文档里就是这一格；写出来是为了它不许被当成
    // 一个会话号往下传 —— 那会让 same_session() 答出一个假的不等。
    const NO_CONSOLE: u32 = u32::MAX;

    let console = unsafe { WTSGetActiveConsoleSessionId() };
    let mut raw = 0u32;
    // SAFETY: `raw` is a writable u32 this call fills, and the pid is this process's own.
    let own = unsafe { ProcessIdToSessionId(std::process::id(), &mut raw) }
        .ok()
        .map(|_| raw);
    // SAFETY: the call takes nothing but its own query; the BOOL is read into an Option.
    let composition = unsafe { DwmIsCompositionEnabled() }.ok().map(|b| b.as_bool());
    DisplayReach {
        console_session: (console != NO_CONSOLE).then_some(console),
        own_session: own,
        composition,
    }
}

/// 非 Windows 那一臂：三格全问不出。它不与任何一个答等价。
#[cfg(not(windows))]
pub fn display_reach() -> DisplayReach {
    DisplayReach::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 问不出就是问不出：三格缺任何一侧，同会话那一问交回的都是 `None`，而不是「在」。这一格在
    /// `display_reach` 的两个臂上同一条 —— 非 Windows 那一臂交回的就是全 `None` 的那一枚。
    #[test]
    fn a_reach_that_cannot_be_asked_does_not_answer_yes() {
        assert_eq!(DisplayReach::default().same_session(), None);
        let only_console = DisplayReach {
            console_session: Some(1),
            ..Default::default()
        };
        assert_eq!(only_console.same_session(), None);
        let only_own = DisplayReach {
            own_session: Some(1),
            ..Default::default()
        };
        assert_eq!(only_own.same_session(), None);

        let same = DisplayReach {
            console_session: Some(1),
            own_session: Some(1),
            composition: Some(true),
        };
        assert_eq!(same.same_session(), Some(true));
        assert_eq!(
            DisplayReach {
                own_session: Some(2),
                ..same
            }
            .same_session(),
            Some(false)
        );
    }

    /// 缺的那一格在 `summary` 里读得出「问不出」三个字。印成 `0` 或 `none` 的那一句，会被读的人
    /// 当成一个答 —— 而「会话 0」在这台机器上正是「服务那一侧」的形状。
    #[test]
    fn a_missing_slot_is_not_printed_as_a_number() {
        let line = DisplayReach {
            console_session: Some(1),
            own_session: None,
            composition: None,
        }
        .summary();
        assert!(line.starts_with("console=1 "), "读数没有点名控制台那一格：{line}");
        assert!(line.contains("own=问不出"), "问不出的那一格被印成了数：{line}");
        assert!(line.contains("composition=问不出"), "同上：{line}");
        assert!(!line.contains('0'), "问不出不许伪装成会话 0：{line}");
    }

    /// 那一枚哨兵数不许活着离开这一手。`0xFFFFFFFF` 在 API 那里是「这一趟进程所在的会话没有连着
    /// 物理控制台」，而它作为会话号往下传会让 `same_session()` 答出一个**假的「不同会话」**—— 读
    /// 的人拿去解释屏幕读取为什么不动，而那个解释是这一格编出来的。连着控制台的那一臂读回的是真
    /// 会话号，没连着的那一臂读回的是 `None`，两句都不许是 `Some(4294967295)`。
    #[test]
    fn the_no_console_sentinel_is_not_a_session_number() {
        let reach = display_reach();
        assert_ne!(
            reach.console_session,
            Some(u32::MAX),
            "「没有连着控制台的会话」那一枚哨兵数被当成会话号交出来了：{reach:?}"
        );
        assert_ne!(
            reach.own_session,
            Some(u32::MAX),
            "本进程的会话号不可能是这一枚哨兵数：{reach:?}"
        );
    }

    /// 交回来的要么是一个名字，要么是「问不出」。中间那一档 —— 拿 `unknown` 或空串冒充一块桌面的
    /// 名字 —— 在这一手最便宜，因为调用点只要写一句 `unwrap_or("unknown")` 就把它做出来了。
    #[test]
    fn an_unaskable_desktop_is_not_a_name() {
        match input_desktop() {
            Some(name) => {
                assert!(!name.is_empty(), "一块没有名字的桌面不是名字，是问不出");
                assert_ne!(name, "unknown", "问不出不许伪装成一块桌面的名字");
            }
            // 无桌面、非 Windows、拿不到那张桌面的读权限：都是答，都不必开屏幕。
            None => {}
        }
    }
}
