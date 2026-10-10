//! §5.5.4 step 4 的那一次点击：把刚落盘的那张图在它自己的目录里亮出来。
//!
//! 走的是 shell 的 `SHOpenFolderAndSelectItems` 而不是 `explorer.exe /select,"..."`。
//! 后者看着更短，但它把路径交回给一套 argv 解析规则：`Command` 会在带空格的那个 arg
//! 外面再包一层引号，而 `/select,` 自己又在逗号处切 —— 于是 `C:\a,b\x.png` 这种名字
//! 根本没法用一条命令行表达，`explorer.exe` 还无论成功失败都常常返回 1，让退出码变成
//! 一个谁都不能拿来判据的数。pidl 这条路把路径当**数据**交出去，一次解析都没有。

use std::path::{Path, PathBuf};

/// 一次揭示要开的东西：`folder` 是要打开的窗口，`item` 是要在里面选中的那一条。
///
/// 两个字段可以相同 —— [`plan`] 对一个目录就是这么答复的（PRD 那句"打开文件或目录"
/// 的后一半），此时没有可选中项，`selectable` 为 `false`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub folder: PathBuf,
    pub item: PathBuf,
    pub selectable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RevealError {
    /// 相对路径不是一条能交给 shell 的路：`explorer` 会按**它自己的**当前目录解析，
    /// 而不是按这张图落盘时那台进程的。宁可不开，也不开一个猜出来的目录。
    #[error("要揭示的路径不是绝对路径：{0}")]
    Relative(String),
    #[error("要揭示的东西不在盘上：{0}")]
    Missing(String),
    #[error("这一台不是 Windows，没有可以揭示的目录")]
    Unsupported,
    #[error("shell 不肯开这个目录：{0}")]
    Shell(String),
}

/// 只问盘，不动 shell：这条路径该开哪个窗口、在里面选中哪一条。
///
/// 拆开是有意的 —— 决定是可以对着文件系统读回的（门问的就是这一半），开窗口则会在
/// 用户桌面上留一个本进程关不掉的窗，所以 [`select`] 是另一次调用。
pub fn plan(path: &Path) -> Result<Target, RevealError> {
    let shown = || path.display().to_string();
    if !path.is_absolute() {
        return Err(RevealError::Relative(shown()));
    }
    let meta = std::fs::metadata(path).map_err(|_| RevealError::Missing(shown()))?;
    if meta.is_dir() {
        return Ok(Target {
            folder: path.to_path_buf(),
            item: path.to_path_buf(),
            selectable: false,
        });
    }
    let folder = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| RevealError::Missing(shown()))?;
    Ok(Target {
        folder: folder.to_path_buf(),
        item: path.to_path_buf(),
        selectable: true,
    })
}

/// 开那个窗口。`Ok(())` 只到 shell 收下这条请求为止：窗归 explorer 那个进程管，
/// 本进程既举不到它，也就没有资格说它已经在屏幕上。
#[cfg(windows)]
pub fn select(target: &Target) -> Result<(), RevealError> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFindLastID, SHOpenFolderAndSelectItems};

    // 这条线程可能早就被 Qt 起过 COM（剪贴板那一路就要），那时返回 S_FALSE；换个
    // 套间模型返回 RPC_E_CHANGED_MODE。两种都不必中止，也不在这里 CoUninitialize：
    // 一次 GUI 线程上的初始化留着比拆掉再让下一个调用者重新猜更便宜。返回值是套间
    // 状态而不是成败，所以丢掉它是有意的，不是把码吞了。
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    let wide = |p: &Path| -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };

    let folder_w = wide(&target.folder);
    let folder_pidl = unsafe { ILCreateFromPathW(PCWSTR::from_raw(folder_w.as_ptr())) };
    if folder_pidl.is_null() {
        return Err(RevealError::Shell(format!(
            "shell 认不出这个目录：{}",
            target.folder.display()
        )));
    }

    let outcome = if target.selectable {
        let item_w = wide(&target.item);
        let item_pidl = unsafe { ILCreateFromPathW(PCWSTR::from_raw(item_w.as_ptr())) };
        if item_pidl.is_null() {
            Err(RevealError::Shell(format!(
                "shell 认不出这个文件：{}",
                target.item.display()
            )))
        } else {
            // ILFindLastID 交回的是 item_pidl 尾上那一段，作为 folder 的孩子正好能用。
            let child = unsafe { ILFindLastID(item_pidl) };
            let opened =
                unsafe { SHOpenFolderAndSelectItems(folder_pidl, Some(&[child as *const _]), 0) };
            unsafe { CoTaskMemFree(Some(item_pidl as *const _)) };
            opened.map_err(|e| RevealError::Shell(e.to_string()))
        }
    } else {
        unsafe { SHOpenFolderAndSelectItems(folder_pidl, None, 0) }
            .map_err(|e| RevealError::Shell(e.to_string()))
    };

    unsafe { CoTaskMemFree(Some(folder_pidl as *const _)) };
    outcome
}

#[cfg(not(windows))]
pub fn select(_target: &Target) -> Result<(), RevealError> {
    Err(RevealError::Unsupported)
}

/// 问完就开：[`plan`] 接 [`select`]。
pub fn reveal(path: &Path) -> Result<Target, RevealError> {
    let target = plan(path)?;
    select(&target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 每支测试自己的目录：`plan` 对着盘回答，盘上有别人的文件就会答出别人的答案。
    fn sandbox(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        dir.push(format!(
            "falcon_reveal_{tag}_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    /// 空格与逗号都要试：这两枚正是 argv 那条路上会翻车的字符，而这条路上它们是
    /// 一次都没有被解析过的数据 —— 所以这里断言的是**逐字相等**，不是包含。
    #[test]
    fn a_file_answers_with_its_own_directory_and_itself() {
        let dir = sandbox("file");
        let name = std::ffi::OsStr::new("a screenshot, with 2 oddities.png");
        let file = dir.join(name);
        std::fs::write(&file, b"not empty").unwrap();

        let got = plan(&file).expect("plan");
        assert_eq!(got.folder, dir, "the window is the file's own directory");
        assert_eq!(got.item, file, "and the item is that very file");
        assert!(got.selectable, "a file is something to select");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// PRD 那句写的是"打开文件**或目录**"。对目录没有可选中项，窗口就是它自己。
    #[test]
    fn a_directory_opens_itself_and_selects_nothing() {
        let dir = sandbox("dir");
        let got = plan(dir.as_path()).expect("plan");
        assert_eq!(got.folder, got.item);
        assert!(!got.selectable, "there is nothing inside to light up");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 相对路径不当成"在我当前目录里"来猜。
    #[test]
    fn a_relative_path_is_refused_rather_than_guessed() {
        let err = plan(Path::new("some/where.png")).unwrap_err();
        assert!(
            matches!(err, RevealError::Relative(_)),
            "wrong refusal: {err}"
        );
    }

    /// 不在盘上的东西不当成"目录开了，只是没选中"来答：这两种情况在用户眼睛里是
    /// 同一个窗，判据上必须分开。
    #[test]
    fn something_not_on_disk_is_missing_not_a_directory() {
        let dir = sandbox("gone");
        let gone = dir.join("never-written.png");
        let err = plan(&gone).unwrap_err();
        assert!(matches!(err, RevealError::Missing(_)), "wrong: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
