//! §5.8.3 第 1 步后半「或将文件拖到应用图标」的机器腿。
//!
//! Explorer 把文件拖到 `falconshot.exe`（或它的快捷方式）上时，Windows 的做法是**再起一趟
//! 进程**，把被拖的那几枚路径当 argv 递进来 —— 托盘图标那一枚收不到拖放（`Shell_NotifyIcon`
//! 没有放文件的目标接口，见 PRD §5.8.3 实现口径注）。所以这一节要的两只手是：
//!
//! - [`claim`]：**我来当那个收活的**（一把命名互斥体；抢到就是这一枚）；
//! - [`handover`] / [`Receiver`]：把那一批路径**递**过去、那一个**接**这一批（那根命名管道）。
//!
//! 这一层只做机器上的那几件事，**不判路径是不是图片**：那一句由
//! [`falcon_core::pin::read_picture_batch`] 说（§5.8.3 第 3 步那一位），本层把它递到手就收工。
//!
//! 坏法各说各话（#453）：「管道的名字不存在」是**没人接**（[`Handover::NoReceiver`]），
//! 「它在、但正忙着」是**等一等**（[`Handover::Busy`]），「写不进去」是**递不过去**
//! （[`Handover::Failed`]）—— 三件事不是一件事。

use std::path::PathBuf;

/// 锁的名字：本地会话内唯一（`Local\` 那一段 = 这一台机器上这个登录会话）。
pub const RECEIVER_MUTEX_NAME: &str = r"Local\ai-falconshot-receiver";

/// 管道的名字：**本地会话内**唯一（名字后面那一段是会话号）。
///
/// 管道没有互斥体那种 `Local\` 前缀（那是命名对象才有的），所以两个登录在同一台机器上的用户各起
/// 一枚产品时，两边都建 `\\.\pipe\ai-falconshot-drop` 时，后来那一枚会跟先来的那一枚**共用**同一根
/// 管道（`PIPE_UNLIMITED_INSTANCES` 允许同名多实例）—— 于是两个用户各自的「拖到图标」会递到对方
/// 手里。会话号把那两枚管道分开（互斥体那一头已经是 `Local\`，会话内唯一）。
pub fn drop_pipe_name() -> String {
    format!(r"\\.\pipe\ai-falconshot-drop-{}", session_id())
}

/// 这一枚进程所在的**会话号**。
///
/// 读不到时答 0：那也是一个稳定的号 —— 同一台机器上读不到的那些进程仍然共用同一根管道（与从前
/// 一样），不会两两错开、也不会这一枚与下一枚错开。
#[cfg(windows)]
fn session_id() -> u32 {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;

    let mut sid = 0u32;
    unsafe {
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut sid);
    }
    sid
}

/// 这一台不是 Windows：没有会话号那一说，答 0（那也就是唯一的号）。
#[cfg(not(windows))]
fn session_id() -> u32 {
    0
}

/// 递文件的结果：四种答法各说各话。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handover {
    /// 交给了已经在跑的那一个。
    Handed,
    /// 那个接收口压根不存在（没人跑，或跑着的那一个还没把它开出来）。
    NoReceiver,
    /// 它在，但它正忙着（管道满了 / 上一趟还没接完）。
    Busy(String),
    /// 递不过去（写不进去、权限不够）。
    Failed(String),
}

/// 那一批路径的字节形状：先四字节的枚数，再逐枚「四字节字节数 + UTF-16LE 的那一句」。
///
/// UTF-16 而不是 UTF-8：Windows 上那一条路径本来就是一串宽字符，而它可以带着**落单的代理项**
/// （UTF-8 编不出来，编出来就是另一个字）。所以这里用 [`std::os::windows::ffi::OsStrExt`] 的
/// `encode_wide` 逐字搬，搬回来的那一句就是原来那一句。
#[cfg(windows)]
pub fn encode_batch(paths: &[PathBuf]) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    let mut out = Vec::new();
    out.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    for path in paths {
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let bytes: Vec<u8> = wide.iter().flat_map(|u| u.to_le_bytes()).collect();
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    out
}

/// [`encode_batch`] 的对面：把那一段字节读回那几枚路径。
///
/// 少一枚或多一枚都读成**空**（而不是读出一半）：那一段字节坏掉时，收到半批路径比收到零枚更
/// 难说清 —— 屏幕上的贴图会少掉几枚，而没有任何一句话解释过。
#[cfg(windows)]
pub fn decode_batch(bytes: &[u8]) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let end = at.checked_add(n)?;
        if end > bytes.len() {
            return None;
        }
        let slice = &bytes[at..end];
        at = end;
        Some(slice)
    };
    let Some(count) = take(4) else {
        return Vec::new();
    };
    let count = u32::from_le_bytes([count[0], count[1], count[2], count[3]]) as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(len) = take(4) else {
            return Vec::new();
        };
        let len = u32::from_le_bytes([len[0], len[1], len[2], len[3]]) as usize;
        let Some(body) = take(len) else {
            return Vec::new();
        };
        if len % 2 != 0 {
            return Vec::new();
        }
        let wide: Vec<u16> = body
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        out.push(PathBuf::from(std::ffi::OsString::from_wide(&wide)));
    }
    out
}

/// 那一把锁的名字（宽字符，带结尾的 NUL）。
#[cfg(windows)]
fn wide_name(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}

/// **我来当那个收活的**：建那把锁，建得出来就是这一枚。
///
/// 建不出来（`ERROR_ALREADY_EXISTS`）时答「不是我」—— 那台机器上已经有一个在跑的了（§5.8.3
/// 第 1 步后半那一趟要的正是把那一批交给它），所以这不是失败。
///
/// 建出来的那把**留着不关**：这个会话内的命名对象随最后一个句柄消失，所以这一把得由这一枚进程
/// 一直开着（句柄的值记在 [`OWNED`] 里，而句柄本身不关）。
///
/// 这一问是**一次**的：抢锁与「是不是有人在跑」是同一句话，不另设第二只手去问第二遍（#281）。
#[cfg(windows)]
pub fn claim() -> bool {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;

    /// 手里那把锁的句柄的值。句柄**不关**：会话内的命名对象随最后一个句柄消失，而这一枚进程
    /// 要一直当那个「有人在跑」的存在 —— 所以这里存的是句柄的值，句柄由这个进程一直开着。
    static OWNED: AtomicUsize = AtomicUsize::new(0);

    if OWNED.load(Ordering::SeqCst) != 0 {
        // 已经是自己手里那把了 —— 同一枚进程里再问一次，答的还是「是我」。
        return true;
    }
    let name = wide_name(RECEIVER_MUTEX_NAME);
    let handle = unsafe { CreateMutexW(None, false, PCWSTR::from_raw(name.as_ptr())) };
    let handle = match handle {
        Ok(h) => h,
        Err(_) => return false,
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return false;
    }
    OWNED.store(handle.0 as usize, Ordering::SeqCst);
    true
}

/// 把那一批路径交给已经在跑的那一个（交到那根**默认**管道上）。
pub fn handover(paths: &[PathBuf]) -> Handover {
    handover_to(&drop_pipe_name(), paths)
}

/// 把那一批路径交给**那根管道**上的那一个。
///
/// 「管道一个实例都没有」与「那个实例正忙着」是两件事：前者是 [`Handover::NoReceiver`]（没人
/// 接），后者是 [`Handover::Busy`]（等它空出来就行）。所以这里先等、再连，三种坏法分开答。
#[cfg(windows)]
pub fn handover_to(pipe: &str, paths: &[PathBuf]) -> Handover {
    use windows::core::{HRESULT, PCWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_SEM_TIMEOUT, GENERIC_READ,
        GENERIC_WRITE, GetLastError,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, OPEN_EXISTING,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;

    if paths.is_empty() {
        return Handover::Handed;
    }
    let name = wide_name(pipe);

    // 先问一句「有没有人在听」：管道一个实例都没有时这个名字不存在，而那一句话在产品那边是
    // 「没人接」，不是「递不过去」，所以这里先分家。
    if !unsafe { WaitNamedPipeW(PCWSTR::from_raw(name.as_ptr()), 1_000) }.as_bool() {
        let err = unsafe { GetLastError() };
        if err == ERROR_FILE_NOT_FOUND {
            return Handover::NoReceiver;
        }
        if err == ERROR_SEM_TIMEOUT || err == ERROR_PIPE_BUSY {
            return Handover::Busy(format!("那个接收口正忙着（Win32 {err:?}）"));
        }
        return Handover::Failed(format!("问那个接收口时它没答（Win32 {err:?}）"));
    }

    let handle = unsafe {
        CreateFileW(
            PCWSTR::from_raw(name.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    let handle = match handle {
        Ok(h) => h,
        Err(e) => {
            // 这一趟的坏法由 Win32 的码说：管道刚被人关掉是「没人接」，管道正占着是「等一等」。
            let code = e.code();
            if code == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) {
                return Handover::NoReceiver;
            }
            if code == HRESULT::from_win32(ERROR_PIPE_BUSY.0) {
                return Handover::Busy(format!("那个接收口正忙着（{e}）"));
            }
            return Handover::Failed(format!("连不上那个已经在跑的：{e}"));
        }
    };
    let payload = encode_batch(paths);
    let mut written = 0u32;
    let written_result = unsafe {
        windows::Win32::Storage::FileSystem::WriteFile(
            handle,
            Some(&payload),
            Some(&mut written),
            None,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    match written_result {
        Ok(()) => Handover::Handed,
        Err(e) => Handover::Failed(format!("写不过去：{e}")),
    }
}

/// 这一台不是 Windows：递不过去。
#[cfg(not(windows))]
pub fn handover(_paths: &[PathBuf]) -> Handover {
    Handover::Failed("这一台不是 Windows：没有那根管道".to_string())
}

/// 这一台不是 Windows：递不过去。
#[cfg(not(windows))]
pub fn handover_to(_pipe: &str, _paths: &[PathBuf]) -> Handover {
    Handover::Failed("这一台不是 Windows：没有那根管道".to_string())
}

/// 收活的那一端：一根管道，等着别人把路径递进来。
#[cfg(windows)]
pub struct Receiver {
    handle: windows::Win32::Foundation::HANDLE,
}

// 那个句柄只在**接活的那一条线程**上碰（[`Receiver::accept`] 那一支）：它在 [`bind`] 那一头
// 造出来，随后整趟都在接收线程上被读。`HANDLE` 本身是个裸指针（不是 `Send`），而这一趟的
// 「造它的那一趟」与「读它的那一趟」是同一趟 —— 所以换线程是安全的，这里把这句话写下来。
#[cfg(windows)]
unsafe impl Send for Receiver {}

#[cfg(windows)]
impl Receiver {
    /// 把管道造出来（名字也就此存在了 —— 于是「有没有人在跑」这一句话有了机器上的对面）。
    pub fn bind() -> Result<Self, String> {
        Receiver::bind_named(&drop_pipe_name())
    }

    /// 把**那根管道**造出来。
    ///
    /// 名字是参数，是为了让证人与产品各走各的管道（一个进程里两个同名管道会各接一半的客户端）。
    pub fn bind_named(pipe: &str) -> Result<Self, String> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::GetLastError;
        use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        };

        let name = wide_name(pipe);
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR::from_raw(name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                None,
            )
        };
        if handle.is_invalid() {
            return Err(format!("管道造不起来：{:?}", unsafe { GetLastError() }));
        }
        Ok(Self { handle })
    }

    /// 接一趟：等一个客户端把那一批递进来，读回来。
    ///
    /// 读不到东西时交回 `Ok(None)`（客户端把话说到一半就走了），不是错误 —— 那一趟没有路径，
    /// 而「没有路径」是屏幕上不必多说一句的事。
    pub fn accept(&self) -> Result<Option<Vec<PathBuf>>, String> {
        use windows::Win32::Foundation::{
            ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, GetLastError,
        };
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};

        unsafe {
            if let Err(_e) = ConnectNamedPipe(self.handle, None) {
                // 两种都不是错：**客户端在 `ConnectNamedPipe` 之前就接上了**（那一头写得很急，
                // 答是 ERROR_PIPE_CONNECTED / 535），或者**接上又走了**（答是 ERROR_NO_DATA /
                // 232 —— 写进去的那几句话还在管道缓冲里，下面那一段读得回来）。
                //
                // 这两条不是"我这条腿的坏法"，是「客户端比我快」这一种正常形状 —— 把它们当成错，
                // 就会把用户拖进来的那一批判成"接收口出错了"，而那一批其实好好地躺在管道里。
                let err = unsafe { GetLastError() };
                if err != ERROR_PIPE_CONNECTED && err != ERROR_NO_DATA {
                    return Err(format!("接不上那一头：{:?}", err));
                }
            }
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let mut read = 0u32;
                match ReadFile(self.handle, Some(&mut chunk), Some(&mut read), None) {
                    Ok(()) => {
                        if read == 0 {
                            // 客户端把话说完就关了 —— 那一批到此为止。
                            break;
                        }
                        buf.extend_from_slice(&chunk[..read as usize]);
                    }
                    Err(_e) => {
                        if GetLastError() == ERROR_BROKEN_PIPE {
                            break;
                        }
                        return Err(format!("读不出那一批：{:?}", GetLastError()));
                    }
                }
            }
            let _ = DisconnectNamedPipe(self.handle);
            if buf.is_empty() {
                return Ok(None);
            }
            Ok(Some(decode_batch(&buf)))
        }
    }
}

/// 这一台不是 Windows：没有接收口。
#[cfg(not(windows))]
pub struct Receiver;

#[cfg(not(windows))]
impl Receiver {
    /// 这一台不是 Windows：没有管道可开。
    pub fn bind() -> Result<Self, String> {
        Err("这一台不是 Windows：没有那根管道".to_string())
    }

    /// 这一台不是 Windows：没有管道可开。
    pub fn bind_named(_pipe: &str) -> Result<Self, String> {
        Err("这一台不是 Windows：没有那根管道".to_string())
    }

    /// 这一台不是 Windows：接不到任何东西。
    pub fn accept(&self) -> Result<Option<Vec<PathBuf>>, String> {
        Err("这一台不是 Windows：没有那根管道".to_string())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 那一段字节的形状：**逐字往返**。Windows 上那一条路径可以带着落单的代理项，UTF-8 编不出
    /// 来（编出来是另一个字），所以这一格钉的是宽字符那一条路。
    #[test]
    fn the_batch_survives_the_wire_byte_for_byte() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let plain = PathBuf::from(r"C:\Users\baiyulong\图片\a b.png");
        let lonely = PathBuf::from(OsString::from_wide(&[0xD800u16, 0x0061u16]));
        let batch = vec![plain.clone(), lonely.clone(), PathBuf::from(r"C:\x\y.png")];
        let bytes = encode_batch(&batch);
        let back = decode_batch(&bytes);
        assert_eq!(back, batch, "{back:?}");
    }

    /// 坏掉的那一段字节读成**零枚**，而不是半批：屏幕上少掉几枚贴图而没有任何一句话解释过，
    /// 是这一族最坏的形状。
    #[test]
    fn a_broken_batch_reads_as_nothing_at_all() {
        let good = encode_batch(&[PathBuf::from(r"C:\a.png")]);
        assert_eq!(decode_batch(&good).len(), 1);
        assert!(decode_batch(&[]).is_empty());
        assert!(decode_batch(&good[..good.len() - 1]).is_empty());
        assert!(decode_batch(&good[..2]).is_empty());
    }

    /// 那把锁是**幂等**的：同一个进程里问两次，答的都是「是我」—— 锁是自己手里那把，
    /// 自己不该把自己挤掉。
    #[test]
    fn the_receiver_claim_is_idempotent_inside_one_process() {
        assert!(claim(), "第一次问：这一枚就是那个收活的");
        assert!(claim(), "再问一次：还是自己，不该把自己挤掉");
    }

    /// 那一批路径**真的走一趟管道**：这一格对着机器上的那根管道递一次、接一次，收到的是
    /// **逐字相同**的那几枚路径（不是一个「大概对」的形状）。
    ///
    /// 这一格钉的是 §5.8.3 第 1 步后半那一支的**通路**（`这一趟进程 → 管道 → 那一个`），
    /// 而「Explorer 到底会不会把路径递成 argv」是 shell 的事，不是这一格的证人能问的。
    ///
    /// 管道名带上这一趟的进程号：产品那根管道上也许正连着别的一批，而证人与产品各走各的管道，
    /// 一根管道上两个实例会各接一半的客户端（那是 Windows 的规矩，不是这一处的选择）。
    #[test]
    fn a_batch_travels_through_the_pipe_byte_for_byte() {
        let pipe = format!(r"\\.\pipe\ai-falconshot-drop-proof-{}", std::process::id());
        let receiver = Receiver::bind_named(&pipe).expect("管道造得起来");
        let batch = vec![PathBuf::from(r"C:\一\二.png"), PathBuf::from(r"C:\三.png")];
        let got = std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = got.clone();
        let reader = std::thread::spawn(move || {
            let mut arrived = sink.lock().unwrap();
            *arrived = receiver.accept().expect("接得住");
        });

        match handover_to(&pipe, &batch) {
            Handover::Handed => {}
            other => panic!("递不过去：{other:?}"),
        }
        reader.join().expect("接收那一头不炸");
        let arrived = got.lock().unwrap().clone().expect("收到了一批");
        assert_eq!(arrived, batch, "{arrived:?}");
    }
}
