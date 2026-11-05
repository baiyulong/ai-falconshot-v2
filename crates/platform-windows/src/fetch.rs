//! §5.21.1 第 3 步「系统获取版本信息」里的那一只手：把一句 http(s) 地址取回成一段字。
//!
//! 自 §5.8.4 起同一只手还有第二个读者（[`bytes`]：拖进贴图窗口的那一句网页图片地址，取回的是
//! 图片本身）。两只读者共用的正是这一处 —— 「这一句地址取不取得回」只该有一个答案，而失败那句
//! 原话也只该有一个作者（#281）；各自开一只手，就会有一条路上「取不回」而另一条路上「连不上」。
//!
//! 本模块只有**一只手在碰网络**，与 [`crate::open`] 只有一只手在碰 shell 同一个形状：
//! 「取回来的那串字该怎么判决」不在这里（那是 [`falcon_core::update`]），「这一枚清单该不该
//! 被取」也不在这里（那是调用者那两枚 `[about]` 键）。这里只答一件事：**取回来了没有**。
//!
//! # 为什么是 wininet.dll 而不是自己拼一套 HTTP
//!
//! 一句 URL 背后是 chunked 传输、重定向、代理自动配置、TLS 与系统证书库、以及一整套超时规则。
//! 自己写一套的代价不是行数，是**这一代写下的那套规则将来要和浏览器不一致**：清单取回来是
//! 「有新版」还是「取不到」，取决于这一套客户端规则，而那一处不一致用户只会看成产品在乱说。
//! `wininet` 用的正是这台机器上浏览器那一族走的同一套栈（含 `[about]` 之外没人配的代理：
//! `INTERNET_OPEN_TYPE_PRECONFIG` 让系统那一份代理设置说话，本产品不替用户再决定一次）。
//!
//! # 为什么这里还要**再判一遍**是不是 http(s)
//!
//! 与 [`crate::open::browser`] 同一个理由，而且这里更强：`InternetOpenUrlW` 对 `file:` 会去读
//! 本地文件，对一枚用户可能扫到的地址表达不了「我只取 http(s)」。判它的规则只有一份 ——
//! [`falcon_core::link`]，本模块只**引用**它，不自己再切一遍字符串（自己切一遍就是第二条会各自
//! 漂移的域名解析路，而这一条路的终点是**把外来的字节喂进判决**）。IDN 那一支同样拒绝：取回
//! 一枚同形字域名指向的清单，比打开它更坏 —— 那串字会被当成版本号读。
//!
//! # 上限与「不截断」
//!
//! 调用者给一个字节数上限，超限是**失败**而不是「取回前 N 字节」。一份被剪掉尾巴的清单会读出
//! 半个通道、一行没有版本号的记录，那是这一族最坏的一种绿：它看起来是一次成功的取回。

/// [`text`] 与 [`bytes`] 的答。每一枚都带得上自己那句**产品的话**（`Display`），所以桥、门与
/// 屏幕上那一行说的是同一句原话，不各处再编一遍（#281）。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// 不是 http(s)，或者是 IDN：这一句是本模块**不**取的那些形态，判定出自 [`falcon_core::link`]。
    #[error("这一句不是可以取回的地址：{0}")]
    Refused(String),
    /// 对面答了一个不是 2xx 的状态码。带回去的是那枚数，不是编出来的解释 —— 「404」与「403」在
    /// 用户耳朵里是两件事（前者是那边没有这一份，后者是那台机器不给他）。这一只手现在有两个读者
    /// （`[about] release_feed` 那份清单，与 §5.8.4 拖进来的那一句图片地址），所以这一句说的是
    /// 「这一趟要取的那一份」而不点名任何一种：点名清单的那一版会在图片那一趟里说错东西。
    #[error("那边答的是状态码 {0}，不是这一趟要取的那一份")]
    Http(u32),
    /// 取回来了，但比调用者给的上限大。**没有**一部分字节被交回去。
    #[error("那一份比这一趟允许的大小（{0} 字节）还大，本程序不拿一半的内容当成整份")]
    TooLarge(usize),
    /// 传输层的事：DNS 解析不了、连不上、超时、证书过不去。带回来的是 wininet 那枚码的名字与数，
    /// 编不出解释时至少还有一个数可以查。
    #[error("取不回那一句地址：{0}")]
    Transport(String),
    /// 取回来了、也在上限之内，但那串字节不是 UTF-8。清单是给人写的一行一行字，读不出字来就是
    /// 读不出——这一枚不替它猜一个编码，猜错了会把一份好清单读成一行乱码的通道名。
    #[error("取回来的那一份读不出字来（不是 UTF-8）")]
    NotText,
    #[error("这一台不是 Windows，没有可以取回地址的那只手")]
    Unsupported,
}

/// wininet 那批 `ERROR_INTERNET_*` 码里，用户真会撞上的几枚的名字。
///
/// 与 [`crate::open::shell_code`] 同一族：这条 API 失败时给的是一个数，而 `FormatMessage`
/// 那一头对这批码说的是本地化后的整句话，换一台机器的语言就换一串字。留名字 + 数，是为了
/// 「门那一侧的读数在两台机器上核得上」，也为了那句拒绝里没有英文残句。
#[cfg(windows)]
fn inet_code(code: u32) -> String {
    use windows::Win32::Networking::WinInet::{
        ERROR_INTERNET_CANNOT_CONNECT, ERROR_INTERNET_CONNECTION_ABORTED, ERROR_INTERNET_TIMEOUT,
        ERROR_INTERNET_NAME_NOT_RESOLVED, ERROR_INTERNET_INCORRECT_HANDLE_STATE,
        ERROR_INTERNET_ITEM_NOT_FOUND, ERROR_INTERNET_OPERATION_CANCELLED,
        ERROR_INTERNET_SEC_CERT_DATE_INVALID, ERROR_INTERNET_SEC_CERT_REVOKED,
    };

    let named = match code {
        ERROR_INTERNET_TIMEOUT => "等不到回话（超时）",
        ERROR_INTERNET_NAME_NOT_RESOLVED => "那个域名在这台机器上解析不出来",
        ERROR_INTERNET_CANNOT_CONNECT => "连不上那台服务器",
        ERROR_INTERNET_CONNECTION_ABORTED => "那次连接被中断",
        ERROR_INTERNET_OPERATION_CANCELLED => "这一趟被取消",
        ERROR_INTERNET_SEC_CERT_DATE_INVALID => "那边的证书日期不对",
        ERROR_INTERNET_SEC_CERT_REVOKED => "那边的证书已被吊销",
        ERROR_INTERNET_INCORRECT_HANDLE_STATE => "那一只手的状态不对（内部错误）",
        ERROR_INTERNET_ITEM_NOT_FOUND => "那边没有这一项",
        _ => "wininet 没给名字",
    };
    format!("{named}（系统返回 {code}）")
}

/// 从一次失败里取回那枚**数**。
///
/// 0.62 的绑定把返回 `BOOL` 的那几只（`InternetReadFile`）包成了 `windows_core::Result`，失败时
/// 里面装的是 `Error::from_win32()` —— 也就是 `GetLastError()` 那枚数被抬进 HRESULT 的低 16 位
/// （`0x8007_xxxx`）。从这一枚错误里取数、而不是再问一次 `GetLastError()`：中间只要有一次别的
/// 调用置过 last error，第二次问读到的就不是这次失败的那个数，而那正是门那一侧要核对的读数。
#[cfg(windows)]
fn code_of(error: &windows_core::Error) -> u32 {
    (error.code().0 as u32) & 0xFFFF
}

/// 递给对面那一句 `User-Agent`。产品名与版本号出自这枚 crate 自己，不写死一个数：写死的那一枚
/// 会在版本走到那天开始对外谎报自己是谁，而那正是对面服务器用来决定给什么的那串字。
#[cfg(windows)]
fn agent() -> String {
    format!("falconshot/{} (Windows)", env!("CARGO_PKG_VERSION"))
}

/// 一枚 http(s)、域名全 ASCII 的地址；其余四形态各自交回**自己那句**拒绝。
///
/// 这只手是本模块唯一的闸：[`text`] 与 [`bytes`] 都先过它，因此「闸前拒的那些句一次 socket
/// 都不碰」这一句承诺只有一处需要证（与 [`crate::open::browser`] 同形状）。
#[cfg(windows)]
fn gate(url: &str) -> Result<falcon_core::link::Web, FetchError> {
    use falcon_core::link::Target;

    match falcon_core::link::classify(url) {
        Target::Web(web) if web.openable() => Ok(web),
        Target::Web(web) => Err(FetchError::Refused(format!(
            "{}：域名含非 ASCII 字符，本轮不取它指来的那份内容",
            web.display_host()
        ))),
        Target::Other(scheme) => Err(FetchError::Refused(format!(
            "scheme 是 {scheme}，这一只手只取 http(s)"
        ))),
        Target::NotALink => Err(FetchError::Refused("读不出主机名".to_string())),
    }
}

/// 取回那一句地址的字节，最多 `limit` 枚。
///
/// 三只句柄（session / 连接 / 请求）里这一只手只用两只：`InternetOpenW` 开的那一枚代表这一进程
/// 的这次访问，`InternetOpenUrlW` 把「解析地址、连上去、发请求、收响应头」一次做完并交回请求
/// 句柄。两只都必须关掉，而关这件事**只有一处**：下面那一对守卫（见 `Guard` 头顶）。wininet 的
/// 句柄不落地成 Rust 的 `Drop`，手写 `InternetCloseHandle` 的每一条出路都是漏一只内核对象的地方。
#[cfg(windows)]
pub fn bytes(url: &str, limit: usize) -> Result<Vec<u8>, FetchError> {
    use crate::open::wide;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Networking::WinInet::{
        HttpQueryInfoW, InternetCloseHandle, InternetOpenUrlW, InternetOpenW, InternetReadFile,
        InternetSetOptionW, HTTP_QUERY_FLAG_NUMBER, HTTP_QUERY_STATUS_CODE,
        INTERNET_FLAG_NO_CACHE_WRITE, INTERNET_FLAG_NO_COOKIES, INTERNET_FLAG_NO_UI,
        INTERNET_FLAG_RELOAD, INTERNET_OPEN_TYPE_PRECONFIG, INTERNET_OPTION_CONNECT_TIMEOUT,
        INTERNET_OPTION_RECEIVE_TIMEOUT, INTERNET_OPTION_SEND_TIMEOUT,
    };

    let web = gate(url)?;

    // 这一只手的超时是三只句柄上的一组，不是「一次总时长」：connect 是 TCP 那一头，send/receive
    // 是每一次读写的静默。三枚都给同样的数，是因为本产品要的是一次**能等完**的取回——调用它的
    // 那一头（`crate::session` 的更新那一臂）跑在另一条线程上，等久了也只是屏幕上那一行还写着
    // 「正在取回…」，不会把 GUI 线程钉住。
    let connect_ms = 8_000u32;
    let agent_name = wide(&agent());
    let target = wide(&web.url);
    // `INTERNET_FLAG_RELOAD` + `NO_CACHE_WRITE`：一份版本清单要的是**此刻**那一份。wininet 默认
    // 会按响应头缓存，而「用户按了一次检查更新，读到的是三天前那一份」恰好是这一枚功能最不能说
    // 的一句话——它说了「已是最新」，而那不是这次问出来的。
    let flags = INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_NO_UI
        // 清单是一次性的读，不给它写 cookie，也不给它带过去：本产品在这条路上没有会话。
        | INTERNET_FLAG_NO_COOKIES;

    // wininet 的句柄不落地成 Rust 的 `Drop`，而这个函数有五条出路。一只最小的守卫把「关」这件事
    // 收到一处：`session` 声明在前、`request` 在后，于是离开作用域时先关 `request` 再关
    // `session`，那正是 wininet 要的顺序；而任何一条 `return` 都带得走它们。忘了关的那一只是一枚
    // 内核对象，而这一只手会被「检查更新」按很多次。
    struct Guard(*const core::ffi::c_void);
    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = InternetCloseHandle(self.0);
                }
            }
        }
    }

    let session = unsafe {
        InternetOpenW(
            PCWSTR::from_raw(agent_name.as_ptr()),
            INTERNET_OPEN_TYPE_PRECONFIG.0,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    };
    if session.is_null() {
        return Err(FetchError::Transport(inet_code(unsafe {
            GetLastError().0
        })));
    }
    let _session = Guard(session);
    let dwell = connect_ms.to_le_bytes();
    for option in [
        INTERNET_OPTION_CONNECT_TIMEOUT,
        INTERNET_OPTION_SEND_TIMEOUT,
        INTERNET_OPTION_RECEIVE_TIMEOUT,
    ] {
        unsafe {
            // 设不上就算了：那三枚值是 wininet 的默认数，默认比我们的数更长，用户等得久而不是
            // 取不到。这一处不因此拒绝一次取回，也不假装成功过——没有读数可报。
            let _ = InternetSetOptionW(
                Some(session),
                option,
                Some(dwell.as_ptr() as *const core::ffi::c_void),
                dwell.len() as u32,
            );
        }
    }

    let request = unsafe {
        InternetOpenUrlW(
            session,
            PCWSTR::from_raw(target.as_ptr()),
            None,
            flags,
            None,
        )
    };
    if request.is_null() {
        let code = unsafe { GetLastError().0 };
        // 状态码在这一刻还问不到（连上去之前就没走到响应那一步），所以这一支交回的是传输层的
        // 那枚码，不是「那边答了什么」。两支的话术分得开，用户才知道该查网络还是该查那份清单。
        return Err(FetchError::Transport(inet_code(code)));
    }
    let _request = Guard(request);

    // 从这一行往下，五条出路共用上面那两只守卫，一处 close 都不用手写。
    let mut status = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    if unsafe {
        HttpQueryInfoW(
            request,
            HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER,
            Some(&mut status as *mut u32 as *mut core::ffi::c_void),
            &mut size,
            None,
        )
    }
    .is_err()
    {
        return Err(FetchError::Transport(
            "那边没给出状态码（响应头读不出）".to_string(),
        ));
    }
    if !(200..300).contains(&status) {
        return Err(FetchError::Http(status));
    }

    let mut body: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let mut read = 0u32;
        let outcome = unsafe {
            InternetReadFile(
                request,
                chunk.as_mut_ptr() as *mut core::ffi::c_void,
                chunk.len() as u32,
                &mut read,
            )
        };
        if let Err(error) = outcome {
            return Err(FetchError::Transport(inet_code(code_of(&error))));
        }
        if read == 0 {
            break;
        }
        let took = read as usize;
        if body.len() + took > limit {
            // 注意这里交回的是**上限**而不是已经读了多少：已经读的那些字节一个都不交回去，
            // 所以「半份清单进了判决」这条路在本模块里结构性地不存在。
            return Err(FetchError::TooLarge(limit));
        }
        body.extend_from_slice(&chunk[..took]);
    }
    Ok(body)
}

/// 取回那一句地址并**读成字**。`Ok` 只到「那一份是 UTF-8 且在上限之内」为止：内容是不是一行
/// 一个通道的清单，判 [`falcon_core::update`] 那一层，不在这只手。
#[cfg(windows)]
pub fn text(url: &str, limit: usize) -> Result<String, FetchError> {
    match bytes(url, limit) {
        Ok(body) => String::from_utf8(body).map_err(|_| FetchError::NotText),
        Err(e) => Err(e),
    }
}

#[cfg(not(windows))]
pub fn bytes(_url: &str, _limit: usize) -> Result<Vec<u8>, FetchError> {
    Err(FetchError::Unsupported)
}

#[cfg(not(windows))]
pub fn text(_url: &str, _limit: usize) -> Result<String, FetchError> {
    Err(FetchError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 闸那一臂**可以**测，也必须测：这一族的承诺是「过不了 [`falcon_core::link`] 那一道判的
    /// 那一句，一次 socket 都不碰」。正面那一臂不在单测里，与 [`crate::open::browser`] 头顶
    /// 写死的同一条规矩（#261 的那一族的反面是「拿一次真取回当判据」）：真的 `InternetOpenUrlW`
    /// 会往这台机器外面发一次请求，而单测不该替用户决定这件事。它由 `--update-check` 那一档
    /// 对着**本机**起的服务器量（见 `crates/qt_bridge/src/update_check.rs` 的取回那两行）。
    #[test]
    fn a_non_http_url_never_reaches_the_network() {
        for text in [
            "file:///C:/Windows/system32/cmd.exe",
            "ftp://example.invalid/RELEASES.txt",
            "javascript:alert(1)",
            "mailto:someone@example.com",
            "notion://x",
            "cmd.exe",
            "",
            "http://",
        ] {
            let err = bytes(text, 1024).unwrap_err();
            assert!(
                matches!(
                    err,
                    FetchError::Refused(_) | FetchError::Unsupported | FetchError::Transport(_)
                ),
                "wrong answer for {text:?}: {err}"
            );
            assert!(
                !matches!(err, FetchError::Http(_) | FetchError::TooLarge(_) | FetchError::NotText),
                "{text:?} 被读成了「已经取回来过」：{err}"
            );
        }
    }

    /// IDN 那一支：它是 http 链接，但本轮不取。这里与 `open::browser` 共用 [`falcon_core::link`]
    /// 那一份判据，所以界面灰掉的理由、开浏览器被拒的理由与取回被拒的理由是同一句原话，
    /// 不是三处各切一遍字符串。
    #[test]
    fn an_idn_address_is_refused_rather_than_fetched() {
        let err = bytes("https://exämple.com/RELEASES.txt", 1024).unwrap_err();
        match err {
            FetchError::Refused(why) => {
                assert!(why.contains("非 ASCII"), "{why}");
                assert!(why.contains("exämple.com"), "{why}");
            }
            FetchError::Unsupported => {}
            other => panic!("wrong: {other}"),
        }
    }

    /// `127.0.0.1:端口` 这种**带端口的 IP** 必须过闸：`--update-check` 那一档对着本机起的那一扇
    /// 服务器走的就是这一枚形态，而它在闸前被拒的话，那一档读到的「取不到」就不是网络的事，
    /// 是这一只手的判据过窄。测的是**闸**本身，不是取回：一次真的 `bytes` 会真去连 8 号口，
    /// 而那台机器上有没有东西在听是环境的事，不是这一条判据的事（#261 反着读同样成立）。
    #[test]
    fn a_loopback_address_with_a_port_passes_the_gate() {
        #[cfg(windows)]
        {
            let web = gate("http://127.0.0.1:8443/RELEASES.txt").expect("loopback 被闸拦下了");
            assert_eq!(web.host, "127.0.0.1");
            assert_eq!(web.port, Some(8443));
            assert!(web.openable());
            // 默认口不额外说：`http://127.0.0.1/` 与 `http://127.0.0.1:80/` 是同一台服务。
            assert_eq!(gate("http://127.0.0.1/RELEASES.txt").unwrap().port, None);
        }
    }

    /// 上限那句话得说得出**那个数**：门与屏幕都只印这句话，用户要判断的是「清单写得太长」还是
    /// 「本程序不许这么大」。
    #[test]
    fn the_size_cap_reports_the_number_it_used() {
        let err = FetchError::TooLarge(4096).to_string();
        assert!(err.contains("4096"), "{err}");
        assert!(err.contains("一半"), "{err}");
    }

    /// 状态码与传输码是两件事，必须说出两句话（#453）：一份 404 的清单和一台连不上的服务器，
    /// 用户查的方向完全不同。
    #[test]
    fn a_status_code_and_a_transport_failure_say_different_things() {
        assert!(FetchError::Http(404).to_string().contains("404"));
        assert!(FetchError::Http(403).to_string().contains("403"));
        assert!(FetchError::Http(404).to_string() != FetchError::Http(403).to_string());
        #[cfg(windows)]
        {
            use windows::Win32::Networking::WinInet::{
                ERROR_INTERNET_NAME_NOT_RESOLVED, ERROR_INTERNET_TIMEOUT,
            };
            let timeout = inet_code(ERROR_INTERNET_TIMEOUT);
            let dns = inet_code(ERROR_INTERNET_NAME_NOT_RESOLVED);
            assert!(timeout.contains("超时") && timeout.contains("12002"), "{timeout}");
            assert!(dns.contains("解析") && dns.contains("12007"), "{dns}");
            assert!(inet_code(99999).contains("系统返回 99999"));
        }
    }

    /// 那一串 `User-Agent` 不许写死版本号：写死的那一枚会在版本走到那天开始对外谎报自己是谁，
    /// 而那正是对面用来决定给什么内容的一串字。
    #[test]
    fn the_agent_carries_the_version_this_build_was_built_with() {
        #[cfg(windows)]
        {
            let agent = agent();
            assert!(agent.starts_with("falconshot/"), "{agent}");
            assert!(
                agent.contains(env!("CARGO_PKG_VERSION")),
                "{agent} 没带上 {}",
                env!("CARGO_PKG_VERSION")
            );
        }
    }
}
