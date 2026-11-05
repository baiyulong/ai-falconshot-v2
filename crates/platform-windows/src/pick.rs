//! §5.8.3 第 1–2 步的那一扇门：开一扇**真的** Windows 文件对话框，把用户挑中的那几枚路径
//! 交回给产品。
//!
//! 这一模块只做「取路径」这一手，别的什么都不判：收哪几种格式、一次收几枚、坏的那几枚怎么
//! 归账、屏幕上一句话怎么说 —— 全在 [`falcon_core::pin`] 那一批（`read_picture_batch` 与
//! `FilePinBatch`）。分成两半的理由不是整洁，而是**可判**：判断那半必须能在没有屏幕、也没有
//! 一双手放在鼠标上的机器上被问一遍，而开一扇对话框既不返回得确定、也不许出现在 `cargo test`
//! 里（§9.1 第十五条 headless 那一半）。
//!
//! 为什么走 shell 的 `IFileOpenDialog` 而不是自己拼一扇窗：§5.8.3 要的是「用户从托盘菜单选择
//! “打开图片并贴图”」之后那一眼就认得出的东西 —— 这台机器上每个人存文件都在用的那一扇。自己
//! 画一扇列目录的窗会多一份 Windows 已经替我们维护过的行为（网络邻居、回收站外的驱动器、
//! 长路径、输入法、拖放、地址栏），而那一份维护不在我们的预算里。
//!
//! [`crate::reveal`] 已经立过同一族规矩：shell 那一头要的是**数据**而不是一条命令行，宽字符
//! 的编法全 crate 只有一份（[`crate::open::wide`]），而 `CoInitializeEx` 的返回值是套间状态
//! 不是成败、丢掉它是有意的。这三条这里一条都不重新发明。

use std::path::PathBuf;

/// 一扇对话框递给用户的一行过滤器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter<'a> {
    /// 用户在过滤器那一下拉里读到的名字，例如「图片」。
    pub name: &'a str,
    /// 分号分隔的模式串，例如 `*.png;*.jpg`。**这一串的作者不在本模块** —— §5.8.3 那一列
    /// 七种格式住在 [`falcon_core::pin::picture_filter_patterns`]，改那一天只需要改一处。
    pub patterns: &'a str,
}

/// 一次挑选要什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    /// 对话框的标题，例如「打开图片并贴图」。
    pub title: &'a str,
    /// 过滤器那一列，顺序就是 shell 显示的顺序。空 = 不设过滤器（shell 自己的默认那一行）。
    pub filters: Vec<Filter<'a>>,
    /// 一次能不能挑多枚。§5.8.3 第 2 步那一句「选择图片文件」按一次挑多枚办，因为「一次选
    /// 三枚得三扇窗」是这一节要成立的一条，而不是一句可以省的话。
    pub multi: bool,
}

/// 用户那一手挑完了没有。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// 按了「打开」：路径按对话框列出的顺序交回，一枚也没有被本模块筛过。
    Picked(Vec<PathBuf>),
    /// 按了取消、或把窗关掉了。这一格与 [`Answer::Picked`] 里那个空列表是同一件事，而它与
    /// `Err` **不是**：一句「对话框开不起来」不能冒充「用户没挑」。
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PickError {
    /// 那扇窗压根没造出来（shell 的组件取不到、或它的返回码不是「用户取消」那一种）。
    ///
    /// 带回来的是 shell 自己那句话，不是一段编出来的解释：这一族坏掉的形状太多（组件没注册、
    /// 策略禁了文件对话框、套间模型不对），而只有 shell 知道自己为什么不肯。
    #[error("文件对话框开不起来：{0}")]
    Shell(String),
    #[error("这一台不是 Windows，没有可以打开的文件对话框")]
    Unsupported,
}

/// 开那扇窗，把挑中的路径交回来。
///
/// 这是唯一一只手会真阻塞：`Show` 对调用它的线程是模态的，所以它**必须**在拥有 Qt 的那条线程
/// 上被叫（`hotkey.rs` 的 `run` 那一手正是那里），不能在热键泵那条线程上 —— 后者一阻塞，
/// `WM_HOTKEY` 就没人收了。
#[cfg(windows)]
pub fn pick(request: &Request<'_>) -> Result<Answer, PickError> {
    use std::ffi::{c_void, OsString};
    use std::os::windows::ffi::OsStringExt;

    use windows::core::{HRESULT, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        Common::COMDLG_FILTERSPEC, FileOpenDialog, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST,
        FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FILEOPENDIALOGOPTIONS, IFileOpenDialog,
        SIGDN_FILESYSPATH,
    };

    // 与 [`crate::reveal::select`] 逐字同一条理由：这一条线程可能早就被 Qt 起过 COM（剪贴板那
    // 一路就要），那时返回 S_FALSE；换个套间模型返回 RPC_E_CHANGED_MODE。两种都不必中止，也
    // 不在这里 CoUninitialize —— 返回值是套间状态而不是成败。
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    // `FileOpenDialog` 在这里是那个 **clsid 常量**（`windows` 把 coclass 的 GUID 发成了同名的
    // 一枚常量），`IFileOpenDialog` 才是要拿的那个接口。
    let dialog: IFileOpenDialog = unsafe {
        CoCreateInstance(
            &FileOpenDialog,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(|e| PickError::Shell(e.to_string()))?
    };

    // 宽字符的缓冲区要有名字、要活过这一次调用：`PCWSTR` 只是一枚指针，shell 不会替我们留着
    // 那份字符串。所以先把每一串编出来收进 `units`，再让 `specs` 指向它。
    let title = crate::open::wide(request.title);
    let mut units: Vec<Vec<u16>> = Vec::with_capacity(request.filters.len() * 2);
    for filter in &request.filters {
        units.push(crate::open::wide(filter.name));
        units.push(crate::open::wide(filter.patterns));
    }
    let specs: Vec<COMDLG_FILTERSPEC> = request
        .filters
        .iter()
        .zip(units.chunks_exact(2))
        .map(|(_, pair)| COMDLG_FILTERSPEC {
            pszName: PCWSTR(pair[0].as_ptr()),
            pszSpec: PCWSTR(pair[1].as_ptr()),
        })
        .collect();

    // 三枚选项各有要挡的那一件事：`ALLOWMULTISELECT` 是「一次挑三枚」那一问的对面；
    // `FILEMUSTEXIST` + `PATHMUSTEXIST` 让对话框自己拒绝一个不存在的名字，于是产品这一头
    // 读不到的那一种坏（名字打错了）由 Windows 说，而不是由我们编一句。
    //
    // `FORCEFILESYSTEM` 挡的是那一类「看起来是枚文件、其实是个 shell 命名空间里的东西」
    // （库、回收站里那一枚）：它们没有 `SIGDN_FILESYSPATH`，交回给我们的就是一枚读不出来的
    // 名字，而那种东西本就不该进 §5.8.3 那一批。
    let mut options = FOS_FILEMUSTEXIST.0 | FOS_PATHMUSTEXIST.0 | FOS_FORCEFILESYSTEM.0;
    if request.multi {
        options |= FOS_ALLOWMULTISELECT.0;
    }

    unsafe {
        if !request.title.is_empty() {
            dialog
                .SetTitle(PCWSTR::from_raw(title.as_ptr()))
                .map_err(|e| PickError::Shell(e.to_string()))?;
        }
        if !specs.is_empty() {
            dialog
                .SetFileTypes(&specs)
                .map_err(|e| PickError::Shell(e.to_string()))?;
        }
        dialog
            .SetOptions(FILEOPENDIALOGOPTIONS(options))
            .map_err(|e| PickError::Shell(e.to_string()))?;

        // `Show` 交回 `ERROR_CANCELLED` 那一枚码，就是「用户按了取消」：它是这一只手最常到的
        // 一种答，不是坏。剩下那些（组件没注册、策略禁了文件对话框）才是坏，而坏的理由归
        // shell 自己说，不由这里编一句。
        if let Err(e) = dialog.Show(None::<HWND>) {
            if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(Answer::Cancelled);
            }
            return Err(PickError::Shell(e.to_string()));
        }

        let results = dialog
            .GetResults()
            .map_err(|e| PickError::Shell(e.to_string()))?;
        let count = results
            .GetCount()
            .map_err(|e| PickError::Shell(e.to_string()))?;
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let item = results
                .GetItemAt(index)
                .map_err(|e| PickError::Shell(e.to_string()))?;
            let name: PWSTR = item
                .GetDisplayName(SIGDN_FILESYSPATH)
                .map_err(|e| PickError::Shell(e.to_string()))?;
            // `GetDisplayName` 给回来的是 shell 自己 alloc 的一块内存，读完就得还。
            // 这一句不写，每挑一枚就漏一条路径的字节，而漏在哪一天没人会去看对话框。
            // `as_wide()` 走到下一个 NUL 为止，与 [`crate::clip`] 读 `CF_HDROP` 那份双 NUL
            // 名单用的是同一枚形状：shell 的宽字符列表在这两头长得一样。
            let text = OsString::from_wide(name.as_wide());
            CoTaskMemFree(Some(name.as_ptr() as *const c_void));
            paths.push(PathBuf::from(text));
        }
        Ok(Answer::Picked(paths))
    }
}

/// 一台不是 Windows 的机器上，这一只手照 [`crate::clip`] 的写法答「这一台没有」，而不是
/// 假装挑到了东西，也不是拿一句中文把「没有对话框」说成「用户按了取消」。
#[cfg(not(windows))]
pub fn pick(_request: &Request<'_>) -> Result<Answer, PickError> {
    Err(PickError::Unsupported)
}
