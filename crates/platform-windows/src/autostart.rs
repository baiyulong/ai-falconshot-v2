//! §5.1.2 开机启动：那一枚开关在机器那一侧的手。
//!
//! PRD 第 3 步「系统写入相应的启动配置」与预期结果第二条「关闭选项后，应移除启动项」，
//! 在一台 Windows 上就是一枚值：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下面
//! 那条以 [`VALUE_NAME`] 命名的 `REG_SZ`，内容是一条能把本程序启动起来的命令。登录时
//! explorer 逐条读那一格、按命令行把里面的东西跑一遍，所以本程序不需要为自己造一个进程 ——
//! §5.1.2 预期结果第一条「应用自动在后台运行」的对面是「这一枚值在不在」，而不在窗里。
//!
//! **为什么值名固定而不是拿 exe 的文件名当值名**：值名是这一枚启动项的**身份**，
//! 「关掉时删得掉」全靠它。路径会变（升级到新目录、开发态与安装态两套路径），一旦路径当了
//! 值名，换一次路径就留下一条没人认领的启动项，第二条永远删不掉 —— 那是往用户的机器上
//! 累积垃圾，正是这一节最容易做坏的一格。
//!
//! **判据是命令的内容，不是「有没有值」**：`desired = true` 且那一枚值写着**别的**路径
//! （上一场安装、别人的复制）时不该报「已经好了」，而该改写成本程序 —— [`decide`] 那三臂
//! 分的正是这件事。反过来读的时候（[`Held::switch_on`]）「值在而指向别处」答**开着**：
//! 机器确实会在登录时启动一个 falconshot，页上说「没开」就是撒谎，而那两条路不同由
//! [`held_line`] 那句话带着，不在判据里被折掉。
//!
//! 开发态与安装态的差别就在这一枚命令串上，读数的人要认得出它：
//! * `cargo run` → `target\debug\falconshot.exe`（换一次 `--target` 或删一次 target 就没了）；
//! * 直装（把 `falconshot.exe` 与那些 dll 放进 `Program Files`）→ 那一条绝对路径，稳定；
//! * **MSIX** → 包内的绝对路径既写不进 `WindowsApps`（那目录归 `TrustedInstaller`），
//!   也每次升级换一个版本号目录，所以打包那一族走的是 `AppxManifest` 里的
//!   `windows.startupTask` 扩展（`Windows.ApplicationModel.StartupTask`），**本模块不冒充它**：
//!   这里只负责 unpackaged exe 那一族。见 §5.1.2 的台账：MSIX 那一头至今没有手。
//!
//! 形状与 [`crate::theme`] 同一族：判据（该写、该删、还是不必动；命令串长什么样；什么算
//! 一致）全是能在 `cargo test` 里问的纯格子，Win32 只留在 [`Registry`] 那一格里；
//! [`sync_with`] 拿的是一枚 [`Store`] 而不是那一格真句柄，所以「接通配置 → 系统」这一整条
//! 编排（含失败要把原因带出来）能在**不碰任何人的注册表**的地方跑完。真实句柄那一腿
//! 由页上那一枚开关与 `[autostart cfg]` 那一行在真人机上验。

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// 启动项住的那一格，`HKCU` 下的相对路径。`HKCU` 而不是 `HKLM`：后者要管理员，
/// 而 §5.1.2 说的是「用户登录操作系统后」——用户自己那一半机器就够。
pub const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

/// 值名 = 这一枚启动项的身份。与设置档那个目录同名（`%APPDATA%\ai-falconshot`，见
/// [`falcon_core::config::config_path`]），所以「这一枚是谁的」在机器里也是读得出来的。
/// 不许拿 `current_exe()` 的文件名当它：理由在本文件头部第三条。
pub const VALUE_NAME: &str = "ai-falconshot";

/// 本程序该以哪条命令被启动：那一枚 exe 的绝对路径，含空白时包一层引号。
///
/// 引号不是装饰。Run 里的那一枚值是交给 `CreateProcess` 的**命令行**而不是 argv，
/// `C:\Program Files\...\falconshot.exe` 不加引号会被切成 `C:\Program` 加
/// `Files\...`，于是登录时什么也不发生、而页上写着「已开启」——这一格坏了不报错，
/// 是最贵的一种坏。已经有成对引号的不再包第二层：[`same_command`] 折的是整串，
/// 每跑一次多包一层会让「写完再读」永远对不上，从而每次启动重写一遍。
pub fn command_for(exe: &Path) -> OsString {
    let raw = exe.as_os_str();
    if !needs_quotes(raw) {
        return raw.to_os_string();
    }
    let mut command = OsString::from("\"");
    command.push(raw);
    command.push("\"");
    command
}

/// 要包引号的是「有空白、而且自己头上尾上没有成对引号」的那一条路径。
fn needs_quotes(path: &OsStr) -> bool {
    let text = path.to_string_lossy();
    !already_quoted(&text) && text.chars().any(char::is_whitespace)
}

/// 至少两枚字符、首尾各一枚 `"`。单枚 `"` 不算：`Chars::next_back` 在取走头上那枚之后
/// 看的是**剩下那一段**的尾巴，所以 `"\""` 与 `"\"\""` 在这一枚上分得开，不必再数一遍长度。
fn already_quoted(text: &str) -> bool {
    let mut rest = text.chars();
    let Some(first) = rest.next() else {
        return false;
    };
    first == '"' && rest.next_back() == Some('"')
}

/// 两条命令是不是同一条：整串相等，大小写折（Windows 的路径不区分大小写），空白不折。
///
/// 空白不折的理由：`"C:\a b\app.exe"` 与 `C:\a b\app.exe` 对 `CreateProcess` 是两回事
/// （后者会被切坏），判成相同就是让一条登录时启动不起来的命令一直留在启动项里。
/// 分隔符 `/` 与 `\` 也不折，同一个理由的反面：`C:/a/app.exe` 那种写法能跑但不是本程序会
/// 写出的形状，判成「不一样」→ 重写一次成规范形状，比判成「一样」→ 永远不改要诚实。
pub fn same_command(a: &OsStr, b: &OsStr) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// 机器此刻那一枚值相对「这一台 exe」站在哪儿。三臂，[`decide`] 与页上的回填分的就是这三臂。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    /// 值不在（或那一格不存在）：登录时没有本程序会被启动。
    Absent,
    /// 值在，写的就是这一台 exe 的那条命令。
    Here,
    /// 值在，写的是**别的**那一串 —— 带着原文，不许在这里就丢掉它：页上要能看见指向的是谁。
    Elsewhere(OsString),
}

impl Held {
    /// 页上那一枚开关该显示的字：值在（不论指向谁）就是开着。
    ///
    /// 「指向别处」也答开，因为这一枚开关问的是「登录时会不会有一个 falconshot 被启动」，
    /// 不是「被启动的是不是我现在跑的这一台」。后者是 [`Held::Elsewhere`] 那一臂的话，
    /// 由 [`held_line`] 原样交出去；把它在这里折成 `false`，就成了「机器里明明有一条启动项，
    /// 页上说没有」，而用户下一次关掉再打开还会以为是自己刚写上的。
    pub fn switch_on(&self) -> bool {
        !matches!(self, Held::Absent)
    }
}

/// 「那一格现在写着什么」+「本程序该是什么」→ [`Held`]。`held` 为 `None` 是「没有这一枚值」。
pub fn classify(held: Option<&OsStr>, want: &OsStr) -> Held {
    match held {
        None => Held::Absent,
        Some(held) if same_command(held, want) => Held::Here,
        Some(held) => Held::Elsewhere(held.to_os_string()),
    }
}

/// 这一趟该动手还是不该动。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Write,
    Delete,
    Leave,
}

/// 开关与机器位置的全映射 —— 「用户拨上去到底发生什么」这一问唯一的答案处。
///
/// `true` + [`Held::Elsewhere`] 走 [`Action::Write`] 是有意的：那是「用户要这一台被启动，
/// 而那条值现在不属于这一台」，改写是本节第 3 步要的事；Windows 上每个有 Run 项的程序升级时
/// 都这么重写自己那一枚值。`false` + 任何「值在」走 [`Action::Delete`]，那是预期结果第二条。
pub fn decide(desired: bool, held: &Held) -> Action {
    match (desired, held) {
        (true, Held::Here) => Action::Leave,
        (true, _) => Action::Write,
        (false, Held::Absent) => Action::Leave,
        (false, _) => Action::Delete,
    }
}

/// 动完手之后，机器站的位置配不配得上那一枚开关。
///
/// 这一枚是「返回值 `Ok` 了」与「页上可以说已开启」之间唯一的桥：写没写进去不是由调用说的，
/// 是由**读完那一次**说的。少了它，`put` 假成功（那一枚值被别人的组策略锁掉、被杀软拦下）
/// 就成了静默的「配置说开了而注册表里没有」。
pub fn satisfies(desired: bool, after: &Held) -> bool {
    after.switch_on() == desired
}

/// 机器站在哪儿，一句话。`Held::Elsewhere` 带着那一串原文 —— 开发态/安装态的差别就是从
/// 这一格读出来的，不许在措辞里把它抹平。
pub fn held_line(held: &Held) -> String {
    match held {
        Held::Absent => format!("{VALUE_NAME} 不在（登录时不会启动本程序）"),
        Held::Here => format!("{VALUE_NAME} = 本程序现在这一条命令"),
        Held::Elsewhere(command) => {
            format!("{VALUE_NAME} 现在指向别处：{}", command.to_string_lossy())
        }
    }
}

/// 动手那一句的名字。`Leave` 有两句：开着且已经指着本程序 / 本来就没有启动项，
/// 由 [`Report::line`] 按位置分开说，这里只给动作本身。
pub const fn action_line(action: Action) -> &'static str {
    match action {
        Action::Write => "写入启动项",
        Action::Delete => "移除启动项",
        Action::Leave => "未改动启动项",
    }
}

/// 一次拧的结果。`held` 是**读完再回来**的那一枚位置，不是「我以为写成了什么」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub action: Action,
    pub held: Held,
    pub command: OsString,
}

impl Report {
    /// 一行说完这一趟，交给页上那一条状态行与 `[autostart cfg]` 那一行。
    /// 失败不走这里 —— 失败由 [`AutostartError`] 自己说，不许在这一格润色。
    pub fn line(&self) -> String {
        let where_ = match self.held {
            Held::Absent if self.action == Action::Leave => "本来就没有启动项",
            Held::Absent => "现在没有启动项",
            Held::Here => "已指向本程序",
            Held::Elsewhere(_) => "指向别处",
        };
        format!(
            "{}{NOTE}：{}；{}",
            action_line(self.action),
            where_,
            held_line(&self.held)
        )
    }
}

/// 页上那一行的说明与本行分家的那一枚记号。与 [`crate`] 侧的 `settings_view::NOTE_SEP` 同形，
/// 在这里各写一遍是因为这一档不依赖 qt_bridge（它是被依赖的那一头）。
const NOTE: &str = " —— ";

/// 启动项那一格。三只手都只碰**本程序自己那一枚值**：[`Store::remove`] 走
/// `RegDeleteValueW` 而不是 `RegDeleteTreeW` —— 后者删的是**整条 `Run`**，
/// 那会把用户机器上别人家的启动项一起删掉，是这一档最不该犯的错。
pub trait Store {
    /// 那一枚值现在写着什么。值不在、或 `Run` 那一格不存在都答 `Ok(None)`（那是「没有启动项」
    /// 而不是「读失败」）；读不动才答 `Err`。
    fn read(&self) -> Result<Option<OsString>, AutostartError>;
    fn put(&self, command: &OsStr) -> Result<(), AutostartError>;
    /// 没有可删的东西算成功：要的状态已经达成了。
    fn remove(&self) -> Result<(), AutostartError>;
}

/// 拧一下：读、判、动、再读回来验。全程只问 `store`，所以这一条编排能在任何一台没有
/// 注册表的机器上跑完（单测就是这么跑的）。
pub fn sync_with(store: &dyn Store, desired: bool, want: &OsStr) -> Result<Report, AutostartError> {
    let before = classify(store.read()?.as_deref(), want);
    let action = decide(desired, &before);
    match action {
        Action::Write => store.put(want)?,
        Action::Delete => store.remove()?,
        Action::Leave => {}
    }
    let after = classify(store.read()?.as_deref(), want);
    if !satisfies(desired, &after) {
        return Err(AutostartError::NotApplied(format!(
            "要的是「{}」，读完机器答的是「{}」",
            if desired {
                "开机自动启动"
            } else {
                "不自动启动"
            },
            held_line(&after)
        )));
    }
    Ok(Report {
        action,
        held: after,
        command: want.to_os_string(),
    })
}

/// 只报告不改动：启动回填与页上回填读这一枚。「读这一枚」与「改这一枚」分在两只手里，
/// 是为了页上能问机器而不把用户没按过的钮按下去。
pub fn inspect_with(store: &dyn Store, want: &OsStr) -> Result<Held, AutostartError> {
    Ok(classify(store.read()?.as_deref(), want))
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AutostartError {
    /// 这台机器没有 `HKCU\...\Run` 这一格。不是「没做成」，是「这里没有可做的东西」——
    /// MVP 只有 Windows，而工作区仍要在别处编译得过（与 [`crate::clip`] 那一族同一安排）。
    #[error("这一台机器不是 Windows，没有可以写入的启动项")]
    Unsupported,
    #[error("取不到本程序自己的绝对路径，因此不知道该把启动项写成什么：{0}")]
    NoExePath(String),
    #[error("打不开启动项那一格：{0}")]
    Open(String),
    #[error("读不动启动项那一枚值：{0}")]
    Read(String),
    #[error("写不进启动项：{0}")]
    Write(String),
    #[error("删不掉启动项：{0}")]
    Delete(String),
    /// 三处「没吞」里的最后一处：`Ok` 了而机器不是要的样子。原因里带着机器此刻的答案，
    /// 而不是只说「失败」。
    #[error("动手之后再读，机器仍不是那一枚开关说的样子：{0}")]
    NotApplied(String),
}

/// 系统那一头的几个返回码。写在这里而不是从 `windows` 引，是为了 [`answers_absent`] 与
/// [`describe_code`] 这两枚判据在一台没有注册表的机器上也能被问（[`crate::theme`] 里
/// `REG_DWORD_KIND` 同一枚理由）。
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_KEY_DELETED: u32 = 1018;

/// 「那一格/那一枚值不存在」的那几枚码。读到它们要答「没有启动项」而不是「读失败了」：
/// 全新 HKCU 里 `Run` 可以压根不存在，而 §5.1.2 的默认值就是「没开」。
pub const fn answers_absent(code: u32) -> bool {
    matches!(
        code,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_KEY_DELETED
    )
}

/// 认得出的码给一句名字，认不出的原样说「没名字」——一句猜出来的原因比没有原因更坏。
/// `ERROR_ACCESS_DENIED` 在 `HKCU` 上正常不该出现，出现了基本是组策略锁了 `Run` 那一格，
/// 这一句正是那一趟要递给用户的话。
pub fn describe_code(code: u32) -> String {
    let name = match code {
        ERROR_FILE_NOT_FOUND => "那一格或那一枚值不存在",
        ERROR_PATH_NOT_FOUND => "路径不存在",
        ERROR_ACCESS_DENIED => "权限被拒（组策略锁住启动项那一格时就是这个）",
        ERROR_KEY_DELETED => "键已被删除（手里的句柄过期了）",
        _ => "系统没给出这一枚码的名字",
    };
    format!("系统返回 {code:#010X}（{name}）")
}

/// 值里的字节 → 那条命令的宽串。
///
/// `REG_SZ`(1) 与 `REG_EXPAND_SZ`(2) 是**认识的形状**；别种（`REG_BINARY`、
/// `REG_MULTI_SZ`…）也照宽串读：那一枚值占着本程序的名字，读成「指向别处」还能改写或删除，
/// 判成「读不动」则会把页上那一枚开关两头都锁死 —— 用户被自己机器里一条怪值卡在既开不了
/// 也关不掉的位置上，那不是这一档该有的答案。含 `%VAR%` 的 `REG_EXPAND_SZ` **不展开**：
/// 展开之后再比就把别人的写法当成自己的，那条值会以「一致」的名义留下来。
/// 收尾的那一枚 NUL 只在**认识的**那两种形状里砍：别种不假定它有，多砍一枚就是把别人的
/// 命令改坏一个字符。长度不是偶数时是真的没法当宽串读，答 `None`。
pub fn decode_value(kind: u32, bytes: &[u8]) -> Option<Vec<u16>> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if known_value_kind(kind) && units.last() == Some(&0) {
        units.pop();
    }
    Some(units)
}

/// 这一枚类型码是不是本程序自己会写出去的那两种之一。它同时管着 [`decode_value`] 里
/// 「砍不砍收尾符」那一判，并让读那一格的人**知道**这条值不是本程序写的形状。
pub const fn known_value_kind(kind: u32) -> bool {
    matches!(kind, REG_SZ_KIND | REG_EXPAND_SZ_KIND)
}

const REG_SZ_KIND: u32 = 1;
const REG_EXPAND_SZ_KIND: u32 = 2;

/// 那条命令 → 要交给 `RegSetValueExW` 的字节：宽串加一枚收尾 NUL，小端。
/// 收尾 NUL 是 `REG_SZ` 的规矩，不是装饰：少了它，读回来的那一串会缺最后一个字符。
pub fn encode_value(units: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(units.len() * 2 + 2);
    for unit in units.iter().chain(std::iter::once(&0)) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// 本程序此刻该以哪条命令被启动 —— [`command_for`] 配上 `current_exe()`。
///
/// 绝对路径这一查不是洁癖：相对路径进了 `Run` 会被按**登录时那个进程的**当前目录解析，
/// 而那没有人能预测，坏了也不报错。
pub fn current_command() -> Result<OsString, AutostartError> {
    let exe = std::env::current_exe().map_err(|e| AutostartError::NoExePath(e.to_string()))?;
    if !exe.is_absolute() {
        return Err(AutostartError::NoExePath(format!(
            "读出来的本程序路径不是绝对的：{}",
            exe.display()
        )));
    }
    Ok(command_for(&exe))
}

/// 本进程那一台机器上的启动项，拧到那一枚开关说的位置。失败带着原因交出去，不许 `let _ =`。
#[cfg(windows)]
pub fn sync(desired: bool) -> Result<Report, AutostartError> {
    sync_with(&Registry, desired, &current_command()?)
}

/// 本进程那一台机器上的启动项现在站在哪儿 —— 只读，不动任何东西。
#[cfg(windows)]
pub fn inspect() -> Result<Held, AutostartError> {
    inspect_with(&Registry, &current_command()?)
}

/// 没有 `HKCU` 的机器答「这里没有可做的东西」，而原因由调用方原样印出去：
/// 那一格在档里、页上、日志里都不许被读成「已经关掉了」。
#[cfg(not(windows))]
pub fn sync(_desired: bool) -> Result<Report, AutostartError> {
    Err(AutostartError::Unsupported)
}

#[cfg(not(windows))]
pub fn inspect() -> Result<Held, AutostartError> {
    Err(AutostartError::Unsupported)
}

/// `HKCU\...\Run` 那一格。本结构没有字段：它就是那一格，句柄只在三只手内部各开各关。
#[cfg(windows)]
#[derive(Debug, Default)]
pub struct Registry;

#[cfg(windows)]
impl Registry {
    /// 开那一格。`READ`/`SET_VALUE` 由调用方选：只要本次真要动的权限。
    ///
    /// 不加 `KEY_WOW64_64KEY`：`CurrentVersion\Run` 是 32/64 两套视图**共享**的那几枚键之一
    /// （不进 WOW64 反射），加与不加读到的是同一格，加了反而多一处要解释为什么在那儿。
    /// `ERROR_FILE_NOT_FOUND` 那一臂交回 `Ok(None)` 而不是 `Err`：那一格不存在就是没有启动项。
    fn open(&self, sam: REG_SAM_FLAGS) -> Result<Option<HKEY>, AutostartError> {
        let sub = wide(OsStr::new(RUN_SUBKEY));
        let mut key = HKEY::default();
        // SAFETY: `sub` 是本函数持有的、以 0 收尾的宽串，活得比这一次调用长；`key` 是本函数的
        // 局部输出；`HKEY_CURRENT_USER` 是常量句柄；`sam` 只要一个方向的权限。
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), None, sam, &mut key) };
        if status.is_err() {
            if answers_absent(status.0) {
                return Ok(None);
            }
            return Err(AutostartError::Open(describe_code(status.0)));
        }
        Ok(Some(key))
    }
}

#[cfg(windows)]
impl Store for Registry {
    fn read(&self) -> Result<Option<OsString>, AutostartError> {
        use std::os::windows::ffi::OsStringExt;

        let name = wide(OsStr::new(VALUE_NAME));
        let Some(key) = self.open(KEY_READ)? else {
            return Ok(None);
        };
        let outcome = unsafe { query(key, &name) };
        // 关不上只是漏一枚句柄，进程收尾时系统会收走；它不是用户要的那一句「为什么没改成」，
        // 所以这里有意丢掉返回值、不改判成败（[`crate::reveal::select`] 对 `CoInitializeEx`
        // 的返回值同一处理）。
        let _ = unsafe { RegCloseKey(key) };
        let Some((kind, bytes)) = outcome? else {
            return Ok(None);
        };
        // 认不出的类型码不改判，只由 [`known_value_kind`] 那一格说得出「这不是本程序写的」。
        let units = decode_value(kind, &bytes).ok_or_else(|| {
            AutostartError::Read(format!(
                "那一枚值的长度不是宽字符的整数倍（{kind} 类型，{} 字节）",
                bytes.len()
            ))
        })?;
        Ok(Some(OsString::from_wide(&units)))
    }

    fn put(&self, command: &OsStr) -> Result<(), AutostartError> {
        let sub = wide(OsStr::new(RUN_SUBKEY));
        let name = wide(command);
        let bytes = encode_value(&widen(command));
        let mut key = HKEY::default();
        // 造而不是开：全新 HKCU 里 `Run` 可以不存在，那一趟用户要的就是「有这一枚启动项」。
        // SAFETY: 三串宽字符都是本函数持有的、以 0 收尾的缓冲，活得比这两次调用长；`key` 与
        // `&bytes` 都是局部；`REG_OPTION_NON_VOLATILE` 要的正是「留在盘上」那一半语义；
        // 没有安全描述符（`None`）与没有 disposition 输出（`None`）都是 API 允许的。
        let opened = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        };
        if opened.is_err() {
            return Err(AutostartError::Write(describe_code(opened.0)));
        }
        // SAFETY: `key` 是上一步真开出来的句柄，本函数在 `RegCloseKey` 之前持有它；
        // `name` 是收尾好的宽串；`bytes` 是本次写的全部数据，长度由切片自己带。
        let set = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes)) };
        // 同上：枚句柄关不上是漏一枚，不是用户那一句「为什么没改成」。
        let _ = unsafe { RegCloseKey(key) };
        if set.is_err() {
            return Err(AutostartError::Write(describe_code(set.0)));
        }
        Ok(())
    }

    fn remove(&self) -> Result<(), AutostartError> {
        let name = wide(OsStr::new(VALUE_NAME));
        let Some(key) = self.open(KEY_SET_VALUE)? else {
            // 那一格都不在：没有本程序的启动项，要的状态已经达成了。
            return Ok(());
        };
        // 删的是**本程序那一枚值**。这里绝不走 `RegDeleteTreeW`：它连子键一起删，而
        // `RUN_SUBKEY` 就是那一整格 —— 那等于替用户把别的软件的开机启动全清了。
        // SAFETY: `key` 由上面那一步开出来且尚未关闭；`name` 是收尾好的宽串。
        let deleted = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
        // 同上：枚句柄关不上是漏一枚，不是用户那一句「为什么没删掉」。
        let _ = unsafe { RegCloseKey(key) };
        if deleted.is_err() && !answers_absent(deleted.0) {
            return Err(AutostartError::Delete(describe_code(deleted.0)));
        }
        Ok(())
    }
}

/// 两趟 `RegQueryValueExW`：第一趟只问长度，第二趟照着那个长度读。
///
/// `Ok(None)` 是「那一枚值不在」（`ERROR_FILE_NOT_FOUND` 那一族）—— 那是「没有启动项」
/// 而不是「读失败」，与 [`Store::read`] 的约定同一句。剩下的 `Err` 才是真的读不动。
#[cfg(windows)]
unsafe fn query(key: HKEY, name: &[u16]) -> Result<Option<(u32, Vec<u8>)>, AutostartError> {
    let mut kind = REG_VALUE_TYPE(0);
    let mut len = 0u32;
    let sizing = RegQueryValueExW(
        key,
        PCWSTR(name.as_ptr()),
        None,
        Some(&mut kind),
        None,
        Some(&mut len),
    );
    if sizing.is_err() {
        if answers_absent(sizing.0) {
            return Ok(None);
        }
        return Err(AutostartError::Read(describe_code(sizing.0)));
    }
    let mut bytes = vec![0u8; len as usize];
    let read = RegQueryValueExW(
        key,
        PCWSTR(name.as_ptr()),
        None,
        Some(&mut kind),
        Some(bytes.as_mut_ptr()),
        Some(&mut len),
    );
    if read.is_err() {
        return Err(AutostartError::Read(describe_code(read.0)));
    }
    // 第二趟之后 `len` 是系统**实际写进缓冲**的字节数；两次之间别人改了那枚值时，
    // 以第二次的答案为准，而不是按第一次问到的长度交出一段多出来的 0。
    bytes.truncate((len as usize).min(bytes.len()));
    Ok(Some((kind.0, bytes)))
}

/// 一段文本 → 以 0 收尾的宽串（给 `PCWSTR` 的那些名字用）。
#[cfg(windows)]
fn wide(text: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// 一段文本 → 宽码元，**不**收尾（字节由 [`encode_value`] 加）。
#[cfg(windows)]
fn widen(text: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.encode_wide().collect()
}

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS,
    REG_SZ, REG_VALUE_TYPE,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 一台假机器，代替那一格真注册表。单测一律对着它跑：真注册表是**用户机器上的启动项**，
    /// 一次 `cargo test` 不该把别人家的启动项删掉、也不该把本程序写进用户的登录流程里。
    ///
    /// 自动化的覆盖面因此分两段：读的那一手在真机器上有一枚 `#[ignore]` 的证人
    /// （`the_real_run_key_answers_a_read_without_reaching`），写与删的那两手**没有**任何对着
    /// 真注册表跑过的量具 —— 它们的对面是页上那一枚开关与启动时 `[autostart cfg]` 那一行，得由
    /// 真人在这台机器上按一次、看一眼 `regedit`。这一条是本节明写的欠账，不是忘了写。
    #[derive(Default)]
    struct Fake {
        held: RefCell<Option<OsString>>,
        /// 哪一只手被叫过、按什么顺序。`Leave` 那一臂的证据就是这一串里没有 `put`/`remove`。
        calls: RefCell<Vec<&'static str>>,
        /// 交回去的那一句失败，原样从手里出来。
        fail: RefCell<Option<AutostartError>>,
        /// 写不进去的那一台：`put` 答成功、值却一动不动（验 [`satisfies`] 那一臂的对面）。
        stubborn: bool,
    }

    impl Fake {
        fn with(held: Option<&str>) -> Fake {
            Fake {
                held: RefCell::new(held.map(OsString::from)),
                ..Fake::default()
            }
        }
        /// 假机器此刻的值。`Ref` 借出来的临时量在断言里活不过一句，所以经这一枚克隆。
        fn peek(&self) -> Option<OsString> {
            self.held.borrow().clone()
        }
        fn names(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl Store for Fake {
        fn read(&self) -> Result<Option<OsString>, AutostartError> {
            self.calls.borrow_mut().push("read");
            Ok(self.held.borrow().clone())
        }
        fn put(&self, command: &OsStr) -> Result<(), AutostartError> {
            self.calls.borrow_mut().push("put");
            if let Some(error) = self.fail.borrow().clone() {
                return Err(error);
            }
            if !self.stubborn {
                *self.held.borrow_mut() = Some(command.to_os_string());
            }
            Ok(())
        }
        fn remove(&self) -> Result<(), AutostartError> {
            self.calls.borrow_mut().push("remove");
            if let Some(error) = self.fail.borrow().clone() {
                return Err(error);
            }
            if !self.stubborn {
                *self.held.borrow_mut() = None;
            }
            Ok(())
        }
    }

    const WANT: &str = r"C:\Program Files\ai\falconshot.exe";
    const OTHER: &str = r"C:\Users\bob\AppData\Local\Temp\falconshot.exe";

    fn want() -> OsString {
        OsString::from(WANT)
    }

    /// 六臂全表：一枚开关 × 机器三个位置。「拨上去没反应」与「关掉没删掉」在这一格里
    /// 是没有地方的 —— 每一臂都只有一种映射。
    #[test]
    fn the_switch_maps_to_exactly_one_action() {
        let here = Held::Here;
        let absent = Held::Absent;
        let elsewhere = Held::Elsewhere(OsString::from(OTHER));
        assert_eq!(decide(true, &absent), Action::Write);
        assert_eq!(decide(true, &here), Action::Leave);
        // 指向别处也重写：用户要的是「这一台会被启动」，一条旧路径不算。
        assert_eq!(decide(true, &elsewhere), Action::Write);
        assert_eq!(decide(false, &elsewhere), Action::Delete);
        assert_eq!(decide(false, &here), Action::Delete);
        assert_eq!(decide(false, &absent), Action::Leave);
    }

    /// 引号只在路径有空白时加，而且不加第二层。这一格坏了是不报错的：登录时 `CreateProcess`
    /// 把 `C:\Program` 当成程序名，页上却写着「已开启」。
    #[test]
    fn the_command_is_quoted_only_when_it_has_to_be() {
        assert_eq!(
            command_for(Path::new(r"C:\Users\bob\falconshot.exe")),
            OsString::from(r"C:\Users\bob\falconshot.exe")
        );
        assert_eq!(
            command_for(Path::new(WANT)),
            OsString::from(format!("\"{WANT}\""))
        );
        // 已经包好的不再包第二层 —— 否则「写完再读」永远对不上，每次启动重写一遍。
        assert_eq!(
            command_for(Path::new(&format!("\"{WANT}\""))),
            OsString::from(format!("\"{WANT}\""))
        );
        assert_eq!(command_for(Path::new("\"")), OsString::from("\""));
    }

    /// 大小写折、空白与分隔符不折。`Held::Here` 的判据就是这一枚，所以它必须既能认出
    /// 「同一台的 exe 换了盘符大小写」，又认不出「一条会被切坏的命令」。
    #[test]
    fn same_command_folds_case_but_not_blanks() {
        assert!(same_command(
            OsStr::new(r"C:\Program Files\ai\falconshot.exe"),
            OsStr::new(r"c:\program files\ai\FALCONSHOT.EXE")
        ));
        assert!(!same_command(
            OsStr::new(WANT),
            OsStr::new(&format!("\"{WANT}\""))
        ));
        assert!(!same_command(OsStr::new(WANT), OsStr::new(OTHER)));
        assert!(!same_command(
            OsStr::new(r"C:/a/falconshot.exe"),
            OsStr::new(r"C:\a\falconshot.exe")
        ));
    }

    /// 三个位置的读法：`None` = 不在；同一串 = Here；别的一条 = 带着原文的 Elsewhere。
    #[test]
    fn classify_stands_where_the_entry_points() {
        assert_eq!(classify(None, &want()), Held::Absent);
        assert_eq!(classify(Some(OsStr::new(WANT)), &want()), Held::Here);
        assert_eq!(
            classify(Some(OsStr::new(OTHER)), &want()),
            Held::Elsewhere(OsString::from(OTHER))
        );
        assert!(!Held::Absent.switch_on());
        assert!(Held::Here.switch_on());
        // 「指向别处」也是开着：机器确实会在登录时启动一个 falconshot。
        assert!(Held::Elsewhere(OsString::from(OTHER)).switch_on());
    }

    /// 编排本体（不碰真注册表的那一条腿）：开 → 写、关 → 删、各读完再验一次。
    #[test]
    fn sync_turns_the_switch_into_the_machine() {
        let store = Fake::with(None);
        let report = sync_with(&store, true, &want()).expect("写入");
        assert_eq!(report.action, Action::Write);
        assert_eq!(report.held, Held::Here);
        assert_eq!(report.command, want());
        assert_eq!(store.names(), vec!["read", "put", "read"]);

        // 再拨一次同一头：不动任何东西，只读。
        let again = sync_with(&store, true, &want()).expect("幂等");
        assert_eq!(again.action, Action::Leave);
        assert_eq!(store.names(), vec!["read", "put", "read", "read", "read"]);

        let off = sync_with(&store, false, &want()).expect("移除");
        assert_eq!(off.action, Action::Delete);
        assert_eq!(off.held, Held::Absent);
        assert_eq!(store.peek(), None);
    }

    /// 关掉那一头对着一条指向别处的值：删掉它，而不是「不动」——§5.1.2 预期结果第二条。
    #[test]
    fn switching_off_removes_even_an_entry_that_points_elsewhere() {
        let store = Fake::with(Some(OTHER));
        let report = sync_with(&store, false, &want()).expect("移除");
        assert_eq!(report.action, Action::Delete);
        assert_eq!(report.held, Held::Absent);
        assert!(store.names().contains(&"remove"));
    }

    /// 系统拒了不许静默：原因从手里一路带出来，而页上那一行因此说不出「已开启」。
    #[test]
    fn a_refusing_machine_hands_its_reason_up() {
        let store = Fake::with(None);
        *store.fail.borrow_mut() = Some(AutostartError::Write(describe_code(ERROR_ACCESS_DENIED)));
        let error = sync_with(&store, true, &want()).expect_err("要失败");
        assert!(
            matches!(error, AutostartError::Write(_)),
            "判据走形了：{error:?}"
        );
        assert!(error.to_string().contains("0x00000005"), "{error}");
        assert!(error.to_string().contains("权限"), "{error}");
        assert_eq!(store.peek(), None, "拒了就不许留下半改的值");
    }

    /// `put` 自称成功而值一动不动（组策略、杀软、别人的写保护）：这一趟必须报错。
    /// 少了 [`satisfies`] 这一臂，页上就会写着「已开启」而机器里什么也没有 —— 那正是
    /// 这一轮要修的欠账的新写法。
    #[test]
    fn a_write_the_machine_did_not_keep_is_not_success() {
        let store = Fake {
            stubborn: true,
            ..Fake::with(None)
        };
        let error = sync_with(&store, true, &want()).expect_err("要报错");
        assert!(
            matches!(error, AutostartError::NotApplied(_)),
            "判据走形了：{error:?}"
        );
        assert!(error.to_string().contains("不在"), "{error}");

        // 关掉那一头对着删不掉的机器：同样不许报成功。
        let stuck = Fake {
            stubborn: true,
            ..Fake::with(Some(WANT))
        };
        assert!(matches!(
            sync_with(&stuck, false, &want()),
            Err(AutostartError::NotApplied(_))
        ));
    }

    /// 已经在的位置一个字节也不动：`Leave` 的对面是「每次启动都重写一遍用户的启动项」。
    #[test]
    fn leaving_the_machine_touches_nothing() {
        let here = Fake::with(Some(WANT));
        assert_eq!(
            sync_with(&here, true, &want()).unwrap().action,
            Action::Leave
        );
        assert_eq!(here.names(), vec!["read", "read"]);
        let gone = Fake::with(None);
        assert_eq!(
            sync_with(&gone, false, &want()).unwrap().action,
            Action::Leave
        );
        assert_eq!(gone.names(), vec!["read", "read"]);
    }

    /// 只报告不改动那一手：读到的位置与 `sync` 判的必须是同一份判据，而不是第二份实现。
    #[test]
    fn inspection_reports_without_reaching() {
        let store = Fake::with(Some(OTHER));
        assert_eq!(
            inspect_with(&store, &want()).unwrap(),
            Held::Elsewhere(OsString::from(OTHER))
        );
        assert_eq!(store.names(), vec!["read"], "只报告那一手不许动机器");
    }

    /// 那一行里能读出「谁占着这一枚值」：开发态与安装态的差别就是从这一格看见的。
    #[test]
    fn the_line_keeps_the_other_command_visible() {
        let elsewhere = Held::Elsewhere(OsString::from(OTHER));
        let line = held_line(&elsewhere);
        assert!(line.contains(OTHER), "{line}");
        assert!(line.contains(VALUE_NAME), "{line}");
        assert!(line.contains("别处"), "{line}");

        let report = Report {
            action: Action::Write,
            held: Held::Here,
            command: want(),
        };
        let written = report.line();
        assert!(written.contains(action_line(Action::Write)), "{written}");
        assert!(written.contains("本程序"), "{written}");
        assert!(
            held_line(&Held::Absent).contains("不在"),
            "{}",
            held_line(&Held::Absent)
        );
    }

    /// 「那一格不存在」与「权限被拒」是两件事：前者是默认状态，后者是一句要递给用户的原因。
    #[test]
    fn absent_and_refused_are_different_answers() {
        assert!(answers_absent(ERROR_FILE_NOT_FOUND));
        assert!(answers_absent(ERROR_PATH_NOT_FOUND));
        assert!(answers_absent(ERROR_KEY_DELETED));
        assert!(!answers_absent(ERROR_ACCESS_DENIED));
        assert!(!answers_absent(0));
        assert!(describe_code(ERROR_ACCESS_DENIED).contains("0x00000005"));
        // 认不出的码不猜名字。
        let unknown = describe_code(0x0BADF00D);
        assert!(unknown.contains("没给出"), "{unknown}");
        assert!(unknown.contains("0x0BADF00D"), "{unknown}");
    }

    /// 值的字节与宽串互为往返；认不出的类型码照样读（不许把用户锁在既开不了也关不掉的位置），
    /// 而不砍它的收尾符；长度不是偶数的那一种是真的没法读。
    #[test]
    fn value_bytes_round_trip_and_unkinds_still_read() {
        let units: Vec<u16> = WANT.encode_utf16().collect();
        let bytes = encode_value(&units);
        assert_eq!(decode_value(REG_SZ_KIND, &bytes), Some(units.clone()));
        assert_eq!(
            decode_value(REG_EXPAND_SZ_KIND, &bytes),
            Some(units.clone())
        );
        // 空值 = 一条命令也没有，读回来是空串而不是"没有这一枚值"。
        assert_eq!(
            decode_value(REG_SZ_KIND, &encode_value(&[])),
            Some(Vec::new())
        );
        assert!(known_value_kind(REG_SZ_KIND));
        assert!(!known_value_kind(3)); // REG_BINARY
        assert!(!known_value_kind(7)); // REG_MULTI_SZ
                                       // 三字节：切不出完整的码元。
        assert_eq!(decode_value(REG_SZ_KIND, &[b'a', b'b', b'c']), None);
        // 认不出的类型码照样读回那一串宽字符，只多带着那枚没人承诺过的收尾符。
        let head = &units[..4];
        assert_eq!(
            decode_value(3, &encode_value(head)),
            Some([head, &[0]].concat())
        );
        // 认识的那两种砍收尾符，读回来的就是原串：往返在自家形状上成立。
        assert_eq!(
            decode_value(REG_SZ_KIND, &encode_value(head)),
            Some(head.to_vec())
        );
    }

    /// `current_command` 走的是真 `current_exe()`，但不碰注册表 —— 这里验的是「说的是
    /// 跑着的这一台」，判据里那层引号在别的格子里已经量过。
    #[test]
    fn the_current_command_names_this_exe() {
        let command = current_command().expect("本程序的路径读得出来");
        let exe = std::env::current_exe().unwrap();
        let text = command.to_string_lossy().to_string();
        let raw = exe.to_string_lossy().to_string();
        assert!(
            text.replace('"', "") == raw,
            "命令串就是这一台的路径（可能带引号）：{text} vs {raw}"
        );
        assert!(!text.is_empty(), "{text}");
    }

    /// 真人机上那一手**只读**的证人：`Registry::read` 那几手 unsafe（开键、问长度、读回来、
    /// 拆解）在没有注册表可开的机器上一律量不到，而判据在假机器上已经跑全套 —— 这一格补的正是
    /// 那一头。它只 `RegOpenKeyExW(KEY_READ)` + `RegQueryValueExW`，一个字节也不改。
    ///
    /// 挂着 `#[ignore]`，与 [`crate::open`] 那枚会真开浏览器标签页的格子同一个规矩：默认那一趟
    /// 不开别人机器上的键，要证这一臂的人自己点名跑
    /// （`cargo test -p platform-windows -- --ignored`）。这一格**不**假设机器上一定有或一定没有
    /// 那一枚启动项 —— 那是用户的事；它要证的是「unsafe 那几手在这台机器上真能回话」。改的那一
    /// 手（`put`/`remove`）在这里照样不碰：删掉谁家的启动项是破坏性动作，没有一格该在量具里干它。
    #[cfg(windows)]
    #[test]
    #[ignore = "会打开这台机器真实的 Run 键（只读，但要真人点头）"]
    fn the_real_run_key_answers_a_read_without_reaching() {
        let want = current_command().expect("本程序的命令串拼不出来");
        let held = inspect().expect("读 Run 键失败：原因就在这一句里");
        println!(
            "[autostart witness] want={want:?} held={}",
            held_line(&held)
        );
        if let Held::Elsewhere(other) = &held {
            assert!(!other.is_empty(), "Elsewhere 交回的是一句空话");
            assert!(
                !same_command(other, &want),
                "Elsewhere 与本程序是同一枚命令：{other:?} vs {want:?}"
            );
        }
    }
}
